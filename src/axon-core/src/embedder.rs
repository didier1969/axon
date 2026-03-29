use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use tracing::{info, error, debug};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::graph::GraphStore;

// NEXUS v10.5: Sovereign Semantic Engine
// We isolate the ONNX runtime inside a pure OS thread to prevent Tokio/jemalloc aborts.
// No Lazy statics, no global Mutex. The model is owned by the background worker.

pub struct SemanticWorkerPool {
    _worker: thread::JoinHandle<()>,
}

impl SemanticWorkerPool {
    pub fn new(graph_store: Arc<GraphStore>) -> Self {
        info!("Semantic Factory: Spawning Native OS ML Worker...");
        let worker = thread::spawn(move || {
            Self::worker_loop(graph_store);
        });
        Self { _worker: worker }
    }

    fn worker_loop(graph_store: Arc<GraphStore>) {
        info!("Semantic Worker: Initializing BGE-Small Model (384d) in isolated thread...");

        let mut options = InitOptions::new(EmbeddingModel::BGESmallENV15);
        options.show_download_progress = true;

        let mut model = match TextEmbedding::try_new(options) {
            Ok(m) => {
                info!("✅ Semantic Worker: BGE-Small Model loaded successfully.");
                m
            },
            Err(e) => {
                error!("❌ Semantic Worker: FATAL ONNX INIT ERROR: {:?}", e);
                return;
            }
        };

        info!("Semantic Worker: Hunting for unembedded symbols...");
        
        loop {
            match graph_store.fetch_unembedded_symbols(100) {
                Ok(symbols) if !symbols.is_empty() => {
                    debug!("Semantic Worker: Embedding {} symbols...", symbols.len());
                    
                    let texts: Vec<String> = symbols.iter().map(|s| s.1.clone()).collect();
                    match model.embed(texts, None) {
                        Ok(embeddings) => {
                            let updates: Vec<(String, Vec<f32>)> = symbols.into_iter()
                                .zip(embeddings.into_iter())
                                .map(|((id, _), emb)| (id, emb))
                                .collect();
                            
                            if let Err(e) = graph_store.update_symbol_embeddings(&updates) {
                                error!("Semantic Worker: DB Write Error: {:?}", e);
                            }
                        },
                        Err(e) => error!("Semantic Worker: Embedding failed: {:?}", e),
                    }
                },
                Ok(_) => thread::sleep(Duration::from_secs(5)),
                Err(e) => {
                    error!("Semantic Worker: DB Fetch error: {:?}", e);
                    thread::sleep(Duration::from_secs(5));
                }
            }
        }
    }
}

// STUB for MCP compatibility without crashing
pub fn batch_embed(_texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    // In v10.5, we disable synchronous MCP embedding to protect the runtime.
    // MCP semantic search will be temporarily bypassed until we build a safe bridge.
    Err(anyhow::anyhow!("MCP Real-time embedding is disabled in safe mode. Use structural search."))
}
