use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use tracing::{info, error, debug};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::graph::GraphStore;
use crate::queue::QueueStore;
use crate::service_guard::{self, ServicePressure};

const SYMBOL_MODEL_ID: &str = "sym-bge-small-en-v1.5-384";
const CHUNK_MODEL_ID: &str = "chunk-bge-small-en-v1.5-384";
const GRAPH_MODEL_ID: &str = "graph-bge-small-en-v1.5-384";
const MODEL_NAME: &str = "BAAI/bge-small-en-v1.5";
const MODEL_VERSION: &str = "1";
const CHUNK_BATCH_SIZE: usize = 16;
const SYMBOL_BATCH_SIZE: usize = 32;
const GRAPH_BATCH_SIZE: usize = 6;

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
        if let Err(e) = graph_store.ensure_embedding_model(GRAPH_MODEL_ID, "graph", MODEL_NAME, 384, MODEL_VERSION) {
            error!("Semantic Worker: failed to register graph embedding model: {:?}", e);
        }

        info!("Semantic Worker: Hunting for unembedded symbols...");
        
        loop {
            let policy = semantic_policy(queue_store.common_len(), service_guard::current_pressure());
            if policy.pause {
                thread::sleep(policy.sleep);
                continue;
            }

            let mut chunk_backlog_active = false;
            match graph_store.fetch_unembedded_chunks(CHUNK_MODEL_ID, CHUNK_BATCH_SIZE) {
                Ok(chunks) if !chunks.is_empty() => {
                    chunk_backlog_active = true;
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

            let mut symbol_backlog_active = false;
            match graph_store.fetch_unembedded_symbols(SYMBOL_BATCH_SIZE) {
                Ok(symbols) if !symbols.is_empty() => {
                    symbol_backlog_active = true;
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

            if chunk_backlog_active
                || symbol_backlog_active
                || service_guard::current_pressure() != ServicePressure::Healthy
            {
                continue;
            }

            match graph_store.fetch_unembedded_graph_projections(GRAPH_MODEL_ID, GRAPH_BATCH_SIZE) {
                Ok(graphs) if !graphs.is_empty() => {
                    debug!("Semantic Worker: Embedding {} graph projections...", graphs.len());
                    let texts: Vec<String> = graphs.iter().map(|(_, _, _, _, _, content)| content.clone()).collect();
                    match model.embed(texts, None) {
                        Ok(embeddings) => {
                            let updates: Vec<(String, String, i64, String, String, Vec<f32>)> = graphs
                                .into_iter()
                                .zip(embeddings.into_iter())
                                .map(|((anchor_type, anchor_id, radius, source_signature, projection_version, _), emb)| {
                                    (anchor_type, anchor_id, radius, source_signature, projection_version, emb)
                                })
                                .collect();

                            if let Err(e) = graph_store.update_graph_embeddings(GRAPH_MODEL_ID, &updates) {
                                error!("Semantic Worker: Graph DB Write Error: {:?}", e);
                            }
                        }
                        Err(e) => {
                            if is_fatal_embedding_error(&e) {
                                error!("Semantic Worker: fatal graph embedding error, disabling semantic worker: {:?}", e);
                                return;
                            }
                            error!("Semantic Worker: Graph embedding failed: {:?}", e);
                        }
                    }
                }
                Ok(_) => thread::sleep(policy.idle_sleep),
                Err(e) => {
                    error!("Semantic Worker: Graph DB Fetch error: {:?}", e);
                    thread::sleep(policy.idle_sleep);
                }
            }
        }
    }
}

impl GraphStore {
    fn escape_embedding_sql(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn graph_projection_embedding_text(&self, anchor_type: &str, anchor_id: &str, radius: i64) -> anyhow::Result<String> {
        let projection = self.query_graph_projection(anchor_type, anchor_id, radius as u64)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&projection).unwrap_or_default();
        let mut lines = vec![
            format!("anchor_type: {}", anchor_type),
            format!("anchor_id: {}", anchor_id),
            format!("radius: {}", radius),
        ];

        for row in rows {
            let target_type = row.first().and_then(|value| value.as_str()).unwrap_or("unknown");
            let target_id = row.get(1).and_then(|value| value.as_str()).unwrap_or("unknown");
            let edge_kind = row.get(2).and_then(|value| value.as_str()).unwrap_or("unknown");
            let distance = row.get(3).and_then(|value| value.as_i64()).unwrap_or(0);
            let label = row.get(4).and_then(|value| value.as_str()).unwrap_or(target_id);
            lines.push(format!(
                "target_type: {} | target_id: {} | edge_kind: {} | distance: {} | label: {}",
                target_type, target_id, edge_kind, distance, label
            ));
        }

        Ok(lines.join("\n"))
    }

    pub fn fetch_unembedded_graph_projections(
        &self,
        model_id: &str,
        count: usize,
    ) -> anyhow::Result<Vec<(String, String, i64, String, String, String)>> {
        let query = format!(
            "SELECT gps.anchor_type, gps.anchor_id, gps.radius, gps.source_signature, gps.projection_version \
             FROM GraphProjectionState gps \
             LEFT JOIN GraphEmbedding ge \
               ON ge.anchor_type = gps.anchor_type \
              AND ge.anchor_id = gps.anchor_id \
              AND ge.radius = gps.radius \
              AND ge.model_id = '{}' \
             WHERE ge.anchor_id IS NULL \
                OR ge.source_signature <> gps.source_signature \
                OR ge.projection_version <> gps.projection_version \
             ORDER BY CASE WHEN gps.anchor_type = 'symbol' THEN 0 ELSE 1 END ASC, gps.updated_at ASC \
             LIMIT {}",
            Self::escape_embedding_sql(model_id),
            count
        );
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        let res = self.query_on_ctx(&query, *guard)?;
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let mut jobs = Vec::new();
        for row in raw {
            let Some(anchor_type) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(anchor_id) = row.get(1).and_then(|value| value.as_str()) else {
                continue;
            };
            let radius = row.get(2).and_then(|value| value.as_i64()).unwrap_or(1);
            let Some(source_signature) = row.get(3).and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(projection_version) = row.get(4).and_then(|value| value.as_str()) else {
                continue;
            };
            let content = self.graph_projection_embedding_text(anchor_type, anchor_id, radius)?;
            jobs.push((
                anchor_type.to_string(),
                anchor_id.to_string(),
                radius,
                source_signature.to_string(),
                projection_version.to_string(),
                content,
            ));
        }

        Ok(jobs)
    }

    pub fn update_graph_embeddings(
        &self,
        model_id: &str,
        updates: &[(String, String, i64, String, String, Vec<f32>)],
    ) -> anyhow::Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let mut queries = Vec::new();
        for (anchor_type, anchor_id, radius, _, _, _) in updates {
            queries.push(format!(
                "DELETE FROM GraphEmbedding WHERE anchor_type = '{}' AND anchor_id = '{}' AND radius = {} AND model_id = '{}';",
                Self::escape_embedding_sql(anchor_type),
                Self::escape_embedding_sql(anchor_id),
                radius,
                Self::escape_embedding_sql(model_id)
            ));
        }

        let now = chrono::Utc::now().timestamp_millis();
        let values: Vec<String> = updates
            .iter()
            .map(|(anchor_type, anchor_id, radius, source_signature, projection_version, vector)| {
                format!(
                    "('{}', '{}', {}, '{}', '{}', '{}', CAST({:?} AS FLOAT[384]), {})",
                    Self::escape_embedding_sql(anchor_type),
                    Self::escape_embedding_sql(anchor_id),
                    radius,
                    Self::escape_embedding_sql(model_id),
                    Self::escape_embedding_sql(source_signature),
                    Self::escape_embedding_sql(projection_version),
                    vector,
                    now
                )
            })
            .collect();

        for chunk in values.chunks(100) {
            queries.push(format!(
                "INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES {};",
                chunk.join(",")
            ));
        }

        self.execute_batch(&queries)
    }
}

#[derive(Debug, Clone, Copy)]
struct SemanticPolicy {
    pause: bool,
    sleep: Duration,
    idle_sleep: Duration,
}

fn semantic_policy(queue_len: usize, service_pressure: ServicePressure) -> SemanticPolicy {
    if service_pressure == ServicePressure::Critical || queue_len >= 3_000 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(10),
            idle_sleep: Duration::from_secs(10),
        };
    }

    if service_pressure == ServicePressure::Degraded || queue_len >= 1_500 {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(3),
            idle_sleep: Duration::from_secs(5),
        };
    }

    if service_pressure == ServicePressure::Recovering {
        return SemanticPolicy {
            pause: true,
            sleep: Duration::from_secs(2),
            idle_sleep: Duration::from_secs(3),
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
    use crate::service_guard::ServicePressure;
    use std::time::Duration;

    #[test]
    fn test_fatal_embedding_error_detection() {
        assert!(is_fatal_embedding_error(&"GetElementType is not implemented"));
        assert!(is_fatal_embedding_error(&"onnxruntime failure"));
        assert!(!is_fatal_embedding_error(&"temporary timeout"));
    }

    #[test]
    fn test_semantic_policy_runs_when_system_is_healthy() {
        let policy = semantic_policy(100, ServicePressure::Healthy);
        assert!(!policy.pause);
        assert_eq!(policy.idle_sleep, Duration::from_secs(5));
    }

    #[test]
    fn test_semantic_policy_pauses_under_queue_pressure() {
        let policy = semantic_policy(2_000, ServicePressure::Healthy);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_pauses_when_live_service_is_critical() {
        let policy = semantic_policy(100, ServicePressure::Critical);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(10));
    }

    #[test]
    fn test_semantic_policy_pauses_when_service_is_degraded() {
        let policy = semantic_policy(100, ServicePressure::Degraded);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(3));
    }

    #[test]
    fn test_semantic_policy_stays_paused_while_service_recovers() {
        let policy = semantic_policy(100, ServicePressure::Recovering);
        assert!(policy.pause);
        assert_eq!(policy.sleep, Duration::from_secs(2));
        assert_eq!(policy.idle_sleep, Duration::from_secs(3));
    }
}
