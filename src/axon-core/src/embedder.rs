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
const DEFAULT_MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";
const DEFAULT_MODEL_SLUG: &str = "bge-small-en-v1.5";
const DEFAULT_MODEL_VERSION: &str = "1";
const DEFAULT_EMBEDDING_DIMENSION: usize = 384;

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
    BGESmallENV15,
}

impl RuntimeEmbeddingModel {
    fn fastembed_model(self) -> EmbeddingModel {
        match self {
            Self::BGESmallENV15 => EmbeddingModel::BGESmallENV15,
        }
    }

    fn startup_label(self) -> &'static str {
        match self {
            Self::BGESmallENV15 => "BGE-Small",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfile {
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

fn embedding_model_id(kind: &'static str, model_slug: &'static str, dimension: usize) -> String {
    let prefix = match kind {
        "symbol" => "sym",
        other => other,
    };
    format!("{prefix}-{model_slug}-{dimension}")
}

impl EmbeddingProfile {
    pub fn new(
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

pub fn default_embedding_profile() -> EmbeddingProfile {
    EmbeddingProfile::new(
        DEFAULT_MODEL_NAME,
        DEFAULT_MODEL_SLUG,
        DEFAULT_MODEL_VERSION,
        DEFAULT_EMBEDDING_DIMENSION,
        RuntimeEmbeddingModel::BGESmallENV15,
        EmbeddingExecutionBackend::Unspecified,
        CHUNK_BATCH_SIZE,
        SYMBOL_BATCH_SIZE,
        FILE_VECTORIZATION_BATCH_SIZE,
        GRAPH_BATCH_SIZE,
    )
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

pub fn embedding_runtime_contract() -> EmbeddingRuntimeContract {
    let profile = default_embedding_profile();
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
        let profile = default_embedding_profile();
        let backend = default_embedding_execution_backend(runtime_profile.gpu_present);
        info!(
            "Semantic Worker: Initializing {} Model ({}d) in isolated thread...",
            profile.runtime_model.startup_label(),
            profile.dimension
        );
        info!(
            "Semantic Worker: embedding backend selected = {} (gpu_present={})",
            backend.name(),
            runtime_profile.gpu_present
        );

        let mut options = InitOptions::new(profile.runtime_model.fastembed_model());
        options.show_download_progress = true;
        options = options.with_execution_providers(embedding_execution_providers(backend));

        let mut model = match TextEmbedding::try_new(options) {
            Ok(m) => {
                info!(
                    "✅ Semantic Worker: {} model loaded successfully.",
                    profile.runtime_model.startup_label()
                );
                m
            }
            Err(e) => {
                error!("❌ Semantic Worker: FATAL ONNX INIT ERROR: {:?}", e);
                return;
            }
        };

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
            match graph_store
                .fetch_pending_file_vectorization_work(profile.file_vectorization_batch_size)
            {
                Ok(pending) if !pending.is_empty() => {
                    file_vectorization_backlog_active = true;
                    debug!(
                        "Semantic Worker: Embedding {} file vectorization jobs...",
                        pending.len()
                    );

                    let mut completed_works: Vec<FileVectorizationWork> = Vec::new();
                    let mut failed: HashMap<String, Vec<FileVectorizationWork>> = HashMap::new();

                    for work in pending {
                        let mut this_work_done = false;
                        let mut this_work_failed = false;
                        let mut iteration_reason = String::new();

                        loop {
                            match graph_store.fetch_unembedded_chunks_for_file(
                                &work.file_path,
                                &profile.chunk.model_id,
                                profile.chunk.batch_size,
                            ) {
                                Ok(chunks) if chunks.is_empty() => {
                                    this_work_done = true;
                                    break;
                                }
                                Ok(chunks) => {
                                    let texts: Vec<String> = chunks
                                        .iter()
                                        .map(|(_, content, _)| content.clone())
                                        .collect();
                                    match model.embed(texts, None) {
                                        Ok(embeddings) => {
                                            let updates: Vec<(String, String, Vec<f32>)> = chunks
                                                .into_iter()
                                                .zip(embeddings.into_iter())
                                                .map(|((id, _, hash), emb)| (id, hash, emb))
                                                .collect();

                                            if let Err(err) = graph_store
                                                .update_chunk_embeddings(
                                                    &profile.chunk.model_id,
                                                    &updates,
                                                )
                                            {
                                                this_work_failed = true;
                                                iteration_reason = format!(
                                                    "failed to persist chunk embeddings: {:?}",
                                                    err
                                                );
                                                error!(
                                                    "Semantic Worker: Chunk DB write error for file {}: {:?}",
                                                    work.file_path, err
                                                );
                                                break;
                                            }

                                            match graph_store.file_has_unembedded_chunks(
                                                &work.file_path,
                                                &profile.chunk.model_id,
                                            ) {
                                                Ok(has_pending) if has_pending => {
                                                    continue;
                                                }
                                                Ok(_) => {
                                                    this_work_done = true;
                                                    break;
                                                }
                                                Err(err) => {
                                                    this_work_failed = true;
                                                    iteration_reason = format!(
                                                        "failed to check pending chunks: {:?}",
                                                        err
                                                    );
                                                    error!(
                                                        "Semantic Worker: failed to check chunk completion for {}: {:?}",
                                                        work.file_path, err
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            this_work_failed = true;
                                            iteration_reason =
                                                format!("chunk embedding failed: {:?}", e);
                                            if is_fatal_embedding_error(&e) {
                                                error!(
                                                    "Semantic Worker: fatal chunk embedding error, disabling semantic worker: {:?}",
                                                    e
                                                );
                                                return;
                                            }
                                            error!(
                                                "Semantic Worker: Chunk embedding failed for {}: {:?}",
                                                work.file_path, e
                                            );
                                            break;
                                        }
                                    }
                                }
                                Err(err) => {
                                    this_work_failed = true;
                                    iteration_reason = format!(
                                        "chunk fetch failed for file {}: {:?}",
                                        work.file_path, err
                                    );
                                    error!(
                                        "Semantic Worker: failed to fetch unembedded chunks for {}: {:?}",
                                        work.file_path, err
                                    );
                                    break;
                                }
                            }
                        }

                        if this_work_done {
                            if let Err(err) =
                                graph_store.mark_file_vectorization_done(
                                    &[work.file_path.clone()],
                                    &profile.chunk.model_id,
                                )
                            {
                                failed
                                    .entry(format!(
                                        "failed to mark vectorization completion for {}: {:?}",
                                        work.file_path, err
                                    ))
                                    .or_default()
                                    .push(work);
                            } else {
                                completed_works.push(work.clone());
                            }
                        } else if this_work_failed {
                            failed.entry(iteration_reason).or_default().push(work);
                        }
                    }

                    if let Err(err) =
                        graph_store.mark_file_vectorization_work_done(&completed_works)
                    {
                        failed
                            .entry(format!("failed to clear file vector queue: {:?}", err))
                            .or_default()
                            .extend(completed_works.iter().cloned());
                    } else {
                        for work in &completed_works {
                            if let Err(err) = graph_store.enqueue_graph_projection_refresh(
                                "file",
                                &work.file_path,
                                FILE_PROJECTION_RADIUS,
                            ) {
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
