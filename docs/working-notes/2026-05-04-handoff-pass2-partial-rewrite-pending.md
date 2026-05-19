# Handoff — 2026-05-04 19:55 — Pass 2 partial landed, ≥30 ch/s rewrite pending

> Audit-only narrative. Canonical handoff lives in SOLL: REQ-AXO-181, DEC-AXO-070, VAL-AXO-029/030 + MEMORY.md active handoff section.

## Outcome
- 4 commits on `feat/AXO-181-simplify-vector-lane`
- Throughput: 0 → 1.07 ch/s end-to-end on Axon repo (8x)
- Target ≥30 ch/s NOT yet reached. Remaining gap = DEC-AXO-070 collapse 5 worker loops → 1 inline pipeline.

## Commits
| SHA | Subject |
|---|---|
| dcc5c95 | fix(scripts): add Nix gcc-cc.lib to indexer LD_LIBRARY_PATH (REQ-AXO-181 partial) |
| b6a2907 | chore(docs): compact project CLAUDE.md (-19.5%) |
| 29d5359 | fix(scripts): propagate AXON_GRAPH_EMBEDDINGS_ENABLED + AXON_GRAPH_WORKERS + recycle disable flags through allowlist |
| c1f1ab9 | fix(embedder): wrap vector_worker_loop body in inner loop (model alive on idle, kills 12s recompile cycle) |

## Three-layer bug structure (validated)
1. **Subprocess CHILD mode panic** — `ort::setup_api` panics specifically when `AXON_GPU_EMBED_SERVICE_CHILD=1` is set. Bench (release binary) doesn't hit it; subprocess (debug binary clone of axon-indexer) does. Workaround: `AXON_GPU_EMBED_SERVICE_ENABLED=0` forces in-process dispatch in `vector_worker_loop.rs:1042`.
2. **Per-iteration TensorRT recompile** — `'worker_lifecycle: loop` re-init'd BGE-Large each iteration. Idle paths fell through to end of body, model went out of scope. Wrapped body in inner unlabeled loop (commit c1f1ab9). Verified: "Vector Worker [0]: ORT GPU-first embedding runner loaded successfully" appears once per run instead of every ~12s.
3. **Multi-stage pipeline overhead** — 5 separate worker loops (refill, prepare, worker, persist, finalize) coordinate via 4 channels. Each stage has its own queue + scheduler. Sustained throughput ~1 ch/s suggests cross-stage latency dominates. NOT YET FIXED. DEC-AXO-070 prescribes collapse to single inline pipeline.

## Discoveries
- **`scripts/lib/axon-instance.sh::axon_clear_inherited_env`** is a strict allowlist that strips any AXON_* env not listed. Was missing AXON_GRAPH_EMBEDDINGS_ENABLED, AXON_GRAPH_WORKERS, AXON_GPU_RECYCLE_*. Added in commit 29d5359.
- **`scripts/lib/axon-ort-runtime.sh::axon_resolve_ort_runtime`** built `cuda_ld_path_segments` without Nix gcc-cc.lib (containing libstdc++ with GLIBCXX 14.3.0+ symbols). Added find/append in commit dcc5c95.
- **Multi-worker GPU contention** under `--indexer-full`: dev profile tries to load 5 BGE-Large instances (4 graph workers + 1 vector + 1 query CPU) on 8 GB GPU → cascade CUDA OOM (graph fall to cpu_fallback). `AXON_GRAPH_WORKERS=1` mitigates but doesn't eliminate.
- **`gpu_recreate_session_after_batch`** flag (line 859 of vector_worker_loop.rs) drops model after each successful batch when env var enabled. Currently disabled by default (`gpu_recreate_session_every_batch_enabled` returns false). Was a red herring during debug.
- **`VramRecycleCoordinator`** (embedder.rs:1100-1180) takes 5 signals (stuck/summit/pre_batch_plateau/low_throughput/vram_critical), recycles on multi-signal agreement OR sustained pressure. With recycle env disabled, signals don't fire. Confirmed via tmux: 0 recycle messages logged after fix.

## Next-session executable plan (per DEC-AXO-070 validation plan section)
1. Continue on `feat/AXO-181-simplify-vector-lane`.
2. Replace `vector_worker_loop` body with the inline sketch from DEC-AXO-070 description (claim → prepare → embed → persist → finalize, all inline, ~150 LOC).
3. Update `embedder.rs` factory (lines ~2020-2055) to stop spawning `vector_refill_workers`, `vector_prepare_workers`, `vector_persist_workers`, `vector_finalize_workers`. Remove the channels.
4. Delete files: `vector_refill_loop.rs`, `vector_prepare_loop.rs`, `vector_persist_loop.rs`, `vector_finalize_loop.rs`. Keep `vector_maintenance_loop.rs`.
5. Run L1 bench preflight: `scripts/dev/embed-bench.sh --n 600 --label preflight --csv` — must remain ≥140 ch/s warm.
6. Run end-to-end probe with workaround envs: `AXON_GPU_EMBED_SERVICE_ENABLED=0 AXON_GRAPH_WORKERS=1 ./scripts/axon-dev start --indexer-full --tensorrt` then `scripts/dev/probe.sh --scope /home/dstadel/projects/axon --duration 180 --tag pass3-rewrite`.
7. If <30 ch/s: investigate single bottleneck (DB write or claim query — most likely candidates per DEC-AXO-070). Do NOT re-add complexity reflexively.
8. Capture VAL-AXO-031 with CSV evidence + `axon_pre_flight_check` + `axon_commit_work`.
9. Update REQ-AXO-181 status to `delivered` if acceptance #1 met.

## Honest assessment
The 8x throughput improvement validates the simplification thesis but doesn't reach the target. The remaining 28x is locked behind the structural rewrite. Per Didier's repeated directive ("avons-nous vraiment besoin de cette complexité ?", "tu peux les enlever") the next session should commit to the radical collapse, not surgical patches.

## Tmux state
- `axon-dev-indexer` session was running, stopped clean via `./scripts/axon-dev stop`.
- GPU at 502 MB baseline.
- Live brain (axon-live) HEALTHY throughout — never touched.

## Bench / probe artifacts
- `dev-probe-v3-validation-20260504T182622Z.csv` (initial subprocess crash, 0 chunks)
- `dev-probe-pass2-v1-20260504T195216Z.csv` (post-config-fix, narrow scope, 0 chunks — watcher didn't detect subdir)
- `dev-probe-pass2-fullrepo-20260504T200127Z.csv` (post-inner-loop-wrap, full repo, 64 chunks/60s = 1.07 ch/s avg, 9.2 ch/s burst)

## Memory rule added this session
- `feedback_session_70pct_threshold.md` — don't propose stop above 70% remaining; reinforced "no questions about stopping" 2026-05-04.
