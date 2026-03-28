use std::sync::Arc;
use std::thread;
use tracing::{info, error};
use crossbeam_channel::{bounded, Sender, Receiver};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::QueueStore;

// Payload for the Writer Actor
pub enum DbWriteTask {
    FileExtraction {
        path: String,
        extraction: crate::parser::ExtractionResult,
        trace_id: String,
        t0: i64, t1: i64, t2: i64, t3: i64,
    },
    ExecuteCypher {
        query: String,
    },
}

pub struct WorkerPool {
    pub db_sender: Sender<DbWriteTask>,
}

impl WorkerPool {
    pub fn new(
        num_fast_workers: usize, 
        graph_store: Arc<GraphStore>,
        queue: Arc<QueueStore>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) -> Self {
        // BOUNDED QUEUES for strict 16GB RAM mechanical backpressure
        let (db_sender, db_receiver) = bounded(1000);

        // 1. Spawn the Singleton Writer Actor
        Self::spawn_writer_actor(graph_store.clone(), queue.clone(), db_receiver, result_sender.clone());

        // 2. Spawn CPU Parallel Workers
        for id in 0..num_fast_workers {
            Self::spawn_immortal(id, queue.clone(), db_sender.clone(), result_sender.clone());
        }

        WorkerPool { db_sender }
    }

    fn spawn_writer_actor(
        graph_store: Arc<GraphStore>, 
        queue: Arc<QueueStore>,
        db_receiver: Receiver<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) {
        thread::Builder::new().name("axon-db-actor".to_string()).spawn(move || {
            info!("DB Writer Actor online. Initializing transactional pipeline...");
            
            let mut batch = Vec::new();
            
            loop {
                // 1. DRAIN CHANNEL
                match db_receiver.recv() {
                    Ok(DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3 }) => {
                        batch.push(crate::worker::DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3 });
                    },
                    Ok(DbWriteTask::ExecuteCypher { query }) => {
                        if let Err(e) = graph_store.execute(&query) {
                            error!("Writer Actor failed to execute SOLL query: {} | {:?}", query, e);
                        }
                    },
                    Err(_) => break,
                }

                // Fill batch up to 50
                while batch.len() < 50 {
                    match db_receiver.try_recv() {
                        Ok(DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3 }) => {
                            batch.push(crate::worker::DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3 });
                        },
                        Ok(DbWriteTask::ExecuteCypher { query }) => {
                            if let Err(e) = graph_store.execute(&query) {
                                error!("Writer Actor failed to execute SOLL query: {} | {:?}", query, e);
                            }
                        },
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                    }
                }

                // 2. EXECUTION PHASE
                let mut feedback_buffer = String::new();
                
                if !batch.is_empty() {
                    info!("Writer Actor: Processing batch of {} extractions...", batch.len());
                }

                // Transactional Batch Insert
                let result = if !batch.is_empty() {
                    graph_store.insert_file_data_batch(&batch)
                } else {
                    Ok(())
                };

                if let Err(e) = &result {
                    error!("Writer Actor: Transactional Commit FAILED: {:?}", e);
                } else if !batch.is_empty() {
                    info!("Writer Actor: Transactional Commit SUCCESS.");
                }

