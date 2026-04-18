use crate::mcp::{JsonRpcRequest, McpServer};
use crate::service_guard::{
    mcp_request_finished_with_class, mcp_request_started_with_class, record_latency,
    McpRequestClass, ServiceKind,
};
use axum::{
    extract::Extension,
    http::{
        header::{HeaderName, HeaderValue},
        HeaderMap, StatusCode,
    },
    response::sse::{Event, Sse},
    response::{IntoResponse, Response},
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
    headers: HeaderMap,
    Json(payload): Json<JsonRpcRequest>,
) -> Response {
    let span = tracing::info_span!("mcp_request", method = %payload.method);

    async move {
        let t0 = Instant::now();
        let request_class = classify_mcp_request(&payload);
        mcp_request_started_with_class(request_class);
        let protocol_version = resolve_response_protocol_version(&headers, &payload);

        let response = if payload.id.is_none() {
            match tokio::task::spawn_blocking(move || server.handle_notification(payload)).await {
                Ok(true) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    StatusCode::ACCEPTED.into_response()
                }
                Ok(false) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32601,
                                "message": "Unsupported notification"
                            }
                        })),
                    )
                        .into_response()
                }
                Err(e) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    tracing::error!("MCP Blocking Task Panicked: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        } else {
            // Offload C-FFI / DB work to a blocking thread pool safely
            // No more mcp_active_flag: Zero-Sleep MVCC architecture handles concurrency.
            match tokio::task::spawn_blocking(move || server.handle_request(payload)).await {
                Ok(Some(response)) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    Json(response).into_response()
                }
                Ok(None) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    StatusCode::BAD_REQUEST.into_response()
                }
                Err(e) => {
                    record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
                    tracing::error!("MCP Blocking Task Panicked: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        };
        mcp_request_finished_with_class(request_class);

        with_protocol_version_header(response, protocol_version)
    }
    .instrument(span)
    .await
}

fn classify_mcp_request(request: &JsonRpcRequest) -> McpRequestClass {
    match request.method.as_str() {
        "initialize" | "tools/list" | "notifications/initialized" => McpRequestClass::Observer,
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

fn resolve_response_protocol_version(
    headers: &HeaderMap,
    request: &JsonRpcRequest,
) -> Option<&'static str> {
    if request.method == "initialize" {
        return Some(McpServer::negotiate_protocol_version(request));
    }

    headers
        .get("MCP-Protocol-Version")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]
                .into_iter()
                .find(|supported| *supported == value)
        })
}

fn with_protocol_version_header(
    mut response: Response,
    protocol_version: Option<&'static str>,
) -> Response {
    if let Some(protocol_version) = protocol_version {
        let header_name = HeaderName::from_static("mcp-protocol-version");
        if let Ok(header_value) = HeaderValue::from_str(protocol_version) {
            response.headers_mut().insert(header_name, header_value);
        }
    }

    response
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
    use crate::mcp_http::{app_router, classify_mcp_request, resolve_response_protocol_version};
    use crate::service_guard;
    use crate::service_guard::{
        mcp_request_finished_with_class, mcp_request_started_with_class, McpRequestClass,
    };
    use axum::{
        body::Body,
        http::{HeaderMap, Request, StatusCode},
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

    #[tokio::test]
    async fn test_mcp_http_initialize_negotiates_protocol_version_and_sets_header() {
        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_http_initialize").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));
        let app = app_router(mcp_server);

        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "id": 1,
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "codex-test", "version": "0.0.0" }
            }
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("mcp-protocol-version")
                .and_then(|value| value.to_str().ok()),
            Some("2025-11-25")
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body_json["result"]["protocolVersion"], "2025-11-25");
    }

    #[tokio::test]
    async fn test_mcp_http_initialized_notification_returns_accepted_without_body() {
        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_http_initialized").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));
        let app = app_router(mcp_server);

        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.is_empty());
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
    fn test_classify_mcp_request_marks_initialized_notification_as_observer() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
            id: None,
        };

        assert!(matches!(
            classify_mcp_request(&req),
            McpRequestClass::Observer
        ));
    }

    #[test]
    fn test_resolve_response_protocol_version_uses_header_for_non_initialize_request() {
        let mut headers = HeaderMap::new();
        headers.insert("MCP-Protocol-Version", "2025-03-26".parse().unwrap());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "notifications/initialized".to_string(),
            params: None,
            id: None,
        };

        assert_eq!(
            resolve_response_protocol_version(&headers, &req),
            Some("2025-03-26")
        );
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
