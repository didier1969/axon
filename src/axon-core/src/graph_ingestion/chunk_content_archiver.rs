//! DEC-AXO-074 Direction A — background Chunk.content archiver.
//!
//! Periodically moves the variable-length `Chunk.content` payload from
//! DuckDB to the Parquet side-store AFTER chunks have been embedded.
//! This avoids the parquet_scan glob overhead on the vector-lane hot
//! path (REQ-AXO-193 falsified inline-write design via VAL-AXO-039:
//! 44 ch/s vs 57 baseline).
//!
//! ## Loop
//!
//! Every `ARCHIVE_INTERVAL` (30s):
//! 1. SELECT up to `ARCHIVE_BATCH_SIZE` chunks where:
//!    - their file is `vector_ready=TRUE` (file fully embedded)
//!    - `content_archived=FALSE` (not yet moved)
//!    - `content` is non-empty (skip already-cleared rows)
//! 2. Append the batch to `ParquetChunkContentStore` (each call writes
//!    + closes a self-contained Parquet file → immediately readable).
//! 3. UPDATE Chunk SET content='', content_archived=TRUE for those rows.
//!
//! ## Crash safety
//!
//! Step 2 may run idempotently after a crash between (2) and (3): the
//! same chunks reappear in the next pass and re-append to Parquet
//! (different filename per call, both files contain identical
//! `(chunk_id, content_hash, content)` tuples). Reads via COALESCE
//! pick any matching row — content is identical for a given hash, so
//! duplicates are harmless. Step 3 idempotency: `content_archived=TRUE`
//! short-circuits subsequent passes.
//!
//! ## Activation
//!
//! Gated by `AXON_PARQUET_CHUNK_CONTENT_ENABLED=true`. When disabled
//! (default), the spawn function is a no-op; legacy DuckDB Chunk.content
//! path runs unchanged.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{info, warn};

use super::parquet_chunk_content_store::ParquetChunkContentStore;
use crate::graph::GraphStore;

const ARCHIVE_INTERVAL: Duration = Duration::from_secs(30);
const ARCHIVE_BATCH_SIZE: usize = 1000;

/// Spawn the archiver thread. Caller must have already installed the
/// `ParquetChunkContentStore` singleton.
pub fn spawn(graph_store: Arc<GraphStore>, parquet_store: Arc<ParquetChunkContentStore>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(ARCHIVE_INTERVAL);
        match archive_one_pass(&graph_store, &parquet_store) {
            Ok(0) => {}
            Ok(n) => info!("chunk_content archiver: archived {} chunks", n),
            Err(e) => warn!("chunk_content archiver pass failed: {:?}", e),
        }
    });
    info!(
        "chunk_content archiver: spawned (interval={}s, batch={})",
        ARCHIVE_INTERVAL.as_secs(),
        ARCHIVE_BATCH_SIZE
    );
}

