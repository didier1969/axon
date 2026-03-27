use axum::{
    extract::Extension,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use tokio_stream::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use crate::mcp::{JsonRpcRequest, JsonRpcResponse, McpServer};

use tracing::Instrument;

pub fn app_router(mcp_server: Arc<McpServer>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp_post))
        .route("/mcp/sse", get(handle_mcp_sse))
        .layer(Extension(mcp_server))
}

async fn handle_mcp_post(
    Extension(server): Extension<Arc<McpServer>>,
    Json(payload): Json<JsonRpcRequest>,
) -> Json<Option<JsonRpcResponse>> {
    let span = tracing::info_span!("mcp_request", method = %payload.method);
    
    async move {
        // Offload C-FFI / DB work to a blocking thread pool safely
        // No more mcp_active_flag: Zero-Sleep MVCC architecture handles concurrency.
        let response = match tokio::task::spawn_blocking(move || {
            server.handle_request(payload)
        }).await {
            Ok(res) => res,
            Err(e) => {
                tracing::error!("MCP Blocking Task Panicked: {:?}", e);
                None
            }
        };
        
        Json(response)
    }.instrument(span).await
}

/// Compliant MCP SSE Endpoint
async fn handle_mcp_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Send the initial endpoint event as per MCP spec
    let endpoint_event = stream::once(async {
        Ok(Event::default().event("endpoint").data("/mcp"))
    });

    // 2. Keep-alive heartbeat every 15 seconds to prevent proxy timeouts
    let heartbeat = tokio_stream::wrappers::IntervalStream::new(
        tokio::time::interval(std::time::Duration::from_secs(15))
    ).map(|_| Ok(Event::default().comment("heartbeat")));

    let stream = endpoint_event.chain(heartbeat);
    Sse::new(stream)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt; 
    use serde_json::{json, Value};
    use crate::mcp_http::app_router;
    use crate::graph::GraphStore;
    use crate::mcp::McpServer;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_mcp_http_endpoint_tools_list() {
        // Updated test server creation to use direct Arc (Zéro-Sleep)
        let store = Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db_http").unwrap()));
        let mcp_server = Arc::new(McpServer::new(store));
        let app = app_router(mcp_server);

        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert!(body_json["result"]["tools"].as_array().unwrap().len() > 0);
    }
}
