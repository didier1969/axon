use std::sync::Arc;
use std::thread;
use tracing::{info, error};
use crossbeam_channel::{Sender, Receiver};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::QueueStore;

// Payload for the Writer Actor
pub enum DbWriteTask {
    FileExtraction {
        path: String,
        content: String,
        extraction: crate::parser::ExtractionResult,
        trace_id: String,
        t0: i64, t1: i64, t2: i64, t3: i64,
    },
    FileSkipped {
        path: String,
        reason: String,
        trace_id: String,
        t0: i64, t1: i64, t2: i64,
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
        let projects_root =
            std::env::var("AXON_PROJECTS_ROOT").unwrap_or_else(|_| "/home/dstadel/projects".to_string());
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
                Self::process_one_task(id, task, &db_sender, &result_sender);
            } else {
                // If really empty, wait a bit longer to save CPU
                thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    pub fn process_one_task(
        _worker_id: usize,
        task: crate::queue::Task,
        db_sender: &Sender<DbWriteTask>,
        _result_sender: &tokio::sync::broadcast::Sender<String>,
    ) {
        let _span = tracing::info_span!("worker_task", path = %task.path).entered();
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;

        match std::fs::read_to_string(&task.path) {
            Ok(content) => {
                // Size safety
                if content.len() > 1024 * 1024 {
                    let _ = db_sender.send(DbWriteTask::FileSkipped {
                        path: task.path.clone(),
                        reason: "File size > 1MB".to_string(),
                        trace_id: task.trace_id.clone(),
                        t0: task.t0, t1: task.t1, t2
                    });
                    return;
                }

                if let Some(parser) = parser::get_parser_for_file(std::path::Path::new(&task.path)) {
                    let mut extraction = parser.parse(&content);
                    if extraction.project_slug.is_none() {
                        extraction.project_slug = Self::infer_project_slug(&task.path);
                    }
                    let t3 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
                    
                    let _ = db_sender.send(DbWriteTask::FileExtraction {
                        path: task.path.clone(),
                        content,
                        extraction,
                        trace_id: task.trace_id.clone(),
                        t0: task.t0, t1: task.t1, t2, t3
                    });
                } else {
                    // Fallback for non-supported but discovered files
                    let _ = db_sender.send(DbWriteTask::FileSkipped {
                        path: task.path.clone(),
                        reason: "No parser found".to_string(),
                        trace_id: task.trace_id.clone(),
                        t0: task.t0, t1: task.t1, t2
                    });
                }
            },
            Err(e) => {
                let _ = db_sender.send(DbWriteTask::FileSkipped {
                    path: task.path.clone(),
                    reason: format!("Read Error: {:?}", e),
                    trace_id: task.trace_id.clone(),
                    t0: task.t0, t1: task.t1, t2
                });
            }
        }
    }

    pub fn spawn_writer_actor(
        graph_store: Arc<GraphStore>, 
        db_receiver: Receiver<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>
    ) {
        thread::spawn(move || {
            info!("DB Writer Actor online. Transactional pipeline ready.");
            let mut batch = Vec::new();
            
            loop {
                // 1. BLOCKING WAIT for first message
                match db_receiver.recv() {
                    Ok(DbWriteTask::FileExtraction { path, content, extraction, trace_id, t0, t1, t2, t3 }) => {
                        batch.push(DbWriteTask::FileExtraction { path, content, extraction, trace_id, t0, t1, t2, t3 });
                    },
                    Ok(DbWriteTask::FileSkipped { path, reason, trace_id, t0, t1, t2 }) => {
                        batch.push(DbWriteTask::FileSkipped { path, reason, trace_id, t0, t1, t2 });
                    },
                    Ok(DbWriteTask::ExecuteCypher { query }) => {
                        let _ = graph_store.execute(&query);
                    },
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
                    let mut combined_feedback = String::new();
                    for task in &batch {
                        if let DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3, .. } = task {
                            let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
                            let msg = serde_json::json!({
                                "FileIndexed": {
                                    "path": path, "status": "ok", "symbol_count": extraction.symbols.len(),
                                    "relation_count": extraction.relations.len(), "trace_id": trace_id,
                                    "t0": t0, "t1": t1, "t2": t2, "t3": t3, "t4": t4
                                }
                            });
                            combined_feedback.push_str(&serde_json::to_string(&msg).unwrap());
                            combined_feedback.push('\n');
                        } else if let DbWriteTask::FileSkipped { path, reason, trace_id, t0, t1, t2 } = task {
                            let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
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

                    if let Err(e) = graph_store.insert_file_data_batch(&batch) {
                        error!("Writer Actor: Batch commit failed: {:?}", e);
                    }

                    if !combined_feedback.is_empty() {
                        let _ = result_sender.send(combined_feedback);
                    }
                    batch.clear();
                }
            }
        });
    }
}
