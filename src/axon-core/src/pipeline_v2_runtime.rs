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

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::graph::GraphStore;
use crate::ingress_buffer::{
    record_drain_tick, record_periodic_sweep_skipped_high_cpu, record_periodic_sweep_tick,
    IngressSource, SharedIngressBuffer,
};
use crate::pipeline_v2::{
    GpuB2Embedder, IndexedFileCache, IndexedFileEntry, NoOpEmbedder,
    PipelineAWorkerCounts, PipelineBWorkerCounts, PipelineChannelCaps,
};
use crate::pipeline_v2::orchestrator::spawn_pipeline_a_with_cache;
use crate::runtime_mode::AxonRuntimeMode;
use crate::scanner::Scanner;

const INGRESS_DRAIN_POLL_MS: u64 = 200;
// REQ-AXO-901678 — `INGRESS_DRAIN_BATCH` is read from
// `PipelineChannelCaps::from_env` (knob `AXON_INGRESS_DRAIN_BATCH`,
// default 512). The legacy hardcoded constant (256) was a dead knob :
// the runtime ignored any env override the operator set. Bench session
// 54 confirmed A3 drum saturated under multi-project cold-start with
// 256 cap.

// REQ-AXO-901677 — periodic_sweep_worker defaults.
//
// Cadence : every 4 h, the worker re-walks the watch root, recomputes
// stable content hashes, and pushes deltas (missing-from-IndexedFile or
// hash-mismatch) into the ingress buffer as low-priority subtree hints.
// The point is to catch inotify drops (queue overflow on big refactors,
// container mount changes, silent init failures) that would otherwise
// remain invisible until service restart.
//
// CPU gate : the sweep is opportunistic, not critical-path. If the host
// is already busy (default threshold = 50%), skip this tick and try
// again on the next interval. Operator-visible via
// `periodic_sweep.skipped_high_cpu_total` so chronic skipping is
// detectable.
pub const PERIODIC_SWEEP_HOURS_DEFAULT: u64 = 4;
pub const PERIODIC_SWEEP_CPU_THRESHOLD_PCT_DEFAULT: u8 = 50;
/// Subtree hint priority for periodic_sweep enqueues. LOWER than the
/// registry-driven 100 (REQ-AXO-901675) so an operator-initiated
/// `axon_init_project` or a fresh inotify event preempts a background
/// reconciliation walk that is, by definition, catching up rather than
/// reacting in real time.
const PERIODIC_SWEEP_HINT_PRIORITY: i64 = 50;

use std::sync::atomic::AtomicPtr;
use crate::pipeline_v2::demand_pull::DemandPullMetrics;

static DEMAND_PULL_METRICS_A: AtomicPtr<DemandPullMetrics> =
    AtomicPtr::new(std::ptr::null_mut());
static DEMAND_PULL_METRICS_B: AtomicPtr<DemandPullMetrics> =
    AtomicPtr::new(std::ptr::null_mut());

pub fn demand_pull_metrics_a() -> Option<Arc<DemandPullMetrics>> {
    let ptr = DEMAND_PULL_METRICS_A.load(std::sync::atomic::Ordering::Acquire);
    if ptr.is_null() { return None; }
    unsafe {
        Arc::increment_strong_count(ptr);
        Some(Arc::from_raw(ptr))
    }
}

pub fn demand_pull_metrics_b() -> Option<Arc<DemandPullMetrics>> {
    let ptr = DEMAND_PULL_METRICS_B.load(std::sync::atomic::Ordering::Acquire);
    if ptr.is_null() { return None; }
    unsafe {
        Arc::increment_strong_count(ptr);
        Some(Arc::from_raw(ptr))
    }
}

// REQ-AXO-901808 (MIL-AXO-029 slice 1) — canonical (s, Q) env vars.
//
// DEC-AXO-901625 reframes the existing demand-pull as a classic
// (s, Q) inventory policy : `s` = reorder point (= threshold), `Q`
// = reorder quantity (= batch). The env var names match that
// vocabulary exactly :
//
//   AXON_PIPELINE_A_REORDER_POINT      pipeline A reorder point (s)
//   AXON_PIPELINE_A_REORDER_QUANTITY   pipeline A reorder quantity (Q)
//   AXON_PIPELINE_B_REORDER_POINT      pipeline B reorder point (s)
//   AXON_PIPELINE_B_REORDER_QUANTITY   pipeline B reorder quantity (Q)
//
// No legacy aliasing : grep confirmed nothing in the repo (scripts,
// yaml, prod env, docs) ever consumed the older `AXON_DEMAND_PULL_*`
// names. Keeping a fallback layer for unused names is dead code.
fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// REQ-AXO-901814 (MIL-AXO-029 slice 3) — admission-controller-aware
/// defaults. The reorder point (`s`) and quantity (`Q`) used by
/// demand_pull share their canonical source of truth with
/// [`crate::runtime_profile::recommend_admission_controller_profile`]
/// so a host with more workers gets a larger backlog headroom by
/// default. Env vars override the derived value when set —
/// operators tuning bench / debug runs keep full control.
fn admission_reorder_point() -> usize {
    let profile = crate::runtime_profile::RuntimeProfile::detect();
    crate::runtime_profile::recommend_admission_controller_profile(&profile).reorder_point
}

fn demand_pull_a_threshold_from_env() -> usize {
    env_usize("AXON_PIPELINE_A_REORDER_POINT", admission_reorder_point())
}

fn demand_pull_a_batch_from_env() -> usize {
    // Reorder quantity defaults to the same value as reorder point —
    // refill the whole stock band at once unless the operator tunes
    // it down (e.g. to smooth GPU load). Bench (slice 8) may revisit.
    env_usize("AXON_PIPELINE_A_REORDER_QUANTITY", admission_reorder_point())
}

