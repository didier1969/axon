// Copyright (c) Didier Stadelmann. All rights reserved.
// NEXUS v10.7: Removed jemallocator. Using default system allocator for FFI/ONNX stability.
mod main_background;
mod main_services;
mod main_telemetry;

use axon_core::bridge::BridgeEvent;
use axon_core::embedder::{embedding_lane_config_from_env, EmbeddingLaneConfig};
use axon_core::file_ingress_guard::{FileIngressGuard, SharedFileIngressGuard};
use axon_core::graph::GraphStore;
use axon_core::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
use axon_core::queue::QueueStore;
use axon_core::runtime_mode::AxonRuntimeMode;
use axon_core::runtime_profile::{
    recommend_embedding_lane_sizing, EmbeddingLaneSizing, RuntimeProfile,
};
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tracing::{error, info, warn};

fn results_broadcast_capacity() -> usize {
    const DEFAULT_CAPACITY: usize = 2_048;

    std::env::var("AXON_RESULTS_BROADCAST_CAPACITY")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
        .unwrap_or(DEFAULT_CAPACITY)
}

fn canonical_embedding_provider_request(gpu_present: bool) -> String {
    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if gpu_present {
                "cuda".to_string()
            } else {
                "cpu".to_string()
            }
        })
}

fn canonical_effective_embedding_lane_config() -> EmbeddingLaneConfig {
    let effective = embedding_lane_config_from_env();
    unsafe {
        std::env::set_var(
            "AXON_QUERY_EMBED_WORKERS",
            effective.query_workers.to_string(),
        );
        std::env::set_var("AXON_VECTOR_WORKERS", effective.vector_workers.to_string());
        std::env::set_var("AXON_GRAPH_WORKERS", effective.graph_workers.to_string());
        std::env::set_var(
            "AXON_CHUNK_BATCH_SIZE",
            effective.chunk_batch_size.to_string(),
        );
        std::env::set_var(
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            effective.file_vectorization_batch_size.to_string(),
        );
        std::env::set_var(
            "AXON_GRAPH_BATCH_SIZE",
            effective.graph_batch_size.to_string(),
        );
    }
    effective
}

fn apply_canonical_ort_runtime_env(gpu_execution_requested: bool) {
    if !gpu_execution_requested {
        return;
    }

    if std::env::var("OMP_NUM_THREADS").is_err() {
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "1");
            std::env::set_var("AXON_ORT_OMP_AUTOCONFIGURED", "true");
        }
    }

    if std::env::var("OMP_WAIT_POLICY").is_err() {
        unsafe {
            std::env::set_var("OMP_WAIT_POLICY", "PASSIVE");
        }
    }

    if std::env::var("AXON_ORT_INTRA_THREADS").is_err() {
        if let Ok(omp_threads) = std::env::var("OMP_NUM_THREADS") {
            let omp_threads = omp_threads.trim();
            if !omp_threads.is_empty() {
                unsafe {
                    std::env::set_var("AXON_ORT_INTRA_THREADS", omp_threads);
                    std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
                }
            }
        }
    }

    let wsl_cuda_lib_dir = "/usr/lib/wsl/lib";
    if std::path::Path::new(wsl_cuda_lib_dir).exists() {
        let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let already_present = current
            .split(':')
            .any(|segment| segment.trim() == wsl_cuda_lib_dir);
        if !already_present {
            let next = if current.trim().is_empty() {
                wsl_cuda_lib_dir.to_string()
            } else {
                format!("{wsl_cuda_lib_dir}:{current}")
            };
            unsafe {
                std::env::set_var("LD_LIBRARY_PATH", next);
            }
        }
    }
}

fn apply_canonical_watcher_runtime_env() {
    if std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").is_err() {
        unsafe {
            std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "128");
        }
    }
}

fn apply_canonical_ort_thread_defaults_from_openmp() {
    if std::env::var("AXON_ORT_INTRA_THREADS").is_ok() {
        return;
    }
    let Ok(omp_threads) = std::env::var("OMP_NUM_THREADS") else {
        return;
    };
    let omp_threads = omp_threads.trim();
    if omp_threads.is_empty() {
        return;
    }
    unsafe {
        std::env::set_var("AXON_ORT_INTRA_THREADS", omp_threads);
        std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
    }
}

