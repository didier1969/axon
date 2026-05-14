//! Stage A1 — Preparation worker (CPT-AXO-054).
//!
//! Reads a file from disk, computes its SHA-256 content hash, and packages
//! everything into a [`PreparedFile`] for the next stage. This is the
//! I/O-bound stage of pipeline A; throughput scales with the number of
//! workers (default 4 live, 2 dev) and underlying disk parallelism.
//!
//! The hashing choice is SHA-256 because the same hash is later persisted to
//! `public.IndexedFile.content_hash` and read back at indexer startup to
//! rebuild the in-RAM filter cache. SHA-256 is cross-restart stable, while
//! Rust's `DefaultHasher` is not.

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::info;

use super::types::PreparedFile;

/// Read `path`, compute its content hash, and return the prepared payload.
///
/// This is the closure handed to [`super::spawn_stage_workers`] for stage A1.
/// Errors are surfaced verbatim so the worker pool can record them in
/// `StageMetrics::errors_total` without crashing the pipeline.
pub async fn a1_prepare(path: PathBuf) -> Result<PreparedFile> {
    // REQ-AXO-345 — A1 in/out trace for silent-drop hunt.
    info!(target: "pipeline_v2::a1", "A1 in: {}", path.display());
    let content = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("A1 read failed for {}", path.display()))?;

    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("A1 metadata failed for {}", path.display()))?;

    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as i64)
        .unwrap_or(0);

    let size_bytes = metadata.len();
    let content_hash = sha256_hex(&content);

    info!(
        target: "pipeline_v2::a1",
        "A1 out: {} size={}",
        path.display(),
        size_bytes
    );
    Ok(PreparedFile {
        path,
        content,
        content_hash,
        mtime_ms,
        size_bytes,
    })
}

/// Helper — hex-encoded SHA-256 of an arbitrary byte slice.
///
/// Public-in-module so subsequent stages (A2 chunk hashes, B1 chunk lookup
/// keys) can reuse the same digest function without re-implementing it.
pub(crate) fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn a1_prepare_returns_content_plus_stable_hash() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "fn main() {{ println!(\"hi\"); }}").unwrap();
        let path = file.path().to_path_buf();

        let prep = a1_prepare(path.clone()).await.unwrap();

        assert_eq!(prep.path, path);
        assert_eq!(prep.content, "fn main() { println!(\"hi\"); }");
        assert_eq!(prep.content.len() as u64, prep.size_bytes);
        assert_eq!(prep.content_hash.len(), 64, "sha256 hex = 64 chars");
        assert!(prep.mtime_ms > 0, "mtime should be populated for a real file");
    }

    #[tokio::test]
    async fn a1_prepare_yields_same_hash_for_identical_content() {
        let mut a = tempfile::NamedTempFile::new().unwrap();
        let mut b = tempfile::NamedTempFile::new().unwrap();
        write!(a, "identical source").unwrap();
        write!(b, "identical source").unwrap();
        let ha = a1_prepare(a.path().to_path_buf()).await.unwrap();
        let hb = a1_prepare(b.path().to_path_buf()).await.unwrap();
        assert_eq!(ha.content_hash, hb.content_hash);
    }

    #[tokio::test]
    async fn a1_prepare_yields_different_hash_for_different_content() {
        let mut a = tempfile::NamedTempFile::new().unwrap();
        let mut b = tempfile::NamedTempFile::new().unwrap();
        write!(a, "version one").unwrap();
        write!(b, "version two").unwrap();
        let ha = a1_prepare(a.path().to_path_buf()).await.unwrap();
        let hb = a1_prepare(b.path().to_path_buf()).await.unwrap();
        assert_ne!(ha.content_hash, hb.content_hash);
    }

    #[tokio::test]
    async fn a1_prepare_fails_cleanly_when_path_does_not_exist() {
        let res = a1_prepare(PathBuf::from("/tmp/does/not/exist/axon-test")).await;
        assert!(res.is_err(), "missing path must surface an error to the worker pool");
    }

    #[test]
    fn sha256_hex_of_empty_string_is_canonical() {
        // SHA-256 of "" is the well-known constant
        // e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
