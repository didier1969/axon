//! Stage A2 — Transformation worker (CPT-AXO-054).
//!
//! Consumes a [`PreparedFile`] (output of A1), dispatches to the canonical
//! tree-sitter parser for the file's language (`parser::get_parser_for_file`),
//! and emits a [`ParsedFile`] carrying the extracted symbols + relations.
//!
//! Parsing is CPU-bound, so we wrap the parser invocation in
//! [`tokio::task::spawn_blocking`] to avoid stalling the tokio runtime when
//! large files arrive. This matches the per-stage worker pool sizing
//! (`AXON_A2_WORKERS` default 8 live, 4 dev) — the blocking pool is what
//! actually parallelises across cores; the `tokio::spawn` in
//! `spawn_stage_workers` just steers items off the channel.

use anyhow::{Context, Result};
use tracing::info;

use super::types::{ParsedFile, PreparedFile};

/// Parse `prep` into a [`ParsedFile`] using the language-appropriate parser.
///
/// Returns an error if no parser exists for the file's extension OR if the
/// blocking task itself panicked. A file with zero symbols is a valid result
/// (e.g. a file containing only comments) — it returns `Ok(ParsedFile { symbols: vec![], ... })`.
pub async fn a2_transform(prep: PreparedFile) -> Result<ParsedFile> {
    // REQ-AXO-345 — A2 in/out trace.
    info!(target: "pipeline_v2::a2", "A2 in: {}", prep.path.display());
    let path_for_log = prep.path.clone();
    // REQ-AXO-347 — defensive empty-file fast-path. Some language
    // parsers (Elixir, Python with eager AST walks, etc.) error or
    // panic on empty input, and the worker pool propagates the stage
    // error to the orchestrator's `errors_total`. Empty content has
    // zero symbols by definition, so short-circuit before parser
    // dispatch : no parser lookup, no spawn_blocking, no risk of a
    // language-specific edge case. Also covers the corner where the
    // file extension has no parser registered (today returns an
    // error) but the file is empty anyway — no useful work was lost.
    if prep.content.is_empty() {
        info!(
            target: "pipeline_v2::a2",
            "A2 out: {} symbols=0 relations=0 (empty-file fast-path)",
            path_for_log.display()
        );
        return Ok(ParsedFile {
            path: prep.path,
            content: prep.content,
            content_hash: prep.content_hash,
            mtime_ms: prep.mtime_ms,
            size_bytes: prep.size_bytes,
            symbols: Vec::new(),
            relations: Vec::new(),
        });
    }
    let result = tokio::task::spawn_blocking(move || {
        let parser = crate::parser::get_parser_for_file(&prep.path).ok_or_else(|| {
            anyhow::anyhow!("A2: no parser registered for {}", prep.path.display())
        })?;
        let extraction = parser.parse(&prep.content);
        Ok(ParsedFile {
            path: prep.path,
            content: prep.content,
            content_hash: prep.content_hash,
            mtime_ms: prep.mtime_ms,
            size_bytes: prep.size_bytes,
            symbols: extraction.symbols,
            relations: extraction.relations,
        })
    })
    .await
    .context("A2 parse task panicked or was cancelled")?;
    if let Ok(ref parsed) = result {
        info!(
            target: "pipeline_v2::a2",
            "A2 out: {} symbols={} relations={}",
            path_for_log.display(),
            parsed.symbols.len(),
            parsed.relations.len()
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prep_with(path: &str, content: &str) -> PreparedFile {
        PreparedFile {
            path: PathBuf::from(path),
            content: content.to_string(),
            content_hash: "deadbeef".to_string(),
            mtime_ms: 1_700_000_000_000,
            size_bytes: content.len() as u64,
        }
    }

    #[tokio::test]
    async fn a2_transform_extracts_at_least_one_symbol_from_a_minimal_rust_file() {
        let prep = prep_with("/tmp/demo.rs", "fn main() { println!(\"hi\"); }\n");
        let parsed = a2_transform(prep).await.unwrap();
        assert_eq!(parsed.path, PathBuf::from("/tmp/demo.rs"));
        assert!(
            parsed.symbols.iter().any(|s| s.name == "main"),
            "rust parser should surface `main`: {:?}",
            parsed.symbols
        );
    }

    #[tokio::test]
    async fn a2_transform_preserves_pivot_metadata_from_prepared_file() {
        let prep = prep_with("/tmp/demo.rs", "fn one() {}\nfn two() {}\n");
        let parsed = a2_transform(prep).await.unwrap();
        assert_eq!(parsed.content_hash, "deadbeef");
        assert_eq!(parsed.mtime_ms, 1_700_000_000_000);
        assert_eq!(parsed.size_bytes, "fn one() {}\nfn two() {}\n".len() as u64);
        assert!(!parsed.content.is_empty(), "content forwarded for A3 chunking");
    }

    #[tokio::test]
    async fn a2_transform_errors_when_extension_has_no_parser() {
        let prep = prep_with("/tmp/file.unknownext", "anything goes");
        let res = a2_transform(prep).await;
        assert!(res.is_err(), "unsupported extension must surface an error");
        let msg = format!("{:#}", res.unwrap_err());
        assert!(
            msg.contains("no parser"),
            "error message should reference the missing parser: {msg}"
        );
    }

    #[tokio::test]
    async fn a2_transform_handles_empty_file_without_panicking() {
        let prep = prep_with("/tmp/empty.rs", "");
        let parsed = a2_transform(prep).await.unwrap();
        // No symbols expected from an empty file — but the call must succeed.
        assert!(parsed.symbols.iter().all(|s| !s.name.is_empty()));
    }

    /// REQ-AXO-347 — empty-file fast-path returns successfully even when
    /// the file extension has no registered parser. Pre-fix this branch
    /// surfaced an `A2: no parser registered for …` error to the worker
    /// pool ; the fast-path now short-circuits before parser dispatch.
    #[tokio::test]
    async fn a2_transform_empty_file_with_unknown_extension_yields_zero_symbols() {
        let prep = prep_with("/tmp/empty.unknown_ext_xyzzy", "");
        let parsed = a2_transform(prep).await.unwrap();
        assert!(parsed.symbols.is_empty());
        assert!(parsed.relations.is_empty());
        assert_eq!(parsed.content, "");
        assert_eq!(parsed.size_bytes, 0);
    }

    /// REQ-AXO-347 — empty file with a known extension returns the same
    /// fast-path shape (no parser invocation, no symbols, no relations).
    /// Locks the invariant for parsers that might evolve later (Elixir,
    /// Python, TS) to ensure they never see empty input.
    #[tokio::test]
    async fn a2_transform_empty_rust_file_uses_fast_path() {
        let prep = prep_with("/tmp/empty.rs", "");
        let parsed = a2_transform(prep).await.unwrap();
        assert!(parsed.symbols.is_empty());
        assert!(parsed.relations.is_empty());
        assert_eq!(parsed.content_hash, "deadbeef");
    }
}
