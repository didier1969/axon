//! Pipeline-v2 wiring for the live `axon-indexer` binary (REQ-AXO-289 S7).
//!
//! This module replaces the legacy `spawn_autonomous_ingestor` +
//! `spawn_ingress_promoter` pair with a thin bridge that:
//!
//! 1. Spawns [`pipeline_v2::spawn_pipeline_a`] (and `spawn_pipeline_b_full`
//!    when the runtime mode enables semantic workers) with a multi-project
//!    resolver (DEC-AXO-081).
//! 2. Performs an initial scan of the watch root via [`Scanner::enumerate_files`]
//!    and feeds every eligible path into pipeline A's `input_tx`.
//! 3. Drains the shared `IngressBuffer` periodically and forwards file
//!    events into the same `input_tx`. The legacy `public.File` state
//!    machine is bypassed entirely — A3's idempotent UPSERTs are the
//!    sole persistence path.
//!
//! All spawns are `tokio::spawn` so the function returns once everything
//! is wired; pipelines run in the background for the lifetime of the
//! process.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::graph::GraphStore;
use crate::ingress_buffer::SharedIngressBuffer;
use crate::pipeline_v2::{
    b1_cold_start_poll, spawn_chunk_pending_listener, spawn_pipeline_a, spawn_pipeline_b_full,
    GpuB2Embedder, NoOpEmbedder, PipelineAWorkerCounts, PipelineBWorkerCounts, PipelineChannelCaps,
};
use crate::runtime_mode::AxonRuntimeMode;
use crate::scanner::Scanner;

const INGRESS_DRAIN_BATCH: usize = 256;
const INGRESS_DRAIN_POLL_MS: u64 = 200;
/// Cadence of the periodic `b1_cold_start_poll` that rattrapes chunks
/// A3's `try_send` dropped under A1→B1 buffer pressure. Conservative
/// default — 30 s — keeps the poll cost negligible (a single
/// `SELECT … LEFT JOIN … LIMIT batch_size` per tick when steady-state)
/// while ensuring the embedding backlog drains promptly when A
/// outpaces B (the common case at boot + large workspaces).
const B1_COLDSTART_POLL_INTERVAL_SECS: u64 = 30;
// REQ-AXO-314 follow-up — read cold-start batch from `caps.b1_coldstart_batch_size`
// at boot (default 4096, env knob `AXON_B1_COLDSTART_BATCH_SIZE`). The
// hardcoded 256-row constant was a dead-knob bug: caps was plumbed but
// never read here, so the env never took effect.

