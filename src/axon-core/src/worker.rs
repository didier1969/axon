// Copyright (c) Didier Stadelmann. All rights reserved.

use crossbeam_channel::{Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use tracing::{error, info};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::{estimate_observed_cost_bytes, ProcessingMode, QueueStore};

// Payload for the Writer Actor
pub enum DbWriteTask {
    FileExtraction {
        path: String,
        content: Option<String>,
        extraction: crate::parser::ExtractionResult,
        processing_mode: ProcessingMode,
        trace_id: String,
        t0: i64,
        t1: i64,
        t2: i64,
        t3: i64,
    },
    FileSkipped {
        path: String,
        reason: String,
        trace_id: String,
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

impl WorkerPool {
    fn infer_project_slug(path: &str) -> Option<String> {
        let projects_root = std::env::var("AXON_PROJECTS_ROOT")
            .unwrap_or_else(|_| "/home/dstadel/projects".to_string());
        let path = std::path::Path::new(path);
        let relative = path.strip_prefix(&projects_root).ok()?;
        let first = relative.components().next()?;
        let slug = first.as_os_str().to_string_lossy().trim().to_string();

        if slug.is_empty() || slug == "." {
            None
        } else {
            Some(slug)
        }
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
                let observed_cost_bytes =
                    Self::process_one_task(id, &task, &db_sender, &result_sender);
                let _ = queue.mark_done(&task, observed_cost_bytes);
            } else {
                // If really empty, wait a bit longer to save CPU
                thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    pub fn process_one_task(
        _worker_id: usize,
        task: &crate::queue::Task,
        db_sender: &Sender<DbWriteTask>,
        _result_sender: &tokio::sync::broadcast::Sender<String>,
    ) -> Option<u64> {
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
                    if extraction.project_slug.is_none() {
                        extraction.project_slug = Self::infer_project_slug(&task.path);
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

                    let _ = db_sender.send(DbWriteTask::FileExtraction {
                        path: task.path.clone(),
                        content: content_for_writer,
                        extraction,
                        processing_mode: task.mode,
                        trace_id: task.trace_id.clone(),
                        t0: task.t0,
                        t1: task.t1,
                        t2,
                        t3,
                    });
                    return Some(observed_cost_bytes);
                } else {
                    // Fallback for non-supported but discovered files
                    let _ = db_sender.send(DbWriteTask::FileSkipped {
                        path: task.path.clone(),
                        reason: "No parser found".to_string(),
                        trace_id: task.trace_id.clone(),
                        t0: task.t0,
                        t1: task.t1,
                        t2,
                    });
                    return Some(task.estimated_cost_bytes);
                }
            }
            Err(e) => {
                let _ = db_sender.send(DbWriteTask::FileSkipped {
                    path: task.path.clone(),
                    reason: format!("Read Error: {:?}", e),
                    trace_id: task.trace_id.clone(),
                    t0: task.t0,
                    t1: task.t1,
                    t2,
                });
                return None;
            }
        }
    }

    pub fn spawn_writer_actor(
        graph_store: Arc<GraphStore>,
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
                        path,
                        content,
                        extraction,
                        processing_mode,
                        trace_id,
                        t0,
                        t1,
                        t2,
                        t3,
                    }) => {
                        batch.push(DbWriteTask::FileExtraction {
                            path,
                            content,
                            extraction,
                            processing_mode,
                            trace_id,
                            t0,
                            t1,
                            t2,
                            t3,
                        });
                    }
                    Ok(DbWriteTask::FileSkipped {
                        path,
                        reason,
                        trace_id,
                        t0,
                        t1,
                        t2,
                    }) => {
                        batch.push(DbWriteTask::FileSkipped {
                            path,
                            reason,
                            trace_id,
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
                    if let Err(e) = graph_store.insert_file_data_batch(&batch) {
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
                    } else {
                        let combined_feedback = build_feedback_messages(&batch);
                        if !combined_feedback.is_empty() {
                            let _ = result_sender.send(combined_feedback);
                        }
                    }
                    batch.clear();
                }
            }
        });
    }
}

fn build_feedback_messages(batch: &[DbWriteTask]) -> String {
    let mut combined_feedback = String::new();
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
                    "t0": t0, "t1": t1, "t2": t2, "t3": t3, "t4": t4
                }
            });
            combined_feedback.push_str(&serde_json::to_string(&msg).unwrap());
            combined_feedback.push('\n');
        } else if let DbWriteTask::FileSkipped {
            path,
            reason,
            trace_id,
            t0,
            t1,
            t2,
        } = task
        {
            let t4 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;
            let msg = serde_json::json!({
                "FileIndexed": {
                    "path": path, "status": "skipped", "error_reason": reason,
                    "trace_id": trace_id, "t0": t0, "t1": t1, "t2": t2, "t3": t2, "t4": t4
                }
            });
            combined_feedback.push_str(&serde_json::to_string(&msg).unwrap());
            combined_feedback.push('\n');
        }
    }
    combined_feedback
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
    use super::{DbWriteTask, WorkerPool};
    use crate::queue::TaskLane;
    use crate::queue::{ProcessingMode, Task};

    #[test]
    fn test_large_file_is_not_skipped_when_budget_admitted_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.txt");
        let mut content = String::from("header\n");
        content.push_str(&"x".repeat(1_200_000));
        std::fs::write(&path, content).unwrap();

        let task = Task {
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

        let (db_sender, db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);

        let observed = WorkerPool::process_one_task(0, &task, &db_sender, &results_tx);
        assert!(observed.is_some());

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

        let (db_sender, db_receiver) = crossbeam_channel::unbounded();
        let (results_tx, _) = tokio::sync::broadcast::channel::<String>(16);

        let observed = WorkerPool::process_one_task(0, &task, &db_sender, &results_tx);
        assert!(observed.is_some());

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
                path: "/tmp/a.ex".to_string(),
                reason: "No parser found".to_string(),
                trace_id: "trace-a".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
            },
            DbWriteTask::FileExtraction {
                path: "/tmp/a.ex".to_string(),
                content: Some("defmodule A do end".to_string()),
                extraction: crate::parser::ExtractionResult::default(),
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-a".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            },
            DbWriteTask::FileSkipped {
                path: "/tmp/b.ex".to_string(),
                reason: "No parser found".to_string(),
                trace_id: "trace-b".to_string(),
                t0: 0,
                t1: 0,
                t2: 0,
            },
        ];

        let claimed = super::claimed_paths_from_batch(&batch);
        assert_eq!(claimed, vec!["/tmp/a.ex".to_string(), "/tmp/b.ex".to_string()]);
    }
}
