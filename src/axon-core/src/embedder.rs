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
#[path = "embedder/inline_embed.rs"]
pub(crate) mod inline_embed;
#[path = "embedder/parquet_embedding_store.rs"]
pub(crate) mod parquet_embedding_store;
#[path = "embedder/provider_contract.rs"]
mod provider_contract;
#[path = "embedder/provider_runtime.rs"]
mod provider_runtime;
#[path = "embedder/vector_executor.rs"]
mod vector_executor;
#[path = "embedder/vector_maintenance_loop.rs"]
mod vector_maintenance_loop;
#[path = "embedder/vector_worker_loop.rs"]
mod vector_worker_loop;

#[cfg(test)]
pub(crate) use batch_lanes::reset_token_lane_classifier_for_tests;
#[cfg(test)]
pub(crate) use batch_lanes::TokenLaneThresholdSource;
pub(crate) use batch_lanes::{
    current_token_lane_thresholds, observe_token_lane_thresholds, TokenLaneThresholds,
    VectorBatchLane,
};
pub(crate) use cpu_query_service::spawn_brain_query_worker_if_needed;
use gpu_backend::{
    cuda_execution_provider_dispatch, ort_cuda_provider_library_available,
    ort_cuda_provider_library_path, OrtGpuFirstTextEmbedding,
};
#[cfg(test)]
use gpu_backend::{cuda_memory_limit_bytes, cuda_tf32_enabled};
pub use gpu_policy::current_gpu_memory_pressure_active;
use gpu_policy::{
    embedding_provider_requested_is_gpu, gpu_recycle_immediate_required,
    gpu_recycle_vram_summit_mb,
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
    publish_embedding_provider_state, register_embedding_provider_diagnostics,
    set_embedding_provider_runtime_state,
};
pub use provider_runtime::{
    current_embedding_provider_diagnostics, embedding_provider_diagnostics,
    EmbeddingProviderDiagnostics,
};
use vector_executor::VectorEmbeddingBackend;

#[allow(dead_code)]
pub(crate) fn embedder_cuda_execution_provider_dispatch(
) -> ort::execution_providers::ExecutionProviderDispatch {
    gpu_backend::cuda_execution_provider_dispatch()
}

#[allow(dead_code)]
pub(crate) fn embedder_ort_cuda_provider_library_available() -> bool {
    gpu_backend::ort_cuda_provider_library_available()
}

const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);
const VECTOR_PERSIST_QUEUE_BOUND: usize = 4;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS: u64 = 120_000;
const DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS: u64 = 10_000;
const DEFAULT_VECTOR_CLAIMABLE_SUPPLY_POLL_INTERVAL_MS: u64 = 250;

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







static QUERY_EMBEDDING_SENDER: OnceLock<Mutex<Option<Sender<QueryEmbeddingRequest>>>> =
    OnceLock::new();

pub struct SemanticWorkerPool {
    _query_workers: Vec<thread::JoinHandle<()>>,
    _vector_workers: Vec<thread::JoinHandle<()>>,
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

fn default_chunks_per_file_estimate() -> usize {
    std::env::var("AXON_VECTOR_DEFAULT_CHUNKS_PER_FILE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
}

fn wait_for_vector_backlog_or_timeout(timeout: Duration) -> bool {
    service_guard::wait_for_vector_backlog_signal(timeout)
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
) -> AnyhowResult<(Vec<Vec<f32>>, u64, u64, u64, u64, u64)> {
    if texts.is_empty() {
        return Ok((Vec::new(), 0, 0, 0, 0, 0));
    }

    // REQ-AXO-176 — instrument tokenization separately. Pre-fix the
    // bench reported `total_embed_ms - sum(host+input+inference+output)`
    // as ~60% of total on n=600, with no breakdown showing where it
    // went. Confirmed via this timer: encode_batch + micro-batch
    // build is the unaccounted phase.
    let tokenize_start = std::time::Instant::now();
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
    let tokenize_ms = tokenize_start.elapsed().as_millis() as u64;

    let mut ordered_embeddings = vec![None; texts.len()];
    let mut host_prepare_ms = 0u64;
    let mut input_copy_ms = 0u64;
    let mut inference_ms = 0u64;
    let mut output_extract_ms = 0u64;

    for batch_indices in micro_batches {
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
    }

    let embeddings = ordered_embeddings
        .into_iter()
        .map(|embedding| {
            embedding.ok_or_else(|| anyhow!("missing embedding after ORT micro-batch scheduling"))
        })
        .collect::<AnyhowResult<Vec<_>>>()?;

    Ok((
        embeddings,
        tokenize_ms,
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
}

/// DEC-AXO-072 follow-up: spawn a thread that issues `CHECKPOINT` against
/// the writer connection every 10s. Prevents WAL accumulation that drove
/// commit_ms from 132ms to 22s+ in VAL-AXO-034 profiling. The CHECKPOINT
/// itself takes the writer mutex briefly; cadence chosen to amortize over
/// many writes without long pauses.
fn spawn_background_checkpoint_thread(graph_store: Arc<GraphStore>) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(10));
        let started = Instant::now();
        match graph_store.execute("CHECKPOINT;") {
            Ok(()) => {
                let elapsed_ms = started.elapsed().as_millis();
                if elapsed_ms > 250 {
                    info!(
                        "Background CHECKPOINT took {} ms",
                        elapsed_ms
                    );
                }
            }
            Err(e) => {
                warn!("Background CHECKPOINT failed: {:?}", e);
            }
        }
    });
    info!("Background CHECKPOINT thread spawned (10s cadence)");
}