/// Boot the streaming pipeline v2 in the indexer binary.
///
/// Returns once handles are spawned; pipelines run in background tokio
/// tasks for the lifetime of the process. The caller keeps no handle —
/// the pipelines drain ingress until `input_tx` is dropped (never,
/// under normal shutdown via SIGTERM).
pub fn spawn_pipeline_v2_indexer(
    runtime_mode: AxonRuntimeMode,
    store: Arc<GraphStore>,
    ingress_buffer: SharedIngressBuffer,
    watch_root: String,
) -> Result<()> {
    let caps = PipelineChannelCaps::from_env();
    let counts_a = PipelineAWorkerCounts::from_env();

    // DEC-AXO-081 — per-file project_code resolver. Scanner constructed
    // with empty explicit code so it delegates to
    // project_meta::resolve_project_identity_for_path on every call.
    let scanner = Arc::new(Scanner::new(&watch_root, ""));
    let store_for_resolver = store.clone();
    let scanner_for_resolver = scanner.clone();
    let resolver = Arc::new(move |path: &std::path::Path| -> String {
        match scanner_for_resolver.project_code_for_path(&store_for_resolver, path) {
            Ok(code) => code,
            Err(err) => {
                warn!(?path, error = %err, "pipeline_v2: project_code resolution failed, falling back to UNK");
                "UNK".to_string()
            }
        }
    });

    info!(
        "pipeline_v2: spawning pipeline A (a1={} a2={} a3={}) under runtime_mode={}",
        counts_a.a1,
        counts_a.a2,
        counts_a.a3,
        runtime_mode.as_str()
    );
    let mut handles_a = spawn_pipeline_a(counts_a, caps, store.clone(), resolver);

    let b1_inbox_rx = std::mem::replace(
        &mut handles_a.b1_inbox_rx,
        mpsc::channel::<String>(1).1,
    );
    // Keep an extra clone of the same channel for the cold-start poll
    // task — A3 also pushes here via try_send during steady state, but
    // any drop on full buffer must be rattrapé by SELECT … LEFT JOIN …
    // ChunkEmbedding IS NULL (CPT-AXO-054 contract).
    let b1_inbox_tx_for_poll = handles_a.b1_inbox_tx.clone();
    // DEC-AXO-086 slice 1B — third producer of the same B1 inbox :
    // a PG LISTEN task consumes 'chunk_pending_embed' notifications
    // (fired by the trigger on Chunk INSERT/UPDATE OF content_hash) and
    // forwards chunk_ids to B1. Three independent producers (A3 try_send
    // / cold-start poll / NOTIFY listener) converge on the same consumer.
    let b1_inbox_tx_for_listener = handles_a.b1_inbox_tx.clone();

    if runtime_mode.semantic_workers_enabled() {
        let counts_b = PipelineBWorkerCounts::from_env();
        info!(
            "pipeline_v2: spawning pipeline B (b1={} b2={} b3={})",
            counts_b.b1, counts_b.b2, counts_b.b3
        );
        let embedder: Arc<dyn crate::pipeline_v2::B2Embedder> = match GpuB2Embedder::try_new_cuda(
            "indexer-pipeline-v2",
            0,
        ) {
            Ok(e) => Arc::new(e),
            Err(err) => {
                warn!(error = %err, "pipeline_v2: GPU embedder init failed, falling back to NoOpEmbedder");
                Arc::new(NoOpEmbedder)
            }
        };
        let mut handles_b = spawn_pipeline_b_full(counts_b, caps, store.clone(), embedder, b1_inbox_rx);
        // REQ-AXO-314 — keep the receipt rx alive by draining it in a
        // background task. Dropping `handles_b.output_rx` immediately
        // would close the receipt channel; B3 then short-circuits on
        // its first `tx.send(receipt)` failure (stage_b3.rs:185) and
        // returns, cascading back through B2 → B1 → b1_inbox close.
        // Observed symptom: exactly one batch embedded post-boot, then
        // NOTIFY listener loops with "b1_inbox closed".
        let mut output_rx_b = std::mem::replace(
            &mut handles_b.output_rx,
            tokio::sync::mpsc::channel(1).1,
        );
        tokio::spawn(async move {
            while output_rx_b.recv().await.is_some() {}
        });

        // CPT-AXO-054 cold-start poll: every 30 s, sweep public.Chunk
        // for rows without a matching ChunkEmbedding and push their
        // chunk_ids into the same inbox. Rattrape:
        //   * chunks A3 try_send-dropped because the buffer was full
        //     (the operator-validated session-22 cause: bootstrap +
        //     watcher push 40k chunks while B side embeds at ~100 ch/s,
        //     so ~30k chunk_ids overflow per cycle without this poll)
        //   * chunks from previous indexer instances (pre-v2 cut-over)
        //   * any race where B1 fetch raced with A3 commit
        // DEC-AXO-086 slice 1B : spawn the PG NOTIFY listener.
        match resolve_listener_database_url() {
            Ok(url) => {
                spawn_chunk_pending_listener(url, b1_inbox_tx_for_listener);
            }
            Err(err) => {
                warn!(error = %err, "pipeline_v2: PG NOTIFY listener disabled (DATABASE_URL unresolved); cold-start poll remains the safety net");
            }
        }

        let store_for_poll = store.clone();
        let coldstart_batch_size = caps.b1_coldstart_batch_size;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(
                B1_COLDSTART_POLL_INTERVAL_SECS,
            ));
            tick.tick().await; // skip the immediate first tick
            loop {
                tick.tick().await;
                match b1_cold_start_poll(
                    store_for_poll.clone(),
                    b1_inbox_tx_for_poll.clone(),
                    coldstart_batch_size,
                )
                .await
                {
                    Ok(n) if n > 0 => {
                        info!(
                            "pipeline_v2 cold-start poll: forwarded {n} chunk_id(s) to B1"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(error = %err, "pipeline_v2 cold-start poll failed");
                    }
                }
            }
        });
    } else {
        // No B side — keep the inbox alive so A3's try_send never gets
        // a closed-channel error, then drain silently.
        let mut rx = b1_inbox_rx;
        let _ = b1_inbox_tx_for_poll;
        let _ = b1_inbox_tx_for_listener;
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                // Silently drop chunk_ids — there's no B to embed them
                // in IndexerGraph mode. They'll be picked up by
                // pipeline_v2 cold-start poll the next time IndexerFull
                // starts.
            }
        });
    }

    // A's receipts are observability-only; drop the rx side. A3 still
    // commits to PG regardless of receipt consumption.
    let mut output_rx_a = handles_a.output_rx;
    tokio::spawn(async move {
        while output_rx_a.recv().await.is_some() {}
    });

    // Bootstrap scan: enumerate every eligible file under the watch
    // root once at boot and feed pipeline A. Re-runs on every restart;
    // IndexedFile + UPSERT idempotence makes this safe.
    let input_tx_bootstrap = handles_a.input_tx.clone();
    let scanner_bootstrap = scanner.clone();
    let watch_root_bootstrap = watch_root.clone();
    tokio::spawn(async move {
        let files = scanner_bootstrap.enumerate_files();
        let count = files.len();
        info!(
            "pipeline_v2 bootstrap: enumerated {} files under {}",
            count, watch_root_bootstrap
        );
        for path in files {
            if input_tx_bootstrap.send(path).await.is_err() {
                return;
            }
        }
        info!("pipeline_v2 bootstrap: handed {} paths to A1", count);
    });

    // Steady-state drain loop: pull file events from the shared
    // ingress_buffer (watcher pushes here on FS notifications) and
    // forward into pipeline A. Subtree hints are completed silently
    // — full subtree re-scans happen via separate scanner sweeps.
    let input_tx_drain = handles_a.input_tx;
    let ingress_for_drain = ingress_buffer.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(INGRESS_DRAIN_POLL_MS));
        loop {
            tick.tick().await;
            let batch = {
                let mut guard = ingress_for_drain
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                guard.drain_batch(INGRESS_DRAIN_BATCH)
            };
            // REQ-AXO-344 — trace drain throughput so we can correlate
            // Scanner walks (`Nexus Scan Complete: N`) with A1 ingress.
            if !batch.files.is_empty() {
                let sample_path = batch
                    .files
                    .first()
                    .map(|f| f.path.clone())
                    .unwrap_or_default();
                info!(
                    target: "pipeline_v2::drain",
                    "drain: forwarded {} paths to A1 (sample: {})",
                    batch.files.len(),
                    sample_path
                );
            }
            for file_event in batch.files {
                let path = PathBuf::from(file_event.path);
                if input_tx_drain.send(path).await.is_err() {
                    return;
                }
            }
            // Subtree hints: mark them complete so the ingress buffer
            // does not re-emit them on the next tick. Path enumeration
            // for any sub-tree happens via the bootstrap scan + native
            // watcher events, not from hint replay.
            for hint in batch.subtree_hints {
                let mut guard = ingress_for_drain
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                guard.complete_subtree_hint(&hint.path);
            }
        }
    });

    Ok(())
}

/// DEC-AXO-086 slice 1B helper : pick the PostgreSQL connection string
/// for the running instance. Honors `AXON_LIVE_DATABASE_URL` /
/// `AXON_DEV_DATABASE_URL` then `DATABASE_URL`, gated by
/// `AXON_INSTANCE_KIND` (default: live).
fn resolve_listener_database_url() -> Result<String> {
    use crate::postgres::{database_url_for, AxonInstance};
    let kind = std::env::var("AXON_INSTANCE_KIND")
        .unwrap_or_else(|_| "live".to_string())
        .to_lowercase();
    let instance = match kind.as_str() {
        "dev" => AxonInstance::Dev,
        _ => AxonInstance::Live,
    };
    database_url_for(instance)
}
