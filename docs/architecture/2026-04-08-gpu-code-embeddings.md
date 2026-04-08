# GPU Code Embeddings Architecture

## Purpose

This note is the source of truth for Axon's current code-embedding runtime on the `feat/workflow-hardening` branch.

It answers four operational questions:
- which model family Axon targets now
- what "GPU-ready" means concretely in this codebase
- how embedding storage and revectorization are governed
- what is already implemented versus still pending

## Current Runtime Truth

Axon no longer treats code embeddings as a fixed `384d` pipeline.

The runtime now carries an explicit embedding contract:
- primary profile: `jinaai/jina-embeddings-v2-base-code`
- fallback profile: `BAAI/bge-base-en-v1.5`
- legacy baseline still visible for comparison: `BAAI/bge-small-en-v1.5`

The current default contract is:
- dimension: `768`
- kinds: `symbol`, `chunk`, `graph`
- profile selection driven by environment:
  - `AXON_EMBEDDING_PROFILE`
  - `AXON_EMBEDDING_FALLBACK_PROFILE`
  - `AXON_EMBEDDING_BACKEND` with values `auto | cpu | cuda`

The canonical code lives in:
- [embedder.rs](/home/dstadel/projects/axon/.worktrees/dev/feat-workflow-hardening/src/axon-core/src/embedder.rs)
- [graph_bootstrap.rs](/home/dstadel/projects/axon/.worktrees/dev/feat-workflow-hardening/src/axon-core/src/graph_bootstrap.rs)
- [graph_ingestion.rs](/home/dstadel/projects/axon/.worktrees/dev/feat-workflow-hardening/src/axon-core/src/graph_ingestion.rs)

## Model Strategy

### Primary

`jinaai/jina-embeddings-v2-base-code`

Reason:
- code-oriented retrieval
- `768d`
- acceptable integration target for local `8 GB VRAM`

### Fallback

`BAAI/bge-base-en-v1.5`

Reason:
- same `768d`
- simpler fallback if Jina initialization fails

### Legacy Baseline

`BAAI/bge-small-en-v1.5`

Reason:
- legacy comparison only
- no longer the target runtime profile

## GPU Semantics

In Axon, "GPU-ready" does not mean "GPU proven."

The code now supports:
- explicit runtime backend selection
- CUDA execution-provider request
- backend-calibrated batching
- explicit benchmark measurement layers:
  - `model_only`
  - `prepare_embed`
  - `full_pipeline`

But the remaining distinction matters:
- `backend requested`: what Axon asks for
- `provider effective`: what ONNX Runtime actually ends up using

Current truth:
- Axon can request CUDA explicitly
- operators can now force backend selection with `AXON_EMBEDDING_BACKEND`, independently from the local `gpu_present` heuristic
- operators can now override embedding batch sizes explicitly with:
  - `AXON_EMBEDDING_CHUNK_BATCH_SIZE`
  - `AXON_EMBEDDING_SYMBOL_BATCH_SIZE`
  - `AXON_EMBEDDING_FILE_VECTORIZATION_BATCH_SIZE`
  - `AXON_EMBEDDING_GRAPH_BATCH_SIZE`
- the benchmark harness can compare CPU and requested-GPU contracts
- the benchmark harness now records whether a run used canonical calibrated batches or runtime override batches, and exposes both `canonical_profile_batches` and `effective_profile_batches`
- the branch does not yet prove the effective provider through runtime telemetry strong enough to certify real GPU execution in production terms

So the correct wording is:
- GPU-requesting and GPU-calibrated: yes
- GPU-certified at runtime: not yet fully

## Storage Contract

Embedding storage is no longer tied to `FLOAT[384]`.

Axon now persists embedding metadata in `RuntimeMetadata`:
- `embedding_version`
- `embedding_dimension`
- `embedding_model_name`

On compatibility drift, the runtime can reshape:
- `Symbol.embedding`
- `ChunkEmbedding.embedding`
- `GraphEmbedding.embedding`

This means a model or dimension change is no longer a silent mismatch between runtime and schema.

## File Vectorization Pipeline

The main throughput change in this branch is structural:
- file vectorization is no longer strictly one-file-at-a-time through the hot path
- the worker now uses a runtime budget:
  - `pause`
  - `file_fetch_limit`
  - `total_chunk_budget`
- chunk embedding waves can span multiple files in one pass

This matters more than just raising batch sizes, because the original bottleneck was:
- single semantic worker
- tiny fixed batches
- repeated `fetch -> embed -> write -> re-fetch` per file

