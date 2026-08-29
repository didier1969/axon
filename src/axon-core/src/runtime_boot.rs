use crate::bridge::BridgeEvent;
use crate::embedder::{
    current_gpu_memory_snapshot, embedding_lane_config_from_env, SemanticWorkerPool,
};
use crate::graph::GraphStore;
use crate::main_background;
use crate::main_services;
use crate::main_telemetry;
use crate::queue::QueueStore;
use crate::runtime_capacity_profile::{
    recommend_embedding_lane_sizing, EmbeddingLaneSizing, RuntimeProfile,
};
use crate::runtime_mode::canonical_embedding_provider_request_for_mode;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_writer_guard::WriterGuard;
// REQ-AXO-901653 slice-5c — v1 `worker::{DbWriteTask, WorkerPool}` retired.
// Pipeline_v2 (REQ-AXO-289 / CPT-AXO-054) owns the ingestion path.
use serde::{Deserialize, Serialize};
use std::fs;
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

/// REQ-AXO-902373 — VRAM the indexer must LEAVE to the other GPU consumers on this
/// host. Measured 2026-08-20 on the 8 GiB reference machine:
///
/// | consumer            | VRAM      | when                                     |
/// |---------------------|-----------|------------------------------------------|
/// | live brain          | ~1.5-2.2 GiB | resident (query-embed CUDA context)   |
/// | Handy (dictation)   | ~1.5-2 GiB   | ON DEMAND — loads its model when the operator speaks |
/// | other host projects | headroom     | opportunistic                        |
///
/// Handy is why this is a RESERVE and not a leftover: it holds no VRAM at rest, so a
/// "free memory" probe at indexer start would happily hand its share to the arena,
/// and the dictation would then fail the moment it is used. An intermittent consumer
/// has to be budgeted for even while it is absent.
///
/// Absolute rather than a percentage: what it covers — CUDA contexts and a Whisper-class
/// model — costs roughly the same regardless of card size.
/// Override with `AXON_GPU_RESERVE_MB`.
const DEFAULT_GPU_RESERVE_MB: u64 = 4_096;

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

    // REQ-AXO-902373 — the indexer is NOT alone on the GPU.
    //
    // The previous ladder derived the budget from the card TOTAL minus a 128 MiB
    // token margin (8192 -> soft 8064 / cuda 7936, i.e. 97% of an 8 GiB card). That
    // is only correct for an indexer that owns the whole GPU. In the standing live
    // deployment the brain holds a CUDA context too (~1.5-2.2 GiB for the query-embed
    // lane), so the two budgets summed to MORE than the card: 7936 + 1518 > 8192.
    // The ORT BFC arena grows monotonically and never returns memory, so it walked up
    // to the ceiling and every subsequent batch died in `BFCArena::Alloc` — which
    // stage_b2 reports as a whole-batch failure, marking 64 healthy chunks `failed`.
    // Measured 2026-08-20: ~233k chunks left without a vector this way.
    //
    // Operator directive (2026-08-20): live is served first, but the indexer must
    // never take the whole card — the spare VRAM belongs to the brain and to the
    // other projects sharing this host. Hence an explicit RESERVE subtracted from the
    // total, rather than a margin token. Absolute (not a percentage) because what it
    // must cover — a second CUDA context — costs roughly the same on any card.
    let reserve_mb = std::env::var("AXON_GPU_RESERVE_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_GPU_RESERVE_MB);
    // Floor: below ~2 GiB the BGE-Large session cannot load at all, so a reserve
    // larger than the card would otherwise produce an indexer that never starts.
    let soft_limit_mb = total_vram_mb.saturating_sub(reserve_mb).max(2_048);
    let cuda_limit_mb = soft_limit_mb.saturating_sub(128).max(1_920);

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

impl RuntimeBootRole {
    /// Token used in the per-role socket paths (`/tmp/axon-live-brain-telemetry.sock`).
    pub(crate) fn socket_token(self) -> &'static str {
        match self {
            RuntimeBootRole::Brain => "brain",
            RuntimeBootRole::Indexer => "indexer",
        }
    }

    pub(crate) fn peer(self) -> RuntimeBootRole {
        match self {
            RuntimeBootRole::Brain => RuntimeBootRole::Indexer,
            RuntimeBootRole::Indexer => RuntimeBootRole::Brain,
        }
    }
}

