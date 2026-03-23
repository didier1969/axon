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
        // 8 threads allows Goldorak to use up to 13-16GB of RAM securely without leaking.
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
    let mcp_listener = UnixListener::bind(mcp_socket_path)?;
    
    info!("Telemetry Server listening on {}", tel_socket_path);
    info!("MCP Server listening on {}", mcp_socket_path);

    let mcp_store = graph_store.clone();
    
    // The "Diplomatic Priority" flag. True when MCP is processing a query.
    let mcp_active_flag = Arc::new(AtomicBool::new(false));
    let mcp_active_for_listener = mcp_active_flag.clone();

    // --- MCP Listener Loop (Pure JSON-RPC) ---
    tokio::spawn(async move {
        while let Ok((socket, _)) = mcp_listener.accept().await {
            info!("IA Client connected to MCP Socket");
            let store_clone = mcp_store.clone();
            let mcp_flag_clone = mcp_active_for_listener.clone();
            
            tokio::spawn(async move {
                let (reader, mut writer) = socket.into_split();
                let mut buf_reader = BufReader::new(reader);
                let mut line = String::new();
                
                while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                    if bytes_read == 0 { break; }
                    let command = line.trim();
                    if command.is_empty() { line.clear(); continue; }

                    let store_for_mcp = store_clone.clone();
                    let command_clone = command.to_string();
                    let flag_for_task = mcp_flag_clone.clone();
                    
                    info!("MCP Processing start for command: {} bytes", command_clone.len());
                    let mcp_server = McpServer::new(store_for_mcp);
                    match serde_json::from_str::<mcp::JsonRpcRequest>(&command_clone) {
                        Ok(request) => {
                            info!("MCP Request Parsed: method={}", request.method);
                            // Signal ingestion to pause
                            flag_for_task.store(true, Ordering::SeqCst);
                            
                            let response_opt = tokio::task::spawn_blocking(move || {
                                info!("MCP Executing in blocking thread...");
                                let res = mcp_server.handle_request(request);
                                // Release ingestion pause
                                flag_for_task.store(false, Ordering::SeqCst);
                                info!("MCP Execution complete.");
                                res
                            }).await.expect("Blocking MCP task panicked");
                            
                            if let Some(response) = response_opt {
                                if let Ok(json_str) = serde_json::to_string(&response) {
                                    info!("MCP Sending response ({} bytes)", json_str.len());
                                    let _ = writer.write_all(format!("{}\n", json_str).as_bytes()).await;
                                    let _ = writer.flush().await;
                                    info!("MCP Response flushed.");
                                }
                            } else {
                                info!("No response required (Notification)");
                            }
                        },
                        Err(e) => {
                            error!("MCP JSON Parse Error: {} | Raw: '{}'", e, command_clone);
                        }
                    }
                    line.clear();
                }
                info!("IA Client disconnected from MCP");
            });
        }
    });

    let tel_mcp_flag = mcp_active_flag.clone();

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
        let projects_root_str = projects_root.to_string();
        let telemetry_mcp_flag = tel_mcp_flag.clone();
        
        // Limit concurrent heavy parsing/embedding tasks to prevent OOM
        let parse_semaphore = Arc::new(tokio::sync::Semaphore::new(4));
        
        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10000);
            
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if writer.write_all(msg.as_bytes()).await.is_err() { break; }
                }
            });

            let mut cancel_token = Arc::new(AtomicBool::new(false));
            let mut scan_task: Option<tokio::task::JoinHandle<()>> = None;

            // THE 8 IMMORTALS: Explicit Worker Pool instantiation.
            let worker_pool = crate::worker::WorkerPool::new(8, store_clone.clone(), tx.clone());
            let worker_sender = worker_pool.get_sender();

            while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                if bytes_read == 0 { break; }
                
                let command = line.trim();
                if command.is_empty() { line.clear(); continue; }
                
                if command.starts_with("WATCHER_EVENT ") {
                    let payload = &command[14..];
                    if let Ok(event_data) = serde_json::from_str::<serde_json::Value>(payload) {
                        let forward_msg = serde_json::to_string(&event_data).unwrap() + "\n";
                        let _ = tx.try_send(forward_msg);
                    }
                } else if command.starts_with("PARSE_FILE ") {
                    // Backpressure: Wait for a slot BEFORE reading more from the socket and parsing JSON
                    let permit = parse_semaphore.clone().acquire_owned().await.unwrap();
                    
                    let payload = &command[11..];
                    if let Ok(file_data) = serde_json::from_str::<serde_json::Value>(payload) {
                        let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                        let lane = file_data["lane"].as_str().unwrap_or("fast").to_string();
                        let is_titan = lane == "titan";
                        
                        // Delegate to the immortals
                        let _ = worker_sender.send(crate::worker::WorkerTask {
                            path,
                            is_titan,
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
                    let tx_clone = tx.clone();
                    let projects_root_task = projects_root_str.clone();
                    
                    scan_task = Some(tokio::spawn(async move {
                        let start = Instant::now();
                        let mut total_files = 0;
                        if let Ok(projects) = fs::read_dir(projects_root_task) {
                            for project in projects.flatten() {
                                if token_clone.load(Ordering::Relaxed) { break; }
                                let project_path = project.path();
                                let project_name = project_path.file_name().unwrap().to_string_lossy().to_string();
                                let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                                let files = scanner.scan();
                                
                                let proj_start_msg = serde_json::to_string(&BridgeEvent::ProjectScanStarted {
                                    project: project_name.clone(), total_files: files.len()
                                }).unwrap() + "\n";
                                let _ = tx_clone.send(proj_start_msg).await;

                                for file_path in files {
                                    if token_clone.load(Ordering::Relaxed) { break; }
                                    total_files += 1;
                                    let final_file_msg = serde_json::to_string(&BridgeEvent::FileIndexed {
                                        path: file_path.to_string_lossy().to_string(), symbol_count: 0, relation_count: 0,
                                        file_count: total_files, entry_points: 0, security_score: 100, coverage_score: 0,
                                        taint_paths: "".to_string(),
                                    }).unwrap() + "\n";
                                    let _ = tx_clone.send(final_file_msg).await;
                                }
                            }
                        }
                        let duration = start.elapsed();
                        let complete_event = BridgeEvent::ScanComplete { total_files: 0, duration_ms: duration.as_millis() as u64 };
                        let _ = tx_clone.send(serde_json::to_string(&complete_event).unwrap() + "\n").await;
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
