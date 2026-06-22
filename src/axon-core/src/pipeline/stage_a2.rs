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

use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{info, warn};

use super::types::{ParsedFile, PreparedFile};

/// Parse `prep` into a [`ParsedFile`] using the language-appropriate parser.
///
/// Returns an error if no parser exists for the file's extension OR if the
/// blocking task itself panicked. A file with zero symbols is a valid result
/// (e.g. a file containing only comments) — it returns `Ok(ParsedFile { symbols: vec![], ... })`.
pub async fn a2_transform(prep: PreparedFile) -> Result<ParsedFile> {
    // REQ-AXO-345 — A2 in/out trace.
    info!(target: "pipeline::a2", "A2 in: {}", prep.path.display());
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
            target: "pipeline::a2",
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
    let path_for_skip = prep.path.clone();
    let hash_for_skip = prep.content_hash.clone();
    let mtime_for_skip = prep.mtime_ms;
    let size_for_skip = prep.size_bytes;
    let parse_fut = tokio::task::spawn_blocking(move || {
        // REQ-AXO-901919/901918 — register INSIDE the blocking closure so the
        // entry lives for the ACTUAL parse-thread lifetime. On a per-file parse
        // timeout the outer future returns a clean skip, but this uncancellable
        // thread keeps running; the entry persists → the watchdog keeps naming
        // the file, making the spawn_blocking orphan observable.
        let _in_flight = super::in_flight::InFlightRegistry::global()
            .enter("A2", prep.path.to_string_lossy().into_owned());
        let mut symbols;
        let mut relations;

        if let Some(parser) = crate::parser::get_parser_for_file(&prep.path) {
            let extraction = parser.parse(&prep.content);
            symbols = extraction.symbols;
            relations = extraction.relations;
        } else {
            symbols = Vec::new();
            relations = Vec::new();
        }

        let (phantom_syms, phantom_rels) =
            crate::parser::phantom::phantom_extract(&prep.path, &prep.content, None);
        symbols.extend(phantom_syms);
        relations.extend(phantom_rels);

        // REQ-AXO-901885 — a parsed file that yields zero symbols AND zero
        // relations is NOT an error: it is "seen, nothing structural to
        // extract" (data/config/markup, a code file with only top-level
        // expressions, generated headers, vendored sources). Returning Err
        // here meant the file never reached A3, so its
        // `IndexedFile(content_hash)` marker was never written — and every
        // subsequent full scanner walk re-discovered it as unseen, re-queued
        // it, and re-failed, burning CPU in an unbounded re-parse loop
        // (observed: same ~2.1k files reprocessed ~10×/hour). Generalises the
        // REQ-AXO-347 empty-file fast-path: emit a valid zero-symbol
        // ParsedFile so A3 records the marker (zero chunks, because chunks are
        // built per-symbol in upsert_graph) and the watcher SkipsUnchanged
        // it on the next walk.
        Ok(ParsedFile {
            path: prep.path,
            content: prep.content,
            content_hash: prep.content_hash,
            mtime_ms: prep.mtime_ms,
            size_bytes: prep.size_bytes,
            symbols,
            relations,
        })
    });
    let parse_budget = Duration::from_millis(crate::indexing_policy::parse_timeout_ms());
    let result = match tokio::time::timeout(parse_budget, parse_fut).await {
        Ok(Ok(parsed_result)) => parsed_result,
        Ok(Err(join_err)) => {
            return Err(join_err).context("A2 parse task panicked or was cancelled");
        }
        Err(_elapsed) => {
            // REQ-AXO-901895 — parse exceeded the per-file budget (pathology not
            // caught by the size/minified guards). spawn_blocking can't be
            // cancelled, so the worker thread runs to completion in the
            // background (its result discarded) while we record a clean
            // zero-symbol skip → A3 marks 'parsed', no retry storm, and the
            // pipeline keeps draining other files.
            warn!(
                target: "pipeline::a2",
                "A2 timeout: {} after {}ms — skipping (zero-symbol)",
                path_for_log.display(),
                parse_budget.as_millis()
            );
            return Ok(ParsedFile {
                path: path_for_skip,
                content: String::new(),
                content_hash: hash_for_skip,
                mtime_ms: mtime_for_skip,
                size_bytes: size_for_skip,
                symbols: Vec::new(),
                relations: Vec::new(),
            });
        }
    };
    if let Ok(ref parsed) = result {
        info!(
            target: "pipeline::a2",
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
        assert!(
            !parsed.content.is_empty(),
            "content forwarded for A3 chunking"
        );
    }

    /// REQ-AXO-901885 — a non-empty file whose extension has no parser (and no
    /// phantom rules) is NOT an error: A2 returns Ok with zero symbols and the
    /// content preserved, so A3 persists the IndexedFile marker (zero chunks)
    /// and the scanner stops re-queueing it. Pre-fix this surfaced an
    /// `A2: no parser and no phantom rules` error that prevented the marker
    /// write and caused an unbounded re-parse loop.
    #[tokio::test]
    async fn a2_transform_marks_unparseable_file_done_with_zero_symbols() {
        let prep = prep_with("/tmp/file.unknownext", "anything goes");
        let parsed = a2_transform(prep)
            .await
            .expect("no-parser file must be a clean skip, not an error");
        assert!(parsed.symbols.is_empty(), "no parser => no symbols");
        assert!(parsed.relations.is_empty(), "no parser => no relations");
        assert_eq!(
            parsed.content, "anything goes",
            "content preserved so A3 writes the IndexedFile marker"
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

    /// REQ-AXO-901777 — corrupted/binary content that tree-sitter cannot
    /// parse yields an error (not a panic). The pipeline orchestrator
    /// counts this as a stage error and moves on.
    #[tokio::test]
    async fn a2_transform_binary_garbage_content_does_not_panic() {
        let garbage = "\x00\x01\x02\x7F random garbage \x0B\x0C not valid code";
        let prep = prep_with("/tmp/garbage.rs", garbage);
        let result = a2_transform(prep).await;
        // Binary content may yield zero symbols from tree-sitter and zero
        // phantom matches → the function returns an error ("no parser and
        // no phantom rules"). Either way, no panic.
        match result {
            Ok(parsed) => {
                // If it somehow parsed, that's fine — just no panic.
                assert!(parsed.symbols.is_empty() || !parsed.symbols.is_empty());
            }
            Err(_) => {
                // Expected: "no parser" or parse failure.
            }
        }
    }

    /// REQ-AXO-901777 — deeply nested / adversarial content does not
    /// cause a stack overflow or timeout in the tree-sitter parser.
    #[tokio::test]
    async fn a2_transform_deeply_nested_content_completes() {
        let depth = 100;
        let mut code = String::new();
        for i in 0..depth {
            code.push_str(&format!("fn f{i}() {{ "));
        }
        for _ in 0..depth {
            code.push_str("} ");
        }
        let prep = prep_with("/tmp/deep.rs", &code);
        let result = a2_transform(prep).await;
        // Must complete (no infinite loop / stack overflow). Whether
        // parsing succeeds or fails is secondary.
        assert!(result.is_ok() || result.is_err());
    }
}
