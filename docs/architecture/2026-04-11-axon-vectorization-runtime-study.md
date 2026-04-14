# Axon GPU Vectorization Runtime Study

Date: 2026-04-11

## Summary

This study was conducted to stop tuning Axon's vectorization path blindly.
The current problem is not simply "GPU underutilization" or "batchs too small".
Axon currently behaves as a shared interactive server with a background semantic pipeline whose real bottlenecks are a mix of:

- sequential stage coupling
- incomplete per-stage observability
- queue/control-plane blind spots
- CPU-heavy pre/post-processing around embedding
- imperfect alignment between live queue state and adaptive control telemetry

The result is a runtime that can progress materially while still producing misleading local snapshots such as:

- `GPU = 0%` during CPU-only/finalization windows
- `CPU` remaining high while the model is not actively embedding
- controller state appearing stagnant even while vector work continues

## What Was Verified Locally

### Runtime facts

Live runtime facts verified during this study:

- `provider_effective = cuda`
- `acceleration_state = gpu_active`
- `vector_workers = 2`
- `graph_workers = 0`
- Axon can run headless in:
  - `--full --no-dashboard --skip-mcp-tests --skip-elixir-prewarm`

Observed live behavior:

- GPU commonly oscillates between `0%` and `~30%`
- CPU remains elevated during non-embedding phases
- queue drain continues even when GPU is temporarily idle

Recent live queue state observed directly over SQL:

- `FileVectorizationQueue`
  - `inflight = 394`
  - `queued = 4598`

This is materially larger than the simple controller-facing view and confirms that queue admission / inflight accounting is one of the places where Axon still lacks a clean control picture.

### Current vector pipeline shape

From the current code in `embedder.rs` and `graph_ingestion.rs`, the vector lane is still effectively:

1. claim file vectorization work
2. fetch unembedded chunks
3. assemble text batch
4. call `embed()`
5. persist embeddings
6. mark files ready
7. clear queue work

Even after recent improvements, this is still a phase-coupled loop, not a fully decoupled continuous pipeline.

### Current observability

Axon already has useful cumulative metrics:

- `fetch_ms_total`
- `embed_ms_total`
- `db_write_ms_total`
- `mark_done_ms_total`
- `chunks_embedded_total`
- `files_completed_total`
- `embed_calls_total`
- `files_touched_total`

And now also a first controller surface:

- `controller_state`
- `controller_reason`
- `target_embed_batch_chunks`
- `target_files_per_cycle`

But this is still insufficient for correct balancing because we do not yet have:

- queue depth per stage inside a multi-stage pipeline
- queue wait time per stage
- batch payload size in bytes/tokens/text-length
- per-stage memory footprint
- per-stage CPU cost
- per-stage percentiles, not only totals
- GPU idle due to lack of prepared work
- persist-stage backpressure visibility

## Structural Findings

### 1. The real bottleneck is not "just batching"

The system improved after:

- multi-file batching
- grouped finalization
- `ChunkEmbedding` PK + `INSERT OR REPLACE`
- `NOT EXISTS` fetch path

But those changes did not fully stabilize GPU feeding.
This means the dominant problem is now the shape of the runtime pipeline itself, not only micro-optimizations.

### 2. Axon still alternates between phases instead of streaming continuously

Current runtime behavior still looks like:

- fetch burst
- embed burst
- write/finalize burst

Instead of:

- continuous prepare
- continuous embed
- continuous persist

This explains:

- transient `GPU = 0%`
- CPU-heavy orchestration windows
- imperfect overlap between compute and persistence

### 3. The adaptive controller is not yet the main lever

Phase 1 adaptive batching was implemented correctly as a bounded controller, but live evidence shows:

- it has not yet made a meaningful adjustment
- in some real windows, the lane is already hitting `64 chunks/embed`
- `avg_files_per_embed_call` has improved materially even before controller action

Conclusion:

