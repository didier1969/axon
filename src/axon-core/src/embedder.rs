use crate::embedding_contract::{
    fastembed_model, CHUNK_MODEL_ID, DIMENSION, GRAPH_MODEL_ID, MODEL_NAME, MODEL_VERSION,
    SYMBOL_MODEL_ID,
};
use crate::embedding_profile::{
    configured_embedding_max_length as profile_configured_embedding_max_length,
    configured_embedding_token_bucket_size as profile_configured_embedding_token_bucket_size,
    embedding_model_cache_dir as profile_embedding_model_cache_dir,
    load_runtime_embedding_tokenizer as profile_load_runtime_embedding_tokenizer,
    runtime_embedding_snapshot_dir as profile_runtime_embedding_snapshot_dir,
};
use crate::graph::GraphStore;
use crate::graph_ingestion::{
    FileVectorizationLeaseSnapshot, FileVectorizationWork, GraphProjectionWork, VectorBatchRun,
    VectorLaneStateRecord, VectorWorkerFault, INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS,
    INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT,
};
use crate::queue::QueueStore;
use crate::runtime_mode::canonical_embedding_provider_request_for_mode;
use crate::runtime_mode::graph_embeddings_enabled;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_profile::{recommend_embedding_lane_sizing, RuntimeProfile};
use crate::runtime_tuning::{
    current_runtime_tuning_snapshot as runtime_tuning_snapshot,
    current_runtime_tuning_state as runtime_tuning_state,
    update_runtime_tuning_state as update_shared_runtime_tuning_state, RuntimeTuningSnapshot,
    RuntimeTuningState,
};
use crate::service_guard::{self, ServicePressure, VectorLaneState};
use crate::vector_control::{
    configured_gpu_ready_high_watermark_chunks, configured_gpu_ready_low_watermark_chunks,
    configured_target_ready_chunks, current_vector_batch_controller_diagnostics,
    graph_projection_allowed, observe_vector_batch_controller,
    reset_vector_batch_controller_for_tests, semantic_policy_with_graph, symbol_embedding_allowed,
    vector_claim_target, vector_ready_chunk_reserve_target, vector_worker_admission_decision,
    VectorBatchControllerObservation, AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
};
use crate::vector_pipeline::{
    ClaimedLeaseSet, FinalizeEnvelope, InflightPersistRequest, InflightPrepareRequest,
    PersistEnvelope, PreparedBatchEnvelope, PreparedBatchQueueSummary, SharedPreparedBatchQueue,
};
use anyhow::{anyhow, Result as AnyhowResult};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use fastembed::{InitOptions, OutputKey, TextEmbedding};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tokenizers::{Encoding, Tokenizer};
use tracing::{debug, error, info, warn};

#[path = "embedder/batch_lanes.rs"]
mod batch_lanes;
#[path = "embedder/cpu_query_service.rs"]
mod cpu_query_service;
#[path = "embedder/gpu_backend.rs"]
mod gpu_backend;
#[path = "embedder/gpu_policy.rs"]
mod gpu_policy;
#[path = "embedder/gpu_telemetry.rs"]
mod gpu_telemetry;
#[path = "embedder/provider_contract.rs"]
mod provider_contract;
#[path = "embedder/provider_runtime.rs"]
mod provider_runtime;
#[path = "embedder/vector_executor.rs"]
mod vector_executor;
#[path = "embedder/vector_finalize.rs"]
mod vector_finalize;
#[path = "embedder/vector_orchestrator.rs"]
mod vector_orchestrator;
#[path = "embedder/vector_refill_loop.rs"]
mod vector_refill_loop;

#[cfg(test)]
pub(crate) use batch_lanes::reset_token_lane_classifier_for_tests;
#[cfg(test)]
pub(crate) use batch_lanes::TokenLaneThresholdSource;
pub(crate) use batch_lanes::{
    current_token_lane_thresholds, observe_token_lane_thresholds, service_guard_batch_lane,
    TokenLaneThresholds, VectorBatchLane,
};
pub(crate) use cpu_query_service::spawn_brain_query_worker_if_needed;
use gpu_backend::{
    abort_gpu_embed_if_vram_summit_reached, cuda_execution_provider_dispatch,
    gpu_embed_service_enabled, gpu_embed_service_prefers_tensorrt, gpu_embedding_service_client,
    ort_cuda_provider_library_available, ort_cuda_provider_library_path,
    recycle_existing_gpu_embedding_service, GpuEmbedSubprocessInit, GpuEmbedSubprocessRequest,
    GpuEmbedSubprocessResponse, OrtGpuFirstTextEmbedding,
};
#[cfg(test)]
use gpu_backend::{cuda_memory_limit_bytes, cuda_tf32_enabled};
pub use gpu_policy::current_gpu_memory_pressure_active;
use gpu_policy::{
    embedding_provider_requested_is_gpu, gpu_primary_worker_max_used_mb,
    gpu_recreate_session_every_batch_enabled, gpu_recycle_after_vram_summit_observe,
    gpu_recycle_immediate_required, gpu_recycle_vram_summit_mb, gpu_secondary_worker_allowed,
    gpu_stuck_recovery_reason, gpu_worker_consumption_allowed, gpu_worker_has_pending_work,
    gpu_worker_should_wait_for_ready,
};
#[cfg(test)]
use gpu_policy::gpu_memory_pressure_active;
pub use gpu_telemetry::{
    current_gpu_memory_snapshot, current_gpu_utilization_snapshot, GpuMemorySnapshot,
    GpuUtilizationSnapshot,
};
#[allow(unused_imports)]
pub(crate) use gpu_telemetry::{
    gpu_telemetry_backend_name, gpu_telemetry_cache_ttl_ms, gpu_telemetry_command,
    gpu_telemetry_device_index, nvml_library_path, parse_nvidia_smi_memory_csv,
    parse_nvidia_smi_utilization_csv,
};
#[cfg(test)]
pub(crate) use gpu_telemetry::clear_gpu_memory_snapshot_cache_for_tests;
pub use provider_contract::{
    ProductionLane, ProviderResolution, ProviderStrategy, ProviderSupportRole,
};
#[cfg(test)]
pub(crate) use provider_runtime::provider_resolution_for_label;
use provider_runtime::{
    cpu_provider_effective_label, current_embedding_provider_effective,
    gpu_service_provider_effective_label, publish_embedding_provider_state,
    register_embedding_provider_diagnostics, set_embedding_provider_runtime_state,
};
pub use provider_runtime::{
    current_embedding_provider_diagnostics, embedding_provider_diagnostics,
    EmbeddingProviderDiagnostics,
};
use vector_executor::VectorEmbeddingBackend;
use vector_finalize::{
    apply_vector_persist_outcome, dispatch_vector_persist_plan,
    flush_completed_vectorization_works, persist_vector_embed_batch, process_finalize_request,
    process_vector_persist_outbox_work,
};
#[cfg(test)]
use vector_finalize::{
    finalize_completed_vectorization_works, is_irrecoverable_outbox_finalize_error,
    reconcile_outbox_finalize_failure,
};
use vector_orchestrator::{VectorRefillCommand, VectorRefillProducerState};

#[allow(dead_code)]
pub(crate) fn embedder_cuda_execution_provider_dispatch(
) -> ort::execution_providers::ExecutionProviderDispatch {
    gpu_backend::cuda_execution_provider_dispatch()
}

#[allow(dead_code)]
pub(crate) fn embedder_ort_cuda_provider_library_available() -> bool {
    gpu_backend::ort_cuda_provider_library_available()
}

const FILE_PROJECTION_RADIUS: i64 = 2;
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);
const VECTOR_FINALIZE_QUEUE_BOUND: usize = 16;
const VECTOR_PERSIST_QUEUE_BOUND: usize = 4;
const VECTOR_OUTBOX_FETCH_BATCH_SIZE: usize = 128;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS: u64 = 120_000;
const DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS: u64 = 10_000;
const DEFAULT_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS: u64 = 250;
const DEFAULT_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS: u64 = 50;
const DEFAULT_VECTOR_PREPARE_PIPELINE_DEPTH: usize = 3;
const DEFAULT_VECTOR_PREPARE_QUEUE_BOUND: usize = 4;
const DEFAULT_VECTOR_PREPARE_WORKERS_PER_VECTOR: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingLaneConfig {
    pub query_workers: usize,
    pub vector_workers: usize,
    pub graph_workers: usize,
    pub chunk_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub graph_batch_size: usize,
    pub max_chunks_per_file: usize,
    pub max_embed_batch_bytes: usize,
}

// NEXUS v10.5: Sovereign Semantic Engine
// We isolate the ONNX runtime inside a pure OS thread to prevent Tokio/jemalloc aborts.
// The model stays owned by the background worker; global state only holds a channel
// sender so synchronous MCP queries can reuse the already-loaded model safely.

pub(super) struct QueryEmbeddingRequest {
    texts: Vec<String>,
    reply: Sender<anyhow::Result<Vec<Vec<f32>>>>,
}

struct VectorPrepareRequest {
    claimed: ClaimedLeaseSet,
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
    enqueued_at: Instant,
    reply: Sender<PreparedVectorPrepareOutcome>,
}

#[derive(Debug)]
pub(crate) struct PreparedVectorPrepareOutcome {
    remaining_claimed_after_success: ClaimedLeaseSet,
}

pub(crate) type PreparedVectorPrepareOutcomeReply = Receiver<PreparedVectorPrepareOutcome>;

struct VectorFinalizeRequest {
    envelope: FinalizeEnvelope,
    enqueued_at: Instant,
}

struct VectorPersistRequest {
    envelope: PersistEnvelope,
    enqueued_at: Instant,
    reply: Sender<VectorPersistOutcome>,
}

pub(crate) type VectorPersistOutcomeReply = Receiver<VectorPersistOutcome>;

static QUERY_EMBEDDING_SENDER: OnceLock<Mutex<Option<Sender<QueryEmbeddingRequest>>>> =
    OnceLock::new();

pub struct SemanticWorkerPool {
    _query_workers: Vec<thread::JoinHandle<()>>,
    _vector_prepare_workers: Vec<thread::JoinHandle<()>>,
    _vector_refill_workers: Vec<thread::JoinHandle<()>>,
    _vector_workers: Vec<thread::JoinHandle<()>>,
    _vector_persist_workers: Vec<thread::JoinHandle<()>>,
    _vector_finalize_workers: Vec<thread::JoinHandle<()>>,
    _vector_maintenance_workers: Vec<thread::JoinHandle<()>>,
    _graph_workers: Vec<thread::JoinHandle<()>>,
}

#[derive(Debug, Clone)]
struct VectorChunkWorkItem {
    file_path: String,
    chunk_id: String,
    content_hash: String,
    text: String,
}

#[derive(Debug, Default)]
struct VectorBatchPlan {
    work_items: Vec<VectorChunkWorkItem>,
    touched_works: Vec<FileVectorizationWork>,
    finalize_after_success: Vec<FileVectorizationWork>,
    immediate_completed: Vec<FileVectorizationWork>,
    oversized_works: Vec<FileVectorizationWork>,
    continuation_works: Vec<FileVectorizationWork>,
    untouched_works: Vec<FileVectorizationWork>,
    files_touched: usize,
    partial_file_cycles: usize,
    fetch_ms_total: u64,
    failed_fetches: Vec<(FileVectorizationWork, String)>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedVectorEmbedBatch {
    batch_id: String,
    prepare_started_at_ms: i64,
    prepare_finished_at_ms: i64,
    prepared_at_ms: i64,
    batch_lane: VectorBatchLane,
    mixed_fallback: bool,
    lane_thresholds: TokenLaneThresholds,
    work_items: Vec<VectorChunkWorkItem>,
    texts: Vec<String>,
    token_counts: Vec<usize>,
    encoded_micro_batches: Vec<PreparedEncodedMicroBatch>,
    touched_works: Vec<FileVectorizationWork>,
    finalize_after_success: Vec<FileVectorizationWork>,
    immediate_completed: Vec<FileVectorizationWork>,
    oversized_works: Vec<FileVectorizationWork>,
    next_active_after_success: Vec<FileVectorizationWork>,
    next_active_after_failure: Vec<FileVectorizationWork>,
    #[cfg_attr(not(test), allow(dead_code))]
    files_touched: usize,
    partial_file_cycles: usize,
    fetch_ms_total: u64,
    failed_fetches: Vec<(FileVectorizationWork, String)>,
}

#[derive(Debug)]
pub(crate) struct PreparedVectorEmbedSequence {
    batches: Vec<PreparedBatchEnvelope>,
    remaining_claimed_after_success: ClaimedLeaseSet,
}

#[derive(Debug, Clone)]
struct PreparedEncodedMicroBatch {
    item_indices: Vec<usize>,
    encodings: Vec<Encoding>,
}

struct VectorWorkerLivenessGuard;

impl VectorWorkerLivenessGuard {
    fn new() -> Self {
        service_guard::record_vector_worker_started();
        Self
    }
}

impl Drop for VectorWorkerLivenessGuard {
    fn drop(&mut self) {
        service_guard::record_vector_worker_stopped();
    }
}

struct GraphWorkerLivenessGuard;

impl GraphWorkerLivenessGuard {
    fn new() -> Self {
        service_guard::record_graph_worker_started();
        Self
    }
}

impl Drop for GraphWorkerLivenessGuard {
    fn drop(&mut self) {
        service_guard::record_graph_worker_stopped();
    }
}

struct LeaseRefreshGuard {
    stop: Arc<std::sync::atomic::AtomicBool>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl LeaseRefreshGuard {
    fn start(
        graph_store: Arc<GraphStore>,
        work: Vec<FileVectorizationWork>,
        lease_owner: &'static str,
    ) -> Option<Self> {
        if work.is_empty() {
            return None;
        }

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let join_handle = thread::spawn(move || {
            while !stop_for_thread.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = graph_store.refresh_file_vectorization_leases_for_owner(&work, lease_owner);
                service_guard::record_vector_worker_heartbeat();
                thread::park_timeout(Duration::from_secs(1));
            }
        });

        Some(Self {
            stop,
            join_handle: Some(join_handle),
        })
    }
}

impl Drop for LeaseRefreshGuard {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(join_handle) = self.join_handle.take() {
            join_handle.thread().unpark();
            let _ = join_handle.join();
        }
    }
}

#[derive(Debug, Clone)]
struct FatalVectorWorkerFault {
    stage: &'static str,
    reason_raw: String,
    fatal_class: String,
    batch_id: Option<String>,
    texts_count: u64,
    input_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct VectorWorkerSupervisorState {
    restart_attempt: u64,
    crash_loop_open: bool,
}

#[derive(Debug)]
pub(crate) struct VectorPersistPlan {
    pub(crate) updates: Vec<(String, String, Vec<f32>)>,
    pub(crate) completed_works: Vec<FileVectorizationWork>,
    pub(crate) next_active_after_failure: Vec<FileVectorizationWork>,
    pub(crate) touched_works: Vec<FileVectorizationWork>,
    pub(crate) batch_run: VectorBatchRun,
}

impl VectorPersistPlan {
    pub(crate) fn sync_batch_run_counts(&mut self) -> (u64, u64) {
        let chunk_count = self.updates.len() as u64;
        let file_count = self.touched_works.len() as u64;
        self.batch_run.chunk_count = chunk_count;
        self.batch_run.file_count = file_count;
        (chunk_count, file_count)
    }
}

#[derive(Debug)]
pub(crate) struct VectorPersistOutcome {
    completed_works: Vec<FileVectorizationWork>,
    batch_runs: Vec<VectorBatchRun>,
    next_active_after_failure: Vec<FileVectorizationWork>,
    touched_works: Vec<FileVectorizationWork>,
    error_reason: Option<String>,
}

const FASTEMBED_OUTPUT_PRECEDENCE: &[OutputKey] = &[
    OutputKey::OnlyOne,
    OutputKey::ByName("text_embeds"),
    OutputKey::ByName("last_hidden_state"),
    OutputKey::ByName("sentence_embedding"),
];

fn normalize_embedding(mut values: Vec<f32>) -> Vec<f32> {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut values {
            *value /= norm;
        }
    }
    values
}

fn configured_embedding_max_length() -> usize {
    profile_configured_embedding_max_length()
}

fn configured_embedding_token_bucket_size() -> usize {
    profile_configured_embedding_token_bucket_size()
}

fn runtime_embedding_snapshot_dir() -> AnyhowResult<PathBuf> {
    profile_runtime_embedding_snapshot_dir()
}

fn gpu_total_vram_hint_mb() -> Option<u64> {
    if let Some(total_mb) = std::env::var("AXON_GPU_TOTAL_VRAM_MB_HINT")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1024)
    {
        return Some(total_mb);
    }
    if !cfg!(test) {
        if let Some(total_mb) = current_gpu_memory_snapshot().map(|snapshot| snapshot.total_mb) {
            return Some(total_mb);
        }
    }
    None
}

fn gpu_bootstrap_vector_worker_cap(
    requested_vector_workers: usize,
    total_vram_mb: Option<u64>,
) -> usize {
    let requested_vector_workers = requested_vector_workers.max(1);
    let cap = match total_vram_mb {
        Some(total_mb) if total_mb <= 8_192 => 1,
        Some(total_mb) if total_mb <= 12_288 => 2,
        _ => 3,
    };
    requested_vector_workers.min(cap)
}

fn bootstrap_embedding_lane_config_from_env() -> EmbeddingLaneConfig {
    let query_workers = env_usize("AXON_QUERY_EMBED_WORKERS", 1);
    let requested_vector_workers = env_usize("AXON_VECTOR_WORKERS", 1);
    let oversubscription_allowed = std::env::var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let vector_workers = if embedding_provider_requested_is_gpu() && !oversubscription_allowed {
        gpu_bootstrap_vector_worker_cap(requested_vector_workers, gpu_total_vram_hint_mb())
    } else {
        requested_vector_workers.max(1)
    };

    EmbeddingLaneConfig {
        query_workers,
        vector_workers,
        graph_workers: if graph_embeddings_enabled() {
            env_usize_nonnegative("AXON_GRAPH_WORKERS", 1)
        } else {
            0
        },
        chunk_batch_size: env_usize("AXON_CHUNK_BATCH_SIZE", CHUNK_BATCH_SIZE),
        file_vectorization_batch_size: env_usize(
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            FILE_VECTORIZATION_BATCH_SIZE,
        ),
        graph_batch_size: env_usize("AXON_GRAPH_BATCH_SIZE", GRAPH_BATCH_SIZE),
        max_chunks_per_file: env_usize("AXON_MAX_CHUNKS_PER_FILE", MAX_CHUNKS_PER_FILE),
        max_embed_batch_bytes: env_usize("AXON_MAX_EMBED_BATCH_BYTES", MAX_EMBED_BATCH_BYTES),
    }
}

fn bootstrap_runtime_tuning_state_from_env() -> RuntimeTuningState {
    let lane_config = bootstrap_embedding_lane_config_from_env();
    let embed_micro_batch_max_items = std::env::var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(lane_config.chunk_batch_size.max(1));
    let max_length = configured_embedding_max_length();
    let embed_micro_batch_max_total_tokens =
        std::env::var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value >= max_length)
            .unwrap_or(
                embed_micro_batch_max_items
                    .saturating_mul((max_length / 2).max(1))
                    .max(max_length),
            );
    let vector_ready_queue_depth = std::env::var("AXON_VECTOR_READY_QUEUE_DEPTH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(24);
    let vector_persist_queue_bound = std::env::var("AXON_VECTOR_PERSIST_QUEUE_BOUND")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(VECTOR_PERSIST_QUEUE_BOUND.max(32));
    let vector_max_inflight_persists = std::env::var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(vector_persist_queue_bound.max(1))
        .max(1);
    let semantic_sleep_scale_pct = std::env::var("AXON_SEMANTIC_SLEEP_SCALE_PCT")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100);
    let semantic_idle_sleep_scale_pct = std::env::var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100);
    RuntimeTuningState {
        vector_workers: lane_config.vector_workers,
        graph_workers: lane_config.graph_workers,
        chunk_batch_size: lane_config.chunk_batch_size,
        file_vectorization_batch_size: lane_config.file_vectorization_batch_size,
        vector_ready_queue_depth,
        vector_persist_queue_bound,
        vector_max_inflight_persists,
        embed_micro_batch_max_items,
        embed_micro_batch_max_total_tokens,
        semantic_sleep_scale_pct,
        semantic_idle_sleep_scale_pct,
    }
}

pub fn bootstrap_runtime_tuning_state() -> RuntimeTuningState {
    bootstrap_runtime_tuning_state_from_env()
}

fn configured_vector_prepare_queue_bound() -> usize {
    let configured = std::env::var("AXON_VECTOR_PREPARE_QUEUE_BOUND")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_VECTOR_PREPARE_QUEUE_BOUND);
    let baseline_chunk_capacity = current_runtime_tuning_snapshot()
        .state
        .chunk_batch_size
        .max(1);
    let target_ready_batches = configured_target_ready_chunks().div_ceil(baseline_chunk_capacity);
    let high_watermark_batches =
        configured_gpu_ready_high_watermark_chunks().div_ceil(baseline_chunk_capacity);
    configured
        .max(high_watermark_batches.saturating_mul(2))
        .max(target_ready_batches.saturating_mul(2))
}

fn effective_vector_lane_graph_backlog_depth(
    lane_config: EmbeddingLaneConfig,
    graph_backlog_depth: usize,
) -> usize {
    if lane_config.graph_workers == 0 {
        0
    } else {
        graph_backlog_depth
    }
}

pub fn current_runtime_tuning_snapshot() -> RuntimeTuningSnapshot {
    runtime_tuning_snapshot(bootstrap_runtime_tuning_state_from_env())
}

#[cfg(test)]
fn refresh_runtime_tuning_snapshot_from_env() -> RuntimeTuningSnapshot {
    crate::runtime_tuning::reset_runtime_tuning_snapshot(bootstrap_runtime_tuning_state_from_env())
}

fn configured_embedding_micro_batch_max_items(total_items: usize) -> usize {
    current_runtime_tuning_snapshot()
        .state
        .embed_micro_batch_max_items
        .clamp(1, total_items.max(1))
}

fn configured_embedding_micro_batch_max_total_tokens(total_items: usize) -> usize {
    let max_length = configured_embedding_max_length();
    current_runtime_tuning_snapshot()
        .state
        .embed_micro_batch_max_total_tokens
        .clamp(max_length, max_length.saturating_mul(total_items.max(1)))
}

fn configured_embedding_batch_max_total_tokens(total_items: usize) -> usize {
    let max_length = configured_embedding_max_length();
    std::env::var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= max_length)
        .unwrap_or_else(|| {
            configured_embedding_micro_batch_max_total_tokens(total_items).saturating_mul(2)
        })
        .clamp(max_length, max_length.saturating_mul(total_items.max(1)))
}

fn configured_vector_prepare_pipeline_depth() -> usize {
    std::env::var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_VECTOR_PREPARE_PIPELINE_DEPTH)
}

fn configured_vector_prepare_workers_per_vector() -> usize {
    std::env::var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_VECTOR_PREPARE_WORKERS_PER_VECTOR)
}

fn ort_pooling_cls(
    shape: &[i64],
    tensor: &[f32],
    batch_size: usize,
) -> AnyhowResult<Vec<Vec<f32>>> {
    match shape.len() {
        2 => {
            let hidden_size = *shape.get(1).unwrap_or(&0) as usize;
            if hidden_size == 0 || tensor.len() != batch_size.saturating_mul(hidden_size) {
                return Err(anyhow!(
                    "invalid ORT embedding output shape {:?} for CLS pooling",
                    shape
                ));
            }
            Ok(tensor
                .chunks(hidden_size)
                .map(|row| row.to_vec())
                .collect::<Vec<_>>())
        }
        3 => {
            let tokens = *shape.get(1).unwrap_or(&0) as usize;
            let hidden_size = *shape.get(2).unwrap_or(&0) as usize;
            if tokens == 0
                || hidden_size == 0
                || tensor.len()
                    != batch_size
                        .saturating_mul(tokens)
                        .saturating_mul(hidden_size)
            {
                return Err(anyhow!(
                    "invalid ORT embedding output shape {:?} for CLS pooling",
                    shape
                ));
            }
            let row_stride = tokens.saturating_mul(hidden_size);
            Ok((0..batch_size)
                .map(|row| {
                    let start = row.saturating_mul(row_stride);
                    tensor[start..start + hidden_size].to_vec()
                })
                .collect())
        }
        _ => Err(anyhow!(
            "invalid ORT embedding output shape {:?}; expected 2D or 3D tensor",
            shape
        )),
    }
}

fn ort_pooling_mean(
    shape: &[i64],
    token_embeddings: &[f32],
    attention_mask: &[i64],
    batch_size: usize,
    sequence_len: usize,
) -> AnyhowResult<Vec<Vec<f32>>> {
    if shape.len() == 2 {
        return ort_pooling_cls(shape, token_embeddings, batch_size);
    } else if shape.len() != 3 {
        return Err(anyhow!(
            "invalid ORT embedding output shape {:?}; expected 2D or 3D tensor",
            shape
        ));
    }

    let tokens = *shape.get(1).unwrap_or(&0) as usize;
    let hidden_size = *shape.get(2).unwrap_or(&0) as usize;
    if tokens != sequence_len
        || attention_mask.len() != batch_size.saturating_mul(sequence_len)
        || token_embeddings.len()
            != batch_size
                .saturating_mul(tokens)
                .saturating_mul(hidden_size)
    {
        return Err(anyhow!(
            "invalid ORT embedding output shape {:?} for mean pooling",
            shape
        ));
    }

    let row_stride = tokens.saturating_mul(hidden_size);
    let mut pooled = Vec::with_capacity(batch_size);
    for row in 0..batch_size {
        let mut sum = vec![0.0_f32; hidden_size];
        let mut weight = 0.0_f32;
        for token_idx in 0..tokens {
            let mask_value = attention_mask[row.saturating_mul(sequence_len) + token_idx] as f32;
            if mask_value <= 0.0 {
                continue;
            }
            let offset = row.saturating_mul(row_stride) + token_idx.saturating_mul(hidden_size);
            for hidden_idx in 0..hidden_size {
                sum[hidden_idx] += token_embeddings[offset + hidden_idx] * mask_value;
            }
            weight += mask_value;
        }
        let divisor = if weight > 0.0 { weight } else { 1.0 };
        for value in &mut sum {
            *value /= divisor;
        }
        pooled.push(sum);
    }
    Ok(pooled)
}

fn configured_gpu_ready_low_watermark() -> usize {
    configured_gpu_ready_low_watermark_chunks()
}

fn configured_gpu_ready_high_watermark() -> usize {
    configured_gpu_ready_high_watermark_chunks()
}

fn gpu_ready_queue_push_allowed(
    ready_depth: usize,
    inflight_prepares: usize,
    low_watermark: usize,
    target_depth: usize,
) -> bool {
    if ready_depth >= target_depth {
        return false;
    }
    if ready_depth.saturating_add(inflight_prepares) < low_watermark {
        return true;
    }
    ready_depth.saturating_add(inflight_prepares) < target_depth
}

fn continuous_prepare_feed_allowed(
    gpu_memory_pressure: bool,
    ready_depth: usize,
    inflight_prepares: usize,
    low_watermark: usize,
    target_depth: usize,
    max_inflight_prepares: usize,
    claimable_backlog_depth: usize,
    active_len: usize,
) -> bool {
    if gpu_memory_pressure || inflight_prepares >= max_inflight_prepares {
        return false;
    }
    if !gpu_ready_queue_push_allowed(ready_depth, inflight_prepares, low_watermark, target_depth) {
        return false;
    }
    active_len > 0 || claimable_backlog_depth > 0
}

fn vector_prepare_prefetch_limits(
    configured_max_inflight_prepares: usize,
    target_ready_depth: usize,
) -> (usize, usize) {
    let baseline_chunk_capacity = current_runtime_tuning_snapshot()
        .state
        .chunk_batch_size
        .max(1);
    let target_ready_depth = target_ready_depth
        .max(configured_gpu_ready_high_watermark().div_ceil(baseline_chunk_capacity))
        .max(1);
    let max_inflight_prepares =
        configured_max_inflight_prepares.max(target_ready_depth.saturating_mul(2));
    (max_inflight_prepares, target_ready_depth)
}

fn replenish_target_chunks(target_chunks: usize, replenishment_deficit: usize) -> usize {
    if replenishment_deficit == 0 {
        return target_chunks.max(1);
    }
    replenishment_deficit.min(target_chunks.max(1)).max(1)
}

fn replenish_target_ready_depth(
    reserve_gap: usize,
    replenishment_deficit: usize,
    request_target_chunks: usize,
    request_ready_depth_ceiling: usize,
) -> usize {
    let replenishment_batch_gap = if replenishment_deficit == 0 {
        0
    } else {
        replenishment_deficit.div_ceil(request_target_chunks.max(1))
    };
    reserve_gap
        .max(replenishment_batch_gap)
        .clamp(1, request_ready_depth_ceiling.max(1))
}

fn chunk_capacity_to_batch_depth(chunk_count: usize, batch_chunk_capacity: usize) -> usize {
    if chunk_count == 0 {
        return 0;
    }
    chunk_count.div_ceil(batch_chunk_capacity.max(1))
}

fn default_chunks_per_file_estimate() -> usize {
    std::env::var("AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
}

fn target_ready_low_watermark_chunks(batch_chunk_capacity: usize) -> usize {
    configured_gpu_ready_low_watermark().max(batch_chunk_capacity.max(1))
}

fn target_ready_high_watermark_chunks(batch_chunk_capacity: usize) -> usize {
    configured_gpu_ready_high_watermark().max(batch_chunk_capacity.max(1))
}

fn single_worker_gpu_prepare_worker_count(
    provider_is_gpu: bool,
    vector_workers: usize,
    configured_workers_per_vector: usize,
) -> usize {
    if provider_is_gpu && vector_workers <= 1 {
        configured_workers_per_vector.max(4)
    } else {
        configured_workers_per_vector
    }
}

fn configured_vector_persist_queue_bound() -> usize {
    current_runtime_tuning_snapshot()
        .state
        .vector_persist_queue_bound
        .max(1)
}

fn configured_vector_max_inflight_persists() -> usize {
    current_runtime_tuning_snapshot()
        .state
        .vector_max_inflight_persists
        .max(1)
}

fn configured_vector_outbox_fetch_batch_size() -> usize {
    std::env::var("AXON_VECTOR_OUTBOX_FETCH_BATCH_SIZE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(VECTOR_OUTBOX_FETCH_BATCH_SIZE)
}

fn wait_for_vector_backlog_or_timeout(timeout: Duration) -> bool {
    service_guard::wait_for_vector_backlog_signal(timeout)
}

fn vector_worker_restart_window_ms() -> i64 {
    std::env::var("AXON_VECTOR_WORKER_RESTART_WINDOW_MS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(600_000)
}

fn vector_worker_restart_budget() -> usize {
    std::env::var("AXON_VECTOR_WORKER_RESTART_BUDGET")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3)
}

fn vector_worker_restart_backoff_ms(restart_attempt: u64) -> u64 {
    let exponent = restart_attempt.saturating_sub(1).min(4) as u32;
    5_000_u64
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(30_000)
}

fn vector_worker_crash_loop_cooldown_ms() -> u64 {
    std::env::var("AXON_VECTOR_WORKER_CRASH_LOOP_COOLDOWN_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(60_000)
}

fn sleep_with_vector_worker_heartbeat(timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        service_guard::record_vector_worker_heartbeat();
        let remaining = timeout.saturating_sub(started.elapsed());
        thread::sleep(remaining.min(Duration::from_millis(250)));
    }
}

/// Whether the pre-batch VRAM guard is active.
///
/// Default: **true**. On 8GB GPUs, VRAM exhaustion causes unified memory
/// spill to system RAM over PCIe, degrading throughput 2-100x (measured by
/// NVIDIA: on-demand streaming ~5.4 GB/s vs local VRAM ~300+ GB/s). The
/// guard MUST be on by default for GPU indexers to prevent this.
fn gpu_pre_batch_vram_guard_enabled() -> bool {
    std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true)
}

/// Number of NVML telemetry samples to collect while waiting for VRAM to
/// drop below the admission threshold.
///
/// Default: **4**. CUDA deallocation is near-instant; when a subprocess is
/// killed, the driver reclaims VRAM within one NVML polling cycle. ORT's
/// BFC arena releases all CUDA memory on session/process destruction. Four
/// samples at 300ms intervals (1.2s total) is sufficient to observe the
/// full memory release. The old value of 6 samples added latency without
/// benefit. Too few (1-2) risks missing a release still in-flight; too
/// many (>6) delays batch dispatch unnecessarily.
fn gpu_pre_batch_vram_guard_samples() -> usize {
    std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
}

/// Milliseconds to wait between NVML telemetry samples.
///
/// Default: **300ms**. The NVML telemetry cache TTL in Axon is 250ms
/// (configured via AXON_GPU_TELEMETRY_CACHE_TTL_MS). A 300ms interval
/// guarantees at least one fresh NVML read per sample (cache expires at
/// 250ms, so 300ms ensures a new driver query). NVML itself has no
/// significant reporting lag -- `nvmlDeviceGetMemoryInfo` is a synchronous
/// driver ioctl. Too short (<250ms) wastes CPU re-reading cached values;
/// too long (>500ms) delays the guard decision and stalls the batch
/// pipeline for up to samples * wait_ms.
fn gpu_pre_batch_vram_guard_wait_ms() -> u64 {
    std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 50)
        .unwrap_or(300)
}

/// Minimum VRAM drop (in MB) required to consider recovery in progress.
///
/// Default: **128MB**. ORT's BFC arena allocates GPU memory in
/// power-of-two chunks (kNextPowerOfTwo strategy). The smallest meaningful
/// release from an embedding model session is ~128MB (model weights +
/// TensorRT workspace for a small transformer). The old 64MB threshold
/// could be triggered by driver bookkeeping fluctuations (~10-50MB) or
/// NVML rounding artifacts (reports in whole MB). Too low (<64MB) causes
/// false "recovery detected" signals from noise; too high (>256MB) misses
/// genuine partial releases.
fn gpu_pre_batch_vram_guard_min_drop_mb() -> u64 {
    std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(128)
}

/// Whether to trigger a recycle when NVML telemetry is unavailable.
///
/// Default: **true**. Without telemetry, the guard cannot distinguish
/// safe VRAM headroom from imminent OOM. On an 8GB GPU, blind embedding
/// risks unified memory spill to system RAM, which destroys throughput
/// by 40x or more. The conservative choice is to recycle the subprocess
/// (which probes VRAM on restart via cudaMalloc). The cost is one 2-4s
/// restart; the risk of not recycling is catastrophic throughput collapse.
fn gpu_pre_batch_vram_guard_unknown_recycle() -> bool {
    std::env::var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true)
}

fn gpu_pre_batch_vram_recycle_reason(
    initial_snapshot: Option<GpuMemorySnapshot>,
) -> Option<String> {
    gpu_pre_batch_vram_recycle_reason_with_probe(initial_snapshot, current_gpu_memory_snapshot)
}

/// After this many consecutive guard-triggered recycles without VRAM improvement,
/// the guard enters a cooldown period to prevent infinite recycle cascades.
///
/// **Research basis (NVIDIA CUDA / ORT / TensorRT on 8GB RTX, WSL2):**
///
/// - **BACKOFF_THRESHOLD = 5**: Each subprocess recycle takes 2-4s (CUDA context
///   init ~250ms + ORT/TensorRT session load from engine cache ~1-2s). Five
///   recycles therefore spans 10-20s of probing, enough to confirm that VRAM
///   pressure is persistent (e.g. another process holds allocations) rather than
///   transient. Setting this too low (e.g. 2-3) causes premature cooldown when
///   the subprocess simply needs one more restart to release a stale BFC arena.
///   Setting it too high wastes throughput on futile recycles.
///
/// - **COOLDOWN_MS = 30_000**: External VRAM consumers (display server, WSL2
///   compositor, other CUDA processes) typically stabilize within 15-30s.
///   A 30s pause lets transient pressure subside without sacrificing 60s of
///   embedding throughput. Too long stalls the pipeline; too short wastes
///   CPU on futile recycles. At 5 recycles × ~2s each, the system has already
///   spent ~10s trying. A 5s cooldown is enough to detect transient VRAM
///   consumers; if persistent, the next 5-recycle cycle provides clear log signal.
// Retained for the internal cooldown logic in gpu_pre_batch_vram_recycle_reason_with_probe.
// The unified VramRecycleCoordinator now governs the outer recycle cadence.
#[allow(dead_code)]
const GUARD_RECYCLE_BACKOFF_THRESHOLD: u32 = 5;
#[allow(dead_code)]
const GUARD_RECYCLE_COOLDOWN_MS: u64 = 5_000;

#[allow(dead_code)]
static GUARD_CONSECUTIVE_RECYCLES: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
static GUARD_COOLDOWN_UNTIL_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Minimum interval between coordinated VRAM recycles (15s = 3x TensorRT cache reload time).
const RECYCLE_MIN_INTERVAL_MS: u64 = 15_000;

/// VRAM usage percentage above which a recycle is considered critical (OOM-imminent).
const RECYCLE_VRAM_CRITICAL_PCT: u64 = 95;

/// Signals collected from the 4 independent VRAM recycle triggers before the
/// coordinator makes a single coordinated decision.
struct RecycleSignals {
    stuck: bool,
    summit: bool,
    pre_batch_plateau: bool,
    low_throughput: bool,
    /// True when VRAM usage exceeds [`RECYCLE_VRAM_CRITICAL_PCT`] of total VRAM.
    vram_critical: bool,
    vram_used_mb: u64,
}

/// Unified recycle coordinator that replaces the 4 independent VRAM recycle
/// triggers with a single decision point.  This prevents cascading restarts
/// by enforcing a global cooldown and requiring signal consensus before
/// recycling.
struct VramRecycleCoordinator {
    last_recycle_at_ms: u64,
    consecutive_pressure_batches: u32,
    total_recycles: u64,
}

impl VramRecycleCoordinator {
    fn new() -> Self {
        Self {
            last_recycle_at_ms: 0,
            consecutive_pressure_batches: 0,
            total_recycles: 0,
        }
    }

    /// Returns `Some(reason)` if a recycle should happen, `None` if suppressed.
    fn should_recycle(&mut self, signals: &RecycleSignals) -> Option<String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let elapsed = now_ms.saturating_sub(self.last_recycle_at_ms);

        // Hard cooldown: never recycle within RECYCLE_MIN_INTERVAL_MS.
        if self.last_recycle_at_ms > 0 && elapsed < RECYCLE_MIN_INTERVAL_MS {
            return None;
        }

        // Critical: OOM-imminent (>95% VRAM) — always recycle.
        if signals.vram_critical {
            self.record_recycle(now_ms);
            return Some(format!(
                "vram_critical used={}MB total_recycles={}",
                signals.vram_used_mb, self.total_recycles
            ));
        }