/// Single archive pass. Returns the number of chunks archived (0 if no
/// work). Public for tests + manual one-shot invocation.
pub fn archive_one_pass(
    graph_store: &GraphStore,
    parquet_store: &ParquetChunkContentStore,
) -> Result<usize> {
    let select_query = format!(
        "SELECT c.id, c.content_hash, c.content \
         FROM Chunk c \
         INNER JOIN File f ON f.path = c.file_path \
         WHERE f.vector_ready = TRUE \
           AND COALESCE(c.content_archived, FALSE) = FALSE \
           AND c.content != '' \
         LIMIT {ARCHIVE_BATCH_SIZE}"
    );
    let raw = graph_store
        .query_json(&select_query)
        .context("archiver: SELECT pending chunks")?;
    if raw == "[]" || raw.is_empty() {
        return Ok(0);
    }
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(&raw).context("archiver: parse JSON")?;

    let mut tuples: Vec<(String, String, String)> = Vec::with_capacity(rows.len());
    for row in &rows {
        if row.len() < 3 {
            continue;
        }
        let id = row[0].as_str().unwrap_or("").to_string();
        let hash = row[1].as_str().unwrap_or("").to_string();
        let content = row[2].as_str().unwrap_or("").to_string();
        if id.is_empty() || content.is_empty() {
            continue;
        }
        tuples.push((id, hash, content));
    }
    if tuples.is_empty() {
        return Ok(0);
    }

    parquet_store
        .append_batch(&tuples)
        .context("archiver: append_batch to Parquet")?;

    let id_list = tuples
        .iter()
        .map(|(id, _, _)| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let update_query = format!(
        "UPDATE Chunk SET content = '', content_archived = TRUE WHERE id IN ({id_list})"
    );
    graph_store
        .execute(&update_query)
        .context("archiver: UPDATE clear DuckDB content")?;

    Ok(tuples.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn install_chunk(store: &GraphStore, project: &str, file_path: &str, vector_ready: bool) {
        store
            .bulk_insert_files(&[(file_path.to_string(), project.to_string(), 1, 1)])
            .unwrap();
        if vector_ready {
            store
                .execute(&format!(
                    "UPDATE File SET vector_ready = TRUE WHERE path = '{file_path}'"
                ))
                .unwrap();
        }
        store
            .execute(&format!(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash) \
                 VALUES ('chunk-{project}-1', 'symbol', 'sym-1', '{project}', '{file_path}', 'function', \
                         'fn hello() {{ println!(\"world\"); }}', 'hash-1')"
            ))
            .unwrap();
    }

    #[test]
    fn archive_one_pass_skips_files_not_yet_vector_ready() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let tmp = TempDir::new().unwrap();
        let parquet = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        install_chunk(&graph, "PRJ", "/tmp/a.rs", false);
        let archived = archive_one_pass(&graph, &parquet).unwrap();
        assert_eq!(archived, 0, "file not ready -> no chunks archived");
    }

    #[test]
    fn archive_one_pass_moves_content_for_ready_files() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let tmp = TempDir::new().unwrap();
        let parquet = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        install_chunk(&graph, "PRJ", "/tmp/b.rs", true);

        let archived = archive_one_pass(&graph, &parquet).unwrap();
        assert_eq!(archived, 1, "ready file -> 1 chunk archived");

        // DuckDB content cleared, archived flag set.
        let raw = graph
            .query_json("SELECT content, content_archived FROM Chunk WHERE id = 'chunk-PRJ-1'")
            .unwrap();
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap();
        let content_after = rows[0][0].as_str().unwrap_or("");
        assert_eq!(content_after, "", "content cleared after archive (got: {:?})", &rows[0][0]);
        // DuckDB JSON serializes BOOLEAN as a string "true"/"false".
        let archived = match &rows[0][1] {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::String(s) => s == "true",
            v => panic!("unexpected content_archived value: {v:?}"),
        };
        assert!(archived, "content_archived flag set");

        // Parquet partition file exists.
        let mut found = false;
        for hour in std::fs::read_dir(tmp.path()).unwrap() {
            for f in std::fs::read_dir(hour.unwrap().path()).unwrap() {
                let p = f.unwrap().path();
                if p.extension().and_then(|s| s.to_str()) == Some("parquet") {
                    found = true;
                    assert!(std::fs::metadata(&p).unwrap().len() > 0);
                }
            }
        }
        assert!(found, "parquet partition file written");
    }

    #[test]
    fn archive_one_pass_idempotent_on_already_archived_rows() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let tmp = TempDir::new().unwrap();
        let parquet = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        install_chunk(&graph, "PRJ", "/tmp/c.rs", true);

        let first = archive_one_pass(&graph, &parquet).unwrap();
        assert_eq!(first, 1);
        let second = archive_one_pass(&graph, &parquet).unwrap();
        assert_eq!(second, 0, "already-archived rows skipped on second pass");
    }

    #[test]
    fn archive_one_pass_handles_empty_select() {
        let graph = crate::tests::test_helpers::create_test_db().unwrap();
        let tmp = TempDir::new().unwrap();
        let parquet = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        // No chunks at all.
        let archived = archive_one_pass(&graph, &parquet).unwrap();
        assert_eq!(archived, 0);
    }

    // Suppress unused warning when only some tests exercise this fixture path.
    fn _retain<T>(_v: T) {}
    #[allow(dead_code)]
    const _ARC_HINT: fn(Arc<()>) = |_| {};
}