fn apply_canonical_embedding_lane_sizing_defaults(lane_sizing: &EmbeddingLaneSizing) {
    for (env_name, marker_name, value) in [
        (
            "AXON_QUERY_EMBED_WORKERS",
            "AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED",
            lane_sizing.query_workers.to_string(),
        ),
        (
            "AXON_VECTOR_WORKERS",
            "AXON_VECTOR_WORKERS_AUTOCONFIGURED",
            lane_sizing.vector_workers.to_string(),
        ),
        (
            "AXON_GRAPH_WORKERS",
            "AXON_GRAPH_WORKERS_AUTOCONFIGURED",
            lane_sizing.graph_workers.to_string(),
        ),
        (
            "AXON_CHUNK_BATCH_SIZE",
            "AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.chunk_batch_size.to_string(),
        ),
        (
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            "AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.file_vectorization_batch_size.to_string(),
        ),
        (
            "AXON_GRAPH_BATCH_SIZE",
            "AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED",
            lane_sizing.graph_batch_size.to_string(),
        ),
    ] {
        if std::env::var(env_name).is_err() {
            unsafe {
                std::env::set_var(env_name, value);
                std::env::set_var(marker_name, "true");
            }
        }
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
            let watch_root_env =
                std::env::var("AXON_WATCH_DIR").unwrap_or_else(|_| projects_root_env.clone());
            let projects_root = projects_root_env.leak();
            let watch_root = watch_root_env.leak();
            let db_root_env = std::env::var("AXON_DB_ROOT")
                .unwrap_or_else(|_| {
                    std::env::var("HOME")
                        .map(|home| format!("{}/.local/share/axon/db", home))
                        .unwrap_or_else(|_| {
                            std::env::current_dir()
                                .map(|dir| format!("{}/.axon/graph_v2", dir.display()))
                                .unwrap_or_else(|_| ".axon/graph_v2".to_string())
                        })
                });
            let db_root = db_root_env.leak();
            let runtime_mode = AxonRuntimeMode::from_env();

            let package_version = env!("CARGO_PKG_VERSION");
            let release_version =
                std::env::var("AXON_RELEASE_VERSION").unwrap_or_else(|_| package_version.to_string());
            let build_id =
                std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| package_version.to_string());
            let install_generation = std::env::var("AXON_INSTALL_GENERATION")
                .unwrap_or_else(|_| "workspace".to_string());

            info!(
                "Starting Axon Core v{} (package={}, build={}, generation={})",
                release_version, package_version, build_id, install_generation
            );
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
            let provider_requested = canonical_embedding_provider_request(profile.gpu_present);
            let gpu_execution_requested =
                profile.gpu_present && provider_requested.eq_ignore_ascii_case("cuda");
            unsafe {
                std::env::set_var("AXON_EMBEDDING_PROVIDER", provider_requested.clone());
                std::env::set_var(
                    "AXON_EMBEDDING_GPU_PRESENT",
                    if profile.gpu_present { "true" } else { "false" },
                );
            }
            apply_canonical_ort_runtime_env(gpu_execution_requested);
            apply_canonical_ort_thread_defaults_from_openmp();
            apply_canonical_watcher_runtime_env();
            if provider_requested.eq_ignore_ascii_case("cuda") && !profile.gpu_present {
                warn!(
                    "Embedding provider requested CUDA, but no accessible GPU was detected. Axon will run semantic workloads on CPU until GPU access is restored."
                );
            }

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

            let mut lane_profile = profile.clone();
            lane_profile.gpu_present = gpu_execution_requested;
            let lane_sizing = recommend_embedding_lane_sizing(&lane_profile);
            apply_canonical_embedding_lane_sizing_defaults(&lane_sizing);
            let effective_lane_sizing = canonical_effective_embedding_lane_config();
            info!(
                "Embedding lane sizing: query_workers={}, vector_workers={}, graph_workers={}, chunk_batch_size={}, file_vectorization_batch_size={}, graph_batch_size={}",
                effective_lane_sizing.query_workers,
                effective_lane_sizing.vector_workers,
                effective_lane_sizing.graph_workers,
                effective_lane_sizing.chunk_batch_size,
                effective_lane_sizing.file_vectorization_batch_size,
                effective_lane_sizing.graph_batch_size
            );

            // Initialize KuzuDB (No RwLock needed: MVCC Snapshot Isolation handles concurrency)
            let graph_store = match GraphStore::new(db_root) {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    error!("Fatal Error initializing DuckDB: {:?}", e);
                    return Err(e);
                }
            };

            let queue_store = Arc::new(QueueStore::with_memory_budget(
                profile.queue_capacity,
                profile
                    .ingestion_memory_budget_gb
                    .saturating_mul(1024 * 1024 * 1024),
            ));
            let hydrated_guard = match FileIngressGuard::hydrate_from_store(&graph_store) {
                Ok(guard) => guard,
                Err(err) => {
                    warn!(
                        "File ingress guard hydration failed at startup: {:?}. Falling back to empty in-memory guard (still enabled).",
                        err
                    );
                    FileIngressGuard::default()
                }
            };
            let file_ingress_guard: SharedFileIngressGuard =
                Arc::new(Mutex::new(hydrated_guard));
            let ingress_buffer: SharedIngressBuffer =
                Arc::new(Mutex::new(IngressBuffer::default()));
            let tel_socket_path = std::env::var("AXON_TELEMETRY_SOCK")
                .unwrap_or_else(|_| "/tmp/axon-telemetry.sock".to_string());
            let mcp_socket_path = std::env::var("AXON_MCP_SOCK")
                .unwrap_or_else(|_| "/tmp/axon-mcp.sock".to_string());

            if std::path::Path::new(&tel_socket_path).exists() {
                let _ = fs::remove_file(&tel_socket_path);
            }
            if std::path::Path::new(&mcp_socket_path).exists() {
                let _ = fs::remove_file(&mcp_socket_path);
            }

            let tel_listener = UnixListener::bind(&tel_socket_path)?;

            let http_port = std::env::var("HYDRA_HTTP_PORT").unwrap_or_else(|_| "44129".to_string());
            info!("Telemetry Server listening on {}", tel_socket_path);
            info!("MCP HTTP/SSE Server listening on 127.0.0.1:{}", http_port);

            main_background::start_memory_watchdog();

            // --- BROADCAST SYSTEM for Telemetry ---
            let results_capacity = results_broadcast_capacity();
            info!(
                "Bridge broadcast capacity configured to {} messages.",
                results_capacity
            );
            let (results_tx, _) = tokio::sync::broadcast::channel::<String>(results_capacity);
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
                match runtime_mode {
                    AxonRuntimeMode::Full => main_services::RuntimeServiceOptions::full(),
                    AxonRuntimeMode::GraphOnly => main_services::RuntimeServiceOptions::graph_only(),
                    AxonRuntimeMode::ReadOnly | AxonRuntimeMode::McpOnly => {
                        main_services::RuntimeServiceOptions::read_only()
                    }
                },
            );

            let projects_root_str = projects_root.to_string();
            let watch_root_str = watch_root.to_string();
            let current_boot_id = Arc::new(tokio::sync::Mutex::new(String::new()));

            if runtime_mode.ingestion_enabled() {
                main_background::spawn_autonomous_ingestor(graph_store.clone(), queue_store.clone());
                main_background::spawn_ingress_promoter(
                    graph_store.clone(),
                    watch_root_str.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );
                main_background::spawn_memory_reclaimer(queue_store.clone(), ingress_buffer.clone());

                main_background::spawn_federation_orchestrator(
                    graph_store.clone(),
                    file_ingress_guard.clone(),
                    ingress_buffer.clone(),
                );
            } else {
                info!("Ingress, watcher, scan and autonomous ingestion disabled by runtime mode.");
            }
            main_background::spawn_reader_snapshot_refresher(graph_store.clone());
            main_background::spawn_shadow_optimizer(graph_store.clone());
            main_background::spawn_runtime_trace_logger(
                graph_store.clone(),
                queue_store.clone(),
                ingress_buffer.clone(),
            );

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

