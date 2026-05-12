//! REQ-AXO-289 S6a — End-to-end streaming pipeline v2 bench.
//!
//! Drives the full v2 topology (A1 → A2 → A3 atomic graph+chunks+FTS
//! → try_send chunk_ids → B1 fetch content → B2 embed → B3 UPSERT
//! ChunkEmbedding) over a real source tree. Reports files/s, chunks/s,
//! and per-stage `items_out_total` + `backpressure_blocks_total` to
//! identify the dominant bottleneck stage.
//!
//! Usage:
//!   axon-bench-pipeline-v2 [--source PATH] [--max-files N]
//!                          [--duration-secs N] [--gpu|--cpu|--noop]
//!                          [--project AXO] [--csv|--human]
//!
//! Defaults: --source $PWD, --max-files 200, --duration-secs 0
//! (process all files then exit), --gpu, --project AXO, --csv.
//!
//! Requires `AXON_DEV_DATABASE_URL` (or `DATABASE_URL`) for GpuB2Embedder
//! mode. The `--noop` mode uses [`axon_core::pipeline_v2::NoOpEmbedder`]
//! and a temp DuckDB-backed store — handy for verifying the topology
//! end-to-end without GPU/PG.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use axon_core::graph::GraphStore;
use axon_core::pipeline_v2::{
    spawn_pipeline_a, spawn_pipeline_b_full, B2Embedder, GpuB2Embedder, NoOpEmbedder,
    PipelineAWorkerCounts, PipelineBWorkerCounts, PipelineChannelCaps,
};

#[derive(Debug, Clone, Copy)]
enum EmbedderMode {
    Gpu,
    Cpu,
    NoOp,
}

#[derive(Debug, Clone, Copy)]
enum OutputMode {
    Csv,
    Human,
}

#[derive(Debug)]
struct Args {
    source: PathBuf,
    max_files: usize,
    duration_secs: u64,
    project: String,
    embedder_mode: EmbedderMode,
    output: OutputMode,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut source: PathBuf = std::env::current_dir()?;
        let mut max_files: usize = 200;
        let mut duration_secs: u64 = 0;
        let mut project = String::from("AXO");
        let mut embedder_mode = EmbedderMode::Gpu;
        let mut output = OutputMode::Csv;
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--source" => source = PathBuf::from(iter.next().context("--source needs PATH")?),
                "--max-files" => max_files = iter.next().context("--max-files N")?.parse()?,
                "--duration-secs" => {
                    duration_secs = iter.next().context("--duration-secs N")?.parse()?
                }
                "--project" => project = iter.next().context("--project AXO")?,
                "--gpu" => embedder_mode = EmbedderMode::Gpu,
                "--cpu" => embedder_mode = EmbedderMode::Cpu,
                "--noop" => embedder_mode = EmbedderMode::NoOp,
                "--csv" => output = OutputMode::Csv,
                "--human" => output = OutputMode::Human,
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(anyhow!("unknown arg: {other}")),
            }
        }
        Ok(Self {
            source,
            max_files,
            duration_secs,
            project,
            embedder_mode,
            output,
        })
    }
}

fn print_help() {
    eprintln!(
        "axon-bench-pipeline-v2 — REQ-AXO-289 S6a\n\
         Usage: axon-bench-pipeline-v2 [--source PATH] [--max-files N] \\\n\
                                       [--duration-secs N] [--gpu|--cpu|--noop] \\\n\
                                       [--project CODE] [--csv|--human]\n\
         Env: AXON_DEV_DATABASE_URL or DATABASE_URL for non-NoOp modes."
    );
}

