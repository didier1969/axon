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

use super::*;

pub(super) fn vector_lane_worker(worker_idx: usize, graph_store: Arc<GraphStore>) {
    let _liveness = VectorWorkerLivenessGuard::new();
    let lane_config = embedding_lane_config_from_env();

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

    let target_chunks = lane_config.chunk_batch_size.max(1);
    let per_file_fetch_limit = lane_config.max_chunks_per_file;
    let batch_max_bytes = lane_config.max_embed_batch_bytes;
    let file_batch_size = lane_config.file_vectorization_batch_size.max(1);

    loop {
        service_guard::record_vector_worker_heartbeat();

        let claimed = match graph_store.fetch_pending_file_vectorization_work(file_batch_size) {
            Ok(work) if !work.is_empty() => work,
            Ok(_) => {
                let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(50));
                continue;
            }
            Err(e) => {
                error!(
                    "Vector lane [{}]: fetch_pending_file_vectorization_work failed: {:?}",
                    worker_idx, e
                );
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };

        if let Err(e) = graph_store.mark_file_vectorization_started(&claimed) {
            warn!(
                "Vector lane [{}]: mark_file_vectorization_started failed: {:?}",
                worker_idx, e
            );
        }

        let mut active = claimed;
        let mut completed_files: Vec<FileVectorizationWork> = Vec::new();
        let mut reserved_chunk_ids: HashSet<String> = HashSet::new();

        while !active.is_empty() {
            let mut prepared = prepare_vector_embed_batch(
                &graph_store,
                &active,
                target_chunks,
                per_file_fetch_limit,
                batch_max_bytes,
                &reserved_chunk_ids,
            );
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

            let made_progress = !prepared.work_items.is_empty()
                || !prepared.immediate_completed.is_empty()
                || !prepared.finalize_after_success.is_empty()
                || !prepared.oversized_works.is_empty();

            if !prepared.texts.is_empty() {
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

                let embed_started = Instant::now();
                let embeddings = match model.embed_prepared_batch_with_breakdown(&prepared) {
                    Ok((embeddings, _, _, _, _)) => {
                        service_guard::record_vector_lane_success();
                        service_guard::record_vector_stage_ms(
                            service_guard::VectorStageKind::Embed,
                            embed_started.elapsed().as_millis() as u64,
                        );
                        embeddings
                    }
                    Err(e) => {
                        let fatal = is_gpu_recycle_immediate_error(&e)
                            || fatal_embedding_error_class(&e).is_some();
                        error!(
                            "Vector lane [{}]: embed failed (fatal={}): {:?}",
                            worker_idx, fatal, e
                        );
                        let _ = graph_store.mark_file_vectorization_work_failed(
                            &prepared.touched_works,
                            &format!("embed: {:?}", e),
                        );
                        if fatal {
                            error!(
                                "Vector lane [{}]: fatal embed error; exiting (axonctl will restart)",
                                worker_idx
                            );
                            return;
                        }
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
                if let Err(e) = graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates) {
                    error!(
                        "Vector lane [{}]: update_chunk_embeddings failed: {:?}",
                        worker_idx, e
                    );
                    let _ = graph_store.mark_file_vectorization_work_failed(
                        &prepared.touched_works,
                        &format!("persist: {:?}", e),
                    );
                    active = prepared.next_active_after_failure;
                    continue;
                }
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::DbWrite,
                    db_started.elapsed().as_millis() as u64,
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

        if !completed_files.is_empty() {
            if let Err(e) = graph_store.mark_file_vectorization_work_done(&completed_files) {
                error!(
                    "Vector lane [{}]: mark_file_vectorization_work_done failed: {:?}",
                    worker_idx, e
                );
            } else {
                service_guard::record_vector_files_completed(completed_files.len() as u64);
            }
        }

        // Symbol embeddings: small-volume side-channel, drain opportunistically.
        match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
            Ok(symbols) if !symbols.is_empty() => {
                let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                match model.embed_texts_with_breakdown(&texts) {
                    Ok((embeddings, _, _, _, _)) => {
                        let updates: Vec<(String, Vec<f32>)> = symbols
                            .into_iter()
                            .zip(embeddings)
                            .map(|((id, _), emb)| (id, emb))
                            .collect();
                        if let Err(e) = graph_store.update_symbol_embeddings(&updates) {
                            error!(
                                "Vector lane [{}]: update_symbol_embeddings failed: {:?}",
                                worker_idx, e
                            );
                        }
                    }
                    Err(e) => {
                        let fatal = is_gpu_recycle_immediate_error(&e)
                            || fatal_embedding_error_class(&e).is_some();
                        error!(
                            "Vector lane [{}]: symbol embed failed (fatal={}): {:?}",
                            worker_idx, fatal, e
                        );
                        if fatal {
                            error!(
                                "Vector lane [{}]: fatal symbol embed; exiting (axonctl will restart)",
                                worker_idx
                            );
                            return;
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                error!(
                    "Vector lane [{}]: fetch_unembedded_symbols failed: {:?}",
                    worker_idx, e
                );
            }
        }

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
