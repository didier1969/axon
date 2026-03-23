use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use log::{info, error};

use crate::graph::GraphStore;
use crate::parser;

// The payload sent to the workers
pub struct WorkerTask {
    pub path: String,
    pub is_titan: bool,
}

pub struct WorkerPool {
    sender: mpsc::Sender<WorkerTask>,
}

impl WorkerPool {
    pub fn new(
        num_fast_workers: usize, 
        graph_store: Arc<RwLock<GraphStore>>,
        result_sender: tokio::sync::mpsc::Sender<String>
    ) -> Self {
        let (task_sender, task_receiver) = mpsc::channel::<WorkerTask>();
        let task_receiver = Arc::new(std::sync::Mutex::new(task_receiver));

        for id in 0..num_fast_workers {
            Self::spawn_immortal(id, task_receiver.clone(), graph_store.clone(), result_sender.clone());
        }

        Self { sender: task_sender }
    }

    pub fn get_sender(&self) -> mpsc::Sender<WorkerTask> {
        self.sender.clone()
    }

    // The Supervisor logic: Spawns a worker that will die and be resurrected
    fn spawn_immortal(
        id: usize,
        receiver: Arc<std::sync::Mutex<mpsc::Receiver<WorkerTask>>>,
        graph_store: Arc<RwLock<GraphStore>>,
        result_sender: tokio::sync::mpsc::Sender<String>,
    ) {
        thread::Builder::new().name(format!("axon-worker-{}", id)).spawn(move || {
            info!("Worker {} born. Initializing isolated AI/WASM engines...", id);
            
            // 1. Thread-local isolated allocation
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
                let task = {
                    let rx = receiver.lock().unwrap();
                    match rx.recv() {
                        Ok(t) => t,
                        Err(_) => break, // Channel closed
                    }
                };

                crate::parser::set_titan_mode(task.is_titan);
                let path_obj = std::path::Path::new(&task.path);
                let mut symbols_count = 0;
                let mut relations_count = 0;
                let mut success = false;

                // FORENSIC SHIELD: Death Rattle Logging
                use std::io::Write;
                if let Ok(mut log_file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/axon_forensic.log") {
                    let lane_str = if task.is_titan { "titan" } else { "fast" };
                    let _ = writeln!(log_file, "START [{}]: {}", lane_str, task.path);
                    let _ = log_file.sync_all(); 
                }

                // Defense in Depth
                let mut skip = false;
                if let Ok(metadata) = std::fs::metadata(path_obj) {
                    if !task.is_titan && metadata.len() > 1_048_576 {
                        skip = true;
                    }
                    if task.is_titan && metadata.len() > 52_428_800 {
                        skip = true;
                    }
                }

                if !skip {
                    if let Ok(content) = std::fs::read_to_string(path_obj) {
                        if let Some(parser) = parser::get_parser_for_file(path_obj) {
                            println!("Worker {} parsing file: {}", id, task.path);
                            let mut extraction = parser.parse(&content);
                            println!("Worker {} extracted {} symbols", id, extraction.symbols.len());

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

                            info!("Worker {} acquiring graph_store write lock...", id);
                            if let Ok(store) = graph_store.write() {
                                info!("Worker {} inserting file data into KuzuDB...", id);
                                if let Err(e) = store.insert_file_data(&task.path, &extraction) {
                                    error!("Graph insertion failed for {}: {:?}", task.path, e);
                                } else {
                                    info!("Worker {} successfully inserted file data.", id);
                                }
                            }
                        } else {
                            info!("Worker {} found NO parser for file: {}", id, task.path);
                        }
                    } else {
                        info!("Worker {} failed to read file: {}", id, task.path);
                    }                }

                if let Ok(mut log_file) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/axon_forensic.log") {
                    let _ = writeln!(log_file, "END:   {}", task.path);
                }

                let finish_msg = serde_json::to_string(&crate::bridge::BridgeEvent::FileIndexed {
                    path: task.path,
                    symbol_count: symbols_count,
                    relation_count: relations_count,
                    file_count: 1,
                    entry_points: 0,
                    security_score: 100,
                    coverage_score: 0,
                    taint_paths: "".to_string(),
                }).unwrap() + "\n";

                let _ = result_sender.blocking_send(finish_msg);

                processed += 1;
                if processed >= max_files_before_death {
                    info!("Worker {} reached end of life ({} files). Committing suicide for memory purity.", id, processed);
                    Self::spawn_immortal(id, receiver.clone(), graph_store.clone(), result_sender.clone());
                    break;
                }
            }
        }).expect("Failed to spawn worker thread");
    }
}
