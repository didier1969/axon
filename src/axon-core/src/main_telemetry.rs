// Copyright (c) Didier Stadelmann. All rights reserved.

use std::sync::Arc;
use std::time::Duration;
use std::{fs, path::PathBuf};

use crate::main_background;
use axon_core::bridge::BridgeEvent;
use axon_core::embedder::{
    current_embedding_provider_diagnostics, embedder_provider_fallback_reason,
    embedding_lane_config_from_env,
};
use axon_core::graph::GraphStore;
use axon_core::ingress_buffer::SharedIngressBuffer;
use axon_core::queue::QueueStore;
use axon_core::runtime_mode::AxonRuntimeMode;
use axon_core::runtime_topology::{current_runtime_process_role, AxonProcessRole};
use axon_core::scanner;
use axon_core::service_guard;
// REQ-AXO-901653 slice-5c — `crossbeam_channel::Sender` removed (was only
// imported for DbWriteTask sender) ; `tokio::sync::broadcast::Sender` is
// re-imported per-signature via the `broadcast::Sender` alias below.
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info, warn};

fn freshness_state_for_feed(runtime_truth_feed: &axon_core::bridge::RuntimeTruthFeed) -> String {
    if runtime_truth_feed.stale {
        "stale".to_string()
    } else if runtime_truth_feed.degraded_reason.is_some() {
        "degraded".to_string()
    } else {
        "fresh".to_string()
    }
}

fn split_run_root(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
    let mut path = PathBuf::from(project_root);
    if instance_kind == "dev" {
        path.push(".axon-dev");
    } else {
        path.push(".axon");
    }
    path.push(format!("run-{role_slug}"));
    path
}

fn split_runtime_heartbeat_path(
    project_root: &str,
    instance_kind: &str,
    role_slug: &str,
) -> PathBuf {
    split_run_root(project_root, instance_kind, role_slug).join("runtime-heartbeat.json")
}

fn projected_indexer_runtime_from_heartbeat() -> Option<serde_json::Value> {
    if !matches!(current_runtime_process_role(), AxonProcessRole::Brain) {
        return None;
    }

    // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE.
    let instance_kind =
        crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "dev");
    let project_root = std::env::var("AXON_PROJECT_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });
    let heartbeat_path = split_runtime_heartbeat_path(&project_root, &instance_kind, "indexer");
    let payload = fs::read_to_string(&heartbeat_path).ok()?;
    let payload: serde_json::Value = serde_json::from_str(&payload).ok()?;
    let runtime_truth_feed: axon_core::bridge::RuntimeTruthFeed = payload
        .get("runtime_truth_feed")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())?;
    let telemetry = payload
        .get("runtime_telemetry")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some(serde_json::json!({
        "available": !telemetry.is_null(),
        "telemetry_source": "indexer_peer_heartbeat",
        "process_role": payload
            .get("process_role")
            .and_then(|value| value.as_str())
            .unwrap_or("indexer"),
        "runtime_mode": payload
            .get("runtime_mode")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown"),
        "runtime_identity": payload
            .get("runtime_identity")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown-runtime"),
        "freshness_state": freshness_state_for_feed(&runtime_truth_feed),
        "observed_age_ms": runtime_truth_feed.observed_age_ms,
        "degraded_reason": runtime_truth_feed.degraded_reason,
        "telemetry": telemetry,
    }))
}

