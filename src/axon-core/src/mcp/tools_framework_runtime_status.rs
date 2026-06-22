use crate::embedder::current_embedding_provider_diagnostics;
use crate::optimizer;
use crate::runtime_command_proxy::RuntimeCommandProxy;
use crate::runtime_mode::{canonical_embedding_provider_request_for_mode, AxonRuntimeMode};
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use crate::runtime_capacity_profile::RuntimeProfile;
use crate::runtime_topology::{current_runtime_shadow_role, AxonProcessRole};
use crate::service_guard;
use crate::vector_control::{
    apply_semantic_policy_runtime_tuning, current_gpu_vector_lease_diagnostics,
    current_utility_first_scheduler_diagnostics, target_semantic_policy_with_graph,
};
use serde_json::{json, Value};

use super::catalog::tools_catalog;
use super::format::{evidence_by_mode, format_standard_contract};
use super::tools_framework::{STATUS_CACHE_TTL_MS, STATUS_FULL_CACHE_TTL_MS};
use super::tools_framework_support::{cache_read, cache_write};
use super::McpServer;

impl McpServer {
    pub(super) fn axon_status_status_impl(&self, args: &Value) -> Option<Value> {
        let mode = args.get("mode").and_then(|value| value.as_str());
        let now_ms = Self::now_unix_ms();
        let runtime_mode = AxonRuntimeMode::from_env();
        let runtime_shadow_role = current_runtime_shadow_role();
        let split_runtime_is_indexer = matches!(
            AxonProcessRole::from_runtime_shadow_role(&runtime_shadow_role),
            Some(AxonProcessRole::Indexer)
        );
        let runtime_profile = AxonRuntimeOperationalProfile::from_mode_and_strings(
            runtime_mode.as_str(),
            std::env::var("AXON_ENABLE_AUTONOMOUS_INGESTOR")
                .ok()
                .as_deref(),
        );
        let cache_key = format!(
            "{}|{}|{}|{}|{}|{}",
            mode.unwrap_or("brief"),
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
            crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "unknown",),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string()),
            // REQ-AXO-902065 : bind cache lifetime to the live release identity so an
            // in-place promote (which rewrites AXON_INSTALL_GENERATION + AXON_BUILD_ID via
            // AXON_ACTIVE_IDENTITY_FILE but NOT AXON_RUNTIME_IDENTITY) forces a cache miss
            // instead of serving the prior build_id for up to STATUS_CACHE_TTL_MS.
            std::env::var("AXON_INSTALL_GENERATION").unwrap_or_else(|_| "workspace".to_string())
        );
        let status_cache_ttl_ms = match mode.unwrap_or("brief") {
            "full" => STATUS_FULL_CACHE_TTL_MS,
            _ => STATUS_CACHE_TTL_MS,
        };
        if let Some(cached) = cache_read(
            Self::status_cache(),
            &cache_key,
            now_ms,
            status_cache_ttl_ms,
        ) {
            return Some(cached);
        }
        let public_tools = tools_catalog(false)
            .get("tools")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let public_tool_names = public_tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|value| value.as_str()))
            .collect::<Vec<_>>();

        let debug = self
            .axon_debug_with_args(&json!({}))
            .unwrap_or_else(|| json!({"data": {}}));
        let debug_data = debug.get("data").cloned().unwrap_or_else(|| json!({}));
        let vector_queue_statuses = debug_data
            .pointer("/embedding_contract/file_vectorization_queue_statuses")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let debug_queued_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "queued" || status == "paused_for_interactive_priority").then_some(count)
            })
            .sum::<u64>();
        let debug_inflight_files = vector_queue_statuses
            .iter()
            .filter_map(|row| {
                let status = row.get("status")?.as_str()?;
                let count = row.get("count")?.as_u64()?;
                (status == "inflight").then_some(count)
            })
            .sum::<u64>();
        // REQ-AXO-901653 Slice 3b — queue helpers removed ; canonical
        // pipeline path tracks via Chunk + ChunkEmbedding directly.
        let (db_queued_files, db_inflight_files): (usize, usize) = (0, 0);
        let queued_files = debug_queued_files.max(db_queued_files as u64);
        let inflight_files = debug_inflight_files.max(db_inflight_files as u64);
        let drain_state = debug_data
            .pointer("/embedding_contract/drain_state")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let stale_threshold_ms = std::env::var("AXON_VECTOR_LEASE_STALE_MS")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(120_000);
        let debug_graph_queue_depth = debug_data
            .pointer("/embedding_contract/runtime_telemetry/graph_projection_queue_depth")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        // REQ-AXO-901653 Slice 3b — graph_projection_queue table dropped.
        let (db_graph_queue_queued, db_graph_queue_inflight): (usize, usize) = (0, 0);
        let graph_queue_depth =
            debug_graph_queue_depth.max(db_graph_queue_queued + db_graph_queue_inflight);
        let persisted_file_pending_depth = if runtime_mode.ingestion_enabled() {
            self.graph_store.count_persisted_file_pending().unwrap_or(0)
        } else {
            0
        };
        let graph_wip_depth = if runtime_mode.ingestion_enabled() {
            self.graph_store.count_graph_wip_files().unwrap_or(0)
        } else {
            0
        };
        let structural_graph_backlog_depth = persisted_file_pending_depth + graph_wip_depth;
        // REQ-AXO-901653 slice-5c — public.File dropped. Pipeline_v2 tracks
        // graph-readiness via Chunk + IndexedFile presence ; the legacy
        // boolean column is gone.
        let graph_ready_depth =
            if runtime_mode.ingestion_enabled() || runtime_mode.semantic_workers_enabled() {
                self.graph_store
                    .query_count("SELECT count(DISTINCT file_path) FROM ist.Chunk")
                    .unwrap_or(0) as usize
            } else {
                0
            };
        let orphan_vectorization_files = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .count_orphaned_file_vectorization_files()
                .unwrap_or(0)
        } else {
            0
        };
        let stale_vector_inflight_files = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .count_stale_inflight_file_vectorization_files(now_ms, stale_threshold_ms)
                .unwrap_or(0)
        } else {
            0
        };
        let oldest_graph_pending_age_ms = if runtime_mode.ingestion_enabled() {
            self.graph_store
                .oldest_graph_pending_age_ms(now_ms)
                .unwrap_or(0)
        } else {
            0
        };
        let oldest_semantic_pending_age_ms = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .oldest_semantic_pending_age_ms(now_ms)
                .unwrap_or(0)
        } else {
            0
        };
        let rich_runtime_diagnostics = matches!(mode.unwrap_or("brief"), "full");
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let embedding_provider = canonical_embedding_provider_request_for_mode(
            runtime_mode,
            RuntimeProfile::detect().gpu_present,
        );
        let semantic_backlog_responsible = match runtime_mode {
            AxonRuntimeMode::IndexerFull | AxonRuntimeMode::IndexerVector => {
                let provider_is_gpu = matches!(
                    embedding_provider.trim().to_ascii_lowercase().as_str(),
                    "cuda" | "gpu"
                );
                if provider_is_gpu {
                    !gpu_vector_lease.exclusive_required
                        || gpu_vector_lease.owned_by_current_instance
                } else {
                    true
                }
            }
            AxonRuntimeMode::BrainOnly | AxonRuntimeMode::IndexerGraph => false,
        };
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            if semantic_backlog_responsible {
                structural_graph_backlog_depth
            } else {
                0
            },
            queued_files as usize + inflight_files as usize,
            service_guard::current_pressure(),
        );
        let runtime_signals = rich_runtime_diagnostics
            .then(|| optimizer::collect_runtime_signals_window(&self.graph_store));

        // REQ-AXO-901653 slice-5c — total_file_count / vector_ready_depth
        // now driven by IndexedFile (pipeline canonical) and ChunkEmbedding.
        let total_file_count =
            if runtime_mode.ingestion_enabled() || runtime_mode.semantic_workers_enabled() {
                self.graph_store
                    .query_count("SELECT count(*) FROM ist.IndexedFile")
                    .unwrap_or(0) as usize
            } else {
                0
            };
        let vector_ready_depth = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .query_count(
                    "SELECT count(DISTINCT c.file_path) FROM ist.Chunk c \
                     JOIN ist.ChunkEmbedding e ON e.chunk_id = c.id",
                )
                .unwrap_or(0) as usize
        } else {
            0
        };
        let canonical_ingestion_stage_model = self.canonical_ingestion_stage_model_snapshot();
        let admission_controller = Self::admission_controller_snapshot(
            runtime_mode.as_str(),
            persisted_file_pending_depth,
            graph_wip_depth,
        );
        let canonical_edges = Self::canonical_edge_control_snapshot(
            runtime_mode.as_str(),
            persisted_file_pending_depth,
            graph_wip_depth,
            graph_ready_depth,
            structural_graph_backlog_depth,
            semantic_backlog_responsible,
        );
        let effective_graph_backlog_depth = if semantic_backlog_responsible {
            structural_graph_backlog_depth
        } else {
            0
        };
        let graph_semantic_policy =
            apply_semantic_policy_runtime_tuning(target_semantic_policy_with_graph(
                queued_files as usize + inflight_files as usize,
                effective_graph_backlog_depth,
                service_guard::current_pressure(),
            ));
        let priority_contract = Self::priority_contract_snapshot(
            runtime_mode.as_str(),
            semantic_backlog_responsible,
            0,
            structural_graph_backlog_depth,
            graph_queue_depth,
            queued_files as usize + inflight_files as usize,
            utility_scheduler,
            graph_semantic_policy.profile,
        );
        let lane_parameters = Self::runtime_lane_authority_snapshot(
            structural_graph_backlog_depth,
            queued_files as usize + inflight_files as usize,
        );
        let quiescent_state = Self::quiescent_runtime_snapshot(
            runtime_mode.as_str(),
            semantic_backlog_responsible,
            structural_graph_backlog_depth,
            graph_queue_depth,
            queued_files as usize + inflight_files as usize,
            graph_semantic_policy.profile,
            runtime_signals
                .as_ref()
                .map(|signals| signals.canonical_chunks_embedded_last_minute)
                .unwrap_or(0),
            runtime_signals
                .as_ref()
                .map(|signals| signals.canonical_files_embedded_last_minute)
                .unwrap_or(0),
        );
        let limiting_factors = runtime_signals
            .as_ref()
            .map(|signals| {
                Self::runtime_limiting_factors_snapshot(
                    runtime_mode.as_str(),
                    semantic_backlog_responsible,
                    structural_graph_backlog_depth,
                    graph_queue_depth,
                    queued_files as usize + inflight_files as usize,
                    signals,
                )
            })
            .unwrap_or_else(|| {
                json!({
                    "available": false,
                    "available_in_mode": "full",
                    "reason": "rich optimizer-backed limiting diagnostics are frozen behind status(mode=full)"
                })
            });

        let job_counts = if split_runtime_is_indexer {
            Vec::new()
        } else {
            let job_counts_raw = self
                .graph_store
                .execute_raw_sql_gateway(
                    "SELECT status, count(*) FROM soll.McpJob GROUP BY 1 ORDER BY 2 DESC, 1 ASC",
                )
                .unwrap_or_else(|_| "[]".to_string());
            let job_rows: Vec<Vec<Value>> =
                serde_json::from_str(&job_counts_raw).unwrap_or_default();
            job_rows
                .iter()
                .filter_map(|row| {
                    Some(json!({
                        "status": row.first()?.as_str()?.to_string(),
                        "count": row.get(1).and_then(|value| value.as_u64()).unwrap_or(0)
                    }))
                })
                .collect::<Vec<_>>()
        };

        // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
        let instance_kind =
            crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "unknown");
        let runtime_identity =
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string());
        // REQ-AXO-108 — `data_root` is the compact form (e.g. `./.axon`)
        // for human display; `data_root_absolute` is the canonical
        // absolute path so an LLM and an operator running `ls /abs/path`
        // or `du -sh /abs/path` can unambiguously confirm they are
        // looking at the same on-disk IST. Without the absolute form,
        // dual instances on the same host (live vs dev) and worktree
        // layouts (`.worktrees/<branch>/.axon`) collapse to similar
        // compact strings.
        let data_root_raw = std::env::var("AXON_DB_ROOT").unwrap_or_else(|_| "unknown".to_string());
        let data_root_absolute = if data_root_raw == "unknown" {
            "unknown".to_string()
        } else {
            std::path::PathBuf::from(&data_root_raw)
                .canonicalize()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| data_root_raw.clone())
        };
        let data_root = Self::compact_runtime_path(data_root_raw);
        let run_root = Self::compact_runtime_path(
            std::env::var("AXON_RUN_ROOT").unwrap_or_else(|_| "unknown".to_string()),
        );
        let project_root = Self::compact_runtime_path(
            std::env::var("AXON_PROJECT_ROOT").unwrap_or_else(|_| "unknown".to_string()),
        );
        let mcp_url = std::env::var("AXON_MCP_URL").unwrap_or_else(|_| "unknown".to_string());
        let sql_url = std::env::var("AXON_SQL_URL").unwrap_or_else(|_| "unknown".to_string());
        let dashboard_url =
            std::env::var("AXON_DASHBOARD_URL").unwrap_or_else(|_| "unknown".to_string());
        let public_host = std::env::var("AXON_PUBLIC_HOST").unwrap_or_default();
        let public_host_source =
            std::env::var("AXON_PUBLIC_HOST_SOURCE").unwrap_or_else(|_| "unresolved".to_string());
        let advertised_mcp_url = std::env::var("AXON_MCP_PUBLIC_URL").unwrap_or_default();
        let advertised_sql_url = std::env::var("AXON_SQL_PUBLIC_URL").unwrap_or_default();
        let advertised_dashboard_url =
            std::env::var("AXON_DASHBOARD_PUBLIC_URL").unwrap_or_default();
        let advertised_available =
            std::env::var("AXON_PUBLIC_ENDPOINTS_AVAILABLE").unwrap_or_default() == "1"
                && !advertised_mcp_url.is_empty();
        let mutation_policy =
            std::env::var("AXON_MUTATION_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let resource_priority =
            std::env::var("AXON_RESOURCE_PRIORITY").unwrap_or_else(|_| "unknown".to_string());
        let background_budget_class =
            std::env::var("AXON_BACKGROUND_BUDGET_CLASS").unwrap_or_else(|_| "unknown".to_string());
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let watcher_policy =
            std::env::var("AXON_WATCHER_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let local_vector_workers =
            std::env::var("AXON_VECTOR_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let local_graph_workers =
            std::env::var("AXON_GRAPH_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let package_version = std::env::var("AXON_PACKAGE_VERSION")
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
        let release_version =
            std::env::var("AXON_RELEASE_VERSION").unwrap_or_else(|_| package_version.clone());
        let build_id = std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| package_version.clone());
        let install_generation =
            std::env::var("AXON_INSTALL_GENERATION").unwrap_or_else(|_| "workspace".to_string());
        let async_allowlisted_tools = McpServer::ASYNC_JOB_TOOL_NAMES
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let monitored_sync_mutation_tools = McpServer::MONITORED_SYNC_MUTATION_TOOLS
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let runtime_authority_state: Value = self.runtime_topology_snapshot(runtime_mode);
        let runtime_authority_converged = runtime_authority_state
            .get("system_converged")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let indexer_feed_state = runtime_authority_state
            .pointer("/indexer_feed/state")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let indexer_feed_reason = runtime_authority_state
            .pointer("/indexer_feed/degraded_reason")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let process_role = runtime_authority_state
            .get("process_role")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let indexed_projection_fresh = indexer_feed_state == "fresh"
            && indexer_feed_reason.is_none()
            && runtime_authority_converged;
        let standalone_brain_only =
            process_role == "brain" && runtime_mode == AxonRuntimeMode::BrainOnly;
        let indexer_feed_degraded = !standalone_brain_only
            && (indexer_feed_state != "fresh" || indexer_feed_reason.as_deref().is_some());
        let mut degraded_notes =
            if process_role == "brain" && runtime_mode == AxonRuntimeMode::BrainOnly {
                Vec::<String>::new()
            } else {
                Vec::<String>::new()
            };
        if !indexed_projection_fresh {
            degraded_notes.push("indexed_projections_not_fresh".to_string());
        }
        if indexer_feed_degraded {
            degraded_notes
                .push(indexer_feed_reason.unwrap_or_else(|| "indexer_feed_degraded".to_string()));
        }
        if !runtime_authority_converged && !standalone_brain_only {
            degraded_notes.push("runtime_authority_not_converged".to_string());
        }
        let truth_status = if degraded_notes.is_empty() {
            "canonical"
        } else {
            "degraded"
        };
        // REQ-AXO-91497 — when blocker is `indexed_projections_not_fresh`,
        // emit a concrete operator command instead of looping on `status`
        // itself. The prior fallback (`status mode=full`) was recursive.
        // Anchor: CPT-AXO-029 IST freshness invariant.
        let (status_next_action, recovery_hint) = derive_recovery_action(&degraded_notes);
        // REQ-AXO-231 — when freshness is degraded, surface the magnitude
        // (not just the boolean flag) so the LLM client can route on
        // quantitative thresholds : how many files behind, how old the
        // oldest one is, sample of paths impacted. Source : public.File.
        // Cheap aggregate; only runs on the degraded path so canonical
        // status calls stay zero-cost. Falls back to Null on query
        // failure (the rest of the response remains useful).
        let staleness = if !indexed_projection_fresh {
            self.compute_staleness_snapshot().unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        // REQ-AXO-104 — the public_tools list is ~60 names and ~700
        // chars; in brief mode (the default) the LLM client gets a
        // repetitive payload on every status call that does not change
        // within a session and is also exposed in `data.public_tools`.
        // Only include the human-readable line in verbose mode; brief
        // mode points the caller to data.public_tools instead. The
        // data field stays always-on so machine consumers are
        // unaffected.
        // REQ-AXO-106 — the prior label "Advanced indexed surfaces
        // visible: yes/no" was opaque; LLM clients had no way to map it
        // to a tool decision. Replace with a freshness label that names
        // the concrete semantic (IST projection lag) and a parenthetical
        // hint clarifying that stale state does NOT gate any tool —
        // structural reads remain authoritative either way.
        let mut evidence = format!(
            "**Runtime mode:** `{}`\n\
**Runtime profile:** `{}`\n\
**Instance kind:** `{}`\n\
**Runtime identity:** `{}`\n\
**Vector backlog:** queued={} inflight={}\n\
**Utility-first scheduler:** `{}` ({})\n\
**Drain state:** `{}`\n",
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
            crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "unknown",),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string()),
            queued_files,
            inflight_files,
            utility_scheduler.state.as_str(),
            utility_scheduler.reason,
            drain_state,
        );
        // REQ-AXO-042 — surface the LLM-actionable signals in the brief
        // text rendering, not just inside `data.truth_cockpit`. Without
        // this, an LLM reading the markdown text has to compute the next
        // best action from low-level fields (truth_status, drain_state,
        // ist freshness) when the server has already derived it.
        let next_best_kind = status_next_action
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("inspect_runtime_status");
        let recovery_command = recovery_hint
            .get("command")
            .and_then(|value| value.as_str());
        let recovery_reason = recovery_hint.get("reason").and_then(|value| value.as_str());
        // REQ-AXO-901871 — usability-first IST reads signal (operator
        // directive 2026-06-04). The freshness gate is TRUST CALIBRATION,
        // not availability: a brain serving a snapshot with 0 files changed
        // since last index is effectively current. Lead with usability and
        // quantify the real lag (modified_files_since, REQ-AXO-231) so an
        // LLM uses the structural tools instead of refusing them on a
        // process-liveness flag (REQ-AXO-087 family: lag misclassified as
        // unavailability). The recurring failure mode: LLMs read
        // `stale`/`degraded`/`blocker` and decline query/inspect/impact.
        let modified_since = staleness
            .get("modified_files_since")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let oldest_age = staleness
            .get("oldest_modified_age_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if indexed_projection_fresh {
            evidence.push_str(
                "**IST reads:** live — reflects latest indexed source\n\
**Structural tools (query/inspect/impact/why/anomalies/path):** valid\n",
            );
        } else if modified_since == 0 {
            evidence.push_str(
                "**IST reads:** usable — snapshot in sync with source (0 files changed since last index; indexer idle)\n\
**Structural tools (query/inspect/impact/why/anomalies/path):** valid\n",
            );
        } else {
            evidence.push_str(&format!(
                "**IST reads:** usable with lag — {n} file(s) changed since last index (oldest {oldest_age}s)\n\
**Structural tools (query/inspect/impact/why/anomalies/path):** valid; cross-check the {n} changed file(s) before high-stakes mutations\n",
                n = modified_since,
            ));
        }
        // REQ-AXO-901992 B4 — backend-LIVENESS note, distinct from freshness. The
        // HYC consumer saw "valid/usable" while `query` timed out twice on the
        // graph backend before recovering on the 3rd call. Freshness (the lines
        // above) is NOT liveness: surface transient backend pressure as an
        // explicit, retryable note WITHOUT flipping the freshness verdict — so
        // REQ-AXO-901871's usable-by-default contract (anti-grep) is preserved
        // while the agent is no longer surprised by a transient timeout.
        if matches!(
            crate::service_guard::current_pressure(),
            crate::service_guard::ServicePressure::Degraded
                | crate::service_guard::ServicePressure::Critical
        ) {
            evidence.push_str(
                "**Structural backend:** under pressure — query/retrieve_context may be transiently slow or time out; retry once before concluding unavailable (this is liveness, not staleness).\n",
            );
        }
        // REQ-AXO-901963 — PUSH code-intel availability (scope completeness N/N)
        // so an LLM never assumes query/inspect/impact return empty and falls back
        // to grep. Availability ≠ process-liveness (CPT-AXO-029): the N/N derives
        // from indexed ist.Chunk rows, present whether or not the indexer is live.
        if let Some(code_intel_project) = self.auto_resolve_project_code_str() {
            if let Some(scope) = self.project_scope_summary(Some(&code_intel_project)) {
                if scope.total_files > 0 {
                    if scope.backlog_files == 0 {
                        evidence.push_str(&format!(
                            "**Code-intel:** LIVE — `{}` {}/{} files indexed, backlog 0 (query/inspect/impact/why operational — prefer over grep)\n",
                            code_intel_project, scope.completed_files, scope.total_files,
                        ));
                    } else {
                        evidence.push_str(&format!(
                            "**Code-intel:** DEGRADED — `{}` scope {}/{} files, backlog {} (`pending`: {}, `indexing`: {})\n",
                            code_intel_project,
                            scope.completed_files,
                            scope.total_files,
                            scope.backlog_files,
                            scope.pending_files,
                            scope.indexing_files,
                        ));
                    }
                }
            }
        }
        if !indexed_projection_fresh {
            if let Some(cmd) = recovery_command {
                evidence.push_str(&format!("**Optional refresh (live reads):** `{}`\n", cmd));
            }
        }
        // REQ-AXO-902053 P1 (DEC-AXO-901640 G3) — surface visualization-surface
        // freshness (Memgraph publication + last SOLL revision) so a skipped
        // publish (Docker down) or a stale projection is detectable IN-MCP, not
        // only by reading the on-disk marker. Brief line only when the Memgraph
        // projection is NOT fresh (keeps the canonical happy-path output clean);
        // the full block always lands in `data.viz_freshness`.
        let viz_freshness = crate::viz_freshness::viz_freshness_snapshot(now_ms);
        let memgraph_verdict = viz_freshness
            .pointer("/memgraph_publication/verdict")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if memgraph_verdict != "fresh" {
            let memgraph_detail = viz_freshness
                .pointer("/memgraph_publication/detail")
                .and_then(Value::as_str)
                .unwrap_or("");
            evidence.push_str(&format!(
                "**Viz (Memgraph):** {} — {} (non-canonical projection; `bash scripts/publish-memgraph.sh` to refresh)\n",
                memgraph_verdict, memgraph_detail
            ));
        }
        // Genuine degradation (NOT the freshness lag, already conveyed above)
        // still surfaces as a blocker so real problems are never hidden.
        let real_blocker = degraded_notes
            .iter()
            .find(|note| note.as_str() != "indexed_projections_not_fresh");
        if let Some(blocker) = real_blocker {
            evidence.push_str(&format!(
                "**Trust:** `{}`\n**Blocker:** {}\n",
                truth_status, blocker
            ));
            if let Some(reason) = recovery_reason {
                evidence.push_str(&format!("**Recovery:** {}\n", reason));
            }
        }
        let _ = next_best_kind; // retained in data.truth_cockpit.next_best_action
        // REQ-AXO-901757 slice C (AC4) — snapshot-cache warmth from the cache
        // hit/miss counters. Ratio = ram_hits / (ram_hits + pg_loads). This is
        // the WHOLE-snapshot cache-warmth across every tool that calls
        // `snapshot()` (admin reporting tools included) — NOT the retrieval
        // fusion lane (see the fused-lane line below).
        let (soll_ram_hits, soll_pg_loads) = self.soll_cache().read_stats();
        let soll_ram_ratio = {
            let total = soll_ram_hits + soll_pg_loads;
            if total == 0 {
                1.0
            } else {
                soll_ram_hits as f64 / total as f64
            }
        };
        // REQ-AXO-902039 element 3 — fused-retrieval-lane RAM coverage. DEC-AXO-
        // 901646: the headline coverage must measure the why/retrieve_context
        // fusion lane (symbol→governing-intent structural reads), not admin SQL
        // tools. This pair counts only that lane, RAM-served vs PG-fallback.
        let (fusion_ram, fusion_pg) = crate::soll_snapshot::fusion_read_stats();
        let fusion_ram_ratio = {
            let total = fusion_ram + fusion_pg;
            if total == 0 {
                1.0
            } else {
                fusion_ram as f64 / total as f64
            }
        };
        if matches!(mode, Some("verbose") | Some("VERBOSE")) {
            evidence.push_str(&format!(
                "**SOLL snapshot-cache warmth:** {:.1}% RAM ({} RAM hits / {} PG loads)\n",
                soll_ram_ratio * 100.0,
                soll_ram_hits,
                soll_pg_loads
            ));
            evidence.push_str(&format!(
                "**SOLL fusion-lane RAM coverage:** {:.1}% RAM ({} RAM / {} PG fallback)\n",
                fusion_ram_ratio * 100.0,
                fusion_ram,
                fusion_pg
            ));
        }
        if matches!(mode, Some("verbose") | Some("VERBOSE")) {
            evidence.push_str(&format!(
                "**Public tools:** {}\n",
                public_tool_names.join(", ")
            ));
        } else {
            evidence.push_str(&format!(
                "**Public tools count:** {} (full list available via `status mode=verbose` or in `data.public_tools`)\n",
                public_tool_names.len()
            ));
        }
        // REQ-AXO-901994 — the server tool list is authoritative; client bindings
        // do NOT auto-refresh when the server adds tools (e.g. on promote). A
        // capability present server-side but stale in the client reads to an LLM
        // as "capability absent" and triggers grep fallback + wasted tokens.
        // Surface the rule so the LLM reconnects instead of concluding absence.
        evidence.push_str(
            "**Client registry note:** this count is the SERVER truth. If a documented tool (e.g. `soll_manager`) is missing/uncallable in your client, your session registry is STALE (the server adds tools on promote; client bindings don't auto-refresh) — reconnect MCP to refresh; `mcp_surface_diagnostics` confirms server vs client.\n",
        );
        let report = format!(
            "## 📌 Axon Status\n\n{}",
            format_standard_contract(
                "ok",
                "operator truth snapshot assembled",
                &format!(
                    "runtime:{} / profile:{}",
                    runtime_mode.as_str(),
                    runtime_profile.as_str()
                ),
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `anomalies` for structural risks",
                    "run `why` on a target symbol for rationale",
                    "run `path` for source/sink traversal"
                ],
                "high",
            )
        );
        let peer_telemetry: Value = runtime_authority_state
            .pointer("/indexer_runtime/telemetry")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let peer_runtime_available = runtime_authority_state
            .pointer("/indexer_runtime/available")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let use_peer_runtime = process_role == "brain"
            && peer_runtime_available
            && peer_telemetry
                .as_object()
                .map(|value| !value.is_empty())
                .unwrap_or(false);
        let ingress_source = if use_peer_runtime {
            "indexer_peer_status_json"
        } else {
            "status_json"
        };
        // REQ-AXO-901893 (LEGACY FEED PURGE) — the local ingress_buffer was
        // ripped, so the non-peer path has nothing to meter (Watchman feeds
        // pipeline A directly). The peer path still tolerates an older indexer
        // that emits these fields, defaulting to 0 when absent.
        let ingress_buffered_entries = peer_telemetry
            .get("ingress_buffered_entries")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let scan_buffered_entries = peer_telemetry
            .get("ingress_scan_entries")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let watcher_buffered_entries = peer_telemetry
            .get("ingress_hot_entries")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let subtree_hints = peer_telemetry
            .get("ingress_subtree_hints")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let subtree_hint_in_flight = peer_telemetry
            .get("ingress_subtree_hint_in_flight")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as usize;
        let graph_queue_machine = if use_peer_runtime {
            json!({
                "queued": peer_telemetry.pointer("/graph_projection_queue/queued").and_then(|value| value.as_u64()).unwrap_or(0),
                "inflight": peer_telemetry.pointer("/graph_projection_queue/inflight").and_then(|value| value.as_u64()).unwrap_or(0),
                "total": peer_telemetry.pointer("/graph_projection_queue/total").and_then(|value| value.as_u64()).unwrap_or(0)
            })
        } else {
            json!({
                "queued": db_graph_queue_queued,
                "inflight": db_graph_queue_inflight,
                "total": graph_queue_depth
            })
        };
        let vector_queue_machine = if use_peer_runtime {
            json!({
                "queued": peer_telemetry.pointer("/file_vectorization_queue/queued").and_then(|value| value.as_u64()).unwrap_or(0),
                "inflight": peer_telemetry.pointer("/file_vectorization_queue/inflight").and_then(|value| value.as_u64()).unwrap_or(0),
                "total": peer_telemetry.pointer("/file_vectorization_queue/total").and_then(|value| value.as_u64()).unwrap_or(0)
            })
        } else {
            json!({
                "queued": queued_files,
                "inflight": inflight_files,
                "total": queued_files + inflight_files
            })
        };
        let (
            vector_chunks_embedded_cumulative,
            chunk_embeddings_per_second,
            chunk_embeddings_rate_window_ms,
            graph_workers_started_total,
            graph_workers_active_current,
            graph_worker_heartbeat_at_ms,
            ready_queue_chunks_current,
            prepare_inflight_chunks_current,
            ready_replenishment_deficit_current,
            oldest_ready_batch_age_ms_current,
        ) = if use_peer_runtime {
            (
                peer_telemetry
                    .get("vector_chunks_embedded_cumulative")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("chunk_embeddings_per_second")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0),
                peer_telemetry
                    .get("chunk_embeddings_rate_window_ms")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("graph_workers_started_total")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("graph_workers_active_current")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("graph_worker_heartbeat_at_ms")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("ready_queue_chunks_current")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("prepare_inflight_chunks_current")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("ready_replenishment_deficit_current")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                peer_telemetry
                    .get("oldest_ready_batch_age_ms_current")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
            )
        } else {
            let vector_runtime = service_guard::vector_runtime_metrics();
            (
                vector_runtime.chunks_embedded_total,
                vector_runtime.chunk_embeddings_per_second,
                vector_runtime.chunk_embeddings_rate_window_ms,
                vector_runtime.graph_workers_started_total,
                vector_runtime.graph_workers_active_current,
                vector_runtime.graph_worker_heartbeat_at_ms,
                vector_runtime.ready_queue_chunks_current,
                vector_runtime.prepare_inflight_chunks_current,
                vector_runtime.ready_replenishment_deficit_current,
                vector_runtime.oldest_ready_batch_age_ms_current,
            )
        };
        let local_vector_runtime_metrics = service_guard::vector_runtime_metrics();
        let vector_stage_telemetry = service_guard::vector_pipeline_stage_telemetry();
        let vector_latency_summaries = service_guard::vector_runtime_latency_summaries();
        // REQ-AXO-91572 — surface embedder lifecycle (phase + last_used_ms
        // + counters) in framework runtime status. Prefers the indexer
        // heartbeat row (`axon.EmbedderLifecycleHeartbeat`) over
        // the brain-local singleton so the JSON reflects the process that
        // actually owns the GPU session. Same freshness gate as
        // `embedding_status` (tools_system.rs). REQ-AXO-901859 — single
        // source for the window, shared with `runtime_topology_snapshot`.
        use super::runtime_topology_support::EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS;
        let embedder_lifecycle_heartbeat = self
            .graph_store
            .latest_lifecycle_heartbeat("indexer")
            .ok()
            .flatten()
            .filter(|row| {
                (now_ms - row.heartbeat_ms).max(0) <= EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS
            });
        let embedder_lifecycle_source = if embedder_lifecycle_heartbeat.is_some() {
            "indexer_heartbeat"
        } else {
            "brain_local_singleton"
        };
        let local_embedder_lifecycle = crate::embedder::lifecycle_machine::process_lifecycle();
        let (
            embedder_lifecycle_phase,
            embedder_lifecycle_last_used_ms,
            embedder_lifecycle_wake_count,
            embedder_lifecycle_sleep_count,
            embedder_lifecycle_pending_count,
        ) = match embedder_lifecycle_heartbeat.as_ref() {
            Some(row) => (
                row.phase.as_str().to_string(),
                row.last_used_ms,
                row.wake_count,
                row.sleep_count,
                row.pending_count,
            ),
            None => (
                local_embedder_lifecycle.phase().as_str().to_string(),
                local_embedder_lifecycle.last_used_ms(),
                local_embedder_lifecycle.wake_count(),
                local_embedder_lifecycle.sleep_count(),
                crate::embedder::lifecycle::process_state().pending_count() as i64,
            ),
        };
        let embedder_lifecycle_heartbeat_age_ms = embedder_lifecycle_heartbeat
            .as_ref()
            .map(|row| (now_ms - row.heartbeat_ms).max(0));
        let embedder_lifecycle_snapshot = json!({
            "lifecycle_phase": embedder_lifecycle_phase,
            "last_used_ms": embedder_lifecycle_last_used_ms,
            "wake_count": embedder_lifecycle_wake_count,
            "sleep_count": embedder_lifecycle_sleep_count,
            "pending_count": embedder_lifecycle_pending_count,
            "source": embedder_lifecycle_source,
            "heartbeat_age_ms": embedder_lifecycle_heartbeat_age_ms,
            "heartbeat_freshness_window_ms": EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        });
        let local_embedding_provider_diagnostics = current_embedding_provider_diagnostics();
        // DEC-AXO-901626 — observable embedder compute. The brain reads the
        // indexer's pid from its lifecycle heartbeat and cross-references
        // `nvidia-smi --query-compute-apps`; PG counters prove throughput.
        // No self-reported provider slot, no cross-process race. The GPU/CPU
        // verdict here SUPERSEDES the old `peer_embedder_provider` extraction
        // (which surfaced the raced `effective=cpu` lie).
        let embedder_observed = self
            .graph_store
            .embedder_observed_state()
            .unwrap_or_default();
        // DEC-AXO-901626 — the compute verdict is OBSERVED and PUBLISHED by the
        // indexer (self nvidia-smi → EmbedderLifecycleHeartbeat.compute). The
        // brain is a pure reader here: no remote pid, no nvidia-smi. Defaults
        // to CPU/unknown when no fresh indexer heartbeat exists.
        // REQ-AXO-901979 — when no indexer heartbeat exists (brain_only), the
        // cross-process nvidia-smi verdict is absent and the old default lied
        // `CPU` even when the brain's OWN query worker ran on GPU (post-901978
        // B1). Fall back to the worker's self-reported provider (it knows whether
        // it loaded the CUDA EP) before defaulting CPU.
        let embedder_compute = match embedder_lifecycle_heartbeat
            .as_ref()
            .and_then(|row| row.compute.as_deref())
        {
            Some(c) => c.to_string(),
            None => crate::embedder::query_worker_compute_label()
                .unwrap_or("CPU")
                .to_string(),
        };
        let embedder_compute_source = match embedder_lifecycle_heartbeat
            .as_ref()
            .and_then(|row| row.compute_source.as_deref())
        {
            Some(s) => s.to_string(),
            None => crate::embedder::query_worker_compute_label()
                .map(|_| "brain_query_worker_self")
                .unwrap_or("unknown")
                .to_string(),
        };
        let embedder_runtime_snapshot = json!({
            "compute": embedder_compute,
            "compute_source": embedder_compute_source,
            "embedded_per_minute": embedder_observed.embedded_60s,
            "embedded_total": embedder_observed.embedded_total,
            "oldest_pending_age_s": embedder_observed.oldest_pending_age_s,
            "indexer_build_id": embedder_lifecycle_heartbeat
                .as_ref()
                .and_then(|row| row.build_id.clone()),
            "heartbeat_age_ms": embedder_lifecycle_heartbeat_age_ms,
        });
        // Provider strings for the (legacy) vector_pipeline_telemetry block,
        // kept coherent with the observable verdict above so neither surface
        // lies. effective_label reflects observed compute; init_error is no
        // longer tracked (failures surface via logs + VectorWorkerFault).
        let provider_effective_label = if embedder_compute == "GPU" {
            if local_embedding_provider_diagnostics
                .provider_requested
                .eq_ignore_ascii_case("tensorrt")
            {
                "tensorrt".to_string()
            } else {
                "cuda".to_string()
            }
        } else {
            "cpu".to_string()
        };
        let provider_init_error: Option<String> = None;
        let embedding_provider_resolution = crate::embedder::provider_resolution_for_label(
            &local_embedding_provider_diagnostics.provider_requested,
            &provider_effective_label,
            false,
            false,
            local_embedding_provider_diagnostics.ort_dylib_path.clone(),
            None,
        );
        let provider_effective_lower = provider_effective_label.trim().to_ascii_lowercase();
        let provider_fallback_count = local_vector_runtime_metrics
            .prepare_fallback_inline_total
            .saturating_add(local_vector_runtime_metrics.finalize_fallback_inline_total)
            .saturating_add(local_vector_runtime_metrics.mixed_fallback_batches_total)
            .saturating_add(u64::from(provider_effective_lower.contains("fallback")));
        let tensorrt_cache_dir = std::env::var("AXON_TENSORRT_CACHE_DIR")
            .ok()
            .or_else(|| std::env::var("ORT_TENSORRT_ENGINE_CACHE_PATH").ok());
        let vector_runtime_machine = json!({
            "chunks_embedded_total": vector_chunks_embedded_cumulative,
            "chunks_inferred_total": local_vector_runtime_metrics.embed_input_texts_total,
            "chunk_embeddings_per_second": chunk_embeddings_per_second,
            "chunk_embeddings_rate_window_ms": chunk_embeddings_rate_window_ms,
            "graph_workers_started_total": graph_workers_started_total,
            "graph_workers_active_current": graph_workers_active_current,
            "graph_worker_heartbeat_at_ms": graph_worker_heartbeat_at_ms,
            "ready_queue_chunks_current": ready_queue_chunks_current,
            "prepare_inflight_chunks_current": prepare_inflight_chunks_current,
            "ready_replenishment_deficit_current": ready_replenishment_deficit_current,
            "oldest_ready_batch_age_ms_current": oldest_ready_batch_age_ms_current,
            "ready_queue_chunks_small": local_vector_runtime_metrics.ready_queue_chunks_small,
            "ready_queue_chunks_medium": local_vector_runtime_metrics.ready_queue_chunks_medium,
            "ready_queue_chunks_large": local_vector_runtime_metrics.ready_queue_chunks_large,
            "ready_batches_small": local_vector_runtime_metrics.ready_batches_small,
            "ready_batches_medium": local_vector_runtime_metrics.ready_batches_medium,
            "ready_batches_large": local_vector_runtime_metrics.ready_batches_large,
            "ready_batches_mixed": local_vector_runtime_metrics.ready_batches_mixed,
            "homogeneous_batches_total": local_vector_runtime_metrics.homogeneous_batches_total,
            "mixed_fallback_batches_total": local_vector_runtime_metrics.mixed_fallback_batches_total,
            "last_consumed_batch_lane": local_vector_runtime_metrics.last_consumed_batch_lane.as_str(),
            "active_small_max_tokens": local_vector_runtime_metrics.active_small_max_tokens,
            "active_medium_max_tokens": local_vector_runtime_metrics.active_medium_max_tokens,
            "embed_attempts_total": local_vector_runtime_metrics.embed_attempts_total,
            "embed_inflight_started_at_ms": local_vector_runtime_metrics.embed_inflight_started_at_ms,
            "embed_inflight_texts_current": local_vector_runtime_metrics.embed_inflight_texts_current,
            "embed_inflight_text_bytes_current": local_vector_runtime_metrics.embed_inflight_text_bytes_current,
            "last_embed_attempt_wall_ms": local_vector_runtime_metrics.last_embed_attempt_wall_ms,
            "avg_embed_attempt_wall_ms": local_vector_runtime_metrics.avg_embed_attempt_wall_ms,
            "max_embed_attempt_wall_ms": local_vector_runtime_metrics.max_embed_attempt_wall_ms,
            "last_embed_gap_ms": local_vector_runtime_metrics.last_embed_gap_ms,
            "avg_embed_gap_ms": local_vector_runtime_metrics.avg_embed_gap_ms,
            "max_embed_gap_ms": local_vector_runtime_metrics.max_embed_gap_ms,
            "vector_workers_started_total": local_vector_runtime_metrics.vector_workers_started_total,
            "vector_workers_stopped_total": local_vector_runtime_metrics.vector_workers_stopped_total,
            "vector_workers_active_current": local_vector_runtime_metrics.vector_workers_active_current,
            "vector_worker_heartbeat_at_ms": local_vector_runtime_metrics.vector_worker_heartbeat_at_ms,
            "vector_worker_restarts_total": local_vector_runtime_metrics.vector_worker_restarts_total,
            "vector_lane_state": local_vector_runtime_metrics.vector_lane_state.as_str()
        });
        let vector_pipeline_telemetry = json!({
            "contract": "tensorrt_ready_vector_pipeline_v1",
            "production_lanes": ["graph", "vector"],
            "stage_totals": {
                "prepare_ms": vector_stage_telemetry.prepare_ms_total,
                "ready_wait_ms": vector_stage_telemetry.ready_wait_ms_total,
                "inference_ms": vector_stage_telemetry.inference_ms_total,
                "output_extract_ms": vector_stage_telemetry.output_extract_ms_total,
                "persist_ms": vector_stage_telemetry.persist_ms_total,
                "finalize_ms": vector_stage_telemetry.finalize_ms_total
            },
            "recent_stage_latency": {
                "prepare": {
                    "samples": vector_latency_summaries.fetch.samples,
                    "p50_ms": vector_latency_summaries.fetch.p50_ms,
                    "p95_ms": vector_latency_summaries.fetch.p95_ms,
                    "max_ms": vector_latency_summaries.fetch.max_ms
                },
                "inference": {
                    "samples": vector_latency_summaries.embed.samples,
                    "p50_ms": vector_latency_summaries.embed.p50_ms,
                    "p95_ms": vector_latency_summaries.embed.p95_ms,
                    "max_ms": vector_latency_summaries.embed.max_ms
                },
                "persist": {
                    "samples": vector_latency_summaries.db_write.samples,
                    "p50_ms": vector_latency_summaries.db_write.p50_ms,
                    "p95_ms": vector_latency_summaries.db_write.p95_ms,
                    "max_ms": vector_latency_summaries.db_write.max_ms
                },
                "finalize": {
                    "samples": vector_latency_summaries.mark_done.samples,
                    "p50_ms": vector_latency_summaries.mark_done.p50_ms,
                    "p95_ms": vector_latency_summaries.mark_done.p95_ms,
                    "max_ms": vector_latency_summaries.mark_done.max_ms
                }
            },
            "provider": {
                "requested_strategy": embedding_provider_resolution.requested_strategy.as_str(),
                "effective_strategy": embedding_provider_resolution.effective_strategy.as_str(),
                "effective_label": provider_effective_label,
                "fallback_count": provider_fallback_count,
                "fallback_origin": embedding_provider_resolution.fallback_origin,
                "provider_init_error": provider_init_error,
                "tensorrt_cache_dir": tensorrt_cache_dir,
                "tensorrt_engine_cache_hit": Value::Null,
                "recycle_count": 0
            }
        });
        let admission_blocking_authority = admission_controller
            .get("blocking_authority")
            .and_then(|value| value.as_str())
            .unwrap_or("none");
        let graph_blocking_authority = canonical_edges
            .pointer("/graph_production_edge/blocking_authority")
            .and_then(|value| value.as_str())
            .unwrap_or("none");
        let vector_blocking_authority = canonical_edges
            .pointer("/vector_downstream_edge/blocking_authority")
            .and_then(|value| value.as_str())
            .unwrap_or("none");
        let explicit_vector_chunk_signals = ready_queue_chunks_current > 0
            || prepare_inflight_chunks_current > 0
            || ready_replenishment_deficit_current > 0;
        let graph_backlog_fallback_allowed = semantic_backlog_responsible
            && graph_workers_active_current > 0
            && !explicit_vector_chunk_signals
            && vector_blocking_authority == "none"
            && (effective_graph_backlog_depth > 0 || graph_queue_depth > 0);
        let dominant_blocking_authority = if vector_blocking_authority != "none" {
            vector_blocking_authority
        } else if admission_blocking_authority != "none" {
            admission_blocking_authority
        } else if graph_blocking_authority != "none" && graph_workers_active_current > 0 {
            graph_blocking_authority
        } else if graph_backlog_fallback_allowed {
            "graph_backlog_present"
        } else if queued_files as usize + inflight_files as usize > 0 {
            "vector_backlog_present"
        } else {
            "none"
        };
        let machine_status = json!({
            "source": ingress_source,
            "truth_status": truth_status,
            "process_role": if use_peer_runtime { "indexer" } else { process_role },
            "freshness_state": if truth_status == "canonical" { "fresh" } else { "degraded" },
            "authorities": {
                "public_mcp_authority": runtime_authority_state.get("public_mcp_authority").cloned().unwrap_or_else(|| json!("unknown")),
                "soll_writer_authority": runtime_authority_state.get("soll_writer_authority").cloned().unwrap_or_else(|| json!("unknown")),
                "ist_writer_authority": runtime_authority_state.get("ist_writer_authority").cloned().unwrap_or_else(|| json!("unknown"))
            },
            "pipeline": {
                "known": total_file_count,
                "pending": persisted_file_pending_depth,
                "graph_wip": graph_wip_depth,
                "graph_ready": graph_ready_depth,
                "vector_ready": vector_ready_depth,
                "skipped": canonical_ingestion_stage_model.pointer("/explicitly_excluded_from_vectorization/current_count").and_then(|value| value.as_u64()).unwrap_or(0)
            },
            // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress_buffer was ripped.
            // These counters are 0 for a current-process indexer (Watchman feeds
            // pipeline A directly); a peer indexer on an older build may still
            // populate buffered_entries via its telemetry JSON.
            "ingress": {
                "buffered_entries": ingress_buffered_entries,
                "scan_buffered_entries": scan_buffered_entries,
                "watcher_buffered_entries": watcher_buffered_entries,
                "subtree_hints": subtree_hints,
                "subtree_hint_in_flight": subtree_hint_in_flight,
                "notes": "ingress_buffer RIPPED (REQ-AXO-901893); file source = Watchman + DBQ-A"
            },
            "queues": {
                "graph_projection": graph_queue_machine,
                "vectorization": vector_queue_machine,
                "file_vectorization": vector_queue_machine
            },
            "vector": vector_runtime_machine,
            "blocking": {
                "admission": json!(admission_blocking_authority),
                "graph": json!(graph_blocking_authority),
                "vector": json!(vector_blocking_authority),
                "dominant": dominant_blocking_authority
            }
        });
        let truth_cockpit = json!({
            "current_blocker": if degraded_notes.is_empty() {
                Value::Null
            } else {
                json!(degraded_notes.first().cloned().unwrap_or_else(|| "runtime_truth_degraded".to_string()))
            },
            "next_best_action": status_next_action,
            "recovery_hint": recovery_hint,
            "staleness": staleness,
            "confidence": "high",
            "freshness": {
                "state": if truth_status == "canonical" { "fresh" } else { "degraded" },
                "truth_status": truth_status,
                "degraded_notes": degraded_notes
            },
            "proof_gaps": if degraded_notes.is_empty() {
                json!([])
            } else {
                json!(["fresh_indexed_projection"])
            },
            "llm_instruction": "Use `next_best_action` first; treat degraded freshness as partial truth until status(mode=\"full\") explains the blocker."
        });
        // REQ-AXO-098 / DEC-AXO-062 — snapshot subsystem-tagged
        // tristate readiness once and pass both the rolled-up overall
        // and the per-subsystem reports through to the response.
        let (readiness_snapshot, subsystem_reports) =
            crate::runtime_readiness::snapshot_runtime_readiness();
        let readiness_json = serde_json::to_value(&readiness_snapshot)
            .unwrap_or_else(|_| serde_json::json!({"kind": "ready"}));
        let subsystems_json =
            serde_json::to_value(&subsystem_reports).unwrap_or_else(|_| serde_json::json!([]));
        let mut response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "truth_status": truth_status,
                "truth_cockpit": truth_cockpit,
                "next_action": status_next_action,
                "machine_status": machine_status,
                "runtime_mode": runtime_mode.as_str(),
                "runtime_profile": runtime_profile.as_str(),
                "drain_state": drain_state,
                "availability": {
                    // REQ-AXO-106 — `ist_projection_fresh` is the
                    // canonical name for this signal; the historical
                    // `advanced_indexed_surfaces_visible` alias is kept
                    // for backwards compatibility with existing MCP
                    // consumers. New consumers MUST read
                    // `ist_projection_fresh`.
                    "ist_projection_fresh": indexed_projection_fresh,
                    "advanced_indexed_surfaces_visible": indexed_projection_fresh,
                    "degraded_notes": degraded_notes
                },
                // REQ-AXO-098 / DEC-AXO-062 — subsystem-tagged tristate
                // readiness. `subsystems[]` carries one entry per
                // subsystem that has reported state since boot;
                // `readiness` is the rolled-up overall (Failed
                // dominates Degraded; Degraded dominates Ready).
                // Empty subsystems[] (cold registry) collapses to
                // `readiness.kind=ready` per the conservative-no-signal
                // rule documented in CPT-AXO-023.
                "readiness": readiness_json,
                "subsystems": subsystems_json,
                "canonical_sources": Self::canonical_sources_snapshot(),
                "instance_identity": {
                    "instance_kind": instance_kind,
                    "runtime_identity": runtime_identity,
                    "auto_detected_project": self.auto_detect_project_code_from_cwd(),
                    "data_root": data_root,
                    "data_root_absolute": data_root_absolute,
                    "run_root": run_root,
                    "project_root": project_root,
                    "mcp_url": mcp_url,
                    "sql_url": sql_url,
                    "dashboard_url": dashboard_url,
                    "mutation_policy": mutation_policy,
                    // REQ-AXO-143 — workflow-agnostic onboarding pointer for
                    // the auto-detected project. `null` when no pointer is
                    // configured and no legacy handoff fallback applies.
                    "session_pointer": self.resolve_session_pointer(
                        self.auto_resolve_project_code_str().as_deref().unwrap_or(""),
                        Some(project_root.as_str())
                    )
                },
                "advertised_endpoints": {
                    "available": advertised_available,
                    "public_host": public_host,
                    "public_host_source": public_host_source,
                    "mcp_url": advertised_mcp_url,
                    "sql_url": advertised_sql_url,
                    "dashboard_url": advertised_dashboard_url
                },
                "client_reachability_notes": {
                    "instance_identity_is_runtime_local_only": true,
                    "external_endpoint_rule": "Use advertised_endpoints.* for isolated clients when available. instance_identity.*_url is host-local runtime truth.",
                    "stale_client_binding_possible": true
                },
                "resource_policy": {
                    "resource_priority": resource_priority,
                    "background_budget_class": background_budget_class,
                    "gpu_access_policy": gpu_access_policy,
                    "watcher_policy": watcher_policy,
                    // DEC-AXO-901626 — observable effective provider (derived
                    // from the indexer pid's `nvidia-smi` footprint), never the
                    // raced self-reported slot. Coherent with
                    // runtime_authority.embedder_runtime.compute below.
                    "embedding_provider": provider_effective_label.clone(),
                    // REQ-AXO-901836 — same override discipline for worker counts:
                    // when the brain is composing on behalf of a paired indexer,
                    // surface the indexer's own lane_parameters truth instead of
                    // the brain's local AXON_VECTOR_WORKERS/AXON_GRAPH_WORKERS
                    // env values (which under brain_only are 0/6, never the
                    // indexer's actual full-lane settings).
                    "vector_workers": runtime_authority_state
                        .pointer("/indexer_runtime/lane_parameters/vector_workers")
                        .and_then(|value| value.as_u64())
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| local_vector_workers.clone()),
                    "graph_workers": runtime_authority_state
                        .pointer("/indexer_runtime/lane_parameters/graph_workers")
                        .and_then(|value| value.as_u64())
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| local_graph_workers.clone())
                },
                "runtime_authority": {
                    "proposed_control_model": "admission_first_stock_control",
                    "runtime_state": runtime_authority_state,
                    "vector_pipeline_telemetry": vector_pipeline_telemetry,
                    // REQ-AXO-91572 — embedder lifecycle observability.
                    // `source=indexer_heartbeat` means we read the indexer
                    // process state via `axon.EmbedderLifecycleHeartbeat`
                    // (cross-process, the brain reports the GPU owner's truth).
                    // `source=brain_local_singleton` means the indexer hasn't
                    // published a fresh heartbeat (or the table is empty) and
                    // we fell back to the brain's own (likely idle) lifecycle.
                    "embedder_lifecycle": embedder_lifecycle_snapshot,
                    // DEC-AXO-901626 — observable embedder compute (GPU/CPU)
                    // + PG-canonical throughput. The single source of truth
                    // for "is the embedder really on the GPU?", replacing the
                    // raced provider slot the dashboard used to mis-read.
                    "embedder_runtime": embedder_runtime_snapshot,
                    "loop_semantics": Self::loop_semantics_snapshot(),
                    "canonical_ingestion_stage_model": canonical_ingestion_stage_model,
                    "admission_controller": admission_controller,
                    "canonical_edges": canonical_edges,
                    "priority_contract": priority_contract,
                    "lane_parameters": lane_parameters,
                    "quiescent_state": quiescent_state,
                    "limiting_factors": limiting_factors
                },
                "runtime_version": {
                    "release_version": release_version,
                    "package_version": package_version,
                    "build_id": build_id,
                    "install_generation": install_generation
                },
                "file_vectorization_queue": {
                    "queued": queued_files,
                    "inflight": inflight_files
                },
                "utility_first_scheduler": {
                    "state": utility_scheduler.state.as_str(),
                    "reason": utility_scheduler.reason,
                    "semantic_underfeed": utility_scheduler.semantic_underfeed,
                    "ready_reserve_target": utility_scheduler.ready_reserve_target,
                    "target_ready_chunks": utility_scheduler.target_ready_chunks,
                    "hold_window_ms": utility_scheduler.hold_window_ms,
                    "orphan_vectorization_files": orphan_vectorization_files,
                    "stale_vector_inflight_files": stale_vector_inflight_files,
                    "oldest_graph_pending_age_ms": oldest_graph_pending_age_ms,
                    "oldest_semantic_pending_age_ms": oldest_semantic_pending_age_ms
                },
                "public_tools": public_tool_names,
                // REQ-AXO-902053 P1 (DEC-AXO-901640 G3) — visualization-surface
                // freshness: Memgraph publication marker (aged + verdict) + last
                // SOLL revision observed by the dashboard's autodoc-regen listener.
                "viz_freshness": viz_freshness,
                // REQ-AXO-901757 slice C (AC4) — SOLL snapshot-cache warmth.
                // Whole-snapshot reads served from RAM vs PG (re)loads (cold cache /
                // invalidation), across EVERY tool that calls snapshot() — admin
                // reporting tools included. NOT the retrieval fusion lane; see
                // `soll_fusion_lane_coverage` for that (REQ-AXO-902039 element 3).
                "soll_read_coverage": {
                    "ram_hits": soll_ram_hits,
                    "pg_loads": soll_pg_loads,
                    "ram_ratio": soll_ram_ratio,
                    "scope": "soll_snapshot_cache_warmth (all tools; not fusion-lane specific)"
                },
                // REQ-AXO-902039 element 3 / DEC-AXO-901646 — fused-retrieval-lane
                // RAM coverage. The honest headline: how much of the why/
                // retrieve_context symbol→governing-intent structural lookup is
                // served from RAM vs an explicit annotated PG fallback (project
                // unscoped, snapshot cold, or a column not mirrored in RAM).
                "soll_fusion_lane_coverage": {
                    "ram_reads": fusion_ram,
                    "pg_reads": fusion_pg,
                    "ram_ratio": fusion_ram_ratio,
                    "scope": "why/retrieve_context fusion lane (resolve_scoped_symbol_id + traceability + concept bridges)"
                },
                "async_policy": {
                    "mode": "allowlist",
                    "sync_by_default": true,
                    "latency_target_p95_ms": 200,
                    "allowlisted_tools": async_allowlisted_tools,
                    "monitored_sync_mutation_tools": monitored_sync_mutation_tools,
                    "semantic_async_triggers": [
                        "batch",
                        "restore_import",
                        "queue_pipeline",
                        "vectorization_indexation",
                        "deep_analytics"
                    ]
                },
                "async_contract": {
                    "canonical_follow_up_tool": "job_status",
                    "stale_client_binding_possible": true,
                    "preferred_identity_tools": ["project_registry_lookup", "axon_init_project"],
                    "runtime_command_proxy": {
                        "enabled": RuntimeCommandProxy::enabled(),
                        "mode": RuntimeCommandProxy::proxy_mode(),
                        "timeout_ms": RuntimeCommandProxy::timeout_ms(),
                        "timeout_kind": RuntimeCommandProxy::timeout_kind(),
                        "ownership": {
                            "proxy_role": "brain",
                            "execution_role": "indexer",
                            "mutation_owner": "indexer",
                            "duplicate_execution_prevented": true
                        },
                        "retry_policy": {
                            "retryable": true,
                            "max_attempts": 1,
                            "idempotent": true,
                            "duplicate_execution_prevented": true
                        }
                    }
                },
                "job_counts": job_counts,
                "debug_snapshot": debug_data,
                "traceability": debug_data.get("traceability").cloned().unwrap_or_else(|| json!({}))
            }
        });
        if !rich_runtime_diagnostics {
            if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
                data.remove("debug_snapshot");
                data.remove("traceability");
            }
        }
        // REQ-AXO-91484 — call-graph coverage surfaced only in verbose/full so
        // brief stays fast. Cached at the response level via status_cache.
        let verbose_or_full = matches!(mode, Some("verbose") | Some("VERBOSE") | Some("full"));
        if verbose_or_full {
            if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
                data.insert(
                    "ist_call_graph_coverage".to_string(),
                    self.ist_call_graph_coverage_snapshot(),
                );
            }
        }
        // REQ-AXO-91583 — mid-task methodology drift warnings. v0 surfaces
        // the canonical MANDATED SKI set (cross-tenant PRO namespace) so
        // an LLM calling status() can verify which procedural skills it is
        // expected to invoke this session against its own context. Real
        // invocation-tracking via skill_invoke audit log = slice 2 (a
        // future REQ adds the audit ring buffer + diff computation).
        if let Some(data) = response.get_mut("data").and_then(Value::as_object_mut) {
            data.insert(
                "methodology_drift_warnings".to_string(),
                self.methodology_drift_warnings_v0(),
            );
        }
        cache_write(Self::status_cache(), cache_key, now_ms, &response);
        Some(response)
    }

    /// REQ-AXO-91583 — methodology drift surface. v1 reads the SOLL MANDATED
    /// SKI set + the per-process skill_invoke audit ring buffer
    /// (super::tools_skill::recent_skill_invocations) and computes the diff
    /// `mandated_set - invoked_in_last_window_ms`. Default window = 30 min.
    /// Returns `{mandated_skills, recently_invoked, drift_warnings,
    /// tracking_version: "v1_inmemory_audit"}`.
    fn methodology_drift_warnings_v0(&self) -> Value {
        let rows = self
            .graph_store
            .query_json(
                "SELECT id, COALESCE(title, '') \
                 FROM soll.Node \
                 WHERE type='Skill' AND status='current' \
                   AND COALESCE(metadata->>'invocation_mode', 'OPTIONAL') = 'MANDATED' \
                 ORDER BY id",
            )
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
            .unwrap_or_default();
        let mandated: Vec<(String, String)> = rows
            .into_iter()
            .filter(|r| r.len() >= 2)
            .map(|r| (r[0].clone(), r[1].clone()))
            .collect();

        // REQ-AXO-91583 slice 2 — read the audit ring buffer for the last 30min window.
        const DRIFT_WINDOW_MS: u128 = 30 * 60 * 1000;
        let recent = super::tools_skill::recent_skill_invocations(DRIFT_WINDOW_MS);
        let recent_ids: std::collections::HashSet<String> =
            recent.iter().map(|e| e.id.clone()).collect();

        let drift_warnings: Vec<Value> = mandated
            .iter()
            .filter(|(id, _)| !recent_ids.contains(id))
            .map(|(id, title)| {
                json!({
                    "skill_id": id,
                    "title": title,
                    "warning": "MANDATED skill not invoked in last 30 min — methodology drift signal",
                })
            })
            .collect();

        let mandated_json: Vec<Value> = mandated
            .iter()
            .map(|(id, title)| json!({ "id": id, "title": title }))
            .collect();

        let recently_invoked_json: Vec<Value> = recent
            .iter()
            .map(|e| json!({ "id": e.id, "at_unix_ms": e.at_unix_ms }))
            .collect();

        json!({
            "mandated_skills": mandated_json,
            "recently_invoked": recently_invoked_json,
            "drift_warnings": drift_warnings,
            "window_minutes": 30,
            "tracking_version": "v1_inmemory_audit",
            "llm_instruction": "drift_warnings is non-empty when MANDATED skills haven't been invoked in the audit window — invoke them via mcp__axon__skill_invoke or re_anchor for canonical state.",
        })
    }

    // REQ-AXO-91484 — surface per-project/per-language call-graph coverage so
    // operators (and the next-session LLM) can spot parser regressions like
    // "Rust fns=N, outgoing_calls=0" without manually grepping ist.edge.
    // Lang derived from CONTAINS source_id extension (read-only SQL, one round
    // trip, <50ms budget).
    pub(crate) fn ist_call_graph_coverage_snapshot(&self) -> Value {
        let rows: Vec<Vec<String>> = match self.graph_store.query_json(IST_CALL_GRAPH_COVERAGE_SQL)
        {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => return json!({"per_project": {}, "alerts": []}),
        };
        ist_call_graph_coverage_build(&rows)
    }

    /// Auto-detect project_code from cwd by matching against ProjectCodeRegistry.
    /// Returns the code if exactly one project matches, null otherwise.
    /// Uses AXON_PROJECT_ROOT (set by runtime scripts) first, then falls back to cwd.
    fn auto_detect_project_code_from_cwd(&self) -> Value {
        match self.auto_resolve_project_code_str() {
            Some(code) => json!(code),
            None => Value::Null,
        }
    }

    /// REQ-AXO-089 — same logic as `auto_detect_project_code_from_cwd` but
    /// returns `Option<String>` for callers that want a borrowable code
    /// without unwrapping a `Value`. Used by IST/DX tools (retrieve_context,
    /// query, inspect, ...) when the caller omits `project` so the response
    /// scope matches the project the user is actually working in instead of
    /// the workspace fallback. `query_json` emits array-of-arrays rows
    /// (one inner array per row, one element per selected column) — the
    /// surrounding code reads only the first column so that's what we
    /// extract.
    pub(crate) fn auto_resolve_project_code_str(&self) -> Option<String> {
        let search_path = std::env::var("AXON_PROJECT_ROOT")
            .or_else(|_| std::env::current_dir().map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_default()
            .replace('\'', "''");
        if search_path.is_empty() {
            return None;
        }
        let json_str = self.graph_store.query_json(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_path IS NOT NULL AND (project_path = '{}' OR starts_with('{}', project_path || '/'))",
            search_path, search_path
        )).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&json_str).ok()?;
        let codes: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter(|s| !s.is_empty())
            .collect();
        if codes.len() == 1 {
            Some(codes.into_iter().next().unwrap())
        } else {
            None
        }
    }
}

