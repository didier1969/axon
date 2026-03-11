mod parser;
mod scanner;
mod bridge;
mod graph;

use parser::{Parser, python::PythonParser};
use bridge::{Bridge, BridgeEvent};
use graph::GraphStore;
use rayon::prelude::*;
use std::time::Instant;
use std::fs;
use std::env;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    println!("Axon v2 Data Plane : Operational");
    
    let args: Vec<String> = env::args().collect();
    let root = if args.len() > 1 { &args[1] } else { "." };

    // Initialisation du Bridge Dashboard (UDS)
    let socket_path = "/tmp/axon-v2.sock";
    let bridge = Bridge::new(socket_path);
    bridge.start_server()?;

    // Initialisation du GraphStore (LadybugDB)
    let db_path = format!("{}/.axon/graph_v2", root);
    let graph_store = Arc::new(GraphStore::new(&db_path)?);
    println!("GraphStore initialized at {}", db_path);

    let start = Instant::now();
    let scanner = scanner::Scanner::new(root);
    let files = scanner.scan();
    println!("Found {} files to process in {}", files.len(), root);

    let python_parser = PythonParser::new();

    let total_symbols: usize = files.par_iter().map(|path| {
        if let Some(ext) = path.extension() {
            if ext == "py" {
                if let Ok(content) = fs::read_to_string(path) {
                    let result = python_parser.parse(&content);
                    let path_str = path.to_string_lossy().to_string();
                    
                    // Ingestion dans LadybugDB
                    if let Err(e) = graph_store.insert_file_symbols(&path_str, &result.symbols) {
                        eprintln!("Graph error for {}: {}", path_str, e);
                    }
                    
                    // Envoi de l'event via le bridge
                    let _ = Bridge::send_event(socket_path, BridgeEvent::FileIndexed { 
                        path: path_str, 
                        symbol_count: result.symbols.len() 
                    });

                    return result.symbols.len();
                }
            }
        }
        0
    }).sum();

    let duration = start.elapsed();
    println!("Processed {} symbols in {:?}", total_symbols, duration);
    
    // Notification de fin de scan
    let _ = Bridge::send_event(socket_path, BridgeEvent::ScanComplete { 
        total_files: files.len(), 
        duration_ms: duration.as_millis() as u64 
    });

    Ok(())
}