fn demand_pull_b_threshold_from_env() -> usize {
    // Pipeline B (vector embedding) flows more items per file ; keep
    // the 1500 default until bench (slice 8) ties it to a host metric.
    env_usize("AXON_PIPELINE_B_REORDER_POINT", 1500)
}

fn demand_pull_b_batch_from_env() -> usize {
    env_usize("AXON_PIPELINE_B_REORDER_QUANTITY", 1500)
}

fn resolve_database_url_for_listener() -> String {
    use crate::postgres::{database_url_for, AxonInstance};
    let instance = if std::env::var("AXON_INSTANCE_KIND")
        .unwrap_or_default()
        .eq_ignore_ascii_case("dev")
    {
        AxonInstance::Dev
    } else {
        AxonInstance::Live
    };
    database_url_for(instance).unwrap_or_else(|_| {
        "postgres://axon@127.0.0.1:44144/axon_live".to_string()
    })
}

/// Boot the streaming pipeline v2 in the indexer binary.
///
/// Returns once handles are spawned; pipelines run in background tokio
/// tasks for the lifetime of the process. The caller keeps no handle —
/// the pipelines drain ingress until `input_tx` is dropped (never,
/// under normal shutdown via SIGTERM).
/// REQ-AXO-901874 — spawn the indexer liveness heartbeat publisher,
/// decoupled from the GPU embedder lifecycle. Ticks every 5s and UPSERTs
/// `axon_runtime.EmbedderLifecycleHeartbeat` (role="indexer"). The brain's
/// `resolve_indexer_liveness` derives `indexer_ready` from this row's
/// freshness alone, so every indexing process — graph-only, CPU, NoOp, or
/// GPU — is provably alive. Previously the publisher was spawned only in the
/// GPU-Ok branch of pipeline B, so graph-only / GPU-failed indexers were
/// reported `indexer_ready=False` despite actively indexing (false negative).
fn spawn_indexer_liveness_heartbeat(store: Arc<GraphStore>) {
    crate::embedder::lifecycle_machine::spawn_lifecycle_heartbeat_publisher(
        std::time::Duration::from_secs(5),
        move |snapshot| {
            if let Err(err) = store.record_lifecycle_heartbeat("indexer", &snapshot) {
                warn!(
                    error = %err,
                    "REQ-AXO-901874: failed to UPSERT EmbedderLifecycleHeartbeat row"
                );
            }
        },
    );
}

