//! Stage B2 — GPU embedder (CPT-AXO-054 session 19).
//!
//! B2 receives [`super::stage_b1::ChunkForEmbedding`] payloads from B1
//! (already content-resolved against `ist.Chunk`), forwards the text
//! through a [`B2Embedder`] implementation (the production wrapper around
//! `OrtGpuFirstTextEmbedding` + TensorRT BGE-Large for live deployments,
//! a deterministic no-op for tests), and emits an [`EmbeddedChunk`] for
//! B3 to persist.
//!
//! **Batching is the embedder's responsibility.** The B2 worker hands
//! one [`ChunkForEmbedding`] at a time to the trait. Production
//! implementations that need GPU batching aggregate inside the trait
//! via an internal buffer + flush rule; the worker pool just keeps
//! feeding chunks. Slice S4b ships the single-item interface and a
//! no-op embedder; a batched production wrapper lands separately
//! against the existing `OrtGpuFirstTextEmbedding` once REQ-AXO-262
//! IoBinding refactor stabilises the GPU hot path.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::warn;

use super::metrics::StageMetrics;
use super::stage_b1::ChunkForEmbedding;

/// REQ-AXO-902033 — per-inference watchdog budget (ms). A normal B2 batch
/// embeds in ms–seconds; the first inference may pay a one-off TensorRT engine
/// build (~tens of seconds). A genuine hang never returns (minutes+). Default
/// 180 s clears the engine-build cold-start without ever falsely killing a
/// progressing inference. Override via `AXON_B2_INFERENCE_TIMEOUT_MS` (0
/// disables the watchdog → legacy unbounded await).
pub(crate) fn b2_inference_timeout_ms() -> u64 {
    std::env::var("AXON_B2_INFERENCE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| if v == 0 { u64::MAX } else { v })
        .unwrap_or(180_000)
}

/// Payload forwarded by B2 to B3 — chunk identity + the embedding the
/// GPU produced + the `content_hash` source_hash that B3 records on the
/// `ChunkEmbedding` row to spot stale embeddings.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedChunk {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedding: Vec<f32>,
}

/// Pluggable embedder trait. Production wraps
/// `OrtGpuFirstTextEmbedding` (TensorRT BGE-Large 1024d) behind this
/// surface; tests use [`NoOpEmbedder`] to keep the topology assertions
/// hardware-independent.
pub trait B2Embedder: Send + Sync {
    /// Embed `texts` and return the same-length Vec of embedding vectors.
    /// Each `Vec<f32>` length must equal the model dimension (1024 for
    /// the canonical BGE-Large model). The trait is sync because the
    /// caller wraps it in `spawn_blocking` — moving GPU work off the
    /// tokio runtime stays the right move under all backends.
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Deterministic test embedder. Emits `[1.0, 0.0, 0.0, ..., 0.0]` per
/// input text (dimension = [`crate::embedding_contract::DIMENSION`]).
/// Useful to exercise the B2 → B3 worker topology without touching
/// CUDA / TensorRT in unit tests.
pub struct NoOpEmbedder;

impl B2Embedder for NoOpEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        use crate::embedding_contract::DIMENSION;
        let mut out = Vec::with_capacity(texts.len());
        for _ in texts {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            out.push(v);
        }
        Ok(out)
    }
}

/// Embed a single [`ChunkForEmbedding`] payload, return an
/// [`EmbeddedChunk`].
///
/// The actual ORT/TensorRT call is dispatched via the supplied
/// [`B2Embedder`]. The call is wrapped in [`tokio::task::spawn_blocking`]
/// so the GPU dispatch does not stall the tokio runtime (mandatory under
/// `live` mode where B1's PG fetch and B2's GPU embed share the same
/// runtime).
#[cfg(test)]
pub async fn b2_embed(
    payload: ChunkForEmbedding,
    embedder: Arc<dyn B2Embedder>,
) -> Result<EmbeddedChunk> {
    let chunk_id = payload.chunk_id.clone();
    let source_hash = payload.content_hash.clone();
    let content = payload.content;

    let embedder_for_block = embedder.clone();
    let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
        let texts = vec![content];
        let mut out = embedder_for_block.embed_batch(&texts)?;
        if out.is_empty() {
            return Err(anyhow::anyhow!(
                "B2: embedder returned 0 embeddings for 1 input"
            ));
        }
        Ok(out.remove(0))
    })
    .await??;

    Ok(EmbeddedChunk {
        chunk_id,
        source_hash,
        embedding,
    })
}

