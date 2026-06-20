//! Pipeline A + Pipeline B orchestrator (CPT-AXO-054, session 19 topology).
//!
//! Wires A1 → A2 → A3 stages through bounded channels and per-stage worker
//! pools. A3 persists the chunk_ids it produced to PG; the
//! `trg_chunk_notify_pending` trigger fires `pg_notify` so pipeline B wakes.
//! There is NO cross-pipeline push channel — `try_send`/`b1_inbox` are RETIRED
//! (REQ-AXO-901746). graph + chunks + FTS keep their CPU-native cadence
//! (CPT-AXO-053) regardless of B's GPU pace, decoupled through PG rather than
//! an in-process push.
//!
//! Pipeline B (slice 4/5 SOTA) has NO B1 fetch-by-id worker pool — it was
//! collapsed into `demand_pull_b`, which SELECTs pending chunks WITH content
//! and feeds B2 → B3 on the `b_chunks` channel topology.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::graph::GraphStore;

use super::channels::PipelineChannelCaps;
use super::metrics::StageMetrics;
use super::stage_a1::a1_prepare;
use super::stage_a2::a2_transform;
use super::stage_a3::EnrolledFile;
use super::stage_b1::ChunkForEmbedding;
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
/// through env vars `AXON_B2_WORKERS`, `AXON_B3_WORKERS`.
///
/// B1 is retired (REQ-AXO-901746) — demand_pull_b feeds B2 directly, there is
/// no fetch-by-id worker pool. The dead `AXON_B1_WORKERS` knob is gone.
#[derive(Debug, Clone, Copy)]
pub struct PipelineBWorkerCounts {
    pub b2: usize,
    pub b3: usize,
}

impl Default for PipelineBWorkerCounts {
    fn default() -> Self {
        Self { b2: 1, b3: 2 }
    }
}