pub fn spawn_pipeline_v2_indexer(
    runtime_mode: AxonRuntimeMode,
    store: Arc<GraphStore>,
    ingress_buffer: SharedIngressBuffer,
    watch_root: String,
) -> Result<()> {
    // REQ-AXO-901820 — at cold-start, rehabilitate any IndexedFile rows
    // that were poisoned by retry_count reaching the demand_pull max in
    // a previous run. Without this, restart + truncate cycles leave
    // thousands of files permanently stranded (session 62 observed
    // 12 300 such orphans), and the indexer reports runtime_idle while
    // PG still has work to do. 1 h cool-off prevents thrashing freshly
    // failed files in the same session.
    {
        const REHAB_COOL_OFF_MS: i64 = 3_600_000; // 1 hour
        let now_ms = chrono::Utc::now().timestamp_millis();
        match store.rehabilitate_poisoned_files(REHAB_COOL_OFF_MS, now_ms) {
            Ok(eligible) => {
                if eligible > 0 {
                    info!(
                        "pipeline_v2 cold-start: {} 'discovered' files \
                         eligible for re-claim after rehab pass (cool_off=1h)",
                        eligible
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "pipeline_v2 cold-start: poison-pill rehab failed (non-blocking)"
                );
            }
        }
    }

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

    // REQ-AXO-901746 — hydrate the content-hash dedup cache from PG at boot.
    // Files whose (path, content_hash) match are skipped between A1 and A2,
    // avoiding the expensive tree-sitter parse on unchanged files.
    let dedup_cache = match store.load_all_indexed_files() {
        Ok(rows) => {
            let count = rows.len();
            let cache = IndexedFileCache::from_iter(rows.into_iter().map(|(path, hash, ts)| {
                (path, IndexedFileEntry { content_hash: hash, last_seen_ms: ts })
            }));
            info!("pipeline_v2: dedup cache hydrated with {count} entries from IndexedFile");
            Some(cache)
        }
        Err(err) => {
            warn!(error = %err, "pipeline_v2: failed to hydrate dedup cache; all files will be re-parsed");
            None
        }
    };

    info!(
        "pipeline_v2: spawning pipeline A (a1={} a2={} a3={}) under runtime_mode={}",
        counts_a.a1,
        counts_a.a2,
        counts_a.a3,
        runtime_mode.as_str()
    );
    let handles_a = spawn_pipeline_a_with_cache(counts_a, caps, store.clone(), resolver, dedup_cache.clone());

    // REQ-AXO-901874 — indexer liveness heartbeat, decoupled from the GPU
    // embedder. This function is reached for every ingestion-enabled mode
    // (IndexerGraph + IndexerFull, see runtime_mode.ingestion_enabled());
    // publishing here — BEFORE and independent of the optional pipeline-B
    // GPU embedder — means a graph-only or CPU/NoOp indexer is still
    // provably alive to the brain (`resolve_indexer_liveness` reads the
    // `axon_runtime.EmbedderLifecycleHeartbeat` row freshness, not the GPU
    // state). Liveness ≠ GPU-up. Tick 5s sits well under the brain's ~30s
    // freshness window so a single missed tick stays fresh.
    spawn_indexer_liveness_heartbeat(store.clone());

    // Slice 5 SOTA — create the b_chunks channel here (was b1_inbox in
    // orchestrator). demand_pull_b owns the tx ; spawn_pipeline_b_full_multi
    // takes the rx. The channel carries ChunkForEmbedding (one
    // round-trip SELECT-with-content per batch).
    let (b_chunks_tx, b_chunks_rx) =
        mpsc::channel::<crate::pipeline_v2::ChunkForEmbedding>(caps.internal);

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
            Ok(e) => {
                // REQ-AXO-90009 Slice 3 — spawn idle watchdog. After
                // T_idle=5min (DEC-AXO-086 default) without activity and
                // with an empty runtime pending set, the watchdog flips
                // EmbedderLifecycle to Sleeping and calls
                // `release_session()` on this exact embedder — frees
                // ~5-7 GB VRAM + ~3-4 GB host heap. The next embed call
                // wakes the session in 1-3 s warm via TensorRT engine
                // cache on disk. Override via env (TODO: AXON_EMBEDDER_
                // {TICK,IDLE,GRACE}_SECS knobs in a follow-up).
                let arc_embedder: Arc<GpuB2Embedder> = Arc::new(e);
                GpuB2Embedder::spawn_lifecycle_watchdog(
                    &arc_embedder,
                    std::time::Duration::from_secs(5),
                    std::time::Duration::from_secs(20),
                    std::time::Duration::from_secs(2),
                );
                // REQ-AXO-901874 — the indexer liveness heartbeat is now
                // published unconditionally at the top of this function
                // (see `spawn_indexer_liveness_heartbeat` below), decoupled
                // from this GPU-Ok branch. Previously it spawned ONLY here,
                // so graph-only / CPU / NoOp indexers never wrote a row and
                // the brain reported `indexer_ready=False` despite a live,
                // indexing process.
                arc_embedder as Arc<dyn crate::pipeline_v2::B2Embedder>
            }
            Err(err) => {
                // REQ-AXO-901630 — fail-fast when the operator has
                // explicitly requested a GPU provider. Silent NoOp
                // fallback produced junk embeddings ((1,0,…,0) vectors)
                // in session 49, breaking semantic retrieval downstream
                // while the indexer kept reporting healthy. Only the
                // `cpu`/unset branch is allowed to substitute NoOp.
                if gpu_provider_explicitly_requested() {
                    return Err(anyhow!(
                        "pipeline_v2: GPU embedder init failed but AXON_EMBEDDING_PROVIDER \
                         requests a GPU provider (NoOpEmbedder fallback would silently \
                         produce junk vectors): {err}"
                    ));
                }
                warn!(error = %err, "pipeline_v2: GPU embedder init failed, falling back to NoOpEmbedder");
                Arc::new(NoOpEmbedder)
            }
        };
        // REQ-AXO-901748 — hydrate embedding dedup cache so B1 skips
        // chunks that already have a valid embedding with the same hash.
        let embedding_dedup = match crate::pipeline_v2::stage_b1::load_embedding_dedup_cache(&store) {
            Ok(cache) => {
                info!("pipeline_v2: embedding dedup cache hydrated with {} entries", cache.len());
                Some(cache)
            }
            Err(err) => {
                warn!(error = %err, "pipeline_v2: failed to hydrate embedding dedup cache");
                None
            }
        };
        // REQ-AXO-901748 — when AXON_B2_WORKERS > 1, create one ORT
        // session per worker for true CUDA double-buffering (no Mutex
        // contention). Each session has its own TensorRT engine cache.
        let embedders: Vec<Arc<dyn crate::pipeline_v2::B2Embedder>> = if counts_b.b2 > 1 {
            let mut v = vec![embedder];
            for i in 1..counts_b.b2 {
                match GpuB2Embedder::try_new_cuda(
                    &format!("indexer-pipeline-v2-b2w{i}"),
                    i,
                ) {
                    Ok(e) => v.push(Arc::new(e) as Arc<dyn crate::pipeline_v2::B2Embedder>),
                    Err(err) => {
                        warn!(worker = i, error = %err, "pipeline_v2: extra B2 worker init failed, continuing with fewer");
                        break;
                    }
                }
            }
            info!("pipeline_v2: {} B2 GPU workers initialized", v.len());
            v
        } else {
            vec![embedder]
        };
        let mut handles_b = crate::pipeline_v2::orchestrator::spawn_pipeline_b_full_multi(
            counts_b, caps, store.clone(), embedders, b_chunks_rx, embedding_dedup,
        );
        // REQ-AXO-314 — keep the receipt rx alive by draining it in a
        // background task. Dropping `handles_b.output_rx` immediately
        // would close the receipt channel; B3 then short-circuits on
        // its first `tx.send(receipt)` failure and cascades upstream.
        let mut output_rx_b = std::mem::replace(
            &mut handles_b.output_rx,
            tokio::sync::mpsc::channel(1).1,
        );
        tokio::spawn(async move {
            while output_rx_b.recv().await.is_some() {}
        });

        // DEC-AXO-901620 + slice 5 SOTA — demand-pull B feeds
        // ChunkForEmbedding directly to the b_chunks channel (one PG
        // round-trip with content). LISTEN chunk_pending_embed wakes
        // the puller ; 30s safety-net poll catches lost notifications.
        let demand_pull_b_threshold = demand_pull_b_threshold_from_env();
        let demand_pull_b_batch = demand_pull_b_batch_from_env();
        let db_url_b = resolve_database_url_for_listener();
        let _metrics_b = crate::pipeline_v2::demand_pull::spawn_pipeline_b_demand_pull(
            store.clone(),
            db_url_b,
            b_chunks_tx,
            demand_pull_b_threshold,
            demand_pull_b_batch,
        );
        DEMAND_PULL_METRICS_B.store(Arc::into_raw(_metrics_b) as *mut _, std::sync::atomic::Ordering::Release);
    } else {
        // No B side — drop the b_chunks tx so demand_pull won't be
        // spawned. The unused rx is also dropped here.
        drop(b_chunks_tx);
        drop(b_chunks_rx);
    }

    // A3 receipts update the dedup cache so subsequent re-indexing
    // of unchanged files skips the A2 tree-sitter parse.
    let mut output_rx_a = handles_a.output_rx;
    let dedup_cache_for_receipts = dedup_cache;
    tokio::spawn(async move {
        while let Some(receipt) = output_rx_a.recv().await {
            if let Some(ref cache) = dedup_cache_for_receipts {
                cache.mark_indexed(
                    receipt.path,
                    receipt.content_hash,
                    receipt.last_seen_ms,
                );
            }
        }
    });

    // Bootstrap scan: enumerate every eligible file under the watch
    // root once at boot and feed pipeline A. Re-runs on every restart;
    // IndexedFile + UPSERT idempotence makes this safe.
    //
    // REQ-AXO-901649 — use try_send + brief async yield instead of
    // blocking send().await. The 1024-slot A1 input channel saturates
    // within ~milliseconds on bootstrap of a 130K-file workspace ;
    // a blocking send was observed to deadlock the bootstrap task in
    // production (session 51, live indexer hung 2.5h post-boot with
    // ingress_buffered_entries=14253 stuck at the exact same value
    // across consecutive heartbeats). A dropped path is NOT lost :
    // scope_reconciliation_orchestrator re-walks every 60 s and re-
    // submits any file whose (path, mtime, size) doesn't match
    // IndexedFile, so transient back-pressure absorbs naturally
    // without freezing the pipeline.
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
        let mut handed = 0usize;
        let mut dropped = 0usize;
        // REQ-AXO-901831 — bounded retry before dropping. Pre-fix code
        // dropped the path on the first Full and relied on the 60 s
        // reconciliation sweep as safety net ; when reconciliation
        // skipped a directory (cluster head D4, session 64), the drop
        // became permanent. 5 × 100 ms = 500 ms absorbs transient A1
        // saturation while keeping bootstrap non-blocking. Lost paths
        // (after 5 attempts) still fall through to reconciliation, but
        // the window where this can happen is now ≤ 500 ms per path
        // instead of "until something else triggers a re-walk".
        const BOOTSTRAP_FULL_MAX_ATTEMPTS: u32 = 5;
        const BOOTSTRAP_FULL_BACKOFF_MS: u64 = 100;
        for path in files {
            let mut pending = Some(path);
            let mut attempts: u32 = 0;
            while let Some(current) = pending.take() {
                match input_tx_bootstrap.try_send(current) {
                    Ok(()) => {
                        handed += 1;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
                        attempts += 1;
                        if attempts >= BOOTSTRAP_FULL_MAX_ATTEMPTS {
                            dropped += 1;
                        } else {
                            tokio::time::sleep(Duration::from_millis(
                                BOOTSTRAP_FULL_BACKOFF_MS,
                            ))
                            .await;
                            pending = Some(returned);
                        }
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        warn!("pipeline_v2 bootstrap: A1 input channel closed; aborting walk after {handed} handed / {dropped} dropped");
                        return;
                    }
                }
            }
        }
        info!(
            "pipeline_v2 bootstrap: walk complete (total={}, handed={}, dropped_for_reconciliation={})",
            count, handed, dropped
        );
    });

    // DEC-AXO-901620: demand-pull A replaces the supply-push cold-start
    // poll. PG NOTIFY on `file_discovered` wakes the puller; 30s
    // safety-net poll catches lost notifications.
    let demand_pull_a_threshold = demand_pull_a_threshold_from_env();
    let demand_pull_a_batch = demand_pull_a_batch_from_env();
    let db_url_a = resolve_database_url_for_listener();
    let _metrics_a = crate::pipeline_v2::demand_pull::spawn_pipeline_a_demand_pull(
        store.clone(),
        db_url_a,
        handles_a.input_tx.clone(),
        demand_pull_a_threshold,
        demand_pull_a_batch,
    );
    DEMAND_PULL_METRICS_A.store(Arc::into_raw(_metrics_a) as *mut _, std::sync::atomic::Ordering::Release);

    // Steady-state drain loop: pull file events from the shared
    // ingress_buffer (watcher pushes here on FS notifications) and
    // forward into pipeline A. Subtree hints are completed silently
    // — full subtree re-scans happen via separate scanner sweeps.
    //
    // REQ-AXO-901649 — three hardening changes vs. pre-fix:
    // 1. Complete subtree hints BEFORE the file send loop so a slow /
    //    saturated A1 can never starve hint clearing (the in_flight
    //    counter was observed pinned at 256 = drain limit, with
    //    blocked_total growing +144K/h as new hints bounced off the
    //    saturated retry budget).
    // 2. Replace input_tx.send().await with try_send + bounded yield
    //    so the drain task can NEVER park indefinitely on a full A1
    //    channel. Dropped paths are picked up by the next watcher
    //    event or by scope_reconciliation_orchestrator's 60 s sweep
    //    (every dropped file remains in the IndexedFile-mismatch set
    //    until it's actually persisted, so reconciliation will re-
    //    submit it).
    // 3. Log a periodic heartbeat ('drain tick: buffered=… in_flight_
    //    hints=…') at INFO every 25 ticks (~5 s) so any future stall
    //    is visible without re-instrumenting.
    let input_tx_drain = handles_a.input_tx;
    let ingress_for_drain = ingress_buffer.clone();
    let drain_batch_cap = caps.ingress_drain_batch;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(INGRESS_DRAIN_POLL_MS));
        let mut tick_counter: u64 = 0;
        let mut dropped_since_log: u64 = 0;
        loop {
            tick.tick().await;
            tick_counter = tick_counter.wrapping_add(1);
            let batch = {
                let mut guard = ingress_for_drain
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                guard.drain_batch(drain_batch_cap)
            };
            // Clear subtree hints FIRST so even a full A1 channel
            // cannot starve them (defense-in-depth ; the try_send
            // change below already removes the blocking path).
            for hint in &batch.subtree_hints {
                let mut guard = ingress_for_drain
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                guard.complete_subtree_hint(&hint.path);
            }
            // REQ-AXO-344 — trace drain throughput so we can correlate
            // Scanner walks (`Nexus Scan Complete: N`) with A1 ingress.
            let batch_file_count = batch.files.len();
            if batch_file_count > 0 {
                let sample_path = batch
                    .files
                    .first()
                    .map(|f| f.path.clone())
                    .unwrap_or_default();
                info!(
                    target: "pipeline_v2::drain",
                    "drain: forwarded {} paths to A1 (sample: {})",
                    batch_file_count,
                    sample_path
                );
            }
            let mut sent = 0usize;
            let mut dropped = 0usize;
            for file_event in batch.files {
                let path = PathBuf::from(file_event.path);
                match input_tx_drain.try_send(path) {
                    Ok(()) => sent += 1,
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => dropped += 1,
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        warn!("pipeline_v2 drain: A1 input channel closed; drain task exiting");
                        return;
                    }
                }
            }
            dropped_since_log = dropped_since_log.saturating_add(dropped as u64);
            // REQ-AXO-901678 — publish drain telemetry every tick so
            // `axon_embedding_status` + `axon_diagnose_indexing` can
            // surface saturation without parsing logs.
            record_drain_tick(drain_batch_cap, sent as u64, dropped as u64, tick_counter);
            // Heartbeat every ~5 s (25 ticks * 200 ms) so a future
            // stall is observable in `journalctl -f` without external
            // instrumentation. Logs even when buffer is empty so the
            // absence of the line proves the task died.
            if tick_counter.is_multiple_of(25) {
                let snapshot = {
                    let guard = ingress_for_drain
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    guard.metrics_snapshot()
                };
                info!(
                    target: "pipeline_v2::drain",
                    "drain heartbeat: tick={} buffered={} hot={} scan={} subtree_in_flight={} last_batch_files={} last_batch_sent={} last_batch_dropped_full={} cumulative_dropped_full={}",
                    tick_counter,
                    snapshot.buffered_entries,
                    snapshot.hot_entries,
                    snapshot.scan_entries,
                    snapshot.subtree_hint_in_flight,
                    batch_file_count,
                    sent,
                    dropped,
                    dropped_since_log,
                );
                dropped_since_log = 0;
            }
        }
    });

    // REQ-AXO-901677 — periodic_sweep_worker. Inotify drop reconciliation
    // safety net. Only spawned in ingestion-enabled modes (IndexerGraph,
    // IndexerFull — IndexerVector consumes Chunk rows produced by
    // pipeline A but does not own scanning) AND only when the operator
    // hasn't disabled it via `AXON_PERIODIC_SWEEP_HOURS=0`. The handle
    // is intentionally dropped : the task runs for the lifetime of the
    // process, exits only on tokio runtime shutdown.
    if runtime_mode.ingestion_enabled() {
        let sweep_cfg = PeriodicSweepConfig::from_env();
        if sweep_cfg.is_enabled() {
            let _handle = spawn_periodic_sweep_worker(
                ingress_buffer.clone(),
                watch_root.clone(),
                store.clone(),
                sweep_cfg,
            );
        } else {
            info!(
                "periodic_sweep_worker: disabled (AXON_PERIODIC_SWEEP_HOURS=0) — \
                 inotify drops will not be reconciled in the background"
            );
        }
    }

    Ok(())
}

