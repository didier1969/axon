use crate::bridge::BridgeEvent;
use crate::embedder::{
    current_gpu_memory_snapshot, embedding_lane_config_from_env, SemanticWorkerPool,
};
use crate::graph::GraphStore;
use crate::main_background;
use crate::main_services;
use crate::main_telemetry;
use crate::queue::QueueStore;
use crate::runtime_mode::canonical_embedding_provider_request_for_mode;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_profile::{
    recommend_embedding_lane_sizing, EmbeddingLaneSizing, RuntimeProfile,
};
use crate::runtime_writer_guard::WriterGuard;
// REQ-AXO-901653 slice-5c — v1 `worker::{DbWriteTask, WorkerPool}` retired.
// Pipeline_v2 (REQ-AXO-289 / CPT-AXO-054) owns the ingestion path.
use serde::{Deserialize, Serialize};
use std::fs;
use std::future::pending;
use std::sync::Arc;
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
    if profile.role != RuntimeBootRole::Indexer || !runtime_profile.gpu_present {
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
        // DEC-AXO-070 commit G: graph workers MUST NOT load BGE-Large.
        // 4× workers competing for VRAM cascade-OOM into CPU fallback,
        // saturating CPU and starving the vector lane. The graph projection
        // structure (Symbol nodes, relationships, Chunks) is the canonical
        // value; embedding those is delegated to the single-worker vector
        // lane. Operators can re-enable explicitly if they need legacy
        // graph-embedding parity.
        ("AXON_GRAPH_EMBEDDINGS_ENABLED", "false".to_string()),
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

/// REQ-AXO-901869 A1 — thin `JsonSqlStore` adapter so the brain can
/// cold-load IST CSR snapshots at boot via `ist_snapshot::load_snapshot`.
struct BootWarmSqlStore<'a>(&'a GraphStore);

impl crate::ist_snapshot::loader::JsonSqlStore for BootWarmSqlStore<'_> {
    fn query_json(&self, sql: &str) -> Result<String, String> {
        self.0.query_json(sql).map_err(|e| e.to_string())
    }
}

/// REQ-AXO-902005 — owned (`Arc`-backed, `Send + Sync`) `JsonSqlStore` adapter
/// for the `ist_mutated` listener's async serve-stale rebuild. Unlike the
/// lifetime-bound `BootWarmSqlStore`, this moves into a spawned task and is
/// shared across rebuilds.
struct ListenerRefreshSqlStore(Arc<GraphStore>);

impl crate::ist_snapshot::loader::JsonSqlStore for ListenerRefreshSqlStore {
    fn query_json(&self, sql: &str) -> Result<String, String> {
        self.0.query_json(sql).map_err(|e| e.to_string())
    }
}

/// REQ-AXO-901869 A1 — distinct project codes carrying IST symbols, parsed
/// from the `query_json` 2-D array. Pure, so the parse is unit-tested
/// without a live GraphStore.
fn parse_boot_warm_project_codes(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<Vec<serde_json::Value>>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            row.into_iter()
                .next()
                .and_then(|value| value.as_str().map(str::to_string))
        })
        .filter(|code| !code.is_empty())
        .collect()
}

