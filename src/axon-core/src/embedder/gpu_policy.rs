use crate::service_guard;

use super::{
    canonical_embedding_provider_request_for_mode, current_gpu_memory_snapshot,
    gpu_memory_soft_limit_mb, gpu_total_vram_hint_mb, AxonRuntimeMode, GpuMemorySnapshot,
    RuntimeProfile,
};

pub(super) fn gpu_memory_pressure_active(snapshot: GpuMemorySnapshot) -> bool {
    snapshot.used_mb >= gpu_memory_soft_limit_mb()
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
