// Copyright (c) Didier Stadelmann. All rights reserved.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axon_core::embedder::{
    apply_runtime_embedding_lane_adjustment, current_embedding_provider_diagnostics,
    current_gpu_memory_snapshot, current_gpu_utilization_snapshot, embedding_lane_config_from_env,
};
use axon_core::graph::GraphStore;
use axon_core::optimizer::{
    build_admissible_action_profiles, collect_host_snapshot, collect_operator_policy_snapshot,
    collect_recent_analytics_window, collect_runtime_signals_window, observe_reward,
    HeuristicPolicyEngine, PolicyEngine,
};
use axon_core::queue::QueueStore;
use axon_core::runtime_observability::process_memory_snapshot;
use axon_core::service_guard;
use axon_core::service_guard::{InteractivePriority, RuntimeQuiescentState, ServicePressure};
use axon_core::vector_control::{
    current_utility_first_scheduler_diagnostics, current_vector_batch_controller_diagnostics,
    current_vector_drain_state,
};
use serde_json::json;
use tracing::{error, info, warn};

#[path = "main_background/host_pressure.rs"]
mod host_pressure;
#[path = "main_background/memory_config.rs"]
mod memory_config;

use host_pressure::sample_host_pressure;
use memory_config::{
    current_rss_bytes, memory_limit_bytes, memory_reclaimer_enabled,
    memory_reclaimer_min_anon_bytes, parse_rss_from_statm,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GovernorMode {
    Off,
    Shadow,
    Assist,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GovernorState {
    Shadow,
    Assist,
    Live,
    Freeze,
    Rollback,
    Disabled,
}

#[derive(Debug, Clone)]
struct GovernorLoopState {
    last_safe_profile_id: Option<String>,
    freeze_until_ms: i64,
    consecutive_zero_progress_windows: u64,
}

impl GovernorLoopState {
    fn new() -> Self {
        Self {
            last_safe_profile_id: None,
            freeze_until_ms: 0,
            consecutive_zero_progress_windows: 0,
        }
    }
}

const MEMORY_RECLAIMER_POLL_INTERVAL_SECS: u64 = 15;
const QUIESCENT_INTERVAL_SCALE_PCT_DEFAULT: usize = 400;

static OVERSIZED_REFUSALS_TOTAL: AtomicU64 = AtomicU64::new(0);
static DEGRADED_MODE_ENTRIES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_SUCCESSES_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(crate) struct RuntimeTelemetrySnapshot {
    pub budget_bytes: u64,
    pub reserved_bytes: u64,
    pub exhaustion_ratio: f64,
    pub reserved_task_count: usize,
    pub anonymous_trace_reserved_tasks: usize,
    pub anonymous_trace_admissions_total: u64,
    pub reservation_release_misses_total: u64,
    pub queue_depth: usize,
    pub claim_mode: String,
    pub service_pressure: String,
    pub interactive_priority_active: bool,
    pub interactive_priority_level: String,
    pub interactive_requests_in_flight: u64,
    pub oversized_refusals_total: u64,
    pub degraded_mode_entries_total: u64,
    pub background_launches_suppressed_total: u64,
    pub vectorization_suppressed_due_to_interactive: u64,
    pub vectorization_interrupted_due_to_interactive: u64,
    pub vectorization_requeued_for_interactive: u64,
    pub vectorization_resumed_after_interactive: u64,
    pub projection_suppressed_due_to_interactive: u64,
    // REQ-AXO-901893 (LEGACY FEED PURGE) — the FileIngressGuard (`guard_*`) and
    // in-memory ingress_buffer (`ingress_*`) telemetry fields were ripped with
    // their backing modules. The Watchman file source feeds pipeline A directly
    // (no buffer to meter); DBQ-A is the backlog drainer. The dashboard decodes
    // these via Map.get(.., default) so their absence degrades gracefully to 0.
    pub memory_trim_attempts_total: u64,
    pub memory_trim_successes_total: u64,
    pub cpu_load: f64,
    pub ram_load: f64,
    pub io_wait: f64,
    pub host_state: String,
    pub host_guidance_slots: usize,
    pub rss_bytes: u64,
    pub rss_anon_bytes: u64,
    pub rss_file_bytes: u64,
    pub rss_shmem_bytes: u64,
    // REQ-AXO-284 Slice 2 — PG health metrics. `Option` so transient
    // catalog miss doesn't poison the telemetry payload.
    pub pg_database_bytes: Option<i64>,
    pub pg_chunkembedding_total_bytes: Option<i64>,
    pub pg_wal_bytes: Option<i64>,
    pub pg_buffer_hit_ratio: Option<f64>,
    pub vector_chunks_embedded_cumulative: u64,
    pub chunk_embeddings_per_second: f64,
    pub chunk_embeddings_rate_window_ms: u64,
    pub prepare_inflight_chunks_current: u64,
    pub ready_queue_chunks_current: u64,
    pub ready_queue_chunks_small: u64,
    pub ready_queue_chunks_medium: u64,
    pub ready_queue_chunks_large: u64,
    pub ready_batches_small: u64,
    pub ready_batches_medium: u64,
    pub ready_batches_large: u64,
    pub mixed_fallback_batches_total: u64,
    pub homogeneous_batches_total: u64,
    pub last_consumed_batch_lane: String,
    pub active_small_max_tokens: u64,
    pub active_medium_max_tokens: u64,
    pub ready_replenishment_deficit_current: u64,
    pub oldest_ready_batch_age_ms_current: u64,
    pub last_embed_attempt_wall_ms: u64,
    pub avg_embed_attempt_wall_ms: f64,
    pub max_embed_attempt_wall_ms: u64,
    pub last_embed_gap_ms: u64,
    pub avg_embed_gap_ms: f64,
    pub max_embed_gap_ms: u64,
    pub graph_workers_started_total: u64,
    pub graph_workers_active_current: u64,
    pub graph_worker_heartbeat_at_ms: u64,
    pub runtime_truth_last_heartbeat_at_ms: u64,
    pub runtime_truth_last_good_payload_at_ms: u64,
    pub runtime_truth_stale_after_ms: u64,
    pub runtime_truth_degraded_reason: Option<String>,
    pub orphan_vectorization_files: usize,
    pub stale_vector_inflight_files: usize,
    pub oldest_graph_pending_age_ms: u64,
    pub oldest_semantic_pending_age_ms: u64,
    pub utility_first_scheduler_state: String,
    pub utility_first_scheduler_reason: String,
    pub semantic_underfeed: bool,
    pub semantic_ready_reserve_target: usize,
    pub utility_first_scheduler_hold_window_ms: u64,
}

pub(crate) fn start_memory_watchdog() {
    std::thread::spawn(|| {
        let page_size = 4096;
        let limit_bytes = memory_limit_bytes();
        let mut above_limit = false;
        loop {
            if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                if let Some(rss_pages) = parse_rss_from_statm(&content) {
                    let rss_bytes = rss_pages * page_size;
                    if rss_bytes > limit_bytes {
                        if !above_limit {
                            error!(
                            "CRITICAL: Memory threshold reached ({} GB). Holding runtime in degraded mode instead of suicide...",
                            rss_bytes / 1024 / 1024 / 1024
                            );
                            above_limit = true;
                        }
                    } else if above_limit {
                        warn!(
                            "Memory watchdog: RSS returned below threshold ({} GB).",
                            rss_bytes / 1024 / 1024 / 1024
                        );
                        above_limit = false;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(quiescent_scaled_interval_ms(
                10_000, 10_000, 120_000,
            )));
        }
    });
}

pub(crate) fn spawn_memory_reclaimer(queue: Arc<QueueStore>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(memory_reclaimer_poll_interval_ms()));
        service_guard::record_background_runtime_wakeup(
            service_guard::BackgroundWakeDetail::MemoryReclaimer,
            0,
            0,
        );

        if !memory_reclaimer_enabled() {
            continue;
        }

        let process_memory = process_memory_snapshot();
        let min_anon_bytes = memory_reclaimer_min_anon_bytes();

        if !should_attempt_memory_reclaim(queue.common_len(), process_memory, min_anon_bytes) {
            continue;
        }

        MEMORY_TRIM_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
        if axon_core::runtime_observability::malloc_trim_system_allocator() {
            MEMORY_TRIM_SUCCESSES_TOTAL.fetch_add(1, Ordering::Relaxed);
            info!(
                "Memory reclaimer trimmed system allocator after idle period (rss_anon={} MiB).",
                process_memory.rss_anon_bytes / 1024 / 1024
            );
        }
    });
}

