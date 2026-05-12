//! Stage A3 — Enregistrement graphe + chunks + FTS (CPT-AXO-054, session 19 topology).
//!
//! A3 is the **single-transaction persistence stage** for pipeline A. It
//! consumes a [`ParsedFile`] from A2 and writes — atomically via
//! [`GraphStore::upsert_graph_v2`]:
//!
//!   * `public.Symbol` (UPSERT, idempotent)
//!   * AGE `Symbol` + `File` vertex enrichment (under PG)
//!   * `CONTAINS` / `CALLS` / `CALLS_NIF` edges (SQL + AGE dual-write)
//!   * `public.Chunk` rows with full `content` text — REQ-AXO-292 PG FTS
//!     attaches automatically through the `content_tsv` GENERATED column,
//!     so the lexical retrieval lane is ready **without any GPU**
//!     dependency. SOTA hybrid retrieval: lexical + structural on CPU,
//!     vector enrichment optional.
//!   * `public.IndexedFile(path, content_hash, last_seen_ms)` watcher
//!     filter row
//!
//! The chunk_ids persisted are returned to the orchestrator so the A3
//! worker can `try_send` them to the B1 inbox (best-effort, non-blocking)
//! for the GPU embedder lane. If the channel is full, B1's cold-start
//! poll DB pathway (slice S4c) catches the drop.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;

use crate::graph::GraphStore;

use super::types::ParsedFile;

/// Receipt emitted by A3 once persistence committed.
///
/// Carries the chunk_ids the row produced so the orchestrator can fan
/// them out to B1. `symbols_count` / `relations_count` are kept for
/// observability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledFile {
    pub path: String,
    pub content_hash: String,
    pub symbols_count: usize,
    pub relations_count: usize,
    pub last_seen_ms: i64,
    pub chunk_ids: Vec<String>,
}

