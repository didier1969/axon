//! REQ-AXO-901806 — Dashboard state v1 event composition.
//!
//! Single-event architecture replacing the dashboard's polling triple
//! (heartbeat JSON file 1 Hz + MCP embedding_status 3 s + PG count queries
//! per-tick). The brain composes the full dashboard state every second
//! inside `main_telemetry::spawn_runtime_telemetry`, publishes it as a
//! `dashboard_state_v1` event on the telemetry broadcast channel, and
//! caches the latest snapshot here for the `/dashboard/state` HTTP
//! endpoint to serve at instant cost (no recompute on read).
//!
//! Per-project aggregates and totals are computed in PG via the
//! `axon_runtime.dashboard_per_project_counts(5)` and
//! `axon_runtime.dashboard_totals(5)` functions (db/ddl/08_dashboard_state.sql).
//! They use a TTL cache so the 1 Hz brain composition only triggers the
//! expensive aggregate query every 5 s. Cache lives in PG, not brain RAM
//! (survives restart, multi-instance brain ready).

use crate::graph::GraphStore;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::broadcast;
use tracing::warn;

static LATEST_DASHBOARD_STATE: OnceLock<Mutex<Option<Value>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Value>> {
    LATEST_DASHBOARD_STATE.get_or_init(|| Mutex::new(None))
}

/// Returns the latest cached dashboard state, or `None` if the 1 Hz loop
/// in `main_telemetry` has not yet computed one. The HTTP handler at
/// `GET /dashboard/state` calls this for instant responses.
pub fn latest_dashboard_state() -> Option<Value> {
    slot().lock().ok().and_then(|guard| guard.clone())
}

/// Update the cached dashboard state. Called once per second by
/// `main_telemetry::spawn_runtime_telemetry` after composing the event
/// from runtime + IST + PG + embedder state.
pub(crate) fn publish_dashboard_state(state: Value) {
    if let Ok(mut guard) = slot().lock() {
        *guard = Some(state);
    }
}

/// Compose the dashboard_state_v1 JSON envelope. Called by the 1 Hz
/// telemetry loop with all the runtime context available.
///
/// `pg_per_project` and `pg_totals` are the raw jsonb results from the
/// two PG functions (already cached server-side with 5 s TTL — calling
/// them every second is cheap after warm-up).
///
/// Field naming mirrors what the dashboard LiveViews already consume,
/// so the Elixir side can replace its three pollers with a single
/// subscriber + assigns refresh.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_dashboard_state_v1(
    ts_ms: u64,
    build_id: &str,
    install_generation: &str,
    runtime_mode: &str,
    instance_kind: &str,
    degraded_reason: Option<&str>,
    embedder_requested: &str,
    embedder_effective: &str,
    embedder_init_error: Option<&str>,
    pg_per_project: Value,
    pg_totals: Value,
    pipeline_b_workers: u64,
    chunk_embeddings_per_second: f64,
    vector_chunks_embedded_total: u64,
    graph_workers_active: u64,
    graph_workers_started: u64,
    ingress_buffered_entries: u64,
    ingress_hot_entries: u64,
    ready_queue_chunks_current: u64,
    ready_queue_chunks_small: u64,
    ready_queue_chunks_medium: u64,
    ready_queue_chunks_large: u64,
    homogeneous_batches_total: u64,
    mixed_fallback_batches_total: u64,
    last_consumed_batch_lane: &str,
    service_pressure: &str,
    scheduler_state: &str,
    runtime_idle: bool,
) -> Value {
    json!({
        "event": "dashboard_state_v1",
        "ts_ms": ts_ms,
        "runtime": {
            "build_id": build_id,
            "install_generation": install_generation,
            "runtime_mode": runtime_mode,
            "instance_kind": instance_kind,
            "degraded_reason": degraded_reason,
        },
        "embedder": {
            "requested": embedder_requested,
            "effective": embedder_effective,
            "init_error": embedder_init_error,
            "last_lane": last_consumed_batch_lane,
        },
        "telemetry": {
            "chunk_embeddings_per_second": chunk_embeddings_per_second,
            "vector_chunks_embedded_total": vector_chunks_embedded_total,
            "graph_workers_active_current": graph_workers_active,
            "graph_workers_started_total": graph_workers_started,
            "ingress_buffered_entries": ingress_buffered_entries,
            "ingress_hot_entries": ingress_hot_entries,
            "ready_queue_chunks_current": ready_queue_chunks_current,
            "ready_queue_chunks_small": ready_queue_chunks_small,
            "ready_queue_chunks_medium": ready_queue_chunks_medium,
            "ready_queue_chunks_large": ready_queue_chunks_large,
            "homogeneous_batches_total": homogeneous_batches_total,
            "mixed_fallback_batches_total": mixed_fallback_batches_total,
            "service_pressure": service_pressure,
            "scheduler": scheduler_state,
            "runtime_idle": runtime_idle,
            "pipeline_b_workers": pipeline_b_workers,
        },
        "per_project": pg_per_project,
        "totals": pg_totals,
    })
}

