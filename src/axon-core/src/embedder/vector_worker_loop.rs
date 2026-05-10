//! Vector lane worker — single-threaded, in-process, no channels (DEC-AXO-070).
//!
//! Replaces the previous 5-loop pipeline (refill → prepare → worker → persist
//! → finalize) with one synchronous claim → prepare → embed → persist → finalize
//! function. Crash isolation is delegated to axonctl process supervisor
//! (REQ-AXO-097 watchdog). VRAM management is a single signal, not a 5-signal
//! coordinator.
//!
//! Throughput target: ≥30 ch/s end-to-end on Axon repo (Didier directive
//! 2026-05-04). The L1 GPU bench harness still produces ≥140 ch/s warm
//! (DEC-AXO-068 / VAL-AXO-028).

use std::collections::HashSet;
use std::io::Write as _;

use super::vector_pipeline_3stages::{
    run_vector_pipeline_3stages, vector_pipeline_mode_from_env, VectorPipelineMode,
};
use super::*;

/// Write a single trace line to `<AXON_RUN_ROOT>/vector-lane.trace`.
/// Bypasses the tracing subscriber so the diagnostic survives any log-filter
/// or scrollback truncation (REQ-AXO-185).
fn vector_trace(line: &str) {
    let Ok(run_root) = std::env::var("AXON_RUN_ROOT") else {
        return;
    };
    let path = std::path::PathBuf::from(run_root).join("vector-lane.trace");
    let now = chrono::Utc::now().format("%H:%M:%S%.3f");
    let formatted = format!("{} {}\n", now, line);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(formatted.as_bytes()));
}