/// Persist `parsed`'s graph + chunks atomically and return the receipt
/// with chunk_ids ready to fan out to B1.
///
/// Idempotent: re-running A3 on the same [`ParsedFile`] is a no-op for
/// the canonical rows (every INSERT inside [`GraphStore::upsert_graph_v2`]
/// uses `ON CONFLICT DO UPDATE` / `DO NOTHING`).
pub async fn a3_enroll(
    parsed: ParsedFile,
    store: Arc<GraphStore>,
    project_code: Arc<str>,
) -> Result<EnrolledFile> {
    let path_str = parsed.path.to_string_lossy().into_owned();
    let now_ms = Utc::now().timestamp_millis();
    let project_code_str = project_code.to_string();

    let store_clone = store.clone();
    let path_for_block = path_str.clone();
    let hash_for_block = parsed.content_hash.clone();
    let content_for_block = parsed.content.clone();
    let symbols_for_block = parsed.symbols.clone();
    let relations_for_block = parsed.relations.clone();
    let chunk_ids = tokio::task::spawn_blocking(move || {
        store_clone.upsert_graph_v2(
            &path_for_block,
            &project_code_str,
            &content_for_block,
            &hash_for_block,
            now_ms,
            &symbols_for_block,
            &relations_for_block,
        )
    })
    .await??;

    Ok(EnrolledFile {
        path: path_str,
        content_hash: parsed.content_hash,
        symbols_count: parsed.symbols.len(),
        relations_count: parsed.relations.len(),
        last_seen_ms: now_ms,
        chunk_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn sym(name: &str) -> crate::parser::Symbol {
        crate::parser::Symbol {
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
            properties: std::collections::HashMap::new(),
            embedding: None,
        }
    }

    fn parsed_with(path: &str, content: &str, hash: &str, symbols: Vec<&str>) -> ParsedFile {
        ParsedFile {
            path: PathBuf::from(path),
            content: content.to_string(),
            content_hash: hash.to_string(),
            mtime_ms: 1_700_000_000_000,
            size_bytes: content.len() as u64,
            symbols: symbols.into_iter().map(sym).collect(),
            relations: vec![],
        }
    }

    #[tokio::test]
    async fn a3_enroll_writes_indexed_file_row_with_supplied_hash() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed = parsed_with("/tmp/demo_indexed.rs", "fn demo() {}", "hash-abc", vec!["demo"]);

        let receipt = a3_enroll(parsed, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        assert_eq!(receipt.path, "/tmp/demo_indexed.rs");
        assert_eq!(receipt.content_hash, "hash-abc");
        assert_eq!(receipt.symbols_count, 1);
        assert!(receipt.last_seen_ms > 0);
        assert!(
            !receipt.chunk_ids.is_empty(),
            "A3 must emit at least one chunk_id for a parseable symbol"
        );

        let count = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/demo_indexed.rs' AND content_hash = 'hash-abc'",
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a3_enroll_persists_symbol_and_chunk_rows() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed = parsed_with(
            "/tmp/sym_chunk.rs",
            "fn alpha() {}\nfn beta() {}",
            "hash-sc",
            vec!["alpha", "beta"],
        );

        a3_enroll(parsed, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let symbol_count = store
            .query_count(
                "SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name IN ('alpha','beta')",
            )
            .unwrap();
        assert!(
            symbol_count >= 2,
            "A3 must persist Symbol rows for the two parsed fns"
        );

        let chunk_count = store
            .query_count("SELECT count(*) FROM Chunk WHERE file_path = '/tmp/sym_chunk.rs'")
            .unwrap();
        assert!(
            chunk_count >= 1,
            "A3 must persist Chunk rows in the same transaction (session 19)"
        );
    }

    #[tokio::test]
    async fn a3_enroll_full_content_text_persists_for_fts() {
        // REQ-AXO-292: PG FTS attaches to `Chunk.content` via a
        // GENERATED `content_tsv` column. A3 must persist full content
        // text so the GIN index has material to tokenise.
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let marker = "UNIQ_MARKER_TENSORRT_42";
        let body = format!("fn carry() {{ let s = \"{marker}\"; }}\n");
        let parsed = parsed_with("/tmp/a3_fts.rs", &body, "hash-fts", vec!["carry"]);

        a3_enroll(parsed, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let n = store
            .query_count(&format!(
                "SELECT count(*) FROM Chunk WHERE file_path = '/tmp/a3_fts.rs' AND content LIKE '%{marker}%'"
            ))
            .unwrap();
        assert!(
            n >= 1,
            "A3 Chunk row must carry the full content text for FTS GIN"
        );
    }

    #[tokio::test]
    async fn a3_enroll_is_idempotent_on_repeated_calls_with_same_hash() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed_a = parsed_with(
            "/tmp/idem_v2.rs",
            "fn idem() {}",
            "hash-1",
            vec!["idem"],
        );
        let parsed_b = parsed_a.clone();

        let r1 = a3_enroll(parsed_a, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        let r2 = a3_enroll(parsed_b, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        assert_eq!(
            r1.chunk_ids, r2.chunk_ids,
            "two enrolments over the same content must emit identical chunk_id Vecs"
        );

        let indexed_count = store
            .query_count("SELECT count(*) FROM IndexedFile WHERE path = '/tmp/idem_v2.rs'")
            .unwrap();
        assert_eq!(indexed_count, 1);

        let symbol_count = store
            .query_count("SELECT count(*) FROM Symbol WHERE name = 'idem' AND project_code = 'AXO'")
            .unwrap();
        assert_eq!(symbol_count, 1);

        let chunk_count = store
            .query_count("SELECT count(*) FROM Chunk WHERE file_path = '/tmp/idem_v2.rs'")
            .unwrap();
        assert_eq!(chunk_count, r1.chunk_ids.len() as i64);
    }

    #[tokio::test]
    async fn a3_enroll_updates_hash_on_content_change() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed_v1 = parsed_with("/tmp/change_v2.rs", "fn v1() {}", "hash-v1", vec!["v1"]);
        let parsed_v2 = parsed_with("/tmp/change_v2.rs", "fn v2() {}", "hash-v2", vec!["v2"]);

        a3_enroll(parsed_v1, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        a3_enroll(parsed_v2, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let after = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/change_v2.rs' AND content_hash = 'hash-v2'",
            )
            .unwrap();
        assert_eq!(after, 1, "ON CONFLICT must UPDATE the hash in place");

        let stale = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/change_v2.rs' AND content_hash = 'hash-v1'",
            )
            .unwrap();
        assert_eq!(stale, 0, "previous hash must be overwritten");
    }
}
