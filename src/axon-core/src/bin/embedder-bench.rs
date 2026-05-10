//! REQ-AXO-176 — Standalone embedder throughput bench.
//!
//! Bypasses Pipeline 1 / DuckDB / federation orchestrator entirely:
//! loads the configured BGE model directly via `OrtGpuFirstTextEmbedding`
//! and embeds N texts sourced from a real .rs file in the repo
//! (no synthetic fixtures). Reports a single CSV line on stdout.
//!
//! Cycle target: ~5-10s including ORT init, ~30-60s on first-time
//! TensorRT graph compilation.
//!
//! Caller must set ORT env (`ORT_DYLIB_PATH`, `LD_LIBRARY_PATH`,
//! `AXON_GPU_EMBED_SERVICE_TENSORRT`, etc.) — typically via the
//! `scripts/dev/embed-bench.sh` wrapper which sources the active
//! ORT artifact manifest.
//!
//! Usage:
//!   embedder-bench [--n N] [--source PATH] [--no-force-gpu]
//!                  [--label LABEL] [--csv | --human]
//!                  [--sustained-secs N] [--warmup-secs N] [--batch N]
//!
//! Defaults: --n 256, --source src/axon-core/src/embedder.rs,
//! force_gpu=true, --csv (single-line CSV row).
//!
//! REQ-AXO-257 — when `--sustained-secs N` is passed (N > 0) the bench
//! switches to sustained-window mode: ignore --n, run `--warmup-secs`
//! warmup then `--sustained-secs` measurement, report mean +
//! rolling-10s-min + p50/p95.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("❌ embedder-bench: {err:?}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    n: usize,
    source: PathBuf,
    force_gpu: bool,
    label: String,
    output: OutputMode,
    workers: usize,
    // REQ-AXO-257
    sustained_secs: u64,
    warmup_secs: u64,
    batch: usize,
    /// REQ-AXO-257 sweep — comma-separated batch sizes; loads model
    /// once and runs sustained measurements at each. Empty = single-batch.
    sweep_batches: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
enum OutputMode {
    Csv,
    Human,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut n: usize = 256;
        let mut source = PathBuf::from("src/axon-core/src/embedder.rs");
        let mut force_gpu = true;
        let mut label = "bench".to_string();
        let mut output = OutputMode::Csv;
        let mut workers: usize = 1;
        // REQ-AXO-257 sustained-mode defaults
        let mut sustained_secs: u64 = 0;
        let mut warmup_secs: u64 = 30;
        let mut batch: usize = 64;
        let mut sweep_batches: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--sweep-batches" => {
                    let raw = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--sweep-batches requires comma-separated list"))?;
                    sweep_batches = raw
                        .split(',')
                        .map(|s| s.trim().parse::<usize>())
                        .collect::<Result<Vec<_>, _>>()?;
                    i += 2;
                }
                "--sustained-secs" => {
                    sustained_secs = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--sustained-secs requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--warmup-secs" => {
                    warmup_secs = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--warmup-secs requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--batch" => {
                    batch = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--batch requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--n" => {
                    n = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--n requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--source" => {
                    source = PathBuf::from(
                        args.get(i + 1)
                            .ok_or_else(|| anyhow::anyhow!("--source requires path"))?,
                    );
                    i += 2;
                }
                "--no-force-gpu" => {
                    force_gpu = false;
                    i += 1;
                }
                "--force-gpu" => {
                    force_gpu = true;
                    i += 1;
                }
                "--label" => {
                    label = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--label requires value"))?
                        .clone();
                    i += 2;
                }
                "--csv" => {
                    output = OutputMode::Csv;
                    i += 1;
                }
                "--human" => {
                    output = OutputMode::Human;
                    i += 1;
                }
                "--workers" => {
                    workers = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--workers requires value"))?
                        .parse()?;
                    i += 2;
                }
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown arg: {other}"),
            }
        }
        // anti-warning
        let _ = &mut args;
        Ok(Self {
            n,
            source,
            force_gpu,
            label,
            output,
            workers,
            sustained_secs,
            warmup_secs,
            batch,
            sweep_batches,
        })
    }
}

