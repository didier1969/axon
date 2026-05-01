// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{
        current_embedding_provider_diagnostics, embedding_provider_diagnostics,
        provider_resolution_for_label, ProductionLane, ProviderStrategy, ProviderSupportRole,
    };

    #[test]
    fn embedder_public_provider_diagnostics_surface_reflects_runtime_env() {
        let _guard = crate::tests::test_helpers::embedder_env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cuda_service");
            std::env::set_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR", "none");
            std::env::set_var("ORT_STRATEGY", "system");
            std::env::set_var("ORT_DYLIB_PATH", "/tmp/libonnxruntime.so");
            std::env::set_var("AXON_GPU_EMBED_SERVICE_ENABLED", "1");
        }

        let diagnostics = embedding_provider_diagnostics("cuda_service".to_string());
        assert_eq!(diagnostics.provider_effective, "cuda_service");
        assert_eq!(diagnostics.ort_strategy, "system");
        assert_eq!(
            diagnostics.ort_dylib_path.as_deref(),
            Some("/tmp/libonnxruntime.so")
        );
        assert!(diagnostics.gpu_service_enabled);
        assert_eq!(
            diagnostics.resolution.support_role,
            Some(ProviderSupportRole::VectorGpuService)
        );
        assert_eq!(
            diagnostics.resolution.effective_strategy,
            ProviderStrategy::Cuda
        );

        let current = current_embedding_provider_diagnostics();
        assert!(!current.provider_requested.is_empty());

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
            std::env::remove_var("ORT_STRATEGY");
            std::env::remove_var("ORT_DYLIB_PATH");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_ENABLED");
        }
    }

    #[test]
    fn provider_resolution_keeps_tensorrt_as_vector_support_strategy_not_lane() {
        let resolution = provider_resolution_for_label(
            "cuda",
            "tensorrt_service",
            true,
            true,
            Some("/tmp/libonnxruntime.so".to_string()),
            None,
        );

        assert_eq!(resolution.production_lane, None);
        assert_eq!(
            resolution.support_role,
            Some(ProviderSupportRole::VectorGpuService)
        );
        assert_eq!(resolution.effective_strategy, ProviderStrategy::TensorRt);
        assert_eq!(resolution.effective_label, "tensorrt_service");
        assert_eq!(
            resolution.provider_libraries,
            vec!["/tmp/libonnxruntime.so".to_string()]
        );
    }

    #[test]
    fn provider_resolution_defaults_in_process_embeddings_to_vector_lane() {
        let resolution = provider_resolution_for_label("cuda", "cuda", false, true, None, None);

        assert_eq!(resolution.production_lane, Some(ProductionLane::Vector));
        assert_eq!(resolution.support_role, None);
        assert_eq!(resolution.effective_strategy, ProviderStrategy::Cuda);
    }
}
