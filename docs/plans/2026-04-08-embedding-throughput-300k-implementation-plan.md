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

**Status:** Completed on `2026-04-08`

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
- the internal execution path now has a first real split:
  - `full_pipeline` includes corpus collection in the measured total
  - `model_only` prebuilds payload batches before entering the timed inference loop
  - `prepare_embed` now carries explicit `prepare_seconds`
  - `prepare_embed` and `full_pipeline` include preparation time in the measured total
- first measured CPU and CUDA-requested layer matrices now exist for both `jina` and `bge-base`
- current evidence from those matrices:
  - `bge-base` is faster than `jina` on the reduced CPU harness
  - measured preparation cost is negligible in this harness
  - CUDA request does not currently produce a decisive throughput jump
  - the dominant ceiling remains embedding/inference, followed by wider full-pipeline overhead
  - the target `300_000 embeddings/h` is still missed by a very large margin

## Tranche 2. Make Batch Sizes Runtime-Tunable

### Objective

Stop treating GPU batching as compile-time constants.

### Files

- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/embedding_benchmark.rs`
- Modify: `src/axon-core/src/tests/embedding_config_tests.rs`
- Modify: `docs/architecture/2026-04-08-gpu-code-embeddings.md`

### Red

Add failing tests that require:
- runtime overrides for batch sizes
- runtime contract to expose the overrides
- calibrated profile to reflect the overrides even under the GPU floor

Run:
```bash
cargo test embedding_runtime_contract_applies_explicit_batch_overrides --manifest-path src/axon-core/Cargo.toml -- --nocapture
cargo test explicit_batch_overrides_win_over_gpu_floor --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Green

Add explicit env/config overrides for:
- chunk batch size
- symbol batch size
- file vectorization batch size
- graph batch size
- benchmark reports to expose canonical calibrated batches vs effective override batches

### Validate

Run:
```bash
cargo test embedding_config --manifest-path src/axon-core/Cargo.toml -- --nocapture
cargo test embedding_real_benchmark --manifest-path src/axon-core/Cargo.toml -- --nocapture
cargo run --manifest-path src/axon-core/Cargo.toml --bin embedding_benchmark -- --help
```

### Exit Proof

The branch can run a real batch sweep without code edits, and each run can now distinguish:
- canonical calibrated batch sizes
- effective override batch sizes
- whether the run used runtime overrides at all

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

**Current truth after the first reduced saturation probe:**
- the first parallel sweep attempt was invalid for comparison because concurrent benchmark processes contaminated throughput
- the first sequential reduced probe was run on `bge-base + cuda + model_only`
- on the reduced harness, `procedure` throughput at `symbol_batch_size=64` (`~38.5k/h`) was better than both:
  - `symbol_batch_size=96` (`~30.0k/h`)
  - `symbol_batch_size=128` (`~30.1k/h`)
- therefore, increasing `symbol_batch_size` above the current calibrated GPU batch does not improve throughput on this path
- the next tranche should not be “push batch higher again blindly”; it should focus on stronger runtime/provider truth and deeper inference-path diagnosis

## Tranche 4. Expose Stronger Runtime Backend Truth

### Objective

Stop relying on `gpu_present` as the only runtime truth.

### Files

- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/embedding_benchmark.rs`
- Modify: `src/axon-core/src/tests/embedding_provider_tests.rs`
- Modify: `src/axon-core/src/tests/embedding_real_benchmark_tests.rs`
- Modify: `docs/architecture/2026-04-08-gpu-code-embeddings.md`

### Red

Add failing tests that require:
- explicit distinction between local GPU accessibility and backend request
- report semantics that no longer over-interpret `gpu_present`

### Green

Implement the minimal truth model:
- local GPU accessibility
- backend requested
- heuristic backend derived from local visibility
- provider effective only if operationally provable, otherwise `unknown/unverified`

### Validate

Run:
```bash
cargo test embedding_provider --manifest-path src/axon-core/Cargo.toml -- --nocapture
```

### Exit Proof

Benchmark reports become operationally defensible even when the environment is unusual.

**Current truth after Tranche 4:**
- benchmark and worker startup now distinguish:
  - `requested_backend`
  - `gpu_present`
  - `device_heuristic_backend`
  - `provider_effective`
  - `provider_status`
  - `provider_note`
- explicit CPU request is now treated as operationally verifiable
- explicit CUDA request is no longer misreported as proven GPU execution from request alone
- the tranche intentionally does **not** claim exact CUDA provider proof from `fastembed/ort`; that remains a later problem if stronger introspection is required

**Current truth after the ORT registration-probe extension:**
- CUDA-requested startup now performs a bounded ORT preflight using `error_on_failure()`
- the runtime truth now also carries:
  - `provider_provenance`
  - `provider_registration_outcome`
- successful ORT registration is now reported as stronger startup evidence than the local GPU heuristic alone
- failed ORT registration is now explicitly surfaced as a fallback-class outcome
- this still does **not** prove the exact final effective provider for every inference op; it only proves stronger startup registration truth

**Current truth after the CUDA feature activation tranche:**
- `src/axon-core/Cargo.toml` now explicitly enables `ort/cuda`
- `cargo tree -e features -i ort` proves `ort feature "cuda"` is active
- `cargo tree -e features -i ort-sys` proves `ort-sys feature "cuda"` is active
- the reduced smoke benchmark still does **not** register CUDA successfully at runtime
- the new failure is now explicit and lower-level:
  - `libonnxruntime_providers_cuda.so` fails to load because `libcudnn.so.9` is missing
- therefore the blocking truth has changed:
  - it is no longer “CUDA feature not enabled in the build”
  - it is now “CUDA runtime dependency chain incomplete in the active shell/runtime”
- consequence:
  - no current `cuda` benchmark result on this host should be counted as valid GPU-throughput proof until `provider_registration_outcome=registered`
- a shell-level preflight now exists too:
  - `env AXON_EMBEDDING_BACKEND=cuda devenv shell -- bash scripts/validate-devenv.sh`
  - current result on this host: fails early on missing `libcudnn.so.9`
  - this is now the preferred red/green gate before any further CUDA benchmark run

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
