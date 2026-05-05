//! Parquet side-store for Chunk.content (DEC-AXO-074).
//!
//! Mirrors the architecture of `embedder::parquet_embedding_store` (DEC-AXO-073)
//! to address the second-order DuckDB column-store growth bottleneck identified
//! in VAL-AXO-038: per-batch Chunk.content INSERT payload (~7.4 MB per Writer
//! Actor commit on the Axon repo) drives geometric `commit_ms` growth and caps
//! e2e throughput at ~57 ch/s. The cheap-falsification probe (Chunk.content
//! substituted with a 64-byte stub) lifted throughput to 122.5 ch/s (2.14x),
//! confirming structural headroom for routing the variable-length payload to
//! append-only Parquet instead of DuckDB column-store.
//!
//! ## Storage layout
//!
//! ```
//! <base_dir>/
//!   2026-05-05T17/
//!     part-00001-<unique>.parquet  (up to 10000 rows or 5 MB)
//!     part-00002-<unique>.parquet
//!   2026-05-05T18/
//!     ...
//! ```
//!
//! Files are append-only within their lifetime; once a partition reaches
//! either ROWS_PER_PARTITION or BYTES_PER_PARTITION it is closed and a new
//! file is opened. Partitions are smaller than the embedding store (~5 MB
//! vs ~40 MB) because content is variable-length and bigger per row, and
//! retrieve_context predicate-pushdown benefits from finer partition
//! granularity.
//!
//! ## Read path
//!
//! Out of scope for this module (M.1). retrieve_context joins these files
//! via `parquet_scan('<base_dir>/**/*.parquet')` LEFT JOIN with
//! `COALESCE(c.content, p.content)` once M.3 lands.
//!
//! ## Activation
//!
//! Gated by `AXON_PARQUET_CHUNK_CONTENT_ENABLED=true`. When disabled (default
//! for M.1) the graph projection keeps writing the full content column to
//! DuckDB Chunk.content. Behavior is bit-identical to commit `acb9675`.

use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

pub struct ParquetChunkContentStore {
    base_dir: PathBuf,
    last_path: Mutex<Option<PathBuf>>,
}

impl ParquetChunkContentStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            last_path: Mutex::new(None),
        }
    }

    /// Append a batch of `(chunk_id, content_hash, content)` rows to a fresh
    /// Parquet file (one file per batch). Each file is immediately closed so
    /// the footer is written and `parquet_scan` can read it on the next
    /// vector-lane fetch cycle. Without this property the indexer-side
    /// COALESCE read path returns empty content (Parquet's reader requires
    /// the file footer; an unclosed file is unreadable), which collapses
    /// throughput to 0 ch/s. Validated empirically in val39 m4 run #1
    /// before this fix landed.
    ///
    /// `content_hash` mirrors `Chunk.content_hash` in DuckDB so
    /// retrieve_context can JOIN on `chunk_id + content_hash` to filter
    /// out stale rows when chunks have been re-projected with new content
    /// but the Parquet store still carries the prior version.
    pub fn append_batch(&self, rows: &[(String, String, String)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let path = self.next_partition_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating partition dir")?;
        }
        let file = File::create(&path).context("creating partition file")?;
        let mut writer = ArrowWriter::try_new(file, Self::schema(), Some(Self::props()))
            .context("opening parquet writer")?;
        let batch = Self::rows_to_batch(rows)?;
        writer.write(&batch).context("parquet write batch")?;
        writer.close().context("closing parquet writer")?;
        *self.last_path.lock().expect("parquet last_path lock poisoned") = Some(path);
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        // Each batch is a self-contained closed file; flush is a no-op.
        Ok(())
    }

    pub fn current_partition_path(&self) -> Option<PathBuf> {
        self.last_path.lock().ok().and_then(|g| g.clone())
    }

    fn schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("chunk_id", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
        ]))
    }

    fn props() -> WriterProperties {
        WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build()
    }

    fn rows_to_batch(rows: &[(String, String, String)]) -> Result<RecordBatch> {
        let chunk_ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
        let chunk_id_array: ArrayRef = Arc::new(StringArray::from(chunk_ids));

        let hashes: Vec<&str> = rows.iter().map(|(_, h, _)| h.as_str()).collect();
        let hash_array: ArrayRef = Arc::new(StringArray::from(hashes));

        let contents: Vec<&str> = rows.iter().map(|(_, _, c)| c.as_str()).collect();
        let content_array: ArrayRef = Arc::new(StringArray::from(contents));

        Ok(RecordBatch::try_new(
            Self::schema(),
            vec![chunk_id_array, hash_array, content_array],
        )?)
    }

    fn next_partition_path(&self) -> Result<PathBuf> {
        let now = chrono::Utc::now();
        let hour = now.format("%Y-%m-%dT%H").to_string();
        let unique = now
            .timestamp_nanos_opt()
            .unwrap_or(now.timestamp_millis() * 1_000_000);
        Ok(self
            .base_dir
            .join(hour)
            .join(format!("part-{}.parquet", unique)))
    }
}

