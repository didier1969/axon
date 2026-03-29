// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.

use axon_core::bridge::BridgeEvent;
use axon_core::graph::GraphStore;
use axon_core::mcp::McpServer;
use axon_core::queue::QueueStore;
use axon_core::scanner;
use std::fs;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error, debug, warn};

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // MAESTRIA FIX: Cap blocking threads to prevent thread_local! / ONNX Arena explosion.
        .max_blocking_threads(8)
        .build()
        .unwrap()
        .block_on(async {
            tracing_subscriber::fmt::init();
            let boot_time = chrono::Utc::now().to_rfc3339();

            let projects_root_env = std::env::var("AXON_PROJECTS_ROOT").unwrap_or_else(|_| "/home/dstadel/projects".to_string());
            let projects_root = projects_root_env.leak();
            let db_root = "/home/dstadel/projects/axon/.axon/graph_v2";

            info!("Starting Axon Core v2.2 (Nexus Seal - Zero-Sleep Edition)");
            info!("Engine Boot Time: {}", boot_time);

            // Initialize KuzuDB (No RwLock needed: MVCC Snapshot Isolation handles concurrency)
            let graph_store = match GraphStore::new(db_root) {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    error!("Fatal Error initializing LadybugDB: {:?}", e);
                    return Err(e);
                }
            };

            // Initialize In-Memory Bounded Queue (Max 200,000 tasks to buffer ingestion)
            let queue_store = Arc::new(QueueStore::new(200000));
            let tel_socket_path = "/tmp/axon-telemetry.sock";
            let mcp_socket_path = "/tmp/axon-mcp.sock";
            
            if std::path::Path::new(tel_socket_path).exists() { let _ = fs::remove_file(tel_socket_path); }
            if std::path::Path::new(mcp_socket_path).exists() { let _ = fs::remove_file(mcp_socket_path); }

            let tel_listener = UnixListener::bind(tel_socket_path)?;
            
            info!("Telemetry Server listening on {}", tel_socket_path);
            info!("MCP HTTP/SSE Server listening on 127.0.0.1:44129");

            // --- OS-Level Memory Watchdog ---
            std::thread::spawn(|| {
                let page_size = 4096;
                let limit_bytes: u64 = 14 * 1024 * 1024 * 1024; // 14 GB
                loop {
                    if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                        if let Some(rss_pages) = parse_rss_from_statm(&content) {
                            let rss_bytes = rss_pages * page_size;
                            if rss_bytes > limit_bytes {
                                error!("CRITICAL: Memory threshold reached ({} GB). Suicide for recycling...", rss_bytes / 1024 / 1024 / 1024);
                                std::process::exit(0);
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            });

            // --- BROADCAST SYSTEM for Telemetry ---
            let (results_tx, _) = tokio::sync::broadcast::channel::<String>(100000);

            // --- HARDWARE-AWARE SCALING ---
            // NEXUS V8.16: Optimized for 16-core host. 14 workers target.
            let num_workers = 14;
            info!("Power Scaling: Sizing worker pool growth to {} threads.", num_workers);

            // --- Pipeline Actors ---
            let (db_tx, db_rx) = crossbeam_channel::unbounded();
            
            axon_core::worker::WorkerPool::spawn_writer_actor(
                graph_store.clone(), 
                db_rx, 
                results_tx.clone()
            );

            let queue_for_pool = queue_store.clone();
            let store_for_pool = graph_store.clone();
            let results_tx_for_pool = results_tx.clone();
            let db_tx_for_pool = db_tx.clone();
            
            // Spawn WorkerPool in a dedicated task to allow immediate service availability
            tokio::task::spawn_blocking(move || {
                axon_core::worker::WorkerPool::new(
                    num_workers, 
                    queue_for_pool,
                    store_for_pool, 
                    db_tx_for_pool,
                    results_tx_for_pool
                );
            });

            // NEXUS v9.0: Semantic Factory
            // Spawns 1 background worker dedicated to embedding calculation.
            let semantic_store = graph_store.clone();
            tokio::task::spawn_blocking(move || {
                axon_core::embedder::SemanticWorkerPool::new(semantic_store);
            });

            let db_sender = db_tx;

            // --- MCP Listener Loop (HTTP/SSE via Axum) ---
            let mcp_store_for_axum = graph_store.clone();
            tokio::spawn(async move {
                let mcp_server = Arc::new(McpServer::new(mcp_store_for_axum));
                let app = axon_core::mcp_http::app_router(mcp_server);
                match tokio::net::TcpListener::bind("0.0.0.0:44129").await {
                    Ok(listener) => {
                        info!("✅ SQL Gateway/MCP: Listening on 0.0.0.0:44129");
                        let _ = axum::serve(listener, app).await;
                    },
                    Err(e) => error!("❌ SQL Gateway Failure: Could not bind to port 44129: {:?}", e),
                }
            });

            let projects_root_str = projects_root.to_string();
            let current_boot_id = Arc::new(tokio::sync::Mutex::new(String::new()));
            
            // NEXUS v7.0: Autonomous Ingestor (The Native Hopper)
            let store_for_auto = graph_store.clone();
            let queue_for_auto = queue_store.clone();
            tokio::spawn(async move {
                info!("Autonomous Ingestor: Ignition. Monitoring DuckDB for work...");
                loop {
                    // Replenish if queue has capacity (below 5000 tasks)
                    if queue_for_auto.len() < 5000 {
                        // Pull a large batch from DB directly
                        if let Ok(files) = store_for_auto.fetch_pending_batch(2000) {
                            if !files.is_empty() {
                                debug!("Autonomous Ingestor: Feeding {} tasks to workers.", files.len());
                                for f in files {
                                    let _ = queue_for_auto.push(&f.path, 0, &f.trace_id, 0, 0, false);
                                }
                            }
                        }
                    }
                    // Fast cycle for high-throughput
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            });

            // NEXUS v8.18: Sovereign Auto-Ignition
            // The scanner runs in a dedicated OS thread to guarantee execution
            // regardless of the Tokio async executor's load.
            let auto_scan_store = graph_store.clone();
            let auto_scan_root = projects_root_str.clone();
            std::thread::spawn(move || {
                info!("🚀 Auto-Ignition: Beginning initial workspace mapping...");
                axon_core::scanner::Scanner::new(&auto_scan_root).scan(auto_scan_store);
                info!("✅ Auto-Ignition: Initial mapping sequence complete.");
            });

            // --- Telemetry Listener Loop (Elixir/Dashboard) ---
            loop {
                let (mut socket, addr) = match tel_listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                
                info!("New Telemetry connection from {:?}", addr);

                let ready_event = BridgeEvent::SystemReady { start_time_utc: boot_time.clone() };
                let ready_msg = format!("Axon Telemetry Ready\n{}\n", serde_json::to_string(&ready_event).unwrap());
                let _ = socket.write_all(ready_msg.as_bytes()).await;

                let store_clone = graph_store.clone();
                let queue_clone = queue_store.clone();
                let projects_root_task = projects_root_str.clone();
                let boot_id_lock = current_boot_id.clone();
                let db_sender_task = db_sender.clone();
                let mut results_rx = results_tx.subscribe();
                let results_tx_for_conn = results_tx.clone();
                
                tokio::spawn(async move {
                    let (reader, mut writer) = socket.into_split();
                    let mut buf_reader = BufReader::new(reader);
                    
                    // Outgoing Feedback Loop: Dedicated task
                    tokio::spawn(async move {
                        loop {
                            match results_rx.recv().await {
                                Ok(msg) => {
                                    if writer.write_all(msg.as_bytes()).await.is_err() { 
                                        error!("Socket Write Error: Closing feedback loop.");
                                        break; 
                                    }
                                },
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                                    warn!("⚠️ Telemetry Lagged: skipped {} messages.", count);
                                    continue;
                                },
                                Err(_) => break,
                            }
                        }
                    });

                    // Incoming Command Loop: Priority task
                    let mut line = String::new();
                    while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                        if bytes_read == 0 { break; }
                        {
                            let command = line.trim();
                            if !command.is_empty() {
                                debug!("Telemetry: Received command [{}]", command);
                                
                                if command.starts_with("EXECUTE_CYPHER ") {
                                    let query = command[15..].trim().to_string();
                                    let _ = db_sender_task.send(axon_core::worker::DbWriteTask::ExecuteCypher { query });
                                } else if command.starts_with("RAW_QUERY ") {
                                    let query = command[10..].trim().to_string();
                                    let store = store_clone.clone();
                                    let result_tx = results_tx_for_conn.clone();
                                    tokio::spawn(async move {
                                        match store.query_json(&query) {
                                            Ok(res) => { let _ = result_tx.send(res + "\n"); },
                                            Err(e) => { let _ = result_tx.send(format!("{{\"error\": \"{:?}\"}}\n", e)); }
                                        }
                                    });
                                } else if command.starts_with("SESSION_INIT ") {
                                    let payload = &command[13..];
                                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(payload) {
                                        let new_id = data["boot_id"].as_str().unwrap_or("unknown").to_string();
                                        let mut active_id = boot_id_lock.lock().await;
                                        if new_id != *active_id {
                                            info!("🔄 New Elixir Session: {}. Maintaining current pipeline state.", new_id);
                                            // NEXUS v8.11: No more purge here. Progress is sovereign.
                                            *active_id = new_id;
                                        }
                                    }
                                } else if command.starts_with("PARSE_BATCH ") {
                                    let payload = &command[12..];
                                    if let Ok(batch_data) = serde_json::from_str::<serde_json::Value>(payload) {
                                        if let Some(files) = batch_data.as_array() {
                                            for file_data in files {
                                                let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                                                let trace_id = file_data["trace_id"].as_str().unwrap_or("unknown").to_string();
                                                let t0 = file_data["t0"].as_i64().unwrap_or(0);
                                                let t1 = file_data["t1"].as_i64().unwrap_or(0);
                                                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).map(|sys_time| sys_time.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64).unwrap_or(0);
                                                let _ = queue_clone.push(&path, mtime, &trace_id, t0, t1, false);
                                            }
                                    // NEXUS v6.5: Send direct response via broadcast channel to avoid ownership conflict
                                    let ack = serde_json::json!({"event": "BATCH_ACCEPTED"});
                                    if let Ok(msg) = serde_json::to_string(&ack) {
                                        let _ = results_tx_for_conn.send(msg + "\n");
                                    }
                                }
                            }
                        } else if command.starts_with("PULL_PENDING ") {
                                    let count_str = command[13..].trim();
                                    let count = count_str.parse::<usize>().unwrap_or(10);
                                    let store = store_clone.clone();
                                    let result_tx = results_tx_for_conn.clone();
                                    tokio::spawn(async move {
                                        if let Ok(files) = store.fetch_pending_batch(count) {
                                            if !files.is_empty() {
                                                let response = serde_json::json!({"event": "PENDING_BATCH_READY", "files": files});
                                                if let Ok(msg) = serde_json::to_string(&response) {
                                                    let _ = result_tx.send(msg + "\n");
                                                }
                                            }
                                        }
                                    });
                                } else if command == "SCAN_ALL" {
                                    let scan_store = store_clone.clone();
                                    let proj_root = projects_root_task.clone();
                                    tokio::spawn(async move {
                                        scanner::Scanner::new(&proj_root).scan(scan_store);
                                    });
                                } else if command == "RESET" {
                                    std::process::exit(0);
                                }
                            }
                        }
                        line.clear();
                    }
                });
            }
        })
}

fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok())
}