// REQ-AXO-91484 — pure builder factored out so the JSON shape, the alert
// threshold, and the coverage_ratio rounding can be unit-tested without
// needing a live PG fixture. Rows come from IST_CALL_GRAPH_COVERAGE_SQL
// projected as Vec<Vec<String>> by graph_store.query_json.
const IST_CALL_GRAPH_COVERAGE_SQL: &str = "WITH symbol_file AS (\
         SELECT e.target_id AS symbol_id, e.project_code, \
                CASE \
                  WHEN e.source_id LIKE '%.rs' THEN 'rust' \
                  WHEN e.source_id LIKE '%.py' THEN 'python' \
                  WHEN e.source_id LIKE '%.exs' THEN 'elixir_script' \
                  WHEN e.source_id LIKE '%.ex' THEN 'elixir' \
                  WHEN e.source_id LIKE '%.tsx' THEN 'tsx' \
                  WHEN e.source_id LIKE '%.ts' THEN 'typescript' \
                  ELSE NULL \
                END AS lang \
         FROM ist.edge e WHERE e.relation_type = 'CONTAINS'\
       ), fn_per AS (\
         SELECT sf.project_code, sf.lang, COUNT(*) AS fns \
         FROM symbol_file sf \
         JOIN ist.symbol s ON s.id = sf.symbol_id \
         WHERE sf.lang IS NOT NULL AND s.kind IN ('function','method') \
         GROUP BY sf.project_code, sf.lang\
       ), calls_per AS (\
         SELECT sf.project_code, sf.lang, COUNT(*) AS outgoing_calls \
         FROM symbol_file sf \
         JOIN ist.edge e ON e.source_id = sf.symbol_id AND e.relation_type = 'CALLS' \
         WHERE sf.lang IS NOT NULL \
         GROUP BY sf.project_code, sf.lang\
       ) SELECT \
           COALESCE(f.project_code, c.project_code) AS project_code, \
           COALESCE(f.lang, c.lang) AS lang, \
           COALESCE(f.fns, 0) AS fns, \
           COALESCE(c.outgoing_calls, 0) AS outgoing_calls \
         FROM fn_per f FULL OUTER JOIN calls_per c USING (project_code, lang) \
         ORDER BY 1, 2";

