//! Stage B1 — Fetch chunk content for the GPU embedder (CPT-AXO-054, session 19 topology).
//!
//! B1 is the **entry stage of pipeline B** (vectorisation lane only —
//! lexical/FTS is handled CPU-side by A3's Chunk INSERT). It receives a
//! `chunk_id: String` from A3's `try_send` fan-out (or from the cold-start
//! poll DB pathway, slice S4c), SELECTs the chunk's text content from
//! `public.Chunk`, and forwards `(chunk_id, content, content_hash)` to
//! the B2 embedder.
//!
//! **No tree-sitter, no chunking here.** A3 already derived the chunks
//! and UPSERTed them with `content_tsv` GENERATED for FTS. B1 is a pure
//! DB-read + bucketing stage, GPU-driven by B2's pace through the
//! downstream channel.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::graph::GraphStore;

/// Output of stage B1 — the payload B2 (GPU embedder) consumes.
///
/// `chunk_id` is the PK in `public.Chunk`. `content` is the raw text to
/// embed (capped at the model's tokenizer max len upstream of B2 via the
/// seq-len bucketing — REQ-AXO-262). `content_hash` lets B3 dedup
/// embeddings against the same source revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkForEmbedding {
    pub chunk_id: String,
    pub content: String,
    pub content_hash: String,
}

/// SELECT the chunk content for a given `chunk_id`.
///
/// Wraps the DB call inside [`tokio::task::spawn_blocking`] to keep the
/// tokio runtime responsive under multi-worker B1 contention. Returns
/// `Ok(None)` if the row was deleted between A3's `try_send` and B1's
/// read (race with a re-parse) — the worker drops the item silently and
/// moves on; the cold-start poll DB pathway (slice S4c) catches anything
/// the channel missed.
pub async fn b1_fetch_for_embedding(
    chunk_id: String,
    store: Arc<GraphStore>,
) -> Result<Option<ChunkForEmbedding>> {
    let store_clone = store.clone();
    let id_for_block = chunk_id.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        store_clone.fetch_chunk_for_embedding(&id_for_block)
    })
    .await
    .context("B1 fetch task panicked or was cancelled")??;

    Ok(fetched.map(|(content, content_hash)| ChunkForEmbedding {
        chunk_id,
        content,
        content_hash,
    }))
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

    /// Seed Chunk rows via A3's canonical path, then verify B1 can
    /// SELECT them back with content intact.
    #[tokio::test]
    async fn b1_fetches_chunk_content_after_a3_upsert() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn marker_for_b1_fetch_test() { let x = 1; }\n";

        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_fetch.rs",
                "AXO",
                body,
                "hash-fetch",
                1_700_000_000_000,
                &[sym("marker_for_b1_fetch_test")],
                &[],
            )
            .unwrap();
        assert!(!chunk_ids.is_empty(), "A3 must emit at least one chunk_id");

        let cid = chunk_ids[0].clone();
        let fetched = b1_fetch_for_embedding(cid.clone(), store.clone())
            .await
            .unwrap()
            .expect("Chunk row must exist after A3 UPSERT");

        assert_eq!(fetched.chunk_id, cid);
        assert!(
            !fetched.content.is_empty(),
            "content must be non-empty for a Rust fn fixture"
        );
        assert!(
            !fetched.content_hash.is_empty(),
            "content_hash must be populated by A3"
        );
    }

    #[tokio::test]
    async fn b1_returns_none_for_missing_chunk_id() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let res = b1_fetch_for_embedding("AXO::nonexistent::path::sym::chunk".to_string(), store)
            .await
            .unwrap();
        assert!(res.is_none(), "B1 must return None for unknown chunk_id");
    }

    #[tokio::test]
    async fn a3_returns_chunk_ids_that_b1_can_round_trip() {
        // Session-19 contract: A3 returns Vec<String> chunk_ids; every
        // returned id must be addressable by B1 from PG. This locks the
        // try_send fan-out contract A3 → B1 inbox.
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let body = "fn one() {}\nfn two() {}\nfn three() {}\n";

        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_roundtrip.rs",
                "AXO",
                body,
                "hash-rt",
                1_700_000_000_001,
                &[sym("one"), sym("two"), sym("three")],
                &[],
            )
            .unwrap();
        assert!(
            chunk_ids.len() >= 3,
            "expected ≥3 chunk_ids from 3 symbols (saw {})",
            chunk_ids.len()
        );

        for cid in chunk_ids {
            let fetched = b1_fetch_for_embedding(cid.clone(), store.clone())
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("chunk_id {cid} from A3 must round-trip via B1"));
            assert_eq!(fetched.chunk_id, cid);
        }
    }
}
