use crate::embedder::{
    embedding_profile_benchmark_matrix, EmbeddingExecutionBackend, EmbeddingProfileKey,
};

#[test]
fn test_embedding_profile_benchmark_matrix_reports_three_profiles() {
    let rows = embedding_profile_benchmark_matrix();

    assert_eq!(rows.len(), 5, "proxy benchmark matrix should cover cpu/gpu comparison rows");
    assert_eq!(rows[0].profile_key, EmbeddingProfileKey::JinaCodeV2Base);
    assert_eq!(rows[1].profile_key, EmbeddingProfileKey::BgeBaseEnv15);
    assert_eq!(rows[2].profile_key, EmbeddingProfileKey::LegacyBgeSmallEnv15);
    assert_eq!(rows[3].profile_key, EmbeddingProfileKey::JinaCodeV2Base);
    assert_eq!(rows[4].profile_key, EmbeddingProfileKey::BgeBaseEnv15);
}

#[test]
fn test_embedding_profile_benchmark_matrix_exposes_proxy_mode_and_backend() {
    let rows = embedding_profile_benchmark_matrix();

    assert_eq!(rows[0].mode, "proxy");
    assert_eq!(rows[0].backend, EmbeddingExecutionBackend::Cpu);
    assert_eq!(rows[3].backend, EmbeddingExecutionBackend::GpuCuda);
    assert_eq!(rows[4].backend, EmbeddingExecutionBackend::GpuCuda);
}

#[test]
fn test_embedding_profile_benchmark_matrix_calibrates_gpu_rows() {
    let rows = embedding_profile_benchmark_matrix();
    let jina_cpu = rows
        .iter()
        .find(|row| {
            row.profile_key == EmbeddingProfileKey::JinaCodeV2Base
                && row.backend == EmbeddingExecutionBackend::Cpu
        })
        .unwrap();
    let jina_gpu = rows
        .iter()
        .find(|row| {
            row.profile_key == EmbeddingProfileKey::JinaCodeV2Base
                && row.backend == EmbeddingExecutionBackend::GpuCuda
        })
        .unwrap();

    assert!(jina_gpu.chunk_batch_size > jina_cpu.chunk_batch_size);
    assert!(jina_gpu.symbol_batch_size > jina_cpu.symbol_batch_size);
    assert!(jina_gpu.file_vectorization_batch_size > jina_cpu.file_vectorization_batch_size);
    assert!(jina_gpu.total_chunk_budget > jina_cpu.total_chunk_budget);
    assert_eq!(jina_gpu.dimension, 768);
}

#[test]
fn test_embedding_profile_benchmark_matrix_keeps_legacy_baseline_visible() {
    let rows = embedding_profile_benchmark_matrix();
    let legacy = rows
        .iter()
        .find(|row| row.profile_key == EmbeddingProfileKey::LegacyBgeSmallEnv15)
        .unwrap();

    assert_eq!(legacy.model_name, "BAAI/bge-small-en-v1.5");
    assert_eq!(legacy.dimension, 384);
    assert_eq!(legacy.backend, EmbeddingExecutionBackend::Cpu);
}