/// REQ-AXO-902256 — why a socket was already on disk at boot.
///
/// The previous single WARN asserted "potential brain/indexer collision" for EVERY
/// pre-existing socket. That is wrong in the common case and actively costly: the socket
/// paths are ALREADY per-role, so a restarting brain always finds its own
/// `…-brain-…` socket and always tripped the collision wording. In session 104 that
/// sentence sent a production root-cause investigation down a dead end (the real defect
/// was elsewhere entirely — the promote's in-place path never relaunching the indexer).
/// The path carries the answer, so classify instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleSocketKind {
    /// Leftover from a previous instance of THIS role — expected after any restart.
    SelfRestartLeftover,
    /// The path carries the OTHER role's token: a genuine cross-role collision.
    CrossRoleCollision,
    /// No role token in the path (legacy default shared by both roles) — can't tell.
    RoleUnmarked,
}

pub(crate) fn classify_stale_socket(path: &str, role: RuntimeBootRole) -> StaleSocketKind {
    // Match on `-<token>-` so a directory or instance name that merely contains the word
    // (e.g. /home/brainstorm/…) cannot be mistaken for a role marker.
    let own = format!("-{}-", role.socket_token());
    let peer = format!("-{}-", role.peer().socket_token());
    if path.contains(&own) {
        StaleSocketKind::SelfRestartLeftover
    } else if path.contains(&peer) {
        StaleSocketKind::CrossRoleCollision
    } else {
        StaleSocketKind::RoleUnmarked
    }
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

/// REQ-AXO-901869 / REQ-AXO-902005 — single owned (`Arc`-backed, `Send + Sync`)
/// `JsonSqlStore` adapter over `GraphStore` for IST CSR cold-loads. Serves BOTH
/// the boot warm path and the `ist_mutated` serve-stale listener (which moves it
/// into a spawned task), so there is one adapter, not one per call-site.
struct IstLoaderSqlStore(Arc<GraphStore>);

impl crate::ist_snapshot::loader::JsonSqlStore for IstLoaderSqlStore {
    fn query_json(&self, sql: &str) -> Result<String, String> {
        self.0.query_json(sql).map_err(|e| e.to_string())
    }
}

/// REQ-AXO-901869 A1 / REQ-AXO-902177 — distinct project codes from a
/// `SELECT DISTINCT project_code ...` 2-D `query_json` array (first column,
/// non-empty). Pure, so the parse is unit-tested without a live GraphStore.
/// Shared by the IST boot-warm (this file) and the SOLL boot-warm
/// (`McpServer::warm_all_soll_snapshots`) so both enumerate identically.
pub(crate) fn parse_boot_warm_project_codes(raw: &str) -> Vec<String> {
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
/// REQ-AXO-902064 slice 3 — parse `KEY=value` (shlex-quoted) build-info lines,
/// keeping only the release-identity vars. Pure (no I/O) for unit testing.
fn parse_build_info_identity(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let (k, v) = line.split_once('=')?;
            let k = k.trim();
            if !matches!(
                k,
                "AXON_BUILD_ID"
                    | "AXON_RELEASE_VERSION"
                    | "AXON_PACKAGE_VERSION"
                    | "AXON_INSTALL_GENERATION"
            ) {
                return None;
            }
            // build_info_target is written via shlex.quote — strip the optional
            // surrounding single-quotes (identity values are quote-free in practice).
            let v = v.trim().trim_matches('\'').to_string();
            // REQ-AXO-902064 — never override a real env value with a placeholder.
            // The cargo-build-time build-info carries AXON_INSTALL_GENERATION=
            // "workspace" (the generation is assigned at PROMOTE time, not build
            // time, and reaches a full restart via start.sh's env). Only the
            // in-place atomic swap writes the real generation into build-info, so
            // re-source it ONLY when it is a genuine value — otherwise keep the
            // env that start.sh already set correctly.
            if v.is_empty() || (k == "AXON_INSTALL_GENERATION" && v == "workspace") {
                return None;
            }
            Some((k.to_string(), v))
        })
        .collect()
}

