//! Vector refill worker loop — extracted from embedder.rs (REQ-AXO-133).
//!
//! Pure structural extraction of `vector_refill_worker_loop` from the embedder
//! impl block to its own submodule. Behaviour-preserving move; the function
//! body is verbatim. The function uses many free helpers from the parent
//! module (embedder.rs); they remain visible via `super::*` because Rust
//! submodules see crate-private items of their parent.

use super::*;

pub(super) fn vector_refill_worker_loop(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    prepare_tx: Sender<VectorPrepareRequest>,
    refill_rx: Receiver<VectorRefillCommand>,
    ready_batches: Arc<SharedPreparedBatchQueue>,
) {
    let lane_config = embedding_lane_config_from_env();
    let configured_max_inflight_prepares = configured_vector_prepare_pipeline_depth();
    let mut refill_state = VectorRefillProducerState::new(Vec::new());
    let empty_inflight_persists = VecDeque::new();
    let mut wake_idle = true;

    loop {
        service_guard::record_vector_worker_heartbeat();

        while let Ok(command) = refill_rx.try_recv() {
            match command {
                VectorRefillCommand::RequeueWorks(works) => {
                    refill_state.merge_requeued_works(works);
                    wake_idle = false;
                }
                VectorRefillCommand::BatchConsumed(chunks) => {
                    service_guard::record_vector_ready_replenishment_requested(chunks as u64);
                    wake_idle = false;
                }
            }
        }

        refill_state.poll_prepare_replies(worker_idx);

        let claimable_file_backlog_depth = graph_store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap_or(0);
        let ready_queue_summary = ready_batches.summary();
        let ready_batches_len = ready_queue_summary.len;
        let ready_chunk_count = ready_queue_summary.chunk_count;
        let owned_prepare_works = refill_state
            .inflight_prepares()
            .iter()
            .flat_map(|request| request.claimed.clone_works())
            .collect::<Vec<_>>();
        let inflight_prepare_chunks = refill_state.inflight_prepare_chunk_count();
        let current_vector_owned = merge_unique_vectorization_work_sets([
            refill_state.active_works().to_vec(),
            owned_prepare_works.clone(),
            ready_batches.touched_works_snapshot(),
        ]);
        let _ = graph_store
            .refresh_file_vectorization_leases_for_owner(&current_vector_owned, "vector");
        record_ready_queue_summary(&ready_queue_summary);
        service_guard::record_vector_prepare_inflight_depth(
            refill_state.inflight_prepare_count() as u64,
        );
        service_guard::record_vector_prepare_inflight_chunks(inflight_prepare_chunks as u64);
        service_guard::record_vector_oldest_ready_batch_age_ms(
            ready_queue_summary
                .oldest_prepared_at_ms
                .map(|prepared_at_ms| {
                    chrono::Utc::now()
                        .timestamp_millis()
                        .saturating_sub(prepared_at_ms)
                })
                .unwrap_or(0) as u64,
        );

        let controller = observe_vector_batch_controller(
            &lane_config,
            VectorBatchControllerObservation {
                upstream_file_pressure: claimable_file_backlog_depth
                    + refill_state.active_len()
                    + owned_prepare_works.len(),
                front_chunk_supply: ready_chunk_count + inflight_prepare_chunks,
                interactive_active: service_guard::interactive_priority_active()
                    || service_guard::interactive_requests_in_flight() > 0,
                gpu_memory_pressure: current_gpu_memory_pressure_active(),
                metrics: service_guard::vector_runtime_metrics(),
            },
        );
        let vector_metrics = service_guard::vector_runtime_metrics();
        let replenishment_deficit = vector_metrics.ready_replenishment_deficit_current as usize;
        let gpu_memory_pressure = current_gpu_memory_pressure_active();
        let controller_target_chunks = controller.target_embed_batch_chunks.max(1);
        let ready_batches_equivalent =
            chunk_capacity_to_batch_depth(ready_chunk_count, controller_target_chunks);
        let inflight_prepare_batches_equivalent =
            chunk_capacity_to_batch_depth(inflight_prepare_chunks, controller_target_chunks);
        let target_ready_chunks = vector_ready_chunk_reserve_target(
            configured_target_ready_chunks(),
            claimable_file_backlog_depth
                + refill_state.active_len()
                + owned_prepare_works.len(),
            controller.target_files_per_cycle,
            controller_target_chunks,
            ready_chunk_count,
            inflight_prepare_chunks,
            controller.avg_chunks_per_embed_call,
            ready_queue_summary
                .oldest_prepared_at_ms
                .map(|prepared_at_ms| {
                    chrono::Utc::now()
                        .timestamp_millis()
                        .saturating_sub(prepared_at_ms)
                })
                .unwrap_or(0) as u64,
        );
        let gpu_ready_low_watermark_chunks =
            target_ready_low_watermark_chunks(controller_target_chunks);
        let gpu_ready_high_watermark_chunks =
            target_ready_high_watermark_chunks(controller_target_chunks);
        let gpu_ready_low_watermark = chunk_capacity_to_batch_depth(
            gpu_ready_low_watermark_chunks,
            controller_target_chunks,
        );
        let gpu_push_target_depth = chunk_capacity_to_batch_depth(
            target_ready_chunks.max(gpu_ready_high_watermark_chunks),
            controller_target_chunks,
        );
        let (max_inflight_prepares, request_ready_depth_ceiling) =
            vector_prepare_prefetch_limits(
                configured_max_inflight_prepares,
                gpu_push_target_depth,
            );
        let (_, _, active_replenishment_chunks) = vector_replenishment_command(
            target_ready_chunks,
            ready_chunk_count,
            inflight_prepare_chunks,
            replenishment_deficit,
        );

        let mut uninterrupted = Vec::new();
        for work in refill_state.take_active_works() {
            if work.resumed_after_interactive_pause {
                service_guard::record_vectorization_resumed_after_interactive(1);
            }
            if !pause_vectorization_work_if_interactive(&graph_store, &work) {
                uninterrupted.push(work);
            }
        }
        refill_state.replace_active_works(uninterrupted);
        record_vector_pipeline_snapshot(
            &graph_store,
            claimable_file_backlog_depth,
            refill_state.active_works(),
            refill_state.inflight_prepares(),
            &ready_queue_summary,
            &empty_inflight_persists,
            target_ready_chunks,
            active_replenishment_chunks,
        );

        while continuous_prepare_feed_allowed(
            gpu_memory_pressure,
            ready_batches_equivalent,
            inflight_prepare_batches_equivalent,
            gpu_ready_low_watermark,
            gpu_push_target_depth,
            max_inflight_prepares,
            claimable_file_backlog_depth,
            refill_state.active_len(),
        ) {
            let current_ready_chunk_count = ready_batches.summary().chunk_count;
            let current_inflight_prepare_chunks = refill_state.inflight_prepare_chunk_count();
            let current_ready_depth = chunk_capacity_to_batch_depth(
                current_ready_chunk_count,
                controller_target_chunks,
            );
            let current_inflight_prepare_count = chunk_capacity_to_batch_depth(
                current_inflight_prepare_chunks,
                controller_target_chunks,
            );
            if !gpu_ready_queue_push_allowed(
                current_ready_depth,
                current_inflight_prepare_count,
                gpu_ready_low_watermark,
                gpu_push_target_depth,
            ) {
                break;
            }
            let reserve_gap_chunks = target_ready_chunks.saturating_sub(
                current_ready_chunk_count.saturating_add(current_inflight_prepare_chunks),
            );
            let (_, command_chunks, _) = vector_replenishment_command(
                target_ready_chunks,
                current_ready_chunk_count,
                current_inflight_prepare_chunks,
                replenishment_deficit,
            );
            if command_chunks == 0 {
                break;
            }
            let request_target_chunks =
                replenish_target_chunks(controller_target_chunks, command_chunks);
            let request_target_ready_depth = replenish_target_ready_depth(
                chunk_capacity_to_batch_depth(reserve_gap_chunks, request_target_chunks),
                command_chunks,
                request_target_chunks,
                request_ready_depth_ceiling,
            );
            let request_target_ready_chunks =
                request_target_ready_depth.saturating_mul(request_target_chunks);
            let request_claim_target = vector_claim_target(
                controller.target_files_per_cycle,
                controller.avg_files_per_embed_call,
                request_target_chunks,
                controller.avg_chunks_per_embed_call,
                request_target_ready_depth,
                0,
                claimable_file_backlog_depth + refill_state.active_len(),
            );
            if refill_state.active_len() < request_claim_target {
                let top_up_target = request_claim_target.max(refill_state.active_len());
                if let Err(err) = refill_state.top_up_from_claimable_queue(
                    &graph_store,
                    top_up_target,
                    &[],
                    0,
                ) {
                    error!(
                        "Semantic Vector Refill Worker [{}]: failed to top up claimable vector work for prepare dispatch: {:?}",
                        worker_idx, err
                    );
                    break;
                }
            }
            let available_active = refill_state.active_len();
            if available_active == 0 {
                break;
            }
            let dispatch_claim_target = request_claim_target.min(available_active).max(1);
            if dispatch_claim_target == 0 {
                break;
            }
            match refill_state.dispatch_prepare_request(
                &graph_store,
                &prepare_tx,
                dispatch_claim_target,
                request_target_chunks,
                lane_config.max_chunks_per_file,
                lane_config.max_embed_batch_bytes,
                request_target_ready_depth,
                request_target_ready_chunks,
            ) {
                Ok(true) => wake_idle = false,
                Ok(false) => break,
                Err(err) => {
                    error!(
                        "Semantic Vector Refill Worker [{}]: prepare queue unavailable: {:?}",
                        worker_idx, err
                    );
                    thread::yield_now();
                    break;
                }
            }
        }

        let has_pending_work = refill_state.has_pending(ready_batches_len, 0);
        if has_pending_work || claimable_file_backlog_depth > 0 {
            wake_idle = false;
            let _ = wait_for_vector_backlog_or_timeout(Duration::from_millis(1));
            continue;
        }

        match refill_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(command) => {
                match command {
                    VectorRefillCommand::RequeueWorks(works) => {
                        refill_state.merge_requeued_works(works);
                    }
                    VectorRefillCommand::BatchConsumed(chunks) => {
                        service_guard::record_vector_ready_replenishment_requested(
                            chunks as u64,
                        );
                    }
                }
                wake_idle = false;
            }
            Err(RecvTimeoutError::Timeout) => {
                if wake_idle {
                    let signaled =
                        wait_for_vector_backlog_or_timeout(Duration::from_millis(25));
                    if signaled {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            0,
                            claimable_file_backlog_depth as u64,
                        );
                    }
                }
                wake_idle = true;
            }
            Err(RecvTimeoutError::Disconnected) => return,
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
            crossbeam_channel::Sender<super::VectorPrepareRequest>,
            crossbeam_channel::Receiver<super::VectorRefillCommand>,
            std::sync::Arc<super::SharedPreparedBatchQueue>,
        ) = super::vector_refill_worker_loop;
    }
}
