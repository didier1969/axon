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
//! Data sources (PIL-AXO-009 PG canonical where it fits, RAM for
//! high-frequency metrics):
//!
//! * `axon_runtime.dashboard_state_full(5)` — PG composite returning
//!   `{totals, per_project, runtime_config}` in one round-trip.
//!     - `totals` + `per_project`: TTL-cached aggregates (`db/ddl/08`).
//!     - `runtime_config`: boot-time semi-static configs (worker
//!       counts, batch sizes, NOTIFY channel, coldstart cadence) written
//!       by `runtime_config::write_indexer_config_snapshot` at indexer
//!       startup.
//! * `graph_store.latest_lifecycle_heartbeat("indexer")` — PG-backed
//!   lifecycle phase/wake/sleep counts.
//! * `mcp::tools_system::cached_fs_counters()` — filesystem walk with
//!   60 s TTL (`disk_files`, `eligible_files`).
//! * In-memory snapshot from `main_telemetry` 1 Hz tick — live rates,
//!   queues, scheduler, embedder identity, runtime mode.

use crate::graph::GraphStore;
use crate::mcp::tools_system::cached_fs_counters;
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

/// Live in-memory metrics passed from `main_telemetry` 1 Hz tick.
/// Grouped in a struct so the call site is readable instead of dragging
/// 30 positional args. PG-backed fields (totals, per_project,
/// runtime_config, lifecycle) and filesystem counters are sourced
/// inside `compose_publish_and_emit` directly — no need to pipe them
/// through main_telemetry.
pub(crate) struct LiveMetrics<'a> {
    pub ts_ms: u64,
    pub build_id: &'a str,
    pub install_generation: &'a str,
    pub runtime_mode: &'a str,
    pub instance_kind: &'a str,
    pub degraded_reason: Option<&'a str>,
    pub embedder_requested: &'a str,
    pub embedder_effective: &'a str,
    pub embedder_init_error: Option<&'a str>,
    /// DEC-AXO-901626 — observable Pipeline B compute ("GPU" | "CPU"),
    /// derived from the indexer pid + nvidia-smi (not the raced slot).
    pub embedder_compute: &'a str,
    /// "nvidia_smi" | "pg_inferred" | "unknown" — how `embedder_compute`
    /// was determined, so the dashboard can flag inference vs OS truth.
    pub embedder_compute_source: &'a str,
    pub last_consumed_batch_lane: &'a str,
    pub chunk_embeddings_per_second: f64,
    /// Monotone counter since brain boot, includes re-embeddings.
    /// NOT bounded by total chunks — comparing this to `total_chunks`
    /// is meaningless (a chunk re-embedded twice counts twice here).
    pub vector_chunks_embedded_cumulative: u64,
    pub graph_workers_active: u64,
    pub graph_workers_started: u64,
    pub ingress_buffered_entries: u64,
    pub ingress_hot_entries: u64,
    pub ready_queue_chunks_current: u64,
    pub ready_queue_chunks_small: u64,
    pub ready_queue_chunks_medium: u64,
    pub ready_queue_chunks_large: u64,
    pub homogeneous_batches_total: u64,
    pub mixed_fallback_batches_total: u64,
    pub service_pressure: &'a str,
    pub scheduler_state: &'a str,
    pub runtime_idle: bool,
}

// SQL gateway returns single-cell scalar queries as `[[<value>]]` — outer
// array = rows, inner array = columns. JSONB cells are serialized as
// JSON-encoded strings by the FFI bridge, so we decode them back to
// `Value` so downstream `.get("...")` accessors work. Returns `Null`
// when the shape doesn't match (transient PG hiccup).
fn extract_first_cell(raw: &str) -> Value {
    let cell = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array()?.first()?.as_array()?.first().cloned())
        .unwrap_or(Value::Null);
    match cell {
        Value::String(s) => serde_json::from_str::<Value>(&s).unwrap_or(Value::Null),
        other => other,
    }
}

