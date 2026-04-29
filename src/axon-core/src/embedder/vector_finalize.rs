use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result as AnyhowResult;
use crossbeam_channel::{bounded, Sender};
use tracing::{debug, error};

use crate::graph::GraphStore;
use crate::graph_ingestion::{
    FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
    VectorPersistOutboxPayload, VectorPersistOutboxUpdate, VectorPersistOutboxWork,
};
use crate::service_guard;
use crate::vector_pipeline::{FinalizeEnvelope, PersistEnvelope, SharedPreparedBatchQueue};

use super::{
    recover_ready_batches_to_active_works, work_with_lease_snapshots, LeaseRefreshGuard,
    VectorFinalizeRequest, VectorPersistOutcome, VectorPersistOutcomeReply, VectorPersistPlan,
    VectorPersistRequest, CHUNK_MODEL_ID, FILE_PROJECTION_RADIUS,
};

#[derive(Debug)]
pub(super) struct VectorFinalizeOutcome {
    pub(super) completed_works: Vec<FileVectorizationWork>,
    pub(super) batch_runs: Vec<VectorBatchRun>,
}

pub(super) fn persist_vector_embed_batch(
    graph_store: &GraphStore,
    persist_plan: &VectorPersistPlan,
) -> anyhow::Result<VectorPersistOutboxPayload> {
    let completed_lease_snapshots = graph_store
        .capture_file_vectorization_lease_snapshots(&persist_plan.completed_works, "vector")?;
    let handoff_snapshots = completed_lease_snapshots.clone();
    let completed_lease_snapshots = completed_lease_snapshots
        .into_iter()
        .map(|snapshot| FileVectorizationLeaseSnapshot {
            file_path: snapshot.file_path,
            claim_token: snapshot.claim_token,
            lease_epoch: snapshot.lease_epoch.saturating_add(1),
        })
        .collect::<Vec<_>>();
    let payload = VectorPersistOutboxPayload {
        updates: persist_plan
            .updates
            .iter()
            .map(
                |(chunk_id, source_hash, vector)| VectorPersistOutboxUpdate {
                    chunk_id: chunk_id.clone(),
                    source_hash: source_hash.clone(),
                    vector: vector.clone(),
                },
            )
            .collect(),
        completed_works: persist_plan.completed_works.clone(),
        completed_lease_snapshots,
        batch_run: persist_plan.batch_run.clone(),
    };
    graph_store.enqueue_vector_persist_outbox_handoff(&payload, &handoff_snapshots)?;
    Ok(payload)
}

pub(super) fn finalize_completed_vectorization_works(
    graph_store: &GraphStore,
    completed_works: Vec<FileVectorizationWork>,
    lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    batch_runs: Vec<VectorBatchRun>,
) -> Result<VectorFinalizeOutcome, (anyhow::Error, Vec<VectorBatchRun>)> {
    if completed_works.is_empty() {
        return Ok(VectorFinalizeOutcome {
            completed_works,
            batch_runs,
        });
    }

    graph_store
        .finalize_file_vectorization_success_batch(
            &completed_works,
            &lease_snapshots,
            CHUNK_MODEL_ID,
            FILE_PROJECTION_RADIUS,
        )
        .map_err(|err| (err, batch_runs.clone()))?;
    Ok(VectorFinalizeOutcome {
        completed_works,
        batch_runs,
    })
}

pub(super) fn finalize_completed_vectorization_works_for_owner(
    graph_store: &GraphStore,
    completed_works: Vec<FileVectorizationWork>,
    lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    lease_owner: &str,
    batch_runs: Vec<VectorBatchRun>,
) -> Result<VectorFinalizeOutcome, (anyhow::Error, Vec<VectorBatchRun>)> {
    if completed_works.is_empty() {
        return Ok(VectorFinalizeOutcome {
            completed_works,
            batch_runs,
        });
    }

    graph_store
        .finalize_file_vectorization_success_batch_for_owner(
            &completed_works,
            &lease_snapshots,
            lease_owner,
            CHUNK_MODEL_ID,
            FILE_PROJECTION_RADIUS,
        )
        .map_err(|err| (err, batch_runs.clone()))?;
    Ok(VectorFinalizeOutcome {
        completed_works,
        batch_runs,
    })
}