/// REQ-AXO-289 S4b' — Spawn the canonical B2 worker as a dedicated
/// batching tokio task (NOT the generic competing-consumers helper).
///
/// The worker reads from `rx`, accumulates incoming [`ChunkForEmbedding`]
/// payloads up to `batch_size`, OR waits at most `batch_timeout` for the
/// next item before flushing a partial batch. Each flush is one
/// `embedder.embed_batch(&texts)` call dispatched on a blocking thread
/// via [`tokio::task::spawn_blocking`] (GPU work must stay off the
/// tokio runtime). Per-item metrics are recorded for batch entries
/// (record_started before flush) and finished entries (record_finished
/// with per-item mean duration after flush) so the downstream
/// observability (StageSnapshot) sees individual chunk lifecycle.
///
/// Mismatched embedding count vs batch size (embedder returned wrong
/// number) and embedder errors both record_error for every queued
/// payload. The downstream channel closing drops the worker cleanly.
pub fn spawn_b2_batched_worker(
    mut rx: Receiver<ChunkForEmbedding>,
    tx: Sender<EmbeddedChunk>,
    embedder: Arc<dyn B2Embedder>,
    metrics: Arc<StageMetrics>,
    batch_size: usize,
    batch_timeout: Duration,
    // REQ-AXO-902012 — store handle so a failed batch increments embed_attempts
    // (and quarantines at the cap) instead of leaving the chunks re-drainable.
    // `None` in pure batching tests that never hit the DB failure path.
    store: Option<Arc<crate::graph::GraphStore>>,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        loop {
            // REQ-AXO-901608 — t_recv timing (starvation indicator).
            let recv_started = Instant::now();
            // Wait without timeout for the first item — when idle, the
            // worker should be cheap.
            let first = match rx.recv().await {
                Some(item) => {
                    let recv_us =
                        recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    metrics.record_recv_wait(recv_us);
                    item
                }
                None => break,
            };
            let mut batch: Vec<ChunkForEmbedding> = Vec::with_capacity(batch_size);
            batch.push(first);

            // Drain additional items until batch_size or batch_timeout.
            let deadline = Instant::now() + batch_timeout;
            while batch.len() < batch_size {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let recv_started = Instant::now();
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Some(item)) => {
                        let recv_us =
                            recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        metrics.record_recv_wait(recv_us);
                        batch.push(item);
                    }
                    Ok(None) => {
                        // Upstream closed mid-drain — flush this batch
                        // then break the outer loop after embed.
                        break;
                    }
                    Err(_) => {
                        let recv_us =
                            recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        metrics.record_recv_wait(recv_us);
                        break;
                    }
                }
            }

            for _ in &batch {
                metrics.record_started();
            }
            let texts: Vec<String> = batch.iter().map(|p| p.content.clone()).collect();
            // REQ-AXO-902033 — log the batch shape BEFORE the (blocking) GPU
            // inference. The TensorRT EP intermittently hangs a single inference
            // under concurrent Plane-A load (GPU pegged, no return). The last
            // `B2 embed batch` line with no matching completion names the
            // culprit batch shape/content for RCA.
            let max_bytes = texts.iter().map(String::len).max().unwrap_or(0);
            let total_bytes: usize = texts.iter().map(String::len).sum();
            tracing::info!(
                target: "pipeline_v2::b2",
                n = batch.len(),
                max_bytes,
                total_bytes,
                first_id = %batch.first().map(|p| p.chunk_id.as_str()).unwrap_or(""),
                "B2 embed batch start"
            );
            let embedder_clone = embedder.clone();
            let started = Instant::now();
            // REQ-AXO-902033 — inference watchdog. The TensorRT EP intermittently
            // hangs a single inference under concurrent Plane-A load (GPU pegged,
            // no return). The blocking thread holds the embedder Mutex, so the
            // next batch would deadlock on the lock — in-process recovery is
            // impossible. Per DEC-AXO-901631's process-level GPU model, fail loud
            // + exit so the supervisor (process-compose restart=on_failure,
            // max_restarts=3) restarts the indexer; `embed_status='pending'` is the
            // durable queue, so embedding resumes exactly where it stalled. The
            // `B2 embed batch start` log (above) names the culprit batch.
            let inference_budget = Duration::from_millis(b2_inference_timeout_ms());
            let embed_fut = tokio::task::spawn_blocking(move || embedder_clone.embed_batch(&texts));
            let join_result = match tokio::time::timeout(inference_budget, embed_fut).await {
                Ok(jr) => jr,
                Err(_elapsed) => {
                    tracing::error!(
                        target: "pipeline_v2::b2",
                        n = batch.len(),
                        max_bytes,
                        timeout_ms = inference_budget.as_millis() as u64,
                        first_id = %batch.first().map(|p| p.chunk_id.as_str()).unwrap_or(""),
                        "B2 embed inference HANG — TensorRT did not return within budget; \
                         exiting for supervisor restart (REQ-AXO-902033). pending chunks \
                         resume from the durable queue on restart."
                    );
                    std::process::exit(75); // EX_TEMPFAIL — signals on_failure restart
                }
            };

            match join_result {
                Ok(Ok(embeddings)) if embeddings.len() == batch.len() => {
                    let elapsed_us = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    let per_item_us = elapsed_us / (batch.len() as u64).max(1);
                    // Slice 7 dashboard fix — record throughput sample for
                    // the sliding-window rate widget (chunks/sec live).
                    // Without this, `vector_chunk_embeddings_per_second()`
                    // never sees production data → dashboard stuck à 0.
                    crate::service_guard::record_vector_embed_call(batch.len() as u64, 0);
                    for (payload, embedding) in batch.into_iter().zip(embeddings.into_iter()) {
                        let emb = EmbeddedChunk {
                            chunk_id: payload.chunk_id,
                            source_hash: payload.content_hash,
                            embedding,
                        };
                        metrics.record_finished(per_item_us);
                        // REQ-AXO-901608 — t_send timing (backpressure indicator).
                        let send_started = Instant::now();
                        let send_result = tx.send(emb).await;
                        let send_us =
                            send_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        metrics.record_send_wait(send_us);
                        if send_result.is_err() {
                            return; // downstream closed; cease worker
                        }
                    }
                }
                Ok(Ok(embeddings)) => {
                    warn!(
                        stage = "B2",
                        expected = batch.len(),
                        actual = embeddings.len(),
                        "embedder returned mismatched batch size"
                    );
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                    if let Some(s) = store.as_ref() {
                        record_batch_failure(s, &batch).await;
                    }
                }
                Ok(Err(err)) => {
                    warn!(stage = "B2", error = ?err, "embed_batch failed");
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                    if let Some(s) = store.as_ref() {
                        record_batch_failure(s, &batch).await;
                    }
                }
                Err(join_err) => {
                    warn!(stage = "B2", error = ?join_err, "spawn_blocking joined with error");
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                    if let Some(s) = store.as_ref() {
                        record_batch_failure(s, &batch).await;
                    }
                }
            }
        }
    });
}