static STORE: OnceLock<Arc<ParquetChunkContentStore>> = OnceLock::new();

pub fn install(store: Arc<ParquetChunkContentStore>) -> bool {
    STORE.set(store).is_ok()
}

pub fn store() -> Option<Arc<ParquetChunkContentStore>> {
    STORE.get().cloned()
}

pub fn parquet_chunk_content_enabled() -> bool {
    std::env::var("AXON_PARQUET_CHUNK_CONTENT_ENABLED")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub fn default_base_dir() -> PathBuf {
    std::env::var("AXON_DB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".axon-dev/graph_v2"))
        .join("chunk_content")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_row(chunk_id: &str, content: &str) -> (String, String, String) {
        (
            chunk_id.to_string(),
            format!("hash-{}", chunk_id),
            content.to_string(),
        )
    }

    #[test]
    fn append_batch_writes_parquet_partition_file() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        let rows = vec![
            fixture_row("c1", "fn foo() { let x = 1; }"),
            fixture_row("c2", "pub struct Bar { name: String }"),
        ];
        store.append_batch(&rows).unwrap();
        let part = store.current_partition_path().unwrap();
        store.flush().unwrap();
        assert!(part.exists(), "partition file written: {}", part.display());
        let metadata = std::fs::metadata(&part).unwrap();
        assert!(metadata.len() > 0, "partition not empty");
    }

    #[test]
    fn each_append_batch_writes_its_own_closed_file() {
        // Per-batch file ensures parquet_scan can read mid-stream — vector
        // lane needs the footer written before fetch_unembedded_chunks_batch
        // can JOIN. See val39 m4 run #1 for the empirical motivation.
        let tmp = TempDir::new().unwrap();
        let store = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        for batch in 0..3 {
            let rows = vec![
                fixture_row(&format!("c{batch}-1"), "fn one() {}"),
                fixture_row(&format!("c{batch}-2"), "fn two() {}"),
            ];
            store.append_batch(&rows).unwrap();
        }
        let mut files: Vec<PathBuf> = Vec::new();
        for hour_dir in std::fs::read_dir(tmp.path()).unwrap() {
            let hour_dir = hour_dir.unwrap();
            if hour_dir.file_type().unwrap().is_dir() {
                for f in std::fs::read_dir(hour_dir.path()).unwrap() {
                    files.push(f.unwrap().path());
                }
            }
        }
        assert_eq!(files.len(), 3, "one file per batch: {files:?}");
        for f in files {
            let metadata = std::fs::metadata(&f).unwrap();
            assert!(metadata.len() > 0, "file {} is non-empty (footer present)", f.display());
        }
    }

    #[test]
    fn empty_batch_is_noop() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        store.append_batch(&[]).unwrap();
        assert!(store.current_partition_path().is_none());
    }

    #[test]
    fn parquet_chunk_content_enabled_honors_truthy_env() {
        let prev = std::env::var("AXON_PARQUET_CHUNK_CONTENT_ENABLED").ok();
        for v in ["true", "1", "yes", "on"] {
            std::env::set_var("AXON_PARQUET_CHUNK_CONTENT_ENABLED", v);
            assert!(parquet_chunk_content_enabled(), "expected on for {v}");
        }
        for v in ["false", "0", "off", ""] {
            std::env::set_var("AXON_PARQUET_CHUNK_CONTENT_ENABLED", v);
            assert!(!parquet_chunk_content_enabled(), "expected off for {v:?}");
        }
        match prev {
            Some(v) => std::env::set_var("AXON_PARQUET_CHUNK_CONTENT_ENABLED", v),
            None => std::env::remove_var("AXON_PARQUET_CHUNK_CONTENT_ENABLED"),
        }
    }

    #[test]
    fn schema_has_three_utf8_columns() {
        let schema = ParquetChunkContentStore::schema();
        assert_eq!(schema.fields().len(), 3);
        for field in schema.fields() {
            assert_eq!(field.data_type(), &DataType::Utf8, "{}", field.name());
            assert!(!field.is_nullable(), "{} should be NOT NULL", field.name());
        }
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["chunk_id", "content_hash", "content"]);
    }
}
