use crate::embedder::{
    configured_embedding_profile_stack, embedding_profile_for_key,
    embedding_runtime_contract_for_profile, EmbeddingProfileKey, RuntimeEmbeddingModel,
};

#[test]
fn test_default_embedding_profile_stack_prefers_jina_with_bge_base_fallback() {
    let stack = configured_embedding_profile_stack();

    assert_eq!(stack.primary.key, EmbeddingProfileKey::JinaCodeV2Base);
    assert_eq!(
        stack.fallback.as_ref().map(|profile| profile.key),
        Some(EmbeddingProfileKey::BgeBaseEnv15)
    );
    assert_eq!(stack.primary.model_name, "jinaai/jina-embeddings-v2-base-code");
    assert_eq!(stack.primary.dimension, 768);
    assert_eq!(stack.primary.symbol.model_id, "sym-jina-embeddings-v2-base-code-768");
    assert_eq!(stack.primary.chunk.model_id, "chunk-jina-embeddings-v2-base-code-768");
    assert_eq!(stack.primary.graph.model_id, "graph-jina-embeddings-v2-base-code-768");
    assert_eq!(
        stack.fallback.as_ref().unwrap().model_name,
        "BAAI/bge-base-en-v1.5"
    );
    assert_eq!(stack.fallback.as_ref().unwrap().dimension, 768);
}

#[test]
fn test_embedding_profile_catalog_exposes_jina_and_bge_base_runtime_models() {
    let jina = embedding_profile_for_key(EmbeddingProfileKey::JinaCodeV2Base);
    let bge = embedding_profile_for_key(EmbeddingProfileKey::BgeBaseEnv15);

    assert_eq!(jina.runtime_model, RuntimeEmbeddingModel::JinaEmbeddingsV2BaseCode);
    assert_eq!(bge.runtime_model, RuntimeEmbeddingModel::BGEBaseENV15);
    assert_eq!(jina.dimension, 768);
    assert_eq!(bge.dimension, 768);
}

#[test]
fn test_embedding_runtime_contract_can_be_derived_from_explicit_profile() {
    let contract = embedding_runtime_contract_for_profile(&embedding_profile_for_key(
        EmbeddingProfileKey::JinaCodeV2Base,
    ));

    assert_eq!(contract.model_name, "jinaai/jina-embeddings-v2-base-code");
    assert_eq!(contract.dimension, 768);
    assert_eq!(contract.symbol_model_id, "sym-jina-embeddings-v2-base-code-768");
    assert_eq!(contract.chunk_model_id, "chunk-jina-embeddings-v2-base-code-768");
    assert_eq!(contract.graph_model_id, "graph-jina-embeddings-v2-base-code-768");
}
