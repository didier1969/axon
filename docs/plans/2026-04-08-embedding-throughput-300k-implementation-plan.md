# Embedding Throughput 300k Implementation Plan

## Goal

Execute the throughput branch in strict TDD until Axon can either:
- reach `300_000 embeddings/hour`, or
- prove with measurements why it cannot on the current hardware/software stack

## Current Proof Baseline

Known truth entering this plan:
- benchmark harness exists
- target is currently missed by a very large margin
- CUDA activity can be externally observed
- the worker has already been corrected once to honor calibrated fetch limits
- backend choice can now be explicitly overridden with `AXON_EMBEDDING_BACKEND`

## Tranche 1. Split Measurement Layers

**Status:** In progress on `2026-04-08`

### Objective

Separate:
- `model_only`
- `prepare_embed`
- `full_pipeline`

so that throughput losses are attributable.

### Files

- Modify: `src/axon-core/src/embedding_benchmark.rs`
- Modify: `src/axon-core/src/bin/embedding_benchmark.rs`
- Modify: `src/axon-core/src/tests/embedding_real_benchmark_tests.rs`
- Modify: `docs/architecture/2026-04-08-gpu-code-embeddings.md`

### Red

Add failing tests that require:
- benchmark mode enum or equivalent routing
- JSON report to expose which layer was measured
- `model_only` and `prepare_embed` to be distinguishable from the current benchmark mode

Run:
```bash
cargo test embedding_real_benchmark --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Green

Implement the smallest benchmark-layer split that passes the tests.

### Validate

Run:
```bash
cargo test embedding_real_benchmark --manifest-path src/axon-core/Cargo.toml -- --nocapture
cargo run --manifest-path src/axon-core/Cargo.toml --bin embedding_benchmark -- --help
```

### Exit Proof

One benchmark command can now emit clearly attributed results for each measurement layer.

**Current truth after the first Red/Green pass:**
- the benchmark contract now carries an explicit `measurement_layer`
- CLI supports `--measurement-layer model_only|prepare_embed|full_pipeline`
- tests and `--help` output are green
- the internal execution path is not yet split; this first pass stabilizes the public contract before the real benchmark-layer separation

## Tranche 2. Make Batch Sizes Runtime-Tunable

### Objective

Stop treating GPU batching as compile-time constants.

### Files

- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/tests/file_vectorization_throughput_tests.rs`
- Modify: `src/axon-core/src/tests/embedding_provider_tests.rs`
- Modify: `README.md`
- Modify: `docs/getting-started.md`

### Red

Add failing tests that require:
- runtime overrides for batch sizes
- calibrated profile to reflect the overrides
- semantic worker fetch limits to follow the effective runtime values

Run:
```bash
cargo test file_vectorization_throughput --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Green

Add explicit env/config overrides for:
- chunk batch size
- symbol batch size
- file vectorization batch size
- graph batch size

### Validate

Run:
```bash
cargo test file_vectorization_throughput --manifest-path src/axon-core/Cargo.toml -- --nocapture
cargo test embedding_provider --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Exit Proof

The branch can run a real batch sweep without code edits.

## Tranche 3. Produce a Saturation Matrix

### Objective

Empirically find the best stable GPU operating zone.

### Files

- Modify: `src/axon-core/src/embedding_benchmark.rs`
- Modify: `src/axon-core/src/bin/embedding_benchmark.rs`
- Modify: `docs/architecture/2026-04-08-gpu-code-embeddings.md`
- Modify: `docs/plans/2026-04-08-gpu-code-embeddings-implementation-plan.md`

### Red

Add failing tests that require:
- structured matrix output metadata
- each run to record batch size, backend requested, and measured layer

### Green

Implement a benchmark matrix mode or equivalent repeatable operator workflow.

### Validate

Run targeted tests, then real matrix runs on the target machine.

### Exit Proof

One best-known stable batch profile is selected from measured evidence, not intuition.

## Tranche 4. Expose Stronger Runtime Backend Truth

### Objective

Stop relying on `gpu_present` as the only runtime truth.

### Files

- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/embedding_benchmark.rs`
- Modify: `src/axon-core/src/tests/embedding_provider_tests.rs`

### Red

Add failing tests that require:
- explicit distinction between local GPU accessibility and backend request
- report semantics that no longer over-interpret `gpu_present`

### Green

Implement the minimal truth model:
- local GPU accessibility
- backend requested
- provider effective if known, otherwise `unknown`

### Validate

Run:
```bash
cargo test embedding_provider --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Exit Proof

Benchmark reports become operationally defensible even when the environment is unusual.

## Tranche 5. Decouple Inference From Persistence

### Objective

Reduce the amount of DB/queue work inside the embedding hot path.

### Files

- Modify: `src/axon-core/src/embedder.rs`
- Modify supporting persistence/fetch code only as required
- Add focused tests near the semantic worker pipeline

### Red

Add tests that fail until:
- the inference lane can process larger waves without immediate synchronous persistence on every micro-step
- queue correctness remains intact

### Green

Implement the smallest decoupling that preserves correctness:
- stage separation
- buffered persistence handoff

### Validate

Run targeted pipeline tests plus the real benchmark harness again.

### Exit Proof

Full-pipeline throughput materially improves relative to the previous baseline.

## Final Certification

The branch is only complete when it can report all of the following:
- best measured `model_only` throughput
- best measured `prepare_embed` throughput
- best measured `full_pipeline` throughput
- exact profile/backend/batch settings used
- final verdict against `300_000 embeddings/hour`
- explicit explanation of the limiting layer if target is still missed
