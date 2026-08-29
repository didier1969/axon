// REQ-AXO-901735 / DEC-AXO-901615 — mini-serveur HTTP health pour l'indexer.
//
// Reason of existence : le brain expose déjà /livez /readyz /startupz via
// `mcp_http::app_router` (REQ-AXO-901735 Phase 2a), mais l'indexer en
// standalone (modes IndexerGraph / IndexerVector / IndexerFull SANS
// start_mcp_http=true) n'a aucun serveur HTTP. process-compose ne peut
// donc ni probe sa liveness ni gérer ses dépendances aval.
//
// Ce module spawne un mini-serveur axum SUR UN PORT DÉDIÉ
// (`AXON_INDEXER_HEALTH_PORT`, défaut 44130 live / 44149 dev) avec
// uniquement les 3 endpoints de probe — pas de surface MCP / SQL.
//
// `/livez` prouve uniquement que le processus HTTP répond. `/readyz` et
// `/startupz` restent à 503 tant que le pipeline n'est pas démarré et, pour
// un mode d'ingestion, tant qu'un heartbeat durable PG récent n'a pas été
// publié. Une simple présence de PID ne peut donc plus masquer un indexeur
// qui ne produit rien.

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

const HEARTBEAT_FRESHNESS_MS: i64 = 30_000;

/// État partagé entre le boot, le publisher PG et les handlers de probes.
#[derive(Clone)]
pub struct IndexerHealthState {
    pipeline_started: Arc<AtomicBool>,
    heartbeat_required: bool,
    last_heartbeat_ms: Arc<AtomicI64>,
    last_error: Arc<RwLock<Option<String>>>,
}

impl IndexerHealthState {
    pub fn new(heartbeat_required: bool) -> Self {
        Self {
            pipeline_started: Arc::new(AtomicBool::new(false)),
            heartbeat_required,
            last_heartbeat_ms: Arc::new(AtomicI64::new(0)),
            last_error: Arc::new(RwLock::new(None)),
        }
    }

    pub fn mark_pipeline_started(&self) {
        self.pipeline_started.store(true, Ordering::Release);
    }

    pub fn is_started(&self) -> bool {
        self.pipeline_started.load(Ordering::Acquire)
    }

    pub fn record_heartbeat_success(&self, heartbeat_ms: i64) {
        self.last_heartbeat_ms
            .store(heartbeat_ms.max(0), Ordering::Release);
        if let Ok(mut error) = self.last_error.write() {
            *error = None;
        }
    }

    pub fn record_heartbeat_failure(&self, error: impl Into<String>) {
        if let Ok(mut current) = self.last_error.write() {
            *current = Some(error.into());
        }
    }

    pub fn is_ready_at(&self, now_ms: i64) -> bool {
        if !self.is_started() {
            return false;
        }
        if !self.heartbeat_required {
            return true;
        }
        let heartbeat_ms = self.last_heartbeat_ms.load(Ordering::Acquire);
        heartbeat_ms > 0 && now_ms.saturating_sub(heartbeat_ms) <= HEARTBEAT_FRESHNESS_MS
    }

    fn reasons_at(&self, now_ms: i64) -> Vec<String> {
        let mut reasons = Vec::new();
        if !self.is_started() {
            reasons.push("pipeline_not_started".to_string());
        }
        if self.heartbeat_required {
            let heartbeat_ms = self.last_heartbeat_ms.load(Ordering::Acquire);
            if heartbeat_ms <= 0 {
                reasons.push("heartbeat_not_published".to_string());
            } else if now_ms.saturating_sub(heartbeat_ms) > HEARTBEAT_FRESHNESS_MS {
                reasons.push("heartbeat_stale".to_string());
            }
        }
        reasons
    }

    fn status_json(&self, now_ms: i64, ready: bool) -> serde_json::Value {
        let heartbeat_ms = self.last_heartbeat_ms.load(Ordering::Acquire);
        let last_error = self.last_error.read().ok().and_then(|value| value.clone());
        serde_json::json!({
            "state": if ready { "ready" } else { "not_ready" },
            "pipeline_started": self.is_started(),
            "heartbeat_required": self.heartbeat_required,
            "heartbeat_ms": if heartbeat_ms > 0 { Some(heartbeat_ms) } else { None },
            "heartbeat_age_ms": if heartbeat_ms > 0 { Some(now_ms.saturating_sub(heartbeat_ms)) } else { None },
            "reasons": self.reasons_at(now_ms),
            "last_error": last_error,
            "pid": std::process::id(),
            "build_id": std::env::var("AXON_BUILD_ID").ok().filter(|v| !v.trim().is_empty()),
        })
    }
}

