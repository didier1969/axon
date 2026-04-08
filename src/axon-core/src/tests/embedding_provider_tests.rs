use crate::embedder::{
    configured_embedding_execution_backend, default_embedding_execution_backend,
    embedding_execution_backend_name,
    embedding_execution_providers, EmbeddingExecutionBackend,
};

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
    std::env::set_var("AXON_EMBEDDING_BACKEND", "cuda");

    let backend = configured_embedding_execution_backend(false);

    std::env::remove_var("AXON_EMBEDDING_BACKEND");
    assert_eq!(backend, EmbeddingExecutionBackend::GpuCuda);
}

#[test]
fn test_embedding_backend_can_be_forced_to_cpu_by_env() {
    std::env::set_var("AXON_EMBEDDING_BACKEND", "cpu");

    let backend = configured_embedding_execution_backend(true);

    std::env::remove_var("AXON_EMBEDDING_BACKEND");
    assert_eq!(backend, EmbeddingExecutionBackend::Cpu);
}