impl McpServer {
    /// REQ-AXO-231 — staleness magnitude diagnostic.
    /// REQ-AXO-901653 slice-5c — migrated from `public.File` (dropped) to
    /// `ist.IndexedFile` (pipeline canonical). `last_seen_ms` is the
    /// pipeline ingestion timestamp. Staleness here means : how far is
    /// the indexer behind the most-recent ingestion ? Returns 0 stale files
    /// when IndexedFile keeps pace (pipeline writes in-line ; the legacy
    /// "modified files since last publish" decoupling is gone).
    pub(crate) fn compute_staleness_snapshot(&self) -> Result<Value, String> {
        let sql = "SELECT \
                     COALESCE(MAX(last_seen_ms), 0)::text AS last_publish_ts_ms, \
                     '0'::text AS modified_count, \
                     '0'::text AS oldest_age_secs, \
                     ''::text AS sample_paths_pipe \
                   FROM ist.IndexedFile";
        let json = self
            .graph_store
            .query_json(sql)
            .map_err(|e| format!("staleness query failed: {e}"))?;
        let rows: Vec<Vec<String>> =
            serde_json::from_str(&json).map_err(|e| format!("staleness parse failed: {e}"))?;
        Ok(staleness_from_row(rows.first().map(|r| r.as_slice())))
    }
}

