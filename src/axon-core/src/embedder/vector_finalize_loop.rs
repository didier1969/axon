//! Vector finalize worker loop — extracted from embedder.rs (REQ-AXO-080 Phase 5).
//!
//! Pure structural extraction of `vector_finalize_worker_loop` and its tightly
//! coupled helper `process_vector_persist_outbox` from the embedder impl
//! block to their own submodule. Behaviour-preserving move; bodies verbatim.
//! The helper is in this same file because the loop is its only caller.
//!
//! Helpers from the parent module (process_finalize_request,
//! process_vector_persist_outbox_work, configured_vector_outbox_fetch_batch_size,
//! vector_finalize_idle_poll_interval_ms, VECTOR_FINALIZE_QUEUE_BOUND, types)
//! reach this submodule via `use super::*`.

use super::*;

pub(super) fn vector_finalize_worker_loop(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    finalize_rx: Receiver<VectorFinalizeRequest>,
) {
    info!(
        "Semantic Vector Finalize Worker [{}]: ready with bounded queue {}",
        worker_idx, VECTOR_FINALIZE_QUEUE_BOUND
    );
    let mut wake_idle = true;
    loop {
        service_guard::record_vector_finalize_queue_depth(finalize_rx.len() as u64);
        match finalize_rx.recv_timeout(Duration::from_millis(
            vector_finalize_idle_poll_interval_ms(),
        )) {
            Ok(request) => {
                if wake_idle {
                    service_guard::record_runtime_wakeup(
                        service_guard::RuntimeWakeSource::SemanticVector,
                        0,
                        1,
                    );
                }
                service_guard::record_vector_finalize_queue_wait_ms(
                    request.enqueued_at.elapsed().as_millis() as u64,
                );
                process_finalize_request(worker_idx, &graph_store, request);
                while let Ok(request) = finalize_rx.try_recv() {
                    service_guard::record_vector_finalize_queue_wait_ms(
                        request.enqueued_at.elapsed().as_millis() as u64,
                    );
                    process_finalize_request(worker_idx, &graph_store, request);
                }
                while process_vector_persist_outbox(worker_idx, &graph_store) > 0 {}
                wake_idle = finalize_rx.is_empty();
            }
            Err(RecvTimeoutError::Timeout) => {
                let mut processed_any = false;
                while process_vector_persist_outbox(worker_idx, &graph_store) > 0 {
                    processed_any = true;
                }
                if processed_any {
                    if wake_idle {
                        service_guard::record_runtime_wakeup(
                            service_guard::RuntimeWakeSource::SemanticVector,
                            0,
                            1,
                        );
                    }
                    wake_idle = false;
                } else {
                    wake_idle = true;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn process_vector_persist_outbox(worker_idx: usize, graph_store: &Arc<GraphStore>) -> usize {
    let pending = match graph_store
        .fetch_pending_vector_persist_outbox_work(configured_vector_outbox_fetch_batch_size())
    {
        Ok(pending) => pending,
        Err(err) => {
            error!(
                "Semantic Vector Finalize Worker [{}]: failed to fetch outbox work: {:?}",
                worker_idx, err
            );
            return 0;
        }
    };
    let processed = pending.len();
    for work in pending {
        if let Err(err) = process_vector_persist_outbox_work(worker_idx, graph_store, work) {
            error!(
                "Semantic Vector Finalize Worker [{}]: outbox work failed: {:?}",
                worker_idx, err
            );
        }
    }
    processed
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracted_function_links_to_runtime() {
        let _: fn(
            usize,
            std::sync::Arc<crate::graph::GraphStore>,
            crossbeam_channel::Receiver<super::VectorFinalizeRequest>,
        ) = super::vector_finalize_worker_loop;
    }
}
