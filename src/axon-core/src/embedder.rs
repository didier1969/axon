use crate::embedding_contract::{
    fastembed_model, CHUNK_MODEL_ID, DIMENSION, GRAPH_MODEL_ID, MAX_LENGTH, MODEL_NAME,
    MODEL_VERSION, SYMBOL_MODEL_ID,
};
use crate::graph::GraphStore;
use crate::graph_ingestion::{
    FileVectorizationLeaseSnapshot, FileVectorizationWork, GraphProjectionWork, VectorBatchRun,
    INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS, INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT,
};
use crate::queue::QueueStore;
use crate::runtime_profile::{recommend_embedding_lane_sizing, RuntimeProfile};
use crate::runtime_tuning::{
    current_runtime_tuning_snapshot as runtime_tuning_snapshot,
    current_runtime_tuning_state as runtime_tuning_state,
    update_runtime_tuning_state as update_shared_runtime_tuning_state, RuntimeTuningSnapshot,
    RuntimeTuningState,
};
use crate::service_guard::{self, ServicePressure};
use crate::vector_control::{
    graph_projection_allowed, observe_vector_batch_controller,
    reset_vector_batch_controller_for_tests, semantic_policy, symbol_embedding_allowed,
    vector_claim_target, vector_ready_reserve_target, vector_worker_admitted,
    VectorBatchControllerObservation,
};
use crate::vector_pipeline::{
    ClaimedLeaseSet, FinalizeEnvelope, InflightPersistRequest, PersistEnvelope,
    PreparedBatchEnvelope,
};
use anyhow::{anyhow, Result as AnyhowResult};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use fastembed::{InitOptions, OutputKey, Pooling, SingleBatchOutput, TextEmbedding};
use libloading::Library;
use ort::ep;
use ort::execution_providers::ExecutionProviderDispatch;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tokenizers::{
    AddedToken, Encoding, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams,
};
use tracing::{debug, error, info, warn};

const FILE_PROJECTION_RADIUS: i64 = 2;
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);
const VECTOR_FINALIZE_QUEUE_BOUND: usize = 8;
const VECTOR_PERSIST_QUEUE_BOUND: usize = 1;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 512 * 1024;
const DEFAULT_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS: u64 = 30_000;
const DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProviderDiagnostics {
    pub provider_requested: String,
    pub provider_effective: String,
    pub ort_strategy: String,
    pub ort_dylib_path: Option<String>,
    pub provider_init_error: Option<String>,
}

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

struct QueryEmbeddingRequest {
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
    reply: Sender<PreparedVectorEmbedSequence>,
}

type PreparedVectorEmbedSequenceReply = Receiver<PreparedVectorEmbedSequence>;

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
static EMBEDDING_PROVIDER_DIAGNOSTICS: OnceLock<Mutex<EmbeddingProviderDiagnostics>> =
    OnceLock::new();
static GPU_MEMORY_SNAPSHOT_CACHE: OnceLock<Mutex<Option<CachedGpuMemorySnapshot>>> =
    OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuMemorySnapshot {
    pub total_mb: u64,
    pub used_mb: u64,
    pub free_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuUtilizationSnapshot {
    pub gpu_utilization_ratio: f64,
    pub memory_utilization_ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuTelemetryBackend {
    None,
    Nvml,
    NvidiaSmi,
}

#[derive(Debug, Clone, Copy)]
struct CachedGpuMemorySnapshot {
    captured_at: Instant,
    snapshot: Option<GpuMemorySnapshot>,
}

pub struct SemanticWorkerPool {
    _query_workers: Vec<thread::JoinHandle<()>>,
    _vector_prepare_workers: Vec<thread::JoinHandle<()>>,
    _vector_workers: Vec<thread::JoinHandle<()>>,
    _vector_persist_workers: Vec<thread::JoinHandle<()>>,
    _vector_finalize_workers: Vec<thread::JoinHandle<()>>,
    _vector_maintenance_workers: Vec<thread::JoinHandle<()>>,
    _graph_workers: Vec<thread::JoinHandle<()>>,
}

#[derive(Debug, Clone)]
struct VectorChunkWorkItem {
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

#[derive(Debug)]
pub(crate) struct PreparedVectorEmbedBatch {
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
struct PreparedVectorEmbedSequence {
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
                thread::sleep(Duration::from_secs(1));
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
            let _ = join_handle.join();
        }
    }
}

#[derive(Debug)]
pub(crate) struct VectorPersistPlan {
    updates: Vec<(String, String, Vec<f32>)>,
    completed_works: Vec<FileVectorizationWork>,
    next_active_after_failure: Vec<FileVectorizationWork>,
    touched_works: Vec<FileVectorizationWork>,
}

#[derive(Debug)]
pub(crate) struct VectorPersistOutcome {
    completed_works: Vec<FileVectorizationWork>,
    batch_runs: Vec<VectorBatchRun>,
    next_active_after_failure: Vec<FileVectorizationWork>,
    touched_works: Vec<FileVectorizationWork>,
    error_reason: Option<String>,
}

#[derive(Debug)]
struct VectorFinalizeOutcome {
    completed_works: Vec<FileVectorizationWork>,
    batch_runs: Vec<VectorBatchRun>,
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

fn export_fastembed_batches(batches: Vec<SingleBatchOutput>) -> AnyhowResult<Vec<Vec<f32>>> {
    let mut embeddings = Vec::new();
    for batch in batches {
        let pooled =
            batch.select_and_pool_output(&FASTEMBED_OUTPUT_PRECEDENCE, Some(Pooling::Cls))?;
        for row in pooled.rows() {
            let values = row
                .as_slice()
                .ok_or_else(|| anyhow!("failed to convert pooled embedding row to slice"))?;
            embeddings.push(normalize_embedding(values.to_vec()));
        }
    }
    Ok(embeddings)
}

fn configured_embedding_max_length() -> usize {
    std::env::var("AXON_EMBED_MAX_LENGTH")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 32)
        .unwrap_or(MAX_LENGTH)
        .min(MAX_LENGTH)
}

fn configured_embedding_token_bucket_size() -> usize {
    std::env::var("AXON_EMBED_TOKEN_BUCKET_SIZE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 8)
        .unwrap_or(64)
        .min(configured_embedding_max_length().max(8))
}

fn bootstrap_embedding_lane_config_from_env() -> EmbeddingLaneConfig {
    let query_workers = env_usize("AXON_QUERY_EMBED_WORKERS", 1);
    let requested_vector_workers = env_usize("AXON_VECTOR_WORKERS", 1);
    let oversubscription_allowed = std::env::var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let vector_workers = if embedding_provider_requested_is_gpu()
        && requested_vector_workers > 1
        && !oversubscription_allowed
    {
        1
    } else {
        requested_vector_workers
    };

    EmbeddingLaneConfig {
        query_workers,
        vector_workers,
        graph_workers: env_usize_nonnegative("AXON_GRAPH_WORKERS", 1),
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
        .unwrap_or(10);
    let vector_persist_queue_bound = std::env::var("AXON_VECTOR_PERSIST_QUEUE_BOUND")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(VECTOR_PERSIST_QUEUE_BOUND.max(6));
    let vector_max_inflight_persists = std::env::var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(vector_persist_queue_bound.max(1).min(4))
        .max(1);
    RuntimeTuningState {
        vector_workers: lane_config.vector_workers,
        chunk_batch_size: lane_config.chunk_batch_size,
        file_vectorization_batch_size: lane_config.file_vectorization_batch_size,
        vector_ready_queue_depth,
        vector_persist_queue_bound,
        vector_max_inflight_persists,
        embed_micro_batch_max_items,
        embed_micro_batch_max_total_tokens,
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

fn configured_vector_ready_queue_depth() -> usize {
    current_runtime_tuning_snapshot()
        .state
        .vector_ready_queue_depth
        .max(1)
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

fn token_count_from_encoding(encoding: &Encoding) -> usize {
    encoding
        .get_attention_mask()
        .iter()
        .map(|value| *value as usize)
        .sum::<usize>()
        .max(1)
}

fn runtime_fastembed_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var("FASTEMBED_CACHE_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }

    if let Some(path) = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path).join("axon").join("fastembed");
    }

    embedding_model_cache_dir()
}

fn runtime_embedding_snapshot_dir() -> AnyhowResult<PathBuf> {
    let model_root = runtime_fastembed_cache_dir().join("models--Xenova--bge-large-en-v1.5");
    let snapshot_ref = model_root.join("refs").join("main");
    let snapshot = fs::read_to_string(&snapshot_ref)
        .map_err(|err| anyhow!("failed to read {}: {}", snapshot_ref.display(), err))?;
    Ok(model_root.join("snapshots").join(snapshot.trim()))
}

fn load_runtime_embedding_tokenizer() -> AnyhowResult<Tokenizer> {
    let snapshot_dir = runtime_embedding_snapshot_dir()?;
    let tokenizer_json = snapshot_dir.join("tokenizer.json");
    let config_json = snapshot_dir.join("config.json");
    let special_tokens_map_json = snapshot_dir.join("special_tokens_map.json");
    let tokenizer_config_json = snapshot_dir.join("tokenizer_config.json");

    let config: serde_json::Value = serde_json::from_slice(
        &fs::read(&config_json)
            .map_err(|err| anyhow!("failed to read {}: {}", config_json.display(), err))?,
    )
    .map_err(|err| anyhow!("failed to parse {}: {}", config_json.display(), err))?;
    let special_tokens_map: serde_json::Value =
        serde_json::from_slice(&fs::read(&special_tokens_map_json).map_err(|err| {
            anyhow!(
                "failed to read {}: {}",
                special_tokens_map_json.display(),
                err
            )
        })?)
        .map_err(|err| {
            anyhow!(
                "failed to parse {}: {}",
                special_tokens_map_json.display(),
                err
            )
        })?;
    let tokenizer_config: serde_json::Value =
        serde_json::from_slice(&fs::read(&tokenizer_config_json).map_err(|err| {
            anyhow!(
                "failed to read {}: {}",
                tokenizer_config_json.display(),
                err
            )
        })?)
        .map_err(|err| {
            anyhow!(
                "failed to parse {}: {}",
                tokenizer_config_json.display(),
                err
            )
        })?;

    let model_max_length = tokenizer_config["model_max_length"]
        .as_f64()
        .ok_or_else(|| anyhow!("tokenizer_config.json missing model_max_length"))?
        as usize;
    let max_length = configured_embedding_max_length().min(model_max_length);
    let pad_id = config["pad_token_id"].as_u64().unwrap_or(0) as u32;
    let pad_token = tokenizer_config["pad_token"]
        .as_str()
        .ok_or_else(|| anyhow!("tokenizer_config.json missing pad_token"))?
        .to_string();

    let mut tokenizer = Tokenizer::from_file(&tokenizer_json)
        .map_err(|err| anyhow!("{}: {}", tokenizer_json.display(), err))?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        pad_token,
        pad_id,
        ..Default::default()
    }));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length,
            ..Default::default()
        }))
        .map_err(|err| anyhow!("failed to configure tokenizer truncation: {}", err))?;

    if let serde_json::Value::Object(root_object) = special_tokens_map {
        for value in root_object.values() {
            if let Some(content) = value.as_str() {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    special: true,
                    ..Default::default()
                }]);
            } else if let (
                Some(content),
                Some(single_word),
                Some(lstrip),
                Some(rstrip),
                Some(normalized),
            ) = (
                value.get("content").and_then(|v| v.as_str()),
                value.get("single_word").and_then(|v| v.as_bool()),
                value.get("lstrip").and_then(|v| v.as_bool()),
                value.get("rstrip").and_then(|v| v.as_bool()),
                value.get("normalized").and_then(|v| v.as_bool()),
            ) {
                tokenizer.add_special_tokens(&[AddedToken {
                    content: content.to_string(),
                    single_word,
                    lstrip,
                    rstrip,
                    normalized,
                    special: true,
                    ..Default::default()
                }]);
            }
        }
    }

    Ok(tokenizer)
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

