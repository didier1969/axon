use crate::bridge::BridgeEvent;
use crate::embedder::{
    current_gpu_memory_snapshot, embedding_lane_config_from_env, SemanticWorkerPool,
};
use crate::file_ingress_guard::{FileIngressGuard, SharedFileIngressGuard};
use crate::graph::GraphStore;
use crate::ingress_buffer::{IngressBuffer, SharedIngressBuffer};
use crate::main_background;
use crate::main_services;
use crate::main_telemetry;
use crate::queue::QueueStore;
use crate::runtime_mode::canonical_embedding_provider_request_for_mode;
use crate::runtime_mode::graph_embeddings_enabled;
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

fn canonical_embedding_provider_request(
    runtime_mode: AxonRuntimeMode,
    gpu_present: bool,
) -> String {
    canonical_embedding_provider_request_for_mode(runtime_mode, gpu_present)
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

fn graph_first_indexer_lane_sizing(
    profile: RuntimeBootProfile,
    runtime_profile: &RuntimeProfile,
    lane_sizing: EmbeddingLaneSizing,
) -> EmbeddingLaneSizing {
    if profile.role != RuntimeBootRole::Indexer
        || !runtime_profile.gpu_present
        || !graph_embeddings_enabled()
    {
        return lane_sizing;
    }

    let query_workers = 0usize;
    let available_background_workers = runtime_profile
        .recommended_workers
        .saturating_sub(query_workers);
    if available_background_workers <= 1 {
        return lane_sizing;
    }

    let vector_workers = 1usize;
    let graph_workers = available_background_workers
        .saturating_sub(vector_workers)
        .max(1);

    EmbeddingLaneSizing {
        query_workers,
        vector_workers,
        graph_workers,
        chunk_batch_size: lane_sizing.chunk_batch_size.clamp(32, 64),
        file_vectorization_batch_size: lane_sizing.file_vectorization_batch_size.max(48),
        graph_batch_size: lane_sizing.graph_batch_size.max(64),
    }
}

fn apply_graph_first_indexer_memory_defaults(
    profile: RuntimeBootProfile,
    runtime_profile: &RuntimeProfile,
) {
    if profile.role != RuntimeBootRole::Indexer || !runtime_profile.gpu_present {
        return;
    }

    if std::env::var("AXON_GPU_TELEMETRY_BACKEND").is_err() {
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_BACKEND", "nvml");
        }
    }
    if std::env::var("AXON_GPU_TELEMETRY_CACHE_TTL_MS").is_err() {
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS", "250");
        }
    }

    let total_vram_mb = std::env::var("AXON_GPU_TOTAL_VRAM_MB_HINT")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 4_096)
        .or_else(|| current_gpu_memory_snapshot().map(|snapshot| snapshot.total_mb))
        .unwrap_or(8_192);

    let soft_limit_mb = if total_vram_mb <= 8_192 {
        total_vram_mb.saturating_sub(128).max(6_144)
    } else if total_vram_mb <= 12_288 {
        total_vram_mb.saturating_sub(256).max(8_192)
    } else {
        total_vram_mb.saturating_sub((total_vram_mb / 12).max(512))
    };
    let cuda_limit_mb = soft_limit_mb.saturating_sub(128).max(4_096);

    // Respect user-provided env vars: only set defaults when not already configured.
    for (env_name, value) in [
        ("AXON_CUDA_MEMORY_SOFT_LIMIT_MB", soft_limit_mb.to_string()),
        ("AXON_CUDA_MEMORY_LIMIT_MB", cuda_limit_mb.to_string()),
        ("AXON_OPT_MAX_VRAM_USED_MB", soft_limit_mb.to_string()),
        (
            "AXON_GPU_PRIMARY_WORKER_MAX_USED_MB",
            soft_limit_mb.to_string(),
        ),
        ("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED", "true".to_string()),
        ("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true".to_string()),
        // 4 samples x 300ms = 1.2s max probe window. CUDA deallocation is
        // near-instant; ORT BFC arena releases on process kill. 1.2s is
        // sufficient to observe full memory release via NVML.
        ("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES", "4".to_string()),
        // 300ms > 250ms NVML cache TTL, guaranteeing one fresh driver query
        // per sample without wasting CPU on stale cache reads.
        ("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS", "300".to_string()),
        (
            // ORT BFC arena uses power-of-two chunks; smallest meaningful
            // session release is ~128MB. 64MB was within driver noise.
            "AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB",
            "128".to_string(),
        ),
        (
            // Without telemetry, blind embedding risks unified memory spill
            // (40x throughput loss). Conservative default: recycle.
            "AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE",
            "true".to_string(),
        ),
        ("AXON_VECTOR_READY_QUEUE_DEPTH", "48".to_string()),
        ("AXON_VECTOR_TARGET_READY_CHUNKS", (48 * 16).to_string()),
        ("AXON_VECTOR_PREPARE_PIPELINE_DEPTH", "6".to_string()),
        ("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR", "4".to_string()),
        (
            "AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS",
            "50".to_string(),
        ),
        ("AXON_MAX_EMBED_BATCH_BYTES", (512 * 1024).to_string()),
        ("AXON_EMBED_MICRO_BATCH_MAX_ITEMS", "16".to_string()),
        (
            "AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS",
            "2048".to_string(),
        ),
        ("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS", "4096".to_string()),
        ("AXON_SEMANTIC_SLEEP_SCALE_PCT", "10".to_string()),
        ("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT", "10".to_string()),
        ("AXON_GPU_MULTIWORKER_MIN_FREE_MB", "1536".to_string()),
        ("AXON_GPU_TELEMETRY_BACKEND", "nvml".to_string()),
        ("AXON_GPU_TELEMETRY_CACHE_TTL_MS", "250".to_string()),
        ("AXON_GPU_EMBED_SERVICE_ENABLED", "1".to_string()),
        (
            "AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH",
            "0".to_string(),
        ),
        ("AXON_GPU_EMBED_SERVICE_TENSORRT", "1".to_string()),
        ("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT", "true".to_string()),
        (
            "AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT",
            "true".to_string(),
        ),
        ("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT", "96".to_string()),
        ("AXON_GPU_STUCK_RECOVERY_ENABLED", "true".to_string()),
        ("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS", "2500".to_string()),
        ("AXON_GPU_STUCK_RECOVERY_READY_AGE_MS", "5000".to_string()),
    ] {
        if std::env::var(env_name).is_err() {
            unsafe {
                std::env::set_var(env_name, value);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBootRole {
    Brain,
    Indexer,
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
    pub const fn brain() -> Self {
        Self {
            role: RuntimeBootRole::Brain,
            start_mcp_http: true,
            start_ingestion_workers: false,
            promotable: true,
            operator_default: true,
            runtime_mode_override: None,
        }
    }

    pub const fn indexer() -> Self {
        Self {
            role: RuntimeBootRole::Indexer,
            start_mcp_http: false,
            start_ingestion_workers: true,
            promotable: true,
            operator_default: true,
            runtime_mode_override: None,
        }
    }

    pub fn runtime_mode(self) -> AxonRuntimeMode {
        if let Some(runtime_mode) = self.runtime_mode_override {
            return runtime_mode;
        }

        std::env::var("AXON_RUNTIME_MODE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| AxonRuntimeMode::from_str(&value))
            .unwrap_or_else(|| match self.role {
                RuntimeBootRole::Brain => AxonRuntimeMode::BrainOnly,
                RuntimeBootRole::Indexer => AxonRuntimeMode::IndexerFull,
            })
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
            RuntimeBootRole::Brain => &[WriterTarget::Soll],
            RuntimeBootRole::Indexer => &[WriterTarget::Ist],
        }
    }
}

pub fn run_brain() -> anyhow::Result<()> {
    run(RuntimeBootProfile::brain())
}

pub fn run_indexer() -> anyhow::Result<()> {
    run(RuntimeBootProfile::indexer())
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

    apply_graph_first_indexer_memory_defaults(profile, &runtime_profile);

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
    let provider_requested =
        canonical_embedding_provider_request(runtime_mode, runtime_profile.gpu_present);
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
    let lane_sizing = graph_first_indexer_lane_sizing(
        profile,
        &lane_profile,
        recommend_embedding_lane_sizing(&lane_profile),
    );
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
        RuntimeBootRole::Brain => GraphStore::new_brain_reader_soll_writer(db_root),
        RuntimeBootRole::Indexer => GraphStore::new_indexer_ist_writer_without_soll(db_root),
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
            AxonRuntimeMode::BrainOnly => main_services::RuntimeServiceOptions::brain_only(),
            AxonRuntimeMode::IndexerGraph => main_services::RuntimeServiceOptions::indexer_graph(),
            AxonRuntimeMode::IndexerVector => {
                main_services::RuntimeServiceOptions::indexer_vector()
            }
            AxonRuntimeMode::IndexerFull => main_services::RuntimeServiceOptions::indexer_full(),
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
    main_background::spawn_reader_snapshot_refresher(graph_store.clone());
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
    use super::{
        apply_canonical_embedding_lane_sizing_defaults, apply_canonical_ort_runtime_env,
        apply_canonical_ort_thread_defaults_from_openmp, apply_canonical_watcher_runtime_env,
        apply_graph_first_indexer_memory_defaults, canonical_effective_embedding_lane_config,
        canonical_embedding_provider_request, graph_first_indexer_lane_sizing, RuntimeBootProfile,
        RuntimeBootRole,
    };
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_profile::{EmbeddingLaneSizing, RuntimeProfile};
    use crate::runtime_writer_guard::WriterTarget;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cuda_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerFull, true),
            "cuda"
        );
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_cpu_without_gpu() {
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerFull, false),
            "cpu"
        );
    }

    #[test]
    fn canonical_embedding_provider_request_respects_explicit_cpu_override_even_when_gpu_present() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerFull, true),
            "cpu"
        );

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }
    }

    #[test]
    fn canonical_embedding_provider_request_forces_cpu_when_runtime_mode_disables_semantic_workers()
    {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerGraph, true),
            "cpu"
        );
        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::BrainOnly, true),
            "cpu"
        );

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
        crate::runtime_tuning::reset_runtime_tuning_snapshot(
            crate::embedder::bootstrap_runtime_tuning_state(),
        );

        let config = canonical_effective_embedding_lane_config();
        assert_eq!(config.vector_workers, 2);
        assert_eq!(
            std::env::var("AXON_VECTOR_WORKERS").unwrap(),
            "2",
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

    #[test]
    fn split_boot_roles_claim_only_owned_writer_targets() {
        let brain = RuntimeBootProfile::brain();
        assert_eq!(brain.role, RuntimeBootRole::Brain);
        assert_eq!(brain.writer_targets(), &[WriterTarget::Soll]);
        assert_eq!(brain.runtime_mode(), AxonRuntimeMode::BrainOnly);

        let indexer = RuntimeBootProfile::indexer();
        assert_eq!(indexer.role, RuntimeBootRole::Indexer);
        assert_eq!(indexer.writer_targets(), &[WriterTarget::Ist]);
        assert_eq!(indexer.runtime_mode(), AxonRuntimeMode::IndexerFull);

        let duplicate_indexer = RuntimeBootProfile::indexer();
        assert_eq!(duplicate_indexer.role, RuntimeBootRole::Indexer);
        assert_eq!(duplicate_indexer.writer_targets(), &[WriterTarget::Ist]);
        assert_eq!(
            duplicate_indexer.runtime_mode(),
            AxonRuntimeMode::IndexerFull
        );
    }

    #[test]
    fn indexer_shadow_gpu_boot_prefers_graph_first_lane_sizing() {
        let runtime_profile = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 5,
            max_blocking_threads: 8,
            queue_capacity: 100_000,
        };
        let base = EmbeddingLaneSizing {
            query_workers: 1,
            vector_workers: 2,
            graph_workers: 2,
            chunk_batch_size: 96,
            file_vectorization_batch_size: 24,
            graph_batch_size: 8,
        };

        let adjusted =
            graph_first_indexer_lane_sizing(RuntimeBootProfile::indexer(), &runtime_profile, base);

        assert_eq!(adjusted.query_workers, 0);
        assert_eq!(adjusted.vector_workers, 1);
        assert_eq!(adjusted.graph_workers, 4);
        assert_eq!(adjusted.chunk_batch_size, 64);
        assert_eq!(adjusted.file_vectorization_batch_size, 48);
        assert_eq!(adjusted.graph_batch_size, 64);
    }

    #[test]
    fn non_indexer_boot_preserves_base_lane_sizing() {
        let runtime_profile = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 5,
            max_blocking_threads: 8,
            queue_capacity: 100_000,
        };
        let base = EmbeddingLaneSizing {
            query_workers: 1,
            vector_workers: 2,
            graph_workers: 2,
            chunk_batch_size: 96,
            file_vectorization_batch_size: 24,
            graph_batch_size: 8,
        };

        let adjusted =
            graph_first_indexer_lane_sizing(RuntimeBootProfile::brain(), &runtime_profile, base);

        assert_eq!(adjusted, base);
    }

    #[test]
    fn indexer_shadow_gpu_boot_applies_conservative_memory_defaults_for_8gb_gpu() {
        let runtime_profile = RuntimeProfile {
            cpu_cores: 8,
            ram_total_gb: 32,
            ram_budget_gb: 24,
            ingestion_memory_budget_gb: 8,
            gpu_present: true,
            recommended_workers: 5,
            max_blocking_threads: 8,
            queue_capacity: 100_000,
        };

        unsafe {
            std::env::set_var("AXON_GPU_TOTAL_VRAM_MB_HINT", "8192");
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
            std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
            std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
            std::env::remove_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB");
            std::env::remove_var("AXON_MAX_EMBED_BATCH_BYTES");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_GPU_MULTIWORKER_MIN_FREE_MB");
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
            std::env::remove_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_ENABLED");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
            std::env::remove_var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE");
            std::env::remove_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT");
            std::env::remove_var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT");
            std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT");
            std::env::remove_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES");
        }

        apply_graph_first_indexer_memory_defaults(RuntimeBootProfile::indexer(), &runtime_profile);

        assert_eq!(
            std::env::var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB").unwrap(),
            "8064"
        );
        assert_eq!(std::env::var("AXON_CUDA_MEMORY_LIMIT_MB").unwrap(), "7936");
        assert_eq!(std::env::var("AXON_OPT_MAX_VRAM_USED_MB").unwrap(), "8064");
        assert_eq!(
            std::env::var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB").unwrap(),
            "8064"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_READY_QUEUE_DEPTH").unwrap(),
            "48"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES").unwrap(),
            "4"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS").unwrap(),
            "300"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB").unwrap(),
            "128"
        );
        assert_eq!(
            std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH").unwrap(),
            "6"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR").unwrap(),
            "4"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS").unwrap(),
            "50"
        );
        assert_eq!(
            std::env::var("AXON_MAX_EMBED_BATCH_BYTES").unwrap(),
            "524288"
        );
        assert_eq!(
            std::env::var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS").unwrap(),
            "16"
        );
        assert_eq!(
            std::env::var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS").unwrap(),
            "2048"
        );
        assert_eq!(
            std::env::var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS").unwrap(),
            "4096"
        );
        assert_eq!(std::env::var("AXON_GPU_TELEMETRY_BACKEND").unwrap(), "nvml");
        assert_eq!(
            std::env::var("AXON_GPU_TELEMETRY_CACHE_TTL_MS").unwrap(),
            "250"
        );
        assert_eq!(
            std::env::var("AXON_GPU_EMBED_SERVICE_ENABLED").unwrap(),
            "1"
        );
        assert_eq!(
            std::env::var("AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH").unwrap(),
            "0"
        );
        assert_eq!(
            std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT").unwrap(),
            "1"
        );
        assert_eq!(
            std::env::var("AXON_GPU_STUCK_RECOVERY_ENABLED").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT").unwrap(),
            "true"
        );
        assert_eq!(
            std::env::var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT").unwrap(),
            "96"
        );
        assert_eq!(
            std::env::var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS").unwrap(),
            "2500"
        );
        assert_eq!(
            std::env::var("AXON_GPU_STUCK_RECOVERY_READY_AGE_MS").unwrap(),
            "5000"
        );
        assert_eq!(
            std::env::var("AXON_SEMANTIC_SLEEP_SCALE_PCT").unwrap(),
            "10"
        );
        assert_eq!(
            std::env::var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT").unwrap(),
            "10"
        );
        assert_eq!(
            std::env::var("AXON_GPU_MULTIWORKER_MIN_FREE_MB").unwrap(),
            "1536"
        );

        unsafe {
            std::env::remove_var("AXON_GPU_TOTAL_VRAM_MB_HINT");
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
            std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
            std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
            std::env::remove_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH");
            std::env::remove_var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR");
            std::env::remove_var("AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS");
            std::env::remove_var("AXON_MAX_EMBED_BATCH_BYTES");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT");
            std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT");
            std::env::remove_var("AXON_GPU_MULTIWORKER_MIN_FREE_MB");
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
            std::env::remove_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_ENABLED");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_RECYCLE_EVERY_BATCH");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
            std::env::remove_var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB");
            std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE");
            std::env::remove_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT");
            std::env::remove_var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT");
            std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT");
            std::env::remove_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES");
        }
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
        let lane_config = embedding_lane_config_from_env();
        info!(
            "Runtime services: semantic workers enabled (mode={}, graph_embeddings_enabled={}, query_workers={}, vector_workers={}, graph_workers={}).",
            runtime_mode.as_str(),
            graph_embeddings_enabled(),
            lane_config.query_workers,
            lane_config.vector_workers,
            lane_config.graph_workers
        );
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
