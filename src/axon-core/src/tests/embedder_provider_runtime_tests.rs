// Copyright (c) Didier Stadelmann. All rights reserved.

#[cfg(test)]
mod tests {
    use crate::embedder::{
        provider_resolution_for_label, ProductionLane, ProviderStrategy, ProviderSupportRole,
    };

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
