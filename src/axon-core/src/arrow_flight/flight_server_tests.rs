// REQ-AXO-902679 — TDD Tests for Native Arrow Flight SQL Streaming Server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arrow_array::cast::AsArray;
use arrow_array::types::Int32Type;
use tempfile::tempdir;
use tokio::sync::watch;

use crate::arrow_flight::flight_server::{
    FlightClient, FlightCommand, FlightDataProvider, FlightServer, FlightServerConfig,
};
use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType};

struct MockDataProvider {
    nodes: Vec<NodeRecord>,
    edges: Vec<EdgeTriple>,
}

impl FlightDataProvider for MockDataProvider {
    fn get_nodes(&self, _project: Option<&str>) -> Vec<NodeRecord> {
        self.nodes.clone()
    }

    fn get_edges(&self, _project: Option<&str>) -> Vec<EdgeTriple> {
        self.edges.clone()
    }
}

fn sample_data() -> (Vec<NodeRecord>, Vec<EdgeTriple>) {
    let nodes = (0..250)
        .map(|i| NodeRecord {
            id: format!("AXO::server::sym_{:04}", i),
            name: format!("sym_{:04}", i),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
            complexity: Some((i % 15) as i32 + 1),
        })
        .collect();

    let edges = (0..120)
        .map(|i| EdgeTriple {
            source: format!("AXO::server::sym_{:04}", i),
            target: format!("AXO::server::sym_{:04}", i + 1),
            rel: RelationType::Calls,
        })
        .collect();

    (nodes, edges)
}

#[tokio::test]
async fn test_flight_server_unix_socket_streaming() {
    let dir = tempdir().expect("tempdir");
    let socket_path = dir.path().join("axon_flight_test.sock");

    let (nodes, edges) = sample_data();
    let provider = Arc::new(MockDataProvider {
        nodes: nodes.clone(),
        edges,
    });

    let config = FlightServerConfig {
        unix_socket_path: Some(socket_path.clone()),
        tcp_bind_addr: None,
        default_batch_size: 50,
    };

    let server = FlightServer::new(config, provider);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.expect("server run failed");
    });

    // Allow socket creation
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect and query symbols via Unix socket
    let cmd = FlightCommand::Symbols {
        project: None,
        batch_size: Some(50),
    };
    let batches = FlightClient::query_unix_stream(&socket_path, &cmd)
        .await
        .expect("unix query failed");

    assert!(!batches.is_empty(), "batches must not be empty");
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 250);

    // Verify Arrow schema and contents
    let first_batch = &batches[0];
    assert_eq!(first_batch.num_columns(), 5);
    let id_col = first_batch.column(0).as_string::<i32>();
    assert_eq!(id_col.value(0), "AXO::server::sym_0000");

    let comp_col = first_batch.column(4).as_primitive::<Int32Type>();
    assert_eq!(comp_col.value(0), 1);

    // Shutdown server
    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_flight_server_tcp_socket_streaming() {
    let (nodes, edges) = sample_data();
    let provider = Arc::new(MockDataProvider {
        nodes,
        edges: edges.clone(),
    });

    // Bind to loopback with OS-assigned port
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let config = FlightServerConfig {
        unix_socket_path: None,
        tcp_bind_addr: Some(bind_addr),
        default_batch_size: 40,
    };

    let server = FlightServer::new(config, provider);
    let actual_addr = server.tcp_local_addr().expect("tcp listener bound");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let server_handle = tokio::spawn(async move {
        server.run(shutdown_rx).await.expect("server run failed");
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect and query edges via TCP
    let cmd = FlightCommand::Edges {
        project: None,
        batch_size: Some(40),
    };
    let batches = FlightClient::query_tcp_stream(actual_addr, &cmd)
        .await
        .expect("tcp query failed");

    assert!(!batches.is_empty(), "edges batches must not be empty");
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 120);

    // Verify Arrow schema
    let first_batch = &batches[0];
    assert_eq!(first_batch.num_columns(), 3);
    let src_col = first_batch.column(0).as_string::<i32>();
    assert_eq!(src_col.value(0), "AXO::server::sym_0000");

    // Ping check
    let ping_res = FlightClient::ping_tcp(actual_addr).await.expect("ping tcp");
    assert_eq!(ping_res, "pong");

    // Shutdown
    let _ = shutdown_tx.send(true);
    let _ = server_handle.await;
}