fn record_vector_batch_run_failure(
    graph_store: &GraphStore,
    batch_run: &mut VectorBatchRun,
    reason: String,
    mark_done_ms: u64,
) {
    batch_run.mark_done_ms = mark_done_ms;
    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
    batch_run.success = false;
    batch_run.error_reason = Some(reason);
    if let Err(err) = graph_store.record_vector_batch_run(batch_run) {
        error!("Failed to persist failed vector batch run: {:?}", err);
    }
}

pub(super) fn is_irrecoverable_outbox_finalize_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}");
    message.contains("finalize refused:")
        && (message.contains("outbox-owned rows") || message.contains("lease snapshots"))
}

pub(super) fn reconcile_outbox_finalize_failure(
    graph_store: &GraphStore,
    outbox_id: &str,
    payload: &VectorPersistOutboxPayload,
    err: &anyhow::Error,
    batch_runs: &mut [VectorBatchRun],
    reason: &str,
    mark_done_ms: u64,
) -> AnyhowResult<bool> {
    for batch_run in batch_runs {
        record_vector_batch_run_failure(graph_store, batch_run, reason.to_string(), mark_done_ms);
    }
    if let Err(failure_err) =
        graph_store.mark_file_vectorization_work_failed(&payload.completed_works, reason)
    {
        error!(
            "Semantic Vector Finalize Worker: failed to persist outbox finalize failure state: {:?}",
            failure_err
        );
    }
    if is_irrecoverable_outbox_finalize_error(err) {
        graph_store.mark_vector_persist_outbox_done(outbox_id)?;
        return Ok(true);
    }
    graph_store.mark_vector_persist_outbox_failed(outbox_id, reason)?;
    Ok(false)
}

pub(super) fn process_finalize_request(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    request: VectorFinalizeRequest,
) {
    let finalize_queue_wait_ms = request.enqueued_at.elapsed().as_millis() as u64;
    service_guard::record_vector_finalize_queue_wait_ms(finalize_queue_wait_ms);
    let FinalizeEnvelope {
        completed_works,
        lease_snapshots: vector_lease_snapshots,
        mut batch_runs,
    } = request.envelope;
    let finalize_started_at_ms = chrono::Utc::now().timestamp_millis();
    for batch_run in &mut batch_runs {
        batch_run.finalize_queue_wait_ms = finalize_queue_wait_ms;
        if batch_run.finalize_enqueued_at_ms == 0 {
            batch_run.finalize_enqueued_at_ms = finalize_started_at_ms;
        }
    }
    let finalize_lease_snapshots = match graph_store.transfer_file_vectorization_lease_owner(
        &vector_lease_snapshots,
        "vector",
        "finalize",
    ) {
        Ok(snapshots) => snapshots,
        Err(err) => {
            error!(
                "Semantic Vector Finalize Worker [{}]: failed to claim finalize lease ownership: {:?}",
                worker_idx, err
            );
            for mut batch_run in batch_runs {
                record_vector_batch_run_failure(
                    graph_store,
                    &mut batch_run,
                    format!("failed to claim finalize lease ownership: {:?}", err),
                    0,
                );
            }
            return;
        }
    };
    let finalize_owned_works =
        work_with_lease_snapshots(&completed_works, &finalize_lease_snapshots);
    let _ =
        graph_store.refresh_file_vectorization_leases_for_owner(&finalize_owned_works, "finalize");
    let mark_done_started = Instant::now();
    service_guard::record_vector_mark_done_call();
    let _finalize_lease_guard = LeaseRefreshGuard::start(
        Arc::clone(graph_store),
        finalize_owned_works.clone(),
        "finalize",
    );
    match finalize_completed_vectorization_works(
        graph_store,
        finalize_owned_works.clone(),
        finalize_lease_snapshots,
        batch_runs,
    ) {
        Ok(outcome) => {
            let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::MarkDone,
                mark_done_ms,
            );
            for mut batch_run in outcome.batch_runs {
                batch_run.mark_done_ms = mark_done_ms;
                batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                batch_run.finalize_finished_at_ms = batch_run.finished_at_ms;
                if let Err(err) = graph_store.record_vector_batch_run(&batch_run) {
                    error!(
                        "Semantic Vector Finalize Worker [{}]: failed to persist finalized vector batch run: {:?}",
                        worker_idx, err
                    );
                }
            }
            service_guard::record_vector_files_completed(outcome.completed_works.len() as u64);
        }
        Err((err, mut batch_runs)) => {
            let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::MarkDone,
                mark_done_ms,
            );
            let reason = format!("failed to mark vectorization completion: {:?}", err);
            for batch_run in &mut batch_runs {
                record_vector_batch_run_failure(
                    graph_store,
                    batch_run,
                    reason.clone(),
                    mark_done_ms,
                );
            }
            error!(
                "Semantic Vector Finalize Worker [{}]: {}",
                worker_idx, reason
            );
            if let Err(failure_err) =
                graph_store.mark_file_vectorization_work_failed(&finalize_owned_works, &reason)
            {
                error!(
                    "Semantic Vector Finalize Worker [{}]: failed to persist finalize failure state: {:?}",
                    worker_idx, failure_err
                );
            }
        }
    }
}

