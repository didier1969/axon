# GPU Push Backpressure Implementation Plan

> **Superseded:** This plan has been superseded by the big-bang migration defined in [2026-04-24-vector-gpu-big-bang-push-backpressure-design.md](./2026-04-24-vector-gpu-big-bang-push-backpressure-design.md) and [2026-04-24-vector-gpu-big-bang-push-backpressure-implementation-plan.md](./2026-04-24-vector-gpu-big-bang-push-backpressure-implementation-plan.md). Keep this document only as historical context for the earlier incremental push/backpressure attempt.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace fragile refill-by-priority behavior with a push pipeline that continuously feeds the GPU until a bounded ready queue is full, then applies explicit backpressure upstream.

**Architecture:** The graph/vector side keeps pushing work into prepare workers, and prepare workers keep filling a bounded GPU-ready queue. The only regulator is queue pressure: when the ready queue reaches a high watermark, upstream dispatch pauses; when it drops below a low watermark, upstream push resumes. This removes the current dependency on `graph_priority` for GPU refill.

**Tech Stack:** Rust, crossbeam channels, runtime controller logic in `axon-core`, existing benchmark scripts and SQLite telemetry mirror.

---

### Task 1: Freeze the desired push/backpressure behavior with tests

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Write the failing tests**

Add tests for these behaviors:
- when `ready_batches` is below low watermark, upstream push is allowed even with graph backlog
- when `ready_batches` reaches high watermark, upstream push pauses
- backpressure depends on queue occupancy, not on `graph_priority`

Add helper-oriented tests around a new push/backpressure decision function, for example:

```rust
#[test]
fn test_gpu_ready_queue_push_allowed_below_low_watermark() {
    assert!(gpu_ready_queue_push_allowed(4, 2, 16, 32));
}

#[test]
fn test_gpu_ready_queue_push_blocked_at_high_watermark() {
    assert!(!gpu_ready_queue_push_allowed(32, 0, 16, 32));
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_gpu_ready_queue_push_ -- --test-threads=1
```

Expected: FAIL because the helper does not exist yet.

### Task 2: Introduce explicit GPU-ready queue watermarks

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Add watermark configuration**

Add config helpers:
- `configured_gpu_ready_low_watermark()`
- `configured_gpu_ready_high_watermark()`

Rules:
- defaults should start conservative, for example `low=8`, `high=24`
- `high >= low + 1`
- both must stay `<= configured_vector_ready_queue_depth()`

Use env vars such as:
- `AXON_GPU_READY_LOW_WATERMARK`
- `AXON_GPU_READY_HIGH_WATERMARK`

**Step 2: Add a push/backpressure helper**

Add a small helper that decides whether upstream push should continue:

```rust
fn gpu_ready_queue_push_allowed(
    ready_depth: usize,
    inflight_prepares: usize,
    low_watermark: usize,
    high_watermark: usize,
) -> bool
```

Rules:
- if `ready_depth >= high_watermark`, return `false`
- if `ready_depth + inflight_prepares < low_watermark`, return `true`
- otherwise allow a steady-state push policy without overfilling

**Step 3: Run tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_gpu_ready_queue_push_ -- --test-threads=1
```

Expected: PASS.

### Task 3: Replace priority-driven refill in the vector worker loop

**Files:**
- Modify: `src/axon-core/src/embedder.rs:1600-1865`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Remove queue refill dependence on graph-priority sleeps**

Keep the existing VRAM guard.

Change the active vector loop so that:
- upstream top-up and prepare dispatch are driven by `gpu_ready_queue_push_allowed(...)`
- the loop keeps pushing while the ready queue is below the high watermark
- if the ready queue is full, upstream push pauses naturally
- if the ready queue is low, top-up is aggressive regardless of graph backlog

Important:
- do not increase batch token size here
- do not relax VRAM limits
- do not change graph worker count

**Step 2: Make the prepare queue cap match the push model**

The current `DEFAULT_VECTOR_PREPARE_QUEUE_BOUND` is too small for this model.

Change it so the prepare queue can actually support the bounded push design:
- derive the bound from pipeline depth / ready watermark
- or add explicit env-configured bound with a larger sane default

The key requirement:
- the runtime must be able to accumulate enough prepare requests before GPU starvation hits

**Step 3: Run targeted tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_gpu_ready_queue_push_ test_single_worker_gpu_prepare_prefetch_limits_ test_request_prepared_vector_embed_sequence_ -- --test-threads=1
```