- the controller is not useless
- but it is not the next primary throughput lever
- the next primary lever is pipeline decoupling plus deeper per-stage observability

### 4. Queue accounting and controller accounting are not yet aligned

The queue can show hundreds of inflight items while controller windows still look small or reset-like.
This means Axon still has a control-plane observability mismatch.

Likely causes:

- top-up claims inside the vector loop
- global controller state observed by multiple workers
- aggregate metrics used as controller input
- controller windows representing only matured slices rather than the full queue state

This does not make the runtime wrong, but it makes balancing decisions less trustworthy.

## External References Most Relevant To Axon

### 1. EmbedAnything

Repo:
- https://github.com/StarlightSearch/EmbedAnything

Why it matters:

- closest public Rust analogue to Axon's `prepare -> embed -> persist` problem
- explicitly separates preprocessing, inference, and indexing
- uses streaming and configurable buffering

Transferable lesson:

- Axon should move to a stage-separated semantic pipeline with bounded queues between stages

### 2. Vector

Repo:
- https://github.com/vectordotdev/vector

Docs:
- https://vector.dev/docs/architecture/buffering-model/
- https://vector.dev/docs/introduction/concepts/

Why it matters:

- best Rust reference for backpressure, bounded buffers, and sink-aware flow control

Transferable lesson:

- Axon should propagate backpressure upstream through bounded queues instead of relying mainly on global pressure heuristics

### 3. Hugging Face text-embeddings-inference

Repo:
- https://github.com/huggingface/text-embeddings-inference

Why it matters:

- Rust server optimized for interactive embedding workloads
- explicit controls around concurrency, request batching, and batch budgets

Transferable lesson:

- Axon should treat interactive service latency as a first-class batching constraint, not optimize purely for background throughput

### 4. Hugging Face xet-core

Repo:
- https://github.com/huggingface/xet-core

Why it matters:

- documents real runtime debugging with `tokio-console`

Transferable lesson:

- Axon should adopt runtime task-level introspection during tuning, not rely only on ad hoc counters

### 5. TiKV + pprof-rs

References:
- https://github.com/tikv/pprof-rs
- https://tikv.org/blog/quickly-find-rust-program-bottlenecks-online-using-a-go-tool/

Why it matters:

- production-grade example of on-demand CPU bottleneck profiling in Rust systems

Transferable lesson:

- Axon should add production-safe CPU profiling hooks for hotspot confirmation before major balancing changes

## Recommended Tooling Stack For The Next Study Wave

Not one library, but a layered stack:

- `tracing`
  - already present
  - keep as the structured span backbone
- `metrics`
  - add counters/gauges/histograms cleanly
- `hdrhistogram`
  - track p50/p95/p99 per stage
- `tokio-metrics`
  - runtime and scheduler health
- `tokio-console`
  - live async/runtime debugging
- `pprof-rs`
  - CPU hotspot proof
- optional later:
  - `OpenTelemetry`

## Conclusions

### What Axon should not do next

Axon should not:

- continue tuning only `chunk_batch_size`
- make worker counts dynamic before pipeline truth is known
- chase GPU percentage as an isolated KPI
- assume the controller is the main missing piece
- optimize background throughput without a stricter MCP service contract

### What Axon should do next

Axon should move toward a fully instrumented staged pipeline:

1. `prepare`
   - claim work
   - fetch chunks
   - tokenize / prepare payloads
   - fill bounded prepared-work queue

2. `embed`
   - consume prepared queue
   - launch continuous GPU inference
   - emit embedded results into bounded result queue

3. `persist`
   - write embeddings
   - finalize files
   - update queue/work state

And before balancing this pipeline, Axon must add:

- per-stage queue depth
- per-stage queue wait
- per-stage CPU time
- per-stage histograms
- per-stage memory pressure
- GPU idle due to empty input queue
- persist backpressure metrics

### Core design rule

The MCP client remains king.

Therefore the correct final architecture is not:

- "maximum background throughput"