        // Soft: multiple signals agree — recycle.
        let signal_count = [
            signals.stuck,
            signals.summit,
            signals.pre_batch_plateau,
            signals.low_throughput,
        ]
        .iter()
        .filter(|s| **s)
        .count();

        if signal_count >= 2 {
            self.record_recycle(now_ms);
            return Some(format!(
                "multi_signal count={} stuck={} summit={} plateau={} low_throughput={} used={}MB total_recycles={}",
                signal_count, signals.stuck, signals.summit, signals.pre_batch_plateau,
                signals.low_throughput, signals.vram_used_mb, self.total_recycles
            ));
        }

        // Single signal: count consecutive, recycle only after 5 consecutive batches.
        if signal_count == 1 {
            self.consecutive_pressure_batches += 1;
            if self.consecutive_pressure_batches >= 5 {
                self.record_recycle(now_ms);
                return Some(format!(
                    "sustained_pressure consecutive={} stuck={} summit={} plateau={} low_throughput={} used={}MB total_recycles={}",
                    self.consecutive_pressure_batches, signals.stuck, signals.summit,
                    signals.pre_batch_plateau, signals.low_throughput, signals.vram_used_mb,
                    self.total_recycles
                ));
            }
            return None;
        }

        // No signals: reset counter.
        self.consecutive_pressure_batches = 0;
        None
    }

    fn record_recycle(&mut self, now_ms: u64) {
        self.last_recycle_at_ms = now_ms;
        self.consecutive_pressure_batches = 0;
        self.total_recycles += 1;
    }
}

fn gpu_pre_batch_vram_recycle_reason_with_probe(
    initial_snapshot: Option<GpuMemorySnapshot>,
    mut next_snapshot: impl FnMut() -> Option<GpuMemorySnapshot>,
) -> Option<String> {
    if !gpu_pre_batch_vram_guard_enabled() {
        return None;
    }

    // Backoff: if we hit the cascade threshold, enter cooldown and skip guard.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let cooldown_until = GUARD_COOLDOWN_UNTIL_MS.load(std::sync::atomic::Ordering::Relaxed);
    if now_ms < cooldown_until {
        return None;
    }

    let admission_mb = gpu_primary_worker_max_used_mb();
    let mut snapshot = initial_snapshot.or_else(&mut next_snapshot);
    let Some(first) = snapshot else {
        return gpu_pre_batch_vram_guard_unknown_recycle()
            .then(|| "gpu_pre_batch_vram_unknown telemetry_unavailable_before_batch".to_string());
    };
    if first.used_mb < admission_mb {
        return None;
    }

    let mut lowest_used_mb = first.used_mb;
    let mut last_used_mb = first.used_mb;
    let wait = Duration::from_millis(gpu_pre_batch_vram_guard_wait_ms());
    for _ in 0..gpu_pre_batch_vram_guard_samples() {
        sleep_with_vector_worker_heartbeat(wait);
        snapshot = next_snapshot();
        let Some(current) = snapshot else {
            return gpu_pre_batch_vram_guard_unknown_recycle().then(|| {
                format!(
                    "gpu_pre_batch_vram_unknown_after_wait first_used_mb={} admission_mb={}",
                    first.used_mb, admission_mb
                )
            });
        };
        lowest_used_mb = lowest_used_mb.min(current.used_mb);
        last_used_mb = current.used_mb;
        if current.used_mb < admission_mb {
            return None;
        }
    }

    let observed_drop_mb = first.used_mb.saturating_sub(lowest_used_mb);
    if observed_drop_mb < gpu_pre_batch_vram_guard_min_drop_mb() {
        return Some(format!(
            "gpu_pre_batch_vram_plateau first_used_mb={} last_used_mb={} lowest_used_mb={} admission_mb={} observed_drop_mb={}",
            first.used_mb, last_used_mb, lowest_used_mb, admission_mb, observed_drop_mb
        ));
    }

    Some(format!(
        "gpu_pre_batch_vram_still_above_admission first_used_mb={} last_used_mb={} lowest_used_mb={} admission_mb={} observed_drop_mb={}",
        first.used_mb, last_used_mb, lowest_used_mb, admission_mb, observed_drop_mb
    ))
}

fn token_count_from_encoding(encoding: &Encoding) -> usize {
    encoding
        .get_attention_mask()
        .iter()
        .map(|value| *value as usize)
        .sum::<usize>()
        .max(1)
}

fn load_runtime_embedding_tokenizer() -> AnyhowResult<Tokenizer> {
    profile_load_runtime_embedding_tokenizer()
}

fn build_token_aware_micro_batches(
    token_counts: &[usize],
    bucket_size: usize,
    max_items: usize,
    max_total_tokens: usize,
) -> Vec<Vec<usize>> {
    if token_counts.is_empty() {
        return Vec::new();
    }

    let bucket_size = bucket_size.max(1);
    let max_items = max_items.max(1);
    let max_total_tokens = max_total_tokens.max(1);
    let mut bucketed: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (index, token_count) in token_counts.iter().copied().enumerate() {
        let bucket = token_count.saturating_sub(1) / bucket_size;
        bucketed
            .entry(bucket)
            .or_default()
            .push((index, token_count));
    }

    let mut bucket_ids = bucketed.keys().copied().collect::<Vec<_>>();
    bucket_ids.sort_unstable();

    let mut micro_batches = Vec::new();
    for bucket in bucket_ids {
        let mut current = Vec::new();
        let mut current_tokens = 0usize;
        for (index, token_count) in bucketed.remove(&bucket).unwrap_or_default() {
            let token_budget = token_count.min(max_total_tokens);
            let would_overflow_items = current.len() >= max_items;
            let would_overflow_tokens = !current.is_empty()
                && current_tokens.saturating_add(token_budget) > max_total_tokens;
            if would_overflow_items || would_overflow_tokens {
                micro_batches.push(current);
                current = Vec::new();
                current_tokens = 0;
            }
            current.push(index);
            current_tokens = current_tokens.saturating_add(token_budget);
        }
        if !current.is_empty() {
            micro_batches.push(current);
        }
    }

    micro_batches
}

fn attach_preencoded_micro_batches(
    tokenizer: &Tokenizer,
    prepared: &mut PreparedVectorEmbedBatch,
) -> AnyhowResult<()> {
    if prepared.texts.is_empty() {
        prepared.token_counts = Vec::new();
        prepared.encoded_micro_batches = Vec::new();
        return Ok(());
    }

    let inputs = prepared
        .texts
        .iter()
        .map(|text| text.as_str())
        .collect::<Vec<_>>();
    let encodings = tokenizer
        .encode_batch(inputs, true)
        .map_err(|err| anyhow!("Failed to encode prepared batch: {}", err))?;
    prepared.token_counts = encodings.iter().map(token_count_from_encoding).collect();
    prepared.lane_thresholds = observe_token_lane_thresholds(&prepared.token_counts);
    let observed_lanes = prepared
        .token_counts
        .iter()
        .map(|token_count| prepared.lane_thresholds.classify(*token_count))
        .collect::<HashSet<_>>();
    prepared.batch_lane = if observed_lanes.len() == 1 {
        observed_lanes
            .iter()
            .copied()
            .next()
            .unwrap_or(VectorBatchLane::Mixed)
    } else {
        VectorBatchLane::Mixed
    };
    prepared.encoded_micro_batches = build_token_aware_micro_batches(
        &prepared.token_counts,
        configured_embedding_token_bucket_size(),
        configured_embedding_micro_batch_max_items(prepared.texts.len()),
        configured_embedding_micro_batch_max_total_tokens(prepared.texts.len()),
    )
    .into_iter()
    .map(|item_indices| PreparedEncodedMicroBatch {
        encodings: item_indices
            .iter()
            .map(|index| encodings[*index].clone())
            .collect(),
        item_indices,
    })
    .collect();
    Ok(())
}

fn clone_prepared_batch_with_indices(
    prepared: &PreparedVectorEmbedBatch,
    lane: VectorBatchLane,
    mixed_fallback: bool,
    indices: &[usize],
) -> PreparedVectorEmbedBatch {
    let selected_token_counts = indices
        .iter()
        .map(|index| prepared.token_counts[*index])
        .collect::<Vec<_>>();
    let encoding_lookup = prepared
        .encoded_micro_batches
        .iter()
        .flat_map(|batch| {
            batch
                .item_indices
                .iter()
                .copied()
                .zip(batch.encodings.iter().cloned())
        })
        .collect::<HashMap<usize, Encoding>>();
    let encoded_micro_batches = build_token_aware_micro_batches(
        &selected_token_counts,
        configured_embedding_token_bucket_size(),
        configured_embedding_micro_batch_max_items(indices.len()),
        configured_embedding_micro_batch_max_total_tokens(indices.len()),
    )
    .into_iter()
    .map(|item_indices| PreparedEncodedMicroBatch {
        encodings: item_indices
            .iter()
            .map(|new_index| {
                let old_index = indices[*new_index];
                encoding_lookup
                    .get(&old_index)
                    .cloned()
                    .expect("encoding must exist for split prepared batch")
            })
            .collect(),
        item_indices,
    })
    .collect::<Vec<_>>();
    PreparedVectorEmbedBatch {
        batch_id: format!("{}-{}", prepared.batch_id, lane.as_str()),
        prepare_started_at_ms: prepared.prepare_started_at_ms,
        prepare_finished_at_ms: prepared.prepare_finished_at_ms,
        prepared_at_ms: prepared.prepared_at_ms,
        batch_lane: lane,
        mixed_fallback,
        lane_thresholds: prepared.lane_thresholds,
        work_items: indices
            .iter()
            .map(|index| prepared.work_items[*index].clone())
            .collect(),
        texts: indices
            .iter()
            .map(|index| prepared.texts[*index].clone())
            .collect(),
        token_counts: selected_token_counts,
        encoded_micro_batches,
        touched_works: prepared.touched_works.clone(),
        finalize_after_success: prepared.finalize_after_success.clone(),
        immediate_completed: prepared.immediate_completed.clone(),
        oversized_works: prepared.oversized_works.clone(),
        next_active_after_success: prepared.next_active_after_success.clone(),
        next_active_after_failure: prepared.next_active_after_failure.clone(),
        files_touched: prepared.files_touched,
        partial_file_cycles: prepared.partial_file_cycles,
        fetch_ms_total: prepared.fetch_ms_total,
        failed_fetches: prepared.failed_fetches.clone(),
    }
}

fn split_prepared_batch_by_lane(
    prepared: PreparedVectorEmbedBatch,
    target_chunks: usize,
    ready_chunk_count: usize,
) -> Vec<PreparedBatchEnvelope> {
    if prepared.texts.is_empty() || prepared.token_counts.is_empty() {
        return vec![PreparedBatchEnvelope::new(prepared)];
    }

    let mut groups = HashMap::<VectorBatchLane, Vec<usize>>::new();
    for (index, token_count) in prepared.token_counts.iter().copied().enumerate() {
        groups
            .entry(prepared.lane_thresholds.classify(token_count))
            .or_default()
            .push(index);
    }

    let mut file_lanes = HashMap::<&str, VectorBatchLane>::new();
    let file_spans_multiple_lanes = prepared.work_items.iter().enumerate().any(|(index, item)| {
        let lane = prepared
            .lane_thresholds
            .classify(prepared.token_counts[index]);
        match file_lanes.insert(item.file_path.as_str(), lane) {
            Some(existing_lane) => existing_lane != lane,
            None => false,
        }
    });

    if groups.len() <= 1 || file_spans_multiple_lanes {
        return vec![PreparedBatchEnvelope::new(prepared)];
    }

    let useful_lane_chunks = target_chunks.clamp(2, 8);
    let largest_lane = groups.values().map(Vec::len).max().unwrap_or(0);
    let should_fallback_mixed =
        ready_chunk_count == 0 && largest_lane < useful_lane_chunks && prepared.chunk_count() > 0;
    if should_fallback_mixed {
        let mut fallback = prepared;
        fallback.batch_lane = VectorBatchLane::Mixed;
        fallback.mixed_fallback = true;
        return vec![PreparedBatchEnvelope::new(fallback)];
    }

    let mut split_batches = Vec::new();
    for lane in [
        VectorBatchLane::Large,
        VectorBatchLane::Medium,
        VectorBatchLane::Small,
    ] {
        let Some(indices) = groups.get(&lane) else {
            continue;
        };
        if indices.is_empty() {
            continue;
        }
        split_batches.push(PreparedBatchEnvelope::new(
            clone_prepared_batch_with_indices(&prepared, lane, false, indices),
        ));
    }
    if split_batches.is_empty() {
        vec![PreparedBatchEnvelope::new(prepared)]
    } else {
        split_batches
    }
}

fn split_prepared_batch_for_gpu_budget(
    prepared: PreparedVectorEmbedBatch,
) -> Vec<PreparedBatchEnvelope> {
    if prepared.texts.len() <= 1 || prepared.token_counts.is_empty() {
        return vec![PreparedBatchEnvelope::new(prepared)];
    }

    let max_total_tokens = configured_embedding_batch_max_total_tokens(prepared.texts.len());
    let mut segments = Vec::new();
    let mut current_indices = Vec::new();
    let mut current_tokens = 0usize;

    for (index, token_count) in prepared.token_counts.iter().copied().enumerate() {
        let token_budget = token_count.min(max_total_tokens);
        let would_overflow = !current_indices.is_empty()
            && current_tokens.saturating_add(token_budget) > max_total_tokens;
        if would_overflow {
            segments.push(current_indices);
            current_indices = Vec::new();
            current_tokens = 0;
        }
        current_indices.push(index);
        current_tokens = current_tokens.saturating_add(token_budget);
    }

    if !current_indices.is_empty() {
        segments.push(current_indices);
    }

    if segments.len() <= 1 {
        return vec![PreparedBatchEnvelope::new(prepared)];
    }

    segments
        .into_iter()
        .enumerate()
        .map(|(segment_idx, indices)| {
            let mut segment = clone_prepared_batch_with_indices(
                &prepared,
                prepared.batch_lane(),
                prepared.mixed_fallback(),
                indices.as_slice(),
            );
            segment.batch_id = format!("{}-gpu-{}", prepared.batch_id, segment_idx);
            PreparedBatchEnvelope::new(segment)
        })
        .collect()
}

fn build_prepared_vector_embed_sequence(
    graph_store: &GraphStore,
    active_works: &[FileVectorizationWork],
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
    prepare_started_at_ms: i64,
) -> PreparedVectorEmbedSequence {
    let mut batches = Vec::new();
    let mut current_active = active_works.to_vec();
    let mut reserved_chunk_ids = HashSet::new();
    let target_ready_depth = target_ready_depth.max(1);
    while !current_active.is_empty() && batches.len() < target_ready_depth {
        let mut prepared = prepare_vector_embed_batch(
            graph_store,
            &current_active,
            target_chunks,
            per_file_fetch_limit,
            batch_max_bytes,
            &reserved_chunk_ids,
        );
        for item in &prepared.work_items {
            reserved_chunk_ids.insert(item.chunk_id.clone());
        }
        let made_progress = !prepared.work_items.is_empty()
            || !prepared.immediate_completed.is_empty()
            || !prepared.oversized_works.is_empty()
            || !prepared.finalize_after_success.is_empty()
            || !prepared.failed_fetches.is_empty();
        current_active = prepared.next_active_after_success.clone();
        prepared.prepare_started_at_ms = prepare_started_at_ms;
        prepared.prepare_finished_at_ms = prepared.prepared_at_ms;
        batches.push(PreparedBatchEnvelope::new(prepared));
        if !made_progress {
            current_active.clear();
        }
    }

    PreparedVectorEmbedSequence {
        batches,
        remaining_claimed_after_success: ClaimedLeaseSet::new(current_active),
    }
}

fn embed_prepared_batch_with_breakdown_ort(
    model: &mut OrtGpuFirstTextEmbedding,
    prepared: &PreparedVectorEmbedBatch,
) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
    if !prepared.encoded_micro_batches.is_empty() {
        let total_started = Instant::now();
        let mut ordered_embeddings = vec![None; prepared.texts.len()];
        let mut host_prepare_ms = 0u64;
        let mut input_copy_ms = 0u64;
        let mut inference_ms = 0u64;
        let mut output_extract_ms = 0u64;

        for micro_batch in &prepared.encoded_micro_batches {
            abort_gpu_embed_if_vram_summit_reached()?;
            let (
                batch_embeddings,
                batch_host_prepare_ms,
                batch_input_copy_ms,
                batch_inference_ms,
                batch_output_extract_ms,
            ) = model.transform_encoded_with_breakdown(&micro_batch.encodings)?;
            host_prepare_ms = host_prepare_ms.saturating_add(batch_host_prepare_ms);
            input_copy_ms = input_copy_ms.saturating_add(batch_input_copy_ms);
            inference_ms = inference_ms.saturating_add(batch_inference_ms);
            output_extract_ms = output_extract_ms.saturating_add(batch_output_extract_ms);
            for (index, embedding) in micro_batch
                .item_indices
                .iter()
                .copied()
                .zip(batch_embeddings)
            {
                ordered_embeddings[index] = Some(embedding);
            }
            abort_gpu_embed_if_vram_summit_reached()?;
        }

        let embeddings = ordered_embeddings
            .into_iter()
            .map(|embedding| {
                embedding.ok_or_else(|| anyhow!("missing embedding after prepared ORT micro-batch"))
            })
            .collect::<AnyhowResult<Vec<_>>>()?;
        let _total_ms = total_started.elapsed().as_millis() as u64;
        return Ok((
            embeddings,
            host_prepare_ms,
            input_copy_ms,
            inference_ms,
            output_extract_ms,
        ));
    }

    if prepared.texts.is_empty() {
        return Ok((Vec::new(), 0, 0, 0, 0));
    }

    Err(anyhow!(
        "prepared batch reached ORT GPU-first runner without pre-tokenized micro-batches"
    ))
}

fn embed_texts_with_breakdown_ort(
    model: &mut OrtGpuFirstTextEmbedding,
    texts: &[String],
) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64)> {
    if texts.is_empty() {
        return Ok((Vec::new(), 0, 0, 0, 0));
    }

    let inputs = texts.iter().map(|text| text.as_str()).collect::<Vec<_>>();
    let encodings = model
        .tokenizer
        .encode_batch(inputs, true)
        .map_err(|err| anyhow!("Failed to encode the batch: {}", err))?;
    let token_counts = encodings
        .iter()
        .map(token_count_from_encoding)
        .collect::<Vec<_>>();
    let micro_batches = build_token_aware_micro_batches(
        &token_counts,
        configured_embedding_token_bucket_size(),
        configured_embedding_micro_batch_max_items(texts.len()),
        configured_embedding_micro_batch_max_total_tokens(texts.len()),
    );

    let mut ordered_embeddings = vec![None; texts.len()];
    let mut host_prepare_ms = 0u64;
    let mut input_copy_ms = 0u64;
    let mut inference_ms = 0u64;
    let mut output_extract_ms = 0u64;

    for batch_indices in micro_batches {
        abort_gpu_embed_if_vram_summit_reached()?;
        let batch_encodings = batch_indices
            .iter()
            .map(|index| encodings[*index].clone())
            .collect::<Vec<_>>();
        let (
            batch_embeddings,
            batch_host_prepare_ms,
            batch_input_copy_ms,
            batch_inference_ms,
            batch_output_extract_ms,
        ) = model.transform_encoded_with_breakdown(&batch_encodings)?;
        host_prepare_ms = host_prepare_ms.saturating_add(batch_host_prepare_ms);
        input_copy_ms = input_copy_ms.saturating_add(batch_input_copy_ms);
        inference_ms = inference_ms.saturating_add(batch_inference_ms);
        output_extract_ms = output_extract_ms.saturating_add(batch_output_extract_ms);
        for (index, embedding) in batch_indices.into_iter().zip(batch_embeddings) {
            ordered_embeddings[index] = Some(embedding);
        }
        abort_gpu_embed_if_vram_summit_reached()?;
    }

    let embeddings = ordered_embeddings
        .into_iter()
        .map(|embedding| {
            embedding.ok_or_else(|| anyhow!("missing embedding after ORT micro-batch scheduling"))
        })
        .collect::<AnyhowResult<Vec<_>>>()?;

    Ok((
        embeddings,
        host_prepare_ms,
        input_copy_ms,
        inference_ms,
        output_extract_ms,
    ))
}