The current implementation reduces this waste by:
- calibrating the profile for CPU vs CUDA request
- fetching multiple file jobs
- collapsing chunk embedding work into cross-file waves
- clearing queue entries only when files are truly `vector_ready`

Correction now applied on this branch:
- the semantic worker no longer computes a GPU-calibrated profile and then partially ignores it
- symbol embedding fetch size and graph projection fetch size now follow the calibrated runtime profile instead of staying pinned to legacy CPU constants

## Benchmarking Truth

Axon now exposes a proxy benchmark matrix for embedding profiles.

It compares:
- `jina`
- `bge-base`
- `legacy bge-small`

Across:
- CPU contract
- GPU-requested contract for the modern profiles

The proxy matrix reports:
- model name
- dimension
- model IDs by kind
- calibrated batch sizes
- file-vectorization runtime budget
- requested backend

This is intentionally not a real inference benchmark.

Why:
- TDD and local CI must stay stable
- real model loading introduces network/cache/runtime variability
- "real GPU benchmark" must be a separate explicit run mode

## Revectorization Runbook

### When Revectorization Is Required

Revectorize when any of these changes:
- embedding model name
- embedding dimension
- embedding version
- embedding serialization/storage contract

### What Axon Does Automatically

At startup, compatibility checks can:
- detect embedding drift
- invalidate semantic derived layers
- reshape storage columns/tables to the current embedding dimension

### What Operators Must Still Treat Carefully

Before a risky production intervention:
- back up `IST`
- back up `SOLL`
- never assume old embeddings remain compatible after a profile change

Operationally:
1. verify the target embedding profile and fallback
2. start Axon on a safe copy or development worktree first
3. allow compatibility logic to reshape storage
4. confirm queue refill / semantic invalidation behavior
5. only then run against production data

## Local Operator Usage

Examples:

```bash
AXON_EMBEDDING_PROFILE=jina \
AXON_EMBEDDING_FALLBACK_PROFILE=bge-base \
AXON_EMBEDDING_BACKEND=auto \
./scripts/start.sh
```

Force CUDA request explicitly when the process can reach the provider but the local
device-file heuristic is not trustworthy:

```bash
AXON_EMBEDDING_PROFILE=jina \
AXON_EMBEDDING_FALLBACK_PROFILE=bge-base \
AXON_EMBEDDING_BACKEND=cuda \
./scripts/start.sh
```

Force legacy baseline for comparison only:

```bash
AXON_EMBEDDING_PROFILE=legacy-bge-small \
./scripts/start.sh
```

Inspect the current proxy comparison through tests:

```bash
cargo test --lib embedding_profile_benchmark -- --nocapture
```

## Verified Scope On This Branch

Implemented and verified:
- configurable embedding contract
- explicit backend request surface
- storage migration beyond `384d`
- Jina primary + BGE base fallback
- GPU-calibrated file-vectorization runtime budgeting
- proxy benchmark matrix across profiles
- real benchmark harness with local corpus extraction and JSON output

Not yet fully certified:
- effective provider proof in production runtime telemetry
- full GPU benchmark on the target machine with visible CUDA provider
- retrieval-quality benchmark on a representative Axon corpus
- full MCP/tooling migration away from all legacy `*-384` identifiers

## Real Benchmark Truth On This Machine

Measured on `2026-04-08` from the worktree corpus in `src/axon-core`, using the new
`embedding_benchmark` binary and local corpus extraction.

Important distinction:
- the harness reports `gpu_present=false`
- this comes from Axon's current device-node heuristic in `RuntimeProfile::detect()`
- that heuristic is incomplete in this environment
- external GPU truth was confirmed separately with `nvidia-smi` during the CUDA runs

External machine truth observed:
- GPU present: `NVIDIA GeForce RTX 3070 Laptop GPU`
- VRAM: `8192 MiB`
- driver: `581.83`

### CPU runs

#### Legacy baseline: `BAAI/bge-small-en-v1.5` (`384d`)

- file target: about `28_992 embeddings/h`
- type target: about `33_549 embeddings/h`
- procedure target: about `28_974 embeddings/h`

#### Primary target: `jinaai/jina-embeddings-v2-base-code` (`768d`)

Fast downsampled run (`16` measured samples per target) used to obtain a first real reading:

- file target: about `7_902 embeddings/h`
- type target: about `14_464 embeddings/h`
- procedure target: about `13_209 embeddings/h`

### CUDA-requested runs with external GPU confirmation

#### Legacy baseline: `BAAI/bge-small-en-v1.5` (`384d`)

During the run, external telemetry observed:
- GPU utilization: about `41%`
- VRAM used: about `798 MiB / 8192 MiB`