fn write_runtime_heartbeat_export(
    runtime_mode: AxonRuntimeMode,
    runtime_truth_feed: &axon_core::bridge::RuntimeTruthFeed,
    runtime_snapshot: &main_background::RuntimeTelemetrySnapshot,
) {
    let Ok(run_root) = std::env::var("AXON_RUN_ROOT") else {
        warn!("Runtime heartbeat export skipped because AXON_RUN_ROOT is unset.");
        return;
    };

    let release_version = std::env::var("AXON_RELEASE_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    let build_id =
        std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    let install_generation =
        std::env::var("AXON_INSTALL_GENERATION").unwrap_or_else(|_| "workspace".to_string());
    let process_role = current_runtime_process_role().as_str();
    let runtime_identity = std::env::var("AXON_RUNTIME_IDENTITY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown-runtime".to_string());
    let embedder_provider = current_embedding_provider_diagnostics();
    // REQ-AXO-901836 — publish the indexer's effective lane parameters so
    // the brain composer can surface paired indexer's worker counts /
    // batch sizes instead of always returning its own local (brain_only,
    // vector_workers=0) values. Source: same `embedding_lane_config_from_env`
    // call as the indexer's own status reporter, so the heartbeat reflects
    // exactly what this indexer instance is configured to run.
    let indexer_lane_config = embedding_lane_config_from_env();
    let indexer_lane_parameters = serde_json::json!({
        "vector_workers": indexer_lane_config.vector_workers,
        "graph_workers": indexer_lane_config.graph_workers,
        "query_workers": indexer_lane_config.query_workers,
        "chunk_batch_size": indexer_lane_config.chunk_batch_size,
        "file_vectorization_batch_size": indexer_lane_config.file_vectorization_batch_size,
        "graph_batch_size": indexer_lane_config.graph_batch_size,
    });
    // REQ-AXO-184 #4 / REQ-AXO-185 #2: surface silent embedder fallback in
    // heartbeat's degraded_reason so operators see "embedder_provider_fallback"
    // within one tick instead of after a full probe window.
    let embedder_fallback_reason = embedder_provider_fallback_reason(
        &embedder_provider.provider_requested,
        &embedder_provider.provider_effective,
        embedder_provider.provider_init_error.as_deref(),
    );
    let merged_degraded_reason = match (
        runtime_truth_feed.degraded_reason.as_deref(),
        embedder_fallback_reason.as_deref(),
    ) {
        (Some(existing), Some(fb)) => Some(format!("{}; {}", existing, fb)),
        (None, Some(fb)) => Some(fb.to_string()),
        (Some(existing), None) => Some(existing.to_string()),
        (None, None) => None,
    };
    let payload = serde_json::json!({
        "process_role": process_role,
        "runtime_mode": runtime_mode.as_str(),
        "runtime_identity": runtime_identity,
        "release_version": release_version,
        "build_id": build_id,
        "install_generation": install_generation,
        "last_heartbeat_at_ms": runtime_truth_feed.last_heartbeat_at_ms,
        "last_good_payload_at_ms": runtime_truth_feed.last_good_payload_at_ms,
        "observed_age_ms": runtime_truth_feed.observed_age_ms,
        "stale_after_ms": runtime_truth_feed.stale_after_ms,
        "stale": runtime_truth_feed.stale,
        "degraded_reason": merged_degraded_reason,
        "runtime_truth_feed": runtime_truth_feed,
        // DEC-AXO-901626 — the raced `embedder_provider` self-report block is
        // gone. The effective provider is derived observably by the brain
        // composer (indexer pid + nvidia-smi). `degraded_reason` above still
        // flags a GPU→CPU fallback as a fail-loud signal.
        "lane_parameters": indexer_lane_parameters,
        "runtime_telemetry": {
            "ingress_enabled": runtime_snapshot.ingress_enabled,
            "ingress_buffered_entries": runtime_snapshot.ingress_buffered_entries,
            "ingress_hot_entries": runtime_snapshot.ingress_hot_entries,
            "ingress_scan_entries": runtime_snapshot.ingress_scan_entries,
            "ingress_subtree_hints": runtime_snapshot.ingress_subtree_hints,
            "ingress_subtree_hint_in_flight": runtime_snapshot.ingress_subtree_hint_in_flight,
            "ingress_subtree_hint_accepted_total": runtime_snapshot.ingress_subtree_hint_accepted_total,
            "ingress_subtree_hint_blocked_total": runtime_snapshot.ingress_subtree_hint_blocked_total,
            "ingress_subtree_hint_suppressed_total": runtime_snapshot.ingress_subtree_hint_suppressed_total,
            "ingress_flush_count": runtime_snapshot.ingress_flush_count,
            "ingress_last_flush_duration_ms": runtime_snapshot.ingress_last_flush_duration_ms,
            "ingress_last_promoted_count": runtime_snapshot.ingress_last_promoted_count,
            "ingress_promoted_total": runtime_snapshot.ingress_promoted_total,
            "ingress_last_durably_persisted_count": runtime_snapshot.ingress_last_durably_persisted_count,
            "ingress_durably_persisted_total": runtime_snapshot.ingress_durably_persisted_total,
            "ingress_last_excluded_from_pending_count": runtime_snapshot.ingress_last_excluded_from_pending_count,
            "ingress_excluded_from_pending_total": runtime_snapshot.ingress_excluded_from_pending_total,
            "pg_database_bytes": runtime_snapshot.pg_database_bytes,
            "pg_chunkembedding_total_bytes": runtime_snapshot.pg_chunkembedding_total_bytes,
            "pg_wal_bytes": runtime_snapshot.pg_wal_bytes,
            "pg_buffer_hit_ratio": runtime_snapshot.pg_buffer_hit_ratio,
            "vector_chunks_embedded_cumulative": runtime_snapshot.vector_chunks_embedded_cumulative,
            "chunk_embeddings_per_second": runtime_snapshot.chunk_embeddings_per_second,
            "chunk_embeddings_rate_window_ms": runtime_snapshot.chunk_embeddings_rate_window_ms,
            "prepare_inflight_chunks_current": runtime_snapshot.prepare_inflight_chunks_current,
            "ready_queue_chunks_current": runtime_snapshot.ready_queue_chunks_current,
            "ready_queue_chunks_small": runtime_snapshot.ready_queue_chunks_small,
            "ready_queue_chunks_medium": runtime_snapshot.ready_queue_chunks_medium,
            "ready_queue_chunks_large": runtime_snapshot.ready_queue_chunks_large,
            "ready_batches_small": runtime_snapshot.ready_batches_small,
            "ready_batches_medium": runtime_snapshot.ready_batches_medium,
            "ready_batches_large": runtime_snapshot.ready_batches_large,
            "mixed_fallback_batches_total": runtime_snapshot.mixed_fallback_batches_total,
            "homogeneous_batches_total": runtime_snapshot.homogeneous_batches_total,
            "last_consumed_batch_lane": runtime_snapshot.last_consumed_batch_lane,
            "active_small_max_tokens": runtime_snapshot.active_small_max_tokens,
            "active_medium_max_tokens": runtime_snapshot.active_medium_max_tokens,
            "ready_replenishment_deficit_current": runtime_snapshot.ready_replenishment_deficit_current,
            "oldest_ready_batch_age_ms_current": runtime_snapshot.oldest_ready_batch_age_ms_current,
            "graph_workers_started_total": runtime_snapshot.graph_workers_started_total,
            "graph_workers_active_current": runtime_snapshot.graph_workers_active_current,
            "graph_worker_heartbeat_at_ms": runtime_snapshot.graph_worker_heartbeat_at_ms,
            "claim_mode": runtime_snapshot.claim_mode,
            "service_pressure": runtime_snapshot.service_pressure,
            "utility_first_scheduler_state": runtime_snapshot.utility_first_scheduler_state,
            "utility_first_scheduler_reason": runtime_snapshot.utility_first_scheduler_reason,
            "semantic_underfeed": runtime_snapshot.semantic_underfeed,
        },
    });
    let runtime_heartbeat_path = std::path::Path::new(&run_root).join("runtime-heartbeat.json");
    if let Err(err) = std::fs::create_dir_all(&run_root) {
        warn!(
            "Runtime heartbeat export could not create run root {}: {:?}",
            run_root, err
        );
        return;
    }
    let existed_before = runtime_heartbeat_path.exists();
    if let Err(err) = std::fs::write(
        &runtime_heartbeat_path,
        serde_json::to_vec_pretty(&payload).unwrap_or_else(|_| b"{}".to_vec()),
    ) {
        warn!(
            "Runtime heartbeat export write failed for {}: {:?}",
            runtime_heartbeat_path.display(),
            err
        );
        return;
    }
    if !existed_before {
        info!(
            "Runtime heartbeat export initialized at {} (mode={}, stale={}).",
            runtime_heartbeat_path.display(),
            runtime_mode.as_str(),
            runtime_truth_feed.stale
        );
    }
}

pub(crate) fn spawn_runtime_telemetry(
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
    ingress_buffer: SharedIngressBuffer,
    results_tx: broadcast::Sender<String>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;
            let snapshot =
                main_background::runtime_telemetry_snapshot(&store, &queue, &ingress_buffer);
            let runtime_mode = AxonRuntimeMode::from_env();
            let runtime_truth_feed = if runtime_mode.ingestion_enabled() {
                service_guard::record_runtime_truth_bridge_dispatch(None)
            } else {
                service_guard::current_runtime_truth_feed()
            };
            write_runtime_heartbeat_export(runtime_mode, &runtime_truth_feed, &snapshot);
            let telemetry_source = "local_runtime".to_string();
            let telemetry_process_role = current_runtime_process_role().as_str().to_string();
            let telemetry_freshness_state = freshness_state_for_feed(&runtime_truth_feed);
            let telemetry_observed_age_ms = runtime_truth_feed.observed_age_ms;
            let telemetry_degraded_reason = runtime_truth_feed.degraded_reason.clone();
            let projected_indexer_runtime = projected_indexer_runtime_from_heartbeat();
            // REQ-AXO-901806 — clone owned strings before they're moved
            // into BridgeEvent so the dashboard composer below can still
            // read them. Cheap (small strings, ~100 bytes total).
            let dashboard_last_lane = snapshot.last_consumed_batch_lane.clone();
            let dashboard_service_pressure = snapshot.service_pressure.clone();
            let dashboard_claim_mode = snapshot.claim_mode.clone();
            let event = BridgeEvent::RuntimeTelemetry {
                telemetry_source,
                telemetry_process_role,
                telemetry_freshness_state,
                telemetry_observed_age_ms,
                telemetry_degraded_reason,
                budget_bytes: snapshot.budget_bytes,
                reserved_bytes: snapshot.reserved_bytes,
                exhaustion_ratio: snapshot.exhaustion_ratio,
                reserved_task_count: snapshot.reserved_task_count,
                anonymous_trace_reserved_tasks: snapshot.anonymous_trace_reserved_tasks,
                anonymous_trace_admissions_total: snapshot.anonymous_trace_admissions_total,
                reservation_release_misses_total: snapshot.reservation_release_misses_total,
                queue_depth: snapshot.queue_depth,
                claim_mode: snapshot.claim_mode,
                service_pressure: snapshot.service_pressure,
                interactive_priority_active: snapshot.interactive_priority_active,
                interactive_priority_level: snapshot.interactive_priority_level,
                interactive_requests_in_flight: snapshot.interactive_requests_in_flight,
                oversized_refusals_total: snapshot.oversized_refusals_total,
                degraded_mode_entries_total: snapshot.degraded_mode_entries_total,
                background_launches_suppressed_total: snapshot.background_launches_suppressed_total,
                vectorization_suppressed_due_to_interactive: snapshot
                    .vectorization_suppressed_due_to_interactive,
                vectorization_interrupted_due_to_interactive: snapshot
                    .vectorization_interrupted_due_to_interactive,
                vectorization_requeued_for_interactive: snapshot
                    .vectorization_requeued_for_interactive,
                vectorization_resumed_after_interactive: snapshot
                    .vectorization_resumed_after_interactive,
                projection_suppressed_due_to_interactive: snapshot
                    .projection_suppressed_due_to_interactive,
                guard_hits: snapshot.guard_hits,
                guard_misses: snapshot.guard_misses,
                guard_bypassed_total: snapshot.guard_bypassed_total,
                guard_hydrated_entries: snapshot.guard_hydrated_entries,
                guard_hydration_duration_ms: snapshot.guard_hydration_duration_ms,
                ingress_enabled: snapshot.ingress_enabled,
                ingress_buffered_entries: snapshot.ingress_buffered_entries,
                ingress_subtree_hints: snapshot.ingress_subtree_hints,
                ingress_subtree_hint_in_flight: snapshot.ingress_subtree_hint_in_flight,
                ingress_subtree_hint_accepted_total: snapshot.ingress_subtree_hint_accepted_total,
                ingress_subtree_hint_blocked_total: snapshot.ingress_subtree_hint_blocked_total,
                ingress_subtree_hint_suppressed_total: snapshot
                    .ingress_subtree_hint_suppressed_total,
                ingress_subtree_hint_productive_total: snapshot
                    .ingress_subtree_hint_productive_total,
                ingress_subtree_hint_unproductive_total: snapshot
                    .ingress_subtree_hint_unproductive_total,
                ingress_subtree_hint_dropped_total: snapshot.ingress_subtree_hint_dropped_total,
                ingress_collapsed_total: snapshot.ingress_collapsed_total,
                ingress_flush_count: snapshot.ingress_flush_count,
                ingress_last_flush_duration_ms: snapshot.ingress_last_flush_duration_ms,
                ingress_last_promoted_count: snapshot.ingress_last_promoted_count,
                ingress_promoted_total: snapshot.ingress_promoted_total,
                ingress_last_durably_persisted_count: snapshot.ingress_last_durably_persisted_count,
                ingress_durably_persisted_total: snapshot.ingress_durably_persisted_total,
                ingress_last_excluded_from_pending_count: snapshot
                    .ingress_last_excluded_from_pending_count,
                ingress_excluded_from_pending_total: snapshot.ingress_excluded_from_pending_total,
                memory_trim_attempts_total: snapshot.memory_trim_attempts_total,
                memory_trim_successes_total: snapshot.memory_trim_successes_total,
                cpu_load: snapshot.cpu_load,
                ram_load: snapshot.ram_load,
                io_wait: snapshot.io_wait,
                host_state: snapshot.host_state,
                host_guidance_slots: snapshot.host_guidance_slots,
                rss_bytes: snapshot.rss_bytes,
                rss_anon_bytes: snapshot.rss_anon_bytes,
                rss_file_bytes: snapshot.rss_file_bytes,
                rss_shmem_bytes: snapshot.rss_shmem_bytes,
                pg_database_bytes: snapshot.pg_database_bytes,
                pg_chunkembedding_total_bytes: snapshot.pg_chunkembedding_total_bytes,
                pg_wal_bytes: snapshot.pg_wal_bytes,
                pg_buffer_hit_ratio: snapshot.pg_buffer_hit_ratio,
                vector_chunks_embedded_cumulative: snapshot.vector_chunks_embedded_cumulative,
                chunk_embeddings_per_second: snapshot.chunk_embeddings_per_second,
                chunk_embeddings_rate_window_ms: snapshot.chunk_embeddings_rate_window_ms,
                prepare_inflight_chunks_current: snapshot.prepare_inflight_chunks_current,
                ready_queue_chunks_current: snapshot.ready_queue_chunks_current,
                ready_queue_chunks_small: snapshot.ready_queue_chunks_small,
                ready_queue_chunks_medium: snapshot.ready_queue_chunks_medium,
                ready_queue_chunks_large: snapshot.ready_queue_chunks_large,
                ready_batches_small: snapshot.ready_batches_small,
                ready_batches_medium: snapshot.ready_batches_medium,
                ready_batches_large: snapshot.ready_batches_large,
                mixed_fallback_batches_total: snapshot.mixed_fallback_batches_total,
                homogeneous_batches_total: snapshot.homogeneous_batches_total,
                last_consumed_batch_lane: snapshot.last_consumed_batch_lane,
                active_small_max_tokens: snapshot.active_small_max_tokens,
                active_medium_max_tokens: snapshot.active_medium_max_tokens,
                last_embed_attempt_wall_ms: snapshot.last_embed_attempt_wall_ms,
                avg_embed_attempt_wall_ms: snapshot.avg_embed_attempt_wall_ms,
                max_embed_attempt_wall_ms: snapshot.max_embed_attempt_wall_ms,
                last_embed_gap_ms: snapshot.last_embed_gap_ms,
                avg_embed_gap_ms: snapshot.avg_embed_gap_ms,
                max_embed_gap_ms: snapshot.max_embed_gap_ms,
                graph_workers_started_total: snapshot.graph_workers_started_total,
                graph_workers_active_current: snapshot.graph_workers_active_current,
                graph_worker_heartbeat_at_ms: snapshot.graph_worker_heartbeat_at_ms,
                runtime_truth_feed: runtime_truth_feed.clone(),
                projected_indexer_runtime,
            };

            if let Ok(message) = serde_json::to_string(&event) {
                let _ = results_tx.send(message + "\n");
            }

            // REQ-AXO-901806 — dashboard_state_v1 emit (single-event
            // architecture replacing dashboard's polling triple).
            // PG functions are TTL-cached server-side ; warm-path cost
            // ~18 ms vs ~200 ms cold. Failures degrade gracefully.
            let dashboard_ts_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let dashboard_install_generation = std::env::var("AXON_INSTALL_GENERATION")
                .unwrap_or_else(|_| "workspace".to_string());
            let dashboard_instance_kind = std::env::var("AXON_INSTANCE_KIND")
                .unwrap_or_else(|_| "unknown".to_string());
            let dashboard_embedder = crate::embedder::current_embedding_provider_diagnostics();
            let dashboard_build_id = std::env::var("AXON_BUILD_ID")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
            // DEC-AXO-901626 — observable Pipeline B compute for the dashboard
            // is READ from the indexer's PG heartbeat (the indexer self-observes
            // and publishes the verdict). The brain does no nvidia-smi here.
            // Brain = CPU and Pipeline A = CPU are rendered as constants
            // dashboard-side (architectural invariants).
            let dashboard_heartbeat = store
                .latest_lifecycle_heartbeat("indexer")
                .ok()
                .flatten()
                .filter(|row| (dashboard_ts_ms as i64 - row.heartbeat_ms).max(0) <= 30_000);
            let dashboard_compute = dashboard_heartbeat
                .as_ref()
                .and_then(|row| row.compute.as_deref())
                .unwrap_or("CPU");
            let dashboard_compute_source = dashboard_heartbeat
                .as_ref()
                .and_then(|row| row.compute_source.as_deref())
                .unwrap_or("unknown");
            // Effective provider label coherent with the observed compute:
            // the brain-local diagnostics slot would say "cpu" (the brain
            // never embeds), so derive the label from the observed verdict
            // instead — otherwise the dashboard would resurface the old lie.
            let dashboard_effective_label = if dashboard_compute == "GPU" {
                if dashboard_embedder
                    .provider_requested
                    .eq_ignore_ascii_case("tensorrt")
                {
                    "tensorrt"
                } else {
                    "cuda"
                }
            } else {
                "cpu"
            };
            crate::dashboard_state::compose_publish_and_emit(
                &store,
                &results_tx,
                crate::dashboard_state::LiveMetrics {
                    ts_ms: dashboard_ts_ms,
                    build_id: &dashboard_build_id,
                    install_generation: &dashboard_install_generation,
                    runtime_mode: runtime_mode.as_str(),
                    instance_kind: &dashboard_instance_kind,
                    degraded_reason: runtime_truth_feed.degraded_reason.as_deref(),
                    embedder_requested: &dashboard_embedder.provider_requested,
                    embedder_effective: dashboard_effective_label,
                    embedder_init_error: dashboard_embedder.provider_init_error.as_deref(),
                    embedder_compute: dashboard_compute,
                    embedder_compute_source: dashboard_compute_source,
                    last_consumed_batch_lane: dashboard_last_lane.as_str(),
                    chunk_embeddings_per_second: snapshot.chunk_embeddings_per_second,
                    vector_chunks_embedded_cumulative: snapshot.vector_chunks_embedded_cumulative,
                    graph_workers_active: snapshot.graph_workers_active_current,
                    graph_workers_started: snapshot.graph_workers_started_total,
                    ingress_buffered_entries: snapshot.ingress_buffered_entries as u64,
                    ingress_hot_entries: snapshot.ingress_hot_entries as u64,
                    ready_queue_chunks_current: snapshot.ready_queue_chunks_current,
                    ready_queue_chunks_small: snapshot.ready_queue_chunks_small,
                    ready_queue_chunks_medium: snapshot.ready_queue_chunks_medium,
                    ready_queue_chunks_large: snapshot.ready_queue_chunks_large,
                    homogeneous_batches_total: snapshot.homogeneous_batches_total,
                    mixed_fallback_batches_total: snapshot.mixed_fallback_batches_total,
                    service_pressure: dashboard_service_pressure.as_str(),
                    scheduler_state: dashboard_claim_mode.as_str(),
                    runtime_idle: snapshot.queue_depth == 0 && snapshot.exhaustion_ratio < 0.1,
                },
            );
        }
    });
}

