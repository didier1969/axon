# Axon Env Variable Inventory — 2026-05-22 (Session 52)

**Auditor:** Claude (Opus 4.7 · session 52) on operator request.
**Scope:** all `*.rs` under `src/`, `scripts/`, `db/`, `devenv.nix`.
**Mechanical baseline:** 413 unique `AXON_*` tokens grepped repo-wide ; 217 distinct names actually read via `std::env::var("…")` from Rust ; the rest are exports in shell scripts, doc strings, env-source labels (`AXON_POLICY_SOURCE_*`), test fixtures, or auto-config side-effect twins.
**Backing files:** `git log --oneline -20` ; `grep -rhE 'env::var\(["\x27][A-Z][A-Z0-9_]+["\x27]\)' src/`.
**Companion doc:** `docs/audits/2026-05-22-kpis-inventory.md`.

Status legend :
- **active** = read in production hot path, current architecture (pipeline v2 / PG canonical / lifecycle machine).
- **deprecated** = still read but covered by superseded architecture (DEC-AXO-060 4-verb, REQ-AXO-289 pipeline v2, MIL-AXO-017 PG canonical).
- **dead** = references AGE, DuckDB, FileVectorizationQueue, legacy vector_worker_loop, parquet store, scope reconcile orchestrator — already removed or behind a default-off opt-in.
- **test-only** = only read in `tests/`, benches or `axon-bench-*` binaries.
- **unknown** = surface confusing or owner unclear ; flagged for triage.

---

## 1. AXON_* — Runtime mode + identity

| Name | Read at | Default | Effect | Status | Notes |
|---|---|---|---|---|---|
| `AXON_RUNTIME_MODE` | `src/axon-core/src/runtime_boot.rs` ; `mcp/tools_governance.rs:117` ; `start-brain.sh` | derived | `brain_only` / `indexer_only` / `indexer_graph` / `indexer_full` selector | active | DEC-AXO-060 canonical. Cross-checked by `runtime_profile.rs`. |
| `AXON_RUNTIME_PROFILE` | `runtime_boot.rs` | derived | profile name for resource policy resolution | active | |
| `AXON_RUNTIME_IDENTITY` | `runtime_boot.rs` | `axon-{instance}-{binary}` | string used in status banner | active | |
| `AXON_INSTANCE` | `scripts/lib/*.sh` | `live` | live/dev instance switch | active | |
| `AXON_INSTANCE_KIND` | `runtime_boot.rs` | `live` | same as above, runtime-side | active | duplicate of `AXON_INSTANCE` — see cluster A |
| `AXON_INSTANCE_STATE_FILE` | `scripts/lib/runtime-state.sh` | runtime path | JSON state file | active | |
| `AXON_LIVE` / `AXON_DEV_DATABASE_URL` / `AXON_LIVE_DATABASE_URL` | embedder, postgres modules | unset | PG URL overrides | active | dev/live triple maps to two PG ports (44144/44137) |
| `AXON_DATABASE_URL` | postgres module fallback | derived | global override | active | |
| `AXON_BUILD_ID` | runtime_boot | from binary | identity probe | active | |
| `AXON_PACKAGE_VERSION` | runtime_boot | crate version | identity | active | |
| `AXON_RELEASE_VERSION` | runtime_boot | from manifest | live release tag | active | |
| `AXON_INSTALL_GENERATION` | promote scripts | counter | generation epoch | active | REQ-AXO-901638 fix |
| `AXON_RUNTIME_CONFIG_FILE` | runtime_boot | derived | path | active | |
| `AXON_RUNTIME_STATE_FILE` | runtime_boot | derived | path | active | |
| `AXON_RUNTIME_BOOT_ROLE` | runtime_boot | derived | role label | active | |
| `AXON_RUNTIME_SHADOW_ROLE` | shadow optimizer | unset | shadow role | deprecated | shadow optimizer disabled by default (REQ-AXO-90009) |
| `AXON_RUNTIME_REACTIVATION_PATH` | runtime_boot | unset | reactivation hint path | unknown | |
| `AXON_LAST_RUNTIME_MODE` | runtime_boot | unset | persisted last mode | active | |
| `AXON_RUNTIME_COMMAND_PROXY_ENABLED` | command proxy | `false` | proxy switch | unknown | only test path observed |
| `AXON_RUNTIME_COMMAND_PROXY_TEST_PANIC` | proxy test harness | unset | panic injection | test-only | |
| `AXON_RUNTIME_COMMAND_PROXY_TEST_LATENCY_MS` | proxy test harness | unset | latency injection | test-only | |
| `AXON_RUNTIME_COMMAND_PROXY_TIMEOUT_MS` | proxy | unset | proxy deadline | unknown | |
| `AXON_RUNTIME_TRACE_ENABLED` | trace plumbing | `false` | runtime trace | active | dev triage |
| `AXON_RUNTIME_TRACE_INTERVAL_MS` | trace | `1000` | sample interval | active | |
| `AXON_RUNTIME_TRACE_PATH` | trace | derived | output path | active | |
| `AXON_RUN_ROOT` / `AXON_RUN_ROOT_BASE` | runtime_boot | derived | runtime root dir | active | |
| `AXON_PROJECT_CODE` | shell | unset | project override | active | usually auto-resolved from cwd |
| `AXON_PROJECT_ROOT` | runtime_boot | cwd-derived | project root | active | |
| `AXON_PROJECTS_ROOT` | runtime_boot | parent dir | multi-project root | active | |
| `AXON_REPO` / `AXON_REPO_ROOT` / `AXON_REPO_SLUG` | scripts | derived | repo metadata | active | scripts only |
| `AXON_WORKTREE_ENV_LOADED` | scripts/devenv.nix | bool | shell entry guard | active | |
| `AXON_ENV_VARS_LOADED` | scripts | bool | shell entry guard | active | duplicate semantic with WORKTREE — see cluster H |
| `AXON_PID_FILE` | scripts | derived | pid file path | active | |

