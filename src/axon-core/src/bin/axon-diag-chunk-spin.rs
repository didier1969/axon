//! REQ-AXO-901902/901903/901906 — CPU chunk-spin culprit finder.
//!
//! Diagnostic for the pipeline-A stall whose root cause re-measure (session
//! 2026-06-08) localised to a USERSPACE spin (wchan=0, 268% CPU) in the
//! chunker/tokenizer path on a dense aps3d file — NOT a deadlock, NOT the
//! claim-feeder (that was removed in the PIL-007 refactor and the stall
//! persisted at the same ~936-file mark).
//!
//! The production A2/A3 path already carries wall-clock budgets
//! (`parse_timeout_ms`, `chunk_budget_ms`) but `spawn_blocking` cannot be
//! cancelled, so a budget *timeout* still leaves the offending thread spinning
//! in the background while the pipeline appears to "drain other files". No
//! `WARN chunk budget exceeded` was observed, so the spin is upstream of the
//! DP budget check — most likely a single un-budgeted full-tokenizer encode
//! (`estimated_token_count` on a whole symbol, code_chunker.rs:477) or the
//! tree-sitter parse itself on a pathological file.
//!
//! This bin replays the EXACT producer-side CPU path (A1 guards → A2 parse →
//! A3 build_symbol_chunks + fuse_small_chunks) single-threaded over a source
//! tree, with:
//!   - the SAME A1 guards the real pipeline applies (size + minified), so a
//!     file the pipeline already skips is not falsely flagged ;
//!   - a WATCHDOG thread that names the in-flight file + step + elapsed every
//!     few seconds even when the main thread is wedged in a userspace spin ;
//!   - per-file timing with a SLOW threshold so the culprit is named the
//!     instant it is reached, before it wedges forever.
//!
//! No GPU, no DB, no operator gate. Run:
//!   cargo run --release --bin axon-diag-chunk-spin -- /home/dstadel/projects/aps3d
//!
//! Env overrides honoured (same as production): AXON_CHUNK_BUDGET_MS,
//! AXON_A_PARSE_TIMEOUT_MS, AXON_MAX_PARSE_BYTES, AXON_TARGET_CHUNK_TOKENS.
//! The diagnostic intentionally does NOT enforce the parse timeout (we WANT it
//! to hang on the culprit so the watchdog can name it).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use axon_core::bench_pipeline_stages::collect_supported_files;
use axon_core::code_chunker::{
    active_chunk_profile, build_file_chunks, fuse_small_chunks, TaggedChunk,
};
use axon_core::indexing_policy::{is_minified, max_line_bytes, max_parse_bytes};
use axon_core::parser::get_parser_for_file;
use axon_core::parser::phantom::phantom_extract;

/// Shared in-flight marker the watchdog reads. Updated (and lock released)
/// BEFORE each heavy step so the watchdog never blocks on the spinning thread.
struct InFlight {
    path: PathBuf,
    step: &'static str,
    started: Instant,
    file_index: usize,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let source = args
        .next()
        .unwrap_or_else(|| "/home/dstadel/projects/aps3d".to_string());
    let max_files: usize = args
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    // Files whose per-file wall-clock exceeds this are printed immediately.
    let slow_ms: u128 = std::env::var("DIAG_SLOW_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    // Watchdog alert threshold for a still-running file.
    let watchdog_alert_ms: u128 = std::env::var("DIAG_WATCHDOG_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4_000);

    let source_path = PathBuf::from(&source);
    eprintln!(
        "[diag] source={} max_files={} slow_ms={} watchdog_ms={}",
        source, max_files, slow_ms, watchdog_alert_ms
    );
    eprintln!(
        "[diag] guards: max_parse_bytes={} max_line_bytes={} target_chunk_tokens={}",
        max_parse_bytes(),
        max_line_bytes(),
        active_chunk_profile().target_chunk_tokens
    );

    let candidates = match collect_supported_files(&source_path, max_files) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[diag] FATAL collect_supported_files: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[diag] {} supported candidate files", candidates.len());

    let in_flight: &'static Mutex<Option<InFlight>> = Box::leak(Box::new(Mutex::new(None)));
    let done: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let last_alerted: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(usize::MAX)));

