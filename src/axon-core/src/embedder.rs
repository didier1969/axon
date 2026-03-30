use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use tracing::{info, error, debug};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::graph::GraphStore;
use crate::queue::QueueStore;
use crate::service_guard;

const SYMBOL_MODEL_ID: &str = "sym-bge-small-en-v1.5-384";
const CHUNK_MODEL_ID: &str = "chunk-bge-small-en-v1.5-384";
const MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";
const MODEL_VERSION: &str = "1";
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;

// NEXUS v10.5: Sovereign Semantic Engine
// We isolate the ONNX runtime inside a pure OS thread to prevent Tokio/jemalloc aborts.
// No Lazy statics, no global Mutex. The model is owned by the background worker.

pub struct SemanticWorkerPool {
    _worker: thread::JoinHandle<()>,
}

impl SemanticWorkerPool {
    pub fn new(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) -> Self {
        info!("Semantic Factory: Spawning Native OS ML Worker...");
        let worker = thread::spawn(move || {
            Self::worker_loop(graph_store, queue_store);
        });
        Self { _worker: worker }
    }

    fn worker_loop(graph_store: Arc<GraphStore>, queue_store: Arc<QueueStore>) {
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

        if let Err(e) = graph_store.ensure_embedding_model(SYMBOL_MODEL_ID, "symbol", MODEL_NAME, 384, MODEL_VERSION) {
            error!("Semantic Worker: failed to register symbol embedding model: {:?}", e);
        }
        if let Err(e) = graph_store.ensure_embedding_model(CHUNK_MODEL_ID, "chunk", MODEL_NAME, 384, MODEL_VERSION) {
            error!("Semantic Worker: failed to register chunk embedding model: {:?}", e);
        }

        info!("Semantic Worker: Hunting for unembedded symbols...");
        
        loop {
            let policy = semantic_policy(queue_store.len(), service_guard::recent_peak_latency_ms());
            if policy.pause {
                thread::sleep(policy.sleep);
                continue;
            }

            match graph_store.fetch_unembedded_chunks(CHUNK_MODEL_ID, CHUNK_BATCH_SIZE) {
                Ok(chunks) if !chunks.is_empty() => {
                    debug!("Semantic Worker: Embedding {} chunks...", chunks.len());
                    let texts: Vec<String> = chunks.iter().map(|(_, content, _)| content.clone()).collect();
                    match model.embed(texts, None) {
                        Ok(embeddings) => {
                            let updates: Vec<(String, String, Vec<f32>)> = chunks.into_iter()
                                .zip(embeddings.into_iter())
                                .map(|((id, _, hash), emb)| (id, hash, emb))
                                .collect();

                            if let Err(e) = graph_store.update_chunk_embeddings(CHUNK_MODEL_ID, &updates) {
                                error!("Semantic Worker: Chunk DB Write Error: {:?}", e);
                            }
                        }
                        Err(e) => {
                            if is_fatal_embedding_error(&e) {
                                error!("Semantic Worker: fatal chunk embedding error, disabling semantic worker: {:?}", e);
                                return;
                            }
                            error!("Semantic Worker: Chunk embedding failed: {:?}", e);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => error!("Semantic Worker: Chunk DB Fetch error: {:?}", e),
            }

            match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
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
                        Err(e) => {
                            if is_fatal_embedding_error(&e) {
                                error!("Semantic Worker: fatal symbol embedding error, disabling semantic worker: {:?}", e);
                                return;
                            }
                            error!("Semantic Worker: Embedding failed: {:?}", e);
                        },
                    }
                },
                Ok(_) => thread::sleep(policy.idle_sleep),
                Err(e) => {
                    error!("Semantic Worker: DB Fetch error: {:?}", e);
                    thread::sleep(policy.idle_sleep);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SemanticPolicy {
    pause: bool,
    sleep: Duration,
    idle_sleep: Duration,
}

fn semantic_policy(queue_len: usize, recent_service_latency_ms: u64) -> SemanticPolicy {
    if recent_service_latency_ms >= 1_500 || queue_len >= 3_000 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(10),
            idle_sleep: Duration::from_secs(10),
        };
    }

    if recent_service_latency_ms >= 500 || queue_len >= 1_500 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(3),
            idle_sleep: Duration::from_secs(5),
        };
    }

    SemanticPolicy {
        pause: false,
        sleep: Duration::from_secs(1),
        idle_sleep: Duration::from_secs(5),
    }
}

fn is_fatal_embedding_error<E: std::fmt::Debug>(err: &E) -> bool {
    let rendered = format!("{:?}", err);
    rendered.contains("GetElementType is not implemented")
        || rendered.contains("ORT")
        || rendered.contains("onnxruntime")
}

// STUB for MCP compatibility without crashing
pub fn batch_embed(_texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
    // In v10.5, we disable synchronous MCP embedding to protect the runtime.
    // MCP semantic search will be temporarily bypassed until we build a safe bridge.
    Err(anyhow::anyhow!("MCP Real-time embedding is disabled in safe mode. Use structural search."))
}

#[cfg(test)]
mod tests {
    use super::{is_fatal_embedding_error, semantic_policy};
    use std::time::Duration;

    #[test]
    fn test_fatal_embedding_error_detection() {
        assert!(is_fatal_embedding_error(&"GetElementType is not implemented"));
        assert!(is_fatal_embedding_error(&"onnxruntime failure"));
        assert!(!is_fatal_embedding_error(&"temporary timeout"));
    }

    #[test]
    fn test_semantic_policy_runs_when_system_is_healthy() {
        let policy = semantic_policy(100, 0);
        assert!(!policy.pause);
        assert_eq!(policy.idle_sleep, Duration::from_secs(5));
    }

    #[test]
    fn test_semantic_policy_pauses_under_queue_pressure() {
        let policy = semantic_policy(2_000, 0);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let policy = semantic_policy(100, 2_000);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(10));
    }
}
