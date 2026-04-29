use crate::service_guard;

use super::{
    canonical_embedding_provider_request_for_mode, current_gpu_memory_snapshot,
    gpu_memory_soft_limit_mb, gpu_total_vram_hint_mb, AxonRuntimeMode, GpuMemorySnapshot,
    RuntimeProfile,
};

pub(super) fn gpu_memory_pressure_active(snapshot: GpuMemorySnapshot) -> bool {
    snapshot.used_mb >= gpu_memory_soft_limit_mb()
}

fn gpu_multiworker_min_free_mb() -> u64 {
    std::env::var("AXON_GPU_MULTIWORKER_MIN_FREE_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 128)
        .unwrap_or(768)
}

pub(super) fn gpu_secondary_worker_allowed(
    worker_idx: usize,
    snapshot: Option<GpuMemorySnapshot>,
) -> bool {
    if worker_idx == 0 {
        return true;
    }
    snapshot
        .map(|snapshot| snapshot.free_mb >= gpu_multiworker_min_free_mb())
        .unwrap_or(true)
}

pub(super) fn gpu_primary_worker_max_used_mb() -> u64 {
    let soft_limit = gpu_memory_soft_limit_mb();
    std::env::var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 512)
        .unwrap_or_else(|| soft_limit.saturating_sub((soft_limit / 10).max(512)))
        .min(soft_limit)
}