pub(super) fn vector_lane_worker(worker_idx: usize, graph_store: Arc<GraphStore>) {
    let _liveness = VectorWorkerLivenessGuard::new();

    // REQ-AXO-270 AC1.3 — factory dispatch. AXON_VECTOR_PIPELINE_STAGES=3
    // routes to the 3-stage pipeline (Phase 1 = stubs only). Default and
    // any other value keep DEC-AXO-070 single-loop behavior below.
    if vector_pipeline_mode_from_env() == VectorPipelineMode::ThreeStages {
        vector_trace(&format!(
            "[{}] dispatch=3stages (REQ-AXO-270 Phase 1 skeleton)",
            worker_idx
        ));
        run_vector_pipeline_3stages(worker_idx, graph_store);
        return;
    }

    let lane_config = embedding_lane_config_from_env();
    vector_trace(&format!("[{}] worker_entry", worker_idx));

    if let Err(e) = graph_store.ensure_embedding_model(
        SYMBOL_MODEL_ID,
        "symbol",
        MODEL_NAME,
        DIMENSION as i64,
        MODEL_VERSION,
    ) {
        error!(
            "Vector lane [{}]: failed to register symbol embedding model: {:?}",
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
            "Vector lane [{}]: failed to register chunk embedding model: {:?}",
            worker_idx, e
        );
    }

    info!(
        "Vector lane [{}]: initializing BGE-Large (1024d) + TensorRT EP",
        worker_idx
    );
    let mut model = match build_vector_embedding_model(worker_idx) {
        Some(m) => m,
        None => {
            error!(
                "Vector lane [{}]: model init failed; exiting (axonctl will restart)",
                worker_idx
            );
            return;
        }
    };
    let tokenizer = match load_runtime_embedding_tokenizer() {
        Ok(t) => t,
        Err(e) => {
            error!(
                "Vector lane [{}]: tokenizer load failed: {:?}; exiting",
                worker_idx, e
            );
            return;
        }
    };
    info!("Vector lane [{}]: ready, polling for work", worker_idx);
    vector_trace(&format!("[{}] ready", worker_idx));

    // DEC-AXO-071 H.1: register inline-embed inbox so graph projection
    // workers can route embed requests through this single vector lane
    // (single GPU model load — guards against the REQ-AXO-181 step 4
    // multi-worker OOM cascade). H.1 only wires the channel; graph
    // projection does not call `embed_via_vector_lane` until H.2 lands.
    // With zero senders by default the drain below is a no-op for
    // existing operators (DEC-AXO-070 commit G behavior preserved).
    let (inline_tx, inline_rx) = inline_embed::create_vector_lane_inbox();
    if !inline_embed::register_vector_lane_inbox(inline_tx) {
        warn!(
            "Vector lane [{}]: inline embed inbox already registered; this lane will not receive inline requests",
            worker_idx
        );
    }

    let target_chunks = lane_config.chunk_batch_size.max(1);
    let per_file_fetch_limit = lane_config.max_chunks_per_file;
    let batch_max_bytes = lane_config.max_embed_batch_bytes;
    let file_batch_size = lane_config.file_vectorization_batch_size.max(1);

    let mut tick: u64 = 0;
    let mut last_iter_done_at: Option<Instant> = None;
    loop {
        service_guard::record_vector_worker_heartbeat();
        tick += 1;
        if tick % 200 == 1 {
            vector_trace(&format!("[{}] tick={} loop_alive", worker_idx, tick));
        }
        let iter_started_at = Instant::now();
        let inter_iter_idle_ms = last_iter_done_at
            .map(|t| iter_started_at.duration_since(t).as_millis())
            .unwrap_or(0);

        // DEC-AXO-071 H.1: drain inline embed requests before fetching
        // from the queue. Implicit priority — keeps inline graph
        // projection latency low. No-op when no graph worker has
        // registered as a sender (default in H.1).
        while let Ok(req) = inline_rx.try_recv() {
            let result = model
                .embed_texts_with_breakdown(&req.texts)
                .map(|(embeddings, _, _, _, _)| embeddings);
            let _ = req.respond_to.send(result);
        }

        // DEC-AXO-072 J.4: when the hot status cache is enabled, claim
        // pending files from cache (process-local; no DB JOIN). Cache
        // hydrated at boot from FVQ; subsequent mark_ready calls from
        // graph_ingestion populate it. Cache disabled -> existing
        // multi-table fetch JOIN against DB.
        let hot_cache_enabled = crate::hot_status_cache::cache_enabled();
        let claimed = if hot_cache_enabled {
            match crate::hot_status_cache::cache() {
                Some(cache) => {
                    let pending = cache.pending_for_lane(file_batch_size);
                    if pending.is_empty() {
                        if tick % 200 == 1 {
                            vector_trace(&format!("[{}] tick={} cache_empty", worker_idx, tick));
                        }
                        let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(50));
                        continue;
                    }
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let lane_owner = format!("vector-{}", worker_idx);
                    let granted: Vec<FileVectorizationWork> = pending
                        .into_iter()
                        .filter_map(|path| {
                            cache
                                .try_claim(&path, &lane_owner, now_ms)
                                .map(|_| FileVectorizationWork {
                                    file_path: path,
                                    resumed_after_interactive_pause: false,
                                })
                        })
                        .collect();
                    if granted.is_empty() {
                        let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(50));
                        continue;
                    }
                    granted
                }
                None => Vec::new(),
            }
        } else {
            match graph_store.fetch_pending_file_vectorization_work(file_batch_size) {
                Ok(work) if !work.is_empty() => work,
                Ok(_) => {
                    if tick % 200 == 1 {
                        vector_trace(&format!("[{}] tick={} fetch_empty", worker_idx, tick));
                    }
                    let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(50));
                    continue;
                }
                Err(e) => {
                    vector_trace(&format!("[{}] fetch_err: {:?}", worker_idx, e));
                    error!(
                        "Vector lane [{}]: fetch_pending_file_vectorization_work failed: {:?}",
                        worker_idx, e
                    );
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
        };
        vector_trace(&format!("[{}] claimed={}", worker_idx, claimed.len()));
        let claim_done_at = Instant::now();
        let claim_ms = claim_done_at.duration_since(iter_started_at).as_millis();
        let files_claimed = claimed.len();

        info!(
            "Vector lane [{}]: claimed {} file(s) for embedding",
            worker_idx,
            claimed.len()
        );
        // mark_file_vectorization_started updates File.vectorization_started_at_ms
        // (cold field on File table, not on FVQ). Always call — cache only
        // covers the FVQ side; File-side bookkeeping stays direct-DB.
        if let Err(e) = graph_store.mark_file_vectorization_started(&claimed) {
            warn!(
                "Vector lane [{}]: mark_file_vectorization_started failed: {:?}",
                worker_idx, e
            );
        }
        let mark_started_done_at = Instant::now();
        let mark_started_ms = mark_started_done_at.duration_since(claim_done_at).as_millis();

        let mut active = claimed;
        let mut completed_files: Vec<FileVectorizationWork> = Vec::new();
        let mut reserved_chunk_ids: HashSet<String> = HashSet::new();
        let mut total_prep_ms: u128 = 0;
        let mut total_tok_ms: u128 = 0;
        let mut total_embed_ms: u128 = 0;
        let mut total_persist_ms: u128 = 0;
        let mut total_chunks_persisted: usize = 0;

        while !active.is_empty() {
            let prep_started_at = Instant::now();
            let mut prepared = prepare_vector_embed_batch(
                &graph_store,
                &active,
                target_chunks,
                per_file_fetch_limit,
                batch_max_bytes,
                &reserved_chunk_ids,
            );
            total_prep_ms += prep_started_at.elapsed().as_millis();
            for item in &prepared.work_items {
                reserved_chunk_ids.insert(item.chunk_id.clone());
            }

            for w in &prepared.oversized_works {
                if let Err(err) =
                    graph_store.mark_file_oversized_for_current_budget(&w.file_path)
                {
                    warn!(
                        "Vector lane [{}]: failed to mark oversized {}: {:?}",
                        worker_idx, w.file_path, err
                    );
                }
            }
            for (w, reason) in &prepared.failed_fetches {
                error!(
                    "Vector lane [{}]: chunk fetch failed for {}: {}",
                    worker_idx, w.file_path, reason
                );
                let _ = graph_store
                    .mark_file_vectorization_work_failed(std::slice::from_ref(w), reason);
            }

            info!(
                "Vector lane [{}]: prepared chunks={} immediate_completed={} finalize_after_success={} oversized={} next_active_success={} next_active_failure={} failed_fetches={}",
                worker_idx,
                prepared.work_items.len(),
                prepared.immediate_completed.len(),
                prepared.finalize_after_success.len(),
                prepared.oversized_works.len(),
                prepared.next_active_after_success.len(),
                prepared.next_active_after_failure.len(),
                prepared.failed_fetches.len()
            );
            let made_progress = !prepared.work_items.is_empty()
                || !prepared.immediate_completed.is_empty()
                || !prepared.finalize_after_success.is_empty()
                || !prepared.oversized_works.is_empty();

            if !prepared.texts.is_empty() {
                let tok_started_at = Instant::now();
                if let Err(e) = attach_preencoded_micro_batches(&tokenizer, &mut prepared) {
                    error!(
                        "Vector lane [{}]: tokenize failed: {:?}",
                        worker_idx, e
                    );
                    let _ = graph_store.mark_file_vectorization_work_failed(
                        &prepared.touched_works,
                        &format!("tokenize: {:?}", e),
                    );
                    active = prepared.next_active_after_failure;
                    continue;
                }
                total_tok_ms += tok_started_at.elapsed().as_millis();

                let embed_started = Instant::now();
                let embeddings = match model.embed_prepared_batch_with_breakdown(&prepared) {
                    Ok((embeddings, _, _, _, _)) => {
                        let embed_elapsed = embed_started.elapsed().as_millis();
                        total_embed_ms += embed_elapsed;
                        service_guard::record_vector_lane_success();
                        service_guard::record_vector_stage_ms(
                            service_guard::VectorStageKind::Embed,
                            embed_elapsed as u64,
                        );
                        embeddings
                    }
                    Err(e) => {
                        // DEC-AXO-070 single-loop design: never exit on transient
                        // embed errors. Mark the batch failed and continue. axonctl
                        // supervises the process; in-loop self-termination just
                        // hides the recoverable failure mode.
                        error!(
                            "Vector lane [{}]: embed failed: {:?}",
                            worker_idx, e
                        );
                        let _ = graph_store.mark_file_vectorization_work_failed(
                            &prepared.touched_works,
                            &format!("embed: {:?}", e),
                        );
                        active = prepared.next_active_after_failure;
                        continue;
                    }
                };

                let updates: Vec<(String, String, Vec<f32>)> = prepared
                    .work_items
                    .iter()
                    .zip(embeddings.iter())
                    .map(|(item, emb)| {
                        (item.chunk_id.clone(), item.content_hash.clone(), emb.clone())
                    })
                    .collect();
                let db_started = Instant::now();
                // DEC-AXO-073 L.2: when the Parquet side-store is
                // enabled, write the FLOAT[1024] vectors to append-only
                // Parquet files instead of the DuckDB ChunkEmbedding
                // table. Skipping the DuckDB INSERT eliminates the
                // column-store growth penalty (VAL-AXO-034). When
                // disabled, fall through to update_chunk_embeddings
                // (commit G + H.2 path). On Parquet failure, fall back
                // to DuckDB so no chunk is lost.
                let persist_result: Result<(), anyhow::Error> = if crate::embedder::parquet_embedding_store::parquet_store_enabled() {
                    match crate::embedder::parquet_embedding_store::store() {
                        Some(parquet) => {
                            let rows: Vec<(String, String, Vec<f32>)> = updates
                                .iter()
                                .map(|(id, hash, emb)| (id.clone(), hash.clone(), emb.clone()))
                                .collect();
                            match parquet.append_batch(&rows) {
                                Ok(()) => Ok(()),
                                Err(e) => {
                                    warn!(
                                        "Vector lane [{}]: parquet append failed: {:?}; falling back to DuckDB",
                                        worker_idx, e
                                    );
                                    graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates)
                                }
                            }
                        }
                        None => graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates),
                    }
                } else {
                    graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates)
                };
                if let Err(e) = persist_result {
                    error!(
                        "Vector lane [{}]: persist failed: {:?}",
                        worker_idx, e
                    );
                    let _ = graph_store.mark_file_vectorization_work_failed(
                        &prepared.touched_works,
                        &format!("persist: {:?}", e),
                    );
                    active = prepared.next_active_after_failure;
                    continue;
                }
                let db_elapsed = db_started.elapsed().as_millis();
                total_persist_ms += db_elapsed;
                total_chunks_persisted += updates.len();
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::DbWrite,
                    db_elapsed as u64,
                );
                service_guard::record_vector_embed_call(
                    updates.len() as u64,
                    prepared.touched_works.len() as u64,
                );
            }

            completed_files.extend(prepared.immediate_completed.iter().cloned());
            completed_files.extend(prepared.finalize_after_success.iter().cloned());

            active = prepared.next_active_after_success;
            if !made_progress {
                break;
            }
        }

        vector_trace(&format!("[{}] completed_files={}", worker_idx, completed_files.len()));
        let finalize_started_at = Instant::now();
        if !completed_files.is_empty() {
            // DEC-AXO-072 J.4: when cache is enabled, evict completed
            // entries from cache so subsequent pending_for_lane scans
            // don't re-claim them. The DB DELETE below removes the
            // FVQ row (cache.mark_done is evict-only; flush thread
            // never sees a Done state, no race vs the DELETE).
            if hot_cache_enabled {
                if let Some(cache) = crate::hot_status_cache::cache() {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    for f in &completed_files {
                        cache.mark_done(&f.file_path, now_ms);
                    }
                }
            }
            if let Err(e) = graph_store.mark_file_vectorization_work_done(&completed_files) {
                vector_trace(&format!("[{}] mark_done_err: {:?}", worker_idx, e));
                error!(
                    "Vector lane [{}]: mark_file_vectorization_work_done failed: {:?}",
                    worker_idx, e
                );
            } else {
                service_guard::record_vector_files_completed(completed_files.len() as u64);
            }
        }
        let finalize_ms = finalize_started_at.elapsed().as_millis();
        let iter_done_at = Instant::now();
        let iter_total_ms = iter_done_at.duration_since(iter_started_at).as_millis();
        // Per-iteration profiling summary (DEC-AXO-072 follow-up). Emitted
        // for every iteration that did real work (claimed > 0); skip for
        // pure-idle ticks to avoid noise.
        if files_claimed > 0 {
            vector_trace(&format!(
                "[{}] iter inter_idle_ms={} claim_ms={} mark_started_ms={} prep_ms={} tok_ms={} embed_ms={} persist_ms={} finalize_ms={} total_ms={} files={} chunks={}",
                worker_idx,
                inter_iter_idle_ms,
                claim_ms,
                mark_started_ms,
                total_prep_ms,
                total_tok_ms,
                total_embed_ms,
                total_persist_ms,
                finalize_ms,
                iter_total_ms,
                files_claimed,
                total_chunks_persisted
            ));
        }
        last_iter_done_at = Some(iter_done_at);

        // DEC-AXO-070 commit G: vector lane is chunks-only. Symbol embeddings
        // were judged low-value (operator directive 2026-05-05) and were
        // additionally creating writer contention that blocked chunk
        // throughput. The Symbol.embedding column persists in the schema for
        // downstream callers but is no longer populated by the live indexer.

        if let Some(snap) = current_gpu_memory_snapshot() {
            if snap.used_mb >= 7000 {
                warn!(
                    "Vector lane [{}]: VRAM headroom tight: {} MB used",
                    worker_idx, snap.used_mb
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn build_vector_embedding_model(worker_idx: usize) -> Option<VectorEmbeddingBackend> {
    let provider_requested = effective_provider_request_for_lane("vector");
    let cuda_requested = provider_requested.eq_ignore_ascii_case("cuda");

    let cuda_available = std::env::var("AXON_EMBEDDING_GPU_PRESENT")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let cuda_provider_library_available = ort_cuda_provider_library_available();

    if cuda_requested && cuda_available && !cuda_provider_library_available {
        let provider_path = ort_cuda_provider_library_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        error!(
            "❌ Vector lane [{}]: CUDA requested but ONNX Runtime provider library is missing: {}",
            worker_idx, provider_path
        );
        set_embedding_provider_runtime_state("cpu_missing_cuda_provider", None);
    }

    let model_result = if cuda_requested && cuda_available && cuda_provider_library_available {
        match OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, true) {
            Ok(model) => {
                set_embedding_provider_runtime_state("cuda", None);
                Ok(model)
            }
            Err(err) => {
                let rendered = format!("{err:?}");
                error!(
                    "❌ Vector lane [{}]: ORT CUDA init failed, falling back to CPU: {:?}",
                    worker_idx, err
                );
                set_embedding_provider_runtime_state("cpu_fallback", Some(&rendered));
                apply_cpu_fallback_ort_runtime_env();
                OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
            }
        }
    } else {
        set_embedding_provider_runtime_state(
            cpu_provider_effective_label(
                cuda_requested,
                cuda_available,
                cuda_provider_library_available,
            ),
            None,
        );
        OrtGpuFirstTextEmbedding::try_new("vector", worker_idx, false)
    };

    match model_result {
        Ok(model) => {
            let provider_effective = current_embedding_provider_effective();
            register_embedding_provider_diagnostics(embedding_provider_diagnostics(
                provider_effective.clone(),
            ));
            if provider_effective.starts_with("cuda") {
                Some(VectorEmbeddingBackend::cuda_in_process(model))
            } else {
                Some(VectorEmbeddingBackend::cpu_in_process(model))
            }
        }
        Err(err) => {
            let rendered = format!("{err:?}");
            error!(
                "❌ Vector lane [{}]: FATAL ORT GPU-FIRST INIT ERROR: {:?}",
                worker_idx, err
            );
            publish_embedding_provider_state("unavailable", Some(&rendered));
            None
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn vector_lane_worker_links_to_runtime() {
        let _: fn(usize, std::sync::Arc<crate::graph::GraphStore>) = super::vector_lane_worker;
    }

}