                for task in batch.drain(..) {
                    if let DbWriteTask::FileExtraction { path, extraction, trace_id, t0, t1, t2, t3 } = task {
                        let (status, reason) = match &result {
                            Ok(_) => {
                                let _ = queue.mark_done(&path);
                                ("ok", "".to_string())
                            },
                            Err(e) => ("error", format!("{:?}", e))
                        };

                        if let Some(msg) = Self::format_feedback(&path, status, &reason, extraction.symbols.len(), extraction.relations.len(), &trace_id, t0, t1, t2, t3) {
                            let _ = result_sender.send(msg);
                        }
                    }
                }
            }
        }).expect("Failed to spawn Writer Actor");
    }

    fn format_feedback(
        path: &str,
        status: &str, 
        error_reason: &str,
        symbol_count: usize,
        relation_count: usize,
        trace_id: &str,
        t0: i64, t1: i64, t2: i64, t3: i64
    ) -> Option<String> {
        let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        match serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
            path: path.to_string(),
            status: status.to_string(),
            error_reason: error_reason.to_string(),
            symbol_count,
            relation_count,
            file_count: 1,
            entry_points: 0,
            security_score: 100,
            coverage_score: 0,
            taint_paths: "".to_string(),
            trace_id: trace_id.to_string(),
            t0, t1, t2, t3, t4,
        }) {
            Ok(m) => Some(m + "\n"),
            Err(_) => None,
        }
    }

    fn send_feedback(
        result_sender: &tokio::sync::broadcast::Sender<String>, 
        path: &str,
        status: &str, 
        error_reason: &str,
        symbol_count: usize,
        relation_count: usize,
        trace_id: &str,
        t0: i64, t1: i64, t2: i64, t3: i64
    ) {
        if let Some(msg) = Self::format_feedback(path, status, error_reason, symbol_count, relation_count, trace_id, t0, t1, t2, t3) {
            let _ = result_sender.send(msg);
        }
    }

    fn spawn_immortal(
        id: usize,
        queue: Arc<QueueStore>,
        db_sender: Sender<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
    ) {
        thread::Builder::new().name(format!("axon-worker-{}", id)).spawn(move || {
            info!("Worker {} born. Initializing isolated AI/WASM engines...", id);
            
            loop {
                if let Some(task) = queue.pop() {
                    Self::process_one_task(id, task, &db_sender, &result_sender);
                } else {
                    thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }).expect("Failed to spawn worker thread");
    }

    pub fn process_one_task(
        worker_id: usize,
        task: crate::queue::Task,
        db_sender: &Sender<DbWriteTask>,
        result_sender: &tokio::sync::broadcast::Sender<String>,
    ) {
        info!("Worker {}: Processing file: {}", worker_id, task.path);
        let _span = tracing::info_span!("worker_task", path = %task.path).entered();
        let t2 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;

        match std::fs::read_to_string(&task.path) {
            Ok(content) => {
                if content.len() > 1024 * 1024 {
                    error!("Skipping heavy parse/embed for file > 1MB: {}", task.path);
                    Self::send_feedback(result_sender, &task.path, "skipped", "File size > 1MB", 0, 0, &task.trace_id, task.t0, task.t1, t2, t2);
                    return;
                }

                if let Some(parser) = parser::get_parser_for_file(std::path::Path::new(&task.path)) {
                    let mut extraction = parser.parse(&content);
                    let t3 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
                    
                    // Batch Embed all symbols
                    let sym_names: Vec<String> = extraction.symbols.iter().map(|s| s.name.clone()).collect();
                    if let Ok(embeddings) = crate::embedder::batch_embed(sym_names) {
                        for (i, vec) in embeddings.into_iter().enumerate() {
                            if i < extraction.symbols.len() {
                                extraction.symbols[i].embedding = Some(vec);
                            }
                        }
                    }

                    let write_task = DbWriteTask::FileExtraction {
                        path: task.path.clone(),
                        extraction,
                        trace_id: task.trace_id.clone(),
                        t0: task.t0, t1: task.t1, t2, t3
                    };
                    
                    if let Err(e) = db_sender.send(write_task) { 
                        error!("Worker {}: Failed to send write task: {:?}", worker_id, e);
                    }
                } else {
                    Self::send_feedback(result_sender, &task.path, "skipped", "No parser found", 0, 0, &task.trace_id, task.t0, task.t1, t2, t2);
                }
            },
            Err(e) => {
                error!("Failed to read file {}: {:?}", task.path, e);
                Self::send_feedback(result_sender, &task.path, "error", &format!("{:?}", e), 0, 0, &task.trace_id, task.t0, task.t1, t2, t2);
            }
        }
    }
}