## 2. AXON_* — Pipeline v2 (REQ-AXO-289 canonical lanes)

| Name | Read at | Default | Effect | Status |
|---|---|---|---|---|
| `AXON_A1_WORKERS` | `mcp/tools_system.rs:379` ; `pipeline_v2_runtime.rs` | 4 | Pipeline A lane 1 (parser) workers | active |
| `AXON_A2_WORKERS` | same | 8 | A lane 2 (chunk+FTS) workers | active |
| `AXON_A3_WORKERS` | same | 2 | A lane 3 (writer batch) workers | active |
| `AXON_A3_BATCH_SIZE` | same | 32 | A3 batch size | active |
| `AXON_A3_BATCH_TIMEOUT_MS` | same | 10 | A3 flush deadline | active |
| `AXON_A3_TO_B1_BUFFER` | `mcp/tools_system.rs:408` | `A3_TO_B1_BUFFER_CAP_DEFAULT` | try_send buffer cap | active |
| `AXON_B1_WORKERS` | same | 4 | B lane 1 (claim) workers | active |
| `AXON_B2_WORKERS` | same | 1 | B lane 2 (GPU embed) workers | active |
| `AXON_B3_WORKERS` | same | 2 | B lane 3 (persist) workers | active |
| `AXON_B1_COLDSTART_BATCH_SIZE` | `mcp/tools_system.rs:404` | const | cold-start poll batch | active |
| `AXON_B2_BATCH_SIZE` / `_TIMEOUT_MS` | same | const | GPU embed batch | active |
| `AXON_B3_BATCH_SIZE` / `_TIMEOUT_MS` | same | const | persist batch | active |
| `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP` | pipeline_v2 | const | alt name for `AXON_A3_TO_B1_BUFFER` | dead | duplicate — see cluster B |
| `AXON_PIPELINE_INTERNAL_CHANNEL_CAP` | pipeline_v2 | const | internal mpmc cap | active |
| `AXON_PIPELINE_TRACE_CSV` | pipeline_v2 | unset | per-event CSV trace | active | dev triage only |
| `AXON_A_WORKERS` / `AXON_A1` / `AXON_B_WORKERS` / `AXON_B1` | grep-only | n/a | scripts-only aliases | dead | only appearance is in `_w*_runner.sh` debug logs |

## 3. AXON_* — Pipeline v1 legacy (overlaps with v2)

| Name | Read at | Default | Status | Notes |
|---|---|---|---|---|
| `AXON_VECTOR_WORKERS` | `embedder.rs:328` | 1 | deprecated | overlaps `AXON_B2_WORKERS` |
| `AXON_VECTOR_WORKERS_AUTOCONFIGURED` | `embedder.rs:1665` | bool | deprecated | auto-config flag for v1 |
| `AXON_VECTOR_PRODUCERS` / `AXON_VECTOR_PERSISTERS` / `AXON_VECTOR_EMBEDDERS` | embedder | const | deprecated | v1 stage names |
| `AXON_VECTOR_PIPELINE_STAGES` | embedder | const | deprecated | v1 stage count |
| `AXON_VECTOR_PIPELINE_INLINE` | embedder | `false` | deprecated | v1 inline mode |
| `AXON_VECTOR_PREPARE_PIPELINE_DEPTH` | embedder | const | deprecated | v1 prep stage |
| `AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_READY_QUEUE_DEPTH` / `_AUTOCONFIGURED` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_TARGET_READY_CHUNKS` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_PERSIST_QUEUE_BOUND` / `_AUTOCONFIGURED` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_MAX_INFLIGHT_PERSISTS` / `_AUTOCONFIGURED` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_LEASE_STALE_MS` | embedder | const | deprecated | v1 lease |
| `AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE` | embedder | const | deprecated | v1 |
| `AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING` | embedder | `false` | deprecated | v1 |
| `AXON_GRAPH_WORKERS` / `_AUTOCONFIGURED` | embedder.rs:342 | 1 | deprecated | overlaps `AXON_A2/A3_WORKERS` |
| `AXON_GRAPH_BATCH_SIZE` / `_AUTOCONFIGURED` | embedder.rs:348 | const | deprecated | overlaps A3 batch |
| `AXON_LEGACY_VECTOR_WORKER_LOOP` | `embedder.rs:1229` | `false` | deprecated | opt-in to legacy loop — REQ-AXO-901653 Slice 1 partial |
| `AXON_FILE_VECTORIZATION_BATCH_SIZE` / `_AUTOCONFIGURED` | embedder | const | dead | FileVectorizationQueue table removed |
| `AXON_TSV_BATCH_SIZE` | tsv worker | const | dead | TSV (transactional source vector) v1 |
| `AXON_TSV_POLL_INTERVAL_MS` | tsv worker | const | dead | |
| `AXON_TSV_VISIBILITY_TIMEOUT_S` | tsv worker | const | dead | |
| `AXON_TSV_WORKER_CONCURRENCY` | tsv worker | const | dead | |
| `AXON_TSV_` | scripts/dev | prefix | dead | prefix wrapper |
| `AXON_CHUNK_BATCH_SIZE` / `_AUTOCONFIGURED` | embedder | const | deprecated | overlaps `AXON_A3_BATCH_SIZE` |
| `AXON_CHUNK_OVERLAP_TOKENS` | chunker | const | active | shared across A2 + v1 |
| `AXON_CHUNK_MODEL_ID` | grep-only | unset | deprecated | legacy override |
| `AXON_TARGET_CHUNK_TOKENS` | chunker | const | active | |
| `AXON_MAX_CHUNKS_PER_FILE` | grep-only | const | active | |
| `AXON_QUERY_EMBED_WORKERS` / `_AUTOCONFIGURED` | grep-only | const | deprecated | only consumed by v1 |
| `AXON_QUERY_EMBED_PROVIDER` | embedder | unset | active | LLM-facing query embed |

