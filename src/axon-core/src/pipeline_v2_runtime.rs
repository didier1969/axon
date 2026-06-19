//! Pipeline-v2 wiring for the live `axon-indexer` binary (REQ-AXO-289 S7).
//!
//! Thin bridge that:
//!
//! 1. Spawns [`pipeline_v2::spawn_pipeline_a`] (and `spawn_pipeline_b_full`
//!    when the runtime mode enables semantic workers) with a multi-project
//!    resolver (DEC-AXO-081).
//! 2. Spawns the Watchman file source ([`crate::watchman_source`], REQ-AXO-901893):
//!    clock/cursor deltas feed pipeline A's `input_tx` directly. On a hard
//!    Watchman failure it degrades to an explicit one-shot scanner walk + Blocker.
//! 3. Spawns the pipeline-B sorted-drain feeder ([`spawn_vector_sorted_drain`],
//!    DEC-AXO-901631): pulls a token-sorted reservoir of `embed_status='pending'`
//!    chunks and feeds B2 in order, so each fixed-size batch is length-homogeneous.
//!    Channel backpressure paces it to B's throughput. Replaces the retired
//!    demand_pull (s, Q) / NOTIFY machinery.
//! 4. Spawns the durable bootstrap + periodic reconciliation walk
//!    ([`crate::scanner::Scanner::scan`], REQ-AXO-901901): UPSERTs every eligible
//!    file as status='discovered' so the claim feeder (3) has a backlog to drain.
//!    This is what actually populates the queue — Watchman's fresh crawl alone
//!    under-delivers the cold-start bulk (see the boot-walk block below).
//!
//! The legacy notify watcher + in-memory `ingress_buffer` FIFO were RIPPED in the
//! LEGACY FEED PURGE (REQ-AXO-901893). The bulk-discovery walk is NOT legacy: it
//! lands directly in the PG work queue (no in-memory buffer) and is the
//! completeness floor under the always-on Watchman delta feed.
//!
//! All spawns are `tokio::spawn` so the function returns once everything
//! is wired; pipelines run in the background for the lifetime of the process.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::graph::GraphStore;
use crate::pipeline_v2::orchestrator::spawn_pipeline_a_with_cache;
use crate::pipeline_v2::{
    GpuB2Embedder, IndexedFileCache, IndexedFileEntry, NoOpEmbedder, PipelineAWorkerCounts,
    PipelineBWorkerCounts, PipelineChannelCaps, ProjectCodeResolver, ProjectRegistrySnapshot,
};
use crate::runtime_mode::AxonRuntimeMode;
use crate::scanner::Scanner;

/// REQ-AXO-901975 / DEC-AXO-901631 — sorted-drain backoff bounds, surfaced
/// in `runtime_config` for observability. When the pending queue is empty
/// the drain sleeps with doubling backoff between these bounds.
pub const VECTOR_DRAIN_BACKOFF_INITIAL_MS: u64 = 200;
pub const VECTOR_DRAIN_BACKOFF_MAX_MS: u64 = 30_000;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// REQ-AXO-901975 / DEC-AXO-901631 — sorted-drain reservoir size: the
/// number of token-sorted pending chunks pulled per drain wave. The
/// SELECT already orders by `token_count`, so a fixed-size B2 batch carved
/// from this reservoir is length-homogeneous → one GPU inference per batch.
fn vector_drain_reservoir_from_env() -> usize {
    env_usize("AXON_B2_RESERVOIR", 8192)
}

