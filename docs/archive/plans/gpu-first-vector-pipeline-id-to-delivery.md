# GPU-First Graph-to-Vector ID-to-Delivery Plan

## Objective
Rebuild the `graph -> vector` path around sustained GPU throughput, not VRAM reclamation. The target is a steady-state embedding pipeline that keeps one hot GPU session busy with minimal host/device copy overhead and explicit recovery boundaries.

## Scope Lock
- This plan is about `graph_ready -> embedding persisted`.
- The current lane-aware stock control may be preserved only if it helps sustained GPU feed.
- VRAM-to-zero cycling is not a throughput objective.
- No iterative side quests, ad hoc micro-fixes, or benchmark-driven detours outside this plan.

## Locked Decisions
- Primary optimization target: sustained `chunks/sec`, not instantaneous burst rate.
- Primary runtime model: one long-lived GPU embedding session per active embedding model.
- Recovery model: process or worker recycle only on explicit fault or unrecoverable degradation, not as normal flow control.
- First-class GPU contract: pinned or device-resident buffers, explicit stream/sync behavior, minimized implicit copies.
- Current ONNX Runtime CUDA EP path is treated as provisional, not sacred.
- TensorRT EP is an allowed primary destination, not only a fallback, if the model and packaging constraints permit it.

## Non-Goals
- Do not optimize around `nvidia-smi` alone.
- Do not keep adding batch-shaping heuristics before the GPU path is rewritten.
- Do not pursue "VRAM drops to zero" as a success metric.
- Do not keep worker-local hacks that mask the real GPU contract.

## Delivery Contract
Execution must follow these milestones in order:

1. Establish GPU truth surfaces.
2. Replace the hot path with a GPU-first embedding contract.
3. Rebuild graph-to-vector scheduling around that contract.
4. Add a hard recovery boundary.
5. Only then run qualification and throughput benchmarking.

No milestone may be skipped.

## Milestone 1: Establish GPU Truth Surfaces
### Goal
Measure the real bottleneck at the right boundary: copies, stream idle gaps, inference time, export time, persist time, and queue wait.

### Required Work
- Add per-batch timing breakdown for:
  - host tokenization / preparation
  - H2D copy
  - model inference
  - output extraction / D2H
  - persist enqueue
  - persist complete
  - inter-batch idle gap
- Add explicit batch identifiers that link:
  - prepared batch
  - GPU execution
  - persist completion
  - consumed lane/shape
- Add runtime counters for:
  - GPU batches started/completed
  - GPU idle gap histogram
  - average tokens per launched batch
  - copy volume and copy duration if observable
  - prepared-but-not-consumed age
- Use the authoritative runtime status surfaces and benchmark SQLite only where appropriate. Do not push benchmark telemetry into IST.

### Exit Criteria
- We can explain one full batch lifecycle from CPU preparation to persisted embeddings without inference gaps in the observability model.
- We can distinguish:
  - starvation before GPU
  - copy-bound GPU path
  - inference-bound GPU path
  - persist-bound completion path

## Milestone 2: Replace the Hot Path with a GPU-First Embedding Contract
### Goal
Make embedding execution device-oriented and long-lived instead of session-fragmented and copy-implicit.

### Required Work
- Refactor the embedding lane so the GPU worker owns:
  - one hot model/session
  - one explicit compute stream strategy
  - reusable input/output buffers
- Replace implicit host/device transfers on the hot path with:
  - I/O binding if supported through the chosen ORT layer
  - pinned host buffers where direct device binding is not possible
  - device tensors where feasible
- Eliminate avoidable CPU materialization between:
  - tokenized micro-batch
  - provider input
  - provider output
  - persist handoff
- Redefine micro-batching around GPU execution cost, not just token homogeneity.
- Preserve lane homogeneity only insofar as it improves steady-state throughput.
- Reassess the current `fastembed::TextEmbedding` wrapper boundary:
  - keep it only if it exposes enough control for device binding and stable throughput
  - otherwise replace it with a lower-level ORT integration path

### Architecture Preference Order
1. ORT CUDA EP with explicit device/pinned I/O control and hot session reuse.
2. ORT TensorRT EP if the model and deployment shape are compatible and it yields a cleaner throughput contract.
3. Current fastembed wrapper only if it can be made to satisfy the same hot-path guarantees.

### Exit Criteria
- The GPU worker no longer depends on repeated implicit allocations or implicit copies for nominal operation.
- The session/model lifecycle is clearly separated from batch lifecycle.
- The hot path has one sustained execution contract that is independent of VRAM reclamation heuristics.

