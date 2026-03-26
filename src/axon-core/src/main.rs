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

use bridge::BridgeEvent;
use graph::GraphStore;
use mcp::McpServer;
use std::time::Instant;
use std::fs;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use log::{info, error};

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // MAESTRIA FIX: Cap blocking threads to prevent thread_local! / ONNX Arena explosion.
        .max_blocking_threads(8)
        .build()
        .unwrap()
        .block_on(async {
            env_logger::init();
            let boot_time = chrono::Utc::now().to_rfc3339();

            let projects_root = "/home/dstadel/projects";
            let db_path = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";

            info!("Starting Axon Core v2");
            info!("Engine Boot Time: {}", boot_time);

            let graph_store = match GraphStore::new(db_path) {
                Ok(store) => Arc::new(std::sync::RwLock::new(store)),
                Err(e) => {
                    error!("Fatal Error initializing LadybugDB: {:?}", e);
                    return Err(e);
                }
            };

            let tel_socket_path = "/tmp/axon-telemetry.sock";
            let mcp_socket_path = "/tmp/axon-mcp.sock";
            
            if std::path::Path::new(tel_socket_path).exists() { let _ = fs::remove_file(tel_socket_path); }
            if std::path::Path::new(mcp_socket_path).exists() { let _ = fs::remove_file(mcp_socket_path); }

            let tel_listener = UnixListener::bind(tel_socket_path)?;
            let _mcp_listener = UnixListener::bind(mcp_socket_path)?;
            
            info!("Telemetry Server listening on {}", tel_socket_path);
            info!("MCP Server listening on {}", mcp_socket_path);

            let mcp_active_flag = Arc::new(AtomicBool::new(false));

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
            let (results_tx, _) = tokio::sync::broadcast::channel::<String>(10000);

            // --- THE 8 IMMORTALS (GLOBAL POOL) ---
            // Move worker pool outside the loop to prevent memory explosion from multiple model loads.
            let worker_pool = crate::worker::WorkerPool::new(8, graph_store.clone(), results_tx.clone(), mcp_active_flag.clone());
            let worker_sender = worker_pool.get_sender();

            // --- MCP Listener Loop (HTTP/SSE via Axum) ---
            let mcp_store_for_axum = graph_store.clone();
            let mcp_flag_for_axum = mcp_active_flag.clone();
            tokio::spawn(async move {
                info!("Starting MCP HTTP/SSE Server on port 44129...");
                let mcp_server = Arc::new(McpServer::new(mcp_store_for_axum));
                let app = crate::mcp_http::app_router(mcp_server, mcp_flag_for_axum);
                
                let listener = tokio::net::TcpListener::bind("127.0.0.1:44129").await.expect("Failed to bind to port 44129");
                if let Err(e) = axum::serve(listener, app).await {
                    error!("MCP HTTP Server error: {}", e);
                }
            });

            let projects_root_str = projects_root.to_string();
            
            // --- Telemetry Listener Loop (Elixir/Dashboard) ---
            loop {
                let (mut socket, _) = match tel_listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!("Error accepting connection: {}", e);
                        continue;
                    }
                };
                info!("Elixir Dashboard connected to Telemetry Socket");
                
                let ready_event = BridgeEvent::SystemReady { start_time_utc: boot_time.clone() };
                let ready_msg = format!("Axon Telemetry Ready\n{}\n", serde_json::to_string(&ready_event).unwrap());
                
                if let Err(e) = socket.write_all(ready_msg.as_bytes()).await {
                    error!("Failed to write to telemetry socket: {}", e);
                    continue;
                }

                let store_clone = graph_store.clone();
                let projects_root_task = projects_root_str.clone();
                let worker_sender_clone = worker_sender.clone();
                let mut results_rx = results_tx.subscribe();
                
                tokio::spawn(async move {
                    let (reader, mut writer) = socket.into_split();
                    let mut buf_reader = BufReader::new(reader);
                    let mut line = String::new();
                    
                    // Task to forward global worker results to THIS dashboard instance
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
                        
                        if command.starts_with("WATCHER_EVENT ") {
                            // Forward watcher events to Elixir (legacy behavior)
                            // We might want to handle this differently in v2, but for now just bypass
                        } else if command.starts_with("PARSE_FILE ") {
                            let payload = &command[11..];
                            if let Ok(file_data) = serde_json::from_str::<serde_json::Value>(payload) {
                                let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                                let lane = file_data["lane"].as_str().unwrap_or("fast").to_string();
                                let _ = worker_sender_clone.send(crate::worker::WorkerTask {
                                    path,
                                    is_titan: lane == "titan",
                                });
                            }
                        } else if command == "SCAN_ALL" {
                            info!("Received SCAN_ALL command. Starting fleet ingestion...");
                            if let Some(task) = scan_task.take() {
                                cancel_token.store(true, Ordering::Relaxed);
                                let _ = task.await; 
                            }
                            cancel_token = Arc::new(AtomicBool::new(false));
                            let token_clone = cancel_token.clone();
                            let scan_store = store_clone.clone();
                            let ws_clone = worker_sender_clone.clone();
                            let proj_root = projects_root_task.clone();

                            scan_task = Some(tokio::spawn(async move {
                                let start = Instant::now();
                                let mut total_files = 0;
                                if let Ok(projects) = fs::read_dir(proj_root) {
                                    for project in projects.flatten() {
                                        if token_clone.load(Ordering::Relaxed) { break; }
                                        let project_path = project.path();
                                        let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                                        let files = scanner.scan(Some(scan_store.clone()));
                                        for file_path in files {
                                            if token_clone.load(Ordering::Relaxed) { break; }
                                            total_files += 1;
                                            let _ = ws_clone.send(crate::worker::WorkerTask {
                                                path: file_path.to_string_lossy().to_string(),
                                                is_titan: false,
                                            });
                                        }
                                    }
                                }
                                info!("Global scan complete: {} files queued in {}ms", total_files, start.elapsed().as_millis());
                            }));
                        } else if command.starts_with("SCAN_PROJECT ") {
                            let project_name = command[13..].trim().to_string();
                            info!("Received SCAN_PROJECT command for: {}. Starting sector ingestion...", project_name);
                            if let Some(task) = scan_task.take() {
                                cancel_token.store(true, Ordering::Relaxed);
                                let _ = task.await; 
                            }
                            cancel_token = Arc::new(AtomicBool::new(false));
                            let token_clone = cancel_token.clone();
                            let scan_store = store_clone.clone();
                            let ws_clone = worker_sender_clone.clone();
                            let proj_root = projects_root_task.clone();

                            scan_task = Some(tokio::spawn(async move {
                                let start = Instant::now();
                                let mut project_path = std::path::PathBuf::from(proj_root);
                                project_path.push(&project_name);
                                
                                let mut total_files = 0;
                                if project_path.exists() {
                                    let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                                    let files = scanner.scan(Some(scan_store.clone()));
                                    for file_path in files {
                                        if token_clone.load(Ordering::Relaxed) { break; }
                                        total_files += 1;
                                        let _ = ws_clone.send(crate::worker::WorkerTask {
                                            path: file_path.to_string_lossy().to_string(),
                                            is_titan: false,
                                        });
                                    }
                                }
                                info!("Project scan for {} complete: {} files queued in {}ms", project_name, total_files, start.elapsed().as_millis());
                            }));
                        } else if command == "STOP" {
                            cancel_token.store(true, Ordering::Relaxed);
                        } else if command == "RESET" {
                            cancel_token.store(true, Ordering::Relaxed);
                            let db_path_str = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
                            {
                                let mut locked = store_clone.write().unwrap();
                                let _ = std::fs::remove_dir_all(db_path_str);
                                if let Ok(new_store) = GraphStore::new(db_path_str) {
                                    *locked = new_store;
                                }
                            }
                        }
                        line.clear();
                    }
                    info!("Elixir Dashboard disconnected from Telemetry");
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

    #[test]
    fn test_memory_threshold_logic() {
        let limit_bytes: u64 = 14 * 1024 * 1024 * 1024;
        let page_size: u64 = 4096;
        let rss_pages = (15 * 1024 * 1024 * 1024) / 4096;
        assert!(rss_pages * page_size > limit_bytes);
    }
}