/// Read the `dashboard_state_full(3)` PG composite. Returns the jsonb
/// body or an empty fallback (PG hiccup degradation).
///
/// REQ-AXO-901834 — TTL=3 s compromise. TTL=1 s revert : empirical
/// measurement under ingest load showed SP recompute = ~692 ms cold
/// (sequential scan on chunk/chunkembedding/symbol/edge) and ~0.85 ms
/// warm. At 1 Hz brain push, TTL=1 forced recompute every tick → brain
/// saturated (89 % CPU, /readyz unresponsive). TTL=3 yields 3 ticks
/// warm + 1 tick cold (~692 ms) ≈ 25 % brain budget per cycle, well
/// within room. Visible dashboard staleness ≤ 3 s, within the "5 s max"
/// contract.
///
/// If corpus growth raises recompute cost beyond 1 s, three follow-ups
/// to consider (in order) : (a) materialize the per-project aggregate
/// as a refresh-on-write index on chunk/chunkembedding, (b) move the
/// 1 Hz brain push to a separate tokio task so the heavy SP call
/// doesn't block /readyz, (c) bump TTL further (still ≤ 5 s).
fn read_dashboard_state_full(store: &Arc<GraphStore>) -> Value {
    match store.execute_raw_sql_gateway("SELECT axon_runtime.dashboard_state_full(3)") {
        Ok(raw) => {
            let cell = extract_first_cell(&raw);
            if matches!(cell, Value::Null) {
                // Shape mismatch = transient PG hiccup or schema not yet
                // materialised. Warn (audit), return the empty fallback
                // so the dashboard renders "—" rather than crash.
                warn!(
                    "dashboard_state: dashboard_state_full returned undecodable shape (raw_len={})",
                    raw.len()
                );
                json!({
                    "totals": {},
                    "per_project": [],
                    "runtime_config": {},
                })
            } else {
                cell
            }
        }
        Err(e) => {
            warn!("dashboard_state: dashboard_state_full query failed: {e:?}");
            json!({
                "totals": {},
                "per_project": [],
                "runtime_config": {},
            })
        }
    }
}

/// Compute the lifecycle block. Prefers the PG-backed indexer
/// heartbeat row (fresh ≤30 s) ; falls back to the brain's local
/// embedder lifecycle singleton when no fresh heartbeat exists.
fn compose_lifecycle_block(store: &Arc<GraphStore>) -> Value {
    const HEARTBEAT_FRESHNESS_MS: i64 = 30_000;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0);

    let indexer_hb = store
        .latest_lifecycle_heartbeat("indexer")
        .ok()
        .flatten()
        .filter(|row| (now_ms - row.heartbeat_ms).max(0) <= HEARTBEAT_FRESHNESS_MS);

    if let Some(row) = indexer_hb {
        let age_ms = (now_ms - row.heartbeat_ms).max(0);
        json!({
            "phase": row.phase,
            "source": "indexer_heartbeat",
            "heartbeat_age_ms": age_ms,
            "wake_count": row.wake_count,
            "sleep_count": row.sleep_count,
            "last_used_ms": row.last_used_ms,
        })
    } else {
        let local = crate::embedder::lifecycle_machine::process_lifecycle();
        json!({
            "phase": local.phase().as_str(),
            "source": "brain_local_singleton",
            "heartbeat_age_ms": Value::Null,
            "wake_count": local.wake_count(),
            "sleep_count": local.sleep_count(),
            "last_used_ms": local.last_used_ms(),
        })
    }
}

/// Pipeline observability taxonomy (slice 3 SOTA refactor) — answers
/// the operator's "pourquoi tu ne travailles pas ?" without needing
/// bash + psql. Computed from `runtime_mode`, `runtime_idle`, the
/// pending chunks count, and `embedder_init_error`. Cf. plan
/// `tablis-le-plan-avec-bright-hare.md` section 3.
pub(crate) fn compute_pipeline_status(
    runtime_mode: &str,
    runtime_idle: bool,
    pending_chunks: i64,
    embedder_init_error: Option<&str>,
) -> (&'static str, Option<&'static str>) {
    let pending_gt_zero = pending_chunks > 0;

    // gpu_unavailable wins over every other diagnosis: the embedder
    // can't run regardless of pending count.
    if embedder_init_error.is_some() {
        return ("indexer_idle_blocked", Some("gpu_unavailable"));
    }

    // brain_only + pending = 17K orphans without indexer paired (the
    // canonical operator pain point session 63 — AXO 17 252 chunks).
    if runtime_mode == "brain_only" && pending_gt_zero {
        return ("indexer_idle_blocked", Some("no_indexer_paired"));
    }

    // brain_only + no pending = trivial happy idle.
    if runtime_mode == "brain_only" {
        return ("brain_only", None);
    }

    // indexer modes (indexer_graph / indexer_vector / indexer_full).
    if !runtime_idle {
        return ("indexer_active", None);
    }

    // Idle. Distinguish done from blocked by whether there's pending
    // work the pipeline should be draining.
    if pending_gt_zero {
        ("indexer_idle_blocked", Some("demand_pull_b_stalled"))
    } else {
        ("indexer_idle_done", None)
    }
}

