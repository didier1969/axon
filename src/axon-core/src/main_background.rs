// Copyright (c) Didier Stadelmann. All rights reserved.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axon_core::embedder::{
    current_embedding_provider_diagnostics, current_gpu_memory_snapshot,
    current_gpu_utilization_snapshot, embedding_lane_config_from_env,
};
use axon_core::graph::GraphStore;
use axon_core::optimizer::collect_runtime_signals_window;
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
    memory_reclaimer_min_anon_bytes, vm_memory_floor_bytes,
};

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

/// REQ-AXO-902152 — ACTIVE memory watchdog (was purely observational: it set an
/// `above_limit` flag consumed NOWHERE → dead-code false-safety). It now:
///   1. classifies pressure from BOTH per-process RSS vs cap AND host-wide
///      MemAvailable vs floor (the real OOM driver is aggregate WSL-cap saturation,
///      which never trips a per-process cap — incident 2026-06-28);
///   2. publishes the level so the pipeline-A intake throttles itself (good co-tenant);
///   3. trims the allocator NOW under pressure (returns freed arenas to the OS),
///      instead of logging and doing nothing;
///   4. polls faster while under pressure so it reacts before a freeze.
/// Axon is well-behaved (~3.4 GB); this is host-safety/cohabitation (PIL-AXO-007),
/// not a fix for an Axon leak — it makes Axon recede when the SHARED VM is saturated.
pub(crate) fn start_memory_watchdog() {
    use axon_core::runtime_observability::{
        classify_memory_pressure, malloc_trim_system_allocator, mem_available_bytes,
        set_memory_pressure, MemoryPressure,
    };
    std::thread::spawn(move || {
        let limit_bytes = memory_limit_bytes();
        let floor_bytes = vm_memory_floor_bytes();
        let mut last_logged = MemoryPressure::Normal;
        loop {
            let rss_bytes = current_rss_bytes().unwrap_or(0);
            let available = mem_available_bytes();
            let level = classify_memory_pressure(rss_bytes, limit_bytes, available, floor_bytes);
            set_memory_pressure(level);

            if level != MemoryPressure::Normal {
                // Active mitigation: trim the system allocator immediately (cheap; returns idle
                // arenas to the OS so the aggregate VM recovers headroom). The pipeline-A intake
                // reads the published level and backs off in parallel.
                MEMORY_TRIM_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
                if malloc_trim_system_allocator() {
                    MEMORY_TRIM_SUCCESSES_TOTAL.fetch_add(1, Ordering::Relaxed);
                }
            }

            if level != last_logged {
                let avail_mb = available.map(|b| b / 1024 / 1024).unwrap_or(0);
                let rss_mb = rss_bytes / 1024 / 1024;
                match level {
                    MemoryPressure::Critical => error!(
                        "CRITICAL memory pressure (rss={rss_mb} MB, host_available={avail_mb} MB, floor={} MB): trimming + throttling A1 intake — Axon backing off as co-tenant [REQ-AXO-902152]",
                        floor_bytes / 1024 / 1024
                    ),
                    MemoryPressure::Elevated => warn!(
                        "Elevated memory pressure (rss={rss_mb} MB, host_available={avail_mb} MB): trimming allocator [REQ-AXO-902152]"
                    ),
                    MemoryPressure::Normal => info!(
                        "Memory pressure cleared (rss={rss_mb} MB, host_available={avail_mb} MB) [REQ-AXO-902152]"
                    ),
                }
                last_logged = level;
            }

            // React fast under pressure; idle-scale otherwise.
            let interval_ms = if level == MemoryPressure::Normal {
                quiescent_scaled_interval_ms(10_000, 10_000, 120_000)
            } else {
                2_000
            };
            std::thread::sleep(Duration::from_millis(interval_ms));
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
        let pressure = axon_core::runtime_observability::current_memory_pressure();

        if !should_attempt_memory_reclaim(
            queue.common_len(),
            process_memory,
            min_anon_bytes,
            pressure,
        ) {
            continue;
        }

        MEMORY_TRIM_ATTEMPTS_TOTAL.fetch_add(1, Ordering::Relaxed);
        if axon_core::runtime_observability::malloc_trim_system_allocator() {
            MEMORY_TRIM_SUCCESSES_TOTAL.fetch_add(1, Ordering::Relaxed);
            info!(
                "Memory reclaimer trimmed system allocator (rss_anon={} MiB, pressure={}).",
                process_memory.rss_anon_bytes / 1024 / 1024,
                pressure.as_str()
            );
        }
    });
}

/// REQ-AXO-901757 slice B2 — periodic brain sweep embedding SOLL nodes whose
/// description embedding is missing or stale. Brain owns the SOLL writer AND the
/// in-process query worker, so the sweep lives brain-side, off the request path.
/// Each pass embeds up to `batch` nodes: an empty pass sleeps the idle interval;
/// a productive pass loops promptly (brief yield) to drain a backlog — e.g. the
/// one-shot embedding of an existing SOLL corpus on first boot after this lands.
/// Best-effort: a worker-not-ready (boot race) or model-load error is logged and
/// retried next tick, never fatal. Default ON so SOLL semantic retrieval (B3b)
/// has vectors to fuse without any config.
pub(crate) fn spawn_soll_embedding_sweep(store: Arc<GraphStore>) {
    if !soll_embedding_sweep_enabled() {
        info!("SOLL embedding sweep disabled via AXON_SOLL_EMBED_SWEEP_ENABLED (REQ-AXO-901757 B2)");
        return;
    }
    let batch = soll_embedding_sweep_batch();
    let idle = Duration::from_millis(soll_embedding_sweep_idle_interval_ms());
    std::thread::spawn(move || {
        info!(
            "SOLL embedding sweep started (batch={batch}, idle={}ms, REQ-AXO-901757 B2)",
            idle.as_millis()
        );
        loop {
            match store.embed_pending_soll_nodes(batch) {
                Ok(0) => std::thread::sleep(idle),
                Ok(n) => {
                    info!("SOLL embedding sweep: embedded {n} node(s) this pass (REQ-AXO-901757 B2)");
                    // productive pass — yield briefly, then loop to drain backlog
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => {
                    // worker not yet up (boot race) or model load gap — retry next tick
                    warn!("SOLL embedding sweep deferred: {err:#}");
                    std::thread::sleep(idle);
                }
            }
        }
    });
}

pub(crate) fn spawn_runtime_trace_logger(store: Arc<GraphStore>, queue: Arc<QueueStore>) {
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

fn memory_reclaimer_poll_interval_ms() -> u64 {
    quiescent_scaled_interval_ms(
        MEMORY_RECLAIMER_POLL_INTERVAL_SECS.saturating_mul(1_000),
        5_000,
        120_000,
    )
}

/// REQ-AXO-901757 slice B2 — sweep config. Enabled by default; disable with
/// AXON_SOLL_EMBED_SWEEP_ENABLED in {0,false,no,off}.
fn soll_embedding_sweep_enabled() -> bool {
    !matches!(
        std::env::var("AXON_SOLL_EMBED_SWEEP_ENABLED")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0" | "false" | "no" | "off")
    )
}

fn soll_embedding_sweep_batch() -> usize {
    std::env::var("AXON_SOLL_EMBED_SWEEP_BATCH")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(64)
        .clamp(1, 512)
}

fn soll_embedding_sweep_idle_interval_ms() -> u64 {
    let base_ms = std::env::var("AXON_SOLL_EMBED_SWEEP_IDLE_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(60_000);
    quiescent_scaled_interval_ms(base_ms, 1_000, 600_000)
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

fn should_attempt_memory_reclaim(
    queue_len: usize,
    process_memory: axon_core::runtime_observability::ProcessMemorySnapshot,
    min_anon_bytes: u64,
    pressure: axon_core::runtime_observability::MemoryPressure,
) -> bool {
    use axon_core::runtime_observability::MemoryPressure;

    // REQ-AXO-902152 — under sustained aggregate VM pressure, trim even while the
    // queue is BUSY: mid-indexing is exactly when anon RSS climbs and the old
    // idle-only gate (REQ-AXO-901893) never fired. A lower floor applies so the
    // trim actually fires for Axon's modest footprint (it never reaches the 4 GB
    // idle floor). 256 MiB minimum keeps it from churning on a tiny process.
    if pressure != MemoryPressure::Normal {
        let pressure_floor = (min_anon_bytes / 8).max(256 * 1024 * 1024);
        return process_memory.rss_anon_bytes >= pressure_floor;
    }

    // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress_buffer backlog gate was
    // ripped with the buffer. Otherwise reclaim only when the work queue is idle
    // and the anonymous RSS is above the trim floor.
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
    // slice-5d. Canonical pipeline path writes Chunk + ChunkEmbedding directly.
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
    // diagnostics consume 0 until rewired against pipeline ready-queue
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
            };
        }
        InteractivePriority::InteractivePriority => {
            service_guard::record_background_launch_suppressed();
            return ClaimPolicy {
                mode: ClaimMode::Guarded,
                claim_count: 50,
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
        };
    }

    if service_pressure == ServicePressure::Degraded
        || rss_ratio >= 0.82
        || budget_exhaustion_ratio >= 0.88
    {
        return ClaimPolicy {
            mode: ClaimMode::Guarded,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Guarded),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
        };
    }

    if budget_exhaustion_ratio >= 0.72 {
        return ClaimPolicy {
            mode: ClaimMode::Slow,
            claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Slow),
        };
    }

    ClaimPolicy {
        mode: ClaimMode::Fast,
        claim_count: dynamic_claim_count(dynamic_pressure, ClaimMode::Fast),
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
mod tests {
    use super::{
        memory_limit_bytes, memory_reclaimer_enabled, memory_reclaimer_min_anon_bytes,
        should_attempt_memory_reclaim,
    };
    use crate::test_support::env_test_lock;

    #[test]
    fn test_memory_limit_uses_default_when_env_missing() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
        assert_eq!(memory_limit_bytes(), 14 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_memory_limit_uses_env_when_valid() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("AXON_MEMORY_LIMIT_GB", "10");
        }
        assert_eq!(memory_limit_bytes(), 10 * 1024 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_MEMORY_LIMIT_GB");
        }
    }

    #[test]
    fn test_memory_reclaimer_can_be_disabled_with_env() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
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
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
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
        use axon_core::runtime_observability::MemoryPressure;
        let mem_over = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_bytes: 24 * 1024 * 1024 * 1024,
            rss_anon_bytes: 23 * 1024 * 1024 * 1024,
            rss_file_bytes: 64 * 1024 * 1024,
            rss_shmem_bytes: 0,
        };
        let floor = 4 * 1024 * 1024 * 1024;
        // Normal pressure, idle queue + anon above floor → reclaim.
        assert!(should_attempt_memory_reclaim(
            0,
            mem_over,
            floor,
            MemoryPressure::Normal
        ));
        // Normal pressure, non-empty queue → never reclaim (work in flight).
        assert!(!should_attempt_memory_reclaim(
            7,
            mem_over,
            floor,
            MemoryPressure::Normal
        ));
        // Normal pressure, idle queue but anon below floor → no reclaim.
        let mem_under = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_anon_bytes: 1024 * 1024,
            ..mem_over
        };
        assert!(!should_attempt_memory_reclaim(
            0,
            mem_under,
            floor,
            MemoryPressure::Normal
        ));
    }

    /// REQ-AXO-902152 — under aggregate VM pressure the reclaimer trims even while
    /// the queue is BUSY, against a LOWER floor (mid-indexing is when anon climbs).
    #[test]
    fn test_memory_reclaim_fires_under_pressure_even_when_busy() {
        use axon_core::runtime_observability::MemoryPressure;
        let floor = 4 * 1024 * 1024 * 1024; // 4 GiB idle floor → pressure floor = 512 MiB
                                            // Axon-sized footprint: 2 GiB anon, queue BUSY.
        let mem = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_bytes: 2 * 1024 * 1024 * 1024,
            rss_anon_bytes: 2 * 1024 * 1024 * 1024,
            rss_file_bytes: 0,
            rss_shmem_bytes: 0,
        };
        // Normal pressure + busy + below 4 GiB idle floor → no trim (old behaviour).
        assert!(!should_attempt_memory_reclaim(
            500,
            mem,
            floor,
            MemoryPressure::Normal
        ));
        // Critical pressure + busy + above the 512 MiB pressure floor → trim.
        assert!(should_attempt_memory_reclaim(
            500,
            mem,
            floor,
            MemoryPressure::Critical
        ));
        // Elevated pressure with tiny anon (below 256 MiB min) → no trim.
        let tiny = axon_core::runtime_observability::ProcessMemorySnapshot {
            rss_anon_bytes: 100 * 1024 * 1024,
            ..mem
        };
        assert!(!should_attempt_memory_reclaim(
            500,
            tiny,
            floor,
            MemoryPressure::Elevated
        ));
    }
}
