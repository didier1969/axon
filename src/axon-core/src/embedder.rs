use crate::embedding_contract::fastembed_model;
use crate::embedding_profile::{
    configured_embedding_max_length as profile_configured_embedding_max_length,
    configured_embedding_token_bucket_size as profile_configured_embedding_token_bucket_size,
    embedding_model_cache_dir as profile_embedding_model_cache_dir,
    load_runtime_embedding_tokenizer as profile_load_runtime_embedding_tokenizer,
    runtime_embedding_snapshot_dir as profile_runtime_embedding_snapshot_dir,
};
use crate::graph::GraphStore;
use crate::queue::QueueStore;
use crate::runtime_mode::canonical_embedding_provider_request_for_mode;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_profile::{recommend_embedding_lane_sizing, RuntimeProfile};
use crate::runtime_tuning::{
    current_runtime_tuning_snapshot as runtime_tuning_snapshot,
    current_runtime_tuning_state as runtime_tuning_state,
    update_runtime_tuning_state as update_shared_runtime_tuning_state, RuntimeTuningSnapshot,
    RuntimeTuningState,
};
use crate::service_guard::{self, ServicePressure};
use crate::vector_control::reset_vector_batch_controller_for_tests;
use anyhow::{anyhow, Result as AnyhowResult};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use fastembed::{InitOptions, OutputKey, TextEmbedding};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tokenizers::{Encoding, Tokenizer};
use tracing::{error, info};

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
#[path = "embedder/lifecycle.rs"]
pub(crate) mod lifecycle;
#[path = "embedder/lifecycle_machine.rs"]
pub(crate) mod lifecycle_machine;
#[path = "embedder/provider_contract.rs"]
mod provider_contract;
#[path = "embedder/provider_runtime.rs"]
mod provider_runtime;

pub(crate) use cpu_query_service::spawn_brain_query_worker_if_needed;
pub(crate) use gpu_backend::OrtGpuFirstTextEmbedding;
pub(crate) use gpu_backend::{
    cuda_execution_provider_dispatch, ort_cuda_provider_library_available,
    ort_cuda_provider_library_path,
};
#[cfg(test)]
use gpu_backend::{cuda_memory_limit_bytes, cuda_tf32_enabled};
pub use gpu_policy::current_gpu_memory_pressure_active;
use gpu_policy::embedding_provider_requested_is_gpu;
#[cfg(test)]
use gpu_policy::gpu_memory_pressure_active;
#[cfg(test)]
pub(crate) use gpu_telemetry::clear_gpu_memory_snapshot_cache_for_tests;
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
pub use provider_contract::{
    ProductionLane, ProviderResolution, ProviderStrategy, ProviderSupportRole,
};
// REQ-AXO-901836 — brain status composer (mcp::tools_framework_runtime_status)
// re-derives the resolution from a paired indexer's heartbeat-supplied
// requested/effective labels so the surfaced vector_pipeline_telemetry stays
// coherent. Keep the re-export crate-wide, not test-only.
use provider_runtime::cpu_provider_effective_label;
pub(crate) use provider_runtime::provider_resolution_for_label;
pub use provider_runtime::{
    current_embedding_provider_diagnostics, current_gpu_present, embedder_provider_fallback_reason,
    embedding_provider_diagnostics, set_gpu_present, EmbeddingProviderDiagnostics,
};

const CHUNK_BATCH_SIZE: usize = 16;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);
const VECTOR_PERSIST_QUEUE_BOUND: usize = 4;
const MAX_CHUNKS_PER_FILE: usize = 64;
const MAX_EMBED_BATCH_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_VECTOR_STALE_INFLIGHT_CLAIM_AGE_MS: u64 = 120_000;
const DEFAULT_VECTOR_STALE_INFLIGHT_RECOVERY_INTERVAL_MS: u64 = 10_000;

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

/// REQ-AXO-901979 — process-local effective-compute of THIS process's in-process
/// query embedding worker (cpu_query_service). 0 = unknown, 1 = CPU, 2 = GPU.
/// The worker KNOWS its provider at model-build time (it loaded the CUDA EP or
/// not), so for self-observation this is strictly more precise than the
/// cross-process `nvidia-smi --query-compute-apps` verdict (which masks
/// per-process memory as [N/A] on WSL2 and reads the INDEXER's heartbeat — absent
/// in brain_only → the stale `compute:CPU` lie after REQ-AXO-901978 B1). NOT the
/// cross-process slot DEC-AXO-901626 retired: this is the brain reporting ITSELF.
static QUERY_WORKER_COMPUTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub(crate) fn set_query_worker_compute_gpu(is_gpu: bool) {
    QUERY_WORKER_COMPUTE.store(
        if is_gpu { 2 } else { 1 },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// `Some("GPU")` / `Some("CPU")` once this process's query worker has built its
/// model ; `None` before the first build (unknown). Read by status /
/// embedding_status to report the brain's OWN embedder compute in brain_only,
/// where no indexer heartbeat exists.
pub(crate) fn query_worker_compute_label() -> Option<&'static str> {
    match QUERY_WORKER_COMPUTE.load(std::sync::atomic::Ordering::Relaxed) {
        2 => Some("GPU"),
        1 => Some("CPU"),
        _ => None,
    }
}

/// REQ-AXO-901984 — runtime override of the query-embed provider, settable
/// WITHOUT restarting the indexer/brain. The GPU is often needed by Axon Live
/// or another service ; dev can release it (`cpu`) and re-grab it (`gpu`) on the
/// fly. 0 = unset (env + auto), 1 = cpu, 2 = gpu, 3 = auto (force GPU-detect,
/// ignore env). The query worker rebuilds its model lazily on the next request
/// when the reload generation changes.
static QUERY_EMBED_PROVIDER_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);
static QUERY_RELOAD_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Set the runtime query-embed provider override and bump the reload generation
/// so the worker rebuilds with the new provider on its next request. Returns the
/// normalised label on success.
pub(crate) fn set_query_embed_provider_override(value: &str) -> Result<&'static str, String> {
    let (code, label) = match value.trim().to_ascii_lowercase().as_str() {
        "cpu" => (1u8, "cpu"),
        "gpu" | "cuda" | "tensorrt" => (2u8, "gpu"),
        "auto" => (3u8, "auto"),
        other => {
            return Err(format!(
                "invalid query-embed provider `{other}` (expected cpu|gpu|auto)"
            ))
        }
    };
    QUERY_EMBED_PROVIDER_OVERRIDE.store(code, std::sync::atomic::Ordering::Relaxed);
    QUERY_RELOAD_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(label)
}

fn query_embed_provider_override() -> Option<&'static str> {
    match QUERY_EMBED_PROVIDER_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some("cpu"),
        2 => Some("gpu"),
        3 => Some("auto"),
        _ => None,
    }
}

