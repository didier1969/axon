# REQ-AXO-290 Slice S1 — Env Var Inventory (2026-05-12 session 17)

**173 distinct `AXON_*` env vars** are read by `src/axon-core/src/**/*.rs` via `std::env::var(...)`.
This is the operator-flagged sprawl. This doc is the first-pass inventory + classification framework
for REQ-AXO-290. Next-session work = (1) drill per-var to confirm, (2) ship the two
env config templates, (3) delete the obsolete reads from code.

## Classification rubric

- **KEEP** — has a real effect under PG + streaming pipeline v2, operator-tunable, appears in
  `axon-dev.env.template` or `axon-live.env.template`.
- **RENAME** — has a real effect but the name predates v2 vocabulary; rename in v2 cut-over.
- **DELETE-DUCKDB** — DuckDB-era residue, will be deleted alongside `axon-plugin-duckdb` retirement (REQ-AXO-289 slice S7-S8).
- **DELETE-V2** — superseded by streaming v2 architecture (e.g. tuning knobs for the
  state-machine that no longer exists).
- **DELETE-DEAD** — no effective consumer / always overridden / vestigial.
- **INTERNAL** — set by the runtime itself (not user-facing); document but don't expose.

## First-pass classification (session 17, to be confirmed slice S1 follow-up)

### KEEP — runtime topology (operator surface)
| Var | Comment |
|---|---|
| `AXON_RUNTIME_MODE` | brain_only / indexer_graph / indexer_vector / indexer_full (CPT-AXO-053) |
| `AXON_INSTANCE_KIND` | dev / live tagging |
| `AXON_INDEXER_PG_OPT_IN` | PG indexer gate; delete after v2 cut-over (slice S8) |
| `AXON_DB_BACKEND` | postgres canonical; delete after duckdb plugin retirement |

### KEEP — database
| Var | Comment |
|---|---|
| `AXON_LIVE_DATABASE_URL` | live PG URL |
| `AXON_DEV_DATABASE_URL` | dev PG URL |
| `AXON_DB_ROOT` | DB filesystem root (may rename for v2 if scope changes) |
| `AXON_SOLL_SEED_PATH` | SOLL seed bootstrap |
| `AXON_STRUCTURAL_HISTORY_DIR` | revisit usefulness post-v2 |

### KEEP — paths + identity (mostly internal)
| Var | Comment |
|---|---|
| `AXON_PROJECTS_ROOT`, `AXON_PROJECT_ROOT`, `AXON_WATCH_DIR` | scope roots |
| `AXON_RUN_ROOT`, `AXON_INDEXER_RUN_ROOT` | runtime working dirs |
| `AXON_RUNTIME_IDENTITY` | service identity tag |
| `AXON_TELEMETRY_SOCK`, `AXON_MCP_SOCK` | local socket paths |
| `AXON_BUILD_ID`, `AXON_INSTALL_GENERATION`, `AXON_PACKAGE_VERSION`, `AXON_RELEASE_VERSION` | metadata (set by build, read for telemetry) |
| `AXON_PUBLIC_HOST`, `AXON_PUBLIC_HOST_SOURCE`, `AXON_PUBLIC_ENDPOINTS_AVAILABLE`, `AXON_MCP_PUBLIC_URL`, `AXON_MCP_URL`, `AXON_SQL_URL`, `AXON_SQL_PUBLIC_URL`, `AXON_DASHBOARD_URL`, `AXON_DASHBOARD_PUBLIC_URL` | service endpoint advertisement |

### KEEP — resource governance (PIL-AXO-006)
| Var | Comment |
|---|---|
| `AXON_RESOURCE_PRIORITY` | critical / best_effort |
| `AXON_BACKGROUND_BUDGET_CLASS` | balanced / conservative |
| `AXON_GPU_ACCESS_POLICY` | preferred / avoid |
| `AXON_WATCHER_POLICY` | full / bounded |
| `AXON_MEMORY_LIMIT_GB` | RSS budget |
| `AXON_CUDA_MEMORY_LIMIT_MB`, `AXON_CUDA_MEMORY_SOFT_LIMIT_MB` | VRAM budget (rename to AXON_VRAM_BUDGET_MB ?) |