/// REQ-AXO-901869 A1 — warm the RAM IST snapshot for every IST-bearing
/// project at brain boot. REQ-AXO-901952 made RAM unconditional (no opt-out),
/// so boot always warms ; per-project failures log and leave that project's
/// snapshot cold (callers then surface a loud degraded error, never a silent
/// 0). Runs on a blocking thread so it never stalls the async runtime at boot.
fn warm_all_ist_snapshots_at_boot(graph_store: Arc<GraphStore>) {
    tokio::task::spawn_blocking(move || {
        let store = BootWarmSqlStore(&graph_store);
        let raw = match store.0.query_json(
            "SELECT DISTINCT project_code FROM ist.Symbol \
             WHERE project_code IS NOT NULL ORDER BY project_code",
        ) {
            Ok(raw) => raw,
            Err(err) => {
                warn!(error = %err, "REQ-AXO-901869 A1: boot warm project enumeration failed");
                return;
            }
        };
        for project in parse_boot_warm_project_codes(&raw) {
            match crate::ist_snapshot::load_snapshot(&store, &project) {
                Ok((graph, stats)) => {
                    crate::ist_snapshot::publish_process_snapshot(project.clone(), Arc::new(graph));
                    info!(
                        project = %project,
                        nodes = stats.nodes_loaded,
                        edges = stats.edges_loaded,
                        "REQ-AXO-901869 A1: warmed IST snapshot at boot"
                    );
                }
                Err(err) => warn!(
                    project = %project,
                    error = %err,
                    "REQ-AXO-901869 A1: boot warm failed (PG fallback remains)"
                ),
            }
        }
    });
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

/// REQ-AXO-901728: rolling on-disk tracing with strict retention.
/// WARN+ERROR sink is always-on (last ~24h, HOURLY × 24) so post-mortem
/// is possible without re-running. INFO sink is opt-in via
/// `AXON_INFO_LOG_FILE=1` (last ~20 min, MINUTELY × 20) — disk-quiet by
/// default ; operators flip the toggle only when actively debugging.
/// stdout is preserved for tmux/console visibility regardless. Files
/// land in `$AXON_RUN_ROOT` (set by the launch script per role). If
/// `AXON_RUN_ROOT` is unset (tests, ad-hoc runs), file sinks are
/// skipped and only stdout remains. Operators enable per-module DEBUG
/// ad-hoc via `RUST_LOG=axon_core::<module>=debug`.
fn init_runtime_tracing() {
    use tracing_subscriber::{
        filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let info_log_enabled = std::env::var("AXON_INFO_LOG_FILE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);

    let (info_appender, error_appender) = match std::env::var("AXON_RUN_ROOT") {
        Ok(run_root) => {
            let run_root = std::path::PathBuf::from(run_root);
            if std::fs::create_dir_all(&run_root).is_ok() {
                let info = if info_log_enabled {
                    tracing_appender::rolling::Builder::new()
                        .rotation(tracing_appender::rolling::Rotation::MINUTELY)
                        .filename_prefix("info")
                        .filename_suffix("log")
                        .max_log_files(20)
                        .build(&run_root)
                        .ok()
                } else {
                    None
                };
                let errors = tracing_appender::rolling::Builder::new()
                    .rotation(tracing_appender::rolling::Rotation::HOURLY)
                    .filename_prefix("errors")
                    .filename_suffix("log")
                    .max_log_files(24)
                    .build(&run_root)
                    .ok();
                (info, errors)
            } else {
                (None, None)
            }
        }
        Err(_) => (None, None),
    };

    let info_layer = info_appender.map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_writer(appender)
            .with_ansi(false)
            .with_filter(LevelFilter::INFO)
    });
    let error_layer = error_appender.map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_writer(appender)
            .with_ansi(false)
            .with_filter(LevelFilter::WARN)
    });

    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(info_layer)
        .with(error_layer)
        .try_init();
}