It is:

- bounded, observable, backpressured background work
- plus a protected interactive fast path

Only after that instrumentation exists should Axon attempt a "perfect loop" controller.

## Final Assessment

Axon is no longer being tuned blindly.

We now know:

- what the runtime is actually doing
- where current observability is still insufficient
- which open-source Rust systems validate the intended direction
- which architectural leap is truly next

The next serious engineering pass should be a staged pipeline study/implementation, not another isolated batch-size pass.

## Addendum: Live Runtime Evidence After Stage-Latency Instrumentation

After adding recent-window stage latency summaries to `axon_debug`, we captured live CUDA-backed drain snapshots on the real runtime.

### Snapshot A

- `provider_effective = cuda`
- `vector_workers = 2`
- `graph_workers = 0`
- `file_vectorization_queue_statuses`
  - `queued = 4456`
  - `inflight = 88`
- `chunks_embedded_total = 896`
- `files_completed_total = 408`
- `avg_chunks_per_embed_call = 64.0`
- `avg_files_per_embed_call = 3.0`

Recent stage latencies:

- `fetch p50/p95/max = 14 / 126 / 381 ms` over `256` samples
- `embed p50/p95/max = 12291 / 12657 / 13101 ms` over `14` samples
- `db_write p50/p95/max = 435 / 572 / 649 ms` over `14` samples
- `mark_done p50/p95/max = 90 / 178 / 439 ms` over `17` samples

Correlated machine signals:

- `axon-core CPU ~= 99% of one full process listing`, effectively saturating roughly one full process allocation with high multi-core usage in other prior snapshots
- `GPU util ~= 35%`

### Snapshot B

- `provider_effective = cuda`
- `file_vectorization_queue_statuses`
  - `queued = 4446`
  - `inflight = 98`
- `chunks_embedded_total = 1152`
- `avg_chunks_per_embed_call = 64.0`
- `avg_files_per_embed_call = 3.11`

Recent stage latencies:

- `fetch p50/p95/max = 14 / 127 / 381 ms`
- `embed p50/p95/max = 10438 / 12657 / 13101 ms`
- `db_write p50/p95/max = 469 / 649 / 669 ms`
- `mark_done p50/p95/max = 90 / 178 / 439 ms`

Correlated machine signals:

- `GPU util ~= 25%`
- `axon-core` still reported `99%` CPU in process listing at sample time

### What these snapshots prove

They sharpen the previous diagnosis:

- Axon is no longer primarily blocked by `db_write`
- Axon is not primarily blocked by `mark_done`
- `fetch` is noticeable, but still much smaller than `embed`
- the dominant wall-clock stage is now clearly the `embed()` call itself

But this does **not** mean "the GPU is the bottleneck" in the classical sense.

The evidence instead shows:

- GPU is active
- GPU is not saturated
- per-call `embed()` wall time is large
- batch density is stable but not high enough to push GPU utilization near saturation
- the system is still operating as a coarse phase loop, not a continuously fed pipeline

### Updated interpretation

The key bottleneck is now:

- the end-to-end duration of each `embed()` cycle
- combined with insufficient continuity of GPU feeding

In other words:

- `embed()` is the dominant stage
- but the runtime still behaves too much like
  - `prepare some work`
  - `run one large embed`
  - `persist`
  - `repeat`
- rather than a true `prepare -> embed -> persist` conveyor

### Immediate implications for the next phase

Before attempting any deeper adaptive balancing, Axon now has enough evidence to justify:

1. adding explicit per-stage queue metrics
   - prepared queue depth
   - embedded-result queue depth
   - queue wait times
2. capturing stage-specific CPU and memory signals
3. introducing a decoupled staged vector pipeline
4. keeping MCP protection explicit at the admission boundary

This addendum upgrades the prior conclusion from "likely needed" to "runtime-evidenced":

- the next serious step is a decoupled staged pipeline study/implementation
- not another batch-size-only iteration
