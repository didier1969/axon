# Axon KPI / Metric Inventory — 2026-05-22 (Session 52)

**Auditor:** Claude (Opus 4.7 · session 52) on operator request.
**Scope:** all metric-bearing surfaces exposed by `axon-brain` (MCP server), `axon-indexer`, the dashboard, and the telemetry socket.
**Mechanical baseline:**
- `src/axon-core/src/service_guard.rs` : **138 `AtomicU64` counters**, **113 `pub fn` accessors** (records + readers).
- `src/axon-core/src/mcp/tools_framework_runtime_status.rs` : **241 unique JSON field keys** in the `status` tool output.
- `src/axon-core/src/mcp/tools_system.rs` `axon_embedding_status` : **21 fields** (Storage + Pipeline A + Pipeline B + lifecycle).
- `src/axon-core/src/mcp/tools_governance.rs` `axon_diagnose_indexing` : ~12 causes, 4 raw tables, free-form markdown.
- Dashboard `src/dashboard/lib/axon_dashboard_web/live/pipeline_live.ex` : assigns `:heartbeat`, `:mcp`, `:rate_series` ; KPIs derived from MCP `embedding_status` + telemetry socket.
- `src/axon-core/src/main_telemetry.rs` : 1 unix-socket consumer, publishes runtime_truth_bridge_dispatch + current_runtime_truth_feed.

**Companion doc:** `docs/audits/2026-05-22-env-vars-inventory.md`.

Status legend :
- **canonical** = relied on by LLM clients / dashboard / operator.
- **internal** = useful at debug time, not part of the public contract.
- **dead** = tied to retired code path (graph_worker_loop, vector_worker_loop, FileVectorizationQueue) ; still emitted, no longer meaningful.
- **redundant** = same physical quantity exposed under a different name on the same response.

---

## 1. MCP `status` (tools_framework_runtime_status.rs) — top-level fields

