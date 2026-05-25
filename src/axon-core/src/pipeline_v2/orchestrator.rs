//! Pipeline A + Pipeline B orchestrator (CPT-AXO-054, session 19 topology).
//!
//! Wires A1 → A2 → A3 stages through bounded channels and per-stage worker
//! pools. A3 try_sends the chunk_ids it just persisted to a downstream
//! `b1_inbox` channel — that's the hand-off slot for pipeline B (the GPU
//! embedder lane). The cross-pipeline `try_send` is non-blocking per
//! CPT-AXO-053: graph + chunks + FTS keep their CPU-native cadence
//! regardless of B's GPU pace.
//!
//! Pipeline B (slice S4a) wires B1 (fetch content from PG by chunk_id).
//! B2 / B3 land in slice S4b on the same channel topology.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::graph::GraphStore;

use super::channels::PipelineChannelCaps;
use super::metrics::StageMetrics;
use super::stage_a1::a1_prepare;
use super::stage_a2::a2_transform;
use super::stage_a3::EnrolledFile;
use super::stage_b1::{b1_fetch_for_embedding, ChunkForEmbedding};
use super::stage_b2::{B2Embedder, EmbeddedChunk};
use super::stage_b3::PersistedEmbedding;
use super::types::ParsedFile;
use super::worker_pool::spawn_stage_workers;

/// Tunable per-stage worker counts. Operator-overridable through env vars
/// (REQ-AXO-290) `AXON_A1_WORKERS`, `AXON_A2_WORKERS`, `AXON_A3_WORKERS`.
#[derive(Debug, Clone, Copy)]
pub struct PipelineAWorkerCounts {
    pub a1: usize,
    pub a2: usize,
    pub a3: usize,
}

impl Default for PipelineAWorkerCounts {
    fn default() -> Self {
        Self {
            a1: 4,
            a2: 8,
            a3: 2,
        }
    }
}

/// Tunable per-stage worker counts for Pipeline B. Operator-overridable
/// through env vars `AXON_B1_WORKERS`, `AXON_B2_WORKERS`, `AXON_B3_WORKERS`.
///
/// Slice S4a wires B1 only; B2 / B3 fields are reserved for slice S4b.
#[derive(Debug, Clone, Copy)]
pub struct PipelineBWorkerCounts {
    pub b1: usize,
    pub b2: usize,
    pub b3: usize,
}

impl Default for PipelineBWorkerCounts {
    fn default() -> Self {
        Self {
            b1: 4,
            b2: 1,
            b3: 2,
        }
    }
}

impl PipelineBWorkerCounts {
    pub fn from_env() -> Self {
        let mut counts = Self::default();
        if let Ok(v) = std::env::var("AXON_B1_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.b1 = v;
            }
        }
        if let Ok(v) = std::env::var("AXON_B2_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.b2 = v;
            }
        }
        if let Ok(v) = std::env::var("AXON_B3_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.b3 = v;
            }
        }
        counts
    }
}

impl PipelineAWorkerCounts {
    pub fn from_env() -> Self {
        let mut counts = Self::default();
        if let Ok(v) = std::env::var("AXON_A1_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.a1 = v;
            }
        }
        if let Ok(v) = std::env::var("AXON_A2_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.a2 = v;
            }
        }
        if let Ok(v) = std::env::var("AXON_A3_WORKERS").and_then(|raw| {
            raw.trim()
                .parse::<usize>()
                .map_err(|_| std::env::VarError::NotPresent)
        }) {
            if v > 0 {
                counts.a3 = v;
            }
        }
        counts
    }
}

