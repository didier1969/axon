// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{
        clear_gpu_memory_snapshot_cache_for_tests, current_gpu_memory_snapshot,
        gpu_telemetry_backend_name, GpuMemorySnapshot,
    };
    use crate::test_support::{env_test_lock, EnvVarGuard};

    #[test]
    fn embedder_gpu_telemetry_public_surface_exposes_backend_and_snapshot_shape() {
        // REQ-AXO-099 Phase 2 — env_test_lock serializes against
        // every other env-mutating test in the crate; EnvVarGuard
        // restores the prior value on Drop (panic-safe). The cache
        // clear is required because a prior test in this run may
        // have populated the snapshot cache; the TTL-protected
        // cache returns a stale `Some(...)` even after we set the
        // backend to `disabled`.
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _backend_guard = EnvVarGuard::set("AXON_GPU_TELEMETRY_BACKEND", "disabled");
        clear_gpu_memory_snapshot_cache_for_tests();

        assert_eq!(gpu_telemetry_backend_name(), "none");
        assert_eq!(current_gpu_memory_snapshot(), None);

        let sample = GpuMemorySnapshot {
            total_mb: 8192,
            used_mb: 2048,
            free_mb: 6144,
        };
        assert_eq!(sample.total_mb - sample.used_mb, sample.free_mb);
    }
}
