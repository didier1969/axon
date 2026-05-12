//! Pivot types that flow through the v2 pipeline stages (CPT-AXO-054).
//!
//! Each stage emits the type the next stage consumes:
//!
//! * Watcher (out): `WatchedPath`
//! * A1 Preparation (in: `WatchedPath`, out: [`PreparedFile`])
//! * A2 Transformation (in: [`PreparedFile`], out: `ParsedFile`)
//! * A3 Enregistrement (in: `ParsedFile`, out: success ack + best-effort
//!   try_send to B1 of chunk identifiers)
//!
//! Slice S3 of REQ-AXO-289 lands [`PreparedFile`]. The other types are
//! introduced in subsequent slices to keep each change reviewable.

use std::path::PathBuf;

/// Output of stage A1 — Preparation.
///
/// Carries everything the parser stage (A2) needs to extract symbols, edges
/// and chunks, without re-reading the file from disk. The `content_hash`
/// field is the key the [`crate::pipeline_v2::IndexedFileCache`] uses to
/// detect "already seen" duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFile {
    /// Absolute filesystem path of the file that was read.
    pub path: PathBuf,
    /// Full source text.
    pub content: String,
    /// SHA-256 hex digest of `content` — stable cross-restart so the
    /// `IndexedFile` cache reload reproduces identical filter behaviour.
    pub content_hash: String,
    /// Last-modified time in milliseconds since the Unix epoch (best-effort —
    /// 0 when the filesystem refuses to surface it).
    pub mtime_ms: i64,
    /// Size in bytes of the file as read.
    pub size_bytes: u64,
}
