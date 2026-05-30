//! Stage B1 — Triage + fetch for the GPU embedder (CPT-AXO-054).
//!
//! B1 receives `B1InboxItem` from A3 (Inline with content) or from the
//! demand-pull NOTIFY listener (FetchById, requires PG SELECT).
//! Inline items are forwarded directly — zero PG cost (REQ-AXO-901746).
//! FetchById items are batched and SELECTed from `public.Chunk`.
//!
//! Embedding dedup (REQ-AXO-901748): chunks whose `content_hash`
//! matches the existing `ChunkEmbedding` are skipped entirely.

use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use anyhow::Context;

use crate::graph::GraphStore;

/// Load all existing (chunk_id, source_hash) pairs from ChunkEmbedding
/// for hydrating the embedding dedup cache at boot.
pub fn load_embedding_dedup_cache(store: &GraphStore) -> Result<Arc<dashmap::DashMap<String, String>>> {
    let model_id = crate::embedding_contract::CHUNK_MODEL_ID;
    let safe = model_id.replace('\'', "''");
    let raw = store.query_json_writer(&format!(
        "SELECT chunk_id, source_hash FROM chunkembedding WHERE model_id = '{safe}'"
    ))?;
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let map = dashmap::DashMap::with_capacity(rows.len());
    for row in rows {
        if let (Some(cid), Some(hash)) = (
            row.first().and_then(|v| v.as_str()),
            row.get(1).and_then(|v| v.as_str()),
        ) {
            map.insert(cid.to_string(), hash.to_string());
        }
    }
    Ok(Arc::new(map))
}

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

/// Item on the B1 inbox channel — chunk_id pending embed.
///
/// Slice 4 SOTA — B1 inbox is NOTIFY-only : demand_pull_b LISTEN
/// emits `FetchById(chunk_id)` after PG `trg_chunk_notify_pending`
/// fires post-COMMIT on Chunk INSERT/UPDATE. The legacy `Inline(payload)`
/// fast-path from A3 (try_send) was removed — single wake-up path
/// reduces complexity (no silent drop mode, no dispatch branch).
#[derive(Debug, Clone)]
pub enum B1InboxItem {
    FetchById(String),
}

/// SELECT the chunk content for a given `chunk_id`.
///
/// Wraps the DB call inside [`tokio::task::spawn_blocking`] to keep the
/// tokio runtime responsive under multi-worker B1 contention. Returns
/// `Ok(None)` if the row was deleted between A3's `try_send` and B1's
/// read (race with a re-parse) — the worker drops the item silently and
/// moves on; the demand-pull DB pathway catches anything the channel
/// missed.
#[cfg(test)]
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
/// REQ-AXO-901748 — set of `(chunk_id, content_hash)` pairs that
/// already have a valid embedding in ChunkEmbedding. B1 skips Inline
/// items that match, avoiding redundant GPU work on re-indexation.
pub type EmbeddingDedupCache = Option<Arc<dashmap::DashMap<String, String>>>;

pub fn spawn_b1_batched_worker(
    rx: tokio::sync::mpsc::Receiver<B1InboxItem>,
    tx: tokio::sync::mpsc::Sender<ChunkForEmbedding>,
    store: Arc<crate::graph::GraphStore>,
    metrics: Arc<super::metrics::StageMetrics>,
    batch_size: usize,
    batch_timeout: std::time::Duration,
) {
    spawn_b1_batched_worker_with_dedup(rx, tx, store, metrics, batch_size, batch_timeout, None)
}

pub fn spawn_b1_batched_worker_with_dedup(
    mut rx: tokio::sync::mpsc::Receiver<B1InboxItem>,
    tx: tokio::sync::mpsc::Sender<ChunkForEmbedding>,
    store: Arc<crate::graph::GraphStore>,
    metrics: Arc<super::metrics::StageMetrics>,
    batch_size: usize,
    batch_timeout: std::time::Duration,
    embedding_cache: EmbeddingDedupCache,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        let mut _dedup_skipped: u64 = 0;
        loop {
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
            let mut batch: Vec<B1InboxItem> = Vec::with_capacity(batch_size);
            batch.push(first);

            let deadline = tokio::time::Instant::now() + batch_timeout;
            while batch.len() < batch_size {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
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
                        let recv_us = recv_started
                            .elapsed()
                            .as_micros()
                            .min(u128::from(u64::MAX)) as u64;
                        metrics.record_recv_wait(recv_us);
                        break;
                    }
                }
            }

            let mut fetch_ids: Vec<String> = Vec::new();
            for item in &batch {
                match item {
                    B1InboxItem::FetchById(id) => fetch_ids.push(id.clone()),
                }
            }
            // Slice 4 SOTA — embedding_cache dedup now runs against the
            // FetchById path only (Inline removed). The cache is still
            // honored : if the chunk already has a fresh embedding, B1
            // skips the SELECT entirely.
            if let Some(ref cache) = embedding_cache {
                fetch_ids.retain(|id| {
                    if cache.get(id).is_some() {
                        _dedup_skipped += 1;
                        false
                    } else {
                        true
                    }
                });
            }

            for _ in &fetch_ids {
                metrics.record_started();
            }
            let started = std::time::Instant::now();

            if !fetch_ids.is_empty() {
                let store_clone = store.clone();
                let ids_for_block = fetch_ids.clone();
                let join_result = tokio::task::spawn_blocking(move || {
                    store_clone.fetch_chunks_for_embedding_batch(&ids_for_block)
                })
                .await;

                match join_result {
                    Ok(Ok(fetched)) => {
                        let elapsed_us =
                            started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        let per_item_us = elapsed_us / (batch.len() as u64).max(1);
                        let fetched_len = fetched.len();
                        for (chunk_id, content, content_hash) in fetched {
                            metrics.record_finished(per_item_us);
                            let payload = ChunkForEmbedding {
                                chunk_id,
                                content,
                                content_hash,
                            };
                            let send_started = std::time::Instant::now();
                            let send_result = tx.send(payload).await;
                            let send_us =
                                send_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                            metrics.record_send_wait(send_us);
                            if send_result.is_err() {
                                return;
                            }
                        }
                        for _ in 0..(fetch_ids.len() - fetched_len) {
                            metrics.record_error();
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(
                            stage = "B1",
                            error = ?err,
                            "fetch_chunks_for_embedding_batch failed"
                        );
                        for _ in 0..fetch_ids.len() {
                            metrics.record_error();
                        }
                    }
                    Err(join_err) => {
                        tracing::warn!(
                            stage = "B1",
                            error = ?join_err,
                            "B1 batched fetch spawn_blocking joined with error"
                        );
                        for _ in 0..fetch_ids.len() {
                            metrics.record_error();
                        }
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
