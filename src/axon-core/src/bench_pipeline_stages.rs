//! REQ-AXO-259/260/261 — Pipeline-stage throughput benches (4-bench
//! diagnostic framework, operator directive 2026-05-10 session 12).
//!
//! Companion to REQ-AXO-257 (`embedder.rs::run_embedder_sustained_bench`
//! + `run_embedder_pipeline_bench` — Bench 2 GPU-only).
//!
//! - **Bench 1** — `run_graph_projection_bench` : watcher → parse →
//!   symbol extract → chunk derive. With/without DB insert (insert path
//!   is opt-in to keep the harness usable cold).
//! - **Bench 3** — `run_writer_bench` : pre-built synthetic chunks +
//!   embeddings → DB persist (parquet/pgvector/AGE). Isolates the
//!   writer ceiling.
//! - **Bench 4** — `run_end_to_end_bench` : full indexer-full
//!   measurement (typically driven from a shell wrapper, but exposes
//!   the metric-collection helpers programmatically).
//!
//! All four benches share `compute_window_summary` (extracted from
//! REQ-AXO-257 rolling-window code, GUI-PRO-013 DRY) and
//! `vram_peak_mib` telemetry (operator 2026-05-10: VRAM margin matters
//! to avoid OOM-induced production stall).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::code_chunker::build_symbol_chunks;
use crate::parser::get_parser_for_file;

/// REQ-AXO-259 — Result of a graph-projection bench run. Captures
/// the producer-side ceiling: how fast the watcher → parse → chunk
/// path can produce embeddable units.
#[derive(Debug, Clone)]
pub struct GraphProjectionBench {
    pub label: String,
    pub source_dir: PathBuf,
    pub files_processed: usize,
    pub files_skipped_unsupported: usize,
    pub symbols_extracted: usize,
    pub chunks_derived: usize,
    pub elapsed_ms: u64,
    pub files_per_s: f64,
    pub symbols_per_s: f64,
    pub chunks_per_s: f64,
}

/// REQ-AXO-259 — Bench 1 entry point. Walks `source_dir` (depth-first,
/// limited to `max_files` supported-ecosystem files), parses each via
/// `parser::get_parser_for_file`, derives chunks via
/// `code_chunker::build_symbol_chunks`. **No DB inserts** — pure
/// producer-side measurement.
///
/// Returns aggregate throughput. Per-file timings are not surfaced
/// (operator directive: ceiling vs interface diagnosis, not per-file
/// debug — use `inspect` for symbol-level drill-down).
pub fn run_graph_projection_bench(
    label: &str,
    source_dir: &Path,
    max_files: usize,
) -> anyhow::Result<GraphProjectionBench> {
    if !source_dir.exists() {
        anyhow::bail!("source_dir does not exist: {:?}", source_dir);
    }
    if max_files == 0 {
        anyhow::bail!("max_files must be >= 1");
    }

    let candidates = collect_supported_files(source_dir, max_files)?;
    let total_candidates = candidates.len();

    let start = Instant::now();
    let mut files_processed: usize = 0;
    let mut files_skipped_unsupported: usize = 0;
    let mut symbols_extracted: usize = 0;
    let mut chunks_derived: usize = 0;

    for path in &candidates {
        let parser = match get_parser_for_file(path) {
            Some(p) => p,
            None => {
                files_skipped_unsupported += 1;
                continue;
            }
        };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                // unreadable file (binary, permission, encoding) —
                // count as unsupported to keep the bench progressing.
                files_skipped_unsupported += 1;
                continue;
            }
        };
        let result = parser.parse(&content);
        symbols_extracted += result.symbols.len();
        for symbol in &result.symbols {
            let chunks = build_symbol_chunks(symbol, &content);
            chunks_derived += chunks.len();
        }
        files_processed += 1;
    }

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    let elapsed_secs = elapsed.as_secs_f64().max(1e-9);

    let safe_div = |num: f64, den: f64| -> f64 {
        if den <= 0.0 { 0.0 } else { num / den }
    };

    let _ = total_candidates;
    Ok(GraphProjectionBench {
        label: label.to_string(),
        source_dir: source_dir.to_path_buf(),
        files_processed,
        files_skipped_unsupported,
        symbols_extracted,
        chunks_derived,
        elapsed_ms,
        files_per_s: safe_div(files_processed as f64, elapsed_secs),
        symbols_per_s: safe_div(symbols_extracted as f64, elapsed_secs),
        chunks_per_s: safe_div(chunks_derived as f64, elapsed_secs),
    })
}