## Milestone 3: Rebuild Graph-to-Vector Scheduling Around the GPU Contract
### Goal
Turn the pipeline into a feed system for a hot GPU, not a stock-management system that happens to own a GPU.

### Required Work
- Convert vector scheduling from chunk-stock-first to GPU-consumption-first:
  - ready stock remains a safety buffer
  - GPU batch cadence becomes the control truth
- Keep one canonical operator knob for stock if needed, but do not let it dominate the scheduler.
- Rework prepare workers so they prepare work for the next GPU launch window, not just fill generic queues.
- Replace any refill pacing that sleeps conservatively under demand with a scheduler driven by:
  - GPU idle gap
  - ready age
  - next eligible batch tokens
  - persist backpressure
- Reassess lane logic:
  - keep `small/medium/large` only if they improve observed cadence
  - mixed fallback remains explicit
  - do not let lane purity create GPU dead air
- Ensure the selected batch policy optimizes for:
  - low inter-batch gap
  - stable launch cadence
  - acceptable per-batch token density
  - bounded variance

### Exit Criteria
- The scheduler is demonstrably centered on feeding the next GPU launch.
- Queue growth without GPU consumption becomes detectable as a control failure, not just a metric.

## Milestone 4: Add a Hard Recovery Boundary
### Goal
Stop trying to use nominal flow control to solve unrecoverable GPU runtime pathologies.

### Required Work
- Separate nominal throughput control from fault recovery.
- Define explicit recovery triggers:
  - provider failure
  - reproducible OOM
  - stuck batch / no completions beyond threshold
  - invalid session state
- Recovery must recycle at a hard enough boundary to actually clear provider state:
  - worker process if sufficient
  - full service process if required by provider behavior
- Recovery must preserve:
  - ready stock when safe
  - claim correctness
  - persist correctness
  - exact failure reason
- Remove VRAM-summit heuristics from nominal throughput logic unless they remain purely diagnostic.

### Exit Criteria
- Recovery is a separate mode with explicit triggers.
- Normal operation does not depend on speculative session restarts.

## Milestone 5: Qualification and Benchmarking
### Goal
Validate the rebuilt pipeline only after the architecture is complete.

### Required Work
- Run cold and warm benchmarks with fixed scenarios only after milestones 1-4 are complete.
- Compare against the current lane-aware baseline on:
  - sustained `chunks/sec`
  - inter-batch idle gap
  - average batch latency
  - ready stock age
  - GPU-side failure rate
  - persist lag
- Use Nsight or equivalent high-fidelity tooling if the ORT/runtime counters still leave ambiguity around copies vs kernels vs sync.

### Exit Criteria
- Throughput is judged on full-window sustained rate, not burst peaks.
- A benchmark run can be explained from counters without relying on guesswork from VRAM graphs alone.

## Implementation Order
1. Instrument the existing path until the batch lifecycle is fully visible.
2. Introduce the new GPU worker contract behind a feature flag or isolated runtime path.
3. Migrate one embedding lane to the new contract.
4. Migrate the scheduler/control logic to target GPU cadence.
5. Remove obsolete VRAM-recycle hot-path logic.
6. Benchmark.
7. Promote the new path as canonical.

## Design Constraints
- One source of truth for runtime health: MCP/runtime status.
- One source of truth for benchmark telemetry: benchmark SQLite when benchmark mode is active.
- No benchmark-only logic in IST writer paths.
- No silent fallback from the new GPU-first path to the old path without telemetry indicating it.

## Risk Register
- `fastembed` may not expose enough control for a real GPU-first hot path.
- ORT CUDA EP may remain memory-opaque even after better I/O control.
- TensorRT EP may require packaging/runtime concessions.
- Persist path may emerge as the next bottleneck once GPU cadence is fixed.
- Lane-aware scheduling may need simplification, not further sophistication.

## Decision Rule
If Milestone 2 proves that `fastembed` prevents real device-oriented control, the plan must switch to lower-level ORT integration immediately. Do not spend further cycles trying to coerce the wrapper.

If Milestone 2 proves that ORT CUDA EP still cannot deliver acceptable steady-state behavior with explicit device/pinned I/O and a hot session, the plan must switch to TensorRT EP evaluation immediately.

## Definition of Done
- The canonical graph-to-vector path is GPU-first.
- The hot path is built around sustained GPU execution, not memory reclamation.
- Recovery is explicit and separated from nominal scheduling.
- Full-window throughput materially exceeds the current baseline.
- The resulting runtime can be explained by instrumentation rather than inference.
