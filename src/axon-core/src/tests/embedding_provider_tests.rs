use crate::embedder::{
    default_embedding_execution_backend, embedding_execution_backend_name,
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
