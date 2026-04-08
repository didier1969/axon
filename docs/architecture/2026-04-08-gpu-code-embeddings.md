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

But the remaining distinction matters:
- `backend requested`: what Axon asks for
- `provider effective`: what ONNX Runtime actually ends up using

Current truth:
- Axon can request CUDA explicitly
- the benchmark harness can compare CPU and requested-GPU contracts
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

Not yet fully certified:
- effective provider proof in production runtime telemetry
- real `embeddings/s` benchmark on your machine
- retrieval-quality benchmark on a representative Axon corpus
- full MCP/tooling migration away from all legacy `*-384` identifiers

## Next Logical Steps

- add a real benchmark mode with cold/warm timings
- expose effective provider telemetry
- finish removing legacy `*-384` assumptions from MCP and tests
- add retrieval-quality evaluation on code queries
