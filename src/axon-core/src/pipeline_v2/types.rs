//! Pivot types that flow through the v2 pipeline stages (CPT-AXO-054).
//!
//! Each stage emits the type the next stage consumes:
//!
//! * Watcher (out): `WatchedPath`
//! * A1 Preparation (in: `WatchedPath`, out: [`PreparedFile`])
//! * A2 Transformation (in: [`PreparedFile`], out: [`ParsedFile`])
//! * A3 Enregistrement (in: [`ParsedFile`], out: success ack + best-effort
//!   try_send to B1 of chunk identifiers)
//!
//! Slice S3a + S3b of REQ-AXO-289 land [`PreparedFile`] + [`ParsedFile`]. The
//! A3 stage is introduced in subsequent slices to keep each change
//! reviewable.

use std::path::PathBuf;

use crate::parser::{Relation, Symbol};

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

/// Output of stage A2 — Transformation (tree-sitter parse).
///
/// Carries the symbols, relations and pivot metadata the A3 stage needs to
/// UPSERT the graph layer (Symbol + AGE relations) and to record
/// `IndexedFile(path, content_hash, last_seen_ms)`. Chunk extraction lives
/// in the persistence path inside A3 so it shares the same transaction as
/// the symbol writes.
#[derive(Debug, Clone)]
pub struct ParsedFile {
    pub path: PathBuf,
    /// Original source text preserved for chunk-extraction inside A3.
    pub content: String,
    pub content_hash: String,
    pub mtime_ms: i64,
    pub size_bytes: u64,
    /// Symbols extracted by the tree-sitter parser dispatch for this file's
    /// extension.
    pub symbols: Vec<Symbol>,
    /// Relations (CALLS / CALLS_NIF / CONTAINS / etc.) extracted alongside
    /// symbols.
    pub relations: Vec<Relation>,
}
