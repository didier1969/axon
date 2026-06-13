# Session 77 — Pipeline B simplification + optimizer retirement

**SOLL**: DEC-AXO-901631 / REQ-AXO-901975 (REFINES REQ-AXO-901896, resolves CPT-AXO-90047).
**Goal**: pipeline B vectorization ~35 ch/s → 200-250 ch/s. Operator: « complexité tue la performance ».

## Root cause (confirmed by code + SOLL)
The embed lane was starved (GPU 1 %) by three stacked control layers, not GPU limits:
1. Predictive RL optimizer (OFF by default, buggy — REQ-AXO-901862/901832) sizing on in-process backlog.
2. Token-bucketing (`build_token_aware_micro_batches`) re-fragmenting batches into 4-16 item micro-inferences.
3. Stale TensorRT engine cache (the profiles are already dynamic 1×1/64×256/64×512).

**Key discovery**: the pending SELECT already `ORDER BY token_count` — the sort was half-there, its value destroyed downstream by layer 2.

## Delivered (commits on main, NOT pushed)
| Commit | Wave | Content |
|---|---|---|
| 70c6981e | 2+3 | Sorted-drain replaces demand_pull (s,Q); `demand_pull.rs` deleted (602 l.). Token-bucketing collapsed: one inference per length-homogeneous batch (`OrtGpuFirstTextEmbedding::embed_texts`). |
| d53ff61c | 4a | Predictive optimizer actuation retired (shadow loop + governor). |
| a9318bc5 | 4b | `optimizer.rs` stripped 2756 → 572 l. (RL engine removed; signal collectors kept for display). |
| 86fbafd6 | 4c | Dead decision/reward log tables + writers dropped (DDL + assertions). |
| 6831f22e | 6 | GPU session sleep/wake retired — always-resident (process-level GPU exclusion handles cohabitation). |

## Design (correct-by-construction)
Sorted-drain loop (`spawn_vector_sorted_drain`): pull token-sorted reservoir → feed B2 in order → fixed batch → one stable GPU shape. Channel backpressure prevents the 901862 runaway by construction. `embed_status='pending'` = durable queue (B3 stamps idempotently); no dedup cache, no NOTIFY, no claim column.

Env knobs (fixed, non-overlapping): `AXON_B2_RESERVOIR=8192`, `AXON_B2_BATCH_SIZE=64`, `AXON_EMBEDDER_SEQ_BUCKETS=128,256,384,512`.

## Wave outcomes 5/7/8
- **Wave 5** (dead bulk-vector): `vector_worker_loop` / `vector_embed_file_batch_sharded` already removed (REQ-AXO-901653) — no-op.
- **Wave 7** (UtilityFirstScheduler): NOT orphaned — it's the live vector batch controller + drain-state, used by telemetry. Kept (already decoupled from the optimizer).
- **Wave 8**: deleted dead `config/optimizer-weights.example.toml`.

## Follow-up (documented, not done)
- **Obsolete sweep-based bench tooling**: `embed_texts_with_breakdown_ort` + `build_token_aware_micro_batches` + `configured_embedding_micro_batch_*` now serve ONLY the standalone bench bins (`embedder-bench` sweep, `axon-bench-pipeline` v1) + `AXON_EMBED_MICRO_BATCH_*` + sweep scripts. These are dev tooling for the OLD sweep approach (obsolete under fixed-batch); a focused removal can follow once Wave 9 validates the new design. NOT production complexity (that's resolved).
- **Cosmetic**: rename `optimizer` module → `runtime_signals` (it now only holds signal collectors); 5 importers.

## Wave 9 — validation (NEXT, the proof)
```
rm -rf <tensorrt_cache_dir>/engine-cache/      # one-time, rebuild on dynamic profile
./scripts/clean_axon_dev.sh --yes && ./scripts/axon-dev start full
# env CUDA: ORT_DYLIB_PATH + LD_LIBRARY_PATH=/usr/lib/wsl/lib
cargo run --release --bin axon-bench-pipeline-v2 -- --source <PATH> --gpu --human
```
Target ≥200 ch/s, measured via `ChunkEmbedding` row-count/60s (nvidia-smi hangs under load). Iterate `AXON_B2_BATCH_SIZE`/`AXON_B2_RESERVOIR`. Then promote-live operator-gated.