// REQ-AXO-901653 slice-5c — `db_sender` parameter (Sender<worker::DbWriteTask>)
// removed from telemetry. Worker.rs + DbWriteTask + EXECUTE_CYPHER command path
// were the v1 writer-actor bridge ; pipeline_v2 (REQ-AXO-289) writes through
// GraphStore directly. EXECUTE_CYPHER + PULL_PENDING command handlers deleted
// — they had no production callers post v1 retirement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_telemetry_connection(
    socket: UnixStream,
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
    projects_root: String,
    boot_id_lock: Arc<Mutex<String>>,
    mut results_rx: broadcast::Receiver<String>,
    results_tx: broadcast::Sender<String>,
) {
    tokio::spawn(async move {
        let (reader, mut writer) = socket.into_split();
        let mut buf_reader = BufReader::new(reader);

        tokio::spawn(async move {
            loop {
                match results_rx.recv().await {
                    Ok(msg) => {
                        if writer.write_all(msg.as_bytes()).await.is_err() {
                            error!("Socket Write Error: Closing feedback loop.");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        warn!("⚠️ Telemetry Lagged: skipped {} messages.", count);
                        continue;
                    }
                    Err(_) => break,
                }
            }
        });

        let mut line = String::new();
        while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
            if bytes_read == 0 {
                break;
            }
            let command = line.trim();
            handle_telemetry_command(
                command,
                store.clone(),
                queue.clone(),
                projects_root.clone(),
                boot_id_lock.clone(),
                results_tx.clone(),
            )
            .await;
            line.clear();
        }
    });
}