| Field | Source | Computed how | Status | Notes |
|---|---|---|---|---|
| `truth_status` | runtime_authority resolver | `canonical` / `degraded` based on indexer heartbeat freshness | canonical | LLM gate |
| `truth_cockpit.current_blocker` | `degraded_notes.first()` | first degraded reason | canonical | LLM action driver |
| `truth_cockpit.next_best_action` | resolver | command suggestion | canonical | |
| `truth_cockpit.recovery_hint` | resolver | hint string | canonical | |
| `truth_cockpit.staleness` | resolver | `N file(s) modified since last publish` | canonical | |
| `truth_cockpit.freshness.state` | `fresh` / `degraded` | redundant with `truth_status` | redundant | same signal, two names |
| `truth_cockpit.freshness.truth_status` | copy of root `truth_status` | redundant | redundant | |
| `truth_cockpit.confidence` | hard-coded `"high"` | internal | always `high` |
| `truth_cockpit.proof_gaps` | `["fresh_indexed_projection"]` if degraded | internal | |
| `truth_cockpit.llm_instruction` | static string | canonical | LLM prompt fragment |
| `machine_status` | mirror of root | bundles `source`, `process_role`, `freshness_state` | canonical | |
| `runtime_mode` | env-resolved | brain_only / indexer_* | canonical | |
| `runtime_profile` | env-resolved | profile string | canonical | |
| `drain_state` | utility scheduler | quiet_cruise / draining / etc. | canonical | |
| `availability.ist_projection_fresh` | freshness gate | bool | canonical | preferred name (REQ-AXO-106) |
| `availability.advanced_indexed_surfaces_visible` | alias | bool | redundant | legacy alias kept for back-compat |
| `availability.degraded_notes` | resolver | string[] | canonical | |
| `readiness` | runtime_readiness::snapshot | tristate | canonical | DEC-AXO-062 |
| `subsystems[]` | runtime_readiness::snapshot | per-subsystem report | canonical | |
| `instance_identity.instance_kind` | env | string | canonical | |
| `instance_identity.runtime_identity` | env | string | canonical | |
| `instance_identity.auto_detected_project` | cwd resolver | string | canonical | |
| `instance_identity.data_root` / `.data_root_absolute` | env | path | canonical | |
| `instance_identity.run_root` / `.project_root` | env | path | canonical | |
| `instance_identity.mcp_url` / `.sql_url` / `.dashboard_url` | env | URL | canonical | |
| `instance_identity.mutation_policy` | env | strict / permissive | canonical | |
| `instance_identity.session_pointer` | SOLL resolver | SOLL ID (CPT-AXO-052) | canonical | |
| `advertised_endpoints.*` | env | URL set | canonical | duplicates `instance_identity.*_url` for external callers |
| `client_reachability_notes.*` | hard-coded | string | internal | hint block |
| `resource_policy.resource_priority` | env | string | canonical | |
| `resource_policy.background_budget_class` | env | string | canonical | |
| `resource_policy.gpu_access_policy` | env | string | canonical | |
| `resource_policy.watcher_policy` | env | string | canonical | |
| `resource_policy.embedding_provider` | env | string | canonical | |
| `resource_policy.vector_workers` / `.graph_workers` | env | int | **dead** | v1 worker counts ; v2 lanes ignore them |
| `runtime_authority.runtime_state` | resolver | bag | internal | rich debug |
| `runtime_authority.vector_pipeline_telemetry` | service_guard | bag (50+ subfields, see § 2) | internal | huge ; mostly internal |
| `runtime_authority.loop_semantics_snapshot` | hard-coded | bag | internal | |
| `runtime_authority.canonical_ingestion_stage_model` | runtime model | bag | internal | |
| `runtime_authority.admission_controller` | service_guard | bag | internal | |
| `runtime_authority.canonical_edges` | constants | bag | internal | |
| `runtime_authority.priority_contract` | constants | bag | internal | |
| `runtime_authority.lane_parameters` | env | bag | internal | |
| `runtime_authority.quiescent_state` | service_guard | bag | internal | |
| `runtime_authority.limiting_factors` | resolver | bag | internal | |
| `runtime_version.release_version` / `.package_version` / `.build_id` / `.install_generation` | env | string | canonical | |
| `file_vectorization_queue.queued` / `.inflight` | service_guard | int | **dead** | FileVectorizationQueue table dropped ; counts always 0 |
| `utility_first_scheduler.state` / `.reason` | scheduler | string | canonical | |
| `utility_first_scheduler.semantic_underfeed` | scheduler | bool | canonical | |
| `utility_first_scheduler.ready_reserve_target` | env | int | internal | |
| `utility_first_scheduler.target_ready_chunks` | env | int | internal | |
| `utility_first_scheduler.hold_window_ms` | env | int | internal | |
| `utility_first_scheduler.orphan_vectorization_files` | sql | int | canonical | |
| `utility_first_scheduler.stale_vector_inflight_files` | sql | int | canonical | |
| `utility_first_scheduler.oldest_graph_pending_age_ms` | sql | int | canonical | |
| `utility_first_scheduler.oldest_semantic_pending_age_ms` | sql | int | canonical | |
| `public_tools[]` | mcp registry | string[] | canonical | LLM-facing |
| `async_policy.*` (5 keys) | mcp registry | bag | canonical | |
| `canonical_sources` | snapshot | bag | canonical | |

Total observed top-level keys : **~70 first-level + 170 nested = 241 unique**.

## 2. `runtime_authority.vector_pipeline_telemetry` (service_guard surface)

