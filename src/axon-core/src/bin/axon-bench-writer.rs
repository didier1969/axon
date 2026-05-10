//! REQ-AXO-260 — Bench 3 of the 4-bench diagnostic framework.
//!
//! Pre-built synthetic chunk + embedding rows → DB persist. Isolates the
//! writer-lane ceiling from the GPU and producer.
//!
//! Backends:
//!   - `noop`     : pure CPU rate (measures synthesis + closure overhead)
//!   - `pgvector` : real `bulk_writer::flush_chunk_embeddings` (REQ-AXO-238)
//!                  Requires AXON_DB_BACKEND=postgres + bulk-writer pool
//!                  configured (AXON_DATABASE_URL etc.)
//!
//! Usage:
//!   axon-bench-writer [--backend noop|pgvector] [--total N]
//!                     [--batch N] [--dim N]
//!                     [--project-code AXO] [--model-id bge-large-en-v1.5]
//!                     [--label L] [--csv|--human]
//!
//! Defaults: --backend noop --total 10000 --batch 1000 --dim 1024
//! --project-code AXO --model-id bge-large-en-v1.5 --csv.

use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("❌ axon-bench-writer: {err:?}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct Args {
    backend: String,
    total: usize,
    batch: usize,
    dim: usize,
    project_code: String,
    model_id: String,
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
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::parse_from(&args)
    }

    fn parse_from(args: &[String]) -> anyhow::Result<Self> {
        let mut backend = "noop".to_string();
        let mut total: usize = 10_000;
        let mut batch: usize = 1_000;
        let mut dim: usize = 1024;
        let mut project_code = "AXO".to_string();
        let mut model_id = "bge-large-en-v1.5".to_string();
        let mut label = "writer".to_string();
        let mut output = OutputMode::Csv;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--backend" => {
                    backend = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--backend requires value"))?
                        .clone();
                    i += 2;
                }
                "--total" => {
                    total = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--total requires value"))?
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
                "--dim" => {
                    dim = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--dim requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--project-code" => {
                    project_code = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--project-code requires value"))?
                        .clone();
                    i += 2;
                }
                "--model-id" => {
                    model_id = args
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--model-id requires value"))?
                        .clone();
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
            backend,
            total,
            batch,
            dim,
            project_code,
            model_id,
            label,
            output,
        })
    }
}

fn print_help() {
    println!("axon-bench-writer [--backend noop|pgvector] [--total N] [--batch N] [--dim N]");
    println!("                  [--project-code C] [--model-id M] [--label L] [--csv|--human]");
    println!();
    println!("REQ-AXO-260 (Bench 3 of the 4-bench framework).");
    println!("  Pre-built synthetic chunks + embeddings → backend persist.");
    println!();
    println!("  --backend B          'noop' (CPU rate ceiling) or 'pgvector' (real PG persist).");
    println!("  --total N            total chunks to write (default 10000)");
    println!("  --batch N            batch size per persist call (default 1000)");
    println!("  --dim N              embedding dim (default 1024 = BGE-Large)");
    println!("  --project-code C     project_code passed to bulk_writer (default AXO)");
    println!("  --model-id M         model_id stored on ChunkEmbedding rows");
    println!("  --label L            run label for CSV (default writer)");
    println!("  --csv | --human      output mode (default csv)");
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse()?;

    eprintln!(
        "📊 axon-bench-writer: backend={} total={} batch={} dim={} label={}",
        args.backend, args.total, args.batch, args.dim, args.label
    );
    if args.backend == "pgvector" {
        eprintln!(
            "   AXON_DB_BACKEND={}",
            std::env::var("AXON_DB_BACKEND").unwrap_or_else(|_| "<unset>".into())
        );
        eprintln!(
            "   AXON_BULK_WRITER_ENABLED={}",
            std::env::var("AXON_BULK_WRITER_ENABLED").unwrap_or_else(|_| "<unset>".into())
        );
    }

    let bench = match args.backend.as_str() {
        "noop" => axon_core::bench_pipeline_stages::run_writer_bench(
            &args.label,
            args.total,
            args.batch,
            args.dim,
            "noop",
            |_rows| Ok(()),
        )?,
        "pgvector" => bench_pgvector(&args)?,
        other => anyhow::bail!("unknown backend: {other} (expected noop|pgvector)"),
    };

    match args.output {
        OutputMode::Csv => {
            eprintln!(
                "label,backend,total,batch,dim,batches_written,chunks_written,elapsed_ms,chunks_per_s,batches_per_s"
            );
            println!(
                "{},{},{},{},{},{},{},{},{:.2},{:.2}",
                bench.label,
                bench.backend,
                args.total,
                bench.batch_size,
                args.dim,
                bench.batches_written,
                bench.chunks_written,
                bench.elapsed_ms,
                bench.chunks_per_s,
                bench.batches_per_s,
            );
        }
        OutputMode::Human => {
            println!("📊 axon-bench-writer [{}] backend={}", bench.label, bench.backend);
            println!("   total           {}", args.total);
            println!("   batch           {}", bench.batch_size);
            println!("   dim             {}", args.dim);
            println!("   batches_written {}", bench.batches_written);
            println!("   chunks_written  {}", bench.chunks_written);
            println!("   elapsed_ms      {}", bench.elapsed_ms);
            println!("   chunks_per_s    {:.2}", bench.chunks_per_s);
            println!("   batches_per_s   {:.2}", bench.batches_per_s);
        }
    }
    Ok(())
}

fn bench_pgvector(
    args: &Args,
) -> anyhow::Result<axon_core::bench_pipeline_stages::WriterBench> {
    use axon_core::graph_ingestion::async_writer::ChunkEmbeddingPersistRow;
    use axon_core::postgres::bulk_writer::flush_chunk_embeddings;

    let project_code = args.project_code.clone();
    let model_id = args.model_id.clone();
    let embedded_at_ms = chrono_now_ms();

    axon_core::bench_pipeline_stages::run_writer_bench(
        &args.label,
        args.total,
        args.batch,
        args.dim,
        "pgvector",
        |rows| {
            let persist_rows: Vec<ChunkEmbeddingPersistRow> = rows
                .iter()
                .map(|r| ChunkEmbeddingPersistRow {
                    chunk_id: r.symbol_id.clone(),
                    source_hash: r.content_hash.clone(),
                    embedding: r.embedding.clone(),
                })
                .collect();
            flush_chunk_embeddings(&project_code, &model_id, &persist_rows, embedded_at_ms)?;
            Ok(())
        },
    )
}

fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_millis() as i64
}

// REQ-AXO-260 / GUI-PRO-001 — sibling tests file for the args parser.
#[cfg(test)]
#[path = "axon-bench-writer_tests.rs"]
mod axon_bench_writer_tests;
