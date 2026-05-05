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

const ROWS_PER_PARTITION: usize = 10_000;
const BYTES_PER_PARTITION: usize = 5 * 1024 * 1024;

pub struct ParquetChunkContentStore {
    base_dir: PathBuf,
    current: Mutex<Option<CurrentPartition>>,
}

struct CurrentPartition {
    path: PathBuf,
    writer: ArrowWriter<File>,
    rows_written: usize,
    bytes_written: usize,
}

impl ParquetChunkContentStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            current: Mutex::new(None),
        }
    }

    /// Append a batch of `(chunk_id, content_hash, content)` rows to the
    /// current Parquet partition. `content_hash` mirrors `Chunk.content_hash`
    /// in DuckDB so retrieve_context can JOIN on `chunk_id + content_hash` to
    /// filter out stale rows when chunks have been re-projected with new
    /// content but the Parquet store still carries the prior version.
    pub fn append_batch(&self, rows: &[(String, String, String)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let batch_bytes: usize = rows
            .iter()
            .map(|(id, h, c)| id.len() + h.len() + c.len())
            .sum();
        let mut guard = self.current.lock().expect("parquet partition lock poisoned");
        let need_rotate = match guard.as_ref() {
            None => true,
            Some(part) => {
                part.rows_written + rows.len() > ROWS_PER_PARTITION
                    || part.bytes_written + batch_bytes > BYTES_PER_PARTITION
            }
        };
        if need_rotate {
            if let Some(mut part) = guard.take() {
                part.writer.close().context("closing previous partition")?;
            }
            let path = self.next_partition_path()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("creating partition dir")?;
            }
            let file = File::create(&path).context("creating partition file")?;
            let writer = ArrowWriter::try_new(file, Self::schema(), Some(Self::props()))
                .context("opening parquet writer")?;
            *guard = Some(CurrentPartition {
                path,
                writer,
                rows_written: 0,
                bytes_written: 0,
            });
        }
        let part = guard.as_mut().expect("partition just opened");
        let batch = Self::rows_to_batch(rows)?;
        part.writer.write(&batch).context("parquet write batch")?;
        part.rows_written += rows.len();
        part.bytes_written += batch_bytes;
        Ok(())
    }

    pub fn flush(&self) -> Result<()> {
        let mut guard = self.current.lock().expect("parquet partition lock poisoned");
        if let Some(mut part) = guard.take() {
            part.writer.close().context("flushing parquet partition")?;
        }
        Ok(())
    }

    pub fn current_partition_path(&self) -> Option<PathBuf> {
        self.current
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.path.clone()))
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
    fn append_batch_rotates_partition_at_row_threshold() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        // Fill most of the partition with tiny content so we hit the row cap, not byte cap.
        let mut rows: Vec<(String, String, String)> = (0..ROWS_PER_PARTITION - 10)
            .map(|i| fixture_row(&format!("c{}", i), "x"))
            .collect();
        store.append_batch(&rows).unwrap();
        let part_a = store.current_partition_path().unwrap();
        // Overflow row cap.
        rows = (0..50)
            .map(|i| fixture_row(&format!("c2-{}", i), "x"))
            .collect();
        store.append_batch(&rows).unwrap();
        let part_b = store.current_partition_path().unwrap();
        assert_ne!(part_a, part_b, "partition rotated when row threshold crossed");
        store.flush().unwrap();
        assert!(part_a.exists());
        assert!(part_b.exists());
    }

    #[test]
    fn append_batch_rotates_partition_at_byte_threshold() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetChunkContentStore::new(tmp.path().to_path_buf());
        // 50 rows of ~120 KB each -> 6 MB > 5 MB cap, but only 50 rows (well under 10k cap).
        // Force rotate via byte budget, not row count.
        let big_content = "y".repeat(120 * 1024);
        let rows: Vec<(String, String, String)> = (0..50)
            .map(|i| fixture_row(&format!("c{}", i), &big_content))
            .collect();
        store.append_batch(&rows).unwrap();
        let part_a = store.current_partition_path().unwrap();
        let next: Vec<(String, String, String)> = (0..5)
            .map(|i| fixture_row(&format!("c2-{}", i), "small"))
            .collect();
        store.append_batch(&next).unwrap();
        let part_b = store.current_partition_path().unwrap();
        assert_ne!(part_a, part_b, "partition rotated when byte threshold crossed");
        store.flush().unwrap();
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
