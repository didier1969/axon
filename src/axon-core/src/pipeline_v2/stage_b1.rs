//! Stage B1 — Fetch chunk content for the GPU embedder (CPT-AXO-054, session 19 topology).
//!
//! B1 is the **entry stage of pipeline B** (vectorisation lane only —
//! lexical/FTS is handled CPU-side by A3's Chunk INSERT). It receives a
//! `chunk_id: String` from A3's `try_send` fan-out (or from the cold-start
//! poll DB pathway, slice S4c), SELECTs the chunk's text content from
//! `public.Chunk`, and forwards `(chunk_id, content, content_hash)` to
//! the B2 embedder.
//!
//! **No tree-sitter, no chunking here.** A3 already derived the chunks
//! and UPSERTed them with `content_tsv` GENERATED for FTS. B1 is a pure
//! DB-read + bucketing stage, GPU-driven by B2's pace through the
//! downstream channel.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc::Sender;

use crate::graph::GraphStore;

/// Output of stage B1 — the payload B2 (GPU embedder) consumes.
///
/// `chunk_id` is the PK in `public.Chunk`. `content` is the raw text to
/// embed (capped at the model's tokenizer max len upstream of B2 via the
/// seq-len bucketing — REQ-AXO-262). `content_hash` lets B3 dedup
/// embeddings against the same source revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkForEmbedding {
    pub chunk_id: String,
    pub content: String,
    pub content_hash: String,
}

/// SELECT the chunk content for a given `chunk_id`.
///
/// Wraps the DB call inside [`tokio::task::spawn_blocking`] to keep the
/// tokio runtime responsive under multi-worker B1 contention. Returns
/// `Ok(None)` if the row was deleted between A3's `try_send` and B1's
/// read (race with a re-parse) — the worker drops the item silently and
/// moves on; the cold-start poll DB pathway (slice S4c) catches anything
/// the channel missed.
/// REQ-AXO-289 S4c — B1 cold-start poll DB pathway.
///
/// Drains every chunk that exists in `public.Chunk` but has no
/// matching `ChunkEmbedding` row for the canonical model, sending the
/// chunk_ids into the same `b1_inbox` channel that A3 try_sends to in
/// steady-state. Idempotent — re-running yields zero chunks once all
/// chunks have an embedding.
///
/// `batch_size` caps each SQL round-trip (default 256, configurable
/// via `AXON_B1_COLDSTART_BATCH_SIZE`). The loop keeps issuing batches
/// until the SELECT returns less than `batch_size` rows, which means
/// the table caught up.
///
/// Returns the total number of chunk_ids forwarded to the inbox.
/// Forwarding uses `send().await` (blocking) — the cold-start runs at
/// boot when no other producers are racing for the inbox, so blocking
/// is safe; if the inbox is closed mid-poll, we propagate the error.
pub async fn b1_cold_start_poll(
    store: Arc<GraphStore>,
    b1_inbox_tx: Sender<String>,
    batch_size: usize,
) -> Result<usize> {
    if batch_size == 0 {
        return Ok(0);
    }
    let mut total = 0usize;
    loop {
        let store_clone = store.clone();
        let batch = tokio::task::spawn_blocking(move || {
            store_clone.select_chunks_needing_embedding(batch_size)
        })
        .await
        .context("B1 cold-start poll task panicked")??;
        let drained = batch.len();
        if drained == 0 {
            break;
        }
        for cid in batch {
            b1_inbox_tx
                .send(cid)
                .await
                .map_err(|e| anyhow::anyhow!("B1 cold-start: inbox closed mid-send ({e})"))?;
            total += 1;
        }
        if drained < batch_size {
            break;
        }
    }
    Ok(total)
}

pub async fn b1_fetch_for_embedding(
    chunk_id: String,
    store: Arc<GraphStore>,
) -> Result<Option<ChunkForEmbedding>> {
    let store_clone = store.clone();
    let id_for_block = chunk_id.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        store_clone.fetch_chunk_for_embedding(&id_for_block)
    })
    .await
    .context("B1 fetch task panicked or was cancelled")??;

    Ok(fetched.map(|(content, content_hash)| ChunkForEmbedding {
        chunk_id,
        content,
        content_hash,
    }))
}