async fn boot(profile: RuntimeBootProfile, runtime_profile: RuntimeProfile) -> anyhow::Result<()> {
    init_runtime_tracing();
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
    // REQ-AXO-901737 : AXON_EMBEDDING_PROVIDER remains the only env var
    // (operator-facing request). gpu_present moves to the in-process
    // diagnostics struct instead of AXON_EMBEDDING_GPU_PRESENT.
    unsafe {
        std::env::set_var("AXON_EMBEDDING_PROVIDER", provider_requested.clone());
    }
    crate::embedder::set_gpu_present(runtime_profile.gpu_present);
    apply_canonical_ort_runtime_env(gpu_execution_requested);
    apply_canonical_ort_thread_defaults_from_openmp();
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

    // REQ-AXO-128 / DEC-AXO-061 — spawn the in-process CPU query
    // embedding worker when the runtime profile does not own a GPU
    // subprocess (brain_only, indexer_graph). The worker registers
    // itself as the canonical query_embedding_sender so batch_embed
    // routes through it transparently. No-op for indexer_vector /
    // indexer_full where the SemanticWorkerPool spawns its own
    // GPU-backed worker via the canonical pipeline.
    crate::embedder::spawn_brain_query_worker_if_needed(runtime_mode);

    // REQ-AXO-098 / DEC-AXO-062 — initial subsystem readiness
    // reports. Each role declares its primary subsystem(s) Ready at
    // boot completion; failures detected after this point flip the
    // subsystem to Degraded or Failed via the relevant code paths
    // (e.g. embedder model load failure flips Embedder to Failed
    // inside query_worker_loop). The empty-registry fresh-boot state
    // collapses to Ready per CPT-AXO-023; the explicit reports here
    // make the readiness signal observable from the first status
    // call onward, not just after the first request.
    match profile.role {
        RuntimeBootRole::Brain => {
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::BrainMcp,
                crate::runtime_readiness::SubsystemState::Ready,
            );
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::IstReader,
                crate::runtime_readiness::SubsystemState::Ready,
            );
            // REQ-AXO-097 — opt brain subsystems into watchdog
            // staleness supervision and start their heartbeat
            // tasks. A panic in the BrainMcp tokio runtime will
            // freeze the heartbeater, the watchdog will observe
            // the staleness, and `mcp__axon__status` will report
            // Failed instead of HEALTHY.
            crate::runtime_watchdog::wire_brain_role_heartbeats();
        }
        RuntimeBootRole::Indexer => {
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::IstWriter,
                crate::runtime_readiness::SubsystemState::Ready,
            );
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::Watcher,
                crate::runtime_readiness::SubsystemState::Ready,
            );
            crate::runtime_watchdog::wire_indexer_role_heartbeats();
        }
    }
    // REQ-AXO-097 — spawn the watchdog tick task once both roles
    // have wired their heartbeaters. Idempotent across re-init.
    crate::runtime_watchdog::spawn_watchdog_task(crate::runtime_watchdog::DEFAULT_TICK_INTERVAL_MS);

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
            error!("Fatal Error initializing GraphStore: {:?}", e);
            return Err(e);
        }
    };

    // REQ-AXO-901806 F2 — Indexer writes its runtime config (worker
    // counts, batch sizes, NOTIFY channel, coldstart cadence) once at
    // boot so `dashboard_state_full(ttl)` PG function can return the
    // composite dashboard envelope without 15+ args traveling through
    // `main_telemetry → compose_dashboard_state_v1`. Best-effort: a
    // PG failure here doesn't abort boot — dashboard degrades to
    // empty `runtime_config` block.
    if profile.role == RuntimeBootRole::Indexer {
        if let Err(err) = crate::runtime_config::write_indexer_config_snapshot(&graph_store) {
            warn!(
                "runtime_config_snapshot write failed at boot: {err:#}. Dashboard runtime_config will read empty until next successful write."
            );
        } else {
            info!("runtime_config_snapshot written (indexer role).");
        }
    }

    let queue_store = Arc::new(QueueStore::with_memory_budget(
        runtime_profile.queue_capacity,
        runtime_profile
            .ingestion_memory_budget_gb
            .saturating_mul(1024 * 1024 * 1024),
    ));
    // REQ-AXO-901893 (LEGACY FEED PURGE) — the FileIngressGuard + in-memory
    // ingress_buffer that the notify watcher / scanner pushed into are gone.
    // Watchman feeds pipeline A's input_tx directly; DBQ-A drains the backlog.
    let tel_socket_path = std::env::var("AXON_TELEMETRY_SOCK")
        .unwrap_or_else(|_| "/tmp/axon-telemetry.sock".to_string());
    let mcp_socket_path =
        std::env::var("AXON_MCP_SOCK").unwrap_or_else(|_| "/tmp/axon-mcp.sock".to_string());

    // REQ-AXO-901835 patch 3 — fail-loud sur bind collision. Avant ce
    // patch les `fs::remove_file` ci-dessus étaient silent (`let _ =`),
    // ce qui orphelinait le listener du voisin (brain ou indexer) si les
    // deux processus partageaient un path identique exporté depuis le
    // shell parent (cf. patches 1+2 commit db422574). Désormais on warn
    // explicitement chaque fois qu'un sock préexistant est supprimé : si
    // un peer encore vivant écoutait dessus, le warn surface dans les
    // logs et l'opérateur sait que la collision s'est produite.
    if std::path::Path::new(&tel_socket_path).exists() {
        match fs::remove_file(&tel_socket_path) {
            Ok(()) => warn!(
                socket = %tel_socket_path,
                "telemetry socket pre-existed at boot; removed before bind (potential brain/indexer collision — verify per-role AXON_TELEMETRY_SOCK env override)"
            ),
            Err(err) => warn!(
                socket = %tel_socket_path,
                error = %err,
                "telemetry socket pre-existed at boot but remove failed; bind may fail"
            ),
        }
    }
    if std::path::Path::new(&mcp_socket_path).exists() {
        match fs::remove_file(&mcp_socket_path) {
            Ok(()) => warn!(
                socket = %mcp_socket_path,
                "mcp socket pre-existed at boot; removed before bind (potential brain/indexer collision — verify per-role AXON_MCP_SOCK env override)"
            ),
            Err(err) => warn!(
                socket = %mcp_socket_path,
                error = %err,
                "mcp socket pre-existed at boot but remove failed; bind may fail"
            ),
        }
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

    let http_port = std::env::var("AXON_BRAIN_PORT").unwrap_or_else(|_| "44129".to_string());
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
        results_tx.clone(),
    );
    // Recompute the AXON_WATCH_DIR filesystem counters off-runtime so the
    // (potentially multi-second) tree walk never blocks the 1 Hz telemetry
    // loop / runtime heartbeat — root cause of watchdog no_telemetry_window
    // flips + apparent MCP/dashboard unresponsiveness.
    crate::mcp::tools_system::spawn_fs_counter_refresher();

    let num_workers = runtime_profile.recommended_workers;
    info!(
        "Power Scaling: Sizing worker pool growth to {} threads.",
        num_workers
    );

    // REQ-AXO-901653 slice-5c — `db_sender` removed (v1 writer-actor retired).
    // Pipeline_v2 (REQ-AXO-289) writes via GraphStore directly.
    if profile.start_mcp_http {
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
        );
    } else {
        start_indexer_only_services(
            graph_store.clone(),
            queue_store.clone(),
            results_tx.clone(),
            num_workers,
            runtime_mode,
        );
    }

    // REQ-AXO-901869 A1 — when this process serves MCP reads (brain), warm
    // the in-RAM IstGraphView CSR snapshot for every IST-bearing project at
    // boot, so the first impact / retrieve_context / query calls dispatch to
    // the canonical RAM graph (PIL-AXO-9002) instead of the degraded PG
    // fallback. Best-effort + off the async runtime (spawn_blocking); on
    // failure the PG fallback remains (correct post REQ-AXO-901869 A3).
    if profile.start_mcp_http {
        warm_all_ist_snapshots_at_boot(graph_store.clone());
    }

    let projects_root_str = projects_root.to_string();
    let watch_root_str = watch_root.to_string();
    let current_boot_id = Arc::new(tokio::sync::Mutex::new(String::new()));

    if runtime_mode.ingestion_enabled() {
        // REQ-AXO-289 S7 / DEC-AXO-081 — streaming pipeline v2 replaces
        // the DuckDB-era public.File state machine. spawn_pipeline_v2_indexer
        // boots A1→A2→A3 (and B1→B2→B3 when semantic workers are enabled),
        // feeds them from the Watchman file source + the DBQ-A claim feeder
        // (REQ-AXO-901893 / REQ-AXO-901897), and resolves project_code per file.
        // The legacy notify watcher + federation/scope orchestrators that pushed
        // into the in-memory ingress_buffer were RIPPED in the LEGACY FEED PURGE.
        if let Err(err) = crate::pipeline_v2_runtime::spawn_pipeline_v2_indexer(
            runtime_mode,
            graph_store.clone(),
            watch_root_str.clone(),
        ) {
            warn!(error = %err, "pipeline_v2_runtime: failed to spawn streaming indexer");
        }
        main_background::spawn_memory_reclaimer(queue_store.clone());
    } else {
        info!("Scan and autonomous ingestion disabled by runtime mode.");
    }

    // REQ-AXO-901658 — wire the `ist_mutated` LISTEN/NOTIFY consumer that
    // was DEFINED (REQ-AXO-91487) but never spawned. The PG triggers in
    // `db/ddl/05_ist_notify.sql` fire `pg_notify('ist_mutated', ...)`
    // on every `ist.symbol` / `ist.edge` mutation. The listener
    // evicts the affected project from the process `IstSnapshotCache` ;
    // the next MCP call cold-loads a fresh CSR snapshot from PG.
    //
    // Without this wire, brain in split-topology (`brain_only` + separate
    // indexer process) NEVER refreshes its in-RAM IST after boot. Session
    // 51 diagnosis : indexer wrote +1560 `IndexedFile` rows to PG over
    // hours while brain MCP kept serving the boot-time snapshot. User-
    // visible symptom : "Axon does not index" (false — it indexes, but
    // brain cannot see the writes).
    // REQ-AXO-901657 slice 4 cluster A : canonical = AXON_INSTANCE
    // (alias AXON_INSTANCE_KIND still honored with one-shot warn).
    match crate::postgres::database_url_for(
        match crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "live")
            .to_lowercase()
            .as_str()
        {
            "dev" => crate::postgres::AxonInstance::Dev,
            _ => crate::postgres::AxonInstance::Live,
        },
    ) {
        Ok(_url) => {
            // REQ-AXO-902005 — the listener now refreshes serve-stale (async
            // rebuild + atomic swap) instead of evicting, so it needs a store
            // handle to cold-load the fresh CSR off the hot path.
            let refresh_store: Arc<dyn crate::ist_snapshot::loader::JsonSqlStore + Send + Sync> =
                Arc::new(ListenerRefreshSqlStore(graph_store.clone()));
            crate::ist_snapshot::notify_listener::spawn_ist_mutation_listener(
                _url.clone(),
                refresh_store,
            );
            info!("ist_mutated listener spawned (REQ-AXO-901658/902005) — IST cache serve-stale async refresh wired");
            // REQ-AXO-901893 (LEGACY FEED PURGE) — the axon_registry_changed
            // listener (REQ-AXO-901675) was RIPPED with ingress_buffer: it
            // pushed an IngressSource::Scan subtree hint into the in-memory
            // ingress_buffer, which no longer exists. The PG trigger in
            // `db/ddl/07_registry_notify.sql` still fires; live new-project
            // discovery now relies on the next indexer restart (Watchman
            // resolves all watch_root roots at boot, DBQ-A drains the
            // 'discovered' backlog by construction). Tracked: REQ-AXO-901899.
        }
        Err(err) => {
            warn!(
                error = %err,
                "ist_mutated listener disabled: PG URL unresolved ; \
                 IST cache will stay stale across mutations"
            );
        }
    }

    // DEC-AXO-901631 — the predictive shadow optimizer was retired (the
    // sorted-drain makes embed throughput correct-by-construction). Only the
    // runtime trace telemetry logger remains.
    main_background::spawn_runtime_trace_logger(graph_store.clone(), queue_store.clone());

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
        apply_canonical_ort_thread_defaults_from_openmp, apply_graph_first_indexer_memory_defaults,
        canonical_effective_embedding_lane_config, canonical_embedding_provider_request,
        graph_first_indexer_lane_sizing, parse_boot_warm_project_codes, RuntimeBootProfile,
        RuntimeBootRole,
    };
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_profile::{EmbeddingLaneSizing, RuntimeProfile};
    use crate::runtime_writer_guard::WriterTarget;

    /// REQ-AXO-901869 A1 — the boot-warm project enumeration tolerates the
    /// `query_json` 2-D array shape, filters null/empty codes, and degrades
    /// to empty on malformed input (best-effort: a parse failure must not
    /// crash boot — the PG fallback stays correct).
    #[test]
    fn parse_boot_warm_project_codes_extracts_nonempty_first_column() {
        assert_eq!(
            parse_boot_warm_project_codes("[[\"AXO\"],[\"BKS\"]]"),
            vec!["AXO".to_string(), "BKS".to_string()]
        );
        assert_eq!(
            parse_boot_warm_project_codes("[[null],[\"\"],[\"AXO\"]]"),
            vec!["AXO".to_string()]
        );
        assert!(parse_boot_warm_project_codes("not json").is_empty());
        assert!(parse_boot_warm_project_codes("[]").is_empty());
    }

    /// REQ-AXO-099 Phase 1 — delegate to the crate-wide
    /// `test_support::env_test_lock` so runtime_boot env-mutating
    /// tests serialize against optimizer (and any future) env-mutating
    /// tests, not just against each other. Without this, a leak from
    /// e.g. apply_graph_first_indexer_memory_defaults_* contaminates
    /// optimizer::tests::* between modules.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::env_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn canonical_embedding_provider_request_defaults_to_tensorrt_when_gpu_present() {
        // REQ-AXO-901737 / operator directive 2026-05-24 : two-value world
        // (cpu | tensorrt). Default for a detected GPU is tensorrt.
        let _guard = env_lock();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerFull, true),
            "tensorrt"
        );
    }

    #[test]
    fn canonical_embedding_provider_request_normalises_cuda_to_tensorrt() {
        let _guard = env_lock();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        }

        assert_eq!(
            canonical_embedding_provider_request(AxonRuntimeMode::IndexerFull, true),
            "tensorrt"
        );

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }
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
    fn split_boot_roles_claim_only_owned_writer_targets() {
        // REQ-AXO-099 Phase 4 — `runtime_mode()` reads
        // AXON_RUNTIME_MODE; a prior test in the suite leaks
        // values like "indexer_full" through it. Lock + unset so
        // this test sees only the role-default fallback.
        let _lock = env_lock();
        let _g_mode = crate::test_support::EnvVarGuard::unset("AXON_RUNTIME_MODE");

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
        let _guard = env_lock();
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

        // DEC-AXO-070 commit G: graph_embeddings_enabled defaults to false.
        // This test exercises the legacy graph-first sizing path, so we
        // explicitly opt back in for the duration of the assertion.
        unsafe {
            std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        }

        let adjusted =
            graph_first_indexer_lane_sizing(RuntimeBootProfile::indexer(), &runtime_profile, base);

        unsafe {
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        }

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
        // DEC-AXO-070 commit G: VRAM summit guard + stuck-recovery defaults
        // were removed; their call sites were dead since commit C and the
        // 2 GB summit threshold was misconfigured (AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT=96
        // failed the [50,95] parser filter, falling back to soft_limit_mb).
        // Replaced by the single canonical AXON_GRAPH_EMBEDDINGS_ENABLED=false
        // default that prevents multi-worker BGE-Large GPU contention.
        assert_eq!(
            std::env::var("AXON_GRAPH_EMBEDDINGS_ENABLED").unwrap(),
            "false"
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
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        }
    }
}

