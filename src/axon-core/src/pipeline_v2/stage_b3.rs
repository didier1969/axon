//! Stage B3 — UPSERT ChunkEmbedding (CPT-AXO-054 session 19).
//!
//! B3 receives [`super::stage_b2::EmbeddedChunk`] payloads and persists
//! them via [`crate::graph::GraphStore::upsert_chunk_embedding_v2`]
//! (`ON CONFLICT (chunk_id, model_id) DO UPDATE`). The Chunk row B2
//! embedded was already written by A3, so B3 only touches
//! `public.ChunkEmbedding`.
//!
//! B3 is the canonical write boundary for the vector lane — a successful
//! commit means the chunk is queryable via pgvector ANN search. Crash
//! between B2 and B3 = lost in RAM; cold-start poll DB (slice S4c)
//! catches the chunk on next boot.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;

use crate::graph::GraphStore;

use super::stage_b2::EmbeddedChunk;

/// Receipt emitted by B3 once the embedding committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedEmbedding {
    pub chunk_id: String,
    pub source_hash: String,
    pub embedded_at_ms: i64,
}

/// UPSERT `embedded`'s embedding row into `public.ChunkEmbedding`.
///
/// `project_code` is the canonical 3-letter code the indexer is rooted
/// at (CPT-AXO-053 single-project per indexer instance). The write is
/// wrapped in [`tokio::task::spawn_blocking`] so the synchronous SQL
/// dispatch does not stall the tokio runtime.
pub async fn b3_persist_embedding(
    embedded: EmbeddedChunk,
    store: Arc<GraphStore>,
    project_code: Arc<str>,
) -> Result<PersistedEmbedding> {
    let chunk_id = embedded.chunk_id.clone();
    let source_hash = embedded.source_hash.clone();
    let embedding = embedded.embedding;
    let now_ms = Utc::now().timestamp_millis();
    let project_code_str = project_code.to_string();

    let store_clone = store.clone();
    let chunk_id_for_block = chunk_id.clone();
    let source_hash_for_block = source_hash.clone();
    tokio::task::spawn_blocking(move || {
        store_clone.upsert_chunk_embedding_v2(
            &chunk_id_for_block,
            &project_code_str,
            &source_hash_for_block,
            &embedding,
            now_ms,
        )
    })
    .await??;

    Ok(PersistedEmbedding {
        chunk_id,
        source_hash,
        embedded_at_ms: now_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Symbol;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: "function".into(),
            start_line: 1,
            end_line: 2,
            docstring: None,
            is_entry_point: false,
            is_public: false,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        }
    }

    /// Seed a Chunk row via A3, then have B3 persist a no-op embedding
    /// for it. ChunkEmbedding row must exist after the UPSERT.
    #[tokio::test]
    async fn b3_persists_chunk_embedding_after_a3_seeded_the_chunk_row() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_demo_target() { let q = 1; }\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b3_demo.rs",
                "AXO",
                body,
                "hash-b3",
                1_700_000_000_010,
                &[sym("b3_demo_target")],
                &[],
            )
            .unwrap();
        assert!(!chunk_ids.is_empty());

        let embedding = {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            v
        };
        let cid = chunk_ids[0].clone();
        let payload = EmbeddedChunk {
            chunk_id: cid.clone(),
            source_hash: "hash-b3-chunk".to_string(),
            embedding,
        };

        let receipt = b3_persist_embedding(payload, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        assert_eq!(receipt.chunk_id, cid);
        assert!(receipt.embedded_at_ms > 0);

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(n, 1, "B3 must persist exactly one ChunkEmbedding row");
    }

    #[tokio::test]
    async fn b3_is_idempotent_on_repeated_persist_for_same_chunk_id() {
        use crate::embedding_contract::DIMENSION;

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn b3_idem() {}\n";
        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b3_idem.rs",
                "AXO",
                body,
                "hash-b3i",
                1_700_000_000_011,
                &[sym("b3_idem")],
                &[],
            )
            .unwrap();
        let cid = chunk_ids[0].clone();

        let mk_payload = || -> EmbeddedChunk {
            let mut v = vec![0.0_f32; DIMENSION];
            v[0] = 1.0;
            EmbeddedChunk {
                chunk_id: cid.clone(),
                source_hash: "hash-b3i-chunk".to_string(),
                embedding: v,
            }
        };

        b3_persist_embedding(mk_payload(), store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        b3_persist_embedding(mk_payload(), store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = '{cid}'"
            ))
            .unwrap();
        assert_eq!(n, 1, "ON CONFLICT must keep exactly one row per (chunk_id, model_id)");
    }
}