/// REQ-AXO-314 batched B1 — accumulates chunk_ids from `rx` up to
/// `batch_size` or `batch_timeout`, then issues ONE batched SELECT
/// (`WHERE id IN (...)`) and forwards each [`ChunkForEmbedding`] to B2.
///
/// Replaces the generic-per-item `spawn_stage_workers` wiring for B1.
/// Mirrors [`super::stage_b2::spawn_b2_batched_worker`] /
/// [`super::stage_b3::spawn_b3_batched_worker`] in shape so the three
/// pipeline-B stages now share one batched-workload pattern.
///
/// Throughput rationale (CPT-AXO-054):
/// * Per-item B1 caps at ~4 × (1 / SELECT-latency) ≈ 50-80 ch/s on PG
///   under deadpool contention, leaving B2's GPU batch=64 chronically
///   under-filled (B2 timeouts flush partial batches → BGE-Large
///   under-utilized).
/// * Batched B1 issues one SELECT per 64 chunk_ids → matches B2's
///   batch granularity → GPU runs at peak.
pub fn spawn_b1_batched_worker(
    mut rx: tokio::sync::mpsc::Receiver<String>,
    tx: tokio::sync::mpsc::Sender<ChunkForEmbedding>,
    store: Arc<crate::graph::GraphStore>,
    metrics: Arc<super::metrics::StageMetrics>,
    batch_size: usize,
    batch_timeout: std::time::Duration,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        loop {
            // REQ-AXO-901608 — t_recv timing (starvation indicator).
            let recv_started = std::time::Instant::now();
            let first = match rx.recv().await {
                Some(item) => {
                    let recv_us =
                        recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    metrics.record_recv_wait(recv_us);
                    item
                }
                None => break,
            };
            let mut batch: Vec<String> = Vec::with_capacity(batch_size);
            batch.push(first);

            let deadline = tokio::time::Instant::now() + batch_timeout;
            while batch.len() < batch_size {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                // REQ-AXO-901608 — accumulate intra-batch recv wait too.
                let recv_started = std::time::Instant::now();
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Some(item)) => {
                        let recv_us = recv_started
                            .elapsed()
                            .as_micros()
                            .min(u128::from(u64::MAX)) as u64;
                        metrics.record_recv_wait(recv_us);
                        batch.push(item);
                    }
                    Ok(None) => break,
                    Err(_) => {
                        // Timeout means upstream is starved relative to
                        // batch_timeout — count this too.
                        let recv_us = recv_started
                            .elapsed()
                            .as_micros()
                            .min(u128::from(u64::MAX)) as u64;
                        metrics.record_recv_wait(recv_us);
                        break;
                    }
                }
            }

            for _ in &batch {
                metrics.record_started();
            }
            let started = std::time::Instant::now();
            let store_clone = store.clone();
            let batch_for_block = batch.clone();
            let join_result = tokio::task::spawn_blocking(move || {
                store_clone.fetch_chunks_for_embedding_batch(&batch_for_block)
            })
            .await;

            match join_result {
                Ok(Ok(fetched)) => {
                    let elapsed_us =
                        started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    let per_item_us = elapsed_us / (batch.len() as u64).max(1);
                    // Missing chunk_ids (race with re-parse) count as
                    // errors so the metric stays comparable with the
                    // per-item path; B2 won't see them.
                    let fetched_len = fetched.len();
                    for (chunk_id, content, content_hash) in fetched {
                        metrics.record_finished(per_item_us);
                        let payload = ChunkForEmbedding {
                            chunk_id,
                            content,
                            content_hash,
                        };
                        // REQ-AXO-901608 — t_send timing (backpressure indicator).
                        let send_started = std::time::Instant::now();
                        let send_result = tx.send(payload).await;
                        let send_us =
                            send_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        metrics.record_send_wait(send_us);
                        if send_result.is_err() {
                            return; // downstream closed
                        }
                    }
                    for _ in 0..(batch.len() - fetched_len) {
                        metrics.record_error();
                    }
                }
                Ok(Err(err)) => {
                    tracing::warn!(
                        stage = "B1",
                        error = ?err,
                        "fetch_chunks_for_embedding_batch failed"
                    );
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                }
                Err(join_err) => {
                    tracing::warn!(
                        stage = "B1",
                        error = ?join_err,
                        "B1 batched fetch spawn_blocking joined with error"
                    );
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Symbol;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: "function".into(),
            start_line: 1,
            end_line: 2,
            docstring: None,
            is_entry_point: false,
            is_public: false,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        }
    }

    /// Seed Chunk rows via A3's canonical path, then verify B1 can
    /// SELECT them back with content intact.
    #[tokio::test]
    async fn b1_fetches_chunk_content_after_a3_upsert() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn marker_for_b1_fetch_test() { let x = 1; }\n";

        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_fetch.rs",
                "AXO",
                body,
                "hash-fetch",
                1_700_000_000_000,
                &[sym("marker_for_b1_fetch_test")],
                &[],
            )
            .unwrap();
        assert!(!chunk_ids.is_empty(), "A3 must emit at least one chunk_id");

        let cid = chunk_ids[0].clone();
        let fetched = b1_fetch_for_embedding(cid.clone(), store.clone())
            .await
            .unwrap()
            .expect("Chunk row must exist after A3 UPSERT");

        assert_eq!(fetched.chunk_id, cid);
        assert!(
            !fetched.content.is_empty(),
            "content must be non-empty for a Rust fn fixture"
        );
        assert!(
            !fetched.content_hash.is_empty(),
            "content_hash must be populated by A3"
        );
    }

    #[tokio::test]
    async fn b1_returns_none_for_missing_chunk_id() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let res = b1_fetch_for_embedding("AXO::nonexistent::path::sym::chunk".to_string(), store)
            .await
            .unwrap();
        assert!(res.is_none(), "B1 must return None for unknown chunk_id");
    }

    #[tokio::test]
    async fn b1_cold_start_poll_emits_chunks_without_embeddings() {
        use tokio::sync::mpsc;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_poll.rs",
                "AXO",
                body,
                "hash-poll",
                1_700_000_000_020,
                &[sym("alpha"), sym("beta"), sym("gamma")],
                &[],
            )
            .unwrap();
        assert!(chunk_ids.len() >= 3);

        // None of these chunk_ids has an embedding yet — cold-start
        // poll must surface all of them.
        let (tx, mut rx) = mpsc::channel::<String>(128);
        let total = b1_cold_start_poll(store.clone(), tx, 32).await.unwrap();
        assert!(total >= chunk_ids.len() as usize);

        let mut emitted: Vec<String> = Vec::new();
        while let Ok(Some(cid)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
        {
            emitted.push(cid);
        }
        for cid in &chunk_ids {
            assert!(
                emitted.contains(cid),
                "cold-start poll must emit chunk_id {cid}"
            );
        }
    }

    #[tokio::test]
    async fn b1_cold_start_poll_skips_already_embedded_chunks() {
        use crate::embedding_contract::DIMENSION;
        use tokio::sync::mpsc;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn standalone() {}\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_skip.rs",
                "AXO",
                body,
                "hash-skip",
                1_700_000_000_021,
                &[sym("standalone")],
                &[],
            )
            .unwrap();
        let cid = chunk_ids[0].clone();

        // Pre-embed the chunk.
        let mut embedding = vec![0.0_f32; DIMENSION];
        embedding[0] = 1.0;
        store
            .upsert_chunk_embedding_v2(&cid, "AXO", "hash-skip-chunk", &embedding, 1)
            .unwrap();

        let (tx, mut rx) = mpsc::channel::<String>(8);
        let total = b1_cold_start_poll(store.clone(), tx, 8).await.unwrap();

        // The pre-embedded chunk must not surface.
        let mut emitted: Vec<String> = Vec::new();
        while let Ok(Some(cid)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
        {
            emitted.push(cid);
        }
        assert!(
            !emitted.contains(&cid),
            "already-embedded chunk_id must NOT surface in cold-start poll"
        );
        assert_eq!(
            total as usize, emitted.len(),
            "poll counter must match emitted count"
        );
    }

    #[tokio::test]
    async fn a3_returns_chunk_ids_that_b1_can_round_trip() {
        // Session-19 contract: A3 returns Vec<String> chunk_ids; every
        // returned id must be addressable by B1 from PG. This locks the
        // try_send fan-out contract A3 → B1 inbox.
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn one() {}\nfn two() {}\nfn three() {}\n";

        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_roundtrip.rs",
                "AXO",
                body,
                "hash-rt",
                1_700_000_000_001,
                &[sym("one"), sym("two"), sym("three")],
                &[],
            )
            .unwrap();
        assert!(
            chunk_ids.len() >= 3,
            "expected ≥3 chunk_ids from 3 symbols (saw {})",
            chunk_ids.len()
        );

        for cid in chunk_ids {
            let fetched = b1_fetch_for_embedding(cid.clone(), store.clone())
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("chunk_id {cid} from A3 must round-trip via B1"));
            assert_eq!(fetched.chunk_id, cid);
        }
    }
}
