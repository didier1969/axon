use futures_util::stream::StreamExt;

use crate::arrow_flight::flight_stream::FlightSqlStreamBridge;
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

fn generate_mock_nodes(count: usize) -> Vec<NodeRecord> {
    (0..count)
        .map(|i| NodeRecord {
            id: format!("AXO::stream::symbol_{:04}", i),
            name: format!("symbol_{:04}", i),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some((i % 10) as i32 + 1),
        })
        .collect()
}

fn generate_mock_edges(count: usize) -> Vec<EdgeTriple> {
    (0..count)
        .map(|i| EdgeTriple {
            source: format!("AXO::stream::src_{:04}", i),
            target: format!("AXO::stream::tgt_{:04}", i),
            rel: RelationType::Calls,
        })
        .collect()
}

#[tokio::test]
async fn test_flight_sql_batch_streaming_pipeline() {
    let nodes = generate_mock_nodes(500);
    let bridge = FlightSqlStreamBridge::new();

    let mut stream = bridge.stream_symbols_batches(nodes, 100);
    let mut total_rows = 0;
    let mut batch_count = 0;

    while let Some(batch_res) = stream.next().await {
        let batch = batch_res.expect("batch should decode cleanly");
        assert_eq!(batch.num_rows(), 100);
        assert_eq!(batch.num_columns(), 5);
        total_rows += batch.num_rows();
        batch_count += 1;
    }

    assert_eq!(batch_count, 5);
    assert_eq!(total_rows, 500);
}

#[tokio::test]
async fn test_flight_sql_edges_streaming_pipeline() {
    let edges = generate_mock_edges(250);
    let bridge = FlightSqlStreamBridge::new();

    let mut stream = bridge.stream_edges_batches(edges, 100);
    let mut batch_sizes = Vec::new();

    while let Some(batch_res) = stream.next().await {
        let batch = batch_res.expect("edge batch should decode cleanly");
        batch_sizes.push(batch.num_rows());
    }

    assert_eq!(batch_sizes, vec![100, 100, 50]);
}
