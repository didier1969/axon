use std::sync::Arc;
use parking_lot::RwLock;
use std::thread;
use tracing::{info, error, instrument};
use crossbeam_channel::{bounded, Sender, Receiver};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::QueueStore;
use std::sync::atomic::{AtomicUsize, Ordering};

// Payload for the Writer Actor
pub struct DbWriteTask {
    pub path: String,
    pub extraction: crate::parser::ExtractionResult,
    pub trace_id: String,
    pub t0: i64,
    pub t1: i64,
    pub t2: i64,
    pub t3: i64,
}

pub struct WorkerPool {}

impl WorkerPool {
    pub fn new(
        num_fast_workers: usize, 
        graph_store: Arc<RwLock<GraphStore>>,
        queue: Arc<QueueStore>,
        result_sender: tokio::sync::broadcast::Sender<String>,
        mcp_active: Arc<AtomicUsize>
    ) -> Self {
        // BOUNDED QUEUES for strict 16GB RAM mechanical backpressure
        // We only keep the DbWriteTask bounded queue. The big list is in SQLite.
        let (db_sender, db_receiver) = bounded::<DbWriteTask>(1000); 

        // 1. Spawn the SINGLE Writer Actor (Micro-batching CQRS style)
        Self::spawn_writer_actor(db_receiver, graph_store.clone(), mcp_active.clone(), queue.clone(), result_sender.clone());

        // 2. Spawn the parsing Immortals (CPU Bound)
        for id in 0..num_fast_workers {
            Self::spawn_immortal(
                id, 
                queue.clone(),
                db_sender.clone(),
                result_sender.clone(), 
                mcp_active.clone()
            );
        }

        Self {}
    }