pub(crate) async fn handle_telemetry_command(
    command: &str,
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
    projects_root: String,
    boot_id_lock: Arc<Mutex<String>>,
    results_tx: broadcast::Sender<String>,
) {
    if command.is_empty() {
        return;
    }

    debug!("Telemetry: Received command [{}]", command);

    if let Some(stripped) = command.strip_prefix("RAW_QUERY ") {
        let query = stripped.trim().to_string();
        tokio::spawn(async move {
            match store.execute_raw_sql_gateway(&query) {
                Ok(res) => {
                    let _ = results_tx.send(res + "\n");
                }
                Err(e) => {
                    let _ = results_tx.send(format!("{{\"error\": \"{:?}\"}}\n", e));
                }
            }
        });
        return;
    }

    if let Some(payload) = command.strip_prefix("SESSION_INIT ") {
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(payload) {
            let new_id = data["boot_id"].as_str().unwrap_or("unknown").to_string();
            let mut active_id = boot_id_lock.lock().await;
            if new_id != *active_id {
                info!(
                    "🔄 New Elixir Session: {}. Maintaining current pipeline state.",
                    new_id
                );
                *active_id = new_id;
            }
        }
        return;
    }

    if let Some(payload) = command.strip_prefix("PARSE_BATCH ") {
        if let Ok(batch_data) = serde_json::from_str::<serde_json::Value>(payload) {
            let batch_id = batch_data
                .get("batch_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let files_value = batch_data.get("files").unwrap_or(&batch_data);

            if let Some(files) = files_value.as_array() {
                for file_data in files {
                    let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                    let trace_id = file_data["trace_id"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string();
                    let t0 = file_data["t0"].as_i64().unwrap_or(0);
                    let t1 = file_data["t1"].as_i64().unwrap_or(0);
                    let mtime = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|sys_time| {
                            sys_time
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs() as i64
                        })
                        .unwrap_or(0);
                    let _ = queue.push(&path, mtime, &trace_id, t0, t1, false);
                }
                let ack = serde_json::json!({"event": "BATCH_ACCEPTED", "batch_id": batch_id});
                if let Ok(msg) = serde_json::to_string(&ack) {
                    let _ = results_tx.send(msg + "\n");
                }
            }
        }
        return;
    }

    // REQ-AXO-901653 slice-5c — `PULL_PENDING` command path deleted ; relied
    // on `fetch_pending_batch` (now a no-op stub) and the legacy v1 worker
    // pool. Pipeline_v2 (REQ-AXO-289) streams files directly from
    // `ingress_buffer` ; no pull semantics needed.

    if command == "SCAN_ALL" {
        tokio::spawn(async move {
            scanner::Scanner::new(&projects_root, "").scan(store);
        });
        return;
    }

    if command == "SHUTDOWN" {
        std::process::exit(0);
    }

    // REQ-AXO-094 — BEAM alarm classification. Elixir dashboard
    // pushes raw `:alarm_handler` observations as line-based
    // commands; the Rust side owns the alarm→subsystem mapping so
    // the readiness contract authority (PIL-AXO-001 / REQ-AXO-098)
    // stays in the brain. Unknown alarms are logged but do NOT
    // mutate the registry (defensive: a dashboard bug or malicious
    // payload cannot flap arbitrary subsystems).
    if let Some(payload) = command.strip_prefix("BEAM_ALARM ") {
        handle_beam_alarm(payload);
        return;
    }
}

/// REQ-AXO-094 — parse a BEAM_ALARM payload of the shape
/// `{"alarm": "<name>", "action": "set"|"clear"}` and project it
/// onto `runtime_readiness` per the alarm→subsystem mapping
/// documented in DEC-AXO-062.
pub(crate) fn handle_beam_alarm(payload: &str) {
    use crate::runtime_readiness::{report_subsystem_state, SubsystemState};
    let parsed: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                target = "axon::beam_alarm",
                "BEAM_ALARM payload is not valid JSON: {err}; payload={payload}"
            );
            return;
        }
    };
    let alarm = parsed.get("alarm").and_then(|v| v.as_str()).unwrap_or("");
    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("set");
    let Some((subsystem, degraded_reason)) = beam_alarm_to_subsystem(alarm) else {
        warn!(
            target = "axon::beam_alarm",
            "BEAM_ALARM ignored: unknown alarm `{alarm}` (no canonical subsystem mapping)"
        );
        return;
    };
    let state = match action {
        "clear" => SubsystemState::Ready,
        _ => SubsystemState::Degraded {
            reason: degraded_reason.to_string(),
        },
    };
    info!(
        target = "axon::beam_alarm",
        event = "beam_alarm_projected",
        alarm = alarm,
        action = action,
        subsystem = subsystem.as_str(),
        "REQ-AXO-094: dashboard reported BEAM alarm; readiness updated"
    );
    report_subsystem_state(subsystem, state);
}

/// REQ-AXO-094 / DEC-AXO-062 — canonical mapping of BEAM
/// `:alarm_handler` events to subsystem+reason. Returns None for
/// alarms that have no defined mapping (those are logged but do
/// not mutate the registry).
fn beam_alarm_to_subsystem(
    alarm: &str,
) -> Option<(crate::runtime_readiness::Subsystem, &'static str)> {
    use crate::runtime_readiness::Subsystem;
    match alarm {
        "system_memory_high_watermark" => Some((Subsystem::Dashboard, "memory_pressure")),
        "disk_almost_full" => Some((Subsystem::IstWriter, "disk_almost_full")),
        _ => None,
    }
}

#[cfg(test)]
#[path = "main_telemetry_beam_alarm_tests.rs"]
mod main_telemetry_beam_alarm_tests;
