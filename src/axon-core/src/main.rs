mod parser;
mod scanner;
mod bridge;
mod config;
mod graph;
mod mcp;
mod embedder;

use bridge::BridgeEvent;
use graph::GraphStore;
use mcp::McpServer;
use std::time::Instant;
use std::fs;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use log::{info, error};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let boot_time = chrono::Utc::now().to_rfc3339();
    
    let projects_root = "/home/dstadel/projects";
    let db_path = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
    
    info!("Starting Axon Core v2");
    info!("Engine Boot Time: {}", boot_time);
    
    // We wrap GraphStore in an Arc to be accessible and also an Arc<Mutex> maybe?
    // Wait, GraphStore itself provides thread-safe access (it's using Kuzu connection). 
    // To implement RESET, we need to be able to recreate the GraphStore. 
    // Using Arc<tokio::sync::RwLock<GraphStore>> is safer for RESET.
    let graph_store = match GraphStore::new(db_path) {
        Ok(store) => Arc::new(std::sync::RwLock::new(store)),
        Err(e) => {
            error!("Fatal Error initializing LadybugDB: {:?}", e);
            return Err(e);
        }
    };

    let socket_path = "/tmp/axon-v2.sock";
    
    if std::path::Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    info!("UDS Server listening on {}", socket_path);

    loop {
        let (mut socket, _) = match listener.accept().await {
            Ok(s) => s,
            Err(e) => {
                error!("Error accepting connection: {}", e);
                continue;
            }
        };
        info!("Client connected to UDS");
        
        let ready_event = BridgeEvent::SystemReady { start_time_utc: boot_time.clone() };
        let ready_msg = format!("Axon Bridge Ready\n{}\n", serde_json::to_string(&ready_event).unwrap());
        
        if let Err(e) = socket.write_all(ready_msg.as_bytes()).await {
            error!("Failed to write to socket: {}", e);
            continue;
        }

        let store_clone = graph_store.clone();
        
        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut buf_reader = BufReader::new(reader);
            let mut line = String::new();
            
            // Setup a channel for sending messages back to the UI asynchronously
            let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);
            
            // Spawn writer loop
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if writer.write_all(msg.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });

            let mut cancel_token = Arc::new(AtomicBool::new(false));
            let mut scan_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                if bytes_read == 0 {
                    break; // EOF
                }
                
                let command = line.trim();
                
                if command.starts_with("WATCHER_EVENT ") {
                    let payload = &command[14..];
                    if let Ok(event_data) = serde_json::from_str::<serde_json::Value>(payload) {
                        // Forward the event directly to connected dashboard clients
                        let forward_msg = serde_json::to_string(&event_data).unwrap() + "\n";
                        let _ = tx.send(forward_msg).await;
                    }
                } else if command.starts_with("PARSE_FILE ") {
                    let payload = &command[11..];
                    if let Ok(file_data) = serde_json::from_str::<serde_json::Value>(payload) {
                        let path = file_data["path"].as_str().unwrap_or("unknown").to_string();
                        let content = file_data["content"].as_str().unwrap_or("").to_string();
                        info!("Received PARSE_FILE request for: {}", path);
                        
                        let store_for_parse = store_clone.clone();
                        let tx_clone = tx.clone();
                        
                        tokio::spawn(async move {
                            let mut symbols_count = 0;
                            let mut rels_count = 0;
                            
                            let path_obj = std::path::Path::new(&path);
                            if let Some(parser) = parser::get_parser_for_file(path_obj) {
                                let extraction = parser.parse(&content);
                                symbols_count = extraction.symbols.len();
                                rels_count = extraction.relations.len();
                                
                                if let Ok(store) = store_for_parse.read() {
                                    let _ = store.insert_file_data(&path, &extraction);
                                }
                            }
                            
                            let finish_msg = serde_json::to_string(&BridgeEvent::FileIndexed {
                                path: path.clone(),
                                symbol_count: symbols_count,
                                relation_count: rels_count,
                                file_count: 1,
                                entry_points: 0,
                                security_score: 100,
                                coverage_score: 0,
                                taint_paths: String::new(),
                            }).unwrap() + "\n";
                            let _ = tx_clone.send(finish_msg).await;
                        });
                    }
                } else if command == "START" {
                    info!("Received START command");
                    if let Some(task) = scan_task.take() {
                        cancel_token.store(true, Ordering::Relaxed);
                        let _ = task.await; // Wait for old task to finish cleanly
                    }
                    cancel_token = Arc::new(AtomicBool::new(false));
                    
                    let token_clone = cancel_token.clone();
                    let tx_clone = tx.clone();
                    let store_for_scan = store_clone.clone();
                    let proj_root = projects_root.to_string();

                    scan_task = Some(tokio::spawn(async move {
                        let start = Instant::now();
                        
                        let project_dirs = fs::read_dir(&proj_root).unwrap()
                            .filter_map(|entry| entry.ok())
                            .filter(|entry| entry.path().is_dir())
                            .filter(|entry| entry.path().join(".axon").exists())
                            .collect::<Vec<_>>();

                        let start_msg = serde_json::to_string(&BridgeEvent::ScanStarted { 
                            total_files: project_dirs.len() 
                        }).unwrap() + "\n";
                        let _ = tx_clone.send(start_msg).await;

                        for project in project_dirs {
                            if token_clone.load(Ordering::Relaxed) { break; }
                            
                            let project_path = project.path();
                            let project_name = project_path.file_name().unwrap().to_string_lossy().to_string();
                            
                            info!("Scanning project: {}", project_name);
                            let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                            let files = scanner.scan();
                            
                            let proj_start_msg = serde_json::to_string(&BridgeEvent::ProjectScanStarted {
                                project: project_name.clone(),
                                total_files: files.len(),
                            }).unwrap() + "\n";
                            let _ = tx_clone.send(proj_start_msg).await;
                            
                            let chunk_size = 50;
                            let mut _total_symbols_for_project = 0;
                            let mut _total_files_for_project = 0;
                            let mut _total_rels_for_project = 0;
                            
                            for chunk in files.chunks(chunk_size) {
                                if token_clone.load(Ordering::Relaxed) { break; }
                                
                                let chunk_vec = chunk.to_vec();
                                let store_for_thread = store_for_scan.clone();
                                
                                let (chunk_symbols, chunk_rels): (usize, usize) = tokio::task::spawn_blocking(move || {
                                    let locked_store = store_for_thread.read().unwrap();
                                    // Do heavy work
                                    let mut local_syms = 0;
                                    let mut local_rels = 0;
                                    for path in chunk_vec {
                                        if let Some(parser) = parser::get_parser_for_file(&path) {
                                            if let Ok(content) = fs::read_to_string(&path) {
                                                let mut result = parser.parse(&content);
                                                let texts_to_embed: Vec<String> = result.symbols.iter()
                                                    .map(|s| {
                                                        let doc = s.docstring.as_deref().unwrap_or("");
                                                        format!("Symbol: {} Kind: {} Doc: {}", s.name, s.kind, doc)
                                                    })
                                                    .collect();
                                                if let Ok(embeddings) = crate::embedder::batch_embed(texts_to_embed) {
                                                    for (sym, emb) in result.symbols.iter_mut().zip(embeddings.into_iter()) {
                                                        sym.embedding = Some(emb);
                                                    }
                                                }
                                                local_syms += result.symbols.len();
                                                local_rels += result.relations.len();
                                                let path_str = path.to_string_lossy().to_string();
                                                let _ = locked_store.insert_file_data(&path_str, &result);
                                            }
                                        }
                                    }
                                    (local_syms, local_rels)
                                }).await.unwrap_or((0, 0));
                                
                                _total_symbols_for_project += chunk_symbols;
                                _total_files_for_project += chunk.len();
                                _total_rels_for_project += chunk_rels;

                                let file_msg = serde_json::to_string(&BridgeEvent::FileIndexed { 
                                    path: project_name.clone(), 
                                    symbol_count: chunk_symbols, 
                                    relation_count: chunk_rels,
                                    file_count: chunk.len(),
                                    entry_points: 0, 
                                    security_score: 100,
                                    coverage_score: 0,
                                    taint_paths: String::new(),
                                    }).unwrap() + "\n";                                let _ = tx_clone.send(file_msg).await;
                            }
                            
                            if !token_clone.load(Ordering::Relaxed) {
                                let (sec_score, taint_paths, cov_score, entry_count) = {
                                    let locked_store = store_for_scan.read().unwrap();
                                    let (sec_score, paths) = locked_store.get_security_audit(&project_name).unwrap_or((100, "[]".to_string()));
                                    let cov_score = locked_store.get_coverage_score(&project_name).unwrap_or(0);
                                    let entry_count = locked_store.query_count(&format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' AND s.tested = true RETURN count(s)", project_name)).unwrap_or(0) as usize; 
                                    (sec_score, paths, cov_score, entry_count)
                                };

                                let final_file_msg = serde_json::to_string(&BridgeEvent::FileIndexed { 
                                    path: project_name.clone(), 
                                    symbol_count: 0, 
                                    relation_count: 0,
                                    file_count: 0,
                                    entry_points: entry_count,
                                    security_score: sec_score,
                                    coverage_score: cov_score,
                                    taint_paths,
                                }).unwrap() + "\n";
                                let _ = tx_clone.send(final_file_msg).await;
                            }
                        }

                        if !token_clone.load(Ordering::Relaxed) {
                            let duration = start.elapsed();
                            info!("Fleet Ingestion Complete in {:?}", duration);
                            let complete_msg = serde_json::to_string(&BridgeEvent::ScanComplete { 
                                total_files: 0, 
                                duration_ms: duration.as_millis() as u64 
                            }).unwrap() + "\n";
                            let _ = tx_clone.send(complete_msg).await;
                        } else {
                            info!("Fleet Ingestion STOPPED by user.");
                            let complete_msg = serde_json::to_string(&BridgeEvent::ScanComplete { 
                                total_files: 0, 
                                duration_ms: 0
                            }).unwrap() + "\n";
                            let _ = tx_clone.send(complete_msg).await;
                        }
                    }));
                } else if command == "STOP" {
                    info!("Received STOP command");
                    cancel_token.store(true, Ordering::Relaxed);
                    if let Some(task) = scan_task.take() {
                        let _ = task.await; // Wait cleanly
                    }
                } else if command == "RESET" {
                    info!("Received RESET command. Purging KuzuDB...");
                    cancel_token.store(true, Ordering::Relaxed);
                    if let Some(task) = scan_task.take() {
                        let _ = task.await; // Ensure nothing is writing
                    }
                    
                    // Drop the old graph_store instance explicitly inside a write lock block to free handles
                    {
                        let mut locked = store_clone.write().unwrap();
                        if let Ok(temp_store) = GraphStore::new(":memory:") {
                            *locked = temp_store;
                        }
                    }
                    
                    let db_path_str = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
                    let _ = std::fs::remove_dir_all(db_path_str);
                    
                    // Recreate
                    {
                        let mut locked = store_clone.write().unwrap();
                        match GraphStore::new(db_path_str) {
                            Ok(new_store) => {
                                *locked = new_store;
                                info!("Database RESET complete.");
                            },
                            Err(e) => {
                                error!("Failed to recreate database after reset: {}", e);
                            }
                        }
                    }
                    let complete_msg = serde_json::to_string(&BridgeEvent::ScanComplete { 
                        total_files: 0, 
                        duration_ms: 0
                    }).unwrap() + "\n";
                    let _ = tx.send(complete_msg).await;
                    
                } else if command.starts_with('{') {
                    // MCP Request - Offload heavy graph queries from Tokio worker thread
                    let store_for_mcp = store_clone.clone();
                    let command_clone = command.to_string();
                    let tx_clone = tx.clone();
                    
                    tokio::spawn(async move {
                        let mcp_server = McpServer::new(store_for_mcp);
                        if let Ok(request) = serde_json::from_str::<mcp::JsonRpcRequest>(&command_clone) {
                            
                            // Execute synchronous FFI graph query in blocking thread pool
                            let response = tokio::task::spawn_blocking(move || {
                                mcp_server.handle_request(request)
                            }).await.expect("Blocking MCP task panicked");
                            
                            if let Ok(json_str) = serde_json::to_string(&response) {
                                let _ = tx_clone.send(format!("{}\n", json_str)).await;
                            }
                        }
                    });
                }
                
                line.clear();
            }
            info!("Client disconnected");
        });
    }
}