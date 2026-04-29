use std::sync::{Mutex, OnceLock};

use super::provider_contract::{
    requested_strategy_from_label, ProductionLane, ProviderResolution, ProviderStrategy,
    ProviderSupportRole,
};
use crate::runtime_mode::{canonical_embedding_provider_request_for_mode, AxonRuntimeMode};
use crate::runtime_profile::RuntimeProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProviderDiagnostics {
    pub provider_requested: String,
    pub provider_effective: String,
    pub ort_strategy: String,
    pub ort_dylib_path: Option<String>,
    pub gpu_service_enabled: bool,
    pub gpu_service_tensorrt_requested: bool,
    pub provider_init_error: Option<String>,
    pub resolution: ProviderResolution,
}

static EMBEDDING_PROVIDER_DIAGNOSTICS: OnceLock<Mutex<EmbeddingProviderDiagnostics>> =
    OnceLock::new();

fn embedding_provider_slot() -> &'static Mutex<EmbeddingProviderDiagnostics> {
    EMBEDDING_PROVIDER_DIAGNOSTICS
        .get_or_init(|| Mutex::new(embedding_provider_diagnostics("unspecified".to_string())))
}

pub(crate) fn gpu_service_provider_effective_label() -> &'static str {
    if super::gpu_embed_service_prefers_tensorrt() {
        "tensorrt_service"
    } else {
        "cuda_service"
    }
}

pub(crate) fn current_embedding_provider_effective() -> String {
    std::env::var("AXON_EMBEDDING_PROVIDER_EFFECTIVE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "cpu".to_string())
}

pub(crate) fn set_embedding_provider_runtime_state(
    provider_effective: &str,
    init_error: Option<&str>,
) {
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", provider_effective);
        match init_error {
            Some(value) => std::env::set_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR", value),
            None => std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR"),
        }
    }
}

pub(crate) fn publish_embedding_provider_state(provider_effective: &str, init_error: Option<&str>) {
    set_embedding_provider_runtime_state(provider_effective, init_error);
    register_embedding_provider_diagnostics(embedding_provider_diagnostics(
        provider_effective.to_string(),
    ));
}

pub(crate) fn cpu_provider_effective_label(
    cuda_requested: bool,
    cuda_available: bool,
    cuda_provider_library_available: bool,
) -> &'static str {
    if cuda_requested && cuda_available && !cuda_provider_library_available {
        "cpu_missing_cuda_provider"
    } else {
        "cpu"
    }
}

pub(crate) fn register_embedding_provider_diagnostics(diagnostics: EmbeddingProviderDiagnostics) {
    let mut slot = embedding_provider_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *slot = diagnostics;
}

pub fn current_embedding_provider_diagnostics() -> EmbeddingProviderDiagnostics {
    let mut diagnostics = embedding_provider_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    diagnostics.provider_requested = canonical_embedding_provider_request_for_mode(
        AxonRuntimeMode::from_env(),
        RuntimeProfile::detect().gpu_present,
    );
    diagnostics.resolution.requested_strategy =
        requested_strategy_from_label(&diagnostics.provider_requested);
    diagnostics
}

pub fn embedding_provider_diagnostics(provider_effective: String) -> EmbeddingProviderDiagnostics {
    let runtime_mode = AxonRuntimeMode::from_env();
    let gpu_present = RuntimeProfile::detect().gpu_present;
    let provider_requested =
        canonical_embedding_provider_request_for_mode(runtime_mode, gpu_present);
    let ort_dylib_path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let gpu_service_enabled = super::gpu_embed_service_enabled();
    let gpu_service_tensorrt_requested = super::gpu_embed_service_prefers_tensorrt();
    let provider_init_error = std::env::var("AXON_EMBEDDING_PROVIDER_INIT_ERROR")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let resolution = provider_resolution_for_label(
        &provider_requested,
        &provider_effective,
        gpu_service_enabled,
        gpu_service_tensorrt_requested,
        ort_dylib_path.clone(),
        provider_init_error.clone(),
    );

    EmbeddingProviderDiagnostics {
        provider_requested,
        provider_effective,
        ort_strategy: std::env::var("ORT_STRATEGY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unspecified".to_string()),
        ort_dylib_path,
        gpu_service_enabled,
        gpu_service_tensorrt_requested,
        provider_init_error,
        resolution,
    }
}

pub(crate) fn provider_resolution_for_label(
    provider_requested: &str,
    provider_effective: &str,
    gpu_service_enabled: bool,
    gpu_service_tensorrt_requested: bool,
    ort_dylib_path: Option<String>,
    provider_init_error: Option<String>,
) -> ProviderResolution {
    let requested_strategy = requested_strategy_from_label(provider_requested);
    let reason = provider_init_error.clone();
    let mut resolution = if gpu_service_enabled {
        ProviderResolution::for_support_role(
            ProviderSupportRole::VectorGpuService,
            requested_strategy,
            provider_effective.to_string(),
            reason,
        )
    } else {
        ProviderResolution::for_production_lane(
            ProductionLane::Vector,
            requested_strategy,
            provider_effective.to_string(),
            reason,
        )
    };
    if gpu_service_enabled && gpu_service_tensorrt_requested {
        resolution.effective_strategy = ProviderStrategy::TensorRt;
    }
    if let Some(path) = ort_dylib_path {
        resolution.provider_libraries.push(path);
    }
    if provider_effective.contains("fallback") || provider_effective.contains("missing") {
        resolution.fallback_origin = Some(provider_effective.to_string());
    }
    resolution
}
