//! Stage B3 — UPSERT ChunkEmbedding (CPT-AXO-054 session 19).
//!
//! B3 receives [`super::stage_b2::EmbeddedChunk`] payloads and persists
//! them via [`crate::graph::GraphStore::upsert_chunk_embedding_v2`]
//! (`ON CONFLICT (chunk_id, model_id) DO UPDATE`). The Chunk row B2
//! embedded was already written by A3, so B3 only touches
//! `public.ChunkEmbedding`.
//!
//! B3 is the canonical write boundary for the vector lane — a successful
//! commit means the chunk is queryable via pgvector ANN search. Crash
//! between B2 and B3 = lost in RAM; cold-start poll DB (slice S4c)
//! catches the chunk on next boot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::warn;

use crate::graph::GraphStore;

use super::metrics::StageMetrics;
use super::stage_b2::EmbeddedChunk;

/// Receipt emitted by B3 once the embedding committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedEmbedding {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedded_at_ms: i64,
}

/// UPSERT `embedded`'s embedding row into `public.ChunkEmbedding`.
///
/// `project_code` is the canonical 3-letter code the indexer is rooted
/// at (CPT-AXO-053 single-project per indexer instance). The write is
/// wrapped in [`tokio::task::spawn_blocking`] so the synchronous SQL
/// dispatch does not stall the tokio runtime.
pub async fn b3_persist_embedding(
    embedded: EmbeddedChunk,
    store: Arc<GraphStore>,
    project_code: Arc<str>,
) -> Result<PersistedEmbedding> {
    let chunk_id = embedded.chunk_id.clone();
    let source_hash = embedded.source_hash.clone();
    let embedding = embedded.embedding;
    let now_ms = Utc::now().timestamp_millis();
    let project_code_str = project_code.to_string();

    let store_clone = store.clone();
    let chunk_id_for_block = chunk_id.clone();
    let source_hash_for_block = source_hash.clone();
    tokio::task::spawn_blocking(move || {
        store_clone.upsert_chunk_embedding_v2(
            &chunk_id_for_block,
            &project_code_str,
            &source_hash_for_block,
            &embedding,
            now_ms,
        )
    })
    .await??;

    Ok(PersistedEmbedding {
        chunk_id,
        source_hash,
        embedded_at_ms: now_ms,
    })
}

/// REQ-AXO-295 — Spawn the canonical batched B3 worker.
///
/// Same shape as [`super::stage_a3::spawn_a3_batched_worker`]:
/// accumulate [`EmbeddedChunk`] payloads up to `batch_size` or wait
/// `batch_timeout`, then UPSERT all rows in one
/// `GraphStore::upsert_chunk_embedding_v2_batch` call. Amortizes the
/// per-row pgvector HNSW contention paid by `spawn_stage_workers` +
/// `b3_persist_embedding`.
pub fn spawn_b3_batched_worker(
    mut rx: Receiver<EmbeddedChunk>,
    tx: Sender<PersistedEmbedding>,
    store: Arc<GraphStore>,
    project_code: Arc<str>,
    metrics: Arc<StageMetrics>,
    batch_size: usize,
    batch_timeout: Duration,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        // REQ-AXO-295 — tick-based batching (see
        // stage_a3::spawn_a3_batched_worker for the canonical comment).
        let mut tick = tokio::time::interval(batch_timeout);
        tick.tick().await;
        let mut buffer: Vec<EmbeddedChunk> = Vec::with_capacity(batch_size);

        loop {
            let flush_now = tokio::select! {
                biased;
                received = rx.recv() => {
                    match received {
                        Some(item) => {
                            buffer.push(item);
                            buffer.len() >= batch_size
                        }
                        None => {
                            if buffer.is_empty() {
                                return;
                            }
                            true
                        }
                    }
                }
                _ = tick.tick() => {
                    !buffer.is_empty()
                }
            };

            if !flush_now {
                continue;
            }

            let upstream_closed_after_drain = rx.is_closed() && buffer.len() < batch_size;
            let batch: Vec<EmbeddedChunk> = std::mem::take(&mut buffer);
            for _ in &batch {
                metrics.record_started();
            }

            let now_ms = Utc::now().timestamp_millis();
            let items: Vec<(String, String, Vec<f32>, i64)> = batch
                .iter()
                .map(|e| {
                    (
                        e.chunk_id.clone(),
                        e.source_hash.clone(),
                        e.embedding.clone(),
                        now_ms,
                    )
                })
                .collect();

            let store_clone = store.clone();
            let pc_str = project_code.to_string();
            let started = Instant::now();
            let join_result = tokio::task::spawn_blocking(move || {
                store_clone.upsert_chunk_embedding_v2_batch(&pc_str, &items)
            })
            .await;

            match join_result {
                Ok(Ok(())) => {
                    let elapsed_us =
                        started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    let per_item_us = elapsed_us / (batch.len() as u64).max(1);
                    for embedded in batch {
                        metrics.record_finished(per_item_us);
                        let receipt = PersistedEmbedding {
                            chunk_id: embedded.chunk_id,
                            source_hash: embedded.source_hash,
                            embedded_at_ms: now_ms,
                        };
                        if tx.send(receipt).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(Err(err)) => {
                    warn!(stage = "B3", error = ?err, "upsert_chunk_embedding_v2_batch failed");
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                }
                Err(join_err) => {
                    warn!(
                        stage = "B3",
                        error = ?join_err,
                        "spawn_blocking joined with error"
                    );
                    for _ in 0..batch.len() {
                        metrics.record_error();
                    }
                }
            }

            if upstream_closed_after_drain {
                return;
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

    /// Seed a Chunk row via A3, then have B3 persist a no-op embedding
    /// for it. ChunkEmbedding row must exist after the UPSERT.
    #[tokio::test]
    async fn b3_persists_chunk_embedding_after_a3_seeded_the_chunk_row() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_demo_target() { let q = 1; }\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b3_demo.rs",
                "AXO",
                body,
                "hash-b3",
                1_700_000_000_010,
                &[sym("b3_demo_target")],
                &[],
            )
            .unwrap();
        assert!(!chunk_ids.is_empty());

        let embedding = {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            v
        };
        let cid = chunk_ids[0].clone();
        let payload = EmbeddedChunk {
            chunk_id: cid.clone(),
            source_hash: "hash-b3-chunk".to_string(),
            embedding,
        };

        let receipt = b3_persist_embedding(payload, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        assert_eq!(receipt.chunk_id, cid);
        assert!(receipt.embedded_at_ms > 0);

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(n, 1, "B3 must persist exactly one ChunkEmbedding row");
    }

    #[tokio::test]
    async fn b3_is_idempotent_on_repeated_persist_for_same_chunk_id() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_idem() {}\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b3_idem.rs",
                "AXO",
                body,
                "hash-b3i",
                1_700_000_000_011,
                &[sym("b3_idem")],
                &[],
            )
            .unwrap();
        let cid = chunk_ids[0].clone();

        let mk_payload = || -> EmbeddedChunk {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            EmbeddedChunk {
                chunk_id: cid.clone(),
                source_hash: "hash-b3i-chunk".to_string(),
                embedding: v,
            }
        };

        b3_persist_embedding(mk_payload(), store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        b3_persist_embedding(mk_payload(), store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(n, 1, "ON CONFLICT must keep exactly one row per (chunk_id, model_id)");
    }
}
