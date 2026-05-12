//! Stage A3 — Enregistrement graphe (CPT-AXO-054, session 18 topology).
//!
//! A3 is the **graph-only** persistence stage for pipeline A. It consumes
//! a [`ParsedFile`] from A2 and writes — in a single atomic transaction
//! via [`GraphStore::upsert_graph_v2`]:
//!
//!   * `public.Symbol` rows (UPSERT, idempotent)
//!   * AGE `Symbol` + `File` vertex enrichment (when AGE dual-write is
//!     active under PG)
//!   * `CONTAINS` / `CALLS` / `CALLS_NIF` relation edges (SQL + AGE)
//!   * `public.IndexedFile(path, content_hash, last_seen_ms)` — the
//!     minimal watcher-filter row
//!
//! A3 does **not** persist `public.Chunk` rows. Chunking is the entry
//! stage of pipeline B (B1) — running it inside the graph pipeline
//! would slow the graph for vector-pipeline preparation work that has
//! no graph-side consumer. PG FTS (REQ-AXO-292) attaches to B1's
//! Chunk INSERTs via a GENERATED `content_tsv` column, so the lexical
//! retrieval lane comes for free as a side-effect — no separate FTS
//! stage exists in the topology.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;

use crate::graph::GraphStore;

use super::types::ParsedFile;

/// Receipt emitted by A3 once graph persistence committed successfully.
///
/// Carries the bare minimum for downstream observability and for the
/// B-pipeline hand-off: the file path, the content_hash that was
/// recorded in `IndexedFile`, the counts (informational), and the
/// commit timestamp. The chunking work that B1 performs derives its
/// inputs from the same path + content that A1 read, so this receipt
/// does NOT carry chunk_ids — those don't exist yet at A3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledFile {
    pub path: String,
    pub content_hash: String,
    pub symbols_count: usize,
    pub relations_count: usize,
    pub last_seen_ms: i64,
}

/// Persist `parsed`'s graph layer atomically and return an
/// [`EnrolledFile`] receipt.
///
/// Idempotent: re-running A3 on the same [`ParsedFile`] is a no-op
/// (every INSERT inside [`GraphStore::upsert_graph_v2`] uses ON CONFLICT
/// DO UPDATE / DO NOTHING).
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
    let symbols_for_block = parsed.symbols.clone();
    let relations_for_block = parsed.relations.clone();
    tokio::task::spawn_blocking(move || {
        store_clone.upsert_graph_v2(
            &path_for_block,
            &project_code_str,
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

        let count = store
            .query_count(
                "SELECT count(*) FROM IndexedFile WHERE path = '/tmp/demo_indexed.rs' AND content_hash = 'hash-abc'",
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a3_enroll_persists_symbol_row_for_each_parsed_symbol() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed = parsed_with(
            "/tmp/sym_demo.rs",
            "fn alpha() {}\nfn beta() {}",
            "hash-syms",
            vec!["alpha", "beta"],
        );

        a3_enroll(parsed, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let alpha = store
            .query_count(
                "SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name = 'alpha'",
            )
            .unwrap();
        let beta = store
            .query_count("SELECT count(*) FROM Symbol WHERE project_code = 'AXO' AND name = 'beta'")
            .unwrap();
        assert_eq!(alpha, 1, "A3 must persist Symbol row for `alpha`");
        assert_eq!(beta, 1, "A3 must persist Symbol row for `beta`");
    }

    #[tokio::test]
    async fn a3_enroll_does_not_persist_chunk_rows() {
        // A3 is graph-only — chunking is B1's job (slice S4). Re-confirm
        // that this invariant holds so future LLMs don't regress it.
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let parsed = parsed_with(
            "/tmp/no_chunks.rs",
            "fn x() { 1 + 1; }",
            "hash-nc",
            vec!["x"],
        );

        a3_enroll(parsed, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let chunks = store
            .query_count("SELECT count(*) FROM Chunk WHERE file_path = '/tmp/no_chunks.rs'")
            .unwrap();
        assert_eq!(
            chunks, 0,
            "A3 must NOT persist Chunk rows — that's B1's responsibility (slice S4)"
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

        a3_enroll(parsed_a, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();
        a3_enroll(parsed_b, store.clone(), Arc::from("AXO"))
            .await
            .unwrap();

        let indexed_count = store
            .query_count("SELECT count(*) FROM IndexedFile WHERE path = '/tmp/idem_v2.rs'")
            .unwrap();
        assert_eq!(indexed_count, 1, "IndexedFile UPSERT must not duplicate");

        let symbol_count = store
            .query_count("SELECT count(*) FROM Symbol WHERE name = 'idem' AND project_code = 'AXO'")
            .unwrap();
        assert_eq!(symbol_count, 1, "Symbol UPSERT must not duplicate");
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