// REQ-AXO-901653 slice-5c — WorkerPool spawn removed ; pipeline_v2 owns ingestion.
fn start_indexer_only_services(
    graph_store: Arc<GraphStore>,
    queue_store: Arc<QueueStore>,
    _results_tx: tokio::sync::broadcast::Sender<String>,
    _num_workers: usize,
    runtime_mode: AxonRuntimeMode,
) {
    if runtime_mode.ingestion_enabled() {
        info!("Runtime services: indexing handled by pipeline_v2 (REQ-AXO-289).");
    } else {
        info!("Runtime services: indexing workers disabled by runtime mode.");
    }

    if runtime_mode.semantic_workers_enabled() {
        let lane_config = embedding_lane_config_from_env();
        info!(
            "Runtime services: semantic workers enabled (mode={}, query_workers={}, vector_workers={}, graph_workers={}).",
            runtime_mode.as_str(),
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

    // REQ-AXO-901735 / DEC-AXO-901615 — health probes HTTP indexer pour
    // que process-compose puisse observer liveness / readiness / startup
    // sans inspection ad-hoc (PID file, pgrep). Port dédié séparé de
    // celui du brain pour cohabitation live brain :44129 + indexer :44139.
    // Best-effort : si bind échoue, l'indexer tourne sans HTTP (process-
    // compose perdra ses probes mais ne crash pas).
    let health_state = crate::indexer_health_http::IndexerHealthState::new();
    let health_port = crate::indexer_health_http::resolve_health_port();
    let health_state_for_spawn = health_state.clone();
    tokio::spawn(async move {
        crate::indexer_health_http::serve_health_probes(health_port, health_state_for_spawn).await;
    });
    // Init terminé (workers spawnés ci-dessus) → /startupz peut retourner
    // 200. Avant ce point, /startupz retourne 503 + {state:starting}.
    health_state.mark_startup_done();
}