pub(super) fn process_vector_persist_outbox_work(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    work: VectorPersistOutboxWork,
) -> AnyhowResult<()> {
    let VectorPersistOutboxWork { outbox_id, payload } = work;
    debug!(
        "Semantic Vector Finalize Worker [{}]: outbox {} starting db_write (updates={}, completed_works={}, batch_run_chunks={}, batch_run_files={})",
        worker_idx,
        outbox_id,
        payload.updates.len(),
        payload.completed_works.len(),
        payload.batch_run.chunk_count,
        payload.batch_run.file_count
    );
    let outbox_ids = vec![outbox_id.clone()];
    let _ = graph_store.refresh_vector_persist_outbox_leases(&outbox_ids);
    let _outbox_file_lease_guard = LeaseRefreshGuard::start(
        Arc::clone(graph_store),
        payload.completed_works.clone(),
        "outbox",
    );
    let db_write_started = Instant::now();
    let updates = payload
        .updates
        .iter()
        .map(|update| {
            (
                update.chunk_id.clone(),
                update.source_hash.clone(),
                update.vector.clone(),
            )
        })
        .collect::<Vec<_>>();
    let write_result = graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates);
    let db_write_ms = db_write_started.elapsed().as_millis() as u64;
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::DbWrite, db_write_ms);
    if let Err(err) = write_result {
        let reason = format!("failed to persist chunk embeddings from outbox: {:?}", err);
        error!(
            "Semantic Vector Finalize Worker [{}]: outbox {} db_write failed after {} ms (updates={}, completed_works={}): {:?}",
            worker_idx,
            outbox_id,
            db_write_ms,
            updates.len(),
            payload.completed_works.len(),
            err
        );
        let _ = graph_store.mark_vector_persist_outbox_failed(&outbox_id, &reason);
        return Err(err);
    }

    let _ = graph_store.refresh_vector_persist_outbox_leases(&outbox_ids);
    let mark_done_started = Instant::now();
    service_guard::record_vector_mark_done_call();
    let mut batch_run = payload.batch_run.clone();
    batch_run.db_write_ms = db_write_ms;
    batch_run.finalize_finished_at_ms = 0;
    match finalize_completed_vectorization_works_for_owner(
        graph_store,
        payload.completed_works.clone(),
        payload.completed_lease_snapshots.clone(),
        "outbox",
        vec![batch_run.clone()],
    ) {
        Ok(outcome) => {
            let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::MarkDone,
                mark_done_ms,
            );
            for mut finished in outcome.batch_runs {
                finished.db_write_ms = db_write_ms;
                finished.mark_done_ms = mark_done_ms;
                finished.finished_at_ms = chrono::Utc::now().timestamp_millis();
                finished.persist_finished_at_ms = finished.finished_at_ms;
                finished.finalize_finished_at_ms = finished.finished_at_ms;
                if let Err(err) = graph_store.record_vector_batch_run(&finished) {
                    error!(
                        "Semantic Vector Finalize Worker [{}]: failed to persist outbox vector batch run: {:?}",
                        worker_idx, err
                    );
                }
            }
            service_guard::record_vector_files_completed(outcome.completed_works.len() as u64);
            if let Err(err) = graph_store.mark_vector_persist_outbox_done(&outbox_id) {
                error!(
                    "Semantic Vector Finalize Worker [{}]: outbox {} finalize succeeded but delete failed: {:?}",
                    worker_idx, outbox_id, err
                );
                return Err(err);
            }
            Ok(())
        }
        Err((err, mut batch_runs)) => {
            let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
            service_guard::record_vector_stage_ms(
                service_guard::VectorStageKind::MarkDone,
                mark_done_ms,
            );
            let reason = format!(
                "failed to finalize outbox vectorization completion: {:?}",
                err
            );
            match reconcile_outbox_finalize_failure(
                graph_store,
                &outbox_id,
                &payload,
                &err,
                &mut batch_runs,
                &reason,
                mark_done_ms,
            ) {
                Ok(true) => Ok(()),
                Ok(false) => Err(err),
                Err(reconcile_err) => Err(reconcile_err),
            }
        }
    }
}