/// Handles for talking to a running Pipeline A.
///
/// * `input_tx` — feed paths to A1 (typically wired to the watcher debounce
///   handler). Bounded; blocks `send().await` if A1 is saturated (natural
///   upstream backpressure).
/// * `output_rx` — receive [`EnrolledFile`] receipts from A3.
/// * `b1_inbox_rx` — `chunk_id: String` items A3 fan-outs via `try_send`
///   (best-effort, non-blocking, cap `caps.a3_to_b1` = 10 000). Hand off
///   this receiver to [`spawn_pipeline_b_b1_only`] to wire pipeline B.
/// * `metrics_*` — observable per-stage telemetry.
pub struct PipelineAHandles {
    pub input_tx: Sender<PathBuf>,
    pub output_rx: Receiver<EnrolledFile>,
    pub b1_inbox_rx: Receiver<super::stage_b1::B1InboxItem>,
    /// Additional clone of the same `b1_inbox_tx` A3 workers push into.
    /// Used by external pollers (e.g. `pipeline_v2_runtime::spawn_pipeline_v2_indexer`'s
    /// periodic `b1_cold_start_poll` task) to rattrape chunks A3
    /// `try_send` dropped under buffer pressure (CPT-AXO-054 cold-start
    /// poll DB contract).
    pub b1_inbox_tx: Sender<super::stage_b1::B1InboxItem>,
    pub metrics_a1: Arc<StageMetrics>,
    pub metrics_a2: Arc<StageMetrics>,
    pub metrics_a3: Arc<StageMetrics>,
}

/// Spawn the three Pipeline A stages and return their handles.
///
/// The function returns immediately; stage workers run on the current tokio
/// runtime in the background. To stop the pipeline, drop `input_tx` — A1
/// workers will see `recv() = None` and exit, which closes the A1→A2
/// channel, which propagates through A2 and A3 in turn.
///
/// `resolver` returns the canonical 3-letter project_code for each
/// file path the pipeline processes. DEC-AXO-081 (supersedes the
/// single-project-per-indexer line in CPT-AXO-054): a single
/// pipeline_v2 instance can serve N projects; A3 calls the resolver
/// once per file and groups its batches by the resolved code so each
/// PG transaction stays homogeneous-project. Bench / tests pass
/// [`super::const_resolver`] for backward compatibility.
pub fn spawn_pipeline_a(
    counts: PipelineAWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    resolver: super::project_resolver::ProjectCodeResolver,
) -> PipelineAHandles {
    spawn_pipeline_a_with_cache(counts, caps, store, resolver, None)
}