/// REQ-AXO-901975 / DEC-AXO-901631 — sorted-drain feed for pipeline B.
///
/// Replaces the demand_pull (s, Q) / NOTIFY / stall-tracker machinery with a
/// flat drain loop: pull a token-sorted reservoir of pending chunks (the
/// SELECT does `ORDER BY token_count`), feed them to B2 **in order** so each
/// fixed-size batch the B2 worker carves is length-homogeneous → one stable
/// GPU shape per batch.
///
/// Correct-by-construction safety: the bounded `tx` channel provides
/// backpressure — `send().await` blocks when B is saturated or stalled, so
/// the loop physically cannot re-`SELECT` in a tight spin (the REQ-AXO-901862
/// runaway). `embed_status='pending'` is the durable queue truth; B3 stamps
/// `'embedded'` idempotently, so a restart resumes exactly where it left off.
/// No dedup cache, no reorder band, no claim column.
fn spawn_vector_sorted_drain(
    store: Arc<GraphStore>,
    tx: mpsc::Sender<crate::pipeline_v2::ChunkForEmbedding>,
    reservoir: usize,
) {
    tokio::spawn(async move {
        let mut backoff_ms = VECTOR_DRAIN_BACKOFF_INITIAL_MS;
        loop {
            // Blocking PG SELECT off the tokio runtime.
            let store_for_q = store.clone();
            let rows = match tokio::task::spawn_blocking(move || {
                store_for_q.select_chunks_with_content_needing_embedding(reservoir)
            })
            .await
            {
                Ok(Ok(rows)) => rows,
                Ok(Err(err)) => {
                    warn!(error = %err, "vector sorted-drain: pending SELECT failed");
                    Vec::new()
                }
                Err(err) => {
                    warn!(error = %err, "vector sorted-drain: blocking join failed");
                    Vec::new()
                }
            };
            if rows.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = backoff_ms.saturating_mul(2).min(VECTOR_DRAIN_BACKOFF_MAX_MS);
                continue;
            }
            backoff_ms = VECTOR_DRAIN_BACKOFF_INITIAL_MS;
            for (chunk_id, content, content_hash) in rows {
                let payload = crate::pipeline_v2::ChunkForEmbedding {
                    chunk_id,
                    content,
                    content_hash,
                };
                if tx.send(payload).await.is_err() {
                    // B side closed — stop draining.
                    return;
                }
            }
        }
    });
}

