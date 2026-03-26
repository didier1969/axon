use std::sync::Arc;
use parking_lot::RwLock;
use std::thread;
use tracing::{info, error, instrument};
use crossbeam_channel::{bounded, Sender, Receiver};

use crate::graph::GraphStore;
use crate::parser;
use crate::queue::QueueStore;
use std::sync::atomic::{AtomicBool, Ordering};

// Payload for the Writer Actor
pub struct DbWriteTask {
    pub path: String,
    pub extraction: crate::parser::ExtractionResult,
}

pub struct WorkerPool {}

impl WorkerPool {
    pub fn new(
        num_fast_workers: usize, 
        graph_store: Arc<RwLock<GraphStore>>,
        queue: Arc<QueueStore>,
        result_sender: tokio::sync::broadcast::Sender<String>,
        mcp_active: Arc<AtomicBool>
    ) -> Self {
        // BOUNDED QUEUES for strict 16GB RAM mechanical backpressure
        // We only keep the DbWriteTask bounded queue. The big list is in SQLite.
        let (db_sender, db_receiver) = bounded::<DbWriteTask>(1000); 

        // 1. Spawn the SINGLE Writer Actor (Micro-batching CQRS style)
        Self::spawn_writer_actor(db_receiver, graph_store.clone(), mcp_active.clone(), queue.clone());

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
        mcp_active: Arc<AtomicBool>,
        queue: Arc<QueueStore>
    ) {
        thread::Builder::new().name("axon-db-writer".to_string()).spawn(move || {
            info!("Writer Actor born. Holding exclusive keys to KuzuDB.");
            loop {
                // Yield immediately if MCP is querying to ensure 0 latency
                while mcp_active.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }

                match receiver.recv() {
                    Ok(task) => {
                        let _span = tracing::info_span!("db_writer_task", path = %task.path).entered();
                        let mut store = graph_store.write();
                        if let Err(e) = store.insert_file_data(&task.path, &task.extraction) {
                            error!("Writer Actor failed to insert {}: {:?}", task.path, e);
                        } else {
                            let _ = queue.mark_done(&task.path);
                        }
                    },
                    Err(_) => break, // Channel closed
                }
            }
        }).expect("Failed to spawn Writer Actor");
    }

    // THE WORKER: Pure CPU, no DB locks
    fn spawn_immortal(
        id: usize,
        queue: Arc<QueueStore>,
        db_sender: Sender<DbWriteTask>,
        result_sender: tokio::sync::broadcast::Sender<String>,
        mcp_active: Arc<AtomicBool>
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
                while mcp_active.load(Ordering::Relaxed) {
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
                    if size > 104_857_600 { skip = true; } // Skip if > 100MB
                    else if size > 1_048_576 { auto_titan = true; } // Auto-Titan if > 1MB
                }

                if !skip {
                    if let Ok(content) = std::fs::read_to_string(path_obj) {
                        if let Some(parser) = parser::get_parser_for_file(path_obj) {
                            crate::parser::set_titan_mode(auto_titan);
                            let mut extraction = parser.parse(&content);
                            parser::scan_secrets(&content, &mut extraction);

                            let texts_to_embed: Vec<String> = extraction.symbols.iter()
                                .map(|s| format!("Symbol: {} Kind: {}", s.name, s.kind))
                                .collect();

                            if !texts_to_embed.is_empty() {
                                let mut all_embeddings = Vec::with_capacity(texts_to_embed.len());
                                for chunk in texts_to_embed.chunks(64) {
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

                            // SEND TO ACTOR (No DB Locks here!)
                            if let Err(e) = db_sender.send(DbWriteTask { path: task.path.clone(), extraction }) {
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

                let finish_msg = match serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
                    path: task.path,
                    symbol_count: symbols_count,
                    relation_count: relations_count,
                    file_count: 1,
                    entry_points: 0,
                    security_score: 100,
                    coverage_score: 0,
                    taint_paths: "".to_string(),
                }) {
                    Ok(msg) => msg + "\n",
                    Err(e) => {
                        error!("Worker {} failed to serialize telemetry: {:?}", id, e);
                        continue;
                    }
                };

                let _ = result_sender.send(finish_msg);

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
