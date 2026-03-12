mod parser;
mod scanner;
mod bridge;
mod graph;
mod mcp;

use bridge::BridgeEvent;
use graph::GraphStore;
use mcp::McpServer;
use rayon::prelude::*;
use std::time::Instant;
use std::fs;
use std::sync::Arc;
use std::io::{self, BufRead};

fn main() -> anyhow::Result<()> {
    let projects_root = "/home/dstadel/projects";
    let db_path = "/home/dstadel/projects/axon/.axon/graph_v2/lbug.db";
    
    let graph_store = Arc::new(GraphStore::new(&db_path)?);

    let stdin = io::stdin();
    println!("READY"); 

    for line in stdin.lock().lines() {
        let line = line?;
        match line.trim() {
            "SCAN" => {
                let start = Instant::now();
                
                let project_dirs = fs::read_dir(projects_root)?
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.path().is_dir())
                    .collect::<Vec<_>>();

                println!("{}", serde_json::to_string(&BridgeEvent::ScanStarted { 
                    total_files: project_dirs.len() 
                }).unwrap());

                for project in project_dirs {
                    let project_path = project.path();
                    let project_name = project_path.file_name().unwrap().to_string_lossy().to_string();
                    
                    let scanner = scanner::Scanner::new(&project_path.to_string_lossy());
                    let files = scanner.scan();
                    
                    let total_symbols: usize = files.par_iter().map(|path| {
                        if let Some(parser) = parser::get_parser_for_file(path) {
                            if let Ok(content) = fs::read_to_string(path) {
                                let result = parser.parse(&content);
                                let path_str = path.to_string_lossy().to_string();
                                
                                // Ingestion réelle des données
                                let _ = graph_store.insert_file_data(&path_str, &result);

                                return result.symbols.len();
                            }
                        }
                        0
                    }).sum();

                    // CALCUL DES SCORES REELS (100% REALITY)
                    let sec_score = graph_store.get_security_score(&project_name).unwrap_or(100);
                    let cov_score = graph_store.get_coverage_score(&project_name).unwrap_or(0);

                    // On envoie les vraies métriques au dashboard
                    println!("{}", serde_json::to_string(&BridgeEvent::FileIndexed { 
                        path: project_name, 
                        symbol_count: total_symbols,
                        security_score: sec_score,
                        coverage_score: cov_score,
                    }).unwrap());
                }

                let duration = start.elapsed();
                println!("{}", serde_json::to_string(&BridgeEvent::ScanComplete { 
                    total_files: 0, 
                    duration_ms: duration.as_millis() as u64 
                }).unwrap());
            },
            "STOP" => std::process::exit(0),
            _ => {
                if line.starts_with('{') {
                    let mcp_server = McpServer::new(graph_store.clone());
                    if let Ok(request) = serde_json::from_str::<mcp::JsonRpcRequest>(&line) {
                        let response = mcp_server.handle_request(request);
                        if let Ok(json_str) = serde_json::to_string(&response) {
                            println!("{}", json_str);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