// SQL gateway returns single-cell scalar queries as `[[<value>]]` — outer
// array = rows, inner array = columns. Extract the first cell or fall back
// to JSON null when the shape doesn't match (transient PG hiccup).
fn extract_first_cell(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array()?.first()?.as_array()?.first().cloned())
        .unwrap_or(Value::Null)
}

/// REQ-AXO-901806 — Convenience composer used by the 1 Hz telemetry loop.
/// Queries the two PG functions (cached server-side with 5 s TTL), assembles
/// the `dashboard_state_v1` event, publishes to the slot for the HTTP
/// endpoint, and emits on the broadcast channel for live subscribers.
///
/// All work is synchronous. Failures degrade gracefully: a PG miss emits
/// the event with empty `per_project` / `totals` and a warning log ; a
/// broadcast send error is silently ignored (no live subscribers).
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_publish_and_emit(
    store: &Arc<GraphStore>,
    results_tx: &broadcast::Sender<String>,
    ts_ms: u64,
    build_id: &str,
    install_generation: &str,
    runtime_mode: &str,
    instance_kind: &str,
    degraded_reason: Option<&str>,
    embedder_requested: &str,
    embedder_effective: &str,
    embedder_init_error: Option<&str>,
    pipeline_b_workers: u64,
    chunk_embeddings_per_second: f64,
    vector_chunks_embedded_total: u64,
    graph_workers_active: u64,
    graph_workers_started: u64,
    ingress_buffered_entries: u64,
    ingress_hot_entries: u64,
    ready_queue_chunks_current: u64,
    ready_queue_chunks_small: u64,
    ready_queue_chunks_medium: u64,
    ready_queue_chunks_large: u64,
    homogeneous_batches_total: u64,
    mixed_fallback_batches_total: u64,
    last_consumed_batch_lane: &str,
    service_pressure: &str,
    scheduler_state: &str,
    runtime_idle: bool,
) {
    let per_project = match store
        .execute_raw_sql_gateway("SELECT axon_runtime.dashboard_per_project_counts(5)")
    {
        Ok(raw) => extract_first_cell(&raw),
        Err(e) => {
            warn!("dashboard_state: per_project query failed: {e:?}");
            Value::Array(vec![])
        }
    };

    let totals = match store
        .execute_raw_sql_gateway("SELECT axon_runtime.dashboard_totals(5)")
    {
        Ok(raw) => extract_first_cell(&raw),
        Err(e) => {
            warn!("dashboard_state: totals query failed: {e:?}");
            Value::Object(serde_json::Map::new())
        }
    };

    let state = compose_dashboard_state_v1(
        ts_ms,
        build_id,
        install_generation,
        runtime_mode,
        instance_kind,
        degraded_reason,
        embedder_requested,
        embedder_effective,
        embedder_init_error,
        per_project,
        totals,
        pipeline_b_workers,
        chunk_embeddings_per_second,
        vector_chunks_embedded_total,
        graph_workers_active,
        graph_workers_started,
        ingress_buffered_entries,
        ingress_hot_entries,
        ready_queue_chunks_current,
        ready_queue_chunks_small,
        ready_queue_chunks_medium,
        ready_queue_chunks_large,
        homogeneous_batches_total,
        mixed_fallback_batches_total,
        last_consumed_batch_lane,
        service_pressure,
        scheduler_state,
        runtime_idle,
    );

    // 1) Update the in-memory slot for HTTP /dashboard/state.
    publish_dashboard_state(state.clone());

    // 2) Broadcast to live socket subscribers (dashboard via BridgeClient).
    if let Ok(message) = serde_json::to_string(&state) {
        let _ = results_tx.send(message + "\n");
    }
}
