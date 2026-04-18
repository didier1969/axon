// Copyright (c) Didier Stadelmann. All rights reserved.

use crossbeam_channel::{Receiver, Sender};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use tracing::{error, info};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::{estimate_observed_cost_bytes, ProcessingMode, QueueStore};

// Payload for the Writer Actor
#[derive(Debug, Clone)]
pub enum DbWriteTask {
    FileExtraction {
        reservation_id: String,
        path: String,
        content: Option<String>,
        extraction: crate::parser::ExtractionResult,
        processing_mode: ProcessingMode,
        trace_id: String,
        observed_cost_bytes: u64,
        t0: i64,
        t1: i64,
        t2: i64,
        t3: i64,
    },
    FileSkipped {
        reservation_id: String,
        path: String,
        reason: String,
        trace_id: String,
        observed_cost_bytes: Option<u64>,
        t0: i64,
        t1: i64,
        t2: i64,
    },
    ExecuteCypher {
        query: String,
    },
}

pub struct WorkerPool {
    _workers: Vec<thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskDispatchOutcome {
    Enqueued,
    Rejected(Option<u64>),
}

impl WorkerPool {
    fn infer_project_code(graph_store: &GraphStore, path: &str) -> Option<String> {
        crate::project_meta::resolve_registered_project_identity_for_path(
            graph_store,
            std::path::Path::new(path),
        )
        .ok()
        .map(|identity| identity.code)
    }

    fn normalize_project_code(
        graph_store: &GraphStore,
        project_code: Option<String>,
        path: &str,
    ) -> Option<String> {
        let normalized = project_code
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| {
                if crate::project_meta::is_valid_project_code(value) {
                    crate::project_meta::resolve_registered_project_identity(graph_store, value)
                        .ok()
                        .map(|identity| identity.code)
                } else {
                    None
                }
            });

        normalized.or_else(|| Self::infer_project_code(graph_store, path))
    }

    fn normalize_extraction_project_code(
        graph_store: &GraphStore,
        extraction: &mut crate::parser::ExtractionResult,
        path: &str,
    ) -> anyhow::Result<()> {
        extraction.project_code =
            Self::normalize_project_code(graph_store, extraction.project_code.clone(), path);
        extraction.project_code.as_ref().map(|_| ()).ok_or_else(|| {
            anyhow::anyhow!(
                "Chemin `{}` rejeté: aucun projet canonique enregistré ne correspond",
                path
            )
        })
    }

    pub fn new(
        count: usize,
        queue: Arc<QueueStore>,
        graph_store: Arc<GraphStore>,
        db_sender: Sender<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) -> Self {
        let mut workers = Vec::new();
        for i in 0..count {
            let q = queue.clone();
            let gs = graph_store.clone();
            let d_tx = db_sender.clone();
            let r_tx = result_sender.clone();

            // NEXUS v8.17: Instant Ignition (Shared Model)
            info!("WorkerPool: Spawning worker {}/{}...", i + 1, count);
            workers.push(thread::spawn(move || {
                Self::worker_loop(i, q, gs, d_tx, r_tx);
            }));
        }
        Self { _workers: workers }
    }