/// REQ-AXO-902064 — set release identity from the promote-written active-identity
/// file (path in AXON_ACTIVE_IDENTITY_FILE), OVERRIDING the inherited env. The
/// promote writes this file with the MANIFEST build_id + install_generation, so an
/// in-place `process restart` (which re-execs the brain with the daemon's FROZEN
/// env, lagging the new build) reports the PROMOTED identity and passes the
/// PIL-AXO-005 post-check. Unlike `<exe>.build-info` (which cargo caches for a
/// binary-unchanged / script-only commit → stale build_id), this file is always
/// the manifest value, so an override is correct for BOTH the full restart (env
/// already == manifest) and the in-place restart (env stale). No env var / empty
/// path / unreadable file → inherited env stays (dev / fresh checkout). Pure
/// identity reporting: it never touches the SOLL writer lock or any state, so the
/// in-place restart stays a normal sequential single-brain stop→start (no
/// concurrent-writer / lock-handoff risk — that is the blue-green slice).
pub fn resource_release_identity() {
    let Ok(path) = std::env::var("AXON_ACTIVE_IDENTITY_FILE") else {
        return;
    };
    if path.trim().is_empty() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    for (k, v) in parse_build_info_identity(&contents) {
        // Promote-authoritative: the file always carries the manifest identity,
        // so overriding the (possibly stale, inherited) env is correct.
        std::env::set_var(k, v);
    }
}