    // --- Watchdog thread: names the wedged file even if main spins. ---
    let watchdog = std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if done.load(Ordering::Relaxed) {
                break;
            }
            // Snapshot under a SHORT lock, then release before printing.
            let snap = {
                let guard = in_flight.lock().unwrap();
                guard.as_ref().map(|f| {
                    (
                        f.path.clone(),
                        f.step,
                        f.started.elapsed().as_millis(),
                        f.file_index,
                    )
                })
            };
            if let Some((path, step, elapsed, idx)) = snap {
                if elapsed >= watchdog_alert_ms {
                    // Avoid spamming: re-alert the same file at most ~every
                    // 1.5s tick, but always (it's the prime suspect).
                    last_alerted.store(idx, Ordering::Relaxed);
                    eprintln!(
                        "[diag][WATCHDOG] still in file #{idx} step={step} elapsed={}ms :: {}",
                        elapsed,
                        path.display()
                    );
                }
            }
        }
    });

    let profile = active_chunk_profile();
    let mut processed = 0usize;
    let mut skipped_size = 0usize;
    let mut skipped_minified = 0usize;
    let mut skipped_unreadable = 0usize;
    let mut total_symbols = 0usize;
    let mut total_chunks = 0usize;
    let run_start = Instant::now();

    for (idx, path) in candidates.iter().enumerate() {
        let file_start = Instant::now();

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                skipped_unreadable += 1;
                continue;
            }
        };

        // --- A1 guards (mirror stage_a1.rs) ---
        if content.len() as u64 > max_parse_bytes() {
            skipped_size += 1;
            continue;
        }
        if is_minified(&content, max_line_bytes()) {
            skipped_minified += 1;
            continue;
        }

        let parser = match get_parser_for_file(path) {
            Some(p) => p,
            None => continue,
        };

        // step: PARSE
        set_in_flight(in_flight, path, "parse", file_start, idx);
        let extraction = parser.parse(&content);
        let mut symbols = extraction.symbols;

        // step: PHANTOM (regex symbol/relation extraction — A2 runs this AFTER
        // tree-sitter, stage_a2.rs:72. phantom.rs:226 recomputes the line number
        // by re-slicing `content[..start].lines().count()` per distinct match →
        // O(matches × N) on a dense file: prime spin suspect that the production
        // A2 parse-timeout would flag as `A2 timeout` (NOT `chunk budget`).
        set_in_flight(in_flight, path, "phantom", file_start, idx);
        let (phantom_syms, _phantom_rels) = phantom_extract(path, &content, None);
        symbols.extend(phantom_syms);
        total_symbols += symbols.len();

        // step: BUILD — faithful PRODUCTION path (REQ-AXO-902024): build_file_chunks
        // splits lines once (fix B) and enforces the per-FILE budget (fix C), so a
        // file dense in matched ids no longer wedges here. (Was a per-symbol
        // build_symbol_chunks loop that re-split the file per symbol.)
        set_in_flight(in_flight, path, "build", file_start, idx);
        let sym_refs: Vec<&_> = symbols.iter().collect();
        let tagged: Vec<TaggedChunk> = build_file_chunks(&sym_refs, &content)
            .into_iter()
            .map(|(i, chunk)| TaggedChunk {
                symbol_id: format!("{}::{}", path.display(), symbols[i].name),
                symbol_name: symbols[i].name.clone(),
                chunk,
            })
            .collect();

        // step: FUSE (re-tokenizes combined groups — graph_ingestion.rs:987)
        set_in_flight(in_flight, path, "fuse", file_start, idx);
        let fused = fuse_small_chunks(tagged, profile.target_chunk_tokens);
        total_chunks += fused.len();

        processed += 1;
        let file_ms = file_start.elapsed().as_millis();
        if file_ms >= slow_ms {
            println!(
                "[diag][SLOW] {:>7}ms  symbols={:<5} chunks={:<5} size={:<9} :: {}",
                file_ms,
                symbols.len(),
                fused.len(),
                content.len(),
                path.display()
            );
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }

    done.store(true, Ordering::Relaxed);
    let _ = watchdog.join();

    eprintln!(
        "[diag] DONE in {}ms — processed={} symbols={} chunks={} | skipped: size={} minified={} unreadable={}",
        run_start.elapsed().as_millis(),
        processed,
        total_symbols,
        total_chunks,
        skipped_size,
        skipped_minified,
        skipped_unreadable
    );
}

fn set_in_flight(
    cell: &Mutex<Option<InFlight>>,
    path: &std::path::Path,
    step: &'static str,
    started: Instant,
    file_index: usize,
) {
    let mut guard = cell.lock().unwrap();
    *guard = Some(InFlight {
        path: path.to_path_buf(),
        step,
        started,
        file_index,
    });
    // guard drops here — heavy work runs WITHOUT holding the lock.
}