    fn worker_loop(
        id: usize,
        queue: Arc<QueueStore>,
        _graph_store: Arc<GraphStore>,
        db_sender: Sender<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) {
        info!("Worker {} online. (Prefetching enabled)", id);
        let mut local_buffer = Vec::with_capacity(2);

        loop {
            // 1. Refill local buffer if needed
            while local_buffer.len() < 2 {
                if let Some(task) = queue.try_pop() {
                    local_buffer.push(task);
                } else {
                    break;
                }
            }

            // 2. Process first task if available
            if !local_buffer.is_empty() {
                let task = local_buffer.remove(0);
                match Self::process_one_task(id, &task, &_graph_store, &db_sender, &result_sender) {
                    TaskDispatchOutcome::Enqueued => {}
                    TaskDispatchOutcome::Rejected(observed_cost_bytes) => {
                        let _ = queue.mark_done(&task, observed_cost_bytes);
                    }
                }
            } else {
                // If really empty, wait a bit longer to save CPU
                thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    pub fn process_one_task(
        _worker_id: usize,
        task: &crate::queue::Task,
        graph_store: &Arc<GraphStore>,
        db_sender: &Sender<DbWriteTask>,
        _result_sender: &tokio::sync::broadcast::Sender<String>,
    ) -> TaskDispatchOutcome {
        let _span = tracing::info_span!("worker_task", path = %task.path).entered();
        let t2 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64;
        let started_at = Instant::now();

        match std::fs::read_to_string(&task.path) {
            Ok(content) => {
                if let Some(parser) = parser::get_parser_for_file(std::path::Path::new(&task.path))
                {
                    let mut extraction = parser.parse(&content);
                    if let Err(err) = Self::normalize_extraction_project_code(
                        graph_store,
                        &mut extraction,
                        &task.path,
                    ) {
                        let observed_cost_bytes = estimate_observed_cost_bytes(
                            &task.path,
                            task.size_bytes.max(content.len() as u64),
                            started_at.elapsed(),
                            task.mode,
                        );
                        let _ = db_sender.send(DbWriteTask::FileSkipped {
                            reservation_id: task.reservation_id.clone(),
                            path: task.path.clone(),
                            reason: format!("unknown_project_identity: {}", err),
                            trace_id: task.trace_id.clone(),
                            observed_cost_bytes: Some(observed_cost_bytes),
                            t0: task.t0,
                            t1: task.t1,
                            t2,
                        });
                        return TaskDispatchOutcome::Enqueued;
                    }
                    let t3 = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_micros() as i64;
                    let observed_cost_bytes = estimate_observed_cost_bytes(
                        &task.path,
                        task.size_bytes.max(content.len() as u64),
                        started_at.elapsed(),
                        task.mode,
                    );
                    let content_for_writer = match task.mode {
                        ProcessingMode::Full => Some(content),
                        ProcessingMode::StructureOnly => None,
                    };

                    if db_sender
                        .send(DbWriteTask::FileExtraction {
                            reservation_id: task.reservation_id.clone(),
                            path: task.path.clone(),
                            content: content_for_writer,
                            extraction,
                            processing_mode: task.mode,
                            trace_id: task.trace_id.clone(),
                            observed_cost_bytes,
                            t0: task.t0,
                            t1: task.t1,
                            t2,
                            t3,
                        })
                        .is_err()
                    {
                        let _ = graph_store.requeue_claimed_file_with_reason(
                            &task.path,
                            "requeued_after_writer_channel_failure",
                        );
                        return TaskDispatchOutcome::Rejected(Some(observed_cost_bytes));
                    }

                    let _ = graph_store.mark_claimed_file_writer_pending_commit(&task.path);
                    TaskDispatchOutcome::Enqueued
                } else {
                    // Fallback for non-supported but discovered files
                    if db_sender
                        .send(DbWriteTask::FileSkipped {
                            reservation_id: task.reservation_id.clone(),
                            path: task.path.clone(),
                            reason: "No parser found".to_string(),
                            trace_id: task.trace_id.clone(),
                            observed_cost_bytes: Some(task.estimated_cost_bytes),
                            t0: task.t0,
                            t1: task.t1,
                            t2,
                        })
                        .is_err()
                    {
                        let _ = graph_store.requeue_claimed_file_with_reason(
                            &task.path,
                            "requeued_after_writer_channel_failure",
                        );
                        return TaskDispatchOutcome::Rejected(Some(task.estimated_cost_bytes));
                    }

                    let _ = graph_store.mark_claimed_file_writer_pending_commit(&task.path);
                    TaskDispatchOutcome::Enqueued
                }
            }
            Err(e) => {
                if db_sender
                    .send(DbWriteTask::FileSkipped {
                        reservation_id: task.reservation_id.clone(),
                        path: task.path.clone(),
                        reason: format!("Read Error: {:?}", e),
                        trace_id: task.trace_id.clone(),
                        observed_cost_bytes: None,
                        t0: task.t0,
                        t1: task.t1,
                        t2,
                    })
                    .is_err()
                {
                    let _ = graph_store.requeue_claimed_file_with_reason(
                        &task.path,
                        "requeued_after_writer_channel_failure",
                    );
                    return TaskDispatchOutcome::Rejected(None);
                }

                let _ = graph_store.mark_claimed_file_writer_pending_commit(&task.path);
                TaskDispatchOutcome::Enqueued
            }
        }
    }

    pub fn spawn_writer_actor(
        graph_store: Arc<GraphStore>,
        queue: Arc<QueueStore>,
        db_receiver: Receiver<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) {
        thread::spawn(move || {
            info!("DB Writer Actor online. Transactional pipeline ready.");
            let mut batch = Vec::new();

            loop {
                // 1. BLOCKING WAIT for first message
                match db_receiver.recv() {
                    Ok(DbWriteTask::FileExtraction {
                        reservation_id,
                        path,
                        content,
                        extraction,
                        processing_mode,
                        trace_id,
                        observed_cost_bytes,
                        t0,
                        t1,
                        t2,
                        t3,
                    }) => {
                        batch.push(DbWriteTask::FileExtraction {
                            reservation_id,
                            path,
                            content,
                            extraction,
                            processing_mode,
                            trace_id,
                            observed_cost_bytes,
                            t0,
                            t1,
                            t2,
                            t3,
                        });
                    }
                    Ok(DbWriteTask::FileSkipped {
                        reservation_id,
                        path,
                        reason,
                        trace_id,
                        observed_cost_bytes,
                        t0,
                        t1,
                        t2,
                    }) => {
                        batch.push(DbWriteTask::FileSkipped {
                            reservation_id,
                            path,
                            reason,
                            trace_id,
                            observed_cost_bytes,
                            t0,
                            t1,
                            t2,
                        });
                    }
                    Ok(DbWriteTask::ExecuteCypher { query }) => {
                        let _ = graph_store.execute(&query);
                    }
                    Err(_) => break,
                }

                // 2. FILL BATCH up to 100
                while batch.len() < 100 {
                    match db_receiver.try_recv() {
                        Ok(task) => batch.push(task),
                        _ => break,
                    }
                }

                // 3. COMMIT BATCH
                if !batch.is_empty() {
                    let commit_batch = consolidate_writer_batch(&batch);
                    if let Err(e) = graph_store.insert_file_data_batch(&commit_batch) {
                        error!("Writer Actor: Batch commit failed: {:?}", e);
                        let claimed_paths = claimed_paths_from_batch(&batch);
                        if let Err(requeue_err) = graph_store.requeue_claimed_paths_with_reason(
                            &claimed_paths,
                            "requeued_after_writer_batch_failure",
                        ) {
                            error!(
                                "Writer Actor: failed to requeue {} claimed files after batch failure: {:?}",
                                claimed_paths.len(),
                                requeue_err
                            );
                        }
                        release_writer_batch_reservations(&queue, &batch);
                    } else {
                        for feedback in build_feedback_messages(&commit_batch) {
                            let _ = result_sender.send(feedback);
                        }
                        release_writer_batch_reservations(&queue, &batch);
                    }
                    batch.clear();
                }
            }
        });
    }
}

fn build_feedback_messages(batch: &[DbWriteTask]) -> Vec<String> {
    let mut feedback_messages = Vec::new();
    for task in batch {
        if let DbWriteTask::FileExtraction {
            path,
            extraction,
            processing_mode,
            trace_id,
            t0,
            t1,
            t2,
            t3,
            ..
        } = task
        {
            let t4 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;
            let queue_wait_us = (t2 - t1).max(0);
            let parse_us = (t3 - t2).max(0);
            let commit_us = (t4 - t3).max(0);
            let msg = serde_json::json!({
                "FileIndexed": {
                    "path": path,
                    "status": if matches!(processing_mode, ProcessingMode::StructureOnly) { "indexed_degraded" } else { "ok" },
                    "processing_mode": match processing_mode {
                        ProcessingMode::Full => "full",
                        ProcessingMode::StructureOnly => "structure_only",
                    },
                    "symbol_count": extraction.symbols.len(),
                    "relation_count": extraction.relations.len(), "trace_id": trace_id,
                    "t0": t0, "t1": t1, "t2": t2, "t3": t3, "t4": t4,
                    "queue_wait_us": queue_wait_us,
                    "parse_us": parse_us,
                    "commit_us": commit_us
                }
            });
            feedback_messages.push(format!("{}\n", serde_json::to_string(&msg).unwrap()));
        } else if let DbWriteTask::FileSkipped {
            path,
            reason,
            trace_id,
            t0,
            t1,
            t2,
            ..
        } = task
        {
            let t4 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;
            let queue_wait_us = (t2 - t1).max(0);
            let parse_us = 0;
            let commit_us = (t4 - t2).max(0);
            let msg = serde_json::json!({
                "FileIndexed": {
                    "path": path, "status": "skipped", "error_reason": reason,
                    "trace_id": trace_id,
                    "t0": t0, "t1": t1, "t2": t2, "t3": t2, "t4": t4,
                    "queue_wait_us": queue_wait_us,
                    "parse_us": parse_us,
                    "commit_us": commit_us
                }
            });
            feedback_messages.push(format!("{}\n", serde_json::to_string(&msg).unwrap()));
        }
    }
    feedback_messages
}

fn consolidate_writer_batch(batch: &[DbWriteTask]) -> Vec<DbWriteTask> {
    let mut latest_by_path = HashMap::new();
    let mut passthrough = Vec::new();

    for (idx, task) in batch.iter().enumerate() {
        match task {
            DbWriteTask::FileExtraction { path, .. } | DbWriteTask::FileSkipped { path, .. } => {
                latest_by_path.insert(path.clone(), idx);
            }
            DbWriteTask::ExecuteCypher { .. } => passthrough.push((idx, task)),
        }
    }

    let mut consolidated = Vec::new();
    for (idx, task) in batch.iter().enumerate() {
        match task {
            DbWriteTask::FileExtraction { path, .. } | DbWriteTask::FileSkipped { path, .. } => {
                if latest_by_path.get(path).copied() == Some(idx) {
                    consolidated.push(clone_db_write_task(task));
                }
            }
            DbWriteTask::ExecuteCypher { .. } => {
                if passthrough
                    .iter()
                    .any(|(passthrough_idx, _)| *passthrough_idx == idx)
                {
                    consolidated.push(clone_db_write_task(task));
                }
            }
        }
    }

    consolidated
}

fn clone_db_write_task(task: &DbWriteTask) -> DbWriteTask {
    match task {
        DbWriteTask::FileExtraction {
            reservation_id,
            path,
            content,
            extraction,
            processing_mode,
            trace_id,
            observed_cost_bytes,
            t0,
            t1,
            t2,
            t3,
        } => DbWriteTask::FileExtraction {
            reservation_id: reservation_id.clone(),
            path: path.clone(),
            content: content.clone(),
            extraction: extraction.clone(),
            processing_mode: *processing_mode,
            trace_id: trace_id.clone(),
            observed_cost_bytes: *observed_cost_bytes,
            t0: *t0,
            t1: *t1,
            t2: *t2,
            t3: *t3,
        },
        DbWriteTask::FileSkipped {
            reservation_id,
            path,
            reason,
            trace_id,
            observed_cost_bytes,
            t0,
            t1,
            t2,
        } => DbWriteTask::FileSkipped {
            reservation_id: reservation_id.clone(),
            path: path.clone(),
            reason: reason.clone(),
            trace_id: trace_id.clone(),
            observed_cost_bytes: *observed_cost_bytes,
            t0: *t0,
            t1: *t1,
            t2: *t2,
        },
        DbWriteTask::ExecuteCypher { query } => DbWriteTask::ExecuteCypher {
            query: query.clone(),
        },
    }
}

fn release_writer_batch_reservations(queue: &Arc<QueueStore>, batch: &[DbWriteTask]) {
    for task in batch {
        match task {
            DbWriteTask::FileExtraction {
                reservation_id,
                observed_cost_bytes,
                ..
            } => {
                let _ = queue.mark_done_by_reservation(reservation_id, Some(*observed_cost_bytes));
            }
            DbWriteTask::FileSkipped {
                reservation_id,
                observed_cost_bytes,
                ..
            } => {
                let _ = queue.mark_done_by_reservation(reservation_id, *observed_cost_bytes);
            }
            DbWriteTask::ExecuteCypher { .. } => {}
        }
    }
}

fn claimed_paths_from_batch(batch: &[DbWriteTask]) -> Vec<String> {
    let mut paths = Vec::new();
    for task in batch {
        match task {
            DbWriteTask::FileExtraction { path, .. } | DbWriteTask::FileSkipped { path, .. } => {
                if !paths.iter().any(|existing| existing == path) {
                    paths.push(path.clone());
                }
            }
            DbWriteTask::ExecuteCypher { .. } => {}
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::{consolidate_writer_batch, DbWriteTask, TaskDispatchOutcome, WorkerPool};
    use crate::parser::ExtractionResult;
    use crate::queue::TaskLane;
    use crate::queue::{ProcessingMode, QueueStore, Task};
    use std::sync::Arc;

    #[test]
    fn test_large_file_is_not_skipped_when_budget_admitted_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.txt");
        let mut content = String::from("header\n");
        content.push_str(&"x".repeat(1_200_000));
        std::fs::write(&path, content).unwrap();

        let task = Task {
            reservation_id: "res-large".to_string(),
            path: path.to_string_lossy().to_string(),
            trace_id: "trace-large".to_string(),
            lane: TaskLane::Bulk,
            size_bytes: 1_200_007,
            estimated_cost_bytes: 400 * 1024 * 1024,
            parser_key: "txt".to_string(),
            t0: 0,
            t1: 0,
            t2: 0,
            mode: ProcessingMode::Full,
        };

        let graph = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (db_sender, db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);

        let outcome = WorkerPool::process_one_task(0, &task, &graph, &db_sender, &results_tx);
        assert_eq!(outcome, TaskDispatchOutcome::Enqueued);

        match db_receiver.recv().unwrap() {
            DbWriteTask::FileExtraction {
                path, extraction, ..
            } => {
                assert!(path.ends_with("large.txt"));
                assert!(
                    !extraction.symbols.is_empty(),
                    "large file should still be parsed when budget admitted it"
                );
            }
            DbWriteTask::FileSkipped { reason, .. } => {
                panic!(
                    "large file should no longer be skipped by a fixed size gate: {}",
                    reason
                );
            }
            DbWriteTask::ExecuteCypher { .. } => {
                panic!("unexpected cypher task");
            }
        }
    }

    #[test]
    fn test_structure_only_mode_avoids_sending_full_content_to_writer() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large_structure_only.txt");
        let mut content = String::from("header\n");
        content.push_str(&"x".repeat(128_000));
        std::fs::write(&path, content).unwrap();

        let task = Task {
            reservation_id: "res-structure-only".to_string(),
            path: path.to_string_lossy().to_string(),
            trace_id: "trace-structure-only".to_string(),
            lane: TaskLane::Bulk,
            size_bytes: 128_007,
            estimated_cost_bytes: 50 * 1024 * 1024,
            parser_key: "txt".to_string(),
            t0: 0,
            t1: 0,
            t2: 0,
            mode: ProcessingMode::StructureOnly,
        };

        let graph = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let (db_sender, db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);

        let outcome = WorkerPool::process_one_task(0, &task, &graph, &db_sender, &results_tx);
        assert_eq!(outcome, TaskDispatchOutcome::Enqueued);

        match db_receiver.recv().unwrap() {
            DbWriteTask::FileExtraction {
                path,
                content,
                processing_mode,
                ..
            } => {
                assert!(path.ends_with("large_structure_only.txt"));
                assert!(content.is_none(), "structure-only degradation should not retain full file contents for downstream writes");
                assert_eq!(processing_mode, ProcessingMode::StructureOnly);
            }
            DbWriteTask::FileSkipped { reason, .. } => {
                panic!(
                    "structure-only mode should still parse the file: {}",
                    reason
                );
            }
            DbWriteTask::ExecuteCypher { .. } => {
                panic!("unexpected cypher task");
            }
        }
    }

    #[test]
    fn claimed_paths_from_batch_deduplicates_file_paths() {
        let batch = vec![
            DbWriteTask::FileSkipped {
                reservation_id: "res-a".to_string(),
                path: "/tmp/a.ex".to_string(),
                reason: "No parser found".to_string(),
                trace_id: "trace-a".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            },
            DbWriteTask::FileExtraction {
                reservation_id: "res-b".to_string(),
                path: "/tmp/a.ex".to_string(),
                content: Some("defmodule A do end".to_string()),
                extraction: crate::parser::ExtractionResult::default(),
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-a".to_string(),
                observed_cost_bytes: 42,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            },
            DbWriteTask::FileSkipped {
                reservation_id: "res-c".to_string(),
                path: "/tmp/b.ex".to_string(),
                reason: "No parser found".to_string(),
                trace_id: "trace-b".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            },
        ];

        let claimed = super::claimed_paths_from_batch(&batch);
        assert_eq!(
            claimed,
            vec!["/tmp/a.ex".to_string(), "/tmp/b.ex".to_string()]
        );
    }

    #[test]
    fn consolidate_writer_batch_keeps_only_latest_task_per_path() {
        let batch = vec![
            DbWriteTask::FileSkipped {
                reservation_id: "res-a-old".to_string(),
                path: "/tmp/a.ex".to_string(),
                reason: "old".to_string(),
                trace_id: "trace-a-old".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            },
            DbWriteTask::FileExtraction {
                reservation_id: "res-a-new".to_string(),
                path: "/tmp/a.ex".to_string(),
                content: Some("new".to_string()),
                extraction: ExtractionResult::default(),
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-a-new".to_string(),
                observed_cost_bytes: 7,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            },
            DbWriteTask::FileSkipped {
                reservation_id: "res-b".to_string(),
                path: "/tmp/b.ex".to_string(),
                reason: "skip".to_string(),
                trace_id: "trace-b".to_string(),
                observed_cost_bytes: None,
                t0: 0,
                t1: 0,
                t2: 0,
            },
        ];

        let consolidated = consolidate_writer_batch(&batch);
        assert_eq!(consolidated.len(), 2);

        match &consolidated[0] {
            DbWriteTask::FileExtraction {
                reservation_id,
                path,
                ..
            } => {
                assert_eq!(reservation_id, "res-a-new");
                assert_eq!(path, "/tmp/a.ex");
            }
            other => panic!("expected latest extraction for /tmp/a.ex, got {other:?}"),
        }

        match &consolidated[1] {
            DbWriteTask::FileSkipped {
                reservation_id,
                path,
                ..
            } => {
                assert_eq!(reservation_id, "res-b");
                assert_eq!(path, "/tmp/b.ex");
            }
            other => panic!("expected skipped task for /tmp/b.ex, got {other:?}"),
        }
    }

    #[test]
    fn build_feedback_messages_emits_one_line_per_task() {
        let batch = vec![
            DbWriteTask::FileExtraction {
                reservation_id: "res-a".to_string(),
                path: "/tmp/a.ex".to_string(),
                content: Some("defmodule A do end".to_string()),
                extraction: crate::parser::ExtractionResult::default(),
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-a".to_string(),
                observed_cost_bytes: 42,
                t0: 1,
                t1: 2,
                t2: 3,
                t3: 4,
            },
            DbWriteTask::FileSkipped {
                reservation_id: "res-b".to_string(),
                path: "/tmp/b.ex".to_string(),
                reason: "No parser found".to_string(),
                trace_id: "trace-b".to_string(),
                observed_cost_bytes: None,
                t0: 1,
                t1: 2,
                t2: 3,
            },
        ];

        let messages = super::build_feedback_messages(&batch);
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|message| message.ends_with('\n')));
        assert!(messages[0].contains("\"FileIndexed\""));
        assert!(messages[0].contains("/tmp/a.ex"));
        assert!(messages[1].contains("\"status\":\"skipped\""));
        assert!(messages[1].contains("/tmp/b.ex"));
    }

    #[test]
    fn process_one_task_keeps_reservation_until_writer_ack() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("writer_ack.rs");
        std::fs::write(&path, "defmodule WriterAck do\nend\n").unwrap();

        let graph = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        graph
            .bulk_insert_files(&[(path.to_string_lossy().to_string(), "PRJ".to_string(), 32, 1)])
            .unwrap();
        graph
            .claim_pending_paths(&[path.to_string_lossy().to_string()])
            .unwrap();

        let queue = QueueStore::with_memory_budget(10, 64 * 1024 * 1024);
        queue
            .push(
                path.to_string_lossy().as_ref(),
                1,
                "trace-writer-ack",
                0,
                0,
                false,
            )
            .unwrap();
        let task = queue.pop().unwrap();
        let reserved_before = queue.memory_budget_snapshot().reserved_bytes;
        assert!(reserved_before > 0);

        let (db_sender, db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);

        let outcome = WorkerPool::process_one_task(0, &task, &graph, &db_sender, &results_tx);
        assert_eq!(outcome, TaskDispatchOutcome::Enqueued);
        assert_eq!(
            queue.memory_budget_snapshot().reserved_bytes,
            reserved_before,
            "writer backlog must still hold the reservation until commit ack"
        );

        let queued = db_receiver.recv().unwrap();
        super::release_writer_batch_reservations(&Arc::new(queue), &[queued]);
    }