impl PipelineBWorkerCounts {
    pub fn from_env() -> Self {
        let mut counts = Self::default();
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
/// * `metrics_*` — observable per-stage telemetry.
///
/// Slice 5 SOTA — `b1_inbox_*` fields removed. The cross-pipeline
/// channel A3→B1 is gone : B is fed exclusively by the demand-pull
/// NOTIFY listener (`pipeline_v2/demand_pull.rs`) which SELECTs chunks
/// with content directly and emits `ChunkForEmbedding` to the b_chunks
/// channel owned by `pipeline_v2_runtime`.
pub struct PipelineAHandles {
    pub input_tx: Sender<PathBuf>,
    pub output_rx: Receiver<EnrolledFile>,
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
    // REQ-AXO-901906 — input/output carry tiny payloads (PathBuf / chunk_ids)
    // so they keep the large `internal` cap; the A1→A2 / A2→A3 channels carry
    // file CONTENT (≤5 MB/slot) and use the small `a_content` cap — that pairing
    // (small content channel + send().await) IS the pipeline-A memory bound.
    let (input_tx, input_rx) = mpsc::channel::<PathBuf>(caps.internal);
    let (a1_to_a2_tx, a1_to_a2_rx) = mpsc::channel(caps.a_content);
    let (a2_to_a3_tx, a2_to_a3_rx) = mpsc::channel(caps.a_content);
    let (output_tx, output_rx) = mpsc::channel::<EnrolledFile>(caps.internal);

    let metrics_a1 = StageMetrics::new("A1");
    let metrics_a2 = StageMetrics::new("A2");
    let metrics_a3 = StageMetrics::new("A3");

    // REQ-AXO-901919 — in-flight file watchdog: WARNs the moment any A-stage
    // file exceeds the budget, naming path + stage + age (turns a silent wedge
    // into a named culprit, without a bespoke diagnostic).
    super::in_flight::spawn_watchdog();

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

    // REQ-AXO-901916 CP2b — level-1 (mtime/size) I/O pre-filter. When a dedup
    // cache is present, a bare `stat()` BEFORE A1's read skips unchanged files
    // (mtime AND size match the last index) with ZERO I/O — no read, no sha256,
    // no parse. Restores the change-detection the scanner did in PG
    // (persist_discovery_batch) before the PIL-AXO-007 direct flow removed it,
    // so a re-walk / restart re-reads only the delta, not the whole fleet.
    let a1_input_rx = if let Some(cache_l1) = dedup_cache.clone() {
        let (pf_tx, pf_rx) = mpsc::channel::<PathBuf>(caps.internal);
        tokio::spawn(async move {
            let mut in_rx = input_rx;
            let (mut skipped, mut forwarded): (u64, u64) = (0, 0);
            while let Some(path) = in_rx.recv().await {
                let needs_read = match tokio::fs::metadata(&path).await {
                    Ok(md) => {
                        let (mtime_ms, size_bytes) = super::stage_a1::mtime_size_ms(&md);
                        cache_l1.should_read(&path.to_string_lossy(), mtime_ms, size_bytes)
                    }
                    // stat failed (deleted / perm) → let A1 surface it cleanly.
                    Err(_) => true,
                };
                if needs_read {
                    if pf_tx.send(path).await.is_err() {
                        break;
                    }
                    forwarded += 1;
                } else {
                    skipped += 1;
                }
                if (forwarded + skipped) % 1000 == 0 {
                    tracing::info!(
                        forwarded,
                        skipped,
                        "A1 pre-filter (mtime/size): {} files skipped without reading",
                        skipped
                    );
                }
            }
            tracing::info!(
                forwarded,
                skipped,
                "A1 pre-filter done: {} files skipped (no read/hash/parse)",
                skipped
            );
        });
        pf_rx
    } else {
        input_rx
    };

    spawn_stage_workers(
        counts.a1,
        a1_input_rx,
        a1_to_a2_tx,
        |path: PathBuf| async move { a1_prepare(path).await },
        metrics_a1.clone(),
    );

    // REQ-AXO-901746 — content-hash dedup filter between A1 and A2.
    // When a cache is provided, skip the expensive A2 tree-sitter parse
    // for files whose content is unchanged since last indexing.
    let a2_input_rx = if let Some(cache) = dedup_cache {
        let (filtered_tx, filtered_rx) = mpsc::channel(caps.a_content);
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
                    // REQ-AXO-902045 MUR 2 — content is unchanged, but L1
                    // forwarded this file because its mtime/size drifted (a
                    // `touch` / checkout / reformat-then-revert). Without
                    // refreshing the cached metadata, L1 re-reads + re-hashes it
                    // on EVERY reconciliation walk forever (the host-wide-watch
                    // CPU leak). Refresh L1 with the new mtime/size so the next
                    // walk skips it with zero I/O. The durable PG row self-heals
                    // on the next genuine content change (A3 UPSERT).
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    cache.mark_indexed(
                        path_str,
                        prep.content_hash,
                        now_ms,
                        prep.mtime_ms,
                        prep.size_bytes,
                    );
                    skipped += 1;
                }
                if (forwarded + skipped) % 500 == 0 && (forwarded + skipped) > 0 {
                    tracing::info!(
                        forwarded,
                        skipped,
                        "dedup filter: {:.0}% skipped",
                        skipped as f64 / (forwarded + skipped) as f64 * 100.0
                    );
                }
            }
            tracing::info!(
                forwarded,
                skipped,
                "dedup filter done: {:.0}% skipped",
                if forwarded + skipped > 0 {
                    skipped as f64 / (forwarded + skipped) as f64 * 100.0
                } else {
                    0.0
                }
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
                store.clone(),
                resolver.clone(),
                metrics_a3.clone(),
                bs,
                bto,
            );
        } else {
            let mut worker_txs: Vec<mpsc::Sender<ParsedFile>> = Vec::with_capacity(n_workers);
            for _ in 0..n_workers {
                let (wtx, wrx) = mpsc::channel::<ParsedFile>(caps.a_content);
                worker_txs.push(wtx);
                super::stage_a3::spawn_a3_batched_worker(
                    wrx,
                    output_tx.clone(),
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
        metrics_a1,
        metrics_a2,
        metrics_a3,
    }
}

/// Handles for talking to the full Pipeline B (B2 + B3 ; B1 collapsed
/// into demand_pull in slice 5 SOTA).
///
/// `output_rx` yields one [`PersistedEmbedding`] receipt per chunk that
/// successfully traversed B2 (GPU embed) → B3 (UPSERT). Soft-skipped
/// chunks (B2 embedder error) do NOT surface on this channel ; their
/// counts live on the `errors_total` stage metric instead.
pub struct PipelineBFullHandles {
    pub output_rx: Receiver<PersistedEmbedding>,
    pub metrics_b2: Arc<StageMetrics>,
    pub metrics_b3: Arc<StageMetrics>,
}

/// Spawn the Pipeline B GPU stages (B2 + B3) and return their handles.
///
/// Slice 5 SOTA — B1 stage worker collapsed into `demand_pull_b`. The
/// caller must own a `Receiver<ChunkForEmbedding>` that demand_pull
/// feeds directly (one SELECT-with-content round-trip per batch). B1
/// stage workers (4× parallel SELECT-by-id) are gone : a single
/// demand_pull task in `pipeline_v2_runtime` produces the b_chunks
/// stream, B2 GPU drum consumes it.
pub fn spawn_pipeline_b_full(
    counts: PipelineBWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    embedder: Arc<dyn B2Embedder>,
    b_chunks_rx: Receiver<super::stage_b1::ChunkForEmbedding>,
) -> PipelineBFullHandles {
    spawn_pipeline_b_full_multi(counts, caps, store, vec![embedder], b_chunks_rx)
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
    b_chunks_rx: Receiver<super::stage_b1::ChunkForEmbedding>,
) -> PipelineBFullHandles {
    // DEC-AXO-081 — B3 self-extracts project_code from each chunk_id
    // prefix; no orchestrator-level project_code needed here.
    let b1_to_b2_rx = b_chunks_rx;
    let (b2_to_b3_tx, b2_to_b3_rx) = mpsc::channel::<EmbeddedChunk>(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<PersistedEmbedding>(caps.internal);

    let metrics_b2 = StageMetrics::new("B2");
    let metrics_b3 = StageMetrics::new("B3");

    let _ = counts; // PipelineBWorkerCounts unused here — B2 count derives from the embedder vec

    // REQ-AXO-901748 — B2 with per-worker embedder sessions. Each
    // embedder in the vec gets its own batched worker + ORT session.
    // CUDA interleaves transfers and compute across sessions.
    // F-06 fix: each B2 worker keeps the full batch_size. Dividing it
    // would force TensorRT to compile a new engine for the smaller size
    // and reduce GPU compute density (more padding per batch). The
    // double-buffering gain comes from overlapping memory transfers
    // across sessions, not from smaller batches.
    {
        let n_b2 = embedders.len().max(1);
        let b2_timeout = std::time::Duration::from_millis(caps.b2_batch_timeout_ms);
        if n_b2 == 1 {
            super::stage_b2::spawn_b2_batched_worker(
                b1_to_b2_rx,
                b2_to_b3_tx,
                embedders.into_iter().next().unwrap(),
                metrics_b2.clone(),
                caps.b2_batch_size,
                b2_timeout,
                Some(store.clone()),
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
                    caps.b2_batch_size,
                    b2_timeout,
                    Some(store.clone()),
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
            super::stage_b3::spawn_b3_batched_worker(
                b2_to_b3_rx,
                output_tx,
                store.clone(),
                metrics_b3.clone(),
                bs,
                bto,
            );
        } else {
            let mut worker_txs: Vec<mpsc::Sender<EmbeddedChunk>> = Vec::with_capacity(n_workers);
            for _ in 0..n_workers {
                let (wtx, wrx) = mpsc::channel::<EmbeddedChunk>(caps.internal);
                worker_txs.push(wtx);
                super::stage_b3::spawn_b3_batched_worker(
                    wrx,
                    output_tx.clone(),
                    store.clone(),
                    metrics_b3.clone(),
                    bs,
                    bto,
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
        metrics_b2,
        metrics_b3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pipeline_a_end_to_end_persists_graph_chunks_and_indexed_file_for_a_rust_fixture() {
        // Session-19 contract: A persists graph + chunks in ONE transaction
        // (content_tsv FTS is back-filled out-of-band by the pgmq tsv_worker,
        // REQ-AXO-901624). Receipt carries chunk_ids ready for B's GPU
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
        let mut handles = spawn_pipeline_a(
            counts,
            caps,
            store.clone(),
            super::super::const_resolver("AXO"),
        );

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
            .query_count("SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name = 'main'")
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

        // Slice 4 SOTA — A3 no longer fan-outs to B1 inbox. The
        // EnrolledFile receipt is the canonical contract from A3.
        // B1 wake-up is via PG NOTIFY (trg_chunk_notify_pending) →
        // demand_pull_b LISTEN, integration-tested separately.

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pipeline_a_clean_skips_unparseable_extension_without_error() {
        // REQ-AXO-901885 — a file whose extension has no parser is NOT an error:
        // A2 emits a valid zero-symbol ParsedFile so A3 writes the IndexedFile
        // marker (zero chunks) and the scanner stops re-queueing it. Erroring
        // here was the root of the unbounded re-parse loop REQ-AXO-901885 fixed,
        // and the Memory Shield (REQ-AXO-901895) relies on the same non-error
        // zero-symbol skip path. (Was: pipeline_a_records_error_metrics_on_
        // unparseable_extension, which encoded the retired error-on-no-parser
        // contract.)
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_supported.unknownext");
        std::fs::write(&path, "anything").unwrap();

        let counts = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let handles = spawn_pipeline_a(
            counts,
            PipelineChannelCaps::default(),
            store,
            super::super::const_resolver("AXO"),
        );

        handles.input_tx.send(path.clone()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let snap_a1 = handles.metrics_a1.snapshot();
        let snap_a2 = handles.metrics_a2.snapshot();
        let snap_a3 = handles.metrics_a3.snapshot();
        assert_eq!(
            snap_a1.items_out_total, 1,
            "A1 reads any file regardless of extension"
        );
        assert_eq!(
            snap_a1.errors_total, 0,
            "A1 read of a text file is not an error"
        );
        assert_eq!(
            snap_a2.errors_total, 0,
            "REQ-AXO-901885: no-parser extension is a clean zero-symbol skip, NOT an error",
        );
        assert_eq!(
            snap_a2.items_out_total, 1,
            "A2 forwards a zero-symbol ParsedFile so A3 can write the IndexedFile marker",
        );
        assert_eq!(
            snap_a3.items_out_total, 1,
            "A3 enrolls the zero-symbol file (marker write, zero chunks)",
        );
        assert_eq!(snap_a3.errors_total, 0, "A3 marker write is not an error");
    }

    #[test]
    fn default_pipeline_b_worker_counts_match_session_19_table() {
        let counts = PipelineBWorkerCounts::default();
        assert_eq!(counts.b2, 1);
        assert_eq!(counts.b3, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pipelines_a_and_b_full_persist_chunk_embeddings_end_to_end() {
        // Full A → B (demand_pull → B2 → B3) happy path with NoOpEmbedder.
        // After both pipelines drain the fixture, the store must contain:
        //   * Symbol rows (A3)
        //   * Chunk rows with content_tsv-ready content (A3)
        //   * IndexedFile row (A3)
        //   * ChunkEmbedding rows (B3) — one per chunk_id A3 emitted
        //
        // Slice 5 SOTA — the b_chunks channel is created here and a
        // synthetic poll-feeder mirrors what demand_pull_b does at
        // runtime (SELECT-with-content + push to b_chunks_tx).
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
        let counts_b = PipelineBWorkerCounts { b2: 1, b3: 1 };

        let mut handles_a = spawn_pipeline_a(
            counts_a,
            caps,
            store.clone(),
            super::super::const_resolver("AXO"),
        );

        // Send the file into A and await its receipt FIRST, so B's feed can be
        // scoped to exactly THIS file's chunk_ids. Was: a DB-WIDE
        // `select_chunks_with_content_needing_embedding(64)` poll-feeder — on
        // the process-shared test DB it also drained orphan 'pending' chunks
        // left by sibling A-only tests, so B embedded a foreign chunk and the
        // `enrolled.chunk_ids.contains(&receipt.chunk_id)` assertion failed.
        // REQ-AXO-901903 — feed B by id (deterministic, residue-independent).
        handles_a.input_tx.send(path.clone()).await.unwrap();

        let enrolled = tokio::time::timeout(Duration::from_secs(5), handles_a.output_rx.recv())
            .await
            .expect("A must produce a receipt within 5 s")
            .expect("A output channel must yield Some(EnrolledFile)");

        // Feed B ONLY this file's chunks (by id), mirroring demand_pull_b's
        // SELECT-with-content but scoped so the test is independent of any other
        // test's residue in the shared test DB.
        let (b_chunks_tx, b_chunks_rx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
        let store_for_feeder = store.clone();
        let feed_ids = enrolled.chunk_ids.clone();
        tokio::spawn(async move {
            let pulled = tokio::task::spawn_blocking(move || {
                store_for_feeder.fetch_chunks_for_embedding_batch(&feed_ids)
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
            for (chunk_id, content, content_hash) in pulled {
                let _ = b_chunks_tx
                    .send(ChunkForEmbedding {
                        chunk_id,
                        content,
                        content_hash,
                    })
                    .await;
            }
        });

        let embedder: Arc<dyn B2Embedder> = Arc::new(NoOpEmbedder);
        let mut handles_b =
            spawn_pipeline_b_full(counts_b, caps, store.clone(), embedder, b_chunks_rx);

        let expected_chunks = enrolled.chunk_ids.len();
        assert!(expected_chunks >= 1);

        let mut persisted = 0usize;
        for _ in 0..expected_chunks {
            let receipt = tokio::time::timeout(Duration::from_secs(5), handles_b.output_rx.recv())
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

        let snap_b2 = handles_b.metrics_b2.snapshot();
        let snap_b3 = handles_b.metrics_b3.snapshot();
        assert_eq!(snap_b2.items_out_total as usize, expected_chunks);
        assert_eq!(snap_b3.items_out_total as usize, expected_chunks);
        assert_eq!(snap_b2.errors_total, 0);
        assert_eq!(snap_b3.errors_total, 0);
    }

    // Slice 5 SOTA — `pipelines_a_and_b_together_yield_chunk_for_embedding_payloads`
    // test removed. It validated the A3 → b1_inbox → B1 fetch-by-id
    // path which is gone (B1 stage worker collapsed into demand_pull,
    // A3 no longer pushes Inline payloads). The A → demand_pull → B2
    // happy path is exercised end-to-end by the bench harness
    // `axon-bench-pipeline-v2` and slice 6 demonstration (24K files).
}
