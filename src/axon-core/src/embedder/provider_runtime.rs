use std::sync::atomic::{AtomicBool, Ordering};

use super::provider_contract::{
    requested_strategy_from_label, ProductionLane, ProviderResolution, ProviderStrategy,
    ProviderSupportRole,
};
use crate::runtime_mode::{canonical_embedding_provider_request_for_mode, AxonRuntimeMode};
use crate::runtime_capacity_profile::RuntimeProfile;

/// REQ-AXO-901737 : Single source of truth for embedder provider state.
/// `AXON_EMBEDDING_PROVIDER` env var is the ONLY operator-facing input (request).
/// All derived state (effective, init_error, gpu_present) lives in this struct,
/// protected by a process-wide Mutex. Replaces the env-var fan-out across
/// AXON_EMBEDDING_PROVIDER_EFFECTIVE / _INIT_ERROR / _GPU_PRESENT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProviderDiagnostics {
    pub provider_requested: String,
    pub provider_effective: String,
    pub ort_strategy: String,
    pub ort_dylib_path: Option<String>,
    pub gpu_service_enabled: bool,
    pub gpu_service_tensorrt_requested: bool,
    pub provider_init_error: Option<String>,
    pub gpu_present: bool,
    pub resolution: ProviderResolution,
}

// DEC-AXO-901626 — the raced `Mutex<EmbeddingProviderDiagnostics>` slot is
// GONE. Storing the effective provider was anti-SOTA: several embedder
// init lanes wrote it last-writer-wins (GpuB2Embedder → "tensorrt", the
// fastembed query lane → "cpu_fallback"), so the value lied most of the
// time. `provider_effective` is now DERIVED from an OS observation (cached
// nvidia-smi footprint, see `crate::observed_gpu`) — it cannot be raced.
//
// `gpu_present` remains a boot-time fact (single writer at startup via
// `set_gpu_present`), kept in a plain AtomicBool — not a provider slot.
static GPU_PRESENT: AtomicBool = AtomicBool::new(false);

/// Record GPU presence once at boot (runtime_boot). Single-writer fact,
/// not a raced provider label.
pub fn set_gpu_present(gpu_present: bool) {
    GPU_PRESENT.store(gpu_present, Ordering::Relaxed);
}