fn effective_provider_request_for_lane(lane: &str) -> String {
    let normalized_lane = lane.trim().to_ascii_lowercase();
    let runtime_mode = AxonRuntimeMode::from_env();
    let canonical_provider = canonical_embedding_provider_request_for_mode(
        runtime_mode,
        RuntimeProfile::detect().gpu_present,
    )
    .to_ascii_lowercase();
    if normalized_lane == "query" {
        if let Some(explicit) = std::env::var("AXON_QUERY_EMBED_PROVIDER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
        {
            return explicit;
        }

        if canonical_provider.eq_ignore_ascii_case("cuda") {
            return "cpu".to_string();
        }
    }

    if normalized_lane == "graph" {
        if let Some(explicit) = std::env::var("AXON_GRAPH_EMBED_PROVIDER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
        {
            return explicit;
        }
    }

    canonical_provider
}

impl VectorBatchPlan {
    #[cfg_attr(not(test), allow(dead_code))]
    fn next_active_after_success(self) -> Vec<FileVectorizationWork> {
        let mut next = self.continuation_works;
        next.extend(self.untouched_works);
        next
    }
}

impl PreparedVectorEmbedBatch {
    pub(crate) fn chunk_count(&self) -> u64 {
        self.work_items.len() as u64
    }

    pub(crate) fn into_touched_works(self) -> Vec<FileVectorizationWork> {
        self.touched_works
    }

    pub(crate) fn touched_works_slice(&self) -> &[FileVectorizationWork] {
        &self.touched_works
    }

    pub(crate) fn prepared_at_ms(&self) -> i64 {
        self.prepared_at_ms
    }

    pub(crate) fn batch_lane(&self) -> VectorBatchLane {
        self.batch_lane
    }

    pub(crate) fn mixed_fallback(&self) -> bool {
        self.mixed_fallback
    }

    pub(crate) fn lane_thresholds(&self) -> TokenLaneThresholds {
        self.lane_thresholds
    }

    fn from_plan(plan: VectorBatchPlan) -> Self {
        Self::from_plan_with_prepare_started(plan, chrono::Utc::now().timestamp_millis())
    }

    fn from_plan_with_prepare_started(plan: VectorBatchPlan, prepare_started_at_ms: i64) -> Self {
        let VectorBatchPlan {
            work_items,
            touched_works,
            finalize_after_success,
            immediate_completed,
            oversized_works,
            continuation_works,
            untouched_works,
            files_touched,
            partial_file_cycles,
            fetch_ms_total,
            failed_fetches,
        } = plan;
        let prepared_at_ms = chrono::Utc::now().timestamp_millis();
        let texts = work_items.iter().map(|item| item.text.clone()).collect();
        let mut next_active_after_success = continuation_works;
        next_active_after_success.extend(untouched_works.clone());
        Self {
            batch_id: format!(
                "vec-prepared-{}",
                chrono::Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_micros())
            ),
            prepare_started_at_ms,
            prepare_finished_at_ms: prepared_at_ms,
            prepared_at_ms,
            batch_lane: VectorBatchLane::Mixed,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            texts,
            token_counts: Vec::new(),
            encoded_micro_batches: Vec::new(),
            work_items,
            touched_works,
            finalize_after_success,
            immediate_completed,
            oversized_works,
            next_active_after_success,
            next_active_after_failure: untouched_works,
            files_touched,
            partial_file_cycles,
            fetch_ms_total,
            failed_fetches,
        }
    }

    fn total_token_count(&self) -> u64 {
        self.token_counts.iter().copied().sum::<usize>() as u64
    }

    pub(crate) fn density_score(&self) -> u64 {
        let chunk_count = self.chunk_count().max(1);
        let base = self.total_token_count().saturating_mul(100) / chunk_count;
        if self.mixed_fallback {
            base / 4
        } else {
            base.saturating_add((self.batch_lane.priority() as u64) * 10_000)
        }
    }

    fn max_item_tokens(&self) -> u64 {
        self.token_counts.iter().copied().max().unwrap_or(0) as u64
    }

    fn avg_item_tokens(&self) -> f64 {
        if self.token_counts.is_empty() {
            0.0
        } else {
            self.total_token_count() as f64 / self.token_counts.len() as f64
        }
    }

    fn micro_batch_count(&self) -> u64 {
        self.encoded_micro_batches.len() as u64
    }

    fn max_micro_batch_tokens(&self) -> u64 {
        self.encoded_micro_batches
            .iter()
            .map(|batch| {
                batch
                    .item_indices
                    .iter()
                    .map(|index| self.token_counts.get(*index).copied().unwrap_or_default())
                    .sum::<usize>() as u64
            })
            .max()
            .unwrap_or(0)
    }

    fn avg_micro_batch_tokens(&self) -> f64 {
        if self.encoded_micro_batches.is_empty() {
            0.0
        } else {
            let total = self
                .encoded_micro_batches
                .iter()
                .map(|batch| {
                    batch
                        .item_indices
                        .iter()
                        .map(|index| self.token_counts.get(*index).copied().unwrap_or_default())
                        .sum::<usize>() as u64
                })
                .sum::<u64>();
            total as f64 / self.encoded_micro_batches.len() as f64
        }
    }

    pub(crate) fn into_persist_plan(
        self,
        embeddings: Vec<Vec<f32>>,
        batch_run: VectorBatchRun,
    ) -> AnyhowResult<VectorPersistPlan> {
        if self.work_items.len() != embeddings.len() {
            return Err(anyhow!(
                "embedding count mismatch: work_items={} embeddings={}",
                self.work_items.len(),
                embeddings.len()
            ));
        }
        let updates = self
            .work_items
            .into_iter()
            .zip(embeddings)
            .map(|(item, emb)| (item.chunk_id, item.content_hash, emb))
            .collect();
        let mut completed_works = self.immediate_completed;
        completed_works.extend(self.finalize_after_success);
        Ok(VectorPersistPlan {
            updates,
            completed_works,
            next_active_after_failure: self.next_active_after_failure,
            touched_works: self.touched_works,
            batch_run,
        })
    }
}

impl SemanticWorkerPool {
    pub fn new(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) -> Self {
        let config = embedding_lane_config_from_env();
        info!(
            "Semantic Factory: spawning graph/vector semantic workers (query_support_workers={}, vector_workers={}, graph_workers={}, chunk_batch_size={}, file_batch_size={}, graph_batch_size={})",
            config.query_workers,
            config.vector_workers,
            config.graph_workers,
            config.chunk_batch_size,
            config.file_vectorization_batch_size,
            config.graph_batch_size
        );

        let (query_tx, query_rx) = unbounded();
        register_query_embedding_sender(query_tx);

        let mut query_workers = Vec::new();
        for worker_idx in 0..config.query_workers {
            let query_rx = query_rx.clone();
            query_workers.push(thread::spawn(move || {
                Self::query_worker_loop(worker_idx, query_rx);
            }));
        }

        let mut vector_prepare_workers = Vec::new();
        let mut vector_refill_workers = Vec::new();
        let mut vector_workers = Vec::new();
        let mut vector_persist_workers = Vec::new();
        let mut vector_finalize_workers = Vec::new();
        let mut vector_maintenance_workers = Vec::new();
        if config.vector_workers > 0 {
            let maintenance_graph_store = Arc::clone(&graph_store);
            vector_maintenance_workers.push(thread::spawn(move || {
                Self::vector_maintenance_worker_loop(maintenance_graph_store);
            }));
        }
        let bootstrap_prepare_workers_per_vector = single_worker_gpu_prepare_worker_count(
            embedding_provider_requested_is_gpu(),
            config.vector_workers,
            configured_vector_prepare_workers_per_vector(),
        );
        for worker_idx in 0..config.vector_workers {
            let graph_store = Arc::clone(&graph_store);
            let ready_queue = Arc::new(SharedPreparedBatchQueue::new());
            let (prepare_tx, prepare_rx) =
                bounded::<VectorPrepareRequest>(configured_vector_prepare_queue_bound());
            let (refill_tx, refill_rx) = unbounded::<VectorRefillCommand>();
            let (persist_tx, persist_rx) =
                bounded::<VectorPersistRequest>(configured_vector_persist_queue_bound());
            let (finalize_tx, finalize_rx) =
                bounded::<VectorFinalizeRequest>(VECTOR_FINALIZE_QUEUE_BOUND);
            for _ in 0..bootstrap_prepare_workers_per_vector {
                let prepare_graph_store = Arc::clone(&graph_store);
                let prepare_rx = prepare_rx.clone();
                let prepare_ready_queue = Arc::clone(&ready_queue);
                vector_prepare_workers.push(thread::spawn(move || {
                    Self::vector_prepare_worker_loop(
                        worker_idx,
                        prepare_graph_store,
                        prepare_rx,
                        prepare_ready_queue,
                    );
                }));
            }
            let persist_graph_store = Arc::clone(&graph_store);
            vector_persist_workers.push(thread::spawn(move || {
                Self::vector_persist_worker_loop(worker_idx, persist_graph_store, persist_rx);
            }));
            let finalize_graph_store = Arc::clone(&graph_store);
            vector_finalize_workers.push(thread::spawn(move || {
                Self::vector_finalize_worker_loop(worker_idx, finalize_graph_store, finalize_rx);
            }));
            let worker_ready_queue = Arc::clone(&ready_queue);
            let refill_graph_store = Arc::clone(&graph_store);
            let refill_prepare_tx = prepare_tx.clone();
            let refill_ready_queue = Arc::clone(&ready_queue);
            vector_refill_workers.push(thread::spawn(move || {
                vector_refill_loop::vector_refill_worker_loop(
                    worker_idx,
                    refill_graph_store,
                    refill_prepare_tx,
                    refill_rx,
                    refill_ready_queue,
                );
            }));
            vector_workers.push(thread::spawn(move || {
                Self::vector_worker_loop(
                    worker_idx,
                    graph_store,
                    refill_tx,
                    persist_tx,
                    finalize_tx,
                    worker_ready_queue,
                );
            }));
        }

        let mut graph_workers = Vec::new();
        for worker_idx in 0..config.graph_workers {
            let graph_store = Arc::clone(&graph_store);
            let queue_store = Arc::clone(&queue_store);
            graph_workers.push(thread::spawn(move || {
                Self::graph_worker_loop(worker_idx, graph_store, queue_store);
            }));
        }
        Self {
            _query_workers: query_workers,
            _vector_prepare_workers: vector_prepare_workers,
            _vector_refill_workers: vector_refill_workers,
            _vector_workers: vector_workers,
            _vector_persist_workers: vector_persist_workers,
            _vector_finalize_workers: vector_finalize_workers,
            _vector_maintenance_workers: vector_maintenance_workers,
            _graph_workers: graph_workers,
        }
    }

    fn vector_maintenance_worker_loop(graph_store: Arc<GraphStore>) {
        info!("Semantic Vector Maintenance Worker: stale inflight recovery enabled");
        let claimable_supply_poll_interval =
            Duration::from_millis(vector_claimable_supply_poll_interval_ms());
        let stale_recovery_interval =
            Duration::from_millis(vector_stale_inflight_recovery_interval_ms());
        let mut last_claimable_supply_maintenance = Instant::now()
            .checked_sub(claimable_supply_poll_interval)
            .unwrap_or_else(Instant::now);
        let mut last_stale_recovery = Instant::now()
            .checked_sub(stale_recovery_interval)
            .unwrap_or_else(Instant::now);
        loop {
            let mut woke = false;
            let now = Instant::now();

            if now.duration_since(last_claimable_supply_maintenance)
                >= claimable_supply_poll_interval
            {
                last_claimable_supply_maintenance = now;
                match maintain_vector_claimable_supply(&graph_store) {
                    Ok(promoted) if promoted > 0 => {
                        woke = true;
                        info!(
                            "Semantic Vector Maintenance Worker: promoted {} graph-ready files into claimable vector supply",
                            promoted
                        );
                    }
                    Ok(_) => {}
                    Err(err) => error!(
                        "Semantic Vector Maintenance Worker: failed to maintain claimable vector supply: {:?}",
                        err
                    ),
                }
            }

            if now.duration_since(last_stale_recovery) >= stale_recovery_interval {
                last_stale_recovery = now;
                let now_ms = chrono::Utc::now().timestamp_millis();
                match recover_stale_vector_inflight_now(&graph_store, now_ms) {
                    Ok(recovered) if recovered > 0 => {
                        woke = true;
                        info!(
                            "Semantic Vector Maintenance Worker: recovered {} stale inflight vectorization jobs",
                            recovered
                        )
                    }
                    Ok(_) => {}
                    Err(err) => error!(
                        "Semantic Vector Maintenance Worker: failed to recover stale inflight vectorization jobs: {:?}",
                        err
                    ),
                }
                match recover_stale_vector_outbox_now(&graph_store, now_ms) {
                    Ok(recovered) if recovered > 0 => {
                        woke = true;
                        info!(
                            "Semantic Vector Maintenance Worker: recovered {} stale inflight outbox jobs",
                            recovered
                        )
                    }
                    Ok(_) => {}
                    Err(err) => error!(
                        "Semantic Vector Maintenance Worker: failed to recover stale inflight outbox jobs: {:?}",
                        err
                    ),
                }
            }
            if woke {
                service_guard::record_runtime_wakeup(
                    service_guard::RuntimeWakeSource::SemanticVector,
                    0,
                    0,
                );
            }
            let next_claimable_due = claimable_supply_poll_interval
                .saturating_sub(last_claimable_supply_maintenance.elapsed());
            let next_recovery_due =
                stale_recovery_interval.saturating_sub(last_stale_recovery.elapsed());
            thread::sleep(
                next_claimable_due
                    .min(next_recovery_due)
                    .max(Duration::from_millis(25)),
            );
        }
    }

    pub(super) fn query_worker_loop(worker_idx: usize, query_rx: Receiver<QueryEmbeddingRequest>) {
        info!(
            "Semantic Query Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
            worker_idx
        );

        let Some(mut model) = Self::build_text_embedding_model("query", worker_idx) else {
            // REQ-AXO-098 — model load failed. Flip the embedder
            // subsystem to Failed so the readiness contract reflects
            // the broken state instead of letting it remain whatever
            // initial value the registry held.
            crate::runtime_readiness::report_subsystem_state(
                crate::runtime_readiness::Subsystem::Embedder,
                crate::runtime_readiness::SubsystemState::Failed {
                    reason: "model_load_failed".to_string(),
                },
            );
            return;
        };

        // REQ-AXO-098 — the model loaded; the embedder subsystem is
        // now Ready. Subsequent failures (per-request errors, GPU
        // pressure pauses) are transient and surfaced through
        // batch_embed's existing error path, not through the
        // subsystem state.
        crate::runtime_readiness::report_subsystem_state(
            crate::runtime_readiness::Subsystem::Embedder,
            crate::runtime_readiness::SubsystemState::Ready,
        );

        loop {
            match query_rx.recv() {
                Ok(request) => serve_query_embedding_request(&mut model, request),
                Err(_) => return,
            }
        }
    }


    fn vector_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        refill_tx: Sender<VectorRefillCommand>,
        persist_tx: Sender<VectorPersistRequest>,
        finalize_tx: Sender<VectorFinalizeRequest>,
        ready_batches: Arc<SharedPreparedBatchQueue>,
    ) {
        let _liveness = VectorWorkerLivenessGuard::new();
        let mut restart_window: VecDeque<i64> = VecDeque::new();
        let mut restart_attempt = 0_u64;
        let mut gpu_recycle_candidate_batches = 0_u32;
        let mut recycle_coordinator = VramRecycleCoordinator::new();
        let mut wake_idle = true;

        if let Err(e) = graph_store.ensure_embedding_model(
            SYMBOL_MODEL_ID,
            "symbol",
            MODEL_NAME,
            DIMENSION as i64,
            MODEL_VERSION,
        ) {
            error!(
                "Semantic Worker: failed to register symbol embedding model: {:?}",
                e
            );
        }
        if let Err(e) = graph_store.ensure_embedding_model(
            CHUNK_MODEL_ID,
            "chunk",
            MODEL_NAME,
            DIMENSION as i64,
            MODEL_VERSION,
        ) {
            error!(
                "Semantic Worker: failed to register chunk embedding model: {:?}",
                e
            );
        }

        info!(
            "Semantic Vector Worker [{}]: Hunting for unembedded symbols and file chunks...",
            worker_idx
        );

        'worker_lifecycle: loop {
            persist_vector_lane_state(
                &graph_store,
                VectorLaneState::Starting,
                worker_idx,
                restart_attempt,
                Some("model_init".to_string()),
                None,
            );
            info!(
                "Semantic Vector Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
                worker_idx
            );
            let Some(mut model) = Self::build_vector_embedding_model(worker_idx) else {
                let init_reason = std::env::var("AXON_EMBEDDING_PROVIDER_INIT_ERROR")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "failed to initialize embedding model".to_string());
                schedule_vector_worker_restart(
                    &graph_store,
                    worker_idx,
                    FatalVectorWorkerFault {
                        stage: "model_init",
                        reason_raw: init_reason,
                        fatal_class: "model_init".to_string(),
                        batch_id: None,
                        texts_count: 0,
                        input_bytes: 0,
                    },
                    &mut restart_window,
                    &mut restart_attempt,
                );
                continue;
            };
            persist_vector_lane_state(
                &graph_store,
                VectorLaneState::Healthy,
                worker_idx,
                restart_attempt,
                Some("model_loaded".to_string()),
                None,
            );
            service_guard::record_vector_worker_heartbeat();
            let current_pressure = service_guard::current_pressure();
            let claimable_file_backlog_depth = graph_store
                .fetch_claimable_file_vectorization_queue_count()
                .unwrap_or(0);
            let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = graph_store
                .fetch_file_vectorization_queue_counts()
                .unwrap_or((0, 0));
            let (outbox_queued, outbox_inflight) = graph_store
                .fetch_vector_persist_outbox_counts()
                .unwrap_or((0, 0));
            let aggregate_vector_backlog_depth = file_vectorization_queue_queued
                + file_vectorization_queue_inflight
                + outbox_queued
                + outbox_inflight;
            let (graph_projection_queue_queued, graph_projection_queue_inflight) = graph_store
                .fetch_graph_projection_queue_counts()
                .unwrap_or((0, 0));
            let graph_backlog_depth =
                graph_projection_queue_queued + graph_projection_queue_inflight;
            let effective_graph_backlog_depth = effective_vector_lane_graph_backlog_depth(
                embedding_lane_config_from_env(),
                graph_backlog_depth,
            );
            let gpu_available = effective_embedding_provider_is_gpu();
            if gpu_available
                && !gpu_secondary_worker_allowed(worker_idx, current_gpu_memory_snapshot())
            {
                service_guard::record_vector_worker_admission_reason(
                    "gpu_secondary_worker_vram_guard",
                    1,
                );
                service_guard::record_vectorization_suppressed();
                thread::sleep(Duration::from_millis(
                    vector_worker_non_admitted_backlog_wait_ms(aggregate_vector_backlog_depth),
                ));
                continue;
            }
            let admission = vector_worker_admission_decision(
                worker_idx,
                current_pressure,
                gpu_available,
                claimable_file_backlog_depth,
            );
            service_guard::record_vector_worker_admission_reason(
                admission.reason,
                admission.allowed_gpu_workers,
            );
            if !admission.admitted {
                if claimable_file_backlog_depth == 0 {
                    let signaled = wait_for_vector_backlog_or_timeout(Duration::from_millis(
                        vector_worker_non_admitted_idle_wait_ms(claimable_file_backlog_depth),
                    ));
                    if signaled {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            graph_backlog_depth as u64,
                            aggregate_vector_backlog_depth as u64,
                        );
                    }
                } else {
                    thread::sleep(Duration::from_millis(
                        vector_worker_non_admitted_backlog_wait_ms(claimable_file_backlog_depth),
                    ));
                }
                continue;
            }
            let policy = semantic_policy_with_graph(
                claimable_file_backlog_depth,
                effective_graph_backlog_depth,
                current_pressure,
            );
            if policy.pause {
                if claimable_file_backlog_depth == 0 {
                    let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                    if signaled {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            graph_backlog_depth as u64,
                            aggregate_vector_backlog_depth as u64,
                        );
                    }
                } else {
                    thread::sleep(policy.sleep);
                }
                continue;
            }

            let mut backlog_active = ready_batches.len() > 0 || claimable_file_backlog_depth > 0;
            let mut completed_works: Vec<FileVectorizationWork> = Vec::new();
            let mut completed_batch_runs: Vec<VectorBatchRun> = Vec::new();
            let mut failed: HashMap<String, Vec<FileVectorizationWork>> = HashMap::new();
            let mut inflight_persists: VecDeque<InflightPersistRequest> = VecDeque::new();
            let max_inflight_persists = configured_vector_max_inflight_persists();

            while gpu_worker_has_pending_work(
                ready_batches.len(),
                inflight_persists.len(),
                service_guard::vector_runtime_metrics().prepare_inflight_current,
                claimable_file_backlog_depth,
            ) {
                while let Some(inflight) = inflight_persists.front() {
                    match inflight.reply_rx.try_recv() {
                        Ok(outcome) => {
                            inflight_persists.pop_front();
                            let requeue = apply_vector_persist_outcome(
                                outcome,
                                ready_batches.as_ref(),
                                &mut completed_works,
                                &mut completed_batch_runs,
                                &mut failed,
                            );
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                requeue,
                            );
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            inflight_persists.pop_front();
                            error!(
                                "Semantic Vector Worker [{}]: persist reply disconnected before completion",
                                worker_idx
                            );
                        }
                    }
                }

                // --- Unified VRAM recycle coordinator (pre-batch collection point) ---
                let recycle_metrics = service_guard::vector_runtime_metrics();
                let recycle_snapshot = current_gpu_memory_snapshot();
                let recycle_chunks_per_second =
                    service_guard::vector_chunk_embeddings_per_second();
                let recycle_vram_used_mb = recycle_snapshot
                    .map(|s| s.used_mb)
                    .unwrap_or(0);
                let recycle_signals = RecycleSignals {
                    stuck: gpu_stuck_recovery_reason(recycle_metrics, inflight_persists.len())
                        .is_some(),
                    summit: gpu_recycle_immediate_required(
                        recycle_snapshot,
                        inflight_persists.len(),
                    ),
                    pre_batch_plateau: gpu_pre_batch_vram_recycle_reason(recycle_snapshot)
                        .is_some(),
                    low_throughput: recycle_snapshot
                        .map(|s| s.used_mb >= gpu_recycle_vram_summit_mb())
                        .unwrap_or(false)
                        && recycle_chunks_per_second <= 8.0,
                    vram_critical: recycle_snapshot
                        .map(|s| {
                            s.total_mb > 0
                                && s.used_mb > s.total_mb * RECYCLE_VRAM_CRITICAL_PCT / 100
                        })
                        .unwrap_or(false),
                    vram_used_mb: recycle_vram_used_mb,
                };
                if let Some(reason) = recycle_coordinator.should_recycle(&recycle_signals) {
                    warn!(
                        "Semantic Vector Worker [{}]: unified recycle: {}",
                        worker_idx, reason
                    );
                    schedule_vector_worker_restart(
                        &graph_store,
                        worker_idx,
                        FatalVectorWorkerFault {
                            stage: "unified_recycle",
                            reason_raw: reason,
                            fatal_class: "gpu_recycle".to_string(),
                            batch_id: None,
                            texts_count: recycle_metrics.embed_inflight_texts_current,
                            input_bytes: recycle_metrics.embed_inflight_text_bytes_current,
                        },
                        &mut restart_window,
                        &mut restart_attempt,
                    );
                    drop(model);
                    continue 'worker_lifecycle;
                }

                if !completed_works.is_empty() {
                    let _ =
                        graph_store.refresh_inflight_file_vectorization_claims(&completed_works);
                }

                let ready_queue_depth_at_gpu_start = ready_batches.len() as u64;
                let ready_queue_chunks_at_gpu_start = ready_batches.summary().chunk_count as u64;
                let prepare_inflight_at_gpu_start =
                    service_guard::vector_runtime_metrics().prepare_inflight_current;
                let prepare_inflight_chunks_at_gpu_start =
                    service_guard::vector_runtime_metrics().prepare_inflight_chunks_current;
                let Some(prepared) = ready_batches.pop_best() else {
                    if gpu_worker_should_wait_for_ready(
                        ready_batches.len(),
                        inflight_persists.len(),
                        service_guard::vector_runtime_metrics().prepare_inflight_current,
                        claimable_file_backlog_depth,
                    ) {
                        let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(1));
                        backlog_active = true;
                        continue;
                    }
                    if let Some(inflight) = inflight_persists.pop_front() {
                        if let Some(outcome) = wait_for_vector_persist_outcome(
                            &graph_store,
                            worker_idx,
                            inflight.reply_rx,
                            inflight.claimed.as_slice(),
                        ) {
                            let persist_succeeded = outcome.error_reason.is_none();
                            let requeue = apply_vector_persist_outcome(
                                outcome,
                                ready_batches.as_ref(),
                                &mut completed_works,
                                &mut completed_batch_runs,
                                &mut failed,
                            );
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                requeue,
                            );
                            // VRAM summit observe: feed signal to coordinator
                            // (actual recycle decision deferred to top of loop).
                            if persist_succeeded {
                                let vram_used_mb = current_gpu_memory_snapshot()
                                    .map(|snapshot| snapshot.used_mb)
                                    .unwrap_or(0);
                                let chunks_per_second =
                                    service_guard::vector_chunk_embeddings_per_second();
                                gpu_recycle_after_vram_summit_observe(
                                    vram_used_mb,
                                    chunks_per_second,
                                    &mut gpu_recycle_candidate_batches,
                                );
                            } else {
                                gpu_recycle_candidate_batches = 0;
                            }
                        }
                        flush_completed_vectorization_works(
                            worker_idx,
                            &graph_store,
                            &finalize_tx,
                            &mut completed_works,
                            &mut completed_batch_runs,
                            &mut failed,
                        );
                        let ready_summary = ready_batches.summary();
                        record_ready_queue_summary(&ready_summary);
                        continue;
                    }
                    break;
                };
                backlog_active = true;
                let consumed_chunk_count = prepared.chunk_count().max(1);
                service_guard::notify_vector_backlog_activity();
                if service_guard::interactive_priority_active() {
                    let interrupted_batches = ready_batches.retain(|batch| {
                        !batch
                            .touched_works
                            .iter()
                            .any(|work| pause_vectorization_work_if_interactive(&graph_store, work))
                    });
                    if !interrupted_batches.is_empty() {
                        let ready_summary = ready_batches.summary();
                        record_ready_queue_summary(&ready_summary);
                    }
                }
                // Pre-batch VRAM guard: the coordinator at the top of the loop
                // already evaluates this signal.  We keep the admission-level
                // check for non-recycle VRAM gating (worker pausing).
                let gpu_memory_snapshot = current_gpu_memory_snapshot();
                if !gpu_worker_consumption_allowed(gpu_available, gpu_memory_snapshot) {
                    service_guard::record_vector_worker_admission_reason(
                        "gpu_primary_worker_vram_guard",
                        (service_guard::current_allowed_gpu_workers().max(1))
                            .try_into()
                            .unwrap_or(usize::MAX),
                    );
                    let ready_depth = ready_batches.push_front(prepared);
                    let _ = ready_depth;
                    record_ready_queue_summary(&ready_batches.summary());
                    thread::sleep(Duration::from_millis(
                        vector_worker_non_admitted_backlog_wait_ms(aggregate_vector_backlog_depth),
                    ));
                    continue;
                }
                let ready_summary = ready_batches.summary();
                record_ready_queue_summary(&ready_summary);
                service_guard::record_vector_last_consumed_batch_lane(service_guard_batch_lane(
                    prepared.batch_lane(),
                ));
                if let Err(err) = refill_tx.send(VectorRefillCommand::BatchConsumed(
                    consumed_chunk_count as usize,
                )) {
                    error!(
                        "Semantic Vector Worker [{}]: failed to publish consumed batch event to refill worker: {}",
                        worker_idx, err
                    );
                    service_guard::record_vector_ready_replenishment_requested(
                        consumed_chunk_count,
                    );
                }

                for (work, reason) in &prepared.failed_fetches {
                    error!(
                        "Semantic Vector Worker [{}]: failed to fetch unembedded chunks for {}: {}",
                        worker_idx, work.file_path, reason
                    );
                    failed.entry(reason.clone()).or_default().push(work.clone());
                }
                for work in &prepared.oversized_works {
                    if let Err(err) =
                        graph_store.mark_file_oversized_for_current_budget(&work.file_path)
                    {
                        error!(
                            "Semantic Vector Worker [{}]: failed to mark oversized file {}: {:?}",
                            worker_idx, work.file_path, err
                        );
                        failed
                            .entry("failed to mark oversized_for_current_budget".to_string())
                            .or_default()
                            .push(work.clone());
                    } else {
                        info!(
                                    "Semantic Vector Worker [{}]: marked oversized file for current budget: {}",
                                    worker_idx, work.file_path
                                );
                    }
                }
                service_guard::record_vector_partial_file_cycles(
                    prepared.partial_file_cycles as u64,
                );
                if prepared.work_items.is_empty() {
                    completed_works.extend(prepared.immediate_completed.clone());
                    completed_works.extend(prepared.finalize_after_success.clone());
                    flush_completed_vectorization_works(
                        worker_idx,
                        &graph_store,
                        &finalize_tx,
                        &mut completed_works,
                        &mut completed_batch_runs,
                        &mut failed,
                    );
                    continue;
                }
                let embed_input_texts = prepared.texts.len() as u64;
                let embed_input_text_bytes = prepared
                    .texts
                    .iter()
                    .map(|text| text.len() as u64)
                    .sum::<u64>();
                let total_tokens = prepared.total_token_count();
                let max_item_tokens = prepared.max_item_tokens();
                let avg_item_tokens = prepared.avg_item_tokens();
                let micro_batch_count = prepared.micro_batch_count();
                let max_micro_batch_tokens = prepared.max_micro_batch_tokens();
                let avg_micro_batch_tokens = prepared.avg_micro_batch_tokens();
                let effective_vector_workers_admitted =
                    service_guard::vector_runtime_metrics().vector_workers_active_current;
                let vector_worker_admission_reason =
                    service_guard::current_vector_worker_admission_reason();
                let allowed_gpu_workers = service_guard::current_allowed_gpu_workers();
                let batch_started_at_ms = chrono::Utc::now().timestamp_millis();
                let last_gpu_finished_at_ms = service_guard::current_last_embed_finished_at_ms();
                let batch_wait_for_ready_ms = if last_gpu_finished_at_ms > 0
                    && (batch_started_at_ms as u64) >= last_gpu_finished_at_ms
                {
                    (batch_started_at_ms as u64).saturating_sub(last_gpu_finished_at_ms)
                } else {
                    0
                };
                if let Err(err) =
                    graph_store.refresh_inflight_file_vectorization_claims(&prepared.touched_works)
                {
                    warn!(
                                "Semantic Vector Worker [{}]: failed to refresh inflight vectorization claims before embed: {:?}",
                                worker_idx, err
                            );
                }
                service_guard::record_vector_embed_attempt(
                    embed_input_texts,
                    embed_input_text_bytes,
                );
                let embed_owned_workset = merge_unique_vectorization_work_sets([
                    ready_batches.touched_works_snapshot(),
                    inflight_persists
                        .iter()
                        .flat_map(|request| request.claimed.clone_works())
                        .collect::<Vec<_>>(),
                    prepared.touched_works.clone(),
                    completed_works.clone(),
                ]);
                let _embed_lease_guard = LeaseRefreshGuard::start(
                    Arc::clone(&graph_store),
                    embed_owned_workset,
                    "vector",
                );
                let gpu_started_at_ms = chrono::Utc::now().timestamp_millis();
                let embed_started = Instant::now();
                let _ = graph_store.mark_file_vectorization_started(&prepared.touched_works);
                let mut recreate_gpu_session_after_batch = false;
                match model.embed_prepared_batch_with_breakdown(&prepared) {
                    Ok((
                        embeddings,
                        host_prepare_ms,
                        input_copy_ms,
                        inference_ms,
                        output_extract_ms,
                    )) => {
                        service_guard::record_vector_embed_inputs(embeddings.len() as u64, 0, 0);
                        service_guard::record_vector_embed_attempt_finished();
                        service_guard::record_vector_lane_success();
                        service_guard::record_vector_embed_breakdown(
                            inference_ms,
                            output_extract_ms,
                        );
                        service_guard::record_vector_embed_inputs(
                            embed_input_texts,
                            embed_input_text_bytes,
                            host_prepare_ms.saturating_add(input_copy_ms),
                        );
                        service_guard::record_vector_stage_ms(
                            service_guard::VectorStageKind::Embed,
                            embed_started.elapsed().as_millis() as u64,
                        );
                        let touched_works = prepared.touched_works.clone();
                        let next_active_after_failure = prepared.next_active_after_failure.clone();
                        let prepared_fetch_ms_total = prepared.fetch_ms_total;
                        let prepared_batch_id = prepared.batch_id.clone();
                        let prepare_started_at_ms = prepared.prepare_started_at_ms;
                        let prepare_finished_at_ms = prepared.prepare_finished_at_ms;
                        let ready_enqueued_at_ms = prepared.prepared_at_ms;
                        let prepared_batch_lane = prepared.batch_lane();
                        let prepared_mixed_fallback = prepared.mixed_fallback();
                        let prepared_lane_thresholds = prepared.lane_thresholds();
                        let persist_plan = match prepared.into_persist_envelope(
                            embeddings,
                            VectorBatchRun {
                                run_id: prepared_batch_id,
                                prepare_started_at_ms,
                                prepare_finished_at_ms,
                                ready_enqueued_at_ms,
                                started_at_ms: batch_started_at_ms,
                                finished_at_ms: chrono::Utc::now().timestamp_millis(),
                                gpu_started_at_ms,
                                gpu_finished_at_ms: chrono::Utc::now().timestamp_millis(),
                                persist_enqueued_at_ms: 0,
                                persist_started_at_ms: 0,
                                persist_finished_at_ms: 0,
                                finalize_enqueued_at_ms: 0,
                                finalize_finished_at_ms: 0,
                                provider: current_embedding_provider_diagnostics()
                                    .provider_effective,
                                runner_kind: "ort_gpu_first_iobinding".to_string(),
                                model_id: CHUNK_MODEL_ID.to_string(),
                                chunk_count: 0,
                                file_count: 0,
                                input_bytes: embed_input_text_bytes,
                                total_tokens,
                                max_item_tokens,
                                avg_item_tokens,
                                micro_batch_count,
                                max_micro_batch_tokens,
                                avg_micro_batch_tokens,
                                effective_vector_workers_admitted,
                                ready_queue_depth_at_gpu_start,
                                prepare_inflight_at_gpu_start,
                                ready_queue_chunks_at_gpu_start,
                                prepare_inflight_chunks_at_gpu_start,
                                vector_worker_admission_reason,
                                allowed_gpu_workers,
                                batch_wait_for_ready_ms,
                                persist_queue_wait_ms: 0,
                                finalize_queue_wait_ms: 0,
                                batch_lane: prepared_batch_lane.as_str().to_string(),
                                batch_shape: if prepared_mixed_fallback {
                                    "mixed_fallback".to_string()
                                } else {
                                    "homogeneous".to_string()
                                },
                                lane_small_max_tokens: prepared_lane_thresholds.small_max_tokens
                                    as u64,
                                lane_medium_max_tokens: prepared_lane_thresholds.medium_max_tokens
                                    as u64,
                                fetch_ms: prepared_fetch_ms_total
                                    .saturating_add(host_prepare_ms)
                                    .saturating_add(input_copy_ms),
                                embed_ms: embed_started.elapsed().as_millis() as u64,
                                db_write_ms: 0,
                                mark_done_ms: 0,
                                success: true,
                                error_reason: None,
                            },
                        ) {
                            Ok(envelope) => envelope,
                            Err(err) => {
                                let reason =
                                    format!("failed to build vector persist plan: {:?}", err);
                                failed.entry(reason).or_default().extend(touched_works);
                                let recovered_ready_works =
                                    recover_ready_batches_to_active_works(&ready_batches);
                                let merge_target = next_active_after_failure
                                    .len()
                                    .saturating_add(recovered_ready_works.len())
                                    .max(1);
                                send_vector_refill_requeue(
                                    &graph_store,
                                    worker_idx,
                                    &refill_tx,
                                    merge_vectorization_work(
                                        next_active_after_failure,
                                        recovered_ready_works,
                                        merge_target,
                                    ),
                                );
                                continue;
                            }
                        };
                        let mut persist_envelope = persist_plan;
                        persist_envelope.sync_batch_run_counts_from_plan();
                        while inflight_persists.len() >= max_inflight_persists {
                            let Some(inflight) = inflight_persists.pop_front() else {
                                break;
                            };
                            if let Some(outcome) = wait_for_vector_persist_outcome(
                                &graph_store,
                                worker_idx,
                                inflight.reply_rx,
                                inflight.claimed.as_slice(),
                            ) {
                                let persist_succeeded = outcome.error_reason.is_none();
                                let requeue = apply_vector_persist_outcome(
                                    outcome,
                                    ready_batches.as_ref(),
                                    &mut completed_works,
                                    &mut completed_batch_runs,
                                    &mut failed,
                                );
                                send_vector_refill_requeue(
                                    &graph_store,
                                    worker_idx,
                                    &refill_tx,
                                    requeue,
                                );
                                // VRAM summit observe: feed signal to coordinator
                                // (actual recycle decision deferred to top of loop).
                                if persist_succeeded {
                                    let vram_used_mb = current_gpu_memory_snapshot()
                                        .map(|snapshot| snapshot.used_mb)
                                        .unwrap_or(0);
                                    let chunks_per_second =
                                        service_guard::vector_chunk_embeddings_per_second();
                                    gpu_recycle_after_vram_summit_observe(
                                        vram_used_mb,
                                        chunks_per_second,
                                        &mut gpu_recycle_candidate_batches,
                                    );
                                } else {
                                    gpu_recycle_candidate_batches = 0;
                                }
                            }
                        }
                        if let Err(err) = graph_store.mark_file_vectorization_persist_started(&touched_works) {
                            warn!(
                                "Semantic Vector Worker [{}]: failed to mark persist_started_at_ms: {:?}",
                                worker_idx, err
                            );
                        }
                        match dispatch_vector_persist_plan(&persist_tx, persist_envelope) {
                            Ok(reply_rx) => {
                                inflight_persists.push_back(InflightPersistRequest {
                                    reply_rx,
                                    claimed: ClaimedLeaseSet::new(touched_works),
                                });
                            }
                            Err(err) => {
                                let reason =
                                    format!("failed to dispatch vector persist plan: {:?}", err);
                                failed.entry(reason).or_default().extend(touched_works);
                                let recovered_ready_works =
                                    recover_ready_batches_to_active_works(&ready_batches);
                                let merge_target = next_active_after_failure
                                    .len()
                                    .saturating_add(recovered_ready_works.len())
                                    .max(1);
                                send_vector_refill_requeue(
                                    &graph_store,
                                    worker_idx,
                                    &refill_tx,
                                    merge_vectorization_work(
                                        next_active_after_failure,
                                        recovered_ready_works,
                                        merge_target,
                                    ),
                                );
                            }
                        }
                        let ready_queue_summary = ready_batches.summary();
                        record_vector_pipeline_snapshot(
                            &graph_store,
                            claimable_file_backlog_depth,
                            &[],
                            &VecDeque::new(),
                            &ready_queue_summary,
                            &inflight_persists,
                            ready_queue_summary.chunk_count,
                            0,
                        );
                        flush_completed_vectorization_works(
                            worker_idx,
                            &graph_store,
                            &finalize_tx,
                            &mut completed_works,
                            &mut completed_batch_runs,
                            &mut failed,
                        );
                        recreate_gpu_session_after_batch =
                            gpu_recreate_session_every_batch_enabled();
                    }
                    Err(e) => {
                        service_guard::record_vector_embed_attempt_finished();
                        let reason = format!("chunk embedding failed: {:?}", e);
                        if is_gpu_recycle_immediate_error(&e) {
                            let recovered_ready_works =
                                recover_ready_batches_to_active_works(&ready_batches);
                            let merge_target = prepared
                                .touched_works
                                .len()
                                .saturating_add(prepared.next_active_after_failure.len())
                                .saturating_add(recovered_ready_works.len())
                                .max(1);
                            send_vector_refill_requeue(
                                &graph_store,
                                worker_idx,
                                &refill_tx,
                                merge_vectorization_work(
                                    prepared.touched_works.clone(),
                                    merge_vectorization_work(
                                        prepared.next_active_after_failure.clone(),
                                        recovered_ready_works,
                                        merge_target,
                                    ),
                                    merge_target,
                                ),
                            );
                            drop(model);
                            schedule_vector_worker_restart(
                                &graph_store,
                                worker_idx,
                                FatalVectorWorkerFault {
                                    stage: "gpu_recycle_immediate",
                                    reason_raw: reason,
                                    fatal_class: "gpu_recycle".to_string(),
                                    batch_id: Some(prepared.batch_id.clone()),
                                    texts_count: embed_input_texts,
                                    input_bytes: embed_input_text_bytes,
                                },
                                &mut restart_window,
                                &mut restart_attempt,
                            );
                            continue 'worker_lifecycle;
                        }
                        if let Some(fatal_class) = fatal_embedding_error_class(&e) {
                            error!(
                                        "Semantic Vector Worker [{}]: fatal chunk embedding error, restarting semantic lane: {:?}",
                                        worker_idx, e
                                    );
                            drop(model);
                            schedule_vector_worker_restart(
                                &graph_store,
                                worker_idx,
                                FatalVectorWorkerFault {
                                    stage: "embed",
                                    reason_raw: reason,
                                    fatal_class: fatal_class.to_string(),
                                    batch_id: Some(prepared.batch_id.clone()),
                                    texts_count: embed_input_texts,
                                    input_bytes: embed_input_text_bytes,
                                },
                                &mut restart_window,
                                &mut restart_attempt,
                            );
                            continue 'worker_lifecycle;
                        }
                        error!(
                            "Semantic Vector Worker [{}]: Chunk embedding failed: {:?}",
                            worker_idx, e
                        );
                        let recovered_ready_works =
                            recover_ready_batches_to_active_works(&ready_batches);
                        failed
                            .entry(reason)
                            .or_default()
                            .extend(prepared.touched_works.iter().cloned());
                        let merge_target = prepared
                            .next_active_after_failure
                            .len()
                            .saturating_add(recovered_ready_works.len())
                            .max(1);
                        send_vector_refill_requeue(
                            &graph_store,
                            worker_idx,
                            &refill_tx,
                            merge_vectorization_work(
                                prepared.next_active_after_failure.clone(),
                                recovered_ready_works,
                                merge_target,
                            ),
                        );
                    }
                }
                while let Some(inflight) = inflight_persists.pop_front() {
                    if let Some(outcome) = wait_for_vector_persist_outcome(
                        &graph_store,
                        worker_idx,
                        inflight.reply_rx,
                        inflight.claimed.as_slice(),
                    ) {
                        let persist_succeeded = outcome.error_reason.is_none();
                        let requeue = apply_vector_persist_outcome(
                            outcome,
                            ready_batches.as_ref(),
                            &mut completed_works,
                            &mut completed_batch_runs,
                            &mut failed,
                        );
                        send_vector_refill_requeue(&graph_store, worker_idx, &refill_tx, requeue);
                        // VRAM summit observe: feed signal to coordinator
                        // (actual recycle decision deferred to top of loop).
                        if persist_succeeded {
                            let vram_used_mb = current_gpu_memory_snapshot()
                                .map(|snapshot| snapshot.used_mb)
                                .unwrap_or(0);
                            let chunks_per_second =
                                service_guard::vector_chunk_embeddings_per_second();
                            gpu_recycle_after_vram_summit_observe(
                                vram_used_mb,
                                chunks_per_second,
                                &mut gpu_recycle_candidate_batches,
                            );
                        } else {
                            gpu_recycle_candidate_batches = 0;
                        }
                    }
                }

                flush_completed_vectorization_works(
                    worker_idx,
                    &graph_store,
                    &finalize_tx,
                    &mut completed_works,
                    &mut completed_batch_runs,
                    &mut failed,
                );
                if recreate_gpu_session_after_batch {
                    info!(
                        "Semantic Vector Worker [{}]: recreating GPU session after completed batch for diagnostic VRAM control",
                        worker_idx
                    );
                    drop(model);
                    continue 'worker_lifecycle;
                }

                for (reason, works) in std::mem::take(&mut failed) {
                    if let Err(err) =
                        graph_store.mark_file_vectorization_work_failed(&works, &reason)
                    {
                        error!(
                                "Semantic Vector Worker [{}]: failed to persist file vector backlog failure [{}]: {:?}",
                                worker_idx, reason, err
                            );
                    }
                }
            }

            if !symbol_embedding_allowed(aggregate_vector_backlog_depth, current_pressure) {
                if !backlog_active {
                    if claimable_file_backlog_depth == 0 {
                        wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                    } else {
                        thread::sleep(policy.idle_sleep);
                    }
                }
                continue;
            }

            match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
                Ok(symbols) if !symbols.is_empty() => {
                    backlog_active = true;
                    if wake_idle {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            graph_backlog_depth as u64,
                            aggregate_vector_backlog_depth as u64,
                        );
                        wake_idle = false;
                    }
                    debug!(
                        "Semantic Vector Worker [{}]: Embedding {} symbols...",
                        worker_idx,
                        symbols.len()
                    );

                    let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                    match model.embed_texts_with_breakdown(&texts) {
                        Ok((
                            embeddings,
                            host_prepare_ms,
                            input_copy_ms,
                            inference_ms,
                            output_extract_ms,
                        )) => {
                            service_guard::record_vector_embed_breakdown(
                                host_prepare_ms
                                    .saturating_add(input_copy_ms)
                                    .saturating_add(inference_ms),
                                output_extract_ms,
                            );
                            let updates: Vec<(String, Vec<f32>)> = symbols
                                .into_iter()
                                .zip(embeddings)
                                .map(|((id, _), emb)| (id, emb))
                                .collect();

                            if let Err(e) = graph_store.update_symbol_embeddings(&updates) {
                                error!(
                                    "Semantic Vector Worker [{}]: symbol DB write error: {:?}",
                                    worker_idx, e
                                );
                            }
                        }
                        Err(e) => {
                            if is_gpu_recycle_immediate_error(&e) {
                                drop(model);
                                schedule_vector_worker_restart(
                                    &graph_store,
                                    worker_idx,
                                    FatalVectorWorkerFault {
                                        stage: "gpu_recycle_immediate",
                                        reason_raw: format!(
                                            "symbol gpu recycle immediate: {:?}",
                                            e
                                        ),
                                        fatal_class: "gpu_recycle".to_string(),
                                        batch_id: None,
                                        texts_count: texts.len() as u64,
                                        input_bytes: texts
                                            .iter()
                                            .map(|text| text.len() as u64)
                                            .sum(),
                                    },
                                    &mut restart_window,
                                    &mut restart_attempt,
                                );
                                continue 'worker_lifecycle;
                            }
                            if let Some(fatal_class) = fatal_embedding_error_class(&e) {
                                error!(
                                    "Semantic Vector Worker [{}]: fatal symbol embedding error, restarting semantic lane: {:?}",
                                    worker_idx, e
                                );
                                drop(model);
                                schedule_vector_worker_restart(
                                    &graph_store,
                                    worker_idx,
                                    FatalVectorWorkerFault {
                                        stage: "symbol_embed",
                                        reason_raw: format!("symbol embedding failed: {:?}", e),
                                        fatal_class: fatal_class.to_string(),
                                        batch_id: None,
                                        texts_count: texts.len() as u64,
                                        input_bytes: texts
                                            .iter()
                                            .map(|text| text.len() as u64)
                                            .sum(),
                                    },
                                    &mut restart_window,
                                    &mut restart_attempt,
                                );
                                continue 'worker_lifecycle;
                            }
                            error!(
                                "Semantic Vector Worker [{}]: symbol embedding failed: {:?}",
                                worker_idx, e
                            );
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!(
                        "Semantic Vector Worker [{}]: symbol fetch error: {:?}",
                        worker_idx, e
                    );
                    if claimable_file_backlog_depth == 0 {
                        let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                        if signaled {
                            service_guard::record_runtime_wakeup(
                                service_guard::RuntimeWakeSource::SemanticVector,
                                graph_backlog_depth as u64,
                                aggregate_vector_backlog_depth as u64,
                            );
                        }
                    } else {
                        thread::sleep(policy.idle_sleep);
                    }
                }
            }

            if !backlog_active {
                wake_idle = true;
                if claimable_file_backlog_depth == 0 {
                    let signaled = wait_for_vector_backlog_or_timeout(policy.idle_sleep);
                    if signaled {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            graph_backlog_depth as u64,
                            aggregate_vector_backlog_depth as u64,
                        );
                    }
                } else {
                    thread::sleep(policy.idle_sleep);
                }
            }
        }
    }

    fn vector_finalize_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        finalize_rx: Receiver<VectorFinalizeRequest>,
    ) {
        info!(
            "Semantic Vector Finalize Worker [{}]: ready with bounded queue {}",
            worker_idx, VECTOR_FINALIZE_QUEUE_BOUND
        );
        let mut wake_idle = true;
        loop {
            service_guard::record_vector_finalize_queue_depth(finalize_rx.len() as u64);
            match finalize_rx.recv_timeout(Duration::from_millis(
                vector_finalize_idle_poll_interval_ms(),
            )) {
                Ok(request) => {
                    if wake_idle {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            0,
                            1,
                        );
                    }
                    service_guard::record_vector_finalize_queue_wait_ms(
                        request.enqueued_at.elapsed().as_millis() as u64,
                    );
                    process_finalize_request(worker_idx, &graph_store, request);
                    while let Ok(request) = finalize_rx.try_recv() {
                        service_guard::record_vector_finalize_queue_wait_ms(
                            request.enqueued_at.elapsed().as_millis() as u64,
                        );
                        process_finalize_request(worker_idx, &graph_store, request);
                    }
                    while Self::process_vector_persist_outbox(worker_idx, &graph_store) > 0 {}
                    wake_idle = finalize_rx.is_empty();
                }
                Err(RecvTimeoutError::Timeout) => {
                    let mut processed_any = false;
                    while Self::process_vector_persist_outbox(worker_idx, &graph_store) > 0 {
                        processed_any = true;
                    }
                    if processed_any {
                        if wake_idle {
                            service_guard::record_runtime_wakeup(
                                service_guard::RuntimeWakeSource::SemanticVector,
                                0,
                                1,
                            );
                        }
                        wake_idle = false;
                    } else {
                        wake_idle = true;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn vector_prepare_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        prepare_rx: Receiver<VectorPrepareRequest>,
        ready_batches: Arc<SharedPreparedBatchQueue>,
    ) {
        info!(
            "Semantic Vector Prepare Worker [{}]: ready with bounded queue {}",
            worker_idx,
            configured_vector_prepare_queue_bound()
        );
        let mut tokenizer = load_runtime_embedding_tokenizer().ok();
        let mut wake_idle = true;
        while let Ok(request) = prepare_rx.recv() {
            if wake_idle {
                service_guard::record_runtime_wakeup(
                    service_guard::RuntimeWakeSource::SemanticVector,
                    0,
                    1,
                );
                wake_idle = false;
            }
            service_guard::record_vector_prepare_queue_depth(prepare_rx.len() as u64);
            service_guard::record_vector_prepare_queue_wait_ms(
                request.enqueued_at.elapsed().as_millis() as u64,
            );
            let prepare_started_at_ms = chrono::Utc::now().timestamp_millis();
            let mut sequence = build_prepared_vector_embed_sequence(
                &graph_store,
                request.claimed.as_slice(),
                request.target_chunks,
                request.per_file_fetch_limit,
                request.batch_max_bytes,
                request.target_ready_depth,
                prepare_started_at_ms,
            );
            let mut rejected_works = Vec::new();
            for mut prepared in sequence.batches.drain(..) {
                if !prepared.texts.is_empty() {
                    if tokenizer.is_none() {
                        tokenizer = load_runtime_embedding_tokenizer().ok();
                    }
                    if let Some(active_tokenizer) = tokenizer.as_ref() {
                        match attach_preencoded_micro_batches(active_tokenizer, &mut prepared) {
                            Ok(()) => {}
                            Err(err) => {
                                error!(
                                    "Semantic Vector Prepare Worker [{}]: failed to pre-tokenize batch; rejecting it before GPU admission: {:?}",
                                    worker_idx, err
                                );
                                rejected_works.extend(prepared.touched_works.clone());
                                rejected_works.extend(prepared.immediate_completed.clone());
                                continue;
                            }
                        }
                    } else {
                        error!(
                            "Semantic Vector Prepare Worker [{}]: tokenizer unavailable; rejecting non-tokenized batch before GPU admission",
                            worker_idx
                        );
                        rejected_works.extend(prepared.touched_works.clone());
                        rejected_works.extend(prepared.immediate_completed.clone());
                        continue;
                    }
                }
                service_guard::record_vector_active_lane_thresholds(
                    prepared.lane_thresholds.small_max_tokens as u64,
                    prepared.lane_thresholds.medium_max_tokens as u64,
                );
                let split_batches = split_prepared_batch_by_lane(
                    prepared.into_inner(),
                    request.target_chunks,
                    ready_batches.summary().chunk_count,
                );
                let budgeted_batches = split_batches
                    .into_iter()
                    .flat_map(|batch| split_prepared_batch_for_gpu_budget(batch.into_inner()))
                    .collect::<Vec<_>>();
                for batch in &budgeted_batches {
                    service_guard::record_vector_batch_shape(!batch.mixed_fallback());
                    service_guard::record_vector_prepare_outcome(
                        batch.work_items.len() as u64,
                        batch.immediate_completed.len() as u64,
                        batch.failed_fetches.len() as u64,
                    );
                }
                let fulfilled_chunk_count = budgeted_batches
                    .iter()
                    .map(|batch| batch.chunk_count())
                    .sum::<u64>()
                    .max(1);
                let _ready_depth = ready_batches.push_back_many(budgeted_batches);
                service_guard::record_vector_ready_replenishment_fulfilled(fulfilled_chunk_count);
                service_guard::record_vector_prepare_prefetch();
                record_ready_queue_summary(&ready_batches.summary());
            }
            if !rejected_works.is_empty() {
                let remaining = sequence.remaining_claimed_after_success.into_inner();
                sequence.remaining_claimed_after_success =
                    ClaimedLeaseSet::new(merge_vectorization_work(
                        rejected_works,
                        remaining,
                        request.claimed.as_slice().len().max(1),
                    ));
            }
            if request
                .reply
                .send(PreparedVectorPrepareOutcome {
                    remaining_claimed_after_success: sequence.remaining_claimed_after_success,
                })
                .is_err()
            {
                error!(
                    "Semantic Vector Prepare Worker [{}]: embed worker dropped prepare outcome reply channel",
                    worker_idx
                );
            }
            if prepare_rx.is_empty() {
                wake_idle = true;
            }
        }
    }

    fn process_vector_persist_outbox(worker_idx: usize, graph_store: &Arc<GraphStore>) -> usize {
        let pending = match graph_store
            .fetch_pending_vector_persist_outbox_work(configured_vector_outbox_fetch_batch_size())
        {
            Ok(pending) => pending,
            Err(err) => {
                error!(
                    "Semantic Vector Finalize Worker [{}]: failed to fetch outbox work: {:?}",
                    worker_idx, err
                );
                return 0;
            }
        };
        let processed = pending.len();
        for work in pending {
            if let Err(err) = process_vector_persist_outbox_work(worker_idx, graph_store, work) {
                error!(
                    "Semantic Vector Finalize Worker [{}]: outbox work failed: {:?}",
                    worker_idx, err
                );
            }
        }
        processed
    }

    fn vector_persist_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        persist_rx: Receiver<VectorPersistRequest>,
    ) {
        info!(
            "Semantic Vector Persist Worker [{}]: ready with bounded queue {}",
            worker_idx,
            configured_vector_persist_queue_bound()
        );
        let mut wake_idle = true;
        while let Ok(request) = persist_rx.recv() {
            if wake_idle {
                service_guard::record_runtime_wakeup(
                    service_guard::RuntimeWakeSource::SemanticVector,
                    0,
                    1,
                );
                wake_idle = false;
            }
            service_guard::record_vector_persist_queue_depth(persist_rx.len() as u64);
            let persist_queue_wait_ms = request.enqueued_at.elapsed().as_millis() as u64;
            service_guard::record_vector_persist_queue_wait_ms(persist_queue_wait_ms);
            let db_write_started = Instant::now();
            let PersistEnvelope {
                persist_plan,
                mut batch_run,
            } = request.envelope;
            batch_run.persist_started_at_ms = chrono::Utc::now().timestamp_millis();
            batch_run.persist_queue_wait_ms = persist_queue_wait_ms;
            let _persist_lease_guard = LeaseRefreshGuard::start(
                Arc::clone(&graph_store),
                persist_plan.touched_works.clone(),
                "vector",
            );
            let outcome = match persist_vector_embed_batch(&graph_store, &persist_plan) {
                Ok(payload) => {
                    let db_write_ms = db_write_started.elapsed().as_millis() as u64;
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::DbWrite,
                        db_write_ms,
                    );
                    service_guard::record_vector_embed_call(
                        payload.updates.len() as u64,
                        persist_plan.touched_works.len() as u64,
                    );
                    service_guard::notify_vector_backlog_activity();
                    batch_run.db_write_ms = db_write_ms;
                    VectorPersistOutcome {
                        completed_works: Vec::new(),
                        batch_runs: Vec::new(),
                        next_active_after_failure: persist_plan.next_active_after_failure,
                        touched_works: persist_plan.touched_works,
                        error_reason: None,
                    }
                }
                Err(err) => {
                    let db_write_ms = db_write_started.elapsed().as_millis() as u64;
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::DbWrite,
                        db_write_ms,
                    );
                    batch_run.db_write_ms = db_write_ms;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    batch_run.persist_finished_at_ms = batch_run.finished_at_ms;
                    batch_run.success = false;
                    batch_run.error_reason =
                        Some(format!("failed to persist chunk embeddings: {:?}", err));
                    if let Err(batch_err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Persist Worker [{}]: failed to persist failed vector batch run: {:?}",
                            worker_idx, batch_err
                        );
                    }
                    VectorPersistOutcome {
                        completed_works: Vec::new(),
                        batch_runs: Vec::new(),
                        next_active_after_failure: persist_plan.next_active_after_failure,
                        touched_works: persist_plan.touched_works,
                        error_reason: Some(format!(
                            "failed to persist chunk embeddings: {:?}",
                            err
                        )),
                    }
                }
            };
            if request.reply.send(outcome).is_err() {
                error!(
                    "Semantic Vector Persist Worker [{}]: vector worker dropped persist reply channel",
                    worker_idx
                );
            }
            if persist_rx.is_empty() {
                wake_idle = true;
            }
        }
    }

    fn graph_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        queue_store: Arc<QueueStore>,
    ) {
        let _liveness = GraphWorkerLivenessGuard::new();
        if !graph_embeddings_enabled() {
            info!(
                "Semantic Graph Worker [{}]: graph embeddings disabled, worker exiting",
                worker_idx
            );
            return;
        }
        let mut model: Option<TextEmbedding> = None;
        let mut graph_model_registered = false;

        loop {
            service_guard::record_graph_worker_heartbeat();
            let lane_config = embedding_lane_config_from_env();
            let (graph_projection_queue_queued, graph_projection_queue_inflight) = graph_store
                .fetch_graph_projection_queue_counts()
                .unwrap_or((0, 0));
            let graph_backlog_depth =
                graph_projection_queue_queued + graph_projection_queue_inflight;
            let policy = semantic_policy_with_graph(
                queue_store.common_len(),
                graph_backlog_depth,
                service_guard::current_pressure(),
            );
            let service_pressure = service_guard::current_pressure();
            let (vector_queue_queued, vector_queue_inflight) = graph_store
                .fetch_file_vectorization_queue_counts()
                .unwrap_or((0, 0));
            let (outbox_queued, outbox_inflight) = graph_store
                .fetch_vector_persist_outbox_counts()
                .unwrap_or((0, 0));
            let vector_backlog_depth =
                vector_queue_queued + vector_queue_inflight + outbox_queued + outbox_inflight;
            service_guard::record_runtime_wakeup(
                service_guard::RuntimeWakeSource::Graph,
                graph_backlog_depth as u64,
                vector_backlog_depth as u64,
            );
            let gpu_available = effective_embedding_provider_is_gpu();
            if worker_idx >= lane_config.graph_workers {
                thread::sleep(policy.idle_sleep);
                continue;
            }
            if !graph_projection_allowed(
                queue_store.common_len(),
                service_pressure,
                vector_backlog_depth,
                gpu_available,
            ) {
                thread::sleep(policy.sleep);
                continue;
            }

            match graph_store.fetch_pending_graph_projection_work(lane_config.graph_batch_size) {
                Ok(pending) if !pending.is_empty() => {
                    if model.is_none() {
                        info!(
                            "Semantic Graph Worker [{}]: Initializing BGE-Large Model (1024d) on first admitted graph workload...",
                            worker_idx
                        );
                        model = Self::build_text_embedding_model("graph", worker_idx);
                        if model.is_none() {
                            return;
                        }
                    }
                    if !graph_model_registered {
                        if let Err(e) = graph_store.ensure_embedding_model(
                            GRAPH_MODEL_ID,
                            "graph",
                            MODEL_NAME,
                            DIMENSION as i64,
                            MODEL_VERSION,
                        ) {
                            error!(
                                "Semantic Graph Worker [{}]: failed to register graph embedding model: {:?}",
                                worker_idx, e
                            );
                        }
                        graph_model_registered = true;
                    }
                    debug!(
                        "Semantic Graph Worker [{}]: Embedding {} graph projection jobs...",
                        worker_idx,
                        pending.len()
                    );

                    let mut to_embed: Vec<(GraphProjectionWork, String, String, String)> =
                        Vec::new();
                    let mut failed: HashMap<String, Vec<GraphProjectionWork>> = HashMap::new();

                    for work in pending {
                        let maybe_state = match work.anchor_type.as_str() {
                            "file" => {
                                if let Err(err) = graph_store
                                    .refresh_file_projection(&work.anchor_id, work.radius as u64)
                                {
                                    let reason =
                                        format!("failed to refresh file projection: {:?}", err);
                                    failed.entry(reason).or_default().push(work.clone());
                                    None
                                } else {
                                    graph_store
                                        .fetch_graph_projection_state(
                                            "file",
                                            &work.anchor_id,
                                            work.radius,
                                        )
                                        .ok()
                                        .and_then(|state| match state {
                                            Some(state) => Some((work.clone(), state)),
                                            None => {
                                                failed
                                                    .entry(format!(
                                                        "missing projection state for {}",
                                                        work.anchor_id
                                                    ))
                                                    .or_default()
                                                    .push(work.clone());
                                                None
                                            }
                                        })
                                }
                            }
                            "symbol" => match graph_store
                                .refresh_symbol_projection(&work.anchor_id, work.radius as u64)
                            {
                                Ok(Some(_anchor_id)) => graph_store
                                    .fetch_graph_projection_state(
                                        "symbol",
                                        &work.anchor_id,
                                        work.radius,
                                    )
                                    .ok()
                                    .and_then(|state| match state {
                                        Some(state) => Some((work.clone(), state)),
                                        None => {
                                            failed
                                                .entry(format!(
                                                    "missing projection state for {}",
                                                    work.anchor_id
                                                ))
                                                .or_default()
                                                .push(work.clone());
                                            None
                                        }
                                    }),
                                Ok(None) => {
                                    debug!(
                                        "Semantic Graph Worker [{}]: symbol projection anchor gone, dropping job {}",
                                        worker_idx, work.anchor_id
                                    );
                                    if let Err(err) = graph_store.mark_graph_projection_work_done(
                                        std::slice::from_ref(&work),
                                    ) {
                                        let reason = format!(
                                            "failed to drop stale symbol projection job: {:?}",
                                            err
                                        );
                                        failed.entry(reason).or_default().push(work.clone());
                                    }
                                    None
                                }
                                Err(err) => {
                                    let reason =
                                        format!("failed to refresh symbol projection: {:?}", err);
                                    failed.entry(reason).or_default().push(work.clone());
                                    None
                                }
                            },
                            anchor_type => {
                                failed
                                    .entry(format!("unsupported anchor_type {}", anchor_type))
                                    .or_default()
                                    .push(work.clone());
                                None
                            }
                        };

                        if let Some((work, (source_signature, projection_version))) = maybe_state {
                            match graph_store.has_matching_graph_projection_embedding(
                                &work.anchor_type,
                                &work.anchor_id,
                                work.radius,
                                GRAPH_MODEL_ID,
                                &source_signature,
                                &projection_version,
                            ) {
                                Ok(true) => {
                                    if let Err(err) = graph_store.mark_graph_projection_work_done(
                                        std::slice::from_ref(&work),
                                    ) {
                                        failed
                                            .entry(format!(
                                                "failed to clear up-to-date projection job: {:?}",
                                                err
                                            ))
                                            .or_default()
                                            .push(work.clone());
                                    }
                                    continue;
                                }
                                Ok(false) => {}
                                Err(err) => {
                                    failed
                                        .entry(format!(
                                            "failed to check projection freshness: {:?}",
                                            err
                                        ))
                                        .or_default()
                                        .push(work.clone());
                                    continue;
                                }
                            }

                            match graph_store.graph_projection_embedding_text(
                                &work.anchor_type,
                                &work.anchor_id,
                                work.radius,
                            ) {
                                Ok(content) => to_embed.push((
                                    work,
                                    source_signature,
                                    projection_version,
                                    content,
                                )),
                                Err(err) => {
                                    failed
                                        .entry(format!(
                                            "failed to build projection text: {:?}",
                                            err
                                        ))
                                        .or_default()
                                        .push(work);
                                }
                            }
                        }
                    }

                    if !to_embed.is_empty() {
                        let texts: Vec<String> = to_embed
                            .iter()
                            .map(|(_, _, _, content)| content.clone())
                            .collect();
                        match model
                            .as_mut()
                            .expect("graph model should be initialized before embedding")
                            .embed(texts, None)
                        {
                            Ok(embeddings) => {
                                let updates: Vec<(String, String, i64, String, String, Vec<f32>)> =
                                    to_embed
                                        .iter()
                                        .zip(embeddings)
                                        .map(
                                            |(
                                                (work, source_signature, projection_version, _),
                                                embedding,
                                            )| {
                                                (
                                                    work.anchor_type.clone(),
                                                    work.anchor_id.clone(),
                                                    work.radius,
                                                    source_signature.clone(),
                                                    projection_version.clone(),
                                                    embedding,
                                                )
                                            },
                                        )
                                        .collect();
                                let done_works: Vec<GraphProjectionWork> = to_embed
                                    .iter()
                                    .map(|(work, _, _, _)| work.clone())
                                    .collect();
                                if let Err(err) =
                                    graph_store.update_graph_embeddings(GRAPH_MODEL_ID, &updates)
                                {
                                    let reason =
                                        format!("graph embedding DB write failed: {:?}", err);
                                    for work in done_works {
                                        failed.entry(reason.clone()).or_default().push(work);
                                    }
                                } else if let Err(err) =
                                    graph_store.mark_graph_projection_work_done(&done_works)
                                {
                                    error!(
                                        "Semantic Graph Worker [{}]: failed to clear done projection jobs: {:?}",
                                        worker_idx, err
                                    );
                                    for work in done_works {
                                        failed
                                            .entry(format!(
                                                "failed to clear done projection jobs: {:?}",
                                                err
                                            ))
                                            .or_default()
                                            .push(work);
                                    }
                                }
                            }
                            Err(err) => {
                                if is_fatal_embedding_error(&err) {
                                    error!(
                                        "Semantic Graph Worker [{}]: fatal graph embedding error, disabling semantic worker: {:?}",
                                        worker_idx, err
                                    );
                                    return;
                                }
                                error!(
                                    "Semantic Graph Worker [{}]: graph embedding failed: {:?}",
                                    worker_idx, err
                                );
                                for (work, _, _, _) in to_embed {
                                    failed
                                        .entry(format!("graph embedding failed: {:?}", err))
                                        .or_default()
                                        .push(work);
                                }
                            }
                        }
                    }

                    for (reason, work) in failed {
                        if let Err(err) =
                            graph_store.mark_graph_projection_work_failed(&work, &reason)
                        {
                            error!(
                                "Semantic Graph Worker [{}]: failed to persist projection failure state [{}]: {:?}",
                                worker_idx, reason, err
                            );
                        }
                    }
                    continue;
                }
                Ok(_) => thread::sleep(policy.idle_sleep),
                Err(err) => {
                    error!(
                        "Semantic Graph Worker [{}]: graph projection fetch error: {:?}",
                        worker_idx, err
                    );
                    thread::sleep(policy.idle_sleep);
                }
            }
        }
    }

    fn build_text_embedding_model(lane: &str, worker_idx: usize) -> Option<TextEmbedding> {
        let options = InitOptions::new(fastembed_model())
            .with_cache_dir(embedding_model_cache_dir())
            .with_show_download_progress(embedding_download_progress_enabled())
            .with_max_length(configured_embedding_max_length());
        let provider_requested = effective_provider_request_for_lane(lane);
        let cuda_requested = provider_requested.eq_ignore_ascii_case("cuda");
        let cuda_available = std::env::var("AXON_EMBEDDING_GPU_PRESENT")
            .ok()
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let cuda_provider_library_available = ort_cuda_provider_library_available();
        if cuda_requested && cuda_available && !cuda_provider_library_available {
            let provider_path = ort_cuda_provider_library_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            error!(
                "❌ Semantic {} Worker [{}]: CUDA requested but ONNX Runtime provider library is missing: {}",
                lane, worker_idx, provider_path
            );
            set_embedding_provider_runtime_state("cpu_missing_cuda_provider", None);
        }
        let cuda_options = if cuda_requested && cuda_available && cuda_provider_library_available {
            Some(
                options
                    .clone()
                    .with_execution_providers(vec![cuda_execution_provider_dispatch()]),
            )
        } else {
            None
        };

        let model_result = if let Some(cuda_options) = cuda_options {
            match TextEmbedding::try_new(cuda_options) {
                Ok(model) => {
                    set_embedding_provider_runtime_state("cuda", None);
                    Ok(model)
                }
                Err(err) => {
                    let rendered = format!("{err:?}");
                    error!(
                        "❌ Semantic {} Worker [{}]: CUDA init failed, falling back to CPU: {:?}",
                        lane, worker_idx, err
                    );
                    set_embedding_provider_runtime_state("cpu_fallback", Some(&rendered));
                    apply_cpu_fallback_ort_runtime_env();
                    TextEmbedding::try_new(options)
                }
            }
        } else {
            set_embedding_provider_runtime_state(
                cpu_provider_effective_label(
                    cuda_requested,
                    cuda_available,
                    cuda_provider_library_available,
                ),
                None,
            );
            TextEmbedding::try_new(options)
        };

        match model_result {
            Ok(model) => {
                let provider_effective = current_embedding_provider_effective();
                register_embedding_provider_diagnostics(embedding_provider_diagnostics(
                    provider_effective.clone(),
                ));
                info!(
                    "✅ Semantic {} Worker [{}]: BGE-Large model loaded successfully (provider_effective={}).",
                    lane, worker_idx, provider_effective
                );
                Some(model)
            }
            Err(e) => {
                let rendered = format!("{e:?}");
                error!(
                    "❌ Semantic {} Worker [{}]: FATAL ONNX INIT ERROR: {:?}",
                    lane, worker_idx, e
                );
                set_embedding_provider_runtime_state(
                    &current_embedding_provider_effective(),
                    Some(&rendered),
                );
                None
            }
        }
    }

    fn build_vector_embedding_model(worker_idx: usize) -> Option<VectorEmbeddingBackend> {
        if gpu_embed_service_enabled() {
            match gpu_embedding_service_client() {
                Ok(client) => {
                    publish_embedding_provider_state(gpu_service_provider_effective_label(), None);
                    return Some(VectorEmbeddingBackend::gpu_service(
                        client,
                        gpu_embed_service_prefers_tensorrt(),
                    ));
                }
                Err(err) => {
                    let rendered = format!("{err:?}");
                    error!(
                        "❌ Semantic vector Worker [{}]: GPU embedding service init failed: {:?}",
                        worker_idx, err
                    );
                    publish_embedding_provider_state("unavailable", Some(&rendered));
                    return None;
                }
            }
        }

        let provider_requested = effective_provider_request_for_lane("vector");
        let cuda_requested = provider_requested.eq_ignore_ascii_case("cuda");
        let cuda_available = std::env::var("AXON_EMBEDDING_GPU_PRESENT")
            .ok()
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let cuda_provider_library_available = ort_cuda_provider_library_available();

        if cuda_requested && cuda_available && !cuda_provider_library_available {
            let provider_path = ort_cuda_provider_library_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            error!(
                "❌ Semantic vector Worker [{}]: CUDA requested but ONNX Runtime provider library is missing: {}",
                worker_idx, provider_path
            );
            set_embedding_provider_runtime_state("cpu_missing_cuda_provider", None);
        }

        let model_result = if cuda_requested && cuda_available && cuda_provider_library_available {
            match OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, true) {
                Ok(model) => {
                    set_embedding_provider_runtime_state("cuda", None);
                    Ok(model)
                }
                Err(err) => {
                    let rendered = format!("{err:?}");
                    error!(
                            "❌ Semantic vector Worker [{}]: ORT CUDA init failed, falling back to CPU: {:?}",
                            worker_idx, err
                        );
                    set_embedding_provider_runtime_state("cpu_fallback", Some(&rendered));
                    apply_cpu_fallback_ort_runtime_env();
                    OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
                }
            }
        } else {
            set_embedding_provider_runtime_state(
                cpu_provider_effective_label(
                    cuda_requested,
                    cuda_available,
                    cuda_provider_library_available,
                ),
                None,
            );
            OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
        };

        match model_result {
            Ok(model) => {
                let provider_effective = current_embedding_provider_effective();
                register_embedding_provider_diagnostics(embedding_provider_diagnostics(
                    provider_effective.clone(),
                ));
                if provider_effective.starts_with("cuda") {
                    Some(VectorEmbeddingBackend::cuda_in_process(model))
                } else {
                    Some(VectorEmbeddingBackend::cpu_in_process(model))
                }
            }
            Err(err) => {
                let rendered = format!("{err:?}");
                error!(
                    "❌ Semantic vector Worker [{}]: FATAL ORT GPU-FIRST INIT ERROR: {:?}",
                    worker_idx, err
                );
                publish_embedding_provider_state("unavailable", Some(&rendered));
                None
            }
        }
    }
}

