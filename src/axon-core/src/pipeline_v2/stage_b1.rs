//! Stage B1 types module (CPT-AXO-054, slice 5 SOTA collapse).
//!
//! After slice 5, B1 as a stage worker disappears : its work (SELECT
//! chunk content + emit ChunkForEmbedding) collapsed into
//! `demand_pull::pull_and_feed_b` (one round-trip SELECT-with-content).
//!
//! Remaining surface here :
//! - `ChunkForEmbedding` payload struct consumed by B2
//! - `EmbeddingDedupCache` + `load_embedding_dedup_cache` (dedup state
//!   hydrated at boot, applied inline by demand_pull)
//! - `b1_fetch_for_embedding` (test-only helper, exercises single-row
//!   SELECT path through `fetch_chunk_for_embedding`)

use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use anyhow::Context;

use crate::graph::GraphStore;

/// Load all existing (chunk_id, source_hash) pairs from ChunkEmbedding
/// for hydrating the embedding dedup cache at boot.
pub fn load_embedding_dedup_cache(store: &GraphStore) -> Result<Arc<dashmap::DashMap<String, String>>> {
    let model_id = crate::embedding_contract::CHUNK_MODEL_ID;
    let safe = model_id.replace('\'', "''");
    let raw = store.query_json_writer(&format!(
        "SELECT chunk_id, source_hash FROM chunkembedding WHERE model_id = '{safe}'"
    ))?;
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let map = dashmap::DashMap::with_capacity(rows.len());
    for row in rows {
        if let (Some(cid), Some(hash)) = (
            row.first().and_then(|v| v.as_str()),
            row.get(1).and_then(|v| v.as_str()),
        ) {
            map.insert(cid.to_string(), hash.to_string());
        }
    }
    Ok(Arc::new(map))
}

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
/// moves on; the demand-pull DB pathway catches anything the channel
/// missed.
#[cfg(test)]
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

/// REQ-AXO-901748 — set of `(chunk_id, content_hash)` pairs that
/// already have a valid embedding in ChunkEmbedding. Demand-pull skips
/// pending chunks that match, avoiding redundant GPU work on re-index.
///
/// Slice 5 SOTA — `spawn_b1_batched_worker*` deleted. The dedup logic
/// now lives inline in `demand_pull::pull_and_feed_b` (single batched
/// SELECT-with-content + dedup retain).
pub type EmbeddingDedupCache = Option<Arc<dashmap::DashMap<String, String>>>;

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
