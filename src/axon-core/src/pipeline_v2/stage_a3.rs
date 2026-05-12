//! Stage A3 — Enregistrement worker (CPT-AXO-054).
//!
//! Receives a [`ParsedFile`] from A2, persists the watcher-filter row in
//! `IndexedFile` (path, content_hash, last_seen_ms), and emits an
//! [`EnrolledFile`] receipt that downstream code can use to push chunk IDs to
//! B1 (slice S4 wires that channel — for now the receipt simply confirms
//! enrollment succeeded).
//!
//! **Scope of this slice (S3c)**: IndexedFile UPSERT only — the minimum to
//! validate end-to-end A1 → A2 → A3 streaming and the watcher filter loop.
//! Symbol / AGE-edge / Chunk persistence stays on the legacy ingestion path
//! and is migrated to A3 in a follow-up slice (S3d) so each change stays
//! small and reviewable. The legacy `bulk_upsert_file_queries` +
//! `insert_file_data_batch` machinery keeps writing the graph until the
//! cut-over (slice S7).

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;

use crate::graph::GraphStore;

use super::types::ParsedFile;

/// Receipt emitted by A3 once enrollment succeeded.
///
/// Carries the bare minimum the downstream stages need: the source path
/// (so logs / metrics correlate), the content_hash that was persisted, the
/// number of symbols / relations the parse produced (purely informational
/// until S3d wires the Symbol UPSERT), and the timestamp recorded in
/// `IndexedFile.last_seen_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledFile {
    pub path: String,
    pub content_hash: String,
    pub symbols_count: usize,
    pub relations_count: usize,
    pub last_seen_ms: i64,
}

/// Persist `parsed`'s watcher-filter row and return an [`EnrolledFile`]
/// receipt.
///
/// Idempotent: re-running A3 on the same `ParsedFile` is a no-op for the
/// existing `IndexedFile` row except for refreshing `last_seen_ms`. The
/// UPSERT `ON CONFLICT (path) DO UPDATE` clause carries the intent.
pub async fn a3_enroll(parsed: ParsedFile, store: Arc<GraphStore>) -> Result<EnrolledFile> {
    let path_str = parsed.path.to_string_lossy().into_owned();
    let now_ms = Utc::now().timestamp_millis();

    // CPU-light single SQL UPSERT — keep on the tokio runtime; spawn_blocking
    // would add scheduling overhead without throughput benefit for one
    // statement.
    let store_clone = store.clone();
    let path_for_block = path_str.clone();
    let hash_for_block = parsed.content_hash.clone();
    tokio::task::spawn_blocking(move || {
        store_clone.upsert_indexed_file(&path_for_block, &hash_for_block, now_ms)
    })
    .await??;

    Ok(EnrolledFile {
        path: path_str,
        content_hash: parsed.content_hash,
        symbols_count: parsed.symbols.len(),
        relations_count: parsed.relations.len(),
        last_seen_ms: now_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn parsed_with(path: &str, content: &str, hash: &str, symbols: usize) -> ParsedFile {
        use crate::parser::Symbol;
        let symbols_vec = (0..symbols)
            .map(|i| Symbol {
                name: format!("sym_{i}"),
                kind: "function".into(),
                start_line: 1,
                end_line: 2,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: std::collections::HashMap::new(),
                embedding: None,
            })
            .collect();
        ParsedFile {
            path: PathBuf::from(path),
            content: content.to_string(),
            content_hash: hash.to_string(),
            mtime_ms: 1_700_000_000_000,
            size_bytes: content.len() as u64,
            symbols: symbols_vec,
            relations: vec![],
        }
    }

    #[tokio::test]
    async fn a3_enroll_writes_indexed_file_row_with_supplied_hash() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed = parsed_with("/tmp/demo.rs", "fn demo() {}", "hash-abc", 1);

        let receipt = a3_enroll(parsed, store.clone()).await.unwrap();

        assert_eq!(receipt.path, "/tmp/demo.rs");
        assert_eq!(receipt.content_hash, "hash-abc");
        assert_eq!(receipt.symbols_count, 1);
        assert!(receipt.last_seen_ms > 0);

        let count = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/demo.rs' AND content_hash = 'hash-abc'",
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a3_enroll_is_idempotent_on_repeated_calls_with_same_hash() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed_a = parsed_with("/tmp/idem.rs", "fn idem() {}", "hash-1", 0);
        let parsed_b = parsed_with("/tmp/idem.rs", "fn idem() {}", "hash-1", 0);

        a3_enroll(parsed_a, store.clone()).await.unwrap();
        a3_enroll(parsed_b, store.clone()).await.unwrap();

        let count = store
            .query_count("SELECT count(*) FROM IndexedFile WHERE path = '/tmp/idem.rs'")
            .unwrap();
        assert_eq!(count, 1, "UPSERT must not create duplicate rows");
    }

    #[tokio::test]
    async fn a3_enroll_updates_hash_on_content_change() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed_v1 = parsed_with("/tmp/change.rs", "fn v1() {}", "hash-v1", 0);
        let parsed_v2 = parsed_with("/tmp/change.rs", "fn v2() {}", "hash-v2", 0);

        a3_enroll(parsed_v1, store.clone()).await.unwrap();
        a3_enroll(parsed_v2, store.clone()).await.unwrap();

        let after = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/change.rs' AND content_hash = 'hash-v2'",
            )
            .unwrap();
        assert_eq!(after, 1, "ON CONFLICT must UPDATE the hash in place");

        let stale = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/change.rs' AND content_hash = 'hash-v1'",
            )
            .unwrap();
        assert_eq!(stale, 0, "previous hash must be overwritten");
    }
}
