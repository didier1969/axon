//! Stage A1 — Preparation worker (CPT-AXO-054).
//!
//! Reads a file from disk, computes its SHA-256 content hash, and packages
//! everything into a [`PreparedFile`] for the next stage. This is the
//! I/O-bound stage of pipeline A; throughput scales with the number of
//! workers (default 4 live, 2 dev) and underlying disk parallelism.
//!
//! The hashing choice is SHA-256 because the same hash is later persisted to
//! `ist.IndexedFile.content_hash` and read back at indexer startup to
//! rebuild the in-RAM filter cache. SHA-256 is cross-restart stable, while
//! Rust's `DefaultHasher` is not.

use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tracing::info;

use super::types::PreparedFile;

/// Extract `(mtime_ms, size_bytes)` from filesystem metadata — the level-1
/// change-detection key shared by A1 and the orchestrator's pre-read filter
/// (PIL-AXO-007 CP2b). Single source of truth so both compute mtime identically.
pub fn mtime_size_ms(metadata: &std::fs::Metadata) -> (i64, u64) {
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as i64)
        .unwrap_or(0);
    (mtime_ms, metadata.len())
}

/// Read `path`, compute its content hash, and return the prepared payload.
///
/// This is the closure handed to [`super::spawn_stage_workers`] for stage A1.
/// Errors are surfaced verbatim so the worker pool can record them in
/// `StageMetrics::errors_total` without crashing the pipeline.
pub async fn a1_prepare(path: PathBuf) -> Result<PreparedFile> {
    // REQ-AXO-901919 — register as in-flight for the whole stage so the
    // watchdog can name this file if A1 (metadata/read) ever stalls. Drops on
    // return OR cancellation.
    let _in_flight = super::in_flight::InFlightRegistry::global()
        .enter("A1", path.to_string_lossy().into_owned());
    // REQ-AXO-345 — A1 in/out trace for silent-drop hunt.
    info!(target: "pipeline_v2::a1", "A1 in: {}", path.display());

    // Metadata FIRST so the size guard fires before reading the file into RAM.
    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("A1 metadata failed for {}", path.display()))?;

    let (mtime_ms, size_bytes) = mtime_size_ms(&metadata);

    // REQ-AXO-901895 Memory Shield (restored) — oversized files are skipped
    // BEFORE the read (the legacy 5 MB shield's "before read attempts"). Emit
    // empty content so A2's empty fast-path yields zero symbols and A3 writes
    // the 'parsed' marker → the file is never re-claimed and never wedges a
    // worker. Change detection on rescan is mtime+size (stored at discovery),
    // so a later shrink/edit re-evaluates it.
    if size_bytes > crate::indexing_policy::max_parse_bytes() {
        info!(
            target: "pipeline_v2::a1",
            "A1 skip: {} reason=oversized size={}",
            path.display(),
            size_bytes
        );
        return Ok(skipped_prepared(path, size_bytes, mtime_ms, "oversized"));
    }

    // Read as UTF-8 text. Binary content surfaces as `InvalidData` — skip it
    // cleanly (restores the legacy `:skipped_binary`) instead of bubbling an
    // error that churns the file through retries to the dead-letter cap.
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
            info!(
                target: "pipeline_v2::a1",
                "A1 skip: {} reason=binary size={}",
                path.display(),
                size_bytes
            );
            return Ok(skipped_prepared(path, size_bytes, mtime_ms, "binary"));
        }
        Err(err) => {
            return Err(err).with_context(|| format!("A1 read failed for {}", path.display()));
        }
    };

    // REQ-AXO-901895 — minified/generated guard: a single over-long physical
    // line (bundled JS, source maps, one-line JSON) builds a pathological
    // tree-sitter AST and is char-windowed for ~zero retrieval value. Skip with
    // the real content hash (edit re-evaluates) but empty content so A2
    // fast-paths to zero symbols.
    if crate::indexing_policy::is_minified(&content, crate::indexing_policy::max_line_bytes()) {
        let content_hash = sha256_hex(&content);
        info!(
            target: "pipeline_v2::a1",
            "A1 skip: {} reason=minified size={}",
            path.display(),
            size_bytes
        );
        return Ok(PreparedFile {
            path,
            content: String::new(),
            content_hash,
            mtime_ms,
            size_bytes,
        });
    }

    // REQ-AXO-901920 — generated code-extension files (protobuf/gRPC stubs,
    // framework codegen, minified bundles, source maps, lockfiles) carry a
    // parser-claimed extension but ~nil hand-authored value; parsing + chunking
    // them is wasted CPU + embedding noise, and a large generated file is prime
    // tree-sitter-spin / spawn_blocking-orphan fuel (REQ-AXO-901918). Skip with
    // the real content hash (edit re-evaluates) and empty content → A2 zero-symbol.
    if crate::indexing_policy::is_generated_code_file(&path) {
        let content_hash = sha256_hex(&content);
        info!(
            target: "pipeline_v2::a1",
            "A1 skip: {} reason=generated size={}",
            path.display(),
            size_bytes
        );
        return Ok(PreparedFile {
            path,
            content: String::new(),
            content_hash,
            mtime_ms,
            size_bytes,
        });
    }

    let content_hash = sha256_hex(&content);
    info!(
        target: "pipeline_v2::a1",
        "A1 out: {} size={}",
        path.display(),
        size_bytes
    );
    // REQ-AXO-901906 — memory is bounded by the (small) A-content channel caps +
    // send().await backpressure (mirrors pipeline B); no per-file budget guard.
    Ok(PreparedFile {
        path,
        content,
        content_hash,
        mtime_ms,
        size_bytes,
    })
}

