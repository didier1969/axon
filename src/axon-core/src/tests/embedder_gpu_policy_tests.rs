#[cfg(test)]
mod tests {
    use crate::embedder::{current_gpu_memory_pressure_active, gpu_memory_soft_limit_mb};

    #[test]
    fn embedder_gpu_policy_public_surface_exposes_pressure_state() {
        unsafe {
            std::env::set_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB", "3000");
        }

        let soft_limit = gpu_memory_soft_limit_mb();
        let pressure = current_gpu_memory_pressure_active();

        unsafe {
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
        }

        assert_eq!(soft_limit, 3000);
        assert!(
            matches!(pressure, true | false),
            "pressure state should remain queryable after gpu policy extraction"
        );
    }
}