/// Like [`spawn_pipeline_a`] but with an optional content-hash dedup
/// cache. When provided, files whose `(path, content_hash)` match the
/// cache are silently dropped between A1 and A2 — the expensive
/// tree-sitter parse is skipped entirely. The cache is updated by A3
/// after a successful UPSERT.
pub fn spawn_pipeline_a_with_cache(
    counts: PipelineAWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    resolver: super::project_resolver::ProjectCodeResolver,
    dedup_cache: Option<Arc<super::IndexedFileCache>>,
) -> PipelineAHandles {
    let (input_tx, input_rx) = mpsc::channel::<PathBuf>(caps.internal);
    let (a1_to_a2_tx, a1_to_a2_rx) = mpsc::channel(caps.internal);
    let (a2_to_a3_tx, a2_to_a3_rx) = mpsc::channel(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<EnrolledFile>(caps.internal);
    let (b1_inbox_tx, b1_inbox_rx) = mpsc::channel::<super::stage_b1::B1InboxItem>(caps.a3_to_b1);

    let metrics_a1 = StageMetrics::new("A1");
    let metrics_a2 = StageMetrics::new("A2");
    let metrics_a3 = StageMetrics::new("A3");

    // REQ-AXO-901624 — P4 Lazy Async TSV Build. Spawn the TsvBuilderWorker
    // pool alongside A1/A2/A3 ; it drains `pgmq.tsv_pending` out of band
    // and back-fills `Chunk.content_tsv`. Handles are intentionally
    // leaked : workers loop forever, lifetime = process. Disabled when
    // `AXON_TSV_WORKER_CONCURRENCY=0` for bench A/B comparisons against
    // the pre-P4 baseline.
    {
        let cfg = super::tsv_worker::TsvWorkerConfig::from_env();
        if cfg.concurrency > 0 {
            let _ = super::tsv_worker::spawn_tsv_workers(store.clone(), cfg);
        }
    }

    spawn_stage_workers(
        counts.a1,
        input_rx,
        a1_to_a2_tx,
        |path: PathBuf| async move { a1_prepare(path).await },
        metrics_a1.clone(),
    );

    // REQ-AXO-901746 — content-hash dedup filter between A1 and A2.
    // When a cache is provided, skip the expensive A2 tree-sitter parse
    // for files whose content is unchanged since last indexing.
    let a2_input_rx = if let Some(cache) = dedup_cache {
        let (filtered_tx, filtered_rx) = mpsc::channel(caps.internal);
        tokio::spawn(async move {
            let mut a1_rx = a1_to_a2_rx;
            let mut skipped: u64 = 0;
            let mut forwarded: u64 = 0;
            while let Some(prep) = a1_rx.recv().await {
                let path_str = prep.path.to_string_lossy().to_string();
                if cache.should_index(&path_str, &prep.content_hash) {
                    if filtered_tx.send(prep).await.is_err() {
                        break;
                    }
                    forwarded += 1;
                } else {
                    skipped += 1;
                }
                if (forwarded + skipped) % 500 == 0 && (forwarded + skipped) > 0 {
                    tracing::info!(
                        forwarded, skipped,
                        "dedup filter: {:.0}% skipped",
                        skipped as f64 / (forwarded + skipped) as f64 * 100.0
                    );
                }
            }
            tracing::info!(
                forwarded, skipped,
                "dedup filter done: {:.0}% skipped",
                if forwarded + skipped > 0 { skipped as f64 / (forwarded + skipped) as f64 * 100.0 } else { 0.0 }
            );
        });
        filtered_rx
    } else {
        a1_to_a2_rx
    };

    spawn_stage_workers(
        counts.a2,
        a2_input_rx,
        a2_to_a3_tx,
        |prep| async move { a2_transform(prep).await },
        metrics_a2.clone(),
    );

    // REQ-AXO-295 — A3 runs a dedicated batched worker. Per-file
    // BEGIN/COMMIT is the upstream throughput cliff: at A3=6 workers
    // PG locks contend so badly that sustained throughput is 2.7× LESS
    // than at A3=2 (NoOp bench 2026-05-12: 22 ch/s vs 57 ch/s). The
    // batched worker amortizes the transaction cost — N files written
    // in one execute_batch call. AXON_A3_BATCH_SIZE /
    // AXON_A3_BATCH_TIMEOUT_MS configure the lever. `counts.a3` is
    // honored: when > 1, we spawn N batched workers each with their
    // own intake channel, and a round-robin dispatcher fans the
    // upstream `a2_to_a3_rx` across them.
    {
        let bs = caps.a3_batch_size;
        let bto = std::time::Duration::from_millis(caps.a3_batch_timeout_ms);
        let n_workers = counts.a3.max(1);
        if n_workers == 1 {
            super::stage_a3::spawn_a3_batched_worker(
                a2_to_a3_rx,
                output_tx,
                b1_inbox_tx.clone(),
                store.clone(),
                resolver.clone(),
                metrics_a3.clone(),
                bs,
                bto,
            );
        } else {
            let mut worker_txs: Vec<mpsc::Sender<ParsedFile>> = Vec::with_capacity(n_workers);
            for _ in 0..n_workers {
                let (wtx, wrx) = mpsc::channel::<ParsedFile>(caps.internal);
                worker_txs.push(wtx);
                super::stage_a3::spawn_a3_batched_worker(
                    wrx,
                    output_tx.clone(),
                    b1_inbox_tx.clone(),
                    store.clone(),
                    resolver.clone(),
                    metrics_a3.clone(),
                    bs,
                    bto,
                );
            }
            // Drop the orchestrator-held tx so when all workers exit
            // the output channel actually closes.
            drop(output_tx);
            let mut a2_to_a3_rx_for_dispatch = a2_to_a3_rx;
            tokio::spawn(async move {
                let mut next = 0usize;
                while let Some(item) = a2_to_a3_rx_for_dispatch.recv().await {
                    let target = next % worker_txs.len();
                    next = next.wrapping_add(1);
                    if worker_txs[target].send(item).await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    PipelineAHandles {
        input_tx,
        output_rx,
        b1_inbox_rx,
        b1_inbox_tx,
        metrics_a1,
        metrics_a2,
        metrics_a3,
    }
}

/// Handles for talking to a running Pipeline B (S4a scope: B1 only).
///
/// `output_rx` yields one [`ChunkForEmbedding`] per chunk_id B1
/// successfully fetched from `public.Chunk`. None-fetches (race with a
/// concurrent re-parse that re-derived chunk_ids) are dropped silently
/// and do NOT surface on this channel — they just don't get embedded
/// this round; B1 cold-start poll DB (slice S4c) catches them later.
pub struct PipelineBHandles {
    pub output_rx: Receiver<ChunkForEmbedding>,
    pub metrics_b1: Arc<StageMetrics>,
}

/// Spawn Pipeline B stage workers (B1 only for S4a).
///
/// `b1_inbox_rx` is the receiver returned by [`spawn_pipeline_a`] —
/// pass it here to connect the A → B hand-off. B2 (GPU embedder) and
/// B3 (ChunkEmbedding UPSERT) land in slice S4b.
pub fn spawn_pipeline_b_b1_only(
    counts: PipelineBWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    b1_inbox_rx: Receiver<super::stage_b1::B1InboxItem>,
) -> PipelineBHandles {
    let (output_tx, output_rx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
    let metrics_b1 = StageMetrics::new("B1");

    let store_for_b1 = store.clone();
    spawn_stage_workers(
        counts.b1,
        b1_inbox_rx,
        output_tx,
        move |item: super::stage_b1::B1InboxItem| {
            let store = store_for_b1.clone();
            async move {
                match item {
                    super::stage_b1::B1InboxItem::Inline(payload) => Ok(payload),
                    super::stage_b1::B1InboxItem::FetchById(chunk_id) => {
                        match b1_fetch_for_embedding(chunk_id, store).await? {
                            Some(payload) => Ok(payload),
                            None => Err(anyhow::anyhow!("B1: chunk_id no longer in PG (race)")),
                        }
                    }
                }
            }
        },
        metrics_b1.clone(),
    );

    PipelineBHandles {
        output_rx,
        metrics_b1,
    }
}

/// Handles for talking to the full Pipeline B (B1 + B2 + B3).
///
/// `output_rx` yields one [`PersistedEmbedding`] receipt per chunk that
/// successfully traversed B1 (fetch) → B2 (GPU embed) → B3 (UPSERT).
/// Soft-skipped chunks (B1 None-fetch on race, B2 embedder error) do
/// NOT surface on this channel; their counts live on the
/// `errors_total` stage metric instead.
pub struct PipelineBFullHandles {
    pub output_rx: Receiver<PersistedEmbedding>,
    pub metrics_b1: Arc<StageMetrics>,
    pub metrics_b2: Arc<StageMetrics>,
    pub metrics_b3: Arc<StageMetrics>,
}

/// Spawn the three Pipeline B stages and return their handles.
///
/// `b1_inbox_rx` is the receiver returned by [`spawn_pipeline_a`] —
/// pass it here to connect the A → B hand-off. `embedder` is the
/// [`B2Embedder`] trait object that drives B2's GPU work; tests inject
/// [`super::stage_b2::NoOpEmbedder`], production wires the
/// `OrtGpuFirstTextEmbedding` wrapper (slice S4d).
pub fn spawn_pipeline_b_full(
    counts: PipelineBWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    embedder: Arc<dyn B2Embedder>,
    b1_inbox_rx: Receiver<super::stage_b1::B1InboxItem>,
) -> PipelineBFullHandles {
    spawn_pipeline_b_full_with_dedup(counts, caps, store, embedder, b1_inbox_rx, None)
}

pub fn spawn_pipeline_b_full_with_dedup(
    counts: PipelineBWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    embedder: Arc<dyn B2Embedder>,
    b1_inbox_rx: Receiver<super::stage_b1::B1InboxItem>,
    embedding_cache: super::stage_b1::EmbeddingDedupCache,
) -> PipelineBFullHandles {
    spawn_pipeline_b_full_multi(counts, caps, store, vec![embedder], b1_inbox_rx, embedding_cache)
}

/// REQ-AXO-901748 — multi-embedder variant. Each embedder in the vec
/// gets its own B2 batched worker with its own ORT session. CUDA
/// interleaves memory transfers and compute across sessions
/// (double-buffering). `counts.b2` is overridden by `embedders.len()`.
pub fn spawn_pipeline_b_full_multi(
    counts: PipelineBWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    embedders: Vec<Arc<dyn B2Embedder>>,
    b1_inbox_rx: Receiver<super::stage_b1::B1InboxItem>,
    embedding_cache: super::stage_b1::EmbeddingDedupCache,
) -> PipelineBFullHandles {
    // DEC-AXO-081 — B3 self-extracts project_code from each chunk_id
    // prefix; no orchestrator-level project_code needed here.
    let (b1_to_b2_tx, b1_to_b2_rx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
    let (b2_to_b3_tx, b2_to_b3_rx) = mpsc::channel::<EmbeddedChunk>(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<PersistedEmbedding>(caps.internal);

    let metrics_b1 = StageMetrics::new("B1");
    let metrics_b2 = StageMetrics::new("B2");
    let metrics_b3 = StageMetrics::new("B3");

    // REQ-AXO-314 — B1 runs the canonical batched worker (mirror of B2/B3).
    // Per-item B1 was bottlenecking GPU at 27% util: 4 PG SELECT/worker ×
    // ~50ms each = ~80 ch/s feed rate vs B2 batch=64 / 200ms timeout
    // (~320 ch/s needed to keep batches full). Batched B1 = 1 SELECT IN
    // (..) per pool_size chunk_ids → matches B2 granularity → GPU saturates.
    // `counts.b1` is no longer wired to a parallel-worker fan-out because
    // a single batched worker already serializes the SELECTs cheaper
    // than the deadpool can parallelize them; the count is preserved on
    // the struct for telemetry symmetry with B3.
    //
    // DEC-AXO-086 follow-up (option-b bucket-sort) — B1 pool size = 4× B2
    // batch size so every fetch returns ≥4 B2-batches worth of chunks,
    // and the fetch SQL orders them by token_count. B2 then captures
    // consecutive 64-windows that statistically fall in the SAME TensorRT
    // seq_bucket → padding ≈ 0 per GPU batch. Smaller pool means B2
    // batches cross bucket boundaries (current bug); larger pool wastes
    // memory + latency without further benefit.
    let _ = counts.b1;
    let b1_pool_size = caps.b2_batch_size.saturating_mul(4).max(caps.b2_batch_size);
    let embedding_cache_for_b3 = embedding_cache.clone();
    super::stage_b1::spawn_b1_batched_worker_with_dedup(
        b1_inbox_rx,
        b1_to_b2_tx,
        store.clone(),
        metrics_b1.clone(),
        b1_pool_size,
        std::time::Duration::from_millis(caps.b2_batch_timeout_ms),
        embedding_cache,
    );

    // REQ-AXO-901748 — B2 with per-worker embedder sessions. Each
    // embedder in the vec gets its own batched worker + ORT session.
    // CUDA interleaves transfers and compute across sessions.
    {
        let n_b2 = embedders.len().max(1);
        let b2_batch = caps.b2_batch_size / n_b2.max(1);
        let b2_timeout = std::time::Duration::from_millis(caps.b2_batch_timeout_ms);
        if n_b2 == 1 {
            super::stage_b2::spawn_b2_batched_worker(
                b1_to_b2_rx,
                b2_to_b3_tx,
                embedders.into_iter().next().unwrap(),
                metrics_b2.clone(),
                caps.b2_batch_size,
                b2_timeout,
            );
        } else {
            let mut worker_txs: Vec<mpsc::Sender<ChunkForEmbedding>> = Vec::with_capacity(n_b2);
            for emb in embedders {
                let (wtx, wrx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
                worker_txs.push(wtx);
                super::stage_b2::spawn_b2_batched_worker(
                    wrx,
                    b2_to_b3_tx.clone(),
                    emb,
                    metrics_b2.clone(),
                    b2_batch.max(1),
                    b2_timeout,
                );
            }
            drop(b2_to_b3_tx);
            let mut b1_to_b2_rx_for_dispatch = b1_to_b2_rx;
            tokio::spawn(async move {
                let mut next = 0usize;
                while let Some(item) = b1_to_b2_rx_for_dispatch.recv().await {
                    let target = next % worker_txs.len();
                    next = next.wrapping_add(1);
                    if worker_txs[target].send(item).await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    // REQ-AXO-295 — B3 mirrors A3: multi-row UPSERT amortizes
    // pgvector HNSW contention. AXON_B3_BATCH_SIZE /
    // AXON_B3_BATCH_TIMEOUT_MS expose the lever. `counts.b3 > 1` fans
    // out via a round-robin dispatcher so multiple batched B3 workers
    // can compete on the same upstream EmbeddedChunk stream.
    {
        let bs = caps.b3_batch_size;
        let bto = std::time::Duration::from_millis(caps.b3_batch_timeout_ms);
        let n_workers = counts.b3.max(1);
        if n_workers == 1 {
            super::stage_b3::spawn_b3_batched_worker_with_cache(
                b2_to_b3_rx,
                output_tx,
                store.clone(),
                metrics_b3.clone(),
                bs,
                bto,
                embedding_cache_for_b3.clone(),
            );
        } else {
            let mut worker_txs: Vec<mpsc::Sender<EmbeddedChunk>> =
                Vec::with_capacity(n_workers);
            for _ in 0..n_workers {
                let (wtx, wrx) = mpsc::channel::<EmbeddedChunk>(caps.internal);
                worker_txs.push(wtx);
                super::stage_b3::spawn_b3_batched_worker_with_cache(
                    wrx,
                    output_tx.clone(),
                    store.clone(),
                    metrics_b3.clone(),
                    bs,
                    bto,
                    embedding_cache_for_b3.clone(),
                );
            }
            drop(output_tx);
            let mut b2_to_b3_rx_for_dispatch = b2_to_b3_rx;
            tokio::spawn(async move {
                let mut next = 0usize;
                while let Some(item) = b2_to_b3_rx_for_dispatch.recv().await {
                    let target = next % worker_txs.len();
                    next = next.wrapping_add(1);
                    if worker_txs[target].send(item).await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    PipelineBFullHandles {
        output_rx,
        metrics_b1,
        metrics_b2,
        metrics_b3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn pipeline_a_end_to_end_persists_graph_chunks_and_indexed_file_for_a_rust_fixture() {
        // Session-19 contract: A persists graph + chunks + FTS in ONE
        // transaction. Receipt carries chunk_ids ready for B's GPU
        // lane. b1_inbox_rx receives the same chunk_ids via try_send.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e2e_fixture.rs");
        std::fs::write(&path, "fn main() { let x = 42; println!(\"{x}\"); }\n").unwrap();

        let counts = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let caps = PipelineChannelCaps::default();
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let mut handles = spawn_pipeline_a(counts, caps, store.clone(), super::super::const_resolver("AXO"));

        handles.input_tx.send(path.clone()).await.unwrap();

        let receipt = tokio::time::timeout(Duration::from_secs(5), handles.output_rx.recv())
            .await
            .expect("pipeline A must produce a receipt within 5 s")
            .expect("output channel must yield Some(EnrolledFile)");

        assert_eq!(receipt.path, path.to_string_lossy());
        assert_eq!(receipt.content_hash.len(), 64, "sha256 hex digest");
        assert!(
            receipt.symbols_count >= 1,
            "rust parser surfaces at least one symbol from `fn main` fixture"
        );
        assert!(
            !receipt.chunk_ids.is_empty(),
            "A3 must emit at least one chunk_id (session 19 chunking in A)"
        );

        // IndexedFile + Symbol + Chunk rows must all be in PG after the
        // single A3 transaction committed.
        let indexed = store
            .query_count(&format!(
                "SELECT count(*) FROM IndexedFile WHERE path = '{}'",
                path.to_string_lossy()
            ))
            .unwrap();
        assert_eq!(indexed, 1);

        let symbols = store
            .query_count(
                "SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name = 'main'",
            )
            .unwrap();
        assert!(symbols >= 1);

        let chunks = store
            .query_count(&format!(
                "SELECT count(*) FROM Chunk WHERE file_path = '{}'",
                path.to_string_lossy()
            ))
            .unwrap();
        assert!(
            chunks >= 1,
            "A3 must persist Chunk rows in the same transaction (session 19)"
        );

        // B1 inbox must have received the chunk_ids via A3's try_send.
        let first_id = tokio::time::timeout(Duration::from_secs(1), handles.b1_inbox_rx.recv())
            .await
            .expect("b1_inbox must receive within 1 s")
            .expect("b1_inbox receiver yields Some(chunk_id: String)");
        assert!(
            receipt.chunk_ids.contains(&first_id),
            "chunk_id fanned out to B1 must match one of the ids returned by A3"
        );

        let snap_a1 = handles.metrics_a1.snapshot();
        let snap_a2 = handles.metrics_a2.snapshot();
        let snap_a3 = handles.metrics_a3.snapshot();
        assert_eq!(snap_a1.items_out_total, 1, "A1 emitted 1 PreparedFile");
        assert_eq!(snap_a2.items_out_total, 1, "A2 emitted 1 ParsedFile");
        assert_eq!(snap_a3.items_out_total, 1, "A3 emitted 1 EnrolledFile");
        assert_eq!(snap_a1.errors_total, 0);
        assert_eq!(snap_a2.errors_total, 0);
        assert_eq!(snap_a3.errors_total, 0);
    }

    #[tokio::test]
    async fn pipeline_a_records_error_metrics_on_unparseable_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_supported.unknownext");
        std::fs::write(&path, "anything").unwrap();

        let counts = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let handles = spawn_pipeline_a(counts, PipelineChannelCaps::default(), store, super::super::const_resolver("AXO"));

        handles.input_tx.send(path.clone()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let snap_a1 = handles.metrics_a1.snapshot();
        let snap_a2 = handles.metrics_a2.snapshot();
        assert_eq!(snap_a1.items_out_total, 1, "A1 reads any file regardless of extension");
        assert_eq!(
            snap_a2.errors_total, 1,
            "A2 must record an error for unsupported extensions",
        );
        assert_eq!(
            snap_a2.items_out_total, 0,
            "A2 must NOT forward errored items to A3",
        );
    }

    #[test]
    fn default_worker_counts_match_live_template() {
        let counts = PipelineAWorkerCounts::default();
        assert_eq!(counts.a1, 4);
        assert_eq!(counts.a2, 8);
        assert_eq!(counts.a3, 2);
    }

    #[test]
    fn default_pipeline_b_worker_counts_match_session_19_table() {
        let counts = PipelineBWorkerCounts::default();
        assert_eq!(counts.b1, 4);
        assert_eq!(counts.b2, 1);
        assert_eq!(counts.b3, 2);
    }

    #[tokio::test]
    async fn pipelines_a_and_b_full_persist_chunk_embeddings_end_to_end() {
        // Full A → B (B1+B2+B3) happy path with NoOpEmbedder. After
        // both pipelines drain the fixture, the store must contain:
        //   * Symbol rows (A3)
        //   * Chunk rows with content_tsv-ready content (A3)
        //   * IndexedFile row (A3)
        //   * ChunkEmbedding rows (B3) — one per chunk_id A3 emitted
        use super::super::stage_b2::NoOpEmbedder;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ab_full_fixture.rs");
        std::fs::write(&path, "fn alpha() {}\nfn beta() { let x = 1; }\n").unwrap();

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let caps = PipelineChannelCaps::default();
        let counts_a = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let counts_b = PipelineBWorkerCounts {
            b1: 1,
            b2: 1,
            b3: 1,
        };

        let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), super::super::const_resolver("AXO"));
        let b1_inbox_rx = std::mem::replace(&mut handles_a.b1_inbox_rx, mpsc::channel(1).1);
        let embedder: Arc<dyn B2Embedder> = Arc::new(NoOpEmbedder);
        let mut handles_b =
            spawn_pipeline_b_full(counts_b, caps, store.clone(), embedder, b1_inbox_rx);

        handles_a.input_tx.send(path.clone()).await.unwrap();

        let enrolled = tokio::time::timeout(Duration::from_secs(5), handles_a.output_rx.recv())
            .await
            .expect("A must produce a receipt within 5 s")
            .expect("A output channel must yield Some(EnrolledFile)");

        let expected_chunks = enrolled.chunk_ids.len();
        assert!(expected_chunks >= 1);

        let mut persisted = 0usize;
        for _ in 0..expected_chunks {
            let receipt =
                tokio::time::timeout(Duration::from_secs(5), handles_b.output_rx.recv())
                    .await
                    .expect("B3 must produce a persist receipt within 5 s")
                    .expect("B3 output channel must yield Some(PersistedEmbedding)");
            assert!(enrolled.chunk_ids.contains(&receipt.chunk_id));
            persisted += 1;
        }
        assert_eq!(persisted, expected_chunks);

        let embed_count = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id IN ({})",
                enrolled
                    .chunk_ids
                    .iter()
                    .map(|c| format!("'{c}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            ))
            .unwrap();
        assert_eq!(embed_count as usize, expected_chunks);

        let snap_b1 = handles_b.metrics_b1.snapshot();
        let snap_b2 = handles_b.metrics_b2.snapshot();
        let snap_b3 = handles_b.metrics_b3.snapshot();
        assert_eq!(snap_b1.items_out_total as usize, expected_chunks);
        assert_eq!(snap_b2.items_out_total as usize, expected_chunks);
        assert_eq!(snap_b3.items_out_total as usize, expected_chunks);
        assert_eq!(snap_b1.errors_total, 0);
        assert_eq!(snap_b2.errors_total, 0);
        assert_eq!(snap_b3.errors_total, 0);
    }

    #[tokio::test]
    async fn pipelines_a_and_b_together_yield_chunk_for_embedding_payloads() {
        // Full A → B (B1 only) happy path. A3 writes graph + chunks +
        // FTS in one tx and try_sends chunk_ids to B1. B1 fetches the
        // chunk content back from PG and emits ChunkForEmbedding ready
        // for the slice S4b GPU embedder.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ab_fixture.rs");
        std::fs::write(&path, "fn alpha() { 1 + 1; }\nfn beta() { let q = 2; }\n").unwrap();

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let caps = PipelineChannelCaps::default();
        let counts_a = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let counts_b = PipelineBWorkerCounts {
            b1: 1,
            b2: 1,
            b3: 1,
        };

        let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), super::super::const_resolver("AXO"));
        let b1_inbox_rx = std::mem::replace(&mut handles_a.b1_inbox_rx, mpsc::channel(1).1);
        let mut handles_b = spawn_pipeline_b_b1_only(counts_b, caps, store.clone(), b1_inbox_rx);

        handles_a.input_tx.send(path.clone()).await.unwrap();

        let enrolled = tokio::time::timeout(Duration::from_secs(5), handles_a.output_rx.recv())
            .await
            .expect("A must produce a receipt within 5 s")
            .expect("A output channel must yield Some(EnrolledFile)");
        assert!(
            !enrolled.chunk_ids.is_empty(),
            "A3 must emit chunk_ids for the fixture"
        );

        // Drain B1: each chunk_id A3 emitted must eventually round-trip
        // through B1 as a ChunkForEmbedding (no GPU yet, but the
        // payload is ready for B2).
        let expected = enrolled.chunk_ids.len();
        let mut received = Vec::new();
        for _ in 0..expected {
            let payload = tokio::time::timeout(Duration::from_secs(5), handles_b.output_rx.recv())
                .await
                .expect("B1 must produce a payload within 5 s")
                .expect("B1 output channel must yield Some(ChunkForEmbedding)");
            received.push(payload);
        }
        assert_eq!(received.len(), expected);
        for payload in &received {
            assert!(enrolled.chunk_ids.contains(&payload.chunk_id));
            assert!(!payload.content.is_empty());
        }

        let snap_b1 = handles_b.metrics_b1.snapshot();
        assert_eq!(snap_b1.items_out_total as usize, expected);
        assert_eq!(snap_b1.errors_total, 0);
    }
}