### KEEP — vectorization (preserved into v2 stages B1-B3)
| Var | Comment |
|---|---|
| `AXON_EMBEDDING_PROVIDER` | cpu / cuda / tensorrt |
| `AXON_QUERY_EMBED_PROVIDER` | brain-side query embedder EP |
| `AXON_GRAPH_EMBED_PROVIDER` | graph embedding EP (verify still used) |
| `AXON_EMBEDDER_SEQ_BUCKETS` | REQ-AXO-262 bucketing |
| `AXON_EMBED_MAX_LENGTH` | model max seq |
| `AXON_TARGET_CHUNK_TOKENS`, `AXON_CHUNK_OVERLAP_TOKENS` | chunking strategy |
| `AXON_TENSORRT_CACHE_DIR` | TRT engine cache |
| `AXON_TRT_PROFILE_MIN_SHAPES`, `AXON_TRT_PROFILE_OPT_SHAPES`, `AXON_TRT_PROFILE_MAX_SHAPES` | TRT optimization profile |

### RENAME — to per-stage v2 worker pools (REQ-AXO-289 slice S1)
| Old var | Proposed | Comment |
|---|---|---|
| `AXON_GRAPH_WORKERS` | split into `AXON_A1_WORKERS` + `AXON_A2_WORKERS` + `AXON_A3_WORKERS` | per CPT-AXO-054 |
| `AXON_VECTOR_WORKERS` | split into `AXON_B1_WORKERS` + `AXON_B2_WORKERS` + `AXON_B3_WORKERS` | same |
| `AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR` | absorbed into `AXON_B1_WORKERS` | |
| `AXON_CHUNK_BATCH_SIZE`, `AXON_FILE_VECTORIZATION_BATCH_SIZE`, `AXON_GRAPH_BATCH_SIZE` | retire — bounded channel cap replaces batch concept | |

### DELETE-DUCKDB (eliminated with DuckDB plugin retirement)
| Var | Comment |
|---|---|
| `AXON_DUCKDB_MEMORY_LIMIT_GB` | DuckDB only |
| `AXON_BG_CHECKPOINT_DISABLED` | DuckDB checkpoint behavior |
| `AXON_BULK_WRITER_ENABLED` | Writer Actor / DuckDB-era |
| `AXON_WRITER_GUARD_DB_ROOT` | DuckDB writer guard helper |
| `AXON_WRITER_GUARD_HELPER_MODE` | same |
| `AXON_WRITER_GUARD_READY_FILE` | same |
| `AXON_WRITER_QUEUE_CAPACITY` | Writer Actor capacity |
| `AXON_SPLIT_BRAIN_IST_READER_ONLY` | DuckDB single-writer workaround |
| `AXON_SPLIT_SHADOW_ONLY` | DuckDB shadow split |
| `AXON_RUNTIME_SHADOW_ROLE` | DuckDB shadow role |

