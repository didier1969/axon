#[cfg(not(target_env = "msvc"))]
use jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

mod parser;
mod scanner;
mod bridge;
mod config;
mod graph;
mod mcp;
mod mcp_http;
mod embedder;
mod worker;
mod queue;

use bridge::BridgeEvent;
use graph::GraphStore;
use mcp::McpServer;
use std::time::Instant;
use std::fs;
use std::sync::{Arc, atomic::{AtomicBool, AtomicUsize, Ordering}};
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error};
use parking_lot::RwLock;
use queue::QueueStore;

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

            let projects_root = "/home/dstadel/projects";
            let db_path = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";

            info!("Starting Axon Core v2.1 (Native Backpressure Edition)");
            info!("Engine Boot Time: {}", boot_time);

            // Initialize KuzuDB
            let graph_store = match GraphStore::new(db_path) {
                Ok(store) => Arc::new(RwLock::new(store)),
                Err(e) => {
                    error!("Fatal Error initializing LadybugDB: {:?}", e);
                    return Err(e);
                }
            };

            // Initialize In-Memory Bounded Queue (Max 500 tasks to block Elixir via UDS)
            let queue_store = Arc::new(QueueStore::new(500));
            let tel_socket_path = "/tmp/axon-telemetry.sock";
            let mcp_socket_path = "/tmp/axon-mcp.sock";
            
            if std::path::Path::new(tel_socket_path).exists() { let _ = fs::remove_file(tel_socket_path); }
            if std::path::Path::new(mcp_socket_path).exists() { let _ = fs::remove_file(mcp_socket_path); }

            let tel_listener = UnixListener::bind(tel_socket_path)?;
            let _mcp_listener = UnixListener::bind(mcp_socket_path)?;
            
            info!("Telemetry Server listening on {}", tel_socket_path);
            info!("MCP Server listening on {}", mcp_socket_path);

            let mcp_active_flag = Arc::new(AtomicUsize::new(0));

            // --- OS-Level Sledgehammer (Option B) Memory Watchdog ---
            std::thread::spawn(|| {
                let page_size = 4096;
                let limit_bytes: u64 = 14 * 1024 * 1024 * 1024; // 14 GB
                loop {
                    if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                        if let Some(rss_pages) = parse_rss_from_statm(&content) {
                            let rss_bytes = rss_pages * page_size;
                            if rss_bytes > limit_bytes {
                                error!("CRITICAL: Memory threshold reached ({} GB). Executing Process Cycling (Option B) suicide...", rss_bytes / 1024 / 1024 / 1024);
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
            // Detect available CPU cores to size the worker pool dynamically
            let available_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let num_fast_workers = if available_cores > 2 { available_cores - 2 } else { 1 };
            info!("Hardware-Aware Scaling: Detected {} cores. Sizing worker pool to {} threads.", available_cores, num_fast_workers);

            // Move worker pool outside the loop to prevent memory explosion from multiple model loads.
            let _worker_pool = crate::worker::WorkerPool::new(
                num_fast_workers, 
                graph_store.clone(), 
                queue_store.clone(),
                results_tx.clone(), 
                mcp_active_flag.clone()
            );

            // --- MCP Listener Loop (HTTP/SSE via Axum) ---
            let mcp_store_for_axum = graph_store.clone();
            let mcp_flag_for_axum = mcp_active_flag.clone();
            tokio::spawn(async move {
                let mcp_server = Arc::new(McpServer::new(mcp_store_for_axum));
                let app = crate::mcp_http::app_router(mcp_server, mcp_flag_for_axum);
                let listener = tokio::net::TcpListener::bind("127.0.0.1:44129").await.expect("Failed to bind to port 44129");
                let _ = axum::serve(listener, app).await;
            });

            let projects_root_str = projects_root.to_string();
            
            // --- Telemetry Listener Loop (Elixir/Dashboard) ---
            loop {
                let (mut socket, _) = match tel_listener.accept().await {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                
                let ready_event = BridgeEvent::SystemReady { start_time_utc: boot_time.clone() };
                let ready_msg = format!("Axon Telemetry Ready\n{}\n", serde_json::to_string(&ready_event).unwrap());
                let _ = socket.write_all(ready_msg.as_bytes()).await;

                let store_clone = graph_store.clone();
                let queue_clone = queue_store.clone();
                let projects_root_task = projects_root_str.clone();
                let mut results_rx = results_tx.subscribe();
                
                tokio::spawn(async move {
                    let (reader, mut writer) = socket.into_split();
                    let mut buf_reader = BufReader::new(reader);
                    let mut line = String::new();
                    
                    tokio::spawn(async move {
                        while let Ok(msg) = results_rx.recv().await {
                            if writer.write_all(msg.as_bytes()).await.is_err() { break; }
                        }
                    });

                    let mut cancel_token = Arc::new(AtomicBool::new(false));
                    let mut scan_task: Option<tokio::task::JoinHandle<()>> = None;

                    while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                        if bytes_read == 0 { break; }
                        let command = line.trim();
                        if command.is_empty() { line.clear(); continue; }
                        
                        if command.starts_with("PARSE_FILE ") {
                            let payload = &command[11..];
                            if let Ok(file_data) = serde_json::from_str::<serde_json::Value>(payload) {
                                let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                                let trace_id = file_data["trace_id"].as_str().unwrap_or("unknown").to_string();
                                let t0 = file_data["t0"].as_i64().unwrap_or(0);
                                let t1 = file_data["t1"].as_i64().unwrap_or(0);
                                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).map(|sys_time| sys_time.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64).unwrap_or(0);
                                let _ = queue_clone.push(&path, mtime, &trace_id, t0, t1);
                            }
                        } else if command == "SCAN_ALL" {
                            info!("🚀 SCAN_ALL: Indexing EVERY project in workspace...");
                            if let Some(task) = scan_task.take() {
                                cancel_token.store(true, Ordering::Relaxed);
                                let _ = task.await; 
                            }
                            let _ = queue_clone.purge_all(); // Purge old SQLite queue to prevent zombie processing
                            cancel_token = Arc::new(AtomicBool::new(false));
                            let token_clone = cancel_token.clone();
                            let scan_store = store_clone.clone();
                            let scan_queue = queue_clone.clone();
                            let proj_root = projects_root_task.clone();

                            scan_task = Some(tokio::spawn(async move {
                                let start = Instant::now();
                                if let Ok(projects) = fs::read_dir(proj_root) {
                                    for project in projects.flatten() {
                                        if token_clone.load(Ordering::Relaxed) { break; }
                                        let project_path = project.path();
                                        let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                                        scanner.scan(Some(scan_store.clone()), Some(scan_queue.clone()));
                                    }
                                }
                                info!("🏁 Global scan complete in {}ms", start.elapsed().as_millis());
                            }));
                        } else if command.starts_with("SCAN_PROJECT ") {
                            let project_name = command[13..].trim().to_string();
                            info!("🚀 SCAN_PROJECT: Indexing sector {}...", project_name);
                            if let Some(task) = scan_task.take() {
                                cancel_token.store(true, Ordering::Relaxed);
                                let _ = task.await; 
                            }
                            cancel_token = Arc::new(AtomicBool::new(false));
                            let token_clone = cancel_token.clone();
                            let scan_store = store_clone.clone();
                            let scan_queue = queue_clone.clone();
                            let proj_root = projects_root_task.clone();

                            scan_task = Some(tokio::spawn(async move {
                                let start = Instant::now();
                                let mut project_path = std::path::PathBuf::from(proj_root);
                                project_path.push(&project_name);
                                
                                if project_path.exists() {
                                    let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                                    scanner.scan(Some(scan_store.clone()), Some(scan_queue.clone()));
                                }
                                info!("🏁 Project scan for {} complete in {}ms", project_name, start.elapsed().as_millis());
                            }));
                        } else if command == "STOP" {
                            cancel_token.store(true, Ordering::Relaxed);
                        } else if command == "RESET" {
                            cancel_token.store(true, Ordering::Relaxed);
                            let db_path_str = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
                            {
                                let mut locked = store_clone.write();
                                let _ = std::fs::remove_dir_all(db_path_str);
                                if let Ok(new_store) = GraphStore::new(db_path_str) {
                                    *locked = new_store;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse_statm_rss() {
        let content = "1234 5678 9012 34 56 78 90";
        assert_eq!(parse_rss_from_statm(content), Some(5678));
    }
}
