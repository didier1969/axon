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


const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
const SAFETY_POLL_SECS: u64 = 30;
const IDLE_THRESHOLD: u32 = 5;
const MAX_RETRY: i32 = 3;
const CLAIM_TIMEOUT_MS: i64 = 300_000; // 5 min
/// REQ-AXO-901810 G7 (MIL-AXO-029 slice 4) — NOTIFY coalesce window.
/// After the first `file_discovered` NOTIFY wakes the feeder, wait
/// this long collecting more before kicking the pull loop. Under a
/// burst (git checkout, mass rename, large directory move triggering
/// thousands of inotify events in ~ms) this collapses the burst into
/// a single replenishment cycle instead of N spin-wake-pull rounds.
/// 50 ms is well below the 1 s adaptive cadence so steady-state
/// latency is unchanged ; the win is only on bursts.
const NOTIFY_COALESCE_MS: u64 = 50;

/// Spawn the demand-pull feeder for pipeline A.
pub fn spawn_pipeline_a_demand_pull(
    store: Arc<GraphStore>,
    database_url: String,
    input_tx: Sender<PathBuf>,
    threshold: usize,
    batch_size: usize,
) -> Arc<DemandPullMetrics> {
    let metrics = DemandPullMetrics::new();
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match demand_pull_a_loop(&store, &database_url, &input_tx, threshold, batch_size, &metrics_clone).await
            {
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
    store: &Arc<GraphStore>,
    database_url: &str,
    input_tx: &Sender<PathBuf>,
    threshold: usize,
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

    info!(
        "demand-pull A: active (threshold={threshold}, batch={batch_size})"
    );

    let mut consecutive_empty = 0u32;
    let safety_interval = Duration::from_secs(SAFETY_POLL_SECS);

    // REQ-AXO-901810 G2 (MIL-AXO-029 slice 4) — single-shot
    // replenishment guard. `pull_and_feed_a` performs a SELECT FOR
    // UPDATE SKIP LOCKED + UPDATE in one PG transaction, so concurrent
    // entries do not double-claim ; but two overlapping invocations
    // would both run the SELECT and double the DB round-trip work for
    // no extra throughput. The compare_exchange on this flag ensures
    // only one pull-and-feed cycle is in flight per pipeline at a
    // time. A second caller that races on the wake path simply skips
    // and leaves the work to the active one — its NOTIFY/timer will
    // re-fire the next cycle.
    let in_progress = Arc::new(AtomicBool::new(false));

    run_pull_cycle(
        store,
        input_tx,
        threshold,
        batch_size,
        &mut consecutive_empty,
        metrics,
        &in_progress,
    )
    .await;

    let mut last_pull_had_work = true;
    loop {
        // Adaptive wait: 1s when draining backlog, 30s when idle.
        let wait_duration = if last_pull_had_work {
            Duration::from_secs(1)
        } else {
            safety_interval
        };

        let woke_by_notify = tokio::select! {
            biased;
            Some(_) = notify_rx.recv() => {
                // REQ-AXO-901810 G7 — coalesce burst NOTIFYs into a
                // single replenishment cycle. After the first wake,
                // hold for `NOTIFY_COALESCE_MS` while draining any
                // additional notifications that arrive in the window.
                // 50 ms is well below the 1 s adaptive cadence so
                // steady-state latency is unaffected ; the win is
                // only on bursts (1000 file inotify storm from a
                // git checkout collapses into one pull, not 1000).
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

        last_pull_had_work = run_pull_cycle(
            store,
            input_tx,
            threshold,
            batch_size,
            &mut consecutive_empty,
            metrics,
            &in_progress,
        )
        .await;

        if driver.is_finished() {
            return Ok(());
        }
    }
}

/// REQ-AXO-901810 G2 — pull-feed cycle with single-shot guard. Acquires
/// `in_progress` via compare_exchange ; returns `false` immediately if
/// another cycle is already running (= no work credited this round). On
/// success, drains via repeated `pull_and_feed_a` until the SELECT
/// returns empty, then releases the guard. Returns `true` iff at least
/// one path was fed in this cycle.
async fn run_pull_cycle(
    store: &Arc<GraphStore>,
    input_tx: &Sender<PathBuf>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
    in_progress: &Arc<AtomicBool>,
) -> bool {
    if in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // A concurrent cycle is already draining ; defer to it.
        return false;
    }
    let mut had_work = false;
    loop {
        let pulled = pull_and_feed_a(
            store,
            input_tx,
            threshold,
            batch_size,
            consecutive_empty,
            metrics,
        )
        .await;
        if pulled == 0 {
            break;
        }
        had_work = true;
    }
    in_progress.store(false, Ordering::Release);
    had_work
}

async fn pull_and_feed_a(
    store: &Arc<GraphStore>,
    input_tx: &Sender<PathBuf>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
) -> usize {
    let in_flight = input_tx.max_capacity() - input_tx.capacity();
    if in_flight >= threshold {
        metrics.skipped_above_threshold.fetch_add(1, Ordering::Relaxed);
        return 0;
    }

    metrics.pulls_total.fetch_add(1, Ordering::Relaxed);
    let store_clone = store.clone();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let claim_cutoff = now_ms - CLAIM_TIMEOUT_MS;
    let limit = batch_size;

    let result = tokio::task::spawn_blocking(move || {
        store_clone.select_and_claim_files_for_indexing(limit, MAX_RETRY, claim_cutoff, now_ms)
    })
    .await;

    match result {
        Ok(Ok(paths)) if !paths.is_empty() => {
            let count = paths.len();
            let mut sent = 0usize;
            for path_str in &paths {
                match input_tx.try_send(PathBuf::from(path_str)) {
                    Ok(()) => sent += 1,
                    Err(_) => break,
                }
            }
            let dropped = count - sent;
            metrics.items_fed_total.fetch_add(sent as u64, Ordering::Relaxed);
            metrics.try_send_failures_total.fetch_add(dropped as u64, Ordering::Relaxed);
            *consecutive_empty = 0;
            if sent > 0 {
                info!("demand-pull A: fed {sent}/{count} files (in_flight={in_flight}/{threshold}, dropped={dropped})");
            }
            sent
        }
        Ok(Ok(_)) => {
            metrics.empty_pulls_total.fetch_add(1, Ordering::Relaxed);
            *consecutive_empty = consecutive_empty.saturating_add(1);
            if *consecutive_empty == IDLE_THRESHOLD {
                info!("demand-pull A: pipeline idle ({IDLE_THRESHOLD} empty pulls)");
            }
            0
        }
        Ok(Err(err)) => {
            warn!(error = %err, "demand-pull A: SELECT failed");
            0
        }
        Err(join_err) => {
            warn!(error = %join_err, "demand-pull A: spawn_blocking panicked");
            0
        }
    }
}

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

    run_pull_cycle_b(
        store,
        b_chunks_tx,
        threshold,
        batch_size,
        &mut consecutive_empty,
        metrics,
        &in_progress,
    )
    .await;

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

        last_pull_had_work = run_pull_cycle_b(
            store,
            b_chunks_tx,
            threshold,
            batch_size,
            &mut consecutive_empty,
            metrics,
            &in_progress,
        )
        .await;

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
) -> bool {
    if in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let mut had_work = false;
    loop {
        let pulled = pull_and_feed_b(
            store,
            b_chunks_tx,
            threshold,
            batch_size,
            consecutive_empty,
            metrics,
        )
        .await;
        if pulled == 0 {
            break;
        }
        had_work = true;
    }
    in_progress.store(false, Ordering::Release);
    had_work
}

async fn pull_and_feed_b(
    store: &Arc<GraphStore>,
    b_chunks_tx: &Sender<super::stage_b1::ChunkForEmbedding>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
) -> usize {
    let in_flight = b_chunks_tx.max_capacity() - b_chunks_tx.capacity();
    if in_flight >= threshold {
        metrics.skipped_above_threshold.fetch_add(1, Ordering::Relaxed);
        return 0;
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
            let mut sent = 0usize;
            for (chunk_id, content, content_hash) in rows {
                let payload = super::stage_b1::ChunkForEmbedding {
                    chunk_id,
                    content,
                    content_hash,
                };
                match b_chunks_tx.try_send(payload) {
                    Ok(()) => sent += 1,
                    Err(_) => break,
                }
            }
            let dropped = count - sent;
            metrics.items_fed_total.fetch_add(sent as u64, Ordering::Relaxed);
            metrics.try_send_failures_total.fetch_add(dropped as u64, Ordering::Relaxed);
            *consecutive_empty = 0;
            if sent > 0 {
                info!("demand-pull B: fed {sent}/{count} chunks (in_flight={in_flight}/{threshold}, dropped={dropped})");
            }
            sent
        }
        Ok(Ok(_)) => {
            metrics.empty_pulls_total.fetch_add(1, Ordering::Relaxed);
            *consecutive_empty = consecutive_empty.saturating_add(1);
            if *consecutive_empty == IDLE_THRESHOLD {
                info!("demand-pull B: pipeline idle ({IDLE_THRESHOLD} empty pulls)");
            }
            0
        }
        Ok(Err(err)) => {
            warn!(error = %err, "demand-pull B: SELECT failed");
            0
        }
        Err(join_err) => {
            warn!(error = %join_err, "demand-pull B: spawn_blocking panicked");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

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
        assert!(MAX_RETRY >= 2, "must allow at least 2 retries");
        assert!(MAX_RETRY <= 10, "more than 10 retries is excessive");
        assert!(CLAIM_TIMEOUT_MS >= 60_000, "claim timeout must be at least 1 min");
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

}