/// REQ-AXO-231 — pure assembly from raw row to JSON. Extracted for
/// unit testing without a PG instance.
pub(crate) fn staleness_from_row(row: Option<&[String]>) -> Value {
    let Some(row) = row else {
        return Value::Null;
    };
    let last_publish_ts_ms: i64 = row.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let modified_count: u64 = row.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let oldest_age_secs: i64 = row.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let sample_paths: Vec<String> = row
        .get(3)
        .map(|s| {
            if s.is_empty() {
                Vec::new()
            } else {
                s.split('|').map(String::from).collect()
            }
        })
        .unwrap_or_default();
    let last_publish_iso = if last_publish_ts_ms > 0 {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(last_publish_ts_ms)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    } else {
        String::new()
    };
    json!({
        "last_publish_ts_ms": last_publish_ts_ms,
        "last_publish_ts": last_publish_iso,
        "modified_files_since": modified_count,
        "oldest_modified_age_seconds": oldest_age_secs,
        "sample_paths": sample_paths,
    })
}

/// REQ-AXO-91497 — derive `(next_best_action, recovery_hint)` from
/// degraded notes. When the first blocker has a known concrete recovery
/// command, surface it ; otherwise fall back to a non-recursive default.
/// `next_best_action.tool` MUST NEVER be `status` itself (the prior bug).
pub(crate) fn derive_recovery_action(degraded_notes: &[String]) -> (Value, Value) {
    let first = degraded_notes.first().map(String::as_str).unwrap_or("");
    match first {
        "" => (
            json!({
                "kind": "read_project_truth",
                "tool": "project_status",
                "when": "now"
            }),
            Value::Null,
        ),
        "indexed_projections_not_fresh" => (
            json!({
                "kind": "start_indexer",
                "tool": "axon-live",
                "arguments": { "command": "start --indexer-graph" },
                "when": "now"
            }),
            json!({
                "action": "start_indexer",
                "command": "./scripts/axon-live start --indexer-graph",
                "reason": "brain alone serves frozen IST snapshot; indexer process required for freshness (CPT-AXO-029)",
                "verification": "status mode=brief should report freshness=fresh after ~30s"
            }),
        ),
        _ => (
            json!({
                "kind": "inspect_runtime_status",
                "tool": "status",
                "arguments": { "mode": "full" },
                "when": "now"
            }),
            Value::Null,
        ),
    }
}

