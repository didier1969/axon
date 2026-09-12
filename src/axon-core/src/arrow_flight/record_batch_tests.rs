use arrow_array::cast::AsArray;
use arrow_array::types::{Float32Type, Int32Type};

use crate::arrow_flight::record_batch::{
    edges_to_record_batch, embeddings_to_record_batch, symbols_to_record_batch,
};
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

#[test]
fn test_symbols_to_record_batch_encoding() {
    let nodes = vec![
        NodeRecord {
            id: "AXO::engine::run".to_string(),
            name: "run".to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some(5),
        },
        NodeRecord {
            id: "AXO::engine::stop".to_string(),
            name: "stop".to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: None,
        },
    ];

    let batch = symbols_to_record_batch(&nodes).expect("encode symbols to record batch");
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 5);

    let id_col = batch.column(0).as_string::<i32>();
    assert_eq!(id_col.value(0), "AXO::engine::run");
    assert_eq!(id_col.value(1), "AXO::engine::stop");

    let name_col = batch.column(1).as_string::<i32>();
    assert_eq!(name_col.value(0), "run");
    assert_eq!(name_col.value(1), "stop");

    let complexity_col = batch.column(4).as_primitive::<Int32Type>();
    assert_eq!(complexity_col.value(0), 5);
    assert_eq!(complexity_col.value(1), -1); // Sentinel -1 for None
}

#[test]
fn test_edges_to_record_batch_encoding() {
    let edges = vec![
        EdgeTriple {
            source: "AXO::engine::run".to_string(),
            target: "AXO::engine::stop".to_string(),
            rel: RelationType::Calls,
        },
        EdgeTriple {
            source: "AXO::engine::run".to_string(),
            target: "AXO::db::query".to_string(),
            rel: RelationType::FlowsTo,
        },
    ];

    let batch = edges_to_record_batch(&edges).expect("encode edges to record batch");
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 3);

    let src_col = batch.column(0).as_string::<i32>();
    assert_eq!(src_col.value(0), "AXO::engine::run");

    let rel_col = batch.column(2).as_string::<i32>();
    assert_eq!(rel_col.value(0), "CALLS");
    assert_eq!(rel_col.value(1), "FLOWS_TO");
}

#[test]
fn test_embeddings_to_record_batch_encoding() {
    let chunk_ids = vec!["chk_001".to_string(), "chk_002".to_string()];
    let vectors = vec![
        vec![0.1f32, 0.2f32, 0.3f32, 0.4f32],
        vec![0.5f32, 0.6f32, 0.7f32, 0.8f32],
    ];

    let batch = embeddings_to_record_batch(&chunk_ids, &vectors, 4)
        .expect("encode embeddings to record batch");
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.num_columns(), 2);

    let id_col = batch.column(0).as_string::<i32>();
    assert_eq!(id_col.value(0), "chk_001");
    assert_eq!(id_col.value(1), "chk_002");

    let list_col = batch.column(1).as_fixed_size_list();
    assert_eq!(list_col.value_length(), 4);

    let values = list_col.values().as_primitive::<Float32Type>();
    assert_eq!(values.value(0), 0.1f32);
    assert_eq!(values.value(7), 0.8f32);
}
