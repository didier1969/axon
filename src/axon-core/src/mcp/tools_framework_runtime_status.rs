use crate::embedder::current_embedding_provider_diagnostics;
use crate::ingress_buffer::ingress_metrics_snapshot;
use crate::optimizer;
use crate::runtime_command_proxy::RuntimeCommandProxy;
use crate::runtime_mode::{canonical_embedding_provider_request_for_mode, AxonRuntimeMode};
use crate::runtime_operational_profile::AxonRuntimeOperationalProfile;
use crate::runtime_profile::RuntimeProfile;
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
            "{}|{}|{}|{}|{}",
            mode.unwrap_or("brief"),
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string()),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string())
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
        let (db_queued_files, db_inflight_files) = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .fetch_file_vectorization_queue_counts()
                .unwrap_or((0, 0))
        } else {
            (0, 0)
        };
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
        let (db_graph_queue_queued, db_graph_queue_inflight) = if runtime_mode.ingestion_enabled() {
            self.graph_store
                .fetch_graph_projection_queue_counts()
                .unwrap_or((0, 0))
        } else {
            (0, 0)
        };
        let graph_queue_depth =
            debug_graph_queue_depth.max(db_graph_queue_queued + db_graph_queue_inflight);
        let ingress = ingress_metrics_snapshot();
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
        let graph_ready_depth = if runtime_mode.ingestion_enabled()
            || runtime_mode.semantic_workers_enabled()
        {
            self.graph_store
                .query_count("SELECT count(*) FROM File WHERE COALESCE(graph_ready, FALSE) = TRUE")
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

        let total_file_count =
            if runtime_mode.ingestion_enabled() || runtime_mode.semantic_workers_enabled() {
                self.graph_store
                    .query_count("SELECT count(*) FROM File")
                    .unwrap_or(0) as usize
            } else {
                0
            };
        let vector_ready_depth = if runtime_mode.semantic_workers_enabled() {
            self.graph_store
                .query_count("SELECT count(*) FROM File WHERE COALESCE(vector_ready, FALSE) = TRUE")
                .unwrap_or(0) as usize
        } else {
            0
        };
        let canonical_ingestion_stage_model = self.canonical_ingestion_stage_model_snapshot();
        let admission_controller = Self::admission_controller_snapshot(
            runtime_mode.as_str(),
            ingress,
            persisted_file_pending_depth,
            graph_wip_depth,
        );
        let canonical_edges = Self::canonical_edge_control_snapshot(
            runtime_mode.as_str(),
            ingress,
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
            ingress.buffered_entries,
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

        let instance_kind =
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string());
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
        let data_root_raw =
            std::env::var("AXON_DB_ROOT").unwrap_or_else(|_| "unknown".to_string());
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
        let max_axon_workers =
            std::env::var("MAX_AXON_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let queue_memory_budget_bytes = std::env::var("AXON_QUEUE_MEMORY_BUDGET_BYTES")
            .unwrap_or_else(|_| "unknown".to_string());
        let watcher_subtree_hint_budget = std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET")
            .unwrap_or_else(|_| "unknown".to_string());
        let vector_workers =
            std::env::var("AXON_VECTOR_WORKERS").unwrap_or_else(|_| "unknown".to_string());
        let graph_workers =
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
        let standalone_brain_only = process_role == "brain"
            && runtime_mode == AxonRuntimeMode::BrainOnly
            && !matches!(
                std::env::var("AXON_SPLIT_SHADOW_ONLY")
                    .ok()
                    .as_deref()
                    .map(str::trim),
                Some("1") | Some("true") | Some("yes") | Some("on")
            );
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
        let status_next_action = if degraded_notes.is_empty() {
            json!({
                "kind": "read_project_truth",
                "tool": "project_status",
                "when": "now"
            })
        } else {
            json!({
                "kind": "inspect_runtime_status",
                "tool": "status",
                "arguments": { "mode": "full" },
                "when": "now"
            })
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
**IST projection freshness:** {} ({})\n\
**Vector backlog:** queued={} inflight={}\n\
**Utility-first scheduler:** `{}` ({})\n\
**Drain state:** `{}`\n",
            runtime_mode.as_str(),
            runtime_profile.as_str(),
            std::env::var("AXON_INSTANCE_KIND").unwrap_or_else(|_| "unknown".to_string()),
            std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown".to_string()),
            if indexed_projection_fresh { "fresh" } else { "stale" },
            if indexed_projection_fresh {
                "reads reflect latest indexed source"
            } else {
                "reads may lag latest source; tools remain usable"
            },
            queued_files,
            inflight_files,
            utility_scheduler.state.as_str(),
            utility_scheduler.reason,
            drain_state,
        );
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
        let ingress_buffered_entries = if use_peer_runtime {
            peer_telemetry
                .get("ingress_buffered_entries")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize
        } else {
            ingress.buffered_entries
        };
        let scan_buffered_entries = if use_peer_runtime {
            peer_telemetry
                .get("ingress_scan_entries")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize
        } else {
            ingress.scan_entries
        };
        let watcher_buffered_entries = if use_peer_runtime {
            peer_telemetry
                .get("ingress_hot_entries")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize
        } else {
            ingress.hot_entries
        };
        let subtree_hints = if use_peer_runtime {
            peer_telemetry
                .get("ingress_subtree_hints")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize
        } else {
            ingress.subtree_hints
        };
        let subtree_hint_in_flight = if use_peer_runtime {
            peer_telemetry
                .get("ingress_subtree_hint_in_flight")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize
        } else {
            ingress.subtree_hint_in_flight
        };
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
            vector_chunks_embedded_total,
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
                    .get("vector_chunks_embedded_total")
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
        let embedding_provider_diagnostics = current_embedding_provider_diagnostics();
        let embedding_provider_resolution = embedding_provider_diagnostics.resolution.clone();
        let provider_effective_lower = embedding_provider_diagnostics
            .provider_effective
            .trim()
            .to_ascii_lowercase();
        let provider_fallback_count = local_vector_runtime_metrics
            .prepare_fallback_inline_total
            .saturating_add(local_vector_runtime_metrics.finalize_fallback_inline_total)
            .saturating_add(local_vector_runtime_metrics.mixed_fallback_batches_total)
            .saturating_add(u64::from(provider_effective_lower.contains("fallback")));
        let tensorrt_cache_dir = std::env::var("AXON_TENSORRT_CACHE_DIR")
            .ok()
            .or_else(|| std::env::var("ORT_TENSORRT_ENGINE_CACHE_PATH").ok());
        let vector_runtime_machine = json!({
            "chunks_embedded_total": vector_chunks_embedded_total,
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
                "effective_label": embedding_provider_diagnostics.provider_effective,
                "fallback_count": provider_fallback_count,
                "fallback_origin": embedding_provider_resolution.fallback_origin,
                "provider_init_error": embedding_provider_diagnostics.provider_init_error,
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
            "ingress": {
                "buffered_entries": ingress_buffered_entries,
                "scan_buffered_entries": scan_buffered_entries,
                "watcher_buffered_entries": watcher_buffered_entries,
                "subtree_hints": subtree_hints,
                "subtree_hint_in_flight": subtree_hint_in_flight,
                "flush_count": admission_controller.get("admission_flush_count").and_then(|value| value.as_u64()).unwrap_or(0),
                "last_promoted_count": admission_controller.get("admission_last_promoted_count").and_then(|value| value.as_u64()).unwrap_or(0),
                "last_durably_persisted_count": admission_controller.get("admission_last_durably_persisted_count").and_then(|value| value.as_u64()).unwrap_or(0),
                "last_excluded_from_pending_count": admission_controller.get("admission_last_excluded_from_pending_count").and_then(|value| value.as_u64()).unwrap_or(0)
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
        let subsystems_json = serde_json::to_value(&subsystem_reports)
            .unwrap_or_else(|_| serde_json::json!([]));
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
                    "embedding_provider": embedding_provider,
                    "max_axon_workers": max_axon_workers,
                    "queue_memory_budget_bytes": queue_memory_budget_bytes,
                    "watcher_subtree_hint_budget": watcher_subtree_hint_budget,
                    "vector_workers": vector_workers,
                    "graph_workers": graph_workers
                },
                "runtime_authority": {
                    "proposed_control_model": "admission_first_stock_control",
                    "runtime_state": runtime_authority_state,
                    "vector_pipeline_telemetry": vector_pipeline_telemetry,
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
        cache_write(Self::status_cache(), cache_key, now_ms, &response);
        Some(response)
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
