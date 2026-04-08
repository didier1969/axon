use crate::embedder::{default_embedding_profile, embedding_runtime_contract};

#[test]
fn test_embedding_profile_exposes_canonical_model_contract() {
    let profile = default_embedding_profile();

    assert_eq!(profile.model_name, "BAAI/bge-small-en-v1.5");
    assert_eq!(profile.model_version, "1");
    assert_eq!(profile.dimension, 384);
    assert_eq!(profile.symbol.kind, "symbol");
    assert_eq!(profile.symbol.model_id, "sym-bge-small-en-v1.5-384");
    assert_eq!(profile.chunk.kind, "chunk");
    assert_eq!(profile.chunk.model_id, "chunk-bge-small-en-v1.5-384");
    assert_eq!(profile.graph.kind, "graph");
    assert_eq!(profile.graph.model_id, "graph-bge-small-en-v1.5-384");
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
