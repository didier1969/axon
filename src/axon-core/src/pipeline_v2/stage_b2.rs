//! Stage B2 — GPU embedder (CPT-AXO-054 session 19).
//!
//! B2 receives [`super::stage_b1::ChunkForEmbedding`] payloads from B1
//! (already content-resolved against `public.Chunk`), forwards the text
//! through a [`B2Embedder`] implementation (the production wrapper around
//! `OrtGpuFirstTextEmbedding` + TensorRT BGE-Large for live deployments,
//! a deterministic no-op for tests), and emits an [`EmbeddedChunk`] for
//! B3 to persist.
//!
//! **Batching is the embedder's responsibility.** The B2 worker hands
//! one [`ChunkForEmbedding`] at a time to the trait. Production
//! implementations that need GPU batching aggregate inside the trait
//! via an internal buffer + flush rule; the worker pool just keeps
//! feeding chunks. Slice S4b ships the single-item interface and a
//! no-op embedder; a batched production wrapper lands separately
//! against the existing `OrtGpuFirstTextEmbedding` once REQ-AXO-262
//! IoBinding refactor stabilises the GPU hot path.

use std::sync::Arc;

use anyhow::Result;

use super::stage_b1::ChunkForEmbedding;

/// Payload forwarded by B2 to B3 — chunk identity + the embedding the
/// GPU produced + the `content_hash` source_hash that B3 records on the
/// `ChunkEmbedding` row to spot stale embeddings.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedChunk {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedding: Vec<f32>,
}

/// Pluggable embedder trait. Production wraps
/// `OrtGpuFirstTextEmbedding` (TensorRT BGE-Large 1024d) behind this
/// surface; tests use [`NoOpEmbedder`] to keep the topology assertions
/// hardware-independent.
pub trait B2Embedder: Send + Sync {
    /// Embed `texts` and return the same-length Vec of embedding vectors.
    /// Each `Vec<f32>` length must equal the model dimension (1024 for
    /// the canonical BGE-Large model). The trait is sync because the
    /// caller wraps it in `spawn_blocking` — moving GPU work off the
    /// tokio runtime stays the right move under all backends.
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Deterministic test embedder. Emits `[1.0, 0.0, 0.0, ..., 0.0]` per
/// input text (dimension = [`crate::embedding_contract::DIMENSION`]).
/// Useful to exercise the B2 → B3 worker topology without touching
/// CUDA / TensorRT in unit tests.
pub struct NoOpEmbedder;

impl B2Embedder for NoOpEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        use crate::embedding_contract::DIMENSION;
        let mut out = Vec::with_capacity(texts.len());
        for _ in texts {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            out.push(v);
        }
        Ok(out)
    }
}

/// Embed a single [`ChunkForEmbedding`] payload, return an
/// [`EmbeddedChunk`].
///
/// The actual ORT/TensorRT call is dispatched via the supplied
/// [`B2Embedder`]. The call is wrapped in [`tokio::task::spawn_blocking`]
/// so the GPU dispatch does not stall the tokio runtime (mandatory under
/// `live` mode where B1's PG fetch and B2's GPU embed share the same
/// runtime).
pub async fn b2_embed(
    payload: ChunkForEmbedding,
    embedder: Arc<dyn B2Embedder>,
) -> Result<EmbeddedChunk> {
    let chunk_id = payload.chunk_id.clone();
    let source_hash = payload.content_hash.clone();
    let content = payload.content;

    let embedder_for_block = embedder.clone();
    let embedding = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
        let texts = vec![content];
        let mut out = embedder_for_block.embed_batch(&texts)?;
        if out.is_empty() {
            return Err(anyhow::anyhow!("B2: embedder returned 0 embeddings for 1 input"));
        }
        Ok(out.remove(0))
    })
    .await??;

    Ok(EmbeddedChunk {
        chunk_id,
        source_hash,
        embedding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_op_embedder_returns_canonical_dimension_vectors() {
        use crate::embedding_contract::DIMENSION;
        let payload = ChunkForEmbedding {
            chunk_id: "AXO::demo::sym::chunk".to_string(),
            content: "fn alpha() {}".to_string(),
            content_hash: "deadbeef".to_string(),
        };
        let embedder: Arc<dyn B2Embedder> = Arc::new(NoOpEmbedder);
        let result = b2_embed(payload, embedder).await.unwrap();

        assert_eq!(result.chunk_id, "AXO::demo::sym::chunk");
        assert_eq!(result.source_hash, "deadbeef");
        assert_eq!(
            result.embedding.len(),
            DIMENSION,
            "embedding must match canonical model dimension"
        );
        // Sanity check the no-op shape.
        assert_eq!(result.embedding[0], 1.0);
        assert!(result.embedding[1..].iter().all(|v| *v == 0.0));
    }

    #[tokio::test]
    async fn b2_embed_surfaces_zero_result_as_error() {
        struct ZeroEmbedder;
        impl B2Embedder for ZeroEmbedder {
            fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
                Ok(Vec::new())
            }
        }

        let payload = ChunkForEmbedding {
            chunk_id: "x".to_string(),
            content: "y".to_string(),
            content_hash: "z".to_string(),
        };
        let embedder: Arc<dyn B2Embedder> = Arc::new(ZeroEmbedder);
        let res = b2_embed(payload, embedder).await;
        assert!(res.is_err(), "missing embedding must propagate as error");
    }
}