fn gpu_primary_batch_guard_enabled() -> bool {
    std::env::var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(super) fn gpu_primary_batch_allowed(snapshot: Option<GpuMemorySnapshot>) -> bool {
    if !gpu_primary_batch_guard_enabled() {
        return true;
    }
    snapshot
        .map(|snapshot| snapshot.used_mb < gpu_primary_worker_max_used_mb())
        .unwrap_or(true)
}

pub(super) fn gpu_worker_has_pending_work(
    ready_depth: usize,
    inflight_persists: usize,
    prepare_inflight: u64,
    claimable_backlog_depth: usize,
) -> bool {
    ready_depth > 0 || inflight_persists > 0 || prepare_inflight > 0 || claimable_backlog_depth > 0
}

pub(super) fn gpu_worker_should_wait_for_ready(
    ready_depth: usize,
    inflight_persists: usize,
    prepare_inflight: u64,
    claimable_backlog_depth: usize,
) -> bool {
    ready_depth == 0
        && inflight_persists == 0
        && (claimable_backlog_depth > 0 || prepare_inflight > 0)
}

pub(super) fn gpu_worker_consumption_allowed(
    gpu_available: bool,
    snapshot: Option<GpuMemorySnapshot>,
) -> bool {
    !gpu_available || gpu_primary_batch_allowed(snapshot)
}

pub(super) fn gpu_recreate_session_every_batch_enabled() -> bool {
    std::env::var("AXON_GPU_RECREATE_SESSION_EVERY_BATCH")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn gpu_stuck_recovery_enabled() -> bool {
    std::env::var("AXON_GPU_STUCK_RECOVERY_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn gpu_stuck_recovery_idle_gap_ms() -> u64 {
    std::env::var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 250)
        .unwrap_or(2_500)
}

fn gpu_stuck_recovery_ready_age_ms() -> u64 {
    std::env::var("AXON_GPU_STUCK_RECOVERY_READY_AGE_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 500)
        .unwrap_or(5_000)
}

pub(super) fn gpu_stuck_recovery_reason(
    metrics: service_guard::VectorRuntimeMetrics,
    inflight_persists: usize,
) -> Option<String> {
    if !gpu_stuck_recovery_enabled() {
        return None;
    }
    let idle_gap_ms = gpu_stuck_recovery_idle_gap_ms();
    let ready_age_ms = gpu_stuck_recovery_ready_age_ms();
    let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;

    if metrics.embed_inflight_started_at_ms > 0
        && now_ms.saturating_sub(metrics.embed_inflight_started_at_ms) >= idle_gap_ms
    {
        return Some(format!(
            "embed_inflight_stuck gap={} inflight_started_at_ms={}",
            now_ms.saturating_sub(metrics.embed_inflight_started_at_ms),
            metrics.embed_inflight_started_at_ms
        ));
    }

    if inflight_persists == 0
        && metrics.persist_queue_depth_current == 0
        && metrics.ready_queue_chunks_current > 0
        && metrics.oldest_ready_batch_age_ms_current >= ready_age_ms
        && metrics.last_embed_gap_ms >= idle_gap_ms
    {
        return Some(format!(
            "ready_stock_stalled ready_age_ms={} last_embed_gap_ms={} ready_chunks={}",
            metrics.oldest_ready_batch_age_ms_current,
            metrics.last_embed_gap_ms,
            metrics.ready_queue_chunks_current
        ));
    }

    None
}

fn gpu_recycle_on_vram_summit_enabled() -> bool {
    std::env::var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn gpu_recycle_immediate_on_vram_summit_enabled() -> bool {
    std::env::var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn configured_gpu_recycle_vram_summit_pct() -> Option<u64> {
    std::env::var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| (50..=95).contains(value))
}

pub(super) fn gpu_recycle_vram_summit_mb() -> u64 {
    std::env::var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 512)
        .or_else(|| {
            configured_gpu_recycle_vram_summit_pct().and_then(|pct| {
                current_gpu_memory_snapshot()
                    .map(|snapshot| snapshot.total_mb)
                    .or_else(gpu_total_vram_hint_mb)
                    .map(|total_mb| {
                        let derived = ((total_mb as f64) * (pct as f64 / 100.0)).round() as u64;
                        derived.clamp(4_096, total_mb.saturating_sub(256).max(4_096))
                    })
            })
        })
        .unwrap_or_else(gpu_memory_soft_limit_mb)
}

fn gpu_recycle_min_chunks_per_second() -> f64 {
    std::env::var("AXON_GPU_RECYCLE_MIN_CHUNKS_PER_SECOND")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(8.0)
}

fn gpu_recycle_required_batches() -> u32 {
    std::env::var("AXON_GPU_RECYCLE_REQUIRED_BATCHES")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value >= 1)
        .unwrap_or(2)
}

pub(super) fn gpu_recycle_after_vram_summit_observe(
    vram_used_mb: u64,
    chunks_per_second: f64,
    consecutive_batches: &mut u32,
) -> bool {
    if !gpu_recycle_on_vram_summit_enabled() {
        *consecutive_batches = 0;
        return false;
    }
    let should_count = vram_used_mb >= gpu_recycle_vram_summit_mb()
        && chunks_per_second <= gpu_recycle_min_chunks_per_second();
    if should_count {
        *consecutive_batches = consecutive_batches.saturating_add(1);
    } else {
        *consecutive_batches = 0;
    }
    *consecutive_batches >= gpu_recycle_required_batches()
}

pub(super) fn gpu_recycle_immediate_required(
    snapshot: Option<GpuMemorySnapshot>,
    inflight_persists: usize,
) -> bool {
    if !gpu_recycle_on_vram_summit_enabled()
        || !gpu_recycle_immediate_on_vram_summit_enabled()
        || inflight_persists > 0
    {
        return false;
    }
    snapshot
        .map(|snapshot| snapshot.used_mb >= gpu_recycle_vram_summit_mb())
        .unwrap_or(false)
}

pub fn current_gpu_memory_pressure_active() -> bool {
    current_gpu_memory_snapshot()
        .map(gpu_memory_pressure_active)
        .unwrap_or(false)
}

pub(super) fn embedding_provider_requested_is_gpu() -> bool {
    canonical_embedding_provider_request_for_mode(
        AxonRuntimeMode::from_env(),
        RuntimeProfile::detect().gpu_present,
    )
    .eq_ignore_ascii_case("cuda")
}