/// Construit le router probe-only — pas de Extension métier autre que
/// l'IndexerHealthState (cloné pour chaque handler via le closure du
/// router).
pub fn health_router(state: IndexerHealthState) -> Router {
    Router::new()
        .route(
            "/livez",
            get({
                let _s = state.clone();
                move || async { (StatusCode::OK, "ok").into_response() }
            }),
        )
        .route(
            "/readyz",
            get({
                let s = state.clone();
                move || {
                    let s = s.clone();
                    async move {
                        let now_ms = crate::clock::now_unix_ms();
                        let ready = s.is_ready_at(now_ms);
                        let status = if ready {
                            StatusCode::OK
                        } else {
                            StatusCode::SERVICE_UNAVAILABLE
                        };
                        (status, Json(s.status_json(now_ms, ready))).into_response()
                    }
                }
            }),
        )
        .route(
            "/startupz",
            get(move || {
                let state = state.clone();
                async move {
                    let now_ms = crate::clock::now_unix_ms();
                    let ready = state.is_ready_at(now_ms);
                    if ready {
                        (StatusCode::OK, Json(state.status_json(now_ms, true))).into_response()
                    } else {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(state.status_json(now_ms, false)),
                        )
                            .into_response()
                    }
                }
            }),
        )
}

/// Bind + serve le mini-router. Best-effort : si le port est pris ou le
/// bind échoue, log un warn et continue (l'indexer reste fonctionnel sans
/// HTTP probe, juste process-compose ne pourra pas le surveiller). Le
/// caller doit `tokio::spawn` cet appel.
pub async fn serve_health_probes(port: u16, state: IndexerHealthState) {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            info!(
                "Indexer health probes listening on http://{} ({{livez,readyz,startupz}})",
                addr
            );
            let app = health_router(state);
            if let Err(e) = axum::serve(listener, app).await {
                warn!(
                    error = %e,
                    addr = %addr,
                    "Indexer health probes server exited with error"
                );
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                addr = %addr,
                "Indexer health probes bind failed; process-compose probes will time out. \
                 Indexer continues without HTTP probes."
            );
        }
    }
}

/// REQ-AXO-902550 — run probes outside the indexer's application runtime.
///
/// CUDA/ORT session construction contains long synchronous sections. Running
/// axum through `tokio::spawn` on that same runtime produced a deceptive state:
/// the TCP listener existed, but its accept loop could not be polled and more
/// than 50 liveness requests accumulated in the kernel backlog. A dedicated OS
/// thread with a single-thread Tokio runtime makes the observer independent of
/// the workload it observes.
pub fn spawn_health_probe_server(port: u16, state: IndexerHealthState) {
    let thread = std::thread::Builder::new().name("axon-indexer-health".to_string());
    if let Err(error) = thread.spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                warn!(%error, "Indexer health probe runtime creation failed");
                return;
            }
        };
        runtime.block_on(serve_health_probes(port, state));
    }) {
        warn!(%error, "Indexer health probe thread creation failed");
    }
}

/// Resolve le port health depuis l'env : `AXON_INDEXER_HEALTH_PORT` >
/// `AXON_BRAIN_PORT + 1` > 44130. Ports explicites par instance dans
/// process-compose yaml (live=44130, dev=44149). Le +1 est le fallback
/// quand aucun override n'est posé.
pub fn resolve_health_port() -> u16 {
    resolve_health_port_from(
        std::env::var("AXON_INDEXER_HEALTH_PORT").ok(),
        std::env::var("AXON_BRAIN_PORT").ok(),
    )
}