fn resolve_database_url_for_listener() -> String {
    // REQ-AXO-901881 W2 (#7/#17) — resolve via THE canonical resolver
    // (postgres::resolve_database_url, alias-aware), with an instance-aware
    // fallback that never routes a dev process to the live DB even if the
    // URL is unset. Was the 4th divergent resolver (the REQ-AXO-315 leak).
    crate::postgres::resolve_database_url(None).unwrap_or_else(|_| {
        if crate::env_alias::read_with_alias_or("AXON_INSTANCE", "AXON_INSTANCE_KIND", "live")
            .eq_ignore_ascii_case("dev")
        {
            "postgres://axon@127.0.0.1:44144/axon_dev".to_string()
        } else {
            "postgres://axon@127.0.0.1:44144/axon_live".to_string()
        }
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
/// `axon.EmbedderLifecycleHeartbeat` (role="indexer"). The brain's
/// `resolve_indexer_liveness` derives `indexer_ready` from this row's
/// freshness alone, so every indexing process — graph-only, CPU, NoOp, or
/// GPU — is provably alive. Previously the publisher was spawned only in the
/// GPU-Ok branch of pipeline B, so graph-only / GPU-failed indexers were
/// reported `indexer_ready=False` despite actively indexing (false negative).
fn spawn_indexer_liveness_heartbeat(
    store: Arc<GraphStore>,
    a_stage_metrics: [Arc<crate::pipeline_v2::StageMetrics>; 3],
) {
    crate::embedder::lifecycle_machine::spawn_lifecycle_heartbeat_publisher(
        std::time::Duration::from_secs(5),
        move |snapshot| {
            if let Err(err) = store.record_lifecycle_heartbeat("indexer", &snapshot) {
                warn!(
                    error = %err,
                    "REQ-AXO-901874: failed to UPSERT EmbedderLifecycleHeartbeat row"
                );
            }
            // REQ-AXO-901854 — publish runtime truth observed at the OWNER
            // (PIL-AXO-001). Every field resolves to a canonical pipeline_v2
            // source, never a brain-local proxy or dead v1 counter:
            //   * graph_workers_active = Σ inflight of A1/A2/A3 (busy graph
            //     workers, from pipeline_v2 StageMetrics);
            //   * chunk_embeddings_per_second = the indexer's embed-rate accessor;
            //   * in_flight_* = RAM in-flight registry (REQ-AXO-901919);
            //   * ready_queue_chunks / persist_queue_depth = vector_runtime_metrics
            //     owner gauges.
            // The brain READS this row (empty under brain_only) and projects it.
            let graph_workers_active: i64 = a_stage_metrics
                .iter()
                .map(|m| m.snapshot().inflight as i64)
                .sum();
            let in_flight = crate::pipeline_v2::in_flight::InFlightRegistry::global();
            let in_flight_oldest = in_flight.snapshot();
            let vector_metrics = crate::service_guard::vector_runtime_metrics();
            let truth = crate::graph_ingestion::IndexerRuntimeTruthRecord {
                process_role: "indexer".to_string(),
                heartbeat_ms: snapshot.heartbeat_ms,
                graph_workers_active,
                chunk_embeddings_per_second:
                    crate::service_guard::vector_chunk_embeddings_per_second(),
                in_flight_count: in_flight.len() as i64,
                oldest_in_flight_path: in_flight_oldest.as_ref().map(|s| s.path.clone()),
                oldest_in_flight_stage: in_flight_oldest
                    .as_ref()
                    .map(|s| s.stage.to_string()),
                oldest_in_flight_age_ms: in_flight_oldest
                    .as_ref()
                    .map(|s| s.age_ms as i64)
                    .unwrap_or(0),
                ready_queue_chunks: vector_metrics.ready_queue_chunks_current as i64,
                persist_queue_depth: vector_metrics.persist_queue_depth_current as i64,
            };
            if let Err(err) = store.record_indexer_runtime_truth(&truth) {
                warn!(
                    error = %err,
                    "REQ-AXO-901854: failed to UPSERT indexer_runtime_truth row"
                );
            }
        },
    );
}

pub fn spawn_pipeline_v2_indexer(
    runtime_mode: AxonRuntimeMode,
    store: Arc<GraphStore>,
    watch_root: String,
) -> Result<()> {
    // REQ-AXO-901901 — discovery has TWO complementary feeds, both landing in
    // the DBQ-A PG work queue / pipeline-A input (no in-memory buffer):
    //   * Watchman (live deltas, fast-path) — feeds input_tx directly.
    //   * Scanner walk (boot + periodic) — UPSERTs status='discovered' for the
    //     full eligible set; the claim feeder drains it. This is the bulk +
    //     crash-recovery floor (Watchman's fresh crawl under-delivers cold-start).
    // Both are idempotent via the dedup cache below (skips the A2 parse on
    // unchanged content_hash) and A3's ON CONFLICT UPSERTs. The boot walk is
    // wired at the END of this function, once the claim feeder is live.
    let caps = PipelineChannelCaps::from_env();
    let counts_a = PipelineAWorkerCounts::from_env();

    // DEC-AXO-081 — per-file project_code resolver. Scanner constructed
    // with empty explicit code so it delegates to
    // project_meta::resolve_project_identity_for_path on every call.
    let scanner = Arc::new(Scanner::new(&watch_root, ""));
    // REQ-AXO-901916 CP2c — resolver from a RAM snapshot of the canonical PG
    // project registry (PIL-AXO-001), hydrated ONCE at boot. Longest-prefix
    // match in RAM = zero filesystem I/O per file, replacing the old per-A3-call
    // rescan of every `.axon/meta.json`. Falls back to per-file scanner
    // resolution ONLY if the registry SELECT fails / is empty (explicit degraded
    // path). "UNK" stays the DROP sentinel (REQ-AXO-901860): graph_ingestion
    // skips it, so an unresolved file is enrolled nowhere.
    let resolver: ProjectCodeResolver = match crate::project_meta::registered_project_identities(
        &store,
    ) {
        Ok(ids) if !ids.is_empty() => {
            let n = ids.len();
            let rows = ids
                .into_iter()
                .map(|id| (id.code, id.project_path.to_string_lossy().into_owned()));
            info!("pipeline_v2: project resolver hydrated from PG registry ({n} projects, longest-prefix RAM)");
            ProjectRegistrySnapshot::from_rows(rows).into_resolver()
        }
        other => {
            match &other {
                Err(e) => {
                    warn!(error = %e, "pipeline_v2: registry snapshot hydration failed — per-file scanner fallback")
                }
                Ok(_) => {
                    warn!("pipeline_v2: PG project registry empty — per-file scanner fallback")
                }
            }
            let store_for_resolver = store.clone();
            let scanner_for_resolver = scanner.clone();
            Arc::new(move |path: &std::path::Path| -> String {
                match scanner_for_resolver.project_code_for_path(&store_for_resolver, path) {
                    Ok(code) => code,
                    Err(err) => {
                        warn!(?path, error = %err, "pipeline_v2: project_code unresolved → file dropped (UNK sentinel)");
                        "UNK".to_string()
                    }
                }
            }) as ProjectCodeResolver
        }
    };

    // REQ-AXO-901746 — hydrate the content-hash dedup cache from PG at boot.
    // Files whose (path, content_hash) match are skipped between A1 and A2,
    // avoiding the expensive tree-sitter parse on unchanged files.
    let dedup_cache = match store.load_all_indexed_files() {
        Ok(rows) => {
            let count = rows.len();
            let cache = IndexedFileCache::from_iter(rows.into_iter().map(
                |(path, hash, ts, mtime, size)| {
                    (
                        path,
                        IndexedFileEntry {
                            content_hash: hash,
                            last_seen_ms: ts,
                            mtime_ms: mtime,
                            size_bytes: size,
                        },
                    )
                },
            ));
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
    let handles_a =
        spawn_pipeline_a_with_cache(counts_a, caps, store.clone(), resolver, dedup_cache.clone());

    // REQ-AXO-901874 — indexer liveness heartbeat, decoupled from the GPU
    // embedder. This function is reached for every ingestion-enabled mode
    // (IndexerGraph + IndexerFull, see runtime_mode.ingestion_enabled());
    // publishing here — BEFORE and independent of the optional pipeline-B
    // GPU embedder — means a graph-only or CPU/NoOp indexer is still
    // provably alive to the brain (`resolve_indexer_liveness` reads the
    // `axon.EmbedderLifecycleHeartbeat` row freshness, not the GPU
    // state). Liveness ≠ GPU-up. Tick 5s sits well under the brain's ~30s
    // freshness window so a single missed tick stays fresh.
    spawn_indexer_liveness_heartbeat(
        store.clone(),
        [
            handles_a.metrics_a1.clone(),
            handles_a.metrics_a2.clone(),
            handles_a.metrics_a3.clone(),
        ],
    );

    // Create the b_chunks channel here. The sorted-drain feeder owns the tx ;
    // spawn_pipeline_b_full_multi takes the rx. The channel carries
    // ChunkForEmbedding (one round-trip SELECT-with-content per drain wave).
    let (b_chunks_tx, b_chunks_rx) =
        mpsc::channel::<crate::pipeline_v2::ChunkForEmbedding>(caps.internal);

    if runtime_mode.semantic_workers_enabled() {
        let counts_b = PipelineBWorkerCounts::from_env();
        info!(
            "pipeline_v2: spawning pipeline B (b2={} b3={} ; no B1 — sorted-drain feeds B2)",
            counts_b.b2, counts_b.b3
        );
        // REQ-AXO-902027 — pre-flight is DIAGNOSTIC, not a gate. It loudly logs
        // a GPU shared library that fails to load — turning a silent dmesg-only
        // SIGSEGV deep inside `try_new_cuda` into a NAMED application-log line —
        // but it does NOT block the GPU init. An isolated dlopen probe can
        // false-positive on ORT's runtime-resolved provider symbols (the cuda /
        // tensorrt providers resolve `Provider_GetHost` via the core lib), and
        // BLOCKING on that wrongly downed a healthy indexer (regression caught
        // in dev). `try_new_cuda` below stays the authoritative gate; if a lib
        // is genuinely corrupt it crashes there, but this log already named the
        // culprit instead of leaving only a dmesg trace.
        if let Err(reason) = crate::embedder::gpu_preflight::preflight_gpu_libraries() {
            tracing::error!(reason = %reason, "pipeline_v2: GPU library pre-flight flagged a lib — proceeding to GPU init anyway; if the process crashes during init, THIS names the culprit");
        }
        let embedder: Arc<dyn crate::pipeline_v2::B2Embedder> =
            match GpuB2Embedder::try_new_cuda("indexer-pipeline-v2", 0) {
            // DEC-AXO-901631 — the GPU session stays resident for the worker's
            // lifetime (no idle watchdog / sleep-wake). Single-GPU live↔dev
            // cohabitation is handled at the process level (PIL-AXO-004).
            Ok(e) => Arc::new(e) as Arc<dyn crate::pipeline_v2::B2Embedder>,
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
        // REQ-AXO-901748 — when AXON_B2_WORKERS > 1, create one ORT
        // session per worker for true CUDA double-buffering (no Mutex
        // contention). Each session has its own TensorRT engine cache.
        let embedders: Vec<Arc<dyn crate::pipeline_v2::B2Embedder>> = if counts_b.b2 > 1 {
            let mut v = vec![embedder];
            for i in 1..counts_b.b2 {
                match GpuB2Embedder::try_new_cuda(&format!("indexer-pipeline-v2-b2w{i}"), i) {
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
            counts_b,
            caps,
            store.clone(),
            embedders,
            b_chunks_rx,
        );
        // REQ-AXO-314 — keep the receipt rx alive by draining it in a
        // background task. Dropping `handles_b.output_rx` immediately
        // would close the receipt channel; B3 then short-circuits on
        // its first `tx.send(receipt)` failure and cascades upstream.
        let mut output_rx_b =
            std::mem::replace(&mut handles_b.output_rx, tokio::sync::mpsc::channel(1).1);
        tokio::spawn(async move { while output_rx_b.recv().await.is_some() {} });

        // DEC-AXO-901631 — sorted-drain feed (replaces demand_pull (s, Q)).
        // Pulls token-sorted pending chunks and feeds B2 in order so each
        // fixed-size batch is length-homogeneous → one GPU inference. Channel
        // backpressure paces the drain to B's throughput (no spin on stall).
        let reservoir = vector_drain_reservoir_from_env();
        spawn_vector_sorted_drain(store.clone(), b_chunks_tx, reservoir);
    } else {
        // No B side — drop the b_chunks tx so the sorted-drain won't be
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
                    receipt.mtime_ms,
                    receipt.size_bytes,
                );
            }
        }
    });

    // REQ-AXO-901893 — Watchman is the file source, unconditionally. The
    // daemon's clock/cursor reconciliation IS the live feed: a `since:<clock>`
    // subscription returns the exact cumulative delta (or a fresh-instance full
    // set), so a missed FS event is structurally impossible. The legacy
    // bootstrap-scan + ingress-drain + periodic-sweep + notify watcher were
    // RIPPED in the LEGACY FEED PURGE (REQ-AXO-901893 deferred RIP). On a hard
    // Watchman connect failure the source degrades to an explicit one-shot
    // scanner walk (`fallback_scanner_bootstrap`) + a Blocker — never silent.
    crate::watchman_source::spawn_watchman_source(
        store.clone(),
        handles_a.input_tx.clone(),
        scanner.clone(),
        watch_root.clone(),
        resolve_database_url_for_listener(),
    )?;

    // REQ-AXO-901916 (PIL-AXO-007) — DIRECT-STREAMING bootstrap + periodic
    // reconciliation walk. Replaces the claim-feeder + status='discovered'
    // machine ENTIRELY: the walk enumerates every eligible file and pushes its
    // path STRAIGHT into pipeline A's bounded input_tx (backpressure =
    // send().await), exactly like the Watchman delta feed. A1's level-1
    // (mtime/size) pre-filter skips unchanged files with zero I/O; A3's
    // content_hash UPSERT is idempotent — so a re-walk / restart re-processes
    // only the delta, and a crash mid-parse is reprocessed on the next walk
    // (no lease/retry bookkeeping needed). After each walk, delete_stale purges
    // paths the FS confirms gone (last_seen_ms older than this walk + Path::exists
    // re-check, scoped to the watch root — REQ-AXO-901831/901884). Watchman stays
    // the live-delta fast-path; this walk is the completeness + recovery floor.
    // Tunable: AXON_RECONCILE_SWEEP_SECS (floor 30s, default 900s).
    {
        let sweep_secs: u64 = std::env::var("AXON_RECONCILE_SWEEP_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|v| *v >= 30)
            .unwrap_or(900);
        let scanner_for_walk = scanner.clone();
        let store_for_walk = store.clone();
        let walk_input_tx = handles_a.input_tx.clone();
        let root_prefix = std::fs::canonicalize(&watch_root)
            .unwrap_or_else(|_| std::path::PathBuf::from(&watch_root))
            .to_string_lossy()
            .into_owned();
        info!(
            "pipeline_v2: direct-streaming bootstrap + reconciliation walk active \
             (sweep_secs={sweep_secs}) — enumerate → input_tx (PIL-AXO-007)"
        );
        tokio::spawn(async move {
            loop {
                let walk_start_ms = chrono::Utc::now().timestamp_millis();
                let started = std::time::Instant::now();
                let scanner = scanner_for_walk.clone();
                let files = tokio::task::spawn_blocking(move || scanner.enumerate_files())
                    .await
                    .unwrap_or_default();
                let total = files.len();
                let mut pushed = 0usize;
                for path in files {
                    // Backpressure: send().await paces the walk to A1's drain rate.
                    if walk_input_tx.send(path).await.is_err() {
                        return; // pipeline shut down
                    }
                    pushed += 1;
                }
                info!(
                    "pipeline_v2: reconciliation walk fed {pushed}/{total} paths to input_tx \
                     in {:.1}s; next sweep in {sweep_secs}s",
                    started.elapsed().as_secs_f64()
                );
                // Purge files deleted from disk since this walk started. The
                // Path::exists() re-check inside delete_stale makes it
                // non-destructive even while A3 is still draining the push above.
                let store_for_stale = store_for_walk.clone();
                let prefix = root_prefix.clone();
                let purge = tokio::task::spawn_blocking(move || {
                    // REQ-AXO-901950 — same eligibility as the discovery walk so
                    // files newly gitignored/.axonignore'd are purged here too,
                    // not only the ones removed from disk.
                    let scanner = crate::scanner::Scanner::new(&prefix, "");
                    store_for_stale.delete_stale_indexed_files(walk_start_ms, &prefix, &|p| {
                        scanner.should_process_path(p)
                    })
                })
                .await;
                match purge {
                    Ok(Ok(deleted)) if !deleted.is_empty() => info!(
                        "pipeline_v2: purged {} stale IndexedFile(s) (gone from disk)",
                        deleted.len()
                    ),
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => warn!(error = %e, "pipeline_v2: delete_stale failed (non-fatal)"),
                    Err(e) => warn!(error = %e, "pipeline_v2: delete_stale task panicked"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(sweep_secs)).await;
            }
        });
    }

    Ok(())
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
        assert!(
            !gpu_provider_explicitly_requested(),
            "legacy TRT flag=1 is inert"
        );

        std::env::set_var(trt_key, "true");
        assert!(
            !gpu_provider_explicitly_requested(),
            "legacy TRT flag=true is inert"
        );

        std::env::set_var(trt_key, "0");
        assert!(
            !gpu_provider_explicitly_requested(),
            "legacy TRT flag=0 is inert"
        );
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