fn ist_call_graph_coverage_build(rows: &[Vec<String>]) -> Value {
    let mut per_project = serde_json::Map::new();
    let mut alerts: Vec<String> = Vec::new();
    for row in rows {
        if row.len() < 4 {
            continue;
        }
        let code = row[0].as_str();
        let lang = row[1].as_str();
        let fns: u64 = row[2].parse().unwrap_or(0);
        let calls: u64 = row[3].parse().unwrap_or(0);
        let coverage_ratio = if fns > 0 {
            ((calls as f64 / fns as f64) * 100.0).round() / 100.0
        } else {
            0.0
        };
        if fns > 100 && calls == 0 {
            alerts.push(format!("{}:{}:zero_outgoing_calls", code, lang));
        }
        let entry = per_project
            .entry(code.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                lang.to_string(),
                json!({
                    "fns": fns,
                    "outgoing_calls": calls,
                    "coverage_ratio": coverage_ratio
                }),
            );
        }
    }
    json!({
        "per_project": per_project,
        "alerts": alerts
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(code: &str, lang: &str, fns: u64, calls: u64) -> Vec<String> {
        vec![
            code.to_string(),
            lang.to_string(),
            fns.to_string(),
            calls.to_string(),
        ]
    }

    #[test]
    fn coverage_build_groups_per_project() {
        let rows = vec![
            row("AXO", "rust", 3617, 0),
            row("AXO", "python", 460, 2727),
            row("OPT", "elixir", 50, 80),
        ];
        let out = ist_call_graph_coverage_build(&rows);
        assert_eq!(
            out.pointer("/per_project/AXO/rust/fns")
                .and_then(Value::as_u64),
            Some(3617)
        );
        assert_eq!(
            out.pointer("/per_project/AXO/python/outgoing_calls")
                .and_then(Value::as_u64),
            Some(2727)
        );
        assert_eq!(
            out.pointer("/per_project/OPT/elixir/coverage_ratio")
                .and_then(Value::as_f64),
            Some(1.6)
        );
    }

    #[test]
    fn coverage_build_emits_zero_outgoing_calls_alert_above_threshold() {
        let rows = vec![row("AXO", "rust", 3617, 0), row("AXO", "python", 460, 2727)];
        let out = ist_call_graph_coverage_build(&rows);
        let alerts = out.get("alerts").and_then(Value::as_array).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].as_str(), Some("AXO:rust:zero_outgoing_calls"));
    }

    #[test]
    fn coverage_build_skips_alert_below_threshold() {
        let rows = vec![row("AXO", "rust", 50, 0)];
        let out = ist_call_graph_coverage_build(&rows);
        let alerts = out.get("alerts").and_then(Value::as_array).unwrap();
        assert!(alerts.is_empty());
    }

    #[test]
    fn coverage_build_zero_fns_yields_zero_ratio() {
        let rows = vec![row("AXO", "rust", 0, 0)];
        let out = ist_call_graph_coverage_build(&rows);
        assert_eq!(
            out.pointer("/per_project/AXO/rust/coverage_ratio")
                .and_then(Value::as_f64),
            Some(0.0)
        );
    }

    #[test]
    fn coverage_build_handles_short_or_malformed_rows() {
        let rows = vec![
            vec!["AXO".to_string(), "rust".to_string()],
            row("OPT", "python", 10, 5),
        ];
        let out = ist_call_graph_coverage_build(&rows);
        assert!(out.pointer("/per_project/AXO").is_none());
        assert_eq!(
            out.pointer("/per_project/OPT/python/fns")
                .and_then(Value::as_u64),
            Some(10)
        );
    }

    // REQ-AXO-91497 — recovery action contract
    #[test]
    fn recovery_action_canonical_when_no_blockers() {
        let (action, hint) = derive_recovery_action(&[]);
        assert_eq!(
            action.get("kind").and_then(Value::as_str),
            Some("read_project_truth")
        );
        assert_eq!(
            action.get("tool").and_then(Value::as_str),
            Some("project_status")
        );
        assert!(hint.is_null());
    }

    #[test]
    fn recovery_action_emits_concrete_command_for_stale_ist() {
        let notes = vec!["indexed_projections_not_fresh".to_string()];
        let (action, hint) = derive_recovery_action(&notes);
        assert_eq!(
            action.get("kind").and_then(Value::as_str),
            Some("start_indexer")
        );
        assert_eq!(
            action.get("tool").and_then(Value::as_str),
            Some("axon-live")
        );
        assert_eq!(
            hint.get("command").and_then(Value::as_str),
            Some("./scripts/axon-live start --indexer-graph")
        );
        assert!(hint.get("reason").is_some());
        assert!(hint.get("verification").is_some());
    }

    #[test]
    fn recovery_action_never_recurses_into_status_for_known_blocker() {
        let notes = vec!["indexed_projections_not_fresh".to_string()];
        let (action, _) = derive_recovery_action(&notes);
        // The bug being fixed: `next_best_action.tool` MUST NEVER be `status` itself.
        assert_ne!(action.get("tool").and_then(Value::as_str), Some("status"));
    }

    // REQ-AXO-231 — staleness magnitude
    #[test]
    fn staleness_from_row_returns_null_for_none() {
        assert!(staleness_from_row(None).is_null());
    }

    #[test]
    fn staleness_from_row_parses_canonical_aggregate() {
        // last_publish_ts_ms=1700000000000, modified=3, oldest=42s,
        // sample_paths joined with '|'.
        let row = vec![
            "1700000000000".to_string(),
            "3".to_string(),
            "42".to_string(),
            "src/a.rs|src/b.rs|src/c.rs".to_string(),
        ];
        let v = staleness_from_row(Some(&row));
        assert_eq!(v["last_publish_ts_ms"].as_i64(), Some(1_700_000_000_000));
        assert_eq!(v["modified_files_since"].as_u64(), Some(3));
        assert_eq!(v["oldest_modified_age_seconds"].as_i64(), Some(42));
        let paths = v["sample_paths"].as_array().unwrap();
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0].as_str(), Some("src/a.rs"));
        // RFC3339 must be present and well-formed.
        assert!(v["last_publish_ts"]
            .as_str()
            .map(|s| s.starts_with("20"))
            .unwrap_or(false));
    }

    #[test]
    fn staleness_from_row_handles_empty_paths() {
        let row = vec![
            "0".to_string(),
            "0".to_string(),
            "0".to_string(),
            "".to_string(),
        ];
        let v = staleness_from_row(Some(&row));
        assert_eq!(v["sample_paths"].as_array().unwrap().len(), 0);
        // ts=0 should emit empty RFC3339 (no last publish recorded).
        assert_eq!(v["last_publish_ts"].as_str(), Some(""));
    }

    #[test]
    fn staleness_from_row_clamps_missing_columns() {
        // Only 2 columns present — others fall through to defaults.
        let row = vec!["1700000000000".to_string(), "5".to_string()];
        let v = staleness_from_row(Some(&row));
        assert_eq!(v["modified_files_since"].as_u64(), Some(5));
        assert_eq!(v["oldest_modified_age_seconds"].as_i64(), Some(0));
        assert_eq!(v["sample_paths"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn recovery_action_unknown_blocker_falls_back_to_status_full() {
        // For unknown blockers we still fall back to `status mode=full`. This is
        // the prior behaviour, intentional: the LLM bumps to a richer report to
        // diagnose. Not recursive in spirit (different mode produces different output).
        let notes = vec!["runtime_authority_not_converged".to_string()];
        let (action, hint) = derive_recovery_action(&notes);
        assert_eq!(
            action.get("kind").and_then(Value::as_str),
            Some("inspect_runtime_status")
        );
        assert_eq!(
            action.pointer("/arguments/mode").and_then(Value::as_str),
            Some("full")
        );
        assert!(hint.is_null());
    }
}