Expected: PASS.

### Task 4: De-emphasize graph-priority for GPU feeding

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Test: `src/axon-core/src/embedder.rs`

**Step 1: Reduce graph-priority influence on GPU feeding**

Do not let `graph_backlog_present` suppress vector push when the GPU-ready queue is below its low watermark.

Minimal acceptable approaches:
- keep `graph_priority` for other scheduling decisions, but not for GPU refill
- or route GPU feeding around that scheduler branch entirely

**Step 2: Add tests**

Add tests proving:
- graph backlog does not block GPU push when ready queue is low
- graph backlog still can matter when ready queue is healthy/full

**Step 3: Run tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_utility_first_scheduler_ test_gpu_ready_queue_push_ -- --test-threads=1
```

Expected: PASS.

### Task 5: Keep VRAM-safe constraints intact

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Use: `scripts/benchmark-vector-token-matrix.sh`

**Step 1: Preserve hard runtime safety**

Do not remove:
- VRAM budget `7000`
- admission threshold `6300`
- batch-size safety envelope around `16000`

The push pipeline is allowed to fill the queue, but it must still stop when:
- VRAM guard blocks admission
- queue high watermark is reached

**Step 2: Verify no regression in guards**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_gpu_memory_pressure_active_uses_soft_limit -- --test-threads=1
```

Expected: PASS.

### Task 6: Run a VRAM-safe qualification benchmark

**Files:**
- Use: `scripts/benchmark-vector-token-matrix.sh`
- Read: `.axon/benchmarks/.../results.tsv`
- Read: `.axon-dev/run/benchmark.sqlite3`

**Step 1: Run the benchmark**

```bash
env AXON_INSTANCE_KIND=dev bash scripts/benchmark-vector-token-matrix.sh \
  --mode warm \
  --duration 120 \
  --interval 1 \
  --tokens 16000 \
  --ready-depth 96 \
  --pipeline-depth 12 \
  --prepare-workers 8 \
  --max-items 128 \
  --max-batch-bytes 8388608 \
  --graph-workers 2 \
  --max-vram-used-mb 7000 \
  --gpu-admission-vram-used-mb 6300 \
  --label-prefix oven-push-backpressure-v1
```

**Step 2: Validate acceptance criteria**

Reject the run if:
- `vram_budget_exceeded != 0`
- `max_gpu_used_mb >= 7000`
- `window_chunks_per_second` is worse than the current valid baseline
- `ready_queue_depth_at_gpu_start` still sits near `2..5`
- `batch_wait_for_ready_ms` does not materially improve

Query batch-level data scoped to the run window:

```bash
python3 - <<'PY'
import sqlite3
run_started_ms = ...
run_finished_ms = ...
con = sqlite3.connect('.axon-dev/run/benchmark.sqlite3')
for row in con.execute(\"\"\"
select chunk_count, total_tokens, gpu_used_mb,
       ready_queue_depth_at_gpu_start,
       prepare_inflight_at_gpu_start,
       batch_wait_for_ready_ms,
       (gpu_finished_at_ms - gpu_started_at_ms) as gpu_window_ms
from vector_batch_run
where started_at_ms >= ? and started_at_ms <= ?
order by started_at_ms asc
\"\"\", (run_started_ms, run_finished_ms)):
    print(row)
PY
```

### Task 7: Update the master refactor plan

**Files:**
- Modify: `docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md`

**Step 1: Add the new direction**

Document:
- previous diagnosis: push works, pull refill fails
- new direction: bounded push with explicit backpressure
- benchmark success criteria under VRAM-safe envelope

**Step 2: Diff hygiene**

```bash
git diff --check -- docs/plans/2026-04-24-gpu-push-backpressure-plan.md docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md
```

Expected: no diff-check errors.