fn build_vector_batch_plan(
    graph_store: &GraphStore,
    active_works: &[FileVectorizationWork],
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    reserved_chunk_ids: &HashSet<String>,
) -> VectorBatchPlan {
    let mut plan = VectorBatchPlan::default();
    if active_works.is_empty() || target_chunks == 0 {
        return plan;
    }

    let per_file_fetch_limit = per_file_fetch_limit.max(1);
    let batch_max_bytes = batch_max_bytes.max(1);

    // Batch-fetch all files in a single query (eliminates N+1 roundtrips).
    let file_paths: Vec<&str> = active_works.iter().map(|w| w.file_path.as_str()).collect();
    let fetch_started = Instant::now();
    let fetched_map = match graph_store.fetch_unembedded_chunks_batch(
        &file_paths,
        CHUNK_MODEL_ID,
        per_file_fetch_limit.saturating_add(1),
    ) {
        Ok(map) => {
            let fetch_ms = fetch_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::Fetch,
                fetch_ms,
            );
            plan.fetch_ms_total = fetch_ms;
            map
        }
        Err(err) => {
            let fetch_ms = fetch_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::Fetch,
                fetch_ms,
            );
            plan.fetch_ms_total = fetch_ms;
            for work in active_works {
                plan.failed_fetches
                    .push((work.clone(), format!("{:?}", err)));
            }
            return plan;
        }
    };

    let mut touched_files = HashSet::new();
    let mut planned_chunks = 0usize;
    let mut planned_bytes = 0usize;

    for work in active_works {
        let chunks = fetched_map
            .get(&work.file_path)
            .cloned()
            .unwrap_or_default();

        if chunks.is_empty() {
            plan.immediate_completed.push(work.clone());
            continue;
        }

        let remaining_chunk_budget = target_chunks.saturating_sub(planned_chunks).max(1);
        let fetch_limit = per_file_fetch_limit.min(remaining_chunk_budget);

        let filtered_chunks = chunks
            .into_iter()
            .filter(|(chunk_id, _, _)| !reserved_chunk_ids.contains(chunk_id))
            .collect::<Vec<_>>();
        let mut batch_chunks = Vec::new();
        let mut fetched_bytes = 0usize;
        let mut has_more_after_batch = false;
        let total_filtered_chunks = filtered_chunks.len();
        for (index, chunk) in filtered_chunks.into_iter().enumerate() {
            let chunk_bytes = chunk.1.len();
            let exceeds_chunk_budget = batch_chunks.len() >= fetch_limit;
            let exceeds_byte_budget = !batch_chunks.is_empty()
                && planned_bytes
                    .saturating_add(fetched_bytes)
                    .saturating_add(chunk_bytes)
                    > batch_max_bytes;
            if exceeds_chunk_budget || exceeds_byte_budget {
                has_more_after_batch = true;
                break;
            }
            fetched_bytes = fetched_bytes.saturating_add(chunk_bytes);
            batch_chunks.push(chunk);
            if index + 1 >= fetch_limit {
                has_more_after_batch = total_filtered_chunks > fetch_limit;
                break;
            }
        }
        let fetched_count = batch_chunks.len();
        let exceeds_chunk_budget =
            planned_chunks.saturating_add(fetched_count) > target_chunks;
        let exceeds_byte_budget =
            planned_bytes.saturating_add(fetched_bytes) > batch_max_bytes;
        if batch_chunks.is_empty() {
            if total_filtered_chunks > 0 && fetch_limit <= 1 {
                plan.oversized_works.push(work.clone());
            } else {
                plan.untouched_works.push(work.clone());
            }
            continue;
        }
        if fetched_count == 1
            && fetched_bytes > batch_max_bytes
            && planned_chunks == 0
            && planned_bytes == 0
        {
            plan.oversized_works.push(work.clone());
            continue;
        }
        if exceeds_chunk_budget || exceeds_byte_budget {
            plan.untouched_works.push(work.clone());
            continue;
        }
        touched_files.insert(work.file_path.clone());
        plan.touched_works.push(work.clone());
        for (chunk_id, text, content_hash) in batch_chunks {
            plan.work_items.push(VectorChunkWorkItem {
                file_path: work.file_path.clone(),
                chunk_id,
                content_hash,
                text,
            });
        }
        planned_chunks = planned_chunks.saturating_add(fetched_count);
        planned_bytes = planned_bytes.saturating_add(fetched_bytes);
        if has_more_after_batch {
            plan.partial_file_cycles = plan.partial_file_cycles.saturating_add(1);
            plan.continuation_works.push(work.clone());
        } else {
            plan.finalize_after_success.push(work.clone());
        }
    }

    plan.files_touched = touched_files.len();
    plan
}

fn prepare_vector_embed_batch(
    graph_store: &GraphStore,
    active_works: &[FileVectorizationWork],
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    reserved_chunk_ids: &HashSet<String>,
) -> PreparedVectorEmbedBatch {
    PreparedVectorEmbedBatch::from_plan(build_vector_batch_plan(
        graph_store,
        active_works,
        target_chunks,
        per_file_fetch_limit,
        batch_max_bytes,
        reserved_chunk_ids,
    ))
}

fn work_with_lease_snapshots(
    work: &[FileVectorizationWork],
    snapshots: &[FileVectorizationLeaseSnapshot],
) -> Vec<FileVectorizationWork> {
    let snapshot_by_path = snapshots
        .iter()
        .map(|snapshot| (snapshot.file_path.as_str(), snapshot))
        .collect::<std::collections::HashMap<_, _>>();
    work.iter()
        .map(|item| {
            if let Some(_snapshot) = snapshot_by_path.get(item.file_path.as_str()) {
                FileVectorizationWork {
                    file_path: item.file_path.clone(),
                    resumed_after_interactive_pause: item.resumed_after_interactive_pause,
                }
            } else {
                item.clone()
            }
        })
        .collect()
}

fn recover_ready_batches_to_active_works(
    ready_batches: &SharedPreparedBatchQueue,
) -> Vec<FileVectorizationWork> {
    let recovered = ready_batches
        .drain()
        .into_iter()
        .flat_map(|batch| batch.into_touched_works())
        .collect::<Vec<_>>();
    record_vector_frontier_metrics(&PreparedBatchQueueSummary::default(), 0, 0, 0, 0);
    recovered
}

fn record_vector_frontier_metrics(
    ready_queue_summary: &PreparedBatchQueueSummary,
    prepare_inflight_depth: usize,
    prepare_inflight_chunks: usize,
    target_ready_chunks: usize,
    active_replenishment_chunks: usize,
) {
    let oldest_ready_batch_age_ms = ready_queue_summary
        .oldest_prepared_at_ms
        .map(|prepared_at_ms| {
            chrono::Utc::now()
                .timestamp_millis()
                .saturating_sub(prepared_at_ms)
        })
        .unwrap_or(0) as u64;
    let front_chunk_supply = ready_queue_summary
        .chunk_count
        .saturating_add(prepare_inflight_chunks);

    service_guard::record_vector_prepare_inflight_depth(prepare_inflight_depth as u64);
    service_guard::record_vector_prepare_inflight_chunks(prepare_inflight_chunks as u64);
    record_ready_queue_summary(ready_queue_summary);
    service_guard::record_vector_oldest_ready_batch_age_ms(oldest_ready_batch_age_ms);
    let fallback_stock_gap = target_ready_chunks.saturating_sub(front_chunk_supply);
    let active_command_gap = if active_replenishment_chunks > 0 {
        active_replenishment_chunks
    } else {
        fallback_stock_gap
    };
    service_guard::set_vector_ready_replenishment_deficit(active_command_gap as u64);
}

fn send_vector_refill_requeue(
    graph_store: &GraphStore,
    worker_idx: usize,
    refill_tx: &Sender<VectorRefillCommand>,
    works: Vec<FileVectorizationWork>,
) {
    if works.is_empty() {
        return;
    }
    if let Err(err) = refill_tx.send(VectorRefillCommand::RequeueWorks(works.clone())) {
        error!(
            "Semantic Vector Worker [{}]: refill worker unavailable while requeueing vector work: {}",
            worker_idx, err
        );
        if let Err(requeue_err) = graph_store
            .mark_file_vectorization_work_failed(&works, "refill_worker_unavailable_requeued")
        {
            error!(
                "Semantic Vector Worker [{}]: failed to requeue vector work after refill worker loss: {:?}",
                worker_idx, requeue_err
            );
        }
    }
}

fn wait_for_vector_persist_outcome(
    graph_store: &Arc<GraphStore>,
    worker_idx: usize,
    reply_rx: VectorPersistOutcomeReply,
    owned_works: &[FileVectorizationWork],
) -> Option<VectorPersistOutcome> {
    loop {
        service_guard::record_vector_worker_heartbeat();
        let _ = graph_store.refresh_file_vectorization_leases_for_owner(owned_works, "vector");
        match reply_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(outcome) => return Some(outcome),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                error!(
                    "Semantic Vector Worker [{}]: persist reply unavailable: disconnected",
                    worker_idx
                );
                return None;
            }
        }
    }
}

fn split_claimed_vectorization_work(
    active_works: &mut Vec<FileVectorizationWork>,
    target_files: usize,
) -> Vec<FileVectorizationWork> {
    if target_files == 0 || active_works.is_empty() {
        return Vec::new();
    }

    let take = target_files.min(active_works.len());
    let remainder = if active_works.len() > take {
        active_works.split_off(take)
    } else {
        Vec::new()
    };
    std::mem::replace(active_works, remainder)
}

fn apply_prepared_vector_prepare_outcome(
    outcome: PreparedVectorPrepareOutcome,
    active_works: &mut Vec<FileVectorizationWork>,
) {
    let remaining = outcome.remaining_claimed_after_success.into_inner();
    let merge_target = active_works.len().saturating_add(remaining.len()).max(1);
    let current_active = std::mem::take(active_works);
    *active_works = merge_vectorization_work(current_active, remaining, merge_target);
}

#[cfg(test)]
fn wait_for_inflight_prepare_sequence(
    graph_store: &Arc<GraphStore>,
    worker_idx: usize,
    inflight: InflightPrepareRequest,
) -> Result<PreparedVectorPrepareOutcome, ClaimedLeaseSet> {
    let claimed = inflight.claimed;
    loop {
        service_guard::record_vector_worker_heartbeat();
        let _ =
            graph_store.refresh_file_vectorization_leases_for_owner(claimed.as_slice(), "vector");
        match inflight.reply_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(prepared) => {
                service_guard::record_vector_prepare_reply_wait_ms(
                    inflight.dispatched_at.elapsed().as_millis() as u64,
                );
                return Ok(prepared);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                error!(
                    "Semantic Vector Worker [{}]: prepare reply unavailable: disconnected",
                    worker_idx
                );
                return Err(claimed);
            }
        }
    }
}

fn record_vector_pipeline_snapshot(
    graph_store: &GraphStore,
    queued_backlog_depth: usize,
    active_works: &[FileVectorizationWork],
    inflight_prepares: &VecDeque<InflightPrepareRequest>,
    ready_queue_summary: &PreparedBatchQueueSummary,
    inflight_persists: &VecDeque<InflightPersistRequest>,
    target_ready_chunks: usize,
    active_replenishment_chunks: usize,
) {
    let active_claimed = active_works.len() as u64;
    let prepare_claimed = merge_unique_vectorization_work_sets(
        inflight_prepares
            .iter()
            .map(|request| request.claimed.clone_works())
            .collect::<Vec<_>>(),
    )
    .len() as u64;
    let ready_claimed = ready_queue_summary.touched_works_count as u64;
    let persist_claimed = merge_unique_vectorization_work_sets(
        inflight_persists
            .iter()
            .map(|request| request.claimed.clone_works())
            .collect::<Vec<_>>(),
    )
    .len() as u64;
    let (outbox_queued, outbox_inflight) = graph_store
        .fetch_vector_persist_outbox_counts()
        .unwrap_or((0, 0));
    let canonical_backlog = queued_backlog_depth as u64
        + active_claimed
        + prepare_claimed
        + ready_claimed
        + persist_claimed
        + outbox_queued as u64
        + outbox_inflight as u64;
    service_guard::record_vector_active_claimed(active_claimed);
    service_guard::record_vector_prepare_claimed(prepare_claimed);
    service_guard::record_vector_ready_claimed(ready_claimed);
    service_guard::record_vector_persist_claimed(persist_claimed);
    service_guard::record_vector_persist_queue_depth(inflight_persists.len() as u64);
    service_guard::record_vector_canonical_backlog_depth(canonical_backlog);
    record_vector_frontier_metrics(
        ready_queue_summary,
        inflight_prepares.len(),
        inflight_prepares
            .iter()
            .map(|request| request.planned_chunk_count)
            .sum(),
        target_ready_chunks,
        active_replenishment_chunks,
    );
}

fn record_ready_queue_summary(summary: &PreparedBatchQueueSummary) {
    service_guard::record_vector_ready_queue_depth(summary.len as u64);
    service_guard::record_vector_ready_queue_chunks(summary.chunk_count as u64);
    service_guard::record_vector_ready_queue_lane_chunks(
        summary.small_chunk_count as u64,
        summary.medium_chunk_count as u64,
        summary.large_chunk_count as u64,
    );
    service_guard::record_vector_ready_queue_lane_batches(
        summary.small_batch_count as u64,
        summary.medium_batch_count as u64,
        summary.large_batch_count as u64,
        summary.mixed_batch_count as u64,
    );
}

fn dispatch_prepared_vector_embed_sequence(
    prepare_tx: &Sender<VectorPrepareRequest>,
    claimed: ClaimedLeaseSet,
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
) -> AnyhowResult<PreparedVectorPrepareOutcomeReply> {
    let (reply_tx, reply_rx) = bounded(1);
    service_guard::record_vector_prepare_dispatch();
    let prepare_send_started = Instant::now();
    prepare_tx
        .send(VectorPrepareRequest {
            claimed,
            target_chunks,
            per_file_fetch_limit,
            batch_max_bytes,
            target_ready_depth,
            enqueued_at: Instant::now(),
            reply: reply_tx,
        })
        .map_err(|err| anyhow!("prepare worker unavailable: {}", err))?;
    service_guard::record_vector_prepare_send_wait_ms(
        prepare_send_started.elapsed().as_millis() as u64
    );
    service_guard::record_vector_prepare_queue_depth(prepare_tx.len() as u64);
    Ok(reply_rx)
}

fn merge_vectorization_work(
    existing: Vec<FileVectorizationWork>,
    additional: Vec<FileVectorizationWork>,
    target_files_per_cycle: usize,
) -> Vec<FileVectorizationWork> {
    if target_files_per_cycle == 0 {
        return Vec::new();
    }

    let mut merged = Vec::with_capacity(target_files_per_cycle);
    let mut seen = HashSet::new();
    for work in existing.into_iter().chain(additional) {
        if seen.insert(work.file_path.clone()) {
            merged.push(work);
        }
        if merged.len() >= target_files_per_cycle {
            break;
        }
    }
    merged
}

fn merge_unique_vectorization_work_sets<I>(sets: I) -> Vec<FileVectorizationWork>
where
    I: IntoIterator<Item = Vec<FileVectorizationWork>>,
{
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for set in sets {
        for work in set {
            if seen.insert(work.file_path.clone()) {
                merged.push(work);
            }
        }
    }
    merged
}

fn query_embedding_sender_slot() -> &'static Mutex<Option<Sender<QueryEmbeddingRequest>>> {
    QUERY_EMBEDDING_SENDER.get_or_init(|| Mutex::new(None))
}

