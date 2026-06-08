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
/// `chunk_id` is the PK in `ist.Chunk`. `content` is the raw text to
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

    // REQ-AXO-901877 — the GraphStore test fixture (`create_test_db`) and the
    // sync store methods (`upsert_graph_v2`, `query_count`) drive the PG plugin
    // via an internal `block_on`, so they must run OUTSIDE a tokio runtime.
    // These tests therefore stay sync (`#[test]`) and drive only the genuinely
    // async stage fn (which internally uses `spawn_blocking`) through a local
    // current-thread runtime — avoiding the "runtime within a runtime" panic
    // that `#[tokio::test]` triggered at fixture construction.
    fn run_async<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    fn sym(name: &str) -> Symbol {
        sym_span(name, 1, 2)
    }

    fn sym_span(name: &str, start_line: usize, end_line: usize) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: "function".into(),
            start_line,
            end_line,
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
    #[test]
    fn b1_fetches_chunk_content_after_a3_upsert() {
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
        let fetched = run_async(b1_fetch_for_embedding(cid.clone(), store.clone()))
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

    #[test]
    fn b1_returns_none_for_missing_chunk_id() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let res = run_async(b1_fetch_for_embedding(
            "AXO::nonexistent::path::sym::chunk".to_string(),
            store,
        ))
        .unwrap();
        assert!(res.is_none(), "B1 must return None for unknown chunk_id");
    }

    #[test]
    fn a3_returns_chunk_ids_that_b1_can_round_trip() {
        // Session-19 contract: A3 returns Vec<String> chunk_ids; every
        // returned id must be addressable by B1 from PG. This locks the
        // try_send fan-out contract A3 → B1 inbox.
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());

        // Each fn body must clear MIN_FUSE_TOKENS (100) so fuse_small_chunks does
        // NOT coalesce the three symbols into one chunk (REQ-AXO-901846) — else
        // we'd get <3 chunk_ids and the fan-out contract wouldn't be exercised.
        // 14 distinct statements per body ≈ ~200 tokens each.
        let block = |n: &str| -> String {
            let lines: Vec<String> = (0..14)
                .map(|i| format!("    let v_{n}_{i} = {i} + {i} * 3 - 1;"))
                .collect();
            format!("fn {n}() {{\n{}\n}}\n", lines.join("\n"))
        };
        let body = format!("{}{}{}", block("one"), block("two"), block("three"));
        // Each block is 16 lines (fn + 14 body + closing brace): one=1..=16,
        // two=17..=32, three=33..=48 — build_symbol_chunks slices file_content by
        // these spans, so they must match the concatenation.
        let symbols = [
            sym_span("one", 1, 16),
            sym_span("two", 17, 32),
            sym_span("three", 33, 48),
        ];

        let chunk_ids = store
            .upsert_graph_v2(
                "/tmp/b1_roundtrip.rs",
                "AXO",
                &body,
                "hash-rt",
                1_700_000_000_001,
                &symbols,
                &[],
            )
            .unwrap();
        assert!(
            chunk_ids.len() >= 3,
            "expected ≥3 chunk_ids from 3 non-fusable symbols (saw {})",
            chunk_ids.len()
        );

        for cid in chunk_ids {
            let fetched = run_async(b1_fetch_for_embedding(cid.clone(), store.clone()))
                .unwrap()
                .unwrap_or_else(|| panic!("chunk_id {cid} from A3 must round-trip via B1"));
            assert_eq!(fetched.chunk_id, cid);
        }
    }
}