/// PURE core of `resolve_health_port` — both env values arrive as parameters.
///
/// REQ-AXO-902261 — the tests for this used to mutate the process environment behind a
/// `static ENV_TEST_LOCK: Mutex<()>` declared right here: a FOURTH private lock over the
/// one process environment, serializing this file against itself and nothing else. They
/// also left both variables unset afterwards, for every test that ran later.
///
/// Extracting the decision beats locking it: the precedence rule is what deserves the
/// test, and it can be exercised without touching global state at all.
pub fn resolve_health_port_from(
    indexer_override: Option<String>,
    brain_port: Option<String>,
) -> u16 {
    if let Some(n) = indexer_override.and_then(|p| p.trim().parse::<u16>().ok()) {
        return n;
    }
    let base = brain_port
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(44129);
    base.saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    use tower::ServiceExt;

    // REQ-AXO-902261 — no lock and no env mutation: the values are arguments now.

    #[test]
    fn resolve_health_port_uses_indexer_override_first() {
        assert_eq!(
            resolve_health_port_from(Some("33333".into()), Some("44129".into())),
            33333,
            "the explicit indexer override outranks the brain port"
        );
    }

    #[test]
    fn resolve_health_port_falls_back_to_brain_port_plus_one() {
        assert_eq!(resolve_health_port_from(None, Some("44129".into())), 44130);
    }

    #[test]
    fn resolve_health_port_defaults_when_nothing_is_set() {
        assert_eq!(resolve_health_port_from(None, None), 44130);
    }

    #[test]
    fn resolve_health_port_ignores_a_malformed_override() {
        // A typo in AXON_INDEXER_HEALTH_PORT must fall through to the brain-port rule,
        // never yield 0. Untested before — the old tests only fed valid values.
        assert_eq!(
            resolve_health_port_from(Some("not-a-port".into()), Some("44129".into())),
            44130
        );
    }

    #[test]
    fn resolve_health_port_does_not_overflow_at_the_top_of_the_range() {
        // base + 1 on 65535 would wrap to 0 and bind a random port. `saturating_add`
        // keeps it in range; nothing pinned this before.
        assert_eq!(resolve_health_port_from(None, Some("65535".into())), 65535);
    }

    #[test]
    fn dedicated_probe_runtime_answers_without_a_caller_runtime() {
        let reservation = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let state = IndexerHealthState::new(false);
        state.mark_pipeline_started();
        spawn_health_probe_server(port, state);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut stream = loop {
            match std::net::TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => break stream,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("dedicated health runtime did not bind: {error}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream
            .write_all(b"GET /livez HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }

    #[test]
    fn startup_state_transitions() {
        let s = IndexerHealthState::new(true);
        assert!(!s.is_started());
        assert!(!s.is_ready_at(1_000));

        s.mark_pipeline_started();
        assert!(s.is_started());
        assert!(
            !s.is_ready_at(1_000),
            "a spawned pipeline is not ready until its first heartbeat is durable"
        );

        s.record_heartbeat_success(1_000);
        assert!(s.is_ready_at(1_001));
        assert!(
            !s.is_ready_at(31_001),
            "readiness must expire with the canonical 30s heartbeat window"
        );
    }

    #[test]
    fn non_ingestion_indexer_does_not_require_pipeline_heartbeat() {
        let s = IndexerHealthState::new(false);
        assert!(!s.is_ready_at(1_000));
        s.mark_pipeline_started();
        assert!(s.is_started());
        assert!(s.is_ready_at(1_000));
    }

    #[test]
    fn transient_heartbeat_failure_is_tolerated_until_freshness_expires() {
        let s = IndexerHealthState::new(true);
        s.mark_pipeline_started();
        s.record_heartbeat_success(1_000);
        assert!(s.is_ready_at(1_001));
        s.record_heartbeat_failure("postgres unavailable");
        assert!(s.is_ready_at(1_002));
        assert!(!s.is_ready_at(31_001));
    }

    #[tokio::test]
    async fn http_probes_distinguish_process_liveness_from_durable_readiness() {
        let state = IndexerHealthState::new(true);

        let live = health_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/livez")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(live.status(), StatusCode::OK);

        let not_ready = health_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(not_ready.status(), StatusCode::SERVICE_UNAVAILABLE);

        state.mark_pipeline_started();
        state.record_heartbeat_success(crate::clock::now_unix_ms());
        let ready = health_router(state)
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::OK);
    }
}