fn build_prepared_vector_embed_sequence(
    graph_store: &GraphStore,
    active_works: &[FileVectorizationWork],
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
) -> PreparedVectorEmbedSequence {
    let mut batches = Vec::new();
    let mut current_active = active_works.to_vec();
    let mut reserved_chunk_ids = HashSet::new();
    while !current_active.is_empty() && batches.len() < target_ready_depth.max(1) {
        let prepared = prepare_vector_embed_batch(
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
        batches.push(PreparedBatchEnvelope::new(prepared));
        if !made_progress {
            break;
        }
    }

    PreparedVectorEmbedSequence {
        batches,
        remaining_claimed_after_success: ClaimedLeaseSet::new(current_active),
    }
}

fn embed_texts_with_breakdown(
    model: &mut TextEmbedding,
    texts: &[String],
) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64)> {
    if texts.is_empty() {
        return Ok((Vec::new(), 0, 0));
    }

    let total_started = Instant::now();
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
    let mut export_ms = 0u64;

    for batch_indices in micro_batches {
        let batch_encodings = batch_indices
            .iter()
            .map(|index| encodings[*index].clone())
            .collect::<Vec<_>>();
        let batches = model.transform_encoded(&batch_encodings)?;
        let export_started = Instant::now();
        let batch_embeddings = export_fastembed_batches(batches.into_raw())?;
        export_ms = export_ms.saturating_add(export_started.elapsed().as_millis() as u64);

        for (index, embedding) in batch_indices.into_iter().zip(batch_embeddings) {
            ordered_embeddings[index] = Some(embedding);
        }
    }

    let embeddings = ordered_embeddings
        .into_iter()
        .map(|embedding| {
            embedding.ok_or_else(|| anyhow!("missing embedding after micro-batch scheduling"))
        })
        .collect::<AnyhowResult<Vec<_>>>()?;
    let total_ms = total_started.elapsed().as_millis() as u64;
    let transform_ms = total_ms.saturating_sub(export_ms);

    Ok((embeddings, transform_ms, export_ms))
}

fn embed_prepared_batch_with_breakdown(
    model: &mut TextEmbedding,
    prepared: &PreparedVectorEmbedBatch,
) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64)> {
    if !prepared.encoded_micro_batches.is_empty() {
        let total_started = Instant::now();
        let mut ordered_embeddings = vec![None; prepared.texts.len()];
        let mut export_ms = 0u64;

        for micro_batch in &prepared.encoded_micro_batches {
            let batches = model.transform_encoded(&micro_batch.encodings)?;
            let export_started = Instant::now();
            let batch_embeddings = export_fastembed_batches(batches.into_raw())?;
            export_ms = export_ms.saturating_add(export_started.elapsed().as_millis() as u64);
            for (index, embedding) in micro_batch
                .item_indices
                .iter()
                .copied()
                .zip(batch_embeddings)
            {
                ordered_embeddings[index] = Some(embedding);
            }
        }

        let embeddings = ordered_embeddings
            .into_iter()
            .map(|embedding| {
                embedding.ok_or_else(|| anyhow!("missing embedding after prepared micro-batch"))
            })
            .collect::<AnyhowResult<Vec<_>>>()?;
        let total_ms = total_started.elapsed().as_millis() as u64;
        let transform_ms = total_ms.saturating_sub(export_ms);
        return Ok((embeddings, transform_ms, export_ms));
    }

    embed_texts_with_breakdown(model, &prepared.texts)
}

fn cuda_execution_provider_dispatch() -> ExecutionProviderDispatch {
    let mut cuda = ep::CUDA::default()
        .with_device_id(0)
        .with_memory_limit(cuda_memory_limit_bytes())
        .with_arena_extend_strategy(ort::ep::ArenaExtendStrategy::SameAsRequested)
        .with_conv_max_workspace(false)
        .with_conv_algorithm_search(ort::ep::cuda::ConvAlgorithmSearch::Heuristic);
    if cuda_tf32_enabled() {
        cuda = cuda.with_tf32(true);
    }
    ExecutionProviderDispatch::from(cuda.build()).error_on_failure()
}

fn cuda_memory_limit_bytes() -> usize {
    (std::env::var("AXON_CUDA_MEMORY_LIMIT_MB")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value >= 512)
        .map(|value| value as u64)
        .unwrap_or_else(gpu_memory_soft_limit_mb)
        .max(512) as usize)
        .saturating_mul(1024 * 1024)
}

