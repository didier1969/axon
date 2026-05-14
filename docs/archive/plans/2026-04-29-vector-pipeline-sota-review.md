# Vector Pipeline SOTA Review

Date: 2026-04-29
Project: AXO
Scope: graph -> vector -> TensorRT/CUDA/CPU pipeline

## ID To Delivery Verdict

The current pipeline is necessary in intent and materially cleaner than before, but it is not yet state of the art.

It is best described as:

- robust graph/vector production pipeline
- TensorRT-ready partial implementation
- not yet TensorRT-proven
- not yet fully GPU-first

Two independent reviews converged on the same conclusion: keep the graph/vector lane architecture and multi-provider capability, but do not promote TensorRT as the central path until the fallback policy, provider authority, telemetry, and qualification harness are stronger.

User correction locked after review: CUDA has already failed to provide meaningful performance value for the project goals. The next priority is therefore not to optimize CUDA or keep it as a peer strategy. CUDA remains a baseline and emergency fallback only. The active delivery path is to install, qualify, stress, and push TensorRT to its limits.

Additional operational constraint locked after review: Axon has already observed VRAM overflow/explosion severe enough to nearly paralyze vectorization. TensorRT qualification is therefore not only a throughput exercise. It must prove bounded GPU memory behavior under realistic and adversarial vectorization workloads, or it is not promotable.

## Macro Analysis

### What Is Worth Keeping

The two production lanes are the right abstraction:

- graph lane: persisted file work becomes graph-ready structural truth
- vector lane: graph-ready work becomes vector-ready semantic acceleration

The following capabilities still have product value:

- `FileVectorizationQueue` ownership and leases
- prepare, ready, embed, persist, finalize separation
- outbox-style persist/finalize durability
- GPU subprocess boundary for TensorRT/CUDA provider isolation
- CPU fallback for dev, CI, non-GPU platforms, and controlled degraded operation
- CUDA fallback behind TensorRT during qualification, not as a performance destination
- runtime status and dashboard telemetry for operator visibility

These are not accidental complexity. They protect correctness, portability, crash isolation, and MCP availability.

### What Is Not Yet Good Enough

The pipeline is still not pure enough:

- `embedder.rs` remains the gravity center for orchestration, provider selection, fallback, batching, persist, finalize, restart logic, and many tests.
- Provider truth is still partly global and environment-backed.
- TensorRT is still mostly a GPU service/provider flag, not a fully qualified runtime contract.
- Fallback behavior is observable but not policy-driven enough.
- Graph embeddings blur the conceptual boundary between graph structural production and vector support.

The current architecture is acceptable as a transition layer, not as the final high-performance TensorRT product shape.

### Fallback Decision

Fallbacks are still needed, but their role must be narrowed.

Keep:

- CPU fallback for portability and degraded operation.
- CUDA fallback during TensorRT qualification and packaging instability.
- subprocess recycle on provider failure.

Change:

- TensorRT is the next optimization target.
- CUDA must be treated as previously disappointing evidence, not as a path to optimize.
- production TensorRT mode should support `fail_closed` when GPU/TensorRT is explicitly required.
- CPU fallback must be explicitly labeled degraded, never treated as equivalent to GPU success.
- fallback policy should be lane-scoped and immutable for a worker lifetime.

Remove or demote:

- VRAM-summit recycle as nominal flow control.
- hidden env-global provider truth as the authority for query, graph, and vector at once.

## Micro Analysis

### Provider Authority

Current state:

- `ProviderResolution` exists and names `Cpu`, `Cuda`, `TensorRt`, `Unavailable`.
- The runtime still publishes effective provider state through global env-backed state.
- Query, graph support, vector in-process, and vector GPU service can still blur the effective provider label.

Required next state:

- provider state must be scoped to `VectorLane`, `GraphLane`, `QuerySupport`, and `VectorGpuService`.
- fallback origin must be explicit on every vector execution result.
- TensorRT, CUDA, and CPU must report the same execution contract shape.

### TensorRT

Current state:

- TensorRT is enabled by `AXON_GPU_EMBED_SERVICE_TENSORRT`.
- The GPU service provider list becomes TensorRT then CUDA.
- cache directories are configured.
- runtime telemetry exposes a TensorRT-ready contract, but several fields are still placeholders or aliases.

Missing:

- real engine cache hit/miss
- cold build latency
- warm inference latency
- provider init error class
- recycle count
- fixed TensorRT qualification workload, with CUDA retained only as a historical/baseline comparator

Conclusion: TensorRT is the priority path to qualify and stress. CUDA is not a competing destination anymore.

### VRAM Control

Current risk:

- prior GPU vectorization attempts produced VRAM overflow/explosion and near-paralysis of vectorization.
- TensorRT can reduce runtime inefficiency, but it can also allocate substantial memory during engine build and runtime context creation if left unbounded.
- a faster provider is not useful if engine construction, dynamic shapes, or batch pressure can push the process into OOM/recycle loops.

Required next state:

- TensorRT workspace/memory pool limits are explicit and versioned with the runtime profile.
- TensorRT dynamic shape profiles define bounded min/opt/max batch and sequence/token shapes.
- engine cache and timing cache are mandatory for qualification, with cold-build and warm-cache paths measured separately.
- VRAM high-water mark, allocation failures, engine rebuilds, recycle causes, and recovery latency are first-class telemetry.
- OOM must trigger controlled degradation or fail-closed behavior according to policy, never silent provider drift.
- the ORT TensorRT artifact build defaults to an Axon embedding profile: TensorRT and CUDA providers stay enabled, but generative attention kernels and multi-GPU collectives that do not serve Axon vectorization are disabled to reduce build time and attack surface.

Qualification implication:

