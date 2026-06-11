use super::{
    canonical_embedding_provider_request_for_mode, current_gpu_memory_snapshot,
    gpu_memory_soft_limit_mb, AxonRuntimeMode, GpuMemorySnapshot, RuntimeProfile,
};

pub(super) fn gpu_memory_pressure_active(snapshot: GpuMemorySnapshot) -> bool {
    snapshot.used_mb >= gpu_memory_soft_limit_mb()
}

pub fn current_gpu_memory_pressure_active() -> bool {
    current_gpu_memory_snapshot()
        .map(gpu_memory_pressure_active)
        .unwrap_or(false)
}

pub(super) fn embedding_provider_requested_is_gpu() -> bool {
    // REQ-AXO-901737: the canonical provider request only ever emits `cpu`
    // or `tensorrt` (legacy `cuda` is normalised to `tensorrt`). The GPU
    // request token is therefore `tensorrt`, matching derive_effective_label's
    // GPU set; we keep `cuda` for defence-in-depth even though it is normalised
    // away upstream.
    let canonical = canonical_embedding_provider_request_for_mode(
        AxonRuntimeMode::from_env(),
        RuntimeProfile::detect().gpu_present,
    );
    canonical.eq_ignore_ascii_case("tensorrt") || canonical.eq_ignore_ascii_case("cuda")
}