| Field | Status | Notes |
|---|---|---|
| `chunks_embedded_total` | canonical | the headline throughput counter |
| `chunks_inferred_total` | redundant | input texts ≈ output chunks ; rarely diverges |
| `chunk_embeddings_per_second` | canonical | windowed rate |
| `chunk_embeddings_rate_window_ms` | internal | window size |
| `graph_workers_started_total` | **dead** | graph_worker_loop removed in commit 2717359b |
| `graph_workers_active_current` | **dead** | always 0 |
| `graph_worker_heartbeat_at_ms` | **dead** | last heartbeat from removed loop |
| `vector_workers_started_total` | **dead** | vector_worker_loop pending removal (REQ-AXO-901653 Slice 1) |
| `vector_workers_active_current` | **dead** | always 0 |
| `vector_worker_heartbeat_at_ms` | **dead** | |
| `vector_worker_restarts_total` | **dead** | |
| `ready_queue_chunks_current` | canonical | depth signal |
| `ready_queue_chunks_small` / `_medium` / `_large` | internal | lane breakdown |
| `ready_batches_small` / `_medium` / `_large` / `_mixed` | internal | lane breakdown |
| `prepare_inflight_chunks_current` | internal | |
| `ready_replenishment_deficit_current` | internal | |
| `oldest_ready_batch_age_ms_current` | canonical | latency signal |
| `homogeneous_batches_total` / `mixed_fallback_batches_total` | internal | |
| `last_consumed_batch_lane` | internal | string label |
| `active_small_max_tokens` / `active_medium_max_tokens` | internal | bucket sizes |
| `avg_embed_attempt_wall_ms` / `avg_embed_gap_ms` | canonical | latency averages |
| `tensorrt_engine_cache_hit` | canonical | TRT cache hit |
| `vector_lane_state` | canonical | enum lane state |
| `bridge_*` (n keys) | internal | dispatch counters |
| `vector_fetch_ms_total` / `vector_embed_ms_total` / `vector_db_write_ms_total` / `vector_completion_check_ms_total` / `vector_mark_done_ms_total` | **dead** | v1 stage timings ; stages removed in pipeline_v2 |
| `vector_batches_total` / `vector_chunks_embedded_total` / `vector_files_completed_total` / `vector_embed_calls_total` / `vector_claimed_work_items_total` / `vector_partial_file_cycles_total` / `vector_mark_done_calls_total` / `vector_files_touched_total` | **mostly dead** | only `chunks_embedded_total` should survive |
| `vector_prepare_dispatch_total` / `_prepare_prefetch_total` / `_prepare_fallback_inline_total` / `_prepared_work_items_total` / `_prepare_empty_batches_total` / `_prepare_immediate_completed_total` / `_prepare_failed_fetches_total` | **dead** | v1 prepare stage retired |
| `vector_finalize_enqueued_total` / `_finalize_fallback_inline_total` | **dead** | v1 finalize stage retired |
| `vector_prepare_reply_wait_ms_total` / `_prepare_send_wait_ms_total` / `_finalize_send_wait_ms_total` / `_prepare_queue_wait_ms_total` | **dead** | v1 queue timings |
| `background_launches_suppressed_total` / `vectorization_suppressed_total` / `projection_suppressed_total` / `vectorization_interrupted_total` / `vectorization_requeued_for_interactive_total` / `vectorization_resumed_after_interactive_total` | canonical | interactive-priority gating events |
| `last_sql_latency_ms` / `last_mcp_latency_ms` / `last_mcp_sample_at_ms` / `last_sample_at_ms` / `last_degraded_at_ms` / `interactive_requests_in_flight` / `last_interactive_at_ms` | canonical | freshness + load signal |

Net : **~120 fields nested under `vector_pipeline_telemetry`**, of which only ≈ 15 are operator-meaningful post-pipeline-v2.

## 3. MCP `embedding_status` (tools_system.rs `axon_embedding_status`)

| Field | Source | Status | Notes |
|---|---|---|---|
| `project` | arg | canonical | |
| `symbols` | `SELECT count(*) FROM public.Symbol` | canonical | |
| `total_chunks` | `count(*) public.Chunk` | canonical | |
| `embedded_chunks` | `count(*) public.ChunkEmbedding` | canonical | |
| `pending_chunks` | `total_chunks - embedded_chunks` | canonical | derived |
| `coverage_pct` | derived | canonical | |
| `edges` | `count(*) public.Edge` | canonical | |
| `indexed_files` | `count(*) public.IndexedFile` | canonical | |
| `projects` | `count(*) public.Project` | canonical | |
| `pipeline_a.{a1,a2,a3,a3_batch_size,a3_batch_timeout_ms}` | env | canonical | env-resolved by RESPONDING process — brain vs indexer can differ |
| `pipeline_b.{b1,b2,b3,b2_batch_size,b2_batch_timeout_ms,b3_batch_size,b3_batch_timeout_ms,a3_to_b1_buffer_cap,coldstart_batch_size}` | env | canonical | same caveat |
| `notify_channel` | hard-coded `chunk_pending_embed` | canonical | |
| `coldstart_poll_interval_secs` | hard-coded `30` | canonical | |
| `runtime_pending_count` | `EmbedderRuntimeState::pending_count()` | canonical | in-memory ; should track `pending_chunks` |
| `runtime_idle` | bool | canonical | derived |
| `lifecycle_phase` | indexer heartbeat OR local singleton | canonical | REQ-AXO-91572 — when brain alone replies, falls back to brain singleton which is uninformative |
| `lifecycle_last_used_ms` | heartbeat | canonical | |
| `lifecycle_wake_count` | heartbeat | canonical | |
| `lifecycle_sleep_count` | heartbeat | canonical | |
| `lifecycle_source` | `"indexer_heartbeat"` / `"brain_local_singleton"` | canonical | provenance |
| `lifecycle_heartbeat_age_ms` | derived | canonical | |