    #[test]
    fn process_one_task_keeps_file_claimed_until_writer_commit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("writer_stage.rs");
        std::fs::write(&path, "defmodule WriterStage do\nend\n").unwrap();
        let path_str = path.to_string_lossy().to_string();

        let graph = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        graph
            .bulk_insert_files(&[(path_str.clone(), "PRJ".to_string(), 32, 1)])
            .unwrap();
        graph
            .claim_pending_paths(std::slice::from_ref(&path_str))
            .unwrap();

        let task = Task {
            reservation_id: "res-writer-stage".to_string(),
            path: path_str.clone(),
            trace_id: "trace-writer-stage".to_string(),
            lane: TaskLane::Bulk,
            size_bytes: 32,
            estimated_cost_bytes: 1024,
            parser_key: "ex".to_string(),
            t0: 0,
            t1: 0,
            t2: 0,
            mode: ProcessingMode::Full,
        };

        let (db_sender, _db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);
        let outcome = WorkerPool::process_one_task(0, &task, &graph, &db_sender, &results_tx);
        assert_eq!(outcome, TaskDispatchOutcome::Enqueued);

        let row = graph
            .query_json_writer(&format!(
                "SELECT status, status_reason, file_stage FROM File WHERE path = '{}'",
                path_str.replace('\'', "''")
            ))
            .unwrap();
        assert!(row.contains("indexing"), "{row}");
        assert!(row.contains("claimed_for_indexing"), "{row}");
        assert!(row.contains("claimed"), "{row}");
    }

    #[test]
    fn normalize_project_code_prefers_canonical_code_for_repo_path() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|path| path.parent())
            .expect("repo root");
        let worker_path = repo_root.join("src/axon-core/src/worker.rs");

        let normalized = WorkerPool::normalize_project_code(
            &graph,
            Some("axon".to_string()),
            worker_path.to_string_lossy().as_ref(),
        );

        assert_eq!(normalized.as_deref(), Some("AXO"));
    }

    #[test]
    fn normalize_project_code_rejects_unregistered_path_instead_of_falling_back_to_global() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("orphan.rs");
        std::fs::write(&path, "fn orphan() {}\n").unwrap();

        let normalized =
            WorkerPool::normalize_project_code(&graph, None, path.to_string_lossy().as_ref());

        assert_eq!(normalized, None);
    }
}