### DELETE-V2 (state-machine knobs that no longer apply)
| Var | Comment |
|---|---|
| `AXON_QUEUE_MEMORY_BUDGET_BYTES` | replaced by bounded channels |
| `AXON_WATCHER_SUBTREE_HINT_BUDGET` | subtree_hints subsystem retired with v2 watcher |
| `AXON_ENABLE_AUTONOMOUS_INGESTOR` | autonomous_ingestor retired |
| `AXON_ENABLE_FEDERATION_ORCHESTRATOR` | revisit — may keep federation w/o legacy mech |
| `AXON_ENABLE_MEMORY_RECLAIMER` | tied to admission controller, retired |
| `AXON_ENABLE_SHADOW_OPTIMIZER` | tied to shadow path |
| `AXON_GPU_READY_HIGH_WATERMARK`, `AXON_GPU_READY_LOW_WATERMARK`, `*_CHUNKS` | DuckDB-era admission |
| `AXON_GPU_PRESSURE_EMBED_BATCH_CHUNKS`, `AXON_GPU_PRESSURE_FILES_PER_CYCLE` | DuckDB-era pressure heuristics |
| `AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS`, `AXON_GPU_WARMUP_FILES_PER_CYCLE` | warmup tied to admission |
| `AXON_VECTOR_PERSIST_QUEUE_BOUND`, `*_AUTOCONFIGURED` | persister subsystem retired in favor of B3 worker pool |
| `AXON_VECTOR_MAX_INFLIGHT_PERSISTS`, `*_AUTOCONFIGURED` | same |
| `AXON_VECTOR_PIPELINE_INLINE` | DuckDB-era inline-vs-pipeline switch |
| `AXON_VECTOR_PREPARE_PIPELINE_DEPTH` | replaced by channel cap |
| `AXON_VECTOR_READY_QUEUE_DEPTH`, `*_AUTOCONFIGURED` | replaced by channel cap |
| `AXON_VECTOR_TARGET_READY_CHUNKS` | DuckDB-era target |
| `AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS` | claim subsystem retired |
| `AXON_VECTOR_LEASE_STALE_MS`, `AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS`, `AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS` | lease subsystem retired |
| `AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE` | DuckDB-era heuristic |
| `AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING` | revisit if symbol-level embeddings supported under v2 |
| `AXON_GOVERNOR_MODE`, `AXON_GOVERNOR_FREEZE_COOLDOWN_MS` | governor subsystem retired |
| `AXON_HOT_STATUS_CACHE_ENABLED` | status caching tied to DuckDB read latency |
| `AXON_READER_REFRESH_INTERVAL_MS`, `AXON_READER_REFRESH_REQUEST_DEBOUNCE_MS`, `AXON_READER_REFRESH_SMALL_LAG_EPOCHS` | reader-snapshot-refresher retired with single-writer mutex |
| `AXON_IST_SNAPSHOT_STALE_AFTER_MS` | snapshot subsystem retired |
| `AXON_QUIESCENT_INTERVAL_SCALE_PCT` | idle interval scaling tied to admission |
| `AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT`, `AXON_SEMANTIC_SLEEP_SCALE_PCT`, `*_AUTOCONFIGURED` | semantic sleep tied to claim_policy |
| `AXON_GPU_PRE_BATCH_VRAM_GUARD_*` | (5 vars) VRAM guard subsystem, simplifiable |
| `AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED`, `AXON_GPU_PRIMARY_WORKER_MAX_USED_MB` | primary worker guard tied to lease |
| `AXON_GPU_VECTOR_EXCLUSIVE_LEASE`, `AXON_GPU_VECTOR_LEASE_PATH` | exclusive lease tied to mutex |
| `AXON_GPU_RECYCLE_*` | (4 vars) GPU recycle heuristics, revisit |
| `AXON_GPU_MULTIWORKER_MIN_FREE_MB` | tied to admission |
| `AXON_GPU_TELEMETRY_BACKEND`, `*_CACHE_TTL_MS`, `*_COMMAND`, `*_DEVICE_INDEX` | telemetry backend, may keep but simplify |
| `AXON_GPU_TOTAL_VRAM_MB_HINT` | hint for admission, retired |
| `AXON_GPU_EMBED_SERVICE_ENABLED`, `*_RECYCLE_EVERY_BATCH`, `*_TENSORRT` | GPU embed subprocess service, replaced by in-process B2 worker |
| `AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION` | admission-related |
| `AXON_OPTIONAL_TELEMETRY_SOCKET` | optional, revisit |
| `AXON_MAX_EMBED_BATCH_BYTES`, `AXON_EMBED_BATCH_MAX_TOTAL_TOKENS`, `AXON_EMBED_MICRO_BATCH_MAX_ITEMS`, `AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS`, `AXON_EMBED_TOKEN_BUCKET_SIZE` | token-cap micro-batching subsystem (CPT-AXO-026 layer 10) retired |
| `AXON_OPT_*` | (3 vars) optimizer config tied to shadow optimizer |
| `AXON_MCP_PREWARM`, `AXON_MCP_PREWARM_BLOCKING`, `AXON_MCP_GUIDANCE_AUTHORITATIVE`, `AXON_MCP_GUIDANCE_SHADOW` | revisit relevance under v2 |
| `AXON_RUNTIME_TRACE_ENABLED`, `*_INTERVAL_MS`, `*_PATH` | runtime trace logger retired (per-stage metrics replace) |
| `AXON_PIPELINE_TRACE_CSV` | replaced by `AXON_METRICS_EXPORT_CSV` per REQ-AXO-290 |
| `AXON_RUNTIME_REACTIVATION_PATH` | dead |
| `AXON_GRAY_ZONE_CHAR_THRESHOLD` | DuckDB-era heuristic |
| `AXON_AGE_DUAL_WRITE`, `AXON_AGE_ONLY_RELATIONS`, `AXON_AGE_READ` | (3 vars) AGE dual-write migration toggle — verify if still needed post-cut-over |
| `AXON_MUTATION_POLICY`, `AXON_MCP_MUTATION_JOBS` | revisit |
| `AXON_RUNTIME_COMMAND_PROXY_ENABLED`, `*_TEST_PANIC` | proxy subsystem, revisit |
| `AXON_RESULTS_BROADCAST_CAPACITY` | revisit |