fn warm_all_ist_snapshots_at_boot(graph_store: Arc<GraphStore>) {
    tokio::task::spawn_blocking(move || {
        let store = IstLoaderSqlStore(graph_store);
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
    // REQ-AXO-902027 — when this process was re-exec'd as a one-shot GPU
    // dlopen probe (`axon-indexer --__gpu-lib-probe <path>`), load the lib and
    // exit with the verdict BEFORE booting anything. A corrupt lib faults here,
    // in the throwaway child, never in the live indexer.
    if let Some(code) = crate::embedder::gpu_preflight::run_dlopen_probe_if_requested() {
        std::process::exit(code);
    }
    run(RuntimeBootProfile::indexer())
}

fn run(profile: RuntimeBootProfile) -> anyhow::Result<()> {
    // REQ-AXO-902064 — re-source release identity from the promote-written
    // active-identity file BEFORE the tokio runtime spawns (single-threaded here,
    // so env::set_var is sound). An in-place `process restart` re-execs the brain
    // with the daemon's FROZEN env, which lags the new build → without this the
    // brain would report the stale AXON_BUILD_ID / INSTALL_GENERATION and fail the
    // PIL-AXO-005 identity post-check. The active-identity file carries the promote
    // MANIFEST values, so it is authoritative for both full and in-place restarts.
    resource_release_identity();
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

    // REQ-AXO-902341 — acquire writer ownership HERE, at the earliest point after
    // db_root is resolved and identity is logged, and BEFORE any observable state
    // is written (no env mutation, no worker spawn, no report_subsystem_state, no
    // role heartbeats — all of which follow below). A second process that does not
    // own the lock is refused right here and exits having announced NOTHING; the
    // supervisor and watchdog therefore never see a phantom "Ready" / heartbeat
    // from an imposter. The guards are held in `_writer_guards` for the whole boot
    // scope, so the lock is released at process exit exactly as before — only the
    // ACQUISITION moves earlier, not the release.
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

    // REQ-AXO-902341 — writer-guard acquisition moved UP, to right after the
    // identity logging (see `_writer_guards` above). It used to sit HERE, AFTER
    // report_subsystem_state + wire_*_role_heartbeats above: a second, correctly
    // REFUSED process had by then already announced its subsystems Ready and
    // emitted role heartbeats — signals the supervisor and watchdog consume to
    // judge the LIVE role's health. A refused process must leave NO trace in
    // observable state; its only legitimate output is the refusal.

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
    // REQ-AXO-902256 — report WHICH of the two cases this is (see classify_stale_socket).
    // Only a cross-role path warrants the collision wording; a same-role leftover is the
    // normal consequence of a restart and must not read as an anomaly.
    let log_stale_socket = |path: &str, kind: StaleSocketKind, env_var: &str, which: &str| match kind
    {
        StaleSocketKind::SelfRestartLeftover => info!(
            socket = %path,
            "{which} socket left by a previous instance of this role; removed before bind (expected after a restart — NOT a cross-role collision)"
        ),
        StaleSocketKind::CrossRoleCollision => warn!(
            socket = %path,
            "{which} socket belongs to the OTHER role and was removed before bind — genuine brain/indexer collision, a live peer may have been orphaned; verify the per-role {env_var} override"
        ),
        StaleSocketKind::RoleUnmarked => warn!(
            socket = %path,
            "{which} socket pre-existed at boot with no role marker in its path (legacy shared default); removed before bind — set a per-role {env_var} so collisions become diagnosable"
        ),
    };
    if std::path::Path::new(&tel_socket_path).exists() {
        let kind = classify_stale_socket(&tel_socket_path, profile.role);
        match fs::remove_file(&tel_socket_path) {
            Ok(()) => log_stale_socket(
                &tel_socket_path,
                kind,
                "AXON_TELEMETRY_SOCK",
                "telemetry",
            ),
            Err(err) => warn!(
                socket = %tel_socket_path,
                error = %err,
                "telemetry socket pre-existed at boot but remove failed; bind may fail"
            ),
        }
    }
    if std::path::Path::new(&mcp_socket_path).exists() {
        let kind = classify_stale_socket(&mcp_socket_path, profile.role);
        match fs::remove_file(&mcp_socket_path) {
            Ok(()) => log_stale_socket(&mcp_socket_path, kind, "AXON_MCP_SOCK", "mcp"),
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

    let num_workers = runtime_profile.recommended_workers;
    info!(
        "Power Scaling: Sizing worker pool growth to {} threads.",
        num_workers
    );

    // REQ-AXO-901653 slice-5c — `db_sender` removed (v1 writer-actor retired).
    // Pipeline_v2 (REQ-AXO-289) writes via GraphStore directly.
    let indexer_health = if profile.start_mcp_http {
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
        None
    } else {
        Some(start_indexer_only_services(
            graph_store.clone(),
            queue_store.clone(),
            results_tx.clone(),
            num_workers,
            runtime_mode,
        ))
    };

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
        // the DuckDB-era public.File state machine. spawn_pipeline_indexer
        // boots A1→A2→A3 (and B1→B2→B3 when semantic workers are enabled),
        // feeds them from the Watchman file source + the scanner/reconciliation
        // walk (REQ-AXO-901893 / REQ-AXO-901916 — the DBQ-A claim feeder named
        // here before was deleted with PIL-AXO-007), and resolves project_code
        // per file.
        // The legacy notify watcher + federation/scope orchestrators that pushed
        // into the in-memory ingress_buffer were RIPPED in the LEGACY FEED PURGE.
        let health_state = indexer_health.clone().ok_or_else(|| {
            anyhow::anyhow!("ingestion-enabled runtime has no indexer health state")
        })?;
        if let Err(err) = crate::pipeline_runtime::spawn_pipeline_indexer(
            runtime_mode,
            graph_store.clone(),
            watch_root_str.clone(),
            health_state.clone(),
        ) {
            health_state.record_heartbeat_failure("pipeline_spawn_failed");
            return Err(err.context("pipeline_runtime: failed to spawn streaming indexer"));
        }
        health_state.mark_pipeline_started();
        main_background::spawn_memory_reclaimer(queue_store.clone());
    } else {
        if let Some(health_state) = &indexer_health {
            health_state.mark_pipeline_started();
        }
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
                Arc::new(IstLoaderSqlStore(graph_store.clone()));
            crate::ist_snapshot::notify_listener::spawn_ist_mutation_listener(
                _url.clone(),
                refresh_store,
            );
            info!("ist_mutated listener spawned (REQ-AXO-901658/902005) — IST cache serve-stale async refresh wired");
            // REQ-AXO-902234 — desired-state consumer, indexer-side ONLY: the
            // idle-drop watchdog it steers lives in the pipeline (indexer), and a
            // brain would only seed/obey a row it has no watchdog for. Reuses the
            // URL already resolved for the ist_mutated listener above.
            if profile.role == RuntimeBootRole::Indexer {
                crate::pipeline::embedder_control_listener::spawn_embedder_control_listener(
                    _url.clone(),
                    crate::pipeline::embedder_control_listener::ROLE_INDEXER.to_string(),
                );
                info!(
                    "embedder_control listener spawned (REQ-AXO-902234) — idle-drop policy \
                     flippable at runtime via the `idle_drop` MCP tool, no restart"
                );
                // REQ-AXO-902262 — same two-process problem, other direction: the
                // `rescan_project full=true` tool runs in the BRAIN and wipes PG rows, but
                // the cache that decides whether a file is re-read is the indexer's in-RAM
                // `IndexedFileCache`. Without this listener the tool DESTROYED a project's
                // chunks and could not rebuild them (LLL: 434/434 → 2/438, no automatic
                // recovery, the reconciliation walk replaying the same skip every 15 min).
                crate::pipeline::cache_invalidate_listener::spawn_cache_invalidate_listener(
                    _url.clone(),
                );
                info!(
                    "ist_cache_invalidate listener spawned (REQ-AXO-902262) — rescan_project \
                     full=true can now purge the dedup cache, no indexer restart"
                );
            }
            // REQ-AXO-901893 (LEGACY FEED PURGE) — the axon_registry_changed
            // listener (REQ-AXO-901675) was RIPPED with ingress_buffer: it
            // pushed an IngressSource::Scan subtree hint into the in-memory
            // ingress_buffer, which no longer exists. The PG trigger in
            // `db/ddl/07_registry_notify.sql` still fires; live new-project
            // discovery now relies on the next indexer restart (Watchman
            // resolves all watch_root roots at boot and the boot walk streams
            // the paths straight into pipeline A). Tracked: REQ-AXO-901899.
            // REQ-AXO-902260 — the old wording said "DBQ-A drains the
            // 'discovered' backlog by construction": that feeder was deleted by
            // REQ-AXO-901916, and no backlog is drained by construction here.
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

    // REQ-AXO-901757 slice B2 — brain owns the SOLL writer + the in-process query
    // worker, so the periodic SOLL-description embedding sweep runs brain-side
    // only. Indexer has no SOLL writer; spawning it there would be a no-op churn.
    if profile.role == RuntimeBootRole::Brain {
        main_background::spawn_soll_embedding_sweep(graph_store.clone());
    }

    // REQ-AXO-902233 — graceful shutdown. Both keep-alive paths (the telemetry
    // accept loop and the bare park) now RACE a SIGTERM/SIGINT future via
    // `select!`, so a process-compose stop (or an OS reboot) unwinds the runtime
    // and lets the process EXIT instead of parking forever. Root cause of the
    // brain-zombie-terminating incident (2026-07-13): the never-resolving
    // keep-alive left the process stuck in `Terminating` until SIGKILL, after
    // which no fresh brain was spawned → total outage. Symmetric for both roles
    // (brain + indexer share `boot`); on the indexer this also gives Drop a chance
    // to release the GPU session on stop instead of a hard kill.
    let telemetry_loop = async {
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
            std::future::pending::<()>().await;
        }
    };

    tokio::select! {
        _ = telemetry_loop => {}
        _ = shutdown_signal() => {
            info!("🛑 Shutdown signal (SIGTERM/SIGINT) received — {:?} runtime unwinding for a clean exit (REQ-AXO-902233)", profile.role);
            // REQ-AXO-902271 — the indexer leaves WITHOUT running the GPU
            // teardown that wedges the WSL2 vmbus channel. Unwinding normally is
            // what turns a stop into an unkillable `Terminating` process.
            if should_hard_exit_on_shutdown(profile.role) {
                info!(
                    "⏭️  {:?} exiting hard — GPU teardown skipped on purpose (REQ-AXO-902271): \
                     the deinit is a synchronous call on WSL2's single GPU channel and has \
                     wedged this process unkillably; the driver reclaims the session anyway",
                    profile.role
                );
                hard_exit_skipping_gpu_teardown();
            }
        }
    }
    Ok(())
}

/// REQ-AXO-902271 — which role must exit WITHOUT letting the GPU destructors run.
///
/// REQ-AXO-902233 deliberately let `Drop` release the GPU session on stop,
/// "instead of a hard kill". On WSL2 that good intention is the wedge: the
/// CUDA/TensorRT deinit is a SYNCHRONOUS call onto the single serialised GPU
/// vmbus channel (`dxgvmb_send_sync_msg`). When anything else touches the GPU in
/// that window, the indexer's tokio worker blocks there in UNINTERRUPTIBLE sleep
/// — unkillable (SIGKILL included), never reaped by process-compose, so the
/// process sits in `Terminating` forever and only `wsl --shutdown` clears it.
/// Measured: 4 wedges over 191 promotes, every one at the lifecycle gate.
///
/// The indexer can afford a hard exit and the brain cannot:
///   * the indexer is a DERIVED, idempotent writer — the IST is rebuilt from
///     source, so the worst case is re-scanning the in-flight batch;
///   * the brain owns SOLL writes, which are preserve-always (PIL-AXO-003) —
///     dropping one in flight is not recoverable by re-running anything.
/// The brain is also not the offender: its D-thread on 2026-08-13 came from a
/// query-embed issued while the channel was ALREADY wedged, not from its own
/// teardown.
///
/// Releasing the GPU session on exit was never load-bearing: the driver reclaims
/// every resource a process held when it dies. The teardown bought tidiness and
/// cost availability.
pub(crate) fn should_hard_exit_on_shutdown(role: RuntimeBootRole) -> bool {
    matches!(role, RuntimeBootRole::Indexer)
}

/// REQ-AXO-902271 — leave the process NOW, skipping Rust destructors AND libc
/// `atexit` handlers.
///
/// `std::process::exit` is not enough: it runs `atexit` hooks, and that is
/// precisely where a GPU runtime registers its deinit (this crate already
/// installs one in `test_support::test_db`). Only `_exit` — the raw
/// `exit_group` syscall — guarantees no GPU call happens on the way out.
/// stdout/stderr are flushed first by hand, since `_exit` skips that too.
fn hard_exit_skipping_gpu_teardown() -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: `_exit` is async-signal-safe and terminates the process; nothing
    // after it can observe the skipped destructors.
    unsafe { libc::_exit(0) }
}

/// REQ-AXO-902233 — resolves on the first SIGTERM (process-compose stop, OS
/// reboot) or SIGINT (Ctrl-C). Replaces the never-resolving `pending()`
/// keep-alive so the runtime unwinds and the process EXITS promptly instead of
/// lingering in `Terminating` until SIGKILL (brain-zombie incident 2026-07-13).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = sigterm.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(err) => {
                error!(error = %err, "SIGTERM handler install failed; falling back to Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_canonical_embedding_lane_sizing_defaults, apply_canonical_ort_runtime_env,
        apply_canonical_ort_thread_defaults_from_openmp, apply_graph_first_indexer_memory_defaults,
        canonical_effective_embedding_lane_config, canonical_embedding_provider_request,
        graph_first_indexer_lane_sizing, parse_boot_warm_project_codes,
        classify_stale_socket, parse_build_info_identity, resource_release_identity,
        should_hard_exit_on_shutdown, RuntimeBootProfile, RuntimeBootRole, StaleSocketKind,
    };
    use crate::runtime_mode::AxonRuntimeMode;
    use crate::runtime_capacity_profile::{EmbeddingLaneSizing, RuntimeProfile};
    use crate::runtime_writer_guard::WriterTarget;

    // REQ-AXO-902271 — the indexer must NOT run its GPU teardown on SIGTERM.
    //
    // The exit itself cannot be unit-tested (it would kill the test runner),
    // which is exactly why the DECISION is a separate pure predicate: the part
    // that can be got wrong is the role split, and that part is pinned here.
    #[test]
    fn only_the_indexer_skips_gpu_teardown_on_shutdown() {
        assert!(
            should_hard_exit_on_shutdown(RuntimeBootRole::Indexer),
            "the indexer is a derived idempotent writer and the process that wedges \
             on dxgvmb — it exits hard rather than deinit the GPU synchronously"
        );
        assert!(
            !should_hard_exit_on_shutdown(RuntimeBootRole::Brain),
            "the brain owns preserve-always SOLL writes (PIL-AXO-003): an in-flight \
             write is not recoverable by re-running anything, so it keeps unwinding"
        );
    }

    /// REQ-AXO-902256 — the real session-104 path: a restarting BRAIN finds
    /// `/tmp/axon-live-brain-telemetry.sock`. The old code called that a
    /// "potential brain/indexer collision", which is false and cost a wrong
    /// root-cause branch during a production investigation.
    #[test]
    fn own_role_socket_is_a_restart_leftover_not_a_collision() {
        assert_eq!(
            classify_stale_socket(
                "/tmp/axon-live-brain-telemetry.sock",
                RuntimeBootRole::Brain
            ),
            StaleSocketKind::SelfRestartLeftover
        );
        assert_eq!(
            classify_stale_socket(
                "/tmp/axon-live-indexer-telemetry.sock",
                RuntimeBootRole::Indexer
            ),
            StaleSocketKind::SelfRestartLeftover
        );
    }

    /// The case the WARN was actually written for must still be reported loudly.
    #[test]
    fn peer_role_socket_is_a_genuine_cross_role_collision() {
        assert_eq!(
            classify_stale_socket(
                "/tmp/axon-live-indexer-telemetry.sock",
                RuntimeBootRole::Brain
            ),
            StaleSocketKind::CrossRoleCollision
        );
        assert_eq!(
            classify_stale_socket("/tmp/axon-live-brain-mcp.sock", RuntimeBootRole::Indexer),
            StaleSocketKind::CrossRoleCollision
        );
    }

    /// The legacy defaults (`/tmp/axon-telemetry.sock`, `/tmp/axon-mcp.sock`) carry no role
    /// token, so both roles would share them — undiagnosable, and worth saying so.
    #[test]
    fn unmarked_path_is_reported_as_role_unmarked() {
        assert_eq!(
            classify_stale_socket("/tmp/axon-telemetry.sock", RuntimeBootRole::Brain),
            StaleSocketKind::RoleUnmarked
        );
        assert_eq!(
            classify_stale_socket("/tmp/axon-mcp.sock", RuntimeBootRole::Indexer),
            StaleSocketKind::RoleUnmarked
        );
    }

    /// A directory that merely CONTAINS a role word must not be read as a role marker —
    /// hence the `-<token>-` delimiters rather than a bare `contains`.
    #[test]
    fn role_word_inside_a_directory_name_is_not_a_marker() {
        assert_eq!(
            classify_stale_socket("/home/brainstorm/axon.sock", RuntimeBootRole::Brain),
            StaleSocketKind::RoleUnmarked
        );
    }

    /// REQ-AXO-902064 — the active-identity re-source OVERRIDES the inherited
    /// (stale) env from the promote-written file. This is the mechanism that lets
    /// an in-place `process restart` report the PROMOTED identity despite the
    /// daemon's frozen env. Saves/restores the process env around the assertion.
    #[test]
    fn resource_release_identity_overrides_env_from_active_file() {
        // REQ-AXO-902261 — saving and restoring the vars is NOT serialization: a sibling
        // test can still read or clobber them between the save and the restore.
        let _lock = env_lock();
        let saved_file = std::env::var("AXON_ACTIVE_IDENTITY_FILE").ok();
        let saved_build = std::env::var("AXON_BUILD_ID").ok();
        let saved_gen = std::env::var("AXON_INSTALL_GENERATION").ok();

        let path = std::env::temp_dir().join("axon_test_active_identity_REQ902064.env");
        std::fs::write(
            &path,
            "AXON_BUILD_ID=v9.9.9-promoted\nAXON_INSTALL_GENERATION=gen-promoted\n",
        )
        .unwrap();
        // Simulate the in-place restart: the daemon reinjected a STALE build_id.
        std::env::set_var("AXON_BUILD_ID", "v0.0.0-stale-inherited");
        std::env::set_var("AXON_INSTALL_GENERATION", "gen-stale");
        std::env::set_var("AXON_ACTIVE_IDENTITY_FILE", path.to_str().unwrap());

        resource_release_identity();

        let got_build = std::env::var("AXON_BUILD_ID").unwrap();
        let got_gen = std::env::var("AXON_INSTALL_GENERATION").unwrap();

        // restore env before asserting (so a failure can't leak state)
        match saved_file {
            Some(v) => std::env::set_var("AXON_ACTIVE_IDENTITY_FILE", v),
            None => std::env::remove_var("AXON_ACTIVE_IDENTITY_FILE"),
        }
        match saved_build {
            Some(v) => std::env::set_var("AXON_BUILD_ID", v),
            None => std::env::remove_var("AXON_BUILD_ID"),
        }
        match saved_gen {
            Some(v) => std::env::set_var("AXON_INSTALL_GENERATION", v),
            None => std::env::remove_var("AXON_INSTALL_GENERATION"),
        }
        std::fs::remove_file(&path).ok();

        assert_eq!(got_build, "v9.9.9-promoted", "active-identity must override stale env");
        assert_eq!(got_gen, "gen-promoted");
    }

    #[test]
    fn resource_release_identity_noop_without_env() {
        // REQ-AXO-902261 — see the sibling test above: save/restore is not a lock.
        let _lock = env_lock();
        let saved_file = std::env::var("AXON_ACTIVE_IDENTITY_FILE").ok();
        let saved_build = std::env::var("AXON_BUILD_ID").ok();
        std::env::remove_var("AXON_ACTIVE_IDENTITY_FILE");
        std::env::set_var("AXON_BUILD_ID", "v1.2.3-keep");
        resource_release_identity(); // no file env → must not touch AXON_BUILD_ID
        let got = std::env::var("AXON_BUILD_ID").unwrap();
        match saved_file {
            Some(v) => std::env::set_var("AXON_ACTIVE_IDENTITY_FILE", v),
            None => std::env::remove_var("AXON_ACTIVE_IDENTITY_FILE"),
        }
        match saved_build {
            Some(v) => std::env::set_var("AXON_BUILD_ID", v),
            None => std::env::remove_var("AXON_BUILD_ID"),
        }
        assert_eq!(got, "v1.2.3-keep", "no active-identity env → env untouched");
    }

    /// REQ-AXO-902064 slice 3 — build-info identity parse: keeps only the four
    /// release-identity vars, strips shlex single-quotes, ignores other lines.
    #[test]
    fn parse_build_info_identity_extracts_release_vars() {
        let raw = "AXON_RELEASE_VERSION=v0.8.0\n\
                   AXON_BUILD_ID=v0.8.0-1139-ged70b99f\n\
                   AXON_PACKAGE_VERSION='0.8.0'\n\
                   AXON_INSTALL_GENERATION=live-20260621T113105Z\n\
                   AXON_OTHER=ignored\n";
        let got = parse_build_info_identity(raw);
        assert_eq!(got.len(), 4, "only the 4 identity vars, not AXON_OTHER");
        let map: std::collections::HashMap<_, _> = got.into_iter().collect();
        assert_eq!(map["AXON_BUILD_ID"], "v0.8.0-1139-ged70b99f");
        assert_eq!(map["AXON_PACKAGE_VERSION"], "0.8.0"); // single-quotes stripped
        assert_eq!(map["AXON_INSTALL_GENERATION"], "live-20260621T113105Z");
        assert!(!map.contains_key("AXON_OTHER"));
    }

    #[test]
    fn parse_build_info_identity_empty_and_malformed() {
        assert!(parse_build_info_identity("").is_empty());
        assert!(parse_build_info_identity("no-equals-sign\n").is_empty());
    }

    #[test]
    fn parse_build_info_identity_skips_workspace_generation_placeholder() {
        // REQ-AXO-902064 — the cargo-build build-info carries the "workspace"
        // placeholder generation; it must NOT override the real env value that
        // start.sh sets for a full restart. build_id (real) is still re-sourced.
        let raw = "AXON_BUILD_ID=v0.8.0-1142-g9d0f3164\n\
                   AXON_INSTALL_GENERATION=workspace\n";
        let map: std::collections::HashMap<_, _> =
            parse_build_info_identity(raw).into_iter().collect();
        assert_eq!(map.get("AXON_BUILD_ID").map(String::as_str), Some("v0.8.0-1142-g9d0f3164"));
        assert!(
            !map.contains_key("AXON_INSTALL_GENERATION"),
            "workspace placeholder must be skipped"
        );
    }

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
        // REQ-AXO-902261 — 57 env mutations, previously unserialized.
        let _lock = env_lock();
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

        // REQ-AXO-902373 — 8192 total - 4096 reserve = 4096 soft, 3968 cuda.
        // The reserve is what the brain (and any other GPU consumer on this host)
        // gets to keep; the old 8064/7936 values oversubscribed the card.
        assert_eq!(
            std::env::var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB").unwrap(),
            "4096"
        );
        assert_eq!(std::env::var("AXON_CUDA_MEMORY_LIMIT_MB").unwrap(), "3968");
        assert_eq!(std::env::var("AXON_OPT_MAX_VRAM_USED_MB").unwrap(), "4096");
        assert_eq!(
            std::env::var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB").unwrap(),
            "4096"
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

// REQ-AXO-901653 slice-5c — WorkerPool spawn removed ; pipeline owns ingestion.
fn start_indexer_only_services(
    graph_store: Arc<GraphStore>,
    queue_store: Arc<QueueStore>,
    _results_tx: tokio::sync::broadcast::Sender<String>,
    _num_workers: usize,
    runtime_mode: AxonRuntimeMode,
) -> crate::indexer_health_http::IndexerHealthState {
    if runtime_mode.ingestion_enabled() {
        info!("Runtime services: indexing handled by pipeline (REQ-AXO-289).");
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
    let health_state =
        crate::indexer_health_http::IndexerHealthState::new(runtime_mode.ingestion_enabled());
    let health_port = crate::indexer_health_http::resolve_health_port();
    let health_state_for_spawn = health_state.clone();
    tokio::spawn(async move {
        crate::indexer_health_http::serve_health_probes(health_port, health_state_for_spawn).await;
    });
    health_state
}