- TensorRT is only successful if it improves control over GPU memory pressure as well as throughput.
- CUDA remains useful only as a baseline and emergency fallback, because prior evidence showed weak performance value.

### Batching And Scheduling

Current state:

- token lanes `small`, `medium`, `large`, `mixed` exist.
- mixed fallback prevents GPU starvation when homogeneous batches would be too small.
- ready queue and refill state preserve continuity.

Risk:

- the scheduler is still stock/backpressure-first.
- SOTA target should be GPU-cadence-first: idle gap, next eligible batch, token density, persist pressure.
- token lanes may help, but they are not proven. They should survive only if benchmarks show reduced idle gap or better sustained chunks/sec.

### Persist And Finalize

Current state:

- persist and finalize are separated and durable.
- vector workers still may wait on persist outcomes in ways that reduce GPU/DB overlap.

Required next state:

- GPU hot loop should be `pop ready -> execute -> enqueue persist`.
- persist should be downstream and non-blocking until `max_inflight_persists` is reached.
- finalize should remain off the hot GPU path unless a hard correctness invariant requires waiting.

### Recovery

Current state:

- subprocess recycle exists and is valuable.
- stuck recovery and VRAM recycle controls exist.

Required next state:

- recovery must be separate from nominal scheduling.
- valid recovery triggers: provider failure, reproducible OOM, stuck batch, no completions beyond threshold, invalid session state.
- VRAM-summit logic should become diagnostic or recovery-only, not a throughput control primitive.

## Delivery Plan

### Phase 1: Freeze Policy And Authority

1. Add explicit fallback policy:
   - `degrade_cpu`
   - `cuda_then_cpu`
   - `tensorrt_then_cuda`
   - `gpu_required_fail_closed`
2. Scope provider truth by role/lane.
3. Stop using one global provider effective label as runtime authority.
4. Keep compatibility labels for MCP/dashboard until consumers are migrated.

### Phase 2: Prove Or Simplify Batching

1. Add benchmark counters for GPU idle gap, token density, ready age, and consumed batch lane.
2. Compare homogeneous token lanes vs simpler FIFO/mixed policy.
3. Keep small/medium/large only if they improve sustained chunks/sec or reduce idle gap.
4. Otherwise collapse to a simpler `ready batch` policy with explicit token budget.

### Phase 3: Make Persist Truly Downstream

1. Reduce vector worker hot loop to execute and enqueue.
2. Enforce `max_inflight_persists` as the only normal persist backpressure boundary.
3. Keep finalize asynchronous and off the hot path.
4. Preserve lease correctness and exact failure reasons.

### Phase 4: TensorRT Qualification And Stress

1. Run fixed cold and warm TensorRT workloads.
2. Capture:
   - sustained chunks/sec
   - p50/p95 batch latency
   - GPU idle gap
   - cache build/hit
   - VRAM
   - provider errors
   - persist lag
   - MCP quality before/after
3. Capture memory-specific evidence:
   - VRAM high-water mark
   - TensorRT workspace/memory pool limit used
   - engine build peak memory
   - runtime context peak memory
   - OOM count and error class
   - provider recycle count and recovery latency
   - engine rebuild count caused by shape/profile misses
4. Stress TensorRT across batch sizes, token distributions, cache states, recycle conditions, OOM boundaries, and long-run stability windows.
5. Use CUDA only as a baseline/fallback comparator because prior evidence already showed weak performance.
6. Promote TensorRT only if it gives sustained throughput, better control, bounded VRAM behavior, or a clearly superior operational envelope.

### Phase 5: Remove Transitional Complexity

1. Demote VRAM-summit recycle from nominal path.
2. Remove provider-global env authority.
3. Move remaining vector orchestration out of `embedder.rs`.
4. Keep graph structural lane separate from graph embedding support.

## Decision Rules

- If TensorRT cannot materially improve sustained chunks/sec, control stability, or operational observability, do not promote it.
- If TensorRT cannot bound VRAM pressure and recover predictably from OOM-adjacent states, do not promote it.
- If token lanes do not reduce idle gap or improve sustained throughput, simplify them.
- If CPU fallback happens in a production GPU-required profile, fail closed and report exact recovery guidance.
- If a feature exists only for historical debugging and is not used by qualification, remove or demote it.

## External Technical Findings

- ONNX Runtime TensorRT EP exposes provider options for `trt_max_workspace_size`, FP16/INT8, engine cache, timing cache, context memory sharing, CUDA graph, and dynamic shape profiles. These are the correct control points for Axon qualification rather than raw CUDA tuning.
- NVIDIA TensorRT documentation states that build-time temporary memory is controlled through builder memory pool limits and that TensorRT reports memory usage around critical builder/runtime operations. Axon should capture equivalent evidence in its own telemetry and qualification logs.
- `trtexec` remains the right external probe for isolating TensorRT engine build/runtime behavior before blaming Axon orchestration. It supports bounded shape ranges, memory pool sizing, engine serialization, timing cache generation, and near-gapless throughput measurement.
- Local build evidence showed the full ORT CUDA+TensorRT artifact compiling FlashAttention sources with `USE_FLASH_ATTENTION=1`, `USE_MEMORY_EFFICIENT_ATTENTION=1`, and `USE_NCCL=1`. These are not central to Axon embedding qualification and should be disabled in the default `axon_embedding` artifact profile.

## Immediate Recommendation

Do not remove CPU/CUDA/TensorRT fallback support wholesale.

Instead, make fallback policy explicit, lane-scoped, and measurable. The most valuable next work is not more feature code; it is a qualification-oriented cleanup:

1. provider authority cleanup
2. fallback policy contract
3. real TensorRT telemetry
4. VRAM-bound TensorRT qualification and stress harness
5. GPU-cadence benchmark centered on TensorRT
6. OOM/recovery drills before promotion
7. simplification based on measured evidence
