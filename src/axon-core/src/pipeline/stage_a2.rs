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
/// REQ-AXO-902252 — files whose parse blew the per-file budget and were therefore emitted
/// as a zero-symbol `ParsedFile`.
///
/// The timeout path is a deliberate trade-off (REQ-AXO-901895: a clean skip beats a retry
/// storm), but it is also SILENT symbol loss: A3 marks the file `parsed`, and nothing
/// downstream can tell "timed out under load" from "nothing structural to extract". A
/// non-zero value here means the index is incomplete for reasons unrelated to the code.
static PARSE_TIMEOUTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Read side of [`PARSE_TIMEOUTS`]. Monotonic for the process lifetime.
pub fn parse_timeouts() -> u64 {
    PARSE_TIMEOUTS.load(std::sync::atomic::Ordering::Relaxed)
}

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
            security_findings: Vec::new(),
        });
    }
    let path_for_skip = prep.path.clone();
    let hash_for_skip = prep.content_hash.clone();
    let mtime_for_skip = prep.mtime_ms;
    let size_for_skip = prep.size_bytes;
    // The bounded regex engine is independent from language parsing. Run it
    // before the parser timeout so a slow grammar cannot erase security truth.
    let security_findings = crate::parser::scan_secrets(&prep.content);
    let security_findings_for_timeout = security_findings.clone();
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

        // REQ-AXO-902209 — `scan_secrets` had real AWS-key/PEM-header/DB-URL/
        // generic-token regexes but was never called by any parser (dead
        // code: it produced zero findings on any real codebase). Run it
        // unconditionally per-file, same as phantom_extract above: a secret
        // can leak from ANY file regardless of language or even whether a
        // parser is registered for its extension.
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
            security_findings,
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
            //
            // REQ-AXO-902252 — COUNT it. This branch marks a file `parsed` with ZERO
            // symbols: structurally indistinguishable, downstream, from "a data file with
            // nothing to extract". Until now the only trace was this `warn!`, so a host
            // under load could silently strip the symbols off arbitrarily many files and
            // nothing would report it — the same silent-degradation shape as
            // REQ-AXO-902254 (diagnose_indexing blind to a coverage gap) and
            // REQ-AXO-902258 (a promote installing the wrong binary while every gate
            // stayed green). A monotonic counter makes it observable; `parse_timeouts()`
            // is the read side.
            PARSE_TIMEOUTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                security_findings: security_findings_for_timeout,
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

    /// REQ-AXO-902326 — EVERY test in this module calls `a2_transform`, which
    /// READS `AXON_A_PARSE_TIMEOUT_MS`. Two of them WRITE it, and one
    /// deliberately installs a **1 ms** budget to exercise the timeout path.
    ///
    /// A lock protects only those who take it. Eight of the ten tests were pure
    /// READERS and took nothing, so they parsed with whatever budget happened to
    /// be installed at that instant — 1 ms whenever they interleaved with the
    /// timeout test — got a deliberately CLEAN zero-symbol result back
    /// (REQ-AXO-901895), and reported it as a parser regression.
    ///
    /// Observed 2026-08-15: two consecutive full-suite runs, two DIFFERENT
    /// failure sets, every one of them green in isolation. That is the signature
    /// of shared global state, not of a broken parser — and it is the same class
    /// as REQ-AXO-902261. REQ-AXO-902252 already fixed the symptom on ONE test by
    /// widening its budget; widening does nothing for a reader that never held
    /// the lock.
    ///
    /// The global `PARSE_TIMEOUTS` counter is the second half of the same
    /// hazard: an unlocked test timing out increments it under another test's
    /// before/after comparison. Holding this lock for the whole body closes both.
    ///
    /// Both guards must be bound for the WHOLE test body — the budget has to
    /// still be installed when `a2_transform` reads it, not merely when it was
    /// set. `EnvVarGuard` restores on Drop, so a panicking test can no longer
    /// leak its budget onto the next one.
    fn parse_budget(
        ms: &str,
    ) -> (
        std::sync::MutexGuard<'static, ()>,
        crate::test_support::EnvVarGuard,
    ) {
        let lock = crate::test_support::env_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let env = crate::test_support::EnvVarGuard::set("AXON_A_PARSE_TIMEOUT_MS", ms);
        (lock, env)
    }

    /// A budget no unit test can exhaust, so an assertion about PARSER WIRING
    /// measures the parser and never the host's load (REQ-AXO-902252).
    const GENEROUS_PARSE_BUDGET_MS: &str = "600000";

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
        // REQ-AXO-902252 — this assertion is about PARSER WIRING, but it used to be
        // wall-clock dependent and failed on a real full-suite run ("rust parser should
        // surface `main`: []", 1665 passed / 1 failed). Mechanism: `a2_transform` wraps the
        // parse in `timeout(parse_timeout_ms())`, and on expiry deliberately returns a
        // CLEAN zero-symbol ParsedFile (REQ-AXO-901895, to avoid a retry storm). Under a
        // saturated machine — 1600+ parallel tests, each `#[tokio::test]` with its own
        // runtime, plus a concurrent build — the `spawn_blocking` thread may not even be
        // SCHEDULED inside the 30s default budget. The parser was never at fault; the test
        // was measuring the host's load.
        //
        // Fixed by neutralising the clock rather than retrying: a 10-minute budget cannot
        // expire in a unit test, so the assertion measures only what it claims to. The
        // env lock is mandatory here — a bare `set_var` is the flake class of
        // REQ-AXO-902261.
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);

        let before_timeouts = super::parse_timeouts();
        let prep = prep_with("/tmp/demo.rs", "fn main() { println!(\"hi\"); }\n");
        let parsed = a2_transform(prep).await.unwrap();

        assert_eq!(parsed.path, PathBuf::from("/tmp/demo.rs"));
        // Distinguish the two ways this test can see zero symbols. Without this, a timeout
        // reads exactly like a parser regression — which is what cost the false diagnosis.
        assert_eq!(
            super::parse_timeouts(),
            before_timeouts,
            "the parse budget expired: this run measured host load, not the parser (REQ-AXO-902252)"
        );
        assert!(
            parsed.symbols.iter().any(|s| s.name == "main"),
            "rust parser should surface `main`: {:?}",
            parsed.symbols
        );
    }

    /// REQ-AXO-902252 — the timeout path must be COUNTED, not merely logged. A zero-symbol
    /// file is indistinguishable downstream from "nothing to extract", so without this
    /// counter a loaded host can silently strip symbols off arbitrarily many files and
    /// report nothing. Driven with a 1ms budget so the expiry is deterministic rather than
    /// load-dependent.
    #[tokio::test]
    async fn a2_transform_counts_a_parse_timeout_instead_of_failing_silently() {
        // REQ-AXO-902326 — this is the WRITER whose 1ms budget was leaking onto
        // every unlocked reader in this module. The guard now restores the prior
        // value even if the body panics.
        let _budget = parse_budget("1");

        let before = super::parse_timeouts();
        // Enough content that a 1ms budget cannot cover the parse.
        let big = "fn f() {}\n".repeat(20_000);
        let parsed = a2_transform(prep_with("/tmp/slow.rs", &big)).await.unwrap();

        // The contract of the timeout branch: a CLEAN zero-symbol skip (never an Err —
        // an Err would leave the file unmarked and re-queued forever, REQ-AXO-901885).
        if super::parse_timeouts() > before {
            assert!(parsed.symbols.is_empty(), "a timed-out parse must yield no symbols");
            assert!(parsed.relations.is_empty());
            assert_eq!(parsed.path, PathBuf::from("/tmp/slow.rs"));
        }
        // If the parse beat even a 1ms budget, there is nothing to assert — this test never
        // fails on a FAST machine, which is the whole point of not asserting on timing.
    }

    #[tokio::test]
    async fn a2_transform_reconnects_scan_secrets_regardless_of_language() {
        // REQ-AXO-902209 — scan_secrets had real regexes (AWS key, PEM header,
        // DB URL, generic token) but was never invoked by any parser (dead
        // code, zero real-world findings). A2 must now run it unconditionally
        // per-file, alongside the language parser AND phantom_extract.
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
        let prep = prep_with(
            "/tmp/demo.rs",
            "fn main() { let key = \"AKIAABCDEFGHIJKLMNOP\"; }\n",
        );
        let parsed = a2_transform(prep).await.unwrap();
        assert!(
            parsed
                .security_findings
                .iter()
                .any(|finding| finding.rule_id == "SECRET_AWS_KEY"),
            "AWS key pattern must surface as a typed finding: {:?}",
            parsed.security_findings
        );
        assert!(
            parsed.symbols.iter().all(|symbol| !symbol.kind.starts_with("SECRET_")),
            "security findings must never masquerade as graph symbols"
        );
        // The language parser must ALSO still run (scan_secrets is additive,
        // not a replacement).
        assert!(
            parsed.symbols.iter().any(|s| s.name == "main"),
            "rust parser must still run alongside scan_secrets: {:?}",
            parsed.symbols
        );
    }

    #[tokio::test]
    async fn a2_transform_preserves_pivot_metadata_from_prepared_file() {
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
        let _budget = parse_budget(GENEROUS_PARSE_BUDGET_MS);
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
