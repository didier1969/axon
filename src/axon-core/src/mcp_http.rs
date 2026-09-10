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
    // REQ-AXO-901735 — health probes uniformes (Sridharan + k8s style).
    // process-compose et tout client externe lit l'état via ces 3 endpoints
    // standards plutôt que par inspection ad-hoc des sockets/PID files.
    Router::new()
        .route("/mcp", post(handle_mcp_post))
        .route("/mcp/sse", get(handle_mcp_sse))
        .route("/sql", post(handle_sql_post))
        .route("/livez", get(handle_livez))
        .route("/readyz", get(handle_readyz))
        .route("/startupz", get(handle_startupz))
        // REQ-AXO-901806 — dashboard state v1. Read-only snapshot of the
        // event the 1 Hz telemetry loop pushes on the broadcast channel ;
        // served from the in-memory cache populated by main_telemetry.
        .route("/dashboard/state", get(handle_dashboard_state))
        // REQ-AXO-902392 — Prometheus metrics exporter endpoint. Aggregates brain,
        // indexer heartbeat, B2/B3 pressure, and chunk queues.
        .route("/metrics", get(handle_metrics))
        .layer(Extension(mcp_server))
}

// /livez — process vivant. Le simple fait que axum réponde prouve le
// liveness ; on retourne 200 tant qu'aucun deadlock interne ne bloque la
// tâche tokio. Réservé aux liveness probes (jamais 503 sauf hard freeze).
async fn handle_livez() -> Response {
    (StatusCode::OK, "ok").into_response()
}

// /readyz — deps OK + accepting traffic. Pour le brain : la DB doit
// répondre à un `SELECT 1`. On peut renvoyer 200+JSON {state:degraded,
// reasons:[...]} pour graceful degradation, mais V1 = strict 200/503.
//
// REQ-AXO-902563: run the DB probe on an isolated OS thread with timeout,
// NEVER routing through the global blocking thread pool where startup IST/SOLL
// snapshot warming can starve readiness probes.
async fn handle_readyz(Extension(server): Extension<Arc<McpServer>>) -> Response {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let probe_server = server.clone();
    let spawn_result = std::thread::Builder::new()
        .name("axon-readyz-probe".to_string())
        .spawn(move || {
            let res = probe_server.execute_raw_sql("SELECT 1");
            let _ = tx.send(res);
        });

    if let Err(err) = spawn_result {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "state": "degraded",
                "reasons": ["probe_spawn_failed"],
                "error": err.to_string(),
            })),
        )
            .into_response();
    }

    match tokio::time::timeout(std::time::Duration::from_secs(2), rx).await {
        Ok(Ok(Ok(_))) => (StatusCode::OK, Json(serde_json::json!({"state": "ready"}))).into_response(),
        Ok(Ok(Err(e))) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "state": "degraded",
                "reasons": ["db_probe_failed"],
                "error": format!("{:?}", e),
            })),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "state": "degraded",
                "reasons": ["db_probe_task_panic"],
                "error": "readiness probe thread dropped channel without response",
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "state": "degraded",
                "reasons": ["db_probe_timeout"],
                "error": "timed out waiting for database readiness probe",
            })),
        )
            .into_response(),
    }
}

// /startupz — one-shot init terminé. Pour le brain : si on répond à HTTP,
// l'init du runtime est forcément terminé (start_runtime_services est appelé
// avant axum::serve). V1 retourne toujours 200 ; raffinable plus tard pour
// distinguer "IST chargé / embedder warmed" via un AtomicBool partagé.
async fn handle_startupz() -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({"state": "started"})),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct SqlRequest {
    query: String,
}

// REQ-AXO-901806 — /dashboard/state handler. Reads the latest snapshot
// from the in-memory slot populated by `main_telemetry::spawn_runtime_telemetry`
// every 1 s. Cost is constant-time (Mutex lock + clone), no PG roundtrip.
// Returns 503 if the slot is empty (brain just booted, no tick yet).
async fn handle_dashboard_state() -> Response {
    match crate::dashboard_state::latest_dashboard_state() {
        Some(state) => (StatusCode::OK, Json(state)).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "dashboard_state_not_ready",
                "hint": "Telemetry loop has not yet completed a tick. Retry after 1s.",
            })),
        )
            .into_response(),
    }
}

