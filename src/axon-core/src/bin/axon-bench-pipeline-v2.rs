//! REQ-AXO-289 S6a — End-to-end streaming pipeline v2 bench.
//!
//! Drives the full v2 topology (A1 → A2 → A3 atomic graph+chunks; FTS
//! content_tsv is back-filled out-of-band by the pgmq tsv_worker
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
//! but still writes through real PostgreSQL (DuckDB fully purged,
//! REQ-AXO-271) — handy for verifying the topology end-to-end without GPU.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use axon_core::graph::GraphStore;
use axon_core::pipeline_v2::{
    const_resolver, spawn_pipeline_a, spawn_pipeline_b_full_multi,
    B2Embedder, ChunkForEmbedding, GpuB2Embedder, NoOpEmbedder, PipelineAWorkerCounts,
    PipelineBWorkerCounts, PipelineChannelCaps, StageSnapshot,
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
    warmup_secs: u64,
    cycle: bool,
    project: String,
    embedder_mode: EmbedderMode,
    output: OutputMode,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut source: PathBuf = std::env::current_dir()?;
        // REQ-AXO-289 S4b' / S6a tuning : operator northstar ≥250 ch/s
        // needs ≥3 000 chunks of material to amortise TensorRT compile
        // (~5 s cold start) and to keep B2's GPU saturated through the
        // full sustained-throughput plateau. At ~20 chunks per source
        // file, --max-files 3000 yields ~60 000 chunks total — way past
        // saturation, lets the operator pick a useful slice.
        let mut max_files: usize = 3000;
        let mut duration_secs: u64 = 0;
        let mut warmup_secs: u64 = 0;
        let mut cycle = false;
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
                "--warmup-secs" => warmup_secs = iter.next().context("--warmup-secs N")?.parse()?,
                "--cycle" => cycle = true,
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
            warmup_secs,
            cycle,
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
                                       [--duration-secs N] [--warmup-secs N] [--cycle] \\\n\
                                       [--gpu|--cpu|--noop] [--project CODE] [--csv|--human]\n\
         \n\
         Modes:\n\
         * Burst (default)       : drain --max-files once then exit. CSV counts the full run.\n\
         * Sustained (--duration-secs N --warmup-secs M [--cycle])\n\
                                  : run for N seconds. After M seconds warmup, snapshot metrics ;\n\
                                    at end-of-bench, report differential = sustained-plateau throughput.\n\
                                    --cycle recycles the file list when exhausted — needed when\n\
                                    --max-files * processing-time-per-file < --duration-secs.\n\
         \n\
         Env: AXON_DEV_DATABASE_URL or DATABASE_URL for non-NoOp modes.\n\
              AXON_B2_BATCH_SIZE (default 64), AXON_B2_BATCH_TIMEOUT_MS (default 200),\n\
              AXON_A1/A2/A3_WORKERS, AXON_B1/B2/B3_WORKERS, AXON_PIPELINE_INTERNAL_CHANNEL_CAP,\n\
              AXON_PIPELINE_A3_TO_B1_BUFFER_CAP."
    );
}