pub(crate) fn spawn_shadow_optimizer(store: Arc<GraphStore>) {
    if !shadow_optimizer_enabled() {
        info!("Shadow optimizer disabled; graph-first runtime keeps optimizer off the hot path.");
        return;
    }
    std::thread::spawn(move || {
        let engine = HeuristicPolicyEngine;
        let mut governor = GovernorLoopState::new();
        let mut previous: Option<(
            String,
            axon_core::optimizer::RuntimeSignalsWindow,
            axon_core::optimizer::OperatorPolicySnapshot,
        )> = None;

        loop {
            service_guard::record_background_runtime_wakeup(
                service_guard::BackgroundWakeDetail::ShadowOptimizer,
                0,
                0,
            );
            let host = collect_host_snapshot();
            let policy = collect_operator_policy_snapshot(&host);
            let signals = collect_runtime_signals_window(&store);
            let analytics = collect_recent_analytics_window(&store);
            let action_profiles = build_admissible_action_profiles(&host, &signals, &policy);
            let reward = previous.as_ref().map(
                |(previous_decision_id, previous_signals, previous_policy)| {
                    observe_reward(
                        previous_decision_id,
                        previous_signals,
                        &signals,
                        previous_policy,
                        5.0,
                    )
                },
            );
            governor.consecutive_zero_progress_windows = reward
                .as_ref()
                .map(|value| {
                    let qualifies = signals.file_vectorization_queue_depth >= 32
                        && signals.ready_queue_chunks_current == 0
                        && signals.prepare_inflight_chunks_current == 0
                        && value.throughput_chunks_per_hour <= 0.0
                        && value.throughput_files_per_hour <= 0.0
                        && value.penalty_liveness == 0.0;
                    if qualifies {
                        governor.consecutive_zero_progress_windows.saturating_add(1)
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            if let Some(decision) =
                engine.choose(&host, &signals, &policy, &analytics, &action_profiles)
            {
                let current_profile_id = current_runtime_profile_id();
                let constraints = optimizer_constraint_flags(&signals, &policy);
                let governor_state = resolve_governor_state(
                    configured_governor_mode(),
                    &signals,
                    &policy,
                    reward.as_ref(),
                    governor.freeze_until_ms,
                    governor.consecutive_zero_progress_windows,
                    &current_profile_id,
                    &action_profiles,
                );
                let selected_profile = action_profiles
                    .iter()
                    .find(|profile| profile.id == decision.action_profile_id)
                    .cloned();
                let effective_profile = select_governor_profile(
                    governor_state,
                    &action_profiles,
                    selected_profile.as_ref(),
                    &current_profile_id,
                    governor.last_safe_profile_id.as_deref(),
                );

                let mut applied = false;
                if matches!(
                    governor_state,
                    GovernorState::Assist | GovernorState::Live | GovernorState::Rollback
                ) {
                    if let Some(profile) = effective_profile.as_ref() {
                        if profile.id != current_profile_id {
                            apply_live_optimizer_profile(profile, &policy);
                            applied = true;
                        }
                    }
                }
                if matches!(
                    governor_state,
                    GovernorState::Freeze | GovernorState::Rollback
                ) {
                    governor.freeze_until_ms = chrono::Utc::now()
                        .timestamp_millis()
                        .saturating_add(governor_freeze_cooldown_ms() as i64);
                }

                let mut effective_decision = decision.clone();
                if let Some(profile) = effective_profile.as_ref() {
                    effective_decision.action_profile_id = profile.id.clone();
                }
                effective_decision.decision_reason = format!(
                    "{}|governor:{}",
                    effective_decision.decision_reason,
                    governor_state_label(governor_state)
                );

                let host_json = serde_json::to_string(&host).unwrap_or_else(|_| "{}".to_string());
                let policy_json =
                    serde_json::to_string(&policy).unwrap_or_else(|_| "{}".to_string());
                let signals_json =
                    serde_json::to_string(&signals).unwrap_or_else(|_| "{}".to_string());
                let analytics_json =
                    serde_json::to_string(&analytics).unwrap_or_else(|_| "{}".to_string());
                let decision_json =
                    serde_json::to_string(&effective_decision).unwrap_or_else(|_| "{}".to_string());
                let constraints_json =
                    serde_json::to_string(&constraints).unwrap_or_else(|_| "[]".to_string());
                let would_apply = effective_profile
                    .as_ref()
                    .is_some_and(|profile| profile.id != current_profile_id);

                if let Err(err) = store.log_optimizer_decision(
                    &effective_decision.decision_id,
                    effective_decision.proposed_at_ms,
                    governor_state_label(governor_state),
                    &host_json,
                    &policy_json,
                    &signals_json,
                    &analytics_json,
                    &effective_decision.action_profile_id,
                    &decision_json,
                    &constraints_json,
                    would_apply,
                    applied,
                    effective_decision.evaluation_window_start_ms,
                    effective_decision.evaluation_window_end_ms,
                ) {
                    warn!("Governor: failed to persist decision log: {:?}", err);
                }

                if let Some(reward) = reward.as_ref() {
                    let reward_json =
                        serde_json::to_string(&reward).unwrap_or_else(|_| "{}".to_string());
                    let pressure_json = serde_json::json!({
                        "cpu_usage_ratio": signals.cpu_usage_ratio,
                        "ram_available_ratio": signals.ram_available_ratio,
                        "io_wait_ratio": signals.io_wait_ratio,
                        "vram_used_mb": signals.vram_used_mb,
                        "interactive_requests_in_flight": signals.interactive_requests_in_flight,
                        "vector_workers_active_current": signals.vector_workers_active_current
                    })
                    .to_string();
                    let violations_json = serde_json::json!({
                        "cpu": reward.penalty_cpu,
                        "ram": reward.penalty_ram,
                        "vram": reward.penalty_vram,
                        "mcp": reward.penalty_mcp,
                        "io": reward.penalty_io,
                        "liveness": reward.penalty_liveness,
                        "churn": reward.penalty_churn
                    })
                    .to_string();
                    if let Err(err) = store.log_reward_observation(
                        &reward.decision_id,
                        reward.observed_at_ms,
                        reward.window_start_ms,
                        reward.window_end_ms,
                        &reward_json,
                        reward.throughput_chunks_per_hour,
                        reward.throughput_files_per_hour,
                        &violations_json,
                        &pressure_json,
                    ) {
                        warn!("Governor: failed to persist reward log: {:?}", err);
                    }
                }

                if let Some(profile) = effective_profile.as_ref() {
                    if reward
                        .as_ref()
                        .is_none_or(|value| value.reward > 0.0 && value.penalty_liveness == 0.0)
                    {
                        governor.last_safe_profile_id = Some(profile.id.clone());
                    }
                }

                previous = Some((effective_decision.decision_id.clone(), signals, policy));
            }

            std::thread::sleep(Duration::from_millis(
                optimizer_loop_interval_ms().max(10_000),
            ));
        }
    });
}

pub(crate) fn shadow_optimizer_enabled() -> bool {
    std::env::var("AXON_ENABLE_SHADOW_OPTIMIZER")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(crate) fn spawn_runtime_trace_logger(
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
) {
    if !runtime_trace_enabled() {
        return;
    }

    let trace_path = runtime_trace_path();
    let interval = Duration::from_millis(runtime_trace_interval_ms());
    std::thread::spawn(move || {
        if let Some(parent) = trace_path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                warn!(
                    "Runtime trace: failed to create parent directory {}: {:?}",
                    parent.display(),
                    err
                );
                return;
            }
        }

        loop {
            let telemetry = runtime_telemetry_snapshot(&store, &queue);
            // REQ-AXO-901674 — FVQ/GPQ depths stubbed to 0 (tables dropped
            // slice-5d) ; wake detail signals are vestigial for these axes.
            service_guard::record_background_runtime_wakeup(
                service_guard::BackgroundWakeDetail::RuntimeTrace,
                0,
                0,
            );
            let signals = collect_runtime_signals_window(&store);
            let gpu_memory = current_gpu_memory_snapshot();
            let gpu_utilization = current_gpu_utilization_snapshot();
            let lane_config = embedding_lane_config_from_env();
            let provider = current_embedding_provider_diagnostics();
            let controller = current_vector_batch_controller_diagnostics(&lane_config);
            let line = json!({
                "captured_at_ms": chrono::Utc::now().timestamp_millis(),
                "runtime_telemetry": {
                    "queue_depth": telemetry.queue_depth,
                    "claim_mode": telemetry.claim_mode,
                    "service_pressure": telemetry.service_pressure,
                    "interactive_priority_level": telemetry.interactive_priority_level,
                    "interactive_requests_in_flight": telemetry.interactive_requests_in_flight,
                    "orphan_vectorization_files": telemetry.orphan_vectorization_files,
                    "stale_vector_inflight_files": telemetry.stale_vector_inflight_files,
                    "oldest_graph_pending_age_ms": telemetry.oldest_graph_pending_age_ms,
                    "oldest_semantic_pending_age_ms": telemetry.oldest_semantic_pending_age_ms,
                    "utility_first_scheduler_state": telemetry.utility_first_scheduler_state,
                    "utility_first_scheduler_reason": telemetry.utility_first_scheduler_reason,
                    "semantic_underfeed": telemetry.semantic_underfeed,
                    "semantic_ready_reserve_target": telemetry.semantic_ready_reserve_target,
                    "utility_first_scheduler_hold_window_ms": telemetry.utility_first_scheduler_hold_window_ms,
                    "runtime_truth_feed": {
                        "last_heartbeat_at_ms": telemetry.runtime_truth_last_heartbeat_at_ms,
                        "last_good_payload_at_ms": telemetry.runtime_truth_last_good_payload_at_ms,
                        "stale_after_ms": telemetry.runtime_truth_stale_after_ms,
                        "degraded_reason": telemetry.runtime_truth_degraded_reason,
                    },
                },
                "signals": {
                    "cpu_usage_ratio": signals.cpu_usage_ratio,
                    "ram_available_ratio": signals.ram_available_ratio,
                    "io_wait_ratio": signals.io_wait_ratio,
                    "vram_used_mb": signals.vram_used_mb,
                    "vram_free_mb": signals.vram_free_mb,
                    "gpu_utilization_ratio": signals.gpu_utilization_ratio,
                    "gpu_memory_utilization_ratio": signals.gpu_memory_utilization_ratio,
                    "file_vectorization_queue_depth": signals.file_vectorization_queue_depth,
                    "ready_queue_depth_current": signals.ready_queue_depth_current,
                    "ready_queue_depth_max": signals.ready_queue_depth_max,
                    "persist_queue_depth_current": signals.persist_queue_depth_current,
                    "persist_queue_depth_max": signals.persist_queue_depth_max,
                    "gpu_idle_wait_ms_total": signals.gpu_idle_wait_ms_total,
                    "prepare_queue_wait_ms_total": signals.prepare_queue_wait_ms_total,
                    "persist_queue_wait_ms_total": signals.persist_queue_wait_ms_total,
                    "latency_recent_fetch_p95_ms": signals.latency_recent_fetch_p95_ms,
                    "latency_recent_embed_p95_ms": signals.latency_recent_embed_p95_ms,
                    "latency_recent_db_write_p95_ms": signals.latency_recent_db_write_p95_ms,
                    "latency_recent_mark_done_p95_ms": signals.latency_recent_mark_done_p95_ms,
                    "mcp_latency_recent_ms": signals.mcp_latency_recent_ms,
                    "vector_workers_active_current": signals.vector_workers_active_current,
                    "vector_worker_heartbeat_at_ms": signals.vector_worker_heartbeat_at_ms,
                    "chunks_embedded_total": signals.canonical_chunk_embeddings_total,
                    "files_completed_total": signals.files_completed_total,
                },
                "gpu_memory": gpu_memory.as_ref().map(|snapshot| json!({
                    "used_mb": snapshot.used_mb,
                    "total_mb": snapshot.total_mb,
                    "free_mb": snapshot.free_mb
                })),
                "gpu_utilization": gpu_utilization.as_ref().map(|snapshot| json!({
                    "gpu_utilization_ratio": snapshot.gpu_utilization_ratio,
                    "memory_utilization_ratio": snapshot.memory_utilization_ratio
                })),
                "drain_state": current_vector_drain_state(
                    // REQ-AXO-901674 — FVQ depth stub (table dropped slice-5d).
                    0,
                    service_guard::current_pressure(),
                    telemetry.interactive_priority_active,
                    &provider.provider_requested,
                    &provider.provider_effective,
                ).as_str(),
                "vector_batch_controller": {
                    "state": controller.state.as_str(),
                    "reason": controller.reason,
                    "target_embed_batch_chunks": controller.target_embed_batch_chunks,
                    "target_files_per_cycle": controller.target_files_per_cycle,
                    "gpu_ready_low_watermark_chunks": controller.gpu_ready_low_watermark_chunks,
                    "gpu_ready_high_watermark_chunks": controller.gpu_ready_high_watermark_chunks,
                    "avg_chunks_per_embed_call": controller.avg_chunks_per_embed_call,
                    "avg_files_per_embed_call": controller.avg_files_per_embed_call,
                    "embed_ms_per_chunk": controller.embed_ms_per_chunk,
                    "window_embed_calls": controller.window_embed_calls,
                    "window_chunks": controller.window_chunks,
                    "window_files_touched": controller.window_files_touched,
                    "adjustments_total": controller.adjustments_total,
                }
            });

            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&trace_path)
            {
                Ok(mut file) => {
                    if let Err(err) = writeln!(file, "{line}") {
                        warn!(
                            "Runtime trace: failed to append to {}: {:?}",
                            trace_path.display(),
                            err
                        );
                    }
                }
                Err(err) => warn!(
                    "Runtime trace: failed to open {}: {:?}",
                    trace_path.display(),
                    err
                ),
            }

            std::thread::sleep(interval);
        }
    });
}

fn apply_live_optimizer_profile(
    profile: &axon_core::optimizer::ActionProfile,
    policy: &axon_core::optimizer::OperatorPolicySnapshot,
) {
    let allow_vector_workers = policy
        .allowed_actuators
        .iter()
        .any(|actuator| actuator == "vector_workers");
    let allow_chunk_batch_size = policy
        .allowed_actuators
        .iter()
        .any(|actuator| actuator == "chunk_batch_size");
    let allow_file_vectorization_batch_size = policy
        .allowed_actuators
        .iter()
        .any(|actuator| actuator == "file_vectorization_batch_size");
    apply_runtime_embedding_lane_adjustment(
        if allow_vector_workers {
            Some(profile.target_vector_workers)
        } else {
            None
        },
        None,
        if allow_chunk_batch_size {
            Some(profile.target_chunk_batch_size)
        } else {
            None
        },
        if allow_file_vectorization_batch_size {
            Some(profile.target_file_vectorization_batch_size)
        } else {
            None
        },
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
}

fn configured_governor_mode() -> GovernorMode {
    match std::env::var("AXON_GOVERNOR_MODE")
        .ok()
        .unwrap_or_else(|| "shadow".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "off" => GovernorMode::Off,
        "assist" => GovernorMode::Assist,
        "live" => GovernorMode::Live,
        _ => GovernorMode::Shadow,
    }
}

fn governor_state_label(state: GovernorState) -> &'static str {
    match state {
        GovernorState::Shadow => "shadow",
        GovernorState::Assist => "assist",
        GovernorState::Live => "live",
        GovernorState::Freeze => "freeze",
        GovernorState::Rollback => "rollback",
        GovernorState::Disabled => "disabled",
    }
}

fn governor_freeze_cooldown_ms() -> u64 {
    std::env::var("AXON_GOVERNOR_FREEZE_COOLDOWN_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(60_000)
}

fn current_runtime_profile_id() -> String {
    let current = axon_core::embedder::embedding_lane_config_from_env();
    format!(
        "vw{}-cb{}-fb{}",
        current.vector_workers, current.chunk_batch_size, current.file_vectorization_batch_size
    )
}

fn resolve_governor_state(
    mode: GovernorMode,
    signals: &axon_core::optimizer::RuntimeSignalsWindow,
    policy: &axon_core::optimizer::OperatorPolicySnapshot,
    _reward: Option<&axon_core::optimizer::RewardObservation>,
    freeze_until_ms: i64,
    consecutive_zero_progress_windows: u64,
    current_profile_id: &str,
    action_profiles: &[axon_core::optimizer::ActionProfile],
) -> GovernorState {
    if matches!(mode, GovernorMode::Off) {
        return GovernorState::Disabled;
    }
    if matches!(mode, GovernorMode::Shadow) {
        return GovernorState::Shadow;
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    let worker_heartbeat_stale_ms =
        env_u64("AXON_GOVERNOR_VECTOR_HEARTBEAT_STALE_MS", 30_000) as i64;
    let embed_stall_ms = env_u64("AXON_GOVERNOR_EMBED_STALL_MS", 45_000) as i64;
    let worker_heartbeat_age_ms =
        now_ms.saturating_sub(signals.vector_worker_heartbeat_at_ms as i64);
    let embed_inflight_age_ms = if signals.embed_inflight_started_at_ms > 0 {
        now_ms.saturating_sub(signals.embed_inflight_started_at_ms as i64)
    } else {
        0
    };
    if freeze_until_ms > now_ms {
        return GovernorState::Freeze;
    }
    if !action_profiles
        .iter()
        .any(|profile| profile.id == current_profile_id)
    {
        return GovernorState::Rollback;
    }
    let embed_inflight = signals.embed_inflight_started_at_ms > 0;
    let heartbeat_stalled_without_inflight =
        !embed_inflight && worker_heartbeat_age_ms > worker_heartbeat_stale_ms;
    let embed_stalled = embed_inflight && embed_inflight_age_ms > embed_stall_ms;
    let interactive_pressure_active = signals.interactive_requests_in_flight > 0
        || signals.interactive_priority != "background_normal";
    let severe_vram_pressure = signals.vram_used_mb
        > policy
            .max_vram_used_mb
            .saturating_add(policy.max_vram_used_mb / 10)
            .max(policy.max_vram_used_mb);
    let severe_cpu_pressure =
        interactive_pressure_active && signals.cpu_usage_ratio > policy.max_cpu_ratio;
    let severe_mcp_pressure =
        interactive_pressure_active && signals.mcp_latency_recent_ms > policy.max_mcp_p95_ms;
    if heartbeat_stalled_without_inflight
        || embed_stalled
        || severe_mcp_pressure
        || severe_vram_pressure
        || severe_cpu_pressure
        || signals.ram_available_ratio < policy.min_ram_available_ratio
    {
        return GovernorState::Freeze;
    }
    if consecutive_zero_progress_windows >= 4
        && signals.file_vectorization_queue_depth >= 32
        && signals.ready_queue_chunks_current == 0
        && signals.prepare_inflight_chunks_current == 0
    {
        return GovernorState::Freeze;
    }
    match mode {
        GovernorMode::Assist => GovernorState::Assist,
        GovernorMode::Live => GovernorState::Live,
        GovernorMode::Off | GovernorMode::Shadow => GovernorState::Shadow,
    }
}

fn select_governor_profile(
    state: GovernorState,
    action_profiles: &[axon_core::optimizer::ActionProfile],
    selected_profile: Option<&axon_core::optimizer::ActionProfile>,
    current_profile_id: &str,
    last_safe_profile_id: Option<&str>,
) -> Option<axon_core::optimizer::ActionProfile> {
    let current_index = action_profiles
        .iter()
        .position(|profile| profile.id == current_profile_id);
    let target_index = selected_profile.and_then(|selected| {
        action_profiles
            .iter()
            .position(|profile| profile.id == selected.id)
    });
    match state {
        GovernorState::Shadow | GovernorState::Disabled => None,
        GovernorState::Freeze | GovernorState::Rollback => last_safe_profile_id
            .and_then(|safe_id| action_profiles.iter().find(|profile| profile.id == safe_id))
            .cloned()
            .or_else(|| {
                action_profiles
                    .iter()
                    .find(|profile| profile.label == "hold")
                    .cloned()
            })
            .or_else(|| action_profiles.first().cloned()),
        GovernorState::Assist => match (current_index, target_index) {
            (Some(current), Some(target)) if target > current => {
                action_profiles.get(current + 1).cloned()
            }
            (Some(current), Some(target)) if target < current && current > 0 => {
                action_profiles.get(current - 1).cloned()
            }
            _ => selected_profile.cloned(),
        },
        GovernorState::Live => selected_profile.cloned(),
    }
}

#[cfg(test)]
mod governor_tests {
    use super::{resolve_governor_state, select_governor_profile, GovernorMode, GovernorState};
    use axon_core::optimizer::{ActionProfile, OperatorPolicySnapshot, RuntimeSignalsWindow};

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis().max(0)
    }

    fn profile(id: &str, label: &str) -> ActionProfile {
        ActionProfile {
            id: id.to_string(),
            label: label.to_string(),
            target_vector_workers: 1,
            target_chunk_batch_size: 48,
            target_file_vectorization_batch_size: 12,
            target_ready_queue_depth: 8,
            target_persist_queue_bound: 1,
            target_max_inflight_persists: 2,
            target_embed_micro_batch_max_items: 64,
            target_embed_micro_batch_max_total_tokens: 8192,
        }
    }

    #[test]
    fn freeze_prefers_hold_when_no_last_safe_profile_exists() {
        let profiles = vec![profile("hold", "hold"), profile("vw1-cb64-fb16", "step-up")];
        let selected = select_governor_profile(
            GovernorState::Freeze,
            &profiles,
            profiles.get(1),
            "vw1-cb64-fb16",
            None,
        )
        .expect("freeze profile");

        assert_eq!(selected.id, "hold");
    }

    fn signals() -> RuntimeSignalsWindow {
        let captured_at_ms = now_ms();
        RuntimeSignalsWindow {
            window_start_ms: 0,
            window_end_ms: captured_at_ms,
            captured_at_ms,
            source: "test".to_string(),
            cpu_usage_ratio: 0.1,
            ram_available_ratio: 0.8,
            io_wait_ratio: 0.0,
            process_memory: Default::default(),
            vram_used_mb: 512,
            vram_free_mb: 1_024,
            gpu_utilization_ratio: 0.5,
            gpu_memory_utilization_ratio: 0.2,
            file_vectorization_queue_depth: 32,
            graph_projection_queue_depth: 0,
            canonical_vector_backlog_depth: 32,
            ready_queue_depth_current: 1,
            ready_queue_depth_max: 1,
            ready_queue_chunks_current: 16,
            ready_queue_chunks_max: 16,
            ready_replenishment_deficit_current: 0,
            ready_replenishment_deficit_max: 0,
            active_claimed_current: 0,
            prepare_claimed_current: 0,
            ready_claimed_current: 0,
            persist_queue_depth_current: 0,
            persist_queue_depth_max: 0,
            persist_claimed_current: 0,
            prepare_inflight_current: 0,
            prepare_inflight_max: 0,
            prepare_inflight_chunks_current: 0,
            prepare_inflight_chunks_max: 0,
            gpu_idle_wait_ms_total: 0,
            prepare_queue_wait_ms_total: 0,
            prepare_reply_wait_ms_total: 0,
            persist_queue_wait_ms_total: 0,
            oldest_ready_batch_age_ms_current: 0,
            oldest_ready_batch_age_ms_max: 0,
            latency_recent_fetch_p95_ms: 0,
            latency_recent_embed_p95_ms: 0,
            latency_recent_db_write_p95_ms: 0,
            latency_recent_mark_done_p95_ms: 0,
            mcp_latency_recent_ms: 0,
            vector_workers_active_current: 1,
            vector_worker_heartbeat_at_ms: captured_at_ms as u64,
            embed_inflight_started_at_ms: 0,
            interactive_requests_in_flight: 0,
            interactive_priority: "background_normal".to_string(),
            canonical_chunk_embeddings_total: 10,
            canonical_chunks_embedded_last_minute: 0,
            canonical_files_embedded_last_minute: 0,
            canonical_files_embedded_total: 0,
            chunk_embedding_writes_total: 0,
            files_completed_total: 1,
            target_ready_chunks_current: 96,
            gpu_ready_low_watermark_chunks: 32,
            gpu_ready_high_watermark_chunks: 64,
        }
    }

    fn policy() -> OperatorPolicySnapshot {
        let captured_at_ms = now_ms();
        OperatorPolicySnapshot {
            captured_at_ms,
            max_cpu_ratio: 0.8,
            min_ram_available_ratio: 0.2,
            max_mcp_p95_ms: 300,
            max_vram_used_ratio: 0.75,
            max_vram_used_mb: 6_144,
            max_io_wait_ratio: 0.2,
            backlog_priority_weight: 1.0,
            interactive_priority_weight: 1.0,
            shadow_mode_enabled: false,
            allowed_actuators: vec![],
            evaluation_window_ms: 60_000,
        }
    }

    #[test]
    fn freeze_triggers_when_vector_worker_heartbeat_is_stale() {
        std::env::set_var("AXON_GOVERNOR_VECTOR_HEARTBEAT_STALE_MS", "1");
        let mut stalled = signals();
        stalled.vector_worker_heartbeat_at_ms = 0;
        let state = resolve_governor_state(
            GovernorMode::Assist,
            &stalled,
            &policy(),
            None,
            0,
            0,
            "hold",
            &[profile("hold", "hold")],
        );
        assert_eq!(state, GovernorState::Freeze);
        std::env::remove_var("AXON_GOVERNOR_VECTOR_HEARTBEAT_STALE_MS");
    }

    #[test]
    fn worker_down_alone_does_not_trigger_freeze() {
        let mut stalled = signals();
        stalled.vector_workers_active_current = 0;
        let state = resolve_governor_state(
            GovernorMode::Live,
            &stalled,
            &policy(),
            None,
            0,
            0,
            "hold",
            &[profile("hold", "hold")],
        );
        assert_eq!(state, GovernorState::Live);
    }

    #[test]
    fn liveness_penalty_alone_does_not_trigger_freeze() {
        let reward = axon_core::optimizer::RewardObservation {
            decision_id: "d1".to_string(),
            observed_at_ms: 1_000,
            window_start_ms: 0,
            window_end_ms: 1_000,
            throughput_chunks_per_hour: 0.0,
            throughput_files_per_hour: 0.0,
            reward: -1.0,
            penalty_cpu: 0.0,
            penalty_ram: 0.0,
            penalty_vram: 0.0,
            penalty_mcp: 0.0,
            penalty_io: 0.0,
            penalty_liveness: 10.0,
            penalty_stability: 0.0,
            penalty_churn: 0.0,
        };
        let state = resolve_governor_state(
            GovernorMode::Live,
            &signals(),
            &policy(),
            Some(&reward),
            0,
            0,
            "hold",
            &[profile("hold", "hold")],
        );
        assert_eq!(state, GovernorState::Live);
    }

    #[test]
    fn freeze_triggers_when_embed_inflight_stalls() {
        std::env::set_var("AXON_GOVERNOR_EMBED_STALL_MS", "1");
        let mut stalled = signals();
        stalled.embed_inflight_started_at_ms = 1;
        let state = resolve_governor_state(
            GovernorMode::Live,
            &stalled,
            &policy(),
            None,
            0,
            0,
            "hold",
            &[profile("hold", "hold")],
        );
        assert_eq!(state, GovernorState::Freeze);
        std::env::remove_var("AXON_GOVERNOR_EMBED_STALL_MS");
    }
}

fn optimizer_loop_interval_ms() -> u64 {
    let base_ms = std::env::var("AXON_OPT_LOOP_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(15_000);
    quiescent_scaled_interval_ms(base_ms, 1_000, 120_000)
}

fn memory_reclaimer_poll_interval_ms() -> u64 {
    quiescent_scaled_interval_ms(
        MEMORY_RECLAIMER_POLL_INTERVAL_SECS.saturating_mul(1_000),
        5_000,
        120_000,
    )
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn runtime_trace_enabled() -> bool {
    matches!(
        std::env::var("AXON_RUNTIME_TRACE_ENABLED")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn runtime_trace_interval_ms() -> u64 {
    let base_ms = std::env::var("AXON_RUNTIME_TRACE_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(5_000);
    quiescent_scaled_interval_ms(base_ms, 1_000, 120_000)
}

fn quiescent_interval_scale_pct() -> usize {
    std::env::var("AXON_QUIESCENT_INTERVAL_SCALE_PCT")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(QUIESCENT_INTERVAL_SCALE_PCT_DEFAULT)
        .clamp(100, 2000)
}

fn current_quiescent_state_without_backlog_visibility() -> RuntimeQuiescentState {
    service_guard::current_runtime_quiescent_state(0, 0)
}

fn quiescent_scaled_interval_ms(base_ms: u64, min_ms: u64, max_ms: u64) -> u64 {
    service_guard::scale_interval_for_quiescent(
        base_ms,
        current_quiescent_state_without_backlog_visibility(),
        quiescent_interval_scale_pct(),
        min_ms,
        max_ms,
    )
}

fn runtime_trace_path() -> PathBuf {
    std::env::var("AXON_RUNTIME_TRACE_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
            PathBuf::from(format!(".axon/runtime-trace-{ts}.jsonl"))
        })
}

fn optimizer_constraint_flags(
    signals: &axon_core::optimizer::RuntimeSignalsWindow,
    policy: &axon_core::optimizer::OperatorPolicySnapshot,
) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if signals.cpu_usage_ratio > policy.max_cpu_ratio {
        flags.push("cpu");
    }
    if signals.ram_available_ratio < policy.min_ram_available_ratio {
        flags.push("ram");
    }
    if signals.vram_used_mb > policy.max_vram_used_mb {
        flags.push("vram");
    }
    if signals.io_wait_ratio > policy.max_io_wait_ratio {
        flags.push("io");
    }
    if signals.mcp_latency_recent_ms > policy.max_mcp_p95_ms {
        flags.push("mcp");
    }
    flags
}

fn should_attempt_memory_reclaim(
    queue_len: usize,
    process_memory: axon_core::runtime_observability::ProcessMemorySnapshot,
    min_anon_bytes: u64,
) -> bool {
    // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress_buffer backlog gate was
    // ripped with the buffer. Reclaim only when the work queue is idle and the
    // anonymous RSS is above the trim floor.
    if queue_len > 0 {
        return false;
    }

    process_memory.rss_anon_bytes >= min_anon_bytes
}



pub(crate) fn runtime_telemetry_snapshot(
    store: &GraphStore,
    queue: &QueueStore,
) -> RuntimeTelemetrySnapshot {
    let runtime_mode = axon_core::runtime_mode::AxonRuntimeMode::from_env();
    let vector_runtime_enabled = runtime_mode.semantic_workers_enabled();
    let graph_runtime_enabled = runtime_mode.ingestion_enabled();
    let budget = queue.memory_budget_snapshot();
    let queue_depth = queue.common_len();
    let service_pressure = service_guard::current_pressure();
    let policy = claim_policy(
        queue_depth,
        budget.exhaustion_ratio,
        current_rss_bytes(),
        memory_limit_bytes(),
        service_pressure,
    );
    let host_pressure = sample_host_pressure();
    let process_memory = process_memory_snapshot();
    // REQ-AXO-901674 — FVQ/GPQ queue tables dropped post MIL-AXO-017 / REQ-AXO-289 /
    // slice-5d. Canonical pipeline_v2 path writes Chunk + ChunkEmbedding directly.
    let persisted_file_pending_depth = if graph_runtime_enabled {
        store.count_persisted_file_pending().unwrap_or(0)
    } else {
        0
    };
    let runtime_truth_feed = service_guard::current_runtime_truth_feed();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let stale_threshold_ms = std::env::var("AXON_VECTOR_LEASE_STALE_MS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(120_000);
    let orphan_vectorization_files = if vector_runtime_enabled {
        store.count_orphaned_file_vectorization_files().unwrap_or(0)
    } else {
        0
    };
    let stale_vector_inflight_files = if vector_runtime_enabled {
        store
            .count_stale_inflight_file_vectorization_files(now_ms, stale_threshold_ms)
            .unwrap_or(0)
    } else {
        0
    };
    let oldest_graph_pending_age_ms = if graph_runtime_enabled {
        store.oldest_graph_pending_age_ms(now_ms).unwrap_or(0)
    } else {
        0
    };
    let oldest_semantic_pending_age_ms = if vector_runtime_enabled {
        store.oldest_semantic_pending_age_ms(now_ms).unwrap_or(0)
    } else {
        0
    };
    let effective_graph_scheduler_depth = if service_guard::graph_workers_active_current() > 0 {
        persisted_file_pending_depth
    } else {
        0
    };
    // REQ-AXO-901674 — FVQ depth stub (table dropped slice-5d) ; scheduler
    // diagnostics consume 0 until rewired against pipeline_v2 ready-queue
    // counters (separate REQ).
    let utility_scheduler = current_utility_first_scheduler_diagnostics(
        effective_graph_scheduler_depth,
        0,
        service_pressure,
    );

    let interactive_priority = service_guard::current_interactive_priority();
    let vector_runtime = service_guard::vector_runtime_metrics();

    RuntimeTelemetrySnapshot {
        budget_bytes: budget.budget_bytes,
        reserved_bytes: budget.reserved_bytes,
        exhaustion_ratio: budget.exhaustion_ratio,
        reserved_task_count: budget.reserved_task_count,
        anonymous_trace_reserved_tasks: budget.anonymous_trace_reserved_tasks,
        anonymous_trace_admissions_total: budget.anonymous_trace_admissions_total,
        reservation_release_misses_total: budget.reservation_release_misses_total,
        queue_depth,
        claim_mode: policy.mode.label().to_string(),
        service_pressure: service_pressure_label(service_pressure).to_string(),
        interactive_priority_active: interactive_priority != InteractivePriority::BackgroundNormal,
        interactive_priority_level: interactive_priority.as_str().to_string(),
        interactive_requests_in_flight: service_guard::interactive_requests_in_flight(),
        oversized_refusals_total: OVERSIZED_REFUSALS_TOTAL.load(Ordering::Relaxed),
        degraded_mode_entries_total: DEGRADED_MODE_ENTRIES_TOTAL.load(Ordering::Relaxed),
        background_launches_suppressed_total: service_guard::background_launches_suppressed_total(),
        vectorization_suppressed_due_to_interactive: service_guard::vectorization_suppressed_total(
        ),
        vectorization_interrupted_due_to_interactive:
            service_guard::vectorization_interrupted_total(),
        vectorization_requeued_for_interactive:
            service_guard::vectorization_requeued_for_interactive_total(),
        vectorization_resumed_after_interactive:
            service_guard::vectorization_resumed_after_interactive_total(),
        projection_suppressed_due_to_interactive: service_guard::projection_suppressed_total(),
        memory_trim_attempts_total: MEMORY_TRIM_ATTEMPTS_TOTAL.load(Ordering::Relaxed),
        memory_trim_successes_total: MEMORY_TRIM_SUCCESSES_TOTAL.load(Ordering::Relaxed),
        cpu_load: host_pressure.cpu_load,
        ram_load: host_pressure.ram_load,
        io_wait: host_pressure.io_wait,
        host_state: host_state_label(policy.mode, budget.exhaustion_ratio, service_pressure)
            .to_string(),
        host_guidance_slots: policy.claim_count,
        rss_bytes: process_memory.rss_bytes,
        rss_anon_bytes: process_memory.rss_anon_bytes,
        rss_file_bytes: process_memory.rss_file_bytes,
        rss_shmem_bytes: process_memory.rss_shmem_bytes,
        // REQ-AXO-284 Slice 2 — PG health probe. Cheap catalog reads, run
        // once per heartbeat tick ; absorbed errors return None so a
        // catalog hiccup never breaks the telemetry pipeline.
        pg_database_bytes: store.pg_database_size_bytes(),
        pg_chunkembedding_total_bytes: store.pg_chunkembedding_total_bytes(),
        pg_wal_bytes: store.pg_wal_bytes(),
        pg_buffer_hit_ratio: store.pg_buffer_hit_ratio(),
        vector_chunks_embedded_cumulative: service_guard::vector_chunks_embedded_cumulative(),
        chunk_embeddings_per_second: service_guard::vector_chunk_embeddings_per_second(),
        chunk_embeddings_rate_window_ms: service_guard::vector_chunk_embeddings_rate_window_ms(),
        prepare_inflight_chunks_current: vector_runtime.prepare_inflight_chunks_current,
        ready_queue_chunks_current: vector_runtime.ready_queue_chunks_current,
        ready_queue_chunks_small: vector_runtime.ready_queue_chunks_small,
        ready_queue_chunks_medium: vector_runtime.ready_queue_chunks_medium,
        ready_queue_chunks_large: vector_runtime.ready_queue_chunks_large,
        ready_batches_small: vector_runtime.ready_batches_small,
        ready_batches_medium: vector_runtime.ready_batches_medium,
        ready_batches_large: vector_runtime.ready_batches_large,
        mixed_fallback_batches_total: vector_runtime.mixed_fallback_batches_total,
        homogeneous_batches_total: vector_runtime.homogeneous_batches_total,
        last_consumed_batch_lane: vector_runtime.last_consumed_batch_lane.as_str().to_string(),
        active_small_max_tokens: vector_runtime.active_small_max_tokens,
        active_medium_max_tokens: vector_runtime.active_medium_max_tokens,
        ready_replenishment_deficit_current: vector_runtime.ready_replenishment_deficit_current,
        oldest_ready_batch_age_ms_current: vector_runtime.oldest_ready_batch_age_ms_current,
        last_embed_attempt_wall_ms: vector_runtime.last_embed_attempt_wall_ms,
        avg_embed_attempt_wall_ms: vector_runtime.avg_embed_attempt_wall_ms,
        max_embed_attempt_wall_ms: vector_runtime.max_embed_attempt_wall_ms,
        last_embed_gap_ms: vector_runtime.last_embed_gap_ms,
        avg_embed_gap_ms: vector_runtime.avg_embed_gap_ms,
        max_embed_gap_ms: vector_runtime.max_embed_gap_ms,
        graph_workers_started_total: service_guard::graph_workers_started_total(),
        graph_workers_active_current: service_guard::graph_workers_active_current(),
        graph_worker_heartbeat_at_ms: service_guard::graph_worker_heartbeat_at_ms(),
        runtime_truth_last_heartbeat_at_ms: runtime_truth_feed.last_heartbeat_at_ms.unwrap_or(0),
        runtime_truth_last_good_payload_at_ms: runtime_truth_feed
            .last_good_payload_at_ms
            .unwrap_or(0),
        runtime_truth_stale_after_ms: runtime_truth_feed.stale_after_ms,
        runtime_truth_degraded_reason: runtime_truth_feed.degraded_reason,
        orphan_vectorization_files,
        stale_vector_inflight_files,
        oldest_graph_pending_age_ms,
        oldest_semantic_pending_age_ms,
        utility_first_scheduler_state: utility_scheduler.state.as_str().to_string(),
        utility_first_scheduler_reason: utility_scheduler.reason.to_string(),
        semantic_underfeed: utility_scheduler.semantic_underfeed,
        semantic_ready_reserve_target: utility_scheduler.ready_reserve_target,
        utility_first_scheduler_hold_window_ms: utility_scheduler.hold_window_ms,
    }
}

#[derive(Debug, Clone, Copy)]
struct ClaimPolicy {
    mode: ClaimMode,
    claim_count: usize,
    #[cfg(test)]
    sleep: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimMode {
    Fast,
    Slow,
    Guarded,
    Paused,
}

impl ClaimMode {
    fn label(self) -> &'static str {
        match self {
            ClaimMode::Fast => "fast",
            ClaimMode::Slow => "slow",
            ClaimMode::Guarded => "guarded",
            ClaimMode::Paused => "paused",
        }
    }
}

fn claim_policy(
    queue_len: usize,
    budget_exhaustion_ratio: f64,
    rss_bytes: Option<u64>,
    memory_limit: u64,
    service_pressure: ServicePressure,
) -> ClaimPolicy {
    match service_guard::current_interactive_priority() {
        InteractivePriority::InteractiveCritical => {
            service_guard::record_background_launch_suppressed();
            return ClaimPolicy {
                mode: ClaimMode::Paused,
                claim_count: 0,
                #[cfg(test)]
                sleep: quiescent_scaled_claim_sleep(1_000, queue_len),
            };
        }
        InteractivePriority::InteractivePriority => {
            service_guard::record_background_launch_suppressed();
            return ClaimPolicy {
                mode: ClaimMode::Guarded,
                claim_count: 50,
                #[cfg(test)]
                sleep: quiescent_scaled_claim_sleep(750, queue_len),
            };
        }
        InteractivePriority::BackgroundNormal => {}
    }

    let rss_ratio = rss_bytes
        .map(|rss| rss as f64 / memory_limit.max(1) as f64)
        .unwrap_or(0.0);
    let queue_pressure = (queue_len as f64 / 6_000.0).clamp(0.0, 1.0);
    let service_pressure_score = match service_pressure {
        ServicePressure::Healthy => 0.0,
        ServicePressure::Recovering => 0.35,
        ServicePressure::Degraded => 0.70,
        ServicePressure::Critical => 1.0,
    };
    let dynamic_pressure = ((queue_pressure * 0.35)
        + (budget_exhaustion_ratio.clamp(0.0, 1.0) * 0.25)
        + (rss_ratio.clamp(0.0, 1.0) * 0.30)
        + (service_pressure_score * 0.40))
        .clamp(0.0, 1.0);

    if service_pressure == ServicePressure::Critical
        || rss_ratio >= 0.92
        || budget_exhaustion_ratio >= 0.98
    {
        return ClaimPolicy {
            mode: ClaimMode::Paused,
            claim_count: 0,
            #[cfg(test)]
            sleep: quiescent_scaled_claim_sleep(1_000, queue_len),
        };
    }

    if service_pressure == ServicePressure::Degraded
        || rss_ratio >= 0.82
        || budget_exhaustion_ratio >= 0.88
    {
        return ClaimPolicy {
            mode: ClaimMode::Guarded,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Guarded),
            #[cfg(test)]
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Guarded, queue_len),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            #[cfg(test)]
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow, queue_len),
        };
    }

    if budget_exhaustion_ratio >= 0.72 {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            #[cfg(test)]
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow, queue_len),
        };
    }

    ClaimPolicy {
        mode: ClaimMode::Fast,
        claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Fast),
        #[cfg(test)]
        sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Fast, queue_len),
    }
}

