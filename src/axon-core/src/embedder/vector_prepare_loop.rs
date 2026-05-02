//! Vector prepare worker loop — extracted from embedder.rs (REQ-AXO-080 Phase 3).
//!
//! Pure structural extraction of `vector_prepare_worker_loop` from the embedder
//! impl block to its own submodule. Behaviour-preserving move; the function
//! body is verbatim. Free helpers from the parent module are visible via
//! `use super::*` because Rust submodules see crate-private items of their
//! parent.

use super::*;

pub(super) fn vector_prepare_worker_loop(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    prepare_rx: Receiver<VectorPrepareRequest>,
    ready_batches: Arc<SharedPreparedBatchQueue>,
) {
    info!(
        "Semantic Vector Prepare Worker [{}]: ready with bounded queue {}",
        worker_idx,
        configured_vector_prepare_queue_bound()
    );
    let mut tokenizer = load_runtime_embedding_tokenizer().ok();
    let mut wake_idle = true;
    while let Ok(request) = prepare_rx.recv() {
        if wake_idle {
            service_guard::record_runtime_wakeup(
                service_guard::RuntimeWakeSource::SemanticVector,
                0,
                1,
            );
            wake_idle = false;
        }
        service_guard::record_vector_prepare_queue_depth(prepare_rx.len() as u64);
        service_guard::record_vector_prepare_queue_wait_ms(
            request.enqueued_at.elapsed().as_millis() as u64,
        );
        let prepare_started_at_ms = chrono::Utc::now().timestamp_millis();
        let mut sequence = build_prepared_vector_embed_sequence(
            &graph_store,
            request.claimed.as_slice(),
            request.target_chunks,
            request.per_file_fetch_limit,
            request.batch_max_bytes,
            request.target_ready_depth,
            prepare_started_at_ms,
        );
        let mut rejected_works = Vec::new();
        for mut prepared in sequence.batches.drain(..) {
            if !prepared.texts.is_empty() {
                if tokenizer.is_none() {
                    tokenizer = load_runtime_embedding_tokenizer().ok();
                }
                if let Some(active_tokenizer) = tokenizer.as_ref() {
                    match attach_preencoded_micro_batches(active_tokenizer, &mut prepared) {
                        Ok(()) => {}
                        Err(err) => {
                            error!(
                                "Semantic Vector Prepare Worker [{}]: failed to pre-tokenize batch; rejecting it before GPU admission: {:?}",
                                worker_idx, err
                            );
                            rejected_works.extend(prepared.touched_works.clone());
                            rejected_works.extend(prepared.immediate_completed.clone());
                            continue;
                        }
                    }
                } else {
                    error!(
                        "Semantic Vector Prepare Worker [{}]: tokenizer unavailable; rejecting non-tokenized batch before GPU admission",
                        worker_idx
                    );
                    rejected_works.extend(prepared.touched_works.clone());
                    rejected_works.extend(prepared.immediate_completed.clone());
                    continue;
                }
            }
            service_guard::record_vector_active_lane_thresholds(
                prepared.lane_thresholds.small_max_tokens as u64,
                prepared.lane_thresholds.medium_max_tokens as u64,
            );
            let split_batches = split_prepared_batch_by_lane(
                prepared.into_inner(),
                request.target_chunks,
                ready_batches.summary().chunk_count,
            );
            let budgeted_batches = split_batches
                .into_iter()
                .flat_map(|batch| split_prepared_batch_for_gpu_budget(batch.into_inner()))
                .collect::<Vec<_>>();
            for batch in &budgeted_batches {
                service_guard::record_vector_batch_shape(!batch.mixed_fallback());
                service_guard::record_vector_prepare_outcome(
                    batch.work_items.len() as u64,
                    batch.immediate_completed.len() as u64,
                    batch.failed_fetches.len() as u64,
                );
            }
            let fulfilled_chunk_count = budgeted_batches
                .iter()
                .map(|batch| batch.chunk_count())
                .sum::<u64>()
                .max(1);
            let _ready_depth = ready_batches.push_back_many(budgeted_batches);
            service_guard::record_vector_ready_replenishment_fulfilled(fulfilled_chunk_count);
            service_guard::record_vector_prepare_prefetch();
            record_ready_queue_summary(&ready_batches.summary());
        }
        if !rejected_works.is_empty() {
            let remaining = sequence.remaining_claimed_after_success.into_inner();
            sequence.remaining_claimed_after_success =
                ClaimedLeaseSet::new(merge_vectorization_work(
                    rejected_works,
                    remaining,
                    request.claimed.as_slice().len().max(1),
                ));
        }
        if request
            .reply
            .send(PreparedVectorPrepareOutcome {
                remaining_claimed_after_success: sequence.remaining_claimed_after_success,
            })
            .is_err()
        {
            error!(
                "Semantic Vector Prepare Worker [{}]: embed worker dropped prepare outcome reply channel",
                worker_idx
            );
        }
        if prepare_rx.is_empty() {
            wake_idle = true;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracted_function_links_to_runtime() {
        let _: fn(
            usize,
            std::sync::Arc<crate::graph::GraphStore>,
            crossbeam_channel::Receiver<super::VectorPrepareRequest>,
            std::sync::Arc<super::SharedPreparedBatchQueue>,
        ) = super::vector_prepare_worker_loop;
    }
}