fn walk_source(root: &Path, max_files: usize) -> Result<Vec<PathBuf>> {
    // REQ-AXO-295 Phase 3 — delegate to the canonical Scanner so the
    // bench sees exactly the same file set the production watcher
    // would emit: .gitignore + .axonignore + .axoninclude stack +
    // ignored-directory-segments + supported_extensions config. The
    // previous ad-hoc filter (hard-coded extension list, drop names
    // starting with '.') diverged from the production filter by
    // 5-10× on real repos.
    let project_code = "AXO";
    let scanner = axon_core::scanner::Scanner::new(&root.to_string_lossy(), project_code);
    let mut files = scanner.enumerate_files();
    if files.len() > max_files {
        files.truncate(max_files);
    }
    Ok(files)
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

fn build_store(_mode: EmbedderMode) -> Result<GraphStore> {
    // PostgreSQL is the only supported backend (REQ-AXO-271 / operator
    // directive 2026-05-12). Every embedder mode — including --noop —
    // writes through real PG so the bench characterises production
    // behaviour, never a phantom embedded-store ceiling.
    //
    // REQ-AXO-901626 — explicit override of pg backend resolution so the
    // URL surfaced by `--help` (AXON_DEV_DATABASE_URL / DATABASE_URL) is
    // honoured verbatim. Without `new_with_database`, GraphStore::new
    // ignored the url and fell back to resolve_pg_database_url() which
    // privileges AXON_LIVE_DATABASE_URL when AXON_INSTANCE_KIND is unset
    // — silently routing bench writes to axon_live in any devenv shell
    // that exports both URLs (the canonical dev config).
    let url = std::env::var("AXON_DEV_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .context("set AXON_DEV_DATABASE_URL or DATABASE_URL — PG is the canonical store")?;
    GraphStore::new_with_database(&url, &url)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> ExitCode {
    #[cfg(feature = "tokio-console")]
    console_subscriber::init();

    #[cfg(not(feature = "tokio-console"))]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }

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
    // Honor env counts verbatim. The previous `.max(6)` clamp on A3 made
    // it impossible to characterize the upstream (A2 vs A3) bottleneck:
    // setting AXON_A3_WORKERS=2 to probe A3 capacity was silently lifted
    // to 6, falsifying the isolation experiment the operator wanted.
    // Operator stays responsible for choosing sensible counts.
    let counts_a = PipelineAWorkerCounts::from_env();
    let counts_b = PipelineBWorkerCounts::from_env();

    eprintln!(
        "axon-bench-pipeline-v2: caps={caps:?} a={counts_a:?} b={counts_b:?} \
         b2_batch_size={} b2_batch_timeout_ms={}",
        caps.b2_batch_size, caps.b2_batch_timeout_ms
    );

    let resolver = const_resolver(args.project.clone());
    let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), resolver);

    // Slice 5 SOTA — the bench creates its own b_chunks channel and
    // a synthetic demand_pull task to feed B2/B3. demand_pull does
    // SELECT-with-content from PG (a chunk B2 just persisted via A3).
    let (b_chunks_tx, b_chunks_rx) = tokio::sync::mpsc::channel::<ChunkForEmbedding>(caps.internal);

    let mut handles_b = spawn_pipeline_b_full_multi(
        counts_b,
        caps,
        store.clone(),
        vec![embedder],
        b_chunks_rx,
    );

    // Bench-local demand_pull feeder : poll PG for chunks needing
    // embedding and push them via b_chunks_tx. Mirrors the production
    // demand_pull but without the NOTIFY listener (the bench A3 writes
    // are synchronous and we poll right after).
    {
        let store_for_pull = store.clone();
        let tx = b_chunks_tx.clone();
        tokio::spawn(async move {
            loop {
                let store_clone = store_for_pull.clone();
                let pulled = tokio::task::spawn_blocking(move || {
                    store_clone.select_chunks_with_content_needing_embedding(256)
                })
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or_default();
                if pulled.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                for (chunk_id, content, content_hash) in pulled {
                    let payload = ChunkForEmbedding {
                        chunk_id,
                        content,
                        content_hash,
                    };
                    if tx.send(payload).await.is_err() {
                        return;
                    }
                }
            }
        });
    }
    drop(b_chunks_tx);

    let total_files = files.len();
    let start = Instant::now();
    // A bench is meaningless if the pipeline runs out of material mid-
    // measurement — we'd just measure "no file to process". So when a
    // duration window is set, cycling is implicit : the feeder loops
    // through the walked pool indefinitely, throttled by the A1 input
    // channel's natural backpressure. --cycle is still honoured as an
    // explicit toggle for single-pass-with-duration cases.
    let cycle = args.cycle || args.duration_secs > 0;
    let deadline =
        (args.duration_secs > 0).then(|| Instant::now() + Duration::from_secs(args.duration_secs));

    // Move the sole input_tx into the feeder so its drop closes the
    // channel once feeding stops. Keeping a clone on the stack here
    // would leave A1 workers waiting on recv() forever.
    let input_tx = handles_a.input_tx;
    let feeder_files = files.clone();
    let feeder = tokio::spawn(async move {
        if cycle {
            // Sustained mode : recycle the file list until --duration-secs
            // window elapses (or downstream closes). input_tx.send().await
            // backpressures naturally when A1 is saturated.
            loop {
                for path in &feeder_files {
                    if let Some(d) = deadline {
                        if Instant::now() >= d {
                            return;
                        }
                    }
                    if input_tx.send(path.clone()).await.is_err() {
                        return;
                    }
                }
                if deadline.is_none() {
                    return; // no duration cap + cycle = single pass
                }
            }
        } else {
            for path in feeder_files {
                if input_tx.send(path).await.is_err() {
                    return;
                }
            }
            // input_tx dropped on scope-exit -> A1 recv() returns None ->
            // cascade-drain.
        }
    });

    // Consumer task — drain receipts and count. In sustained mode we
    // snapshot metrics at end-of-warmup and end-of-bench to compute the
    // differential plateau throughput (vs the cold-start period dominated
    // by TensorRT compile).
    let warmup_marker =
        (args.warmup_secs > 0).then(|| Instant::now() + Duration::from_secs(args.warmup_secs));
    let mut warmup_snapshot_a: Option<u64> = None;
    let mut warmup_snapshot_b: Option<u64> = None;
    let mut warmup_marker_t: Option<Instant> = None;

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
        // Capture warmup-end snapshot exactly once.
        if let Some(m) = warmup_marker {
            if warmup_snapshot_a.is_none() && Instant::now() >= m {
                warmup_snapshot_a = Some(a_count as u64);
                warmup_snapshot_b = Some(b_count as u64);
                warmup_marker_t = Some(Instant::now());
                eprintln!(
                    "axon-bench-pipeline-v2: warmup snapshot at {:.1}s — a_count={a_count} b_count={b_count}",
                    args.warmup_secs as f64
                );
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

    // Sustained-plateau differential : substract warmup counts to isolate
    // the steady-state throughput from the cold-start tail.
    let sustained_a = warmup_snapshot_a.map(|w| a_count.saturating_sub(w as usize));
    let sustained_b = warmup_snapshot_b.map(|w| b_count.saturating_sub(w as usize));
    let sustained_elapsed = warmup_marker_t.map(|t| t.elapsed());

    // Post-run sanity counts via the writer ctx — under the embedded
    // test backend the reader ctx serves a stale snapshot during the
    // shutdown window, which makes legitimate Chunk / Symbol writes
    // appear as zero. Writer ctx is authoritative across backends.
    fn writer_count(store: &GraphStore, table: &str) -> i64 {
        let raw = match store.query_json_writer(&format!("SELECT count(*) FROM {table}")) {
            Ok(r) => r,
            Err(_) => return -1,
        };
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.first()
            .and_then(|r| r.first())
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(-1)
    }
    let chunk_rows = writer_count(&store, "Chunk");
    let embedding_rows = writer_count(&store, "ChunkEmbedding");
    let symbol_rows = writer_count(&store, "Symbol");
    let indexed_rows = writer_count(&store, "IndexedFile");

    let snap_a1 = handles_a.metrics_a1.snapshot();
    let snap_a2 = handles_a.metrics_a2.snapshot();
    let snap_a3 = handles_a.metrics_a3.snapshot();
    // Slice 5 SOTA — B1 stage worker collapsed into demand_pull. Keep
    // a zeroed "B1 (collapsed)" snapshot so historical bench CSV/table
    // schemas don't break ; downstream Polars scripts can treat
    // all-zero rows as "stage gone".
    let snap_b1 = axon_core::pipeline_v2::StageMetrics::new("B1").snapshot();
    let snap_b2 = handles_b.metrics_b2.snapshot();
    let snap_b3 = handles_b.metrics_b3.snapshot();

    let files_per_sec = a_count as f64 / elapsed.as_secs_f64().max(0.000_001);
    let chunks_per_sec = b_count as f64 / elapsed.as_secs_f64().max(0.000_001);
    // Sustained-plateau throughput (post-warmup, pre-deadline window).
    // -1.0 when warmup wasn't configured.
    let (sustained_files_per_sec, sustained_chunks_per_sec) =
        match (sustained_a, sustained_b, sustained_elapsed) {
            (Some(sa), Some(sb), Some(el)) => {
                let secs = el.as_secs_f64().max(0.000_001);
                (sa as f64 / secs, sb as f64 / secs)
            }
            _ => (-1.0, -1.0),
        };

    // REQ-AXO-901608 / CPT-AXO-90025 — Goldratt-canonical drum detection.
    // Identify the stage with the highest t_work_ratio across all six stages
    // (capacity-bound = "le drum"). Stages with high t_recv_ratio are
    // starved (machine de décolletage sans barres) ; stages with high
    // t_send_ratio are backpressured (drum aval bouché).
    let snaps_all: [&StageSnapshot; 6] =
        [&snap_a1, &snap_a2, &snap_a3, &snap_b1, &snap_b2, &snap_b3];
    let drum = snaps_all
        .iter()
        .max_by(|a, b| {
            a.t_work_ratio()
                .partial_cmp(&b.t_work_ratio())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap();
    let drum_name = drum.name;
    let drum_ratio = drum.t_work_ratio();

    // Little's Law sanity check per stage : L = λ · W
    // L = items currently in flight (inflight)
    // λ = throughput (items_out / elapsed_secs)
    // W = mean latency (mean_duration_us / 1e6)
    // L_predicted ≈ λ · W ; |L_predicted - L_measured| / L_measured small → steady-state
    // We compute L_predicted ; the human output displays the comparison.
    let elapsed_secs = elapsed.as_secs_f64().max(0.000_001);
    let little_law_l_predicted = |snap: &StageSnapshot| -> f64 {
        let lambda = snap.items_out_total as f64 / elapsed_secs;
        let w_s = snap.mean_duration_us as f64 / 1_000_000.0;
        lambda * w_s
    };

    match args.output {
        OutputMode::Csv => {
            println!(
                "label,files,chunks,elapsed_ms,files_per_sec,chunks_per_sec,\
                 sustained_files_per_sec,sustained_chunks_per_sec,sustained_elapsed_ms,\
                 a1_in,a1_out,a1_err,a1_bp,a2_in,a2_out,a2_err,a2_bp,\
                 a3_in,a3_out,a3_err,a3_bp,b1_in,b1_out,b1_err,b1_bp,\
                 b2_in,b2_out,b2_err,b2_bp,b3_in,b3_out,b3_err,b3_bp,\
                 a1_t_recv_us,a1_t_work_us,a1_t_send_us,\
                 a2_t_recv_us,a2_t_work_us,a2_t_send_us,\
                 a3_t_recv_us,a3_t_work_us,a3_t_send_us,\
                 b1_t_recv_us,b1_t_work_us,b1_t_send_us,\
                 b2_t_recv_us,b2_t_work_us,b2_t_send_us,\
                 b3_t_recv_us,b3_t_work_us,b3_t_send_us,\
                 a1_work_ratio,a2_work_ratio,a3_work_ratio,\
                 b1_work_ratio,b2_work_ratio,b3_work_ratio,\
                 drum_identified,drum_work_ratio,pool_size"
            );
            println!(
                "v2-bench,{},{},{:.0},{:.2},{:.2},{:.2},{:.2},{:.0},\
                 {},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\
                 {},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\
                 {:.4},{:.4},{:.4},{:.4},{:.4},{:.4},\
                 {},{:.4},{}",
                a_count,
                b_count,
                elapsed.as_millis(),
                files_per_sec,
                chunks_per_sec,
                sustained_files_per_sec,
                sustained_chunks_per_sec,
                sustained_elapsed
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(-1.0),
                snap_a1.items_in_total,
                snap_a1.items_out_total,
                snap_a1.errors_total,
                snap_a1.backpressure_blocks_total,
                snap_a2.items_in_total,
                snap_a2.items_out_total,
                snap_a2.errors_total,
                snap_a2.backpressure_blocks_total,
                snap_a3.items_in_total,
                snap_a3.items_out_total,
                snap_a3.errors_total,
                snap_a3.backpressure_blocks_total,
                snap_b1.items_in_total,
                snap_b1.items_out_total,
                snap_b1.errors_total,
                snap_b1.backpressure_blocks_total,
                snap_b2.items_in_total,
                snap_b2.items_out_total,
                snap_b2.errors_total,
                snap_b2.backpressure_blocks_total,
                snap_b3.items_in_total,
                snap_b3.items_out_total,
                snap_b3.errors_total,
                snap_b3.backpressure_blocks_total,
                snap_a1.t_recv_total_us,
                snap_a1.t_work_total_us,
                snap_a1.t_send_total_us,
                snap_a2.t_recv_total_us,
                snap_a2.t_work_total_us,
                snap_a2.t_send_total_us,
                snap_a3.t_recv_total_us,
                snap_a3.t_work_total_us,
                snap_a3.t_send_total_us,
                snap_b1.t_recv_total_us,
                snap_b1.t_work_total_us,
                snap_b1.t_send_total_us,
                snap_b2.t_recv_total_us,
                snap_b2.t_work_total_us,
                snap_b2.t_send_total_us,
                snap_b3.t_recv_total_us,
                snap_b3.t_work_total_us,
                snap_b3.t_send_total_us,
                snap_a1.t_work_ratio(),
                snap_a2.t_work_ratio(),
                snap_a3.t_work_ratio(),
                snap_b1.t_work_ratio(),
                snap_b2.t_work_ratio(),
                snap_b3.t_work_ratio(),
                drum_name,
                drum_ratio,
                total_files,
            );
        }
        OutputMode::Human => {
            // Goldratt drum identification + Little's Law sanity per stage.
            // Three canonical values per operator (machine de décolletage):
            //   pool_size = matière disponible, a_count = matière traitée par A,
            //   b_count = matière traitée par B.
            let row = |snap: &StageSnapshot| -> String {
                let l_pred = little_law_l_predicted(snap);
                format!(
                    "  {:<4} in/out/err/bp = {}/{}/{}/{} · t_recv/work/send μs = {}/{}/{} · t_work_ratio = {:>6.2}% · L_pred ≈ {:>5.2} (inflight={})",
                    snap.name,
                    snap.items_in_total, snap.items_out_total, snap.errors_total, snap.backpressure_blocks_total,
                    snap.t_recv_total_us, snap.t_work_total_us, snap.t_send_total_us,
                    snap.t_work_ratio() * 100.0,
                    l_pred,
                    snap.inflight,
                )
            };
            println!(
                "axon-bench-pipeline-v2: {} files / {} chunks in {:.1}s · pool_size = {} (matière disponible)\n\
                 → wall    : {:.2} files/s · {:.2} chunks/s\n\
                 → sustained (post-warmup): {} files/s · {} chunks/s\n\
                 \n\
                 Goldratt drum (max t_work_ratio): {} @ {:.2}%\n\
                 \n\
                 Per-stage (in/out/err/bp · t_recv/work/send μs · t_work_ratio · Little's Law L_pred):\n\
                 {}\n{}\n{}\n{}\n{}\n{}\n\
                 \n\
                 PG rows: Symbol={} Chunk={} IndexedFile={} ChunkEmbedding={}\n\
                 cycle={cycle} duration_secs={duration_secs} warmup_secs={warmup_secs} pool_size={total_files}",
                a_count, b_count, elapsed.as_secs_f64(), total_files,
                files_per_sec, chunks_per_sec,
                if sustained_files_per_sec >= 0.0 { format!("{:.2}", sustained_files_per_sec) } else { "n/a".to_string() },
                if sustained_chunks_per_sec >= 0.0 { format!("{:.2}", sustained_chunks_per_sec) } else { "n/a".to_string() },
                drum_name, drum_ratio * 100.0,
                row(&snap_a1), row(&snap_a2), row(&snap_a3),
                row(&snap_b1), row(&snap_b2), row(&snap_b3),
                symbol_rows, chunk_rows, indexed_rows, embedding_rows,
                cycle = cycle,
                duration_secs = args.duration_secs,
                warmup_secs = args.warmup_secs,
                total_files = total_files,
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
        // Unsupported extension must be skipped. REQ-AXO-901687 :
        // `.md` is in the canonical supported_extensions list (config.rs
        // default), so we use a truly unsupported binary extension here
        // to keep the test's stated intent intact.
        std::fs::write(dir.path().join("doc.bin"), "ignored").unwrap();

        let files = walk_source(dir.path(), 100).unwrap();
        assert_eq!(files.len(), 5, "exactly 5 .rs files under nested/");
        assert!(files
            .iter()
            .all(|p| p.extension().and_then(|e| e.to_str()) == Some("rs")));

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
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(names.contains(&"kept.rs".to_string()));
        assert!(!names.contains(&"ignored.js".to_string()));
    }
}