Measured throughput:
- file target: about `27_252 embeddings/h`
- type target: about `22_153 embeddings/h`
- procedure target: about `26_302 embeddings/h`

#### Primary target: `jinaai/jina-embeddings-v2-base-code` (`768d`)

During the run, external telemetry observed:
- GPU utilization: about `28%` to `34%`
- VRAM used: about `798` to `810 MiB / 8192 MiB`

Measured throughput:
- file target: about `7_973 embeddings/h`
- type target: about `8_889 embeddings/h`
- procedure target: about `12_519 embeddings/h`

Operational conclusion:
- the branch now supports real measured CPU and CUDA-requested benchmark runs
- CUDA execution was externally confirmed on this machine, even though Axon's internal `gpu_present` heuristic still reports `false`
- the strategic target `300_000 embeddings/h` is not remotely reached in the current implementation
- on this machine, both CPU and current CUDA runs remain far below the target
- the next proof required is not "can we ask for CUDA?" but "why does real GPU throughput remain this low, and what change would move the curve materially?"
- the benchmark contract now explicitly distinguishes the measurement layer, even if the first implementation still routes all real runs through the historical benchmark path
- the branch now applies a first real semantic split:
  - `full_pipeline` includes corpus collection in `total_seconds`
  - `model_only` prebuilds batches before timing the inference loop
  - `prepare_embed` now carries explicit `prepare_seconds`
  - `prepare_embed` and `full_pipeline` include preparation time in `total_seconds`

### CPU layer-split runs on `2026-04-08`

These runs used the same reduced local corpus and `16` measured samples per target.

#### `jinaai/jina-embeddings-v2-base-code` (`768d`)

- `model_only`
  - file: about `5_470 embeddings/h`
  - type: about `12_366 embeddings/h`
  - procedure: about `9_562 embeddings/h`
- `prepare_embed`
  - file: about `7_464 embeddings/h`
  - type: about `14_746 embeddings/h`
  - procedure: about `13_793 embeddings/h`
- `full_pipeline`
  - file: about `5_345 embeddings/h`
  - type: about `7_784 embeddings/h`
  - procedure: about `7_356 embeddings/h`

#### `BAAI/bge-base-en-v1.5` (`768d`)

- `model_only`
  - file: about `13_691 embeddings/h`
  - type: about `12_139 embeddings/h`
  - procedure: about `15_555 embeddings/h`
- `prepare_embed`
  - file: about `12_849 embeddings/h`
  - type: about `11_470 embeddings/h`
  - procedure: about `13_454 embeddings/h`
- `full_pipeline`
  - file: about `6_833 embeddings/h`
  - type: about `6_535 embeddings/h`
  - procedure: about `7_546 embeddings/h`

What these layer-split CPU runs prove:
- `bge-base` is currently faster than `jina` on this harness for the same `768d` storage contract
- the measured `prepare_seconds` remain effectively negligible in this reduced harness
- the dominant loss is still not payload preparation; it is the embedding/model path, then the broader full-pipeline overhead
- full-pipeline throughput drops materially relative to `model_only`, especially on `bge-base`, so the pipeline outside raw inference is still expensive
- even the best observed CPU layer result remains far below `300_000 embeddings/h`

### CUDA-requested layer-split runs on `2026-04-08`

These runs used the same reduced local corpus and `16` measured samples per target.
External GPU activity had already been confirmed separately on this machine, but the
benchmark JSON still reports `gpu_present=false`, so the internal runtime truth remained
incomplete until the provider-truth tranche below.

#### `jinaai/jina-embeddings-v2-base-code` (`768d`)

- `model_only`
  - file: about `9_119 embeddings/h`
  - type: about `9_614 embeddings/h`
  - procedure: about `11_339 embeddings/h`
- `prepare_embed`
  - file: about `8_333 embeddings/h`
  - type: about `14_161 embeddings/h`
  - procedure: about `12_558 embeddings/h`
- `full_pipeline`
  - file: about `5_768 embeddings/h`
  - type: about `6_293 embeddings/h`
  - procedure: about `5_995 embeddings/h`

#### `BAAI/bge-base-en-v1.5` (`768d`)

- `model_only`
  - file: about `12_114 embeddings/h`
  - type: about `12_293 embeddings/h`
  - procedure: about `14_208 embeddings/h`
- `prepare_embed`
  - file: about `12_312 embeddings/h`
  - type: about `11_495 embeddings/h`
  - procedure: about `13_356 embeddings/h`
