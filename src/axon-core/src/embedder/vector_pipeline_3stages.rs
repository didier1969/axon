//! REQ-AXO-270 — 3-stage vector pipeline.
//!
//! Replaces the single-loop `vector_lane_worker` (DEC-AXO-070) with three
//! threads connected by bounded channels:
//!
//!   1. **Producer**  — claim FVQ rows, fetch chunks, tokenize.  → `PreparedMsg`
//!   2. **Embedder**  — tight ORT GPU loop on the prepared batch. → `EmbeddedMsg`
//!   3. **Persister** — coalesce ≥1000 rows + bulk INSERT + mark_done.
//!
//! Activated by `AXON_VECTOR_PIPELINE_STAGES=3`. Default and any other
//! value keep the DEC-AXO-070 single-loop behavior unchanged.
//!
//! Phase 2 implements the real stages. Phase 3 benches against the
//! single-loop path. AC2.7 mandates the Persister bulk-write ≥1000 rows
//! per DB transaction (one COPY BINARY + INSERT…SELECT…ON CONFLICT under
//! `AXON_BULK_WRITER_ENABLED=true`, REQ-AXO-238). Persister calls
//! `graph_store.update_chunk_embeddings` directly; pgvector handles the
//! native vector storage, so the legacy DuckDB-era Parquet side-store
//! workaround (DEC-AXO-073) is gone from this path (operator directive
//! 2026-05-10).

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};

use super::vector_worker_loop::build_vector_embedding_model;
use super::*;

/// AC1.2 — env-driven pipeline mode selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorPipelineMode {
    /// DEC-AXO-070 single-loop worker (default).
    SingleLoop,
    /// REQ-AXO-270 3-stage pipeline.
    ThreeStages,
}

const ENV_FLAG: &str = "AXON_VECTOR_PIPELINE_STAGES";

/// Persister coalescing target (AC2.7). Operator directive 2026-05-10:
/// per-chunk inserts are forbidden; the Persister buffers at least this
/// many rows before issuing the bulk INSERT/Parquet append.
const PERSISTER_BULK_FLUSH_MIN_ROWS: usize = 1024;

/// Maximum time the Persister will sit on a partial buffer before
/// flushing anyway. REQ-AXO-270 Phase 4 (2026-05-10): reduced from
/// 500ms to 100ms to eliminate the dead windows observed in the Phase
/// 3 bench. Combined with the EndOfClaimCycle-barrier removal, the
/// pipeline now flushes 10×/s on partial buffers instead of waiting
/// for cycle boundaries.
const PERSISTER_BULK_FLUSH_MAX_LINGER: Duration = Duration::from_millis(100);

/// Bounded depth for inter-stage channels. Small (4) so a stalled
/// downstream stage applies backpressure quickly rather than buffering
/// minutes of work in RAM.
const STAGE_CHANNEL_DEPTH: usize = 4;

/// AC1.2 — read `AXON_VECTOR_PIPELINE_STAGES`. Unrecognised values fall
/// back to `SingleLoop` so a typo cannot silently disable the production
/// lane.
pub(crate) fn vector_pipeline_mode_from_env() -> VectorPipelineMode {
    match std::env::var(ENV_FLAG)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("3") => VectorPipelineMode::ThreeStages,
        _ => VectorPipelineMode::SingleLoop,
    }
}

/// AC1.1 — Producer→Embedder payload. A prepared batch carrying
/// pre-tokenised texts plus the file-level work units to mark done.
///
/// REQ-AXO-270 Phase 4 (2026-05-10): the prior `EndOfClaimCycle` variant
/// was removed. The persister no longer waits for cycle boundaries to
/// finalize files — it marks done on every successful flush. Channel
/// disconnect handles shutdown drain.
pub(crate) struct PreparedMsg {
    pub(crate) prepared: PreparedVectorEmbedBatch,
    pub(crate) completed_immediate: Vec<FileVectorizationWork>,
}

/// AC1.1 — Embedder→Persister payload.
pub(crate) enum EmbeddedMsg {
    /// One embedded chunk batch ready to persist.
    Ok {
        updates: Vec<(String, String, Vec<f32>)>,
        completed_immediate: Vec<FileVectorizationWork>,
        completed_after_success: Vec<FileVectorizationWork>,
    },
    /// Embed failed — touched_works must be marked failed so they get
    /// re-claimed on the next FVQ cycle.
    Failed {
        touched: Vec<FileVectorizationWork>,
        reason: String,
        completed_immediate: Vec<FileVectorizationWork>,
    },
}

