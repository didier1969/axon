use crate::embedder::{
    calibrated_embedding_profile_for_backend, configured_embedding_profile_stack,
    default_embedding_profile, default_runtime_embedding_model, embedding_runtime_contract,
    EmbeddingExecutionBackend, EmbeddingProfile, EmbeddingProfileKey, RuntimeEmbeddingModel,
};

#[test]
fn test_embedding_profile_exposes_canonical_model_contract() {
    let profile = default_embedding_profile();

    assert_eq!(profile.key, EmbeddingProfileKey::JinaCodeV2Base);
    assert_eq!(profile.model_name, "jinaai/jina-embeddings-v2-base-code");
    assert_eq!(profile.model_version, "1");
    assert_eq!(profile.dimension, 768);
    assert_eq!(profile.symbol.kind, "symbol");
    assert_eq!(profile.symbol.model_id, "sym-jina-embeddings-v2-base-code-768");
    assert_eq!(profile.chunk.kind, "chunk");
    assert_eq!(profile.chunk.model_id, "chunk-jina-embeddings-v2-base-code-768");
    assert_eq!(profile.graph.kind, "graph");
    assert_eq!(profile.graph.model_id, "graph-jina-embeddings-v2-base-code-768");
    assert_eq!(profile.symbol.batch_size, 32);
    assert_eq!(profile.chunk.batch_size, 16);
    assert_eq!(profile.file_vectorization_batch_size, 8);
    assert_eq!(profile.graph.batch_size, 6);
}

#[test]
fn test_embedding_runtime_contract_is_derived_from_embedding_profile() {
    let profile = default_embedding_profile();
    let contract = embedding_runtime_contract();

    assert_eq!(contract.model_name, profile.model_name);
    assert_eq!(contract.symbol_model_id, profile.symbol.model_id);
    assert_eq!(contract.chunk_model_id, profile.chunk.model_id);
    assert_eq!(contract.graph_model_id, profile.graph.model_id);
    assert_eq!(contract.dimension, profile.dimension);
    assert_eq!(contract.symbol_batch_size, profile.symbol.batch_size);
    assert_eq!(contract.chunk_batch_size, profile.chunk.batch_size);
    assert_eq!(
        contract.file_vectorization_batch_size,
        profile.file_vectorization_batch_size
    );
    assert_eq!(contract.graph_batch_size, profile.graph.batch_size);
}

#[test]
fn test_default_runtime_embedding_model_is_derived_from_profile() {
    let profile = default_embedding_profile();

    assert_eq!(
        profile.runtime_model,
        RuntimeEmbeddingModel::JinaEmbeddingsV2BaseCode
    );
    assert_eq!(
        default_runtime_embedding_model(),
        RuntimeEmbeddingModel::JinaEmbeddingsV2BaseCode
    );
}

#[test]
fn test_embedding_profile_can_be_constructed_from_custom_canonical_values() {
    let profile = EmbeddingProfile::new(
        EmbeddingProfileKey::BgeBaseEnv15,
        "test/model",
        "test-model",
        "7",
        768,
        RuntimeEmbeddingModel::BGEBaseENV15,
        EmbeddingExecutionBackend::Cpu,
        24,
        48,
        12,
        10,
    );

    assert_eq!(profile.model_name, "test/model");
    assert_eq!(profile.model_version, "7");
    assert_eq!(profile.dimension, 768);
    assert_eq!(profile.execution_provider, Some("cpu"));
    assert_eq!(profile.symbol.model_id, "sym-test-model-768");
    assert_eq!(profile.chunk.model_id, "chunk-test-model-768");
    assert_eq!(profile.graph.model_id, "graph-test-model-768");
    assert_eq!(profile.symbol.batch_size, 48);
    assert_eq!(profile.chunk.batch_size, 24);
    assert_eq!(profile.graph.batch_size, 10);
    assert_eq!(profile.file_vectorization_batch_size, 12);
}

#[test]
fn test_default_embedding_profile_stack_prefers_jina_with_bge_fallback() {
    let stack = configured_embedding_profile_stack();

    assert_eq!(stack.primary.key, EmbeddingProfileKey::JinaCodeV2Base);
    assert_eq!(
        stack.fallback.as_ref().map(|profile| profile.key),
        Some(EmbeddingProfileKey::BgeBaseEnv15)
    );
}

#[test]
fn test_embedding_runtime_contract_applies_explicit_batch_overrides() {
    std::env::set_var("AXON_EMBEDDING_CHUNK_BATCH_SIZE", "20");
    std::env::set_var("AXON_EMBEDDING_SYMBOL_BATCH_SIZE", "40");
    std::env::set_var("AXON_EMBEDDING_FILE_VECTORIZATION_BATCH_SIZE", "10");
    std::env::set_var("AXON_EMBEDDING_GRAPH_BATCH_SIZE", "7");

    let contract = embedding_runtime_contract();

    std::env::remove_var("AXON_EMBEDDING_CHUNK_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_SYMBOL_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_FILE_VECTORIZATION_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_GRAPH_BATCH_SIZE");

    assert_eq!(contract.chunk_batch_size, 20);
    assert_eq!(contract.symbol_batch_size, 40);
    assert_eq!(contract.file_vectorization_batch_size, 10);
    assert_eq!(contract.graph_batch_size, 7);
}

#[test]
fn test_explicit_batch_overrides_win_over_gpu_floor() {
    std::env::set_var("AXON_EMBEDDING_CHUNK_BATCH_SIZE", "20");
    std::env::set_var("AXON_EMBEDDING_SYMBOL_BATCH_SIZE", "40");
    std::env::set_var("AXON_EMBEDDING_FILE_VECTORIZATION_BATCH_SIZE", "10");
    std::env::set_var("AXON_EMBEDDING_GRAPH_BATCH_SIZE", "7");

    let calibrated = calibrated_embedding_profile_for_backend(
        &default_embedding_profile(),
        EmbeddingExecutionBackend::GpuCuda,
    );

    std::env::remove_var("AXON_EMBEDDING_CHUNK_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_SYMBOL_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_FILE_VECTORIZATION_BATCH_SIZE");
    std::env::remove_var("AXON_EMBEDDING_GRAPH_BATCH_SIZE");

    assert_eq!(calibrated.chunk.batch_size, 20);
    assert_eq!(calibrated.symbol.batch_size, 40);
    assert_eq!(calibrated.file_vectorization_batch_size, 10);
    assert_eq!(calibrated.graph.batch_size, 7);
}