fn dynamic_claim_count(pressure: f64, mode: ClaimMode) -> usize {
    let base =
        ((2_000.0 * (1.0 - pressure.clamp(0.0, 1.0)).powi(2)).round() as usize).clamp(25, 2_000);

    match mode {
        ClaimMode::Fast => base,
        ClaimMode::Slow => ((base as f64) * 0.60).round() as usize,
        ClaimMode::Guarded => ((base as f64) * 0.20).round() as usize,
        ClaimMode::Paused => 0,
    }
    .clamp(25, 2_000)
}

fn service_pressure_label(service_pressure: ServicePressure) -> &'static str {
    match service_pressure {
        ServicePressure::Healthy => "healthy",
        ServicePressure::Recovering => "recovering",
        ServicePressure::Degraded => "degraded",
        ServicePressure::Critical => "critical",
    }
}

fn host_state_label(
    mode: ClaimMode,
    exhaustion_ratio: f64,
    service_pressure: ServicePressure,
) -> &'static str {
    if matches!(mode, ClaimMode::Paused)
        || matches!(service_pressure, ServicePressure::Critical)
        || exhaustion_ratio >= 1.0
    {
        "constrained"
    } else if matches!(mode, ClaimMode::Slow | ClaimMode::Guarded)
        || matches!(
            service_pressure,
            ServicePressure::Degraded | ServicePressure::Recovering
        )
        || exhaustion_ratio >= 0.75
    {
        "watch"
    } else {
        "healthy"
    }
}


