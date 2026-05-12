//! Pipeline A orchestrator (CPT-AXO-054, session 18 topology).
//!
//! Wires A1 → A2 → A3 stages through bounded channels and per-stage worker
//! pools. A1's output ALSO fans out via `try_send` to a downstream
//! `b1_inbox` channel — that's the hand-off slot for pipeline B (the
//! vectorisation lane) once slice S4 wires its consumer. The graph
//! pipeline (A) and vector pipeline (B) thus stay throughput-independent
//! per CPT-AXO-053: A never blocks on B's pace.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::graph::GraphStore;

use super::channels::PipelineChannelCaps;
use super::metrics::StageMetrics;
use super::stage_a1::a1_prepare;
use super::stage_a2::a2_transform;
use super::stage_a3::{a3_enroll, EnrolledFile};
use super::types::PreparedFile;
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
/// * `output_rx` — receive [`EnrolledFile`] receipts from A3. Optional to
///   consume — if you don't care about acks, just drop the receiver and the
///   sender side is fine (mpsc senders survive a dropped receiver only until
///   they try to send, at which point the worker exits cleanly).
/// * `b1_inbox_rx` — non-blocking fan-out from A1 to the B-pipeline. Each
///   [`PreparedFile`] published by A1 is also `try_send`'d here so B1 can
///   chunk-and-vectorise it in parallel. The channel is bounded at
///   `caps.a3_to_b1` (default 10_000). When full, A1 drops the send
///   silently — B1's cold-start poll DB pathway picks up any leakage.
///   Slice S4 wires B1 to consume from this receiver; until then it's
///   exposed so callers can drain or assert on it in tests.
/// * `metrics` — one [`StageMetrics`] per stage, observable for telemetry.
pub struct PipelineAHandles {
    pub input_tx: Sender<PathBuf>,
    pub output_rx: Receiver<EnrolledFile>,
    pub b1_inbox_rx: Receiver<PreparedFile>,
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
/// passing it here once is enough — A3 stamps every Symbol / IndexedFile
/// row with this code.
pub fn spawn_pipeline_a(
    counts: PipelineAWorkerCounts,
    caps: PipelineChannelCaps,
    store: Arc<GraphStore>,
    project_code: impl Into<Arc<str>>,
) -> PipelineAHandles {
    let project_code: Arc<str> = project_code.into();
    let (input_tx, input_rx) = mpsc::channel::<PathBuf>(caps.internal);
    let (a1_to_a2_tx, a1_to_a2_rx) = mpsc::channel::<PreparedFile>(caps.internal);
    let (a2_to_a3_tx, a2_to_a3_rx) = mpsc::channel(caps.internal);
    let (output_tx, output_rx) = mpsc::channel::<EnrolledFile>(caps.internal);
    let (b1_inbox_tx, b1_inbox_rx) = mpsc::channel::<PreparedFile>(caps.a3_to_b1);

    let metrics_a1 = StageMetrics::new("A1");
    let metrics_a2 = StageMetrics::new("A2");
    let metrics_a3 = StageMetrics::new("A3");

    // A1 produces a single PreparedFile; the worker pool emits it onto
    // `a1_to_a2_tx`, AND we tap a fan-out so the same PreparedFile is
    // `try_send`'d to `b1_inbox_tx` (best-effort, non-blocking) for B's
    // consumption. Wrapping the user closure in `forward_with_b1_fanout`
    // keeps the worker_pool generic intact.
    let b1_tx_for_a1 = b1_inbox_tx.clone();
    spawn_stage_workers(
        counts.a1,
        input_rx,
        a1_to_a2_tx,
        move |path: PathBuf| {
            let b1_tx = b1_tx_for_a1.clone();
            async move {
                let prepared = a1_prepare(path).await?;
                // Best-effort fan-out to B1's inbox. Graph priority:
                // never block A on B's pace.
                let _ = b1_tx.try_send(prepared.clone());
                Ok(prepared)
            }
        },
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
    spawn_stage_workers(
        counts.a3,
        a2_to_a3_rx,
        output_tx,
        move |parsed| {
            let store = store_for_a3.clone();
            let pc = pc_for_a3.clone();
            async move { a3_enroll(parsed, store, pc).await }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn pipeline_a_end_to_end_persists_graph_and_indexed_file_for_a_rust_fixture() {
        // Create a real .rs file on disk so A1 has something to read,
        // A2 has something to parse, and A3 has something to UPSERT.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("e2e_fixture.rs");
        std::fs::write(&path, "fn main() { let x = 42; println!(\"{x}\"); }\n").unwrap();

        // Single-worker counts keep the test deterministic and snappy.
        let counts = PipelineAWorkerCounts {
            a1: 1,
            a2: 1,
            a3: 1,
        };
        let caps = PipelineChannelCaps::default();
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let mut handles = spawn_pipeline_a(counts, caps, store.clone(), "AXO");

        // Feed the fixture path in.
        handles.input_tx.send(path.clone()).await.unwrap();

        // Wait for A3's receipt with a generous timeout (CI hosts vary).
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

        // IndexedFile row must be in the canonical store after A3 enrolled the file.
        let in_db = store
            .query_count(&format!(
                "SELECT count(*) FROM IndexedFile WHERE path = '{}'",
                path.to_string_lossy()
            ))
            .unwrap();
        assert_eq!(in_db, 1, "A3 must persist exactly one IndexedFile row");

        // Symbol row(s) for the parsed file must be in the canonical store.
        let symbol_count = store
            .query_count(
                "SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name = 'main'",
            )
            .unwrap();
        assert!(
            symbol_count >= 1,
            "A3 must persist Symbol row(s) for the parsed fixture (saw {symbol_count})"
        );

        // A3 is graph-only — no Chunk rows. B1 (slice S4) owns Chunk
        // persistence. Re-asserting here so the topology invariant is
        // graven into the test suite.
        let chunk_count = store
            .query_count(&format!(
                "SELECT count(*) FROM Chunk WHERE file_path = '{}'",
                path.to_string_lossy()
            ))
            .unwrap();
        assert_eq!(
            chunk_count, 0,
            "A3 must NOT persist Chunk rows; that work belongs to B1 (slice S4)"
        );

        // B1 inbox must have received the PreparedFile via A1's
        // try_send fan-out (cap 10_000, never full for one item).
        let b1_prepared =
            tokio::time::timeout(Duration::from_secs(1), handles.b1_inbox_rx.recv())
                .await
                .expect("b1_inbox must receive within 1 s")
                .expect("b1_inbox receiver yields Some(PreparedFile)");
        assert_eq!(b1_prepared.path, path);
        assert_eq!(
            b1_prepared.content_hash, receipt.content_hash,
            "B1 inbox must carry the same content_hash A3 recorded in IndexedFile"
        );

        // Per-stage metrics: each stage must have processed exactly one item.
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

        // Give the pipeline a moment to process and surface the error.
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
}
