use crate::graph::GraphStore;
use crate::graph_ingestion::{embedding_cast_sql, FileVectorizationWork, GraphProjectionWork};
use crate::queue::QueueStore;
use crate::service_guard::{self, ServicePressure};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use fastembed::{EmbeddingModel, ExecutionProviderDispatch, InitOptions, TextEmbedding};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info};

const FILE_PROJECTION_RADIUS: i64 = 2;
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const FILE_VECTORIZATION_BATCH_SIZE: usize = 8;
const GRAPH_BATCH_SIZE: usize = 6;
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(15);
const EMBEDDING_KINDS: [&str; 3] = ["symbol", "chunk", "graph"];
const GPU_CHUNK_BATCH_SIZE: usize = 32;
const GPU_SYMBOL_BATCH_SIZE: usize = 64;
const GPU_FILE_VECTORIZATION_BATCH_SIZE: usize = 16;
const GPU_GRAPH_BATCH_SIZE: usize = 8;
const JINA_CODE_MODEL_NAME: &str = "jinaai/jina-embeddings-v2-base-code";
const JINA_CODE_MODEL_SLUG: &str = "jina-embeddings-v2-base-code";
const JINA_CODE_MODEL_VERSION: &str = "1";
const JINA_CODE_EMBEDDING_DIMENSION: usize = 768;
const BGE_BASE_MODEL_NAME: &str = "BAAI/bge-base-en-v1.5";
const BGE_BASE_MODEL_SLUG: &str = "bge-base-en-v1.5";
const BGE_BASE_MODEL_VERSION: &str = "1";
const BGE_BASE_EMBEDDING_DIMENSION: usize = 768;
const LEGACY_BGE_SMALL_MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";
const LEGACY_BGE_SMALL_MODEL_SLUG: &str = "bge-small-en-v1.5";
const LEGACY_BGE_SMALL_MODEL_VERSION: &str = "1";
const LEGACY_BGE_SMALL_EMBEDDING_DIMENSION: usize = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingProfileKey {
    JinaCodeV2Base,
    BgeBaseEnv15,
    LegacyBgeSmallEnv15,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingKindConfig {
    pub kind: &'static str,
    pub model_id: String,
    pub batch_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingExecutionBackend {
    Unspecified,
    Cpu,
    GpuCuda,
}

impl EmbeddingExecutionBackend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Cpu => "cpu",
            Self::GpuCuda => "cuda",
        }
    }

    fn execution_provider(self) -> Option<&'static str> {
        match self {
            Self::Unspecified => None,
            Self::Cpu => Some("cpu"),
            Self::GpuCuda => Some("cuda"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeEmbeddingModel {
    JinaEmbeddingsV2BaseCode,
    BGEBaseENV15,
    BGESmallENV15,
}

impl RuntimeEmbeddingModel {
    fn fastembed_model(self) -> EmbeddingModel {
        match self {
            Self::JinaEmbeddingsV2BaseCode => EmbeddingModel::JinaEmbeddingsV2BaseCode,
            Self::BGEBaseENV15 => EmbeddingModel::BGEBaseENV15,
            Self::BGESmallENV15 => EmbeddingModel::BGESmallENV15,
        }
    }

    fn startup_label(self) -> &'static str {
        match self {
            Self::JinaEmbeddingsV2BaseCode => "Jina-Code-V2-Base",
            Self::BGEBaseENV15 => "BGE-Base",
            Self::BGESmallENV15 => "BGE-Small",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfile {
    pub key: EmbeddingProfileKey,
    pub model_name: &'static str,
    pub model_slug: &'static str,
    pub model_version: &'static str,
    pub dimension: usize,
    pub runtime_model: RuntimeEmbeddingModel,
    pub symbol: EmbeddingKindConfig,
    pub chunk: EmbeddingKindConfig,
    pub graph: EmbeddingKindConfig,
    pub file_vectorization_batch_size: usize,
    pub execution_provider: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfileStack {
    pub primary: EmbeddingProfile,
    pub fallback: Option<EmbeddingProfile>,
}

fn embedding_model_id(kind: &'static str, model_slug: &'static str, dimension: usize) -> String {
    let prefix = match kind {
        "symbol" => "sym",
        other => other,
    };
    format!("{prefix}-{model_slug}-{dimension}")
}

impl EmbeddingProfile {
    pub fn new(
        key: EmbeddingProfileKey,
        model_name: &'static str,
        model_slug: &'static str,
        model_version: &'static str,
        dimension: usize,
        runtime_model: RuntimeEmbeddingModel,
        backend: EmbeddingExecutionBackend,
        chunk_batch_size: usize,
        symbol_batch_size: usize,
        file_vectorization_batch_size: usize,
        graph_batch_size: usize,
    ) -> Self {
        Self {
            key,
            model_name,
            model_slug,
            model_version,
            dimension,
            runtime_model,
            symbol: EmbeddingKindConfig {
                kind: "symbol",
                model_id: embedding_model_id("symbol", model_slug, dimension),
                batch_size: symbol_batch_size,
            },
            chunk: EmbeddingKindConfig {
                kind: "chunk",
                model_id: embedding_model_id("chunk", model_slug, dimension),
                batch_size: chunk_batch_size,
            },
            graph: EmbeddingKindConfig {
                kind: "graph",
                model_id: embedding_model_id("graph", model_slug, dimension),
                batch_size: graph_batch_size,
            },
            file_vectorization_batch_size,
            execution_provider: backend.execution_provider(),
        }
    }
}

pub fn embedding_profile_for_key(key: EmbeddingProfileKey) -> EmbeddingProfile {
    match key {
        EmbeddingProfileKey::JinaCodeV2Base => EmbeddingProfile::new(
            EmbeddingProfileKey::JinaCodeV2Base,
            JINA_CODE_MODEL_NAME,
            JINA_CODE_MODEL_SLUG,
            JINA_CODE_MODEL_VERSION,
            JINA_CODE_EMBEDDING_DIMENSION,
            RuntimeEmbeddingModel::JinaEmbeddingsV2BaseCode,
            EmbeddingExecutionBackend::Unspecified,
            CHUNK_BATCH_SIZE,
            SYMBOL_BATCH_SIZE,
            FILE_VECTORIZATION_BATCH_SIZE,
            GRAPH_BATCH_SIZE,
        ),
        EmbeddingProfileKey::BgeBaseEnv15 => EmbeddingProfile::new(
            EmbeddingProfileKey::BgeBaseEnv15,
            BGE_BASE_MODEL_NAME,
            BGE_BASE_MODEL_SLUG,
            BGE_BASE_MODEL_VERSION,
            BGE_BASE_EMBEDDING_DIMENSION,
            RuntimeEmbeddingModel::BGEBaseENV15,
            EmbeddingExecutionBackend::Unspecified,
            CHUNK_BATCH_SIZE,
            SYMBOL_BATCH_SIZE,
            FILE_VECTORIZATION_BATCH_SIZE,
            GRAPH_BATCH_SIZE,
        ),
        EmbeddingProfileKey::LegacyBgeSmallEnv15 => EmbeddingProfile::new(
            EmbeddingProfileKey::LegacyBgeSmallEnv15,
            LEGACY_BGE_SMALL_MODEL_NAME,
            LEGACY_BGE_SMALL_MODEL_SLUG,
            LEGACY_BGE_SMALL_MODEL_VERSION,
            LEGACY_BGE_SMALL_EMBEDDING_DIMENSION,
            RuntimeEmbeddingModel::BGESmallENV15,
            EmbeddingExecutionBackend::Unspecified,
            CHUNK_BATCH_SIZE,
            SYMBOL_BATCH_SIZE,
            FILE_VECTORIZATION_BATCH_SIZE,
            GRAPH_BATCH_SIZE,
        ),
    }
}

fn embedding_profile_key_from_env(value: &str) -> Option<EmbeddingProfileKey> {
    match value.trim().to_ascii_lowercase().as_str() {
        "jina" | "jina-code" | "jina-code-v2-base" => Some(EmbeddingProfileKey::JinaCodeV2Base),
        "bge-base" | "bge-base-en-v1.5" => Some(EmbeddingProfileKey::BgeBaseEnv15),
        "bge-small" | "bge-small-en-v1.5" | "legacy-bge-small" => {
            Some(EmbeddingProfileKey::LegacyBgeSmallEnv15)
        }
        _ => None,
    }
}

pub fn configured_embedding_profile_stack() -> EmbeddingProfileStack {
    let primary_key = std::env::var("AXON_EMBEDDING_PROFILE")
        .ok()
        .and_then(|value| embedding_profile_key_from_env(&value))
        .unwrap_or(EmbeddingProfileKey::JinaCodeV2Base);
    let fallback_key = std::env::var("AXON_EMBEDDING_FALLBACK_PROFILE")
        .ok()
        .and_then(|value| embedding_profile_key_from_env(&value))
        .or(Some(EmbeddingProfileKey::BgeBaseEnv15))
        .filter(|fallback| *fallback != primary_key);

    EmbeddingProfileStack {
        primary: embedding_profile_for_key(primary_key),
        fallback: fallback_key.map(embedding_profile_for_key),
    }
}

pub fn default_embedding_profile() -> EmbeddingProfile {
    configured_embedding_profile_stack().primary
}

pub fn default_embedding_execution_backend(gpu_present: bool) -> EmbeddingExecutionBackend {
    if gpu_present {
        EmbeddingExecutionBackend::GpuCuda
    } else {
        EmbeddingExecutionBackend::Cpu
    }
}

pub fn embedding_execution_backend_name(backend: EmbeddingExecutionBackend) -> &'static str {
    backend.name()
}

pub fn embedding_execution_providers(
    backend: EmbeddingExecutionBackend,
) -> Vec<ExecutionProviderDispatch> {
    match backend {
        EmbeddingExecutionBackend::Unspecified | EmbeddingExecutionBackend::Cpu => {
            vec![ort::ep::CPU::default().build()]
        }
        EmbeddingExecutionBackend::GpuCuda => vec![
            ort::ep::CUDA::default().build(),
            ort::ep::CPU::default().build(),
        ],
    }
}

pub fn default_runtime_embedding_model() -> RuntimeEmbeddingModel {
    default_embedding_profile().runtime_model
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRuntimeContract {
    pub model_name: String,
    pub symbol_model_id: String,
    pub chunk_model_id: String,
    pub graph_model_id: String,
    pub dimension: usize,
    pub chunk_batch_size: usize,
    pub symbol_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub graph_batch_size: usize,
    pub kinds: &'static [&'static str],
    pub execution_provider: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfileBenchmarkRow {
    pub profile_key: EmbeddingProfileKey,
    pub mode: &'static str,
    pub backend: EmbeddingExecutionBackend,
    pub model_name: String,
    pub dimension: usize,
    pub symbol_model_id: String,
    pub chunk_model_id: String,
    pub graph_model_id: String,
    pub chunk_batch_size: usize,
    pub symbol_batch_size: usize,
    pub file_vectorization_batch_size: usize,
    pub graph_batch_size: usize,
    pub file_fetch_limit: usize,
    pub total_chunk_budget: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileVectorizationRuntimeBudget {
    pub pause: bool,
    pub file_fetch_limit: usize,
    pub total_chunk_budget: usize,
}

pub fn calibrated_embedding_profile_for_backend(
    profile: &EmbeddingProfile,
    backend: EmbeddingExecutionBackend,
) -> EmbeddingProfile {
    if backend != EmbeddingExecutionBackend::GpuCuda {
        return profile.clone();
    }

    EmbeddingProfile::new(
        profile.key,
        profile.model_name,
        profile.model_slug,
        profile.model_version,
        profile.dimension,
        profile.runtime_model,
        backend,
        profile.chunk.batch_size.max(GPU_CHUNK_BATCH_SIZE),
        profile.symbol.batch_size.max(GPU_SYMBOL_BATCH_SIZE),
        profile
            .file_vectorization_batch_size
            .max(GPU_FILE_VECTORIZATION_BATCH_SIZE),
        profile.graph.batch_size.max(GPU_GRAPH_BATCH_SIZE),
    )
}

pub fn file_vectorization_runtime_budget(
    profile: &EmbeddingProfile,
    service_pressure: ServicePressure,
    queue_depth: usize,
) -> FileVectorizationRuntimeBudget {
    if service_pressure == ServicePressure::Critical || queue_depth >= 4_000 {
        return FileVectorizationRuntimeBudget {
            pause: true,
            file_fetch_limit: 0,
            total_chunk_budget: 0,
        };
    }

    if service_pressure == ServicePressure::Degraded || queue_depth >= 2_000 {
        return FileVectorizationRuntimeBudget {
            pause: false,
            file_fetch_limit: (profile.file_vectorization_batch_size / 2).max(2),
            total_chunk_budget: (profile.chunk.batch_size * 2).max(profile.chunk.batch_size),
        };
    }

    if service_pressure == ServicePressure::Recovering || queue_depth >= 1_000 {
        return FileVectorizationRuntimeBudget {
            pause: false,
            file_fetch_limit: (profile.file_vectorization_batch_size * 3 / 4).max(2),
            total_chunk_budget: (profile.chunk.batch_size * 3).max(profile.chunk.batch_size),
        };
    }

    FileVectorizationRuntimeBudget {
        pause: false,
        file_fetch_limit: profile.file_vectorization_batch_size,
        total_chunk_budget: profile.chunk.batch_size * 4,
    }
}

pub fn embedding_runtime_contract_for_profile(profile: &EmbeddingProfile) -> EmbeddingRuntimeContract {
    EmbeddingRuntimeContract {
        model_name: profile.model_name.to_string(),
        symbol_model_id: profile.symbol.model_id.clone(),
        chunk_model_id: profile.chunk.model_id.clone(),
        graph_model_id: profile.graph.model_id.clone(),
        dimension: profile.dimension,
        chunk_batch_size: profile.chunk.batch_size,
        symbol_batch_size: profile.symbol.batch_size,
        file_vectorization_batch_size: profile.file_vectorization_batch_size,
        graph_batch_size: profile.graph.batch_size,
        kinds: &EMBEDDING_KINDS,
        execution_provider: profile.execution_provider,
    }
}

pub fn embedding_runtime_contract() -> EmbeddingRuntimeContract {
    embedding_runtime_contract_for_profile(&default_embedding_profile())
}

fn benchmark_row_for_profile(
    key: EmbeddingProfileKey,
    backend: EmbeddingExecutionBackend,
    queue_depth: usize,
) -> EmbeddingProfileBenchmarkRow {
    let base = embedding_profile_for_key(key);
    let calibrated = calibrated_embedding_profile_for_backend(&base, backend);
    let budget =
        file_vectorization_runtime_budget(&calibrated, ServicePressure::Healthy, queue_depth);
    let contract = embedding_runtime_contract_for_profile(&calibrated);

    EmbeddingProfileBenchmarkRow {
        profile_key: key,
        mode: "proxy",
        backend,
        model_name: contract.model_name,
        dimension: contract.dimension,
        symbol_model_id: contract.symbol_model_id,
        chunk_model_id: contract.chunk_model_id,
        graph_model_id: contract.graph_model_id,
        chunk_batch_size: contract.chunk_batch_size,
        symbol_batch_size: contract.symbol_batch_size,
        file_vectorization_batch_size: contract.file_vectorization_batch_size,
        graph_batch_size: contract.graph_batch_size,
        file_fetch_limit: budget.file_fetch_limit,
        total_chunk_budget: budget.total_chunk_budget,
    }
}

pub fn embedding_profile_benchmark_matrix() -> Vec<EmbeddingProfileBenchmarkRow> {
    vec![
        benchmark_row_for_profile(
            EmbeddingProfileKey::JinaCodeV2Base,
            EmbeddingExecutionBackend::Cpu,
            120,
        ),
        benchmark_row_for_profile(
            EmbeddingProfileKey::BgeBaseEnv15,
            EmbeddingExecutionBackend::Cpu,
            120,
        ),
        benchmark_row_for_profile(
            EmbeddingProfileKey::LegacyBgeSmallEnv15,
            EmbeddingExecutionBackend::Cpu,
            120,
        ),
        benchmark_row_for_profile(
            EmbeddingProfileKey::JinaCodeV2Base,
            EmbeddingExecutionBackend::GpuCuda,
            120,
        ),
        benchmark_row_for_profile(
            EmbeddingProfileKey::BgeBaseEnv15,
            EmbeddingExecutionBackend::GpuCuda,
            120,
        ),
    ]
}

// NEXUS v10.5: Sovereign Semantic Engine
// We isolate the ONNX runtime inside a pure OS thread to prevent Tokio/jemalloc aborts.
// The model stays owned by the background worker; global state only holds a channel
// sender so synchronous MCP queries can reuse the already-loaded model safely.

struct QueryEmbeddingRequest {
    texts: Vec<String>,
    reply: Sender<anyhow::Result<Vec<Vec<f32>>>>,
}

static QUERY_EMBEDDING_SENDER: OnceLock<Mutex<Option<Sender<QueryEmbeddingRequest>>>> =
    OnceLock::new();

pub struct SemanticWorkerPool {
    _worker: thread::JoinHandle<()>,
}

impl SemanticWorkerPool {
    pub fn new(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) -> Self {
        info!("Semantic Factory: Spawning Native OS ML Worker...");
        let worker = thread::spawn(move || {
            Self::worker_loop(graph_store, queue_store);
        });
        Self { _worker: worker }
    }

    fn worker_loop(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) {
        let runtime_profile = crate::runtime_profile::RuntimeProfile::detect();
        let profile_stack = configured_embedding_profile_stack();
        let backend = default_embedding_execution_backend(runtime_profile.gpu_present);
        let mut candidate_profiles = vec![profile_stack.primary.clone()];
        if let Some(fallback) = profile_stack.fallback.clone() {
            candidate_profiles.push(fallback);
        }
        let profile_labels = candidate_profiles
            .iter()
            .map(|profile| profile.runtime_model.startup_label())
            .collect::<Vec<_>>()
            .join(" -> ");
        info!(
            "Semantic Worker: Initializing embedding profile stack [{}] in isolated thread...",
            profile_labels
        );
        info!(
            "Semantic Worker: embedding backend selected = {} (gpu_present={})",
            backend.name(),
            runtime_profile.gpu_present
        );
        let (mut model, profile) =
            match Self::load_text_embedding_with_fallback(&candidate_profiles, backend) {
                Ok(resolved) => resolved,
                Err(e) => {
                    error!("❌ Semantic Worker: FATAL ONNX INIT ERROR: {:?}", e);
                    return;
                }
            };
        let profile = calibrated_embedding_profile_for_backend(&profile, backend);

        let (query_tx, query_rx) = unbounded();
        register_query_embedding_sender(query_tx);

        if let Err(e) = graph_store.ensure_embedding_model(
            &profile.symbol.model_id,
            profile.symbol.kind,
            profile.model_name,
            profile.dimension as i64,
            profile.model_version,
        ) {
            error!(
                "Semantic Worker: failed to register symbol embedding model: {:?}",
                e
            );
        }
        if let Err(e) = graph_store.ensure_embedding_model(
            &profile.chunk.model_id,
            profile.chunk.kind,
            profile.model_name,
            profile.dimension as i64,
            profile.model_version,
        ) {
            error!(
                "Semantic Worker: failed to register chunk embedding model: {:?}",
                e
            );
        }
        if let Err(e) = graph_store.ensure_embedding_model(
            &profile.graph.model_id,
            profile.graph.kind,
            profile.model_name,
            profile.dimension as i64,
            profile.model_version,
        ) {
            error!(
                "Semantic Worker: failed to register graph embedding model: {:?}",
                e
            );
        }

        info!("Semantic Worker: Hunting for unembedded symbols...");

        loop {
            if handle_pending_query_requests(&mut model, &query_rx, 8) {
                continue;
            }

            let policy =
                semantic_policy(queue_store.common_len(), service_guard::current_pressure());
            let mut file_vectorization_backlog_active = false;
            let file_queue_depth = graph_store
                .fetch_file_vectorization_queue_counts()
                .map(|(queued, inflight)| queued + inflight)
                .unwrap_or_default();
            let file_budget = file_vectorization_runtime_budget(
                &profile,
                service_guard::current_pressure(),
                file_queue_depth,
            );
            match graph_store.fetch_pending_file_vectorization_work(file_budget.file_fetch_limit) {
                Ok(pending) if !pending.is_empty() => {
                    file_vectorization_backlog_active = true;
                    if file_budget.pause {
                        wait_for_query_request(&mut model, &query_rx, policy.sleep);
                        continue;
                    }
                    debug!(
                        "Semantic Worker: Embedding {} file vectorization jobs...",
                        pending.len()
                    );

                    let pending_paths: Vec<String> =
                        pending.iter().map(|work| work.file_path.clone()).collect();
                    let mut failed: HashMap<String, Vec<FileVectorizationWork>> = HashMap::new();

                    match graph_store.fetch_unembedded_chunks_for_files(
                        &pending_paths,
                        &profile.chunk.model_id,
                        file_budget.total_chunk_budget,
                    ) {
                        Ok(chunks) => {
                            if !chunks.is_empty() {
                                let texts: Vec<String> = chunks
                                    .iter()
                                    .map(|(_, _, content, _)| content.clone())
                                    .collect();
                                match model.embed(texts, None) {
                                    Ok(embeddings) => {
                                        let updates: Vec<(String, String, Vec<f32>)> = chunks
                                            .into_iter()
                                            .zip(embeddings.into_iter())
                                            .map(|((_, chunk_id, _, hash), emb)| {
                                                (chunk_id, hash, emb)
                                            })
                                            .collect();

                                        if let Err(err) = graph_store
                                            .update_chunk_embeddings(
                                                &profile.chunk.model_id,
                                                &updates,
                                            )
                                        {
                                            failed
                                                .entry(format!(
                                                    "failed to persist chunk embeddings: {:?}",
                                                    err
                                                ))
                                                .or_default()
                                                .extend(pending.iter().cloned());
                                        }
                                    }
                                    Err(e) => {
                                        if is_fatal_embedding_error(&e) {
                                            error!(
                                                "Semantic Worker: fatal chunk embedding error, disabling semantic worker: {:?}",
                                                e
                                            );
                                            return;
                                        }
                                        failed
                                            .entry(format!("chunk embedding failed: {:?}", e))
                                            .or_default()
                                            .extend(pending.iter().cloned());
                                    }
                                }
                            }

                            if failed.is_empty() {
                                if let Err(err) = graph_store.mark_file_vectorization_done(
                                    &pending_paths,
                                    &profile.chunk.model_id,
                                ) {
                                    failed
                                        .entry(format!(
                                            "failed to refresh file vector readiness: {:?}",
                                            err
                                        ))
                                        .or_default()
                                        .extend(pending.iter().cloned());
                                } else {
                                    let ready_paths = graph_store
                                        .fetch_vector_ready_file_paths(&pending_paths)
                                        .unwrap_or_default();
                                    let completed_works: Vec<FileVectorizationWork> = pending
                                        .iter()
                                        .filter(|work| ready_paths.contains(&work.file_path))
                                        .cloned()
                                        .collect();

                                    if let Err(err) = graph_store
                                        .mark_file_vectorization_work_done(&completed_works)
                                    {
                                        failed
                                            .entry(format!(
                                                "failed to clear file vector queue: {:?}",
                                                err
                                            ))
                                            .or_default()
                                            .extend(completed_works.iter().cloned());
                                    } else {
                                        for work in &completed_works {
                                            if let Err(err) = graph_store
                                                .enqueue_graph_projection_refresh(
                                                    "file",
                                                    &work.file_path,
                                                    FILE_PROJECTION_RADIUS,
                                                )
                                            {
                                                failed
                                                    .entry(format!(
                                                        "failed to enqueue file projection for {}: {:?}",
                                                        work.file_path, err
                                                    ))
                                                    .or_default()
                                                    .push(work.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            failed
                                .entry(format!(
                                    "failed to fetch cross-file chunk batch: {:?}",
                                    err
                                ))
                                .or_default()
                                .extend(pending.iter().cloned());
                        }
                    }

                    for (reason, works) in failed {
                        if let Err(err) =
                            graph_store.mark_file_vectorization_work_failed(&works, &reason)
                        {
                            error!(
                                "Semantic Worker: failed to persist file vector backlog failure [{}]: {:?}",
                                reason, err
                            );
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => error!("Semantic Worker: File vectorization fetch error: {:?}", e),
            }

            let mut symbol_backlog_active = false;
            match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
                Ok(symbols) if !symbols.is_empty() => {
                    symbol_backlog_active = true;
                    debug!("Semantic Worker: Embedding {} symbols...", symbols.len());

                    let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                    match model.embed(texts, None) {
                        Ok(embeddings) => {
                            let updates: Vec<(String, Vec<f32>)> = symbols
                                .into_iter()
                                .zip(embeddings.into_iter())
                                .map(|((id, _), emb)| (id, emb))
                                .collect();

                            if let Err(e) = graph_store.update_symbol_embeddings(&updates) {
                                error!("Semantic Worker: DB Write Error: {:?}", e);
                            }
                        }
                        Err(e) => {
                            if is_fatal_embedding_error(&e) {
                                error!("Semantic Worker: fatal symbol embedding error, disabling semantic worker: {:?}", e);
                                return;
                            }
                            error!("Semantic Worker: Embedding failed: {:?}", e);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Semantic Worker: DB Fetch error: {:?}", e);
                    wait_for_query_request(&mut model, &query_rx, policy.idle_sleep);
                }
            }

            if file_vectorization_backlog_active || symbol_backlog_active {
                continue;
            }

            if policy.pause || service_guard::current_pressure() != ServicePressure::Healthy {
                wait_for_query_request(&mut model, &query_rx, policy.sleep);
                continue;
            }

            match graph_store.fetch_pending_graph_projection_work(GRAPH_BATCH_SIZE) {
                Ok(pending) if !pending.is_empty() => {
                    debug!(
                        "Semantic Worker: Embedding {} graph projection jobs...",
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
                                        "Semantic Worker: symbol projection anchor gone, dropping job {}",
                                        work.anchor_id
                                    );
                                    if let Err(err) =
                                        graph_store.mark_graph_projection_work_done(&[work.clone()])
                                    {
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
                                &profile.graph.model_id,
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
                        match model.embed(texts, None) {
                            Ok(embeddings) => {
                                let updates: Vec<(String, String, i64, String, String, Vec<f32>)> =
                                    to_embed
                                        .iter()
                                        .zip(embeddings.into_iter())
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
                                    graph_store.update_graph_embeddings(
                                        &profile.graph.model_id,
                                        &updates,
                                    )
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
                                        "Semantic Worker: failed to clear done projection jobs: {:?}",
                                        err
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
                                        "Semantic Worker: fatal graph embedding error, disabling semantic worker: {:?}",
                                        err
                                    );
                                    return;
                                }
                                error!("Semantic Worker: Graph embedding failed: {:?}", err);
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
                                "Semantic Worker: failed to persist projection failure state [{}]: {:?}",
                                reason, err
                            );
                        }
                    }
                    continue;
                }
                Ok(_) => wait_for_query_request(&mut model, &query_rx, policy.idle_sleep),
                Err(err) => {
                    error!("Semantic Worker: Graph projection fetch error: {:?}", err);
                    wait_for_query_request(&mut model, &query_rx, policy.idle_sleep);
                }
            }
        }
    }

    fn load_text_embedding_with_fallback(
        candidate_profiles: &[EmbeddingProfile],
        backend: EmbeddingExecutionBackend,
    ) -> anyhow::Result<(TextEmbedding, EmbeddingProfile)> {
        let mut last_error = None;

        for profile in candidate_profiles {
            let mut options = InitOptions::new(profile.runtime_model.fastembed_model());
            options.show_download_progress = true;
            options = options.with_execution_providers(embedding_execution_providers(backend));

            match TextEmbedding::try_new(options) {
                Ok(model) => {
                    info!(
                        "✅ Semantic Worker: {} model loaded successfully.",
                        profile.runtime_model.startup_label()
                    );
                    return Ok((model, profile.clone()));
                }
                Err(err) => {
                    error!(
                        "Semantic Worker: failed to initialize {} ({}d): {:?}",
                        profile.runtime_model.startup_label(),
                        profile.dimension,
                        err
                    );
                    last_error = Some(err);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("no embedding profile candidates configured")))
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

fn current_query_embedding_sender() -> Option<Sender<QueryEmbeddingRequest>> {
    query_embedding_sender_slot()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

fn wait_for_query_request(
    model: &mut TextEmbedding,
    query_rx: &Receiver<QueryEmbeddingRequest>,
    timeout: Duration,
) {
    match query_rx.recv_timeout(timeout) {
        Ok(request) => serve_query_embedding_request(model, request),
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => {}
    }
}

fn handle_pending_query_requests(
    model: &mut TextEmbedding,
    query_rx: &Receiver<QueryEmbeddingRequest>,
    limit: usize,
) -> bool {
    let mut handled = 0;

    while handled < limit {
        match query_rx.try_recv() {
            Ok(request) => {
                serve_query_embedding_request(model, request);
                handled += 1;
            }
            Err(_) => break,
        }
    }

    handled > 0
}

fn serve_query_embedding_request(model: &mut TextEmbedding, request: QueryEmbeddingRequest) {
    let result = model
        .embed(request.texts, None)
        .map_err(anyhow::Error::from);
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
                        "('{}', '{}', {}, '{}', '{}', '{}', {}, {})",
                        Self::escape_embedding_sql(anchor_type),
                        Self::escape_embedding_sql(anchor_id),
                        radius,
                        Self::escape_embedding_sql(model_id),
                        Self::escape_embedding_sql(source_signature),
                        Self::escape_embedding_sql(projection_version),
                        embedding_cast_sql(vector, default_embedding_profile().dimension),
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

#[derive(Debug, Clone, Copy)]
struct SemanticPolicy {
    pause: bool,
    sleep: Duration,
    idle_sleep: Duration,
}

fn query_embedding_allowed(service_pressure: ServicePressure) -> bool {
    matches!(
        service_pressure,
        ServicePressure::Healthy | ServicePressure::Recovering
    )
}

fn semantic_policy(queue_len: usize, service_pressure: ServicePressure) -> SemanticPolicy {
    if service_pressure == ServicePressure::Critical || queue_len >= 3_000 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(10),
            idle_sleep: Duration::from_secs(10),
        };
    }

    if service_pressure == ServicePressure::Degraded || queue_len >= 1_500 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(3),
            idle_sleep: Duration::from_secs(5),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(2),
            idle_sleep: Duration::from_secs(3),
        };
    }

    SemanticPolicy {
        pause: false,
        sleep: Duration::from_secs(1),
        idle_sleep: Duration::from_secs(5),
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
    use super::{
        is_fatal_embedding_error, query_embedding_allowed, request_query_embedding,
        semantic_policy, QueryEmbeddingRequest,
    };
    use crate::service_guard::ServicePressure;
    use crossbeam_channel::unbounded;
    use std::time::Duration;

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
        let policy = semantic_policy(100, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.idle_sleep, Duration::from_secs(5));
    }

    #[test]
    fn test_semantic_policy_pauses_under_queue_pressure() {
        let policy = semantic_policy(2_000, ServicePressure::Healthy);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let policy = semantic_policy(100, ServicePressure::Critical);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(10));
    }

    #[test]
    fn test_semantic_policy_pauses_when_service_is_degraded() {
        let policy = semantic_policy(100, ServicePressure::Degraded);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_stays_paused_while_service_recovers() {
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