// REQ-AXO-902392 — /metrics handler. Exposes Prometheus exposition format
// for fleet metrics collectors (VPC / Prometheus / Grafana). Spawns probe
// on an isolated OS thread to avoid starvations under heavy warming tasks.
async fn handle_metrics(Extension(server): Extension<Arc<McpServer>>) -> Response {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let probe_server = server.clone();
    let spawn_result = std::thread::Builder::new()
        .name("axon-metrics-probe".to_string())
        .spawn(move || {
            let body = crate::metrics_exporter::render_prometheus_metrics(probe_server.graph_store());
            let _ = tx.send(body);
        });

    if let Err(err) = spawn_result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain; charset=utf-8")],
            format!("# Error spawning metrics probe: {err}\naxon_brain_up 1\nup 1\n"),
        )
            .into_response();
    }

    match tokio::time::timeout(std::time::Duration::from_secs(3), rx).await {
        Ok(Ok(body)) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            body,
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain; charset=utf-8")],
            "# Metrics probe thread panicked\naxon_brain_up 1\nup 1\n".to_string(),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            [("content-type", "text/plain; charset=utf-8")],
            "# Metrics probe timed out\naxon_brain_up 1\nup 1\n".to_string(),
        )
            .into_response(),
    }
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
        // REQ-AXO-902286 — the stdio tunnel (spawned by the client IN its project
        // directory) forwards its cwd here so the shared brain resolves project_code
        // against the CALLER's project, not the brain's own (AXO) directory. Absent
        // header (SSE, an old tunnel, a direct call) → today's env/cwd fallback.
        let client_cwd = headers
            .get("x-axon-client-cwd")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        // REQ-AXO-902555 — client attribution. Carried via header or extracted from payload.
        let client_name = headers
            .get("x-axon-client-name")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                if payload.method == "initialize" {
                    payload
                        .params
                        .as_ref()
                        .and_then(|p| p.get("clientInfo"))
                        .and_then(|ci| ci.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .or_else(|| {
                headers
                    .get("user-agent")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            });

        let response = if payload.id.is_none() {
            let _ = tokio::task::spawn_blocking(move || server.handle_notification(payload)).await;
            record_latency(ServiceKind::Mcp, t0.elapsed().as_millis() as u64);
            // Per JSON-RPC 2.0 (Section 4.1 & 4.2): The Server MUST NOT reply to a Notification.
            // HTTP transport signals receipt via 202 Accepted with an empty body.
            StatusCode::ACCEPTED.into_response()
        } else {
            // Offload C-FFI / DB work to a blocking thread pool safely
            // No more mcp_active_flag: Zero-Sleep MVCC architecture handles concurrency.
            // REQ-AXO-902286 — install the client cwd for the duration of this one
            // synchronous dispatch; the RAII guard clears it (even on panic) before the
            // blocking thread is reused.
            // REQ-AXO-902555 — install the client identity guard symmetrically.
            match tokio::task::spawn_blocking(move || {
                let _client_cwd_guard = crate::mcp::ClientCwdGuard::install(client_cwd);
                let _client_name_guard = crate::mcp::ClientNameGuard::install(client_name);
                server.handle_request(payload)
            })
            .await
            {
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
                // REQ-AXO-902434 — source unique du nom canonique.
                .map(crate::mcp::canonical_tool_name);
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

#[allow(dead_code)]
fn is_runtime_command_proxy_tool_name(name: &str) -> bool {
    matches!(name, "resume_vectorization")
}

/// Compliant MCP SSE Endpoint
async fn handle_mcp_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // 1. Send the initial endpoint event as per MCP spec
    let endpoint_event =
        stream::once(async { Ok(Event::default().event("endpoint").data("/mcp")) });

    // 2. REQ-AXO-902063 — proactively tell a (re)connecting client its cached
    // tool list may be stale. After a promote restarts the brain with a changed
    // surface, a client that reconnects without re-running `initialize` keeps the
    // old registry and reads new tools as "absent" (the systemic cause of 3 false
    // llm_feedback doléances). The spec-compliant notifications/tools/list_changed
    // prompts a compliant client to re-fetch tools/list. Additive + harmless to
    // clients that ignore it (they still get the `status` anti-stale note).
    let list_changed = stream::once(async {
        Ok(Event::default().data(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#))
    });

    // 3. Keep-alive heartbeat every 15 seconds to prevent proxy timeouts
    let heartbeat = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
        std::time::Duration::from_secs(15),
    ))
    .map(|_| Ok(Event::default().comment("heartbeat")));

    let stream = endpoint_event.chain(list_changed).chain(heartbeat);
    Sse::new(stream)
}

/// REQ-AXO-902563 — run brain MCP and health probes outside the application runtime.
///
/// Long-running startup routines (such as `warm_all_ist_snapshots_at_boot` warming
/// IST snapshots across ~75 projects or SOLL snapshot warming) saturate Tokio's
/// global blocking thread pool and can stall the async executor. Running axum
/// on a dedicated OS thread with an independent multi-thread Tokio runtime makes
/// probe serving (/livez, /readyz, /startupz) and MCP routes resilient to workload
/// starvation, matching the indexer architecture established in commit 05fa97af.
///
/// The socket is bound immediately to the provided address to ensure immediate
/// availability before any background warming starts.
pub fn spawn_mcp_http_server(
    mcp_server: Arc<McpServer>,
    bind_addr: std::net::SocketAddr,
) -> Result<std::net::SocketAddr, std::io::Error> {
    let std_listener = std::net::TcpListener::bind(bind_addr)?;
    std_listener.set_nonblocking(true)?;
    let local_addr = std_listener.local_addr()?;

    let thread = std::thread::Builder::new().name("axon-brain-http".to_string());
    if let Err(error) = thread.spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("axon-brain-http-worker")
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(error) => {
                tracing::warn!(%error, "Brain HTTP multi-thread runtime creation failed, falling back to current_thread");
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        tracing::error!(%err, "Brain HTTP runtime creation failed");
                        return;
                    }
                }
            }
        };

        runtime.block_on(async move {
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(err) => {
                    tracing::error!(%err, "Converting std TcpListener to tokio TcpListener failed");
                    return;
                }
            };
            tracing::info!("✅ SQL Gateway/MCP: Listening on http://{}", local_addr);
            let app = app_router(mcp_server);
            if let Err(e) = axum::serve(listener, app).await {
                tracing::warn!(error = %e, addr = %local_addr, "Brain HTTP server exited with error");
            }
        });
    }) {
        tracing::error!(%error, "Brain HTTP thread creation failed");
        return Err(std::io::Error::other(format!(
            "Brain HTTP thread creation failed: {error}"
        )));
    }

    Ok(local_addr)
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
    async fn tools_call_http_exposes_init_bundle_as_structured_content() {
        // REQ-AXO-902517 — exercise the actual HTTP boundary consumed by MCP
        // clients, not only the producer or in-process dispatcher.
        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_http_structured").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));
        let app = app_router(mcp_server);
        let scope = crate::tests::test_helpers::unique_test_scope("http-structured-init");
        let project_path = format!("/tmp/{scope}");
        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "id": 902517,
            "params": {
                "name": "axon_init_project",
                "arguments": { "project_path": project_path }
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

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_json: Value = serde_json::from_slice(&body).unwrap();
        let result = &body_json["result"];
        assert!(result["content"][0]["text"].is_string(), "{result}");
        assert!(
            result["structuredContent"]["kickoff_bundle"].is_object(),
            "HTTP tools/call must expose the machine kickoff bundle: {result}"
        );
        assert_eq!(
            result["structuredContent"]["kickoff_bundle"],
            result["data"]["kickoff_bundle"],
            "the HTTP envelope must not fork canonical producer data"
        );
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
        // REQ-AXO-902274 / REQ-AXO-902630 — meme raison qu'en `scanner.rs` :
        // etat PROCESSUS-global, donc le reset doit passer par le verrou
        // partage. Vu de ce test, tout allait bien ; c'est ailleurs que ca
        // cassait.
        let _sg_guard = crate::test_support::service_guard_test_lock().lock();
        service_guard::reset_for_tests();
        mcp_request_started_with_class(McpRequestClass::Observer);
        assert_eq!(service_guard::interactive_requests_in_flight(), 0);
        mcp_request_finished_with_class(McpRequestClass::Observer);
        assert_eq!(service_guard::interactive_requests_in_flight(), 0);
    }

    fn http_get_raw(addr: std::net::SocketAddr, path: &str) -> (String, String) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr)
            .unwrap_or_else(|err| panic!("TcpStream::connect to {addr} failed: {err}"));
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
        (head.to_string(), body.to_string())
    }

    #[test]
    fn dedicated_brain_http_runtime_answers_without_caller_runtime() {
        use super::spawn_mcp_http_server;
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_dedicated_http").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));
        let bound = spawn_mcp_http_server(mcp_server, ([127, 0, 0, 1], port).into())
            .expect("spawn_mcp_http_server must succeed");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let (head, body) = loop {
            match std::net::TcpStream::connect(bound) {
                Ok(mut stream) => {
                    use std::io::{Read, Write};
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .write_all(
                            b"GET /livez HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                        )
                        .unwrap();
                    let mut resp = String::new();
                    stream.read_to_string(&mut resp).unwrap();
                    let (h, b) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
                    break (h.to_string(), b.to_string());
                }
                Err(error) if std::time::Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("dedicated brain http runtime did not bind: {error}"),
            }
        };

        assert!(head.starts_with("HTTP/1.1 200"), "head was {head}");
        assert_eq!(body, "ok");
    }

    #[test]
    fn health_probes_not_starved_during_heavy_blocking_warming_load() {
        use super::spawn_mcp_http_server;
        // REQ-AXO-902563: Test that /livez and /readyz answer promptly even when
        // the caller runtime's global blocking thread pool is completely saturated
        // with heavy startup tasks (e.g. warming IST/SOLL snapshots).
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_warming_starve").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));

        let bound = spawn_mcp_http_server(mcp_server, ([127, 0, 0, 1], port).into())
            .expect("spawn_mcp_http_server must succeed");

        rt.block_on(async move {
            // Saturate all 2 blocking threads of the application runtime with long-running jobs
            let (unblock_tx, unblock_rx) = std::sync::mpsc::channel();
            let unblock_rx = Arc::new(std::sync::Mutex::new(unblock_rx));
            for _ in 0..2 {
                let rx = unblock_rx.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = rx
                        .lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(5));
                });
            }

            // Give blocking threads a moment to pick up the tasks
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Interrogate /livez while blocking pool is 100% saturated
            let t0 = std::time::Instant::now();
            let (livez_head, livez_body) = http_get_raw(bound, "/livez");
            let livez_duration = t0.elapsed();
            assert!(
                livez_head.starts_with("HTTP/1.1 200"),
                "livez returned: {livez_head}"
            );
            assert_eq!(livez_body, "ok");
            assert!(
                livez_duration < std::time::Duration::from_millis(1500),
                "livez took too long ({:?}) during warming load",
                livez_duration
            );

            // Interrogate /readyz while blocking pool is 100% saturated
            let t1 = std::time::Instant::now();
            let (readyz_head, readyz_body) = http_get_raw(bound, "/readyz");
            let readyz_duration = t1.elapsed();
            assert!(
                readyz_head.starts_with("HTTP/1.1 200"),
                "readyz returned: {readyz_head} body: {readyz_body}"
            );
            let parsed_readyz: serde_json::Value =
                serde_json::from_str(&readyz_body).expect("valid JSON body on readyz");
            assert_eq!(parsed_readyz["state"], "ready");
            assert!(
                readyz_duration < std::time::Duration::from_millis(1500),
                "readyz took too long ({:?}) during warming load",
                readyz_duration
            );

            // Unblock the simulated warming tasks
            let _ = unblock_tx.send(());
            let _ = unblock_tx.send(());
        });
    }

    #[test]
    fn http_socket_bound_immediately_on_spawn() {
        use super::spawn_mcp_http_server;
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_immediate_bind").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));

        let bound = spawn_mcp_http_server(mcp_server, ([127, 0, 0, 1], port).into())
            .expect("spawn_mcp_http_server should bind immediately");

        // Connecting synchronously must succeed immediately without any retry loop
        let stream = std::net::TcpStream::connect(bound);
        assert!(
            stream.is_ok(),
            "TCP socket must accept connections immediately upon return from spawn"
        );
    }

    #[test]
    fn test_metrics_endpoint_prometheus_exposition() {
        use super::spawn_mcp_http_server;
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let store = Arc::new(
            crate::tests::test_helpers::create_test_db()
                .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_metrics_exposition").unwrap()),
        );
        let mcp_server = Arc::new(McpServer::new(store));

        let bound = spawn_mcp_http_server(mcp_server, ([127, 0, 0, 1], port).into())
            .expect("spawn_mcp_http_server must succeed");

        let (head, body) = http_get_raw(bound, "/metrics");
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "/metrics must return 200 OK, got: {head}"
        );
        assert!(
            head.to_lowercase().contains("content-type: text/plain"),
            "content-type must be text/plain, got: {head}"
        );

        // Required Prometheus gauges and counters specified by REQ-AXO-902392
        assert!(body.contains("axon_brain_up 1"), "missing axon_brain_up in: {body}");
        assert!(body.contains("up 1"), "missing up 1 in: {body}");
        assert!(body.contains("axon_indexer_alive "), "missing axon_indexer_alive in: {body}");
        assert!(body.contains("axon_chunks_pending "), "missing axon_chunks_pending in: {body}");
        assert!(body.contains("axon_chunks_embedded "), "missing axon_chunks_embedded in: {body}");
        assert!(body.contains("axon_chunks_total "), "missing axon_chunks_total in: {body}");
        assert!(body.contains("axon_coverage_pct "), "missing axon_coverage_pct in: {body}");

        // Strict invariant: when not armed, b2_cpu_fallback_ratio MUST be -1, NEVER 0
        assert!(body.contains("axon_b2_armed 0"), "b2 must be unarmed in fresh test db: {body}");
        assert!(body.contains("axon_b2_cpu_fallback_ratio -1"), "unarmed ratio MUST be -1: {body}");
        assert!(body.contains("axon_b2_degraded_threshold "), "missing degraded threshold: {body}");
        assert!(body.contains("axon_b2_critical_threshold "), "missing critical threshold: {body}");
    }
}