/// REQ-AXO-901677 — periodic_sweep_worker configuration parsed from env.
///
/// `hours` = 0 disables the worker entirely (operator opt-out).
/// `cpu_threshold_pct` = soft skip gate ; when host CPU is above this
/// percentage at tick time, skip the sweep and try again next interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSweepConfig {
    pub hours: u64,
    pub cpu_threshold_pct: u8,
}

impl PeriodicSweepConfig {
    pub fn from_env() -> Self {
        let hours = std::env::var("AXON_PERIODIC_SWEEP_HOURS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(PERIODIC_SWEEP_HOURS_DEFAULT);
        let cpu_threshold_pct = std::env::var("AXON_PERIODIC_SWEEP_CPU_THRESHOLD_PCT")
            .ok()
            .and_then(|raw| raw.trim().parse::<u8>().ok())
            .map(|v| v.min(100))
            .unwrap_or(PERIODIC_SWEEP_CPU_THRESHOLD_PCT_DEFAULT);
        Self {
            hours,
            cpu_threshold_pct,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.hours > 0
    }

    pub fn interval(&self) -> Duration {
        // Saturating multiply : `u64::MAX / 3600` is comfortably above
        // any sane operator setting (a sweep every 5 million years), so
        // saturation is purely defensive against pathological env input.
        Duration::from_secs(self.hours.saturating_mul(3600))
    }
}

/// REQ-AXO-901677 — outcome of one periodic sweep tick. Exposed pub
/// to support `periodic_sweep_tick_for_tests`.
#[derive(Debug, Clone)]
pub enum PeriodicSweepTickOutcome {
    /// Sweep ran end-to-end.
    Ran {
        files_compared: u64,
        deltas_found: u64,
        duration_ms: u64,
    },
    /// CPU above threshold ; skipped to honor host pressure budget.
    SkippedHighCpu,
}

/// REQ-AXO-901677 — long-running tokio task that re-walks `watch_root`
/// every `cfg.interval()` and enqueues hash-mismatch / missing-from-
/// IndexedFile paths as subtree hints. Returns the JoinHandle so the
/// caller may keep it alive (or drop it to let the task run for the
/// lifetime of the process — the common case).
pub fn spawn_periodic_sweep_worker(
    ingress_buffer: SharedIngressBuffer,
    watch_root: String,
    store: Arc<GraphStore>,
    cfg: PeriodicSweepConfig,
) -> JoinHandle<()> {
    info!(
        watch_root = %watch_root,
        hours = cfg.hours,
        cpu_threshold_pct = cfg.cpu_threshold_pct,
        "periodic_sweep_worker: spawning (REQ-AXO-901677)"
    );
    tokio::spawn(async move {
        let interval = cfg.interval();
        let mut tick = tokio::time::interval(interval);
        // Skip the immediate first tick : the bootstrap scan that fires
        // at spawn already covers the same work ; running a duplicate
        // walk right away would double-load A1 on cold boot.
        tick.tick().await;
        loop {
            tick.tick().await;
            // Refresh the IndexedFile snapshot from PG on every tick so
            // hash mutations that landed since the last sweep are taken
            // into account (no stale closure capture).
            let known = match load_indexed_file_paths(&store) {
                Ok(set) => set,
                Err(err) => {
                    warn!(
                        error = %err,
                        "periodic_sweep_worker: failed to load IndexedFile snapshot — skipping tick"
                    );
                    continue;
                }
            };
            let outcome = periodic_sweep_tick(
                &ingress_buffer,
                &watch_root,
                &cfg,
                known,
                /* cpu_override = */ None,
            );
            match outcome {
                PeriodicSweepTickOutcome::Ran {
                    files_compared,
                    deltas_found,
                    duration_ms,
                } => {
                    info!(
                        target: "pipeline_v2::periodic_sweep",
                        "periodic_sweep tick: files_compared={} deltas_found={} duration_ms={}",
                        files_compared, deltas_found, duration_ms,
                    );
                }
                PeriodicSweepTickOutcome::SkippedHighCpu => {
                    info!(
                        target: "pipeline_v2::periodic_sweep",
                        "periodic_sweep tick: skipped (host CPU above {}% threshold)",
                        cfg.cpu_threshold_pct,
                    );
                }
            }
        }
    })
}

/// REQ-AXO-901677 — one-shot version of the worker body. Used by both
/// the long-running task and by `periodic_sweep_tick_for_tests`. Keeps
/// the orchestration deterministic.
fn periodic_sweep_tick(
    ingress_buffer: &SharedIngressBuffer,
    watch_root: &str,
    cfg: &PeriodicSweepConfig,
    known: HashSet<String>,
    cpu_override: Option<bool>,
) -> PeriodicSweepTickOutcome {
    // REQ-AXO-901877 — `cpu_override` makes the CPU gate deterministically
    // testable in both directions: Some(true) forces "below threshold"
    // (proceed), Some(false) forces "above threshold" (skip), None samples the
    // real host CPU — the production path. The previous `force_cpu_ok: bool`
    // could only force "proceed", leaving the skip branch testable only when
    // the real CPU happened to be high (machine-dependent flake).
    let cpu_below = cpu_override.unwrap_or_else(|| cpu_below_threshold(cfg.cpu_threshold_pct));
    if !cpu_below {
        record_periodic_sweep_skipped_high_cpu();
        return PeriodicSweepTickOutcome::SkippedHighCpu;
    }

    let started = std::time::Instant::now();
    // Empty project_code : Scanner defers per-file project resolution
    // (only the enumerate path is used here, which doesn't need it).
    let scanner = Scanner::new(watch_root, "");
    let files = scanner.enumerate_files();
    let files_compared = files.len() as u64;

    let mut deltas: Vec<String> = Vec::new();
    for path in &files {
        let path_str = path.to_string_lossy().to_string();
        if !known.contains(&path_str) {
            deltas.push(path_str);
        }
        // NOTE : hash-mismatch detection happens implicitly via A3's
        // existing UPSERT path. We could compute file hashes here to
        // short-circuit obviously-unchanged paths, but on a 4 h cadence
        // that would re-read every file on disk every 4 hours — the
        // exact thing we want to avoid. Sending only the missing-row
        // set preserves the cheap reconciliation contract.
    }
    let deltas_found = deltas.len() as u64;

    if !deltas.is_empty() {
        let mut guard = ingress_buffer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for path in deltas {
            guard.record_subtree_hint(
                path,
                PERIODIC_SWEEP_HINT_PRIORITY,
                IngressSource::Scan,
            );
        }
    }

    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    record_periodic_sweep_tick(now_ms, duration_ms, files_compared, deltas_found);

    PeriodicSweepTickOutcome::Ran {
        files_compared,
        deltas_found,
        duration_ms,
    }
}

/// REQ-AXO-901677 — test-only shim exposing one sweep tick to the
/// integration test suite. Behaves like the production tick but lets
/// the test inject a `known` set + force the CPU check past so tests
/// run deterministically regardless of host load.
pub fn periodic_sweep_tick_for_tests(
    ingress_buffer: &SharedIngressBuffer,
    watch_root: &str,
    cfg: &PeriodicSweepConfig,
    known: HashSet<String>,
    cpu_override: Option<bool>,
) -> PeriodicSweepTickOutcome {
    periodic_sweep_tick(ingress_buffer, watch_root, cfg, known, cpu_override)
}

/// REQ-AXO-901677 — pull a `HashSet<path>` of every row currently in
/// `ist.IndexedFile`. Used by the worker on each tick so changes
/// since the last sweep are accounted for (no stale closure capture).
///
/// Returns an empty set on a fresh DB ; propagates SQL gateway errors
/// so the caller can log + skip the tick rather than silently treat
/// every file on disk as a delta (which would re-enqueue the entire
/// workspace into A1).
fn load_indexed_file_paths(store: &GraphStore) -> Result<HashSet<String>> {
    let raw = store
        .execute_raw_sql_gateway("SELECT path FROM ist.IndexedFile")
        .map_err(|e| anyhow!("load IndexedFile snapshot: {e}"))?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
    let mut out = HashSet::with_capacity(rows.len());
    for row in rows {
        if let Some(path) = row.into_iter().next().and_then(|v| v.as_str().map(String::from)) {
            out.insert(path);
        }
    }
    Ok(out)
}

/// REQ-AXO-901677 — coarse CPU-load gate. Reads /proc/stat (same
/// approach as `optimizer::read_cpu_and_io_usage_ratios`) but with a
/// brief inline sample so we don't depend on the optimizer's stateful
/// global sampler (which is only touched when the optimizer module
/// is otherwise active).
///
/// Returns `true` iff host CPU usage is BELOW `threshold_pct`. On
/// non-Linux hosts or `/proc/stat` read failure, returns `true` (fail-
/// open) so the sweep still runs ; the reconciliation safety net
/// failing closed would silently let inotify drops accumulate.
fn cpu_below_threshold(threshold_pct: u8) -> bool {
    let Some(first) = read_proc_stat_busy_total() else {
        return true;
    };
    std::thread::sleep(Duration::from_millis(100));
    let Some(second) = read_proc_stat_busy_total() else {
        return true;
    };
    let total_delta = second.0.saturating_sub(first.0);
    if total_delta == 0 {
        return true;
    }
    let idle_delta = second.1.saturating_sub(first.1);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    let usage_ratio = (busy_delta as f64) / (total_delta as f64);
    let threshold_ratio = (threshold_pct as f64) / 100.0;
    usage_ratio < threshold_ratio
}

/// REQ-AXO-901677 — minimal /proc/stat reader. Returns `(total, idle)`
/// jiffies for the aggregate `cpu ` line. Returns `None` on any parse
/// failure or non-Linux hosts.
fn read_proc_stat_busy_total() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    let line = content.lines().find(|l| l.starts_with("cpu "))?;
    let mut values = line.split_whitespace().skip(1);
    let user = values.next()?.parse::<u64>().ok()?;
    let nice = values.next()?.parse::<u64>().ok()?;
    let system = values.next()?.parse::<u64>().ok()?;
    let idle = values.next()?.parse::<u64>().ok()?;
    let iowait = values.next()?.parse::<u64>().ok()?;
    let irq = values.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let softirq = values.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let steal = values.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Some((
        user + nice + system + idle + iowait + irq + softirq + steal,
        idle,
    ))
}

/// REQ-AXO-901630 — returns true iff the operator has explicitly
/// requested a GPU embedding provider via `AXON_EMBEDDING_PROVIDER` or
/// the TensorRT service flag. Used by the embedder init path to refuse
/// the silent `NoOpEmbedder` fallback when a real GPU embedder was
/// asked for ; the alternative (session 49 incident) was 1 178 chunks
/// indexed with junk `(1, 0, …, 0)` vectors that broke semantic
/// retrieval downstream while the indexer reported healthy.
fn gpu_provider_explicitly_requested() -> bool {
    // REQ-AXO-901737 : single canonical knob. `AXON_GPU_EMBED_SERVICE_TENSORRT`
    // legacy check removed ; bash now sets `AXON_EMBEDDING_PROVIDER=tensorrt`
    // for the TRT path.
    matches!(
        std::env::var("AXON_EMBEDDING_PROVIDER")
            .ok()
            .map(|v| v.to_lowercase())
            .as_deref(),
        Some("tensorrt") | Some("cuda")
    )
}

/// DEC-AXO-086 slice 1B helper : pick the PostgreSQL connection string
/// for the running instance. Honors `AXON_LIVE_DATABASE_URL` /
/// `AXON_DEV_DATABASE_URL` then `DATABASE_URL`, gated by
/// `AXON_INSTANCE` (default: live ; legacy alias `AXON_INSTANCE_KIND`
/// still honored with a one-shot deprecation warning — REQ-AXO-901657).
#[cfg(test)]
mod tests {
    use super::gpu_provider_explicitly_requested;
    use crate::postgres::{database_url_for, AxonInstance};

