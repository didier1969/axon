use axum::{
    extract::{Extension, State},
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use tokio_stream::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use crate::mcp::{JsonRpcRequest, JsonRpcResponse, McpServer};

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
    let response = server.handle_request(payload);
    Json(response)
}

async fn handle_mcp_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = mpsc::channel(100);
    // TODO: Setup proper SSE endpoint logic mapping to MCP SSE specification
    let stream = ReceiverStream::new(rx).map(|msg: String| Ok(Event::default().data(msg)));
    Sse::new(stream)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        Router,
    };
    use tower::ServiceExt; 
    use serde_json::{json, Value};
    use crate::mcp_http::app_router;
    use crate::graph::GraphStore;
    use crate::mcp::McpServer;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_mcp_http_endpoint_tools_list() {
        let store = Arc::new(std::sync::RwLock::new(GraphStore::new(":memory:").unwrap()));
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