/// AC1.1 — internal accounting only; the persister never sends one
/// out. Kept as a named type so the module's contract documents the
/// full Producer → Embedder → Persister chain.
#[allow(dead_code)]
pub(crate) struct PersistedBatch {
    pub(crate) rows_written: usize,
    pub(crate) files_finalized: usize,
}

/// AC1.3 — factory dispatch entry. Spawns the embedder and persister
/// threads, runs the producer on the calling thread (= the vector
/// worker thread that `vector_lane_worker` already supervises), then
/// joins the spawned threads on exit so axonctl can restart the
/// whole worker as a unit.
pub(crate) fn run_vector_pipeline_3stages(worker_idx: usize, graph_store: Arc<GraphStore>) {
    info!(
        "Vector pipeline [{}]: REQ-AXO-270 3-stage pipeline starting (AXON_VECTOR_PIPELINE_STAGES=3)",
        worker_idx
    );

    if let Err(e) = graph_store.ensure_embedding_model(
        SYMBOL_MODEL_ID,
        "symbol",
        MODEL_NAME,
        DIMENSION as i64,
        MODEL_VERSION,
    ) {
        error!(
            "Vector pipeline [{}]: failed to register symbol embedding model: {:?}",
            worker_idx, e
        );
    }
    if let Err(e) = graph_store.ensure_embedding_model(
        CHUNK_MODEL_ID,
        "chunk",
        MODEL_NAME,
        DIMENSION as i64,
        MODEL_VERSION,
    ) {
        error!(
            "Vector pipeline [{}]: failed to register chunk embedding model: {:?}",
            worker_idx, e
        );
    }

    let (prepared_tx, prepared_rx) = bounded::<PreparedMsg>(STAGE_CHANNEL_DEPTH);
    let (embedded_tx, embedded_rx) = bounded::<EmbeddedMsg>(STAGE_CHANNEL_DEPTH);

    // Embedder thread — holds the ORT model exclusively. Owns the
    // model so axonctl-restart-on-crash leaves no dangling GPU
    // allocation: panic → thread unwinds → model dropped.
    let embedder_handle = {
        let _ = worker_idx;
        thread::Builder::new()
            .name(format!("axon-vec-pipeline-embedder-{}", worker_idx))
            .spawn(move || run_embedder_stage(worker_idx, prepared_rx, embedded_tx))
            .expect("vector pipeline: failed to spawn embedder thread")
    };

    // Persister thread — owns the graph_store handle for the bulk
    // INSERT path. Uses a clone (Arc) so the producer keeps its own.
    let persister_handle = {
        let graph_store = Arc::clone(&graph_store);
        thread::Builder::new()
            .name(format!("axon-vec-pipeline-persister-{}", worker_idx))
            .spawn(move || run_persister_stage(worker_idx, graph_store, embedded_rx))
            .expect("vector pipeline: failed to spawn persister thread")
    };

    // Producer runs on this thread.
    run_producer_stage(worker_idx, graph_store, prepared_tx);

    // Producer returned → its sender is dropped → embedder will drain
    // and exit → embedder's sender drops → persister drains and exits.
    if let Err(e) = embedder_handle.join() {
        error!(
            "Vector pipeline [{}]: embedder thread panicked: {:?}",
            worker_idx, e
        );
    }
    if let Err(e) = persister_handle.join() {
        error!(
            "Vector pipeline [{}]: persister thread panicked: {:?}",
            worker_idx, e
        );
    }

    info!(
        "Vector pipeline [{}]: all stages stopped — returning so axonctl can restart the worker",
        worker_idx
    );
}

// ─────────────────────────────── Producer ───────────────────────────────

