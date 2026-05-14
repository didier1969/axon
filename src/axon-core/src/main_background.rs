// Copyright (c) Didier Stadelmann. All rights reserved.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axon_core::embedder::{
    apply_runtime_embedding_lane_adjustment, current_embedding_provider_diagnostics,
    current_gpu_memory_snapshot, current_gpu_utilization_snapshot, embedding_lane_config_from_env,
};
use axon_core::file_ingress_guard::{
    guard_metrics_snapshot, FileIngressRow, SharedFileIngressGuard,
};
use axon_core::fs_watcher::{self, HOT_PRIORITY};
use axon_core::graph::GraphStore;
use axon_core::graph::PendingFile;
use axon_core::ingress_buffer::{
    record_ingress_flush, wait_for_ingress_activity_or_timeout, IngressMetricsSnapshot,
    IngressSubtreeHint, SharedIngressBuffer,
};
use axon_core::optimizer::{
    build_admissible_action_profiles, collect_host_snapshot, collect_operator_policy_snapshot,
    collect_recent_analytics_window, collect_runtime_signals_window, observe_reward,
    HeuristicPolicyEngine, PolicyEngine,
};
use axon_core::queue::{ProcessingMode, QueueStore};
use axon_core::runtime_observability::{
    duckdb_memory_snapshot, duckdb_storage_snapshot, process_memory_snapshot,
};
use axon_core::runtime_profile::{
    current_admission_controller_state, recommend_admission_controller_profile, RuntimeProfile,
};
use axon_core::scanner::Scanner;
use axon_core::service_guard;
use axon_core::service_guard::{InteractivePriority, RuntimeQuiescentState, ServicePressure};
use axon_core::vector_control::{
    current_utility_first_scheduler_diagnostics, current_vector_batch_controller_diagnostics,
    current_vector_drain_state,
};
use axon_core::watcher_probe;
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use serde_json::json;
use tracing::{debug, error, info, warn};

#[path = "main_background/host_pressure.rs"]
mod host_pressure;
#[path = "main_background/memory_config.rs"]
mod memory_config;