/// Compose the `dashboard_state_v1` JSON envelope from live in-memory
/// metrics + PG composite (totals, per_project, runtime_config) +
/// lifecycle block + filesystem counters. Field naming matches what
/// the dashboard LiveViews consume so the Elixir side can replace its
/// three pollers with a single PubSub subscriber.
pub(crate) fn compose_dashboard_state_v1(
    live: &LiveMetrics<'_>,
    pg_state: Value,
    lifecycle: Value,
    disk_files: i64,
    eligible_files: i64,
) -> Value {
    let pending_chunks = pg_state
        .get("totals")
        .and_then(|t| t.get("pending"))
        .and_then(|p| p.as_i64())
        .unwrap_or(0);
    let (pipeline_status, blocked_reason) = compute_pipeline_status(
        live.runtime_mode,
        live.runtime_idle,
        pending_chunks,
        live.embedder_init_error,
    );
    json!({
        "event": "dashboard_state_v1",
        "ts_ms": live.ts_ms,
        "runtime": {
            "build_id": live.build_id,
            "install_generation": live.install_generation,
            "runtime_mode": live.runtime_mode,
            "instance_kind": live.instance_kind,
            "degraded_reason": live.degraded_reason,
            "runtime_idle": live.runtime_idle,
            "pipeline_status": pipeline_status,
            "blocked_reason": blocked_reason,
        },
        "embedder": {
            "requested": live.embedder_requested,
            "effective": live.embedder_effective,
            "init_error": live.embedder_init_error,
            "last_lane": live.last_consumed_batch_lane,
            // DEC-AXO-901626 — observable Pipeline B compute verdict.
            "compute": live.embedder_compute,
            "compute_source": live.embedder_compute_source,
        },
        "telemetry": {
            "chunk_embeddings_per_second": live.chunk_embeddings_per_second,
            "vector_chunks_embedded_cumulative": live.vector_chunks_embedded_cumulative,
            "graph_workers_active_current": live.graph_workers_active,
            "graph_workers_started_total": live.graph_workers_started,
            "ingress_buffered_entries": live.ingress_buffered_entries,
            "ingress_hot_entries": live.ingress_hot_entries,
            "ready_queue_chunks_current": live.ready_queue_chunks_current,
            "ready_queue_chunks_small": live.ready_queue_chunks_small,
            "ready_queue_chunks_medium": live.ready_queue_chunks_medium,
            "ready_queue_chunks_large": live.ready_queue_chunks_large,
            "homogeneous_batches_total": live.homogeneous_batches_total,
            "mixed_fallback_batches_total": live.mixed_fallback_batches_total,
            "service_pressure": live.service_pressure,
            "scheduler": live.scheduler_state,
        },
        "filesystem": {
            "disk_files": disk_files,
            "eligible_files": eligible_files,
        },
        "lifecycle": lifecycle,
        "totals": pg_state.get("totals").cloned().unwrap_or_else(|| json!({})),
        "per_project": pg_state.get("per_project").cloned().unwrap_or_else(|| json!([])),
        "runtime_config": pg_state.get("runtime_config").cloned().unwrap_or_else(|| json!({})),
    })
}