fn print_help() {
    println!(
        "embedder-bench [--n N] [--source PATH] [--no-force-gpu] [--label L] [--csv|--human] [--workers N]"
    );
    println!("               [--sustained-secs N] [--warmup-secs N] [--batch N]");
    println!("  --n N            number of texts to embed (default 256, single-shot mode only)");
    println!("  --source PATH    real source file to read embedding texts from");
    println!("                   (default src/axon-core/src/embedder.rs)");
    println!("  --no-force-gpu   pass force_gpu=false to OrtGpuFirstTextEmbedding");
    println!("  --label LABEL    label passed to OrtGpuFirstTextEmbedding::try_new");
    println!("  --csv            (default) single-line CSV: see header on first stderr line");
    println!("  --human          multi-line summary");
    println!("  --workers N      spawn N parallel embedder instances (REQ-AXO-176)");
    println!("                   each instance ≈ 680 MB VRAM; default 1");
    println!();
    println!("REQ-AXO-257 sustained-window mode (set --sustained-secs > 0):");
    println!("  --sustained-secs N   measure for N seconds after warmup (default 0 = single-shot)");
    println!("  --warmup-secs N      warmup phase length in seconds (default 30)");
    println!("  --batch N            batch size per embed iteration (default 64)");
    println!("  --sweep-batches LIST comma-separated batch sizes (e.g. 128,160,180,220).");
    println!("                       Loads model ONCE, runs sustained at each — saves ~1min");
    println!("                       process-boot per batch on cold runs (operator 2026-05-10).");
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse()?;

    // Build N texts from the real source file. We do NOT generate
    // synthetic strings: real code-shaped tokens are what the
    // embedder actually sees in production.
    let raw = std::fs::read_to_string(&args.source)
        .map_err(|err| anyhow::anyhow!("failed to read --source {:?}: {err}", args.source))?;
    let texts = build_chunks_from_source(&raw, args.n);
    if texts.len() != args.n {
        anyhow::bail!(
            "source file too small for n={}: only got {} non-empty chunks of ~512 chars",
            args.n,
            texts.len()
        );
    }

    if args.sustained_secs > 0 {
        if !args.sweep_batches.is_empty() {
            return run_sustained_sweep(args, texts);
        }
        return run_sustained(args, texts);
    }

    eprintln!(
        "📊 embedder-bench: n={} force_gpu={} workers={} source={} label={}",
        args.n,
        args.force_gpu,
        args.workers,
        args.source.display(),
        args.label
    );
    eprintln!("   ORT_DYLIB_PATH={}",
        std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| "<unset>".into())
    );
    eprintln!("   AXON_GPU_EMBED_SERVICE_TENSORRT={}",
        std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT").unwrap_or_else(|_| "<unset>".into())
    );

    let bench = if args.workers > 1 {
        axon_core::embedder::run_embedder_throughput_bench_parallel(
            &args.label,
            texts,
            args.force_gpu,
            args.workers,
        )?
    } else {
        axon_core::embedder::run_embedder_throughput_bench(
            &args.label,
            texts,
            args.force_gpu,
        )?
    };

    match args.output {
        OutputMode::Csv => {
            // Header on stderr (so stdout stays parseable as a single CSV row)
            eprintln!(
                "label,n,dim,load_ms,total_embed_ms,tokenize_ms,host_prepare_ms,input_copy_ms,inference_ms,output_extract_ms,chunks_per_sec"
            );
            println!(
                "{},{},{},{},{},{},{},{},{},{},{:.2}",
                args.label,
                bench.n,
                bench.embedding_dim,
                bench.load_ms,
                bench.total_embed_ms,
                bench.tokenize_ms,
                bench.host_prepare_ms,
                bench.input_copy_ms,
                bench.inference_ms,
                bench.output_extract_ms,
                bench.chunks_per_second()
            );
        }
        OutputMode::Human => {
            let unaccounted = bench.total_embed_ms.saturating_sub(
                bench.tokenize_ms
                    + bench.host_prepare_ms
                    + bench.input_copy_ms
                    + bench.inference_ms
                    + bench.output_extract_ms,
            );
            println!("📊 embedder-bench [{}]", args.label);
            println!("   n              {}", bench.n);
            println!("   dim            {}", bench.embedding_dim);
            println!("   load_ms        {}", bench.load_ms);
            println!("   total_embed_ms {}", bench.total_embed_ms);
            println!("     tokenize     {} ({}%)",
                bench.tokenize_ms,
                pct(bench.tokenize_ms, bench.total_embed_ms));
            println!("     host_prepare {}", bench.host_prepare_ms);
            println!("     input_copy   {}", bench.input_copy_ms);
            println!("     inference    {} ({}%)",
                bench.inference_ms,
                pct(bench.inference_ms, bench.total_embed_ms));
            println!("     output_extract {}", bench.output_extract_ms);
            println!("     unaccounted  {}", unaccounted);
            println!("   chunks/sec     {:.2}", bench.chunks_per_second());
        }
    }
    Ok(())
}

