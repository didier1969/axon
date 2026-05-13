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
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::warn;

use crate::graph::GraphStore;

use super::metrics::StageMetrics;
use super::project_resolver::ProjectCodeResolver;
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
/// `resolver` returns the project_code for `parsed.path`. DEC-AXO-081
/// allows a single pipeline_v2 instance to serve N projects — the
/// resolver is invoked once per call, the resulting code stamps every
/// Symbol / Chunk / IndexedFile row written for this file.
///
/// Idempotent: re-running A3 on the same [`ParsedFile`] is a no-op for
/// the canonical rows (every INSERT inside [`GraphStore::upsert_graph_v2`]
/// uses `ON CONFLICT DO UPDATE` / `DO NOTHING`).
pub async fn a3_enroll(
    parsed: ParsedFile,
    store: Arc<GraphStore>,
    resolver: ProjectCodeResolver,
) -> Result<EnrolledFile> {
    let path_str = parsed.path.to_string_lossy().into_owned();
    let now_ms = Utc::now().timestamp_millis();
    let project_code_str = resolver(&parsed.path);

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

/// REQ-AXO-295 — Spawn the canonical batched A3 worker.
///
/// Mirrors the pattern of [`super::stage_b2::spawn_b2_batched_worker`]:
/// blocks on the first `ParsedFile`, then drains additional files until
/// `batch_size` or `batch_timeout`, and writes the whole batch in one
/// `GraphStore::upsert_graph_v2_batch` round-trip — one BEGIN/COMMIT
/// per batch instead of one per file. The chunk_ids returned for each
/// file are individually `try_send`-fanned to the B1 inbox; the
/// downstream `tx.send` carries the per-file [`EnrolledFile`] receipt.
///
/// `resolver` returns the project_code per file (DEC-AXO-081). Each
/// flush groups the batch by resolved project_code and issues one
/// `upsert_graph_v2_batch` per group — keeping the single-PG-transaction
/// guarantee within each project while letting a single pipeline_v2
/// serve N projects.
///
/// Metrics: `record_started` on each item entering the batch,
/// `record_finished` with per-item mean duration after the batched
/// write commits. `record_error` per item if the batch write fails.
pub fn spawn_a3_batched_worker(
    mut rx: Receiver<ParsedFile>,
    tx: Sender<EnrolledFile>,
    b1_inbox_tx: Sender<String>,
    store: Arc<GraphStore>,
    resolver: ProjectCodeResolver,
    metrics: Arc<StageMetrics>,
    batch_size: usize,
    batch_timeout: Duration,
) {
    let batch_size = batch_size.max(1);
    tokio::spawn(async move {
        // REQ-AXO-295 — tick-based semantics: every `batch_timeout`
        // ms we attempt to flush whatever is queued. If the queue is
        // empty at tick time we do nothing. If it reaches
        // `batch_size` between two ticks we flush early without
        // waiting for the tick. The timer is anchored on the last
        // flush (or worker start) — NOT on the arrival of the first
        // item, so a single straggler waits AT MOST `batch_timeout`
        // ms from when it enters the buffer.
        let mut tick = tokio::time::interval(batch_timeout);
        // Skip the immediate first tick that `interval` fires; with
        // an empty buffer it would be a no-op anyway.
        tick.tick().await;
        let mut buffer: Vec<ParsedFile> = Vec::with_capacity(batch_size);

        loop {
            let flush_now = tokio::select! {
                biased;
                received = rx.recv() => {
                    match received {
                        Some(item) => {
                            buffer.push(item);
                            buffer.len() >= batch_size
                        }
                        None => {
                            // Upstream closed — drain remaining
                            // buffer then exit.
                            if buffer.is_empty() {
                                return;
                            }
                            true
                        }
                    }
                }
                _ = tick.tick() => {
                    !buffer.is_empty()
                }
            };

            if !flush_now {
                continue;
            }

            let upstream_closed_after_drain = rx.is_closed() && buffer.len() < batch_size;
            let batch: Vec<ParsedFile> = std::mem::take(&mut buffer);
            for _ in &batch {
                metrics.record_started();
            }

            // DEC-AXO-081 — group the batch by per-file resolved
            // project_code so each upsert_graph_v2_batch call still
            // sees a homogeneous-project group (the SQL renderer
            // assumes one project_code per call).
            let mut groups: std::collections::BTreeMap<String, Vec<ParsedFile>> =
                std::collections::BTreeMap::new();
            for parsed in batch {
                let code = resolver(&parsed.path);
                groups.entry(code).or_default().push(parsed);
            }

            let started = Instant::now();
            let total_items: usize = groups.values().map(|v| v.len()).sum();
            for (pc_str, group_batch) in groups {
                let store_clone = store.clone();
                let group_for_block = group_batch.clone();
                let join_result = tokio::task::spawn_blocking(move || {
                    store_clone.upsert_graph_v2_batch(&group_for_block, &pc_str)
                })
                .await;

                let group_len = group_batch.len();
                match join_result {
                    Ok(Ok(chunk_ids_per_file)) if chunk_ids_per_file.len() == group_len => {
                        let elapsed_us = started.elapsed().as_micros().min(u128::from(u64::MAX))
                            as u64;
                        let per_item_us = elapsed_us / (total_items as u64).max(1);
                        let now_ms = Utc::now().timestamp_millis();
                        for (parsed, chunk_ids) in
                            group_batch.into_iter().zip(chunk_ids_per_file.into_iter())
                        {
                            for cid in &chunk_ids {
                                let _ = b1_inbox_tx.try_send(cid.clone());
                            }
                            let receipt = EnrolledFile {
                                path: parsed.path.to_string_lossy().into_owned(),
                                content_hash: parsed.content_hash,
                                symbols_count: parsed.symbols.len(),
                                relations_count: parsed.relations.len(),
                                last_seen_ms: now_ms,
                                chunk_ids,
                            };
                            metrics.record_finished(per_item_us);
                            if tx.send(receipt).await.is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Ok(chunk_ids_per_file)) => {
                        warn!(
                            stage = "A3",
                            expected = group_len,
                            actual = chunk_ids_per_file.len(),
                            "upsert_graph_v2_batch returned mismatched per-file count"
                        );
                        for _ in 0..group_len {
                            metrics.record_error();
                        }
                    }
                    Ok(Err(err)) => {
                        warn!(stage = "A3", error = ?err, "upsert_graph_v2_batch failed");
                        for _ in 0..group_len {
                            metrics.record_error();
                        }
                    }
                    Err(join_err) => {
                        warn!(
                            stage = "A3",
                            error = ?join_err,
                            "spawn_blocking joined with error"
                        );
                        for _ in 0..group_len {
                            metrics.record_error();
                        }
                    }
                }
            }

            if upstream_closed_after_drain {
                return;
            }
        }
    });
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

        let receipt = a3_enroll(parsed, store.clone(), super::super::const_resolver("AXO"))
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

        a3_enroll(parsed, store.clone(), super::super::const_resolver("AXO"))
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

        a3_enroll(parsed, store.clone(), super::super::const_resolver("AXO"))
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

        let r1 = a3_enroll(parsed_a, store.clone(), super::super::const_resolver("AXO"))
            .await
            .unwrap();
        let r2 = a3_enroll(parsed_b, store.clone(), super::super::const_resolver("AXO"))
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

        a3_enroll(parsed_v1, store.clone(), super::super::const_resolver("AXO"))
            .await
            .unwrap();
        a3_enroll(parsed_v2, store.clone(), super::super::const_resolver("AXO"))
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