/// REQ-AXO-901895 — a deliberately-skipped file (oversized/binary) we did NOT
/// read. Emits empty content (→ A2 zero-symbol fast-path → A3 'parsed' marker)
/// with a hash keyed on size+mtime so a later edit changing either re-evaluates
/// the file instead of being deduped as "already seen".
fn skipped_prepared(path: PathBuf, size_bytes: u64, mtime_ms: i64, reason: &str) -> PreparedFile {
    let content_hash = sha256_hex(&format!("__axon_skip_{reason}__:{size_bytes}:{mtime_ms}"));
    PreparedFile {
        path,
        content: String::new(),
        content_hash,
        mtime_ms,
        size_bytes,
    }
}

/// Helper — hex-encoded SHA-256 of an arbitrary byte slice.
///
/// Public-in-module so subsequent stages (A2 chunk hashes, B1 chunk lookup
/// keys) can reuse the same digest function without re-implementing it.
pub(crate) fn sha256_hex(content: &str) -> String {
    use std::fmt::Write;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{:02x}", byte);
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

    /// REQ-AXO-901895 Memory Shield — an oversized file (> 5 MB default) is
    /// skipped with empty content (A2 fast-path → A3 'parsed' marker), so it
    /// never reaches the parser/chunker and cannot wedge a worker.
    #[tokio::test]
    async fn a1_prepare_skips_oversized_file_with_empty_content() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        // 5.6 MB of short lines → over the 5 MB default, not minified ; the
        // size guard fires before the read either way.
        file.write_all("x\n".repeat(2_800_000).as_bytes()).unwrap();
        file.flush().unwrap();
        let prep = a1_prepare(file.path().to_path_buf()).await.unwrap();
        assert!(prep.content.is_empty(), "oversized file must yield empty content");
        assert!(prep.size_bytes > 5 * 1024 * 1024);
        assert_eq!(prep.content_hash.len(), 64);
    }

    /// REQ-AXO-901895 — a minified/generated file (single physical line over the
    /// 8 KB default) is skipped with empty content but a real content hash (so a
    /// later edit re-evaluates it).
    #[tokio::test]
    async fn a1_prepare_skips_minified_single_line_with_empty_content() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all("x".repeat(9000).as_bytes()).unwrap(); // one 9 KB line > 8 KB
        file.flush().unwrap();
        let prep = a1_prepare(file.path().to_path_buf()).await.unwrap();
        assert!(prep.content.is_empty(), "minified file must yield empty content");
        assert_eq!(prep.content_hash.len(), 64);
        assert_eq!(prep.size_bytes, 9000);
    }

    /// REQ-AXO-901895 — binary content (non-UTF8) is skipped cleanly (restores
    /// the legacy `:skipped_binary`) instead of bubbling a read error that
    /// churns the file through retries.
    #[tokio::test]
    async fn a1_prepare_skips_binary_file_with_empty_content() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&[0xFFu8, 0xFE, 0x00, 0x9F, 0x01, 0x02]).unwrap(); // invalid UTF-8
        file.flush().unwrap();
        let prep = a1_prepare(file.path().to_path_buf()).await.unwrap();
        assert!(prep.content.is_empty(), "binary file must yield empty content");
        assert_eq!(prep.content_hash.len(), 64);
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
