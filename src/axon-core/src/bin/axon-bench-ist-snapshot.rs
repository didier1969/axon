//! REQ-AXO-91485 — Bench for IST cold-load + neighbor traversal.
//!
//! Loads one project's snapshot from PG via the graph_store JSON SQL surface,
//! reports memory footprint + load time, then runs a synthetic micro-bench
//! across N forward-neighbor probes to measure traversal latency.
//!
//! Usage:
//!   axon-bench-ist-snapshot --project AXO [--probes 1000] [--human|--csv]

use std::process::ExitCode;
use std::time::Instant;

use axon_core::graph::GraphStore;
use axon_core::ist_snapshot::loader::{load_snapshot, JsonSqlStore};

struct GraphStoreSqlAdapter<'a> {
    inner: &'a GraphStore,
}

impl<'a> JsonSqlStore for GraphStoreSqlAdapter<'a> {
    fn query_json(&self, sql: &str) -> Result<String, String> {
        self.inner.query_json(sql).map_err(|e| e.to_string())
    }
}

#[derive(Debug)]
struct Args {
    project: String,
    probes: usize,
    output: Output,
    // REQ-AXO-140 demo — print the reverse callers (resolved via the RAM
    // projection) of this symbol name, proving synthetic CALLS targets resolve.
    symbol: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum Output {
    Human,
    Csv,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut project = None;
        let mut probes: usize = 1000;
        let mut output = Output::Human;
        let mut symbol: Option<String> = None;
        let raw = std::env::args().skip(1).collect::<Vec<_>>();
        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--project" => {
                    project = Some(
                        raw.get(i + 1)
                            .ok_or_else(|| anyhow::anyhow!("--project requires value"))?
                            .clone(),
                    );
                    i += 2;
                }
                "--probes" => {
                    probes = raw
                        .get(i + 1)
                        .ok_or_else(|| anyhow::anyhow!("--probes requires value"))?
                        .parse()?;
                    i += 2;
                }
                "--human" => {
                    output = Output::Human;
                    i += 1;
                }
                "--csv" => {
                    output = Output::Csv;
                    i += 1;
                }
                "-h" | "--help" => {
                    println!("axon-bench-ist-snapshot --project CODE [--probes N] [--human|--csv]");
                    std::process::exit(0);
                }
                "--symbol" => {
                    symbol = Some(
                        raw.get(i + 1)
                            .ok_or_else(|| anyhow::anyhow!("--symbol requires value"))?
                            .clone(),
                    );
                    i += 2;
                }
                other => {
                    return Err(anyhow::anyhow!("unknown arg: {}", other));
                }
            }
        }
        Ok(Self {
            project: project.ok_or_else(|| anyhow::anyhow!("--project is required"))?,
            probes,
            output,
            symbol,
        })
    }
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("❌ axon-bench-ist-snapshot: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("❌ axon-bench-ist-snapshot: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> anyhow::Result<()> {
    let db_url = std::env::var("AXON_LIVE_DATABASE_URL")
        .or_else(|_| std::env::var("AXON_DEV_DATABASE_URL"))
        .or_else(|_| std::env::var("DATABASE_URL"))
        .map_err(|_| {
            anyhow::anyhow!("set AXON_LIVE_DATABASE_URL, AXON_DEV_DATABASE_URL or DATABASE_URL")
        })?;
    let graph_store = GraphStore::new(&db_url)?;
    let store = GraphStoreSqlAdapter {
        inner: &graph_store,
    };
    let (graph, stats) = load_snapshot(&store, &args.project)
        .map_err(|e| anyhow::anyhow!("load_snapshot failed: {}", e))?;

    // REQ-AXO-140 demo — show the reverse callers of a symbol, resolved via the
    // RAM projection (synthetic CALLS targets resolved at IstGraph::build). This
    // is what `bidi_trace`/`impact` traverse; on the pre-fix live brain it
    // returns 0 for cross-module callees.
    if let Some(sym) = &args.symbol {
        let needle = format!("::{}", sym);
        let mut found = None;
        for idx in 0..graph.node_count() as u32 {
            let id = graph.id_of(idx);
            if id == sym.as_str() || id.ends_with(&needle) {
                found = Some(idx);
                break;
            }
        }
        match found {
            Some(idx) => {
                let id = graph.id_of(idx).to_string();
                let mut callers: Vec<String> = graph
                    .reverse_neighbors(idx)
                    .map(|(s, _)| graph.id_of(s).to_string())
                    .collect();
                callers.sort();
                callers.dedup();
                println!(
                    "\n=== REQ-AXO-140 — reverse callers of `{}` (id={}) via RAM resolution ===",
                    sym, id
                );
                println!("resolved callers: {}", callers.len());
                for c in &callers {
                    println!("  <- {}", c);
                }
            }
            None => println!("symbol `{}` not found in snapshot", sym),
        }
    }

    let mut samples_us: Vec<u128> = Vec::with_capacity(args.probes);
    if graph.node_count() == 0 {
        // Empty snapshot — emit zero-row report instead of dividing by zero.
        emit(&args, &stats, &samples_us);
        return Ok(());
    }
    let node_count = graph.node_count() as u32;
    for i in 0..args.probes {
        let idx = (i as u32) % node_count;
        let started = Instant::now();
        let count = graph.forward_neighbors(idx).count();
        let elapsed_us = started.elapsed().as_micros();
        samples_us.push(elapsed_us);
        // Force the optimizer to materialize the iteration.
        std::hint::black_box(count);
    }
    emit(&args, &stats, &samples_us);
    Ok(())
}

fn emit(args: &Args, stats: &axon_core::ist_snapshot::loader::LoadStats, samples_us: &[u128]) {
    let (p50, p99) = if samples_us.is_empty() {
        (0, 0)
    } else {
        let mut sorted = samples_us.to_vec();
        sorted.sort_unstable();
        let p50 = sorted[sorted.len() / 2];
        let p99_idx = ((sorted.len() as f64) * 0.99) as usize;
        let p99 = sorted[p99_idx.min(sorted.len() - 1)];
        (p50, p99)
    };
    match args.output {
        Output::Human => {
            println!("project           : {}", stats.project_code);
            println!("nodes_loaded      : {}", stats.nodes_loaded);
            println!("edges_loaded      : {}", stats.edges_loaded);
            println!("load_ms           : {}", stats.load_ms);
            println!(
                "approximate_bytes : {} ({:.1} MB)",
                stats.approximate_bytes,
                (stats.approximate_bytes as f64) / (1024.0 * 1024.0)
            );
            println!("probes            : {}", samples_us.len());
            println!("neighbor_p50_us   : {}", p50);
            println!("neighbor_p99_us   : {}", p99);
        }
        Output::Csv => {
            println!(
                "project,nodes,edges,load_ms,approximate_bytes,probes,neighbor_p50_us,neighbor_p99_us"
            );
            println!(
                "{},{},{},{},{},{},{},{}",
                stats.project_code,
                stats.nodes_loaded,
                stats.edges_loaded,
                stats.load_ms,
                stats.approximate_bytes,
                samples_us.len(),
                p50,
                p99
            );
        }
    }
}
