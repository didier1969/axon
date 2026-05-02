//! Vector persist worker loop — extracted from embedder.rs (REQ-AXO-080 Phase 4).
//!
//! Pure structural extraction of `vector_persist_worker_loop` from the embedder
//! impl block to its own submodule. Behaviour-preserving move; the function
//! body is verbatim. Free helpers from the parent module are visible via
//! `use super::*`. The cross-submodule call to `persist_vector_embed_batch`
//! (in sibling vector_finalize) is reached via the explicit path.

use super::vector_finalize::persist_vector_embed_batch;
use super::*;

pub(super) fn vector_persist_worker_loop(
    worker_idx: usize,
    graph_store: Arc<GraphStore>,
    persist_rx: Receiver<VectorPersistRequest>,
) {
    info!(
        "Semantic Vector Persist Worker [{}]: ready with bounded queue {}",
        worker_idx,
        configured_vector_persist_queue_bound()
    );
    let mut wake_idle = true;
    while let Ok(request) = persist_rx.recv() {
        if wake_idle {
            service_guard::record_runtime_wakeup(
                service_guard::RuntimeWakeSource::SemanticVector,
                0,
                1,
            );
            wake_idle = false;
        }
        service_guard::record_vector_persist_queue_depth(persist_rx.len() as u64);
        let persist_queue_wait_ms = request.enqueued_at.elapsed().as_millis() as u64;
        service_guard::record_vector_persist_queue_wait_ms(persist_queue_wait_ms);
        let db_write_started = Instant::now();
        let PersistEnvelope {
            persist_plan,
            mut batch_run,
        } = request.envelope;
        batch_run.persist_started_at_ms = chrono::Utc::now().timestamp_millis();
        batch_run.persist_queue_wait_ms = persist_queue_wait_ms;
        let _persist_lease_guard = LeaseRefreshGuard::start(
            Arc::clone(&graph_store),
            persist_plan.touched_works.clone(),
            "vector",
        );
        let outcome = match persist_vector_embed_batch(&graph_store, &persist_plan) {
            Ok(payload) => {
                let db_write_ms = db_write_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::DbWrite,
                    db_write_ms,
                );
                service_guard::record_vector_embed_call(
                    payload.updates.len() as u64,
                    persist_plan.touched_works.len() as u64,
                );
                service_guard::notify_vector_backlog_activity();
                batch_run.db_write_ms = db_write_ms;
                VectorPersistOutcome {
                    completed_works: Vec::new(),
                    batch_runs: Vec::new(),
                    next_active_after_failure: persist_plan.next_active_after_failure,
                    touched_works: persist_plan.touched_works,
                    error_reason: None,
                }
            }
            Err(err) => {
                let db_write_ms = db_write_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::DbWrite,
                    db_write_ms,
                );
                batch_run.db_write_ms = db_write_ms;
                batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                batch_run.persist_finished_at_ms = batch_run.finished_at_ms;
                batch_run.success = false;
                batch_run.error_reason =
                    Some(format!("failed to persist chunk embeddings: {:?}", err));
                if let Err(batch_err) = graph_store.record_vector_batch_run(&batch_run) {
                    error!(
                        "Semantic Vector Persist Worker [{}]: failed to persist failed vector batch run: {:?}",
                        worker_idx, batch_err
                    );
                }
                VectorPersistOutcome {
                    completed_works: Vec::new(),
                    batch_runs: Vec::new(),
                    next_active_after_failure: persist_plan.next_active_after_failure,
                    touched_works: persist_plan.touched_works,
                    error_reason: Some(format!(
                        "failed to persist chunk embeddings: {:?}",
                        err
                    )),
                }
            }
        };
        if request.reply.send(outcome).is_err() {
            error!(
                "Semantic Vector Persist Worker [{}]: vector worker dropped persist reply channel",
                worker_idx
            );
        }
        if persist_rx.is_empty() {
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
            crossbeam_channel::Receiver<super::VectorPersistRequest>,
        ) = super::vector_persist_worker_loop;
    }
}
