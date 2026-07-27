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
// V1 : les 3 endpoints retournent 200 OK. Le simple fait que axum réponde
// prouve liveness + readiness côté indexer (le process est en train de
// tourner la pipeline). V2 raffinera /readyz (PG ping via tokio-postgres,
// freshness IST snapshot) et /startupz (flag AtomicBool set par init).

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

/// État partagé minimal — flag StartupDone set par init runtime quand les
/// workers sémantiques + pipeline sont spawnés.
#[derive(Clone, Default)]
pub struct IndexerHealthState {
    pub startup_done: Arc<AtomicBool>,
}

impl IndexerHealthState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_startup_done(&self) {
        self.startup_done.store(true, Ordering::Release);
    }

    pub fn is_started(&self) -> bool {
        self.startup_done.load(Ordering::Acquire)
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
                let _s = state.clone();
                move || async {
                    // V1 : si axum répond, l'indexer est ready.
                    // V2 (TODO) : ping PG via tokio-postgres + check
                    // freshness IST snapshot pour distinguer ready vs
                    // degraded (cf. doctrine Sridharan graceful degradation).
                    (StatusCode::OK, Json(serde_json::json!({"state": "ready"}))).into_response()
                }
            }),
        )
        .route(
            "/startupz",
            get(move || {
                let state = state.clone();
                async move {
                    if state.is_started() {
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({"state": "started"})),
                        )
                            .into_response()
                    } else {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({
                                "state": "starting",
                                "reasons": ["indexer_init_not_complete"]
                            })),
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
    fn startup_state_transitions() {
        let s = IndexerHealthState::new();
        assert!(!s.is_started());
        s.mark_startup_done();
        assert!(s.is_started());
    }
}