fn pct(part: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        (part * 100) / total
    }
}

/// REQ-AXO-257 / operator 2026-05-10 — sweep dispatch. Loads model
/// once, runs sustained at each batch size, emits CSV row per batch.
fn run_sustained_sweep(args: Args, texts: Vec<String>) -> anyhow::Result<()> {
    eprintln!(
        "📊 embedder-bench (sweep): batches={:?} warmup={}s sustained={}s force_gpu={} pool={} label={}",
        args.sweep_batches,
        args.warmup_secs,
        args.sustained_secs,
        args.force_gpu,
        texts.len(),
        args.label
    );
    eprintln!(
        "   Model loaded once — saves ~{}s vs separate runs.",
        args.sweep_batches.len().saturating_sub(1) * 60
    );

    let results = axon_core::embedder::run_embedder_sustained_sweep(
        &args.label,
        texts,
        &args.sweep_batches,
        args.warmup_secs,
        args.sustained_secs,
        args.force_gpu,
    )?;

    match args.output {
        OutputMode::Csv => {
            eprintln!(
                "label,batch,warmup_secs,sustained_secs,total_chunks,mean_ch_per_s,rolling_10s_min,p50,p95,dim,mean_iter_ms,mean_inter_iter_gap_ms"
            );
            for bench in results {
                println!(
                    "{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{},{:.2},{:.2}",
                    bench.label,
                    bench.batch_size,
                    bench.warmup_secs,
                    bench.sustained_secs,
                    bench.total_chunks,
                    bench.mean_ch_per_s,
                    bench.rolling_10s_min,
                    bench.p50_ch_per_s,
                    bench.p95_ch_per_s,
                    bench.embedding_dim,
                    bench.mean_iter_ms,
                    bench.mean_inter_iter_gap_ms,
                );
            }
        }
        OutputMode::Human => {
            println!("📊 embedder-bench [sweep: {}]", args.label);
            for bench in &results {
                println!("  ── batch={} ──", bench.batch_size);
                println!("     mean_ch_per_s   {:.2}", bench.mean_ch_per_s);
                println!("     rolling_10s_min {:.2}", bench.rolling_10s_min);
                println!("     p50 / p95       {:.2} / {:.2}", bench.p50_ch_per_s, bench.p95_ch_per_s);
                println!("     iter_ms / gap   {:.2} / {:.2}", bench.mean_iter_ms, bench.mean_inter_iter_gap_ms);
                println!("     total_chunks    {}", bench.total_chunks);
            }
        }
    }
    Ok(())
}

