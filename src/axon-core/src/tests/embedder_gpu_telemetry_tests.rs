// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{
        current_gpu_memory_snapshot, gpu_telemetry_backend_name, GpuMemorySnapshot,
    };

    #[test]
    fn embedder_gpu_telemetry_public_surface_exposes_backend_and_snapshot_shape() {
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_BACKEND", "disabled");
        }
        assert_eq!(gpu_telemetry_backend_name(), "none");
        assert_eq!(current_gpu_memory_snapshot(), None);

        let sample = GpuMemorySnapshot {
            total_mb: 8192,
            used_mb: 2048,
            free_mb: 6144,
        };
        assert_eq!(sample.total_mb - sample.used_mb, sample.free_mb);

        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
        }
    }
}
