//! Stage B3 — UPSERT ChunkEmbedding (CPT-AXO-054 session 19).
//!
//! B3 receives [`super::stage_b2::EmbeddedChunk`] payloads and persists
//! them via [`crate::graph::GraphStore::upsert_chunk_embedding_v2`]
//! (`ON CONFLICT (chunk_id, model_id) DO UPDATE`). The Chunk row B2
//! embedded was already written by A3, so B3 only touches
//! `ist.ChunkEmbedding`.
//!
//! B3 is the canonical write boundary for the vector lane — a successful
//! commit means the chunk is queryable via pgvector ANN search. Crash
//! between B2 and B3 = lost in RAM; cold-start poll DB (slice S4c)
//! catches the chunk on next boot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::warn;

use crate::embedder::lifecycle::process_state as embedder_state;
use crate::graph::GraphStore;

use super::metrics::StageMetrics;
use super::project_resolver::project_code_from_chunk_id;
use super::stage_b2::EmbeddedChunk;

/// Receipt emitted by B3 once the embedding committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedEmbedding {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedded_at_ms: i64,
}

/// UPSERT `embedded`'s embedding row into `ist.ChunkEmbedding`.
///
/// DEC-AXO-081 — `project_code` is extracted from the canonical
/// `chunk_id` prefix (`"{project_code}::path::name::chunk[::part-NN]"`).
/// Falls back to `embedded.fallback_project_code` (default `"AXO"`)
/// when the prefix is malformed — bench / tests rely on the
/// fallback so their hand-built chunk_ids stay accepted. The write
/// is wrapped in [`tokio::task::spawn_blocking`] so the synchronous
/// SQL dispatch does not stall the tokio runtime.
#[cfg(test)]
pub async fn b3_persist_embedding(
    embedded: EmbeddedChunk,
    store: Arc<GraphStore>,
) -> anyhow::Result<PersistedEmbedding> {
    let chunk_id = embedded.chunk_id.clone();
    let source_hash = embedded.source_hash.clone();
    let embedding = embedded.embedding;
    let now_ms = Utc::now().timestamp_millis();
    let project_code_str = project_code_from_chunk_id(&chunk_id)
        .unwrap_or("AXO")
        .to_string();

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

    // REQ-AXO-90009 Slice 1 — clear pending state AFTER the embedding
    // row is committed. Pre-commit would risk a half-state where the
    // chunk is "not pending" yet has no ChunkEmbedding row.
    embedder_state().mark_embedded(&chunk_id);

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
    metrics: Arc<StageMetrics>,
    batch_size: usize,
    batch_timeout: Duration,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        // REQ-AXO-295 — tick-based batching (see
        // stage_a3::spawn_a3_batched_worker for the canonical comment).
        let mut tick = tokio::time::interval(batch_timeout);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        let mut buffer: Vec<EmbeddedChunk> = Vec::with_capacity(batch_size);

        loop {
            // REQ-AXO-901608 — t_recv timing (starvation indicator).
            let recv_started = Instant::now();
            let flush_now = tokio::select! {
                biased;
                received = rx.recv() => {
                    let recv_us =
                        recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    metrics.record_recv_wait(recv_us);
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
                    let recv_us =
                        recv_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    metrics.record_recv_wait(recv_us);
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

            // DEC-AXO-081 — group items by project_code parsed from
            // each chunk_id (canonical prefix). Each
            // upsert_chunk_embedding_v2_batch call stamps a single
            // project_code, so the per-project subgroup is the
            // largest natural granularity.
            let mut groups: std::collections::BTreeMap<String, Vec<EmbeddedChunk>> =
                std::collections::BTreeMap::new();
            for embedded in batch {
                let code = project_code_from_chunk_id(&embedded.chunk_id)
                    .unwrap_or("AXO")
                    .to_string();
                groups.entry(code).or_default().push(embedded);
            }

            for (pc_str, group_batch) in groups {
                let items: Vec<(String, String, Vec<f32>, i64)> = group_batch
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
                let pc_for_block = pc_str.clone();
                // REQ-AXO-902014 — time THIS group's write (a shared `started`
                // inflated per_item_us for the 2nd+ project group).
                let group_started = Instant::now();
                let join_result = tokio::task::spawn_blocking(move || {
                    store_clone.upsert_chunk_embedding_v2_batch(&pc_for_block, &items)
                })
                .await;

                let group_len = group_batch.len();
                match join_result {
                    Ok(Ok(())) => {
                        // REQ-AXO-902047 — a clean persist resets the systemic
                        // failure latch so a transient blip never sticks the
                        // drain in backoff.
                        crate::pipeline::stage_health::b3_health().record_success();
                        // REQ-AXO-90009 Slice 1 — clear pending state for
                        // every chunk just committed. Batched UPSERT
                        // succeeded for the whole group atomically, so it
                        // is safe to drop all chunk_ids from the pending
                        // set in one pass.
                        let state = embedder_state();
                        let elapsed_us =
                            group_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                        let per_item_us = elapsed_us / (group_len as u64).max(1);
                        for embedded in group_batch {
                            state.mark_embedded(&embedded.chunk_id);
                            metrics.record_finished(per_item_us);
                            let receipt = PersistedEmbedding {
                                chunk_id: embedded.chunk_id,
                                source_hash: embedded.source_hash,
                                embedded_at_ms: now_ms,
                            };
                            // REQ-AXO-901608 — t_send timing (backpressure indicator).
                            let send_started = Instant::now();
                            let send_result = tx.send(receipt).await;
                            let send_us =
                                send_started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                            metrics.record_send_wait(send_us);
                            if send_result.is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Err(err)) => {
                        // REQ-AXO-902047 — capture the REAL error (anyhow
                        // alternate Display = full `caused by` chain incl the
                        // root PG message + SQLSTATE, no longer masked) into the
                        // process-global B3 health signal, deduped by signature.
                        // Throttle the WARN so a systemic failure (every batch)
                        // does not flood the log thousands of times — log the
                        // first and every 50th, with the running count.
                        let n = crate::pipeline::stage_health::b3_health()
                            .record_failure(format!("{err:#}"), now_ms);
                        if n == 1 || n % 50 == 0 {
                            warn!(
                                stage = "B3",
                                consecutive_failures = n,
                                error = format!("{err:#}"),
                                "upsert_chunk_embedding_v2_batch failed (B3 persist) — \
                                 see embedding_status / pipeline_health for the live signal"
                            );
                        }
                        for _ in 0..group_len {
                            metrics.record_error();
                        }
                    }
                    Err(join_err) => {
                        warn!(
                            stage = "B3",
                            error = ?join_err,
                            "spawn_blocking joined with error"
                        );
                        for _ in 0..group_len {
                            metrics.record_error();
                        }
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

    // REQ-AXO-901877 — see stage_b1: the GraphStore fixture + sync store
    // methods drive the PG plugin via an internal `block_on`, so the
    // single-shot B3 tests stay sync (`#[test]`) and drive only the async
    // `b3_persist_embedding` (itself `spawn_blocking`-based) through a local
    // current-thread runtime, avoiding the runtime-within-a-runtime panic.
    fn run_async<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

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
    #[test]
    fn b3_persists_chunk_embedding_after_a3_seeded_the_chunk_row() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_demo_target() { let q = 1; }\n";
        let chunk_ids = store
            .upsert_graph(
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

        let receipt = run_async(b3_persist_embedding(payload, store.clone())).unwrap();
        assert_eq!(receipt.chunk_id, cid);
        assert!(receipt.embedded_at_ms > 0);

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(n, 1, "B3 must persist exactly one ChunkEmbedding row");
    }

    #[test]
    fn b3_is_idempotent_on_repeated_persist_for_same_chunk_id() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_idem() {}\n";
        let chunk_ids = store
            .upsert_graph(
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

        run_async(b3_persist_embedding(mk_payload(), store.clone())).unwrap();
        run_async(b3_persist_embedding(mk_payload(), store.clone())).unwrap();

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(
            n, 1,
            "ON CONFLICT must keep exactly one row per (chunk_id, model_id)"
        );
    }

    /// REQ-AXO-901777 — B3 with wrong embedding dimension propagates
    /// the PG vector constraint error (not a panic or silent corruption).
    #[test]
    fn b3_wrong_dimension_embedding_returns_error() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_dim_test() {}\n";
        let chunk_ids = store
            .upsert_graph(
                "/tmp/b3_dim.rs",
                "AXO",
                body,
                "hash-b3dim",
                1_700_000_000_012,
                &[sym("b3_dim_test")],
                &[],
            )
            .unwrap();
        let cid = chunk_ids[0].clone();

        // Wrong dimension: 10 instead of DIMENSION (1024).
        let bad_embedding = vec![1.0_f32; 10];
        let payload = EmbeddedChunk {
            chunk_id: cid,
            source_hash: "hash-bad-dim".to_string(),
            embedding: bad_embedding,
        };

        let result = run_async(b3_persist_embedding(payload, store));
        assert!(
            result.is_err(),
            "wrong dimension must surface as a PG error, not silent success"
        );
    }

    /// REQ-AXO-901777 — B3 batched worker metrics record errors when
    /// the PG batch write fails, and the worker continues running.
    #[tokio::test]
    async fn b3_batched_worker_records_errors_on_write_failure() {
        use tokio::sync::mpsc;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (in_tx, in_rx) = mpsc::channel::<EmbeddedChunk>(8);
        let (out_tx, _out_rx) = mpsc::channel::<PersistedEmbedding>(8);
        let metrics = StageMetrics::new("B3");

        spawn_b3_batched_worker(
            in_rx,
            out_tx,
            store.clone(),
            metrics.clone(),
            4,
            Duration::from_millis(50),
        );

        // Seed 2 REAL Chunk rows: orphan chunk_ids are now deliberately SKIPPED
        // (not errored) by the `JOIN ist.Chunk` FK-race guard (REQ-AXO-901884),
        // so they can no longer drive this test. Instead force a GENUINE write
        // failure with a WRONG-dimension embedding (mirrors the single-row test
        // b3_wrong_dimension_embedding_returns_error): the pgvector write rejects
        // it → the batch errors → B3 records one error per chunk.
        let mut real_ids = Vec::new();
        for (i, (file, name)) in [
            ("/tmp/b3_err0.rs", "b3_err_fn0"),
            ("/tmp/b3_err1.rs", "b3_err_fn1"),
        ]
        .iter()
        .enumerate()
        {
            // upsert_graph drives the bulk_writer's own runtime via block_on;
            // calling it directly inside this #[tokio::test] panics ("runtime
            // within a runtime"), so seed on a blocking thread (no tokio context).
            let store_seed = store.clone();
            let file = file.to_string();
            let name = name.to_string();
            let ids = tokio::task::spawn_blocking(move || {
                store_seed
                    .upsert_graph(
                        &file,
                        "AXO",
                        &format!("fn {name}() {{}}\n"),
                        &format!("h-b3err-{i}"),
                        1_700_000_000_020 + i as i64,
                        &[sym(&name)],
                        &[],
                    )
                    .unwrap()
            })
            .await
            .unwrap();
            real_ids.push(ids[0].clone());
        }

        for cid in &real_ids {
            in_tx
                .send(EmbeddedChunk {
                    chunk_id: cid.clone(),
                    source_hash: "h".to_string(),
                    // Wrong dimension (10 != DIMENSION 1024) → PG vector error.
                    embedding: vec![0.0_f32; 10],
                })
                .await
                .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(in_tx);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let snap = metrics.snapshot();
        assert!(
            snap.errors_total >= 2,
            "B3 must record errors for chunks whose embedding write fails (got {})",
            snap.errors_total
        );
    }
}