Total : **21 fields**, only the 4 lifecycle ones are post-pipeline-v2 additions ; the rest is the original cockpit.

## 4. MCP `diagnose_indexing` (tools_governance.rs)

Output : markdown report only, no `structuredContent`. Internals (used to compose) :
- 5 SQL scalars : `File count`, `pending`, `indexing`, `indexed*`, `Symbol count`.
- 2 SQL group-by : top 5 `status_reason` + top 5 `last_error_reason`.
- Skipped legacy `CALLS` / `CALLS_NIF` tables (always 0 post-Stop-A).
- Cause selector (10 patterns) :
  - `watch_root_unconfigured`
  - `runtime_mode_excludes_indexing`
  - `path_not_in_runtime_registry`
  - `discovery_absent_or_filtered`
  - `ingestion_not_completed`
  - `file_too_large_for_budget`
  - `parser_extraction_gap`
  - `call_graph_gap` (uses legacy CALLS tables — **dead** post-Stop-A)
  - `no_blocker_detected`

Action : add `structuredContent` block mirroring the markdown ; drop `call_graph_gap` and the SQL probes against `CALLS` / `CALLS_NIF`.

## 5. `main_telemetry` (unix socket consumer)

Source : `src/axon-core/src/main_telemetry.rs` (255 LoC).

Surfaces :
- `service_guard::record_runtime_truth_bridge_dispatch(None)` — heartbeat write.
- `service_guard::current_runtime_truth_feed()` — read of the current feed buffer.
- Listens on `AXON_TELEMETRY_SOCK` or `AXON_OPTIONAL_TELEMETRY_SOCKET`.
- Connects out to dashboard (Elixir-side `axon_dashboard_web/telemetry.ex` consumer) via Phoenix `bridge_client.ex`.

Status : **canonical**. This is the one path the dashboard's `:heartbeat` field comes from.

## 6. `service_guard` counters (138 atomic counters)

Grouped count :
- `LAST_*_LATENCY_MS`, `LAST_*_AT_MS`, `LAST_DEGRADED_AT_MS` (timestamp/latency probes) : ~10 counters — **canonical**.
- `BACKGROUND_LAUNCHES_SUPPRESSED_TOTAL`, `VECTORIZATION_SUPPRESSED_TOTAL`, `PROJECTION_SUPPRESSED_TOTAL`, `VECTORIZATION_INTERRUPTED_TOTAL`, `VECTORIZATION_REQUEUED_FOR_INTERACTIVE_TOTAL`, `VECTORIZATION_RESUMED_AFTER_INTERACTIVE_TOTAL` : 6 counters — **canonical** (interactive-priority gating).
- `VECTOR_*_MS_TOTAL` + `VECTOR_*_TOTAL` + `VECTOR_*_CALLS_TOTAL` : ~25 counters tracking v1 vector-worker-loop stages — **mostly dead**, kept only because `vector_worker_loop` is not yet removed.
- `VECTOR_PREPARE_*` + `VECTOR_FINALIZE_*` + `VECTOR_PREPARED_WORK_ITEMS_TOTAL` etc : ~12 counters for v1 prepare/finalize stage — **dead**.
- `GRAPH_WORKERS_*` : 3 counters — **dead** (graph_worker_loop removed in commit 2717359b).
- `VECTOR_WORKERS_*` : 3 counters — **dead-on-removal** of `vector_worker_loop`.
- `BRIDGE_*` (runtime_truth_bridge stats) : ~10 counters — internal, dashboard plumbing.
- `READY_QUEUE_*`, `READY_BATCHES_*`, `ACTIVE_*_MAX_TOKENS`, `HOMOGENEOUS_BATCHES_TOTAL`, `MIXED_FALLBACK_BATCHES_TOTAL`, `LAST_CONSUMED_BATCH_LANE` : ~12 counters — canonical (lane-routing observability).
- `EMBED_INPUT_TEXTS_TOTAL`, `EMBED_INPUT_BYTES_TOTAL`, `EMBED_CLONE_MS_TOTAL`, `EMBED_TRANSFORM_MS_TOTAL`, `EMBED_EXPORT_MS_TOTAL`, `EMBED_ATTEMPT_*` : ~10 counters — canonical (embedder-internal).
- `TENSORRT_ENGINE_CACHE_HIT` etc : ~3 counters — canonical.
- The rest (~50) : various rare/internal probes.