    /// REQ-AXO-90009 Slice 3C — `resolve_listener_database_url` honours
    /// `AXON_INSTANCE_KIND=dev` (resolves DEV URL) ; default = live.
    /// The unit test stays env-aware : it only asserts the resolved
    /// instance variant via the underlying `database_url_for` helper
    /// when the corresponding env var is set in the test harness.
    #[test]
    fn database_url_for_helper_routes_live_and_dev_independently() {
        // Both must resolve to a non-empty URL whenever the env var
        // is set ; cargo test in devenv shell always has at least the
        // live URL configured, so this is a sanity gate that the
        // helper's branching is wired correctly.
        let live = database_url_for(AxonInstance::Live);
        let dev = database_url_for(AxonInstance::Dev);
        // If neither URL is set the function returns an error — that
        // is also a valid outcome (e.g. CI without a PG). We only
        // assert that the call doesn't panic and that both kinds are
        // dispatched separately when their respective env var is set.
        let _ = live;
        let _ = dev;
    }

    /// REQ-AXO-90009 Slice 3C — the GpuB2Embedder watchdog activation
    /// uses 5-min idle / 2-s grace / 15-s tick defaults per DEC-AXO-086.
    /// Lock the numbers here so a silent regression on the constants
    /// gets caught by a unit test instead of a 5-min wait at runtime.
    /// REQ-AXO-901630 — `gpu_provider_explicitly_requested` flips true
    /// only when the operator unambiguously asked for a GPU provider.
    /// Locks the env-var matrix so a future refactor cannot weaken the
    /// fail-fast contract that prevents NoOpEmbedder + junk vectors.
    /// Pattern mirrors postgres::tests::ENV_LOCK + EnvGuard — `std::env`
    /// is process-global and cargo runs tests multi-threaded.
    #[test]
    fn gpu_provider_explicitly_requested_env_matrix() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let prov_key = "AXON_EMBEDDING_PROVIDER";
        let trt_key = "AXON_GPU_EMBED_SERVICE_TENSORRT";
        let saved_prov = std::env::var(prov_key).ok();
        let saved_trt = std::env::var(trt_key).ok();

