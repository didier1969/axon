use std::sync::Arc;

use axon_core::graph::GraphStore;
use axon_core::mcp::McpServer;
use axon_core::queue::QueueStore;
use crossbeam_channel::Sender;
use tracing::{error, info};

pub(crate) fn start_runtime_services(
    graph_store: Arc<GraphStore>,
    queue_store: Arc<QueueStore>,
    results_tx: tokio::sync::broadcast::Sender<String>,
    num_workers: usize,
) -> Sender<axon_core::worker::DbWriteTask> {
    let (db_tx, db_rx) = crossbeam_channel::unbounded();

    axon_core::worker::WorkerPool::spawn_writer_actor(
        graph_store.clone(),
        db_rx,
        results_tx.clone(),
    );

    let queue_for_pool = queue_store.clone();
    let store_for_pool = graph_store.clone();
    let results_tx_for_pool = results_tx.clone();
    let db_tx_for_pool = db_tx.clone();

    tokio::task::spawn_blocking(move || {
        axon_core::worker::WorkerPool::new(
            num_workers,
            queue_for_pool,
            store_for_pool,
            db_tx_for_pool,
            results_tx_for_pool,
        );
    });

    let semantic_store = graph_store.clone();
    tokio::task::spawn_blocking(move || {
        axon_core::embedder::SemanticWorkerPool::new(semantic_store);
    });

    let mcp_store_for_axum = graph_store;
    tokio::spawn(async move {
        let mcp_server = Arc::new(McpServer::new(mcp_store_for_axum));
        let app = axon_core::mcp_http::app_router(mcp_server);
        match tokio::net::TcpListener::bind("0.0.0.0:44129").await {
            Ok(listener) => {
                info!("✅ SQL Gateway/MCP: Listening on 0.0.0.0:44129");
                let _ = axum::serve(listener, app).await;
            }
            Err(e) => error!("❌ SQL Gateway Failure: Could not bind to port 44129: {:?}", e),
        }
    });

    db_tx
}