/// REQ-AXO-257 — sustained-window dispatch: feeds all available source
/// texts as a cycling pool to `run_embedder_sustained_bench`.
fn run_sustained(args: Args, texts: Vec<String>) -> anyhow::Result<()> {
    eprintln!(
        "📊 embedder-bench (sustained): batch={} warmup={}s sustained={}s force_gpu={} pool={} label={}",
        args.batch,
        args.warmup_secs,
        args.sustained_secs,
        args.force_gpu,
        texts.len(),
        args.label
    );
    eprintln!("   ORT_DYLIB_PATH={}",
        std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| "<unset>".into())
    );
    eprintln!("   AXON_GPU_EMBED_SERVICE_TENSORRT={}",
        std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT").unwrap_or_else(|_| "<unset>".into())
    );

    let bench = axon_core::embedder::run_embedder_sustained_bench(
        &args.label,
        texts,
        args.batch,
        args.warmup_secs,
        args.sustained_secs,
        args.force_gpu,
    )?;

    match args.output {
        OutputMode::Csv => {
            eprintln!(
                "label,batch,warmup_secs,sustained_secs,total_chunks,mean_ch_per_s,rolling_10s_min,p50,p95,dim,mean_iter_ms,mean_inter_iter_gap_ms"
            );
            println!(
                "{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{},{:.2},{:.2}",
                bench.label,
                bench.batch_size,
                bench.warmup_secs,
                bench.sustained_secs,
                bench.total_chunks,
                bench.mean_ch_per_s,
                bench.rolling_10s_min,
                bench.p50_ch_per_s,
                bench.p95_ch_per_s,
                bench.embedding_dim,
                bench.mean_iter_ms,
                bench.mean_inter_iter_gap_ms,
            );
        }
        OutputMode::Human => {
            println!("📊 embedder-bench [sustained: {}]", bench.label);
            println!("   batch          {}", bench.batch_size);
            println!("   warmup_secs    {}", bench.warmup_secs);
            println!("   sustained_secs {}", bench.sustained_secs);
            println!("   total_chunks   {}", bench.total_chunks);
            println!("   mean_ch_per_s  {:.2}", bench.mean_ch_per_s);
            println!("   rolling_10s_min {:.2}", bench.rolling_10s_min);
            println!("   p50_ch_per_s   {:.2}", bench.p50_ch_per_s);
            println!("   p95_ch_per_s   {:.2}", bench.p95_ch_per_s);
            println!("   mean_iter_ms   {:.2}", bench.mean_iter_ms);
            println!("   mean_inter_iter_gap_ms {:.2}  (← REQ-AXO-262 dispatch overhead)", bench.mean_inter_iter_gap_ms);
            println!("   dim            {}", bench.embedding_dim);
            // REQ-AXO-262 — iter-by-iter trace to expose bimodal distribution.
            println!("   iter_ch_per_s trace (first 30):");
            for (i, v) in bench.iter_ch_per_s.iter().take(30).enumerate() {
                println!("     [{:3}] {:8.2} ch/s", i, v);
            }
        }
    }
    Ok(())
}

/// Split the source file into roughly-512-char chunks, tagged with
/// chunk index so each text is unique (avoids tokenizer caching
/// across identical inputs).
fn build_chunks_from_source(raw: &str, n: usize) -> Vec<String> {
    let chunk_size: usize = 512;
    let mut chunks = Vec::with_capacity(n);
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut idx = 0usize;
    while chunks.len() < n && i < bytes.len() {
        let end = (i + chunk_size).min(bytes.len());
        // Snap to UTF-8 boundary so we don't slice mid-codepoint.
        let mut end_safe = end;
        while end_safe < bytes.len() && (bytes[end_safe] & 0b1100_0000) == 0b1000_0000 {
            end_safe += 1;
        }
        if end_safe > bytes.len() {
            end_safe = bytes.len();
        }
        let slice = &raw[i..end_safe];
        if !slice.trim().is_empty() {
            chunks.push(format!("// chunk {idx}\n{slice}"));
            idx += 1;
        }
        i = end_safe;
    }
    chunks
}
