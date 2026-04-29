use crate::embedder::{
    current_embedding_provider_diagnostics, current_runtime_tuning_state,
    embedding_lane_config_from_env,
};
use crate::optimizer;
use crate::runtime_mode::AxonRuntimeMode;
use crate::service_guard;
use crate::vector_control::{
    current_gpu_vector_lease_diagnostics, current_utility_first_scheduler_diagnostics,
    current_vector_batch_controller_diagnostics,
};
use serde_json::{json, Value};

use super::McpServer;

impl McpServer {
    fn env_interval_ms(key: &str, default: u64, min: u64) -> u64 {
        std::env::var(key)
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|value| *value >= min)
            .unwrap_or(default)
    }

    pub(super) fn quiescent_runtime_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        semantic_profile: &'static str,
        canonical_chunks_embedded_last_minute: u64,
        canonical_files_embedded_last_minute: u64,
    ) -> Value {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let interactive_priority = service_guard::current_interactive_priority();
        let interactive_requests = service_guard::interactive_requests_in_flight();
        let pressure = service_guard::current_pressure();
        let pressure_label = match pressure {
            service_guard::ServicePressure::Healthy => "healthy",
            service_guard::ServicePressure::Recovering => "recovering",
            service_guard::ServicePressure::Degraded => "degraded",
            service_guard::ServicePressure::Critical => "critical",
        };
        let vector_runtime = service_guard::vector_runtime_metrics();
        let embedding_provider = current_embedding_provider_diagnostics();
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let wake_summary = service_guard::runtime_wake_summary(
            structural_graph_backlog_depth as u64,
            file_backlog_depth as u64,
        );
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            structural_graph_backlog_depth,
            file_backlog_depth,
            pressure,
        );
        let dominant_wake_source = [
            ("background", wake_summary.wake_source_background_total),
            (
                "semantic_vector",
                wake_summary.wake_source_semantic_vector_total,
            ),
            ("graph", wake_summary.wake_source_graph_total),
        ]
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(label, _)| label)
        .unwrap_or("background");
        let dominant_wake_total = match dominant_wake_source {
            "semantic_vector" => wake_summary.wake_source_semantic_vector_total,
            "graph" => wake_summary.wake_source_graph_total,
            _ => wake_summary.wake_source_background_total,
        };
        let total_wake_sources = wake_summary
            .wake_source_background_total
            .saturating_add(wake_summary.wake_source_semantic_vector_total)
            .saturating_add(wake_summary.wake_source_graph_total);
        let dominant_wake_share_pct = if total_wake_sources > 0 {
            dominant_wake_total.saturating_mul(100) / total_wake_sources
        } else {
            0
        };
        let wake_noise_level = match wake_summary.wakeups_last_60s {
            0..=2 => "low",
            3..=10 => "moderate",
            _ => "high",
        };
        let vector_worker_heartbeat_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_worker_heartbeat_at_ms);
        let vector_lane_last_success_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_lane_last_success_at_ms);
        let vector_lane_last_fault_age_ms =
            now_ms.saturating_sub(vector_runtime.vector_lane_last_fault_at_ms);
        let reader_refresh_interval_ms =
            Self::env_interval_ms("AXON_READER_REFRESH_INTERVAL_MS", 5_000, 250);
        let shadow_optimizer_enabled = crate::main_background::shadow_optimizer_enabled();
        let optimizer_loop_interval_ms =
            Self::env_interval_ms("AXON_OPT_LOOP_INTERVAL_MS", 15_000, 1);
        let runtime_trace_interval_ms =
            Self::env_interval_ms("AXON_RUNTIME_TRACE_INTERVAL_MS", 5_000, 1_000);
        let watcher_promoter_poll_interval_ms = 50_u64;

        let runtime_mode_kind = AxonRuntimeMode::from_str(runtime_mode);
        let (effective_graph_backlog_depth, effective_semantic_backlog_depth, backlog_scope) =
            if !runtime_mode_kind.ingestion_enabled() {
                (0_usize, 0_usize, "runtime_processing_disabled")
            } else if !runtime_mode_kind.semantic_workers_enabled() {
                (structural_graph_backlog_depth, 0_usize, "indexer_graph")
            } else {
                (
                    structural_graph_backlog_depth,
                    if semantic_backlog_responsible {
                        file_backlog_depth
                    } else {
                        0_usize
                    },
                    if semantic_backlog_responsible {
                        "graph_and_semantic"
                    } else {
                        "indexer_graph_current_instance_responsibility"
                    },
                )
            };

        let state = if interactive_requests > 0 {
            "interactive_guarded"
        } else if effective_graph_backlog_depth > 0 || effective_semantic_backlog_depth > 0 {
            "active_backlog"
        } else if vector_runtime.ready_queue_chunks_current > 0
            || vector_runtime.prepare_inflight_chunks_current > 0
            || vector_runtime.ready_replenishment_deficit_current > 0
            || vector_runtime.prepare_queue_depth_current > 0
            || vector_runtime.persist_queue_depth_current > 0
            || vector_runtime.active_claimed_current > 0
        {
            "draining_residual_work"
        } else {
            "quiescent_candidate"
        };
        let healthy_semantic_drain_candidate = state == "active_backlog"
            && effective_semantic_backlog_depth > 0
            && vector_runtime.vector_lane_state == service_guard::VectorLaneState::Healthy
            && vector_worker_heartbeat_age_ms <= 5_000
            && vector_lane_last_success_age_ms <= 15_000
            && vector_runtime.active_claimed_current > 0;
        let semantic_progress_measured =
            canonical_files_embedded_last_minute > 0 || canonical_chunks_embedded_last_minute > 0;
        let provider_effective_is_gpu = embedding_provider
            .provider_effective
            .trim()
            .to_ascii_lowercase()
            .starts_with("cuda")
            || embedding_provider
                .provider_effective
                .trim()
                .to_ascii_lowercase()
                .starts_with("tensorrt");
        let semantic_drain_health = if !semantic_backlog_responsible
            || effective_semantic_backlog_depth == 0
        {
            if provider_effective_is_gpu && file_backlog_depth > 0 {
                "gpu_lease_not_owned"
            } else {
                "not_applicable"
            }
        } else if embedding_provider.provider_effective == "cpu" && gpu_access_policy == "avoid" {
            "cpu_policy_limited"
        } else if healthy_semantic_drain_candidate && semantic_progress_measured {
            "healthy_draining"
        } else if healthy_semantic_drain_candidate {
            "warming_or_long_batch"
        } else if utility_scheduler.semantic_underfeed
            || (vector_runtime.active_claimed_current == 0
                && vector_runtime.ready_queue_chunks_current == 0
                && vector_runtime.prepare_inflight_chunks_current == 0
                && vector_runtime.prepare_queue_depth_current == 0
                && vector_runtime.persist_queue_depth_current == 0)
        {
            "underfed"
        } else if vector_lane_last_success_age_ms > 30_000
            && vector_worker_heartbeat_age_ms > 10_000
        {
            "stalled"
        } else {
            "draining_uncertain"
        };
        let semantic_drain_recommendation = match semantic_drain_health {
            "healthy_draining" => "measure_backlog_burn_rate",
            "gpu_lease_not_owned" => "qualify_on_gpu_owner_instance_or_release_lease",
            "cpu_policy_limited" => "use_gpu_qualified_runtime_for_throughput_measurement",
            "warming_or_long_batch" => "extend_burn_rate_probe_before_calling_stall",
            "underfed" => "investigate_semantic_feed_path",
            "stalled" => "investigate_semantic_lane_stall",
            "draining_uncertain" => "observe_drain_progress_over_time",
            _ => "not_applicable",
        };
        let estimated_semantic_backlog_drain_minutes =
            if effective_semantic_backlog_depth > 0 && canonical_files_embedded_last_minute > 0 {
                Some(
                    ((effective_semantic_backlog_depth as f64)
                        / (canonical_files_embedded_last_minute as f64))
                        .ceil(),
                )
            } else {
                None
            };
        let burn_rate_state = match semantic_drain_health {
            "not_applicable" => "not_applicable",
            "healthy_draining"
                if canonical_files_embedded_last_minute > 0
                    || canonical_chunks_embedded_last_minute > 0 =>
            {
                "measurable_progress"
            }
            "healthy_draining" => "warming_or_long_batch",
            "underfed" => "not_progressing_underfed",
            "stalled" => "not_progressing_stalled",
            _ => "progress_uncertain",
        };
        let burn_rate_recommendation = match burn_rate_state {
            "measurable_progress" => "track_burn_rate_until_backlog_turns_down",
            "warming_or_long_batch" => "observe_one_more_measurement_window",
            "not_progressing_underfed" => "repair_semantic_feed_before_idle_tuning",
            "not_progressing_stalled" => "repair_semantic_lane_before_idle_tuning",
            "progress_uncertain" => "observe_another_window_before_concluding",
            _ => "not_applicable",
        };
        let healthy_semantic_drain_in_progress = semantic_drain_health == "healthy_draining";
        let operator_focus = match (
            dominant_wake_source,
            wake_summary.dominant_background_wake_detail,
            wake_summary.last_quiescent_exit_reason,
            state,
        ) {
            _ if semantic_drain_health == "gpu_lease_not_owned" => {
                "gpu_vector_lease_not_owned_by_current_instance"
            }
            _ if semantic_drain_health == "cpu_policy_limited" => {
                "cpu_vector_lane_policy_limits_throughput"
            }
            _ if healthy_semantic_drain_in_progress => "healthy_semantic_drain_in_progress",
            _ if semantic_drain_health == "warming_or_long_batch" => {
                "semantic_drain_long_batch_probe_in_progress"
            }
            ("background", "federation_orchestrator", _, "active_backlog") => {
                "project_discovery_is_active_under_backlog"
            }
            ("background", "ingress_promoter", _, _) => {
                "ingress_promoter_dominates_background_wakes"
            }
            ("background", "autonomous_ingestor", _, _) => {
                "autonomous_ingestor_dominates_background_wakes"
            }
            ("background", _, _, "quiescent_candidate") => "background_loops_still_dominate_idle",
            ("semantic_vector", _, _, "quiescent_candidate") => {
                "semantic_lane_still_wakes_while_idle"
            }
            ("graph", _, _, "quiescent_candidate") => "graph_lane_still_wakes_while_idle",
            (_, _, "interactive_guarded", _) => "interactive_load_breaks_quiescence",
            (_, _, "draining_residual_work", _) => "residual_work_prevents_full_quiescence",
            (_, _, "active_backlog", _) => "backlog_prevents_quiescence",
            _ => "quiescent_state_nominal",
        };
        let focus_recommendation = match operator_focus {
            "gpu_vector_lease_not_owned_by_current_instance" => {
                "qualify_on_gpu_owner_instance_or_release_lease"
            }
            "cpu_vector_lane_policy_limits_throughput" => {
                "qualify_on_gpu_enabled_runtime_or_override_policy"
            }
            "healthy_semantic_drain_in_progress" => "measure_backlog_burn_rate_not_idle_tuning",
            "semantic_drain_long_batch_probe_in_progress" => {
                "extend_burn_rate_probe_before_calling_stall"
            }
            "project_discovery_is_active_under_backlog" => {
                "measure_again_after_registry_stabilizes"
            }
            "ingress_promoter_dominates_background_wakes" => "tighten_ingress_promoter_first",
            "autonomous_ingestor_dominates_background_wakes" => "tighten_autonomous_ingestor_first",
            "background_loops_still_dominate_idle" => "tighten_background_pollers_first",
            "semantic_lane_still_wakes_while_idle" => "tighten_semantic_lane_idle_first",
            "graph_lane_still_wakes_while_idle" => "tighten_graph_lane_idle_first",
            "interactive_load_breaks_quiescence" => "measure_under_lower_interactive_load",
            "residual_work_prevents_full_quiescence" => "drain_residual_queues_before_idle_tuning",
            "backlog_prevents_quiescence" => "measure_again_after_backlog_drain",
            _ => "no_immediate_quiescent_hotspot_detected",
        };
        let confidence = if total_wake_sources < 3 {
            "low"
        } else if dominant_wake_share_pct >= 70 {
            "high"
        } else if dominant_wake_share_pct >= 50 {
            "medium"
        } else {
            "low"
        };
        let measurement_readiness = match (state, confidence, wake_noise_level) {
            ("quiescent_candidate", "high", _) => "actionable",
            ("quiescent_candidate", "medium", "low" | "moderate") => "actionable_with_caution",
            ("quiescent_candidate", _, "high") => "observe_longer_before_tuning",
            ("active_backlog", _, _) => "blocked_by_backlog",
            ("draining_residual_work", _, _) => "blocked_by_residual_work",
            ("interactive_guarded", _, _) => "blocked_by_interactive_load",
            _ => "observe_longer_before_tuning",
        };
        let measurement_readiness = if operator_focus == "project_discovery_is_active_under_backlog"
        {
            "blocked_by_project_discovery"
        } else if operator_focus == "gpu_vector_lease_not_owned_by_current_instance" {
            "blocked_by_gpu_vector_lease"
        } else if operator_focus == "cpu_vector_lane_policy_limits_throughput" {
            "blocked_by_cpu_vector_policy"
        } else if operator_focus == "semantic_drain_long_batch_probe_in_progress" {
            "blocked_by_long_batch_probe"
        } else if operator_focus == "healthy_semantic_drain_in_progress" {
            "blocked_by_healthy_semantic_drain"
        } else {
            measurement_readiness
        };
        let recommended_next_measurement = match measurement_readiness {
            "actionable" => "tune_dominant_wake_source_now",
            "actionable_with_caution" => "tune_gently_and_remeasure",
            "blocked_by_gpu_vector_lease" => "qualify_on_gpu_owner_instance",
            "blocked_by_cpu_vector_policy" => "qualify_on_gpu_enabled_runtime",
            "blocked_by_healthy_semantic_drain" => "measure_semantic_backlog_burn_rate",
            "blocked_by_long_batch_probe" => "extend_semantic_burn_rate_probe",
            "blocked_by_project_discovery" => "rerun_after_registry_stabilizes",
            "blocked_by_backlog" => "rerun_after_backlog_drain",
            "blocked_by_residual_work" => "rerun_after_residual_drain",
            "blocked_by_interactive_load" => "rerun_under_lower_interactive_load",
            _ => "extend_observation_window",
        };
        let qualification_verdict = match (state, confidence, wake_noise_level) {
            ("quiescent_candidate", "high", "low" | "moderate") => "pass",
            ("quiescent_candidate", "medium", "low" | "moderate") => "watch",
            ("quiescent_candidate", _, "high") => "watch",
            ("active_backlog", _, _) => "blocked",
            ("draining_residual_work", _, _) => "blocked",
            ("interactive_guarded", _, _) => "blocked",
            _ => "watch",
        };
        let qualification_reason = match qualification_verdict {
            "pass" => "quiescent_state_is_stable_enough_to_act",
            "blocked" => measurement_readiness,
            _ => "signal_exists_but_is_not_yet_strong_enough",
        };
        let mut blocking_factors = Vec::new();
        if operator_focus == "healthy_semantic_drain_in_progress" {
            blocking_factors.push("healthy_semantic_drain_active");
        }
        if operator_focus == "gpu_vector_lease_not_owned_by_current_instance" {
            blocking_factors.push("gpu_vector_lease_not_owned");
        }
        if operator_focus == "cpu_vector_lane_policy_limits_throughput" {
            blocking_factors.push("cpu_vector_policy_active");
        }
        if operator_focus == "semantic_drain_long_batch_probe_in_progress" {
            blocking_factors.push("long_batch_probe_active");
        }
        if operator_focus == "project_discovery_is_active_under_backlog" {
            blocking_factors.push("project_discovery_active");
        }
        if state == "active_backlog" {
            blocking_factors.push("backlog_active");
        }
        if state == "draining_residual_work" {
            blocking_factors.push("residual_work_active");
        }
        if state == "interactive_guarded" {
            blocking_factors.push("interactive_load_active");
        }
        if confidence == "low" {
            blocking_factors.push("low_confidence_signal");
        }
        if wake_noise_level == "high" {
            blocking_factors.push("high_wake_noise");
        }
        let actionable_now = matches!(
            measurement_readiness,
            "actionable" | "actionable_with_caution"
        );

        json!({
            "state": state,
            "authority_state": "transitional",
            "wake_contract_state": "fragmented",
            "wake_observability_state": "partial",
            "diagnosis": {
                "operator_focus": operator_focus,
                "focus_recommendation": focus_recommendation,
                "dominant_background_wake_detail": wake_summary.dominant_background_wake_detail,
                "confidence": confidence,
                "wake_noise_level": wake_noise_level,
                "dominant_wake_share_pct": dominant_wake_share_pct,
                "measurement_readiness": measurement_readiness,
                "recommended_next_measurement": recommended_next_measurement,
                "qualification_verdict": qualification_verdict,
                "qualification_reason": qualification_reason,
                "actionable_now": actionable_now,
                "blocking_factors": blocking_factors
            },
            "interactive_priority": interactive_priority.as_str(),
            "interactive_requests_in_flight": interactive_requests,
            "service_pressure": pressure_label,
            "backlog_scope": backlog_scope,
            "semantic_backlog_responsible": semantic_backlog_responsible,
            "graph_backlog_depth": structural_graph_backlog_depth,
            "graph_projection_queue_depth": graph_projection_queue_depth,
            "semantic_backlog_depth": file_backlog_depth,
            "effective_graph_backlog_depth": effective_graph_backlog_depth,
            "effective_semantic_backlog_depth": effective_semantic_backlog_depth,
            "semantic_policy_profile": semantic_profile,
            "backlog_drain": {
                "semantic_health": semantic_drain_health,
                "recommendation": semantic_drain_recommendation,
                "utility_scheduler_state": utility_scheduler.state.as_str(),
                "utility_scheduler_reason": utility_scheduler.reason,
                "semantic_underfeed": utility_scheduler.semantic_underfeed,
                "ready_reserve_target": utility_scheduler.ready_reserve_target,
                "target_ready_chunks": utility_scheduler.target_ready_chunks,
                "provider_requested": embedding_provider.provider_requested,
                "provider_effective": embedding_provider.provider_effective,
                "gpu_access_policy": gpu_access_policy,
                "vector_lane_state": vector_runtime.vector_lane_state.as_str(),
                "vector_worker_heartbeat_age_ms": vector_worker_heartbeat_age_ms,
                "vector_lane_last_success_age_ms": vector_lane_last_success_age_ms,
                "active_claimed_current": vector_runtime.active_claimed_current,
                "prepare_queue_depth_current": vector_runtime.prepare_queue_depth_current,
                "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                "persist_queue_depth_current": vector_runtime.persist_queue_depth_current,
                "files_completed_total": vector_runtime.files_completed_total,
                "chunks_embedded_total": vector_runtime.chunks_embedded_total,
                "burn_rate": {
                    "measurement_window_sec": 60,
                    "state": burn_rate_state,
                    "recommendation": burn_rate_recommendation,
                    "files_vector_ready_last_minute": canonical_files_embedded_last_minute,
                    "chunks_embedded_last_minute": canonical_chunks_embedded_last_minute,
                    "effective_semantic_backlog_depth": effective_semantic_backlog_depth,
                    "estimated_semantic_backlog_drain_minutes": estimated_semantic_backlog_drain_minutes
                }
            },
            "loop_intervals_ms": {
                "reader_refresh": reader_refresh_interval_ms,
                "optimizer_loop": if shadow_optimizer_enabled { json!(optimizer_loop_interval_ms) } else { Value::Null },
                "runtime_trace": runtime_trace_interval_ms,
                "ingress_promoter_poll": watcher_promoter_poll_interval_ms
            },
            "background_controls": {
                "shadow_optimizer_enabled": shadow_optimizer_enabled
            },
            "wake_activity": {
                "wakeups_last_60s": wake_summary.wakeups_last_60s,
                "last_wakeup_at_ms": wake_summary.last_wakeup_at_ms,
                "quiescent_entered_at_ms": wake_summary.quiescent_entered_at_ms,
                "last_quiescent_exited_at_ms": wake_summary.last_quiescent_exited_at_ms,
                "quiescent_dwell_ms_current": wake_summary.quiescent_dwell_ms_current,
                "resume_latency_samples": wake_summary.resume_latency_samples,
                "resume_latency_p50_ms": wake_summary.resume_latency_p50_ms,
                "resume_latency_p95_ms": wake_summary.resume_latency_p95_ms,
                "resume_latency_max_ms": wake_summary.resume_latency_max_ms,
                "useful_resume_latency_samples": wake_summary.useful_resume_latency_samples,
                "useful_resume_latency_p50_ms": wake_summary.useful_resume_latency_p50_ms,
                "useful_resume_latency_p95_ms": wake_summary.useful_resume_latency_p95_ms,
                "useful_resume_latency_max_ms": wake_summary.useful_resume_latency_max_ms,
                "last_useful_resume_at_ms": wake_summary.last_useful_resume_at_ms,
                "last_quiescent_exit_reason": wake_summary.last_quiescent_exit_reason,
                "exit_due_to_active_backlog_total": wake_summary.exit_due_to_active_backlog_total,
                "exit_due_to_draining_residual_total": wake_summary.exit_due_to_draining_residual_total,
                "exit_due_to_interactive_guarded_total": wake_summary.exit_due_to_interactive_guarded_total,
                "last_wake_source": wake_summary.last_wake_source,
                "dominant_wake_source": dominant_wake_source,
                "last_background_wake_detail": wake_summary.last_background_wake_detail,
                "dominant_background_wake_detail": wake_summary.dominant_background_wake_detail,
                "wake_source_background_total": wake_summary.wake_source_background_total,
                "wake_source_semantic_vector_total": wake_summary.wake_source_semantic_vector_total,
                "wake_source_graph_total": wake_summary.wake_source_graph_total,
                "background_wake_memory_reclaimer_total": wake_summary.background_wake_memory_reclaimer_total,
                "background_wake_shadow_optimizer_total": wake_summary.background_wake_shadow_optimizer_total,
                "background_wake_runtime_trace_total": wake_summary.background_wake_runtime_trace_total,
                "background_wake_reader_refresh_total": wake_summary.background_wake_reader_refresh_total,
                "background_wake_autonomous_ingestor_total": wake_summary.background_wake_autonomous_ingestor_total,
                "background_wake_ingress_promoter_total": wake_summary.background_wake_ingress_promoter_total,
                "background_wake_federation_orchestrator_total": wake_summary.background_wake_federation_orchestrator_total
            },
            "lane_liveness": {
                "vector_worker_heartbeat_at_ms": vector_runtime.vector_worker_heartbeat_at_ms,
                "vector_worker_heartbeat_age_ms": vector_worker_heartbeat_age_ms,
                "vector_lane_state": vector_runtime.vector_lane_state.as_str(),
                "vector_lane_last_success_at_ms": vector_runtime.vector_lane_last_success_at_ms,
                "vector_lane_last_success_age_ms": vector_lane_last_success_age_ms,
                "vector_lane_last_fault_at_ms": vector_runtime.vector_lane_last_fault_at_ms,
                "vector_lane_last_fault_age_ms": vector_lane_last_fault_age_ms
            },
            "observed_residual_work": {
                "prepare_queue_depth_current": vector_runtime.prepare_queue_depth_current,
                "ready_queue_depth_current": vector_runtime.ready_queue_depth_current,
                "persist_queue_depth_current": vector_runtime.persist_queue_depth_current,
                "active_claimed_current": vector_runtime.active_claimed_current
            }
        })
    }

    pub(super) fn runtime_limiting_factors_snapshot(
        runtime_mode: &str,
        semantic_backlog_responsible: bool,
        structural_graph_backlog_depth: usize,
        graph_projection_queue_depth: usize,
        file_backlog_depth: usize,
        runtime_signals: &optimizer::RuntimeSignalsWindow,
    ) -> Value {
        let host = optimizer::collect_host_snapshot();
        let policy = optimizer::collect_operator_policy_snapshot(&host);
        let effective = embedding_lane_config_from_env();
        let runtime_target = current_runtime_tuning_state();
        let batch_controller = current_vector_batch_controller_diagnostics(&effective);
        let utility_scheduler = current_utility_first_scheduler_diagnostics(
            structural_graph_backlog_depth,
            file_backlog_depth,
            service_guard::current_pressure(),
        );
        let embedding_provider = current_embedding_provider_diagnostics();
        let gpu_access_policy =
            std::env::var("AXON_GPU_ACCESS_POLICY").unwrap_or_else(|_| "unknown".to_string());
        let gpu_vector_lease = current_gpu_vector_lease_diagnostics();
        let avg_chunk_density_ratio = if batch_controller.target_embed_batch_chunks > 0 {
            (batch_controller.avg_chunks_per_embed_call
                / batch_controller.target_embed_batch_chunks as f64)
                .clamp(0.0, 4.0)
        } else {
            0.0
        };
        let ready_buffer_thin = runtime_signals.ready_queue_chunks_current
            < utility_scheduler.ready_reserve_target as u64;
        let prepare_pipeline_shallow = runtime_signals.prepare_inflight_chunks_current
            <= batch_controller.target_embed_batch_chunks as u64
            && runtime_signals
                .ready_queue_chunks_current
                .saturating_add(runtime_signals.prepare_inflight_chunks_current)
                < utility_scheduler.target_ready_chunks as u64;
        let ram_pressure = runtime_signals.ram_available_ratio > 0.0
            && runtime_signals.ram_available_ratio < policy.min_ram_available_ratio;
        let vram_pressure =
            policy.max_vram_used_mb > 0 && runtime_signals.vram_used_mb >= policy.max_vram_used_mb;
        let persist_congested = runtime_signals.persist_queue_depth_current
            >= runtime_target.vector_persist_queue_bound as u64
            || runtime_signals.persist_claimed_current
                >= runtime_target.vector_max_inflight_persists as u64;
        let gpu_effective = embedding_provider
            .provider_effective
            .eq_ignore_ascii_case("cuda");
        let gpu_compute_bound = gpu_effective
            && file_backlog_depth > 0
            && runtime_signals.gpu_utilization_ratio >= 0.80
            && !vram_pressure;
        let cpu_prepare_underfeed = gpu_effective
            && file_backlog_depth > 0
            && runtime_signals.gpu_utilization_ratio <= 0.45
            && !vram_pressure
            && runtime_signals.cpu_usage_ratio < policy.max_cpu_ratio * 0.75
            && runtime_signals.ram_available_ratio > policy.min_ram_available_ratio
            && (ready_buffer_thin || prepare_pipeline_shallow);
        let batch_density_collapse = gpu_effective
            && file_backlog_depth > 0
            && batch_controller.window_embed_calls > 0
            && avg_chunk_density_ratio < 0.60;

        let mut secondary = Vec::new();
        let (primary, reason, actionable) = if runtime_signals.interactive_requests_in_flight > 0 {
            (
                "interactive_guarded",
                "interactive traffic currently suppresses aggressive backlog tuning",
                false,
            )
        } else if ram_pressure {
            (
                "ram_bound",
                "available RAM is below the operator policy floor",
                true,
            )
        } else if vram_pressure {
            (
                "vram_bound",
                "VRAM usage is at or above the operator safety budget",
                true,
            )
        } else if persist_congested {
            (
                "persist_congested",
                "persist queue depth or inflight persists reached the current bound",
                true,
            )
        } else if semantic_backlog_responsible
            && !gpu_effective
            && gpu_access_policy.eq_ignore_ascii_case("avoid")
        {
            (
                "cpu_vector_policy_limited",
                "the current runtime policy keeps semantic vectorization on CPU",
                true,
            )
        } else if semantic_backlog_responsible
            && !gpu_effective
            && gpu_vector_lease.exclusive_required
            && !gpu_vector_lease.owned_by_current_instance
        {
            (
                "gpu_scaling_blocked",
                "the current instance does not own the exclusive GPU vector lease",
                true,
            )
        } else if batch_density_collapse {
            (
                "batch_density_collapse",
                "GPU batches are materially thinner than the controller target",
                true,
            )
        } else if cpu_prepare_underfeed {
            (
                "cpu_prepare_underfeed",
                "GPU demand is higher than the current CPU-side prepare and ready pipeline feed",
                true,
            )
        } else if gpu_compute_bound {
            (
                "gpu_compute_bound",
                "GPU utilization is already high while backlog remains active",
                false,
            )
        } else if file_backlog_depth == 0 && structural_graph_backlog_depth == 0 {
            (
                "not_currently_limited",
                "no active graph or semantic backlog is currently competing for throughput",
                false,
            )
        } else {
            (
                "not_currently_limited",
                "no single hard limiter is currently dominant above the observation threshold",
                false,
            )
        };

        if ram_pressure && primary != "ram_bound" {
            secondary.push("ram_bound");
        }
        if vram_pressure && primary != "vram_bound" {
            secondary.push("vram_bound");
        }
        if persist_congested && primary != "persist_congested" {
            secondary.push("persist_congested");
        }
        if batch_density_collapse && primary != "batch_density_collapse" {
            secondary.push("batch_density_collapse");
        }
        if cpu_prepare_underfeed && primary != "cpu_prepare_underfeed" {
            secondary.push("cpu_prepare_underfeed");
        }
        if gpu_compute_bound && primary != "gpu_compute_bound" {
            secondary.push("gpu_compute_bound");
        }

        json!({
            "primary": primary,
            "secondary": secondary,
            "actionable": actionable,
            "reason": reason,
            "controller_reason": batch_controller.reason,
            "scheduler_reason": utility_scheduler.reason,
            "runtime_mode": runtime_mode,
            "signals": {
                "cpu_usage_ratio": runtime_signals.cpu_usage_ratio,
                "ram_available_ratio": runtime_signals.ram_available_ratio,
                "gpu_utilization_ratio": runtime_signals.gpu_utilization_ratio,
                "vram_used_mb": runtime_signals.vram_used_mb,
                "vram_total_mb": host.vram_total_mb,
                "ready_queue_depth_current": runtime_signals.ready_queue_depth_current,
                "ready_queue_chunks_current": runtime_signals.ready_queue_chunks_current,
                "prepare_inflight_current": runtime_signals.prepare_inflight_current,
                "prepare_inflight_chunks_current": runtime_signals.prepare_inflight_chunks_current,
                "ready_replenishment_deficit_current": runtime_signals.ready_replenishment_deficit_current,
                "prepare_claimed_current": runtime_signals.prepare_claimed_current,
                "persist_queue_depth_current": runtime_signals.persist_queue_depth_current,
                "avg_chunks_per_embed_call": batch_controller.avg_chunks_per_embed_call,
                "target_embed_batch_chunks": batch_controller.target_embed_batch_chunks,
                "target_files_per_cycle": batch_controller.target_files_per_cycle,
                "gpu_ready_low_watermark_chunks": batch_controller.gpu_ready_low_watermark_chunks,
                "gpu_ready_high_watermark_chunks": batch_controller.gpu_ready_high_watermark_chunks,
                "ready_reserve_target": utility_scheduler.ready_reserve_target,
                "target_ready_chunks": utility_scheduler.target_ready_chunks,
                "semantic_backlog_depth": file_backlog_depth,
                "graph_backlog_depth": structural_graph_backlog_depth,
                "graph_projection_queue_depth": graph_projection_queue_depth
            },
            "thresholds": {
                "max_cpu_ratio": policy.max_cpu_ratio,
                "min_ram_available_ratio": policy.min_ram_available_ratio,
                "max_vram_used_mb": policy.max_vram_used_mb,
                "density_collapse_ratio": 0.60,
                "gpu_underutilized_ratio": 0.45
            },
            "policy": {
                "gpu_access_policy": gpu_access_policy,
                "provider_requested": embedding_provider.provider_requested,
                "provider_effective": embedding_provider.provider_effective,
                "gpu_lease_owned_by_current_instance": gpu_vector_lease.owned_by_current_instance,
                "semantic_backlog_responsible": semantic_backlog_responsible
            }
        })
    }
}
