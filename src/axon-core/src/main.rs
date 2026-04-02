// Copyright (c) Didier Stadelmann. All rights reserved.
// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
mod main_background;
mod main_services;
mod main_telemetry;

use axon_core::bridge::BridgeEvent;
use axon_core::file_ingress_guard::{FileIngressGuard, SharedFileIngressGuard};
use axon_core::graph::GraphStore;
use axon_core::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
use axon_core::queue::QueueStore;
use axon_core::runtime_profile::RuntimeProfile;
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tracing::{error, info};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeMode {
    Full,
    ReadOnly,
    McpOnly,
}

impl RuntimeMode {
    fn from_env() -> Self {
        match std::env::var("AXON_RUNTIME_MODE")
            .unwrap_or_else(|_| "full".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "read_only" | "readonly" | "read-only" => Self::ReadOnly,
            "mcp_only" | "mcponly" | "mcp-only" => Self::McpOnly,
            _ => Self::Full,
        }
    }

    fn service_options(self) -> main_services::RuntimeServiceOptions {
        match self {
            Self::Full => main_services::RuntimeServiceOptions::full(),
            Self::ReadOnly | Self::McpOnly => main_services::RuntimeServiceOptions::read_only(),
        }
    }

    fn ingestion_enabled(self) -> bool {
        matches!(self, Self::Full)
    }
}

fn main() -> anyhow::Result<()> {
    let profile = RuntimeProfile::detect();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(profile.max_blocking_threads)
        .build()
        .unwrap()
        .block_on(async {
            tracing_subscriber::fmt::init();
            let boot_time = chrono::Utc::now().to_rfc3339();

            let projects_root_env = std::env::var("AXON_PROJECTS_ROOT")
                .unwrap_or_else(|_| "/home/dstadel/projects".to_string());
            let projects_root = projects_root_env.leak();
            let db_root = "/home/dstadel/projects/axon/.axon/graph_v2";
            let runtime_mode = RuntimeMode::from_env();

            info!("Starting Axon Core v2.2 (Nexus Seal - Zero-Sleep Edition)");
            info!("Engine Boot Time: {}", boot_time);
            info!("Runtime Mode: {:?}", runtime_mode);
            info!(
                "Runtime Profile: cpu_cores={}, ram_total_gb={}, ram_budget_gb={}, ingestion_memory_budget_gb={}, gpu_present={}, workers={}, max_blocking_threads={}, queue_capacity={}",
                profile.cpu_cores,
                profile.ram_total_gb,
                profile.ram_budget_gb,
                profile.ingestion_memory_budget_gb,
                profile.gpu_present,
                profile.recommended_workers,
                profile.max_blocking_threads,
                profile.queue_capacity
            );

            unsafe {
                std::env::set_var("AXON_MEMORY_LIMIT_GB", profile.ram_budget_gb.to_string());
                std::env::set_var(
                    "AXON_QUEUE_MEMORY_BUDGET_BYTES",
                    profile
                        .ingestion_memory_budget_gb
                        .saturating_mul(1024 * 1024 * 1024)
                        .to_string(),
                );
            }

            // Initialize KuzuDB (No RwLock needed: MVCC Snapshot Isolation handles concurrency)
            let graph_store = match GraphStore::new(db_root) {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    error!("Fatal Error initializing LadybugDB: {:?}", e);
                    return Err(e);
                }
            };

            let queue_store = Arc::new(QueueStore::with_memory_budget(
                profile.queue_capacity,
                profile
                    .ingestion_memory_budget_gb
                    .saturating_mul(1024 * 1024 * 1024),
            ));
            let file_ingress_guard: SharedFileIngressGuard = Arc::new(Mutex::new(
                FileIngressGuard::hydrate_from_store(&graph_store).unwrap_or_default(),
            ));
            let ingress_buffer: SharedIngressBuffer =
                Arc::new(Mutex::new(IngressBuffer::default()));
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
            main_telemetry::spawn_runtime_telemetry(
                graph_store.clone(),
                queue_store.clone(),
                ingress_buffer.clone(),
                results_tx.clone(),
            );

            let num_workers = profile.recommended_workers;
            info!("Power Scaling: Sizing worker pool growth to {} threads.", num_workers);

            let db_sender = main_services::start_runtime_services(
                graph_store.clone(),
                queue_store.clone(),
                results_tx.clone(),
                num_workers,
                runtime_mode.service_options(),
            );

            let projects_root_str = projects_root.to_string();
            let current_boot_id = Arc::new(tokio::sync::Mutex::new(String::new()));

            if runtime_mode.ingestion_enabled() {
                main_background::spawn_autonomous_ingestor(graph_store.clone(), queue_store.clone());
                main_background::spawn_ingress_promoter(
                    graph_store.clone(),
                    projects_root_str.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );
                main_background::spawn_memory_reclaimer(queue_store.clone(), ingress_buffer.clone());

                main_background::spawn_hot_delta_watcher(
                    graph_store.clone(),
                    projects_root_str.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );

                main_background::spawn_initial_scan(
                    graph_store.clone(),
                    projects_root_str.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );
            } else {
                info!("Ingress, watcher, scan and autonomous ingestion disabled by runtime mode.");
            }

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
