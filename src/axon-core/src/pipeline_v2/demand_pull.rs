//! DEC-AXO-901620 — Demand-pull pipeline feeders with PG NOTIFY wake.
//!
//! Two-value model per pipeline:
//!   - **threshold**: pull only when the pipeline's in-flight count drops
//!     below this value (= seconds_of_work × throughput)
//!   - **batch**: max items per PG SELECT
//!
//! Claim semantics (C3/W1): demand-pull atomically increments retry_count
//! and sets last_attempt_ms before feeding items. Files stuck after 3
//! attempts are skipped (poison pill). A3 resets retry_count on success.
//!
//! W2: demand-pull checks channel capacity before pulling, preserving
//! headroom for real-time watcher events.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::graph::GraphStore;

/// W4: observable demand-pull metrics, queryable by dashboard/MCP.
pub struct DemandPullMetrics {
    pub pulls_total: AtomicU64,
    pub items_fed_total: AtomicU64,
    pub empty_pulls_total: AtomicU64,
    pub try_send_failures_total: AtomicU64,
    pub skipped_above_threshold: AtomicU64,
}

impl DemandPullMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pulls_total: AtomicU64::new(0),
            items_fed_total: AtomicU64::new(0),
            empty_pulls_total: AtomicU64::new(0),
            try_send_failures_total: AtomicU64::new(0),
            skipped_above_threshold: AtomicU64::new(0),
        })
    }

    pub fn snapshot(&self) -> DemandPullSnapshot {
        DemandPullSnapshot {
            pulls_total: self.pulls_total.load(Ordering::Relaxed),
            items_fed_total: self.items_fed_total.load(Ordering::Relaxed),
            empty_pulls_total: self.empty_pulls_total.load(Ordering::Relaxed),
            try_send_failures_total: self.try_send_failures_total.load(Ordering::Relaxed),
            skipped_above_threshold: self.skipped_above_threshold.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DemandPullSnapshot {
    pub pulls_total: u64,
    pub items_fed_total: u64,
    pub empty_pulls_total: u64,
    pub try_send_failures_total: u64,
    pub skipped_above_threshold: u64,
}


/// Adaptive demand-pull backoff floor (active drain cadence) — published to
/// `runtime_config_snapshot` so the dashboard reports the real value, not a
/// hardcoded literal. After a productive pull the loop resets to this.
pub const BACKOFF_INITIAL_MS: u64 = 200;
/// Adaptive demand-pull backoff ceiling (fully idle cadence). Doubling from
/// `BACKOFF_INITIAL_MS` caps here. Also surfaced canonically on the dashboard.
pub const BACKOFF_MAX_MS: u64 = 30_000;
const SAFETY_POLL_SECS: u64 = 30;
const IDLE_THRESHOLD: u32 = 5;
// REQ-AXO-901891 — MAX_RETRY / CLAIM_TIMEOUT_MS removed with the pipeline A
// claim machinery (no more retry_count poison-pill / claim window).
/// REQ-AXO-901810 G7 (MIL-AXO-029 slice 4) — NOTIFY coalesce window.
/// After the first `file_discovered` NOTIFY wakes the feeder, wait
/// this long collecting more before kicking the pull loop. Under a
/// burst (git checkout, mass rename, large directory move triggering
/// thousands of inotify events in ~ms) this collapses the burst into
/// a single replenishment cycle instead of N spin-wake-pull rounds.
/// 50 ms is well below the 1 s adaptive cadence so steady-state
/// latency is unchanged ; the win is only on bursts.
const NOTIFY_COALESCE_MS: u64 = 50;

/// Spawn the demand-pull feeder for pipeline B.
///
/// Slice 5 SOTA — feeder now emits `ChunkForEmbedding` directly to the
/// b_chunks channel (consumed by B2 GPU). Collapses the previous
/// B1 stage worker pool into this single async loop. SELECT-with-content
/// happens here ; no more 2-round-trip pattern.
pub fn spawn_pipeline_b_demand_pull(
    store: Arc<GraphStore>,
    database_url: String,
    b_chunks_tx: Sender<super::stage_b1::ChunkForEmbedding>,
    threshold: usize,
    batch_size: usize,
) -> Arc<DemandPullMetrics> {
    let metrics = DemandPullMetrics::new();
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match demand_pull_b_loop(&store, &database_url, &b_chunks_tx, threshold, batch_size, &metrics_clone)
                .await
            {
                Ok(()) => {
                    warn!("demand-pull B: LISTEN loop exited cleanly; reconnecting");
                    backoff_ms = BACKOFF_INITIAL_MS;
                }
                Err(err) => {
                    warn!(
                        backoff_ms,
                        error = %err,
                        "demand-pull B: LISTEN errored; backing off"
                    );
                    backoff_ms = (backoff_ms * 2).min(BACKOFF_MAX_MS);
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    });
    metrics
}

/// Outcome of one B pull cycle — what was fed and the identity of the
/// pending set, so [`StallTracker`] can tell a draining consumer from a
/// stalled one.
struct CycleOutcome {
    had_work: bool,
    head: Option<String>,
    total_fed: usize,
}

/// One `pull_and_feed_b` attempt: chunks pushed + the first chunk_id of the
/// pulled batch (`None` when nothing was pulled — backpressure or empty).
struct PullBatch {
    fed: usize,
    head: Option<String>,
}

/// REQ-AXO-901862 — detect a non-draining consumer so demand-pull B backs
/// off instead of spinning the CPU re-pulling the same `pending` chunks that
/// never transition to `embedded`. Live incident (indexer pid 14819): 292%
/// CPU for 2h37, zero ChunkEmbedding rows written, because B2 drained the
/// channel without persisting (vector_workers=0 / NoOp embedder). Lenses:
/// #4 back-pressure (yield when downstream isn't draining), #2 idempotence
/// (re-pull of an identical set), #10 throughput (CPU burned for 0 progress).
///
/// Signal: `select_chunks_with_content_needing_embedding` orders
/// deterministically, so a frozen `pending` set (nothing embedded between
/// cycles) yields the same `(head chunk_id, count)` fingerprint.
/// `STALL_REPEAT` identical non-empty cycles ⇒ stalled. Any genuine drain
/// changes the set (head advances or count drops) and resets the streak.
#[derive(Default)]
struct StallTracker {
    last: Option<(String, usize)>,
    repeat: u32,
}

impl StallTracker {
    const STALL_REPEAT: u32 = 3;

    fn new() -> Self {
        Self::default()
    }

    /// Observe a completed pull cycle. `head` is the first chunk_id pulled
    /// this cycle (`None` if nothing was pulled). Returns true once the same
    /// non-empty fingerprint has repeated `STALL_REPEAT` times in a row.
    fn observe(&mut self, head: Option<String>, count: usize) -> bool {
        match head {
            None => {
                self.last = None;
                self.repeat = 0;
                false
            }
            Some(h) => {
                let fp = (h, count);
                if self.last.as_ref() == Some(&fp) {
                    self.repeat = self.repeat.saturating_add(1);
                } else {
                    self.repeat = 0;
                }
                self.last = Some(fp);
                self.repeat >= Self::STALL_REPEAT
            }
        }
    }
}

async fn demand_pull_b_loop(
    store: &Arc<GraphStore>,
    database_url: &str,
    b_chunks_tx: &Sender<super::stage_b1::ChunkForEmbedding>,
    threshold: usize,
    batch_size: usize,
    metrics: &Arc<DemandPullMetrics>,
) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("demand-pull B: connect failed")?;

    let (notify_tx, mut notify_rx) =
        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(512);

    let driver = tokio::spawn(async move {
        let stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        tokio::pin!(stream);
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(AsyncMessage::Notification(n)) => {
                    if notify_tx.send(n).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(error = %err, "demand-pull B: stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute("LISTEN chunk_pending_embed")
        .await
        .context("demand-pull B: LISTEN failed")?;

    info!(
        "demand-pull B: active (threshold={threshold}, batch={batch_size})"
    );

    let mut consecutive_empty = 0u32;
    let safety_interval = Duration::from_secs(SAFETY_POLL_SECS);

    // REQ-AXO-901810 G2 — same single-shot guard as pipeline A.
    let in_progress = Arc::new(AtomicBool::new(false));

    // REQ-AXO-901862 — back off when the consumer (B2) isn't draining.
    let mut stall = StallTracker::new();
    let mut stall_warned = false;

    let seed = run_pull_cycle_b(
        store,
        b_chunks_tx,
        threshold,
        batch_size,
        &mut consecutive_empty,
        metrics,
        &in_progress,
    )
    .await;
    let _ = stall.observe(seed.head, seed.total_fed);

    let mut last_pull_had_work = true;
    loop {
        let wait_duration = if last_pull_had_work {
            Duration::from_secs(1)
        } else {
            safety_interval
        };

        let woke_by_notify = tokio::select! {
            biased;
            Some(_) = notify_rx.recv() => {
                // REQ-AXO-901810 G7 — coalesce burst NOTIFYs.
                let coalesce_deadline =
                    tokio::time::Instant::now()
                        + Duration::from_millis(NOTIFY_COALESCE_MS);
                while tokio::time::Instant::now() < coalesce_deadline {
                    tokio::select! {
                        biased;
                        Some(_) = notify_rx.recv() => {}
                        _ = tokio::time::sleep_until(coalesce_deadline) => break,
                    }
                }
                while notify_rx.try_recv().is_ok() {}
                true
            }
            _ = tokio::time::sleep(wait_duration) => {
                false
            }
        };

        if woke_by_notify {
            consecutive_empty = 0;
        }

        let outcome = run_pull_cycle_b(
            store,
            b_chunks_tx,
            threshold,
            batch_size,
            &mut consecutive_empty,
            metrics,
            &in_progress,
        )
        .await;

        // REQ-AXO-901862 — if the same pending set is re-pulled with no
        // embedding progress, the consumer is stalled (NoOp / vector_workers
        // =0 / hung embedder). Stop the 1s tight re-pull: fall back to the
        // safety poll and warn once, so the puller can't burn the CPU for
        // zero throughput. Resumes automatically when the set drains.
        if stall.observe(outcome.head, outcome.total_fed) {
            if !stall_warned {
                warn!(
                    fed = outcome.total_fed,
                    backoff_secs = SAFETY_POLL_SECS,
                    "demand-pull B: pending chunks re-pulled with no embedding \
                     progress; consumer (B2) not draining — backing off (check \
                     embedder provider / vector_workers). REQ-AXO-901862"
                );
                stall_warned = true;
            }
            last_pull_had_work = false;
        } else {
            if stall_warned && outcome.had_work {
                info!("demand-pull B: embedding progress resumed; leaving back-off");
            }
            stall_warned = false;
            last_pull_had_work = outcome.had_work;
        }

        if driver.is_finished() {
            return Ok(());
        }
    }
}

/// REQ-AXO-901810 G2 — pipeline B mirror of [`run_pull_cycle`].
async fn run_pull_cycle_b(
    store: &Arc<GraphStore>,
    b_chunks_tx: &Sender<super::stage_b1::ChunkForEmbedding>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
    in_progress: &Arc<AtomicBool>,
) -> CycleOutcome {
    if in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return CycleOutcome {
            had_work: false,
            head: None,
            total_fed: 0,
        };
    }
    let mut had_work = false;
    let mut head: Option<String> = None;
    let mut total_fed = 0usize;
    loop {
        let batch = pull_and_feed_b(
            store,
            b_chunks_tx,
            threshold,
            batch_size,
            consecutive_empty,
            metrics,
        )
        .await;
        if head.is_none() {
            head = batch.head;
        }
        total_fed += batch.fed;
        if batch.fed == 0 {
            break;
        }
        had_work = true;
    }
    in_progress.store(false, Ordering::Release);
    CycleOutcome {
        had_work,
        head,
        total_fed,
    }
}

async fn pull_and_feed_b(
    store: &Arc<GraphStore>,
    b_chunks_tx: &Sender<super::stage_b1::ChunkForEmbedding>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
) -> PullBatch {
    let in_flight = b_chunks_tx.max_capacity() - b_chunks_tx.capacity();
    if in_flight >= threshold {
        metrics.skipped_above_threshold.fetch_add(1, Ordering::Relaxed);
        return PullBatch { fed: 0, head: None };
    }

    metrics.pulls_total.fetch_add(1, Ordering::Relaxed);
    let store_clone = store.clone();
    // Slice 5 SOTA — single round-trip SELECT-with-content. Collapses
    // the previous B1 stage worker (SELECT id then SELECT content).
    let result = tokio::task::spawn_blocking(move || {
        store_clone.select_chunks_with_content_needing_embedding(batch_size)
    })
    .await;

    match result {
        Ok(Ok(rows)) if !rows.is_empty() => {
            let count = rows.len();
            let head = rows.first().map(|(id, _, _)| id.clone());
            let mut sent = 0usize;
            // Slice 6 fix — `send().await` au lieu de `try_send` :
            // les chunks sélectionnés via SELECT-with-content ne doivent
            // PAS être silently dropped quand le b_chunks channel est
            // saturé. Le backpressure naturel ralentit demand_pull au
            // pace du GPU drum (B2) — c'est exactement le contrat
            // « demand-pull ». Avant slice 6 fix : try_send dropping
            // 60-90% des chunks par cycle = re-SELECT massif amplifié
            // PG + churn allocations, sustained throughput catastrophique
            // (14 emb/sec sur GPU capable 300).
            for (chunk_id, content, content_hash) in rows {
                let payload = super::stage_b1::ChunkForEmbedding {
                    chunk_id,
                    content,
                    content_hash,
                };
                if b_chunks_tx.send(payload).await.is_err() {
                    // Receiver dropped (shutdown) — stop pushing.
                    break;
                }
                sent += 1;
            }
            metrics.items_fed_total.fetch_add(sent as u64, Ordering::Relaxed);
            *consecutive_empty = 0;
            if sent > 0 {
                info!("demand-pull B: fed {sent}/{count} chunks (in_flight={in_flight}/{threshold}, backpressured by B2)");
            }
            PullBatch { fed: sent, head }
        }
        Ok(Ok(_)) => {
            metrics.empty_pulls_total.fetch_add(1, Ordering::Relaxed);
            *consecutive_empty = consecutive_empty.saturating_add(1);
            if *consecutive_empty == IDLE_THRESHOLD {
                info!("demand-pull B: pipeline idle ({IDLE_THRESHOLD} empty pulls)");
            }
            PullBatch { fed: 0, head: None }
        }
        Ok(Err(err)) => {
            warn!(error = %err, "demand-pull B: SELECT failed");
            PullBatch { fed: 0, head: None }
        }
        Err(join_err) => {
            warn!(error = %join_err, "demand-pull B: spawn_blocking panicked");
            PullBatch { fed: 0, head: None }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// REQ-AXO-901897 (DBQ slice 1) — pipeline A claim feeder (canonical, unconditional;
// symmetric to demand_pull_b which has no flag — REQ-AXO-901893 legacy purge).
//
// The DB (ist.IndexedFile.status) is the durable A work queue. This feeder is
// a near-exact clone of `spawn_pipeline_b_demand_pull`, swapping the chunk
// SELECT for an atomic FOR UPDATE SKIP LOCKED claim of 'discovered' (and
// stale-lease 'parsing') rows. It runs ALONGSIDE the Watchman fast-path feed:
// Watchman handles live changes, the claim feeder drains the backlog/recovers
// crashed claims BY CONSTRUCTION. Both feed the SAME pipeline-A input_tx
// (Sender<PathBuf>) — the very sink Watchman uses.
// ─────────────────────────────────────────────────────────────────────────

/// Default claim lease (ms). A 'parsing' row whose `lease_until_ms` is older
/// than now is considered abandoned (worker crashed mid-parse) and is
/// re-claimable. Override via `AXON_DBQ_A_LEASE_MS`.
pub const DBQ_A_LEASE_MS_DEFAULT: i64 = 60_000;
/// Rows claimed per UPDATE…RETURNING. Override via `AXON_DBQ_A_CLAIM_BATCH`.
pub const DBQ_A_CLAIM_BATCH_DEFAULT: usize = 256;

fn dbq_a_lease_ms() -> i64 {
    std::env::var("AXON_DBQ_A_LEASE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DBQ_A_LEASE_MS_DEFAULT)
}

pub fn dbq_a_claim_batch() -> usize {
    std::env::var("AXON_DBQ_A_CLAIM_BATCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DBQ_A_CLAIM_BATCH_DEFAULT)
}

/// The atomic claim. Promotes up to `limit` claimable rows to 'parsing' with a
/// fresh lease and RETURNs their paths. The inner SELECT … FOR UPDATE SKIP
/// LOCKED guarantees two concurrent claimers never take the same row (the
/// contention guard demand_pull_b relies on for chunks). Stale-lease 'parsing'
/// rows (crashed mid-parse) are reclaimable — no retry cap / dead-letter.
///
/// Public for the SQL-shape unit test (the query string is the contract).
pub fn build_claim_sql(now_ms: i64, lease_ms: i64, limit: usize) -> String {
    // REQ-AXO-901906 — claim 'discovered' + stale-lease 'parsing' (crash
    // recovery, mirrors pipeline B re-SELECTing 'pending' chunks). No
    // retry_count / dead-letter: A2 has a parse timeout so every file reaches a
    // terminal status, and a clean boot resets any 'parsing' to 'discovered'.
    format!(
        "UPDATE ist.IndexedFile \
            SET status = 'parsing', \
                lease_until_ms = {now} + {lease}, \
                last_attempt_ms = {now} \
          WHERE path IN ( \
              SELECT path FROM ist.IndexedFile \
               WHERE (status = 'discovered' \
                      OR (status = 'parsing' AND lease_until_ms < {now})) \
               ORDER BY discovered_ms \
               FOR UPDATE SKIP LOCKED \
               LIMIT {lim}) \
          RETURNING path",
        now = now_ms,
        lease = lease_ms,
        lim = limit,
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Spawn the demand-pull claim feeder for pipeline A (gated, REQ-AXO-901897).
///
/// `input_tx` is pipeline A's sink — the SAME `Sender<PathBuf>` Watchman feeds.
/// LISTEN `file_discovered` wakes the feeder on new enrolments; a periodic
/// safety tick reclaims stale-lease 'parsing' rows even if the NOTIFY was lost.
pub fn spawn_pipeline_a_claim_feeder(
    database_url: String,
    input_tx: Sender<PathBuf>,
    batch_size: usize,
) -> Arc<DemandPullMetrics> {
    let metrics = DemandPullMetrics::new();
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match demand_pull_a_loop(&database_url, &input_tx, batch_size, &metrics_clone).await {
                Ok(()) => {
                    warn!("demand-pull A: LISTEN loop exited cleanly; reconnecting");
                    backoff_ms = BACKOFF_INITIAL_MS;
                }
                Err(err) => {
                    warn!(
                        backoff_ms,
                        error = %err,
                        "demand-pull A: LISTEN errored; backing off"
                    );
                    backoff_ms = (backoff_ms * 2).min(BACKOFF_MAX_MS);
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    });
    metrics
}

async fn demand_pull_a_loop(
    database_url: &str,
    input_tx: &Sender<PathBuf>,
    batch_size: usize,
    metrics: &Arc<DemandPullMetrics>,
) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("demand-pull A: connect failed")?;

    let (notify_tx, mut notify_rx) =
        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(512);

    let driver = tokio::spawn(async move {
        let stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
        tokio::pin!(stream);
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(AsyncMessage::Notification(n)) => {
                    if notify_tx.send(n).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(error = %err, "demand-pull A: stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute("LISTEN file_discovered")
        .await
        .context("demand-pull A: LISTEN failed")?;

    let lease_ms = dbq_a_lease_ms();
    info!(
        "demand-pull A: active (claim_batch={batch_size}, lease_ms={lease_ms}) \
         — DB work queue claimer (REQ-AXO-901897)"
    );

    let mut consecutive_empty = 0u32;
    let safety_interval = Duration::from_secs(5);

    // REQ-AXO-901810 G2 — single-shot guard so a NOTIFY burst + the periodic
    // tick can't run two overlapping claim cycles on the same connection.
    let in_progress = Arc::new(AtomicBool::new(false));

    let seed = run_claim_cycle_a(
        &client,
        input_tx,
        batch_size,
        lease_ms,
        &mut consecutive_empty,
        metrics,
        &in_progress,
    )
    .await;

    let mut last_had_work = seed.had_work;
    loop {
        // When work is flowing, poll briefly; when idle, fall back to the 5 s
        // reaper cadence that reclaims stale-lease 'parsing' rows.
        let wait_duration = if last_had_work {
            Duration::from_millis(BACKOFF_INITIAL_MS)
        } else {
            safety_interval
        };

        let woke_by_notify = tokio::select! {
            biased;
            Some(_) = notify_rx.recv() => {
                // REQ-AXO-901810 G7 — coalesce the inotify burst (git checkout,
                // mass rename) into one claim cycle instead of N spin rounds.
                let coalesce_deadline =
                    tokio::time::Instant::now() + Duration::from_millis(NOTIFY_COALESCE_MS);
                while tokio::time::Instant::now() < coalesce_deadline {
                    tokio::select! {
                        biased;
                        Some(_) = notify_rx.recv() => {}
                        _ = tokio::time::sleep_until(coalesce_deadline) => break,
                    }
                }
                while notify_rx.try_recv().is_ok() {}
                true
            }
            _ = tokio::time::sleep(wait_duration) => false,
        };

        if woke_by_notify {
            consecutive_empty = 0;
        }

        let outcome = run_claim_cycle_a(
            &client,
            input_tx,
            batch_size,
            lease_ms,
            &mut consecutive_empty,
            metrics,
            &in_progress,
        )
        .await;
        last_had_work = outcome.had_work;

        if driver.is_finished() {
            return Ok(());
        }
    }
}

/// One claim cycle: claim → feed → repeat until a claim returns nothing (the
/// queue is drained or every remaining row is locked/dead-lettered).
async fn run_claim_cycle_a(
    client: &tokio_postgres::Client,
    input_tx: &Sender<PathBuf>,
    batch_size: usize,
    lease_ms: i64,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
    in_progress: &Arc<AtomicBool>,
) -> CycleOutcome {
    if in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return CycleOutcome {
            had_work: false,
            head: None,
            total_fed: 0,
        };
    }
    let mut had_work = false;
    let mut head: Option<String> = None;
    let mut total_fed = 0usize;
    loop {
        let batch =
            claim_and_feed_a(client, input_tx, batch_size, lease_ms, consecutive_empty, metrics)
                .await;
        if head.is_none() {
            head = batch.head;
        }
        total_fed += batch.fed;
        if batch.fed == 0 {
            break;
        }
        had_work = true;
    }
    in_progress.store(false, Ordering::Release);
    CycleOutcome {
        had_work,
        head,
        total_fed,
    }
}

async fn claim_and_feed_a(
    client: &tokio_postgres::Client,
    input_tx: &Sender<PathBuf>,
    batch_size: usize,
    lease_ms: i64,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
) -> PullBatch {
    // REQ-AXO-901906 — yield when A1's channel is full (the backlog drainer
    // yields to the live Watchman fast-path AND lets backpressure from the
    // bounded A-content channels propagate). Memory is bounded by those channel
    // caps + send().await (mirrors pipeline B) — no in-flight byte budget.
    if input_tx.capacity() == 0 {
        metrics.skipped_above_threshold.fetch_add(1, Ordering::Relaxed);
        return PullBatch { fed: 0, head: None };
    }

    metrics.pulls_total.fetch_add(1, Ordering::Relaxed);
    let sql = build_claim_sql(now_ms(), lease_ms, batch_size);
    let rows = match client.query(sql.as_str(), &[]).await {
        Ok(rows) => rows,
        Err(err) => {
            warn!(error = %err, "demand-pull A: claim UPDATE failed");
            return PullBatch { fed: 0, head: None };
        }
    };

    if rows.is_empty() {
        metrics.empty_pulls_total.fetch_add(1, Ordering::Relaxed);
        *consecutive_empty = consecutive_empty.saturating_add(1);
        if *consecutive_empty == IDLE_THRESHOLD {
            info!("demand-pull A: backlog drained ({IDLE_THRESHOLD} empty claims)");
        }
        return PullBatch { fed: 0, head: None };
    }

    let count = rows.len();
    let head = rows.first().map(|r| r.get::<_, String>(0));
    let mut sent = 0usize;
    for row in rows {
        let path: String = row.get(0);
        // BACKPRESSURE, never drop — send().await paces the claim feeder to
        // A1's drain rate (REQ-AXO-901891 contract). A claimed-but-unfed path
        // would otherwise sit 'parsing' until its lease expires and gets
        // reclaimed; backpressure makes that the rare crash case, not steady
        // state.
        if input_tx.send(PathBuf::from(path)).await.is_err() {
            // Receiver dropped (shutdown) — stop feeding.
            break;
        }
        sent += 1;
    }
    metrics.items_fed_total.fetch_add(sent as u64, Ordering::Relaxed);
    *consecutive_empty = 0;
    if sent > 0 {
        info!("demand-pull A: claimed+fed {sent}/{count} files to A1 (DB work queue)");
    }
    PullBatch { fed: sent, head }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    // REQ-AXO-901862 — StallTracker flags a consumer that drains the channel
    // without persisting (same pending set re-pulled), so the puller backs off
    // instead of spinning the CPU.
    #[test]
    fn stall_tracker_flags_repeated_nondraining_pull() {
        let mut t = StallTracker::new();
        // Same (head, count) re-pulled: stall trips on the STALL_REPEAT-th repeat.
        assert!(!t.observe(Some("AXO:c1".into()), 1500)); // repeat=0
        assert!(!t.observe(Some("AXO:c1".into()), 1500)); // repeat=1
        assert!(!t.observe(Some("AXO:c1".into()), 1500)); // repeat=2
        assert!(t.observe(Some("AXO:c1".into()), 1500)); // repeat=3 ⇒ stalled
        assert!(t.observe(Some("AXO:c1".into()), 1500)); // stays stalled
    }

    #[test]
    fn stall_tracker_resets_when_set_drains() {
        let mut t = StallTracker::new();
        for _ in 0..5 {
            t.observe(Some("AXO:c1".into()), 1500);
        }
        // Head advances (lowest-token chunk embedded) ⇒ not stalled, streak reset.
        assert!(!t.observe(Some("AXO:c2".into()), 1499));
        assert!(!t.observe(Some("AXO:c2".into()), 1499)); // repeat=1, not yet
    }

    #[test]
    fn stall_tracker_resets_on_empty_pull() {
        let mut t = StallTracker::new();
        for _ in 0..5 {
            t.observe(Some("AXO:c1".into()), 1500);
        }
        // Empty pull (idle / fully drained) ⇒ reset, never reports stalled.
        assert!(!t.observe(None, 0));
        assert!(!t.observe(Some("AXO:c1".into()), 1500)); // fresh streak
    }

    #[test]
    fn metrics_new_starts_at_zero() {
        let m = DemandPullMetrics::new();
        let snap = m.snapshot();
        assert_eq!(snap.pulls_total, 0);
        assert_eq!(snap.items_fed_total, 0);
        assert_eq!(snap.empty_pulls_total, 0);
        assert_eq!(snap.try_send_failures_total, 0);
        assert_eq!(snap.skipped_above_threshold, 0);
    }

    #[test]
    fn metrics_snapshot_reflects_increments() {
        let m = DemandPullMetrics::new();
        m.pulls_total.fetch_add(10, Ordering::Relaxed);
        m.items_fed_total.fetch_add(200, Ordering::Relaxed);
        m.empty_pulls_total.fetch_add(3, Ordering::Relaxed);
        m.try_send_failures_total.fetch_add(5, Ordering::Relaxed);
        m.skipped_above_threshold.fetch_add(7, Ordering::Relaxed);
        let snap = m.snapshot();
        assert_eq!(snap.pulls_total, 10);
        assert_eq!(snap.items_fed_total, 200);
        assert_eq!(snap.empty_pulls_total, 3);
        assert_eq!(snap.try_send_failures_total, 5);
        assert_eq!(snap.skipped_above_threshold, 7);
    }

    #[test]
    fn constants_are_sensible() {
        // REQ-AXO-901891 — MAX_RETRY / CLAIM_TIMEOUT_MS asserts removed with the
        // pipeline-A claim machinery (retry_count poison-pill / claim window).
        // demand_pull is now B-side only; the live consts below are what remain.
        assert!(SAFETY_POLL_SECS >= 10, "safety poll must be at least 10s");
        assert!(IDLE_THRESHOLD >= 3, "idle detection needs at least 3 empty pulls");
        // REQ-AXO-901810 G7 — coalesce must be small enough that it
        // does not perceptibly slow steady-state replenishment, but
        // large enough to actually catch inotify bursts. 10ms < x <
        // 200ms is the defensible band ; 50ms sits comfortably in it.
        assert!(
            NOTIFY_COALESCE_MS >= 10 && NOTIFY_COALESCE_MS <= 200,
            "coalesce window must be 10..200 ms",
        );
    }

    /// REQ-AXO-901810 G2 — `compare_exchange(false, true)` succeeds
    /// once for an idle guard ; a second concurrent call fails and
    /// the caller defers.
    #[test]
    fn compare_exchange_guard_admits_first_caller_and_rejects_second() {
        let guard = std::sync::Arc::new(AtomicBool::new(false));
        let first = guard
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        let second = guard
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        assert!(first, "first caller must acquire the idle guard");
        assert!(!second, "second caller must be rejected while the cycle is active");
        // Release and verify the guard is reusable.
        guard.store(false, Ordering::Release);
        let third = guard
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        assert!(third, "guard must be re-acquirable after release");
    }

    /// REQ-AXO-901810 G7 — multiple NOTIFY signals arriving within
    /// the coalesce window must drain into a single cycle, not N
    /// spin rounds. `tokio_postgres::Notification` is non-constructable
    /// in tests, so we pin the semantic on a stand-in `()` channel :
    /// after the first wake, a `try_recv` drain loop must clear every
    /// queued event in one pass.
    #[tokio::test]
    async fn coalesce_drains_burst_into_single_cycle() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(64);
        for _ in 0..32 {
            tx.try_send(()).expect("burst send must fit in channel");
        }
        let first = rx.recv().await;
        assert!(first.is_some(), "first burst event must arrive");
        let mut drained = 1;
        while rx.try_recv().is_ok() {
            drained += 1;
        }
        assert_eq!(drained, 32, "all burst events must drain in one cycle");
    }

    #[tokio::test]
    async fn threshold_check_prevents_pull_when_channel_full() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<PathBuf>(10);
        // Fill the channel to capacity.
        for i in 0..10 {
            tx.send(PathBuf::from(format!("/tmp/f{i}"))).await.unwrap();
        }
        let in_flight = tx.max_capacity() - tx.capacity();
        assert_eq!(in_flight, 10);
        // With threshold=5, in_flight(10) >= threshold(5) → should NOT pull.
        assert!(in_flight >= 5);
    }

    #[tokio::test]
    async fn threshold_check_allows_pull_when_channel_empty() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<PathBuf>(100);
        let in_flight = tx.max_capacity() - tx.capacity();
        assert_eq!(in_flight, 0);
        // With threshold=200, in_flight(0) < threshold(200) → should pull.
        assert!(in_flight < 200);
    }

    // ───────────────────────────────────────────────────────────────────
    // REQ-AXO-901897 (DBQ slice 1) — pipeline A claim feeder tests.
    // ───────────────────────────────────────────────────────────────────

    /// (a) The claim SQL is the contract. It must:
    ///   - claim 'discovered' rows,
    ///   - claim stale-lease 'parsing' rows (lease_until_ms < now) — crash recovery,
    ///   - use FOR UPDATE SKIP LOCKED (no two claimers take the same row),
    ///   - promote to 'parsing' + set a fresh lease,
    ///   - RETURNING path so the feeder can feed input_tx,
    ///   - order by discovered_ms (oldest backlog first).
    /// REQ-AXO-901906 — no retry_count / dead-letter (axed; A2 timeout + boot
    /// reset guarantee terminal status).
    #[test]
    fn claim_sql_shape_covers_the_full_contract() {
        let now = 1_000_000i64;
        let lease = 60_000i64;
        let sql = build_claim_sql(now, lease, 256);

        // Promotes to 'parsing' and sets the lease.
        assert!(sql.contains("SET status = 'parsing'"), "must promote to parsing");
        assert!(
            sql.contains(&format!("lease_until_ms = {now} + {lease}")),
            "must set a fresh lease = now + lease_ms"
        );
        assert!(sql.contains(&format!("last_attempt_ms = {now}")), "must stamp last_attempt_ms");
        assert!(!sql.contains("retry_count"), "REQ-AXO-901906 — no retry_count / dead-letter");

        // Claim set = discovered OR stale-lease parsing.
        assert!(sql.contains("status = 'discovered'"), "claims discovered rows");
        assert!(
            sql.contains(&format!("status = 'parsing' AND lease_until_ms < {now}")),
            "claims stale-lease parsing rows (crash recovery)"
        );

        // Concurrency + ordering + return contract.
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "must skip locked rows under concurrency");
        assert!(sql.contains("ORDER BY discovered_ms"), "oldest backlog first");
        assert!(sql.contains("LIMIT 256"), "honors the claim batch limit");
        assert!(sql.trim_end().ends_with("RETURNING path"), "returns claimed paths");
    }

    #[test]
    fn claim_consts_and_env_overrides_are_sane() {
        assert_eq!(DBQ_A_LEASE_MS_DEFAULT, 60_000);
        assert_eq!(DBQ_A_CLAIM_BATCH_DEFAULT, 256);
        // Env-unset resolves to the defaults.
        std::env::remove_var("AXON_DBQ_A_LEASE_MS");
        std::env::remove_var("AXON_DBQ_A_CLAIM_BATCH");
        assert_eq!(dbq_a_lease_ms(), DBQ_A_LEASE_MS_DEFAULT);
        assert_eq!(dbq_a_claim_batch(), DBQ_A_CLAIM_BATCH_DEFAULT);
    }

    /// (a-real) End-to-end against a real PG clone of the canonical template:
    /// the claim promotes 'discovered' + stale-'parsing', skips fresh-lease
    /// 'parsing' and retry-exhausted rows, and is idempotent on re-claim.
    /// REQ-AXO-901897.
    #[tokio::test]
    async fn claim_against_real_pg_selects_only_claimable_rows() {
        let store = std::sync::Arc::new(
            crate::tests::test_helpers::create_test_db().unwrap(),
        );
        let now = now_ms();
        // REQ-AXO-901877 — shared per-process clone: scrub any residual rows we
        // own (left by a prior panicked run) before seeding so the claim/idempotency
        // assertions below see only this test's /tmp/dbq_a/ rows.
        let _ = store.execute("DELETE FROM ist.IndexedFile WHERE path LIKE '/tmp/dbq_a/%'");
        // Seed four rows with distinct lifecycle states (all project AXO).
        // 1. discovered           → claimable
        // 2. stale-lease parsing  → claimable (crash recovery)
        // 3. fresh-lease parsing  → NOT claimable (another worker owns it)
        // 4. discovered, retry=3  → claimable (REQ-AXO-901906: no dead-letter)
        let seed = format!(
            "INSERT INTO ist.IndexedFile \
                (path, project_code, content_hash, last_seen_ms, status, discovered_ms, retry_count, lease_until_ms) VALUES \
                ('/tmp/dbq_a/discovered.rs','AXO','',{now},'discovered',1,0,0), \
                ('/tmp/dbq_a/stale.rs','AXO','',{now},'parsing',2,1,{stale}), \
                ('/tmp/dbq_a/fresh.rs','AXO','',{now},'parsing',3,1,{fresh}), \
                ('/tmp/dbq_a/dead.rs','AXO','',{now},'discovered',4,3,0);",
            now = now,
            stale = now - 1, // lease already expired
            fresh = now + 600_000, // lease far in the future
        );
        store.execute(&seed).unwrap();

        let sql = build_claim_sql(now, 60_000, 256);
        let raw = store.query_json_writer(&sql).unwrap();
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut claimed: Vec<String> = rows
            .iter()
            .filter_map(|r| r.first()?.as_str().map(str::to_string))
            // Scope to this test's own prefix: the claim query (correctly) claims
            // EVERY claimable path, so on the shared clone a sibling row could leak
            // in. Our invariant is about /tmp/dbq_a/ rows only.
            .filter(|p| p.starts_with("/tmp/dbq_a/"))
            .collect();
        claimed.sort();
        assert_eq!(
            claimed,
            vec![
                "/tmp/dbq_a/dead.rs".to_string(),
                "/tmp/dbq_a/discovered.rs".to_string(),
                "/tmp/dbq_a/stale.rs".to_string(),
            ],
            "claims discovered + stale-lease parsing + high-retry (no dead-letter); excludes only fresh-lease"
        );

        // (d) Idempotent re-claim: the two just-claimed rows are now fresh-lease
        // 'parsing' (this claim used lease 60_000), so a second claim with the
        // same `now` returns NOTHING — no double-feed.
        let raw2 = store.query_json_writer(&build_claim_sql(now, 60_000, 256)).unwrap();
        let rows2: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw2).unwrap_or_default();
        let reclaimed_own: Vec<String> = rows2
            .iter()
            .filter_map(|r| r.first()?.as_str().map(str::to_string))
            .filter(|p| p.starts_with("/tmp/dbq_a/"))
            .collect();
        assert!(
            reclaimed_own.is_empty(),
            "re-claim with same now must be a no-op for our rows (fresh lease) — idempotent; got {reclaimed_own:?}"
        );

        // Cleanup (shared template clone is per-process; keep it tidy).
        let _ = store.execute("DELETE FROM ist.IndexedFile WHERE path LIKE '/tmp/dbq_a/%'");
    }

}