use host_pressure::sample_host_pressure;
use memory_config::{
    current_rss_bytes, federation_orchestrator_enabled, memory_limit_bytes,
    memory_reclaimer_enabled, memory_reclaimer_min_anon_bytes, parse_rss_from_statm,
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

#[derive(Debug, Clone)]
struct WatchTarget {
    path: PathBuf,
    recursive: bool,
}

#[derive(Debug, Clone, Default)]
struct AdmissionPlan {
    selected: Vec<AdmissionSelection>,
    deferred: Vec<PendingFile>,
    oversized: Vec<PendingFile>,
    degraded: Vec<String>,
}

#[derive(Debug, Clone)]
struct AdmissionSelection {
    file: PendingFile,
    mode: ProcessingMode,
}

const CLAIM_MODE_SENTINEL: u8 = u8::MAX;
const FAIRNESS_PROMOTION_DEFER_THRESHOLD: u32 = 3;
const OVERSIZED_PROBATION_DEFER_THRESHOLD: u32 = 3;
const AUTONOMOUS_INGESTOR_IDLE_WAIT_MS: u64 = 2_000;
const INGRESS_PROMOTER_POLL_INTERVAL_MS: u64 = 50;
const INGRESS_PROMOTER_IDLE_WAIT_MS: u64 = 2_000;
const INGRESS_HOT_FLUSH_WINDOW_MS: u64 = 100;
const INGRESS_BULK_FLUSH_WINDOW_MS: u64 = 75;
const INGRESS_HINT_FLUSH_WINDOW_MS: u64 = 150;
const INGRESS_MAX_BATCH_SIZE: usize = 2_048;
const INGRESS_FORCE_BATCH_SIZE: usize = 4_096;
const INGRESS_HOT_PRIORITY_BATCH_CAP: usize = 256;
const INGRESS_MIXED_BULK_BATCH_SIZE: usize = 1_024;
const MEMORY_RECLAIMER_POLL_INTERVAL_SECS: u64 = 15;
const QUIESCENT_INTERVAL_SCALE_PCT_DEFAULT: usize = 400;

static OVERSIZED_REFUSALS_TOTAL: AtomicU64 = AtomicU64::new(0);
static DEGRADED_MODE_ENTRIES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MEMORY_TRIM_SUCCESSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static LAST_REPORTED_CLAIM_MODE: AtomicU8 = AtomicU8::new(CLAIM_MODE_SENTINEL);

#[derive(Debug, Clone, Copy)]
struct AdmissionControllerDecision {
    target_band: usize,
    reorder_point: usize,
    max_wip: usize,
    hold_window_ms: u64,
    forced_bulk_fill_threshold: usize,
    blocking_authority: &'static str,
    bulk_fill_preferred: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct IngressFlushOutcome {
    admitted_count: usize,
    subtree_discovered_count: usize,
}

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
    pub guard_hits: u64,
    pub guard_misses: u64,
    pub guard_bypassed_total: u64,
    pub guard_hydrated_entries: u64,
    pub guard_hydration_duration_ms: u64,
    pub ingress_enabled: bool,
    pub ingress_buffered_entries: usize,
    pub ingress_hot_entries: usize,
    pub ingress_scan_entries: usize,
    pub ingress_subtree_hints: usize,
    pub ingress_subtree_hint_in_flight: usize,
    pub ingress_subtree_hint_accepted_total: u64,
    pub ingress_subtree_hint_blocked_total: u64,
    pub ingress_subtree_hint_suppressed_total: u64,
    pub ingress_subtree_hint_productive_total: u64,
    pub ingress_subtree_hint_unproductive_total: u64,
    pub ingress_subtree_hint_dropped_total: u64,
    pub ingress_collapsed_total: u64,
    pub ingress_flush_count: u64,
    pub ingress_last_flush_duration_ms: u64,
    pub ingress_last_promoted_count: u64,
    pub ingress_promoted_total: u64,
    pub ingress_last_durably_persisted_count: u64,
    pub ingress_durably_persisted_total: u64,
    pub ingress_last_excluded_from_pending_count: u64,
    pub ingress_excluded_from_pending_total: u64,
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
    pub db_file_bytes: u64,
    pub db_wal_bytes: u64,
    pub db_total_bytes: u64,
    pub duckdb_memory_bytes: u64,
    pub duckdb_temporary_bytes: u64,
    pub graph_projection_queue_queued: usize,
    pub graph_projection_queue_inflight: usize,
    pub graph_projection_queue_depth: usize,
    pub file_vectorization_queue_queued: usize,
    pub file_vectorization_queue_inflight: usize,
    pub file_vectorization_queue_depth: usize,
    pub vector_chunks_embedded_total: u64,
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

pub(crate) fn spawn_memory_reclaimer(queue: Arc<QueueStore>, ingress_buffer: SharedIngressBuffer) {
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

        let ingress_metrics = ingress_buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .metrics_snapshot();
        let process_memory = process_memory_snapshot();
        let min_anon_bytes = memory_reclaimer_min_anon_bytes();

        if !should_attempt_memory_reclaim(
            queue.common_len(),
            &ingress_metrics,
            process_memory,
            min_anon_bytes,
        ) {
            continue;
        }

        if ingress_metrics.subtree_hints > 0 {
            let dropped = ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .shed_subtree_hints_for_memory_pressure();
            if dropped > 0 {
                warn!(
                    "Memory reclaimer shed {} subtree hint(s) under memory pressure before trim.",
                    dropped
                );
            }
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
    ingress_buffer: SharedIngressBuffer,
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
            let telemetry = runtime_telemetry_snapshot(&store, &queue, &ingress_buffer);
            service_guard::record_background_runtime_wakeup(
                service_guard::BackgroundWakeDetail::RuntimeTrace,
                telemetry.graph_projection_queue_depth as u64,
                telemetry.file_vectorization_queue_depth as u64,
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
                    "file_vectorization_queue_depth": telemetry.file_vectorization_queue_depth,
                    "graph_projection_queue_depth": telemetry.graph_projection_queue_depth,
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
                    telemetry.file_vectorization_queue_depth,
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
            duckdb_memory: Default::default(),
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

fn reader_refresh_interval_ms() -> u64 {
    let base_ms = std::env::var("AXON_READER_REFRESH_INTERVAL_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|v| *v >= 250)
        .unwrap_or(5_000);
    quiescent_scaled_interval_ms(base_ms, 250, 60_000)
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

fn ingress_promoter_poll_interval_ms() -> u64 {
    quiescent_scaled_interval_ms(INGRESS_PROMOTER_POLL_INTERVAL_MS, 50, 2_000)
}

fn autonomous_ingestor_idle_wait(
    timeout: std::time::Duration,
    queue_len: usize,
) -> std::time::Duration {
    std::time::Duration::from_millis(
        quiescent_scaled_interval_ms(AUTONOMOUS_INGESTOR_IDLE_WAIT_MS, 250, 30_000)
            .max(timeout.as_millis().min(u128::from(u64::MAX)) as u64)
            .max(
                quiescent_scaled_claim_sleep(1_000, queue_len)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            ),
    )
}

fn ingress_promoter_idle_wait_ms() -> u64 {
    quiescent_scaled_interval_ms(INGRESS_PROMOTER_IDLE_WAIT_MS, 250, 30_000)
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
    ingress_metrics: &IngressMetricsSnapshot,
    process_memory: axon_core::runtime_observability::ProcessMemorySnapshot,
    min_anon_bytes: u64,
) -> bool {
    if queue_len > 0 || ingress_metrics.buffered_entries > 0 {
        return false;
    }

    process_memory.rss_anon_bytes >= min_anon_bytes
}

pub(crate) fn spawn_reader_snapshot_refresher(store: Arc<GraphStore>) {
    std::thread::spawn(move || {
        let sleep_ms = reader_refresh_interval_ms();
        info!(
            "Reader snapshot refresher enabled (interval={}ms).",
            sleep_ms
        );
        loop {
            let signaled = store.wait_for_reader_refresh_signal(Duration::from_millis(sleep_ms));
            match store.refresh_reader_snapshot_if_needed() {
                Ok(refreshed) => {
                    if signaled || refreshed {
                        service_guard::record_background_runtime_wakeup(
                            service_guard::BackgroundWakeDetail::ReaderRefresh,
                            0,
                            0,
                        );
                    }
                }
                Err(err) => {
                    warn!("Reader snapshot refresh failed: {}", err);
                }
            }
        }
    });
}

pub(crate) fn spawn_autonomous_ingestor(store: Arc<GraphStore>, queue: Arc<QueueStore>) {
    tokio::spawn(async move {
        info!("Autonomous Ingestor: Ignition. Monitoring DuckDB for work...");
        let memory_limit = memory_limit_bytes();
        let mut last_mode: Option<ClaimMode> = None;
        let mut idle = true;
        loop {
            let policy = claim_policy(
                queue.common_len(),
                queue.memory_budget_snapshot().exhaustion_ratio,
                current_rss_bytes(),
                memory_limit,
                service_guard::current_pressure(),
            );
            if last_mode != Some(policy.mode) {
                record_claim_mode_transition(policy.mode);
                info!(
                    "Autonomous Ingestor claim mode={} claim_count={} sleep_ms={} queue_len={} service_pressure={:?}",
                    policy.mode.label(),
                    policy.claim_count,
                    policy.sleep.as_millis(),
                    queue.common_len(),
                    service_guard::current_pressure(),
                );
                last_mode = Some(policy.mode);
            }
            if policy.claim_count > 0 {
                if let Ok(candidates) = store.fetch_pending_candidates(
                    policy.claim_count.saturating_mul(4).max(policy.claim_count),
                ) {
                    let plan = plan_admissions(&queue, candidates, policy.claim_count);
                    let mut made_progress = false;

                    if !plan.deferred.is_empty() {
                        let deferred_paths = plan
                            .deferred
                            .iter()
                            .map(|file| file.path.clone())
                            .collect::<Vec<_>>();
                        if let Err(err) = store.mark_pending_files_deferred(&deferred_paths) {
                            warn!(
                                "Autonomous Ingestor failed to record deferred fairness debt: {}",
                                err
                            );
                        } else {
                            made_progress = true;
                        }
                    }

                    for oversized in &plan.oversized {
                        record_oversized_refusal();
                        if let Err(err) =
                            store.mark_file_oversized_for_current_budget(&oversized.path)
                        {
                            warn!(
                                "Autonomous Ingestor failed to mark {} as oversized: {}",
                                oversized.path, err
                            );
                        } else {
                            made_progress = true;
                        }
                    }

                    let selected_modes = plan
                        .selected
                        .iter()
                        .map(|selection| (selection.file.path.clone(), selection.mode))
                        .collect::<std::collections::HashMap<_, _>>();

                    if let Ok(files) = store.claim_pending_paths(
                        &plan
                            .selected
                            .iter()
                            .map(|selection| selection.file.path.clone())
                            .collect::<Vec<_>>(),
                    ) {
                        if !files.is_empty() {
                            debug!(
                                "Autonomous Ingestor: Feeding {} tasks to workers.",
                                files.len()
                            );
                            enqueue_claimed_files(&store, &queue, files, &selected_modes);
                            made_progress = true;
                        }
                    } else if !plan.selected.is_empty() {
                        warn!("Autonomous Ingestor failed to claim selected pending files.");
                    }

                    if made_progress {
                        idle = false;
                        tokio::task::yield_now().await;
                    } else {
                        let signaled = wait_for_runtime_work_activity_or_timeout_async(
                            autonomous_ingestor_idle_wait(policy.sleep, queue.common_len()),
                        )
                        .await;
                        if signaled {
                            if idle {
                                service_guard::record_background_runtime_wakeup(
                                    service_guard::BackgroundWakeDetail::AutonomousIngestor,
                                    0,
                                    0,
                                );
                            }
                            idle = false;
                        } else {
                            idle = true;
                        }
                    }
                    continue;
                }
            }
            let signaled = wait_for_runtime_work_activity_or_timeout_async(
                autonomous_ingestor_idle_wait(policy.sleep, queue.common_len()),
            )
            .await;
            if signaled {
                if idle {
                    service_guard::record_background_runtime_wakeup(
                        service_guard::BackgroundWakeDetail::AutonomousIngestor,
                        0,
                        0,
                    );
                }
                idle = false;
            } else {
                idle = true;
            }
        }
    });
}

async fn wait_for_runtime_work_activity_or_timeout_async(timeout: std::time::Duration) -> bool {
    tokio::task::spawn_blocking(move || {
        service_guard::wait_for_runtime_work_activity_or_timeout(timeout)
    })
    .await
    .unwrap_or(false)
}

pub(crate) fn runtime_telemetry_snapshot(
    store: &GraphStore,
    queue: &QueueStore,
    ingress_buffer: &SharedIngressBuffer,
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
    let guard_metrics = guard_metrics_snapshot();
    let ingress_metrics = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .metrics_snapshot();
    let process_memory = process_memory_snapshot();
    let storage = duckdb_storage_snapshot(store);
    let duckdb_memory = duckdb_memory_snapshot(store);
    let (graph_projection_queue_queued, graph_projection_queue_inflight) = store
        .fetch_graph_projection_queue_counts()
        .unwrap_or((0, 0));
    let graph_projection_queue_depth =
        graph_projection_queue_queued + graph_projection_queue_inflight;
    let persisted_file_pending_depth = if graph_runtime_enabled {
        store.count_persisted_file_pending().unwrap_or(0)
    } else {
        0
    };
    let (file_vectorization_queue_queued, file_vectorization_queue_inflight) =
        if vector_runtime_enabled {
            store
                .fetch_file_vectorization_queue_counts()
                .unwrap_or((0, 0))
        } else {
            (0, 0)
        };
    let file_vectorization_queue_depth =
        file_vectorization_queue_queued + file_vectorization_queue_inflight;
    let runtime_truth_feed = service_guard::current_runtime_truth_feed();
    service_guard::record_graph_vector_priority_context(
        persisted_file_pending_depth,
        file_vectorization_queue_depth,
    );
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
    let utility_scheduler = current_utility_first_scheduler_diagnostics(
        effective_graph_scheduler_depth,
        file_vectorization_queue_depth,
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
        guard_hits: guard_metrics.hits,
        guard_misses: guard_metrics.misses,
        guard_bypassed_total: guard_metrics.bypassed_total,
        guard_hydrated_entries: guard_metrics.hydrated_entries,
        guard_hydration_duration_ms: guard_metrics.hydration_duration_ms,
        ingress_enabled: ingress_metrics.enabled,
        ingress_buffered_entries: ingress_metrics.buffered_entries,
        ingress_hot_entries: ingress_metrics.hot_entries,
        ingress_scan_entries: ingress_metrics.scan_entries,
        ingress_subtree_hints: ingress_metrics.subtree_hints,
        ingress_subtree_hint_in_flight: ingress_metrics.subtree_hint_in_flight,
        ingress_subtree_hint_accepted_total: ingress_metrics.subtree_hint_accepted_total,
        ingress_subtree_hint_blocked_total: ingress_metrics.subtree_hint_blocked_total,
        ingress_subtree_hint_suppressed_total: ingress_metrics.subtree_hint_suppressed_total,
        ingress_subtree_hint_productive_total: ingress_metrics.subtree_hint_productive_total,
        ingress_subtree_hint_unproductive_total: ingress_metrics.subtree_hint_unproductive_total,
        ingress_subtree_hint_dropped_total: ingress_metrics.subtree_hint_dropped_total,
        ingress_collapsed_total: ingress_metrics.collapsed_total,
        ingress_flush_count: ingress_metrics.flush_count,
        ingress_last_flush_duration_ms: ingress_metrics.last_flush_duration_ms,
        ingress_last_promoted_count: ingress_metrics.last_promoted_count,
        ingress_promoted_total: ingress_metrics.promoted_total,
        ingress_last_durably_persisted_count: ingress_metrics.last_durably_persisted_count,
        ingress_durably_persisted_total: ingress_metrics.durably_persisted_total,
        ingress_last_excluded_from_pending_count: ingress_metrics.last_excluded_from_pending_count,
        ingress_excluded_from_pending_total: ingress_metrics.excluded_from_pending_total,
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
        db_file_bytes: storage.db_file_bytes,
        db_wal_bytes: storage.db_wal_bytes,
        db_total_bytes: storage.db_total_bytes,
        duckdb_memory_bytes: duckdb_memory.memory_usage_bytes,
        duckdb_temporary_bytes: duckdb_memory.temporary_storage_bytes,
        graph_projection_queue_queued,
        graph_projection_queue_inflight,
        graph_projection_queue_depth,
        file_vectorization_queue_queued,
        file_vectorization_queue_inflight,
        file_vectorization_queue_depth,
        vector_chunks_embedded_total: service_guard::vector_chunks_embedded_total(),
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

fn should_flush_ingress_buffer(metrics: &IngressMetricsSnapshot, elapsed: Duration) -> bool {
    if metrics.buffered_entries == 0 && metrics.subtree_hints == 0 {
        return false;
    }

    if metrics.buffered_entries >= INGRESS_FORCE_BATCH_SIZE {
        return true;
    }

    if metrics.hot_entries > 0 && elapsed >= Duration::from_millis(INGRESS_HOT_FLUSH_WINDOW_MS) {
        return true;
    }

    if metrics.subtree_hints > 0 && elapsed >= Duration::from_millis(INGRESS_HINT_FLUSH_WINDOW_MS) {
        return true;
    }

    metrics.scan_entries > 0 && elapsed >= Duration::from_millis(INGRESS_BULK_FLUSH_WINDOW_MS)
}

fn ingress_promoter_sleep_ms(metrics: &IngressMetricsSnapshot, elapsed: Duration) -> u64 {
    if !metrics.enabled || (metrics.buffered_entries == 0 && metrics.subtree_hints == 0) {
        return ingress_promoter_idle_wait_ms();
    }
    let base_poll_ms = ingress_promoter_poll_interval_ms();
    if metrics.buffered_entries >= INGRESS_FORCE_BATCH_SIZE {
        return base_poll_ms;
    }

    let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    let target_window_ms = if metrics.hot_entries > 0 {
        INGRESS_HOT_FLUSH_WINDOW_MS
    } else if metrics.subtree_hints > 0 {
        INGRESS_HINT_FLUSH_WINDOW_MS
    } else if metrics.scan_entries > 0 {
        INGRESS_BULK_FLUSH_WINDOW_MS
    } else {
        base_poll_ms
    };

    if elapsed_ms >= target_window_ms {
        return base_poll_ms;
    }

    let remaining = target_window_ms.saturating_sub(elapsed_ms);
    let floor = base_poll_ms.min(target_window_ms);
    let ceiling = base_poll_ms.max(target_window_ms);
    remaining.clamp(floor, ceiling)
}

fn ingress_promoter_backoff_ms(
    metrics: &IngressMetricsSnapshot,
    elapsed: Duration,
    zero_admission_streak: u32,
) -> u64 {
    let base = ingress_promoter_sleep_ms(metrics, elapsed);
    if metrics.hot_entries > 0
        || metrics.scan_entries >= INGRESS_HOT_PRIORITY_BATCH_CAP
        || metrics.buffered_entries >= INGRESS_MAX_BATCH_SIZE
        || zero_admission_streak == 0
    {
        return base;
    }
    let scaled = base.saturating_mul(1u64 << zero_admission_streak.min(3));
    let ceiling = ingress_promoter_idle_wait_ms().max(base);
    scaled.clamp(base, ceiling)
}

fn admission_controller_decision(
    metrics: &IngressMetricsSnapshot,
    persisted_file_pending_current: usize,
    graph_wip_current: usize,
) -> AdmissionControllerDecision {
    let runtime_profile = RuntimeProfile::detect();
    let profile = recommend_admission_controller_profile(&runtime_profile);
    let state = current_admission_controller_state(
        profile,
        metrics.buffered_entries,
        metrics.hot_entries,
        metrics.scan_entries,
        persisted_file_pending_current,
        graph_wip_current,
        false,
        matches!(service_guard::current_pressure(), ServicePressure::Critical),
    );

    AdmissionControllerDecision {
        target_band: state.profile.target_band,
        reorder_point: state.profile.reorder_point,
        max_wip: state.profile.max_wip,
        hold_window_ms: state.profile.hold_window_ms,
        forced_bulk_fill_threshold: state.profile.forced_bulk_fill_threshold,
        blocking_authority: state.blocking_authority,
        bulk_fill_preferred: state.bulk_fill_preferred,
    }
}

fn ingress_flush_batch_size(
    metrics: &IngressMetricsSnapshot,
    persisted_file_pending_current: usize,
    graph_wip_current: usize,
) -> usize {
    let controller =
        admission_controller_decision(metrics, persisted_file_pending_current, graph_wip_current);
    if controller.bulk_fill_preferred {
        return INGRESS_FORCE_BATCH_SIZE;
    }

    if controller.blocking_authority != "none" {
        return INGRESS_HOT_PRIORITY_BATCH_CAP.min(INGRESS_MAX_BATCH_SIZE);
    }

    if metrics.hot_entries == 0 {
        return INGRESS_MAX_BATCH_SIZE;
    }

    if metrics.scan_entries >= INGRESS_MAX_BATCH_SIZE
        || metrics.buffered_entries >= INGRESS_FORCE_BATCH_SIZE
    {
        return INGRESS_MAX_BATCH_SIZE;
    }

    if metrics.scan_entries >= INGRESS_HOT_PRIORITY_BATCH_CAP {
        return INGRESS_MIXED_BULK_BATCH_SIZE.min(INGRESS_MAX_BATCH_SIZE);
    }

    INGRESS_HOT_PRIORITY_BATCH_CAP.min(INGRESS_MAX_BATCH_SIZE)
}

fn flush_ingress_buffer_once(
    store: Arc<GraphStore>,
    projects_root: &str,
    file_ingress_guard: &SharedFileIngressGuard,
    ingress_buffer: &SharedIngressBuffer,
) -> anyhow::Result<IngressFlushOutcome> {
    let metrics = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .metrics_snapshot();
    if !metrics.enabled || (metrics.buffered_entries == 0 && metrics.subtree_hints == 0) {
        return Ok(IngressFlushOutcome::default());
    }

    let persisted_file_pending_current = store.count_persisted_file_pending()?;
    let graph_wip_current = store.count_graph_wip_files()?;
    let controller =
        admission_controller_decision(&metrics, persisted_file_pending_current, graph_wip_current);
    let batch_size =
        ingress_flush_batch_size(&metrics, persisted_file_pending_current, graph_wip_current);
    let batch = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .drain_batch(batch_size);
    if batch.files.is_empty() && batch.tombstones.is_empty() && batch.subtree_hints.is_empty() {
        return Ok(IngressFlushOutcome::default());
    }

    let started_at = Instant::now();
    let batch_paths = batch
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let pending_before_for_batch = if batch_paths.is_empty() {
        0
    } else {
        count_pending_graph_eligible_rows(&store.fetch_file_ingress_rows(&batch_paths)?)
    };
    let promoted = store.promote_ingress_batch(&batch)?;

    let mut durably_persisted_count = 0usize;
    let mut persisted_but_excluded_count = 0usize;
    if !batch.files.is_empty() {
        if let Ok(rows) = store.fetch_file_ingress_rows(&batch_paths) {
            durably_persisted_count = rows.len();
            let pending_after_for_batch = count_pending_graph_eligible_rows(&rows);
            persisted_but_excluded_count = rows
                .iter()
                .filter(|row| !row.is_pending_graph_eligible())
                .count();
            let admitted_delta = pending_after_for_batch.saturating_sub(pending_before_for_batch);
            let mut locked = file_ingress_guard
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for row in rows {
                locked.record_committed_row(row);
            }
            let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            record_ingress_flush(
                elapsed_ms,
                admitted_delta,
                durably_persisted_count,
                persisted_but_excluded_count,
            );
            watcher_probe::record(
                "ingress.promoted",
                None,
                format!(
                    "files={} tombstones={} subtree_hints={} subtree_discovered={} admitted_delta={} durably_persisted={} excluded_from_pending={} duration_ms={} admission_blocking_authority={} target_band={} reorder_point={} max_wip={} hold_window_ms={} forced_bulk_fill_threshold={} bulk_fill_preferred={}",
                    promoted.promoted_files,
                    promoted.promoted_tombstones,
                    batch.subtree_hints.len(),
                    0,
                    admitted_delta,
                    durably_persisted_count,
                    persisted_but_excluded_count,
                    elapsed_ms,
                    controller.blocking_authority,
                    controller.target_band,
                    controller.reorder_point,
                    controller.max_wip,
                    controller.hold_window_ms,
                    controller.forced_bulk_fill_threshold,
                    controller.bulk_fill_preferred
                ),
            );

            return Ok(IngressFlushOutcome {
                admitted_count: admitted_delta,
                subtree_discovered_count: 0,
            });
        }
    }

    if !batch.tombstones.is_empty() {
        let mut locked = file_ingress_guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in &batch.tombstones {
            locked.record_tombstone(Path::new(path));
        }
    }

    if !batch.subtree_hints.is_empty() {
        spawn_subtree_hint_scans(
            projects_root.to_string(),
            store.clone(),
            file_ingress_guard.clone(),
            ingress_buffer.clone(),
            batch.subtree_hints.clone(),
        );
    }

    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    record_ingress_flush(
        elapsed_ms,
        0,
        durably_persisted_count,
        persisted_but_excluded_count,
    );
    watcher_probe::record(
        "ingress.promoted",
        None,
        format!(
            "files={} tombstones={} subtree_hints={} subtree_discovered={} admitted_delta={} durably_persisted={} excluded_from_pending={} duration_ms={} admission_blocking_authority={} target_band={} reorder_point={} max_wip={} hold_window_ms={} forced_bulk_fill_threshold={} bulk_fill_preferred={}",
            promoted.promoted_files,
            promoted.promoted_tombstones,
            batch.subtree_hints.len(),
            0,
            0,
            durably_persisted_count,
            persisted_but_excluded_count,
            elapsed_ms,
            controller.blocking_authority,
            controller.target_band,
            controller.reorder_point,
            controller.max_wip,
            controller.hold_window_ms,
            controller.forced_bulk_fill_threshold,
            controller.bulk_fill_preferred
        ),
    );

    Ok(IngressFlushOutcome {
        admitted_count: 0,
        subtree_discovered_count: 0,
    })
}

fn count_pending_graph_eligible_rows(rows: &[FileIngressRow]) -> usize {
    rows.iter()
        .filter(|row| row.is_pending_graph_eligible())
        .count()
}

fn spawn_subtree_hint_scans(
    projects_root: String,
    store: Arc<GraphStore>,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
    subtree_hints: Vec<IngressSubtreeHint>,
) {
    for hint in subtree_hints {
        let projects_root = projects_root.clone();
        let store = store.clone();
        let file_ingress_guard = file_ingress_guard.clone();
        let ingress_buffer = ingress_buffer.clone();
        std::thread::spawn(move || {
            let scanner = Scanner::new(&projects_root, "");
            let promoted_hint_files = scanner.scan_subtree_with_guard_and_ingress(
                store,
                Path::new(&hint.path),
                Some(&file_ingress_guard),
                Some(&ingress_buffer),
            );
            ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .complete_subtree_hint_with_stats(&hint.path, promoted_hint_files);
            watcher_probe::record(
                "ingress.subtree_hint_completed",
                None,
                format!(
                    "path={} discovered_files={}",
                    hint.path, promoted_hint_files
                ),
            );
        });
    }
}

fn plan_admissions(
    queue: &QueueStore,
    candidates: Vec<PendingFile>,
    max_count: usize,
) -> AdmissionPlan {
    if max_count == 0 || candidates.is_empty() {
        return AdmissionPlan::default();
    }

    let mut remaining_budget = queue.remaining_budget_bytes();
    let mut plan = AdmissionPlan::default();
    let mut hot_candidates = Vec::new();
    let mut normal_candidates = Vec::new();

    for candidate in candidates {
        if candidate.priority >= HOT_PRIORITY {
            hot_candidates.push(candidate);
        } else {
            normal_candidates.push(candidate);
        }
    }

    fill_admission_plan(
        queue,
        &mut remaining_budget,
        max_count,
        &mut plan,
        hot_candidates,
    );
    if plan.selected.len() < max_count {
        fill_admission_plan(
            queue,
            &mut remaining_budget,
            max_count,
            &mut plan,
            normal_candidates,
        );
    } else {
        plan.deferred.extend(normal_candidates);
    }

    plan
}

fn fill_admission_plan(
    queue: &QueueStore,
    remaining_budget: &mut u64,
    max_count: usize,
    plan: &mut AdmissionPlan,
    mut candidates: Vec<PendingFile>,
) {
    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| fairness_bucket(right).cmp(&fairness_bucket(left)))
            .then_with(|| right.defer_count.cmp(&left.defer_count))
            .then_with(|| {
                left.last_deferred_at_ms
                    .unwrap_or(i64::MAX)
                    .cmp(&right.last_deferred_at_ms.unwrap_or(i64::MAX))
            })
            .then_with(|| left.size_bytes.cmp(&right.size_bytes))
            .then_with(|| left.path.cmp(&right.path))
    });

    for candidate in candidates {
        if plan.selected.len() >= max_count {
            plan.deferred.push(candidate);
            continue;
        }

        let estimated_cost = queue.estimate_cost_for_path_in_mode(
            &candidate.path,
            candidate.size_bytes,
            ProcessingMode::Full,
        );
        let degraded_cost = queue.estimate_cost_for_path_in_mode(
            &candidate.path,
            candidate.size_bytes,
            ProcessingMode::StructureOnly,
        );

        if !queue.can_fit_alone_in_mode(&candidate.path, candidate.size_bytes, ProcessingMode::Full)
        {
            if queue.can_fit_alone_in_mode(
                &candidate.path,
                candidate.size_bytes,
                ProcessingMode::StructureOnly,
            ) && candidate.defer_count >= OVERSIZED_PROBATION_DEFER_THRESHOLD
                && degraded_cost <= *remaining_budget
            {
                *remaining_budget = remaining_budget.saturating_sub(degraded_cost);
                plan.degraded.push(candidate.path.clone());
                plan.selected.push(AdmissionSelection {
                    file: candidate,
                    mode: ProcessingMode::StructureOnly,
                });
            } else if candidate.defer_count < OVERSIZED_PROBATION_DEFER_THRESHOLD
                || queue.can_fit_alone_in_mode(
                    &candidate.path,
                    candidate.size_bytes,
                    ProcessingMode::StructureOnly,
                )
            {
                plan.deferred.push(candidate);
            } else {
                plan.oversized.push(candidate);
            }
        } else if estimated_cost <= *remaining_budget {
            *remaining_budget = remaining_budget.saturating_sub(estimated_cost);
            plan.selected.push(AdmissionSelection {
                file: candidate,
                mode: ProcessingMode::Full,
            });
        } else {
            plan.deferred.push(candidate);
        }
    }
}

fn enqueue_claimed_files(
    store: &GraphStore,
    queue: &QueueStore,
    files: Vec<PendingFile>,
    selected_modes: &std::collections::HashMap<String, ProcessingMode>,
) {
    for file in files {
        let mut mode = selected_modes
            .get(&file.path)
            .copied()
            .unwrap_or(ProcessingMode::Full);

        if !queue.can_fit_alone_in_mode(&file.path, file.size_bytes, mode) {
            if mode == ProcessingMode::Full
                && queue.can_fit_alone_in_mode(
                    &file.path,
                    file.size_bytes,
                    ProcessingMode::StructureOnly,
                )
            {
                mode = ProcessingMode::StructureOnly;
            } else {
                record_oversized_refusal();
                warn!(
                    "Autonomous Ingestor marked {} as oversized for current budget (priority={}, size={}).",
                    file.path,
                    file.priority,
                    file.size_bytes
                );
                if let Err(err) = store.mark_file_oversized_for_current_budget(&file.path) {
                    error!(
                        "Autonomous Ingestor failed to mark oversized claimed file {}: {}",
                        file.path, err
                    );
                }
                continue;
            }
        }

        let is_hot = file.priority >= HOT_PRIORITY;
        if matches!(mode, ProcessingMode::StructureOnly) {
            record_structure_only_admission();
        }

        if let Err(err) = queue.push_with_mode(&file.path, 0, &file.trace_id, 0, 0, is_hot, mode) {
            record_oversized_refusal();
            warn!(
                "Autonomous Ingestor failed to enqueue {} (priority={}, mode={:?}): {}. Requeueing claim.",
                file.path,
                file.priority,
                mode,
                err
            );
            if let Err(requeue_err) = store
                .requeue_claimed_file_with_reason(&file.path, "requeued_after_queue_push_failure")
            {
                error!(
                    "Autonomous Ingestor failed to requeue claimed file {} after queue pressure: {}",
                    file.path,
                    requeue_err
                );
            }
        }
    }
}

fn run_initial_scan(
    store: Arc<GraphStore>,
    project_root: String,
    project_code: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    info!(
        "🚀 Auto-Ignition: Beginning initial workspace mapping for {}...",
        project_root
    );
    let scanner = axon_core::scanner::Scanner::new(&project_root, &project_code);
    scanner.scan_with_guard_and_ingress(store, Some(&file_ingress_guard), Some(&ingress_buffer));
    info!(
        "✅ Auto-Ignition: Initial mapping sequence complete for {}.",
        project_root
    );
}

pub(crate) fn spawn_hot_delta_watcher(
    store: Arc<GraphStore>,
    project_root: String,
    project_code: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || {
        let watch_root = PathBuf::from(project_root);
        let watcher_project_code = project_code.clone();
        let preferred_project_root = Some(watch_root.clone());
        let watcher_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            info!(
                "Rust FS watcher preparing targets under {}",
                watch_root.display()
            );

            let callback_root = watch_root.clone();
            let callback_project_code = watcher_project_code.clone();
            let callback_store = store.clone();
            let callback_guard = file_ingress_guard.clone();
            let callback_ingress = ingress_buffer.clone();
            let callback_active_project_root = preferred_project_root.clone();
            let rescan_guard = Arc::new(AtomicBool::new(false));
            let callback_rescan_guard = rescan_guard.clone();
            let cold_arm_completed_at = Arc::new(Mutex::new(None));
            let callback_cold_arm_completed_at = cold_arm_completed_at.clone();
            let watcher_started_at = Instant::now();

            let mut debouncer = match new_debouncer(
                Duration::from_millis(750),
                None,
                move |result: DebounceEventResult| {
                    handle_watcher_events(
                        callback_store.clone(),
                        callback_root.clone(),
                        callback_project_code.clone(),
                        callback_guard.clone(),
                        callback_ingress.clone(),
                        callback_active_project_root.clone(),
                        callback_rescan_guard.clone(),
                        callback_cold_arm_completed_at.clone(),
                        watcher_started_at,
                        result,
                    );
                },
            ) {
                Ok(debouncer) => debouncer,
                Err(err) => {
                    error!("Rust FS watcher initialization failed: {}", err);
                    return;
                }
            };

            let mut hot_targets = active_project_hot_targets(preferred_project_root.as_deref());
            hot_targets.insert(
                0,
                WatchTarget {
                    path: watch_root.clone(),
                    recursive: false,
                },
            );
            let cold_targets: Vec<WatchTarget> = Vec::new(); // Children are now fully handled by hot_targets since watch_root == preferred_project_root

            let mut armed = 0usize;
            let hot_started_at = Instant::now();
            for target in hot_targets {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match debouncer.watch(&target.path, mode) {
                    Ok(_) => {
                        armed += 1;
                        info!(
                            "Rust FS watcher armed hot target {} ({}) after {} ms",
                            target.path.display(),
                            if target.recursive {
                                "recursive"
                            } else {
                                "non-recursive"
                            },
                            hot_started_at.elapsed().as_millis()
                        );
                    }
                    Err(err) => {
                        warn!(
                            "Rust FS watcher skipped target {}: {}",
                            target.path.display(),
                            err
                        );
                    }
                }
            }

            if armed > 0 {
                info!(
                    "Rust FS watcher armed hot set on {} target(s) under {}",
                    armed,
                    watch_root.display()
                );
            }

            for target in cold_targets {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match debouncer.watch(&target.path, mode) {
                    Ok(_) => {
                        armed += 1;
                        debug!(
                            "Rust FS watcher armed target {} ({})",
                            target.path.display(),
                            if target.recursive {
                                "recursive"
                            } else {
                                "non-recursive"
                            }
                        );
                    }
                    Err(err) => {
                        warn!(
                            "Rust FS watcher skipped target {}: {}",
                            target.path.display(),
                            err
                        );
                    }
                }
            }

            if armed == 0 {
                error!(
                    "Rust FS watcher failed to arm any target under {}",
                    watch_root.display()
                );
                return;
            }

            info!(
                "Rust FS watcher armed on {} target(s) under {}",
                armed,
                watch_root.display()
            );
            if let Ok(mut armed_at) = cold_arm_completed_at.lock() {
                *armed_at = Some(Instant::now());
            }

            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }));

        if let Err(payload) = watcher_result {
            let reason = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            error!("Rust FS watcher thread panicked: {}", reason);
        }
    });
}

pub(crate) fn spawn_ingress_promoter(
    store: Arc<GraphStore>,
    projects_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    std::thread::spawn(move || {
        let mut last_flush = Instant::now();
        let mut zero_admission_streak = 0u32;

        loop {
            let metrics = ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .metrics_snapshot();

            if !metrics.enabled {
                let signaled = wait_for_ingress_activity_or_timeout(Duration::from_millis(
                    ingress_promoter_backoff_ms(
                        &metrics,
                        last_flush.elapsed(),
                        zero_admission_streak,
                    ),
                ));
                if signaled {
                    service_guard::record_background_runtime_wakeup(
                        service_guard::BackgroundWakeDetail::IngressPromoter,
                        0,
                        0,
                    );
                }
                continue;
            }

            if !should_flush_ingress_buffer(&metrics, last_flush.elapsed()) {
                let signaled = wait_for_ingress_activity_or_timeout(Duration::from_millis(
                    ingress_promoter_backoff_ms(
                        &metrics,
                        last_flush.elapsed(),
                        zero_admission_streak,
                    ),
                ));
                if signaled {
                    service_guard::record_background_runtime_wakeup(
                        service_guard::BackgroundWakeDetail::IngressPromoter,
                        0,
                        0,
                    );
                }
                continue;
            }

            service_guard::record_background_runtime_wakeup(
                service_guard::BackgroundWakeDetail::IngressPromoter,
                0,
                0,
            );

            match flush_ingress_buffer_once(
                store.clone(),
                &projects_root,
                &file_ingress_guard,
                &ingress_buffer,
            ) {
                Ok(outcome) if outcome.admitted_count > 0 => {
                    zero_admission_streak = 0;
                    last_flush = Instant::now();
                }
                Ok(_outcome) => {
                    zero_admission_streak = zero_admission_streak.saturating_add(1);
                    let signaled = wait_for_ingress_activity_or_timeout(Duration::from_millis(
                        ingress_promoter_backoff_ms(
                            &metrics,
                            last_flush.elapsed(),
                            zero_admission_streak,
                        ),
                    ));
                    if signaled {
                        service_guard::record_background_runtime_wakeup(
                            service_guard::BackgroundWakeDetail::IngressPromoter,
                            0,
                            0,
                        );
                    }
                }
                Err(err) => {
                    zero_admission_streak = zero_admission_streak.saturating_add(1);
                    warn!("Ingress promoter flush failed: {}", err);
                    std::thread::sleep(Duration::from_millis(INGRESS_BULK_FLUSH_WINDOW_MS));
                }
            }
        }
    });
}

fn active_project_hot_targets(preferred_root: Option<&Path>) -> Vec<WatchTarget> {
    let Some(preferred_root) = preferred_root else {
        return Vec::new();
    };

    let mut targets = vec![WatchTarget {
        path: preferred_root.to_path_buf(),
        recursive: false,
    }];

    let entries = match std::fs::read_dir(preferred_root) {
        Ok(entries) => entries,
        Err(_) => return targets,
    };

    let mut child_targets = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|segment| segment.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if std::fs::read_dir(&path).is_err() {
            continue;
        }
        child_targets.push(WatchTarget {
            path,
            recursive: true,
        });
    }

    child_targets.sort_by_key(|target| project_hot_target_rank(&target.path));
    targets.extend(child_targets);
    targets
}

fn project_hot_target_rank(path: &Path) -> (u8, String) {
    let name = path
        .file_name()
        .and_then(|segment| segment.to_str())
        .unwrap_or_default()
        .to_string();

    let rank = match name.as_str() {
        "src" => 0,
        "lib" => 1,
        "test" | "tests" => 2,
        "docs" => 3,
        "scripts" => 4,
        _ => 10,
    };

    (rank, name)
}

#[derive(Debug, Clone, Copy)]
struct ClaimPolicy {
    mode: ClaimMode,
    claim_count: usize,
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
                sleep: quiescent_scaled_claim_sleep(1_000, queue_len),
            };
        }
        InteractivePriority::InteractivePriority => {
            service_guard::record_background_launch_suppressed();
            return ClaimPolicy {
                mode: ClaimMode::Guarded,
                claim_count: 50,
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
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Guarded, queue_len),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow, queue_len),
        };
    }

    if budget_exhaustion_ratio >= 0.72 {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
            sleep: dynamic_claim_sleep(dynamic_pressure, ClaimMode::Slow, queue_len),
        };
    }

    ClaimPolicy {
        mode: ClaimMode::Fast,
        claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Fast),
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