/// REQ-AXO-901806 — Convenience composer used by the 1 Hz telemetry
/// loop. Reads PG composite (`dashboard_state_full(5)`) + lifecycle +
/// filesystem counters, assembles the `dashboard_state_v1` event,
/// publishes to the slot for the HTTP endpoint, and emits on the
/// broadcast channel for live subscribers.
///
/// All work is synchronous. Failures degrade gracefully: PG hiccups
/// emit the event with empty blocks + a warning log ; a broadcast send
/// error is silently ignored (no live subscribers).
pub(crate) fn compose_publish_and_emit(
    store: &Arc<GraphStore>,
    results_tx: &broadcast::Sender<String>,
    live: LiveMetrics<'_>,
) {
    let pg_state = read_dashboard_state_full(store);
    let lifecycle = compose_lifecycle_block(store);
    let (disk_files, eligible_files) = cached_fs_counters();

    let state = compose_dashboard_state_v1(&live, pg_state, lifecycle, disk_files, eligible_files);

    // 1) Update the in-memory slot for HTTP /dashboard/state.
    publish_dashboard_state(state.clone());

    // 2) Broadcast to live socket subscribers (dashboard via BridgeClient).
    if let Ok(message) = serde_json::to_string(&state) {
        let _ = results_tx.send(message + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Slice 3 SOTA — pipeline_status taxonomy locks. These tests pin
    // the contract between `runtime_mode + pending + init_error` and
    // the surfaced `pipeline_status` + `blocked_reason`. Drift here
    // surfaces immediately to the operator via dashboard banner.

    #[test]
    fn pipeline_status_reports_blocked_when_brain_only_and_pending_gt_zero() {
        // The canonical operator pain point session 63 : 17 252 chunks
        // pending on AXO with brain_only and no indexer paired.
        let (status, reason) = compute_pipeline_status("brain_only", true, 17_252, None);
        assert_eq!(status, "indexer_idle_blocked");
        assert_eq!(reason, Some("no_indexer_paired"));
    }

    #[test]
    fn pipeline_status_brain_only_with_no_pending_is_trivial_idle() {
        let (status, reason) = compute_pipeline_status("brain_only", true, 0, None);
        assert_eq!(status, "brain_only");
        assert_eq!(reason, None);
    }

    #[test]
    fn pipeline_status_indexer_active_when_runtime_not_idle() {
        let (status, reason) = compute_pipeline_status("indexer_full", false, 1_000, None);
        assert_eq!(status, "indexer_active");
        assert_eq!(reason, None);
    }

    #[test]
    fn pipeline_status_indexer_idle_done_when_pending_zero() {
        let (status, reason) = compute_pipeline_status("indexer_full", true, 0, None);
        assert_eq!(status, "indexer_idle_done");
        assert_eq!(reason, None);
    }

    #[test]
    fn pipeline_status_indexer_idle_blocked_when_idle_with_pending() {
        let (status, reason) = compute_pipeline_status("indexer_full", true, 17_252, None);
        assert_eq!(status, "indexer_idle_blocked");
        assert_eq!(reason, Some("demand_pull_b_stalled"));
    }

    #[test]
    fn pipeline_status_gpu_unavailable_wins_over_other_diagnoses() {
        let (status, reason) =
            compute_pipeline_status("indexer_full", false, 500, Some("CUDA error 35"));
        assert_eq!(status, "indexer_idle_blocked");
        assert_eq!(reason, Some("gpu_unavailable"));
    }

    /// Reproduces the session-61 bug : the FFI SQL gateway returns
    /// jsonb columns as JSON-encoded strings inside `[[...]]`. A naive
    /// extractor would hand a `Value::String` to the downstream
    /// `.get("totals")` accessor — which returns `None` and falls back
    /// to empty. The fix is a second `serde_json::from_str` decode when
    /// the cell turns out to be a string.
    #[test]
    fn extract_first_cell_decodes_jsonb_string_payload() {
        let raw = r#"[["{\"chunks\":74173,\"coverage_pct\":107.96,\"embedded\":80080}"]]"#;
        let v = extract_first_cell(raw);
        assert_eq!(v["chunks"], 74173);
        assert!((v["coverage_pct"].as_f64().unwrap() - 107.96).abs() < 1e-9);
        assert_eq!(v["embedded"], 80080);
    }

    #[test]
    fn extract_first_cell_passes_native_object_through() {
        let raw = r#"[[{"files":14855}]]"#;
        let v = extract_first_cell(raw);
        assert_eq!(v["files"], 14855);
    }

    #[test]
    fn extract_first_cell_returns_null_on_malformed_shape() {
        assert_eq!(extract_first_cell("[]"), Value::Null);
        assert_eq!(extract_first_cell("[[]]"), Value::Null);
        assert_eq!(extract_first_cell("not json"), Value::Null);
    }

    /// Round-trip : feed a synthetic PG composite + live metrics into
    /// the composer and confirm every dashboard LiveView accessor path
    /// resolves to its expected source. Locks the JSON envelope shape.
    #[test]
    fn compose_dashboard_state_v1_envelope_shape() {
        let pg_state = json!({
            "totals": {
                "files": 14855,
                "chunks": 74173,
                "embedded": 80080,
                "coverage_pct": 107.96,
                "pending": 0,
                "orphan_embeddings": 6418,
                "projects": 25,
                "symbols": 68708,
                "edges": 207154
            },
            "per_project": [
                {"project_code": "AXO", "chunks": 6895, "embedded": 6895, "coverage_pct": 100.0}
            ],
            "runtime_config": {
                "pipeline_a": {"a1_workers": 4, "a2_workers": 8, "a3_workers": 2},
                "pipeline_b": {"b2_workers": 1},
                "notify_channel": "chunk_pending_embed"
            }
        });
        let lifecycle = json!({"phase": "ready", "source": "indexer_heartbeat"});
        let live = LiveMetrics {
            ts_ms: 1_780_087_724_366,
            build_id: "v0.8.0",
            install_generation: "workspace",
            runtime_mode: "brain_only",
            instance_kind: "dev",
            degraded_reason: None,
            embedder_requested: "cpu",
            embedder_effective: "cpu",
            embedder_init_error: None,
            embedder_compute: "CPU",
            embedder_compute_source: "unknown",
            last_consumed_batch_lane: "unknown",
            chunk_embeddings_per_second: 0.0,
            vector_chunks_embedded_cumulative: 0,
            graph_workers_active: 0,
            graph_workers_started: 0,
            ingress_buffered_entries: 0,
            ingress_hot_entries: 0,
            ready_queue_chunks_current: 0,
            ready_queue_chunks_small: 0,
            ready_queue_chunks_medium: 0,
            ready_queue_chunks_large: 0,
            homogeneous_batches_total: 0,
            mixed_fallback_batches_total: 0,
            service_pressure: "healthy",
            scheduler_state: "fast",
            runtime_idle: true,
        };

        let event = compose_dashboard_state_v1(&live, pg_state, lifecycle, 1_987_358, 23_894);

        // Envelope contract — version + timestamp.
        assert_eq!(event["event"], "dashboard_state_v1");
        assert_eq!(event["ts_ms"], 1_780_087_724_366u64);
        // Runtime + embedder identity.
        assert_eq!(event["runtime"]["build_id"], "v0.8.0");
        assert_eq!(event["runtime"]["instance_kind"], "dev");
        assert_eq!(event["runtime"]["runtime_idle"], true);
        assert_eq!(event["embedder"]["effective"], "cpu");
        // Live telemetry passes through verbatim.
        assert_eq!(event["telemetry"]["scheduler"], "fast");
        assert_eq!(event["telemetry"]["service_pressure"], "healthy");
        // Filesystem counters from cached scan.
        assert_eq!(event["filesystem"]["disk_files"], 1_987_358);
        assert_eq!(event["filesystem"]["eligible_files"], 23_894);
        // Lifecycle block embedded as-is.
        assert_eq!(event["lifecycle"]["phase"], "ready");
        // PG composite blocks extracted from pg_state (covers the F5
        // dashboard pipeline_live worker-config table + funnel).
        assert_eq!(event["totals"]["files"], 14855);
        assert_eq!(event["totals"]["chunks"], 74173);
        assert_eq!(event["per_project"][0]["project_code"], "AXO");
        assert_eq!(event["runtime_config"]["pipeline_a"]["a2_workers"], 8);
        assert_eq!(
            event["runtime_config"]["notify_channel"],
            "chunk_pending_embed"
        );
    }

    /// Guard : when `read_dashboard_state_full` would return `Null`
    /// (transient PG hiccup), the composer must still produce a
    /// well-formed envelope with empty PG blocks rather than a missing
    /// key (which would break the LiveView's `get_in(state, [...])`
    /// pattern).
    #[test]
    fn compose_dashboard_state_v1_falls_back_gracefully_on_null_pg_state() {
        let live = LiveMetrics {
            ts_ms: 0,
            build_id: "",
            install_generation: "",
            runtime_mode: "",
            instance_kind: "",
            degraded_reason: None,
            embedder_requested: "",
            embedder_effective: "",
            embedder_init_error: None,
            embedder_compute: "CPU",
            embedder_compute_source: "unknown",
            last_consumed_batch_lane: "",
            chunk_embeddings_per_second: 0.0,
            vector_chunks_embedded_cumulative: 0,
            graph_workers_active: 0,
            graph_workers_started: 0,
            ingress_buffered_entries: 0,
            ingress_hot_entries: 0,
            ready_queue_chunks_current: 0,
            ready_queue_chunks_small: 0,
            ready_queue_chunks_medium: 0,
            ready_queue_chunks_large: 0,
            homogeneous_batches_total: 0,
            mixed_fallback_batches_total: 0,
            service_pressure: "",
            scheduler_state: "",
            runtime_idle: false,
        };
        let event = compose_dashboard_state_v1(&live, Value::Null, json!({}), -1, -1);
        assert!(event.get("totals").is_some());
        assert!(event.get("per_project").is_some());
        assert!(event.get("runtime_config").is_some());
        assert!(event["totals"].as_object().unwrap().is_empty());
        assert!(event["per_project"].as_array().unwrap().is_empty());
    }
}