/// DEC-AXO-072 J.2: hydrate the hot status cache from FileVectorizationQueue
/// and spawn the periodic flush thread (100ms timer). Called only when
/// `AXON_HOT_STATUS_CACHE_ENABLED=true`.
fn spawn_hot_status_cache_workers(
    graph_store: Arc<GraphStore>,
    cache: Arc<crate::hot_status_cache::HotStatusCache>,
) {
    use crate::hot_status_cache::{
        parse_fvq_status, render_flush_queries, FileHotState, FvqStatus,
    };

    // Boot hydration: load existing FVQ rows so the lane sees pending
    // work after a restart. Only hydrate live rows (queued/inflight) —
    // done/failed are terminal and stay in DB only.
    match graph_store.query_json(
        "SELECT file_path, status, COALESCE(claim_token, ''), COALESCE(lease_epoch, 0), \
                COALESCE(lease_owner, ''), COALESCE(queued_at, 0), COALESCE(claimed_at_ms, 0), \
                COALESCE(last_error_reason, '') \
         FROM FileVectorizationQueue \
         WHERE status IN ('queued', 'inflight')",
    ) {
        Ok(raw) => {
            let rows: Vec<Vec<serde_json::Value>> =
                serde_json::from_str(&raw).unwrap_or_default();
            let mut hydrated = 0usize;
            for row in rows {
                let path = row.first().and_then(|v| v.as_str()).unwrap_or("").to_string();
                if path.is_empty() {
                    continue;
                }
                let status_s = row.get(1).and_then(|v| v.as_str()).unwrap_or("");
                let Some(status) = parse_fvq_status(status_s) else {
                    continue;
                };
                let claim_token = row
                    .get(2)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let lease_epoch = row.get(3).and_then(|v| v.as_i64()).unwrap_or(0);
                let lease_owner = row
                    .get(4)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let enqueued_at_ms = row.get(5).and_then(|v| v.as_i64()).unwrap_or(0);
                let claimed_at_ms = row.get(6).and_then(|v| v.as_i64()).filter(|v| *v > 0);
                let last_error = row
                    .get(7)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let started_at_ms = if matches!(status, FvqStatus::Inflight) {
                    claimed_at_ms
                } else {
                    None
                };
                let state = FileHotState {
                    status,
                    claim_token,
                    lease_epoch,
                    lease_owner,
                    enqueued_at_ms,
                    started_at_ms,
                    last_error,
                    last_change_at_ms: enqueued_at_ms.max(claimed_at_ms.unwrap_or(0)),
                };
                cache.upsert_from_db(&path, state);
                hydrated += 1;
            }
            info!("Hot status cache: hydrated {} entries from FVQ", hydrated);
        }
        Err(e) => {
            warn!("Hot status cache: hydration failed: {:?}", e);
        }
    }

    // Periodic flush thread: snapshot dirty entries every 100ms, render
    // a single batched UPSERT, execute. Runs forever; the indexer
    // process is the only owner.
    let flush_store = graph_store;
    let flush_cache = cache;
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(100));
        let snap = flush_cache.snapshot_dirty();
        if snap.is_empty() {
            continue;
        }
        let queries = render_flush_queries(&snap);
        if queries.is_empty() {
            continue;
        }
        if let Err(e) = flush_store.execute_batch(&queries) {
            warn!(
                "Hot status cache flush: execute_batch failed for {} entries: {:?}",
                snap.len(),
                e
            );
        }
    });
    info!("Hot status cache: flush thread spawned (100ms cadence)");
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

        // DEC-AXO-073 L.1: install Parquet embedding side-store singleton.
        // Disabled by default; vector_lane falls through to DuckDB INSERT.
        // Activated via AXON_PARQUET_EMBEDDING_STORE_ENABLED=true.
        let _ = parquet_embedding_store::install(Arc::new(
            parquet_embedding_store::ParquetEmbeddingStore::new(
                parquet_embedding_store::default_base_dir(),
            ),
        ));

        // DEC-AXO-072 J.2: install hot status cache singleton; enable per
        // env. Cache disabled by default — the flush thread below
        // does nothing and graph_ingestion / vector_lane fall through to
        // direct-DB paths (commit G + H.2 behavior preserved).
        let _ = crate::hot_status_cache::install(std::sync::Arc::new(
            crate::hot_status_cache::HotStatusCache::new(),
        ));
        if let Some(cache) = crate::hot_status_cache::cache() {
            cache.set_enabled(crate::hot_status_cache::parse_env_enabled());
            if cache.is_enabled() {
                spawn_hot_status_cache_workers(Arc::clone(&graph_store), cache);
            }
        }

        // DEC-AXO-072 follow-up VAL-AXO-034: background CHECKPOINT thread
        // every 10s. Profiling showed commit_ms grows to 22s+ as the WAL
        // accumulates between checkpoints (default DuckDB threshold doesn't
        // fire often enough for this workload). Forcing periodic
        // CHECKPOINTs keeps WAL bounded and per-op cost flat over time.
        // Disable via AXON_BG_CHECKPOINT_DISABLED=true if it ever causes
        // issues.
        if !std::env::var("AXON_BG_CHECKPOINT_DISABLED")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true"))
            .unwrap_or(false)
        {
            spawn_background_checkpoint_thread(Arc::clone(&graph_store));
        }

        let mut query_workers = Vec::new();
        for worker_idx in 0..config.query_workers {
            let query_rx = query_rx.clone();
            query_workers.push(thread::spawn(move || {
                Self::query_worker_loop(worker_idx, query_rx);
            }));
        }

        // DEC-AXO-070 single-loop vector lane: claim → prepare → embed →
        // persist → finalize, all in one thread per worker. Channels and the
        // 5-loop pipeline are gone. Maintenance loop (stale-inflight recovery)
        // remains as a separate thread.
        let mut vector_workers = Vec::new();
        let mut vector_maintenance_workers = Vec::new();
        if config.vector_workers > 0 {
            let maintenance_graph_store = Arc::clone(&graph_store);
            vector_maintenance_workers.push(thread::spawn(move || {
                vector_maintenance_loop::vector_maintenance_worker_loop(maintenance_graph_store);
            }));
        }
        for worker_idx in 0..config.vector_workers {
            let lane_graph_store = Arc::clone(&graph_store);
            vector_workers.push(thread::spawn(move || {
                vector_worker_loop::vector_lane_worker(worker_idx, lane_graph_store);
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
            _vector_workers: vector_workers,
            _vector_maintenance_workers: vector_maintenance_workers,
            _graph_workers: graph_workers,
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


/// REQ-AXO-176 — Public benchmarking facade for the in-process ORT
/// embedder. Accepts a vector of texts, loads the configured BGE
/// model with `force_gpu` honoured, and runs `embed_texts_with_breakdown_ort`
/// once. Returns timing breakdown for fast-iteration perf work
/// (target throughput: 30 chunks/s end-to-end → 200 chunks/s stretch).
///
/// All ORT environment variables (`ORT_DYLIB_PATH`,
/// `AXON_GPU_EMBED_SERVICE_TENSORRT`, etc.) MUST be set by the
/// caller — this function does not patch the process env.
pub fn run_embedder_throughput_bench(
    label: &str,
    texts: Vec<String>,
    force_gpu: bool,
) -> anyhow::Result<EmbeddingThroughputBench> {
    let load_start = std::time::Instant::now();
    let mut model = OrtGpuFirstTextEmbedding::try_new(label, 0, force_gpu)?;
    let load_ms = load_start.elapsed().as_millis() as u64;

    let n = texts.len();
    let embed_start = std::time::Instant::now();
    let (
        embeddings,
        tokenize_ms,
        host_prepare_ms,
        input_copy_ms,
        inference_ms,
        output_extract_ms,
    ) = embed_texts_with_breakdown_ort(&mut model, &texts)?;
    let total_embed_ms = embed_start.elapsed().as_millis() as u64;

    Ok(EmbeddingThroughputBench {
        n,
        embedding_dim: embeddings.first().map(|v| v.len()).unwrap_or(0),
        load_ms,
        total_embed_ms,
        tokenize_ms,
        host_prepare_ms,
        input_copy_ms,
        inference_ms,
        output_extract_ms,
    })
}

/// REQ-AXO-176 — Multi-worker variant. Spawns `workers` threads, each
/// with its own `OrtGpuFirstTextEmbedding` instance, partitions `texts`
/// roughly evenly, and runs the embed phase concurrently. Aggregate
/// throughput is `total_n / max(per-worker wall time)`.
///
/// Note: each worker pays the model-load cost (~12 s on warm cache,
/// ~30-60 s cold). VRAM scales with `workers` (each instance ≈ 680 MB
/// engine + activation). On 8 GB GPUs, 2 workers fit; 3+ may OOM.
pub fn run_embedder_throughput_bench_parallel(
    label: &str,
    texts: Vec<String>,
    force_gpu: bool,
    workers: usize,
) -> anyhow::Result<EmbeddingThroughputBench> {
    if workers <= 1 {
        return run_embedder_throughput_bench(label, texts, force_gpu);
    }
    let n_total = texts.len();
    if n_total == 0 {
        anyhow::bail!("parallel bench requires at least one text");
    }

    // Partition texts into `workers` chunks. Last shard absorbs the
    // remainder so the work is balanced within ±1 chunk.
    let shard_size = n_total.div_ceil(workers);
    let mut shards: Vec<Vec<String>> = Vec::with_capacity(workers);
    let mut iter = texts.into_iter();
    for _ in 0..workers {
        let mut shard = Vec::with_capacity(shard_size);
        for _ in 0..shard_size {
            match iter.next() {
                Some(t) => shard.push(t),
                None => break,
            }
        }
        if !shard.is_empty() {
            shards.push(shard);
        }
    }
    let actual_workers = shards.len();

    let parallel_start = std::time::Instant::now();
    let handles: Vec<_> = shards
        .into_iter()
        .enumerate()
        .map(|(idx, shard)| {
            let label = format!("{label}-w{idx}");
            std::thread::spawn(move || -> anyhow::Result<EmbeddingThroughputBench> {
                let load_start = std::time::Instant::now();
                let mut model = OrtGpuFirstTextEmbedding::try_new(&label, idx, force_gpu)?;
                let load_ms = load_start.elapsed().as_millis() as u64;

                let n = shard.len();
                let embed_start = std::time::Instant::now();
                let (
                    embeddings,
                    tokenize_ms,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                ) = embed_texts_with_breakdown_ort(&mut model, &shard)?;
                let total_embed_ms = embed_start.elapsed().as_millis() as u64;
                Ok(EmbeddingThroughputBench {
                    n,
                    embedding_dim: embeddings.first().map(|v| v.len()).unwrap_or(0),
                    load_ms,
                    total_embed_ms,
                    tokenize_ms,
                    host_prepare_ms,
                    input_copy_ms,
                    inference_ms,
                    output_extract_ms,
                })
            })
        })
        .collect();

    let mut aggregate = EmbeddingThroughputBench {
        n: 0,
        embedding_dim: 0,
        load_ms: 0,
        total_embed_ms: 0,
        tokenize_ms: 0,
        host_prepare_ms: 0,
        input_copy_ms: 0,
        inference_ms: 0,
        output_extract_ms: 0,
    };

    for h in handles {
        let res = h
            .join()
            .map_err(|payload| {
                anyhow!(
                    "worker thread panicked: {}",
                    payload
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "<no-payload>".to_string())
                )
            })??;
        aggregate.n = aggregate.n.saturating_add(res.n);
        if res.embedding_dim > aggregate.embedding_dim {
            aggregate.embedding_dim = res.embedding_dim;
        }
        // Per-worker timings: keep MAX (slowest worker bounds wall time)
        // so chunks_per_second() reflects parallel throughput.
        if res.load_ms > aggregate.load_ms {
            aggregate.load_ms = res.load_ms;
        }
        if res.total_embed_ms > aggregate.total_embed_ms {
            aggregate.total_embed_ms = res.total_embed_ms;
        }
        if res.tokenize_ms > aggregate.tokenize_ms {
            aggregate.tokenize_ms = res.tokenize_ms;
        }
        if res.inference_ms > aggregate.inference_ms {
            aggregate.inference_ms = res.inference_ms;
        }
        // host_prepare/input_copy/output_extract are reported as 0 by
        // the inner timer in practice — leave as max for completeness.
        if res.host_prepare_ms > aggregate.host_prepare_ms {
            aggregate.host_prepare_ms = res.host_prepare_ms;
        }
        if res.input_copy_ms > aggregate.input_copy_ms {
            aggregate.input_copy_ms = res.input_copy_ms;
        }
        if res.output_extract_ms > aggregate.output_extract_ms {
            aggregate.output_extract_ms = res.output_extract_ms;
        }
    }

    // Override total_embed_ms with the actual parallel wall time so
    // chunks_per_second is computed on the real elapsed window, not
    // the slowest single-worker phase (in case load was async).
    let parallel_wall_ms = parallel_start.elapsed().as_millis() as u64;
    if parallel_wall_ms > aggregate.total_embed_ms {
        aggregate.total_embed_ms = parallel_wall_ms;
    }

    let _ = actual_workers;
    Ok(aggregate)
}

/// REQ-AXO-176 — Result of `run_embedder_throughput_bench`. All times
/// are in milliseconds. `inference_ms` is GPU/CPU compute only;
/// `total_embed_ms` includes tokenization + host prep + inference +
/// output extract.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingThroughputBench {
    pub n: usize,
    pub embedding_dim: usize,
    pub load_ms: u64,
    pub total_embed_ms: u64,
    /// REQ-AXO-176 — encode_batch + token-count + micro-batch build.
    /// Empirically the dominant non-inference cost on n>100 with BGE-Large.
    pub tokenize_ms: u64,
    pub host_prepare_ms: u64,
    pub input_copy_ms: u64,
    pub inference_ms: u64,
    pub output_extract_ms: u64,
}

impl EmbeddingThroughputBench {
    /// Throughput in chunks per second over the embed phase only
    /// (excludes model load).
    pub fn chunks_per_second(&self) -> f64 {
        if self.total_embed_ms == 0 || self.n == 0 {
            return 0.0;
        }
        (self.n as f64) * 1000.0 / (self.total_embed_ms as f64)
    }
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
        attach_preencoded_micro_batches,
        build_token_aware_micro_batches, build_vector_batch_plan, configured_embedding_max_length,
        cuda_execution_provider_dispatch, current_runtime_tuning_snapshot,
        current_runtime_tuning_state, current_token_lane_thresholds,
        effective_embedding_provider_is_gpu,
        embedding_download_progress_enabled, embedding_lane_config_from_env,
        embedding_model_cache_dir, embedding_provider_diagnostics,
        gpu_memory_soft_limit_mb, is_fatal_embedding_error,
        load_runtime_embedding_tokenizer, observe_token_lane_thresholds, query_embedding_allowed,
        request_query_embedding,
        reset_token_lane_classifier_for_tests, EmbeddingLaneConfig,
        GpuMemorySnapshot, PreparedVectorEmbedBatch, QueryEmbeddingRequest, TokenLaneThresholdSource,
        TokenLaneThresholds, VectorBatchLane, VectorBatchPlan, VectorChunkWorkItem,
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