/// `unset` | `cpu` | `gpu` | `auto` — the current runtime override (for status).
pub(crate) fn query_embed_provider_override_label() -> &'static str {
    query_embed_provider_override().unwrap_or("unset")
}

fn query_reload_generation() -> u64 {
    QUERY_RELOAD_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// REQ-AXO-901984 — the provider the query lane WOULD resolve to right now
/// (override → env → GPU-detect), for the `embed_provider`/status surfaces.
pub(crate) fn query_embed_effective_provider() -> String {
    effective_provider_request_for_lane("query")
}

pub struct SemanticWorkerPool {
    _query_workers: Vec<thread::JoinHandle<()>>,
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

pub(super) fn gpu_total_vram_hint_mb() -> Option<u64> {
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

pub(super) fn gpu_bootstrap_vector_worker_cap(
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

/// REQ-AXO-901634 — canonical disable flag for the graph-embedding lane.
/// Graph embeddings are on by default; `AXON_GRAPH_EMBEDDINGS_ENABLED=false`
/// (or `0`/`no`/`off`) forces `graph_workers=0`, overriding any explicit
/// `AXON_GRAPH_WORKERS`. Without this gate the disable flag was silently
/// ignored and operators kept getting graph workers they asked to turn off.
pub(crate) fn graph_embeddings_enabled_from_env() -> bool {
    std::env::var("AXON_GRAPH_EMBEDDINGS_ENABLED")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
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
        graph_workers: if graph_embeddings_enabled_from_env() {
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

fn token_count_from_encoding(encoding: &Encoding) -> usize {
    encoding
        .get_attention_mask()
        .iter()
        .map(|value| *value as usize)
        .sum::<usize>()
        .max(1)
}

fn load_runtime_embedding_tokenizer() -> AnyhowResult<std::sync::Arc<Tokenizer>> {
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

pub(crate) fn embed_texts_with_breakdown_ort(
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
            _batch_stats,
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
        let gpu_query_available =
            provider_runtime::current_gpu_present() && ort_cuda_provider_library_available();
        // REQ-AXO-901984 — runtime override (toggle GPU↔CPU without restart) beats
        // the env. `gpu` → CUDA when available else CPU ; `auto` → GPU-detect
        // (ignores env) ; `cpu` → always CPU (release the GPU for Live/others).
        match query_embed_provider_override() {
            Some("cpu") => return "cpu".to_string(),
            Some("gpu") => {
                return if gpu_query_available {
                    "cuda".to_string()
                } else {
                    "cpu".to_string()
                }
            }
            Some("auto") => {
                return if gpu_query_available {
                    "cuda".to_string()
                } else {
                    canonical_provider
                }
            }
            _ => {}
        }
        if let Some(explicit) = std::env::var("AXON_QUERY_EMBED_PROVIDER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
        {
            return explicit;
        }

        // REQ-AXO-901978 (B1) — the query lane embeds PUNCTUAL single texts.
        // When a GPU is present and the ORT CUDA provider library is available,
        // use it EVEN in brain_only / indexer_graph — where the BATCH-lane policy
        // (`canonical_embedding_provider_request_for_mode`) forces `cpu` because
        // semantic workers are disabled. In those modes the GPU is IDLE, so a
        // 1-text inference is ~ms on GPU vs ~seconds on CPU (telemetry: why 24s,
        // retrieve_context 10s, query 3.5s — all CPU-embed bound). fastembed's
        // `TextEmbedding` exposes the CUDA EP (not TensorRT), so request `cuda`
        // on this path. Falls through to the canonical (cpu) when no GPU or the
        // provider library is missing (e.g. CPU-only host) — never a hard error.
        if provider_runtime::current_gpu_present() && ort_cuda_provider_library_available() {
            return "cuda".to_string();
        }
        // The two ORT sessions (indexer batch + brain query) cohabit on the same
        // GPU via CUDA time-sharing in indexer modes; query embeds are punctual
        // (~ms) so contention with the pipeline is negligible.
        return canonical_provider;
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

// REQ-AXO-901872 : background CHECKPOINT thread retiré (résidu DuckDB-era). Le
// `CHECKPOINT;` forcé toutes les 10s prenait le writer-mutex et affamait le brain
// MCP (pics latence 5-7s, RCA session 72). PostgreSQL gère les checkpoints
// nativement — voir devenv.nix (checkpoint_completion_target + max_wal_size).

// REQ-AXO-901653 slice-5d — `spawn_hot_status_cache_workers` deleted.
// Subsystem hydrated from + flushed to public.FileVectorizationQueue
// (dropped slice-5a). Pipeline_v2 (REQ-AXO-289) owns chunk-state directly
// via the in-memory IstGraphView + Chunk/ChunkEmbedding rows.

impl SemanticWorkerPool {
    // REQ-AXO-901881 W1 — `graph_store` became unused once the dead
    // async_writer `install_global` (its only consumer) was removed; both
    // store params are now vestigial (the pool configures from env). Param
    // removal + call-site cleanup is deferred to a later wave.
    pub fn new(_graph_store: Arc<GraphStore>, _queue_store: Arc<QueueStore>) -> Self {
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

        // REQ-AXO-271 slice 1 (2026-05-10): the DEC-AXO-073 Parquet
        // embedding side-store + DEC-AXO-074 Parquet chunk-content
        // archiver were removed. Both were DuckDB column-store-cost
        // workarounds, redundant under the post-MIL-AXO-015 PG +
        // pgvector stack.

        // REQ-AXO-901881 W1 — the REQ-AXO-193 async-writer dispatcher
        // (install_global) was DuckDB-era dead machinery: never wired in
        // production (it only flushed RawQueries) and could still spawn a
        // useless writer thread under AXON_ASYNC_WRITER_ENABLED. Retired with
        // the async_writer module; the live write path is bulk_writer.

        // REQ-AXO-901653 slice-5d — hot status cache install block deleted.

        let mut query_workers = Vec::new();
        for worker_idx in 0..config.query_workers {
            let query_rx = query_rx.clone();
            query_workers.push(thread::spawn(move || {
                Self::query_worker_loop(worker_idx, query_rx);
            }));
        }

        // REQ-AXO-901653 Slice 1 — graph_worker_loop spawn removed.
        // The Semantic Graph Worker (graph projection embedding cache) was
        // structurally obsolete since MIL-AXO-017 (AGE retirement) +
        // REQ-AXO-271 slice 2e (refresh_*_projection became no-op +
        // GraphProjectionState never populated). The loop kept polling
        // public.GraphProjectionQueue (a DROP'd table) and emitted
        // `[pg_query_count] db error` + `graph projection fetch error`
        // every iteration, drowning logs and consuming PG conn pool.
        // Authoritative call-graph reads route through ist.Edge
        // (db/ddl/04_graph_functions.sql) ; the projection cache has no
        // remaining consumer.
        //
        // REQ-AXO-901653 Slice 2 — vector_worker_loop +
        // vector_maintenance_worker_loop + vector_pipeline_3stages deleted.
        // The legacy DuckDB-era single-loop vector lane (DEC-AXO-070) and the
        // 3-stage variant became dead code once pipeline_v2
        // (CPT-AXO-054 / DEC-AXO-081 / REQ-AXO-289) took over canonical
        // vectorization under the live runtime. The previous
        // AXON_LEGACY_VECTOR_WORKER_LOOP env-gate (REQ-AXO-901632, session 49)
        // defaulted to false ; this slice removes the gated code outright.
        Self {
            _query_workers: query_workers,
        }
    }

    pub(super) fn query_worker_loop(worker_idx: usize, query_rx: Receiver<QueryEmbeddingRequest>) {
        info!(
            "Semantic Query Worker [{}]: Initializing BGE-Large Model (1024d) in isolated thread...",
            worker_idx
        );

        // REQ-AXO-901945 — idle unload (long T_idle). The CPU query model
        // (~1.3 GB host RAM) is the right architecture (portable per
        // PIL-AXO-008, no GPU contention per PIL-AXO-007), but it never
        // released. We keep it warm during active sessions and drop it
        // only after a long idle window so a bursty interactive session
        // pays zero reload cost, while a machine left idle for 20 min+
        // reclaims the RAM. Wake = the next query rebuilds the model
        // (~1-3 s, BGE files served from the OS page cache). Knob:
        // AXON_QUERY_EMBED_IDLE_SECS (default 1200 = 20 min). 0 disables.
        let idle_timeout = query_embed_idle_timeout();

        let Some(model) = Self::build_text_embedding_model("query", worker_idx) else {
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
        // subsystem state. An idle-drop below stays Ready: it wakes
        // on demand, it is not a failure.
        crate::runtime_readiness::report_subsystem_state(
            crate::runtime_readiness::Subsystem::Embedder,
            crate::runtime_readiness::SubsystemState::Ready,
        );

        let mut slot: Option<TextEmbedding> = Some(model);
        // REQ-AXO-901984 — track the provider-override reload generation so a
        // runtime toggle (embed_provider set ...) drops the model and rebuilds
        // it with the new provider on the next request, without a restart.
        let mut local_reload_gen = query_reload_generation();
        loop {
            // When the model is loaded, wait at most `idle_timeout` so an
            // idle gap drops it. When already dropped, block indefinitely
            // (no spin) until the next request wakes us.
            let recv_result = match (slot.is_some(), idle_timeout) {
                (true, Some(timeout)) => query_rx.recv_timeout(timeout),
                _ => query_rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match recv_result {
                Ok(request) => {
                    // REQ-AXO-901984 — a runtime provider toggle bumped the reload
                    // generation: drop the current model so it rebuilds with the
                    // new provider below (reuses the wake/reload path).
                    let current_reload_gen = query_reload_generation();
                    if current_reload_gen != local_reload_gen {
                        local_reload_gen = current_reload_gen;
                        if slot.is_some() {
                            info!(
                                "Semantic Query Worker [{}]: provider override changed — dropping model to rebuild",
                                worker_idx
                            );
                            slot = None;
                        }
                    }
                    if slot.is_none() {
                        info!(
                            "Semantic Query Worker [{}]: waking — reloading BGE-Large model after idle drop",
                            worker_idx
                        );
                        match Self::build_text_embedding_model("query", worker_idx) {
                            Some(m) => slot = Some(m),
                            None => {
                                // Reload failed on wake — surface it and
                                // bail so the request reply closes rather
                                // than hanging on a dead worker.
                                crate::runtime_readiness::report_subsystem_state(
                                    crate::runtime_readiness::Subsystem::Embedder,
                                    crate::runtime_readiness::SubsystemState::Failed {
                                        reason: "model_reload_on_wake_failed".to_string(),
                                    },
                                );
                                drop(request); // closes reply_rx → caller gets Disconnected
                                return;
                            }
                        }
                    }
                    let m = slot.as_mut().expect("model just ensured Some");
                    serve_query_embedding_request(m, request);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if slot.take().is_some() {
                        info!(
                            "Semantic Query Worker [{}]: idle {}s — dropping BGE-Large model (host RAM reclaimed)",
                            worker_idx,
                            idle_timeout.map(|d| d.as_secs()).unwrap_or(0)
                        );
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
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
        // REQ-AXO-901737 : gpu_present read from in-process diagnostics
        // struct instead of AXON_EMBEDDING_GPU_PRESENT env var.
        let cuda_available = provider_runtime::current_gpu_present();
        let cuda_provider_library_available = ort_cuda_provider_library_available();
        if cuda_requested && cuda_available && !cuda_provider_library_available {
            let provider_path = ort_cuda_provider_library_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            error!(
                "❌ Semantic {} Worker [{}]: CUDA requested but ONNX Runtime provider library is missing: {}",
                lane, worker_idx, provider_path
            );
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

        // DEC-AXO-901626 : the effective provider is no longer written to a
        // shared slot (the race that this decision removes). The label below
        // is local — used only for this worker's init log line. The
        // canonical, observable effective provider is derived on read from
        // the OS (nvidia-smi) by `current_embedding_provider_diagnostics`.
        let (model_result, provider_label) = if let Some(cuda_options) = cuda_options {
            match TextEmbedding::try_new(cuda_options) {
                Ok(model) => (Ok(model), "cuda"),
                Err(err) => {
                    error!(
                        "❌ Semantic {} Worker [{}]: CUDA init failed, falling back to CPU: {:?}",
                        lane, worker_idx, err
                    );
                    apply_cpu_fallback_ort_runtime_env();
                    (TextEmbedding::try_new(options), "cpu_fallback")
                }
            }
        } else {
            (
                TextEmbedding::try_new(options),
                cpu_provider_effective_label(
                    cuda_requested,
                    cuda_available,
                    cuda_provider_library_available,
                ),
            )
        };

        match model_result {
            Ok(model) => {
                info!(
                    "✅ Semantic {} Worker [{}]: BGE-Large model loaded successfully (provider={}).",
                    lane, worker_idx, provider_label
                );
                // REQ-AXO-901979 — publish THIS query worker's effective compute
                // so status/embedding_status report it accurately in brain_only
                // (where no indexer nvidia-smi heartbeat exists). "cuda" = GPU.
                if lane == "query" {
                    set_query_worker_compute_gpu(provider_label == "cuda");
                }
                Some(model)
            }
            Err(e) => {
                error!(
                    "❌ Semantic {} Worker [{}]: FATAL ONNX INIT ERROR: {:?}",
                    lane, worker_idx, e
                );
                None
            }
        }
    }
}

/// REQ-AXO-901945 — idle window after which the in-process CPU query
/// model is dropped to reclaim host RAM. `AXON_QUERY_EMBED_IDLE_SECS`
/// overrides the default of 1200 s (20 min). A value of `0` disables
/// idle-drop entirely (model stays resident for the worker's lifetime),
/// for operators who prefer guaranteed zero reload latency over RAM.
fn query_embed_idle_timeout() -> Option<Duration> {
    parse_query_embed_idle_timeout(std::env::var("AXON_QUERY_EMBED_IDLE_SECS").ok())
}

/// Pure parser for [`query_embed_idle_timeout`] (env-free, unit-testable).
/// `None`/garbage → default 1200 s ; `"0"` → disabled (`None`).
fn parse_query_embed_idle_timeout(raw: Option<String>) -> Option<Duration> {
    const DEFAULT_QUERY_EMBED_IDLE_SECS: u64 = 1200;
    let secs = raw
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_QUERY_EMBED_IDLE_SECS);
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    }
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

pub(super) fn env_usize(name: &str, default: usize) -> usize {
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
        // REQ-AXO-271 slice 2e (PG canonical, post-MIL-AXO-017) :
        // GraphEmbedding.embedding is `vector(N)` (pgvector) ; render
        // as a `'[…]'::vector(N)` text literal. The DuckDB
        // `CAST(... AS FLOAT[N])` arm is dead syntax.
        let values: Vec<String> = updates
            .iter()
            .map(
                |(anchor_type, anchor_id, radius, source_signature, projection_version, vector)| {
                    let embedding_lit = match crate::postgres::vector::vector_literal(vector) {
                        Ok(lit) => lit,
                        Err(e) => {
                            log::warn!(
                                "skipping GraphEmbedding upsert for {}/{}: {}",
                                anchor_type,
                                anchor_id,
                                e
                            );
                            "NULL".to_string()
                        }
                    };
                    format!(
                        "('{}', '{}', {}, '{}', '{}', '{}', {}, {})",
                        Self::escape_embedding_sql(anchor_type),
                        Self::escape_embedding_sql(anchor_id),
                        radius,
                        Self::escape_embedding_sql(model_id),
                        Self::escape_embedding_sql(source_signature),
                        Self::escape_embedding_sql(projection_version),
                        embedding_lit,
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

    // REQ-AXO-901978 (B3) — serve from the query-vector cache first ; only the
    // cache MISSES are embedded. Query embedding is the dominant MCP latency
    // (CPU BGE-large), and re-asks / retries / multi-tool flows repeat the same
    // question — those now skip the embed entirely. Keyed by the RAW text (the
    // BGE prefix is deterministic).
    let mut results: Vec<Option<Vec<f32>>> =
        texts.iter().map(|t| query_vec_cache_get(t)).collect();
    let miss_indices: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_none())
        .map(|(i, _)| i)
        .collect();
    if miss_indices.is_empty() {
        return Ok(results.into_iter().map(|r| r.unwrap_or_default()).collect());
    }

    let pressure = service_guard::current_pressure();
    if !query_embedding_allowed(pressure) {
        return Err(anyhow::anyhow!(
            "MCP real-time embedding paused under {:?} service pressure. Use structural search.",
            pressure
        ));
    }
    // BGE-Large-v1.5 query prefix for optimal retrieval quality (miss texts only).
    let prefixed: Vec<String> = miss_indices
        .iter()
        .map(|&i| {
            format!(
                "Represent this sentence for searching relevant passages: {}",
                texts[i]
            )
        })
        .collect();

    // REQ-AXO-128 — under brain_only / indexer_graph the registered
    // sender belongs to the in-process query worker spawned at boot
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

    let embedded = request_query_embedding(&sender, prefixed)?;
    for (k, &idx) in miss_indices.iter().enumerate() {
        if let Some(vector) = embedded.get(k) {
            query_vec_cache_put(texts[idx].clone(), vector.clone());
            results[idx] = Some(vector.clone());
        }
    }
    results
        .into_iter()
        .map(|r| r.ok_or_else(|| anyhow::anyhow!("query embedding missing for a requested text")))
        .collect()
}

/// REQ-AXO-901978 (B3) — bounded query→vector cache. Capacity bounds RAM
/// (512 × 1024 f32 ≈ 2 MB) ; FIFO eviction at capacity. The embedding model is
/// pinned for the process lifetime, so no invalidation is needed.
const QUERY_VEC_CACHE_CAP: usize = 512;
static QUERY_VEC_CACHE: OnceLock<Mutex<QueryVecCache>> = OnceLock::new();

#[derive(Default)]
struct QueryVecCache {
    map: HashMap<String, Vec<f32>>,
    order: VecDeque<String>,
}

fn query_vec_cache() -> &'static Mutex<QueryVecCache> {
    QUERY_VEC_CACHE.get_or_init(|| Mutex::new(QueryVecCache::default()))
}

fn query_vec_cache_get(key: &str) -> Option<Vec<f32>> {
    query_vec_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .map
        .get(key)
        .cloned()
}

fn query_vec_cache_put(key: String, vector: Vec<f32>) {
    let mut cache = query_vec_cache().lock().unwrap_or_else(|p| p.into_inner());
    if cache.map.contains_key(&key) {
        return;
    }
    if cache.map.len() >= QUERY_VEC_CACHE_CAP {
        if let Some(evicted) = cache.order.pop_front() {
            cache.map.remove(&evicted);
        }
    }
    cache.order.push_back(key.clone());
    cache.map.insert(key, vector);
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
    let (embeddings, tokenize_ms, host_prepare_ms, input_copy_ms, inference_ms, output_extract_ms) =
        embed_texts_with_breakdown_ort(&mut model, &texts)?;
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
        let res = h.join().map_err(|payload| {
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

// =========================================================================
// REQ-AXO-257 — Sustained throughput bench (reconstructed from VAL-AXO-050
// proto/gpu-saturation-probe worktree, lost before commit).
// =========================================================================

/// REQ-AXO-257 — Walk the per-iteration `(start, chunks, ms)` log,
/// computing aggregate ch/s for every rolling `window` ending at each
/// observation, and return the minimum. None when `observations` is
/// empty. Pure function — extracted from sustained + pipeline benches
/// (GUI-PRO-013 DRY) so the algorithm is unit-testable without ORT.
pub(crate) fn rolling_window_min_ch_per_s(
    observations: &[(std::time::Instant, usize, u64)],
    window: std::time::Duration,
) -> Option<f64> {
    if observations.is_empty() {
        return None;
    }
    let mut min: f64 = f64::INFINITY;
    for (anchor_idx, (anchor_t, _, _)) in observations.iter().enumerate() {
        let window_start = anchor_t.checked_sub(window);
        let mut chunks_in_window: usize = 0;
        let mut window_ms: u64 = 0;
        for (back_idx, (t, n, ms)) in observations.iter().enumerate().rev() {
            if back_idx > anchor_idx {
                continue;
            }
            let in_window = match window_start {
                Some(ws) => *t >= ws,
                None => true,
            };
            if !in_window {
                break;
            }
            chunks_in_window += *n;
            window_ms = window_ms.saturating_add(*ms);
        }
        if window_ms == 0 {
            continue;
        }
        let window_chs = (chunks_in_window as f64) * 1000.0 / (window_ms as f64);
        if window_chs < min {
            min = window_chs;
        }
    }
    if min.is_finite() {
        Some(min)
    } else {
        None
    }
}

/// REQ-AXO-257 — Outcome of a sustained-window throughput run. Captures
/// mean and rolling-10s minimum so dips during VRAM recycle / TRT JIT
/// stalls do not get smoothed away by averaging.
#[derive(Debug, Clone)]
pub struct EmbeddingSustainedBench {
    pub label: String,
    pub batch_size: usize,
    pub warmup_secs: u64,
    pub sustained_secs: u64,
    pub total_chunks: usize,
    pub mean_ch_per_s: f64,
    pub rolling_10s_min: f64,
    pub p50_ch_per_s: f64,
    pub p95_ch_per_s: f64,
    /// Per-iteration ch/s series — kept for CSV export and debugging
    /// of variance hypotheses. Length = number of sustained iterations.
    pub iter_ch_per_s: Vec<f64>,
    pub embedding_dim: usize,
    /// REQ-AXO-257 / VAL-AXO-053 follow-up — mean wall time between
    /// consecutive iter_start anchors, minus the iter_ms itself.
    /// Surfaces the dispatch / marshalling / pre-batch overhead that
    /// dominates wall-time when sustained_mean << peak iter ch/s.
    /// Computed only when sustained iterations >= 2.
    pub mean_inter_iter_gap_ms: f64,
    /// REQ-AXO-257 — mean iter_ms across sustained iterations (the
    /// embed call itself, excludes prep_batch + dispatch overhead).
    pub mean_iter_ms: f64,
}

/// REQ-AXO-257 — Sustained sweep entry point. Loads the model once, runs
/// `warmup_secs` of throwaway iterations to settle TRT JIT + VRAM
/// allocator, then loops `sustained_secs` worth of fixed-`batch_size`
/// embed cycles drawn from `source_pool` (cycled with idx-prepended
/// uniqueness so the tokenizer cannot cache hits).
///
/// `source_pool` SHOULD be at least `batch_size` long; if shorter the
/// pool is repeated. Each iteration measures wall time around
/// `embed_texts_with_breakdown_ort` to compute its ch/s contribution.
pub fn run_embedder_sustained_bench(
    label: &str,
    source_pool: Vec<String>,
    batch_size: usize,
    warmup_secs: u64,
    sustained_secs: u64,
    force_gpu: bool,
) -> anyhow::Result<EmbeddingSustainedBench> {
    if source_pool.is_empty() {
        anyhow::bail!("sustained bench requires at least one source text");
    }
    if batch_size == 0 {
        anyhow::bail!("sustained bench requires batch_size >= 1");
    }
    if sustained_secs == 0 {
        anyhow::bail!("sustained bench requires sustained_secs >= 1");
    }

    let mut model = OrtGpuFirstTextEmbedding::try_new(label, 0, force_gpu)?;
    sustained_bench_with_loaded_model(
        label,
        &mut model,
        &source_pool,
        batch_size,
        warmup_secs,
        sustained_secs,
        0,
    )
}

/// REQ-AXO-257 / operator 2026-05-10 — Sweep variant that **loads the
/// model once** and runs sustained measurements at each batch size in
/// `batch_sizes`, sequentially. Saves the ~1-min process-boot model-load
/// cost across N hypotheses (e.g. {128, 160, 180, 220} = ~3-5 min saved
/// vs separate runs).
///
/// TensorRT engine recompile per shape is intrinsic to the EP and is
/// not avoided here — but warmup_secs is still honoured per-batch so
/// the per-shape compile cost is absorbed before measurement.
///
/// `cycle_idx` is carried across batches so identical sustained loops
/// don't tokenize the same text twice (defeats tokenizer cache).
pub fn run_embedder_sustained_sweep(
    label: &str,
    source_pool: Vec<String>,
    batch_sizes: &[usize],
    warmup_secs: u64,
    sustained_secs: u64,
    force_gpu: bool,
) -> anyhow::Result<Vec<EmbeddingSustainedBench>> {
    run_embedder_sustained_sweep_aligned(
        label,
        source_pool,
        batch_sizes,
        warmup_secs,
        sustained_secs,
        force_gpu,
        true, // REQ-AXO-262 / operator 2026-05-10 — align micro-batch by default.
    )
}

/// REQ-AXO-262 / operator 2026-05-10 — Sweep variant with explicit
/// control over micro-batch alignment.
///
/// **Why this matters** : the legacy default of
/// `AXON_EMBED_MICRO_BATCH_MAX_ITEMS = chunk_batch_size` (8-32) means
/// that an external batch of 128 gets resliced into 4-16 micro-batches
/// before reaching the embedder. Each micro-batch has its own
/// padded seq_len, which triggers TensorRT engine recompile or
/// kernel re-tuning per shape. The headline "batch=128 throughput"
/// metric was therefore measuring ~8-32 chunks per actual GPU call,
/// not 128. Forcing alignment lets us compare batch sizes meaningfully.
///
/// `align_microbatch=true` (default) sets
/// `embed_micro_batch_max_items = batch_size` and
/// `embed_micro_batch_max_total_tokens = batch_size * max_length`
/// for each sweep step, then calls
/// `runtime_tuning::reset_runtime_tuning_snapshot` so the embedder
/// sees the new values. Restores the prior snapshot on exit.
pub fn run_embedder_sustained_sweep_aligned(
    label: &str,
    source_pool: Vec<String>,
    batch_sizes: &[usize],
    warmup_secs: u64,
    sustained_secs: u64,
    force_gpu: bool,
    align_microbatch: bool,
) -> anyhow::Result<Vec<EmbeddingSustainedBench>> {
    if source_pool.is_empty() {
        anyhow::bail!("sweep bench requires at least one source text");
    }
    if batch_sizes.is_empty() {
        anyhow::bail!("sweep bench requires at least one batch size");
    }
    for (i, &b) in batch_sizes.iter().enumerate() {
        if b == 0 {
            anyhow::bail!("sweep batch_sizes[{i}] must be >= 1");
        }
    }
    if sustained_secs == 0 {
        anyhow::bail!("sweep sustained_secs must be >= 1");
    }

    let mut model = OrtGpuFirstTextEmbedding::try_new(label, 0, force_gpu)?;
    let mut results = Vec::with_capacity(batch_sizes.len());
    let mut cycle_idx_carry: usize = 0;

    let saved_microbatch_items = if align_microbatch {
        Some(
            current_runtime_tuning_snapshot()
                .state
                .embed_micro_batch_max_items,
        )
    } else {
        None
    };
    let saved_microbatch_tokens = if align_microbatch {
        Some(
            current_runtime_tuning_snapshot()
                .state
                .embed_micro_batch_max_total_tokens,
        )
    } else {
        None
    };

    for &batch_size in batch_sizes {
        if align_microbatch {
            let max_length = configured_embedding_max_length();
            let target_total_tokens = batch_size.saturating_mul(max_length).max(max_length);
            crate::runtime_tuning::update_runtime_tuning_state(
                bootstrap_runtime_tuning_state_from_env(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(batch_size),
                Some(target_total_tokens),
                None,
                None,
            );
        }

        let one = sustained_bench_with_loaded_model(
            label,
            &mut model,
            &source_pool,
            batch_size,
            warmup_secs,
            sustained_secs,
            cycle_idx_carry,
        )?;
        // Estimate cycle advance: warmup iterations ~ unknown, but
        // sustained iterations = total_chunks. Carry the running
        // counter forward so subsequent batches don't repeat the
        // same texts.
        cycle_idx_carry = cycle_idx_carry.saturating_add(one.total_chunks);
        results.push(one);
    }

    // Restore the prior tuning values so we don't leak the override
    // into other consumers of the singleton.
    if let (Some(items), Some(tokens)) = (saved_microbatch_items, saved_microbatch_tokens) {
        crate::runtime_tuning::update_runtime_tuning_state(
            bootstrap_runtime_tuning_state_from_env(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(items),
            Some(tokens),
            None,
            None,
        );
    }

    Ok(results)
}

/// REQ-AXO-257 / operator 2026-05-10 — Internal helper: run one
/// sustained measurement against a model that's already loaded.
/// Pulled out of `run_embedder_sustained_bench` so the sweep variant
/// can amortize the load cost.
fn sustained_bench_with_loaded_model(
    label: &str,
    model: &mut OrtGpuFirstTextEmbedding,
    source_pool: &[String],
    batch_size: usize,
    warmup_secs: u64,
    sustained_secs: u64,
    cycle_idx_start: usize,
) -> anyhow::Result<EmbeddingSustainedBench> {
    if source_pool.is_empty() {
        anyhow::bail!("sustained bench requires at least one source text");
    }
    if batch_size == 0 {
        anyhow::bail!("sustained bench requires batch_size >= 1");
    }
    if sustained_secs == 0 {
        anyhow::bail!("sustained bench requires sustained_secs >= 1");
    }

    let mut cycle_idx: usize = cycle_idx_start;
    let mut prep_batch = |size: usize| -> Vec<String> {
        let mut out = Vec::with_capacity(size);
        for _ in 0..size {
            let base = &source_pool[cycle_idx % source_pool.len()];
            out.push(format!("// iter {cycle_idx}\n{base}"));
            cycle_idx += 1;
        }
        out
    };

    // Warmup — discard timings (per-shape TRT compile absorption).
    if warmup_secs > 0 {
        let warmup_until = std::time::Instant::now() + std::time::Duration::from_secs(warmup_secs);
        while std::time::Instant::now() < warmup_until {
            let texts = prep_batch(batch_size);
            let _ = embed_texts_with_breakdown_ort(model, &texts)?;
        }
    }

    // Sustained — measure each iteration.
    let sustained_until =
        std::time::Instant::now() + std::time::Duration::from_secs(sustained_secs);
    let mut iter_observations: Vec<(std::time::Instant, usize, u64)> = Vec::new();
    let mut total_chunks: usize = 0;
    let mut embedding_dim: usize = 0;

    while std::time::Instant::now() < sustained_until {
        let texts = prep_batch(batch_size);
        let iter_start = std::time::Instant::now();
        let (embeddings, _t, _hp, _ic, _inf, _oe) = embed_texts_with_breakdown_ort(model, &texts)?;
        let iter_ms = iter_start.elapsed().as_millis() as u64;
        if embedding_dim == 0 {
            embedding_dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
        }
        total_chunks += batch_size;
        iter_observations.push((iter_start, batch_size, iter_ms.max(1)));
    }

    if iter_observations.is_empty() {
        anyhow::bail!("sustained loop produced zero iterations — sustained_secs too small?");
    }

    let mut iter_ch_per_s: Vec<f64> = iter_observations
        .iter()
        .map(|(_, n, ms)| (*n as f64) * 1000.0 / (*ms as f64))
        .collect();

    let actual_sustained_secs = sustained_secs as f64;
    let mean_ch_per_s = (total_chunks as f64) / actual_sustained_secs;

    let rolling_min =
        rolling_window_min_ch_per_s(&iter_observations, std::time::Duration::from_secs(10))
            .unwrap_or(mean_ch_per_s);

    // p50/p95 from per-iteration chunks/sec.
    let mut sorted = iter_ch_per_s.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct_at = |percentile: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * percentile).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    };
    let p50 = pct_at(0.5);
    let p95 = pct_at(0.95);

    iter_ch_per_s.shrink_to_fit();

    // REQ-AXO-257 / VAL-AXO-053 — inter-iteration gap analysis.
    // For consecutive (start_i, start_{i+1}) anchors, gap = (start_{i+1} - start_i) - iter_ms_i.
    let mut gaps_ms: Vec<u64> = Vec::with_capacity(iter_observations.len());
    for w in iter_observations.windows(2) {
        let prev = &w[0];
        let next = &w[1];
        let delta = next.0.duration_since(prev.0).as_millis() as u64;
        let gap = delta.saturating_sub(prev.2);
        gaps_ms.push(gap);
    }
    let mean_inter_iter_gap_ms = if gaps_ms.is_empty() {
        0.0
    } else {
        let sum: u64 = gaps_ms.iter().sum();
        (sum as f64) / (gaps_ms.len() as f64)
    };
    let total_iter_ms: u64 = iter_observations.iter().map(|(_, _, ms)| *ms).sum();
    let mean_iter_ms = if iter_observations.is_empty() {
        0.0
    } else {
        (total_iter_ms as f64) / (iter_observations.len() as f64)
    };

    Ok(EmbeddingSustainedBench {
        label: label.to_string(),
        batch_size,
        warmup_secs,
        sustained_secs,
        total_chunks,
        mean_ch_per_s,
        rolling_10s_min: rolling_min,
        p50_ch_per_s: p50,
        p95_ch_per_s: p95,
        iter_ch_per_s,
        embedding_dim,
        mean_inter_iter_gap_ms,
        mean_iter_ms,
    })
}

/// REQ-AXO-257 — Pipeline bench result. Adds `queue_high_water` to
/// surface whether N producers are saturating the GPU consumer or
/// merely keeping pace.
#[derive(Debug, Clone)]
pub struct PipelineBench {
    pub label: String,
    pub batch_size: usize,
    pub producers: usize,
    pub channel_capacity: usize,
    pub warmup_secs: u64,
    pub sustained_secs: u64,
    pub total_chunks: usize,
    pub mean_ch_per_s: f64,
    pub rolling_10s_min: f64,
    pub queue_high_water: usize,
    pub embedding_dim: usize,
}

/// REQ-AXO-257 — N-producer / bounded-channel / single-consumer pipeline
/// bench. Producers cycle through `source_pool` slices, push batches of
/// `batch_size` into a `crossbeam-channel::bounded(channel_capacity)`,
/// the single consumer thread runs the embedder and counts chunks.
///
/// `queue_high_water` = max channel.len() observed by the consumer
/// after each receive. It tells you whether producers can outrun the
/// GPU (>1 = yes; 1 = exactly keeping pace; 0 = consumer waited).
pub fn run_embedder_pipeline_bench(
    label: &str,
    source_pool: Vec<String>,
    producers: usize,
    channel_capacity: usize,
    batch_size: usize,
    warmup_secs: u64,
    sustained_secs: u64,
    force_gpu: bool,
) -> anyhow::Result<PipelineBench> {
    if source_pool.is_empty() {
        anyhow::bail!("pipeline bench requires at least one source text");
    }
    if batch_size == 0 {
        anyhow::bail!("pipeline bench requires batch_size >= 1");
    }
    if sustained_secs == 0 {
        anyhow::bail!("pipeline bench requires sustained_secs >= 1");
    }
    if producers == 0 {
        anyhow::bail!("pipeline bench requires producers >= 1");
    }
    if channel_capacity == 0 {
        anyhow::bail!("pipeline bench requires channel_capacity >= 1");
    }

    use crossbeam_channel::bounded;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    let stop = Arc::new(AtomicBool::new(false));
    let queue_high_water = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = bounded::<Vec<String>>(channel_capacity);

    let pool = Arc::new(source_pool);
    let mut producer_handles = Vec::with_capacity(producers);

    for prod_idx in 0..producers {
        let stop_p = stop.clone();
        let pool_p = pool.clone();
        let tx_p = tx.clone();
        let label_p = label.to_string();
        producer_handles.push(std::thread::spawn(move || {
            let mut cycle: usize = prod_idx;
            while !stop_p.load(Ordering::Relaxed) {
                let mut batch = Vec::with_capacity(batch_size);
                for _ in 0..batch_size {
                    let base = &pool_p[cycle % pool_p.len()];
                    batch.push(format!("// p{prod_idx} iter {cycle}\n{base}"));
                    cycle = cycle.wrapping_add(producers);
                }
                if tx_p.send(batch).is_err() {
                    break;
                }
            }
            let _ = label_p;
        }));
    }
    drop(tx);

    let mut model = OrtGpuFirstTextEmbedding::try_new(label, 0, force_gpu)?;

    // Warmup — discard timings.
    if warmup_secs > 0 {
        let warmup_until = std::time::Instant::now() + std::time::Duration::from_secs(warmup_secs);
        while std::time::Instant::now() < warmup_until {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(texts) => {
                    let len_after_recv = rx.len();
                    let prev = queue_high_water.load(Ordering::Relaxed);
                    if len_after_recv > prev {
                        queue_high_water.store(len_after_recv, Ordering::Relaxed);
                    }
                    let _ = embed_texts_with_breakdown_ort(&mut model, &texts)?;
                }
                Err(_) => continue,
            }
        }
    }

    // Sustained — measure each iteration.
    let sustained_until =
        std::time::Instant::now() + std::time::Duration::from_secs(sustained_secs);
    let mut iter_observations: Vec<(std::time::Instant, usize, u64)> = Vec::new();
    let mut total_chunks: usize = 0;
    let mut embedding_dim: usize = 0;

    while std::time::Instant::now() < sustained_until {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(texts) => {
                let len_after_recv = rx.len();
                let prev = queue_high_water.load(Ordering::Relaxed);
                if len_after_recv > prev {
                    queue_high_water.store(len_after_recv, Ordering::Relaxed);
                }
                let iter_start = std::time::Instant::now();
                let n = texts.len();
                let (embeddings, _t, _hp, _ic, _inf, _oe) =
                    embed_texts_with_breakdown_ort(&mut model, &texts)?;
                let iter_ms = iter_start.elapsed().as_millis() as u64;
                if embedding_dim == 0 {
                    embedding_dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
                }
                total_chunks += n;
                iter_observations.push((iter_start, n, iter_ms.max(1)));
            }
            Err(_) => continue,
        }
    }

    stop.store(true, Ordering::Relaxed);
    drop(rx);
    for h in producer_handles {
        let _ = h.join();
    }

    if iter_observations.is_empty() {
        anyhow::bail!("pipeline sustained loop produced zero iterations");
    }

    let actual_sustained_secs = sustained_secs as f64;
    let mean_ch_per_s = (total_chunks as f64) / actual_sustained_secs;

    let rolling_min =
        rolling_window_min_ch_per_s(&iter_observations, std::time::Duration::from_secs(10))
            .unwrap_or(mean_ch_per_s);

    Ok(PipelineBench {
        label: label.to_string(),
        batch_size,
        producers,
        channel_capacity,
        warmup_secs,
        sustained_secs,
        total_chunks,
        mean_ch_per_s,
        rolling_10s_min: rolling_min,
        queue_high_water: queue_high_water.load(Ordering::Relaxed),
        embedding_dim,
    })
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
    use super::{
        build_token_aware_micro_batches, configured_embedding_max_length,
        cuda_execution_provider_dispatch, current_runtime_tuning_snapshot,
        current_runtime_tuning_state, embedding_lane_config_from_env, embedding_model_cache_dir,
        gpu_memory_soft_limit_mb, query_embedding_allowed, request_query_embedding,
        EmbeddingLaneConfig, QueryEmbeddingRequest,
    };
    use crate::embedding_contract::{fastembed_model, MAX_LENGTH};
    use crate::service_guard::{ServicePressure, VectorRuntimeMetrics};
    use crate::vector_control::{
        allowed_gpu_vector_workers, current_vector_batch_controller_diagnostics,
        current_vector_drain_state, graph_projection_allowed,
        reset_utility_first_scheduler_for_tests, semantic_policy, vector_claim_target,
        vector_embed_target_chunks, vector_ready_reserve_target, VectorBatchController,
        VectorBatchControllerObservation, VectorBatchControllerState, VectorDrainState,
        AGGRESSIVE_DRAIN_FILE_BACKLOG_THRESHOLD, CPU_ONLY_VECTOR_BACKLOG_YIELD_THRESHOLD,
    };
    use crossbeam_channel::unbounded;
    use fastembed::{InitOptions, TextEmbedding};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_TEST_GUARD: Mutex<()> = Mutex::new(());

    fn lock_env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    // REQ-AXO-291 — cross-module test serialization for tests that
    // mutate `service_guard` global atomics (record_vector_*,
    // reset_for_tests). The local `ENV_TEST_GUARD` only serializes
    // env-var-affecting tests within this mod ; the crate-level
    // `service_guard::lock_for_tests` synchronises across modules.
    fn lock_service_guard() -> parking_lot::MutexGuard<'static, ()> {
        crate::service_guard::lock_for_tests()
    }

    // REQ-AXO-901978 (B3) — query→vector cache: hit, miss, idempotent put.
    #[test]
    fn query_vec_cache_stores_hit_miss_and_is_idempotent() {
        super::query_vec_cache_put("rc-cache-key-a".to_string(), vec![1.0, 2.0]);
        assert_eq!(
            super::query_vec_cache_get("rc-cache-key-a"),
            Some(vec![1.0, 2.0])
        );
        assert_eq!(super::query_vec_cache_get("rc-cache-key-absent"), None);
        // Re-put with the same key must NOT overwrite (contains_key early return).
        super::query_vec_cache_put("rc-cache-key-a".to_string(), vec![9.0]);
        assert_eq!(
            super::query_vec_cache_get("rc-cache-key-a"),
            Some(vec![1.0, 2.0])
        );
    }

    // REQ-AXO-901979 — query worker self-reported compute label (GPU/CPU truth).
    #[test]
    fn query_worker_compute_label_reflects_provider() {
        super::set_query_worker_compute_gpu(true);
        assert_eq!(super::query_worker_compute_label(), Some("GPU"));
        super::set_query_worker_compute_gpu(false);
        assert_eq!(super::query_worker_compute_label(), Some("CPU"));
    }

    // REQ-AXO-901984 — runtime query-embed provider override: set/get + reload bump.
    #[test]
    fn query_embed_provider_override_set_get_and_bumps_reload() {
        let gen0 = super::query_reload_generation();
        assert_eq!(super::set_query_embed_provider_override("cpu"), Ok("cpu"));
        assert_eq!(super::query_embed_provider_override_label(), "cpu");
        assert!(super::query_reload_generation() > gen0);
        assert_eq!(super::set_query_embed_provider_override("GPU"), Ok("gpu"));
        assert_eq!(super::query_embed_provider_override_label(), "gpu");
        assert_eq!(super::set_query_embed_provider_override("auto"), Ok("auto"));
        assert!(super::set_query_embed_provider_override("bogus").is_err());
    }

    #[test]
    fn query_embed_idle_timeout_parses_default_disable_and_override() {
        // REQ-AXO-901945 — default 20 min when unset / unparseable.
        assert_eq!(
            super::parse_query_embed_idle_timeout(None),
            Some(std::time::Duration::from_secs(1200))
        );
        assert_eq!(
            super::parse_query_embed_idle_timeout(Some("garbage".into())),
            Some(std::time::Duration::from_secs(1200))
        );
        // 0 disables idle-drop (model stays resident).
        assert_eq!(
            super::parse_query_embed_idle_timeout(Some("0".into())),
            None
        );
        // Explicit override honoured (with whitespace tolerance).
        assert_eq!(
            super::parse_query_embed_idle_timeout(Some("  300 ".into())),
            Some(std::time::Duration::from_secs(300))
        );
    }

    #[test]
    fn test_semantic_policy_runs_when_system_is_healthy() {
        let _guard = lock_env_guard();
        let _guard_sg = lock_service_guard();
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
        let _guard_sg = lock_service_guard();
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
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let _guard = lock_env_guard();
        let _guard_sg = lock_service_guard();
        crate::service_guard::reset_for_tests();
        reset_utility_first_scheduler_for_tests();
        let policy = semantic_policy(100, ServicePressure::Critical);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(2));
    }

    #[test]
    fn test_semantic_policy_throttles_without_pausing_when_service_is_degraded() {
        let _guard = lock_env_guard();
        let _guard_sg = lock_service_guard();
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
        let _guard_sg = lock_service_guard();
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
        let _guard_sg = lock_service_guard();
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
        let _guard_sg = lock_service_guard();
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
    fn test_effective_provider_request_for_query_lane_uses_canonical_provider_not_forced_cpu() {
        // ec917e64 — the brain query lane no longer forces CPU when the
        // global provider is CUDA; both ORT sessions cohabit via CUDA
        // time-sharing, so the query lane returns the canonical provider
        // for the mode (like the bulk lanes). The old assertion (== "cpu")
        // encoded the removed legacy safeguard and failed deterministically.
        // Asserting equality with the canonical request keeps this
        // hardware-independent (RuntimeProfile::detect() varies by host).
        let _guard = lock_env_guard();
        unsafe {
            std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
            std::env::remove_var("AXON_QUERY_EMBED_PROVIDER");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_EFFECTIVE");
            std::env::remove_var("AXON_EMBEDDING_PROVIDER_INIT_ERROR");
        }

        let canonical = crate::runtime_mode::canonical_embedding_provider_request_for_mode(
            crate::runtime_mode::AxonRuntimeMode::from_env(),
            crate::runtime_profile::RuntimeProfile::detect().gpu_present,
        )
        .to_ascii_lowercase();
        let provider = super::effective_provider_request_for_lane("query");

        unsafe {
            std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        }

        assert_eq!(
            provider, canonical,
            "query lane must use the canonical provider, not the removed legacy CPU force (ec917e64)"
        );
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
    fn test_cuda_tf32_enabled_defaults_to_true() {
        // REQ-AXO-262 — TF32 default flipped ON 2026-05-11. Ampere+ tensor
        // cores deliver 1.5–2× speedup on matmul with negligible accuracy
        // loss for embedding inference.
        let _guard = lock_env_guard();
        unsafe {
            std::env::remove_var("AXON_CUDA_ALLOW_TF32");
        }

        assert!(super::cuda_tf32_enabled());
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
    fn test_vector_claim_target_expands_when_ready_reserve_is_missing() {
        assert_eq!(vector_claim_target(24, 0.0, 24, 0.0, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(24, 1.0, 24, 3.0, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(24, 2.2, 24, 6.6, 10, 0, 4_096), 146);
        assert_eq!(vector_claim_target(3, 10.0, 12, 30.0, 8, 6, 128), 10);
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
            controller_observation_with_runtime(
                4_096, false, false, 12, 896, 24, 138_240, 2_048, 0,
            ),
        );
        let second_regression = controller.observe(
            43_000,
            controller_observation_with_runtime(
                4_096, false, false, 16, 1_280, 32, 215_040, 2_048, 0,
            ),
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