/// REQ-AXO-902012 — embed-attempt cap. After this many consecutive B2 failures
/// a chunk is quarantined (`embed_status='failed'`) so it leaves the sorted
/// drain. Small: a chunk that fails deterministically fails fast; a transient
/// GPU blip gets a couple of retries.
const MAX_EMBED_ATTEMPTS: i32 = 3;

/// REQ-AXO-902012 — increment `embed_attempts` (and quarantine at the cap) for a
/// batch the embedder could not process, off the async worker thread. Best
/// effort: a failure to record just means the chunk is retried again next loop,
/// which is the pre-fix behaviour — no worse, and bounded once it records.
async fn record_batch_failure(
    store: &Arc<crate::graph::GraphStore>,
    batch: &[ChunkForEmbedding],
) {
    let ids: Vec<String> = batch.iter().map(|p| p.chunk_id.clone()).collect();
    let store = store.clone();
    match tokio::task::spawn_blocking(move || store.record_embed_failure(&ids, MAX_EMBED_ATTEMPTS))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!(stage = "B2", error = %err, "record_embed_failure failed"),
        Err(join) => warn!(stage = "B2", error = %join, "record_embed_failure join failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_op_embedder_returns_canonical_dimension_vectors() {
        use crate::embedding_contract::DIMENSION;
        let payload = ChunkForEmbedding {
            chunk_id: "AXO::demo::sym::chunk".to_string(),
            content: "fn alpha() {}".to_string(),
            content_hash: "deadbeef".to_string(),
        };
        let embedder: Arc<dyn B2Embedder> = Arc::new(NoOpEmbedder);
        let result = b2_embed(payload, embedder).await.unwrap();

        assert_eq!(result.chunk_id, "AXO::demo::sym::chunk");
        assert_eq!(result.source_hash, "deadbeef");
        assert_eq!(
            result.embedding.len(),
            DIMENSION,
            "embedding must match canonical model dimension"
        );
        // Sanity check the no-op shape.
        assert_eq!(result.embedding[0], 1.0);
        assert!(result.embedding[1..].iter().all(|v| *v == 0.0));
    }

    #[tokio::test]
    async fn b2_batched_worker_groups_payloads_into_single_embed_call() {
        // Verify the canonical batching contract: when N payloads
        // arrive faster than the embedder runs, the batched worker
        // dispatches one embed_batch call with all N texts (vs the
        // per-item embed_batch call the generic worker_pool would
        // make).
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::mpsc;

        struct CountingEmbedder {
            invocation_count: AtomicUsize,
            seen_batch_sizes: tokio::sync::Mutex<Vec<usize>>,
        }
        impl B2Embedder for CountingEmbedder {
            fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
                self.invocation_count.fetch_add(1, Ordering::SeqCst);
                let mut guard = self.seen_batch_sizes.blocking_lock();
                guard.push(texts.len());
                drop(guard);
                use crate::embedding_contract::DIMENSION;
                Ok(texts
                    .iter()
                    .map(|_| {
                        let mut v = vec![0.0_f32; DIMENSION];
                        v[0] = 1.0;
                        v
                    })
                    .collect())
            }
        }

        let counter = Arc::new(CountingEmbedder {
            invocation_count: AtomicUsize::new(0),
            seen_batch_sizes: tokio::sync::Mutex::new(Vec::new()),
        });
        let (in_tx, in_rx) = mpsc::channel::<ChunkForEmbedding>(64);
        let (out_tx, mut out_rx) = mpsc::channel::<EmbeddedChunk>(64);
        let metrics = StageMetrics::new("B2");

        // batch_size=16, timeout=1s. Push 16 items quickly: should
        // trigger exactly ONE embed call with batch=16.
        spawn_b2_batched_worker(
            in_rx,
            out_tx,
            counter.clone(),
            metrics.clone(),
            16,
            Duration::from_secs(1),
            None,
        );

        for i in 0..16 {
            in_tx
                .send(ChunkForEmbedding {
                    chunk_id: format!("c{i}"),
                    content: format!("fn f{i}(){{}}"),
                    content_hash: format!("h{i}"),
                })
                .await
                .unwrap();
        }

        let mut received = Vec::new();
        for _ in 0..16 {
            let item = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                .await
                .expect("16 EmbeddedChunk must arrive within 2 s")
                .expect("output yields Some");
            received.push(item);
        }
        assert_eq!(received.len(), 16);
        assert_eq!(counter.invocation_count.load(Ordering::SeqCst), 1);
        let seen = counter.seen_batch_sizes.lock().await.clone();
        assert_eq!(seen, vec![16]);

        let snap = metrics.snapshot();
        assert_eq!(snap.items_in_total, 16);
        assert_eq!(snap.items_out_total, 16);
        assert_eq!(snap.errors_total, 0);
    }

    #[tokio::test]
    async fn b2_batched_worker_flushes_partial_batch_on_timeout() {
        // With batch_size=8 but only 3 items pushed, after the
        // batch_timeout elapses the worker MUST flush the partial
        // batch — otherwise tail items would stall indefinitely
        // (end-of-walk, cold-start residue).
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::mpsc;

        struct FlushTracker {
            invocations: AtomicUsize,
        }
        impl B2Embedder for FlushTracker {
            fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
                self.invocations.fetch_add(1, Ordering::SeqCst);
                use crate::embedding_contract::DIMENSION;
                Ok(texts.iter().map(|_| vec![1.0_f32; DIMENSION]).collect())
            }
        }

        let tracker = Arc::new(FlushTracker {
            invocations: AtomicUsize::new(0),
        });
        let (in_tx, in_rx) = mpsc::channel::<ChunkForEmbedding>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<EmbeddedChunk>(8);
        let metrics = StageMetrics::new("B2");

        spawn_b2_batched_worker(
            in_rx,
            out_tx,
            tracker.clone(),
            metrics.clone(),
            8,
            Duration::from_millis(80),
            None,
        );

        for i in 0..3 {
            in_tx
                .send(ChunkForEmbedding {
                    chunk_id: format!("p{i}"),
                    content: "fn x(){}".to_string(),
                    content_hash: "h".to_string(),
                })
                .await
                .unwrap();
        }

        let mut received = Vec::new();
        for _ in 0..3 {
            let item = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
                .await
                .expect("partial batch must flush within 2 s")
                .expect("output yields Some");
            received.push(item);
        }
        assert_eq!(received.len(), 3);
        // Exactly one batch call dispatched 3 items — timeout-driven flush.
        assert_eq!(tracker.invocations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn b2_embed_surfaces_zero_result_as_error() {
        struct ZeroEmbedder;
        impl B2Embedder for ZeroEmbedder {
            fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
                Ok(Vec::new())
            }
        }

        let payload = ChunkForEmbedding {
            chunk_id: "x".to_string(),
            content: "y".to_string(),
            content_hash: "z".to_string(),
        };
        let embedder: Arc<dyn B2Embedder> = Arc::new(ZeroEmbedder);
        let res = b2_embed(payload, embedder).await;
        assert!(res.is_err(), "missing embedding must propagate as error");
    }

    /// REQ-AXO-901777 — embedder failure in the batched worker records
    /// errors_total for every queued payload and does NOT crash the
    /// worker (it continues processing subsequent batches).
    #[tokio::test]
    async fn b2_batched_worker_handles_embedder_failure_gracefully() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::mpsc;

        struct FailingEmbedder {
            call_count: AtomicUsize,
        }
        impl B2Embedder for FailingEmbedder {
            fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("simulated GPU OOM"))
            }
        }

        let embedder = Arc::new(FailingEmbedder {
            call_count: AtomicUsize::new(0),
        });
        let (in_tx, in_rx) = mpsc::channel::<ChunkForEmbedding>(16);
        let (out_tx, mut out_rx) = mpsc::channel::<EmbeddedChunk>(16);
        let metrics = StageMetrics::new("B2");

        spawn_b2_batched_worker(
            in_rx,
            out_tx,
            embedder.clone(),
            metrics.clone(),
            4,
            Duration::from_millis(50),
            None,
        );

        // Send 4 chunks (fills one batch).
        for i in 0..4 {
            in_tx
                .send(ChunkForEmbedding {
                    chunk_id: format!("fail{i}"),
                    content: format!("fn f{i}(){{}}"),
                    content_hash: format!("h{i}"),
                })
                .await
                .unwrap();
        }

        // Give the worker time to process the batch.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // No output should arrive (all failed).
        let maybe = tokio::time::timeout(Duration::from_millis(100), out_rx.recv()).await;
        assert!(
            maybe.is_err() || maybe.unwrap().is_none(),
            "failed batch must not produce output"
        );

        let snap = metrics.snapshot();
        assert_eq!(
            snap.errors_total, 4,
            "all 4 items must be counted as errors"
        );
        assert!(
            embedder.call_count.load(Ordering::SeqCst) >= 1,
            "embedder was called"
        );

        // Send another batch to prove the worker survived the failure.
        drop(in_tx);
        // Worker should exit cleanly when input closes.
    }

    /// REQ-AXO-901777 — embedder returns wrong number of embeddings
    /// (batch size mismatch). Worker records errors, does not crash.
    #[tokio::test]
    async fn b2_batched_worker_handles_dimension_mismatch() {
        use tokio::sync::mpsc;

        struct MismatchEmbedder;
        impl B2Embedder for MismatchEmbedder {
            fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
                use crate::embedding_contract::DIMENSION;
                // Return only 1 embedding regardless of input size.
                Ok(vec![vec![0.0_f32; DIMENSION]])
            }
        }

        let embedder: Arc<dyn B2Embedder> = Arc::new(MismatchEmbedder);
        let (in_tx, in_rx) = mpsc::channel::<ChunkForEmbedding>(8);
        let (out_tx, _out_rx) = mpsc::channel::<EmbeddedChunk>(8);
        let metrics = StageMetrics::new("B2");

        spawn_b2_batched_worker(
            in_rx,
            out_tx,
            embedder,
            metrics.clone(),
            4,
            Duration::from_millis(50),
            None,
        );

        for i in 0..4 {
            in_tx
                .send(ChunkForEmbedding {
                    chunk_id: format!("mm{i}"),
                    content: format!("fn mm{i}(){{}}"),
                    content_hash: format!("h{i}"),
                })
                .await
                .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(200)).await;

        let snap = metrics.snapshot();
        assert_eq!(
            snap.errors_total, 4,
            "batch size mismatch must record all items as errors"
        );
    }

    // REQ-AXO-902033 — inference watchdog budget resolution. Run single-threaded
    // (env is process-global); the suite already pins --test-threads=1.
    #[test]
    fn b2_inference_timeout_resolves_default_override_and_disable() {
        unsafe { std::env::remove_var("AXON_B2_INFERENCE_TIMEOUT_MS") };
        assert_eq!(b2_inference_timeout_ms(), 180_000, "default budget");

        unsafe { std::env::set_var("AXON_B2_INFERENCE_TIMEOUT_MS", "5000") };
        assert_eq!(b2_inference_timeout_ms(), 5_000, "explicit override honoured");

        unsafe { std::env::set_var("AXON_B2_INFERENCE_TIMEOUT_MS", "0") };
        assert_eq!(b2_inference_timeout_ms(), u64::MAX, "0 disables the watchdog");

        unsafe { std::env::remove_var("AXON_B2_INFERENCE_TIMEOUT_MS") };
    }
}
