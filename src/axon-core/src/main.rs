mod parser;
mod scanner;
mod bridge;
mod graph;
mod mcp;

use bridge::BridgeEvent;
use graph::GraphStore;
use mcp::McpServer;
use std::time::Instant;
use std::fs;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use log::{info, debug, error, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let boot_time = chrono::Utc::now().to_rfc3339();
    
    let projects_root = "/home/dstadel/projects";
    let db_path = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
    
    info!("Starting Axon Core v2");
    info!("Engine Boot Time: {}", boot_time);
    
    let graph_store = match GraphStore::new(db_path) {
        Ok(store) => Arc::new(store),
        Err(e) => {
            error!("Fatal Error initializing LadybugDB: {:?}", e);
            return Err(e);
        }
    };

    let socket_path = "/tmp/axon-v2.sock";
    
    // Remove existing socket if it exists
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
            
            while let Ok(bytes_read) = buf_reader.read_line(&mut line).await {
                if bytes_read == 0 {
                    break; // EOF
                }
                
                let command = line.trim();
                
                if command == "START" {
                    info!("Received START command from Control Plane");
                    let start = Instant::now();
                    
                    let project_dirs = fs::read_dir(projects_root).unwrap()
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.path().is_dir())
                        .filter(|entry| {
                            entry.path().join(".axon").exists()
                        })
                        .collect::<Vec<_>>();

                    let start_msg = serde_json::to_string(&BridgeEvent::ScanStarted { 
                        total_files: project_dirs.len() 
                    }).unwrap() + "\n";
                    let _ = writer.write_all(start_msg.as_bytes()).await;

                    for project in project_dirs {
                        let project_path = project.path();
                        let project_name = project_path.file_name().unwrap().to_string_lossy().to_string();
                        
                        info!("Scanning project: {}", project_name);
                        let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                        let files = scanner.scan();
                        info!("Found {} files in {}", files.len(), project_name);
                        
                        let chunk_size = 50;
                        let mut _total_symbols_for_project = 0;
                        let mut _total_files_for_project = 0;
                        let mut _total_rels_for_project = 0;
                        
                        for chunk in files.chunks(chunk_size) {
                            let chunk_vec = chunk.to_vec();
                            let store = store_clone.clone();
                            let (chunk_symbols, chunk_rels): (usize, usize) = tokio::task::spawn_blocking(move || {
                                let mut local_syms = 0;
                                let mut local_rels = 0;
                                for path in chunk_vec {
                                    debug!("Parsing file: {:?}", path);
                                    if let Some(parser) = parser::get_parser_for_file(&path) {
                                        if let Ok(content) = fs::read_to_string(&path) {
                                            let result = parser.parse(&content);
                                            local_syms += result.symbols.len();
                                            local_rels += result.relations.len();
                                            let path_str = path.to_string_lossy().to_string();
                                            
                                            if let Err(e) = store.insert_file_data(&path_str, &result) {
                                                warn!("Failed to insert to DB for {:?}: {:?}", path, e);
                                            }
                                        }
                                    }
                                }
                                (local_syms, local_rels)
                            }).await.unwrap_or_else(|e| {
                                error!("Task panicked: {:?}", e);
                                (0, 0)
                            });
                            
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
                            }).unwrap() + "\n";
                            
                            let _ = writer.write_all(file_msg.as_bytes()).await;
                        }
                        
                        info!("Finalizing DB aggregates for {}", project_name);
                        let (sec_score, _) = store_clone.get_security_audit(&project_name).unwrap_or((100, "".to_string()));
                        let cov_score = store_clone.get_coverage_score(&project_name).unwrap_or(0);
                        let entry_count = store_clone.query_count(&format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' AND s.tested = true RETURN count(s)", project_name)).unwrap_or(0) as usize; 

                        let final_file_msg = serde_json::to_string(&BridgeEvent::FileIndexed { 
                            path: project_name.clone(), 
                            symbol_count: 0, 
                            relation_count: 0,
                            file_count: 0,
                            entry_points: entry_count,
                            security_score: sec_score,
                            coverage_score: cov_score,
                        }).unwrap() + "\n";
                        let _ = writer.write_all(final_file_msg.as_bytes()).await;
                    }

                    let duration = start.elapsed();
                    info!("Fleet Ingestion Complete in {:?}", duration);
                    let complete_msg = serde_json::to_string(&BridgeEvent::ScanComplete { 
                        total_files: 0, 
                        duration_ms: duration.as_millis() as u64 
                    }).unwrap() + "\n";
                    let _ = writer.write_all(complete_msg.as_bytes()).await;
                } else if command.starts_with('{') {
                    let mcp_server = McpServer::new(store_clone.clone());
                    if let Ok(request) = serde_json::from_str::<mcp::JsonRpcRequest>(command) {
                        let response = mcp_server.handle_request(request);
                        if let Ok(json_str) = serde_json::to_string(&response) {
                            let _ = writer.write_all(format!("{}\n", json_str).as_bytes()).await;
                        }
                    }
                }
                
                line.clear();
            }
            info!("Client disconnected");
        });
    }
}