fn cuda_tf32_enabled() -> bool {
    std::env::var("AXON_CUDA_ALLOW_TF32")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn ort_cuda_provider_library_path() -> Option<PathBuf> {
    let ort_dylib_path = std::env::var("ORT_DYLIB_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let ort_dir = Path::new(&ort_dylib_path).parent()?;
    Some(ort_dir.join("libonnxruntime_providers_cuda.so"))
}

fn ort_cuda_provider_library_available() -> bool {
    ort_cuda_provider_library_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

fn cpu_provider_effective_label(
    cuda_requested: bool,
    cuda_available: bool,
    cuda_provider_library_available: bool,
) -> &'static str {
    if cuda_requested && cuda_available && !cuda_provider_library_available {
        "cpu_missing_cuda_provider"
    } else {
        "cpu"
    }
}

fn effective_provider_request_for_lane(lane: &str) -> String {
    let normalized_lane = lane.trim().to_ascii_lowercase();
    if normalized_lane == "query" {
        if let Some(explicit) = std::env::var("AXON_QUERY_EMBED_PROVIDER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
        {
            return explicit;
        }

        if embedding_provider_requested_is_gpu() {
            return "cpu".to_string();
        }
    }

    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cpu".to_string())
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
    fn from_plan(plan: VectorBatchPlan) -> Self {
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
        let texts = work_items.iter().map(|item| item.text.clone()).collect();
        let mut next_active_after_success = continuation_works;
        next_active_after_success.extend(untouched_works.clone());
        Self {
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

    pub(crate) fn into_persist_plan(
        self,
        embeddings: Vec<Vec<f32>>,
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
        })
    }
}

impl SemanticWorkerPool {
    pub fn new(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) -> Self {
        let config = embedding_lane_config_from_env();
        info!(
            "Semantic Factory: Spawning split semantic runtime (query_workers={}, vector_workers={}, graph_workers={}, chunk_batch_size={}, file_batch_size={}, graph_batch_size={})",
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
        for worker_idx in 0..config.vector_workers {
            let graph_store = Arc::clone(&graph_store);
            let prepare_graph_store = Arc::clone(&graph_store);
            let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
            let (persist_tx, persist_rx) =
                bounded::<VectorPersistRequest>(configured_vector_persist_queue_bound());
            let (finalize_tx, finalize_rx) =
                bounded::<VectorFinalizeRequest>(VECTOR_FINALIZE_QUEUE_BOUND);
            vector_prepare_workers.push(thread::spawn(move || {
                Self::vector_prepare_worker_loop(worker_idx, prepare_graph_store, prepare_rx);
            }));
            let persist_graph_store = Arc::clone(&graph_store);
            vector_persist_workers.push(thread::spawn(move || {
                Self::vector_persist_worker_loop(worker_idx, persist_graph_store, persist_rx);
            }));
            let finalize_graph_store = Arc::clone(&graph_store);
            vector_finalize_workers.push(thread::spawn(move || {
                Self::vector_finalize_worker_loop(worker_idx, finalize_graph_store, finalize_rx);
            }));
            vector_workers.push(thread::spawn(move || {
                Self::vector_worker_loop(
                    worker_idx,
                    graph_store,
                    prepare_tx,
                    persist_tx,
                    finalize_tx,
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
            _vector_workers: vector_workers,
            _vector_persist_workers: vector_persist_workers,
            _vector_finalize_workers: vector_finalize_workers,
            _vector_maintenance_workers: vector_maintenance_workers,
            _graph_workers: graph_workers,
        }
    }

    fn vector_maintenance_worker_loop(graph_store: Arc<GraphStore>) {
        info!("Semantic Vector Maintenance Worker: stale inflight recovery enabled");
        loop {
            thread::sleep(Duration::from_millis(
                vector_stale_inflight_recovery_interval_ms(),
            ));
            match recover_stale_vector_inflight_now(
                &graph_store,
                chrono::Utc::now().timestamp_millis(),
            ) {
                Ok(recovered) if recovered > 0 => info!(
                    "Semantic Vector Maintenance Worker: recovered {} stale inflight vectorization jobs",
                    recovered
                ),
                Ok(_) => {}
                Err(err) => error!(
                    "Semantic Vector Maintenance Worker: failed to recover stale inflight vectorization jobs: {:?}",
                    err
                ),
            }
        }
    }

    fn query_worker_loop(worker_idx: usize, query_rx: Receiver<QueryEmbeddingRequest>) {
        info!(
            "Semantic Query Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
            worker_idx
        );

        let Some(mut model) = Self::build_text_embedding_model("query", worker_idx) else {
            return;
        };

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
        prepare_tx: Sender<VectorPrepareRequest>,
        persist_tx: Sender<VectorPersistRequest>,
        finalize_tx: Sender<VectorFinalizeRequest>,
    ) {
        let _liveness = VectorWorkerLivenessGuard::new();
        info!(
            "Semantic Vector Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
            worker_idx
        );

        let Some(mut model) = Self::build_text_embedding_model("vector", worker_idx) else {
            return;
        };
        let lane_config = embedding_lane_config_from_env();
        let gpu_available = effective_embedding_provider_is_gpu();

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

        loop {
            service_guard::record_vector_worker_heartbeat();
            let current_pressure = service_guard::current_pressure();
            let (file_vectorization_queue_queued, file_vectorization_queue_inflight) = graph_store
                .fetch_file_vectorization_queue_counts()
                .unwrap_or((0, 0));
            let file_backlog_depth =
                file_vectorization_queue_queued + file_vectorization_queue_inflight;
            if !vector_worker_admitted(
                worker_idx,
                current_pressure,
                gpu_available,
                file_backlog_depth,
            ) {
                thread::sleep(Duration::from_millis(500));
                continue;
            }
            let policy = semantic_policy(file_backlog_depth, current_pressure);
            if policy.pause {
                thread::sleep(policy.sleep);
                continue;
            }

            let mut backlog_active = false;
            let controller = observe_vector_batch_controller(
                &lane_config,
                VectorBatchControllerObservation {
                    queue_pending: file_backlog_depth,
                    interactive_active: service_guard::interactive_priority_active()
                        || service_guard::interactive_requests_in_flight() > 0,
                    gpu_memory_pressure: current_gpu_memory_pressure_active(),
                    metrics: service_guard::vector_runtime_metrics(),
                },
            );
            let initial_ready_target = vector_ready_reserve_target(
                configured_vector_ready_queue_depth(),
                file_backlog_depth,
                controller.target_files_per_cycle,
                controller.target_embed_batch_chunks,
                0,
            );
            match graph_store.fetch_pending_file_vectorization_work(vector_claim_target(
                controller.target_files_per_cycle,
                controller.avg_files_per_embed_call,
                controller.target_embed_batch_chunks,
                controller.avg_chunks_per_embed_call,
                initial_ready_target,
                0,
                file_backlog_depth,
            )) {
                Ok(pending) if !pending.is_empty() => {
                    backlog_active = true;
                    debug!(
                        "Semantic Vector Worker [{}]: Embedding {} file vectorization jobs...",
                        worker_idx,
                        pending.len()
                    );
                    service_guard::record_vector_claimed_work_items(pending.len() as u64);

                    let mut completed_works: Vec<FileVectorizationWork> = Vec::new();
                    let mut completed_batch_runs: Vec<VectorBatchRun> = Vec::new();
                    let mut failed: HashMap<String, Vec<FileVectorizationWork>> = HashMap::new();

                    let mut active_works = pending;
                    let mut estimated_queue_pending =
                        file_backlog_depth.saturating_sub(active_works.len());
                    let mut ready_batches: VecDeque<PreparedBatchEnvelope> = VecDeque::new();
                    let mut inflight_persists: VecDeque<InflightPersistRequest> = VecDeque::new();
                    let max_inflight_persists = configured_vector_max_inflight_persists();
                    while !active_works.is_empty()
                        || !ready_batches.is_empty()
                        || !inflight_persists.is_empty()
                    {
                        while let Some(inflight) = inflight_persists.front() {
                            match inflight.reply_rx.try_recv() {
                                Ok(outcome) => {
                                    inflight_persists.pop_front();
                                    apply_vector_persist_outcome(
                                        outcome,
                                        &mut active_works,
                                        &mut ready_batches,
                                        &mut completed_works,
                                        &mut completed_batch_runs,
                                        &mut failed,
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
                        let owned_ready_works = ready_batches
                            .iter()
                            .flat_map(|batch| batch.touched_works.clone())
                            .collect::<Vec<_>>();
                        let owned_persist_works = inflight_persists
                            .iter()
                            .flat_map(|request| request.claimed.clone_works())
                            .collect::<Vec<_>>();
                        let current_vector_owned = merge_unique_vectorization_work_sets([
                            active_works.clone(),
                            owned_ready_works,
                            owned_persist_works,
                            completed_works.clone(),
                        ]);
                        let _ = graph_store.refresh_file_vectorization_leases_for_owner(
                            &current_vector_owned,
                            "vector",
                        );
                        if !completed_works.is_empty() {
                            let _ = graph_store
                                .refresh_inflight_file_vectorization_claims(&completed_works);
                        }
                        let controller = observe_vector_batch_controller(
                            &lane_config,
                            VectorBatchControllerObservation {
                                queue_pending: estimated_queue_pending
                                    + active_works.len()
                                    + ready_batches.len()
                                    + inflight_persists.len(),
                                interactive_active: service_guard::interactive_priority_active()
                                    || service_guard::interactive_requests_in_flight() > 0,
                                gpu_memory_pressure: current_gpu_memory_pressure_active(),
                                metrics: service_guard::vector_runtime_metrics(),
                            },
                        );
                        let target_ready_depth = vector_ready_reserve_target(
                            configured_vector_ready_queue_depth(),
                            estimated_queue_pending
                                + active_works.len()
                                + ready_batches.len()
                                + inflight_persists.len(),
                            controller.target_files_per_cycle,
                            controller.target_embed_batch_chunks,
                            ready_batches.len(),
                        );
                        let claim_target = vector_claim_target(
                            controller.target_files_per_cycle,
                            controller.avg_files_per_embed_call,
                            controller.target_embed_batch_chunks,
                            controller.avg_chunks_per_embed_call,
                            target_ready_depth,
                            ready_batches.len(),
                            estimated_queue_pending
                                + active_works.len()
                                + ready_batches.len()
                                + inflight_persists.len(),
                        );
                        if active_works.len() < claim_target {
                            if let Ok(top_up) = graph_store.fetch_pending_file_vectorization_work(
                                claim_target.saturating_sub(active_works.len()),
                            ) {
                                estimated_queue_pending =
                                    estimated_queue_pending.saturating_sub(top_up.len());
                                active_works =
                                    merge_vectorization_work(active_works, top_up, claim_target);
                            }
                        }

                        let mut uninterrupted = Vec::new();
                        for work in active_works.drain(..) {
                            if work.resumed_after_interactive_pause {
                                service_guard::record_vectorization_resumed_after_interactive(1);
                            }
                            if !pause_vectorization_work_if_interactive(&graph_store, &work) {
                                uninterrupted.push(work);
                            }
                        }
                        active_works = uninterrupted;
                        let target_chunks = controller.target_embed_batch_chunks.max(1);
                        if ready_batches.len() < target_ready_depth && !active_works.is_empty() {
                            let _ = graph_store
                                .refresh_inflight_file_vectorization_claims(&active_works);
                            let gpu_idle_started = Instant::now();
                            let sequence = match request_prepared_vector_embed_sequence(
                                &graph_store,
                                worker_idx,
                                &prepare_tx,
                                active_works.clone(),
                                target_chunks,
                                lane_config.max_chunks_per_file,
                                lane_config.max_embed_batch_bytes,
                                target_ready_depth.saturating_sub(ready_batches.len()),
                            ) {
                                Ok(sequence) => sequence,
                                Err(err) => {
                                    service_guard::record_vector_prepare_fallback_inline();
                                    error!(
                                        "Semantic Vector Worker [{}]: prepare queue unavailable, falling back inline: {:?}",
                                        worker_idx, err
                                    );
                                    let prepared = prepare_vector_embed_batch(
                                        &graph_store,
                                        &active_works,
                                        target_chunks,
                                        lane_config.max_chunks_per_file,
                                        lane_config.max_embed_batch_bytes,
                                        &HashSet::new(),
                                    );
                                    PreparedVectorEmbedSequence {
                                        remaining_claimed_after_success: ClaimedLeaseSet::new(
                                            prepared.next_active_after_success.clone(),
                                        ),
                                        batches: vec![PreparedBatchEnvelope::new(prepared)],
                                    }
                                }
                            };
                            service_guard::record_vector_gpu_idle_wait_ms(
                                gpu_idle_started.elapsed().as_millis() as u64,
                            );
                            for prefetched in sequence.batches {
                                ready_batches.push_back(prefetched);
                                service_guard::record_vector_prepare_prefetch();
                            }
                            active_works = sequence.remaining_claimed_after_success.into_inner();
                            service_guard::record_vector_ready_queue_depth(
                                ready_batches.len() as u64
                            );
                        }

                        let Some(prepared) = ready_batches.pop_front() else {
                            if let Some(inflight) = inflight_persists.pop_front() {
                                if let Some(outcome) = wait_for_vector_persist_outcome(
                                    &graph_store,
                                    worker_idx,
                                    inflight.reply_rx,
                                    inflight.claimed.as_slice(),
                                ) {
                                    apply_vector_persist_outcome(
                                        outcome,
                                        &mut active_works,
                                        &mut ready_batches,
                                        &mut completed_works,
                                        &mut completed_batch_runs,
                                        &mut failed,
                                    );
                                }
                                flush_completed_vectorization_works(
                                    worker_idx,
                                    &graph_store,
                                    &finalize_tx,
                                    &mut completed_works,
                                    &mut completed_batch_runs,
                                    &mut failed,
                                );
                                service_guard::record_vector_ready_queue_depth(0);
                                continue;
                            }
                            break;
                        };
                        if service_guard::interactive_priority_active() {
                            let mut interrupted_batches = VecDeque::new();
                            let mut preserved_batches = VecDeque::new();
                            for batch in ready_batches.drain(..) {
                                let mut interrupted = false;
                                for work in &batch.touched_works {
                                    if pause_vectorization_work_if_interactive(&graph_store, work) {
                                        interrupted = true;
                                    }
                                }
                                if interrupted {
                                    interrupted_batches.push_back(batch);
                                } else {
                                    preserved_batches.push_back(batch);
                                }
                            }
                            ready_batches = preserved_batches;
                            if !interrupted_batches.is_empty() {
                                service_guard::record_vector_ready_queue_depth(
                                    ready_batches.len() as u64
                                );
                            }
                        }
                        service_guard::record_vector_ready_queue_depth(ready_batches.len() as u64);

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
                                    .entry(
                                        "failed to mark oversized_for_current_budget".to_string(),
                                    )
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
                        let batch_started_at_ms = chrono::Utc::now().timestamp_millis();
                        let embed_clone_ms = 0_u64;
                        service_guard::record_vector_embed_inputs(
                            embed_input_texts,
                            embed_input_text_bytes,
                            embed_clone_ms,
                        );
                        if let Err(err) = graph_store
                            .refresh_inflight_file_vectorization_claims(&prepared.touched_works)
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
                            active_works.clone(),
                            ready_batches
                                .iter()
                                .flat_map(|batch| batch.touched_works.clone())
                                .collect::<Vec<_>>(),
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
                        let embed_started = Instant::now();
                        let _ =
                            graph_store.mark_file_vectorization_started(&prepared.touched_works);
                        match embed_prepared_batch_with_breakdown(&mut model, &prepared) {
                            Ok((embeddings, transform_ms, export_ms)) => {
                                service_guard::record_vector_embed_attempt_finished();
                                service_guard::record_vector_embed_breakdown(
                                    transform_ms,
                                    export_ms,
                                );
                                service_guard::record_vector_stage_ms(
                                    service_guard::VectorStageKind::Embed,
                                    embed_started.elapsed().as_millis() as u64,
                                );
                                let touched_works = prepared.touched_works.clone();
                                let next_active_after_failure =
                                    prepared.next_active_after_failure.clone();
                                let prepared_fetch_ms_total = prepared.fetch_ms_total;
                                let persist_plan = match prepared.into_persist_envelope(
                                    embeddings,
                                    VectorBatchRun {
                                        run_id: format!(
                                            "vec-batch-{}-{}",
                                            worker_idx,
                                            chrono::Utc::now()
                                                .timestamp_nanos_opt()
                                                .unwrap_or_else(
                                                    || chrono::Utc::now().timestamp_micros()
                                                )
                                        ),
                                        started_at_ms: batch_started_at_ms,
                                        finished_at_ms: chrono::Utc::now().timestamp_millis(),
                                        provider: current_embedding_provider_diagnostics()
                                            .provider_effective,
                                        model_id: CHUNK_MODEL_ID.to_string(),
                                        chunk_count: 0,
                                        file_count: 0,
                                        input_bytes: embed_input_text_bytes,
                                        fetch_ms: prepared_fetch_ms_total,
                                        embed_ms: embed_started.elapsed().as_millis() as u64,
                                        db_write_ms: 0,
                                        mark_done_ms: 0,
                                        success: true,
                                        error_reason: None,
                                    },
                                ) {
                                    Ok(envelope) => envelope,
                                    Err(err) => {
                                        let reason = format!(
                                            "failed to build vector persist plan: {:?}",
                                            err
                                        );
                                        failed.entry(reason).or_default().extend(touched_works);
                                        ready_batches.clear();
                                        active_works = next_active_after_failure;
                                        continue;
                                    }
                                };
                                let mut persist_envelope = persist_plan;
                                persist_envelope.batch_run.chunk_count =
                                    persist_envelope.persist_plan.updates.len() as u64;
                                persist_envelope.batch_run.file_count =
                                    persist_envelope.persist_plan.touched_works.len() as u64;
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
                                        apply_vector_persist_outcome(
                                            outcome,
                                            &mut active_works,
                                            &mut ready_batches,
                                            &mut completed_works,
                                            &mut completed_batch_runs,
                                            &mut failed,
                                        );
                                    }
                                }
                                match dispatch_vector_persist_plan(&persist_tx, persist_envelope) {
                                    Ok(reply_rx) => {
                                        inflight_persists.push_back(InflightPersistRequest {
                                            reply_rx,
                                            claimed: ClaimedLeaseSet::new(touched_works),
                                        });
                                    }
                                    Err(err) => {
                                        let reason = format!(
                                            "failed to dispatch vector persist plan: {:?}",
                                            err
                                        );
                                        failed.entry(reason).or_default().extend(touched_works);
                                        ready_batches.clear();
                                        active_works = next_active_after_failure;
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
                            }
                            Err(e) => {
                                service_guard::record_vector_embed_attempt_finished();
                                let reason = format!("chunk embedding failed: {:?}", e);
                                if is_fatal_embedding_error(&e) {
                                    error!(
                                        "Semantic Vector Worker [{}]: fatal chunk embedding error, disabling semantic worker: {:?}",
                                        worker_idx, e
                                    );
                                    return;
                                }
                                error!(
                                    "Semantic Vector Worker [{}]: Chunk embedding failed: {:?}",
                                    worker_idx, e
                                );
                                ready_batches.clear();
                                failed
                                    .entry(reason)
                                    .or_default()
                                    .extend(prepared.touched_works.iter().cloned());
                                active_works = prepared.next_active_after_failure.clone();
                            }
                        }
                    }

                    while let Some(inflight) = inflight_persists.pop_front() {
                        if let Some(outcome) = wait_for_vector_persist_outcome(
                            &graph_store,
                            worker_idx,
                            inflight.reply_rx,
                            inflight.claimed.as_slice(),
                        ) {
                            apply_vector_persist_outcome(
                                outcome,
                                &mut active_works,
                                &mut ready_batches,
                                &mut completed_works,
                                &mut completed_batch_runs,
                                &mut failed,
                            );
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

                    for (reason, works) in failed {
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
                Ok(_) => {}
                Err(e) => error!(
                    "Semantic Vector Worker [{}]: File vectorization fetch error: {:?}",
                    worker_idx, e
                ),
            }

            if !symbol_embedding_allowed(file_backlog_depth, current_pressure) {
                if !backlog_active {
                    thread::sleep(policy.idle_sleep);
                }
                continue;
            }

            match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
                Ok(symbols) if !symbols.is_empty() => {
                    backlog_active = true;
                    debug!(
                        "Semantic Vector Worker [{}]: Embedding {} symbols...",
                        worker_idx,
                        symbols.len()
                    );

                    let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                    match embed_texts_with_breakdown(&mut model, &texts) {
                        Ok((embeddings, transform_ms, export_ms)) => {
                            service_guard::record_vector_embed_breakdown(transform_ms, export_ms);
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
                            if is_fatal_embedding_error(&e) {
                                error!(
                                    "Semantic Vector Worker [{}]: fatal symbol embedding error, disabling semantic worker: {:?}",
                                    worker_idx, e
                                );
                                return;
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
                    thread::sleep(policy.idle_sleep);
                }
            }

            if !backlog_active {
                thread::sleep(policy.idle_sleep);
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
        while let Ok(request) = finalize_rx.recv() {
            service_guard::record_vector_finalize_queue_depth(finalize_rx.len() as u64);
            service_guard::record_vector_finalize_queue_wait_ms(
                request.enqueued_at.elapsed().as_millis() as u64,
            );
            let FinalizeEnvelope {
                completed_works,
                lease_snapshots: vector_lease_snapshots,
                batch_runs,
            } = request.envelope;
            let finalize_lease_snapshots = match graph_store
                .transfer_file_vectorization_lease_owner(
                    &vector_lease_snapshots,
                    "vector",
                    "finalize",
                ) {
                Ok(snapshots) => snapshots,
                Err(err) => {
                    error!(
                        "Semantic Vector Finalize Worker [{}]: failed to claim finalize lease ownership: {:?}",
                        worker_idx, err
                    );
                    for mut batch_run in batch_runs {
                        batch_run.mark_done_ms = 0;
                        batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                        batch_run.success = false;
                        batch_run.error_reason = Some(format!(
                            "failed to claim finalize lease ownership: {:?}",
                            err
                        ));
                        if let Err(record_err) = graph_store.record_vector_batch_run(&batch_run) {
                            error!(
                                "Semantic Vector Finalize Worker [{}]: failed to persist finalize ownership error batch run: {:?}",
                                worker_idx, record_err
                            );
                        }
                    }
                    continue;
                }
            };
            let finalize_owned_works =
                work_with_lease_snapshots(&completed_works, &finalize_lease_snapshots);
            let _ = graph_store
                .refresh_file_vectorization_leases_for_owner(&finalize_owned_works, "finalize");
            let mark_done_started = Instant::now();
            service_guard::record_vector_mark_done_call();
            let _finalize_lease_guard = LeaseRefreshGuard::start(
                Arc::clone(&graph_store),
                finalize_owned_works.clone(),
                "finalize",
            );
            match finalize_completed_vectorization_works(
                &graph_store,
                finalize_owned_works.clone(),
                finalize_lease_snapshots,
                batch_runs,
            ) {
                Ok(outcome) => {
                    let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::MarkDone,
                        mark_done_ms,
                    );
                    for mut batch_run in outcome.batch_runs {
                        batch_run.mark_done_ms = mark_done_ms;
                        batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                        if let Err(err) = graph_store.record_vector_batch_run(&batch_run) {
                            error!(
                                "Semantic Vector Finalize Worker [{}]: failed to persist finalized vector batch run: {:?}",
                                worker_idx, err
                            );
                        }
                    }
                    service_guard::record_vector_files_completed(
                        outcome.completed_works.len() as u64
                    );
                }
                Err((err, mut batch_runs)) => {
                    let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::MarkDone,
                        mark_done_ms,
                    );
                    let reason = format!("failed to mark vectorization completion: {:?}", err);
                    for batch_run in &mut batch_runs {
                        batch_run.mark_done_ms = mark_done_ms;
                        batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                        batch_run.success = false;
                        batch_run.error_reason = Some(reason.clone());
                        if let Err(batch_err) = graph_store.record_vector_batch_run(batch_run) {
                            error!(
                                "Semantic Vector Finalize Worker [{}]: failed to persist failed finalize vector batch run: {:?}",
                                worker_idx, batch_err
                            );
                        }
                    }
                    error!(
                        "Semantic Vector Finalize Worker [{}]: {}",
                        worker_idx, reason
                    );
                    if let Err(failure_err) = graph_store
                        .mark_file_vectorization_work_failed(
                            &finalize_owned_works,
                            &reason,
                        )
                    {
                        error!(
                            "Semantic Vector Finalize Worker [{}]: failed to persist finalize failure state: {:?}",
                            worker_idx, failure_err
                        );
                    }
                }
            }
        }
    }

    fn vector_prepare_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        prepare_rx: Receiver<VectorPrepareRequest>,
    ) {
        info!(
            "Semantic Vector Prepare Worker [{}]: ready with bounded queue 1",
            worker_idx
        );
        let mut tokenizer = load_runtime_embedding_tokenizer().ok();
        while let Ok(request) = prepare_rx.recv() {
            service_guard::record_vector_prepare_queue_depth(prepare_rx.len() as u64);
            service_guard::record_vector_prepare_queue_wait_ms(
                request.enqueued_at.elapsed().as_millis() as u64,
            );
            let mut sequence = build_prepared_vector_embed_sequence(
                &graph_store,
                request.claimed.as_slice(),
                request.target_chunks,
                request.per_file_fetch_limit,
                request.batch_max_bytes,
                request.target_ready_depth,
            );
            for prepared in &mut sequence.batches {
                if !prepared.texts.is_empty() {
                    if tokenizer.is_none() {
                        tokenizer = load_runtime_embedding_tokenizer().ok();
                    }
                    if let Some(active_tokenizer) = tokenizer.as_ref() {
                        match attach_preencoded_micro_batches(active_tokenizer, prepared) {
                            Ok(()) => {}
                            Err(err) => error!(
                                "Semantic Vector Prepare Worker [{}]: failed to pre-tokenize batch, falling back to inline tokenization: {:?}",
                                worker_idx, err
                            ),
                        }
                    }
                }
                service_guard::record_vector_prepare_outcome(
                    prepared.work_items.len() as u64,
                    prepared.immediate_completed.len() as u64,
                    prepared.failed_fetches.len() as u64,
                );
            }
            if request.reply.send(sequence).is_err() {
                error!(
                    "Semantic Vector Prepare Worker [{}]: embed worker dropped prepared batch reply channel",
                    worker_idx
                );
            }
        }
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
        while let Ok(request) = persist_rx.recv() {
            service_guard::record_vector_persist_queue_depth(persist_rx.len() as u64);
            service_guard::record_vector_persist_queue_wait_ms(
                request.enqueued_at.elapsed().as_millis() as u64,
            );
            let db_write_started = Instant::now();
            let PersistEnvelope {
                persist_plan,
                mut batch_run,
            } = request.envelope;
            let _persist_lease_guard = LeaseRefreshGuard::start(
                Arc::clone(&graph_store),
                persist_plan.touched_works.clone(),
                "vector",
            );
            let outcome = match persist_vector_embed_batch(&graph_store, &persist_plan) {
                Ok(()) => {
                    let db_write_ms = db_write_started.elapsed().as_millis() as u64;
                    service_guard::record_vector_stage_ms(
                        service_guard::VectorStageKind::DbWrite,
                        db_write_ms,
                    );
                    service_guard::record_vector_embed_call(
                        persist_plan.updates.len() as u64,
                        persist_plan.touched_works.len() as u64,
                    );
                    batch_run.db_write_ms = db_write_ms;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    if let Err(err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Persist Worker [{}]: failed to persist vector batch run after db write: {:?}",
                            worker_idx, err
                        );
                    }
                    let batch_runs = if persist_plan.completed_works.is_empty() {
                        Vec::new()
                    } else {
                        vec![batch_run]
                    };
                    VectorPersistOutcome {
                        completed_works: persist_plan.completed_works,
                        batch_runs,
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
        }
    }

    fn graph_worker_loop(
        worker_idx: usize,
        graph_store: Arc<GraphStore>,
        queue_store: Arc<QueueStore>,
    ) {
        let lane_config = embedding_lane_config_from_env();
        let mut model: Option<TextEmbedding> = None;
        let mut graph_model_registered = false;

        loop {
            let policy =
                semantic_policy(queue_store.common_len(), service_guard::current_pressure());
            let service_pressure = service_guard::current_pressure();
            let vector_backlog_depth = graph_store
                .fetch_file_vectorization_queue_counts()
                .map(|(queued, inflight)| queued + inflight)
                .unwrap_or(0);
            let gpu_available = effective_embedding_provider_is_gpu();
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
            unsafe {
                std::env::set_var(
                    "AXON_EMBEDDING_PROVIDER_EFFECTIVE",
                    "cpu_missing_cuda_provider",
                );
            }
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
                    unsafe {
                        std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cuda");
                        std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
                    }
                    Ok(model)
                }
                Err(err) => {
                    let rendered = format!("{err:?}");
                    error!(
                        "❌ Semantic {} Worker [{}]: CUDA init failed, falling back to CPU: {:?}",
                        lane, worker_idx, err
                    );
                    unsafe {
                        std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cpu_fallback");
                        std::env::set_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR", rendered);
                    }
                    apply_cpu_fallback_ort_runtime_env();
                    TextEmbedding::try_new(options)
                }
            }
        } else {
            unsafe {
                std::env::set_var(
                    "AXON_EMBEDDING_PROVIDER_EFFECTIVE",
                    cpu_provider_effective_label(
                        cuda_requested,
                        cuda_available,
                        cuda_provider_library_available,
                    ),
                );
                std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
            }
            TextEmbedding::try_new(options)
        };

        match model_result {
            Ok(model) => {
                let provider_effective = std::env::var("AXON_EMBEDDING_PROVIDER_EFFECTIVE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "cpu".to_string());
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
                unsafe {
                    std::env::set_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR", rendered);
                }
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

    let mut touched_files = HashSet::new();
    let mut planned_chunks = 0usize;
    let mut planned_bytes = 0usize;
    let per_file_fetch_limit = per_file_fetch_limit.max(1);
    let batch_max_bytes = batch_max_bytes.max(1);

    for work in active_works {
        let remaining_chunk_budget = target_chunks.saturating_sub(planned_chunks).max(1);
        let fetch_limit = per_file_fetch_limit.min(remaining_chunk_budget);
        let fetch_started = Instant::now();
        match graph_store.fetch_unembedded_chunks_for_file(
            &work.file_path,
            CHUNK_MODEL_ID,
            fetch_limit.saturating_add(1),
        ) {
            Ok(chunks) if chunks.is_empty() => {
                let fetch_ms = fetch_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::Fetch,
                    fetch_ms,
                );
                plan.fetch_ms_total = plan.fetch_ms_total.saturating_add(fetch_ms);
                plan.immediate_completed.push(work.clone());
            }
            Ok(chunks) => {
                let fetch_ms = fetch_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::Fetch,
                    fetch_ms,
                );
                plan.fetch_ms_total = plan.fetch_ms_total.saturating_add(fetch_ms);
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
            Err(err) => {
                let fetch_ms = fetch_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::Fetch,
                    fetch_ms,
                );
                plan.fetch_ms_total = plan.fetch_ms_total.saturating_add(fetch_ms);
                plan.failed_fetches
                    .push((work.clone(), format!("{:?}", err)));
            }
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

fn persist_vector_embed_batch(
    graph_store: &GraphStore,
    persist_plan: &VectorPersistPlan,
) -> anyhow::Result<()> {
    graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &persist_plan.updates)
}

fn finalize_completed_vectorization_works(
    graph_store: &GraphStore,
    completed_works: Vec<FileVectorizationWork>,
    lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    batch_runs: Vec<VectorBatchRun>,
) -> Result<VectorFinalizeOutcome, (anyhow::Error, Vec<VectorBatchRun>)> {
    if completed_works.is_empty() {
        return Ok(VectorFinalizeOutcome {
            completed_works,
            batch_runs,
        });
    }

    graph_store
        .finalize_file_vectorization_success_batch(
            &completed_works,
            &lease_snapshots,
            CHUNK_MODEL_ID,
            FILE_PROJECTION_RADIUS,
        )
        .map_err(|err| (err, batch_runs.clone()))?;
    Ok(VectorFinalizeOutcome {
        completed_works,
        batch_runs,
    })
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
            if let Some(snapshot) = snapshot_by_path.get(item.file_path.as_str()) {
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

fn flush_completed_vectorization_works(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    finalize_tx: &Sender<VectorFinalizeRequest>,
    completed_works: &mut Vec<FileVectorizationWork>,
    completed_batch_runs: &mut Vec<VectorBatchRun>,
    failed: &mut HashMap<String, Vec<FileVectorizationWork>>,
) {
    if completed_works.is_empty() {
        return;
    }

    let works_to_finalize = std::mem::take(completed_works);
    let batch_runs_to_finalize = std::mem::take(completed_batch_runs);
    let vector_lease_snapshots = match graph_store
        .capture_file_vectorization_lease_snapshots(&works_to_finalize, "vector")
    {
        Ok(snapshots) => snapshots,
        Err(err) => {
            failed
                .entry(format!(
                    "failed to capture vector finalize lease snapshots: {:?}",
                    err
                ))
                .or_default()
                .extend(works_to_finalize);
            return;
        }
    };
    let finalize_request = VectorFinalizeRequest {
        envelope: FinalizeEnvelope {
            completed_works: works_to_finalize.clone(),
            lease_snapshots: vector_lease_snapshots.clone(),
            batch_runs: batch_runs_to_finalize.clone(),
        },
        enqueued_at: Instant::now(),
    };
    let finalize_send_started = Instant::now();
    if let Err(err) = finalize_tx.send(finalize_request) {
        service_guard::record_vector_finalize_send_wait_ms(
            finalize_send_started.elapsed().as_millis() as u64,
        );
        service_guard::record_vector_finalize_fallback_inline();
        error!(
            "Semantic Vector Worker [{}]: finalize queue unavailable, falling back to inline finalization",
            worker_idx
        );
        let finalize_lease_snapshots = match graph_store.transfer_file_vectorization_lease_owner(
            &err.0.envelope.lease_snapshots,
            "vector",
            "finalize",
        ) {
            Ok(snapshots) => snapshots,
            Err(transfer_err) => {
                let reason = format!(
                    "failed to transfer finalize lease ownership for inline fallback: {:?}",
                    transfer_err
                );
                failed
                    .entry(reason.clone())
                    .or_default()
                    .extend(err.0.envelope.completed_works.iter().cloned());
                for mut batch_run in err.0.envelope.batch_runs {
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    batch_run.success = false;
                    batch_run.error_reason = Some(reason.clone());
                    if let Err(record_err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist inline finalize ownership error batch run: {:?}",
                            worker_idx, record_err
                        );
                    }
                }
                return;
            }
        };
        let finalize_owned_works =
            work_with_lease_snapshots(&err.0.envelope.completed_works, &finalize_lease_snapshots);
        let _ = graph_store
            .refresh_file_vectorization_leases_for_owner(&finalize_owned_works, "finalize");
        let mark_done_started = Instant::now();
        service_guard::record_vector_mark_done_call();
        let _inline_finalize_lease_guard = LeaseRefreshGuard::start(
            Arc::clone(graph_store),
            finalize_owned_works.clone(),
            "finalize",
        );
        match finalize_completed_vectorization_works(
            graph_store,
            finalize_owned_works,
            finalize_lease_snapshots,
            err.0.envelope.batch_runs,
        ) {
            Err((finalize_err, mut batch_runs)) => {
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::MarkDone,
                    mark_done_started.elapsed().as_millis() as u64,
                );
                let reason = format!(
                    "failed to mark vectorization completion: {:?}",
                    finalize_err
                );
                for batch_run in &mut batch_runs {
                    batch_run.mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    batch_run.success = false;
                    batch_run.error_reason = Some(reason.clone());
                    if let Err(batch_err) = graph_store.record_vector_batch_run(batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist failed inline finalize vector batch run: {:?}",
                            worker_idx, batch_err
                        );
                    }
                }
                failed
                    .entry(reason)
                    .or_default()
                    .extend(works_to_finalize.iter().cloned());
            }
            Ok(outcome) => {
                let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::MarkDone,
                    mark_done_ms,
                );
                for mut batch_run in outcome.batch_runs {
                    batch_run.mark_done_ms = mark_done_ms;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    if let Err(err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist inline finalized vector batch run: {:?}",
                            worker_idx, err
                        );
                    }
                }
                service_guard::record_vector_files_completed(outcome.completed_works.len() as u64);
            }
        }
    } else {
        service_guard::record_vector_finalize_send_wait_ms(
            finalize_send_started.elapsed().as_millis() as u64,
        );
        service_guard::record_vector_finalize_enqueued();
        service_guard::record_vector_finalize_queue_depth(finalize_tx.len() as u64);
    }
}

fn apply_vector_persist_outcome(
    outcome: VectorPersistOutcome,
    active_works: &mut Vec<FileVectorizationWork>,
    ready_batches: &mut VecDeque<PreparedBatchEnvelope>,
    completed_works: &mut Vec<FileVectorizationWork>,
    completed_batch_runs: &mut Vec<VectorBatchRun>,
    failed: &mut HashMap<String, Vec<FileVectorizationWork>>,
) {
    if let Some(reason) = outcome.error_reason {
        failed
            .entry(reason)
            .or_default()
            .extend(outcome.touched_works.into_iter());
        ready_batches.clear();
        let merge_target = outcome
            .next_active_after_failure
            .len()
            .saturating_add(active_works.len())
            .max(1);
        *active_works = merge_vectorization_work(
            outcome.next_active_after_failure,
            std::mem::take(active_works),
            merge_target,
        );
    } else {
        completed_works.extend(outcome.completed_works);
        completed_batch_runs.extend(outcome.batch_runs);
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

fn request_prepared_vector_embed_sequence(
    graph_store: &Arc<GraphStore>,
    worker_idx: usize,
    prepare_tx: &Sender<VectorPrepareRequest>,
    active_works: Vec<FileVectorizationWork>,
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
) -> AnyhowResult<PreparedVectorEmbedSequence> {
    let claimed = ClaimedLeaseSet::new(active_works);
    let reply_rx = dispatch_prepared_vector_embed_sequence(
        prepare_tx,
        claimed.clone(),
        target_chunks,
        per_file_fetch_limit,
        batch_max_bytes,
        target_ready_depth,
    )?;
    let reply_wait_started = Instant::now();
    let prepared = loop {
        service_guard::record_vector_worker_heartbeat();
        let _ =
            graph_store.refresh_file_vectorization_leases_for_owner(claimed.as_slice(), "vector");
        match reply_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(prepared) => break prepared,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!(
                    "prepare worker reply unavailable for vector worker [{}]: disconnected",
                    worker_idx
                ));
            }
        }
    };
    service_guard::record_vector_prepare_reply_wait_ms(
        reply_wait_started.elapsed().as_millis() as u64
    );
    Ok(prepared)
}

fn dispatch_prepared_vector_embed_sequence(
    prepare_tx: &Sender<VectorPrepareRequest>,
    claimed: ClaimedLeaseSet,
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    target_ready_depth: usize,
) -> AnyhowResult<PreparedVectorEmbedSequenceReply> {
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

fn dispatch_vector_persist_plan(
    persist_tx: &Sender<VectorPersistRequest>,
    envelope: PersistEnvelope,
) -> AnyhowResult<VectorPersistOutcomeReply> {
    let (reply_tx, reply_rx) = bounded(1);
    let persist_send_started = Instant::now();
    persist_tx
        .send(VectorPersistRequest {
            envelope,
            enqueued_at: Instant::now(),
            reply: reply_tx,
        })
        .map_err(|err| anyhow!("persist worker unavailable: {}", err))?;
    service_guard::record_vector_persist_send_wait_ms(
        persist_send_started.elapsed().as_millis() as u64
    );
    service_guard::record_vector_persist_queue_depth(persist_tx.len() as u64);
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

fn embedding_provider_slot() -> &'static Mutex<EmbeddingProviderDiagnostics> {
    EMBEDDING_PROVIDER_DIAGNOSTICS
        .get_or_init(|| Mutex::new(embedding_provider_diagnostics("unspecified".to_string())))
}

fn register_query_embedding_sender(sender: Sender<QueryEmbeddingRequest>) {
    let mut slot = query_embedding_sender_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(sender);
}

pub(crate) fn register_embedding_provider_diagnostics(diagnostics: EmbeddingProviderDiagnostics) {
    let mut slot = embedding_provider_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *slot = diagnostics;
}

pub fn current_embedding_provider_diagnostics() -> EmbeddingProviderDiagnostics {
    embedding_provider_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

pub fn embedding_provider_diagnostics(provider_effective: String) -> EmbeddingProviderDiagnostics {
    EmbeddingProviderDiagnostics {
        provider_requested: std::env::var("AXON_EMBEDDING_PROVIDER")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unspecified".to_string()),
        provider_effective,
        ort_strategy: std::env::var("ORT_STRATEGY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unspecified".to_string()),
        ort_dylib_path: std::env::var("ORT_DYLIB_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        provider_init_error: std::env::var("AXON_EMBEDDING_PROVIDER_INIT_ERROR")
            .ok()
            .filter(|value| !value.trim().is_empty()),
    }
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

fn apply_cpu_fallback_ort_runtime_env() {
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
            unsafe {
                std::env::set_var(env_name, value);
            }
        }
    }
}

fn embedding_model_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var("FASTEMBED_CACHE_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }

    if let Some(path) = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(path).join("axon").join("fastembed");
    }

    if let Some(home) = std::env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(home)
            .join(".cache")
            .join("axon")
            .join("fastembed");
    }

    PathBuf::from("/tmp/axon-fastembed")
}

fn embedding_download_progress_enabled() -> bool {
    std::env::var("AXON_EMBEDDING_DOWNLOAD_PROGRESS")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn gpu_memory_snapshot_cache_slot() -> &'static Mutex<Option<CachedGpuMemorySnapshot>> {
    GPU_MEMORY_SNAPSHOT_CACHE.get_or_init(|| Mutex::new(None))
}

fn gpu_telemetry_backend() -> GpuTelemetryBackend {
    match std::env::var("AXON_GPU_TELEMETRY_BACKEND")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("none") | Some("disabled") => GpuTelemetryBackend::None,
        Some("nvml") => GpuTelemetryBackend::Nvml,
        _ => GpuTelemetryBackend::NvidiaSmi,
    }
}

pub fn gpu_telemetry_backend_name() -> &'static str {
    match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => "none",
        GpuTelemetryBackend::Nvml => "nvml",
        GpuTelemetryBackend::NvidiaSmi => "nvidia-smi",
    }
}

fn gpu_telemetry_command() -> String {
    std::env::var("AXON_GPU_TELEMETRY_COMMAND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/lib/wsl/lib/nvidia-smi".to_string())
}

fn nvml_library_path() -> String {
    std::env::var("AXON_NVML_LIBRARY_PATH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/lib/wsl/lib/libnvidia-ml.so.1".to_string())
}

pub fn gpu_telemetry_device_index() -> u32 {
    std::env::var("AXON_GPU_TELEMETRY_DEVICE_INDEX")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

pub fn gpu_telemetry_cache_ttl_ms() -> u64 {
    std::env::var("AXON_GPU_TELEMETRY_CACHE_TTL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 100)
        .unwrap_or(2_000)
}

fn parse_nvidia_smi_memory_csv(line: &str) -> Option<GpuMemorySnapshot> {
    let mut parts = line.split(',').map(|part| part.trim().parse::<u64>().ok());
    let total_mb = parts.next()??;
    let used_mb = parts.next()??;
    let free_mb = parts.next()??;
    Some(GpuMemorySnapshot {
        total_mb,
        used_mb,
        free_mb,
    })
}

fn parse_nvidia_smi_utilization_csv(line: &str) -> Option<GpuUtilizationSnapshot> {
    let mut parts = line.split(',').map(|part| part.trim().parse::<f64>().ok());
    let gpu_utilization = parts.next()??;
    let memory_utilization = parts.next()??;
    Some(GpuUtilizationSnapshot {
        gpu_utilization_ratio: (gpu_utilization / 100.0).clamp(0.0, 1.0),
        memory_utilization_ratio: (memory_utilization / 100.0).clamp(0.0, 1.0),
    })
}

#[repr(C)]
struct NvmlMemoryInfo {
    total: u64,
    free: u64,
    used: u64,
}

#[repr(C)]
struct NvmlUtilizationInfo {
    gpu: u32,
    memory: u32,
}

fn current_gpu_memory_snapshot_via_nvml() -> Option<GpuMemorySnapshot> {
    type NvmlInitV2 = unsafe extern "C" fn() -> i32;
    type NvmlShutdown = unsafe extern "C" fn() -> i32;
    type NvmlDeviceGetHandleByIndexV2 =
        unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32;
    type NvmlDeviceGetMemoryInfo =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlMemoryInfo) -> i32;

    const NVML_SUCCESS: i32 = 0;

    unsafe {
        let library = Library::new(nvml_library_path()).ok()?;
        let nvml_init: libloading::Symbol<'_, NvmlInitV2> = library.get(b"nvmlInit_v2").ok()?;
        let nvml_shutdown: libloading::Symbol<'_, NvmlShutdown> =
            library.get(b"nvmlShutdown").ok()?;
        let nvml_device_get_handle_by_index: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexV2> =
            library.get(b"nvmlDeviceGetHandleByIndex_v2").ok()?;
        let nvml_device_get_memory_info: libloading::Symbol<'_, NvmlDeviceGetMemoryInfo> =
            library.get(b"nvmlDeviceGetMemoryInfo").ok()?;

        if nvml_init() != NVML_SUCCESS {
            return None;
        }

        let mut device: *mut std::ffi::c_void = std::ptr::null_mut();
        if nvml_device_get_handle_by_index(gpu_telemetry_device_index(), &mut device)
            != NVML_SUCCESS
        {
            let _ = nvml_shutdown();
            return None;
        }

        let mut memory = NvmlMemoryInfo {
            total: 0,
            free: 0,
            used: 0,
        };
        let result = if nvml_device_get_memory_info(device, &mut memory) == NVML_SUCCESS {
            Some(GpuMemorySnapshot {
                total_mb: memory.total / (1024 * 1024),
                used_mb: memory.used / (1024 * 1024),
                free_mb: memory.free / (1024 * 1024),
            })
        } else {
            None
        };
        let _ = nvml_shutdown();
        result
    }
}

fn current_gpu_utilization_snapshot_via_nvml() -> Option<GpuUtilizationSnapshot> {
    type NvmlInitV2 = unsafe extern "C" fn() -> i32;
    type NvmlShutdown = unsafe extern "C" fn() -> i32;
    type NvmlDeviceGetHandleByIndexV2 =
        unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32;
    type NvmlDeviceGetUtilizationRates =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut NvmlUtilizationInfo) -> i32;

    const NVML_SUCCESS: i32 = 0;

    unsafe {
        let library = Library::new(nvml_library_path()).ok()?;
        let nvml_init: libloading::Symbol<'_, NvmlInitV2> = library.get(b"nvmlInit_v2").ok()?;
        let nvml_shutdown: libloading::Symbol<'_, NvmlShutdown> =
            library.get(b"nvmlShutdown").ok()?;
        let nvml_device_get_handle_by_index: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexV2> =
            library.get(b"nvmlDeviceGetHandleByIndex_v2").ok()?;
        let nvml_device_get_utilization_rates: libloading::Symbol<
            '_,
            NvmlDeviceGetUtilizationRates,
        > = library.get(b"nvmlDeviceGetUtilizationRates").ok()?;

        if nvml_init() != NVML_SUCCESS {
            return None;
        }

        let mut device: *mut std::ffi::c_void = std::ptr::null_mut();
        if nvml_device_get_handle_by_index(gpu_telemetry_device_index(), &mut device)
            != NVML_SUCCESS
        {
            let _ = nvml_shutdown();
            return None;
        }

        let mut utilization = NvmlUtilizationInfo { gpu: 0, memory: 0 };
        let result = if nvml_device_get_utilization_rates(device, &mut utilization) == NVML_SUCCESS
        {
            Some(GpuUtilizationSnapshot {
                gpu_utilization_ratio: (utilization.gpu as f64 / 100.0).clamp(0.0, 1.0),
                memory_utilization_ratio: (utilization.memory as f64 / 100.0).clamp(0.0, 1.0),
            })
        } else {
            None
        };
        let _ = nvml_shutdown();
        result
    }
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
    std::env::var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value >= 1_000)
        .unwrap_or(DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS)
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

fn gpu_memory_pressure_active(snapshot: GpuMemorySnapshot) -> bool {
    snapshot.used_mb >= gpu_memory_soft_limit_mb()
}

pub fn current_gpu_memory_snapshot() -> Option<GpuMemorySnapshot> {
    let cache_ttl = Duration::from_millis(gpu_telemetry_cache_ttl_ms());
    let cache = gpu_memory_snapshot_cache_slot();
    let now = Instant::now();
    {
        let guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(cached) = *guard {
            if now.duration_since(cached.captured_at) <= cache_ttl {
                return cached.snapshot;
            }
        }
    }

    let snapshot = match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => None,
        GpuTelemetryBackend::Nvml => current_gpu_memory_snapshot_via_nvml(),
        GpuTelemetryBackend::NvidiaSmi => std::process::Command::new(gpu_telemetry_command())
            .args([
                "--query-gpu=memory.total,memory.used,memory.free",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|stdout| {
                stdout
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_string)
            })
            .and_then(|line| parse_nvidia_smi_memory_csv(&line)),
    };

    let mut guard = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(CachedGpuMemorySnapshot {
        captured_at: now,
        snapshot,
    });
    snapshot
}

pub fn current_gpu_utilization_snapshot() -> Option<GpuUtilizationSnapshot> {
    match gpu_telemetry_backend() {
        GpuTelemetryBackend::None => None,
        GpuTelemetryBackend::Nvml => current_gpu_utilization_snapshot_via_nvml(),
        GpuTelemetryBackend::NvidiaSmi => std::process::Command::new(gpu_telemetry_command())
            .args([
                "--query-gpu=utilization.gpu,utilization.memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|stdout| {
                stdout
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_string)
            })
            .and_then(|line| parse_nvidia_smi_utilization_csv(&line)),
    }
}

pub fn current_gpu_memory_pressure_active() -> bool {
    current_gpu_memory_snapshot()
        .map(gpu_memory_pressure_active)
        .unwrap_or(false)
}

fn embedding_provider_requested_is_gpu() -> bool {
    std::env::var("AXON_EMBEDDING_PROVIDER")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("cuda"))
        .unwrap_or(false)
}

pub fn embedding_lane_config_from_env() -> EmbeddingLaneConfig {
    let bootstrap = bootstrap_embedding_lane_config_from_env();
    let tuning = current_runtime_tuning_snapshot().state;
    EmbeddingLaneConfig {
        query_workers: bootstrap.query_workers,
        vector_workers: tuning.vector_workers.max(1),
        graph_workers: bootstrap.graph_workers,
        chunk_batch_size: tuning.chunk_batch_size.max(1),
        file_vectorization_batch_size: tuning.file_vectorization_batch_size.max(1),
        graph_batch_size: bootstrap.graph_batch_size,
        max_chunks_per_file: bootstrap.max_chunks_per_file,
        max_embed_batch_bytes: bootstrap.max_embed_batch_bytes,
    }
}

pub fn apply_runtime_embedding_lane_adjustment(
    vector_workers: Option<usize>,
    chunk_batch_size: Option<usize>,
    file_vectorization_batch_size: Option<usize>,
    vector_ready_queue_depth: Option<usize>,
    vector_persist_queue_bound: Option<usize>,
    vector_max_inflight_persists: Option<usize>,
    embed_micro_batch_max_items: Option<usize>,
    embed_micro_batch_max_total_tokens: Option<usize>,
) {
    let _snapshot = update_shared_runtime_tuning_state(
        bootstrap_runtime_tuning_state_from_env(),
        vector_workers,
        chunk_batch_size,
        file_vectorization_batch_size,
        vector_ready_queue_depth,
        vector_persist_queue_bound,
        vector_max_inflight_persists,
        embed_micro_batch_max_items,
        embed_micro_batch_max_total_tokens,
    );
    if let Some(vector_workers) = vector_workers {
        std::env::set_var("AXON_VECTOR_WORKERS", vector_workers.max(1).to_string());
        std::env::set_var("AXON_VECTOR_WORKERS_AUTOCONFIGURED", "false");
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
    refresh_vector_batch_controller_from_env();
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
        .map(|value| value.trim().eq_ignore_ascii_case("cuda"))
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

fn is_fatal_embedding_error<E: std::fmt::Debug>(err: &E) -> bool {
    let rendered = format!("{:?}", err);
    rendered.contains("GetElementType is not implemented")
        || rendered.contains("ORT")
        || rendered.contains("onnxruntime")
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

    let Some(sender) = current_query_embedding_sender() else {
        return Err(anyhow::anyhow!(
            "MCP real-time embedding worker not ready. Use structural search."
        ));
    };

    request_query_embedding(&sender, texts)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        build_prepared_vector_embed_sequence, build_token_aware_micro_batches,
        build_vector_batch_plan, configured_embedding_max_length, cuda_execution_provider_dispatch,
        current_runtime_tuning_snapshot, current_runtime_tuning_state,
        dispatch_prepared_vector_embed_sequence, effective_embedding_provider_is_gpu,
        embedding_download_progress_enabled, embedding_lane_config_from_env,
        embedding_model_cache_dir, embedding_provider_diagnostics,
        finalize_completed_vectorization_works, flush_completed_vectorization_works,
        gpu_memory_soft_limit_mb, is_fatal_embedding_error, merge_vectorization_work,
        query_embedding_allowed, request_prepared_vector_embed_sequence, request_query_embedding,
        ClaimedLeaseSet, EmbeddingLaneConfig, PreparedBatchEnvelope, PreparedVectorEmbedBatch,
        PreparedVectorEmbedSequence, QueryEmbeddingRequest, VectorBatchPlan, VectorChunkWorkItem,
        VectorFinalizeRequest, VectorPrepareRequest,
    };
    use crate::embedding_contract::{fastembed_model, CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH};
    use crate::graph_ingestion::{FileVectorizationLeaseSnapshot, FileVectorizationWork};
    use crate::service_guard::{ServicePressure, VectorRuntimeMetrics};
    use crate::vector_control::{
        allowed_gpu_vector_workers, current_vector_batch_controller_diagnostics,
        current_vector_drain_state, graph_projection_allowed, semantic_policy,
        symbol_embedding_allowed, vector_claim_target, vector_embed_target_chunks,
        vector_worker_admitted, VectorBatchController, VectorBatchControllerObservation,
        VectorBatchControllerState, VectorDrainState, AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
        CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD, GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD,
        QUIET_CRUISE_FILE_BACKLOG_THRESHOLD, SYMBOL_BACKLOG_RESIDUAL_THRESHOLD,
    };
    use crossbeam_channel::{bounded, unbounded};
    use fastembed::{InitOptions, TextEmbedding};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());

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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(100, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.idle_sleep, Duration::from_secs(5));
    }

    #[test]
    fn test_semantic_policy_prefers_aggressive_drain_under_high_healthy_backlog() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(2_000, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.idle_sleep, Duration::from_millis(250));
    }

    #[test]
    fn test_graph_projection_allowed_under_queue_pressure_when_service_is_healthy() {
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
    fn test_graph_projection_yields_to_large_vector_backlog_on_cpu_only_hosts() {
        assert!(!graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
            false
        ));
    }

    #[test]
    fn test_graph_projection_can_run_with_large_vector_backlog_when_gpu_is_available() {
        assert!(graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
            true
        ));
    }

    #[test]
    fn test_graph_projection_yields_to_large_vector_backlog_on_gpu_hosts_too() {
        crate::service_guard::reset_for_tests();
        assert!(!graph_projection_allowed(
            100,
            ServicePressure::Healthy,
            GPU_VECTOR_BACKLOG_GRAPH_YIELD_THRESHOLD,
            true
        ));
    }

    #[test]
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(100, ServicePressure::Critical);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(10));
    }

    #[test]
    fn test_semantic_policy_pauses_when_service_is_degraded() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(100, ServicePressure::Degraded);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_stays_paused_while_service_recovers() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(100, ServicePressure::Recovering);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(2));
        assert_eq!(policy.idle_sleep, Duration::from_secs(3));
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
    fn test_effective_embedding_provider_is_gpu_detects_cuda_only() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE", "cuda");
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
    fn test_apply_cpu_fallback_ort_runtime_env_restores_cpu_threads_when_autoconfigured() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_ORT_OMP_AUTOCONFIGURED", "true");
            std::env::set_var("OMP_NUM_THREADS", "1");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::set_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED", "true");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "1");
        }

        super::apply_cpu_fallback_ort_runtime_env();

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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_ORT_OMP_AUTOCONFIGURED");
            std::env::set_var("OMP_NUM_THREADS", "3");
            std::env::set_var("OMP_WAIT_POLICY", "ACTIVE");
            std::env::remove_var("AXON_ORT_INTRA_THREADS_AUTOCONFIGURED");
            std::env::set_var("AXON_ORT_INTRA_THREADS", "5");
        }

        super::apply_cpu_fallback_ort_runtime_env();

        assert_eq!(std::env::var("OMP_NUM_THREADS").unwrap(), "3");
        assert_eq!(std::env::var("OMP_WAIT_POLICY").unwrap(), "ACTIVE");
        assert_eq!(std::env::var("AXON_ORT_INTRA_THREADS").unwrap(), "5");

        unsafe {
            std::env::remove_var("OMP_NUM_THREADS");
            std::env::remove_var("OMP_WAIT_POLICY");
            std::env::remove_var("AXON_ORT_INTRA_THREADS");
        }
    }

    #[test]
    fn test_apply_cpu_fallback_ort_runtime_env_restores_cpu_lane_sizing_when_autoconfigured() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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

        super::apply_cpu_fallback_ort_runtime_env();

        assert_eq!(std::env::var("AXON_CHUNK_BATCH_SIZE").unwrap(), "16");
        assert_eq!(
            std::env::var("AXON_FILE_VECTORIZATION_BATCH_SIZE").unwrap(),
            "8"
        );
        assert_eq!(std::env::var("AXON_GRAPH_BATCH_SIZE").unwrap(), "4");
        assert_eq!(std::env::var("AXON_VECTOR_WORKERS").unwrap(), "1");
        assert_eq!(std::env::var("AXON_GRAPH_WORKERS").unwrap(), "0");

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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        crate::service_guard::mcp_request_started();
        let policy = semantic_policy(100, ServicePressure::Healthy);
        crate::service_guard::mcp_request_finished();
        assert!(policy.pause);
    }

    #[test]
    fn test_semantic_policy_prefers_drain_mode_when_mcp_is_idle() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        let policy = semantic_policy(2_000, ServicePressure::Recovering);
        assert!(
            !policy.pause,
            "idle MCP should allow semantic drain despite recovering pressure"
        );
        assert_eq!(policy.idle_sleep, Duration::from_millis(250));
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
    fn test_current_vector_drain_state_reports_quiet_cruise_for_small_backlog() {
        let state = current_vector_drain_state(8, ServicePressure::Healthy, false, "cpu", "cpu");
        assert_eq!(state, VectorDrainState::QuietCruise);
    }

    #[test]
    fn test_symbol_embedding_allowed_only_as_residual_background_work() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        crate::service_guard::reset_for_tests();
        assert!(symbol_embedding_allowed(8, ServicePressure::Healthy));
        assert!(!symbol_embedding_allowed(
            SYMBOL_BACKLOG_RESIDUAL_THRESHOLD,
            ServicePressure::Healthy
        ));
        crate::service_guard::mcp_request_started();
        assert!(!symbol_embedding_allowed(8, ServicePressure::Healthy));
        crate::service_guard::mcp_request_finished();
    }

    #[test]
    fn test_vector_worker_admission_throttles_gpu_background_under_pressure() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        assert!(!vector_worker_admitted(
            1,
            ServicePressure::Healthy,
            true,
            QUIET_CRUISE_FILE_BACKLOG_THRESHOLD
        ));
    }

    #[test]
    fn test_allowed_gpu_vector_workers_scales_only_for_meaningful_backlog() {
        assert_eq!(allowed_gpu_vector_workers(8, ServicePressure::Healthy), 1);
        assert_eq!(
            allowed_gpu_vector_workers(
                AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD,
                ServicePressure::Healthy
            ),
            4
        );
        assert_eq!(
            allowed_gpu_vector_workers(4_096, ServicePressure::Degraded),
            1
        );
    }

    #[test]
    fn test_vector_worker_admission_pauses_gpu_background_when_interactive_priority_is_active() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_QUERY_EMBED_WORKERS", "2");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::set_var("AXON_VECTOR_WORKERS", "5");
            std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        }
        super::refresh_runtime_tuning_snapshot_from_env();
        let config = embedding_lane_config_from_env();
        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
            std::env::remove_var("AXON_VECTOR_WORKERS");
        }

        assert_eq!(config.vector_workers, 1);
    }

    #[test]
    fn test_apply_runtime_embedding_lane_adjustment_updates_live_batch_env_and_controller() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "48");
            std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "12");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
        }
        super::refresh_vector_batch_controller_from_env();

        super::apply_runtime_embedding_lane_adjustment(
            None,
            Some(64),
            Some(16),
            Some(7),
            Some(3),
            Some(2),
            Some(72),
            Some(8_192),
        );

        let config = embedding_lane_config_from_env();
        let diagnostics = current_vector_batch_controller_diagnostics(&config);
        assert_eq!(config.chunk_batch_size, 64);
        assert_eq!(config.file_vectorization_batch_size, 16);
        let runtime_tuning = current_runtime_tuning_state();
        let runtime_snapshot = current_runtime_tuning_snapshot();
        assert_eq!(runtime_tuning.vector_ready_queue_depth, 7);
        assert_eq!(runtime_tuning.vector_persist_queue_bound, 3);
        assert_eq!(runtime_tuning.vector_max_inflight_persists, 2);
        assert_eq!(runtime_tuning.embed_micro_batch_max_items, 72);
        assert_eq!(runtime_tuning.embed_micro_batch_max_total_tokens, 8_192);
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

        unsafe {
            std::env::set_var("AXON_CHUNK_BATCH_SIZE", "96");
        }
        let config_after_env_override = embedding_lane_config_from_env();
        let runtime_tuning_after_env_override = current_runtime_tuning_state();
        assert_eq!(config_after_env_override.chunk_batch_size, 64);
        assert_eq!(runtime_tuning_after_env_override.chunk_batch_size, 64);

        unsafe {
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS");
            std::env::remove_var("AXON_CHUNK_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_READY_QUEUE_DEPTH_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_PERSIST_QUEUE_BOUND_AUTOCONFIGURED");
            std::env::remove_var("AXON_VECTOR_MAX_INFLIGHT_PERSISTS_AUTOCONFIGURED");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_ITEMS_AUTOCONFIGURED");
            std::env::remove_var("AXON_EMBED_MICRO_BATCH_MAX_TOTAL_TOKENS_AUTOCONFIGURED");
        }
    }

    #[test]
    fn test_embedding_lane_config_allows_gpu_vector_worker_oversubscription_with_opt_in() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        unsafe {
            std::env::set_var("ORT_STRATEGY", "system");
            std::env::set_var("ORT_DYLIB_PATH", "/tmp/libonnxruntime.so");
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        }
        let diagnostics = embedding_provider_diagnostics("cpu_fallback".to_string());
        unsafe {
            std::env::remove_var("ORT_STRATEGY");
            std::env::remove_var("ORT_DYLIB_PATH");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(diagnostics.provider_requested, "cuda");
        assert_eq!(diagnostics.provider_effective, "cpu_fallback");
        assert_eq!(diagnostics.ort_strategy, "system");
        assert_eq!(
            diagnostics.ort_dylib_path.as_deref(),
            Some("/tmp/libonnxruntime.so")
        );
    }

    #[test]
    fn test_configured_embedding_max_length_defaults_to_model_cap() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_EMBED_MAX_LENGTH");
        }

        assert_eq!(configured_embedding_max_length(), MAX_LENGTH);
    }

    #[test]
    fn test_configured_embedding_max_length_honors_lower_override_and_caps_high_override() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS", "15000");
        }

        assert_eq!(super::vector_stale_inflight_recovery_interval_ms(), 15_000);

        unsafe {
            std::env::remove_var("AXON_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS");
        }
    }

    #[test]
    fn test_recover_stale_vector_inflight_now_uses_configured_claim_age() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_BACKEND");
        }
        assert_eq!(super::gpu_telemetry_backend_name(), "nvidia-smi");
    }

    #[test]
    fn test_gpu_telemetry_backend_allows_disabling_collection() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_DEVICE_INDEX");
        }
        assert_eq!(super::gpu_telemetry_device_index(), 0);
    }

    #[test]
    fn test_gpu_telemetry_device_index_respects_override() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_GPU_TELEMETRY_CACHE_TTL_MS");
        }
        assert_eq!(super::gpu_telemetry_cache_ttl_ms(), 2_000);
    }

    #[test]
    fn test_gpu_telemetry_cache_ttl_ms_respects_override() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
    fn test_embedding_model_cache_dir_defaults_outside_workspace() {
        unsafe {
            std::env::remove_var("FASTEMBED_CACHE_DIR");
            std::env::remove_var("XDG_CACHE_HOME");
            std::env::set_var("HOME", "/tmp/axon-home");
        }

        let cache_dir = embedding_model_cache_dir();

        unsafe {
            std::env::remove_var("HOME");
        }

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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }

        assert!(!super::cuda_tf32_enabled());
    }

    #[test]
    fn test_cuda_tf32_enabled_honors_explicit_disable() {
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
                chunk_id: "a2".to_string(),
                content_hash: "ha2".to_string(),
                text: "A2".to_string(),
            },
            VectorChunkWorkItem {
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
                 ('chunk-b', 'symbol', 'sym-b', 'proj', '/tmp/segments.rs', 'function', 'B', 'hash-b', 20, 30), \
                 ('chunk-a', 'symbol', 'sym-a', 'proj', '/tmp/segments.rs', 'function', 'A', 'hash-a', 5, 10), \
                 ('chunk-c', 'symbol', 'sym-c', 'proj', '/tmp/segments.rs', 'function', 'C', 'hash-c', 31, 40)",
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
                 ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/a.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'proj', '/tmp/a.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'proj', '/tmp/a.rs', 'function', 'A3', 'hash-a3', 5, 6), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'proj', '/tmp/b.rs', 'function', 'B1', 'hash-b1', 1, 2)",
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
                 ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/a.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'proj', '/tmp/a.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'proj', '/tmp/a.rs', 'function', 'A3', 'hash-a3', 5, 6), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'proj', '/tmp/b.rs', 'function', 'B1', 'hash-b1', 1, 2), \
                 ('chunk-b2', 'symbol', 'sym-b2', 'proj', '/tmp/b.rs', 'function', 'B2', 'hash-b2', 3, 4), \
                 ('chunk-b3', 'symbol', 'sym-b3', 'proj', '/tmp/b.rs', 'function', 'B3', 'hash-b3', 5, 6), \
                 ('chunk-c1', 'symbol', 'sym-c1', 'proj', '/tmp/c.rs', 'function', 'C1', 'hash-c1', 1, 2)",
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
                 ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/limited.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'proj', '/tmp/limited.rs', 'function', 'A2', 'hash-a2', 3, 4), \
                 ('chunk-a3', 'symbol', 'sym-a3', 'proj', '/tmp/limited.rs', 'function', 'A3', 'hash-a3', 5, 6)",
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
                 ('chunk-b1', 'symbol', 'sym-b1', 'proj', '/tmp/complete.rs', 'function', 'B1', 'hash-b1', 1, 2), \
                 ('chunk-b2', 'symbol', 'sym-b2', 'proj', '/tmp/complete.rs', 'function', 'B2', 'hash-b2', 3, 4)",
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
                 ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/bytes-a.rs', 'function', 'AAAA', 'hash-a1', 1, 2), \
                 ('chunk-b1', 'symbol', 'sym-b1', 'proj', '/tmp/bytes-b.rs', 'function', 'BBBB', 'hash-b1', 1, 2)",
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
                 ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/vectorized.rs', 'function', 'A1', 'hash-a1', 1, 2), \
                 ('chunk-a2', 'symbol', 'sym-a2', 'proj', '/tmp/vectorized.rs', 'function', 'A2', 'hash-a2', 3, 4)",
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
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
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
                chunk_id: "a1".to_string(),
                content_hash: "ha1".to_string(),
                text: "A1".to_string(),
            },
            VectorChunkWorkItem {
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
        let persist = prepared
            .into_persist_plan(vec![vec![0.1_f32, 0.2_f32], vec![0.3_f32, 0.4_f32]])
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
    fn test_prepared_vector_embed_batch_rejects_embedding_count_mismatch() {
        let mut plan = VectorBatchPlan::default();
        plan.work_items = vec![VectorChunkWorkItem {
            chunk_id: "a1".to_string(),
            content_hash: "ha1".to_string(),
            text: "A1".to_string(),
        }];

        let prepared = PreparedVectorEmbedBatch::from_plan(plan);
        let err = prepared.into_persist_plan(Vec::new()).unwrap_err();
        assert!(err.to_string().contains("embedding count mismatch"));
    }

    #[test]
    fn test_finalize_completed_vectorization_works_clears_queue_and_enqueues_projection() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/finalize_vector.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
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
                        claim_token: String::new(),
                        lease_epoch: 0,
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
            1
        );
    }

    #[test]
    fn test_finalize_completed_vectorization_works_marks_file_ready_under_file_level_contract() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/file_level_ready.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-1', 'symbol', 'sym-1', 'proj', '/tmp/file_level_ready.rs', 'function', 'fn a() {}', 'hash-a', 1, 1)",
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
                        claim_token: String::new(),
                        lease_epoch: 0,
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
                 ('/tmp/finalize_a.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100), \
                 ('/tmp/finalize_b.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
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
                        claim_token: String::new(),
                        lease_epoch: 0,
                    },
                FileVectorizationLeaseSnapshot {
                        file_path: "/tmp/finalize_b.rs".to_string(),
                        claim_token: String::new(),
                        lease_epoch: 0,
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
            2
        );
    }

    #[test]
    fn test_request_prepared_vector_embed_sequence_round_trips_over_prepare_channel() {
        let store = std::sync::Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        std::thread::spawn(move || {
            let request = prepare_rx.recv().unwrap();
            request
                .reply
                .send(PreparedVectorEmbedSequence {
                    batches: vec![PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
                        work_items: vec![],
                        texts: vec!["prepared".to_string()],
                        token_counts: vec![],
                        encoded_micro_batches: vec![],
                        touched_works: request.claimed.into_inner(),
                        finalize_after_success: vec![],
                        immediate_completed: vec![],
                        oversized_works: vec![],
                        next_active_after_success: vec![],
                        next_active_after_failure: vec![],
                        files_touched: 1,
                        partial_file_cycles: 0,
                        fetch_ms_total: 0,
                        failed_fetches: vec![],
                    })],
                    remaining_claimed_after_success: ClaimedLeaseSet::new(vec![]),
                })
                .unwrap();
        });

        let prepared = request_prepared_vector_embed_sequence(
            &store,
            0,
            &prepare_tx,
            vec![FileVectorizationWork {
                file_path: "/tmp/prepared.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            64,
            64,
            512 * 1024,
            2,
        )
        .unwrap();

        assert_eq!(prepared.batches.len(), 1);
        assert_eq!(prepared.batches[0].texts, vec!["prepared".to_string()]);
        assert_eq!(prepared.batches[0].touched_works.len(), 1);
        assert_eq!(prepared.batches[0].files_touched, 1);
    }

    #[test]
    fn test_flush_completed_vectorization_works_enqueues_finalize_request() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
                 ('/tmp/flush_finalize.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 10, 1, 100)",
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
    fn test_dispatch_prepared_vector_embed_sequence_returns_reply_channel() {
        let (prepare_tx, prepare_rx) = bounded::<VectorPrepareRequest>(1);
        std::thread::spawn(move || {
            let request = prepare_rx.recv().unwrap();
            request
                .reply
                .send(PreparedVectorEmbedSequence {
                    batches: vec![PreparedBatchEnvelope::new(PreparedVectorEmbedBatch {
                        work_items: vec![],
                        texts: vec!["prefetched".to_string()],
                        token_counts: vec![],
                        encoded_micro_batches: vec![],
                        touched_works: request.claimed.into_inner(),
                        finalize_after_success: vec![],
                        immediate_completed: vec![],
                        oversized_works: vec![],
                        next_active_after_success: vec![],
                        next_active_after_failure: vec![],
                        files_touched: 1,
                        partial_file_cycles: 0,
                        fetch_ms_total: 0,
                        failed_fetches: vec![],
                    })],
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

        assert_eq!(prepared.batches.len(), 1);
        assert_eq!(prepared.batches[0].texts, vec!["prefetched".to_string()]);
        assert_eq!(prepared.batches[0].touched_works.len(), 1);
        assert_eq!(prepared.batches[0].files_touched, 1);
    }

    #[test]
    fn test_build_prepared_vector_embed_sequence_stops_at_requested_depth() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a', 'symbol', 'sym-a', 'proj', '/tmp/a.rs', 'function', 'fn a() {}', 'hash-a', 1, 1), \
             ('chunk-b', 'symbol', 'sym-b', 'proj', '/tmp/b.rs', 'function', 'fn b() {}', 'hash-b', 1, 1), \
             ('chunk-c', 'symbol', 'sym-c', 'proj', '/tmp/c.rs', 'function', 'fn c() {}', 'hash-c', 1, 1)"
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

        let sequence = build_prepared_vector_embed_sequence(&store, &active, 1, 1, 512 * 1024, 2);

        assert_eq!(sequence.batches.len(), 2);
        assert_eq!(sequence.remaining_claimed_after_success.as_slice().len(), 1);
    }

    #[test]
    fn test_build_prepared_vector_embed_sequence_does_not_reselect_reserved_chunks() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a1', 'symbol', 'sym-a1', 'proj', '/tmp/a.rs', 'function', 'fn a1() {}', 'hash-a1', 1, 1), \
             ('chunk-a2', 'symbol', 'sym-a2', 'proj', '/tmp/a.rs', 'function', 'fn a2() {}', 'hash-a2', 2, 2), \
             ('chunk-b1', 'symbol', 'sym-b1', 'proj', '/tmp/b.rs', 'function', 'fn b1() {}', 'hash-b1', 1, 1)"
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

        let sequence = build_prepared_vector_embed_sequence(&store, &active, 1, 1, 512 * 1024, 3);

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
             ('chunk-oversized', 'symbol', 'sym-oversized', 'proj', '/tmp/oversized.rs', 'function', '{}', 'hash-oversized', 1, 1)",
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        queue_pending: usize,
        interactive_active: bool,
        gpu_memory_pressure: bool,
        embed_calls_total: u64,
        chunks_embedded_total: u64,
        files_touched_total: u64,
        embed_ms_total: u64,
    ) -> VectorBatchControllerObservation {
        controller_observation_with_runtime(
            queue_pending,
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
        queue_pending: usize,
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
            queue_pending,
            interactive_active,
            gpu_memory_pressure,
            metrics: VectorRuntimeMetrics {
                fetch_ms_total: 0,
                embed_ms_total,
                db_write_ms_total: 0,
                completion_check_ms_total: 0,
                mark_done_ms_total: 0,
                batches_total: embed_calls_total,
                chunks_embedded_total,
                files_completed_total: 0,
                embed_calls_total,
                claimed_work_items_total: 0,
                partial_file_cycles_total: 0,
                mark_done_calls_total: 0,
                files_touched_total,
                prepare_dispatch_total: 0,
                prepare_prefetch_total: 0,
                prepare_fallback_inline_total: 0,
                prepared_work_items_total: 0,
                prepare_empty_batches_total: 0,
                prepare_immediate_completed_total: 0,
                prepare_failed_fetches_total: 0,
                finalize_enqueued_total: 0,
                finalize_fallback_inline_total: 0,
                prepare_reply_wait_ms_total: 0,
                prepare_send_wait_ms_total: 0,
                finalize_send_wait_ms_total: 0,
                prepare_queue_wait_ms_total: 0,
                finalize_queue_wait_ms_total: 0,
                prepare_queue_depth_current: 0,
                prepare_queue_depth_max: 0,
                ready_queue_depth_current,
                ready_queue_depth_max: 0,
                finalize_queue_depth_current: 0,
                finalize_queue_depth_max: 0,
                persist_queue_depth_current: 0,
                persist_queue_depth_max: 0,
                persist_send_wait_ms_total: 0,
                persist_queue_wait_ms_total: 0,
                gpu_idle_wait_ms_total,
                embed_input_texts_total: 0,
                embed_input_text_bytes_total: 0,
                embed_clone_ms_total: 0,
                embed_transform_ms_total: 0,
                embed_export_ms_total: 0,
                embed_attempts_total: 0,
                embed_inflight_started_at_ms: 0,
                embed_inflight_texts_current: 0,
                embed_inflight_text_bytes_current: 0,
                vector_workers_started_total: 0,
                vector_workers_stopped_total: 0,
                vector_workers_active_current: 0,
                vector_worker_heartbeat_at_ms: 0,
            },
        }
    }

    #[test]
    fn test_vector_batch_controller_grows_targets_when_idle_backlog_is_underfed() {
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
        let _guard = ENV_TEST_GUARD.lock().unwrap();
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
        assert_eq!(diagnostics.target_embed_batch_chunks, 104);
        assert_eq!(diagnostics.target_files_per_cycle, 32);
        assert_eq!(diagnostics.adjustments_total, 3);
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
        assert_eq!(diagnostics.target_embed_batch_chunks, 16);
        assert_eq!(diagnostics.target_files_per_cycle, 8);
        assert_eq!(diagnostics.reason, "holding_density");
        assert_eq!(diagnostics.adjustments_total, 0);
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