Of the 138 counters, **roughly 45 are tied to retired code paths**. The active canonical set is **~25**.

## 7. Dashboard surface (Elixir)

- `src/dashboard/lib/axon_dashboard_web/live/pipeline_live.ex` — Pipeline cockpit. Pulls `:heartbeat` from telemetry socket + `:mcp` from `embedding_status` poll. Exposed KPIs (operator-visible) :
  - `total_chunks` / `embedded_chunks` / `coverage_pct` / `pending_chunks` (from MCP).
  - `pipeline_a.{a1,a2,a3}` / `pipeline_b.{b1,b2,b3}` worker counts.
  - `lifecycle_phase` / `wake_count` / `sleep_count`.
  - `utility_first_scheduler_state` (from heartbeat).
  - Per-second rate series (computed dashboard-side from successive `embedded_chunks` snapshots).
  - `:heartbeat.status` (= `:missing` if telemetry socket disconnected — Session 50 stale-gz bug surface).
- `src/dashboard/lib/axon_nexus/axon/watcher/telemetry.ex` — `axon_dashboard_web/telemetry.ex` Telemetry.Metrics declarations. Mostly Phoenix/Ecto built-ins (no Axon-specific metrics).
- `src/dashboard/lib/axon_nexus/axon/watcher/project_metrics.ex` — per-project chunk/file counts derived from MCP `embedding_status`.
- `src/dashboard/lib/axon_nexus/axon/watcher/indexer_heartbeat.ex` — polls indexer heartbeat row.
- `src/dashboard/lib/axon_dashboard_web/live/mcp_live.ex` — MCP catalog cockpit (REQ-AXO-901647).

The dashboard surface is **a thin projection of `embedding_status` + the telemetry socket heartbeat**. It does NOT have its own metric sources.

---

## Redundancy

| Cluster | Members | Recommended consolidation |
|---|---|---|
| **R1 — pending count** | `embedding_status.pending_chunks` ; `embedding_status.runtime_pending_count` ; `status.machine_status.pipeline.pending` ; `status.file_vectorization_queue.queued+inflight` | keep `pending_chunks` (DB ground-truth) + `runtime_pending_count` (in-memory liveness check) ; drop the other two |
| **R2 — freshness** | `status.truth_status` ; `status.truth_cockpit.freshness.state` ; `status.truth_cockpit.freshness.truth_status` ; `status.availability.ist_projection_fresh` ; `status.availability.advanced_indexed_surfaces_visible` ; `status.machine_status.freshness_state` | keep `truth_status` + `availability.ist_projection_fresh` |
| **R3 — workers shown** | `embedding_status.pipeline_a.{a1,a2,a3}` ; `embedding_status.pipeline_b.{b1,b2,b3}` ; `status.resource_policy.vector_workers` ; `status.resource_policy.graph_workers` | keep only `pipeline_a` + `pipeline_b` (v2 lanes), drop the v1 vector/graph fields |
| **R4 — chunks_embedded_total** | `vector_pipeline_telemetry.chunks_embedded_total` ; `vector_pipeline_telemetry.vector_chunks_embedded_total` ; `service_guard::VECTOR_CHUNKS_EMBEDDED_TOTAL` (atomic) | one name. Pipeline v2 = canonical |
| **R5 — graph_projection_queue** | `status.queues.graph_projection.{queued,inflight,total}` ; `status.queues.vectorization.{…}` ; `status.queues.file_vectorization.{…}` (= `vectorization` copy) | drop `file_vectorization` (alias) ; drop `graph_projection` (graph_worker_loop removed) |
| **R6 — lifecycle phase** | `embedding_status.lifecycle_phase` (heartbeat or singleton) ; `service_guard::process_lifecycle()` | reconcile via heartbeat table (REQ-AXO-91572) ; one source of truth |

