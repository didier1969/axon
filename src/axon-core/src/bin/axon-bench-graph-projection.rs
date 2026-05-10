//! REQ-AXO-259 — Bench 1 of the 4-bench diagnostic framework.
//!
//! Drives `bench_pipeline_stages::run_graph_projection_bench`:
//!   walk source_dir → tree-sitter parse → symbol extract → chunk derive
//!
//! NO DB inserts. Pure producer-side ceiling measurement.
//!
//! Usage:
//!   axon-bench-graph-projection [--source-dir PATH] [--max-files N]
//!                               [--label L] [--csv|--human]
//!
//! Defaults: --source-dir . (current dir) --max-files 200 --csv.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("❌ axon-bench-graph-projection: {err:?}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    source_dir: PathBuf,
    max_files: usize,
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
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut source_dir = PathBuf::from(".");
        let mut max_files: usize = 200;
        let mut label = "graph-projection".to_string();
        let mut output = OutputMode::Csv;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--source-dir" => {
                    source_dir = PathBuf::from(
                        args.get(i + 1)
                            .ok_or_else(|| anyhow::anyhow!("--source-dir requires path"))?,
                    );
                    i += 2;
                }
                "--max-files" => {
                    max_files = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--max-files requires value"))?
                        .parse()?;
                    i += 2;
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
        Ok(Self {
            source_dir,
            max_files,
            label,
            output,
        })
    }
}

fn print_help() {
    println!("axon-bench-graph-projection [--source-dir PATH] [--max-files N] [--label L] [--csv|--human]");
    println!();
    println!("REQ-AXO-259 (Bench 1 of the 4-bench framework).");
    println!("  --source-dir PATH    walk this directory (default .)");
    println!("  --max-files N        cap parsed file count (default 200)");
    println!("  --label LABEL        run label for CSV (default graph-projection)");
    println!("  --csv | --human      output mode (default csv)");
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse()?;

    eprintln!(
        "📊 axon-bench-graph-projection: source_dir={:?} max_files={} label={}",
        args.source_dir, args.max_files, args.label
    );

    let bench = axon_core::bench_pipeline_stages::run_graph_projection_bench(
        &args.label,
        &args.source_dir,
        args.max_files,
    )?;

    match args.output {
        OutputMode::Csv => {
            eprintln!(
                "label,source_dir,files_processed,files_skipped,symbols_extracted,chunks_derived,elapsed_ms,files_per_s,symbols_per_s,chunks_per_s"
            );
            println!(
                "{},{},{},{},{},{},{},{:.2},{:.2},{:.2}",
                bench.label,
                bench.source_dir.display(),
                bench.files_processed,
                bench.files_skipped_unsupported,
                bench.symbols_extracted,
                bench.chunks_derived,
                bench.elapsed_ms,
                bench.files_per_s,
                bench.symbols_per_s,
                bench.chunks_per_s,
            );
        }
        OutputMode::Human => {
            println!("📊 axon-bench-graph-projection [{}]", bench.label);
            println!("   source_dir          {:?}", bench.source_dir);
            println!("   files_processed     {}", bench.files_processed);
            println!("   files_skipped       {}", bench.files_skipped_unsupported);
            println!("   symbols_extracted   {}", bench.symbols_extracted);
            println!("   chunks_derived      {}", bench.chunks_derived);
            println!("   elapsed_ms          {}", bench.elapsed_ms);
            println!("   files_per_s         {:.2}", bench.files_per_s);
            println!("   symbols_per_s       {:.2}", bench.symbols_per_s);
            println!("   chunks_per_s        {:.2}", bench.chunks_per_s);
        }
    }
    Ok(())
}
