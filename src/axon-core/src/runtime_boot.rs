use crate::bridge::BridgeEvent;
use crate::embedder::{embedding_lane_config_from_env, SemanticWorkerPool};
use crate::file_ingress_guard::{FileIngressGuard, SharedFileIngressGuard};
use crate::graph::GraphStore;
use crate::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
use crate::main_background;
use crate::main_services;
use crate::main_telemetry;
use crate::queue::QueueStore;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_profile::{
    recommend_embedding_lane_sizing, EmbeddingLaneSizing, RuntimeProfile,
};
use crate::runtime_writer_guard::WriterGuard;
use crate::worker::{DbWriteTask, WorkerPool};
use serde::{Deserialize, Serialize};
use std::fs;
use std::future::pending;
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

fn telemetry_socket_required() -> bool {
    std::env::var("AXON_OPTIONAL_TELEMETRY_SOCKET")
        .ok()
        .map(|value| {
            let trimmed = value.trim();
            !(trimmed.eq_ignore_ascii_case("1")
                || trimmed.eq_ignore_ascii_case("true")
                || trimmed.eq_ignore_ascii_case("yes")
                || trimmed.eq_ignore_ascii_case("on"))
        })
        .unwrap_or(true)
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

fn canonical_effective_embedding_lane_config() -> crate::embedder::EmbeddingLaneConfig {
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

fn split_brain_reader_only_mode() -> bool {
    matches!(
        std::env::var("AXON_SPLIT_BRAIN_IST_READER_ONLY")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("yes") | Some("on")
    ) && matches!(AxonRuntimeMode::from_env(), AxonRuntimeMode::McpOnly)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBootRole {
    Monolith,
    BrainShadow,
    IndexerShadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeBootProfile {
    pub role: RuntimeBootRole,
    pub start_mcp_http: bool,
    pub start_ingestion_workers: bool,
    pub promotable: bool,
    pub operator_default: bool,
    runtime_mode_override: Option<AxonRuntimeMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeBootStatus {
    pub role: RuntimeBootRole,
    pub runtime_mode: String,
    pub operator_default: bool,
    pub shadow_capable: bool,
    pub promotable: bool,
    pub start_mcp_http: bool,
    pub start_ingestion_workers: bool,
}

impl RuntimeBootProfile {
    pub const fn monolith() -> Self {
        Self {
            role: RuntimeBootRole::Monolith,
            start_mcp_http: true,
            start_ingestion_workers: true,
            promotable: true,
            operator_default: true,
            runtime_mode_override: None,
        }
    }

    pub const fn brain_shadow() -> Self {
        Self {
            role: RuntimeBootRole::BrainShadow,
            start_mcp_http: true,
            start_ingestion_workers: false,
            promotable: false,
            operator_default: false,
            runtime_mode_override: Some(AxonRuntimeMode::McpOnly),
        }
    }

    pub const fn indexer_shadow() -> Self {
        Self {
            role: RuntimeBootRole::IndexerShadow,
            start_mcp_http: false,
            start_ingestion_workers: true,
            promotable: false,
            operator_default: false,
            runtime_mode_override: Some(AxonRuntimeMode::Full),
        }
    }

    pub fn runtime_mode(self) -> AxonRuntimeMode {
        self.runtime_mode_override
            .unwrap_or_else(AxonRuntimeMode::from_env)
    }

    pub fn split_status(self) -> RuntimeBootStatus {
        RuntimeBootStatus {
            role: self.role,
            runtime_mode: self.runtime_mode().as_str().to_string(),
            operator_default: self.operator_default,
            shadow_capable: !self.operator_default,
            promotable: self.promotable,
            start_mcp_http: self.start_mcp_http,
            start_ingestion_workers: self.start_ingestion_workers,
        }
    }

    fn writer_targets(self) -> &'static [crate::runtime_writer_guard::WriterTarget] {
        use crate::runtime_writer_guard::WriterTarget;
        match self.role {
            RuntimeBootRole::Monolith => &[WriterTarget::Soll, WriterTarget::Ist],
            RuntimeBootRole::BrainShadow => &[WriterTarget::Soll],
            RuntimeBootRole::IndexerShadow => &[WriterTarget::Ist],
        }
    }
}

pub fn run_monolith() -> anyhow::Result<()> {
    run(RuntimeBootProfile::monolith())
}

pub fn run_brain_shadow() -> anyhow::Result<()> {
    run(RuntimeBootProfile::brain_shadow())
}

pub fn run_indexer_shadow() -> anyhow::Result<()> {
    run(RuntimeBootProfile::indexer_shadow())
}

fn run(profile: RuntimeBootProfile) -> anyhow::Result<()> {
    let runtime_profile = RuntimeProfile::detect();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(runtime_profile.max_blocking_threads)
        .build()
        .unwrap()
        .block_on(async move { boot(profile, runtime_profile).await })
}

async fn boot(profile: RuntimeBootProfile, runtime_profile: RuntimeProfile) -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let boot_time = chrono::Utc::now().to_rfc3339();
    let runtime_mode = profile.runtime_mode();

    if profile.runtime_mode_override.is_some() {
        unsafe {
            std::env::set_var("AXON_RUNTIME_MODE", runtime_mode.as_str());
        }
    }

    let projects_root_env = std::env::var("AXON_PROJECTS_ROOT")
        .unwrap_or_else(|_| "/home/dstadel/projects".to_string());
    let watch_root_env =
        std::env::var("AXON_WATCH_DIR").unwrap_or_else(|_| projects_root_env.clone());
    let projects_root = projects_root_env.leak();
    let watch_root = watch_root_env.leak();
    let db_root_env = std::env::var("AXON_DB_ROOT").unwrap_or_else(|_| {
        std::env::var("HOME")
            .map(|home| format!("{}/.local/share/axon/db", home))
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .map(|dir| format!("{}/.axon/graph_v2", dir.display()))
                    .unwrap_or_else(|_| ".axon/graph_v2".to_string())
            })
    });
    let db_root = db_root_env.leak();

    let package_version = env!("CARGO_PKG_VERSION");
    let release_version =
        std::env::var("AXON_RELEASE_VERSION").unwrap_or_else(|_| package_version.to_string());
    let build_id = std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| package_version.to_string());
    let install_generation =
        std::env::var("AXON_INSTALL_GENERATION").unwrap_or_else(|_| "workspace".to_string());

    info!(
        "Starting Axon Core v{} (package={}, build={}, generation={})",
        release_version, package_version, build_id, install_generation
    );
    info!("Engine Boot Time: {}", boot_time);
    info!(
        "Boot Profile: {}",
        serde_json::to_string(&profile.split_status())?
    );
    info!("Runtime Mode: {:?}", runtime_mode);
    info!(
        "Runtime Profile: cpu_cores={}, ram_total_gb={}, ram_budget_gb={}, ingestion_memory_budget_gb={}, gpu_present={}, workers={}, max_blocking_threads={}, queue_capacity={}",
        runtime_profile.cpu_cores,
        runtime_profile.ram_total_gb,
        runtime_profile.ram_budget_gb,
        runtime_profile.ingestion_memory_budget_gb,
        runtime_profile.gpu_present,
        runtime_profile.recommended_workers,
        runtime_profile.max_blocking_threads,
        runtime_profile.queue_capacity
    );
    if !profile.promotable {
        info!("Split runtime is shadow-only and explicitly non-promotable before Task 6 gates.");
    }
    let provider_requested = canonical_embedding_provider_request(runtime_profile.gpu_present);
    let gpu_execution_requested =
        runtime_profile.gpu_present && provider_requested.eq_ignore_ascii_case("cuda");
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", provider_requested.clone());
        std::env::set_var(
            "AXON_EMBEDDING_GPU_PRESENT",
            if runtime_profile.gpu_present {
                "true"
            } else {
                "false"
            },
        );
    }
    apply_canonical_ort_runtime_env(gpu_execution_requested);
    apply_canonical_ort_thread_defaults_from_openmp();
    apply_canonical_watcher_runtime_env();
    if provider_requested.eq_ignore_ascii_case("cuda") && !runtime_profile.gpu_present {
        warn!(
            "Embedding provider requested CUDA, but no accessible GPU was detected. Axon will run semantic workloads on CPU until GPU access is restored."
        );
    }

    unsafe {
        std::env::set_var(
            "AXON_MEMORY_LIMIT_GB",
            runtime_profile.ram_budget_gb.to_string(),
        );
        std::env::set_var(
            "AXON_QUEUE_MEMORY_BUDGET_BYTES",
            runtime_profile
                .ingestion_memory_budget_gb
                .saturating_mul(1024 * 1024 * 1024)
                .to_string(),
        );
    }

    let mut lane_profile = runtime_profile.clone();
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

    let mut acquired_writer_guards = Vec::new();
    for target in profile.writer_targets() {
        let result = match target {
            crate::runtime_writer_guard::WriterTarget::Soll => WriterGuard::acquire_soll(db_root),
            crate::runtime_writer_guard::WriterTarget::Ist => WriterGuard::acquire_ist(db_root),
        };
        match result {
            Ok(guard) => acquired_writer_guards.push(guard),
            Err(err) => {
                error!("Runtime writer ownership enforcement refused startup: {err:#}");
                return Err(err);
            }
        }
    }
    let _writer_guards = acquired_writer_guards;
    info!(
        "Writer ownership acquired for {:?} under {}",
        profile.writer_targets(),
        std::env::var("AXON_RUNTIME_IDENTITY").unwrap_or_else(|_| "unknown-runtime".to_string())
    );

    let graph_store_result = match profile.role {
        RuntimeBootRole::BrainShadow => GraphStore::new_brain_reader_soll_writer(db_root),
        RuntimeBootRole::IndexerShadow => GraphStore::new_indexer_ist_writer_without_soll(db_root),
        RuntimeBootRole::Monolith => GraphStore::new(db_root),
    };
    let graph_store = match graph_store_result {
        Ok(store) => Arc::new(store),
        Err(e) => {
            error!("Fatal Error initializing DuckDB: {:?}", e);
            return Err(e);
        }
    };
    let queue_store = Arc::new(QueueStore::with_memory_budget(
        runtime_profile.queue_capacity,
        runtime_profile
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
    let file_ingress_guard: SharedFileIngressGuard = Arc::new(Mutex::new(hydrated_guard));
    let ingress_buffer: SharedIngressBuffer = Arc::new(Mutex::new(IngressBuffer::default()));
    let tel_socket_path = std::env::var("AXON_TELEMETRY_SOCK")
        .unwrap_or_else(|_| "/tmp/axon-telemetry.sock".to_string());
    let mcp_socket_path =
        std::env::var("AXON_MCP_SOCK").unwrap_or_else(|_| "/tmp/axon-mcp.sock".to_string());

    if std::path::Path::new(&tel_socket_path).exists() {
        let _ = fs::remove_file(&tel_socket_path);
    }
    if std::path::Path::new(&mcp_socket_path).exists() {
        let _ = fs::remove_file(&mcp_socket_path);
    }

    let tel_listener = match UnixListener::bind(&tel_socket_path) {
        Ok(listener) => Some(listener),
        Err(err) if !telemetry_socket_required() => {
            warn!(
                "Telemetry socket disabled because bind failed for {}: {:?}",
                tel_socket_path, err
            );
            None
        }
        Err(err) => return Err(err.into()),
    };

    let http_port = std::env::var("HYDRA_HTTP_PORT").unwrap_or_else(|_| "44129".to_string());
    if tel_listener.is_some() {
        info!("Telemetry Server listening on {}", tel_socket_path);
    } else {
        warn!("Telemetry Server disabled; unix socket listener unavailable.");
    }
    if profile.start_mcp_http {
        info!("MCP HTTP/SSE Server listening on 127.0.0.1:{}", http_port);
    } else {
        info!("MCP HTTP/SSE Server disabled by boot profile.");
    }

    main_background::start_memory_watchdog();

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

    let num_workers = runtime_profile.recommended_workers;
    info!(
        "Power Scaling: Sizing worker pool growth to {} threads.",
        num_workers
    );

    let db_sender = if profile.start_mcp_http {
        let options = match runtime_mode {
            AxonRuntimeMode::Full => main_services::RuntimeServiceOptions::full(),
            AxonRuntimeMode::GraphOnly => main_services::RuntimeServiceOptions::graph_only(),
            AxonRuntimeMode::ReadOnly | AxonRuntimeMode::McpOnly => {
                main_services::RuntimeServiceOptions::read_only()
            }
        };
        main_services::start_runtime_services(
            graph_store.clone(),
            queue_store.clone(),
            results_tx.clone(),
            num_workers,
            options,
        )
    } else {
        start_indexer_only_services(
            graph_store.clone(),
            queue_store.clone(),
            results_tx.clone(),
            num_workers,
            runtime_mode,
        )
    };

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
    if matches!(profile.role, RuntimeBootRole::BrainShadow) {
        info!("Reader snapshot refresher disabled for split brain reader-only mode.");
    } else {
        main_background::spawn_reader_snapshot_refresher(graph_store.clone());
    }
    main_background::spawn_shadow_optimizer(graph_store.clone());
    main_background::spawn_runtime_trace_logger(
        graph_store.clone(),
        queue_store.clone(),
        ingress_buffer.clone(),
    );

    if let Some(tel_listener) = tel_listener {
        loop {
            let (mut socket, addr) = match tel_listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };

            info!("New Telemetry connection from {:?}", addr);

            let ready_event = BridgeEvent::SystemReady {
                start_time_utc: boot_time.clone(),
            };
            let ready_msg = format!(
                "Axon Telemetry Ready\n{}\n",
                serde_json::to_string(&ready_event).unwrap()
            );
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
    } else {
        pending::<()>().await;
        #[allow(unreachable_code)]
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeBootProfile, RuntimeBootRole};
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_writer_guard::WriterTarget;

    #[test]
    fn split_boot_roles_claim_only_owned_writer_targets() {
        let brain = RuntimeBootProfile::brain_shadow();
        assert_eq!(brain.role, RuntimeBootRole::BrainShadow);
        assert_eq!(brain.writer_targets(), &[WriterTarget::Soll]);
        assert_eq!(brain.runtime_mode(), AxonRuntimeMode::McpOnly);

        let indexer = RuntimeBootProfile::indexer_shadow();
        assert_eq!(indexer.role, RuntimeBootRole::IndexerShadow);
        assert_eq!(indexer.writer_targets(), &[WriterTarget::Ist]);
        assert_eq!(indexer.runtime_mode(), AxonRuntimeMode::Full);

        let monolith = RuntimeBootProfile::monolith();
        assert_eq!(monolith.role, RuntimeBootRole::Monolith);
        assert_eq!(
            monolith.writer_targets(),
            &[WriterTarget::Soll, WriterTarget::Ist]
        );
        assert_eq!(monolith.runtime_mode(), AxonRuntimeMode::Full);
    }
}

fn start_indexer_only_services(
    graph_store: Arc<GraphStore>,
    queue_store: Arc<QueueStore>,
    results_tx: tokio::sync::broadcast::Sender<String>,
    num_workers: usize,
    runtime_mode: AxonRuntimeMode,
) -> crossbeam_channel::Sender<DbWriteTask> {
    let writer_queue_capacity = std::env::var("AXON_WRITER_QUEUE_CAPACITY")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|capacity| *capacity > 0)
        .unwrap_or_else(|| num_workers.saturating_mul(4).clamp(32, 256));
    let (db_tx, db_rx) = crossbeam_channel::bounded(writer_queue_capacity);

    if runtime_mode.ingestion_enabled() {
        info!(
            "Runtime services: writer queue capacity set to {} tasks.",
            writer_queue_capacity
        );
        WorkerPool::spawn_writer_actor(
            graph_store.clone(),
            queue_store.clone(),
            db_rx,
            results_tx.clone(),
        );
        let queue_for_pool = queue_store.clone();
        let store_for_pool = graph_store.clone();
        let results_tx_for_pool = results_tx.clone();
        let db_tx_for_pool = db_tx.clone();

        tokio::task::spawn_blocking(move || {
            WorkerPool::new(
                num_workers,
                queue_for_pool,
                store_for_pool,
                db_tx_for_pool,
                results_tx_for_pool,
            );
        });
    } else {
        info!("Runtime services: indexing workers disabled by runtime mode.");
    }

    if runtime_mode.semantic_workers_enabled() {
        let semantic_store = graph_store.clone();
        let semantic_queue = queue_store.clone();
        tokio::task::spawn_blocking(move || {
            SemanticWorkerPool::new(semantic_store, semantic_queue);
        });
    } else {
        info!("Runtime services: semantic workers disabled by runtime mode.");
    }

    db_tx
}
