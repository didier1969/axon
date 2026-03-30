// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
mod main_background;
mod main_services;
mod main_telemetry;

use axon_core::bridge::BridgeEvent;
use axon_core::graph::GraphStore;
use axon_core::queue::QueueStore;
use std::fs;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tracing::{info, error};

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

            main_background::start_memory_watchdog();

            // --- BROADCAST SYSTEM for Telemetry ---
            let (results_tx, _) = tokio::sync::broadcast::channel::<String>(100000);

            // --- HARDWARE-AWARE SCALING ---
            // NEXUS V8.16: Optimized for 16-core host. 14 workers target.
            let num_workers = 14;
            info!("Power Scaling: Sizing worker pool growth to {} threads.", num_workers);

            let db_sender = main_services::start_runtime_services(
                graph_store.clone(),
                queue_store.clone(),
                results_tx.clone(),
                num_workers,
            );

            let projects_root_str = projects_root.to_string();
            let current_boot_id = Arc::new(tokio::sync::Mutex::new(String::new()));
            
            main_background::spawn_autonomous_ingestor(graph_store.clone(), queue_store.clone());

            main_background::spawn_initial_scan(graph_store.clone(), projects_root_str.clone());

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

                main_telemetry::spawn_telemetry_connection(
                    socket,
                    graph_store.clone(),
                    queue_store.clone(),
                    projects_root_str.clone(),
                    current_boot_id.clone(),
                    db_sender.clone(),
                    results_tx.subscribe(),
                    results_tx.clone(),
                );
            }
        })
}
