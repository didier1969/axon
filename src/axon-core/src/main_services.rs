use std::sync::Arc;

use axon_core::graph::GraphStore;
use axon_core::mcp::McpServer;
use axon_core::queue::QueueStore;
use tracing::{error, info};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeServiceOptions {
    pub spawn_indexing_workers: bool,
    pub spawn_semantic_workers: bool,
}

impl RuntimeServiceOptions {
    pub(crate) fn brain_only() -> Self {
        Self {
            spawn_indexing_workers: false,
            spawn_semantic_workers: false,
        }
    }

    pub(crate) fn indexer_graph() -> Self {
        Self {
            spawn_indexing_workers: true,
            spawn_semantic_workers: false,
        }
    }

    pub(crate) fn indexer_vector() -> Self {
        Self {
            spawn_indexing_workers: false,
            spawn_semantic_workers: true,
        }
    }

    pub(crate) fn indexer_full() -> Self {
        Self {
            spawn_indexing_workers: true,
            spawn_semantic_workers: true,
        }
    }
}

// REQ-AXO-901653 slice-5c — v1 WorkerPool + writer-actor removed. The
// `spawn_indexing_workers` option is preserved for telemetry semantics
// but no longer spawns the legacy worker pool : pipeline_v2 (REQ-AXO-289
// / CPT-AXO-054) owns the ingestion path entirely via `spawn_pipeline_v2_indexer`.
pub(crate) fn start_runtime_services(
    graph_store: Arc<GraphStore>,
    queue_store: Arc<QueueStore>,
    _results_tx: tokio::sync::broadcast::Sender<String>,
    _num_workers: usize,
    options: RuntimeServiceOptions,
) {
    if options.spawn_indexing_workers {
        info!("Runtime services: indexing handled by pipeline_v2 (REQ-AXO-289).");
    } else {
        info!("Runtime services: indexing workers disabled by runtime mode.");
    }

    if options.spawn_semantic_workers {
        let semantic_store = graph_store.clone();
        let semantic_queue = queue_store.clone();
        tokio::task::spawn_blocking(move || {
            axon_core::embedder::SemanticWorkerPool::new(semantic_store, semantic_queue);
        });
    } else {
        info!("Runtime services: semantic workers disabled by runtime mode.");
    }

    let mcp_store_for_axum = graph_store;
    tokio::spawn(async move {
        let mcp_server = Arc::new(McpServer::new(mcp_store_for_axum));
        McpServer::startup_prewarm(mcp_server.clone());
        let app = axon_core::mcp_http::app_router(mcp_server);
        let http_port = std::env::var("AXON_BRAIN_PORT").unwrap_or_else(|_| "44129".to_string());
        let bind_addr = format!("0.0.0.0:{}", http_port);
        match tokio::net::TcpListener::bind(&bind_addr).await {
            Ok(listener) => {
                info!("✅ SQL Gateway/MCP: Listening on {}", bind_addr);
                let _ = axum::serve(listener, app).await;
            }
            Err(e) => error!(
                "❌ SQL Gateway Failure: Could not bind to port {}: {:?}",
                http_port, e
            ),
        }
    });
}