pub(super) fn flush_completed_vectorization_works(
    worker_idx: usize,
    graph_store: &Arc<GraphStore>,
    finalize_tx: &Sender<VectorFinalizeRequest>,
    completed_works: &mut Vec<FileVectorizationWork>,
    completed_batch_runs: &mut Vec<VectorBatchRun>,
    failed: &mut HashMap<String, Vec<FileVectorizationWork>>,
) {
    if completed_works.is_empty() {
        return;
    }

    let works_to_finalize = std::mem::take(completed_works);
    let batch_runs_to_finalize = std::mem::take(completed_batch_runs);
    let vector_lease_snapshots = match graph_store
        .capture_file_vectorization_lease_snapshots(&works_to_finalize, "vector")
    {
        Ok(snapshots) => snapshots,
        Err(err) => {
            failed
                .entry(format!(
                    "failed to capture vector finalize lease snapshots: {:?}",
                    err
                ))
                .or_default()
                .extend(works_to_finalize);
            return;
        }
    };
    let finalize_request = VectorFinalizeRequest {
        envelope: FinalizeEnvelope {
            completed_works: works_to_finalize.clone(),
            lease_snapshots: vector_lease_snapshots.clone(),
            batch_runs: batch_runs_to_finalize
                .into_iter()
                .map(|mut run| {
                    run.finalize_enqueued_at_ms = chrono::Utc::now().timestamp_millis();
                    run
                })
                .collect(),
        },
        enqueued_at: Instant::now(),
    };
    let finalize_send_started = Instant::now();
    if let Err(err) = finalize_tx.send(finalize_request) {
        service_guard::record_vector_finalize_send_wait_ms(
            finalize_send_started.elapsed().as_millis() as u64,
        );
        service_guard::record_vector_finalize_fallback_inline();
        let finalize_lease_snapshots = match graph_store.transfer_file_vectorization_lease_owner(
            &err.0.envelope.lease_snapshots,
            "vector",
            "finalize",
        ) {
            Ok(snapshots) => snapshots,
            Err(transfer_err) => {
                let reason = format!(
                    "failed to transfer finalize lease ownership for inline fallback: {:?}",
                    transfer_err
                );
                failed
                    .entry(reason.clone())
                    .or_default()
                    .extend(err.0.envelope.completed_works.iter().cloned());
                for mut batch_run in err.0.envelope.batch_runs {
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    batch_run.success = false;
                    batch_run.error_reason = Some(reason.clone());
                    if let Err(record_err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist inline finalize ownership error batch run: {:?}",
                            worker_idx, record_err
                        );
                    }
                }
                return;
            }
        };
        let finalize_owned_works =
            work_with_lease_snapshots(&err.0.envelope.completed_works, &finalize_lease_snapshots);
        let _ = graph_store
            .refresh_file_vectorization_leases_for_owner(&finalize_owned_works, "finalize");
        let mark_done_started = Instant::now();
        service_guard::record_vector_mark_done_call();
        let _inline_finalize_lease_guard = LeaseRefreshGuard::start(
            Arc::clone(graph_store),
            finalize_owned_works.clone(),
            "finalize",
        );
        match finalize_completed_vectorization_works(
            graph_store,
            finalize_owned_works,
            finalize_lease_snapshots,
            err.0.envelope.batch_runs,
        ) {
            Err((finalize_err, mut batch_runs)) => {
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::MarkDone,
                    mark_done_started.elapsed().as_millis() as u64,
                );
                let reason = format!(
                    "failed to mark vectorization completion: {:?}",
                    finalize_err
                );
                for batch_run in &mut batch_runs {
                    batch_run.mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    batch_run.success = false;
                    batch_run.error_reason = Some(reason.clone());
                    if let Err(batch_err) = graph_store.record_vector_batch_run(batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist failed inline finalize vector batch run: {:?}",
                            worker_idx, batch_err
                        );
                    }
                }
                failed
                    .entry(reason)
                    .or_default()
                    .extend(works_to_finalize.iter().cloned());
            }
            Ok(outcome) => {
                let mark_done_ms = mark_done_started.elapsed().as_millis() as u64;
                service_guard::record_vector_stage_ms(
                    service_guard::VectorStageKind::MarkDone,
                    mark_done_ms,
                );
                for mut batch_run in outcome.batch_runs {
                    batch_run.mark_done_ms = mark_done_ms;
                    batch_run.finished_at_ms = chrono::Utc::now().timestamp_millis();
                    if let Err(err) = graph_store.record_vector_batch_run(&batch_run) {
                        error!(
                            "Semantic Vector Worker [{}]: failed to persist inline finalized vector batch run: {:?}",
                            worker_idx, err
                        );
                    }
                }
                service_guard::record_vector_files_completed(outcome.completed_works.len() as u64);
            }
        }
    } else {
        service_guard::record_vector_finalize_send_wait_ms(
            finalize_send_started.elapsed().as_millis() as u64,
        );
        service_guard::record_vector_finalize_enqueued();
        service_guard::record_vector_finalize_queue_depth(finalize_tx.len() as u64);
    }
}

