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
//!
//! Defaults: --n 256, --source src/axon-core/src/embedder.rs,
//! force_gpu=true, --csv (single-line CSV row).

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
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
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
        })
    }
}

fn print_help() {
    println!(
        "embedder-bench [--n N] [--source PATH] [--no-force-gpu] [--label L] [--csv|--human]"
    );
    println!("  --n N            number of texts to embed (default 256)");
    println!("  --source PATH    real source file to read embedding texts from");
    println!("                   (default src/axon-core/src/embedder.rs)");
    println!("  --no-force-gpu   pass force_gpu=false to OrtGpuFirstTextEmbedding");
    println!("  --label LABEL    label passed to OrtGpuFirstTextEmbedding::try_new");
    println!("  --csv            (default) single-line CSV: see header on first stderr line");
    println!("  --human          multi-line summary");
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

    eprintln!(
        "📊 embedder-bench: n={} force_gpu={} source={} label={}",
        args.n,
        args.force_gpu,
        args.source.display(),
        args.label
    );
    eprintln!("   ORT_DYLIB_PATH={}",
        std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| "<unset>".into())
    );
    eprintln!("   AXON_GPU_EMBED_SERVICE_TENSORRT={}",
        std::env::var("AXON_GPU_EMBED_SERVICE_TENSORRT").unwrap_or_else(|_| "<unset>".into())
    );

    let bench = axon_core::embedder::run_embedder_throughput_bench(
        &args.label,
        texts,
        args.force_gpu,
    )?;

    match args.output {
        OutputMode::Csv => {
            // Header on stderr (so stdout stays parseable as a single CSV row)
            eprintln!(
                "label,n,dim,load_ms,total_embed_ms,host_prepare_ms,input_copy_ms,inference_ms,output_extract_ms,chunks_per_sec"
            );
            println!(
                "{},{},{},{},{},{},{},{},{},{:.2}",
                args.label,
                bench.n,
                bench.embedding_dim,
                bench.load_ms,
                bench.total_embed_ms,
                bench.host_prepare_ms,
                bench.input_copy_ms,
                bench.inference_ms,
                bench.output_extract_ms,
                bench.chunks_per_second()
            );
        }
        OutputMode::Human => {
            println!("📊 embedder-bench [{}]", args.label);
            println!("   n              {}", bench.n);
            println!("   dim            {}", bench.embedding_dim);
            println!("   load_ms        {}", bench.load_ms);
            println!("   total_embed_ms {}", bench.total_embed_ms);
            println!("     host_prepare {}", bench.host_prepare_ms);
            println!("     input_copy   {}", bench.input_copy_ms);
            println!("     inference    {}", bench.inference_ms);
            println!("     output_extract {}", bench.output_extract_ms);
            println!("   chunks/sec     {:.2}", bench.chunks_per_second());
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