fn walk_source(root: &Path, max_files: usize) -> Result<Vec<PathBuf>> {
    fn rec(dir: &Path, out: &mut Vec<PathBuf>, cap: usize) -> Result<()> {
        if out.len() >= cap {
            return Ok(());
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        for entry in entries.flatten() {
            if out.len() >= cap {
                return Ok(());
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                rec(&path, out, cap)?;
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(
                    ext,
                    "rs" | "ex" | "exs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java"
                ) {
                    out.push(path);
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    rec(root, &mut out, max_files)?;
    Ok(out)
}

fn build_embedder(mode: EmbedderMode) -> Result<Arc<dyn B2Embedder>> {
    match mode {
        EmbedderMode::Gpu => {
            eprintln!("axon-bench-pipeline-v2: initialising GPU embedder (TensorRT compile ~5s)…");
            Ok(Arc::new(GpuB2Embedder::try_new_cuda("v2-bench", 0)?))
        }
        EmbedderMode::Cpu => {
            eprintln!("axon-bench-pipeline-v2: initialising CPU embedder…");
            Ok(Arc::new(GpuB2Embedder::try_new_cpu("v2-bench", 0)?))
        }
        EmbedderMode::NoOp => Ok(Arc::new(NoOpEmbedder)),
    }
}

fn build_store(mode: EmbedderMode) -> Result<GraphStore> {
    match mode {
        EmbedderMode::NoOp => {
            // Temp DuckDB store — smoke-test friendly, no PG required.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let path = format!("/tmp/axon_v2_bench_{}_{}", std::process::id(), now);
            GraphStore::new(&path)
        }
        EmbedderMode::Gpu | EmbedderMode::Cpu => {
            // Real PG store. URL resolution is delegated to GraphStore::new
            // which reads AXON_DB_BACKEND + AXON_*_DATABASE_URL env vars.
            std::env::set_var("AXON_DB_BACKEND", "postgres");
            let url = std::env::var("AXON_DEV_DATABASE_URL")
                .or_else(|_| std::env::var("DATABASE_URL"))
                .context("set AXON_DEV_DATABASE_URL or DATABASE_URL for non-NoOp bench")?;
            GraphStore::new(&url)
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("axon-bench-pipeline-v2: {err:?}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let args = Args::parse()?;
    eprintln!("axon-bench-pipeline-v2: args = {args:?}");

    let files = walk_source(&args.source, args.max_files)?;
    if files.is_empty() {
        return Err(anyhow!(
            "no eligible source files under {}",
            args.source.display()
        ));
    }
    eprintln!(
        "axon-bench-pipeline-v2: walked {} files under {}",
        files.len(),
        args.source.display()
    );

    let embedder = build_embedder(args.embedder_mode)?;
    let store = Arc::new(build_store(args.embedder_mode)?);
    let caps = PipelineChannelCaps::from_env();
    let counts_a = PipelineAWorkerCounts::from_env();
    let counts_b = PipelineBWorkerCounts::from_env();

    eprintln!("axon-bench-pipeline-v2: caps={caps:?} a={counts_a:?} b={counts_b:?}");

    let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), args.project.clone());
    let b1_inbox_rx = std::mem::replace(
        &mut handles_a.b1_inbox_rx,
        tokio::sync::mpsc::channel(1).1,
    );
    let mut handles_b = spawn_pipeline_b_full(
        counts_b,
        caps,
        store.clone(),
        args.project.clone(),
        embedder,
        b1_inbox_rx,
    );

    let total_files = files.len();
    let start = Instant::now();

    // Move the sole input_tx into the feeder so its drop closes the
    // channel once every path has been pushed. Keeping a clone on the
    // stack here would leave A1 workers waiting on recv() forever.
    let input_tx = handles_a.input_tx;
    let feeder = tokio::spawn(async move {
        for path in files {
            if input_tx.send(path).await.is_err() {
                break;
            }
        }
        // input_tx dropped here -> A1 recv() returns None ->
        // A2 / A3 cascade-drain -> output_tx + b1_inbox_tx drop ->
        // B1 / B2 / B3 cascade-drain.
    });

    // Consumer task — drain EnrolledFile + PersistedEmbedding receipts
    // and count them. Time-boxed by --duration-secs if non-zero, else
    // run until both channels close.
    let deadline =
        (args.duration_secs > 0).then(|| Instant::now() + Duration::from_secs(args.duration_secs));

    let mut a_count = 0usize;
    let mut b_count = 0usize;
    let mut a_open = true;
    let mut b_open = true;
    while a_open || b_open {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break;
            }
        }
        tokio::select! {
            biased;
            msg = handles_a.output_rx.recv(), if a_open => match msg {
                Some(_) => a_count += 1,
                None => a_open = false,
            },
            msg = handles_b.output_rx.recv(), if b_open => match msg {
                Some(_) => b_count += 1,
                None => b_open = false,
            },
        }
    }

    let elapsed = start.elapsed();
    let _ = feeder.await;

    // Post-run sanity counts straight from the canonical store. Reveal
    // discrepancies between in-RAM stage counters and persisted PG /
    // legacy backend rows (e.g. B1 oversend, MVCC visibility lag,
    // ON CONFLICT silent dedup in bulk INSERT).
    let chunk_rows = store
        .query_count("SELECT count(*) FROM Chunk")
        .unwrap_or(-1);
    let embedding_rows = store
        .query_count("SELECT count(*) FROM ChunkEmbedding")
        .unwrap_or(-1);
    let symbol_rows = store
        .query_count("SELECT count(*) FROM Symbol")
        .unwrap_or(-1);
    let indexed_rows = store
        .query_count("SELECT count(*) FROM IndexedFile")
        .unwrap_or(-1);

    let snap_a1 = handles_a.metrics_a1.snapshot();
    let snap_a2 = handles_a.metrics_a2.snapshot();
    let snap_a3 = handles_a.metrics_a3.snapshot();
    let snap_b1 = handles_b.metrics_b1.snapshot();
    let snap_b2 = handles_b.metrics_b2.snapshot();
    let snap_b3 = handles_b.metrics_b3.snapshot();

    let files_per_sec = a_count as f64 / elapsed.as_secs_f64().max(0.000_001);
    let chunks_per_sec = b_count as f64 / elapsed.as_secs_f64().max(0.000_001);

    match args.output {
        OutputMode::Csv => {
            println!(
                "label,files,chunks,elapsed_ms,files_per_sec,chunks_per_sec,\
                 a1_in,a1_out,a1_err,a1_bp,a2_in,a2_out,a2_err,a2_bp,\
                 a3_in,a3_out,a3_err,a3_bp,b1_in,b1_out,b1_err,b1_bp,\
                 b2_in,b2_out,b2_err,b2_bp,b3_in,b3_out,b3_err,b3_bp"
            );
            println!(
                "v2-bench,{},{},{:.0},{:.2},{:.2},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                a_count, b_count, elapsed.as_millis(), files_per_sec, chunks_per_sec,
                snap_a1.items_in_total, snap_a1.items_out_total, snap_a1.errors_total, snap_a1.backpressure_blocks_total,
                snap_a2.items_in_total, snap_a2.items_out_total, snap_a2.errors_total, snap_a2.backpressure_blocks_total,
                snap_a3.items_in_total, snap_a3.items_out_total, snap_a3.errors_total, snap_a3.backpressure_blocks_total,
                snap_b1.items_in_total, snap_b1.items_out_total, snap_b1.errors_total, snap_b1.backpressure_blocks_total,
                snap_b2.items_in_total, snap_b2.items_out_total, snap_b2.errors_total, snap_b2.backpressure_blocks_total,
                snap_b3.items_in_total, snap_b3.items_out_total, snap_b3.errors_total, snap_b3.backpressure_blocks_total,
            );
        }
        OutputMode::Human => {
            println!(
                "axon-bench-pipeline-v2: {} files / {} chunks in {:.1}s\n\
                 → {:.2} files/s · {:.2} chunks/s\n\
                 a1 in/out/err/bp = {}/{}/{}/{}\n\
                 a2 in/out/err/bp = {}/{}/{}/{}\n\
                 a3 in/out/err/bp = {}/{}/{}/{}\n\
                 b1 in/out/err/bp = {}/{}/{}/{}\n\
                 b2 in/out/err/bp = {}/{}/{}/{}\n\
                 b3 in/out/err/bp = {}/{}/{}/{}\n\
                 PG rows: Symbol={} Chunk={} IndexedFile={} ChunkEmbedding={}\n\
                 total source files walked = {}",
                a_count, b_count, elapsed.as_secs_f64(),
                files_per_sec, chunks_per_sec,
                snap_a1.items_in_total, snap_a1.items_out_total, snap_a1.errors_total, snap_a1.backpressure_blocks_total,
                snap_a2.items_in_total, snap_a2.items_out_total, snap_a2.errors_total, snap_a2.backpressure_blocks_total,
                snap_a3.items_in_total, snap_a3.items_out_total, snap_a3.errors_total, snap_a3.backpressure_blocks_total,
                snap_b1.items_in_total, snap_b1.items_out_total, snap_b1.errors_total, snap_b1.backpressure_blocks_total,
                snap_b2.items_in_total, snap_b2.items_out_total, snap_b2.errors_total, snap_b2.backpressure_blocks_total,
                snap_b3.items_in_total, snap_b3.items_out_total, snap_b3.errors_total, snap_b3.backpressure_blocks_total,
                symbol_rows, chunk_rows, indexed_rows, embedding_rows,
                total_files,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_source_collects_rust_files_and_respects_max_files_cap() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..5 {
            std::fs::write(sub.join(format!("f{i}.rs")), "fn x() {}\n").unwrap();
        }
        // Hidden dir and target/ must be skipped.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("hooked.rs"), "ignored").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target").join("compiled.rs"), "ignored").unwrap();
        // Unsupported extension must be skipped.
        std::fs::write(dir.path().join("doc.md"), "ignored").unwrap();

        let files = walk_source(dir.path(), 100).unwrap();
        assert_eq!(files.len(), 5, "exactly 5 .rs files under nested/");
        assert!(files.iter().all(|p| p.extension().and_then(|e| e.to_str()) == Some("rs")));

        let capped = walk_source(dir.path(), 3).unwrap();
        assert_eq!(capped.len(), 3, "max_files cap must clamp the walk");
    }

    #[test]
    fn walk_source_skips_hidden_and_well_known_build_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("node_modules").join("ignored.js"), "x").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "fn k(){}\n").unwrap();

        let files = walk_source(dir.path(), 100).unwrap();
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
            .collect();
        assert!(names.contains(&"kept.rs".to_string()));
        assert!(!names.contains(&"ignored.js".to_string()));
    }
}