/// Walk `dir` depth-first, return up to `max_files` paths whose
/// extension is supported by `parser::get_parser_for_file`. Skips
/// hidden directories (`.git`, `.axon`, `node_modules`, `target`,
/// etc.) to mirror the watcher policy.
pub(crate) fn collect_supported_files(
    dir: &Path,
    max_files: usize,
) -> anyhow::Result<Vec<PathBuf>> {
    use std::collections::VecDeque;

    let mut out: Vec<PathBuf> = Vec::with_capacity(max_files.min(8192));
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(dir.to_path_buf());

    while let Some(d) = queue.pop_front() {
        if out.len() >= max_files {
            break;
        }
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if out.len() >= max_files {
                break;
            }
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            // Skip noise dirs to mirror watcher exclusions.
            if path.is_dir() {
                if matches!(
                    name,
                    ".git" | ".axon" | ".devenv" | ".direnv" | "target"
                        | "node_modules" | ".worktrees" | "dist" | "build"
                ) {
                    continue;
                }
                queue.push_back(path);
                continue;
            }
            if get_parser_for_file(&path).is_some() {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// REQ-AXO-260 — Bench 3 result. Captures writer-lane ceiling.
#[derive(Debug, Clone)]
pub struct WriterBench {
    pub label: String,
    pub batch_size: usize,
    pub batches_written: usize,
    pub chunks_written: usize,
    pub elapsed_ms: u64,
    pub chunks_per_s: f64,
    pub batches_per_s: f64,
    pub backend: String,
}

/// REQ-AXO-260 — Synthetic writer bench. Generates `total_chunks`
/// fake `(symbol_id, content_hash, embedding[1024])` records, batches
/// them into `batch_size`, and persists via the configured backend.
///
/// Backend selection is delegated to a closure so the harness can
/// drive parquet, pgvector, sqlite, or a no-op (control) without
/// embedding backend-specific imports here.
///
/// `persist_batch_fn` returns `Ok(())` on success or anyhow error on
/// failure — propagated to the caller.
pub fn run_writer_bench<F>(
    label: &str,
    total_chunks: usize,
    batch_size: usize,
    embedding_dim: usize,
    backend_label: &str,
    mut persist_batch_fn: F,
) -> anyhow::Result<WriterBench>
where
    F: FnMut(&[SyntheticChunkRow]) -> anyhow::Result<()>,
{
    if total_chunks == 0 || batch_size == 0 {
        anyhow::bail!("writer bench requires total_chunks and batch_size >= 1");
    }
    if embedding_dim == 0 {
        anyhow::bail!("writer bench requires embedding_dim >= 1 (BGE-Large = 1024)");
    }

    let start = Instant::now();
    let mut chunks_written: usize = 0;
    let mut batches_written: usize = 0;

    let mut idx: usize = 0;
    while chunks_written < total_chunks {
        let this_batch = batch_size.min(total_chunks - chunks_written);
        let rows: Vec<SyntheticChunkRow> = (0..this_batch)
            .map(|i| SyntheticChunkRow {
                symbol_id: format!("synth/sym/{}", idx + i),
                content_hash: format!("hash{:016x}", (idx + i) as u64),
                embedding: synth_embedding(embedding_dim, idx + i),
            })
            .collect();
        persist_batch_fn(&rows)?;
        chunks_written += this_batch;
        batches_written += 1;
        idx += this_batch;
    }

    let elapsed = start.elapsed();
    let elapsed_ms = elapsed.as_millis() as u64;
    // Use sub-millisecond precision: a no-op control closure can
    // complete N batches in microseconds; integer ms granularity would
    // floor the rate to zero and mask real throughput.
    let elapsed_secs = elapsed.as_secs_f64().max(1e-9);
    let safe_div = |num: f64, den: f64| -> f64 {
        if den <= 0.0 { 0.0 } else { num / den }
    };

    Ok(WriterBench {
        label: label.to_string(),
        batch_size,
        batches_written,
        chunks_written,
        elapsed_ms,
        chunks_per_s: safe_div(chunks_written as f64, elapsed_secs),
        batches_per_s: safe_div(batches_written as f64, elapsed_secs),
        backend: backend_label.to_string(),
    })
}

/// REQ-AXO-260 — Single fake chunk row used by the writer bench.
/// Embedding values are deterministic from `idx` so different runs of
/// the same N produce identical input — supports A/B comparison
/// across backend variants.
#[derive(Debug, Clone)]
pub struct SyntheticChunkRow {
    pub symbol_id: String,
    pub content_hash: String,
    pub embedding: Vec<f32>,
}

fn synth_embedding(dim: usize, seed: usize) -> Vec<f32> {
    // Deterministic pseudo-random values in [-1, 1] using a tiny
    // hash so the writer sees realistic distribution without pulling
    // a real BGE inference path.
    let mut out = Vec::with_capacity(dim);
    let s = seed as u64;
    for i in 0..dim {
        let mix = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add((i as u64).wrapping_mul(1442695040888963407))
            ^ ((s >> 13) | (i as u64).rotate_left(7));
        let v = ((mix & 0xFFFF) as f32 / 32768.0) - 1.0;
        out.push(v);
    }
    out
}

/// REQ-AXO-261 — Bench 4 result placeholder. End-to-end bench is
/// typically a shell wrapper (`scripts/dev/bench-end-to-end.sh`) that
/// drives `axon-live --indexer-full --tensorrt`, captures CSV from the
/// runtime telemetry endpoint, and writes a final summary.
/// The Rust-side helper here exists so the wrapper can compute the
/// summary deterministically (rolling-min, p50/p95) from raw
/// observations matching the embedder bench format.
#[derive(Debug, Clone)]
pub struct EndToEndSummary {
    pub label: String,
    pub mean_ch_per_s: f64,
    pub rolling_10s_min: f64,
    pub p50_ch_per_s: f64,
    pub p95_ch_per_s: f64,
    pub total_chunks: usize,
    pub sample_count: usize,
}

/// REQ-AXO-261 — Compute summary stats from raw `(timestamp_ms,
/// chunks_in_window, window_ms)` samples emitted by the indexer
/// telemetry stream during a sustained measurement. Pure function —
/// shell wrapper feeds samples after collection completes.
pub fn summarize_end_to_end(label: &str, samples: &[(u64, usize, u64)]) -> EndToEndSummary {
    if samples.is_empty() {
        return EndToEndSummary {
            label: label.to_string(),
            mean_ch_per_s: 0.0,
            rolling_10s_min: 0.0,
            p50_ch_per_s: 0.0,
            p95_ch_per_s: 0.0,
            total_chunks: 0,
            sample_count: 0,
        };
    }
    let total_chunks: usize = samples.iter().map(|(_, n, _)| *n).sum();
    let total_ms: u64 = samples.iter().map(|(_, _, ms)| *ms).sum();
    let mean = if total_ms > 0 {
        (total_chunks as f64) * 1000.0 / (total_ms as f64)
    } else {
        0.0
    };

    let per_sample: Vec<f64> = samples
        .iter()
        .map(|(_, n, ms)| {
            if *ms > 0 {
                (*n as f64) * 1000.0 / (*ms as f64)
            } else {
                0.0
            }
        })
        .collect();
    let mut sorted = per_sample.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct_at = |percentile: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * percentile).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    };

    // Rolling 10s min over wall-clock-ordered samples.
    let mut rolling_min = f64::INFINITY;
    for (anchor_idx, (anchor_t, _, _)) in samples.iter().enumerate() {
        let window_lo = anchor_t.saturating_sub(10_000);
        let mut chunks_in_window: usize = 0;
        let mut window_ms: u64 = 0;
        for (back_idx, (t, n, ms)) in samples.iter().enumerate().rev() {
            if back_idx > anchor_idx {
                continue;
            }
            if *t < window_lo {
                break;
            }
            chunks_in_window += *n;
            window_ms = window_ms.saturating_add(*ms);
        }
        if window_ms == 0 {
            continue;
        }
        let chs = (chunks_in_window as f64) * 1000.0 / (window_ms as f64);
        if chs < rolling_min {
            rolling_min = chs;
        }
    }
    if !rolling_min.is_finite() {
        rolling_min = mean;
    }

    EndToEndSummary {
        label: label.to_string(),
        mean_ch_per_s: mean,
        rolling_10s_min: rolling_min,
        p50_ch_per_s: pct_at(0.5),
        p95_ch_per_s: pct_at(0.95),
        total_chunks,
        sample_count: samples.len(),
    }
}

/// REQ-AXO-252 / operator 2026-05-10 — VRAM telemetry shared across
/// all 4 benches. Returns peak VRAM used (MiB) by the current process
/// according to `nvidia-smi --query-compute-apps=...`. Returns None if
/// nvidia-smi is unavailable or the process has no GPU footprint.
///
/// Sampling is single-shot here; benches that want a peak reading
/// across a window must call this periodically and keep the max.
pub fn vram_used_mib_self() -> Option<u64> {
    // DEC-AXO-901626: single source of the `--query-compute-apps` probe +
    // parser lives in `crate::observed_gpu` (DRY / GUI-PRO-013). Self-VRAM
    // is just the probe applied to the current pid.
    crate::observed_gpu::observed_gpu_used_mib(std::process::id())
}

#[cfg(test)]
#[path = "bench_pipeline_stages_tests.rs"]
mod bench_pipeline_stages_tests;

// Suppress warning when Duration is not used yet (Bench 4 may add windows later).
#[allow(dead_code)]
const _DURATION_HINT: Option<Duration> = None;
