// Copyright (c) Didier Stadelmann. All rights reserved.

use std::sync::Arc;
use std::time::Duration;

use crate::main_background;
use axon_core::bridge::BridgeEvent;
use axon_core::graph::GraphStore;
use axon_core::queue::QueueStore;
use axon_core::scanner;
use crossbeam_channel::Sender;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info, warn};

pub(crate) fn spawn_runtime_telemetry(
    queue: Arc<QueueStore>,
    results_tx: broadcast::Sender<String>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;
            let snapshot = main_background::runtime_telemetry_snapshot(&queue);
            let event = BridgeEvent::RuntimeTelemetry {
                budget_bytes: snapshot.budget_bytes,
                reserved_bytes: snapshot.reserved_bytes,
                exhaustion_ratio: snapshot.exhaustion_ratio,
                queue_depth: snapshot.queue_depth,
                claim_mode: snapshot.claim_mode,
                service_pressure: snapshot.service_pressure,
                oversized_refusals_total: snapshot.oversized_refusals_total,
                degraded_mode_entries_total: snapshot.degraded_mode_entries_total,
                guard_hits: snapshot.guard_hits,
                guard_misses: snapshot.guard_misses,
                guard_bypassed_total: snapshot.guard_bypassed_total,
                guard_hydrated_entries: snapshot.guard_hydrated_entries,
                guard_hydration_duration_ms: snapshot.guard_hydration_duration_ms,
                cpu_load: snapshot.cpu_load,
                ram_load: snapshot.ram_load,
                io_wait: snapshot.io_wait,
                host_state: snapshot.host_state,
                host_guidance_slots: snapshot.host_guidance_slots,
            };

            if let Ok(message) = serde_json::to_string(&event) {
                let _ = results_tx.send(message + "\n");
            }
        }
    });
}

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

    if command.starts_with("EXECUTE_CYPHER ") {
        let query = command[15..].trim().to_string();
        let _ = db_sender.send(axon_core::worker::DbWriteTask::ExecuteCypher { query });
        return;
    }

    if command.starts_with("RAW_QUERY ") {
        let query = command[10..].trim().to_string();
        tokio::spawn(async move {
            match store.query_json(&query) {
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

    if command.starts_with("SESSION_INIT ") {
        let payload = &command[13..];
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

    if command.starts_with("PARSE_BATCH ") {
        let payload = &command[12..];
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

    if command.starts_with("PULL_PENDING ") {
        let count = command[13..].trim().parse::<usize>().unwrap_or(10);
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
            scanner::Scanner::new(&projects_root).scan(store);
        });
        return;
    }

    if command == "RESET" {
        std::process::exit(0);
    }
}