---

## Obsolescence (drop with the legacy code)

- **graph_worker_loop counters** (already commented "removed in 2717359b") : `graph_workers_started_total`, `graph_workers_active_current`, `graph_worker_heartbeat_at_ms`, plus all `GRAPH_WORKERS_*` atomics in service_guard.
- **vector_worker_loop counters** (REQ-AXO-901653 Slice 1 partial — remove with the loop) : `vector_workers_started_total`, `_active_current`, `vector_worker_heartbeat_at_ms`, `_restarts_total`.
- **v1 prepare/finalize stage counters** (~12) : `vector_prepare_*`, `vector_finalize_*`, `vector_prepared_work_items_total`, etc.
- **v1 stage timing counters** (~5) : `vector_fetch_ms_total`, `vector_embed_ms_total`, `vector_db_write_ms_total`, `vector_completion_check_ms_total`, `vector_mark_done_ms_total`.
- **FileVectorizationQueue counters** : `status.file_vectorization_queue.{queued,inflight}` (always 0).
- **status.queues.file_vectorization** : entire mirror of `vectorization`.
- **status.queues.graph_projection** : after graph_worker_loop removal.
- **diagnose_indexing.call_graph_gap** cause : legacy `CALLS` / `CALLS_NIF` tables empty post Stop-A.
- **status.truth_cockpit.proof_gaps** : hard-coded single value, never carries information.
- **status.client_reachability_notes** : 3 hard-coded English strings ; better in LLM contract doc than in every response.

---

## Recommended canonical KPI set (≤ 20)

| # | KPI | Provider | Consumer | Refresh | Notes |
|---|---|---|---|---|---|
| 1 | `runtime.truth_status` | brain | LLM | per-call | freshness gate |
| 2 | `runtime.next_best_action` | brain | LLM | per-call | LLM driver |
| 3 | `runtime.staleness` | brain | LLM + dashboard | per-call | files-modified count |
| 4 | `runtime.runtime_mode` | brain | LLM + operator | per-call | |
| 5 | `runtime.build_id` | brain | operator | per-call | release identity |
| 6 | `storage.symbols` | brain | LLM | per-call | |
| 7 | `storage.total_chunks` | brain | LLM + dashboard | per-call | |
| 8 | `storage.embedded_chunks` | brain | LLM + dashboard | per-call | |
| 9 | `storage.coverage_pct` | brain | dashboard | per-call | derived |
| 10 | `storage.indexed_files` | brain | LLM | per-call | |
| 11 | `pipeline.pending_chunks` | brain | LLM + dashboard | per-call | DB ground-truth |
| 12 | `pipeline.runtime_pending_count` | indexer | dashboard | NOTIFY | liveness probe |
| 13 | `pipeline.chunks_embedded_per_second` | indexer | dashboard | 1-s window | throughput |
| 14 | `pipeline.oldest_ready_batch_age_ms` | indexer | dashboard | per-call | tail latency |
| 15 | `pipeline.lifecycle_phase` | indexer | dashboard | heartbeat | sleep/wake state |
| 16 | `pipeline.lifecycle_wake_count` | indexer | dashboard | heartbeat | |
| 17 | `pipeline.lifecycle_sleep_count` | indexer | dashboard | heartbeat | |
| 18 | `pipeline.workers.{a1,a2,a3,b1,b2,b3}` | indexer | dashboard | per-call | v2 lane workers (one struct) |
| 19 | `gpu.tensorrt_engine_cache_hit` | indexer | operator | per-call | TRT cache |
| 20 | `mcp.last_p95_latency_ms` | brain | operator | rolling | health probe |

Everything in `status.runtime_authority.*` (loop_semantics, canonical_ingestion_stage_model, admission_controller, lane_parameters, …) belongs to internal-debug surface, not the canonical LLM contract. Move them behind `status(mode=full)` or `status(verbose=true)` and keep `status(mode=brief)` to the 20 KPIs above.