- `full_pipeline`
  - file: about `6_853 embeddings/h`
  - type: about `6_604 embeddings/h`
  - procedure: about `7_514 embeddings/h`

What these layer-split CUDA-requested runs prove:
- the first complete CPU/CUDA layer matrix now exists for both `jina` and `bge-base`
- requesting CUDA does not currently create a decisive throughput jump on this harness
- `bge-base` remains faster than `jina` on raw `model_only` and comparable on `full_pipeline`
- the largest remaining ceiling is still not the measured preparation step; it is the model path and the broader pipeline around it
- even with CUDA requested, the best observed layer result remains far below `300_000 embeddings/h`
- the next high-leverage question is no longer "can Axon request CUDA?" but "why does the effective path remain this close to CPU, and which batch/pipeline changes materially move the curve?"

### Provider-truth tranche on `2026-04-08`

Axon now exposes a stricter runtime truth model for embedding runs and worker startup:

- `requested_backend`
  - what the operator or benchmark asked for
- `gpu_present`
  - a local device heuristic only
- `device_heuristic_backend`
  - the backend Axon would have inferred from the local heuristic alone
- `provider_effective`
  - populated only when the startup path makes the provider operationally provable
- `provider_status`
  - currently `verified` or `unverified`
- `provider_note`
  - a short explanation of why the provider claim is, or is not, strong

Current semantics:
- explicit `cpu` request is reported as `provider_effective=cpu` and `provider_status=verified`
- explicit `cuda` request is not reported as proven CUDA execution from request alone
- if CUDA is requested, Axon now reports that as `provider_status=unverified` unless a stronger runtime proof exists
- the worker log uses the same truth model at startup, so runtime telemetry and benchmark semantics no longer diverge

What this tranche proves:
- Axon no longer over-interprets `gpu_present`
- Axon no longer treats `requested_backend=cuda` as proof of GPU execution
- benchmark JSON and worker startup logs are now operationally defensible even on hosts where the local heuristic is false-negative

What it still does not prove:
- the exact effective ONNX provider for CUDA-requested runs remains unproven from the current `fastembed` / `ort` integration alone
- external evidence such as `nvidia-smi` is still required when that level of proof matters

### Batch override smoke proof on `2026-04-08`

The harness now also proves a different truth: explicit runtime batch overrides are no longer
hidden.

A reduced smoke run with `legacy-bge-small`, `cpu`, and `model_only` produced:
- `batch_override_active = true`
- `batch_override_source = runtime_env`
- canonical profile batches:
  - chunk: `16`
  - symbol: `32`
  - file vectorization: `8`
  - graph: `6`
- effective profile batches:
  - chunk: `32`
  - symbol: `64`
  - file vectorization: `16`
  - graph: `8`

What this proves:
- batch tuning no longer requires code edits
- the report now makes override usage explicit instead of silently mutating comparisons
- the next sweep can compare canonical vs tuned runs honestly

### Reduced saturation probe on `2026-04-08`

The first directionally honest saturation probe was run sequentially, not in parallel, because
parallel benchmark processes contaminated the measurement. The reduced probe used:

- profile: `BAAI/bge-base-en-v1.5`
- backend requested: `cuda`
- layer: `model_only`
- corpus limits: `max_files=16`, `max_samples_per_target=16`
- measured samples per target: `8`
- target under comparison: `procedure`

Results:
- canonical `symbol_batch_size=64`:
  - `~38_467 embeddings/h`
- override `symbol_batch_size=96`:
  - `~29_955 embeddings/h`
- override `symbol_batch_size=128`:
  - `~30_081 embeddings/h`

What this reduced probe proves:
- on this harness, the current calibrated GPU symbol batch `64` is already better than the first larger neighbors `96` and `128`
- the curve is not monotonically increasing with larger batch sizes
- the next large performance win is unlikely to come from simply pushing `symbol_batch_size` upward on the current path
- batching still matters, but the dominant remaining ceiling is likely deeper in the effective inference/runtime path than in this one knob alone

Additional runtime truth discovered after the first benchmark pass:
- production was still underusing the calibrated profile in part of the worker loop
- specifically, symbol and graph fetch waves were still pinned to `32` and `6` through legacy constants
- this branch now aligns those fetch limits with the calibrated profile, but that correction alone does not explain the full gap to `300_000 embeddings/h`

## Next Logical Steps

- expose stronger effective-provider runtime truth instead of relying on external correlation
- make batch sizes fully runtime-tunable and sweep them under the new layer-split harness
- decouple preparation, inference, and persistence further so `full_pipeline` stops masking the inference ceiling
- add retrieval-quality evaluation on code queries
