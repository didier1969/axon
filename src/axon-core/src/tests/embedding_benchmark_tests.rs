use crate::embedder::embedding_runtime_contract;

#[test]
fn test_embedding_runtime_contract_exposes_current_runtime_truth() {
    let contract = embedding_runtime_contract();

    assert_eq!(contract.model_name, "BAAI/bge-small-en-v1.5");
    assert_eq!(contract.symbol_model_id, "sym-bge-small-en-v1.5-384");
    assert_eq!(contract.chunk_model_id, "chunk-bge-small-en-v1.5-384");
    assert_eq!(contract.graph_model_id, "graph-bge-small-en-v1.5-384");
    assert_eq!(contract.dimension, 384);
    assert_eq!(contract.chunk_batch_size, 16);
    assert_eq!(contract.symbol_batch_size, 32);
    assert_eq!(contract.file_vectorization_batch_size, 8);
    assert_eq!(contract.graph_batch_size, 6);
    assert_eq!(contract.kinds, &["symbol", "chunk", "graph"]);
    assert_eq!(contract.execution_provider.as_deref(), None);
}
