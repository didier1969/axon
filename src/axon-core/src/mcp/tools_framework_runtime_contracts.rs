use crate::embedder::{
    bootstrap_runtime_tuning_state, current_runtime_tuning_state, embedding_lane_config_from_env,
};
use crate::runtime_mode::{canonical_embedding_provider_request_for_mode, AxonRuntimeMode};
use crate::runtime_capacity_profile::{
    canonical_watcher_first_priority_lanes, current_admission_controller_state,
    current_graph_production_state, current_runtime_priority_contract_state,
    current_vector_downstream_state, recommend_admission_controller_profile,
    recommend_embedding_lane_sizing, RuntimeProfile,
};
use crate::service_guard;
use crate::vector_control::{
    apply_semantic_policy_runtime_tuning, baseline_semantic_policy,
    current_gpu_vector_lease_diagnostics, current_vector_batch_controller_diagnostics,
    target_semantic_policy_with_graph,
};
use serde_json::{json, Value};

use super::tools_framework::{STATUS_CACHE_TTL_MS, STATUS_FULL_CACHE_TTL_MS};
use super::McpServer;

impl McpServer {
    pub(super) fn priority_contract_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        ingress_buffered_entries: usize,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        utility_scheduler: crate::vector_control::UtilityFirstSchedulerDiagnostics,
        semantic_profile: &'static str,
    ) -> Value {
        let runtime_mode = AxonRuntimeMode::from_str(runtime_mode);
        let lanes = canonical_watcher_first_priority_lanes();
        let effective_graph_backlog_depth = if semantic_backlog_responsible {
            structural_graph_backlog_depth
        } else {
            0
        };
        let priority_state = current_runtime_priority_contract_state(
            runtime_mode.as_str(),
            ingress_buffered_entries,
            effective_graph_backlog_depth,
        );
        let semantic_runtime_enabled =
            runtime_mode.semantic_workers_enabled() && semantic_backlog_responsible;
        let vector_gate_active = semantic_runtime_enabled && priority_state.graph_backlog_present;

        json!({
            "contract_version": "watcher_graph_vector_v1",
            "authority_state": "declared_runtime_truth",
            "scheduler_enforcement_state": priority_state.enforcement_state,
            "pipeline_order": lanes.iter().map(|lane| {
                json!({
                    "lane": lane.lane,
                    "priority": lane.priority,
                    "admission_requires": lane.admission_requires
                })
            }).collect::<Vec<_>>(),
            "lane_gates": {
                "watcher_identification": {
                    "backlog_gated": priority_state.watcher_identification_backlog_gated,
                    "gate_kind": "none",
                    "current_gate_reason": "watcher_and_scan_admission_do_not_wait_for_graph_or_vector_backlog"
                },
                "graphing_after_enqueue": {
                    "backlog_gated": priority_state.graphing_after_enqueue_backlog_gated,
                    "gate_kind": if priority_state.graphing_after_enqueue_backlog_gated {
                        "upstream_ingress_priority"
                    } else {
                        "none"
                    },
                    "current_gate_reason": if priority_state.graphing_after_enqueue_backlog_gated {
                        "higher_priority_ingress_is_still_buffered_before_graph_ownership"
                    } else {
                        "graph_backlog_itself_is_the_actionable_lane"
                    }
                },
                "vectorization_after_graph_ready": {
                    "backlog_gated": priority_state.vectorization_after_graph_ready_backlog_gated,
                    "gate_kind": if vector_gate_active { "soft_priority_gate" } else { "none" },
                    "current_gate_reason": if vector_gate_active {
                        "graph_backlog_present_so_vectorization_is_contractually_deprioritized_after_graph_work"
                    } else if !semantic_runtime_enabled {
                        "semantic_runtime_not_owned_by_current_mode_or_instance"
                    } else {
                        "no_active_graph_priority_gate"
                    },
                    "semantic_policy_profile": semantic_profile,
                    "scheduler_state": utility_scheduler.state.as_str(),
                    "scheduler_reason": utility_scheduler.reason
                }
            },
            "backlog_scope": {
                "structural_graph_backlog_depth": structural_graph_backlog_depth,
                "effective_graph_backlog_depth": effective_graph_backlog_depth,
                "graph_projection_queue_depth": graph_projection_queue_depth,
                "vector_queue_depth": file_backlog_depth
            },
            "vectorization_can_advance_ahead_of_graph_backlog": {
                "allowed_by_contract": priority_state.vectorization_allowed_ahead_of_graph_backlog,
                "allowed_under_current_runtime": semantic_runtime_enabled && !vector_gate_active,
                "enforcement_state": if vector_gate_active {
                    "hard_blocked_until_graph_backlog_clears"
                } else if !semantic_runtime_enabled {
                    "not_owned_by_current_runtime"
                } else {
                    "allowed_without_active_graph_priority_gate"
                },
                "reason": if vector_gate_active {
                    "current_runtime_blocks_semantic_work_while_graph_backlog_is_present"
                } else if !semantic_runtime_enabled {
                    "current_runtime_or_instance_is_not_responsible_for_semantic_drain"
                } else {
                    "no_graph_priority_gate_is_active_now"
                }
            }
        })
    }

    pub(super) fn admission_controller_snapshot(
        runtime_mode: &str,
        persisted_file_pending_current: usize,
        graph_wip_current: usize,
    ) -> Value {
        let runtime_mode = AxonRuntimeMode::from_str(runtime_mode);
        let profile = RuntimeProfile::detect();
        let controller_profile = recommend_admission_controller_profile(&profile);
        let pressure = service_guard::current_pressure();
        // REQ-AXO-901893 (LEGACY FEED PURGE) — buffered/hot/scan ingress counts
        // are structurally 0 now (the ingress_buffer was ripped). Watchman feeds
        // pipeline A directly; admission gates on persisted_file_pending + WIP.
        let controller_state = current_admission_controller_state(
            controller_profile,
            0,
            0,
            0,
            persisted_file_pending_current,
            graph_wip_current,
            !runtime_mode.ingestion_enabled(),
            matches!(pressure, service_guard::ServicePressure::Critical),
        );

        json!({
            "owner": "admission_controller",
            "control_model_state": "proposed",
            "persisted_file_pending_current": persisted_file_pending_current,
            "graph_wip_current": graph_wip_current,
            "admission_wip_current": graph_wip_current,
            "target_band": controller_state.profile.target_band,
            "reorder_point": controller_state.profile.reorder_point,
            "max_wip": controller_state.profile.max_wip,
            "hold_window_ms": controller_state.profile.hold_window_ms,
            "forced_bulk_fill_threshold": controller_state.profile.forced_bulk_fill_threshold,
            "admission_completion_surface": "ist.IndexedFile(status='discovered'/'parsing') drained by Watchman + DBQ-A",
            "blocking_authority": controller_state.blocking_authority,
            "allowed_by_contract": runtime_mode.ingestion_enabled(),
            "allowed_under_current_runtime": controller_state.admission_open,
            "bulk_fill_preferred": controller_state.bulk_fill_preferred,
            "notes": "Controls the canonical discovered -> graph_ready handoff (Watchman + DBQ-A feed, ingress_buffer RIPPED)."
        })
    }

    pub(super) fn canonical_edge_control_snapshot(
        runtime_mode: &str,
        persisted_file_pending_current: usize,
        graph_wip_current: usize,
        graph_ready_current: usize,
        structural_graph_backlog_depth: usize,
        semantic_runtime_enabled: bool,
    ) -> Value {
        let runtime_mode = AxonRuntimeMode::from_str(runtime_mode);
        let profile = RuntimeProfile::detect();
        let controller_profile = recommend_admission_controller_profile(&profile);
        let pressure = service_guard::current_pressure();
        let runtime_processing_disabled = !runtime_mode.ingestion_enabled();
        let critical_pressure = matches!(pressure, service_guard::ServicePressure::Critical);
        let effective_graph_backlog_depth = if semantic_runtime_enabled {
            structural_graph_backlog_depth
        } else {
            0
        };

        let admission = current_admission_controller_state(
            controller_profile,
            0,
            0,
            0,
            persisted_file_pending_current,
            graph_wip_current,
            runtime_processing_disabled,
            critical_pressure,
        );
        let graph = current_graph_production_state(
            persisted_file_pending_current,
            graph_wip_current,
            (controller_profile.max_wip / 2).max(1),
            runtime_processing_disabled,
            critical_pressure,
        );
        let vector = current_vector_downstream_state(
            graph_ready_current,
            effective_graph_backlog_depth,
            semantic_runtime_enabled,
            critical_pressure,
        );

        json!({
            "admission_edge": {
                "owner": "admission_controller",
                "boundary": "buffered_discovery_to_persisted_file_pending",
                "blocking_authority": admission.blocking_authority,
                "allowed_by_contract": !runtime_processing_disabled,
                "allowed_under_current_runtime": admission.admission_open,
                "source_stock_current": 0,
                "target_stock_current": persisted_file_pending_current,
                "wip_current": graph_wip_current,
            },
            "graph_production_edge": {
                "owner": "graph_production_controller",
                "boundary": "persisted_file_pending_to_graph_ready",
                "blocking_authority": graph.blocking_authority,
                "allowed_by_contract": !runtime_processing_disabled,
                "allowed_under_current_runtime": graph.graph_open,
                "source_stock_current": persisted_file_pending_current,
                "target_stock_current": graph_ready_current,
                "wip_current": graph_wip_current,
            },
            "vector_downstream_edge": {
                "owner": "vector_downstream_controller",
                "boundary": "graph_ready_to_vector_ready",
                "blocking_authority": vector.blocking_authority,
                "allowed_by_contract": semantic_runtime_enabled,
                "allowed_under_current_runtime": vector.vector_open,
                "source_stock_current": graph_ready_current,
                "target_stock_current": 0,
                "wip_current": effective_graph_backlog_depth,
            }
        })
    }

    pub(super) fn loop_semantics_snapshot() -> Value {
        json!({
            "upstream_push_loop": {
                "mode": "push",
                "boundary": "buffered_discovery_to_graph_ready",
                "summary_scope": "high_level_loop_summary",
                "control_layers": [
                    "supply_discovery",
                    "admission_production"
                ],
                "stages": [
                    "buffered_discovery",
                    "persisted_file_pending",
                    "graph_ready"
                ],
                "critical_throughput_stock": "persisted_file_pending",
                "notes": "Watcher and scan should create and replenish canonical work without waiting for GPU cadence. This high-level push loop aggregates supply/discovery and admission/production; persisted_file_pending remains the primary global throughput stock unless runtime evidence disproves it."
            },
            "gpu_paced_downstream_loop": {
                "mode": "pull",
                "boundary": "graph_ready_to_vector_ready",
                "paced_by": "gpu_capacity_and_vram",
                "stages": [
                    "graph_ready",
                    "prepare",
                    "ready_batches",
                    "gpu",
                    "vector_ready"
                ],
                "idle_when_source_stock_empty": true,
                "notes": "The downstream lane should wake and drain according to real GPU demand and may idle cleanly when graph_ready is empty."
            },
            "finalize": {
                "mode": "async",
                "hot_path": false,
                "notes": "Finalize is downstream of GPU execution and must not sit on the hot feed path unless a hard safety invariant requires it."
            }
        })
    }

    pub(super) fn canonical_ingestion_stage_model_snapshot(&self) -> Value {
        let runtime_mode = AxonRuntimeMode::from_env();
        let graph_runtime_enabled = runtime_mode.ingestion_enabled();
        let vector_runtime_enabled = runtime_mode.semantic_workers_enabled();
        // REQ-AXO-901653 slice-5c — public.File + FileVectorizationQueue
        // dropped. Pipeline_v2 canonical : IndexedFile = persisted, Chunk =
        // graph-ready, ChunkEmbedding present = vector-ready. Queue depths
        // are always 0 (in-memory pipeline_v2 stages own back-pressure).
        let persisted_file_count = if graph_runtime_enabled || vector_runtime_enabled {
            self.graph_store
                .query_count("SELECT count(*) FROM ist.IndexedFile")
                .unwrap_or(0)
        } else {
            0
        };
        let structural_graph_queued_count: i64 = 0;
        let structural_graph_inflight_count: i64 = 0;
        let structural_graph_backlog_count: i64 = 0;
        let (graph_queue_queued_count, graph_queue_inflight_count): (usize, usize) = (0, 0);
        let graph_queue_owned_count =
            u64::try_from(graph_queue_queued_count.saturating_add(graph_queue_inflight_count))
                .unwrap_or(0);
        let graph_ready_count = if graph_runtime_enabled || vector_runtime_enabled {
            self.graph_store
                .query_count("SELECT count(DISTINCT file_path) FROM ist.Chunk")
                .unwrap_or(0)
        } else {
            0
        };
        let vector_queue_owned_count: i64 = 0;
        let vector_ready_count = if vector_runtime_enabled {
            self.graph_store
                .query_count(
                    "SELECT count(DISTINCT c.file_path) FROM ist.Chunk c \
                     JOIN ist.ChunkEmbedding e ON e.chunk_id = c.id",
                )
                .unwrap_or(0)
        } else {
            0
        };
        let explicitly_excluded_count: i64 = 0;

        json!({
            "authority_state": "canonical",
            "model_version": "watcher_file_graph_vector_v1",
            "freshness": {
                "brief_status_cache_ttl_ms": STATUS_CACHE_TTL_MS,
                "full_status_cache_ttl_ms": STATUS_FULL_CACHE_TTL_MS,
                "recommended_mode_for_current_counts": "full",
                "notes": "Brief status is cached. Use status(mode=\"full\") when exact current counts matter."
            },
            "file_source": {
                "status": "tracked",
                "ownership_surface": "watchman_source + dbq_a",
                "notes": "REQ-AXO-901893 (LEGACY FEED PURGE): the ingress_buffer (buffered/watcher/scan/promotion stages) was ripped. Watchman clock/cursor deltas feed pipeline A directly; the DBQ-A claim feeder drains the ist.IndexedFile 'discovered' backlog by construction."
            },
            "persisted_file": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": persisted_file_count
            },
            "persisted_file_pending": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_queued_count,
                "notes": "Durably persisted canonical file work that is still eligible and pending graph production."
            },
            "graph_wip": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_inflight_count,
                "notes": "Canonical file work currently owned by the graph worker pool."
            },
            "structural_graph_backlog": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": structural_graph_backlog_count,
                "queue_breakdown": {
                    "queued": structural_graph_queued_count,
                    "inflight": structural_graph_inflight_count
                },
                "notes": "Canonical file graphing backlog before graph_ready. This is the primary graphing stage for watcher->graph->vector."
            },
            "graph_projection_queue_owned": {
                "status": "tracked",
                "ownership_surface": "GraphProjectionQueue",
                "current_count": graph_queue_owned_count,
                "queue_breakdown": {
                    "queued": graph_queue_queued_count,
                    "inflight": graph_queue_inflight_count
                },
                "notes": "Secondary graph projection or graph embedding work. Diagnostic only; not the canonical file graphing backlog."
            },
            "graph_ready": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": graph_ready_count
            },
            "file_vectorization_queue_owned": {
                "status": "tracked",
                "ownership_surface": "FileVectorizationQueue",
                "current_count": vector_queue_owned_count,
                "notes": "These files are currently owned by the vector lane for downstream work."
            },
            "vector_ready": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": vector_ready_count
            },
            "explicitly_excluded_from_vectorization": {
                "status": "tracked",
                "ownership_surface": "File",
                "current_count": explicitly_excluded_count,
                "notes": "Counted from explicit deleted/skipped/oversized file states only."
            }
        })
    }

    fn lane_parameter_snapshot(
        seed: usize,
        target: usize,
        effective: usize,
        clamp_reason: Option<&'static str>,
        authority_state: &'static str,
        target_source: &'static str,
        effective_source: &'static str,
    ) -> Value {
        json!({
            "seed": seed,
            "target": target,
            "effective": effective,
            "clamp_visible": clamp_reason.is_some(),
            "clamp_reason": clamp_reason,
            "authority_state": authority_state,
            "target_source": target_source,
            "effective_source": effective_source
        })
    }

    pub(super) fn runtime_lane_authority_snapshot(
        structural_graph_backlog_depth: usize,
        file_backlog_depth: usize,
    ) -> Value {
        let profile = RuntimeProfile::detect();
        let runtime_mode = AxonRuntimeMode::from_env();
        let provider_requested =
            canonical_embedding_provider_request_for_mode(runtime_mode, profile.gpu_present);
        let mut lane_profile = profile.clone();
        lane_profile.gpu_present =
            profile.gpu_present && provider_requested.eq_ignore_ascii_case("cuda");
        let seed = recommend_embedding_lane_sizing(&lane_profile);
        let runtime_seed = bootstrap_runtime_tuning_state();
        let target = current_runtime_tuning_state();
        let effective = embedding_lane_config_from_env();
        let batch_controller = current_vector_batch_controller_diagnostics(&effective);
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let vector_runtime = service_guard::vector_runtime_metrics();
        let cadence_seed = baseline_semantic_policy(file_backlog_depth);
        let cadence_target = target_semantic_policy_with_graph(
            file_backlog_depth,
            structural_graph_backlog_depth,
            service_guard::current_pressure(),
        );
        let cadence_effective = apply_semantic_policy_runtime_tuning(cadence_target);
        let oversubscription_allowed = std::env::var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION")
            .ok()
            .map(|value| value.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let vector_workers_clamp_reason = if effective.vector_workers != target.vector_workers {
            if provider_requested.eq_ignore_ascii_case("cuda")
                && !oversubscription_allowed
                && target.vector_workers > effective.vector_workers
            {
                Some("hard_safety_clamp:gpu_vector_workers_capped_without_oversubscription")
            } else {
                Some("clamp_visible:effective_vector_workers_diverge_from_target")
            }
        } else {
            None
        };

        let graph_workers_clamp_reason = if effective.graph_workers != target.graph_workers {
            if target.graph_workers > seed.graph_workers {
                Some("startup_bound:graph_worker_pool_cannot_exceed_bootstrap")
            } else if seed.graph_workers > 0 && effective.graph_workers == 0 {
                Some("hard_safety_clamp:graph_embeddings_disabled")
            } else {
                Some("runtime_tuning:graph_worker_pool_admission_reduced")
            }
        } else {
            None
        };

        let chunk_batch_clamp_reason = if effective.chunk_batch_size != target.chunk_batch_size {
            Some("clamp_visible:effective_chunk_batch_size_diverges_from_target")
        } else {
            None
        };

        let file_batch_clamp_reason =
            if effective.file_vectorization_batch_size != target.file_vectorization_batch_size {
                Some("clamp_visible:effective_file_vectorization_batch_size_diverges_from_target")
            } else {
                None
            };

        json!({
            "vector_workers": Self::lane_parameter_snapshot(
                seed.vector_workers,
                target.vector_workers,
                effective.vector_workers,
                vector_workers_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "graph_workers": Self::lane_parameter_snapshot(
                seed.graph_workers,
                target.graph_workers,
                effective.graph_workers,
                graph_workers_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "chunk_batch_size": Self::lane_parameter_snapshot(
                seed.chunk_batch_size,
                target.chunk_batch_size,
                effective.chunk_batch_size,
                chunk_batch_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "file_vectorization_batch_size": Self::lane_parameter_snapshot(
                seed.file_vectorization_batch_size,
                target.file_vectorization_batch_size,
                effective.file_vectorization_batch_size,
                file_batch_clamp_reason,
                "partially_unified",
                "runtime_tuning_controller",
                "embedding_lane_config",
            ),
            "vector_ready_queue_depth": Self::lane_parameter_snapshot(
                runtime_seed.vector_ready_queue_depth,
                target.vector_ready_queue_depth,
                vector_runtime.ready_queue_depth_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_ready_queue_depth",
            ),
            "vector_persist_queue_bound": Self::lane_parameter_snapshot(
                runtime_seed.vector_persist_queue_bound,
                target.vector_persist_queue_bound,
                vector_runtime.persist_queue_depth_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_persist_queue_depth",
            ),
            "vector_max_inflight_persists": Self::lane_parameter_snapshot(
                runtime_seed.vector_max_inflight_persists,
                target.vector_max_inflight_persists,
                vector_runtime.persist_claimed_current as usize,
                None,
                "partially_unified",
                "runtime_tuning_controller",
                "service_guard.current_persist_claims",
            ),
            "queue_persist_effective_semantics": {
                "vector_ready_queue_depth": "observed_current_queue_depth_not_capacity",
                "vector_persist_queue_bound": "observed_current_queue_depth_not_capacity",
                "vector_max_inflight_persists": "observed_current_inflight_not_limit"
            },
            "semantic_cadence": {
                "seed": {
                    "profile": cadence_seed.profile,
                    "pause": cadence_seed.pause,
                    "sleep_ms": cadence_seed.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_seed.idle_sleep.as_millis() as u64,
                },
                "target": {
                    "profile": cadence_target.profile,
                    "pause": cadence_target.pause,
                    "sleep_ms": cadence_target.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_target.idle_sleep.as_millis() as u64,
                },
                "effective": {
                    "profile": cadence_effective.profile,
                    "pause": cadence_effective.pause,
                    "sleep_ms": cadence_effective.sleep.as_millis() as u64,
                    "idle_sleep_ms": cadence_effective.idle_sleep.as_millis() as u64,
                },
                "clamp_visible": cadence_effective.sleep != cadence_target.sleep
                    || cadence_effective.idle_sleep != cadence_target.idle_sleep,
                "clamp_reason": if cadence_effective.sleep != cadence_target.sleep
                    || cadence_effective.idle_sleep != cadence_target.idle_sleep
                {
                    json!("runtime_tuning_scale_pct")
                } else {
                    Value::Null
                },
                "authority_state": "partially_unified",
                "target_source": "semantic_policy_controller",
                "effective_source": "runtime_tuning_scaled_policy",
                "controller_state": batch_controller.state.as_str(),
                "controller_reason": batch_controller.reason,
            },
            "gpu_vector_lease": {
                "exclusive_required": gpu_vector_lease.exclusive_required,
                "path": gpu_vector_lease.path,
                "owned_by_current_instance": gpu_vector_lease.owned_by_current_instance,
                "owner_identity": gpu_vector_lease.owner_identity
            }
        })
    }
}
