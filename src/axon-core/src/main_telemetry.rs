// Copyright (c) Didier Stadelmann. All rights reserved.

use std::sync::Arc;
use std::time::Duration;

use crate::main_background;
use axon_core::bridge::BridgeEvent;
use axon_core::graph::GraphStore;
use axon_core::ingress_buffer::SharedIngressBuffer;
use axon_core::queue::QueueStore;
use axon_core::scanner;
use crossbeam_channel::Sender;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info, warn};

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
            let event = BridgeEvent::RuntimeTelemetry {
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
                db_file_bytes: snapshot.db_file_bytes,
                db_wal_bytes: snapshot.db_wal_bytes,
                db_total_bytes: snapshot.db_total_bytes,
                duckdb_memory_bytes: snapshot.duckdb_memory_bytes,
                duckdb_temporary_bytes: snapshot.duckdb_temporary_bytes,
                graph_projection_queue_queued: snapshot.graph_projection_queue_queued,
                graph_projection_queue_inflight: snapshot.graph_projection_queue_inflight,
                graph_projection_queue_depth: snapshot.graph_projection_queue_depth,
                file_vectorization_queue_queued: snapshot.file_vectorization_queue_queued,
                file_vectorization_queue_inflight: snapshot.file_vectorization_queue_inflight,
                file_vectorization_queue_depth: snapshot.file_vectorization_queue_depth,
            };

            if let Ok(message) = serde_json::to_string(&event) {
                let _ = results_tx.send(message + "\n");
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_telemetry_connection(
    socket: UnixStream,
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
    projects_root: String,
    boot_id_lock: Arc<Mutex<String>>,
    db_sender: Sender<axon_core::worker::DbWriteTask>,
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
                db_sender.clone(),
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
    db_sender: Sender<axon_core::worker::DbWriteTask>,
    results_tx: broadcast::Sender<String>,
) {
    if command.is_empty() {
        return;
    }

    debug!("Telemetry: Received command [{}]", command);

    if let Some(stripped) = command.strip_prefix("EXECUTE_CYPHER ") {
        let query = stripped.trim().to_string();
        let _ = db_sender.send(axon_core::worker::DbWriteTask::ExecuteCypher { query });
        return;
    }

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

    if let Some(stripped) = command.strip_prefix("PULL_PENDING ") {
        let count = stripped.trim().parse::<usize>().unwrap_or(10);
        tokio::spawn(async move {
            if let Ok(files) = store.fetch_pending_batch(count) {
                if !files.is_empty() {
                    let response =
                        serde_json::json!({"event": "PENDING_BATCH_READY", "files": files});
                    if let Ok(msg) = serde_json::to_string(&response) {
                        let _ = results_tx.send(msg + "\n");
                    }
                }
            }
        });
        return;
    }

    if command == "SCAN_ALL" {
        tokio::spawn(async move {
            scanner::Scanner::new(&projects_root, "GLOBAL").scan(store);
        });
        return;
    }

    if command == "SHUTDOWN" {
        std::process::exit(0);
    }
}