    // THE WRITER ACTOR: Single threaded to avoid any DB contention
    fn spawn_writer_actor(
        receiver: Receiver<DbWriteTask>,
        graph_store: Arc<RwLock<GraphStore>>,
        mcp_active: Arc<AtomicUsize>,
        queue: Arc<QueueStore>,
        result_sender: tokio::sync::broadcast::Sender<String>
    ) {
        thread::Builder::new().name("axon-db-writer".to_string()).spawn(move || {
            info!("Writer Actor born. Holding exclusive keys to KuzuDB.");
            
            const BATCH_SIZE: usize = 50;
            let mut batch = Vec::with_capacity(BATCH_SIZE);

            loop {
                // Yield immediately if MCP is querying to ensure 0 latency
                while mcp_active.load(Ordering::Acquire) > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }

                // 1. BLOCKING WAIT for the first task (No CPU spin)
                let first_task = match receiver.recv() {
                    Ok(t) => t,
                    Err(_) => break, // Channel disconnected
                };
                
                batch.push(first_task);

                // 2. SMART DRAIN: Opportunistically pull up to BATCH_SIZE
                while batch.len() < BATCH_SIZE {
                    match receiver.try_recv() {
                        Ok(t) => batch.push(t),
                        Err(crossbeam_channel::TryRecvError::Empty) => break, // Channel empty, flush now!
                        Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                    }
                }

                // 3. EXECUTION PHASE (With Yield Protection)
                if mcp_active.load(Ordering::Acquire) > 0 {
                    // LLM active, reject entire batch with "busy"
                    for task in batch.drain(..) {
                        Self::send_feedback(&result_sender, &task, "busy", "System Contention (Agent Reading)");
                    }
                    continue;
                }

                // Try to acquire write lock
                match graph_store.try_write() {
                    Some(store) => {
                        // FAST PATH: Process the entire batch while holding the lock ONCE
                        for task in batch.drain(..) {
                            let _span = tracing::info_span!("db_writer_task", path = %task.path).entered();
                            if let Err(e) = store.insert_file_data(&task.path, &task.extraction) {
                                error!("Writer Actor failed to insert {}: {:?}", task.path, e);
                                Self::send_feedback(&result_sender, &task, "error", &format!("{:?}", e));
                            } else {
                                let _ = queue.mark_done(&task.path);
                                Self::send_feedback(&result_sender, &task, "ok", "");
                            }
                        }
                    },
                    None => {
                        // LOCK STARVATION: Reject entire batch
                        tracing::warn!("Writer Actor KuzuDB lock busy, yielding {} tasks", batch.len());
                        for task in batch.drain(..) {
                            Self::send_feedback(&result_sender, &task, "busy", "KuzuDB Write Lock Contention");
                        }
                    }
                }
            }
        }).expect("Failed to spawn Writer Actor");
    }

    // Helper fonction for telemetry
    fn send_feedback(
        result_sender: &tokio::sync::broadcast::Sender<String>, 
        task: &DbWriteTask, 
        status: &str, 
        error_reason: &str
    ) {
        let t4 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;
        let msg = match serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
            path: task.path.clone(),
            status: status.to_string(),
            error_reason: error_reason.to_string(),
            symbol_count: task.extraction.symbols.len(),
            relation_count: task.extraction.relations.len(),
            file_count: 1,
            entry_points: 0,
            security_score: 100,
            coverage_score: 0,
            taint_paths: "".to_string(),
            trace_id: task.trace_id.clone(),
            t0: task.t0, t1: task.t1, t2: task.t2, t3: task.t3, t4,
        }) {
            Ok(m) => m + "\n",
            Err(_) => return,
        };
        let _ = result_sender.send(msg);
    }

    // THE WORKER: Pure CPU, no DB locks
    fn spawn_immortal(
        id: usize,
        queue: Arc<QueueStore>,
        db_sender: Sender<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
        mcp_active: Arc<AtomicUsize>
    ) {
        thread::Builder::new().name(format!("axon-worker-{}", id)).spawn(move || {
            info!("Worker {} born. Initializing isolated AI/WASM engines...", id);
            
            let mut fastembed_model = match fastembed::TextEmbedding::try_new(
                fastembed::InitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2)
                    .with_show_download_progress(false)
            ) {
                Ok(m) => m,
                Err(e) => {
                    error!("Worker {} failed to load FastEmbed: {}", id, e);
                    return;
                }
            };

            let max_files_before_death = 500;
            let mut processed = 0;

            loop {
                // Yield CPU to MCP process
                while mcp_active.load(Ordering::Relaxed) > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                let task = match queue.pop() {
                    Some(t) => t,
                    None => {
                        std::thread::sleep(std::time::Duration::from_millis(100)); // Polling delay
                        continue;
                    }
                };

                let path_obj = std::path::Path::new(&task.path);
                let mut symbols_count = 0;
                let mut relations_count = 0;

                let mut skip = false;
                let mut auto_titan = false;
                if let Ok(metadata) = std::fs::metadata(path_obj) {
                    let size = metadata.len();
                    if size > 1_048_576 { 
                        // Hard Stop: Disable heavy semantic extraction for files > 1MB
                        skip = true; 
                        tracing::warn!("Skipping heavy parse/embed for file > 1MB: {}", task.path);
                    } 
                }

                if !skip {
                    if let Ok(content) = std::fs::read_to_string(path_obj) {
                        if let Some(parser) = parser::get_parser_for_file(path_obj) {
                            crate::parser::set_titan_mode(auto_titan);
                            let mut extraction = parser.parse(&content);
                            parser::scan_secrets(&content, &mut extraction);

                            // Extract project slug dynamically from path structure (assuming /home/user/projects/<slug>/...)
                            let mut slug = "global".to_string();
                            if let Some(path_str) = path_obj.to_str() {
                                let parts: Vec<&str> = path_str.split('/').collect();
                                if let Some(idx) = parts.iter().position(|&p| p == "projects") {
                                    if idx + 1 < parts.len() {
                                        slug = parts[idx + 1].to_string();
                                    }
                                }
                            }
                            extraction.project_slug = Some(slug);

                            let texts_to_embed: Vec<String> = extraction.symbols.iter()
                                .map(|s| format!("Symbol: {} Kind: {}", s.name, s.kind))
                                .collect();

                            if !texts_to_embed.is_empty() {
                                let mut all_embeddings = Vec::with_capacity(texts_to_embed.len());
                                let embed_start_time = std::time::Instant::now();
                                
                                for chunk in texts_to_embed.chunks(64) {
                                    if embed_start_time.elapsed().as_secs() >= 5 {
                                        tracing::warn!("Worker {} aborting heavy embedding loop (> 5s) for {}", id, path_obj.display());
                                        break;
                                    }

                                    let mut trunc = Vec::new();
                                    for s in chunk {
                                        if s.len() > 1000 { trunc.push(s[..1000].to_string()); } 
                                        else { trunc.push(s.to_string()); }
                                    }
                                    let refs: Vec<&str> = trunc.iter().map(|s| s.as_str()).collect();
                                    if let Ok(embs) = fastembed_model.embed(refs, None) {
                                        all_embeddings.extend(embs);
                                    }
                                }

                                for (sym, emb) in extraction.symbols.iter_mut().zip(all_embeddings.into_iter()) {
                                    sym.embedding = Some(emb);
                                }
                            }

                            symbols_count = extraction.symbols.len();
                            relations_count = extraction.relations.len();

                            let t3 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros() as i64;

                            // SEND TO ACTOR (No DB Locks here!)
                            if let Err(e) = db_sender.send(DbWriteTask { 
                                path: task.path.clone(), 
                                extraction,
                                trace_id: task.trace_id,
                                t0: task.t0,
                                t1: task.t1,
                                t2: task.t2,
                                t3,
                            }) {
                                error!("Worker {} failed to queue DB write: {}", id, e);
                            }
                        } else {
                           let _ = queue.mark_done(&task.path); // Mark done if no parser
                        }
                    } else {
                       let _ = queue.mark_done(&task.path); // Mark done if unreadable
                    }
                } else {
                    let _ = queue.mark_done(&task.path); // Mark done if skipped
                }

                processed += 1;
                if processed >= max_files_before_death {
                    info!("Worker {} reached end of life. Recycling to free ONNX memory.", id);
                    Self::spawn_immortal(id, queue.clone(), db_sender.clone(), result_sender.clone(), mcp_active.clone());
                    break;
                }
            }
        }).expect("Failed to spawn worker thread");
    }
}