pub(super) fn apply_vector_persist_outcome(
    outcome: VectorPersistOutcome,
    ready_batches: &SharedPreparedBatchQueue,
    completed_works: &mut Vec<FileVectorizationWork>,
    completed_batch_runs: &mut Vec<VectorBatchRun>,
    failed: &mut HashMap<String, Vec<FileVectorizationWork>>,
) -> Vec<FileVectorizationWork> {
    if let Some(reason) = outcome.error_reason {
        let recovered_ready_works = recover_ready_batches_to_active_works(ready_batches);
        failed
            .entry(reason)
            .or_default()
            .extend(outcome.touched_works.into_iter());
        let merge_target = outcome
            .next_active_after_failure
            .len()
            .saturating_add(recovered_ready_works.len())
            .max(1);
        return super::merge_vectorization_work(
            outcome.next_active_after_failure,
            recovered_ready_works,
            merge_target,
        );
    } else {
        completed_works.extend(outcome.completed_works);
        completed_batch_runs.extend(outcome.batch_runs);
    }
    Vec::new()
}

pub(super) fn dispatch_vector_persist_plan(
    persist_tx: &Sender<VectorPersistRequest>,
    mut envelope: PersistEnvelope,
) -> AnyhowResult<VectorPersistOutcomeReply> {
    let (reply_tx, reply_rx) = bounded(1);
    envelope.batch_run.persist_enqueued_at_ms = chrono::Utc::now().timestamp_millis();
    let persist_send_started = Instant::now();
    persist_tx
        .send(VectorPersistRequest {
            envelope,
            enqueued_at: Instant::now(),
            reply: reply_tx,
        })
        .map_err(|err| anyhow::anyhow!("persist worker unavailable: {}", err))?;
    service_guard::record_vector_persist_send_wait_ms(
        persist_send_started.elapsed().as_millis() as u64
    );
    service_guard::record_vector_persist_queue_depth(persist_tx.len() as u64);
    Ok(reply_rx)
}
