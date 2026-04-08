# Embedding Throughput 300k Design

## Goal

Drive Axon toward a measured target of `300_000 embeddings/hour` on the target `8 GiB VRAM` GPU for:
- `file`
- `type`
- `procedure`

This design does **not** assume the target is already achievable. Its first purpose is to convert the current opaque under-performance into a measured, explainable system.

## Strategic Truth

Current measured reality on this branch:
- real benchmark harness exists
- CUDA activity has been externally confirmed with `nvidia-smi`
- target throughput is not reached and the gap is massive
- the runtime still lacks strong internal proof of the effective provider
- the embedding pipeline remains structurally underfed and partially serialized

Therefore the mission is not “tune a few knobs”.
It is:
1. isolate the real bottleneck layers
2. saturate the GPU intentionally
3. decouple inference from orchestration and persistence
4. rerun the full proof loop

## Architecture Principle

Axon must stop treating embedding throughput as a single black-box metric.

The throughput chain must be split into three measurable layers:

1. `model-only`
   - raw `embed()` throughput
   - excludes DB and queue orchestration

2. `prepare+embed`
   - corpus shaping
   - tokenization / host-side preparation
   - inference
   - excludes persistence

3. `full pipeline`
   - fetch
   - prepare
   - embed
   - persist
   - queue state transitions

Without this split, every optimization remains ambiguous.

## Dominant Bottleneck Hypotheses

Based on current code and measured runs, the dominant hypotheses are:

1. GPU underfeeding
   - batch sizes remain too small
   - the worker is still conservative even after recent calibration fixes

2. Host-bound inference path
   - tokenization / payload prep likely dominate a large fraction of end-to-end time
   - the GPU is active but not saturated usefully

3. Serialized orchestration
   - one semantic worker owns too much of the pipeline
   - inference, queue handling, and DB work remain too coupled

4. Incomplete runtime truth
   - `gpu_present` is a local device-file heuristic, not a trustworthy provider truth
   - `provider_effective` is still not strongly exposed

## Target Architecture

The target throughput architecture is:

`AST / structural indexing -> embedding-ready queue -> batch builder -> GPU inference lane -> async persistence lane`

Required properties:
- AST/indexing must remain CPU-priority and must not be blocked by GPU work
- embedding units must be formed into large homogeneous micro-batches
- inference must not wait on DB persistence
- persistence must consume completed embedding batches asynchronously
- runtime must expose:
  - requested backend
  - detected local GPU accessibility
  - effective provider if known
  - current batch size
  - current throughput
  - current VRAM footprint

## Design Decision: Sequence of Work

The work will proceed in three major tranches.

### Tranche A. Measurement Truth

Purpose:
- make bottlenecks separable and undeniable

Scope:
- add `model-only` benchmark mode
- add `prepare+embed` benchmark mode
- retain existing `full pipeline` benchmark
- improve runtime truth for backend selection and provider observability

Exit proof:
- one comparable report per profile/backend/layer

### Tranche B. GPU Saturation

Purpose:
- find the best stable operating zone on the target GPU

Scope:
- make batch sizes explicitly tunable
- run a calibration sweep across representative ranges
- record throughput / latency / VRAM / utilization

Exit proof:
- stable best-known batch configuration with measured gain

### Tranche C. Pipeline Decoupling

Purpose:
- remove orchestration and persistence from the inference hot path

Scope:
- separate fetch/build, infer, and persist stages
- reduce synchronous DB work in the semantic worker loop
- preserve correctness and queue semantics

Exit proof:
- improved full-pipeline throughput with the same corpus and benchmark protocol

## Non-Goals

Not in scope for the first throughput branch:
- retrieval-quality benchmarking
- model replacement beyond currently selected `jina` / `bge-base` unless the throughput diagnosis proves the current model choice is the blocker
- wholesale rewrite of the semantic subsystem without intermediate measurement

## Success Criteria

The branch is successful if it delivers:

1. hard proof of where the time is going
2. a best-known GPU operating point
3. a structurally cleaner inference path
4. a final measured verdict against `300_000 embeddings/hour`

This branch is **not** successful if it only produces new knobs, new hopes, or new proxy numbers.