#[cfg(test)]
fn dynamic_claim_sleep(
    pressure: f64,
    mode: ClaimMode,
    graph_backlog_depth: usize,
) -> std::time::Duration {
    let pressure = pressure.clamp(0.0, 1.0);
    let base_sleep_ms = match mode {
        ClaimMode::Fast => 100 + (pressure * 200.0).round() as u64,
        ClaimMode::Slow => 250 + (pressure * 300.0).round() as u64,
        ClaimMode::Guarded => 500 + (pressure * 400.0).round() as u64,
        ClaimMode::Paused => 1_000,
    };
    quiescent_scaled_claim_sleep(base_sleep_ms, graph_backlog_depth)
}

#[cfg(test)]
fn quiescent_scaled_claim_sleep(
    base_sleep_ms: u64,
    graph_backlog_depth: usize,
) -> std::time::Duration {
    std::time::Duration::from_millis(service_guard::scale_interval_for_quiescent(
        base_sleep_ms,
        service_guard::current_runtime_quiescent_state(graph_backlog_depth as u64, 0),
        quiescent_interval_scale_pct(),
        50,
        4_000,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        memory_limit_bytes, memory_reclaimer_enabled, memory_reclaimer_min_anon_bytes,
        optimizer_loop_interval_ms, should_attempt_memory_reclaim,
    };
    use std::sync::Mutex;

    static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn test_memory_limit_uses_default_when_env_missing() {
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
        assert_eq!(memory_limit_bytes(), 14 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_memory_limit_uses_env_when_valid() {
        unsafe {
            std::env::set_var("AXON_MEMORY_LIMIT_GB", "10");
        }
        assert_eq!(memory_limit_bytes(), 10 * 1024 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
    }

    #[test]
    fn test_optimizer_loop_interval_defaults_to_15_seconds() {
        let _guard = ENV_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::remove_var("AXON_OPT_LOOP_INTERVAL_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
        assert_eq!(optimizer_loop_interval_ms(), 60_000);
    }

    #[test]
    fn test_optimizer_loop_interval_respects_env_override() {
        let _guard = ENV_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("AXON_OPT_LOOP_INTERVAL_MS", "30000");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
        assert_eq!(optimizer_loop_interval_ms(), 120_000);
        unsafe {
            std::env::remove_var("AXON_OPT_LOOP_INTERVAL_MS");
        }
    }

    #[test]
    fn test_shadow_optimizer_disabled_by_default() {
        unsafe {
            std::env::remove_var("AXON_ENABLE_SHADOW_OPTIMIZER");
        }
        assert!(!super::shadow_optimizer_enabled());
    }

    #[test]
    fn test_shadow_optimizer_enabled_via_env() {
        unsafe {
            std::env::set_var("AXON_ENABLE_SHADOW_OPTIMIZER", "true");
        }
        assert!(super::shadow_optimizer_enabled());
        unsafe {
            std::env::remove_var("AXON_ENABLE_SHADOW_OPTIMIZER");
        }
    }

    #[test]
    fn test_memory_reclaimer_can_be_disabled_with_env() {
        unsafe {
            std::env::set_var("AXON_ENABLE_MEMORY_RECLAIMER", "false");
        }
        assert!(!memory_reclaimer_enabled());
        unsafe {
            std::env::remove_var("AXON_ENABLE_MEMORY_RECLAIMER");
        }
    }

    #[test]
    fn test_memory_reclaimer_min_anon_bytes_uses_env_override() {
        unsafe {
            std::env::set_var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB", "2048");
        }
        assert_eq!(memory_reclaimer_min_anon_bytes(), 2_048 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB");
        }
    }

    /// REQ-AXO-901893 (LEGACY FEED PURGE) — the memory reclaimer now gates on
    /// queue idle + anon RSS floor only (the ingress_buffer backlog gate was
    /// ripped with the buffer). Reclaim fires when queue is empty AND anon RSS
    /// is at/above the trim floor; it is suppressed while the queue is non-empty.
    #[test]
    fn test_memory_reclaim_gates_on_queue_idle_and_anon_floor() {
        let mem_over = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_bytes: 24 * 1024 * 1024 * 1024,
            rss_anon_bytes: 23 * 1024 * 1024 * 1024,
            rss_file_bytes: 64 * 1024 * 1024,
            rss_shmem_bytes: 0,
        };
        let floor = 4 * 1024 * 1024 * 1024;
        // Idle queue + anon above floor → reclaim.
        assert!(should_attempt_memory_reclaim(0, mem_over, floor));
        // Non-empty queue → never reclaim (work in flight).
        assert!(!should_attempt_memory_reclaim(7, mem_over, floor));
        // Idle queue but anon below floor → no reclaim (nothing to gain).
        let mem_under = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_anon_bytes: 1024 * 1024,
            ..mem_over
        };
        assert!(!should_attempt_memory_reclaim(0, mem_under, floor));
    }
}