fn record_oversized_refusal() {
    OVERSIZED_REFUSALS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn record_structure_only_admission() {
    DEGRADED_MODE_ENTRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn record_claim_mode_transition(mode: ClaimMode) {
    let code = claim_mode_code(mode);
    let previous = LAST_REPORTED_CLAIM_MODE.swap(code, Ordering::Relaxed);

    if previous != code && matches!(mode, ClaimMode::Guarded | ClaimMode::Paused) {
        DEGRADED_MODE_ENTRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

fn claim_mode_code(mode: ClaimMode) -> u8 {
    match mode {
        ClaimMode::Fast => 0,
        ClaimMode::Slow => 1,
        ClaimMode::Guarded => 2,
        ClaimMode::Paused => 3,
    }
}

fn fairness_bucket(candidate: &PendingFile) -> u8 {
    if candidate.defer_count >= FAIRNESS_PROMOTION_DEFER_THRESHOLD {
        1
    } else {
        0
    }
}

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

#[allow(clippy::too_many_arguments)]
fn handle_watcher_events(
    store: Arc<GraphStore>,
    watch_root: std::path::PathBuf,
    project_code: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
    active_project_root: Option<PathBuf>,
    rescan_guard: Arc<AtomicBool>,
    cold_arm_completed_at: Arc<Mutex<Option<Instant>>>,
    watcher_started_at: Instant,
    result: DebounceEventResult,
) {
    match result {
        Ok(events) => {
            let mut paths = Vec::new();
            let mut rescan_requested = false;

            for event in events {
                if event.need_rescan() {
                    rescan_requested = true;
                }
                paths.extend(event.paths.iter().cloned());
            }

            let cold_arm_completed_at = cold_arm_completed_at.lock().ok().and_then(|guard| *guard);

            if should_suppress_bootstrap_event_storm(
                paths.len(),
                watcher_started_at,
                cold_arm_completed_at,
            ) {
                let salvaged = bootstrap_salvage_paths(&paths, active_project_root.as_deref());
                warn!(
                    "Rust FS watcher suppressed bootstrap event storm ({} path(s)) under {}",
                    paths.len(),
                    watch_root.display()
                );
                watcher_probe::record(
                    "watcher.storm_suppressed",
                    None,
                    format!("paths={} salvaged={}", paths.len(), salvaged.len()),
                );
                if !salvaged.is_empty() {
                    match fs_watcher::enqueue_hot_deltas_with_guard(
                        Some(store.as_ref()),
                        &watch_root,
                        &project_code,
                        salvaged.clone(),
                        HOT_PRIORITY,
                        &file_ingress_guard,
                        &ingress_buffer,
                    ) {
                        Ok(staged) if staged > 0 => {
                            info!(
                                "Rust FS watcher buffered {} hot delta(s) from bootstrap storm.",
                                staged
                            );
                            watcher_probe::record(
                                "watcher.storm_salvaged",
                                None,
                                format!("buffered={}", staged),
                            );
                        }
                        Ok(_) => {
                            watcher_probe::record(
                                "watcher.storm_salvaged_none",
                                None,
                                format!("candidates={}", salvaged.len()),
                            );
                        }
                        Err(err) => {
                            warn!("Rust FS watcher failed to salvage hot delta(s): {}", err);
                            watcher_probe::record(
                                "watcher.storm_salvage_failed",
                                None,
                                err.to_string(),
                            );
                        }
                    }
                }
                return;
            }

            if !paths.is_empty() {
                // REQ-AXO-331: per-batch event log is high-volume (84 % of indexer log
                // bytes under 19-project /home/dstadel/projects watch root). Downgraded
                // to DEBUG; aggregate counts remain queryable via watcher_probe::recent().
                debug!(
                    "Rust FS watcher received {} path event(s) under {}",
                    paths.len(),
                    watch_root.display()
                );
                watcher_probe::record("watcher.received", None, format!("paths={}", paths.len()));
            }

            if rescan_requested {
                if !rescan_guard.swap(true, Ordering::SeqCst) {
                    watcher_probe::record(
                        "watcher.rescan_requested",
                        None,
                        format!("paths={}", paths.len()),
                    );
                    let rescan_store = store.clone();
                    let rescan_root = watch_root.clone();
                    let rescan_project_code = project_code.clone();
                    let rescan_guard_state = file_ingress_guard.clone();
                    let rescan_ingress = ingress_buffer.clone();
                    let rescan_guard_release = rescan_guard.clone();
                    std::thread::spawn(move || {
                        let _guard_reset = RescanGuardReset::new(rescan_guard_release);
                        warn!(
                            "Rust FS watcher requested a safety rescan on {}",
                            rescan_root.display()
                        );
                        watcher_probe::record(
                            "watcher.rescan_started",
                            Some(&rescan_root),
                            "reason=notify_rescan",
                        );
                        Scanner::new(rescan_root.to_string_lossy().as_ref(), &rescan_project_code)
                            .scan_with_guard_and_ingress(
                                rescan_store,
                                Some(&rescan_guard_state),
                                Some(&rescan_ingress),
                            );
                        watcher_probe::record(
                            "watcher.rescan_completed",
                            Some(&rescan_root),
                            "status=ok",
                        );
                    });
                } else {
                    watcher_probe::record("watcher.rescan_skipped", None, "reason=guard_active");
                }
            }

            match fs_watcher::enqueue_hot_deltas_with_guard(
                Some(store.as_ref()),
                &watch_root,
                &project_code,
                paths,
                HOT_PRIORITY,
                &file_ingress_guard,
                &ingress_buffer,
            ) {
                Ok(staged) if staged > 0 => {
                    // REQ-AXO-331: per-batch buffer log downgraded to DEBUG (paired
                    // with watcher.received). watcher.staged remains INFO inside
                    // fs_watcher for the actual file-level staging event.
                    debug!("Rust FS watcher buffered {} hot delta(s).", staged);
                    watcher_probe::record(
                        "watcher.buffered_batch",
                        None,
                        format!("buffered={}", staged),
                    );
                }
                Ok(_) => {
                    debug!("Rust FS watcher received event(s) but buffered no hot delta.");
                    watcher_probe::record(
                        "watcher.buffered_none",
                        None,
                        "reason=no_eligible_delta",
                    );
                }
                Err(err) => {
                    watcher_probe::record("watcher.buffering_failed", None, err.to_string());
                    warn!("Rust FS watcher failed to buffer hot delta(s): {}", err)
                }
            }
        }
        Err(errors) => {
            for err in errors {
                watcher_probe::record("watcher.error", None, err.to_string());
                warn!("Rust FS watcher event error: {}", err);
            }
        }
    }
}

struct RescanGuardReset {
    guard: Arc<AtomicBool>,
}

impl RescanGuardReset {
    fn new(guard: Arc<AtomicBool>) -> Self {
        Self { guard }
    }
}

impl Drop for RescanGuardReset {
    fn drop(&mut self) {
        self.guard.store(false, Ordering::SeqCst);
    }
}

fn should_suppress_bootstrap_event_storm(
    path_count: usize,
    watcher_started_at: Instant,
    cold_arm_completed_at: Option<Instant>,
) -> bool {
    if watcher_started_at.elapsed() <= Duration::from_secs(120) && path_count >= 5_000 {
        return true;
    }

    cold_arm_completed_at
        .map(|armed_at| armed_at.elapsed() <= Duration::from_secs(30) && path_count >= 1_000)
        .unwrap_or(false)
}

fn bootstrap_salvage_paths(paths: &[PathBuf], active_project_root: Option<&Path>) -> Vec<PathBuf> {
    let Some(active_project_root) = active_project_root else {
        return Vec::new();
    };

    paths
        .iter()
        .filter_map(|path| {
            let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let metadata = std::fs::metadata(&absolute).ok()?;
            if metadata.is_file() && absolute.starts_with(active_project_root) {
                Some(absolute)
            } else {
                None
            }
        })
        .collect()
}

fn federation_orchestration_candidates_from_identities(
    identities: Vec<axon_core::project_meta::CanonicalProjectIdentity>,
) -> Vec<(String, String)> {
    let mut projects: Vec<(String, String)> = identities
        .into_iter()
        .filter(|identity| identity.code != "PRO")
        .filter_map(|identity| {
            let project_path = identity.project_path.to_string_lossy().trim().to_string();
            if project_path.is_empty() {
                None
            } else {
                Some((identity.code, project_path))
            }
        })
        .collect();
    projects.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    projects
}

/// REQ-AXO-172 — Restrict the federation orchestrator's project sweep
/// to projects whose canonical path falls under the configured
/// `AXON_WATCH_DIR`. Without this filter, `discover_project_identities`
/// finds every project on disk via cwd / cwd-parent / repo-root /
/// repo-root-parent walking (project_meta.rs:candidate_directories),
/// so the orchestrator spawns a watcher and initial scan for ALL of
/// them — making AXON_WATCH_DIR isolation impossible and breaking
/// DEC-AXO-066 stepwise validation. Empty `watch_root` ⇒ identity
/// (no filter), preserving the legacy "scan everything" behaviour
/// for callers that have not opted in.
pub(crate) fn filter_orchestration_candidates_by_watch_root(
    candidates: Vec<(String, String)>,
    watch_root: &str,
) -> Vec<(String, String)> {
    if watch_root.trim().is_empty() {
        return candidates;
    }
    let watch_root_path = std::path::Path::new(watch_root);
    candidates
        .into_iter()
        .filter(|(_code, path)| std::path::Path::new(path).starts_with(watch_root_path))
        .collect()
}

pub(crate) fn spawn_federation_orchestrator(
    store: Arc<GraphStore>,
    watch_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    if !federation_orchestrator_enabled() {
        info!("Fédération : orchestrateur désactivé via AXON_ENABLE_FEDERATION_ORCHESTRATOR.");
        return;
    }
    std::thread::spawn(move || {
        let mut known_projects = std::collections::HashSet::new();
        let mut stable_sweeps_without_new_projects: u32 = 0;
        info!(
            "Fédération : Démarrage de l'orchestrateur de projets locaux (scope: {}).",
            if watch_root.is_empty() {
                "<unrestricted>"
            } else {
                watch_root.as_str()
            }
        );
        loop {
            let base_interval_ms = if stable_sweeps_without_new_projects >= 8 {
                30_000
            } else if stable_sweeps_without_new_projects >= 3 {
                10_000
            } else {
                1_000
            };
            std::thread::sleep(Duration::from_millis(quiescent_scaled_interval_ms(
                base_interval_ms,
                1_000,
                60_000,
            )));
            // REQ-AXO-172 — apply AXON_WATCH_DIR scoping at orchestration time so
            // a freshly-dropped meta.json under a path outside watch_root never
            // turns into a per-project scan + watcher.
            let local_projects = filter_orchestration_candidates_by_watch_root(
                federation_orchestration_candidates_from_identities(
                    axon_core::project_meta::discover_project_identities(),
                ),
                &watch_root,
            );
            let mut newly_discovered = Vec::new();
            for (project_code, path) in local_projects {
                if !known_projects.contains(&project_code) {
                    known_projects.insert(project_code.clone());
                    newly_discovered.push((project_code, path));
                }
            }

            if newly_discovered.is_empty() {
                stable_sweeps_without_new_projects =
                    stable_sweeps_without_new_projects.saturating_add(1);
                continue;
            }

            stable_sweeps_without_new_projects = 0;
            service_guard::record_background_runtime_wakeup(
                service_guard::BackgroundWakeDetail::FederationOrchestrator,
                0,
                0,
            );
            info!(
                "Fédération : {} nouveau(x) projet(s) local(aux) détecté(s) et orchestré(s).",
                newly_discovered.len()
            );
            let mut scan_jobs = Vec::new();
            for (project_code, path) in &newly_discovered {
                info!(
                    "Fédération : Nouveau projet local détecté et orchestré: {} ({})",
                    project_code, path
                );
                let scan_store = store.clone();
                let scan_path = path.clone();
                let scan_project_code = project_code.clone();
                let scan_guard = file_ingress_guard.clone();
                let scan_ingress = ingress_buffer.clone();
                scan_jobs.push(std::thread::spawn(move || {
                    run_initial_scan(
                        scan_store,
                        scan_path,
                        scan_project_code,
                        scan_guard,
                        scan_ingress,
                    );
                }));
            }

            for scan_job in scan_jobs {
                if let Err(payload) = scan_job.join() {
                    let reason = payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic payload".to_string());
                    error!("Fédération : initial scan thread panicked: {}", reason);
                }
            }

            for (project_code, path) in newly_discovered {
                spawn_hot_delta_watcher(
                    store.clone(),
                    path.clone(),
                    project_code.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );
            }
        }
    });
}

/// REQ-AXO-340 — periodic full re-scan of every federation-eligible project
/// under `AXON_WATCH_DIR`. The diff itself happens inside the canonical
/// Scanner → FileIngressGuard pipeline (`should_stage` returns
/// `SkipUnchanged` for files whose `(path, mtime, size)` already match the
/// guard's hydrated `FileIngressRow` snapshot). Only new or changed files
/// reach the `IngressBuffer`, so steady-state cost is bounded by
/// `walk + stat + HashMap lookup` per file. This closes the gap where
/// federation discovery runs the initial scan exactly once: any file added
/// outside an inotify-emitting touch (cold-clone, partial bootstrap failure,
/// projects discovered after the first sweep) is otherwise invisible until
/// the next process restart.
fn scope_reconciliation_enabled() -> bool {
    std::env::var("AXON_SCOPE_RECONCILE_ENABLED")
        .map(|raw| {
            !matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "off" | "no"
            )
        })
        .unwrap_or(true)
}

fn scope_reconciliation_interval_secs() -> u64 {
    const DEFAULT_SECS: u64 = 60;
    const MIN_SECS: u64 = 5;
    std::env::var("AXON_SCOPE_RECONCILE_INTERVAL_SECS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs >= MIN_SECS)
        .unwrap_or(DEFAULT_SECS)
}

pub(crate) fn spawn_scope_reconciliation_orchestrator(
    store: Arc<GraphStore>,
    watch_root: String,
    file_ingress_guard: SharedFileIngressGuard,
    ingress_buffer: SharedIngressBuffer,
) {
    if !scope_reconciliation_enabled() {
        info!("Reconciliation : orchestrateur désactivé via AXON_SCOPE_RECONCILE_ENABLED.");
        return;
    }
    let interval = Duration::from_secs(scope_reconciliation_interval_secs());
    info!(
        "Reconciliation : démarrage (scope: {}, interval: {}s).",
        if watch_root.is_empty() {
            "<unrestricted>"
        } else {
            watch_root.as_str()
        },
        interval.as_secs()
    );
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(interval);
            // REQ-AXO-340 — reuse the canonical federation candidate filter to
            // honour AXON_WATCH_DIR isolation (REQ-AXO-172). Projects without
            // `.axon/meta.json` are deliberately out of scope for this slice;
            // a follow-up REQ will auto-bootstrap meta.json for un-bound dirs.
            let candidates = filter_orchestration_candidates_by_watch_root(
                federation_orchestration_candidates_from_identities(
                    axon_core::project_meta::discover_project_identities(),
                ),
                &watch_root,
            );
            if candidates.is_empty() {
                continue;
            }
            let pass_start = std::time::Instant::now();
            let mut scanned = 0usize;
            for (project_code, project_path) in &candidates {
                let scanner = Scanner::new(project_path, project_code);
                scanner.scan_with_guard_and_ingress(
                    store.clone(),
                    Some(&file_ingress_guard),
                    Some(&ingress_buffer),
                );
                scanned += 1;
            }
            debug!(
                "Reconciliation : passe terminée — {} projet(s) scanné(s) en {} ms.",
                scanned,
                pass_start.elapsed().as_millis()
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        active_project_hot_targets, admission_controller_decision, bootstrap_salvage_paths,
        claim_policy, enqueue_claimed_files, federation_orchestration_candidates_from_identities,
        federation_orchestrator_enabled, filter_orchestration_candidates_by_watch_root,
        flush_ingress_buffer_once, handle_watcher_events, ingress_flush_batch_size,
        ingress_promoter_backoff_ms, ingress_promoter_sleep_ms, memory_limit_bytes,
        memory_reclaimer_enabled, memory_reclaimer_min_anon_bytes, optimizer_loop_interval_ms,
        plan_admissions, scope_reconciliation_enabled, scope_reconciliation_interval_secs,
        should_attempt_memory_reclaim, should_suppress_bootstrap_event_storm, ClaimMode,
        RescanGuardReset, INGRESS_FORCE_BATCH_SIZE, INGRESS_HOT_PRIORITY_BATCH_CAP,
        INGRESS_MAX_BATCH_SIZE, OVERSIZED_PROBATION_DEFER_THRESHOLD,
    };
    use axon_core::file_ingress_guard::FileIngressGuard;
    use axon_core::graph::{GraphStore, PendingFile};
    use axon_core::ingress_buffer::{
        ingress_metrics_snapshot, reset_ingress_metrics_for_tests, IngressBuffer,
        IngressMetricsSnapshot, IngressSource, SharedIngressBuffer,
    };
    use axon_core::queue::QueueStore;
    use axon_core::service_guard::ServicePressure;
    use axon_core::watcher_probe;
    use notify_debouncer_full::notify::event::{Flag, ModifyKind};
    use notify_debouncer_full::notify::Error;
    use notify_debouncer_full::notify::{Event, EventKind};
    use notify_debouncer_full::DebouncedEvent;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());
    static FLUSH_METRICS_GUARD: Mutex<()> = Mutex::new(());
    /// REQ-AXO-099 Phase 2 — serializes the watcher_probe-touching
    /// tests in this module. The probe is a process-global VecDeque;
    /// without serialization, one test's `clear()` wipes another
    /// test's recorded events between record and assertion.
    static WATCHER_PROBE_GUARD: Mutex<()> = Mutex::new(());

    fn test_file_ingress_guard() -> Arc<Mutex<FileIngressGuard>> {
        Arc::new(Mutex::new(FileIngressGuard::default()))
    }

    fn test_ingress_buffer() -> SharedIngressBuffer {
        Arc::new(Mutex::new(IngressBuffer::default()))
    }

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
    fn test_memory_reclaimer_enabled_defaults_to_true() {
        unsafe {
            std::env::remove_var("AXON_ENABLE_MEMORY_RECLAIMER");
        }
        assert!(memory_reclaimer_enabled());
    }

    #[test]
    fn test_federation_orchestrator_enabled_defaults_to_true() {
        unsafe {
            std::env::remove_var("AXON_ENABLE_FEDERATION_ORCHESTRATOR");
        }
        assert!(federation_orchestrator_enabled());
    }

    #[test]
    fn test_federation_orchestrator_enabled_respects_false_env() {
        unsafe {
            std::env::set_var("AXON_ENABLE_FEDERATION_ORCHESTRATOR", "false");
        }
        assert!(!federation_orchestrator_enabled());
        unsafe {
            std::env::remove_var("AXON_ENABLE_FEDERATION_ORCHESTRATOR");
        }
    }

    #[test]
    fn test_scope_reconciliation_enabled_defaults_to_true() {
        let _lock = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_SCOPE_RECONCILE_ENABLED");
        }
        assert!(scope_reconciliation_enabled());
    }

    #[test]
    fn test_scope_reconciliation_enabled_respects_false_env() {
        let _lock = ENV_TEST_GUARD.lock().unwrap();
        for value in ["false", "FALSE", "0", "off", "no"] {
            unsafe {
                std::env::set_var("AXON_SCOPE_RECONCILE_ENABLED", value);
            }
            assert!(
                !scope_reconciliation_enabled(),
                "value `{value}` should disable reconciliation"
            );
        }
        unsafe {
            std::env::remove_var("AXON_SCOPE_RECONCILE_ENABLED");
        }
    }

    #[test]
    fn test_scope_reconciliation_interval_defaults_and_clamps() {
        let _lock = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_SCOPE_RECONCILE_INTERVAL_SECS");
        }
        assert_eq!(scope_reconciliation_interval_secs(), 60);

        unsafe {
            std::env::set_var("AXON_SCOPE_RECONCILE_INTERVAL_SECS", "300");
        }
        assert_eq!(scope_reconciliation_interval_secs(), 300);

        // Values below the 5s floor fall back to the default.
        unsafe {
            std::env::set_var("AXON_SCOPE_RECONCILE_INTERVAL_SECS", "1");
        }
        assert_eq!(scope_reconciliation_interval_secs(), 60);

        unsafe {
            std::env::set_var("AXON_SCOPE_RECONCILE_INTERVAL_SECS", "not-a-number");
        }
        assert_eq!(scope_reconciliation_interval_secs(), 60);

        unsafe {
            std::env::remove_var("AXON_SCOPE_RECONCILE_INTERVAL_SECS");
        }
    }

    #[test]
    fn test_federation_orchestration_candidates_use_local_canonical_identities() {
        let candidates = federation_orchestration_candidates_from_identities(vec![
            axon_core::project_meta::CanonicalProjectIdentity {
                name: Some("System".to_string()),
                code: "PRO".to_string(),
                project_path: std::path::PathBuf::from("/tmp/pro"),
                meta_path: std::path::PathBuf::from("/tmp/pro/.axon/meta.json"),
            },
            axon_core::project_meta::CanonicalProjectIdentity {
                name: Some("Axon".to_string()),
                code: "AXO".to_string(),
                project_path: std::path::PathBuf::from("/tmp/axon"),
                meta_path: std::path::PathBuf::from("/tmp/axon/.axon/meta.json"),
            },
            axon_core::project_meta::CanonicalProjectIdentity {
                name: Some("BookingSystem".to_string()),
                code: "BKS".to_string(),
                project_path: std::path::PathBuf::from("/tmp/booking"),
                meta_path: std::path::PathBuf::from("/tmp/booking/.axon/meta.json"),
            },
        ]);

        assert_eq!(
            candidates,
            vec![
                ("AXO".to_string(), "/tmp/axon".to_string()),
                ("BKS".to_string(), "/tmp/booking".to_string()),
            ]
        );
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

    #[test]
    fn test_memory_reclaimer_can_run_when_only_stalled_subtree_hints_remain() {
        let ingress = test_ingress_buffer();
        {
            let mut locked = ingress.lock().unwrap_or_else(|poison| poison.into_inner());
            locked.record_subtree_hint(
                "/tmp/project/_build_truth_dashboard_ui".to_string(),
                900,
                IngressSource::Watcher,
            );
        }

        let metrics = ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .metrics_snapshot();
        let process_memory = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_bytes: 24 * 1024 * 1024 * 1024,
            rss_anon_bytes: 23 * 1024 * 1024 * 1024,
            rss_file_bytes: 64 * 1024 * 1024,
            rss_shmem_bytes: 0,
        };

        assert!(
            should_attempt_memory_reclaim(0, &metrics, process_memory, 4 * 1024 * 1024 * 1024),
            "Le reclaim memoire doit rester possible quand seuls des subtree hints stagnants bloquent l'idle parfait"
        );
    }

    #[test]
    fn test_claim_policy_is_fast_when_system_is_healthy() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
        assert!(policy.claim_count > 1_500);
        assert!(policy.sleep <= std::time::Duration::from_millis(200));
    }

    #[test]
    fn test_claim_policy_stays_fast_when_only_queue_grows() {
        // Queue length alone no longer triggers slow/guarded modes.
        // Only budget_exhaustion_ratio, RSS, and service_pressure drive mode changes.
        let policy = claim_policy(
            2_000,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
        assert!(policy.claim_count > 0);
    }

    #[test]
    fn test_claim_policy_reduces_work_progressively_before_mode_switch() {
        let lighter = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        let heavier = claim_policy(
            1_200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );

        assert_eq!(lighter.mode.label(), "fast");
        assert_eq!(heavier.mode.label(), "fast");
        assert!(
            heavier.claim_count < lighter.claim_count,
            "claim count should decrease progressively as pressure rises, even before switching modes"
        );
        assert!(
            heavier.sleep > lighter.sleep,
            "sleep should increase progressively as pressure rises"
        );
    }

    #[test]
    fn test_claim_policy_stays_fast_when_queue_is_high_but_budget_low() {
        // High queue alone does not trigger guarded mode anymore.
        let policy = claim_policy(
            3_500,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
        assert!(policy.claim_count > 0);
    }

    #[test]
    fn test_claim_policy_pauses_claiming_when_pressure_is_critical() {
        let policy = claim_policy(
            500,
            0.10,
            Some(95 * 1024 * 1024),
            100 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_claim_policy_does_not_pause_solely_on_large_queue() {
        // Even very large queues should not pause when budget/RSS/pressure are healthy.
        let policy = claim_policy(
            8_000,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
        assert!(policy.claim_count > 0);
    }

    #[test]
    fn test_claim_policy_enters_guarded_mode_when_service_is_degraded() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Degraded,
        );
        assert_eq!(policy.mode.label(), "guarded");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 1_000);
        assert!(policy.sleep > std::time::Duration::from_millis(400));
    }

    #[test]
    fn test_claim_policy_pauses_when_live_service_is_critical() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Critical,
        );
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_claim_policy_recovers_gradually_after_service_pressure() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Recovering,
        );
        assert_eq!(policy.mode.label(), "slow");
        assert!(policy.claim_count > 500);
        assert!(policy.claim_count < 1_500);
        assert!(policy.sleep > std::time::Duration::from_millis(250));
    }

    #[test]
    fn test_claim_policy_reports_fast_mode() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "fast");
    }

    #[test]
    fn test_claim_policy_reports_guarded_mode_on_budget() {
        // Guarded mode is now triggered by budget exhaustion, not queue length.
        let policy = claim_policy(
            500,
            0.90,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "guarded");
    }

    #[test]
    fn test_claim_policy_reports_paused_mode() {
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Critical,
        );
        assert_eq!(policy.mode.label(), "paused");
    }

    #[test]
    fn test_claim_policy_slows_when_memory_budget_is_warming_up() {
        let policy = claim_policy(
            200,
            0.75,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "slow");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 900);
    }

    #[test]
    fn test_claim_policy_guards_when_memory_budget_is_nearly_full() {
        let policy = claim_policy(
            200,
            0.90,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "guarded");
        assert!(policy.claim_count > 0);
        assert!(policy.claim_count < 250);
    }

    #[test]
    fn test_claim_policy_pauses_when_memory_budget_is_exhausted() {
        let policy = claim_policy(
            200,
            0.99,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        assert_eq!(policy.mode.label(), "paused");
        assert_eq!(policy.claim_count, 0);
    }

    #[test]
    fn test_claim_policy_enters_guarded_mode_during_interactive_priority() {
        axon_core::service_guard::reset_for_tests();
        axon_core::service_guard::mcp_request_started();
        let policy = claim_policy(
            200,
            0.10,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            ServicePressure::Healthy,
        );
        axon_core::service_guard::mcp_request_finished();
        assert!(matches!(
            policy.mode,
            ClaimMode::Guarded | ClaimMode::Paused
        ));
    }

    #[test]
    fn test_handle_watcher_events_stages_modified_file_as_hot_delta() {
        let _flush_guard = FLUSH_METRICS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let guard = test_file_ingress_guard();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Modify(ModifyKind::Data(
                    notify_debouncer_full::notify::event::DataChange::Any,
                )),
                paths: vec![file_path.clone()],
                attrs: Default::default(),
            },
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store.clone(),
            root.to_path_buf(),
            "proj".to_string(),
            guard.clone(),
            ingress_buffer.clone(),
            Some(project.clone()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );
        flush_ingress_buffer_once(
            store.clone(),
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"));
        assert!(row.contains("900"));
    }

    #[test]
    fn test_flush_ingress_buffer_records_durable_but_excluded_from_pending() {
        let _flush_guard = FLUSH_METRICS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset_ingress_metrics_for_tests();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("already-indexed.rs");
        std::fs::write(&file_path, "fn already_indexed() {}\n").unwrap();
        let metadata = std::fs::metadata(&file_path).unwrap();
        let size = metadata.len() as i64;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        store
            .execute(&format!(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES ('{}', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, {}, {}, 900)",
                file_path.to_string_lossy().replace('\'', "''"),
                size,
                mtime
            ))
            .unwrap();

        let ingress_buffer = test_ingress_buffer();
        {
            let mut locked = ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            locked.record_file(axon_core::ingress_buffer::IngressFileEvent::new(
                file_path.to_string_lossy().to_string(),
                "proj",
                size,
                mtime,
                900,
                IngressSource::Watcher,
                axon_core::ingress_buffer::IngressCause::Modified,
            ));
        }

        let guard = test_file_ingress_guard();
        flush_ingress_buffer_once(
            store,
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let metrics = ingress_metrics_snapshot();
        assert_eq!(metrics.flush_count, 1);
        assert_eq!(metrics.last_durably_persisted_count, 1);
        assert_eq!(metrics.last_excluded_from_pending_count, 1);
        assert_eq!(metrics.last_promoted_count, 0);
    }

    #[test]
    fn test_flush_ingress_buffer_does_not_count_already_pending_file_as_new_admission() {
        let _flush_guard = FLUSH_METRICS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset_ingress_metrics_for_tests();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("still-pending.rs");
        std::fs::write(&file_path, "fn still_pending() {}\n").unwrap();
        let metadata = std::fs::metadata(&file_path).unwrap();
        let size = metadata.len() as i64;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        store
            .execute(&format!(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES ('{}', 'proj', 'pending', 'promoted', FALSE, FALSE, {}, {}, 900)",
                file_path.to_string_lossy().replace('\'', "''"),
                size,
                mtime
            ))
            .unwrap();

        let ingress_buffer = test_ingress_buffer();
        {
            let mut locked = ingress_buffer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            locked.record_file(axon_core::ingress_buffer::IngressFileEvent::new(
                file_path.to_string_lossy().to_string(),
                "proj",
                size,
                mtime,
                900,
                IngressSource::Watcher,
                axon_core::ingress_buffer::IngressCause::Modified,
            ));
        }

        let guard = test_file_ingress_guard();
        flush_ingress_buffer_once(
            store,
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let metrics = ingress_metrics_snapshot();
        assert_eq!(metrics.flush_count, 1);
        assert_eq!(metrics.last_durably_persisted_count, 1);
        assert_eq!(metrics.last_excluded_from_pending_count, 0);
        assert_eq!(metrics.last_promoted_count, 0);
    }

    #[test]
    fn test_ingress_flush_batch_size_keeps_hot_priority_without_throttling_large_scan_backlog() {
        let mut metrics = IngressMetricsSnapshot {
            enabled: true,
            buffered_entries: 700,
            hot_entries: 2,
            scan_entries: 698,
            ..Default::default()
        };

        assert_eq!(
            ingress_flush_batch_size(&metrics, 32, 4),
            INGRESS_FORCE_BATCH_SIZE
        );

        metrics.buffered_entries = 320;
        metrics.scan_entries = 318;
        assert_eq!(
            ingress_flush_batch_size(&metrics, 320, 8),
            INGRESS_FORCE_BATCH_SIZE
        );

        metrics.scan_entries = 32;
        metrics.buffered_entries = 34;
        assert_eq!(
            ingress_flush_batch_size(&metrics, 320, 8),
            INGRESS_HOT_PRIORITY_BATCH_CAP
        );
    }

    #[test]
    fn test_admission_controller_prefers_bulk_fill_when_pending_stock_is_below_target_band() {
        let metrics = IngressMetricsSnapshot {
            enabled: true,
            buffered_entries: 900,
            hot_entries: 4,
            scan_entries: 896,
            ..Default::default()
        };

        let decision = admission_controller_decision(&metrics, 24, 2);
        assert_eq!(decision.blocking_authority, "none");
        assert!(decision.bulk_fill_preferred);
        assert_eq!(
            ingress_flush_batch_size(&metrics, 24, 2),
            INGRESS_FORCE_BATCH_SIZE
        );
    }

    #[test]
    fn test_ingress_promoter_backoff_only_scales_when_no_hot_entries_and_no_admission_progress() {
        let cold_metrics = IngressMetricsSnapshot {
            enabled: true,
            buffered_entries: 12,
            hot_entries: 0,
            scan_entries: 12,
            ..Default::default()
        };
        let hot_metrics = IngressMetricsSnapshot {
            enabled: true,
            buffered_entries: 12,
            hot_entries: 2,
            scan_entries: 10,
            ..Default::default()
        };

        let base = ingress_promoter_sleep_ms(&cold_metrics, Duration::from_millis(0));
        assert_eq!(
            ingress_promoter_backoff_ms(&cold_metrics, Duration::from_millis(0), 0),
            base
        );
        assert!(ingress_promoter_backoff_ms(&cold_metrics, Duration::from_millis(0), 2) > base);
        assert_eq!(
            ingress_promoter_backoff_ms(&hot_metrics, Duration::from_millis(0), 3),
            ingress_promoter_sleep_ms(&hot_metrics, Duration::from_millis(0))
        );

        let scan_backlog_metrics = IngressMetricsSnapshot {
            enabled: true,
            buffered_entries: INGRESS_MAX_BATCH_SIZE,
            hot_entries: 0,
            scan_entries: INGRESS_HOT_PRIORITY_BATCH_CAP,
            ..Default::default()
        };
        assert_eq!(
            ingress_promoter_backoff_ms(&scan_backlog_metrics, Duration::from_millis(0), 3),
            ingress_promoter_sleep_ms(&scan_backlog_metrics, Duration::from_millis(0))
        );
    }

    #[test]
    fn test_bootstrap_storm_still_salvages_active_project_delta() {
        let _flush_guard = FLUSH_METRICS_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let _watcher_guard = WATCHER_PROBE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let guard = test_file_ingress_guard();
        let mut events = Vec::new();
        for idx in 0..5_100 {
            let path = if idx == 0 {
                file_path.clone()
            } else {
                root.join(format!("cold-{idx}.tmp"))
            };
            events.push(DebouncedEvent::new(
                Event {
                    kind: EventKind::Modify(ModifyKind::Data(
                        notify_debouncer_full::notify::event::DataChange::Any,
                    )),
                    paths: vec![path],
                    attrs: Default::default(),
                },
                std::time::Instant::now(),
            ));
        }

        handle_watcher_events(
            store.clone(),
            root.to_path_buf(),
            "proj".to_string(),
            guard.clone(),
            ingress_buffer.clone(),
            Some(project.clone()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(events),
        );
        flush_ingress_buffer_once(
            store.clone(),
            root.to_string_lossy().as_ref(),
            &guard,
            &ingress_buffer,
        )
        .unwrap();

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"));
        assert!(row.contains("900"));

        let events = watcher_probe::recent();
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.storm_suppressed")));
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.storm_salvaged")));
    }

    #[test]
    fn test_bootstrap_salvage_paths_keeps_only_active_project_candidates() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let file_path = project.join("src").join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();
        let outside = root.join("other").join("cold.tmp");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, "x").unwrap();

        let salvaged = bootstrap_salvage_paths(
            &[file_path.clone(), outside.clone()],
            Some(project.as_path()),
        );

        assert_eq!(salvaged.len(), 1);
        assert_eq!(salvaged[0], std::fs::canonicalize(file_path).unwrap());
    }

    #[test]
    fn test_bootstrap_salvage_paths_ignores_directories() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let src_dir = project.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let file_path = src_dir.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let salvaged = bootstrap_salvage_paths(
            &[src_dir.clone(), file_path.clone()],
            Some(project.as_path()),
        );

        assert_eq!(salvaged.len(), 1);
        assert_eq!(salvaged[0], std::fs::canonicalize(file_path).unwrap());
    }

    #[test]
    fn test_handle_watcher_events_records_staged_none_reason_for_ineligible_delta() {
        let _watcher_guard = WATCHER_PROBE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("ignored.png");
        std::fs::write(&file_path, "not parsable").unwrap();

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Modify(ModifyKind::Data(
                    notify_debouncer_full::notify::event::DataChange::Any,
                )),
                paths: vec![file_path.clone()],
                attrs: Default::default(),
            },
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            "proj".to_string(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let events = watcher_probe::recent();
        assert!(events.iter().any(|line| line.contains("watcher.filtered")));
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.buffered_none")));
    }

    #[test]
    fn test_handle_watcher_events_records_rescan_request() {
        let _watcher_guard = WATCHER_PROBE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Other,
                paths: vec![file_path],
                attrs: Default::default(),
            }
            .set_flag(Flag::Rescan),
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            "proj".to_string(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let events = watcher_probe::recent();
            let requested = events
                .iter()
                .any(|line| line.contains("watcher.rescan_requested"));
            let completed = events
                .iter()
                .any(|line| line.contains("watcher.rescan_completed"));
            if requested && completed {
                break;
            }
            if Instant::now() >= deadline {
                panic!("watcher rescan checkpoints not observed in time");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn test_handle_watcher_events_records_rescan_skipped_when_guard_active() {
        let _watcher_guard = WATCHER_PROBE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        watcher_probe::clear();

        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap());
        let ingress_buffer = test_ingress_buffer();
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Other,
                paths: vec![file_path],
                attrs: Default::default(),
            }
            .set_flag(Flag::Rescan),
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store,
            root.to_path_buf(),
            "proj".to_string(),
            test_file_ingress_guard(),
            ingress_buffer,
            Some(project),
            Arc::new(AtomicBool::new(true)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Ok(vec![event]),
        );

        let events = watcher_probe::recent();
        assert!(events
            .iter()
            .any(|line| line.contains("watcher.rescan_skipped")));
    }

    #[test]
    fn test_handle_watcher_events_records_watcher_errors() {
        let _watcher_guard = WATCHER_PROBE_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        watcher_probe::clear();

        handle_watcher_events(
            Arc::new(GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap()),
            PathBuf::from("/tmp"),
            "proj".to_string(),
            test_file_ingress_guard(),
            test_ingress_buffer(),
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            Instant::now(),
            Err(vec![Error::generic("boom")]),
        );

        let events = watcher_probe::recent();
        assert!(events.iter().any(|line| line.contains("watcher.error")));
        assert!(events.iter().any(|line| line.contains("boom")));
    }

    #[test]
    fn test_enqueue_claimed_files_requeues_work_when_common_lane_is_full() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("bulk_overflow.ex");
        std::fs::write(&file_path, "defmodule BulkOverflow do\nend\n").unwrap();

        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                10,
                1,
            )])
            .unwrap();

        let claimed = store.fetch_pending_batch(10).unwrap();
        assert_eq!(claimed.len(), 1);

        let queue = QueueStore::new(3);
        for idx in 0..2 {
            let fill_bulk = temp.path().join(format!("fill-bulk-{}.ex", idx));
            std::fs::write(&fill_bulk, "defmodule FillBulk do\nend\n").unwrap();
            queue
                .push(
                    fill_bulk.to_string_lossy().as_ref(),
                    0,
                    &format!("fill-bulk-{}", idx),
                    0,
                    0,
                    false,
                )
                .unwrap();
        }

        enqueue_claimed_files(&store, &queue, claimed, &std::collections::HashMap::new());

        let row = store
            .query_json(&format!(
                "SELECT status, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("pending"));
        assert!(row.contains("null"));
    }

    #[test]
    fn test_plan_admissions_prefers_packable_candidates_over_single_blocking_large_file() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let medium = temp.path().join("medium.txt");
        let small = temp.path().join("small.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&medium, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small, vec![b'x'; 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = vec![
            PendingFile {
                path: large.to_string_lossy().to_string(),
                trace_id: "large".to_string(),
                priority: 100,
                size_bytes: 8 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: medium.to_string_lossy().to_string(),
                trace_id: "medium".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: small.to_string_lossy().to_string(),
                trace_id: "small".to_string(),
                priority: 100,
                size_bytes: 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
        ];

        let plan = plan_admissions(&queue, candidates, 3);
        assert_eq!(
            plan.selected.iter().map(|selection| selection.file.trace_id.as_str()).collect::<Vec<_>>(),
            vec!["small", "medium"],
            "the scheduler should admit the better-fitting small+medium pair instead of blocking on the large candidate"
        );
        assert!(plan.deferred.iter().any(|file| file.trace_id == "large"));
    }

    #[test]
    fn test_plan_admissions_marks_candidate_oversized_when_it_cannot_fit_even_alone() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 2 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(plan.selected.is_empty());
        assert_eq!(plan.oversized.len(), 1);
        assert_eq!(plan.oversized[0].trace_id, "oversized");
    }

    #[test]
    fn test_plan_admissions_prefers_structure_only_degradation_before_oversized_refusal() {
        let temp = tempdir().unwrap();
        let candidate = temp.path().join("candidate.rs");
        std::fs::write(&candidate, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: candidate.to_string_lossy().to_string(),
            trace_id: "candidate".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(
            plan.oversized.is_empty(),
            "a file that fits the degraded envelope should not be marked oversized"
        );
        assert_eq!(plan.selected.len(), 1);
        assert_eq!(plan.selected[0].file.trace_id, "candidate");
        assert_eq!(
            plan.selected[0].mode,
            axon_core::queue::ProcessingMode::StructureOnly
        );
    }

    #[test]
    fn test_plan_admissions_gives_probation_to_cold_oversized_candidate() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 2 * 1024 * 1024);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: 0,
            last_deferred_at_ms: None,
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert!(plan.selected.is_empty());
        assert!(plan.oversized.is_empty(), "a cold oversized candidate should first be deferred while the estimator is still conservative");
        assert_eq!(plan.deferred.len(), 1);
        assert_eq!(plan.deferred[0].trace_id, "oversized");
    }

    #[test]
    fn test_plan_admissions_uses_degraded_mode_before_final_oversized_refusal() {
        let temp = tempdir().unwrap();
        let oversized = temp.path().join("oversized.rs");
        std::fs::write(&oversized, vec![b'x'; 16 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 4_500_000);
        let candidates = vec![PendingFile {
            path: oversized.to_string_lossy().to_string(),
            trace_id: "oversized".to_string(),
            priority: 100,
            size_bytes: 16 * 1024,
            defer_count: OVERSIZED_PROBATION_DEFER_THRESHOLD,
            last_deferred_at_ms: Some(1),
        }];

        let plan = plan_admissions(&queue, candidates, 1);
        assert_eq!(
            plan.selected.len(),
            1,
            "the candidate should still be admitted through the degraded envelope"
        );
        assert_eq!(plan.selected[0].file.trace_id, "oversized");
        assert_eq!(
            plan.degraded.len(),
            1,
            "the degraded admission should be recorded explicitly"
        );
        assert_eq!(plan.degraded[0], oversized.to_string_lossy());
        assert!(
            plan.oversized.is_empty(),
            "degraded admission must win before definitive oversized refusal"
        );
    }

    #[test]
    fn test_plan_admissions_eventually_ages_deferred_large_candidate_into_selection() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let medium = temp.path().join("medium.txt");
        let small = temp.path().join("small.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&medium, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small, vec![b'x'; 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = |large_defer_count: u32| {
            vec![
                PendingFile {
                    path: large.to_string_lossy().to_string(),
                    trace_id: "large".to_string(),
                    priority: 100,
                    size_bytes: 8 * 1024,
                    defer_count: large_defer_count,
                    last_deferred_at_ms: (large_defer_count > 0).then_some(1),
                },
                PendingFile {
                    path: medium.to_string_lossy().to_string(),
                    trace_id: "medium".to_string(),
                    priority: 100,
                    size_bytes: 2 * 1024,
                    defer_count: 0,
                    last_deferred_at_ms: None,
                },
                PendingFile {
                    path: small.to_string_lossy().to_string(),
                    trace_id: "small".to_string(),
                    priority: 100,
                    size_bytes: 1024,
                    defer_count: 0,
                    last_deferred_at_ms: None,
                },
            ]
        };

        let plan = plan_admissions(&queue, candidates(0), 2);
        assert_eq!(
            plan.selected
                .iter()
                .map(|selection| selection.file.trace_id.as_str())
                .collect::<Vec<_>>(),
            vec!["small", "medium"],
            "before aging kicks in, the scheduler should keep picking the better-fitting pair"
        );
        assert!(plan.deferred.iter().any(|file| file.trace_id == "large"));

        let aged = plan_admissions(&queue, candidates(3), 2);
        assert_eq!(
            aged.selected
                .first()
                .map(|selection| selection.file.trace_id.as_str()),
            Some("large"),
            "after repeated deferrals, the large file should gain enough fairness to pass first"
        );
    }

    #[test]
    fn test_plan_admissions_promotes_repeatedly_deferred_large_file_before_smaller_new_work() {
        let temp = tempdir().unwrap();
        let large = temp.path().join("large.txt");
        let small_a = temp.path().join("small-a.txt");
        let small_b = temp.path().join("small-b.txt");
        std::fs::write(&large, vec![b'x'; 8 * 1024]).unwrap();
        std::fs::write(&small_a, vec![b'x'; 2 * 1024]).unwrap();
        std::fs::write(&small_b, vec![b'x'; 2 * 1024]).unwrap();

        let queue = QueueStore::with_memory_budget(100, 3_200_000);
        let candidates = vec![
            PendingFile {
                path: large.to_string_lossy().to_string(),
                trace_id: "large".to_string(),
                priority: 100,
                size_bytes: 8 * 1024,
                defer_count: 3,
                last_deferred_at_ms: Some(1),
            },
            PendingFile {
                path: small_a.to_string_lossy().to_string(),
                trace_id: "small-a".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
            PendingFile {
                path: small_b.to_string_lossy().to_string(),
                trace_id: "small-b".to_string(),
                priority: 100,
                size_bytes: 2 * 1024,
                defer_count: 0,
                last_deferred_at_ms: None,
            },
        ];

        let plan = plan_admissions(&queue, candidates, 2);
        assert!(
            plan.selected.iter().any(|selection| selection.file.trace_id == "large"),
            "a repeatedly deferred large file should eventually be promoted ahead of newer packable work"
        );
    }

    #[test]
    fn test_enqueue_claimed_files_marks_oversized_when_file_cannot_fit_alone() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("oversized.rs");
        std::fs::write(&file_path, vec![b'x'; 16 * 1024]).unwrap();

        let store = GraphStore::new_indexer_ist_writer_without_soll(":memory:").unwrap();
        store
            .bulk_insert_files(&[(
                file_path.to_string_lossy().to_string(),
                "proj".to_string(),
                16 * 1024,
                1,
            )])
            .unwrap();

        let claimed = store.fetch_pending_batch(10).unwrap();
        let queue = QueueStore::with_memory_budget(10, 2 * 1024 * 1024);

        enqueue_claimed_files(&store, &queue, claimed, &std::collections::HashMap::new());

        let row = store
            .query_json(&format!(
                "SELECT status, last_error_reason, worker_id FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("oversized"));
        assert!(row.contains("current budget"));
        assert!(row.contains("null"));
    }

    #[test]
    fn test_rescan_guard_reset_releases_guard_on_drop() {
        let guard = Arc::new(AtomicBool::new(true));
        {
            let _reset = RescanGuardReset::new(guard.clone());
            assert!(guard.load(Ordering::SeqCst));
        }
        assert!(!guard.load(Ordering::SeqCst));
    }

    #[test]
    fn test_active_project_hot_targets_expand_visible_child_subtrees() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();

        let targets = active_project_hot_targets(Some(root));
        let rendered: Vec<(String, bool)> = targets
            .into_iter()
            .map(|target| (target.path.to_string_lossy().to_string(), target.recursive))
            .collect();

        assert_eq!(rendered[0].0, root.to_string_lossy());
        assert!(!rendered[0].1);
        assert!(rendered
            .iter()
            .any(|(path, recursive)| path.ends_with("/src") && *recursive));
        assert!(rendered
            .iter()
            .any(|(path, recursive)| path.ends_with("/docs") && *recursive));
        assert!(!rendered.iter().any(|(path, _)| path.ends_with("/.git")));
    }

    #[test]
    fn test_bootstrap_event_storm_is_suppressed_early() {
        let started = Instant::now();
        assert!(should_suppress_bootstrap_event_storm(6_000, started, None));
    }

    #[test]
    fn test_bootstrap_event_storm_is_not_suppressed_late_or_small() {
        let started = Instant::now() - Duration::from_secs(180);
        assert!(!should_suppress_bootstrap_event_storm(6_000, started, None));
        assert!(!should_suppress_bootstrap_event_storm(
            100,
            Instant::now(),
            None
        ));
    }

    #[test]
    fn test_bootstrap_event_storm_is_suppressed_right_after_cold_arm() {
        let started = Instant::now() - Duration::from_secs(180);
        let cold_arm_completed_at = Some(Instant::now());
        assert!(should_suppress_bootstrap_event_storm(
            2_000,
            started,
            cold_arm_completed_at,
        ));
    }

    #[test]
    fn test_bootstrap_event_storm_is_not_suppressed_long_after_cold_arm() {
        let started = Instant::now() - Duration::from_secs(180);
        let cold_arm_completed_at = Some(Instant::now() - Duration::from_secs(45));
        assert!(!should_suppress_bootstrap_event_storm(
            2_000,
            started,
            cold_arm_completed_at,
        ));
    }

    #[test]
    fn filter_orchestration_candidates_by_watch_root_scopes_to_subtree() {
        // REQ-AXO-172 — restore AXON_WATCH_DIR isolation. Pre-fix, the
        // federation orchestrator discovered every project via
        // project_meta::candidate_directories (cwd / cwd-parent /
        // repo-root / repo-root-parent walking) and spawned a watcher +
        // initial scan for each — making minimal-corpus testing
        // (DEC-AXO-066 stepwise validation) impossible.
        let candidates = vec![
            ("AXO".to_string(), "/home/dstadel/projects/axon".to_string()),
            ("FOO".to_string(), "/home/dstadel/projects/foo".to_string()),
            ("TEST".to_string(), "/tmp/axon-test/sample".to_string()),
        ];

        // watch_root scoped to /tmp ⇒ only TEST kept
        let kept = filter_orchestration_candidates_by_watch_root(
            candidates.clone(),
            "/tmp/axon-test",
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, "TEST");

        // watch_root = parent ⇒ both AXO and FOO kept (TEST out)
        let kept = filter_orchestration_candidates_by_watch_root(
            candidates.clone(),
            "/home/dstadel/projects",
        );
        assert_eq!(kept.len(), 2);
        let codes: Vec<&str> = kept.iter().map(|(c, _)| c.as_str()).collect();
        assert!(codes.contains(&"AXO"));
        assert!(codes.contains(&"FOO"));

        // watch_root = exact project path ⇒ only that one kept
        let kept = filter_orchestration_candidates_by_watch_root(
            candidates.clone(),
            "/home/dstadel/projects/axon",
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].0, "AXO");

        // Empty watch_root ⇒ identity (legacy "scan everything" preserved
        // for callers that have not opted in)
        let kept = filter_orchestration_candidates_by_watch_root(candidates.clone(), "");
        assert_eq!(kept.len(), 3);

        // Whitespace-only watch_root ⇒ also treated as identity
        let kept = filter_orchestration_candidates_by_watch_root(candidates.clone(), "   ");
        assert_eq!(kept.len(), 3);

        // watch_root with no matching projects ⇒ empty result (the
        // path that breaks DEC-AXO-066 if not honoured)
        let kept = filter_orchestration_candidates_by_watch_root(candidates, "/var/empty");
        assert!(kept.is_empty());
    }
}