#[cfg(test)]
mod tests {
    use super::{
        apply_canonical_embedding_lane_sizing_defaults, apply_canonical_ort_runtime_env,
        apply_canonical_ort_thread_defaults_from_openmp, apply_canonical_watcher_runtime_env,
        canonical_effective_embedding_lane_config, canonical_embedding_provider_request,
    };
    use axon_core::runtime_profile::EmbeddingLaneSizing;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cuda_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(canonical_embedding_provider_request(true), "cuda");
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cpu_without_gpu() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(canonical_embedding_provider_request(false), "cpu");
    }

    #[test]
    fn canonical_embedding_provider_request_respects_explicit_cpu_override_even_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        }

        assert_eq!(canonical_embedding_provider_request(true), "cpu");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }
    }

    #[test]
    fn canonical_effective_embedding_lane_config_caps_gpu_vector_workers_in_env() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "2");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }

        let config = canonical_effective_embedding_lane_config();
        assert_eq!(config.vector_workers, 1);
        assert_eq!(
            std::env::var("AXON_VECTOR_WORKERS").unwrap(),
            "1",
            "L'environnement doit exposer le sizing effectif et non le sizing recommande"
        );

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
        }
    }

    #[test]
    fn apply_canonical_embedding_lane_sizing_defaults_marks_autoconfigured_values() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
        }

        apply_canonical_embedding_lane_sizing_defaults(&EmbeddingLaneSizing {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 0,
            chunk_batch_size: 64,
            file_vectorization_batch_size: 24,
            graph_batch_size: 8,
        });

        assert_eq!(
            std::env::var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "true"
        );

        unsafe {
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_sets_gpu_safe_openmp_defaults() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }

        apply_canonical_ort_runtime_env(true);

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "1");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "PASSIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "1");
        assert_eq!(
            std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").unwrap(),
            "true"
        );
        if std::path::Path::new("/usr/lib/wsl/lib").exists() {
            assert!(std::env::var("LD_LIBRARY_PATH")
                .unwrap_or_default()
                .split(':')
                .any(|segment| segment == "/usr/lib/wsl/lib"));
        }

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_preserves_explicit_openmp_configuration() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "3");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::set_var("LD_LIBRARY_PATH", "/tmp/custom-lib");
        }

        apply_canonical_ort_runtime_env(true);

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "4");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "ACTIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "3");
        assert!(std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());
        let ld_library_path = std::env::var("LD_LIBRARY_PATH").unwrap();
        assert!(ld_library_path.contains("/tmp/custom-lib"));
        if std::path::Path::new("/usr/lib/wsl/lib").exists() {
            assert!(ld_library_path
                .split(':')
                .any(|segment| segment == "/usr/lib/wsl/lib"));
        }

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }
    }

    #[test]
    fn apply_canonical_ort_runtime_env_leaves_cpu_hosts_unchanged() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("LD_LIBRARY_PATH");
        }

        apply_canonical_ort_runtime_env(false);

        assert!(
            std::env::var("OMP_NUM_THREADS").is_err(),
            "CPU hosts should not receive GPU-specific OpenMP overrides by default"
        );
        assert!(
            std::env::var("OMP_WAIT_POLICY").is_err(),
            "CPU hosts should not receive GPU-specific OpenMP overrides by default"
        );
        assert!(std::env::var("AXON_ORT_OMP_AUTOCONFIGURED").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS").is_err());
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());
        assert!(
            std::env::var("LD_LIBRARY_PATH").is_err(),
            "CPU hosts should not receive GPU-specific loader overrides by default"
        );
    }

    #[test]
    fn apply_canonical_ort_thread_defaults_from_openmp_sets_missing_ort_threads() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }

        apply_canonical_ort_thread_defaults_from_openmp();

        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "4");
        assert_eq!(
            std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").unwrap(),
            "true"
        );

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }
    }

    #[test]
    fn apply_canonical_ort_thread_defaults_from_openmp_preserves_explicit_ort_threads() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", "4");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "3");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
        }

        apply_canonical_ort_thread_defaults_from_openmp();

        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "3");
        assert!(std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED").is_err());

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
        }
    }

    #[test]
    fn apply_canonical_watcher_runtime_env_sets_default_budget() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }

        apply_canonical_watcher_runtime_env();

        assert_eq!(
            std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").unwrap(),
            "128"
        );

        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }
    }

    #[test]
    fn apply_canonical_watcher_runtime_env_preserves_explicit_budget() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "32");
        }

        apply_canonical_watcher_runtime_env();

        assert_eq!(
            std::env::var("AXON_WATCHER_SUBTREE_HINT_BUDGET").unwrap(),
            "32"
        );

        unsafe {
            std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        }
    }
}