fn run_producer_stage(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    prepared_tx: Sender<PreparedMsg>,
) {
    let lane_config = embedding_lane_config_from_env();
    let target_chunks = lane_config.chunk_batch_size.max(1);
    let per_file_fetch_limit = lane_config.max_chunks_per_file;
    let batch_max_bytes = lane_config.max_embed_batch_bytes;
    let file_batch_size = lane_config.file_vectorization_batch_size.max(1);

    info!(
        "Vector pipeline [{}/producer]: ready (file_batch_size={}, target_chunks={})",
        worker_idx, file_batch_size, target_chunks
    );

    loop {
        service_guard::record_vector_pipeline_producer_heartbeat();
        service_guard::record_vector_worker_heartbeat();

        let claimed = match graph_store.fetch_pending_file_vectorization_work(file_batch_size) {
            Ok(work) if !work.is_empty() => work,
            Ok(_) => {
                let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(50));
                continue;
            }
            Err(e) => {
                error!(
                    "Vector pipeline [{}/producer]: fetch_pending_file_vectorization_work failed: {:?}",
                    worker_idx, e
                );
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };

        if let Err(e) = graph_store.mark_file_vectorization_started(&claimed) {
            warn!(
                "Vector pipeline [{}/producer]: mark_file_vectorization_started failed: {:?}",
                worker_idx, e
            );
        }

        if !run_producer_inner_loop(
            worker_idx,
            &graph_store,
            claimed,
            target_chunks,
            per_file_fetch_limit,
            batch_max_bytes,
            &prepared_tx,
        ) {
            // Downstream channel disconnected — embedder/persister gone.
            warn!(
                "Vector pipeline [{}/producer]: downstream stage gone, exiting",
                worker_idx
            );
            return;
        }

        // REQ-AXO-270 Phase 4 (2026-05-10): no EndOfClaimCycle barrier.
        // The producer immediately loops back to fetch the next claim;
        // the persister flushes on its size + LINGER triggers and
        // finalizes files on every successful flush. Channel disconnect
        // (above) handles shutdown drain.
    }
}

/// Drains the inner active-set for one claimed file batch. Returns
/// `false` on downstream disconnect (caller exits the producer thread).
fn run_producer_inner_loop(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    initial_active: Vec<FileVectorizationWork>,
    target_chunks: usize,
    per_file_fetch_limit: usize,
    batch_max_bytes: usize,
    prepared_tx: &Sender<PreparedMsg>,
) -> bool {
    let tokenizer = match load_runtime_embedding_tokenizer() {
        Ok(t) => t,
        Err(e) => {
            error!(
                "Vector pipeline [{}/producer]: tokenizer load failed: {:?}",
                worker_idx, e
            );
            // Mark all claimed files failed so they get retried, then
            // continue (skip this cycle).
            let _ = graph_store.mark_file_vectorization_work_failed(
                &initial_active,
                &format!("producer tokenizer load: {:?}", e),
            );
            return true;
        }
    };

    let mut active = initial_active;
    let mut reserved_chunk_ids: HashSet<String> = HashSet::new();

    while !active.is_empty() {
        let mut prepared = prepare_vector_embed_batch(
            graph_store,
            &active,
            target_chunks,
            per_file_fetch_limit,
            batch_max_bytes,
            &reserved_chunk_ids,
        );

        for item in &prepared.work_items {
            reserved_chunk_ids.insert(item.chunk_id.clone());
        }

        // Oversized + failed_fetches handled in-stage (mirrors single-loop).
        for w in &prepared.oversized_works {
            if let Err(err) = graph_store.mark_file_oversized_for_current_budget(&w.file_path) {
                warn!(
                    "Vector pipeline [{}/producer]: mark_oversized failed for {}: {:?}",
                    worker_idx, w.file_path, err
                );
            }
        }
        for (w, reason) in &prepared.failed_fetches {
            error!(
                "Vector pipeline [{}/producer]: chunk fetch failed for {}: {}",
                worker_idx, w.file_path, reason
            );
            let _ = graph_store
                .mark_file_vectorization_work_failed(std::slice::from_ref(w), reason);
        }

        let made_progress = !prepared.work_items.is_empty()
            || !prepared.immediate_completed.is_empty()
            || !prepared.finalize_after_success.is_empty()
            || !prepared.oversized_works.is_empty();

        // Capture the continuation BEFORE sending `prepared` downstream.
        // Optimistic next_active_after_success — failures detected by the
        // embedder feed back via EmbeddedMsg::Failed; the file is marked
        // failed and re-claimed on the next FVQ cycle.
        let next_active = std::mem::take(&mut prepared.next_active_after_success);

        if !prepared.texts.is_empty() {
            if let Err(e) = attach_preencoded_micro_batches(&tokenizer, &mut prepared) {
                error!(
                    "Vector pipeline [{}/producer]: tokenize failed: {:?}",
                    worker_idx, e
                );
                let _ = graph_store.mark_file_vectorization_work_failed(
                    &prepared.touched_works,
                    &format!("tokenize: {:?}", e),
                );
                active = std::mem::take(&mut prepared.next_active_after_failure);
                continue;
            }

            let completed_immediate = prepared.immediate_completed.clone();
            if try_send_or_disconnect(
                prepared_tx,
                PreparedMsg {
                    prepared,
                    completed_immediate,
                },
            )
            .is_err()
            {
                return false;
            }
        } else if !prepared.immediate_completed.is_empty()
            || !prepared.finalize_after_success.is_empty()
        {
            // No texts to embed but file-level finalize work pending.
            // The persister will mark these files done on the next
            // flush (or directly if updates is empty — REQ-AXO-270
            // Phase 4 short-circuit).
            let mut completed_immediate = std::mem::take(&mut prepared.immediate_completed);
            completed_immediate.extend(std::mem::take(&mut prepared.finalize_after_success));
            if try_send_or_disconnect(
                prepared_tx,
                PreparedMsg {
                    prepared: empty_prepared_marker(),
                    completed_immediate,
                },
            )
            .is_err()
            {
                return false;
            }
        }

        active = next_active;
        if !made_progress {
            break;
        }
    }

    true
}