fn register_query_embedding_sender(sender: Sender<QueryEmbeddingRequest>) {
    let mut slot = query_embedding_sender_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(sender);
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_usize_nonnegative(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn recommended_ort_auto_threads() -> usize {
    std::env::var("AXON_ORT_AUTO_THREADS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .or_else(|| {
            thread::available_parallelism()
                .ok()
                .map(|parallelism| parallelism.get())
        })
        .unwrap_or(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FallbackAdjustment {
    applied_before_pool_start: bool,
    applied_env: Vec<(String, String)>,
    advisory_lane_sizing: Vec<(String, String)>,
}

impl FallbackAdjustment {
    fn new() -> Self {
        Self {
            applied_before_pool_start: false,
            applied_env: Vec::new(),
            advisory_lane_sizing: Vec::new(),
        }
    }
}

fn apply_cpu_fallback_ort_runtime_env() -> FallbackAdjustment {
    let mut adjustment = FallbackAdjustment::new();
    let auto_configured = std::env::var("AXON_ORT_OMP_AUTOCONFIGURED")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if auto_configured {
        let cpu_threads = recommended_ort_auto_threads();
        unsafe {
            std::env::set_var("OMP_NUM_THREADS", cpu_threads.to_string());
            std::env::set_var("OMP_WAIT_POLICY", "PASSIVE");
        }
        adjustment
            .applied_env
            .push(("OMP_NUM_THREADS".to_string(), cpu_threads.to_string()));
        adjustment
            .applied_env
            .push(("OMP_WAIT_POLICY".to_string(), "PASSIVE".to_string()));
    }

    let ort_threads_autoconfigured = std::env::var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if ort_threads_autoconfigured {
        let cpu_threads = recommended_ort_auto_threads();
        unsafe {
            std::env::set_var("AXON_ORT_INTRA_THREADS", cpu_threads.to_string());
        }
        adjustment.applied_env.push((
            "AXON_ORT_INTRA_THREADS".to_string(),
            cpu_threads.to_string(),
        ));
    }

    let mut cpu_profile = RuntimeProfile::detect();
    cpu_profile.gpu_present = false;
    let cpu_lane_sizing = recommend_embedding_lane_sizing(&cpu_profile);

    for (marker, env_name, value) in [
        (
            "AXON_VECTOR_WORKERS_AUTOCONFIGURED",
            "AXON_VECTOR_WORKERS",
            cpu_lane_sizing.vector_workers.to_string(),
        ),
        (
            "AXON_GRAPH_WORKERS_AUTOCONFIGURED",
            "AXON_GRAPH_WORKERS",
            cpu_lane_sizing.graph_workers.to_string(),
        ),
        (
            "AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED",
            "AXON_CHUNK_BATCH_SIZE",
            cpu_lane_sizing.chunk_batch_size.to_string(),
        ),
        (
            "AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED",
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            cpu_lane_sizing.file_vectorization_batch_size.to_string(),
        ),
        (
            "AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED",
            "AXON_GRAPH_BATCH_SIZE",
            cpu_lane_sizing.graph_batch_size.to_string(),
        ),
    ] {
        let should_restore = std::env::var(marker)
            .ok()
            .map(|raw| raw.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if should_restore {
            adjustment
                .advisory_lane_sizing
                .push((env_name.to_string(), value));
        }
    }
    adjustment
}

fn embedding_model_cache_dir() -> PathBuf {
    profile_embedding_model_cache_dir()
}

fn embedding_download_progress_enabled() -> bool {
    std::env::var("AXON_EMBEDDING_DOWNLOAD_PROGRESS")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn gpu_memory_soft_limit_mb() -> u64 {
    if let Some(configured) = std::env::var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 512)
    {
        return configured;
    }
    if let Some(operator_budget) = std::env::var("AXON_OPT_MAX_VRAM_USED_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 512)
    {
        return operator_budget;
    }
    current_gpu_memory_snapshot()
        .map(|snapshot| {
            let derived = ((snapshot.total_mb as f64) * 0.80).round() as u64;
            derived.clamp(4_096, snapshot.total_mb.saturating_sub(256).max(4_096))
        })
        .unwrap_or(6_144)
}

pub fn vector_stale_inflight_claim_age_ms() -> u64 {
    std::env::var("AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(DEFAULT_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS)
}

pub fn vector_stale_inflight_recovery_interval_ms() -> u64 {
    let base_ms = std::env::var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS);
    scale_vector_maintenance_interval_for_quiescent(base_ms, 1_000, 300_000)
}

fn vector_claimable_supply_poll_interval_ms() -> u64 {
    scale_vector_maintenance_interval_for_quiescent(
        std::env::var("AXON_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value >= 50)
            .unwrap_or(DEFAULT_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS),
        50,
        10_000,
    )
}

fn desired_vector_claimable_supply_depth(
    metrics: service_guard::VectorRuntimeMetrics,
    claimable_file_backlog_depth: usize,
    inflight_file_backlog_depth: usize,
) -> usize {
    let controller = current_vector_batch_controller_diagnostics(&embedding_lane_config_from_env());
    let upstream_file_pressure = claimable_file_backlog_depth
        .saturating_add(inflight_file_backlog_depth)
        .saturating_add(metrics.active_claimed_current as usize)
        .saturating_add(metrics.prepare_claimed_current as usize)
        .saturating_add(metrics.persist_claimed_current as usize);
    let target_ready_chunks = vector_ready_chunk_reserve_target(
        configured_target_ready_chunks(),
        upstream_file_pressure,
        controller.target_files_per_cycle,
        controller.target_embed_batch_chunks,
        metrics.ready_queue_chunks_current as usize,
        metrics.prepare_inflight_chunks_current as usize,
        controller.avg_chunks_per_embed_call,
        metrics.oldest_ready_batch_age_ms_current,
    );
    let front_chunk_demand =
        target_ready_chunks.saturating_add(metrics.ready_replenishment_deficit_current as usize);
    let front_chunk_supply = (metrics.ready_queue_chunks_current as usize)
        .saturating_add(metrics.prepare_inflight_chunks_current as usize);
    let claimable_floor = if upstream_file_pressure >= AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD {
        controller.target_files_per_cycle.max(1).saturating_mul(4)
    } else {
        controller.target_files_per_cycle.max(1).saturating_mul(2)
    };
    front_chunk_demand
        .saturating_sub(front_chunk_supply)
        .div_ceil(default_chunks_per_file_estimate())
        .max(claimable_floor)
        .max(32)
        .max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VectorReplenishmentMode {
    ConsumptionDriven,
    StockGapDriven,
}

fn vector_replenishment_command(
    target_ready_chunks: usize,
    ready_queue_chunks: usize,
    prepare_inflight_chunks: usize,
    pending_replenishment_chunks: usize,
) -> (VectorReplenishmentMode, usize, usize) {
    let front_chunk_supply = ready_queue_chunks.saturating_add(prepare_inflight_chunks);
    let stock_gap_chunks = target_ready_chunks.saturating_sub(front_chunk_supply);
    if pending_replenishment_chunks > 0 {
        let command_chunks = pending_replenishment_chunks.saturating_sub(prepare_inflight_chunks);
        (
            VectorReplenishmentMode::ConsumptionDriven,
            command_chunks,
            pending_replenishment_chunks,
        )
    } else {
        (
            VectorReplenishmentMode::StockGapDriven,
            stock_gap_chunks,
            stock_gap_chunks,
        )
    }
}

fn maintain_vector_claimable_supply(graph_store: &GraphStore) -> anyhow::Result<usize> {
    let claimable_file_backlog_depth =
        graph_store.fetch_claimable_file_vectorization_queue_count()?;
    let (_queued, inflight) = graph_store.fetch_file_vectorization_queue_counts()?;
    let metrics = service_guard::vector_runtime_metrics();
    let desired_claimable_depth =
        desired_vector_claimable_supply_depth(metrics, claimable_file_backlog_depth, inflight);
    let current_supply_depth = claimable_file_backlog_depth
        .saturating_add(metrics.active_claimed_current as usize)
        .saturating_add(metrics.prepare_claimed_current as usize)
        .saturating_add(metrics.persist_claimed_current as usize);
    let refill_deficit = desired_claimable_depth.saturating_sub(current_supply_depth);
    if refill_deficit == 0 {
        return Ok(0);
    }
    graph_store.backfill_file_vectorization_queue_with_limit(refill_deficit)
}

fn vector_finalize_idle_poll_interval_ms() -> u64 {
    scale_vector_maintenance_interval_for_quiescent(
        std::env::var("AXON_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value >= 50)
            .unwrap_or(DEFAULT_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS),
        50,
        5_000,
    )
}

fn vector_worker_non_admitted_idle_wait_ms(file_backlog_depth: usize) -> u64 {
    scale_vector_maintenance_interval_for_quiescent(
        std::env::var("AXON_VECTOR_NON_ADMITTED_IDLE_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value >= 1_000)
            .unwrap_or(20_000),
        1_000,
        120_000,
    )
    .max(if file_backlog_depth == 0 { 1_000 } else { 250 })
}

fn vector_worker_non_admitted_backlog_wait_ms(file_backlog_depth: usize) -> u64 {
    scale_vector_maintenance_interval_for_quiescent(
        std::env::var("AXON_VECTOR_NON_ADMITTED_BACKLOG_WAIT_MS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value >= 50)
            .unwrap_or(500),
        50,
        10_000,
    )
    .max(if file_backlog_depth == 0 { 50 } else { 250 })
}

fn scale_vector_maintenance_interval_for_quiescent(base_ms: u64, min_ms: u64, max_ms: u64) -> u64 {
    let scale_pct = std::env::var("AXON_QUIESCENT_INTERVAL_SCALE_PCT")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(400)
        .clamp(100, 2_000);
    service_guard::scale_interval_for_quiescent(
        base_ms,
        service_guard::current_runtime_quiescent_state(0, 0),
        scale_pct,
        min_ms,
        max_ms,
    )
}

fn recover_stale_vector_inflight_now(
    graph_store: &GraphStore,
    now_ms: i64,
) -> anyhow::Result<usize> {
    graph_store.recover_stale_inflight_file_vectorization_work(
        now_ms,
        vector_stale_inflight_claim_age_ms() as i64,
    )
}

fn recover_stale_vector_outbox_now(graph_store: &GraphStore, now_ms: i64) -> anyhow::Result<usize> {
    graph_store.recover_stale_vector_persist_outbox_inflight(
        now_ms.saturating_sub(vector_stale_inflight_claim_age_ms() as i64),
    )
}

pub fn embedding_lane_config_from_env() -> EmbeddingLaneConfig {
    let bootstrap = bootstrap_embedding_lane_config_from_env();
    let tuning = current_runtime_tuning_snapshot().state;
    let runtime_mode = AxonRuntimeMode::from_env();
    let semantic_workers_enabled = runtime_mode.semantic_workers_enabled();
    EmbeddingLaneConfig {
        query_workers: bootstrap.query_workers,
        vector_workers: if semantic_workers_enabled {
            tuning.vector_workers.max(1)
        } else {
            0
        },
        graph_workers: tuning.graph_workers.min(bootstrap.graph_workers),
        chunk_batch_size: tuning.chunk_batch_size.max(1),
        file_vectorization_batch_size: tuning.file_vectorization_batch_size.max(1),
        graph_batch_size: bootstrap.graph_batch_size,
        max_chunks_per_file: bootstrap.max_chunks_per_file,
        max_embed_batch_bytes: bootstrap.max_embed_batch_bytes,
    }
}

pub fn apply_runtime_embedding_lane_adjustment(
    vector_workers: Option<usize>,
    graph_workers: Option<usize>,
    chunk_batch_size: Option<usize>,
    file_vectorization_batch_size: Option<usize>,
    vector_ready_queue_depth: Option<usize>,
    vector_persist_queue_bound: Option<usize>,
    vector_max_inflight_persists: Option<usize>,
    embed_micro_batch_max_items: Option<usize>,
    embed_micro_batch_max_total_tokens: Option<usize>,
    semantic_sleep_scale_pct: Option<usize>,
    semantic_idle_sleep_scale_pct: Option<usize>,
) {
    let _snapshot = update_shared_runtime_tuning_state(
        bootstrap_runtime_tuning_state_from_env(),
        vector_workers,
        graph_workers,
        chunk_batch_size,
        file_vectorization_batch_size,
        vector_ready_queue_depth,
        vector_persist_queue_bound,
        vector_max_inflight_persists,
        embed_micro_batch_max_items,
        embed_micro_batch_max_total_tokens,
        semantic_sleep_scale_pct,
        semantic_idle_sleep_scale_pct,
    );
    if let Some(vector_workers) = vector_workers {
        std::env::set_var("AXON_VECTOR_WORKERS", vector_workers.max(1).to_string());
        std::env::set_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED", "false");
    }
    if let Some(graph_workers) = graph_workers {
        std::env::set_var("AXON_GRAPH_WORKERS", graph_workers.to_string());
        std::env::set_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED", "false");
    }
    if let Some(chunk_batch_size) = chunk_batch_size {
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", chunk_batch_size.max(1).to_string());
        std::env::set_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED", "false");
    }
    if let Some(file_vectorization_batch_size) = file_vectorization_batch_size {
        std::env::set_var(
            "AXON_FILE_VECTORIZATION_BATCH_SIZE",
            file_vectorization_batch_size.max(1).to_string(),
        );
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED", "false");
    }
    if let Some(vector_ready_queue_depth) = vector_ready_queue_depth {
        std::env::set_var(
            "AXON_VECTOR_READY_QUEUE_DEPTH",
            vector_ready_queue_depth.max(1).to_string(),
        );
        std::env::set_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED", "false");
    }
    if let Some(vector_persist_queue_bound) = vector_persist_queue_bound {
        std::env::set_var(
            "AXON_VECTOR_PERSIST_QUEUE_BOUND",
            vector_persist_queue_bound.max(1).to_string(),
        );
        std::env::set_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED", "false");
    }
    if let Some(vector_max_inflight_persists) = vector_max_inflight_persists {
        std::env::set_var(
            "AXON_VECTOR_MAX_INFLIGHT_PERSISTS",
            vector_max_inflight_persists.max(1).to_string(),
        );
        std::env::set_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED", "false");
    }
    if let Some(embed_micro_batch_max_items) = embed_micro_batch_max_items {
        std::env::set_var(
            "AXON_EMBED_MICRO_BATCH_MAX_ITEMS",
            embed_micro_batch_max_items.max(1).to_string(),
        );
        std::env::set_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS_AUTOCONFIGURED", "false");
    }
    if let Some(embed_micro_batch_max_total_tokens) = embed_micro_batch_max_total_tokens {
        std::env::set_var(
            "AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS",
            embed_micro_batch_max_total_tokens.max(1).to_string(),
        );
        std::env::set_var(
            "AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS_AUTOCONFIGURED",
            "false",
        );
    }
    if let Some(semantic_sleep_scale_pct) = semantic_sleep_scale_pct {
        std::env::set_var(
            "AXON_SEMANTIC_SLEEP_SCALE_PCT",
            semantic_sleep_scale_pct.max(1).to_string(),
        );
        std::env::set_var("AXON_SEMANTIC_SLEEP_SCALE_PCT_AUTOCONFIGURED", "false");
    }
    if let Some(semantic_idle_sleep_scale_pct) = semantic_idle_sleep_scale_pct {
        std::env::set_var(
            "AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT",
            semantic_idle_sleep_scale_pct.max(1).to_string(),
        );
        std::env::set_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT_AUTOCONFIGURED", "false");
    }
    refresh_vector_batch_controller_from_env();
}

pub fn recycle_gpu_embedding_service_for_runtime_control() -> anyhow::Result<bool> {
    recycle_existing_gpu_embedding_service()
}

pub fn current_runtime_tuning_state() -> RuntimeTuningState {
    runtime_tuning_state(bootstrap_runtime_tuning_state_from_env())
}

pub fn refresh_vector_batch_controller_from_env() {
    let lane_config = embedding_lane_config_from_env();
    reset_vector_batch_controller_for_tests(&lane_config);
}

fn current_query_embedding_sender() -> Option<Sender<QueryEmbeddingRequest>> {
    query_embedding_sender_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

fn serve_query_embedding_request(model: &mut TextEmbedding, request: QueryEmbeddingRequest) {
    let result = model.embed(request.texts, None);
    let _ = request.reply.send(result);
}

fn request_query_embedding(
    sender: &Sender<QueryEmbeddingRequest>,
    texts: Vec<String>,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let (reply_tx, reply_rx) = bounded(1);
    sender
        .send(QueryEmbeddingRequest {
            texts,
            reply: reply_tx,
        })
        .map_err(|_| {
            anyhow::anyhow!("MCP real-time embedding worker unavailable. Use structural search.")
        })?;

    match reply_rx.recv_timeout(QUERY_EMBED_TIMEOUT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
            "MCP real-time embedding timed out. Use structural search."
        )),
        Err(RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
            "MCP real-time embedding worker disconnected. Use structural search."
        )),
    }
}

impl GraphStore {
    fn escape_embedding_sql(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn graph_projection_embedding_text(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
    ) -> anyhow::Result<String> {
        let projection = self.query_graph_projection(anchor_type, anchor_id, radius as u64)?;
        let rows: Vec<Vec<serde_json::Value>> =
            serde_json::from_str(&projection).unwrap_or_default();
        let mut lines = vec![
            format!("anchor_type: {}", anchor_type),
            format!("anchor_id: {}", anchor_id),
            format!("radius: {}", radius),
        ];

        for row in rows {
            let target_type = row
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let target_id = row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let edge_kind = row
                .get(2)
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let distance = row.get(3).and_then(|value| value.as_i64()).unwrap_or(0);
            let label = row
                .get(4)
                .and_then(|value| value.as_str())
                .unwrap_or(target_id);
            lines.push(format!(
                "target_type: {} | target_id: {} | edge_kind: {} | distance: {} | label: {}",
                target_type, target_id, edge_kind, distance, label
            ));
        }

        Ok(lines.join("\n"))
    }

    #[allow(clippy::type_complexity)]
    pub fn update_graph_embeddings(
        &self,
        model_id: &str,
        updates: &[(String, String, i64, String, String, Vec<f32>)],
    ) -> anyhow::Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let mut queries = Vec::new();
        for (anchor_type, anchor_id, radius, _, _, _) in updates {
            queries.push(format!(
                "DELETE FROM GraphEmbedding WHERE anchor_type = '{}' AND anchor_id = '{}' AND radius = {} AND model_id = '{}';",
                Self::escape_embedding_sql(anchor_type),
                Self::escape_embedding_sql(anchor_id),
                radius,
                Self::escape_embedding_sql(model_id)
            ));
        }

        let now = chrono::Utc::now().timestamp_millis();
        let values: Vec<String> = updates
            .iter()
            .map(
                |(anchor_type, anchor_id, radius, source_signature, projection_version, vector)| {
                    format!(
                        "('{}', '{}', {}, '{}', '{}', '{}', CAST({:?} AS FLOAT[{DIMENSION}]), {})",
                        Self::escape_embedding_sql(anchor_type),
                        Self::escape_embedding_sql(anchor_id),
                        radius,
                        Self::escape_embedding_sql(model_id),
                        Self::escape_embedding_sql(source_signature),
                        Self::escape_embedding_sql(projection_version),
                        vector,
                        now
                    )
                },
            )
            .collect();

        for chunk in values.chunks(100) {
            queries.push(format!(
                "INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES {};",
                chunk.join(",")
            ));
        }

        self.execute_batch(&queries)
    }
}

fn query_embedding_allowed(service_pressure: ServicePressure) -> bool {
    matches!(
        service_pressure,
        ServicePressure::Healthy | ServicePressure::Recovering
    )
}

fn effective_embedding_provider_is_gpu() -> bool {
    std::env::var("AXON_EMBEDDING_PROVIDER_EFFECTIVE")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            value.starts_with("cuda") || value.starts_with("tensorrt")
        })
        .unwrap_or(false)
}

fn pause_vectorization_work_if_interactive(
    graph_store: &GraphStore,
    work: &FileVectorizationWork,
) -> bool {
    if !service_guard::interactive_priority_active() {
        return false;
    }

    match graph_store.pause_file_vectorization_work_for_interactive_priority(
        std::slice::from_ref(work),
        INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS,
        INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT,
    ) {
        Ok(paused) if paused > 0 => {
            service_guard::record_vectorization_interrupted(paused as u64);
            service_guard::record_vectorization_requeued_for_interactive(paused as u64);
            true
        }
        Ok(_) => false,
        Err(err) => {
            error!(
                "Semantic Worker: failed to pause vectorization work for interactive priority [{}]: {:?}",
                work.file_path, err
            );
            false
        }
    }
}

fn fatal_embedding_error_class<E: std::fmt::Debug>(err: &E) -> Option<&'static str> {
    let rendered = format!("{:?}", err);
    if rendered.contains("GetElementType is not implemented") {
        Some("ort_missing_output_type")
    } else if rendered.contains("GPU embed subprocess response timeout")
        || rendered.contains("GPU embed subprocess init handshake timeout")
        || rendered.contains("GPU embed subprocess recycle init handshake timeout")
    {
        Some("gpu_embed_subprocess_timeout")
    } else if rendered.contains("onnxruntime") || rendered.contains("ORT") {
        Some("onnxruntime")
    } else {
        None
    }
}

fn is_gpu_recycle_immediate_error<E: std::fmt::Debug>(err: &E) -> bool {
    format!("{:?}", err).contains("gpu_recycle_immediate_after_vram_summit")
}

fn is_fatal_embedding_error<E: std::fmt::Debug>(err: &E) -> bool {
    fatal_embedding_error_class(err).is_some()
}

fn persist_vector_lane_state(
    graph_store: &Arc<GraphStore>,
    state: VectorLaneState,
    worker_idx: usize,
    restart_attempt: u64,
    reason: Option<String>,
    last_fault_id: Option<String>,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    match state {
        VectorLaneState::Healthy => service_guard::record_vector_lane_success(),
        VectorLaneState::Degraded | VectorLaneState::Unhealthy => {
            service_guard::record_vector_lane_fault()
        }
        VectorLaneState::Starting | VectorLaneState::Hold => {
            service_guard::record_vector_lane_state(state)
        }
    }
    if let Err(err) = graph_store.upsert_vector_lane_state(&VectorLaneStateRecord {
        lane: "vector".to_string(),
        state: state.as_str().to_string(),
        reason,
        updated_at_ms: now_ms,
        worker_id: Some(worker_idx as i64),
        restart_attempt,
        last_success_at_ms: if matches!(state, VectorLaneState::Healthy) {
            Some(now_ms)
        } else {
            None
        },
        last_fault_id,
    }) {
        error!(
            "Semantic Vector Worker [{}]: failed to persist vector lane state [{}]: {:?}",
            worker_idx,
            state.as_str(),
            err
        );
    }
}

fn record_vector_worker_fault_and_lane_state(
    graph_store: &Arc<GraphStore>,
    worker_idx: usize,
    fault: FatalVectorWorkerFault,
    supervisor: VectorWorkerSupervisorState,
) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let provider = current_embedding_provider_diagnostics().provider_effective;
    let vram_used_mb = current_gpu_memory_snapshot()
        .map(|snapshot| snapshot.used_mb)
        .unwrap_or(0);
    let fault_id = format!("vector-fault-{}-{}-{}", worker_idx, fault.stage, now_ms);
    let persisted = VectorWorkerFault {
        fault_id: fault_id.clone(),
        lane: "vector".to_string(),
        worker_id: worker_idx as i64,
        fatal_stage: fault.stage.to_string(),
        fatal_reason_raw: fault.reason_raw.clone(),
        fatal_class: fault.fatal_class.clone(),
        provider,
        batch_id: fault.batch_id.clone(),
        texts_count: fault.texts_count,
        input_bytes: fault.input_bytes,
        vram_used_mb,
        occurred_at_ms: now_ms,
        restart_attempt: supervisor.restart_attempt,
    };
    if let Err(err) = graph_store.record_vector_worker_fault(&persisted) {
        error!(
            "Semantic Vector Worker [{}]: failed to persist vector fault {:?}: {:?}",
            worker_idx, persisted.fault_id, err
        );
    }
    let lane_state = if supervisor.crash_loop_open {
        VectorLaneState::Unhealthy
    } else {
        VectorLaneState::Degraded
    };
    persist_vector_lane_state(
        graph_store,
        lane_state,
        worker_idx,
        supervisor.restart_attempt,
        Some(fault.reason_raw),
        Some(fault_id.clone()),
    );
    fault_id
}

fn schedule_vector_worker_restart(
    graph_store: &Arc<GraphStore>,
    worker_idx: usize,
    fatal_fault: FatalVectorWorkerFault,
    restart_window: &mut VecDeque<i64>,
    restart_attempt: &mut u64,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let restart_window_ms = vector_worker_restart_window_ms();
    while restart_window
        .front()
        .is_some_and(|timestamp| now_ms.saturating_sub(*timestamp) > restart_window_ms)
    {
        restart_window.pop_front();
    }
    restart_window.push_back(now_ms);
    *restart_attempt = restart_attempt.saturating_add(1);
    service_guard::record_vector_worker_restart();
    let crash_loop_open = restart_window.len() > vector_worker_restart_budget();
    let supervisor = VectorWorkerSupervisorState {
        restart_attempt: *restart_attempt,
        crash_loop_open,
    };
    let fault_id = record_vector_worker_fault_and_lane_state(
        graph_store,
        worker_idx,
        fatal_fault.clone(),
        supervisor,
    );
    if crash_loop_open {
        error!(
            "Semantic Vector Worker [{}]: crash loop opened after fault {}, cooling down for {} ms",
            worker_idx,
            fault_id,
            vector_worker_crash_loop_cooldown_ms()
        );
        sleep_with_vector_worker_heartbeat(Duration::from_millis(
            vector_worker_crash_loop_cooldown_ms(),
        ));
        restart_window.clear();
        *restart_attempt = 0;
        persist_vector_lane_state(
            graph_store,
            VectorLaneState::Hold,
            worker_idx,
            0,
            Some("crash_loop_cooldown".to_string()),
            Some(fault_id),
        );
    } else {
        let backoff_ms = vector_worker_restart_backoff_ms(*restart_attempt);
        warn!(
            "Semantic Vector Worker [{}]: restarting after fatal {} fault in {} ms (attempt #{})",
            worker_idx, fatal_fault.fatal_class, backoff_ms, restart_attempt
        );
        sleep_with_vector_worker_heartbeat(Duration::from_millis(backoff_ms));
    }
}

/// Per REQ-AXO-128 / DEC-AXO-061, brain_only and indexer_graph profiles
/// no longer fail-fast on query-time embedding — they fall back to the
/// in-process CPU embedder (`cpu_query_service`). This function is now
/// only reached when the CPU fallback itself failed to load the ONNX
/// model OR when an indexer profile's GPU subprocess is starting up.
/// The wording reflects each case so the LLM client can decide whether
/// to retry (transient indexer worker boot) or report a config issue
/// (CPU embedder couldn't load the model file).
pub fn unavailable_embedding_reason(mode: crate::runtime_mode::AxonRuntimeMode) -> String {
    if mode.semantic_workers_enabled() {
        "Semantic fallback: MCP real-time embedding worker not yet available (transient). Retry, or fall back to structural search.".to_string()
    } else {
        format!(
            "Semantic fallback: in-process CPU query embedder unavailable in current profile ({}). Verify the model snapshot at runtime_embedding_snapshot_dir/onnx/model.onnx exists. Use structural search until the issue is resolved.",
            mode.as_str()
        )
    }
}

pub fn batch_embed(texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let pressure = service_guard::current_pressure();
    if !query_embedding_allowed(pressure) {
        return Err(anyhow::anyhow!(
            "MCP real-time embedding paused under {:?} service pressure. Use structural search.",
            pressure
        ));
    }

    // REQ-AXO-128 — under brain_only / indexer_graph the registered
    // sender belongs to the in-process CPU worker spawned at boot
    // (see cpu_query_service::spawn_brain_query_worker_if_needed).
    // Under indexer_vector / indexer_full the sender belongs to the
    // SemanticWorkerPool's GPU-backed worker. Either way, the routing
    // is uniform from this function's perspective.
    let Some(sender) = current_query_embedding_sender() else {
        return Err(anyhow::anyhow!(
            "{}",
            unavailable_embedding_reason(crate::runtime_mode::AxonRuntimeMode::from_env())
        ));
    };

    request_query_embedding(&sender, texts)
}

pub fn run_gpu_embed_subprocess_stdio() -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    let mut model = OrtGpuFirstTextEmbedding::try_new("vector-subprocess", 0, true)
        .map_err(|err| anyhow!("failed to initialize GPU embed subprocess model: {err}"))?;
    serde_json::to_writer(
        &mut stdout,
        &GpuEmbedSubprocessInit {
            ok: true,
            error: None,
        },
    )?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;

    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<GpuEmbedSubprocessRequest>(line.trim()) {
            Ok(request) => match embed_texts_with_breakdown_ort(&mut model, &request.texts) {
                Ok((
                    embeddings,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                )) => GpuEmbedSubprocessResponse {
                    ok: true,
                    embeddings: Some(embeddings),
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                    error: None,
                },
                Err(err) => GpuEmbedSubprocessResponse {
                    ok: false,
                    embeddings: None,
                    host_prepare_ms: 0,
                    input_copy_ms: 0,
                    inference_ms: 0,
                    output_extract_ms: 0,
                    error: Some(format!("{err:?}")),
                },
            },
            Err(err) => GpuEmbedSubprocessResponse {
                ok: false,
                embeddings: None,
                host_prepare_ms: 0,
                input_copy_ms: 0,
                inference_ms: 0,
                output_extract_ms: 0,
                error: Some(format!("failed to parse request: {err}")),
            },
        };
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}

// REQ-AXO-087 — sibling tests file (the TDD checker GUI-PRO-001 expects
// a `_tests.rs` companion path on diffs touching `src/axon-core/src/*`;
// inline `#[cfg(test)] mod tests` modules are not recognized by the
// path matcher per REQ-AXO-121). The companion holds focused tests for
// the `unavailable_embedding_reason` LLM-contract helper.
#[cfg(test)]
#[path = "embedder_tests.rs"]
mod embedder_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::{
        attach_preencoded_micro_batches, build_prepared_vector_embed_sequence,
        build_token_aware_micro_batches, build_vector_batch_plan, configured_embedding_max_length,
        configured_gpu_ready_high_watermark, configured_gpu_ready_low_watermark,
        configured_vector_prepare_queue_bound, continuous_prepare_feed_allowed,
        cuda_execution_provider_dispatch, current_runtime_tuning_snapshot,
        current_runtime_tuning_state, current_token_lane_thresholds,
        dispatch_prepared_vector_embed_sequence, effective_embedding_provider_is_gpu,
        embedding_download_progress_enabled, embedding_lane_config_from_env,
        embedding_model_cache_dir, embedding_provider_diagnostics,
        finalize_completed_vectorization_works, flush_completed_vectorization_works,
        gpu_memory_soft_limit_mb, gpu_primary_worker_max_used_mb, gpu_ready_queue_push_allowed,
        gpu_recycle_after_vram_summit_observe, gpu_secondary_worker_allowed,
        gpu_worker_consumption_allowed, gpu_worker_has_pending_work,
        gpu_worker_should_wait_for_ready, is_fatal_embedding_error,
        is_irrecoverable_outbox_finalize_error, load_runtime_embedding_tokenizer,
        merge_vectorization_work, observe_token_lane_thresholds, query_embedding_allowed,
        reconcile_outbox_finalize_failure, record_vector_frontier_metrics, replenish_target_chunks,
        replenish_target_ready_depth, request_query_embedding,
        reset_token_lane_classifier_for_tests, single_worker_gpu_prepare_worker_count,
        split_prepared_batch_by_lane, split_prepared_batch_for_gpu_budget,
        vector_prepare_prefetch_limits, vector_replenishment_command,
        wait_for_inflight_prepare_sequence, ClaimedLeaseSet, EmbeddingLaneConfig,
        GpuMemorySnapshot, PreparedBatchEnvelope, PreparedVectorEmbedBatch,
        PreparedVectorPrepareOutcome, QueryEmbeddingRequest, TokenLaneThresholdSource,
        TokenLaneThresholds, VectorBatchLane, VectorBatchPlan, VectorChunkWorkItem,
        VectorFinalizeRequest, VectorPrepareRequest, VectorRefillProducerState,
        VectorReplenishmentMode,
    };
    use crate::embedding_contract::{fastembed_model, CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH};
    use crate::graph_ingestion::{
        FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
        VectorPersistOutboxPayload,
    };
    use crate::service_guard;
    use crate::service_guard::{ServicePressure, VectorRuntimeMetrics};
    use crate::vector_control::{
        allowed_gpu_vector_workers, current_utility_first_scheduler_diagnostics,
        current_vector_batch_controller_diagnostics, current_vector_drain_state,
        graph_projection_allowed, reset_utility_first_scheduler_for_tests, semantic_policy,
        semantic_policy_with_graph, symbol_embedding_allowed, vector_claim_target,
        vector_embed_target_chunks, vector_ready_reserve_target, vector_worker_admitted,
        UtilityFirstSchedulerState, VectorBatchController, VectorBatchControllerObservation,
        VectorBatchControllerState, VectorDrainState, AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
        CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD, GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD,
        QUIET_CRUISE_FILE_BACKLOG_THRESHOLD, SYMBOL_BACKLOG_RESIDUAL_THRESHOLD,
    };
    use crate::vector_pipeline::{
        InflightPrepareRequest, PreparedBatchQueueSummary, SharedPreparedBatchQueue,
    };
    use crossbeam_channel::{bounded, unbounded};
    use fastembed::{InitOptions, TextEmbedding};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());

    fn lock_env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lane_test_prepared_batch(texts: &[&str]) -> PreparedVectorEmbedBatch {
        let mut prepared = PreparedVectorEmbedBatch {
            batch_id: "lane-test".to_string(),
            prepare_started_at_ms: 1,
            prepare_finished_at_ms: 1,
            prepared_at_ms: 1,
            batch_lane: VectorBatchLane::Mixed,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: texts
                .iter()
                .enumerate()
                .map(|(index, text)| VectorChunkWorkItem {
                    file_path: format!("/tmp/lane-{index}.rs"),
                    chunk_id: format!("chunk-{index}"),
                    content_hash: format!("hash-{index}"),
                    text: (*text).to_string(),
                })
                .collect(),
            texts: texts.iter().map(|text| (*text).to_string()).collect(),
            token_counts: Vec::new(),
            encoded_micro_batches: Vec::new(),
            touched_works: Vec::new(),
            finalize_after_success: Vec::new(),
            immediate_completed: Vec::new(),
            oversized_works: Vec::new(),
            next_active_after_success: Vec::new(),
            next_active_after_failure: Vec::new(),
            files_touched: 0,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: Vec::new(),
        };
        let tokenizer = load_runtime_embedding_tokenizer().expect("runtime tokenizer");
        attach_preencoded_micro_batches(&tokenizer, &mut prepared).expect("tokenized prepared");
        prepared
    }

    #[test]
    fn test_fatal_embedding_error_detection() {
        assert!(is_fatal_embedding_error(
            &"GetElementType is not implemented"
        ));
        assert!(is_fatal_embedding_error(&"onnxruntime failure"));
        assert!(!is_fatal_embedding_error(&"temporary timeout"));
    }

    #[test]
    fn test_semantic_policy_runs_when_system_is_healthy() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(8);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(100, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.profile, "balanced_drain");
        assert_eq!(policy.idle_sleep, Duration::from_millis(75));
    }

    #[test]
    fn test_semantic_policy_prefers_aggressive_drain_under_high_healthy_backlog() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(8);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(2_000, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.profile, "aggressive_drain");
        assert_eq!(policy.idle_sleep, Duration::from_millis(40));
    }

    #[test]
    fn test_graph_projection_allowed_under_queue_pressure_when_service_is_healthy() {
        reset_utility_first_scheduler_for_tests();
        assert!(graph_projection_allowed(
            2_000,
            ServicePressure::Healthy,
            0,
            false
        ));
    }

    #[test]
    fn test_graph_projection_disallowed_when_service_is_not_healthy() {
        assert!(!graph_projection_allowed(
            100,
            ServicePressure::Recovering,
            0,
            false
        ));
        assert!(!graph_projection_allowed(
            100,
            ServicePressure::Degraded,
            0,
            false
        ));
        assert!(!graph_projection_allowed(
            100,
            ServicePressure::Critical,
            0,
            false
        ));
    }

    #[test]
    fn test_graph_projection_ignores_large_vector_backlog_on_cpu_only_hosts() {
        reset_utility_first_scheduler_for_tests();
        assert!(graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
            false
        ));
    }

    #[test]
    fn test_graph_projection_can_run_with_large_vector_backlog_when_gpu_is_available() {
        reset_utility_first_scheduler_for_tests();
        assert!(graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
            true
        ));
    }

    #[test]
    fn test_graph_projection_ignores_large_vector_backlog_on_gpu_hosts_too() {
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        assert!(graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD,
            true
        ));
    }

    #[test]
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        let policy = semantic_policy(100, ServicePressure::Critical);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(2));
    }

    #[test]
    fn test_semantic_policy_throttles_without_pausing_when_service_is_degraded() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(4);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(100, ServicePressure::Degraded);
        assert!(!policy.pause);
        assert_eq!(policy.sleep, Duration::from_millis(50));
    }

    #[test]
    fn test_semantic_policy_stays_throttled_while_service_recovers() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(4);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(100, ServicePressure::Recovering);
        assert!(!policy.pause);
        assert_eq!(policy.sleep, Duration::from_millis(50));
        assert_eq!(policy.idle_sleep, Duration::from_millis(150));
    }

    #[test]
    fn test_query_embedding_allowed_while_healthy_or_recovering() {
        assert!(query_embedding_allowed(ServicePressure::Healthy));
        assert!(query_embedding_allowed(ServicePressure::Recovering));
    }

    #[test]
    fn test_query_embedding_disallowed_under_degraded_or_critical_pressure() {
        assert!(!query_embedding_allowed(ServicePressure::Degraded));
        assert!(!query_embedding_allowed(ServicePressure::Critical));
    }

    #[test]
    fn test_effective_embedding_provider_is_gpu_detects_cuda_variants() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cuda");
        }
        assert!(effective_embedding_provider_is_gpu());
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cuda_service");
        }
        assert!(effective_embedding_provider_is_gpu());
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "tensorrt_service");
        }
        assert!(effective_embedding_provider_is_gpu());

        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cpu");
        }
        assert!(!effective_embedding_provider_is_gpu());
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE");
        }
        assert!(!effective_embedding_provider_is_gpu());
    }

    #[test]
    fn test_gpu_service_provider_effective_label_tracks_tensorrt_toggle() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
        }
        assert_eq!(
            super::gpu_service_provider_effective_label(),
            "cuda_service"
        );

        unsafe {
            std::env::set_var("AXON_GPU_EMBED_SERVICE_TENSORRT", "1");
        }
        assert_eq!(
            super::gpu_service_provider_effective_label(),
            "tensorrt_service"
        );

        unsafe {
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
        }
    }

    #[test]
    fn test_vector_executor_strategy_preserves_provider_roles_without_new_lane() {
        use super::{vector_executor::VectorExecutorStrategy, ProviderStrategy};

        assert_eq!(
            VectorExecutorStrategy::CpuInProcess.provider_strategy(),
            ProviderStrategy::Cpu
        );
        assert_eq!(
            VectorExecutorStrategy::CudaInProcess.provider_strategy(),
            ProviderStrategy::Cuda
        );
        assert_eq!(
            VectorExecutorStrategy::CudaService.provider_strategy(),
            ProviderStrategy::Cuda
        );
        assert_eq!(
            VectorExecutorStrategy::TensorRtService.provider_strategy(),
            ProviderStrategy::TensorRt
        );
        assert_eq!(
            VectorExecutorStrategy::TensorRtService.label(),
            "tensorrt_service"
        );
    }

    #[test]
    fn test_apply_cpu_fallback_ort_runtime_env_restores_cpu_threads_when_autoconfigured() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_ORT_OMP_AUTOCONFIGURED", "true");
            std::env::set_var("OMP_NUM_THREADS", "1");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "1");
        }

        let adjustment = super::apply_cpu_fallback_ort_runtime_env();

        let cpu_threads = std::env::var("OMP_NUM_THREADS")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(cpu_threads >= 1);
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "PASSIVE");
        assert_eq!(
            std::env::var("AXON_ORT_INTRA_THREADS").unwrap(),
            cpu_threads.to_string()
        );
        assert!(adjustment
            .applied_env
            .iter()
            .any(|(name, _)| name == "OMP_NUM_THREADS"));
        assert!(adjustment
            .applied_env
            .iter()
            .any(|(name, _)| name == "AXON_ORT_INTRA_THREADS"));

        unsafe {
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
        }
    }

    #[test]
    fn test_apply_cpu_fallback_ort_runtime_env_preserves_explicit_openmp_configuration() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::set_var("OMP_NUM_THREADS", "3");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "5");
        }

        let adjustment = super::apply_cpu_fallback_ort_runtime_env();

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "3");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "ACTIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "5");
        assert!(adjustment.applied_env.is_empty());

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
        }
    }

    #[test]
    fn test_apply_cpu_fallback_ort_runtime_env_reports_advisory_lane_sizing_when_autoconfigured() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "64");
            std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "24");
            std::env::set_var("AXON_GRAPH_BATCH_SIZE", "8");
            std::env::set_var("AXON_VECTOR_WORKERS", "1");
            std::env::set_var("AXON_GRAPH_WORKERS", "0");
        }

        let adjustment = super::apply_cpu_fallback_ort_runtime_env();

        assert_eq!(std::env::var("AXON_CHUNK_BATCH_SIZE").unwrap(), "64");
        assert_eq!(
            std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE").unwrap(),
            "24"
        );
        assert_eq!(std::env::var("AXON_GRAPH_BATCH_SIZE").unwrap(), "8");
        assert_eq!(std::env::var("AXON_VECTOR_WORKERS").unwrap(), "1");
        assert_eq!(std::env::var("AXON_GRAPH_WORKERS").unwrap(), "0");
        assert!(!adjustment.applied_before_pool_start);
        assert!(adjustment
            .advisory_lane_sizing
            .iter()
            .any(|(name, value)| name == "AXON_CHUNK_BATCH_SIZE" && value == "16"));
        assert!(adjustment
            .advisory_lane_sizing
            .iter()
            .any(|(name, value)| name == "AXON_FILE_VECTORIZATION_BATCH_SIZE" && value == "8"));
        assert!(adjustment
            .advisory_lane_sizing
            .iter()
            .any(|(name, value)| name == "AXON_GRAPH_BATCH_SIZE" && value == "4"));

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GRAPH_WORKERS");
        }
    }

    #[test]
    fn test_semantic_policy_pauses_while_interactive_priority_is_active() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::mcp_request_started();
        let policy = semantic_policy(100, ServicePressure::Healthy);
        crate::service_guard::mcp_request_finished();
        assert!(!policy.pause);
        assert_eq!(policy.profile, "interactive_guarded");
        assert_eq!(policy.sleep, Duration::from_millis(750));
    }

    #[test]
    fn test_semantic_policy_respects_runtime_tuning_scale_pct() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        unsafe {
            std::env::set_var("AXON_SEMANTIC_SLEEP_SCALE_PCT", "200");
            std::env::set_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT", "200");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        crate::service_guard::record_vector_ready_queue_depth(8);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(100, ServicePressure::Healthy);
        assert_eq!(policy.profile, "balanced_drain");
        assert_eq!(policy.sleep, Duration::from_millis(50));
        assert_eq!(policy.idle_sleep, Duration::from_millis(150));

        unsafe {
            std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT");
            std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
    }

    #[test]
    fn test_runtime_tuning_normalizes_queue_and_persist_bounds() {
        let _guard = lock_env_guard();
        super::apply_runtime_embedding_lane_adjustment(
            None,
            None,
            None,
            None,
            Some(128),
            Some(24),
            Some(32),
            None,
            None,
            None,
            None,
        );
        let runtime_tuning = current_runtime_tuning_state();
        assert_eq!(runtime_tuning.vector_ready_queue_depth, 128);
        assert_eq!(runtime_tuning.vector_persist_queue_bound, 24);
        assert_eq!(runtime_tuning.vector_max_inflight_persists, 24);

        unsafe {
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
    }

    #[test]
    fn test_semantic_policy_prefers_drain_mode_when_mcp_is_idle() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(4);
        crate::service_guard::record_vector_prepare_inflight_depth(2);
        crate::service_guard::record_vector_ready_queue_chunks(512);
        crate::service_guard::record_vector_prepare_inflight_chunks(128);
        let policy = semantic_policy(2_000, ServicePressure::Recovering);
        assert!(
            !policy.pause,
            "idle MCP should allow semantic drain despite recovering pressure"
        );
        assert_eq!(policy.idle_sleep, Duration::from_millis(40));
    }

    #[test]
    fn test_vector_feed_backpressure_control_keeps_balanced_drain_even_with_graph_backlog() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(20);
        crate::service_guard::record_vector_prepare_inflight_depth(8);
        crate::service_guard::record_vector_ready_queue_chunks(20 * 16);
        crate::service_guard::record_vector_prepare_inflight_chunks(8 * 16);
        let diagnostics =
            current_utility_first_scheduler_diagnostics(32, 64, ServicePressure::Healthy);
        assert_eq!(diagnostics.state, UtilityFirstSchedulerState::BalancedDrain);
        assert!(!diagnostics.semantic_underfeed);
        assert_eq!(diagnostics.reason, "graph_backlog_observed");
        let policy = semantic_policy_with_graph(64, 32, ServicePressure::Healthy);
        assert_eq!(policy.profile, "balanced_drain");
        assert_eq!(policy.sleep, Duration::from_millis(25));
    }

    #[test]
    fn test_vector_feed_backpressure_control_prefers_refill_when_underfed_even_with_graph_backlog()
    {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(0);
        crate::service_guard::record_vector_prepare_inflight_depth(0);
        crate::service_guard::record_vector_ready_queue_chunks(0);
        crate::service_guard::record_vector_prepare_inflight_chunks(0);
        let diagnostics =
            current_utility_first_scheduler_diagnostics(256, 64, ServicePressure::Healthy);
        assert_eq!(diagnostics.state, UtilityFirstSchedulerState::BalancedDrain);
        assert!(diagnostics.semantic_underfeed);
        assert_eq!(diagnostics.reason, "semantic_underfed");
        let policy = semantic_policy_with_graph(64, 256, ServicePressure::Healthy);
        assert_eq!(policy.profile, "semantic_refill");
        assert_eq!(policy.sleep, Duration::from_millis(5));
    }

    #[test]
    fn test_vector_feed_backpressure_control_uses_full_ready_reserve_target_for_underfeed() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(20);
        crate::service_guard::record_vector_prepare_inflight_depth(0);
        crate::service_guard::record_vector_ready_queue_chunks(20);
        crate::service_guard::record_vector_prepare_inflight_chunks(0);
        crate::service_guard::record_vector_prepare_claimed(12);
        crate::service_guard::record_vector_ready_claimed(24);

        let diagnostics =
            current_utility_first_scheduler_diagnostics(0, 4_096, ServicePressure::Healthy);

        assert!(diagnostics.ready_reserve_target > 20, "{diagnostics:?}");
        assert!(diagnostics.semantic_underfeed, "{diagnostics:?}");
    }

    #[test]
    fn test_vector_feed_backpressure_control_does_not_let_graph_backlog_override_ready_supply() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        crate::service_guard::record_vector_ready_queue_depth(20);
        crate::service_guard::record_vector_prepare_inflight_depth(8);
        crate::service_guard::record_vector_ready_queue_chunks(20 * 16);
        crate::service_guard::record_vector_prepare_inflight_chunks(8 * 16);
        let policy = semantic_policy_with_graph(32, 1_024, ServicePressure::Healthy);
        assert_eq!(policy.profile, "balanced_drain");
        assert_eq!(policy.sleep, Duration::from_millis(25));
    }

    #[test]
    fn test_gpu_ready_queue_push_allowed_below_low_watermark() {
        assert!(gpu_ready_queue_push_allowed(4, 2, 16, 32));
    }

    #[test]
    fn test_gpu_ready_queue_push_blocked_at_high_watermark() {
        assert!(!gpu_ready_queue_push_allowed(32, 0, 16, 32));
    }

    #[test]
    fn test_gpu_ready_queue_push_allowed_while_total_supply_below_high_watermark() {
        assert!(gpu_ready_queue_push_allowed(12, 8, 16, 32));
    }

    #[test]
    fn test_gpu_ready_queue_push_allowed_uses_dynamic_target_depth() {
        assert!(gpu_ready_queue_push_allowed(24, 8, 16, 48));
    }

    #[test]
    fn test_current_vector_drain_state_prefers_interactive_guard() {
        let state =
            current_vector_drain_state(5_000, ServicePressure::Healthy, true, "cuda", "cuda");
        assert_eq!(state, VectorDrainState::InteractiveGuarded);
    }

    #[test]
    fn test_current_vector_drain_state_reports_gpu_scaling_blocked_on_cuda_fallback() {
        let state = current_vector_drain_state(256, ServicePressure::Healthy, false, "cuda", "cpu");
        assert_eq!(state, VectorDrainState::GpuScalingBlocked);
    }

    #[test]
    fn test_current_vector_drain_state_prefers_drain_for_non_empty_backlog() {
        let state = current_vector_drain_state(8, ServicePressure::Healthy, false, "cpu", "cpu");
        assert_eq!(state, VectorDrainState::AggressiveDrain);
    }

    #[test]
    fn test_current_vector_drain_state_prefers_quiet_when_backlog_is_empty() {
        let state =
            current_vector_drain_state(0, ServicePressure::Recovering, false, "cuda", "cuda");
        assert_eq!(state, VectorDrainState::QuietCruise);
    }

    #[test]
    fn test_symbol_embedding_allowed_only_as_residual_background_work() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        unsafe {
            std::env::remove_var("AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING");
        }
        assert!(!symbol_embedding_allowed(8, ServicePressure::Healthy));
        unsafe {
            std::env::set_var("AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING", "true");
        }
        assert!(symbol_embedding_allowed(8, ServicePressure::Healthy));
        assert!(!symbol_embedding_allowed(
            SYMBOL_BACKLOG_RESIDUAL_THRESHOLD,
            ServicePressure::Healthy
        ));
        crate::service_guard::mcp_request_started();
        assert!(!symbol_embedding_allowed(8, ServicePressure::Healthy));
        crate::service_guard::mcp_request_finished();
        unsafe {
            std::env::remove_var("AXON_VECTOR_ENABLE_SYMBOL_EMBEDDING");
        }
    }

    #[test]
    fn test_vector_worker_admission_throttles_gpu_background_under_pressure() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        assert!(vector_worker_admitted(
            0,
            ServicePressure::Critical,
            true,
            4_096
        ));
        assert!(!vector_worker_admitted(
            1,
            ServicePressure::Critical,
            true,
            4_096
        ));
        assert!(vector_worker_admitted(
            1,
            ServicePressure::Recovering,
            true,
            AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD
        ));
        assert!(vector_worker_admitted(
            1,
            ServicePressure::Healthy,
            true,
            QUIET_CRUISE_FILE_BACKLOG_THRESHOLD
        ));
    }

    #[test]
    fn test_gpu_secondary_worker_allowed_requires_free_vram_headroom() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_MULTIWORKER_MIN_FREE_MB", "768");
        }
        assert!(gpu_secondary_worker_allowed(0, None));
        assert!(gpu_secondary_worker_allowed(
            1,
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 7_000,
                free_mb: 900,
            })
        ));
        assert!(!gpu_secondary_worker_allowed(
            1,
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 7_500,
                free_mb: 256,
            })
        ));
        unsafe {
            std::env::remove_var("AXON_GPU_MULTIWORKER_MIN_FREE_MB");
        }
    }

    #[test]
    fn test_allowed_gpu_vector_workers_scales_only_for_meaningful_backlog() {
        assert_eq!(allowed_gpu_vector_workers(8, ServicePressure::Healthy), 2);
        assert_eq!(
            allowed_gpu_vector_workers(
                AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
                ServicePressure::Healthy
            ),
            6
        );
        assert_eq!(
            allowed_gpu_vector_workers(4_096, ServicePressure::Degraded),
            2
        );
    }

    #[test]
    fn test_vector_worker_admission_pauses_gpu_background_when_interactive_priority_is_active() {
        let _guard = lock_env_guard();
        crate::service_guard::reset_for_tests();
        crate::service_guard::mcp_request_started();
        assert!(vector_worker_admitted(
            0,
            ServicePressure::Healthy,
            true,
            4_096
        ));
        assert!(!vector_worker_admitted(
            1,
            ServicePressure::Healthy,
            true,
            4_096
        ));
        crate::service_guard::mcp_request_finished();
    }

    #[test]
    fn test_embedding_lane_config_from_env_supports_split_worker_counts() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_QUERY_EMBED_WORKERS", "2");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
            std::env::set_var("AXON_GRAPH_WORKERS", "0");
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "48");
            std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "24");
            std::env::set_var("AXON_GRAPH_BATCH_SIZE", "9");
            std::env::set_var("AXON_MAX_CHUNKS_PER_FILE", "12");
            std::env::set_var("AXON_MAX_EMBED_BATCH_BYTES", "65536");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_BATCH_SIZE");
            std::env::remove_var("AXON_MAX_CHUNKS_PER_FILE");
            std::env::remove_var("AXON_MAX_EMBED_BATCH_BYTES");
        }

        assert_eq!(config.query_workers, 2);
        assert_eq!(config.vector_workers, 5);
        assert_eq!(config.graph_workers, 0);
        assert_eq!(config.chunk_batch_size, 48);
        assert_eq!(config.file_vectorization_batch_size, 24);
        assert_eq!(config.graph_batch_size, 9);
        assert_eq!(config.max_chunks_per_file, 12);
        assert_eq!(config.max_embed_batch_bytes, 65536);
    }

    #[test]
    fn test_embedding_lane_config_caps_gpu_vector_workers_without_unsafe_opt_in() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::set_var("AXON_GPU_TOTAL_VRAM_MB_HINT", "24576");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GPU_TOTAL_VRAM_MB_HINT");
        }

        assert_eq!(config.vector_workers, 3);
    }

    #[test]
    fn test_embedding_lane_config_caps_gpu_vector_workers_to_one_on_8gb_vram() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::set_var("AXON_GPU_TOTAL_VRAM_MB_HINT", "8192");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_GPU_TOTAL_VRAM_MB_HINT");
        }

        assert_eq!(config.vector_workers, 1);
    }

    #[test]
    fn test_embedding_lane_config_disables_graph_workers_with_canonical_flag() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "false");
            std::env::set_var("AXON_GRAPH_WORKERS", "5");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
            std::env::remove_var("AXON_GRAPH_WORKERS");
        }

        assert_eq!(config.graph_workers, 0);
    }

    #[test]
    fn test_embedding_lane_config_disables_vector_workers_when_runtime_mode_skips_semantic_workers()
    {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_RUNTIME_MODE");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
        }

        assert_eq!(config.vector_workers, 0);
    }

    #[test]
    fn test_vector_lane_ignores_graph_backlog_when_graph_workers_disabled() {
        let lane_config = EmbeddingLaneConfig {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 0,
            chunk_batch_size: 32,
            file_vectorization_batch_size: 8,
            graph_batch_size: 8,
            max_chunks_per_file: 64,
            max_embed_batch_bytes: 4 * 1024 * 1024,
        };

        assert_eq!(
            super::effective_vector_lane_graph_backlog_depth(lane_config, 1_024),
            0
        );
    }

    #[test]
    fn test_vector_lane_keeps_graph_backlog_when_graph_workers_enabled() {
        let lane_config = EmbeddingLaneConfig {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 2,
            chunk_batch_size: 32,
            file_vectorization_batch_size: 8,
            graph_batch_size: 8,
            max_chunks_per_file: 64,
            max_embed_batch_bytes: 4 * 1024 * 1024,
        };

        assert_eq!(
            super::effective_vector_lane_graph_backlog_depth(lane_config, 1_024),
            1_024
        );
    }

    #[test]
    fn test_apply_runtime_embedding_lane_adjustment_updates_live_batch_env_and_controller() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "48");
            std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "12");
            std::env::set_var("AXON_GRAPH_WORKERS", "5");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
        }
        super::refresh_vector_batch_controller_from_env();

        super::apply_runtime_embedding_lane_adjustment(
            None,
            Some(2),
            Some(64),
            Some(16),
            Some(7),
            Some(3),
            Some(2),
            Some(72),
            Some(8_192),
            Some(80),
            Some(150),
        );

        let config = embedding_lane_config_from_env();
        let diagnostics = current_vector_batch_controller_diagnostics(&config);
        assert_eq!(config.chunk_batch_size, 64);
        assert_eq!(config.file_vectorization_batch_size, 16);
        assert_eq!(config.graph_workers, 2);
        let runtime_tuning = current_runtime_tuning_state();
        let runtime_snapshot = current_runtime_tuning_snapshot();
        assert_eq!(runtime_tuning.graph_workers, 2);
        assert_eq!(runtime_tuning.vector_ready_queue_depth, 7);
        assert_eq!(runtime_tuning.vector_persist_queue_bound, 3);
        assert_eq!(runtime_tuning.vector_max_inflight_persists, 2);
        assert_eq!(runtime_tuning.embed_micro_batch_max_items, 72);
        assert_eq!(runtime_tuning.embed_micro_batch_max_total_tokens, 8_192);
        assert_eq!(runtime_tuning.semantic_sleep_scale_pct, 80);
        assert_eq!(runtime_tuning.semantic_idle_sleep_scale_pct, 150);
        assert!(runtime_snapshot.version >= 2);
        assert_eq!(diagnostics.target_embed_batch_chunks, 64);
        assert_eq!(diagnostics.target_files_per_cycle, 16);
        assert_eq!(
            std::env::var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_GRAPH_WORKERS_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_SEMANTIC_SLEEP_SCALE_PCT_AUTOCONFIGURED").unwrap(),
            "false"
        );
        assert_eq!(
            std::env::var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT_AUTOCONFIGURED").unwrap(),
            "false"
        );

        unsafe {
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "96");
        }
        let config_after_env_override = embedding_lane_config_from_env();
        let runtime_tuning_after_env_override = current_runtime_tuning_state();
        assert_eq!(config_after_env_override.chunk_batch_size, 64);
        assert_eq!(runtime_tuning_after_env_override.chunk_batch_size, 64);

        unsafe {
            std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_GRAPH_WORKERS");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT");
            std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_GRAPH_WORKERS_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS_AUTOCONFIGURED");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS_AUTOCONFIGURED");
            std::env::remove_var("AXON_SEMANTIC_SLEEP_SCALE_PCT_AUTOCONFIGURED");
            std::env::remove_var("AXON_SEMANTIC_IDLE_SLEEP_SCALE_PCT_AUTOCONFIGURED");
        }
    }

    #[test]
    fn test_embedding_lane_config_allows_gpu_vector_worker_oversubscription_with_opt_in() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::set_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION", "true");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }

        assert_eq!(config.vector_workers, 5);
    }

    #[test]
    fn test_embedding_provider_diagnostics_reflects_requested_runtime() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("ORT_STRATEGY", "system");
            std::env::set_var("ORT_DYLIB_PATH", "/tmp/libonnxruntime.so");
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_GPU_EMBED_SERVICE_ENABLED", "1");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
        }
        let diagnostics = embedding_provider_diagnostics("cpu_fallback".to_string());
        unsafe {
            std::env::remove_var("ORT_STRATEGY");
            std::env::remove_var("ORT_DYLIB_PATH");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_ENABLED");
            std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
        }

        assert_eq!(diagnostics.provider_requested, "cuda");
        assert_eq!(diagnostics.provider_effective, "cpu_fallback");
        assert_eq!(diagnostics.ort_strategy, "system");
        assert_eq!(
            diagnostics.ort_dylib_path.as_deref(),
            Some("/tmp/libonnxruntime.so")
        );
        assert!(diagnostics.gpu_service_enabled);
        assert!(!diagnostics.gpu_service_tensorrt_requested);
    }

    #[test]
    fn test_configured_embedding_max_length_defaults_to_model_cap() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_EMBED_MAX_LENGTH");
        }

        assert_eq!(configured_embedding_max_length(), MAX_LENGTH);
    }

    #[test]
    fn test_configured_embedding_max_length_honors_lower_override_and_caps_high_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBED_MAX_LENGTH", "384");
        }
        assert_eq!(configured_embedding_max_length(), 384);

        unsafe {
            std::env::set_var("AXON_EMBED_MAX_LENGTH", "2048");
        }
        assert_eq!(configured_embedding_max_length(), MAX_LENGTH);

        unsafe {
            std::env::remove_var("AXON_EMBED_MAX_LENGTH");
        }
    }

    #[test]
    fn test_build_token_aware_micro_batches_respects_bucket_item_and_token_limits() {
        let micro_batches =
            build_token_aware_micro_batches(&[24, 28, 31, 95, 100, 220], 64, 2, 160);

        assert_eq!(
            micro_batches,
            vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5]]
        );
    }

    #[test]
    fn test_build_token_aware_micro_batches_keeps_neighbors_together_when_budget_allows() {
        let micro_batches =
            build_token_aware_micro_batches(&[40, 43, 47, 111, 118, 121], 64, 3, 384);

        assert_eq!(micro_batches, vec![vec![0, 1, 2], vec![3, 4, 5]]);
    }

    #[test]
    fn test_cuda_execution_provider_dispatch_is_strict() {
        let dispatch = cuda_execution_provider_dispatch();
        let rendered = format!("{dispatch:?}");

        assert!(
            rendered.contains("error_on_failure: true"),
            "CUDA EP dispatch should fail loudly so Axon can distinguish real CUDA from silent CPU fallback: {rendered}"
        );
    }

    #[test]
    fn test_cuda_execution_provider_dispatch_omits_tf32_option_by_default() {
        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }
        let dispatch = cuda_execution_provider_dispatch();
        let rendered = format!("{dispatch:?}");
        assert!(
            !rendered.contains("use_tf32"),
            "CUDA EP dispatch should not force use_tf32=0 by default: {rendered}"
        );
    }

    #[test]
    fn test_cuda_memory_limit_bytes_defaults_to_soft_limit_policy() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
            std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "6144");
        }
        assert_eq!(super::cuda_memory_limit_bytes(), 6_144 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
        }
    }

    #[test]
    fn test_cuda_memory_limit_bytes_respects_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_CUDA_MEMORY_LIMIT_MB", "2048");
        }
        assert_eq!(super::cuda_memory_limit_bytes(), 2_048 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
        }
    }

    #[test]
    fn test_cuda_memory_limit_bytes_ignores_too_small_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_CUDA_MEMORY_LIMIT_MB", "128");
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
            std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "6144");
        }
        assert_eq!(super::cuda_memory_limit_bytes(), 6_144 * 1024 * 1024);
        unsafe {
            std::env::remove_var("AXON_CUDA_MEMORY_LIMIT_MB");
            std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
        }
    }

    #[test]
    fn test_parse_nvidia_smi_memory_csv_parses_expected_format() {
        let snapshot = super::parse_nvidia_smi_memory_csv("8192, 2242, 5779").unwrap();
        assert_eq!(snapshot.total_mb, 8192);
        assert_eq!(snapshot.used_mb, 2242);
        assert_eq!(snapshot.free_mb, 5779);
    }

    #[test]
    fn test_gpu_memory_pressure_active_uses_soft_limit() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB", "3000");
        }
        assert!(!super::gpu_memory_pressure_active(
            super::GpuMemorySnapshot {
                total_mb: 8192,
                used_mb: 2242,
                free_mb: 5779,
            }
        ));
        assert!(super::gpu_memory_pressure_active(
            super::GpuMemorySnapshot {
                total_mb: 8192,
                used_mb: 3644,
                free_mb: 4377,
            }
        ));
        unsafe {
            std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
        }
    }

    #[test]
    fn test_parse_nvidia_smi_utilization_csv_normalizes_percentages() {
        let parsed = super::parse_nvidia_smi_utilization_csv("31, 2").expect("parsed");
        assert!((parsed.gpu_utilization_ratio - 0.31).abs() < f64::EPSILON);
        assert!((parsed.memory_utilization_ratio - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn test_vector_stale_inflight_recovery_claim_age_ms_uses_env_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS", "45000");
        }

        assert_eq!(super::vector_stale_inflight_claim_age_ms(), 45_000);

        unsafe {
            std::env::remove_var("AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS");
        }
    }

    #[test]
    fn test_vector_stale_inflight_recovery_interval_ms_uses_env_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS", "15000");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "100");
        }

        assert_eq!(super::vector_stale_inflight_recovery_interval_ms(), 15_000);

        unsafe {
            std::env::remove_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_vector_stale_inflight_recovery_interval_ms_scales_in_quiescent_mode() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS", "15000");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "400");
        }

        assert!(
            super::vector_stale_inflight_recovery_interval_ms() >= 15_000,
            "quiescent scaling should not reduce the maintenance interval"
        );

        unsafe {
            std::env::remove_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_vector_finalize_idle_poll_interval_ms_uses_env_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS", "500");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "100");
        }

        assert_eq!(super::vector_finalize_idle_poll_interval_ms(), 500);

        unsafe {
            std::env::remove_var("AXON_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_vector_finalize_idle_poll_interval_ms_scales_in_quiescent_mode() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS", "250");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "400");
        }

        assert!(
            super::vector_finalize_idle_poll_interval_ms() >= 250,
            "quiescent scaling should not reduce finalize idle polling"
        );

        unsafe {
            std::env::remove_var("AXON_VECTOR_FINALIZE_IDLE_POLL_INTERVAL_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_vector_worker_non_admitted_idle_wait_ms_uses_env_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_NON_ADMITTED_IDLE_WAIT_MS", "30000");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "100");
        }

        assert_eq!(super::vector_worker_non_admitted_idle_wait_ms(0), 30_000);

        unsafe {
            std::env::remove_var("AXON_VECTOR_NON_ADMITTED_IDLE_WAIT_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_vector_worker_non_admitted_backlog_wait_ms_uses_env_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_NON_ADMITTED_BACKLOG_WAIT_MS", "750");
            std::env::set_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT", "100");
        }

        assert_eq!(super::vector_worker_non_admitted_backlog_wait_ms(5), 750);

        unsafe {
            std::env::remove_var("AXON_VECTOR_NON_ADMITTED_BACKLOG_WAIT_MS");
            std::env::remove_var("AXON_QUIESCENT_INTERVAL_SCALE_PCT");
        }
    }

    #[test]
    fn test_recover_stale_vector_inflight_now_uses_configured_claim_age() {
        let _guard = lock_env_guard();
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms) VALUES \
                 ('/tmp/stale-helper.rs', 'inflight', 1, 'claim-stale', 1_000), \
                 ('/tmp/fresh-helper.rs', 'inflight', 2, 'claim-fresh', 9_500)",
            )
            .unwrap();
        unsafe {
            std::env::set_var("AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS", "1000");
        }

        let recovered = super::recover_stale_vector_inflight_now(&store, 10_000).unwrap();

        assert_eq!(recovered, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/stale-helper.rs' \
                       AND status = 'queued' \
                       AND status_reason = 'recovered_after_stale_inflight'"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/fresh-helper.rs' \
                       AND status = 'inflight'"
                )
                .unwrap(),
            1
        );

        unsafe {
            std::env::remove_var("AXON_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS");
        }
    }

    #[test]
    fn test_gpu_telemetry_backend_defaults_to_nvidia_smi() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
        }
        assert_eq!(super::gpu_telemetry_backend_name(), "nvidia-smi");
    }

    #[test]
    fn test_gpu_telemetry_backend_allows_disabling_collection() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_BACKEND", "none");
        }
        assert_eq!(super::gpu_telemetry_backend_name(), "none");
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
        }
    }

    #[test]
    fn test_gpu_telemetry_backend_supports_nvml() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_BACKEND", "nvml");
        }
        assert_eq!(super::gpu_telemetry_backend_name(), "nvml");
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
        }
    }

    #[test]
    fn test_gpu_telemetry_command_respects_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_COMMAND", "/tmp/custom-nvidia-smi");
        }
        assert_eq!(super::gpu_telemetry_command(), "/tmp/custom-nvidia-smi");
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_COMMAND");
        }
    }

    #[test]
    fn test_nvml_library_path_defaults_to_wsl_location() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_NVML_LIBRARY_PATH");
        }
        assert_eq!(
            super::nvml_library_path(),
            "/usr/lib/wsl/lib/libnvidia-ml.so.1"
        );
    }

    #[test]
    fn test_nvml_library_path_respects_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_NVML_LIBRARY_PATH", "/tmp/libnvidia-ml.so.1");
        }
        assert_eq!(super::nvml_library_path(), "/tmp/libnvidia-ml.so.1");
        unsafe {
            std::env::remove_var("AXON_NVML_LIBRARY_PATH");
        }
    }

    #[test]
    fn test_gpu_telemetry_device_index_defaults_to_zero() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_DEVICE_INDEX");
        }
        assert_eq!(super::gpu_telemetry_device_index(), 0);
    }

    #[test]
    fn test_gpu_telemetry_device_index_respects_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_DEVICE_INDEX", "2");
        }
        assert_eq!(super::gpu_telemetry_device_index(), 2);
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_DEVICE_INDEX");
        }
    }

    #[test]
    fn test_gpu_telemetry_cache_ttl_ms_defaults_to_2000() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS");
        }
        assert_eq!(super::gpu_telemetry_cache_ttl_ms(), 2_000);
    }

    #[test]
    fn test_gpu_telemetry_cache_ttl_ms_respects_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS", "750");
        }
        assert_eq!(super::gpu_telemetry_cache_ttl_ms(), 750);
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS");
        }
    }

    #[test]
    #[ignore = "manual runtime probe for FastEmbed CUDA init parity"]
    fn manual_fastembed_cuda_init_matches_direct_ort_probe() {
        let cache_dir = super::embedding_model_cache_dir();
        let options = InitOptions::new(fastembed_model())
            .with_cache_dir(cache_dir)
            .with_show_download_progress(false)
            .with_max_length(MAX_LENGTH)
            .with_execution_providers(vec![cuda_execution_provider_dispatch()]);

        let model = TextEmbedding::try_new(options);
        assert!(
            model.is_ok(),
            "FastEmbed CUDA init should succeed under the same ORT runtime as the direct probe: {:?}",
            model.err()
        );
    }

    #[test]
    fn test_ort_cuda_provider_library_path_uses_ort_dylib_directory() {
        unsafe {
            std::env::set_var(
                "ORT_DYLIB_PATH",
                "/nix/store/test-onnxruntime/lib/libonnxruntime.so",
            );
        }
        let provider_path = super::ort_cuda_provider_library_path();
        unsafe {
            std::env::remove_var("ORT_DYLIB_PATH");
        }

        assert_eq!(
            provider_path,
            Some(PathBuf::from(
                "/nix/store/test-onnxruntime/lib/libonnxruntime_providers_cuda.so"
            ))
        );
    }

    #[test]
    fn test_ort_cuda_provider_library_available_is_false_without_provider_binary() {
        let tempdir = tempfile::tempdir().unwrap();
        let ort_dir = tempdir.path().join("lib");
        std::fs::create_dir_all(&ort_dir).unwrap();
        std::fs::write(ort_dir.join("libonnxruntime.so"), b"placeholder").unwrap();

        unsafe {
            std::env::set_var(
                "ORT_DYLIB_PATH",
                ort_dir.join("libonnxruntime.so").display().to_string(),
            );
        }
        let available = super::ort_cuda_provider_library_available();
        unsafe {
            std::env::remove_var("ORT_DYLIB_PATH");
        }

        assert!(!available);
    }

    #[test]
    fn test_cpu_provider_effective_label_exposes_missing_cuda_provider() {
        assert_eq!(
            super::cpu_provider_effective_label(true, true, false),
            "cpu_missing_cuda_provider"
        );
        assert_eq!(
            super::cpu_provider_effective_label(true, false, false),
            "cpu"
        );
        assert_eq!(
            super::cpu_provider_effective_label(false, true, false),
            "cpu"
        );
        assert_eq!(super::cpu_provider_effective_label(true, true, true), "cpu");
    }

    #[test]
    fn test_effective_provider_request_for_query_lane_defaults_to_cpu_when_global_cuda_is_requested(
    ) {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
        }

        let provider = super::effective_provider_request_for_lane("query");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(provider, "cpu");
    }

    #[test]
    fn test_effective_provider_request_for_query_lane_respects_explicit_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_QUERY_EMBED_PROVIDER", "cuda");
        }

        let provider = super::effective_provider_request_for_lane("query");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
        }

        assert_eq!(provider, "cuda");
    }

    #[test]
    fn test_effective_provider_request_for_vector_lane_keeps_global_cuda_request() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
        }

        let provider = super::effective_provider_request_for_lane("vector");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(provider, "cuda");
    }

    #[test]
    fn test_effective_provider_request_for_graph_lane_respects_explicit_override() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_GRAPH_EMBED_PROVIDER", "cpu");
        }

        let provider = super::effective_provider_request_for_lane("graph");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_GRAPH_EMBED_PROVIDER");
        }

        assert_eq!(provider, "cpu");
    }

    #[test]
    fn test_embedding_model_cache_dir_defaults_outside_workspace() {
        // REQ-AXO-099 Phase 4 — env_test_lock + EnvVarGuard so the
        // mutated env vars are restored on Drop (panic-safe). The
        // prior `std::env::remove_var("HOME")` at the end of this
        // test was unconditionally clearing HOME — DuckDB's INSTALL
        // json then could not find the extension cache, causing 18
        // unrelated tests to fail in the suite. This is the root
        // cause documented in REQ-AXO-099 Phase 4.
        let _lock = crate::test_support::env_test_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _g_fastembed = crate::test_support::EnvVarGuard::unset("FASTEMBED_CACHE_DIR");
        let _g_xdg = crate::test_support::EnvVarGuard::unset("XDG_CACHE_HOME");
        let _g_home = crate::test_support::EnvVarGuard::set("HOME", "/tmp/axon-home");

        let cache_dir = embedding_model_cache_dir();

        assert_eq!(
            cache_dir,
            PathBuf::from("/tmp/axon-home/.cache/axon/fastembed")
        );
    }

    #[test]
    fn test_embedding_download_progress_disabled_by_default() {
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_DOWNLOAD_PROGRESS");
        }

        assert!(!embedding_download_progress_enabled());

        unsafe {
            std::env::set_var("AXON_EMBEDDING_DOWNLOAD_PROGRESS", "true");
        }

        assert!(embedding_download_progress_enabled());

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_DOWNLOAD_PROGRESS");
        }
    }

    #[test]
    fn test_cuda_tf32_enabled_defaults_to_false() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }

        assert!(!super::cuda_tf32_enabled());
    }

    #[test]
    fn test_cuda_tf32_enabled_honors_explicit_disable() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_CUDA_ALLOW_TF32", "false");
        }
        assert!(!super::cuda_tf32_enabled());

        unsafe {
            std::env::set_var("AXON_CUDA_ALLOW_TF32", "0");
        }
        assert!(!super::cuda_tf32_enabled());

        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }
    }

    #[test]
    fn test_cuda_tf32_enabled_honors_explicit_enable() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_CUDA_ALLOW_TF32", "true");
        }
        assert!(super::cuda_tf32_enabled());

        unsafe {
            std::env::set_var("AXON_CUDA_ALLOW_TF32", "1");
        }
        assert!(super::cuda_tf32_enabled());

        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }
    }

    #[test]
    fn test_vector_embed_target_chunks_prefers_larger_batches_when_mcp_is_idle() {
        let lane_config = EmbeddingLaneConfig {
            query_workers: 1,
            vector_workers: 2,
            graph_workers: 0,
            chunk_batch_size: 64,
            file_vectorization_batch_size: 24,
            graph_batch_size: 8,
            max_chunks_per_file: 64,
            max_embed_batch_bytes: 512 * 1024,
        };

        assert_eq!(vector_embed_target_chunks(&lane_config, true), 64);
        assert_eq!(vector_embed_target_chunks(&lane_config, false), 64);
    }

    #[test]
    fn test_vector_batch_plan_advances_multiple_files_and_tracks_partial_cycles() {
        let work_a = FileVectorizationWork {
            file_path: "src/a.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let work_b = FileVectorizationWork {
            file_path: "src/b.rs".to_string(),
            resumed_after_interactive_pause: false,
        };

        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a2".to_string(),
                content_hash: "ha2".to_string(),
                text: "A2".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/b.rs".to_string(),
                chunk_id: "b1".to_string(),
                content_hash: "hb1".to_string(),
                text: "B1".to_string(),
            },
        ];
        plan.touched_works = vec![work_a.clone(), work_b.clone()];
        plan.continuation_works = vec![work_a.clone()];
        plan.finalize_after_success = vec![work_b.clone()];
        plan.files_touched = 2;
        plan.partial_file_cycles = 1;

        let next = plan.next_active_after_success();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].file_path, "src/a.rs");
        assert_eq!(next[0].resumed_after_interactive_pause, false);
    }

    #[test]
    fn test_fetch_segments_for_file_returns_all_segments_in_line_order() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-b', 'symbol', 'sym-b', 'PRJ', '/tmp/segments.rs', 'function', 'B', 'hash-b', 20, 30), \
                 ('chunk-a', 'symbol', 'sym-a', 'PRJ', '/tmp/segments.rs', 'function', 'A', 'hash-a', 5, 10), \
                 ('chunk-c', 'symbol', 'sym-c', 'PRJ', '/tmp/segments.rs', 'function', 'C', 'hash-c', 31, 40)",
            )
            .unwrap();

        let chunks = store.fetch_segments_for_file("/tmp/segments.rs").unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, "chunk-a");
        assert_eq!(chunks[1].0, "chunk-b");
        assert_eq!(chunks[2].0, "chunk-c");
    }

    #[test]
    fn test_build_vector_batch_plan_caps_first_file_to_remaining_chunk_budget() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/a.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'PRJ', '/tmp/a.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'PRJ', '/tmp/a.rs', 'function', 'A3', 'hash-a3', 5, 6), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'PRJ', '/tmp/b.rs', 'function', 'B1', 'hash-b1', 1, 2)",
            )
            .unwrap();

        let active_works = vec![
            FileVectorizationWork {
                file_path: "/tmp/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let plan =
            build_vector_batch_plan(&store, &active_works, 2, 64, 512 * 1024, &HashSet::new());
        assert_eq!(plan.work_items.len(), 2);
        assert_eq!(plan.touched_works.len(), 1);
        assert_eq!(plan.touched_works[0].file_path, "/tmp/a.rs");
        assert!(plan.finalize_after_success.is_empty());
        assert_eq!(plan.untouched_works.len(), 1);
        assert_eq!(plan.untouched_works[0].file_path, "/tmp/b.rs");
        assert_eq!(plan.partial_file_cycles, 1);
        assert_eq!(plan.continuation_works.len(), 1);
        assert_eq!(plan.continuation_works[0].file_path, "/tmp/a.rs");
        assert_eq!(plan.work_items[0].chunk_id, "chunk-a1");
        assert_eq!(plan.work_items[1].chunk_id, "chunk-a2");
    }

    #[test]
    fn test_build_vector_batch_plan_skips_oversized_intermediate_file_to_fill_budget() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/a.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'PRJ', '/tmp/a.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'PRJ', '/tmp/a.rs', 'function', 'A3', 'hash-a3', 5, 6), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'PRJ', '/tmp/b.rs', 'function', 'B1', 'hash-b1', 1, 2), \
                 ('chunk-b2', 'symbol', 'sym-b2', 'PRJ', '/tmp/b.rs', 'function', 'B2', 'hash-b2', 3, 4), \
                 ('chunk-b3', 'symbol', 'sym-b3', 'PRJ', '/tmp/b.rs', 'function', 'B3', 'hash-b3', 5, 6), \
                 ('chunk-c1', 'symbol', 'sym-c1', 'PRJ', '/tmp/c.rs', 'function', 'C1', 'hash-c1', 1, 2)",
            )
            .unwrap();

        let active_works = vec![
            FileVectorizationWork {
                file_path: "/tmp/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/c.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let plan =
            build_vector_batch_plan(&store, &active_works, 4, 64, 512 * 1024, &HashSet::new());
        assert_eq!(plan.work_items.len(), 4);
        assert_eq!(plan.touched_works.len(), 2);
        assert_eq!(plan.touched_works[0].file_path, "/tmp/a.rs");
        assert_eq!(plan.touched_works[1].file_path, "/tmp/b.rs");
        assert_eq!(plan.finalize_after_success.len(), 1);
        assert_eq!(plan.finalize_after_success[0].file_path, "/tmp/a.rs");
        assert_eq!(plan.untouched_works.len(), 1);
        assert_eq!(plan.untouched_works[0].file_path, "/tmp/c.rs");
        assert_eq!(plan.partial_file_cycles, 1);
        assert_eq!(plan.continuation_works.len(), 1);
        assert_eq!(plan.continuation_works[0].file_path, "/tmp/b.rs");
    }

    #[test]
    fn test_build_vector_batch_plan_respects_per_file_fetch_limit() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/limited.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'PRJ', '/tmp/limited.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'PRJ', '/tmp/limited.rs', 'function', 'A3', 'hash-a3', 5, 6)",
            )
            .unwrap();
        let active_works = vec![FileVectorizationWork {
            file_path: "/tmp/limited.rs".to_string(),
            resumed_after_interactive_pause: false,
        }];

        let plan =
            build_vector_batch_plan(&store, &active_works, 16, 2, 512 * 1024, &HashSet::new());
        assert_eq!(plan.work_items.len(), 2);
        assert!(plan.finalize_after_success.is_empty());
        assert_eq!(plan.continuation_works.len(), 1);
        assert_eq!(plan.partial_file_cycles, 1);
    }

    #[test]
    fn test_build_vector_batch_plan_finalizes_only_when_file_is_fully_covered() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-b1', 'symbol', 'sym-b1', 'PRJ', '/tmp/complete.rs', 'function', 'B1', 'hash-b1', 1, 2), \
                 ('chunk-b2', 'symbol', 'sym-b2', 'PRJ', '/tmp/complete.rs', 'function', 'B2', 'hash-b2', 3, 4)",
            )
            .unwrap();
        let active_works = vec![FileVectorizationWork {
            file_path: "/tmp/complete.rs".to_string(),
            resumed_after_interactive_pause: false,
        }];

        let plan =
            build_vector_batch_plan(&store, &active_works, 16, 8, 512 * 1024, &HashSet::new());
        assert_eq!(plan.work_items.len(), 2);
        assert_eq!(plan.finalize_after_success.len(), 1);
        assert!(plan.continuation_works.is_empty());
        assert_eq!(plan.partial_file_cycles, 0);
    }

    #[test]
    fn test_build_vector_batch_plan_respects_batch_byte_budget_after_first_file() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/bytes-a.rs', 'function', 'AAAA', 'hash-a1', 1, 2), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'PRJ', '/tmp/bytes-b.rs', 'function', 'BBBB', 'hash-b1', 1, 2)",
            )
            .unwrap();
        let active_works = vec![
            FileVectorizationWork {
                file_path: "/tmp/bytes-a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/bytes-b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let plan = build_vector_batch_plan(&store, &active_works, 16, 16, 4, &HashSet::new());
        assert_eq!(plan.work_items.len(), 1);
        assert_eq!(plan.finalize_after_success.len(), 1);
        assert_eq!(plan.untouched_works.len(), 1);
        assert_eq!(plan.untouched_works[0].file_path, "/tmp/bytes-b.rs");
    }

    #[test]
    fn test_build_vector_batch_plan_only_fetches_unembedded_chunks() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/vectorized.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'PRJ', '/tmp/vectorized.rs', 'function', 'A2', 'hash-a2', 3, 4)",
            )
            .unwrap();
        store
            .update_chunk_embeddings(
                CHUNK_MODEL_ID,
                &[(
                    "chunk-a1".to_string(),
                    "hash-a1".to_string(),
                    vec![0.0_f32; DIMENSION],
                )],
            )
            .unwrap();

        let active_works = vec![FileVectorizationWork {
            file_path: "/tmp/vectorized.rs".to_string(),
            resumed_after_interactive_pause: false,
        }];

        let plan =
            build_vector_batch_plan(&store, &active_works, 16, 16, 512 * 1024, &HashSet::new());
        assert_eq!(plan.work_items.len(), 1);
        assert_eq!(plan.work_items[0].chunk_id, "chunk-a2");
        assert_eq!(plan.finalize_after_success.len(), 1);
    }

    #[test]
    fn test_merge_vectorization_work_caps_growth_and_avoids_duplicate_paths() {
        let existing = vec![FileVectorizationWork {
            file_path: "src/a.rs".to_string(),
            resumed_after_interactive_pause: false,
        }];
        let additional = vec![
            FileVectorizationWork {
                file_path: "src/a.rs".to_string(),
                resumed_after_interactive_pause: true,
            },
            FileVectorizationWork {
                file_path: "src/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "src/c.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let merged = merge_vectorization_work(existing, additional, 2);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].file_path, "src/a.rs");
        assert_eq!(merged[1].file_path, "src/b.rs");
    }

    #[test]
    fn test_prepared_vector_embed_batch_from_plan_preserves_texts_and_work_state() {
        let work_a = FileVectorizationWork {
            file_path: "src/a.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let work_b = FileVectorizationWork {
            file_path: "src/b.rs".to_string(),
            resumed_after_interactive_pause: true,
        };
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/b.rs".to_string(),
                chunk_id: "b1".to_string(),
                content_hash: "hb1".to_string(),
                text: "B1".to_string(),
            },
        ];
        plan.touched_works = vec![work_a.clone(), work_b.clone()];
        plan.finalize_after_success = vec![work_b.clone()];
        plan.continuation_works = vec![work_a.clone()];
        plan.untouched_works = vec![work_b.clone()];
        plan.immediate_completed = vec![work_a.clone()];
        plan.files_touched = 2;

        let prepared = PreparedVectorEmbedBatch::from_plan(plan);

        assert_eq!(prepared.texts, vec!["A1".to_string(), "B1".to_string()]);
        assert_eq!(prepared.files_touched, 2);
        assert_eq!(prepared.touched_works.len(), 2);
        assert_eq!(prepared.finalize_after_success.len(), 1);
        assert_eq!(prepared.immediate_completed.len(), 1);
        assert_eq!(prepared.next_active_after_success.len(), 2);
        assert_eq!(prepared.next_active_after_failure.len(), 1);
    }

    #[test]
    fn test_prepared_vector_embed_batch_into_persist_plan_zips_embeddings_and_completions() {
        let work_a = FileVectorizationWork {
            file_path: "src/a.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let work_b = FileVectorizationWork {
            file_path: "src/b.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/b.rs".to_string(),
                chunk_id: "b1".to_string(),
                content_hash: "hb1".to_string(),
                text: "B1".to_string(),
            },
        ];
        plan.touched_works = vec![work_a.clone(), work_b.clone()];
        plan.finalize_after_success = vec![work_b.clone()];
        plan.immediate_completed = vec![work_a.clone()];
        plan.continuation_works = vec![work_a.clone()];

        let prepared = PreparedVectorEmbedBatch::from_plan(plan);
        let batch_run = VectorBatchRun {
            run_id: "persist-plan-test".to_string(),
            prepare_started_at_ms: 0,
            prepare_finished_at_ms: 0,
            ready_enqueued_at_ms: 0,
            started_at_ms: 1,
            finished_at_ms: 1,
            gpu_started_at_ms: 0,
            gpu_finished_at_ms: 0,
            persist_enqueued_at_ms: 0,
            persist_started_at_ms: 0,
            persist_finished_at_ms: 0,
            finalize_enqueued_at_ms: 0,
            finalize_finished_at_ms: 0,
            provider: "cpu".to_string(),
            runner_kind: "test".to_string(),
            model_id: CHUNK_MODEL_ID.to_string(),
            chunk_count: 2,
            file_count: 2,
            input_bytes: 4,
            total_tokens: 0,
            max_item_tokens: 0,
            avg_item_tokens: 0.0,
            micro_batch_count: 0,
            max_micro_batch_tokens: 0,
            avg_micro_batch_tokens: 0.0,
            effective_vector_workers_admitted: 0,
            ready_queue_depth_at_gpu_start: 0,
            prepare_inflight_at_gpu_start: 0,
            ready_queue_chunks_at_gpu_start: 0,
            prepare_inflight_chunks_at_gpu_start: 0,
            vector_worker_admission_reason: String::new(),
            allowed_gpu_workers: 0,
            batch_wait_for_ready_ms: 0,
            persist_queue_wait_ms: 0,
            finalize_queue_wait_ms: 0,
            batch_lane: "mixed".to_string(),
            batch_shape: "homogeneous".to_string(),
            lane_small_max_tokens: 0,
            lane_medium_max_tokens: 0,
            fetch_ms: 0,
            embed_ms: 0,
            db_write_ms: 0,
            mark_done_ms: 0,
            success: true,
            error_reason: None,
        };
        let persist = prepared
            .into_persist_plan(
                vec![vec![0.1_f32, 0.2_f32], vec![0.3_f32, 0.4_f32]],
                batch_run,
            )
            .expect("persist plan");

        assert_eq!(persist.updates.len(), 2);
        assert_eq!(persist.updates[0].0, "a1");
        assert_eq!(persist.updates[0].1, "ha1");
        assert_eq!(persist.updates[0].2, vec![0.1_f32, 0.2_f32]);
        assert_eq!(persist.completed_works.len(), 2);
        assert_eq!(persist.completed_works[0].file_path, "src/a.rs");
        assert_eq!(persist.completed_works[1].file_path, "src/b.rs");
        assert!(persist.next_active_after_failure.is_empty());
    }

    #[test]
    fn test_prepared_vector_embed_batch_chunk_count_matches_work_items() {
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a2".to_string(),
                content_hash: "ha2".to_string(),
                text: "A2".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a3".to_string(),
                content_hash: "ha3".to_string(),
                text: "A3".to_string(),
            },
        ];

        let prepared = PreparedVectorEmbedBatch::from_plan(plan);

        assert_eq!(prepared.chunk_count(), 3);
    }

    #[test]
    fn test_prepared_vector_embed_batch_rejects_embedding_count_mismatch() {
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![VectorChunkWorkItem {
            file_path: "src/a.rs".to_string(),
            chunk_id: "a1".to_string(),
            content_hash: "ha1".to_string(),
            text: "A1".to_string(),
        }];

        let prepared = PreparedVectorEmbedBatch::from_plan(plan);
        let err = prepared
            .into_persist_plan(
                Vec::new(),
                VectorBatchRun {
                    run_id: "persist-mismatch-test".to_string(),
                    prepare_started_at_ms: 0,
                    prepare_finished_at_ms: 0,
                    ready_enqueued_at_ms: 0,
                    started_at_ms: 1,
                    finished_at_ms: 1,
                    gpu_started_at_ms: 0,
                    gpu_finished_at_ms: 0,
                    persist_enqueued_at_ms: 0,
                    persist_started_at_ms: 0,
                    persist_finished_at_ms: 0,
                    finalize_enqueued_at_ms: 0,
                    finalize_finished_at_ms: 0,
                    provider: "cpu".to_string(),
                    runner_kind: "test".to_string(),
                    model_id: CHUNK_MODEL_ID.to_string(),
                    chunk_count: 1,
                    file_count: 1,
                    input_bytes: 2,
                    total_tokens: 0,
                    max_item_tokens: 0,
                    avg_item_tokens: 0.0,
                    micro_batch_count: 0,
                    max_micro_batch_tokens: 0,
                    avg_micro_batch_tokens: 0.0,
                    effective_vector_workers_admitted: 0,
                    ready_queue_depth_at_gpu_start: 0,
                    prepare_inflight_at_gpu_start: 0,
                    ready_queue_chunks_at_gpu_start: 0,
                    prepare_inflight_chunks_at_gpu_start: 0,
                    vector_worker_admission_reason: String::new(),
                    allowed_gpu_workers: 0,
                    batch_wait_for_ready_ms: 0,
                    persist_queue_wait_ms: 0,
                    finalize_queue_wait_ms: 0,
                    batch_lane: "mixed".to_string(),
                    batch_shape: "homogeneous".to_string(),
                    lane_small_max_tokens: 0,
                    lane_medium_max_tokens: 0,
                    fetch_ms: 0,
                    embed_ms: 0,
                    db_write_ms: 0,
                    mark_done_ms: 0,
                    success: true,
                    error_reason: None,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("embedding count mismatch"));
    }

    #[test]
    fn test_persist_envelope_syncs_batch_run_counts_for_outbox_and_runtime() {
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![
            VectorChunkWorkItem {
                file_path: "src/a.rs".to_string(),
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                file_path: "src/b.rs".to_string(),
                chunk_id: "b1".to_string(),
                content_hash: "hb1".to_string(),
                text: "B1".to_string(),
            },
        ];
        plan.touched_works = vec![
            FileVectorizationWork {
                file_path: "src/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "src/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let prepared = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch::from_plan(plan));
        let mut envelope = prepared
            .into_persist_envelope(
                vec![vec![0.1_f32, 0.2_f32], vec![0.3_f32, 0.4_f32]],
                VectorBatchRun {
                    run_id: "persist-envelope-sync".to_string(),
                    prepare_started_at_ms: 0,
                    prepare_finished_at_ms: 0,
                    ready_enqueued_at_ms: 0,
                    started_at_ms: 1,
                    finished_at_ms: 1,
                    gpu_started_at_ms: 0,
                    gpu_finished_at_ms: 0,
                    persist_enqueued_at_ms: 0,
                    persist_started_at_ms: 0,
                    persist_finished_at_ms: 0,
                    finalize_enqueued_at_ms: 0,
                    finalize_finished_at_ms: 0,
                    provider: "cpu".to_string(),
                    runner_kind: "test".to_string(),
                    model_id: CHUNK_MODEL_ID.to_string(),
                    chunk_count: 0,
                    file_count: 0,
                    input_bytes: 4,
                    total_tokens: 0,
                    max_item_tokens: 0,
                    avg_item_tokens: 0.0,
                    micro_batch_count: 0,
                    max_micro_batch_tokens: 0,
                    avg_micro_batch_tokens: 0.0,
                    effective_vector_workers_admitted: 0,
                    ready_queue_depth_at_gpu_start: 0,
                    prepare_inflight_at_gpu_start: 0,
                    ready_queue_chunks_at_gpu_start: 0,
                    prepare_inflight_chunks_at_gpu_start: 0,
                    vector_worker_admission_reason: String::new(),
                    allowed_gpu_workers: 0,
                    batch_wait_for_ready_ms: 0,
                    persist_queue_wait_ms: 0,
                    finalize_queue_wait_ms: 0,
                    batch_lane: "mixed".to_string(),
                    batch_shape: "homogeneous".to_string(),
                    lane_small_max_tokens: 0,
                    lane_medium_max_tokens: 0,
                    fetch_ms: 0,
                    embed_ms: 0,
                    db_write_ms: 0,
                    mark_done_ms: 0,
                    success: true,
                    error_reason: None,
                },
            )
            .expect("persist envelope");

        envelope.sync_batch_run_counts_from_plan();

        assert_eq!(envelope.batch_run.chunk_count, 2);
        assert_eq!(envelope.batch_run.file_count, 2);
        assert_eq!(envelope.persist_plan.batch_run.chunk_count, 2);
        assert_eq!(envelope.persist_plan.batch_run.file_count, 2);
    }

    #[test]
    fn test_irrecoverable_outbox_finalize_error_is_quarantined_and_requeued() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/outbox-poison.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
                 VALUES ('/tmp/outbox-poison.rs', 'inflight', 1, 'claim-current', 1, 1, 'outbox', 7)",
            )
            .unwrap();

        let payload = VectorPersistOutboxPayload {
            updates: vec![],
            completed_works: vec![FileVectorizationWork {
                file_path: "/tmp/outbox-poison.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            completed_lease_snapshots: vec![FileVectorizationLeaseSnapshot {
                file_path: "/tmp/outbox-poison.rs".to_string(),
                claim_token: "claim-stale".to_string(),
                lease_epoch: 1,
            }],
            batch_run: VectorBatchRun {
                run_id: "outbox-poison-run".to_string(),
                prepare_started_at_ms: 0,
                prepare_finished_at_ms: 0,
                ready_enqueued_at_ms: 0,
                started_at_ms: 1,
                finished_at_ms: 1,
                gpu_started_at_ms: 0,
                gpu_finished_at_ms: 0,
                persist_enqueued_at_ms: 0,
                persist_started_at_ms: 0,
                persist_finished_at_ms: 0,
                finalize_enqueued_at_ms: 0,
                finalize_finished_at_ms: 0,
                provider: "cpu".to_string(),
                runner_kind: "test".to_string(),
                model_id: CHUNK_MODEL_ID.to_string(),
                chunk_count: 64,
                file_count: 1,
                input_bytes: 1024,
                total_tokens: 0,
                max_item_tokens: 0,
                avg_item_tokens: 0.0,
                micro_batch_count: 0,
                max_micro_batch_tokens: 0,
                avg_micro_batch_tokens: 0.0,
                effective_vector_workers_admitted: 0,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 0,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: String::new(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 0,
                finalize_queue_wait_ms: 0,
                batch_lane: "mixed".to_string(),
                batch_shape: "homogeneous".to_string(),
                lane_small_max_tokens: 0,
                lane_medium_max_tokens: 0,
                fetch_ms: 1,
                embed_ms: 1,
                db_write_ms: 0,
                mark_done_ms: 0,
                success: true,
                error_reason: None,
            },
        };
        let payload_json = serde_json::to_string(&payload).unwrap();
        let escaped_payload_json = payload_json.replace('\'', "''");
        let escaped_run_id = payload.batch_run.run_id.replace('\'', "''");
        let escaped_model_id = payload.batch_run.model_id.replace('\'', "''");
        store
            .execute(&format!(
                "INSERT INTO VectorPersistOutbox (outbox_id, run_id, model_id, status, attempts, queued_at_ms, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, payload_json) \
                 VALUES ('outbox-poison', '{}', '{}', 'inflight', 1, 1, 1, 1, 'outbox', '{}')",
                escaped_run_id,
                escaped_model_id,
                escaped_payload_json
            ))
            .unwrap();

        let mut batch_runs = vec![payload.batch_run.clone()];
        let quarantined = reconcile_outbox_finalize_failure(
            &store,
            "outbox-poison",
            &payload,
            &anyhow::anyhow!("finalize refused: expected 1 outbox-owned rows, matched 0"),
            &mut batch_runs,
            "failed to finalize outbox vectorization completion: finalize refused",
            7,
        )
        .unwrap();

        assert!(quarantined);

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM VectorPersistOutbox WHERE outbox_id = 'outbox-poison'"
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/outbox-poison.rs' \
                       AND status = 'queued' \
                       AND claim_token IS NULL \
                       AND COALESCE(lease_owner, '') = ''"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM sqlite_master \
                     WHERE type = 'table' \
                       AND name = 'VectorBatchRun'"
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn test_irrecoverable_outbox_finalize_error_detection() {
        assert!(is_irrecoverable_outbox_finalize_error(&anyhow::anyhow!(
            "finalize refused: expected 9 outbox-owned rows, matched 0"
        )));
        assert!(is_irrecoverable_outbox_finalize_error(&anyhow::anyhow!(
            "finalize refused: expected 2 lease snapshots, got 1"
        )));
        assert!(!is_irrecoverable_outbox_finalize_error(&anyhow::anyhow!(
            "database is locked"
        )));
    }

    #[test]
    fn test_finalize_completed_vectorization_works_clears_queue_and_enqueues_projection() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/finalize_vector.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
                 VALUES ('/tmp/finalize_vector.rs', 'inflight', 1, 'claim-finalize-vector', 1, 1, 'finalize', 1)",
            )
            .unwrap();

        let outcome = finalize_completed_vectorization_works(
            &store,
            vec![FileVectorizationWork {
                file_path: "/tmp/finalize_vector.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            vec![FileVectorizationLeaseSnapshot {
                file_path: "/tmp/finalize_vector.rs".to_string(),
                claim_token: "claim-finalize-vector".to_string(),
                lease_epoch: 1,
            }],
            vec![],
        )
        .unwrap();

        assert_eq!(outcome.completed_works.len(), 1);
        assert_eq!(
            store.query_count(
                "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/finalize_vector.rs'"
            )
            .unwrap(),
            0
        );
        assert_eq!(
            store.query_count(
                "SELECT count(*) FROM GraphProjectionQueue WHERE anchor_type = 'file' AND anchor_id = '/tmp/finalize_vector.rs'"
            )
            .unwrap(),
            if crate::runtime_mode::graph_embeddings_enabled() {
                1
            } else {
                0
            }
        );
    }

    #[test]
    fn test_finalize_completed_vectorization_works_marks_file_ready_under_file_level_contract() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/file_level_ready.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-1', 'symbol', 'sym-1', 'PRJ', '/tmp/file_level_ready.rs', 'function', 'fn a() {}', 'hash-a', 1, 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
                 VALUES ('/tmp/file_level_ready.rs', 'inflight', 1, 'claim-file-ready', 1, 1, 'finalize', 1)",
            )
            .unwrap();

        finalize_completed_vectorization_works(
            &store,
            vec![FileVectorizationWork {
                file_path: "/tmp/file_level_ready.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            vec![FileVectorizationLeaseSnapshot {
                file_path: "/tmp/file_level_ready.rs".to_string(),
                claim_token: "claim-file-ready".to_string(),
                lease_epoch: 1,
            }],
            vec![],
        )
        .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM File WHERE path = '/tmp/file_level_ready.rs' AND vector_ready = TRUE"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn test_finalize_completed_vectorization_works_batches_graph_projection_refreshes() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
                 ('/tmp/finalize_a.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100), \
                 ('/tmp/finalize_b.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
                 ('/tmp/finalize_a.rs', 'inflight', 1, 'claim-finalize-a', 1, 1, 'finalize', 1), \
                 ('/tmp/finalize_b.rs', 'inflight', 1, 'claim-finalize-b', 1, 1, 'finalize', 1)",
            )
            .unwrap();

        let outcome = finalize_completed_vectorization_works(
            &store,
            vec![
                FileVectorizationWork {
                    file_path: "/tmp/finalize_a.rs".to_string(),
                    resumed_after_interactive_pause: false,
                },
                FileVectorizationWork {
                    file_path: "/tmp/finalize_b.rs".to_string(),
                    resumed_after_interactive_pause: false,
                },
            ],
            vec![
                FileVectorizationLeaseSnapshot {
                    file_path: "/tmp/finalize_a.rs".to_string(),
                    claim_token: "claim-finalize-a".to_string(),
                    lease_epoch: 1,
                },
                FileVectorizationLeaseSnapshot {
                    file_path: "/tmp/finalize_b.rs".to_string(),
                    claim_token: "claim-finalize-b".to_string(),
                    lease_epoch: 1,
                },
            ],
            vec![],
        )
        .unwrap();

        assert_eq!(outcome.completed_works.len(), 2);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'"
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM GraphProjectionQueue WHERE anchor_type = 'file' AND anchor_id IN ('/tmp/finalize_a.rs', '/tmp/finalize_b.rs')"
                )
                .unwrap(),
            if crate::runtime_mode::graph_embeddings_enabled() {
                2
            } else {
                0
            }
        );
    }

    #[test]
    fn test_request_prepared_vector_embed_sequence_round_trips_over_prepare_channel() {
        let store = std::sync::Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let ready_batches: Arc<SharedPreparedBatchQueue> =
            Arc::new(SharedPreparedBatchQueue::new());
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        let ready_batches_for_thread = Arc::clone(&ready_batches);
        std::thread::spawn(move || {
            let request = prepare_rx.recv().unwrap();
            let touched_works = request.claimed.clone_works();
            let _ = ready_batches_for_thread.push_back_many(vec![PreparedBatchEnvelope::new(
                PreparedVectorEmbedBatch {
                    batch_id: "test-prepared-batch".to_string(),
                    prepare_started_at_ms: 1,
                    prepare_finished_at_ms: 1,
                    prepared_at_ms: 1,
                    batch_lane: VectorBatchLane::Mixed,
                    mixed_fallback: false,
                    lane_thresholds: current_token_lane_thresholds(),
                    work_items: vec![],
                    texts: vec!["prepared".to_string()],
                    token_counts: vec![],
                    encoded_micro_batches: vec![],
                    touched_works,
                    finalize_after_success: vec![],
                    immediate_completed: vec![],
                    oversized_works: vec![],
                    next_active_after_success: vec![],
                    next_active_after_failure: vec![],
                    files_touched: 1,
                    partial_file_cycles: 0,
                    fetch_ms_total: 0,
                    failed_fetches: vec![],
                },
            )]);
            request
                .reply
                .send(PreparedVectorPrepareOutcome {
                    remaining_claimed_after_success: ClaimedLeaseSet::new(vec![]),
                })
                .unwrap();
        });

        let claimed = ClaimedLeaseSet::new(vec![FileVectorizationWork {
            file_path: "/tmp/prepared.rs".to_string(),
            resumed_after_interactive_pause: false,
        }]);
        let reply_rx = dispatch_prepared_vector_embed_sequence(
            &prepare_tx,
            claimed.clone(),
            64,
            64,
            512 * 1024,
            2,
        )
        .unwrap();
        let outcome = wait_for_inflight_prepare_sequence(
            &store,
            0,
            InflightPrepareRequest {
                reply_rx,
                claimed,
                planned_chunk_count: 128,
                dispatched_at: Instant::now(),
            },
        )
        .unwrap();
        service_guard::record_vector_ready_queue_depth(ready_batches.len() as u64);

        assert!(outcome
            .remaining_claimed_after_success
            .as_slice()
            .is_empty());
        let snapshot = ready_batches.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].texts, vec!["prepared".to_string()]);
        assert_eq!(snapshot[0].touched_works.len(), 1);
        assert_eq!(snapshot[0].files_touched, 1);
    }

    #[test]
    fn test_shared_ready_queue_push_wakes_vector_backlog_waiters() {
        crate::service_guard::reset_for_tests();
        let ready_batches: Arc<SharedPreparedBatchQueue> =
            Arc::new(SharedPreparedBatchQueue::new());
        let waiter = std::thread::spawn(|| {
            crate::service_guard::wait_for_vector_backlog_signal(Duration::from_millis(250))
        });

        std::thread::sleep(Duration::from_millis(10));

        let _ = ready_batches.push_back_many(vec![PreparedBatchEnvelope::new(
            PreparedVectorEmbedBatch {
                batch_id: "wake-ready-queue".to_string(),
                prepare_started_at_ms: 1,
                prepare_finished_at_ms: 1,
                prepared_at_ms: 1,
                batch_lane: VectorBatchLane::Mixed,
                mixed_fallback: false,
                lane_thresholds: current_token_lane_thresholds(),
                work_items: vec![],
                texts: vec!["prepared".to_string()],
                token_counts: vec![],
                encoded_micro_batches: vec![],
                touched_works: vec![],
                finalize_after_success: vec![],
                immediate_completed: vec![],
                oversized_works: vec![],
                next_active_after_success: vec![],
                next_active_after_failure: vec![],
                files_touched: 0,
                partial_file_cycles: 0,
                fetch_ms_total: 0,
                failed_fetches: vec![],
            },
        )]);

        assert!(waiter.join().unwrap());
        assert_eq!(ready_batches.len(), 1);
    }

    #[test]
    fn test_shared_ready_queue_summary_tracks_metadata_without_full_view() {
        let ready_batches: Arc<SharedPreparedBatchQueue> =
            Arc::new(SharedPreparedBatchQueue::new());

        let first = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "summary-first".to_string(),
            prepare_started_at_ms: 1,
            prepare_finished_at_ms: 2,
            prepared_at_ms: 10,
            batch_lane: VectorBatchLane::Small,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![VectorChunkWorkItem {
                file_path: "/tmp/summary-a.rs".to_string(),
                chunk_id: "summary-a".to_string(),
                content_hash: "hash-a".to_string(),
                text: "a".to_string(),
            }],
            texts: vec!["a".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![FileVectorizationWork {
                file_path: "/tmp/summary-a.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 1,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });
        let second = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "summary-second".to_string(),
            prepare_started_at_ms: 3,
            prepare_finished_at_ms: 4,
            prepared_at_ms: 20,
            batch_lane: VectorBatchLane::Large,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![
                VectorChunkWorkItem {
                    file_path: "/tmp/summary-b.rs".to_string(),
                    chunk_id: "summary-b".to_string(),
                    content_hash: "hash-b".to_string(),
                    text: "b".to_string(),
                },
                VectorChunkWorkItem {
                    file_path: "/tmp/summary-c.rs".to_string(),
                    chunk_id: "summary-c".to_string(),
                    content_hash: "hash-c".to_string(),
                    text: "c".to_string(),
                },
            ],
            texts: vec!["b".to_string(), "c".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![
                FileVectorizationWork {
                    file_path: "/tmp/summary-b.rs".to_string(),
                    resumed_after_interactive_pause: false,
                },
                FileVectorizationWork {
                    file_path: "/tmp/summary-c.rs".to_string(),
                    resumed_after_interactive_pause: false,
                },
            ],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 2,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });

        assert_eq!(ready_batches.push_back_many(vec![first, second]), 2);
        let summary = ready_batches.summary();
        assert_eq!(summary.len, 2);
        assert_eq!(summary.chunk_count, 3);
        assert_eq!(summary.touched_works_count, 3);
        assert_eq!(summary.oldest_prepared_at_ms, Some(10));

        let popped = ready_batches.pop_best().expect("first batch available");
        assert_eq!(popped.batch_id, "summary-second");
        let summary = ready_batches.summary();
        assert_eq!(summary.len, 1);
        assert_eq!(summary.chunk_count, 1);
        assert_eq!(summary.touched_works_count, 1);
        assert_eq!(summary.oldest_prepared_at_ms, Some(10));

        let drained = ready_batches.drain();
        assert_eq!(drained.len(), 1);
        let summary = ready_batches.summary();
        assert_eq!(summary.len, 0);
        assert_eq!(summary.chunk_count, 0);
        assert_eq!(summary.touched_works_count, 0);
        assert_eq!(summary.oldest_prepared_at_ms, None);
    }

    #[test]
    fn test_shared_ready_queue_summary_deduplicates_touched_files_across_batches() {
        let ready_batches: Arc<SharedPreparedBatchQueue> =
            Arc::new(SharedPreparedBatchQueue::new());

        let duplicate_work = FileVectorizationWork {
            file_path: "/tmp/duplicate-summary.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let first = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "duplicate-summary-1".to_string(),
            prepare_started_at_ms: 1,
            prepare_finished_at_ms: 2,
            prepared_at_ms: 10,
            batch_lane: VectorBatchLane::Small,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![VectorChunkWorkItem {
                file_path: "/tmp/duplicate-summary.rs".to_string(),
                chunk_id: "duplicate-a".to_string(),
                content_hash: "duplicate-hash-a".to_string(),
                text: "a".to_string(),
            }],
            texts: vec!["a".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![duplicate_work.clone()],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 1,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });
        let second = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "duplicate-summary-2".to_string(),
            prepare_started_at_ms: 3,
            prepare_finished_at_ms: 4,
            prepared_at_ms: 20,
            batch_lane: VectorBatchLane::Medium,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![VectorChunkWorkItem {
                file_path: "/tmp/duplicate-summary.rs".to_string(),
                chunk_id: "duplicate-b".to_string(),
                content_hash: "duplicate-hash-b".to_string(),
                text: "b".to_string(),
            }],
            texts: vec!["b".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![duplicate_work],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 1,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });

        assert_eq!(ready_batches.push_back_many(vec![first, second]), 2);
        let summary = ready_batches.summary();
        assert_eq!(summary.len, 2);
        assert_eq!(summary.chunk_count, 2);
        assert_eq!(summary.touched_works_count, 1);
    }

    #[test]
    fn test_shared_ready_queue_touched_works_snapshot_is_incremental_and_deduplicated() {
        let ready_batches: Arc<SharedPreparedBatchQueue> =
            Arc::new(SharedPreparedBatchQueue::new());

        let base_work = FileVectorizationWork {
            file_path: "/tmp/duplicate-snapshot.rs".to_string(),
            resumed_after_interactive_pause: false,
        };
        let resumed_work = FileVectorizationWork {
            file_path: "/tmp/duplicate-snapshot.rs".to_string(),
            resumed_after_interactive_pause: true,
        };
        let first = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "duplicate-snapshot-1".to_string(),
            prepare_started_at_ms: 1,
            prepare_finished_at_ms: 2,
            prepared_at_ms: 10,
            batch_lane: VectorBatchLane::Small,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![VectorChunkWorkItem {
                file_path: "/tmp/duplicate-snapshot.rs".to_string(),
                chunk_id: "duplicate-snapshot-a".to_string(),
                content_hash: "duplicate-snapshot-hash-a".to_string(),
                text: "a".to_string(),
            }],
            texts: vec!["a".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![base_work.clone()],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 1,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });
        let second = PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
            batch_id: "duplicate-snapshot-2".to_string(),
            prepare_started_at_ms: 3,
            prepare_finished_at_ms: 4,
            prepared_at_ms: 20,
            batch_lane: VectorBatchLane::Large,
            mixed_fallback: false,
            lane_thresholds: current_token_lane_thresholds(),
            work_items: vec![VectorChunkWorkItem {
                file_path: "/tmp/duplicate-snapshot.rs".to_string(),
                chunk_id: "duplicate-snapshot-b".to_string(),
                content_hash: "duplicate-snapshot-hash-b".to_string(),
                text: "b".to_string(),
            }],
            texts: vec!["b".to_string()],
            token_counts: vec![],
            encoded_micro_batches: vec![],
            touched_works: vec![resumed_work],
            finalize_after_success: vec![],
            immediate_completed: vec![],
            oversized_works: vec![],
            next_active_after_success: vec![],
            next_active_after_failure: vec![],
            files_touched: 1,
            partial_file_cycles: 0,
            fetch_ms_total: 0,
            failed_fetches: vec![],
        });

        assert_eq!(ready_batches.push_back_many(vec![first, second]), 2);

        let snapshot = ready_batches.touched_works_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].file_path, base_work.file_path);
        assert!(snapshot[0].resumed_after_interactive_pause);

        let _ = ready_batches.pop_best().expect("one batch removed");
        let snapshot = ready_batches.touched_works_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].file_path, base_work.file_path);
        assert!(snapshot[0].resumed_after_interactive_pause);

        let _ = ready_batches.pop_best().expect("second batch removed");
        assert!(ready_batches.touched_works_snapshot().is_empty());
    }

    #[test]
    fn test_token_lane_thresholds_bootstrap_then_switch_to_live_quantiles() {
        reset_token_lane_classifier_for_tests();
        let bootstrap = current_token_lane_thresholds();
        assert_eq!(bootstrap.source, TokenLaneThresholdSource::Bootstrap);

        let samples = (1..=96).collect::<Vec<_>>();
        let live = observe_token_lane_thresholds(&samples);
        assert_eq!(live.source, TokenLaneThresholdSource::Live);
        assert!(live.small_max_tokens >= 30 && live.small_max_tokens <= 35);
        assert!(live.medium_max_tokens >= 62 && live.medium_max_tokens <= 66);
    }

    #[test]
    fn test_split_prepared_batch_by_lane_prefers_homogeneous_batches() {
        reset_token_lane_classifier_for_tests();
        let mut prepared = lane_test_prepared_batch(&["a", "b", "c"]);
        prepared.token_counts = vec![16, 160, 400];
        prepared.lane_thresholds = TokenLaneThresholds {
            small_max_tokens: 64,
            medium_max_tokens: 256,
            sample_count: 96,
            source: TokenLaneThresholdSource::Live,
        };
        prepared.batch_lane = VectorBatchLane::Mixed;

        let split = split_prepared_batch_by_lane(prepared, 16, 32);
        let lanes = split
            .iter()
            .map(|batch| batch.batch_lane())
            .collect::<Vec<_>>();

        assert_eq!(
            lanes,
            vec![
                VectorBatchLane::Large,
                VectorBatchLane::Medium,
                VectorBatchLane::Small
            ]
        );
        assert!(split.iter().all(|batch| !batch.mixed_fallback()));
    }

    #[test]
    fn test_split_prepared_batch_by_lane_uses_mixed_fallback_when_gpu_would_starve() {
        reset_token_lane_classifier_for_tests();
        let mut prepared = lane_test_prepared_batch(&["a", "b", "c"]);
        prepared.token_counts = vec![16, 160, 400];
        prepared.lane_thresholds = TokenLaneThresholds {
            small_max_tokens: 64,
            medium_max_tokens: 256,
            sample_count: 96,
            source: TokenLaneThresholdSource::Live,
        };
        prepared.batch_lane = VectorBatchLane::Mixed;

        let split = split_prepared_batch_by_lane(prepared, 8, 0);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].batch_lane(), VectorBatchLane::Mixed);
        assert!(split[0].mixed_fallback());
    }

    #[test]
    fn test_split_prepared_batch_by_lane_keeps_single_file_batches_mixed() {
        reset_token_lane_classifier_for_tests();
        let mut prepared = lane_test_prepared_batch(&["a", "b", "c"]);
        prepared.token_counts = vec![16, 160, 400];
        prepared.lane_thresholds = TokenLaneThresholds {
            small_max_tokens: 64,
            medium_max_tokens: 256,
            sample_count: 96,
            source: TokenLaneThresholdSource::Live,
        };
        prepared.batch_lane = VectorBatchLane::Mixed;
        for item in &mut prepared.work_items {
            item.file_path = "/tmp/shared.rs".to_string();
        }

        let split = split_prepared_batch_by_lane(prepared, 16, 32);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].batch_lane(), VectorBatchLane::Mixed);
    }

    #[test]
    fn test_split_prepared_batch_for_gpu_budget_caps_total_tokens_per_batch() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS", "2048");
        }

        let mut prepared = lane_test_prepared_batch(&["a", "b", "c"]);
        prepared.batch_lane = VectorBatchLane::Mixed;
        prepared.mixed_fallback = true;
        prepared.token_counts = vec![1200, 1200, 600];

        let split = split_prepared_batch_for_gpu_budget(prepared);
        let chunk_counts = split
            .iter()
            .map(|batch| batch.chunk_count())
            .collect::<Vec<_>>();
        let total_tokens = split
            .iter()
            .map(|batch| batch.total_token_count())
            .collect::<Vec<_>>();

        assert_eq!(chunk_counts, vec![1, 1, 1]);
        assert_eq!(total_tokens, vec![1200, 1200, 600]);
        assert!(split
            .iter()
            .all(|batch| batch.batch_lane() == VectorBatchLane::Mixed));
        assert!(split.iter().all(|batch| batch.mixed_fallback()));

        unsafe {
            std::env::remove_var("AXON_EMBED_BATCH_MAX_TOTAL_TOKENS");
        }
    }

    #[test]
    fn test_flush_completed_vectorization_works_enqueues_finalize_request() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
                 ('/tmp/flush_finalize.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
                 ('/tmp/flush_finalize.rs', 'inflight', 1, 'test-claim', 1, 1, 'vector', 0)",
            )
            .unwrap();
        let (finalize_tx, finalize_rx) = bounded::<VectorFinalizeRequest>(1);
        let mut completed_works = vec![FileVectorizationWork {
            file_path: "/tmp/flush_finalize.rs".to_string(),
            resumed_after_interactive_pause: false,
        }];
        let mut completed_batch_runs = vec![];
        let mut failed = HashMap::new();

        flush_completed_vectorization_works(
            0,
            &store,
            &finalize_tx,
            &mut completed_works,
            &mut completed_batch_runs,
            &mut failed,
        );

        assert!(completed_works.is_empty());
        assert!(failed.is_empty());
        let request = finalize_rx.try_recv().expect("finalize request");
        assert_eq!(request.envelope.completed_works.len(), 1);
        assert!(request.envelope.batch_runs.is_empty());
        assert_eq!(
            request.envelope.completed_works[0].file_path,
            "/tmp/flush_finalize.rs"
        );
    }

    #[test]
    fn test_vector_claim_target_expands_when_ready_reserve_is_missing() {
        assert_eq!(vector_claim_target(24, 0.0, 24, 0.0, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(24, 1.0, 24, 3.0, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(24, 2.2, 24, 6.6, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(3, 10.0, 12, 30.0, 8, 6, 128), 10);
    }

    #[test]
    fn test_vector_claim_target_can_expand_beyond_legacy_cap_for_deep_push_refill() {
        assert_eq!(vector_claim_target(24, 0.0, 128, 0.0, 32, 0, 4_096), 1_024);
    }

    #[test]
    fn test_vector_ready_reserve_target_adds_safety_stock_when_supply_is_thin_and_low_density() {
        let reserve = vector_ready_reserve_target(32, 2_048, 24, 128, 2, 0, 36.0, 2_000);
        assert!(reserve > 32);
    }

    #[test]
    fn test_vector_ready_reserve_target_stays_close_to_floor_when_supply_is_healthy() {
        let reserve = vector_ready_reserve_target(32, 256, 24, 96, 34, 4, 92.0, 300);
        assert!(reserve >= 32);
        assert!(reserve <= 40);
    }

    #[test]
    fn test_vector_ready_reserve_target_can_exceed_legacy_extreme_backlog_cap_when_configured() {
        let reserve = vector_ready_reserve_target(96, 4_096, 64, 192, 8, 0, 72.0, 2_000);
        assert!(reserve >= 96);
    }

    #[test]
    fn test_vector_ready_reserve_target_expands_reorder_point_when_supply_is_below_floor() {
        let reserve = vector_ready_reserve_target(32, 1_024, 32, 128, 3, 1, 80.0, 400);
        assert!(reserve > 32);
    }

    #[test]
    fn test_dispatch_prepared_vector_embed_sequence_returns_reply_channel() {
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        std::thread::spawn(move || {
            let request = prepare_rx.recv().unwrap();
            request
                .reply
                .send(PreparedVectorPrepareOutcome {
                    remaining_claimed_after_success: ClaimedLeaseSet::new(vec![]),
                })
                .unwrap();
        });

        let reply_rx = dispatch_prepared_vector_embed_sequence(
            &prepare_tx,
            ClaimedLeaseSet::new(vec![FileVectorizationWork {
                file_path: "/tmp/prefetched.rs".to_string(),
                resumed_after_interactive_pause: false,
            }]),
            64,
            64,
            512 * 1024,
            2,
        )
        .unwrap();
        let prepared = reply_rx.recv().unwrap();

        assert!(prepared
            .remaining_claimed_after_success
            .as_slice()
            .is_empty());
    }

    #[test]
    fn vector_refill_ownership_top_up_moves_into_producer_state() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
                 ('/tmp/refill-a.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100), \
                 ('/tmp/refill-b.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100), \
                 ('/tmp/refill-c.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
                 ('/tmp/refill-a.rs', 'queued', 1), \
                 ('/tmp/refill-b.rs', 'queued', 2), \
                 ('/tmp/refill-c.rs', 'queued', 3)",
            )
            .unwrap();

        let seed_claimed = store.fetch_pending_file_vectorization_work(1).unwrap();
        let mut producer = VectorRefillProducerState::new(seed_claimed);

        let added = producer
            .top_up_from_claimable_queue(&store, 3, &[], 0)
            .expect("producer top-up");

        assert_eq!(added, 2);
        assert_eq!(producer.active_works().len(), 3);
    }

    #[test]
    fn vector_refill_ownership_dispatch_moves_claims_out_of_local_wave_storage() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        let mut producer = VectorRefillProducerState::new(vec![
            FileVectorizationWork {
                file_path: "/tmp/dispatch-a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/dispatch-b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ]);

        assert!(producer
            .dispatch_prepare_request(&store, &prepare_tx, 2, 64, 64, 512 * 1024, 2, 128)
            .expect("dispatch prepare"));

        let request = prepare_rx.try_recv().expect("prepare request");
        assert_eq!(request.claimed.as_slice().len(), 2);
        assert!(producer.active_works().is_empty());
        assert_eq!(producer.inflight_prepare_count(), 1);
        assert_eq!(producer.inflight_prepare_chunk_count(), 128);
    }

    #[test]
    fn vector_refill_ownership_prepare_replies_merge_back_into_producer_state() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        let mut producer = VectorRefillProducerState::new(vec![FileVectorizationWork {
            file_path: "/tmp/reply-a.rs".to_string(),
            resumed_after_interactive_pause: false,
        }]);

        assert!(producer
            .dispatch_prepare_request(&store, &prepare_tx, 1, 64, 64, 512 * 1024, 1, 64)
            .expect("dispatch prepare"));
        let request = prepare_rx.try_recv().expect("prepare request");
        request
            .reply
            .send(PreparedVectorPrepareOutcome {
                remaining_claimed_after_success: ClaimedLeaseSet::new(vec![
                    FileVectorizationWork {
                        file_path: "/tmp/reply-remaining.rs".to_string(),
                        resumed_after_interactive_pause: false,
                    },
                ]),
            })
            .unwrap();

        producer.poll_prepare_replies(0);

        assert_eq!(producer.inflight_prepare_count(), 0);
        assert_eq!(producer.inflight_prepare_chunk_count(), 0);
        assert_eq!(producer.active_works().len(), 1);
        assert_eq!(
            producer.active_works()[0].file_path,
            "/tmp/reply-remaining.rs"
        );
    }

    #[test]
    fn continuous_prepare_feed_continues_when_ready_is_low_and_claimable_supply_exists() {
        assert!(continuous_prepare_feed_allowed(
            false, 0, 0, 8, 16, 12, 24, 0
        ));
    }

    #[test]
    fn continuous_prepare_feed_stops_when_backpressure_is_already_active() {
        assert!(!continuous_prepare_feed_allowed(
            false, 16, 0, 8, 16, 12, 24, 0
        ));
    }

    #[test]
    fn continuous_prepare_feed_stops_only_when_no_supply_remains() {
        assert!(!continuous_prepare_feed_allowed(
            false, 2, 0, 8, 16, 12, 0, 0
        ));
        assert!(continuous_prepare_feed_allowed(
            false, 2, 0, 8, 16, 12, 0, 1
        ));
    }

    #[test]
    fn continuous_prepare_feed_can_exceed_legacy_high_watermark_when_dynamic_target_is_higher() {
        assert!(continuous_prepare_feed_allowed(
            false, 24, 8, 8, 48, 96, 24, 0
        ));
    }

    #[test]
    fn gpu_worker_consumption_only_stays_alive_for_ready_or_persist_or_upstream_supply() {
        assert!(gpu_worker_has_pending_work(1, 0, 0, 0));
        assert!(gpu_worker_has_pending_work(0, 1, 0, 0));
        assert!(gpu_worker_has_pending_work(0, 0, 1, 0));
        assert!(gpu_worker_has_pending_work(0, 0, 0, 1));
        assert!(!gpu_worker_has_pending_work(0, 0, 0, 0));
    }

    #[test]
    fn gpu_worker_consumption_only_waits_only_when_ready_is_empty_but_supply_remains() {
        assert!(gpu_worker_should_wait_for_ready(0, 0, 1, 0));
        assert!(gpu_worker_should_wait_for_ready(0, 0, 0, 1));
        assert!(!gpu_worker_should_wait_for_ready(1, 0, 1, 1));
        assert!(!gpu_worker_should_wait_for_ready(0, 1, 1, 1));
        assert!(!gpu_worker_should_wait_for_ready(0, 0, 0, 0));
    }

    #[test]
    fn gpu_worker_consumption_only_obeys_vram_admission() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED", "true");

        assert!(gpu_worker_consumption_allowed(false, None));
        assert!(gpu_worker_consumption_allowed(
            true,
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 2_048,
                free_mb: 6_144,
            }),
        ));
        assert!(!gpu_worker_consumption_allowed(
            true,
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: gpu_primary_worker_max_used_mb().saturating_add(1),
                free_mb: 0,
            }),
        ));

        std::env::remove_var("AXON_GPU_PRIMARY_BATCH_GUARD_ENABLED");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_allows_batch_when_disabled() {
        let _guard = lock_env_guard();
        // Explicitly disable -- default is now true (to prevent unified memory spill).
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "false");

        assert!(
            super::gpu_pre_batch_vram_recycle_reason(Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 8_000,
                free_mb: 192,
            }))
            .is_none()
        );

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_allows_batch_below_admission() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true");
        std::env::set_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB", "6000");

        assert!(
            super::gpu_pre_batch_vram_recycle_reason(Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 5_500,
                free_mb: 2_692,
            }))
            .is_none()
        );

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
        std::env::remove_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_recycles_on_unknown_telemetry_by_default() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true");
        // Do NOT set UNKNOWN_RECYCLE -- default is now true (conservative:
        // without telemetry, blind embedding risks 40x throughput loss from
        // unified memory spill on 8GB GPUs).
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE");

        let reason = super::gpu_pre_batch_vram_recycle_reason_with_probe(None, || None)
            .expect("unknown telemetry should request recycling by default");
        assert!(reason.contains("gpu_pre_batch_vram_unknown"));

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_skips_unknown_telemetry_when_disabled() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE", "false");

        assert!(
            super::gpu_pre_batch_vram_recycle_reason_with_probe(None, || None).is_none(),
            "unknown telemetry should NOT request recycling when explicitly disabled"
        );

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_UNKNOWN_RECYCLE");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_detects_plateau_with_probe() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true");
        std::env::set_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB", "6000");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES", "2");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS", "50");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB", "128");

        // Simulate VRAM stuck at 7000 MB with no meaningful drop (< 128 MB).
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let reason = super::gpu_pre_batch_vram_recycle_reason_with_probe(
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 7_000,
                free_mb: 1_192,
            }),
            || {
                let n = call_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Simulate minor fluctuation (50 MB) that stays below min_drop threshold.
                Some(GpuMemorySnapshot {
                    total_mb: 8_192,
                    used_mb: if n == 0 { 6_960 } else { 6_950 },
                    free_mb: if n == 0 { 1_232 } else { 1_242 },
                })
            },
        );
        assert!(
            reason.is_some(),
            "guard should detect plateau when drop < min_drop_mb"
        );
        let reason_str = reason.unwrap();
        assert!(
            reason_str.contains("gpu_pre_batch_vram_plateau"),
            "reason should indicate plateau: {}",
            reason_str
        );

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
        std::env::remove_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_MIN_DROP_MB");
    }

    #[test]
    fn gpu_pre_batch_vram_guard_passes_when_vram_recovers_during_probe() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED", "true");
        std::env::set_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB", "6000");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES", "3");
        std::env::set_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS", "50");

        // Simulate VRAM dropping below admission during probe.
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let reason = super::gpu_pre_batch_vram_recycle_reason_with_probe(
            Some(GpuMemorySnapshot {
                total_mb: 8_192,
                used_mb: 7_000,
                free_mb: 1_192,
            }),
            || {
                let n = call_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // Second probe shows VRAM recovered below admission (5500 < 6000).
                let used = if n == 0 { 6_800 } else { 5_500 };
                Some(GpuMemorySnapshot {
                    total_mb: 8_192,
                    used_mb: used,
                    free_mb: 8_192 - used,
                })
            },
        );
        assert!(
            reason.is_none(),
            "guard should pass when VRAM recovers below admission during probe"
        );

        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_ENABLED");
        std::env::remove_var("AXON_GPU_PRIMARY_WORKER_MAX_USED_MB");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_SAMPLES");
        std::env::remove_var("AXON_GPU_PRE_BATCH_VRAM_GUARD_WAIT_MS");
    }

    #[test]
    fn test_build_prepared_vector_embed_sequence_stops_at_requested_depth() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a', 'symbol', 'sym-a', 'PRJ', '/tmp/a.rs', 'function', 'fn a() {}', 'hash-a', 1, 1), \
             ('chunk-b', 'symbol', 'sym-b', 'PRJ', '/tmp/b.rs', 'function', 'fn b() {}', 'hash-b', 1, 1), \
             ('chunk-c', 'symbol', 'sym-c', 'PRJ', '/tmp/c.rs', 'function', 'fn c() {}', 'hash-c', 1, 1)"
        ).unwrap();

        let active = vec![
            FileVectorizationWork {
                file_path: "/tmp/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/c.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let sequence =
            build_prepared_vector_embed_sequence(&store, &active, 1, 1, 512 * 1024, 2, 1);

        assert_eq!(sequence.batches.len(), 2);
        assert_eq!(sequence.remaining_claimed_after_success.as_slice().len(), 1);
    }

    #[test]
    fn test_build_prepared_vector_embed_sequence_streams_multiple_batches_per_dispatch() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a', 'symbol', 'sym-a', 'PRJ', '/tmp/a.rs', 'function', 'fn a() {}', 'hash-a', 1, 1), \
             ('chunk-b', 'symbol', 'sym-b', 'PRJ', '/tmp/b.rs', 'function', 'fn b() {}', 'hash-b', 1, 1), \
             ('chunk-c', 'symbol', 'sym-c', 'PRJ', '/tmp/c.rs', 'function', 'fn c() {}', 'hash-c', 1, 1)"
        ).unwrap();

        let active = vec![
            FileVectorizationWork {
                file_path: "/tmp/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/c.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let sequence =
            build_prepared_vector_embed_sequence(&store, &active, 1, 1, 512 * 1024, 3, 1);

        assert_eq!(sequence.batches.len(), 3);
        assert!(sequence
            .remaining_claimed_after_success
            .as_slice()
            .is_empty());
    }

    #[test]
    fn test_bootstrap_runtime_tuning_does_not_cap_inflight_persists_to_legacy_eight() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_PERSIST_QUEUE_BOUND", "32");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
        }

        let snapshot = super::refresh_runtime_tuning_snapshot_from_env();

        assert_eq!(snapshot.state.vector_persist_queue_bound, 32);
        assert_eq!(snapshot.state.vector_max_inflight_persists, 32);

        unsafe {
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
    }

    #[test]
    fn test_build_prepared_vector_embed_sequence_does_not_reselect_reserved_chunks() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a1', 'symbol', 'sym-a1', 'PRJ', '/tmp/a.rs', 'function', 'fn a1() {}', 'hash-a1', 1, 1), \
             ('chunk-a2', 'symbol', 'sym-a2', 'PRJ', '/tmp/a.rs', 'function', 'fn a2() {}', 'hash-a2', 2, 2), \
             ('chunk-b1', 'symbol', 'sym-b1', 'PRJ', '/tmp/b.rs', 'function', 'fn b1() {}', 'hash-b1', 1, 1)"
        ).unwrap();

        let active = vec![
            FileVectorizationWork {
                file_path: "/tmp/a.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
            FileVectorizationWork {
                file_path: "/tmp/b.rs".to_string(),
                resumed_after_interactive_pause: false,
            },
        ];

        let sequence =
            build_prepared_vector_embed_sequence(&store, &active, 1, 1, 512 * 1024, 3, 1);

        let chunk_ids = sequence
            .batches
            .iter()
            .flat_map(|batch| batch.work_items.iter().map(|item| item.chunk_id.clone()))
            .collect::<Vec<_>>();
        assert_eq!(chunk_ids, vec!["chunk-a1", "chunk-a2", "chunk-b1"]);
    }

    #[test]
    fn test_build_vector_batch_plan_marks_single_oversized_chunk() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let oversized = "x".repeat(4096);
        store.execute(&format!(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-oversized', 'symbol', 'sym-oversized', 'PRJ', '/tmp/oversized.rs', 'function', '{}', 'hash-oversized', 1, 1)",
            oversized
        ))
        .unwrap();

        let plan = build_vector_batch_plan(
            &store,
            &[FileVectorizationWork {
                file_path: "/tmp/oversized.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            8,
            1,
            32,
            &HashSet::new(),
        );

        assert!(plan.work_items.is_empty());
        assert_eq!(plan.oversized_works.len(), 1);
        assert!(plan.untouched_works.is_empty());
    }

    #[test]
    fn test_gpu_memory_soft_limit_mb_falls_back_to_operator_budget() {
        let _guard = lock_env_guard();
        std::env::remove_var("AXON_CUDA_MEMORY_SOFT_LIMIT_MB");
        std::env::set_var("AXON_OPT_MAX_VRAM_USED_MB", "6144");
        assert_eq!(gpu_memory_soft_limit_mb(), 6144);
        std::env::remove_var("AXON_OPT_MAX_VRAM_USED_MB");
    }

    fn controller_test_config() -> EmbeddingLaneConfig {
        EmbeddingLaneConfig {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 0,
            chunk_batch_size: 32,
            file_vectorization_batch_size: 8,
            graph_batch_size: 8,
            max_chunks_per_file: 64,
            max_embed_batch_bytes: 512 * 1024,
        }
    }

    fn controller_observation(
        upstream_file_pressure: usize,
        interactive_active: bool,
        gpu_memory_pressure: bool,
        embed_calls_total: u64,
        chunks_embedded_total: u64,
        files_touched_total: u64,
        embed_ms_total: u64,
    ) -> VectorBatchControllerObservation {
        controller_observation_with_runtime(
            upstream_file_pressure,
            interactive_active,
            gpu_memory_pressure,
            embed_calls_total,
            chunks_embedded_total,
            files_touched_total,
            embed_ms_total,
            0,
            0,
        )
    }

    fn controller_observation_with_runtime(
        upstream_file_pressure: usize,
        interactive_active: bool,
        gpu_memory_pressure: bool,
        embed_calls_total: u64,
        chunks_embedded_total: u64,
        files_touched_total: u64,
        embed_ms_total: u64,
        ready_queue_depth_current: u64,
        gpu_idle_wait_ms_total: u64,
    ) -> VectorBatchControllerObservation {
        VectorBatchControllerObservation {
            upstream_file_pressure,
            front_chunk_supply: ready_queue_depth_current as usize,
            interactive_active,
            gpu_memory_pressure,
            metrics: VectorRuntimeMetrics {
                embed_ms_total,
                batches_total: embed_calls_total,
                chunks_embedded_total,
                embed_calls_total,
                files_touched_total,
                ready_queue_depth_current,
                ready_queue_chunks_current: ready_queue_depth_current,
                gpu_idle_wait_ms_total,
                ..VectorRuntimeMetrics::default()
            },
        }
    }

    #[test]
    fn test_vector_batch_controller_grows_targets_when_idle_backlog_is_underfed() {
        let _guard = lock_env_guard();
        super::refresh_runtime_tuning_snapshot_from_env();
        let mut controller = VectorBatchController::new(&controller_test_config());
        let diagnostics = controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 64, 4, 20_480),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.target_embed_batch_chunks, 80);
        assert_eq!(diagnostics.target_files_per_cycle, 24);
        assert_eq!(diagnostics.adjustments_total, 1);
    }

    #[test]
    fn test_vector_batch_controller_shrinks_targets_when_interactive_priority_activates() {
        let mut controller = VectorBatchController::new(&controller_test_config());
        controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 64, 4, 20_480),
        );

        let diagnostics = controller.observe(
            21_000,
            controller_observation(4_096, true, false, 8, 320, 10, 40_960),
        );

        assert_eq!(
            diagnostics.state,
            VectorBatchControllerState::InteractiveGuarded
        );
        assert_eq!(diagnostics.target_embed_batch_chunks, 32);
        assert_eq!(diagnostics.target_files_per_cycle, 8);
        assert_eq!(diagnostics.adjustments_total, 2);
    }

    #[test]
    fn test_vector_batch_controller_respects_bounds() {
        let mut controller = VectorBatchController::new(&controller_test_config());
        let mut diagnostics =
            current_vector_batch_controller_diagnostics(&controller_test_config());
        for idx in 1..12 {
            diagnostics = controller.observe(
                10_000 + idx * 11_000,
                controller_observation(
                    8_192,
                    false,
                    false,
                    idx * 4,
                    idx * 64,
                    idx * 4,
                    idx * 20_480,
                ),
            );
        }

        assert_eq!(diagnostics.target_embed_batch_chunks, 128);
        assert_eq!(diagnostics.target_files_per_cycle, 32);
    }

    #[test]
    fn test_vector_batch_controller_holds_targets_inside_cooldown() {
        let mut controller = VectorBatchController::new(&controller_test_config());
        let first = controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 128, 4, 20_480),
        );
        let second = controller.observe(
            11_000,
            controller_observation(4_096, false, false, 8, 256, 8, 40_960),
        );

        assert_eq!(
            first.target_embed_batch_chunks,
            second.target_embed_batch_chunks
        );
        assert_eq!(first.target_files_per_cycle, second.target_files_per_cycle);
        assert_eq!(first.adjustments_total, second.adjustments_total);
    }

    #[test]
    fn test_vector_batch_controller_preserves_last_completed_window_during_warmup() {
        let mut controller = VectorBatchController::new(&controller_test_config());
        let matured = controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 128, 4, 20_480),
        );
        let warmup = controller.observe(
            12_000,
            controller_observation(4_096, false, false, 4, 128, 4, 20_480),
        );

        assert_eq!(matured.window_embed_calls, 4);
        assert_eq!(warmup.window_embed_calls, 4);
        assert_eq!(warmup.window_chunks, 128);
        assert_eq!(warmup.window_files_touched, 4);
    }

    #[test]
    fn test_vector_batch_controller_uses_idle_target_during_warmup_when_backlog_is_meaningful() {
        let mut controller = VectorBatchController::new(&controller_test_config());

        let diagnostics = controller.observe(
            10_000,
            controller_observation(4_096, false, false, 0, 0, 0, 0),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.reason, "warming_window");
        assert_eq!(diagnostics.target_embed_batch_chunks, 32);
        assert_eq!(diagnostics.target_files_per_cycle, 8);
        assert_eq!(diagnostics.adjustments_total, 0);
    }

    #[test]
    fn test_vector_batch_controller_uses_small_gpu_warmup_target_before_first_successful_embed() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS", "8");
        std::env::set_var("AXON_GPU_WARMUP_FILES_PER_CYCLE", "1");

        let mut controller = VectorBatchController::new(&controller_test_config());

        let diagnostics = controller.observe(
            10_000,
            controller_observation(4_096, false, false, 0, 0, 0, 0),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.reason, "warming_window");
        assert_eq!(diagnostics.target_embed_batch_chunks, 16);
        assert_eq!(diagnostics.target_files_per_cycle, 4);
        assert_eq!(diagnostics.adjustments_total, 0);

        std::env::remove_var("AXON_GPU_WARMUP_EMBED_BATCH_CHUNKS");
        std::env::remove_var("AXON_GPU_WARMUP_FILES_PER_CYCLE");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    #[test]
    fn test_vector_batch_controller_steps_down_after_embed_efficiency_regresses() {
        let _guard = lock_env_guard();
        super::refresh_runtime_tuning_snapshot_from_env();
        let mut controller = VectorBatchController::new(&controller_test_config());
        controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 64, 4, 20_480),
        );
        controller.observe(
            21_000,
            controller_observation(4_096, false, false, 8, 512, 16, 61_440),
        );

        let diagnostics = controller.observe(
            32_000,
            controller_observation(4_096, false, false, 12, 768, 24, 138_240),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.target_embed_batch_chunks, 80);
        assert_eq!(diagnostics.target_files_per_cycle, 32);
        assert_eq!(diagnostics.adjustments_total, 3);
    }

    #[test]
    fn test_single_gpu_worker_cruise_mode_waits_for_second_regression_before_step_down() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "1");
        super::refresh_runtime_tuning_snapshot_from_env();

        let mut controller = VectorBatchController::new(&controller_test_config());
        controller.observe(
            10_000,
            controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 2_048, 0),
        );
        controller.observe(
            21_000,
            controller_observation_with_runtime(4_096, false, false, 8, 512, 16, 61_440, 2_048, 0),
        );

        let first_regression = controller.observe(
            32_000,
            controller_observation_with_runtime(4_096, false, false, 12, 896, 24, 138_240, 2_048, 0),
        );
        let second_regression = controller.observe(
            43_000,
            controller_observation_with_runtime(4_096, false, false, 16, 1_280, 32, 215_040, 2_048, 0),
        );

        assert_ne!(first_regression.reason, "embed_efficiency_regressed");
        assert_eq!(first_regression.target_embed_batch_chunks, 80);
        assert_eq!(first_regression.target_files_per_cycle, 24);
        assert_eq!(second_regression.reason, "embed_efficiency_regressed");
        assert_eq!(second_regression.target_embed_batch_chunks, 56);
        assert_eq!(second_regression.target_files_per_cycle, 24);

        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    #[test]
    fn test_single_gpu_worker_cruise_mode_grows_more_aggressively_when_ready_queue_starves() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "1");

        let mut controller = VectorBatchController::new(&controller_test_config());
        let diagnostics = controller.observe(
            10_000,
            controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.reason, "ready_queue_starved");
        assert_eq!(diagnostics.target_embed_batch_chunks, 104);
        assert_eq!(diagnostics.target_files_per_cycle, 24);

        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    #[test]
    fn test_single_gpu_worker_cruise_mode_reduces_chunk_target_when_ready_queue_starves_with_low_density(
    ) {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "1");

        let mut controller = VectorBatchController::new(&controller_test_config());
        controller.observe(
            10_000,
            controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
        );
        let diagnostics = controller.observe(
            21_000,
            controller_observation_with_runtime(4_096, false, false, 8, 136, 16, 56_000, 0, 0),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.reason, "ready_queue_starved_low_density");
        assert_eq!(diagnostics.target_embed_batch_chunks, 32);
        assert_eq!(diagnostics.target_files_per_cycle, 32);

        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    #[test]
    fn test_persistent_low_density_underfeed_opens_file_window_more_aggressively() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_VECTOR_WORKERS", "1");
        super::refresh_runtime_tuning_snapshot_from_env();

        let lane_config = EmbeddingLaneConfig {
            chunk_batch_size: 48,
            file_vectorization_batch_size: 16,
            ..controller_test_config()
        };
        let mut controller = VectorBatchController::new(&lane_config);
        let first = controller.observe(
            10_000,
            controller_observation_with_runtime(4_096, false, false, 4, 64, 4, 20_480, 0, 0),
        );
        let second = controller.observe(
            21_000,
            controller_observation_with_runtime(4_096, false, false, 8, 156, 16, 56_000, 0, 0),
        );
        let third = controller.observe(
            32_000,
            controller_observation_with_runtime(4_096, false, false, 12, 228, 28, 84_000, 0, 0),
        );

        assert_eq!(first.reason, "ready_queue_starved");
        assert_eq!(second.reason, "ready_queue_starved_low_density");
        assert_eq!(third.reason, "ready_queue_starved_low_density");
        assert_eq!(second.target_embed_batch_chunks, 48);
        assert_eq!(third.target_embed_batch_chunks, 48);
        assert!(third.target_files_per_cycle > second.target_files_per_cycle);
        assert_eq!(second.target_files_per_cycle, 56);
        assert_eq!(third.target_files_per_cycle, 64);

        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
    }

    #[test]
    fn test_vector_prepare_prefetch_limits_scale_with_target_ready_depth() {
        assert_eq!(vector_prepare_prefetch_limits(3, 10), (48, 24));
        assert_eq!(vector_prepare_prefetch_limits(12, 32), (64, 32));
    }

    #[test]
    fn test_replenish_target_chunks_matches_consumed_chunk_volume() {
        assert_eq!(replenish_target_chunks(64, 0), 64);
        assert_eq!(replenish_target_chunks(64, 8), 8);
        assert_eq!(replenish_target_chunks(64, 96), 64);
    }

    #[test]
    fn test_replenish_target_ready_depth_converts_chunk_deficit_to_batch_depth() {
        assert_eq!(replenish_target_ready_depth(4, 0, 64, 32), 4);
        assert_eq!(replenish_target_ready_depth(0, 8, 8, 32), 1);
        assert_eq!(replenish_target_ready_depth(0, 17, 8, 32), 3);
        assert_eq!(replenish_target_ready_depth(1, 64, 16, 2), 2);
    }

    #[test]
    fn test_vector_replenishment_command_prefers_consumption_debt_when_present() {
        let (mode, command_chunks, visible_gap) = vector_replenishment_command(256, 64, 32, 96);
        assert_eq!(mode, VectorReplenishmentMode::ConsumptionDriven);
        assert_eq!(command_chunks, 64);
        assert_eq!(visible_gap, 96);
    }

    #[test]
    fn test_vector_replenishment_command_falls_back_to_stock_gap_without_consumption_debt() {
        let (mode, command_chunks, visible_gap) = vector_replenishment_command(256, 64, 32, 0);
        assert_eq!(mode, VectorReplenishmentMode::StockGapDriven);
        assert_eq!(command_chunks, 160);
        assert_eq!(visible_gap, 160);
    }

    #[test]
    fn test_record_vector_frontier_metrics_keeps_consumption_debt_authoritative() {
        crate::service_guard::reset_for_tests();
        record_vector_frontier_metrics(
            &PreparedBatchQueueSummary {
                len: 2,
                chunk_count: 96,
                touched_works_count: 2,
                oldest_prepared_at_ms: None,
                ..PreparedBatchQueueSummary::default()
            },
            1,
            64,
            512,
            48,
        );
        assert_eq!(
            crate::service_guard::vector_runtime_metrics().ready_replenishment_deficit_current,
            48
        );
    }

    #[test]
    fn test_maintain_vector_claimable_supply_promotes_missing_graph_ready_work() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        for path in ["/tmp/supply-a.rs", "/tmp/supply-b.rs", "/tmp/supply-c.rs"] {
            store
                .execute(&format!(
                    "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                     VALUES ('{}', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
                    path
                ))
                .unwrap();
            store
                .execute(&format!(
                    "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                     VALUES ('chunk-{}', 'symbol', 'sym-{}', 'PRJ', '{}', 'function', 'body', 'hash-{}', 1, 1)",
                    path.replace('/', "_"),
                    path.replace('/', "_"),
                    path,
                    path.replace('/', "_"),
                ))
                .unwrap();
        }
        crate::service_guard::reset_for_tests();
        crate::service_guard::record_vector_canonical_backlog_depth(64);
        crate::service_guard::record_vector_ready_queue_depth(0);
        crate::service_guard::record_vector_prepare_inflight_depth(0);
        crate::service_guard::record_vector_prepare_claimed(0);
        crate::service_guard::record_vector_ready_claimed(0);
        crate::service_guard::record_vector_active_claimed(0);
        crate::service_guard::record_vector_persist_claimed(0);

        let added = super::maintain_vector_claimable_supply(&store).unwrap();

        assert!(added > 0);
        assert!(
            store
                .fetch_claimable_file_vectorization_queue_count()
                .unwrap()
                > 0,
            "claimable queue should be replenished"
        );
    }

    #[test]
    fn test_prepare_queue_bound_tracks_twice_ready_depth() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_READY_QUEUE_DEPTH", "96");
            std::env::set_var("AXON_GPU_READY_LOW_WATERMARK", "12");
            std::env::set_var("AXON_GPU_READY_HIGH_WATERMARK", "28");
            std::env::set_var("AXON_VECTOR_PREPARE_QUEUE_BOUND", "6");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        assert_eq!(configured_vector_prepare_queue_bound(), 192);
        unsafe {
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_GPU_READY_LOW_WATERMARK");
            std::env::remove_var("AXON_GPU_READY_HIGH_WATERMARK");
            std::env::remove_var("AXON_VECTOR_PREPARE_QUEUE_BOUND");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
    }

    #[test]
    fn test_configured_gpu_ready_watermarks_follow_env() {
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_VECTOR_READY_QUEUE_DEPTH", "96");
            std::env::set_var("AXON_GPU_READY_LOW_WATERMARK", "12");
            std::env::set_var("AXON_GPU_READY_HIGH_WATERMARK", "28");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        assert_eq!(configured_gpu_ready_low_watermark(), 192);
        assert_eq!(configured_gpu_ready_high_watermark(), 448);
        unsafe {
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_GPU_READY_LOW_WATERMARK");
            std::env::remove_var("AXON_GPU_READY_HIGH_WATERMARK");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
    }

    #[test]
    fn test_single_worker_gpu_prepare_worker_count_expands_when_gpu_is_safely_underfed() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR", "2");

        assert_eq!(single_worker_gpu_prepare_worker_count(true, 1, 2), 4);
        assert_eq!(single_worker_gpu_prepare_worker_count(true, 2, 2), 2);
        assert_eq!(single_worker_gpu_prepare_worker_count(false, 1, 2), 2);

        std::env::remove_var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR");
    }

    #[test]
    fn test_gpu_recycle_after_vram_summit_triggers_after_two_low_throughput_batches() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT", "true");
        std::env::set_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB", "7000");
        std::env::set_var("AXON_GPU_RECYCLE_MIN_CHUNKS_PER_SECOND", "8");
        std::env::set_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES", "2");

        let mut consecutive = 0_u32;
        assert!(!gpu_recycle_after_vram_summit_observe(
            7_050,
            4.0,
            &mut consecutive
        ));
        assert_eq!(consecutive, 1);
        assert!(gpu_recycle_after_vram_summit_observe(
            7_020,
            3.5,
            &mut consecutive
        ));
        assert_eq!(consecutive, 2);

        std::env::remove_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT");
        std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB");
        std::env::remove_var("AXON_GPU_RECYCLE_MIN_CHUNKS_PER_SECOND");
        std::env::remove_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES");
    }

    #[test]
    fn test_gpu_recycle_after_vram_summit_resets_when_pressure_or_throughput_recovers() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT", "true");
        std::env::set_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB", "7000");
        std::env::set_var("AXON_GPU_RECYCLE_MIN_CHUNKS_PER_SECOND", "8");
        std::env::set_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES", "2");

        let mut consecutive = 0_u32;
        assert!(!gpu_recycle_after_vram_summit_observe(
            7_050,
            4.0,
            &mut consecutive
        ));
        assert_eq!(consecutive, 1);
        assert!(!gpu_recycle_after_vram_summit_observe(
            6_500,
            12.0,
            &mut consecutive
        ));
        assert_eq!(consecutive, 0);

        std::env::remove_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT");
        std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB");
        std::env::remove_var("AXON_GPU_RECYCLE_MIN_CHUNKS_PER_SECOND");
        std::env::remove_var("AXON_GPU_RECYCLE_REQUIRED_BATCHES");
    }

    #[test]
    fn test_gpu_recycle_after_vram_summit_can_derive_threshold_from_percentage() {
        let _guard = lock_env_guard();
        std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB");
        std::env::set_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT", "80");
        std::env::set_var("AXON_GPU_TOTAL_VRAM_MB_HINT", "8192");

        assert_eq!(super::gpu_recycle_vram_summit_mb(), 6554);

        std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_PCT");
        std::env::remove_var("AXON_GPU_TOTAL_VRAM_MB_HINT");
    }

    #[test]
    fn test_gpu_recycle_immediate_required_triggers_only_above_threshold_without_inflight() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT", "true");
        std::env::set_var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT", "true");
        std::env::set_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB", "7000");

        assert!(super::gpu_recycle_immediate_required(
            Some(GpuMemorySnapshot {
                total_mb: 8192,
                used_mb: 7050,
                free_mb: 1142,
            }),
            0,
        ));
        assert!(!super::gpu_recycle_immediate_required(
            Some(GpuMemorySnapshot {
                total_mb: 8192,
                used_mb: 7050,
                free_mb: 1142,
            }),
            1,
        ));
        assert!(!super::gpu_recycle_immediate_required(
            Some(GpuMemorySnapshot {
                total_mb: 8192,
                used_mb: 6500,
                free_mb: 1692,
            }),
            0,
        ));

        std::env::remove_var("AXON_GPU_RECYCLE_ON_VRAM_SUMMIT");
        std::env::remove_var("AXON_GPU_RECYCLE_IMMEDIATE_ON_VRAM_SUMMIT");
        std::env::remove_var("AXON_GPU_RECYCLE_VRAM_SUMMIT_MB");
    }

    #[test]
    fn test_gpu_stuck_recovery_reason_detects_inflight_stall() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_STUCK_RECOVERY_ENABLED", "true");
        std::env::set_var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS", "1000");

        let mut metrics = crate::service_guard::vector_runtime_metrics();
        metrics.embed_inflight_started_at_ms =
            chrono::Utc::now().timestamp_millis().max(0) as u64 - 1_500;
        metrics.embed_inflight_texts_current = 12;
        metrics.embed_inflight_text_bytes_current = 4_096;

        let reason = super::gpu_stuck_recovery_reason(metrics, 0).expect("recovery reason");
        assert!(reason.contains("embed_inflight_stuck"), "{reason}");

        std::env::remove_var("AXON_GPU_STUCK_RECOVERY_ENABLED");
        std::env::remove_var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS");
    }

    #[test]
    fn test_gpu_stuck_recovery_reason_detects_ready_stock_stall() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_GPU_STUCK_RECOVERY_ENABLED", "true");
        std::env::set_var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS", "1000");
        std::env::set_var("AXON_GPU_STUCK_RECOVERY_READY_AGE_MS", "2000");

        let mut metrics = crate::service_guard::vector_runtime_metrics();
        metrics.ready_queue_chunks_current = 256;
        metrics.oldest_ready_batch_age_ms_current = 3_500;
        metrics.last_embed_gap_ms = 1_500;

        let reason = super::gpu_stuck_recovery_reason(metrics, 0).expect("recovery reason");
        assert!(reason.contains("ready_stock_stalled"), "{reason}");

        std::env::remove_var("AXON_GPU_STUCK_RECOVERY_ENABLED");
        std::env::remove_var("AXON_GPU_STUCK_RECOVERY_IDLE_GAP_MS");
        std::env::remove_var("AXON_GPU_STUCK_RECOVERY_READY_AGE_MS");
    }

    #[test]
    fn test_prepare_depth_and_worker_overrides_are_not_clamped() {
        let _guard = lock_env_guard();
        std::env::set_var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH", "12");
        std::env::set_var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR", "8");

        assert_eq!(super::configured_vector_prepare_pipeline_depth(), 12);
        assert_eq!(super::configured_vector_prepare_workers_per_vector(), 8);

        std::env::remove_var("AXON_VECTOR_PREPARE_PIPELINE_DEPTH");
        std::env::remove_var("AXON_VECTOR_PREPARE_WORKERS_PER_VECTOR");
    }

    #[test]
    fn test_vector_batch_controller_does_not_expand_file_window_when_chunk_density_is_already_good()
    {
        let cpu_config = EmbeddingLaneConfig {
            query_workers: 1,
            vector_workers: 1,
            graph_workers: 0,
            chunk_batch_size: 16,
            file_vectorization_batch_size: 8,
            graph_batch_size: 4,
            max_chunks_per_file: 64,
            max_embed_batch_bytes: 512 * 1024,
        };
        let mut controller = VectorBatchController::new(&cpu_config);

        let diagnostics = controller.observe(
            10_000,
            controller_observation_with_runtime(4_096, false, false, 4, 116, 4, 88_000, 12, 0),
        );

        assert_eq!(diagnostics.state, VectorBatchControllerState::IdleDrain);
        assert_eq!(diagnostics.target_embed_batch_chunks, 64);
        assert_eq!(diagnostics.target_files_per_cycle, 24);
        assert_eq!(diagnostics.reason, "ready_queue_starved");
        assert_eq!(diagnostics.adjustments_total, 1);
    }

    #[test]
    fn test_vector_batch_controller_shrinks_aggressively_under_gpu_memory_pressure() {
        let mut controller = VectorBatchController::new(&controller_test_config());
        controller.observe(
            10_000,
            controller_observation(4_096, false, false, 4, 64, 4, 20_480),
        );

        let diagnostics = controller.observe(
            21_000,
            controller_observation(4_096, false, true, 8, 128, 8, 40_960),
        );

        assert_eq!(diagnostics.reason, "gpu_memory_pressure");
        assert_eq!(
            diagnostics.state,
            VectorBatchControllerState::GpuMemoryGuarded
        );
        assert_eq!(diagnostics.target_embed_batch_chunks, 16);
        assert_eq!(diagnostics.target_files_per_cycle, 4);
        assert_eq!(diagnostics.adjustments_total, 2);
    }

    #[test]
    fn test_vector_batch_controller_surfaces_gpu_memory_pressure_during_warmup() {
        let mut controller = VectorBatchController::new(&controller_test_config());

        let diagnostics = controller.observe(
            10_000,
            controller_observation(4_096, false, true, 1, 8, 1, 1_024),
        );

        assert_eq!(
            diagnostics.state,
            VectorBatchControllerState::GpuMemoryGuarded
        );
        assert_eq!(diagnostics.reason, "gpu_memory_pressure");
        assert_eq!(diagnostics.target_embed_batch_chunks, 16);
        assert_eq!(diagnostics.target_files_per_cycle, 4);
    }

    #[test]
    fn test_request_query_embedding_round_trips_through_registered_worker() {
        let (tx, rx) = unbounded::<QueryEmbeddingRequest>();
        std::thread::spawn(move || {
            let request = rx.recv().unwrap();
            request.reply.send(Ok(vec![vec![0.42, 0.24]])).unwrap();
        });

        let embeddings = request_query_embedding(&tx, vec!["hello".to_string()]).unwrap();
        assert_eq!(embeddings, vec![vec![0.42, 0.24]]);
    }

    #[test]
    fn test_request_query_embedding_returns_worker_disconnect_error() {
        let (tx, rx) = unbounded::<QueryEmbeddingRequest>();
        drop(rx);

        let err = request_query_embedding(&tx, vec!["hello".to_string()]).unwrap_err();
        assert!(err
            .to_string()
            .contains("MCP real-time embedding worker unavailable"));
    }
}
