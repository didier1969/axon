//! Parquet side-store for ChunkEmbedding (DEC-AXO-073).
//!
//! Sidesteps the DuckDB column-store INSERT scaling problem identified in
//! VAL-AXO-034 (commit_ms grew geometrically 132ms->22429ms over 80s) by
//! routing FLOAT[1024] vectors to append-only Parquet files instead of
//! the canonical DuckDB ChunkEmbedding table. VAL-AXO-036 confirmed this
//! path is sound: skipping the DuckDB INSERT entirely lifted throughput
//! from 23.9 ch/s to 79.6 ch/s on the Axon repo.
//!
//! ## Storage layout
//!
//! ```
//! <base_dir>/
//!   2026-05-05T17/
//!     part-00001-<unique>.parquet  (up to ROWS_PER_PARTITION rows)
//!     part-00002-<unique>.parquet
//!   2026-05-05T18/
//!     ...
//! ```
//!
//! Files are append-only within their lifetime; once a partition reaches
//! ROWS_PER_PARTITION it is closed and a new file is opened. Partition
//! boundaries are hourly, giving DuckDB's `parquet_scan` natural
//! partition pruning when retrieve_context filters by recency.
//!
//! ## Read path
//!
//! Out of scope for this module. retrieve_context joins these files via
//! `parquet_scan('<base_dir>/**/*.parquet')` (added in a follow-up patch).
//!
//! ## Activation
//!
//! Gated by `AXON_PARQUET_EMBEDDING_STORE_ENABLED=true`. When disabled
//! (default) the vector lane keeps writing to DuckDB ChunkEmbedding via
//! `update_chunk_embeddings` (commit G + H.2 path).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, FixedSizeListArray, Float32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

const EMBEDDING_DIM: usize = 1024;

pub struct ParquetEmbeddingStore {
    base_dir: PathBuf,
    last_path: Mutex<Option<PathBuf>>,
}