### DELETE-DEAD (no consumer or always overridden)
| Var | Comment |
|---|---|
| `AXON_BENCH_PROFILING_PATH`, `AXON_BENCH_TEXT_COUNT` | bench-only, may keep if benchmarks need |
| `AXON_EMBEDDING_DOWNLOAD_PROGRESS`, `AXON_EMBEDDING_GPU_PRESENT`, `AXON_EMBEDDING_PROVIDER_EFFECTIVE`, `AXON_EMBEDDING_PROVIDER_INIT_ERROR` | internal-set state vars (not user-facing) |
| `AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED`, `AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED`, `AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED`, `AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED`, `AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED`, `AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED`, `AXON_ORT_INTRA_THREADS_AUTOCONFIGURED`, `AXON_ORT_OMP_AUTOCONFIGURED`, `AXON_GRAPH_WORKERS_AUTOCONFIGURED`, `AXON_SEMANTIC_*_AUTOCONFIGURED` | (10+ vars) internal flags marking auto-config; not operator-facing, delete after their owning subsystems retire |
| `AXON_ORT_AUTO_THREADS`, `AXON_ORT_INTRA_THREADS`, `AXON_ORT_BIND_OUTPUT_PER_ITER`, `AXON_ORT_MEMORY_PATTERN` | ORT tuning, revisit relevance |
| `AXON_SMALL_SYMBOL_CHAR_FAST_PATH` | tree-sitter heuristic — verify |
| `AXON_NVML_LIBRARY_PATH` | NVML override, keep only if explicit deploy needs |
| `AXON_SOLL_EXPORT_RETAIN` | revisit |
| `AXON_MEMORY_RECLAIMER_MIN_ANON_MB` | tied to memory_reclaimer (retired) |

### INTERNAL (set by runtime, not by operator)
- `AXON_BUILD_ID`, `AXON_INSTALL_GENERATION`, `AXON_PACKAGE_VERSION`, `AXON_RELEASE_VERSION` (set by build pipeline)
- `AXON_EMBEDDING_PROVIDER_EFFECTIVE`, `AXON_EMBEDDING_PROVIDER_INIT_ERROR`, `AXON_EMBEDDING_GPU_PRESENT`, `AXON_EMBEDDING_DOWNLOAD_PROGRESS` (set by embedder init)
- `AXON_*_AUTOCONFIGURED` (set by autoconfig at boot)

## Summary numbers (first-pass, to be confirmed)

- 173 total `AXON_*` env vars in `src/`.
- **~30 KEEP** (operator surface, post-v2).
- **~10 RENAME** for v2 per-stage workers.
- **~10 DELETE-DUCKDB** (writer guard, shadow, plugin-specific).
- **~80 DELETE-V2** (state-machine knobs, admission, persister subsystem, micro-batch).
- **~25 DELETE-DEAD or INTERNAL** (auto-configured, init-state, vestigial).
- Remaining ~20 = need per-var read of the consumer code to classify.

Net reduction target: ~173 → ~30 operator-facing knobs (~85% sprawl reduction).

## Next-session checklist

1. For each var in DELETE-* categories: grep all consumers, confirm they're dead under v2, delete the read + the consumer code.
2. For each var in RENAME: implement the new name, deprecation period if backwards-compat needed (probably not — single user).
3. Ship the two templates (`config/axon-dev.env.template` + `config/axon-live.env.template`) with the ~30 KEEP vars + comments.
4. Update CLAUDE.md to reference the templates.
5. Run `cargo build --lib` + verify lib tests still green after deletions.

Cross-references: REQ-AXO-290 (this slice), REQ-AXO-289 (parent umbrella), CPT-AXO-053 (architecture),
CPT-AXO-054 (implementation contract).