/// GPU presence as recorded at boot; falls back to a live probe for
/// ad-hoc callers that bypass the boot path (e.g. unit tests).
pub fn current_gpu_present() -> bool {
    GPU_PRESENT.load(Ordering::Relaxed) || RuntimeProfile::detect().gpu_present
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

/// DEC-AXO-901626 — derive the effective provider label from CONFIGURED
/// INTENT (requested GPU strategy ∧ GPU present), not from a raced slot.
/// This in-process label answers "should this process expect GPU work?" for
/// scheduling consumers (quiescence drain policy, optimizer) — it is cheap,
/// non-raced, and never shells out. The CANONICAL OBSERVED verdict (does the
/// embedder REALLY run on the GPU) lives in the indexer's PG heartbeat
/// (`EmbedderLifecycleHeartbeat.compute`, published from `observed_self_compute`)
/// and is surfaced to humans via `status.embedder_runtime` + the dashboard.
fn derive_effective_label(provider_requested: &str, gpu_present: bool) -> String {
    let requested = provider_requested.trim().to_ascii_lowercase();
    if gpu_present && (requested == "tensorrt" || requested == "cuda") {
        requested
    } else {
        "cpu".to_string()
    }
}

pub fn current_embedding_provider_diagnostics() -> EmbeddingProviderDiagnostics {
    let gpu_present = current_gpu_present();
    let provider_requested =
        canonical_embedding_provider_request_for_mode(AxonRuntimeMode::from_env(), gpu_present);
    embedding_provider_diagnostics_with_request(provider_requested, gpu_present)
}

/// Build the diagnostics struct from an explicit effective label. Pure: no
/// stored state. `provider_init_error` is no longer tracked here — init
/// failures surface via tracing logs + `axon.VectorWorkerFault`.
pub fn embedding_provider_diagnostics(provider_effective: String) -> EmbeddingProviderDiagnostics {
    let gpu_present = current_gpu_present();
    let provider_requested =
        canonical_embedding_provider_request_for_mode(AxonRuntimeMode::from_env(), gpu_present);
    let mut diagnostics =
        embedding_provider_diagnostics_with_request(provider_requested, gpu_present);
    diagnostics.provider_effective = provider_effective.clone();
    diagnostics.resolution = provider_resolution_for_label(
        &diagnostics.provider_requested,
        &provider_effective,
        diagnostics.gpu_service_enabled,
        diagnostics.gpu_service_tensorrt_requested,
        diagnostics.ort_dylib_path.clone(),
        None,
    );
    diagnostics
}

/// Shared pure builder: assemble the full diagnostics from a requested
/// label + gpu_present, deriving `provider_effective` observably.
fn embedding_provider_diagnostics_with_request(
    provider_requested: String,
    gpu_present: bool,
) -> EmbeddingProviderDiagnostics {
    let provider_effective = derive_effective_label(&provider_requested, gpu_present);
    let ort_dylib_path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());
    // DEC-AXO-070 commit F: subprocess GPU embed service removed; field
    // shape kept (false/false) for the JSON contract expected by callers.
    let gpu_service_enabled = false;
    let gpu_service_tensorrt_requested = false;
    let resolution = provider_resolution_for_label(
        &provider_requested,
        &provider_effective,
        gpu_service_enabled,
        gpu_service_tensorrt_requested,
        ort_dylib_path.clone(),
        None,
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
        provider_init_error: None,
        gpu_present,
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

/// REQ-AXO-184 #4 / REQ-AXO-185 #2: detect a silent fallback from a requested
/// GPU provider (cuda or tensorrt) to a non-matching effective provider so the
/// heartbeat can surface it as a `degraded_reason` within one tick instead of
/// after a full probe window. Returns `None` when the requested provider is
/// CPU (no fallback expected) or when requested == effective.
pub fn embedder_provider_fallback_reason(
    provider_requested: &str,
    provider_effective: &str,
    provider_init_error: Option<&str>,
) -> Option<String> {
    let requested = provider_requested.trim().to_ascii_lowercase();
    let effective = provider_effective.trim().to_ascii_lowercase();
    let gpu_requested = requested == "cuda" || requested == "tensorrt";
    if !gpu_requested {
        return None;
    }
    if effective == requested {
        return None;
    }
    let init_error = provider_init_error
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let detail = match init_error {
        Some(err) => format!(
            "embedder_provider_fallback: requested={} effective={} init_error={}",
            requested, effective, err
        ),
        None => format!(
            "embedder_provider_fallback: requested={} effective={}",
            requested, effective
        ),
    };
    Some(detail)
}

#[cfg(test)]
mod tests {
    use super::embedder_provider_fallback_reason;

    #[test]
    fn fallback_reason_cuda_to_cpu_with_init_error_includes_error_detail() {
        let reason = embedder_provider_fallback_reason("cuda", "cpu", Some("no_gpu_visible"));
        assert_eq!(
            reason.as_deref(),
            Some("embedder_provider_fallback: requested=cuda effective=cpu init_error=no_gpu_visible"),
        );
    }

    #[test]
    fn fallback_reason_tensorrt_to_cpu_without_init_error_omits_error_detail() {
        let reason = embedder_provider_fallback_reason("tensorrt", "cpu", None);
        assert_eq!(
            reason.as_deref(),
            Some("embedder_provider_fallback: requested=tensorrt effective=cpu"),
        );
    }

    #[test]
    fn fallback_reason_returns_none_when_requested_matches_effective() {
        assert!(embedder_provider_fallback_reason("tensorrt", "tensorrt", None).is_none());
        assert!(embedder_provider_fallback_reason("cuda", "cuda", Some("ignored")).is_none());
    }

    #[test]
    fn fallback_reason_returns_none_when_cpu_was_requested() {
        assert!(embedder_provider_fallback_reason("cpu", "cpu", None).is_none());
        assert!(
            embedder_provider_fallback_reason("cpu", "cpu_missing_cuda_provider", None).is_none()
        );
    }

    #[test]
    fn fallback_reason_treats_empty_init_error_as_absent() {
        let reason = embedder_provider_fallback_reason("cuda", "cpu", Some("   "));
        assert_eq!(
            reason.as_deref(),
            Some("embedder_provider_fallback: requested=cuda effective=cpu"),
        );
    }

    #[test]
    fn fallback_reason_is_case_insensitive_on_provider_labels() {
        let reason = embedder_provider_fallback_reason("CUDA", "CPU", None);
        assert_eq!(
            reason.as_deref(),
            Some("embedder_provider_fallback: requested=cuda effective=cpu"),
        );
    }
}
