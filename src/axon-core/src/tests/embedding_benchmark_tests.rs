use crate::embedder::embedding_runtime_contract;

#[test]
fn test_embedding_runtime_contract_exposes_current_runtime_truth() {
    let contract = embedding_runtime_contract();

    assert_eq!(contract.model_name, "jinaai/jina-embeddings-v2-base-code");
    assert_eq!(contract.symbol_model_id, "sym-jina-embeddings-v2-base-code-768");
    assert_eq!(contract.chunk_model_id, "chunk-jina-embeddings-v2-base-code-768");
    assert_eq!(contract.graph_model_id, "graph-jina-embeddings-v2-base-code-768");
    assert_eq!(contract.dimension, 768);
    assert_eq!(contract.chunk_batch_size, 16);
    assert_eq!(contract.symbol_batch_size, 32);
    assert_eq!(contract.file_vectorization_batch_size, 8);
    assert_eq!(contract.graph_batch_size, 6);
    assert_eq!(contract.kinds, &["symbol", "chunk", "graph"]);
    assert_eq!(contract.execution_provider.as_deref(), None);
}