## 4. AXON_* — Pipeline v1 graph vectorization (mostly retired)

| Name | Status | Notes |
|---|---|---|
| `AXON_GRAPH_EMBEDDINGS_ENABLED` | deprecated | flipped per profile in `runtime_profile.rs` ; pipeline v2 owns embeddings now |
| `AXON_GRAPH_EMBED_PROVIDER` | deprecated | v1 graph embed lane |
| `AXON_ENABLE_GRAPH_VECTORIZATION` | dead | graph vectorization stripped session 51 (commit 2717359b) |
| `AXON_VECTOR_PERSIST_QUEUE_BOUND` … | deprecated | see § 3 |

## 5. AXON_* — Embedder / GPU / TensorRT / VRAM guard

| Name | Default | Status | Notes |
|---|---|---|---|
| `AXON_EMBEDDING_PROVIDER` | derived | active | LLM-facing |
| `AXON_EMBEDDING_PROVIDER_EFFECTIVE` | derived | active | result of resolution |
| `AXON_EMBEDDING_PROVIDER_INIT_ERROR` | unset | active | error pin |
| `AXON_EMBEDDING_DOWNLOAD_PROGRESS` | bool | active | model d/l progress |
| `AXON_EMBEDDING_GPU_PRESENT` | bool | active | GPU probe outcome |
| `AXON_EMBEDDER_SEQ_BUCKETS` | const | active | sequence-length bucketization |
| `AXON_EMBED_MAX_LENGTH` | 512 | active | tokenizer max |
| `AXON_EMBED_BATCH_MAX_TOTAL_TOKENS` | const | active | total tokens per batch |
| `AXON_EMBED_MICRO_BATCH_MAX_ITEMS` / `_AUTOCONFIGURED` | const | active | micro-batch sizing |
| `AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS` / `_AUTOCONFIGURED` | const | active | |
| `AXON_EMBED_TOKEN_BUCKET_SIZE` | const | active | |
| `AXON_MAX_EMBED_BATCH_BYTES` | const | active | |
| `AXON_GPU_ACCESS_POLICY` | derived | active | shared / exclusive |
| `AXON_GPU_BACKEND` | derived | active | CUDA / TensorRT / CPU |
| `AXON_GPU_EMBED_SERVICE_ENABLED` | bool | active | sub-process embed service |
| `AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH` | bool | active | watchdog |
| `AXON_GPU_EMBED_SERVICE_TENSORRT` | bool | active | TRT engine in subservice |
| `AXON_GPU_MULTIWORKER_MIN_FREE_MB` | const | active | guard |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED` | bool | active | |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB` | const | active | |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES` | const | active | |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE` | bool | active | |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS` | const | active | |
| `AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED` | bool | active | |
| `AXON_GPU_PRIMARY_WORKER_MAX_USED_MB` | const | active | |
| `AXON_GPU_READY_HIGH_WATERMARK` / `_CHUNKS` | const | active | duplicate suffix — see cluster D |
| `AXON_GPU_READY_LOW_WATERMARK` / `_CHUNKS` | const | active | same |
| `AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT` | bool | active | |
| `AXON_GPU_RECYCLE_ON_VRAM_SUMMIT` | bool | active | |
| `AXON_GPU_RECYCLE_REQUIRED_BATCHES` | const | active | |
| `AXON_GPU_RECYCLE_VRAM_SUMMIT_MB` / `_PCT` | const | active | |
| `AXON_GPU_TELEMETRY_BACKEND` | derived | active | nvml / cli |
| `AXON_GPU_TELEMETRY_CACHE_TTL_MS` | const | active | |
| `AXON_GPU_TELEMETRY_COMMAND` | str | active | `nvidia-smi` alt |
| `AXON_GPU_TELEMETRY_DEVICE_INDEX` | 0 | active | |
| `AXON_GPU_TOTAL_VRAM_MB_HINT` | derived | active | manual hint |
| `AXON_GPU_VECTOR_EXCLUSIVE_LEASE` | bool | active | |
| `AXON_GPU_VECTOR_LEASE_PATH` | path | active | |
| `AXON_GPU_PRESSURE_EMBED_BATCH_CHUNKS` | const | active | pressure-driven sizing |
| `AXON_GPU_PRESSURE_FILES_PER_CYCLE` | const | active | |
| `AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS` | const | active | warm-up batch |
| `AXON_GPU_WARMUP_FILES_PER_CYCLE` | const | active | |
| `AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION` | bool | active | override safety |
| `AXON_NVML_LIBRARY_PATH` | path | active | NVML override |
| `AXON_CUDA_ALLOW_TF32` | bool | active | |
| `AXON_CUDA_ARCHITECTURES` | str | active | build flag |
| `AXON_CUDA_MEMORY_LIMIT_MB` / `_SOFT_LIMIT_MB` | const | active | |
| `AXON_CUDA_PACKAGE_SET` | str | active | nix package set |
| `AXON_EXPECTED_CUDA_VERSION` | str | active | preflight |
| `AXON_EXPECTED_ORT_VERSION` | str | active | preflight |
| `AXON_EXPECTED_TENSORRT_VERSION` / `_BASENAME` / `_SHA256` | str | active | preflight |
| `AXON_REQUEST_TENSORRT` | bool | active | |
| `AXON_TENSORRT_CACHE_DIR` | path | active | |
| `AXON_TENSORRT_OVERSHOOT_MB` | const | active | |
| `AXON_TENSORRT_PRECHECK_ONLY` | bool | active | |
| `AXON_TRT_PROFILE_MIN_SHAPES` / `_OPT_SHAPES` / `_MAX_SHAPES` | shape triples | active | REQ-AXO-91570 |
| `AXON_TRT_PROFILE_` | prefix | meta | scripts only |

## 6. AXON_* — ORT / ONNX Runtime

| Name | Status | Notes |
|---|---|---|
| `AXON_ORT_ARTIFACT_DIR` / `_LOG_DIR` / `_MANIFEST` | active | nix artifact dirs |
| `AXON_ORT_AUTO_THREADS` | active | |
| `AXON_ORT_BIND_OUTPUT_PER_ITER` | active | session binding |
| `AXON_ORT_BUILD_CORES` | active | rebuild parallelism |
| `AXON_ORT_INTRA_THREADS` / `_AUTOCONFIGURED` | active | |
| `AXON_ORT_MEMORY_PATTERN` | active | |
| `AXON_ORT_OMP_AUTOCONFIGURED` | active | |
| `AXON_ORT_TENSORRT_BUILD_PROFILE` | active | |

### ORT_* (non-AXON-prefixed, ONNX-runtime canonical)

| Name | Status | Notes |
|---|---|---|
| `ORT_STRATEGY` | active | `system` required for production GPU bench |
| `ORT_DYLIB_PATH` | active | dlopen target |
| `ORT_TENSORRT_ENGINE_CACHE_PATH` | active | TRT engine cache |
| All `ORT_*` matches under § 2 (e.g. `ORT_AND_VIEWBOX`, `ORT_BOTTOM`, `ORT_TOP`, …) | dead | false positives from generated SVG snippets in dashboard assets and skill docs — not env vars |

## 7. AXON_* — Resource policy + auto-config

| Name | Status | Notes |
|---|---|---|
| `AXON_RESOURCE_POLICY_CPU_CORES` / `_RAM_GB` / `_COMPUTED_INSTANCE` | active | resource_policy resolution |
| `AXON_RESOURCE_POLICY_` | prefix | meta | snapshot in env |
| `AXON_RESOURCE_PRIORITY` | derived | active | LP / NP / HP |
| `AXON_BACKGROUND_BUDGET_CLASS` | derived | active | |
| `AXON_WATCHER_POLICY` | derived | active | |
| `AXON_WATCHER_SUBTREE_HINT_BUDGET` | const | active | |
| `AXON_WATCH_DIR` | unset | active | indexer root |
| `AXON_QUEUE_MEMORY_BUDGET_BYTES` | derived | active | |
| `AXON_MEMORY_LIMIT_GB` | derived | active | |
| `AXON_MEMORY_RECLAIMER_MIN_ANON_MB` | const | active | |
| `AXON_ENABLE_MEMORY_RECLAIMER` | bool | active | |
| `AXON_EFFECTIVE_QUEUE_MEMORY_BUDGET_BYTES` | derived | active | result twin |
| `AXON_EFFECTIVE_WATCHER_SUBTREE_HINT_BUDGET` | derived | active | result twin |
| `AXON_EFFECTIVE_MAX_AXON_WORKERS` | derived | active | result twin |
| `AXON_EFFECTIVE_` | prefix | meta | export label |
| `AXON_POLICY_SOURCE_*` (8 variants) | derived | active | provenance labels printed at boot — cluster I |
| `AXON_WORKERS` | unset | active | global worker count override |

## 8. AXON_* — Optimizer (RL-style scoring, REQ-AXO-901653)

Massive surface — 50+ `AXON_OPT_*` knobs in `src/axon-core/src/optimizer.rs`.

| Group | Examples | Status |
|---|---|---|
| Reward weights | `AXON_OPT_REWARD_*` (12+) | active but **commercial-friction** : every reward dimension is its own knob |
| Score weights | `AXON_OPT_SCORE_*` (30+) | active, same friction ; cluster F |
| Guards / actuators | `AXON_OPT_ALLOWED_ACTUATORS`, `AXON_OPT_GPU_HEADROOM_MARGIN_MB`, `AXON_OPT_GPU_UNDERUTILIZED_RATIO`, `AXON_OPT_INTERACTIVE_PRIORITY_WEIGHT`, `AXON_OPT_LOOP_INTERVAL_MS`, `AXON_OPT_MAX_CPU_RATIO`, `AXON_OPT_MAX_IO_WAIT_RATIO`, `AXON_OPT_MAX_MCP_P95_MS`, `AXON_OPT_MAX_VRAM_USED_MB`, `AXON_OPT_MIN_RAM_AVAILABLE_RATIO`, `AXON_OPT_EVALUATION_WINDOW_MS`, `AXON_OPT_BACKLOG_PRIORITY_WEIGHT`, `AXON_OPT_SHADOW_MODE_ENABLED`, `AXON_OPT_WARMUP_BACKLOG_THRESHOLD` | active | individual env-knob sprawl |
| `AXON_ENABLE_SHADOW_OPTIMIZER` | active | shadow lane toggle |

## 9. AXON_* — Lifecycle / governor / NOTIFY / readers (REQ-AXO-90009)

| Name | Status | Notes |
|---|---|---|
| `AXON_GOVERNOR_MODE` | active | governor selector |
| `AXON_GOVERNOR_FREEZE_COOLDOWN_MS` | active | |
| `AXON_GOVERNOR_EMBED_STALL_MS` | active | |
| `AXON_GOVERNOR_VECTOR_HEARTBEAT_STALE_MS` | active | |
| `AXON_QUIESCENT_INTERVAL_SCALE_PCT` | active | quiescent dilation |
| `AXON_SEMANTIC_SLEEP_SCALE_PCT` / `_AUTOCONFIGURED` | active | |
| `AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT` / `_AUTOCONFIGURED` | active | |
| `AXON_READER_REFRESH_INTERVAL_MS` | active | reader-cache refresh |
| `AXON_READER_REFRESH_REQUEST_DEBOUNCE_MS` | active | |
| `AXON_READER_REFRESH_SMALL_LAG_EPOCHS` | active | |
| `AXON_IST_RAM_ENABLED` | active | IstGraphView |
| `AXON_IST_SNAPSHOT_STALE_AFTER_MS` | active | freshness gate |
| `AXON_IST_FTS_DISABLED` | active | FTS opt-out |
| `AXON_HOT_STATUS_CACHE_ENABLED` | active | status() cache |
| `AXON_HYBRID_RETRIEVAL_DISABLED` | active | RRF kill-switch |
| `AXON_RRF_ENABLED` | active | RRF on/off |
| `AXON_ENABLE_INGRESS_BUFFER` | active | ingress |
| `AXON_ENABLE_FILE_INGRESS_GUARD` | active | guard |
| `AXON_ENABLE_AUTONOMOUS_INGESTOR` | active | autonomous ingest |
| `AXON_ENABLE_FEDERATION_ORCHESTRATOR` | active | federation switch (off by default) |
| `AXON_RESULTS_BROADCAST_CAPACITY` | active | broadcast buffer |
| `AXON_RUNTIME_` | prefix | meta | snapshot |

## 10. AXON_* — Scope reconcile / SOLL / archive

| Name | Status | Notes |
|---|---|---|
| `AXON_SCOPE_RECONCILE_ENABLED` | dead | reconciliation orchestrator disabled (REQ-AXO-90009) |
| `AXON_SCOPE_RECONCILE_INTERVAL_SECS` | dead | same |
| `AXON_SOLL_BACKUP_DIR` | active | |
| `AXON_SOLL_BACKUP_RETAIN_DAYS` | active | |
| `AXON_SOLL_EXPORT_RETAIN` | active | |
| `AXON_SOLL_SEED_PATH` | active | seed migration |
| `AXON_SOLL_SITE_ROOT` | active | docs site root |
| `AXON_SEED_DIR` | active | |
| `AXON_SEEDED_MIN_SOLL_NODES` | active | min row count for seed validation |
| `AXON_ALLOW_RESERVED_ID` | active | test/dev override |
| `AXON_MUTATION_POLICY` | active | strict / permissive |
| `AXON_STRUCTURAL_HISTORY_DIR` | active | |
| `AXON_DROP_WAL_ON_STOP` | active | scripts/stop.sh — clean WAL on shutdown |
| `AXON_SKIP_SOLL_BACKUP` | active | promote scripts |
| `AXON_SKIP_BIN_SYNC` | active | promote scripts |
| `AXON_SKIP_ELIXIR_PREWARM` | active | dashboard boot |
| `AXON_SKIP_RUNTIME_BOOTSTRAP` | active | dev skip |
| `AXON_SKIP_TMP_CLEANUP` | active | dev skip |

## 11. AXON_* — MCP server / endpoints / advertised URL

| Name | Status | Notes |
|---|---|---|
| `AXON_MCP_URL` | active | local MCP URL |
| `AXON_MCP_PUBLIC_URL` | active | advertised endpoint |
| `AXON_MCP_ENDPOINT` | active | alias — see cluster C |
| `AXON_MCP_SOCK` | active | unix socket fallback |
| `AXON_MCP_FIXTURE_PATH` | test-only | fixture replay |
| `AXON_MCP_PREWARM` | active | prewarm flag |
| `AXON_MCP_PREWARM_BLOCKING` | active | blocking prewarm |
| `AXON_MCP_MUTATION_JOBS` | active | async mutation pool |
| `AXON_MCP_GUIDANCE_AUTHORITATIVE` | active | guidance mode |
| `AXON_MCP_GUIDANCE_SHADOW` | active | shadow guidance |
| `AXON_DASHBOARD_URL` | active | dashboard endpoint |
| `AXON_DASHBOARD_PUBLIC_URL` | active | dashboard advertised |
| `AXON_DASHBOARD_ENABLED` | active | enable bridge |
| `AXON_SQL_URL` / `AXON_SQL_PUBLIC_URL` | active | SQL endpoint |
| `AXON_ADVERTISED_HOST` | active | host override |
| `AXON_PUBLIC_HOST` / `AXON_PUBLIC_HOST_SOURCE` | active | host advertising |
| `AXON_PUBLIC_ENDPOINTS_AVAILABLE` | active | bool advertise |
| `AXON_CANONICAL_TCP_PORTS` / `AXON_DERIVED_TCP_PORTS` / `AXON_TCP_PORTS` | active | port discovery — see cluster G |
| `AXON_TELEMETRY_SOCK` | active | telemetry unix sock |
| `AXON_OPTIONAL_TELEMETRY_SOCKET` | active | optional sock |

## 12. AXON_* — Bench / smoke / qualify / debug

| Name | Status | Notes |
|---|---|---|
| `AXON_BENCH_*` (10 names) | test-only | bench binaries only |
| `AXON_BENCHMARK_ACTIVE` | test-only | suppress mutations during bench |
| `AXON_BENCHMARK_DB_PATH` | dead | DuckDB-era path |
| `AXON_BG_CHECKPOINT_DISABLED` | active | disable bg checkpoint loop |
| `AXON_BULK_WRITER_ENABLED` | deprecated | superseded by pipeline_v2 A3 |
| `AXON_ASYNC_WRITER_ENABLED` | deprecated | same |
| `AXON_QUALIFY_PROJECT` | active | scripts/axon qualify |
| `AXON_QUALIFY_STOP_ON_VRAM_OVERSHOOT` | active | qualify guard |
| `AXON_SMOKE_CORPUS` | test-only | smoke corpus |
| `AXON_LIVE_RELEASE_MANIFEST` | active | live release |
| `AXON_ARTIFACT_BUILD_INFO_PATH` / `_SHA256` / `_SOURCE` | active | live artifact metadata |
| `AXON_BUILD_INFO_FILE` | active | build info path |
| `AXON_STARTUP_TIMEOUT_S` | active | REQ-AXO-91570 |
| `AXON_STOP_DEBUG_MATCH` | active | stop-verify trace |
| `AXON_CLEANUP_DIR` / `_LOG` / `_LOG_MAX_BYTES` | active | scripts |
| `AXON_INDEXER_HEARTBEAT_PATH` | active | indexer heartbeat path |
| `AXON_INDEXER_PG_OPT_IN` | active | indexer PG opt-in (legacy gate) |
| `AXON_INDEXER_RUN_ROOT` | active | indexer run root |
| `AXON_NIXPKGS_SOURCE` | active | nix flake source |
| `AXON_MGCONSOLE_IMAGE` | active | Memgraph console image |
| `AXON_MEMGRAPH_LOAD_ATTEMPTS` | active | retry count |
| `AXON_PG_PLUGIN_TRACE` | active | pgvector trace |
| `AXON_PG_STATEMENT_TIMEOUT_MS` | active | pgvector statement timeout |
| `AXON_DIAG_SKIP_CHUNK_CONTENT` | active | diagnose skip body |
| `AXON_GRAY_ZONE_CHAR_THRESHOLD` | active | parser heuristic |
| `AXON_SMALL_SYMBOL_CHAR_FAST_PATH` | active | parser heuristic |
| `AXON_TEXT_PARSING_AUDIT` | active | parser audit log |
| `AXON_WRITER_GUARD_DB_ROOT` / `_HELPER_MODE` / `_READY_FILE` | active | writer guard |
| `AXON_WRITER_QUEUE_CAPACITY` | active | writer queue cap |
| `AXON_VECTOR_PIPELINE_INLINE` | deprecated | v1 |
| `AXON_SPLIT_BRAIN_IST_READER_ONLY` | active | split-brain mode |
| `AXON_SPLIT_SHADOW_ONLY` | active | shadow-only mode |

## 13. AXON_* — Dead / retired backends

| Name | Status | Notes |
|---|---|---|
| `AXON_DB_BACKEND` | dead | MIL-AXO-015 P3 used to flip postgres ; only test setup still toggles it. Should be removed and PG hardcoded. |
| `AXON_DB_ROOT` | dead | DuckDB era |
| `AXON_DUCKDB_MEMORY_LIMIT_GB` | dead | DuckDB era ; only set in `scripts/lib/start-brain.sh:10` and `scripts/dev/probe_val38.sh` |
| `AXON_AGE_DUAL_WRITE` | dead | MIL-AXO-017 retired AGE |
| `AXON_AGE_READ` | dead | same |
| `AXON_AGE_ONLY_RELATIONS` | dead | same |
| `AXON_PARQUET_CHUNK_CONTENT_ENABLED` | dead | parquet store retired |
| `AXON_PARQUET_EMBEDDING_STORE_ENABLED` | dead | same |
| `AXON_FOO` / `AXON_FOO_NEW` / `AXON_BAR` | test-only | helper test names |

## 14. Non-AXON_* env vars

| Name | Read at | Status | Notes |
|---|---|---|---|
| `PG_ACQUIRE_TIMEOUT_MS` | postgres pool | active | sqlx pool |
| `PG_MAX_CONNECTIONS` | postgres pool | active | sqlx pool |
| `DATABASE_URL` | postgres init | active | sqlx default URL |
| `HF_HOME` / `HF_ENDPOINT` | embedder | active | HuggingFace cache |
| `FASTEMBED_CACHE_DIR` | embedder | active | model cache |
| `OMP_NUM_THREADS` / `OMP_WAIT_POLICY` | ort | active | OpenMP |
| `LD_LIBRARY_PATH` | embedder boot | active | dlopen |
| `XDG_CACHE_HOME` | embedder | active | |
| `HOME` | shell-wide | active | |
| `CARGO_PKG_VERSION` | runtime_boot | active | crate version |
| `CI` | runtime_boot | active | CI hint |
| `HYDRA_HTTP_PORT` (+ `HYDRA_*` 5 ports) | scripts | active | per-host port allocation (44120s range) |
| `RUN_QWEN3_4B` / `_8B` / `_VL_2B` / `_VL_2B_IMAGE` | test runners | test-only | model selection flags |
| `RUST_LOG` | tracing-subscriber | active | log level |
| `ACTIVE` / `PASSIVE` | grep noise | dead | not env vars (constant strings) |

---

## Redundancy clusters

### Cluster A — instance kind
- `AXON_INSTANCE` (shell) + `AXON_INSTANCE_KIND` (runtime) + `AXON_LIVE` (bool) = same intent, 3 names.
- **Canonical proposal:** `AXON_INSTANCE` ∈ {`live`, `dev`}. Drop the other two.

### Cluster B — pipeline buffer caps (most painful one)
- `AXON_A3_TO_B1_BUFFER` (used by status MCP tool, default const) vs `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP` (read in pipeline_v2). Two names for one channel.
- **Canonical:** `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP`. Migrate `mcp/tools_system.rs:408` to read the canonical name with the legacy as a deprecation alias.

### Cluster C — MCP endpoint
- `AXON_MCP_URL` (local, brain side), `AXON_MCP_PUBLIC_URL` (advertised), `AXON_MCP_ENDPOINT` (sometimes used by tests).
- **Canonical:** `AXON_MCP_URL` for binding (host:port) ; `AXON_MCP_PUBLIC_URL` for external advertisement. Drop `AXON_MCP_ENDPOINT`.

### Cluster D — GPU ready watermark
- `AXON_GPU_READY_HIGH_WATERMARK` + `AXON_GPU_READY_HIGH_WATERMARK_CHUNKS` (and the LOW twins).
- The `_CHUNKS` suffix is meaningless — the units are already chunks everywhere. Pick one.

### Cluster E — workers (v1 vs v2)
- v1: `AXON_VECTOR_WORKERS`, `AXON_GRAPH_WORKERS`, `AXON_VECTOR_PRODUCERS`, `AXON_VECTOR_PERSISTERS`, `AXON_VECTOR_EMBEDDERS` (5 knobs).
- v2: `AXON_A1_WORKERS`, `AXON_A2_WORKERS`, `AXON_A3_WORKERS`, `AXON_B1_WORKERS`, `AXON_B2_WORKERS`, `AXON_B3_WORKERS` (6 knobs).
- v1 set should be removed once REQ-AXO-901653 Slice 1 completes (legacy vector_worker_loop removed).

### Cluster F — optimizer scoring (50+ knobs)
- Every reward/score dimension is its own env var.
- **Canonical:** ship a single `AXON_OPT_WEIGHTS_FILE=path.toml` and read all weights from one file. Drop the 50+ individual env vars or keep them as advanced override-only with a warning at boot.

### Cluster G — TCP port discovery
- `AXON_CANONICAL_TCP_PORTS` + `AXON_DERIVED_TCP_PORTS` + `AXON_TCP_PORTS` = three sets surfacing the same port info.
- **Canonical:** `AXON_TCP_PORTS` ; drop the other two.

### Cluster H — shell entry-point guards
- `AXON_WORKTREE_ENV_LOADED` and `AXON_ENV_VARS_LOADED` mean the same.

### Cluster I — env provenance labels
- `AXON_POLICY_SOURCE_AXON_BACKGROUND_BUDGET_CLASS`, `_AXON_EMBEDDING_PROVIDER`, `_AXON_GPU_ACCESS_POLICY`, `_AXON_QUEUE_MEMORY_BUDGET_BYTES`, `_AXON_RESOURCE_PRIORITY`, `_AXON_WATCHER_POLICY`, `_AXON_WATCHER_SUBTREE_HINT_BUDGET`, `_MAX_AXON_WORKERS` (8 vars).
- They merely echo where the matching policy var came from. Not env knobs ; they're labels the brain exports for observability.
- **Canonical:** stop pushing these into the environment ; surface them only in the `status` tool's `resource_policy.provenance` field.

### Cluster J — Pipeline v1 graph vectorization
- `AXON_GRAPH_EMBEDDINGS_ENABLED`, `AXON_GRAPH_EMBED_PROVIDER`, `AXON_ENABLE_GRAPH_VECTORIZATION` — pipeline v2 owns embeddings now ; this lane is empty.

### Cluster K — embed micro-batch
- `AXON_EMBED_MICRO_BATCH_MAX_ITEMS` + `_AUTOCONFIGURED` twin ; `AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS` + `_AUTOCONFIGURED` twin ; `AXON_EMBED_BATCH_MAX_TOTAL_TOKENS` (no twin). The `_AUTOCONFIGURED` suffix duplicates ; folding into structured logging at boot is cleaner.

---

## Obsolescence (drop entirely)

- **DuckDB era (REQ-AXO-271 / MIL-AXO-015):** `AXON_DB_BACKEND`, `AXON_DB_ROOT`, `AXON_DUCKDB_MEMORY_LIMIT_GB`, `AXON_BENCHMARK_DB_PATH`, `AXON_PARQUET_CHUNK_CONTENT_ENABLED`, `AXON_PARQUET_EMBEDDING_STORE_ENABLED`.
- **AGE era (MIL-AXO-017 / DEC-AXO-083):** `AXON_AGE_DUAL_WRITE`, `AXON_AGE_READ`, `AXON_AGE_ONLY_RELATIONS`. Smoke script `scripts/smoke-pg-migration.sh` should be archived.
- **FileVectorizationQueue table (REQ-AXO-901653):** `AXON_FILE_VECTORIZATION_BATCH_SIZE`, `AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED`.
- **Pipeline v1 vector-worker-loop (REQ-AXO-901653 Slice 1 partial — commit 2717359b removed graph_worker_loop, vector_worker_loop next):** `AXON_VECTOR_WORKERS*`, `AXON_VECTOR_PRODUCERS`, `AXON_VECTOR_PERSISTERS`, `AXON_VECTOR_EMBEDDERS`, `AXON_VECTOR_PIPELINE_STAGES`, `AXON_VECTOR_PIPELINE_INLINE`, `AXON_VECTOR_PREPARE_*`, `AXON_VECTOR_READY_QUEUE_DEPTH*`, `AXON_VECTOR_TARGET_READY_CHUNKS`, `AXON_VECTOR_PERSIST_QUEUE_BOUND*`, `AXON_VECTOR_MAX_INFLIGHT_PERSISTS*`, `AXON_VECTOR_LEASE_STALE_MS`, `AXON_VECTOR_STALE_INFLIGHT_*`, `AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS`, `AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE`, `AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING`, `AXON_GRAPH_WORKERS*`, `AXON_GRAPH_BATCH_SIZE*`, `AXON_GRAPH_EMBEDDINGS_ENABLED`, `AXON_GRAPH_EMBED_PROVIDER`, `AXON_ENABLE_GRAPH_VECTORIZATION`, `AXON_LEGACY_VECTOR_WORKER_LOOP`, `AXON_CHUNK_BATCH_SIZE*`, `AXON_BULK_WRITER_ENABLED`, `AXON_ASYNC_WRITER_ENABLED`.
- **TSV (transactional source vector — never made it past poc):** `AXON_TSV_*` (5 names).
- **Scope reconcile orchestrator (REQ-AXO-90009):** `AXON_SCOPE_RECONCILE_ENABLED`, `AXON_SCOPE_RECONCILE_INTERVAL_SECS`.
- **Test helpers leaking into source grep:** `AXON_FOO`, `AXON_FOO_NEW`, `AXON_BAR`, `AXON_TEST_SUPPORT_*` (6 names).

Total candidates for outright deletion: **≈ 55 names**.

---

## Recommended canonical set (≤ 30)

| # | Var | Role | Owner |
|---|---|---|---|
| 1 | `AXON_INSTANCE` | live/dev | shell + runtime |
| 2 | `AXON_RUNTIME_MODE` | brain_only / indexer_full / … | runtime |
| 3 | `AXON_RUNTIME_PROFILE` | resource profile | runtime |
| 4 | `AXON_PROJECT_CODE` | project override (rare ; usually auto-resolved) | runtime |
| 5 | `AXON_WATCH_DIR` | indexer source root | indexer |
| 6 | `AXON_MCP_URL` | local MCP bind | brain |
| 7 | `AXON_MCP_PUBLIC_URL` | advertised MCP | brain |
| 8 | `AXON_SQL_URL` | local SQL | brain |
| 9 | `AXON_DASHBOARD_URL` | dashboard local | dashboard |
| 10 | `AXON_DATABASE_URL` | PG canonical URL | postgres |
| 11 | `AXON_TELEMETRY_SOCK` | telemetry feed | brain |
| 12 | `AXON_TCP_PORTS` | port discovery | shell |
| 13 | `AXON_A1_WORKERS` | pipeline v2 A lane 1 | indexer |
| 14 | `AXON_A2_WORKERS` | A lane 2 | indexer |
| 15 | `AXON_A3_WORKERS` | A lane 3 (writer) | indexer |
| 16 | `AXON_B1_WORKERS` | B lane 1 (claim) | indexer |
| 17 | `AXON_B2_WORKERS` | B lane 2 (GPU embed) | indexer |
| 18 | `AXON_B3_WORKERS` | B lane 3 (persist) | indexer |
| 19 | `AXON_PIPELINE_A3_TO_B1_BUFFER_CAP` | try_send buffer | pipeline |
| 20 | `AXON_EMBED_MICRO_BATCH_MAX_ITEMS` | GPU micro-batch sizing | embedder |
| 21 | `AXON_GPU_ACCESS_POLICY` | shared / exclusive | embedder |
| 22 | `AXON_GPU_TELEMETRY_BACKEND` | nvml / cli | embedder |
| 23 | `AXON_TRT_PROFILE_OPT_SHAPES` | TRT shapes (3 shape vars collapse into one) | embedder |
| 24 | `AXON_RESOURCE_PRIORITY` | LP / NP / HP | runtime |
| 25 | `AXON_QUEUE_MEMORY_BUDGET_BYTES` | indexer queue budget | runtime |
| 26 | `AXON_LIVE_RELEASE_MANIFEST` | live release manifest | deploy |
| 27 | `AXON_ORT_ARTIFACT_DIR` | ORT artifact bundle | embedder |
| 28 | `AXON_STARTUP_TIMEOUT_S` | startup deadline | runtime |
| 29 | `AXON_HOT_STATUS_CACHE_ENABLED` | status fast-path | mcp |
| 30 | `AXON_OPT_WEIGHTS_FILE` (new) | TOML weights file replacing 50 `AXON_OPT_*` env vars | optimizer |

Everything else (≈ 380 names) is either dead, deprecated, derived-twin, internal observability label, or test-only. Build / nix / scripts variables (`HYDRA_*`, `ORT_*`, `PG_*`, `OMP_*`, `HF_*`, …) are infrastructure-canonical and not in this 30-var set ; they are documented separately in `devenv.nix`.