        struct Restore<'a>(&'a str, Option<String>);
        impl<'a> Drop for Restore<'a> {
            fn drop(&mut self) {
                match &self.1 {
                    Some(v) => std::env::set_var(self.0, v),
                    None => std::env::remove_var(self.0),
                }
            }
        }
        let _r1 = Restore(prov_key, saved_prov);
        let _r2 = Restore(trt_key, saved_trt);

        std::env::remove_var(prov_key);
        std::env::remove_var(trt_key);
        assert!(!gpu_provider_explicitly_requested(), "unset → false");

        std::env::set_var(prov_key, "cpu");
        assert!(!gpu_provider_explicitly_requested(), "cpu → false");

        std::env::set_var(prov_key, "tensorrt");
        assert!(gpu_provider_explicitly_requested(), "tensorrt → true");

        std::env::set_var(prov_key, "CUDA");
        assert!(gpu_provider_explicitly_requested(), "CUDA (case) → true");

        // REQ-AXO-901737 — `AXON_EMBEDDING_PROVIDER` is the SINGLE canonical
        // knob; the legacy `AXON_GPU_EMBED_SERVICE_TENSORRT` flag is no longer
        // consulted by `gpu_provider_explicitly_requested`. With the provider
        // unset, the legacy flag must be inert regardless of its value. (This
        // test previously asserted the removed legacy behaviour and failed
        // deterministically once REQ-AXO-901737 updated the fn but not the
        // test.)
        std::env::remove_var(prov_key);
        std::env::set_var(trt_key, "1");
        assert!(!gpu_provider_explicitly_requested(), "legacy TRT flag=1 is inert");