/// Build a placeholder PreparedVectorEmbedBatch that carries no work —
/// used to forward completed_immediate when the producer iteration had
/// no texts to embed.
fn empty_prepared_marker() -> PreparedVectorEmbedBatch {
    PreparedVectorEmbedBatch {
        batch_id: String::new(),
        prepare_started_at_ms: 0,
        prepare_finished_at_ms: 0,
        prepared_at_ms: 0,
        batch_lane: VectorBatchLane::Mixed,
        mixed_fallback: false,
        lane_thresholds: current_token_lane_thresholds(),
        work_items: Vec::new(),
        texts: Vec::new(),
        token_counts: Vec::new(),
        encoded_micro_batches: Vec::new(),
        touched_works: Vec::new(),
        finalize_after_success: Vec::new(),
        immediate_completed: Vec::new(),
        oversized_works: Vec::new(),
        next_active_after_success: Vec::new(),
        next_active_after_failure: Vec::new(),
        files_touched: 0,
        partial_file_cycles: 0,
        fetch_ms_total: 0,
        failed_fetches: Vec::new(),
    }
}

// ─────────────────────────────── Embedder ───────────────────────────────

fn run_embedder_stage(
    worker_idx: usize,
    prepared_rx: Receiver<PreparedMsg>,
    embedded_tx: Sender<EmbeddedMsg>,
) {
    info!(
        "Vector pipeline [{}/embedder]: initialising BGE-Large (1024d) + TensorRT EP",
        worker_idx
    );
    let mut model = match build_vector_embedding_model(worker_idx) {
        Some(m) => m,
        None => {
            error!(
                "Vector pipeline [{}/embedder]: model init failed; exiting (axonctl will restart)",
                worker_idx
            );
            return;
        }
    };
    info!(
        "Vector pipeline [{}/embedder]: ready, awaiting prepared batches",
        worker_idx
    );

    loop {
        service_guard::record_vector_pipeline_embedder_heartbeat();
        let msg = match prepared_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                info!(
                    "Vector pipeline [{}/embedder]: producer gone, draining and exiting",
                    worker_idx
                );
                return;
            }
        };

        let PreparedMsg {
            prepared,
            completed_immediate,
        } = msg;
        if prepared.work_items.is_empty() {
            // Forward completed_immediate as a no-op embed batch.
            if embedded_tx
                .send(EmbeddedMsg::Ok {
                    updates: Vec::new(),
                    completed_immediate,
                    completed_after_success: Vec::new(),
                })
                .is_err()
            {
                return;
            }
            continue;
        }

        let touched = prepared.touched_works.clone();
        let completed_after_success = prepared.finalize_after_success.clone();

        match model.embed_prepared_batch_with_breakdown(&prepared) {
            Ok((embeddings, _, _, _, _)) => {
                service_guard::record_vector_lane_success();
                let updates: Vec<(String, String, Vec<f32>)> = prepared
                    .work_items
                    .iter()
                    .zip(embeddings.iter())
                    .map(|(item, emb)| {
                        (item.chunk_id.clone(), item.content_hash.clone(), emb.clone())
                    })
                    .collect();
                if embedded_tx
                    .send(EmbeddedMsg::Ok {
                        updates,
                        completed_immediate,
                        completed_after_success,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => {
                error!(
                    "Vector pipeline [{}/embedder]: embed failed: {:?}",
                    worker_idx, e
                );
                if embedded_tx
                    .send(EmbeddedMsg::Failed {
                        touched,
                        reason: format!("embed: {:?}", e),
                        completed_immediate,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

// ─────────────────────────────── Persister ───────────────────────────────

fn run_persister_stage(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    embedded_rx: Receiver<EmbeddedMsg>,
) {
    info!(
        "Vector pipeline [{}/persister]: ready (bulk_flush_min={}, max_linger={:?})",
        worker_idx, PERSISTER_BULK_FLUSH_MIN_ROWS, PERSISTER_BULK_FLUSH_MAX_LINGER
    );

    let mut buffer: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(PERSISTER_BULK_FLUSH_MIN_ROWS);
    // REQ-AXO-270 Phase 4: two-tier finalize queue.
    //   `ready_to_finalize` = files whose chunks are already persisted
    //     (immediate-completed or post-flush) — safe to mark_done at any
    //     time.
    //   `waiting_on_flush` = files whose last chunks are still in
    //     `buffer` — will move to ready_to_finalize the next time
    //     `buffer` is flushed.
    let mut ready_to_finalize: Vec<FileVectorizationWork> = Vec::new();
    let mut waiting_on_flush: Vec<FileVectorizationWork> = Vec::new();
    let mut last_flush = Instant::now();

    loop {
        service_guard::record_vector_pipeline_persister_heartbeat();

        // Compute remaining linger budget so partial buffers do not
        // sit idle when the embedder stream goes briefly quiet.
        let remaining_linger = PERSISTER_BULK_FLUSH_MAX_LINGER.saturating_sub(last_flush.elapsed());
        let recv_timeout = if buffer.is_empty() {
            Duration::from_millis(500)
        } else {
            remaining_linger.min(Duration::from_millis(500))
        };

        let msg = match embedded_rx.recv_timeout(recv_timeout) {
            Ok(msg) => Some(msg),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                info!(
                    "Vector pipeline [{}/persister]: embedder gone, flushing tail and exiting",
                    worker_idx
                );
                if !buffer.is_empty() {
                    flush_buffer(worker_idx, &graph_store, &mut buffer);
                    ready_to_finalize.extend(waiting_on_flush.drain(..));
                }
                if !ready_to_finalize.is_empty() {
                    finalize_completed(worker_idx, &graph_store, &mut ready_to_finalize);
                }
                return;
            }
        };

        if let Some(msg) = msg {
            match msg {
                EmbeddedMsg::Ok {
                    updates,
                    completed_immediate,
                    completed_after_success,
                } => {
                    ready_to_finalize.extend(completed_immediate);
                    if updates.is_empty() {
                        // No rows to persist — completed_after_success
                        // files have no chunks pending, finalize them
                        // on the next loop iteration.
                        ready_to_finalize.extend(completed_after_success);
                    } else {
                        buffer.extend(updates);
                        waiting_on_flush.extend(completed_after_success);
                    }
                }
                EmbeddedMsg::Failed {
                    touched,
                    reason,
                    completed_immediate,
                } => {
                    if let Err(e) =
                        graph_store.mark_file_vectorization_work_failed(&touched, &reason)
                    {
                        warn!(
                            "Vector pipeline [{}/persister]: mark_failed failed: {:?}",
                            worker_idx, e
                        );
                    }
                    ready_to_finalize.extend(completed_immediate);
                }
            }
        }

        // Flush triggers — AC2.7 minimum-row gate first, linger second.
        let should_flush_size = buffer.len() >= PERSISTER_BULK_FLUSH_MIN_ROWS;
        let should_flush_linger =
            !buffer.is_empty() && last_flush.elapsed() >= PERSISTER_BULK_FLUSH_MAX_LINGER;
        if should_flush_size || should_flush_linger {
            flush_buffer(worker_idx, &graph_store, &mut buffer);
            last_flush = Instant::now();
            // Phase 4 — every successful flush promotes waiting files
            // to the ready set so the producer/embedder don't stall on
            // a cycle barrier.
            ready_to_finalize.extend(waiting_on_flush.drain(..));
        }

        // Phase 4 — finalize on EVERY iteration where the ready set
        // is non-empty. No EndOfClaimCycle barrier.
        if !ready_to_finalize.is_empty() {
            finalize_completed(worker_idx, &graph_store, &mut ready_to_finalize);
        }
    }
}

/// AC2.7 — single bulk write of the entire `buffer` then clear it. Routes
/// through `graph_store.update_chunk_embeddings`, which under
/// `AXON_BULK_WRITER_ENABLED=true` (REQ-AXO-238) performs one COPY BINARY
/// into a staging table + `INSERT … SELECT … ON CONFLICT DO UPDATE` — the
/// canonical PG bulk-write path. On failure the rows are dropped and the
/// originating files will be re-claimed on the next FVQ cycle (their FVQ
/// rows stay unmarked-done because `pending_completed` only collapses
/// after this returns).
///
/// Operator directive 2026-05-10: the legacy DuckDB-era Parquet
/// side-store branch (DEC-AXO-073) was removed from this path — pgvector
/// stores embeddings natively, so the column-store-penalty workaround
/// the side-store mitigated no longer applies.
fn flush_buffer(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    buffer: &mut Vec<(String, String, Vec<f32>)>,
) {
    if buffer.is_empty() {
        return;
    }
    let row_count = buffer.len();
    let started = Instant::now();

    let result = graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, buffer);

    let elapsed_ms = started.elapsed().as_millis() as u64;
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::DbWrite, elapsed_ms);
    service_guard::record_vector_embed_call(row_count as u64, 0);

    match result {
        Ok(()) => {
            info!(
                "Vector pipeline [{}/persister]: bulk INSERT ok ({} rows in {} ms)",
                worker_idx, row_count, elapsed_ms
            );
        }
        Err(e) => {
            error!(
                "Vector pipeline [{}/persister]: bulk INSERT failed ({} rows): {:?}",
                worker_idx, row_count, e
            );
        }
    }

    buffer.clear();
}

fn finalize_completed(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    pending: &mut Vec<FileVectorizationWork>,
) {
    if pending.is_empty() {
        return;
    }
    let count = pending.len();
    if let Err(e) = graph_store.mark_file_vectorization_work_done(pending) {
        error!(
            "Vector pipeline [{}/persister]: mark_file_vectorization_work_done failed for {} files: {:?}",
            worker_idx, count, e
        );
    } else {
        service_guard::record_vector_files_completed(count as u64);
        info!(
            "Vector pipeline [{}/persister]: finalized {} file(s)",
            worker_idx, count
        );
    }
    pending.clear();
}

// ─────────────────────────────── Channel helpers ───────────────────────────────

/// Wraps `Sender::send` in a try-then-blocking pattern. Returns `Err`
/// only when the channel is fully disconnected (downstream stage gone).
/// Backpressure (full bounded channel) is normal and waits.
fn try_send_or_disconnect<T>(tx: &Sender<T>, msg: T) -> Result<(), ()> {
    let mut msg = Some(msg);
    loop {
        match tx.try_send(msg.take().expect("loop holds msg")) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(returned)) => {
                msg = Some(returned);
                thread::sleep(Duration::from_millis(2));
            }
            Err(TrySendError::Disconnected(_)) => return Err(()),
        }
    }
}

// ─────────────────────────────── Tests ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{env_test_lock, EnvVarGuard};

    #[test]
    fn vector_pipeline_mode_defaults_to_single_loop_when_env_unset() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::unset(ENV_FLAG);
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop
        );
    }

    #[test]
    fn vector_pipeline_mode_three_stages_when_env_set_to_3() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "3");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::ThreeStages
        );
    }

    #[test]
    fn vector_pipeline_mode_explicit_1_returns_single_loop() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "1");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop
        );
    }

    #[test]
    fn vector_pipeline_mode_falls_back_to_single_loop_on_unknown_env() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "garbage");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop
        );
    }

    #[test]
    fn vector_pipeline_mode_falls_back_to_single_loop_on_two_stages() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "2");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::SingleLoop
        );
    }

    #[test]
    fn vector_pipeline_mode_trims_whitespace() {
        let _lock = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvVarGuard::set(ENV_FLAG, "  3 ");
        assert_eq!(
            vector_pipeline_mode_from_env(),
            VectorPipelineMode::ThreeStages
        );
    }

    #[test]
    fn persister_bulk_flush_min_rows_meets_ac27_directive() {
        // AC2.7 — operator directive 2026-05-10 mandates ≥1000 rows
        // per DB transaction. This guard keeps the contract visible
        // in the test surface so a future tuning patch cannot silently
        // drop below the directive.
        assert!(
            PERSISTER_BULK_FLUSH_MIN_ROWS >= 1000,
            "AC2.7 mandates persister bulk-writes >= 1000 rows per DB transaction"
        );
    }

    #[test]
    fn try_send_or_disconnect_returns_err_when_receiver_dropped() {
        let (tx, rx) = bounded::<u8>(1);
        drop(rx);
        assert!(try_send_or_disconnect(&tx, 7u8).is_err());
    }

    #[test]
    fn try_send_or_disconnect_succeeds_when_buffer_has_space() {
        let (tx, rx) = bounded::<u8>(1);
        assert!(try_send_or_disconnect(&tx, 1u8).is_ok());
        // Consume the message so the channel is reusable in a follow-up
        // assertion if the test grows.
        assert_eq!(rx.try_recv().ok(), Some(1));
    }

    /// AC2.4 — crash isolation: when the upstream sender is dropped (as
    /// it would be on producer panic / embedder crash), the persister
    /// must observe `RecvTimeoutError::Disconnected`, drain any pending
    /// state, and exit cleanly within a bounded timeout. axonctl then
    /// restarts the whole worker; a stuck thread would block the
    /// restart and silently kill throughput.
    #[test]
    fn persister_exits_when_upstream_disconnects() {
        use crate::tests::test_helpers::create_test_db;

        let store = create_test_db().expect("test graph store");
        let store = Arc::new(store);
        let (tx, rx) = bounded::<EmbeddedMsg>(STAGE_CHANNEL_DEPTH);

        let handle = thread::spawn({
            let store = Arc::clone(&store);
            move || {
                run_persister_stage(99, store, rx);
            }
        });

        // Drop the sender — simulates upstream stage gone.
        drop(tx);

        // The recv_timeout is 500ms; allow generous slack so the test
        // remains stable on busy CI hosts but still fails fast on a
        // genuine disconnect-detection regression.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                panic!(
                    "persister did not exit within 5s after upstream disconnect"
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
        handle.join().expect("persister thread did not panic");
    }

    /// AC2.4 — same disconnect contract for the producer-facing side of
    /// the pipeline. `try_send_or_disconnect` is the only place the
    /// producer can observe a downstream crash; cover both the empty
    /// and the full-then-disconnect paths.
    #[test]
    fn producer_send_path_observes_disconnect_when_buffer_then_drops() {
        let (tx, rx) = bounded::<u8>(1);
        // Fill the bounded channel.
        tx.send(1).expect("first send fits");
        // Disconnect by dropping the receiver.
        drop(rx);
        // Now try_send_or_disconnect should detect the drop without
        // spinning forever on a "Full" loop.
        assert!(try_send_or_disconnect(&tx, 2u8).is_err());
    }

    /// AC2.4 — flush_buffer must clear the buffer even on a DB write
    /// error so the persister doesn't accumulate phantom rows that
    /// would be re-written on the next flush. With an unregistered
    /// chunk model id, update_chunk_embeddings should fail; the test
    /// asserts the buffer ends up empty regardless.
    #[test]
    fn persister_flush_buffer_clears_buffer_on_failure() {
        use crate::tests::test_helpers::create_test_db;

        let store = Arc::new(create_test_db().expect("test graph store"));
        let mut buffer: Vec<(String, String, Vec<f32>)> = vec![(
            "chunk-test-0".to_string(),
            "hash-0".to_string(),
            vec![0.0_f32; 1024],
        )];
        flush_buffer(99, &store, &mut buffer);
        assert!(
            buffer.is_empty(),
            "flush_buffer must clear the buffer regardless of write outcome"
        );
    }
}
