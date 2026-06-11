//! REQ-AXO-257 — Pipeline throughput bench (reconstructed from VAL-AXO-050).
//!
//! Drives `run_embedder_pipeline_bench` (N producers / bounded channel /
//! 1 consumer) and reports queue_high_water + sustained throughput.
//!
//! Caller must set ORT env (`ORT_DYLIB_PATH`, `LD_LIBRARY_PATH`,
//! `AXON_GPU_EMBED_SERVICE_TENSORRT`, etc.) — typically via the
//! `scripts/dev/embed-bench.sh` wrapper.
//!
//! Usage:
//!   axon-bench-pipeline [--producers N] [--channel N] [--batch N]
//!                       [--warmup-secs N] [--sustained-secs N]
//!                       [--source PATH] [--no-force-gpu]
//!                       [--label L] [--csv|--human]
//!
//! Defaults: --producers 4, --channel 64, --batch 128, --warmup-secs 30,
//! --sustained-secs 60, --source src/axon-core/src/embedder.rs,
//! force_gpu=true, --csv.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("❌ axon-bench-pipeline: {err:?}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    producers: usize,
    channel: usize,
    batch: usize,
    warmup_secs: u64,
    sustained_secs: u64,
    source: PathBuf,
    force_gpu: bool,
    label: String,
    output: OutputMode,
    pool_size: usize,
}

#[derive(Debug, Clone, Copy)]
enum OutputMode {
    Csv,
    Human,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut producers: usize = 4;
        let mut channel: usize = 64;
        let mut batch: usize = 128;
        let mut warmup_secs: u64 = 30;
        let mut sustained_secs: u64 = 60;
        let mut source = PathBuf::from("src/axon-core/src/embedder.rs");
        let mut force_gpu = true;
        let mut label = "pipeline".to_string();
        let mut output = OutputMode::Csv;
        let mut pool_size: usize = 60_000;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--producers" => {
                    producers = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--producers requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--channel" => {
                    channel = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--channel requires value"))?
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
                "--warmup-secs" => {
                    warmup_secs = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--warmup-secs requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--sustained-secs" => {
                    sustained_secs = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--sustained-secs requires value"))?
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
                "--pool-size" => {
                    pool_size = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--pool-size requires value"))?
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
        Ok(Self {
            producers,
            channel,
            batch,
            warmup_secs,
            sustained_secs,
            source,
            force_gpu,
            label,
            output,
            pool_size,
        })
    }
}

fn print_help() {
    println!("axon-bench-pipeline [--producers N] [--channel N] [--batch N]");
    println!("                    [--warmup-secs N] [--sustained-secs N]");
    println!("                    [--source PATH] [--no-force-gpu] [--label L]");
    println!("                    [--csv|--human] [--pool-size N]");
    println!();
    println!("  --producers N      number of producer threads (default 4)");
    println!("  --channel N        bounded channel capacity (default 64)");
    println!("  --batch N          batch size per send/recv (default 128)");
    println!("  --warmup-secs N    warmup duration in seconds (default 30)");
    println!("  --sustained-secs N sustained measurement window (default 60)");
    println!("  --source PATH      source file for chunk pool (default embedder.rs)");
    println!("  --pool-size N      number of unique chunks in pool (default 60000)");
    println!("  --no-force-gpu     pass force_gpu=false");
    println!("  --label LABEL      embedder instance label");
    println!("  --csv | --human    output mode (default csv)");
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse()?;

    let raw = std::fs::read_to_string(&args.source)
        .map_err(|err| anyhow::anyhow!("failed to read --source {:?}: {err}", args.source))?;
    let pool = build_chunks_from_source(&raw, args.pool_size);
    if pool.is_empty() {
        anyhow::bail!(
            "source file too small to build any chunk: {:?}",
            args.source
        );
    }

    eprintln!(
        "📊 axon-bench-pipeline: producers={} channel={} batch={} warmup={}s sustained={}s pool={} label={} force_gpu={}",
        args.producers,
        args.channel,
        args.batch,
        args.warmup_secs,
        args.sustained_secs,
        pool.len(),
        args.label,
        args.force_gpu
    );
    eprintln!(
        "   ORT_DYLIB_PATH={}",
        std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| "<unset>".into())
    );
    eprintln!(
        "   AXON_GPU_EMBED_SERVICE_TENSORRT={}",
        std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT").unwrap_or_else(|_| "<unset>".into())
    );

    let bench = axon_core::embedder::run_embedder_pipeline_bench(
        &args.label,
        pool,
        args.producers,
        args.channel,
        args.batch,
        args.warmup_secs,
        args.sustained_secs,
        args.force_gpu,
    )?;

    match args.output {
        OutputMode::Csv => {
            eprintln!(
                "label,producers,channel,batch,warmup_secs,sustained_secs,total_chunks,mean_ch_per_s,rolling_10s_min,queue_high_water,dim"
            );
            println!(
                "{},{},{},{},{},{},{},{:.2},{:.2},{},{}",
                bench.label,
                bench.producers,
                bench.channel_capacity,
                bench.batch_size,
                bench.warmup_secs,
                bench.sustained_secs,
                bench.total_chunks,
                bench.mean_ch_per_s,
                bench.rolling_10s_min,
                bench.queue_high_water,
                bench.embedding_dim,
            );
        }
        OutputMode::Human => {
            println!("📊 axon-bench-pipeline [{}]", bench.label);
            println!("   producers       {}", bench.producers);
            println!("   channel         {}", bench.channel_capacity);
            println!("   batch           {}", bench.batch_size);
            println!("   warmup_secs     {}", bench.warmup_secs);
            println!("   sustained_secs  {}", bench.sustained_secs);
            println!("   total_chunks    {}", bench.total_chunks);
            println!("   mean_ch_per_s   {:.2}", bench.mean_ch_per_s);
            println!("   rolling_10s_min {:.2}", bench.rolling_10s_min);
            println!(
                "   queue_high_water {} / {}",
                bench.queue_high_water, bench.channel_capacity
            );
            println!("   dim             {}", bench.embedding_dim);
        }
    }
    Ok(())
}

/// Split the source file into roughly-512-char chunks tagged with idx.
/// Same routine as `embedder-bench` to keep pool format identical.
fn build_chunks_from_source(raw: &str, n: usize) -> Vec<String> {
    let chunk_size: usize = 512;
    let mut chunks = Vec::with_capacity(n);
    let bytes = raw.as_bytes();
    let mut i = 0usize;
    let mut idx = 0usize;
    while chunks.len() < n && i < bytes.len() {
        let end = (i + chunk_size).min(bytes.len());
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