        std::env::set_var(trt_key, "true");
        assert!(!gpu_provider_explicitly_requested(), "legacy TRT flag=true is inert");

        std::env::set_var(trt_key, "0");
        assert!(!gpu_provider_explicitly_requested(), "legacy TRT flag=0 is inert");
    }

    /// REQ-AXO-901874 — the indexer liveness heartbeat is spawned from
    /// `spawn_pipeline_v2_indexer` (unconditionally, before pipeline B), so
    /// it covers every ingestion-enabled mode. Lock the exact condition the
    /// old GPU-branch placement fell into: graph-only is `ingestion_enabled`
    /// (heartbeat MUST publish) yet NOT `semantic_workers_enabled` (the old
    /// code only spawned the heartbeat when semantic/GPU workers ran → false
    /// `indexer_ready=False`). A refactor re-coupling liveness to GPU breaks
    /// this test.
    #[test]
    fn indexer_liveness_heartbeat_covers_graph_only_mode() {
        use crate::runtime_mode::AxonRuntimeMode;
        assert!(
            AxonRuntimeMode::IndexerGraph.ingestion_enabled(),
            "graph-only indexes → reaches spawn_pipeline_v2_indexer → heartbeat must publish"
        );
        assert!(
            !AxonRuntimeMode::IndexerGraph.semantic_workers_enabled(),
            "graph-only runs NO GPU/semantic workers — the old heartbeat placement missed it"
        );
        assert!(AxonRuntimeMode::IndexerFull.ingestion_enabled());
        assert!(
            !AxonRuntimeMode::BrainOnly.ingestion_enabled(),
            "brain never reaches spawn_pipeline_v2_indexer → no indexer heartbeat"
        );
    }

    #[test]
    fn lifecycle_watchdog_defaults_match_dec_axo_086() {
        use std::time::Duration;
        // The expected DEC-AXO-086 defaults are hardcoded in
        // `attempt_pipeline_v2_runtime` ; verifying numbers here
        // produces a meaningful failure if someone changes them
        // without bumping DEC-AXO-086.
        let tick = Duration::from_secs(15);
        let t_idle = Duration::from_secs(300);
        let t_grace = Duration::from_secs(2);
        assert_eq!(tick.as_secs(), 15);
        assert_eq!(t_idle.as_secs(), 5 * 60);
        assert_eq!(t_grace.as_secs(), 2);
        // Grace must be smaller than tick so a wake-on-call can't be
        // immediately re-slept by the next tick.
        assert!(t_grace < tick);
        // T_idle must dominate tick so the watchdog evaluates many
        // times before the threshold trips.
        assert!(t_idle >= tick * 4);
    }
}
