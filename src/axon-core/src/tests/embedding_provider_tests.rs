use crate::embedder::{
    configured_embedding_execution_backend, default_embedding_execution_backend,
    embedding_execution_backend_name, resolve_embedding_provider_truth,
    resolve_embedding_provider_truth_with_probe, EmbeddingExecutionBackend,
    EmbeddingProviderStartupProbe, embedding_execution_providers,
};
use once_cell::sync::Lazy;
use std::sync::Mutex;

static EMBEDDING_BACKEND_ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[test]
fn test_embedding_backend_tracks_runtime_gpu_presence() {
    assert_eq!(
        default_embedding_execution_backend(true),
        EmbeddingExecutionBackend::GpuCuda
    );
    assert_eq!(
        default_embedding_execution_backend(false),
        EmbeddingExecutionBackend::Cpu
    );
}

#[test]
fn test_embedding_backend_names_are_explicit() {
    assert_eq!(
        embedding_execution_backend_name(EmbeddingExecutionBackend::GpuCuda),
        "cuda"
    );
    assert_eq!(
        embedding_execution_backend_name(EmbeddingExecutionBackend::Cpu),
        "cpu"
    );
}

#[test]
fn test_embedding_backend_builds_execution_provider_dispatches() {
    let cpu_providers = embedding_execution_providers(EmbeddingExecutionBackend::Cpu);
    let gpu_providers = embedding_execution_providers(EmbeddingExecutionBackend::GpuCuda);

    assert_eq!(cpu_providers.len(), 1);
    assert_eq!(gpu_providers.len(), 2);
}

#[test]
fn test_embedding_backend_can_be_forced_to_cuda_by_env() {
    let _guard = EMBEDDING_BACKEND_ENV_LOCK.lock().unwrap();
    std::env::set_var("AXON_EMBEDDING_BACKEND", "cuda");

    let backend = configured_embedding_execution_backend(false);

    std::env::remove_var("AXON_EMBEDDING_BACKEND");
    assert_eq!(backend, EmbeddingExecutionBackend::GpuCuda);
}

#[test]
fn test_embedding_backend_can_be_forced_to_cpu_by_env() {
    let _guard = EMBEDDING_BACKEND_ENV_LOCK.lock().unwrap();
    std::env::set_var("AXON_EMBEDDING_BACKEND", "cpu");

    let backend = configured_embedding_execution_backend(true);

    std::env::remove_var("AXON_EMBEDDING_BACKEND");
    assert_eq!(backend, EmbeddingExecutionBackend::Cpu);
}

#[test]
fn test_provider_truth_keeps_requested_backend_distinct_from_device_heuristic() {
    let truth = resolve_embedding_provider_truth(EmbeddingExecutionBackend::GpuCuda, false);

    assert_eq!(truth.requested_backend, "cuda");
    assert!(!truth.gpu_present);
    assert_eq!(truth.device_heuristic_backend, "cpu");
}

#[test]
fn test_provider_truth_does_not_claim_cuda_effective_from_request_alone() {
    let truth = resolve_embedding_provider_truth(EmbeddingExecutionBackend::GpuCuda, true);

    assert_eq!(truth.provider_effective, None);
    assert_eq!(truth.provider_status, "unverified");
    assert!(
        truth.provider_note.contains("requested"),
        "the note should explain that CUDA was requested, not proven"
    );
}

#[test]
fn test_provider_truth_verifies_cpu_when_cpu_only_requested() {
    let truth = resolve_embedding_provider_truth(EmbeddingExecutionBackend::Cpu, true);

    assert_eq!(truth.provider_effective, Some("cpu"));
    assert_eq!(truth.provider_status, "verified");
}

#[test]
fn test_provider_truth_verifies_cuda_when_registration_probe_succeeds() {
    let truth = resolve_embedding_provider_truth_with_probe(
        EmbeddingExecutionBackend::GpuCuda,
        false,
        Some(&EmbeddingProviderStartupProbe::registration_succeeded()),
    );

    assert_eq!(truth.provider_effective, Some("cuda"));
    assert_eq!(truth.provider_status, "verified");
    assert_eq!(truth.provider_provenance, "ort_registration_probe");
    assert_eq!(truth.provider_registration_outcome, Some("registered"));
}

#[test]
fn test_provider_truth_marks_cuda_fallback_when_registration_probe_fails() {
    let truth = resolve_embedding_provider_truth_with_probe(
        EmbeddingExecutionBackend::GpuCuda,
        true,
        Some(&EmbeddingProviderStartupProbe::registration_failed(
            "cuda ep registration failed".to_string(),
        )),
    );

    assert_eq!(truth.provider_effective, None);
    assert_eq!(truth.provider_status, "fallback");
    assert_eq!(truth.provider_provenance, "ort_registration_probe");
    assert_eq!(truth.provider_registration_outcome, Some("failed"));
    assert!(
        truth.provider_note.contains("failed"),
        "the note should surface the registration failure"
    );
}
