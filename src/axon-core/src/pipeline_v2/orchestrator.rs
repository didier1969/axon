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
use super::stage_a3::{a3_enroll, EnrolledFile};
use super::stage_b1::{b1_fetch_for_embedding, ChunkForEmbedding};
use super::stage_b2::{b2_embed, B2Embedder, EmbeddedChunk};
use super::stage_b3::{b3_persist_embedding, PersistedEmbedding};
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
    pub b1_inbox_rx: Receiver<String>,
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
/// `project_code` is the canonical 3-letter project the watcher is rooted
/// at. CPT-AXO-053 prescribes single-project per indexer instance, so
/// passing it here once is enough — A3 stamps every Symbol / Chunk /
/// IndexedFile row with this code.
pub fn spawn_pipeline_a(
    counts: PipelineAWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    project_code: impl Into<Arc<str>>,
) -> PipelineAHandles {
    let project_code: Arc<str> = project_code.into();
    let (input_tx, input_rx) = mpsc::channel::<PathBuf>(caps.internal);
    let (a1_to_a2_tx, a1_to_a2_rx) = mpsc::channel(caps.internal);
    let (a2_to_a3_tx, a2_to_a3_rx) = mpsc::channel(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<EnrolledFile>(caps.internal);
    let (b1_inbox_tx, b1_inbox_rx) = mpsc::channel::<String>(caps.a3_to_b1);

    let metrics_a1 = StageMetrics::new("A1");
    let metrics_a2 = StageMetrics::new("A2");
    let metrics_a3 = StageMetrics::new("A3");

    spawn_stage_workers(
        counts.a1,
        input_rx,
        a1_to_a2_tx,
        |path: PathBuf| async move { a1_prepare(path).await },
        metrics_a1.clone(),
    );

    spawn_stage_workers(
        counts.a2,
        a1_to_a2_rx,
        a2_to_a3_tx,
        |prep| async move { a2_transform(prep).await },
        metrics_a2.clone(),
    );

    let store_for_a3 = store.clone();
    let pc_for_a3 = project_code.clone();
    let b1_tx_for_a3 = b1_inbox_tx.clone();
    spawn_stage_workers(
        counts.a3,
        a2_to_a3_rx,
        output_tx,
        move |parsed| {
            let store = store_for_a3.clone();
            let pc = pc_for_a3.clone();
            let b1_tx = b1_tx_for_a3.clone();
            async move {
                let enrolled = a3_enroll(parsed, store, pc).await?;
                // Best-effort fan-out to B1's inbox. Graph priority:
                // drop silently on full — B1's cold-start poll DB
                // pathway (slice S4c) catches the leakage.
                for cid in &enrolled.chunk_ids {
                    let _ = b1_tx.try_send(cid.clone());
                }
                Ok(enrolled)
            }
        },
        metrics_a3.clone(),
    );

    PipelineAHandles {
        input_tx,
        output_rx,
        b1_inbox_rx,
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
    b1_inbox_rx: Receiver<String>,
) -> PipelineBHandles {
    let (output_tx, output_rx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
    let metrics_b1 = StageMetrics::new("B1");

    let store_for_b1 = store.clone();
    spawn_stage_workers(
        counts.b1,
        b1_inbox_rx,
        output_tx,
        move |chunk_id: String| {
            let store = store_for_b1.clone();
            async move {
                // None = chunk_id no longer addressable (race with
                // re-parse). Surface a "soft skip" via Err so the
                // worker pool records it as a no-op step without
                // forwarding to the downstream channel.
                match b1_fetch_for_embedding(chunk_id, store).await? {
                    Some(payload) => Ok(payload),
                    None => Err(anyhow::anyhow!("B1: chunk_id no longer in PG (race)")),
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
    project_code: impl Into<Arc<str>>,
    embedder: Arc<dyn B2Embedder>,
    b1_inbox_rx: Receiver<String>,
) -> PipelineBFullHandles {
    let project_code: Arc<str> = project_code.into();
    let (b1_to_b2_tx, b1_to_b2_rx) = mpsc::channel::<ChunkForEmbedding>(caps.internal);
    let (b2_to_b3_tx, b2_to_b3_rx) = mpsc::channel::<EmbeddedChunk>(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<PersistedEmbedding>(caps.internal);

    let metrics_b1 = StageMetrics::new("B1");
    let metrics_b2 = StageMetrics::new("B2");
    let metrics_b3 = StageMetrics::new("B3");

    let store_for_b1 = store.clone();
    spawn_stage_workers(
        counts.b1,
        b1_inbox_rx,
        b1_to_b2_tx,
        move |chunk_id: String| {
            let store = store_for_b1.clone();
            async move {
                match b1_fetch_for_embedding(chunk_id, store).await? {
                    Some(payload) => Ok(payload),
                    None => Err(anyhow::anyhow!("B1: chunk_id no longer in PG (race)")),
                }
            }
        },
        metrics_b1.clone(),
    );

    let embedder_for_b2 = embedder.clone();
    spawn_stage_workers(
        counts.b2,
        b1_to_b2_rx,
        b2_to_b3_tx,
        move |payload: ChunkForEmbedding| {
            let embedder = embedder_for_b2.clone();
            async move { b2_embed(payload, embedder).await }
        },
        metrics_b2.clone(),
    );

    let store_for_b3 = store.clone();
    let pc_for_b3 = project_code.clone();
    spawn_stage_workers(
        counts.b3,
        b2_to_b3_rx,
        output_tx,
        move |embedded: EmbeddedChunk| {
            let store = store_for_b3.clone();
            let pc = pc_for_b3.clone();
            async move { b3_persist_embedding(embedded, store, pc).await }
        },
        metrics_b3.clone(),
    );

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
        let mut handles = spawn_pipeline_a(counts, caps, store.clone(), "AXO");

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
        let handles = spawn_pipeline_a(counts, PipelineChannelCaps::default(), store, "AXO");

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

        let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), "AXO");
        let b1_inbox_rx = std::mem::replace(&mut handles_a.b1_inbox_rx, mpsc::channel(1).1);
        let embedder: Arc<dyn B2Embedder> = Arc::new(NoOpEmbedder);
        let mut handles_b =
            spawn_pipeline_b_full(counts_b, caps, store.clone(), "AXO", embedder, b1_inbox_rx);

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

        let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), "AXO");
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
