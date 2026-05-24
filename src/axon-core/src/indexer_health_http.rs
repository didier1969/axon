// REQ-AXO-901735 / DEC-AXO-901615 — mini-serveur HTTP health pour l'indexer.
//
// Reason of existence : le brain expose déjà /livez /readyz /startupz via
// `mcp_http::app_router` (REQ-AXO-901735 Phase 2a), mais l'indexer en
// standalone (modes IndexerGraph / IndexerVector / IndexerFull SANS
// start_mcp_http=true) n'a aucun serveur HTTP. process-compose ne peut
// donc ni probe sa liveness ni gérer ses dépendances aval.
//
// Ce module spawne un mini-serveur axum SUR UN PORT DÉDIÉ
// (`AXON_INDEXER_HEALTH_PORT`, défaut `HYDRA_HTTP_PORT + 10`) avec
// uniquement les 3 endpoints de probe — pas de surface MCP / SQL.
//
// V1 : les 3 endpoints retournent 200 OK. Le simple fait que axum réponde
// prouve liveness + readiness côté indexer (le process est en train de
// tourner la pipeline). V2 raffinera /readyz (PG ping via tokio-postgres,
// freshness IST snapshot) et /startupz (flag AtomicBool set par init).

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

/// État partagé minimal — flag StartupDone set par init runtime quand les
/// workers sémantiques + pipeline_v2 sont spawnés.
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
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"state": "ready"})),
                    )
                        .into_response()
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
/// `HYDRA_HTTP_PORT + 10` > 44139. Le +10 garantit pas de collision avec
/// le port HTTP du brain dans la même instance (brain :44129 / health
/// indexer :44139 en live ; brain :44139 / health indexer :44149 en dev).
pub fn resolve_health_port() -> u16 {
    if let Ok(p) = std::env::var("AXON_INDEXER_HEALTH_PORT") {
        if let Ok(n) = p.trim().parse::<u16>() {
            return n;
        }
    }
    let base = std::env::var("HYDRA_HTTP_PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(44129);
    base + 10
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Sérialise les tests qui mutent l env pour éviter les race entre
    // tests parallèles (cargo test par défaut threads multiples).
    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_health_port_uses_indexer_override_first() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("HYDRA_HTTP_PORT");
        std::env::set_var("AXON_INDEXER_HEALTH_PORT", "33333");
        assert_eq!(resolve_health_port(), 33333);
        std::env::remove_var("AXON_INDEXER_HEALTH_PORT");
    }

    #[test]
    fn resolve_health_port_falls_back_to_hydra_plus_10() {
        let _guard = ENV_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("AXON_INDEXER_HEALTH_PORT");
        std::env::set_var("HYDRA_HTTP_PORT", "44129");
        assert_eq!(resolve_health_port(), 44139);
        std::env::remove_var("HYDRA_HTTP_PORT");
    }

    #[test]
    fn startup_state_transitions() {
        let s = IndexerHealthState::new();
        assert!(!s.is_started());
        s.mark_startup_done();
        assert!(s.is_started());
    }
}