impl ParquetEmbeddingStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            last_path: Mutex::new(None),
        }
    }

    /// Append a batch of `(chunk_id, source_hash, embedding)` rows to a
    /// fresh Parquet file (one file per batch, written + closed atomically
    /// so the footer is present and `parquet_scan` can read it on the
    /// next reader cycle). The original long-running-file design left the
    /// current partition open until rotation, which means readers (L.3
    /// retrieve_context, mark_done logic, archivers) saw 0-byte
    /// unfinalized files mid-stream and failed with "File too small to
    /// be a Parquet file" (REQ-AXO-194 Bug 1, observed during VAL-AXO-040).
    ///
    /// `source_hash` is the Chunk content hash at embed time — used by
    /// retrieve_context to filter out stale vectors when chunk content
    /// has since been modified.
    pub fn append_batch(&self, rows: &[(String, String, Vec<f32>)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        for (chunk_id, _source_hash, embedding) in rows {
            if embedding.len() != EMBEDDING_DIM {
                return Err(anyhow::anyhow!(
                    "parquet_store: embedding for {} has dim {} (expected {})",
                    chunk_id,
                    embedding.len(),
                    EMBEDDING_DIM
                ));
            }
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
            Field::new("source_hash", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    EMBEDDING_DIM as i32,
                ),
                false,
            ),
        ]))
    }

    fn props() -> WriterProperties {
        WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::default()))
            .build()
    }

    fn rows_to_batch(rows: &[(String, String, Vec<f32>)]) -> Result<RecordBatch> {
        let chunk_ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
        let chunk_id_array: ArrayRef = Arc::new(StringArray::from(chunk_ids));

        let source_hashes: Vec<&str> = rows.iter().map(|(_, h, _)| h.as_str()).collect();
        let source_hash_array: ArrayRef = Arc::new(StringArray::from(source_hashes));

        let flat: Vec<f32> = rows
            .iter()
            .flat_map(|(_, _, e)| e.iter().copied())
            .collect();
        let values = Float32Array::from(flat);
        let item_field = Arc::new(Field::new("item", DataType::Float32, false));
        let embed_array: ArrayRef = Arc::new(FixedSizeListArray::new(
            item_field,
            EMBEDDING_DIM as i32,
            Arc::new(values),
            None,
        ));

        Ok(RecordBatch::try_new(
            Self::schema(),
            vec![chunk_id_array, source_hash_array, embed_array],
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

static STORE: OnceLock<Arc<ParquetEmbeddingStore>> = OnceLock::new();

pub fn install(store: Arc<ParquetEmbeddingStore>) -> bool {
    STORE.set(store).is_ok()
}

pub fn store() -> Option<Arc<ParquetEmbeddingStore>> {
    STORE.get().cloned()
}

pub fn parquet_store_enabled() -> bool {
    std::env::var("AXON_PARQUET_EMBEDDING_STORE_ENABLED")
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
        .join("embeddings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_row(chunk_id: &str, fill: f32) -> (String, String, Vec<f32>) {
        (
            chunk_id.to_string(),
            format!("hash-{}", chunk_id),
            vec![fill; EMBEDDING_DIM],
        )
    }

    #[test]
    fn append_batch_writes_parquet_partition_file() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetEmbeddingStore::new(tmp.path().to_path_buf());
        let rows = vec![fixture_row("c1", 0.1), fixture_row("c2", 0.2)];
        store.append_batch(&rows).unwrap();
        let part = store.current_partition_path().unwrap();
        store.flush().unwrap();
        assert!(part.exists(), "partition file written: {}", part.display());
        let metadata = std::fs::metadata(&part).unwrap();
        assert!(metadata.len() > 0, "partition not empty");
    }

    #[test]
    fn append_batch_rejects_wrong_dimension() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetEmbeddingStore::new(tmp.path().to_path_buf());
        let bad = vec![("c1".to_string(), "h".to_string(), vec![0.0_f32; 512])];
        let err = store.append_batch(&bad).unwrap_err();
        assert!(err.to_string().contains("dim 512"), "{err}");
    }

    #[test]
    fn each_append_batch_writes_its_own_closed_file() {
        // REQ-AXO-194 Bug 1: per-batch close ensures footer is written so
        // parquet_scan can read mid-stream. Without this fix, downstream
        // readers (L.3 retrieve_context, archivers) saw 0-byte unfinalized
        // files.
        let tmp = TempDir::new().unwrap();
        let store = ParquetEmbeddingStore::new(tmp.path().to_path_buf());
        for batch in 0..3 {
            let rows = vec![
                fixture_row(&format!("c{batch}-1"), 0.1 * batch as f32),
                fixture_row(&format!("c{batch}-2"), 0.2 * batch as f32),
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
    fn parquet_store_enabled_honors_truthy_env() {
        let prev = std::env::var("AXON_PARQUET_EMBEDDING_STORE_ENABLED").ok();
        for v in ["true", "1", "yes", "on"] {
            std::env::set_var("AXON_PARQUET_EMBEDDING_STORE_ENABLED", v);
            assert!(parquet_store_enabled(), "expected on for {v}");
        }
        for v in ["false", "0", "off", ""] {
            std::env::set_var("AXON_PARQUET_EMBEDDING_STORE_ENABLED", v);
            assert!(!parquet_store_enabled(), "expected off for {v:?}");
        }
        match prev {
            Some(v) => std::env::set_var("AXON_PARQUET_EMBEDDING_STORE_ENABLED", v),
            None => std::env::remove_var("AXON_PARQUET_EMBEDDING_STORE_ENABLED"),
        }
    }

    #[test]
    fn empty_batch_is_noop() {
        let tmp = TempDir::new().unwrap();
        let store = ParquetEmbeddingStore::new(tmp.path().to_path_buf());
        store.append_batch(&[]).unwrap();
        assert!(store.current_partition_path().is_none());
    }

    fn _ignore_path<P: AsRef<Path>>(_p: P) {}
}
