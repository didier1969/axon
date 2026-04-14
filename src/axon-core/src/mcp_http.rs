use crate::mcp::{JsonRpcRequest, JsonRpcResponse, McpServer};
use crate::service_guard::{
    mcp_request_finished_with_class, mcp_request_started_with_class, record_latency,
    McpRequestClass, ServiceKind,
};
use axum::{
    extract::Extension,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::StreamExt;

use tracing::Instrument;

pub fn app_router(mcp_server: Arc<McpServer>) -> Router {
    Router::new()
        .route("/mcp", post(handle_mcp_post))
        .route("/mcp/sse", get(handle_mcp_sse))
        .route("/sql", post(handle_sql_post))
        .layer(Extension(mcp_server))
}

#[derive(serde::Deserialize)]
struct SqlRequest {
    query: String,
}

async fn handle_sql_post(
    Extension(server): Extension<Arc<McpServer>>,
    Json(payload): Json<SqlRequest>,
) -> Json<serde_json::Value> {
    let span = tracing::info_span!("sql_gateway", query = %payload.query);

    async move {
        let t0 = Instant::now();
        match tokio::task::spawn_blocking(move || server.execute_raw_sql(&payload.query)).await {
            Ok(Ok(res)) => {
                record_latency(ServiceKind::Sql, t0.elapsed().as_millis() as u64);
                Json(serde_json::from_str(&res).unwrap_or(serde_json::json!([])))
            }
            Ok(Err(e)) => {
                record_latency(ServiceKind::Sql, t0.elapsed().as_millis() as u64);
                Json(serde_json::json!({"error": format!("{:?}", e)}))
            }
            Err(e) => {
                record_latency(ServiceKind::Sql, t0.elapsed().as_millis() as u64);
                Json(serde_json::json!({"error": format!("Task Panic: {:?}", e)}))
            }
        }
    }
    .instrument(span)
    .await
}

async fn handle_mcp_post(
    Extension(server): Extension<Arc<McpServer>>,
    Json(payload): Json<JsonRpcRequest>,
) -> Json<Option<JsonRpcResponse>> {
    let span = tracing::info_span!("mcp_request", method = %payload.method);

    async move {
        let t0 = Instant::now();
        let request_class = classify_mcp_request(&payload);
        mcp_request_started_with_class(request_class);
        // Offload C-FFI / DB work to a blocking thread pool safely
        // No more mcp_active_flag: Zero-Sleep MVCC architecture handles concurrency.
        let response =
            match tokio::task::spawn_blocking(move || server.handle_request(payload)).await {
                Ok(res) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    res
                }
                Err(e) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    tracing::error!("MCP Blocking Task Panicked: {:?}", e);
                    None
                }
            };
        mcp_request_finished_with_class(request_class);

        Json(response)
    }
    .instrument(span)
    .await
}

fn classify_mcp_request(request: &JsonRpcRequest) -> McpRequestClass {
    match request.method.as_str() {
        "initialize" | "tools/list" => McpRequestClass::Observer,
        "tools/call" => {
            let tool_name = request
                .params
                .as_ref()
                .and_then(|params| params.get("name"))
                .and_then(|value| value.as_str())
                .map(|name| {
                    name.strip_prefix("mcp_axon_")
                        .or_else(|| name.strip_prefix("axon_"))
                        .unwrap_or(name)
                });
            if tool_name.is_some_and(is_observer_tool_name) {
                McpRequestClass::Observer
            } else {
                McpRequestClass::Control
            }
        }
        _ => McpRequestClass::Control,
    }
}

fn is_observer_tool_name(name: &str) -> bool {
    matches!(
        name,
        "status"
            | "project_status"
            | "snapshot_history"
            | "snapshot_diff"
            | "conception_view"
            | "change_safety"
            | "why"
            | "path"
            | "anomalies"
            | "job_status"
            | "debug"
            | "health"
            | "truth_check"
    )
}

/// Compliant MCP SSE Endpoint
async fn handle_mcp_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Send the initial endpoint event as per MCP spec
    let endpoint_event =
        stream::once(async { Ok(Event::default().event("endpoint").data("/mcp")) });

    // 2. Keep-alive heartbeat every 15 seconds to prevent proxy timeouts
    let heartbeat = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(15),
    ))
    .map(|_| Ok(Event::default().comment("heartbeat")));

    let stream = endpoint_event.chain(heartbeat);
    Sse::new(stream)
}

#[cfg(test)]
mod tests {
    use crate::graph::GraphStore;
    use crate::mcp::{JsonRpcRequest, McpServer};
    use crate::mcp_http::{app_router, classify_mcp_request};
    use crate::service_guard;
    use crate::service_guard::{
        mcp_request_finished_with_class, mcp_request_started_with_class, McpRequestClass,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_mcp_http_endpoint_tools_list() {
        // Updated test server creation to use direct Arc (Zéro-Sleep)
        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_http").unwrap()),
        );
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body_json["jsonrpc"], "2.0");
        assert!(!body_json["result"]["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_classify_mcp_request_marks_status_as_observer() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": {}
            })),
            id: Some(json!(1)),
        };

        assert!(matches!(
            classify_mcp_request(&req),
            McpRequestClass::Observer
        ));
    }

    #[test]
    fn test_classify_mcp_request_marks_health_and_truth_check_as_observer() {
        for tool_name in ["health", "truth_check"] {
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": tool_name,
                    "arguments": {}
                })),
                id: Some(json!(1)),
            };

            assert!(
                matches!(classify_mcp_request(&req), McpRequestClass::Observer),
                "tool {tool_name} should stay observer-classified"
            );
        }
    }

    #[test]
    fn test_observer_requests_do_not_increment_interactive_inflight() {
        service_guard::reset_for_tests();
        mcp_request_started_with_class(McpRequestClass::Observer);
        assert_eq!(service_guard::interactive_requests_in_flight(), 0);
        mcp_request_finished_with_class(McpRequestClass::Observer);
        assert_eq!(service_guard::interactive_requests_in_flight(), 0);
    }
}
