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
use std::sync::atomic::{AtomicU64, Ordering};
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

    pull_and_feed_a(store, input_tx, threshold, batch_size, &mut consecutive_empty, metrics).await;

    loop {
        let woke_by_notify = tokio::select! {
            biased;
            Some(_) = notify_rx.recv() => {
                while notify_rx.try_recv().is_ok() {}
                true
            }
            _ = tokio::time::sleep(safety_interval) => {
                false
            }
        };

        if woke_by_notify {
            consecutive_empty = 0;
        }

        loop {
            let pulled =
                pull_and_feed_a(store, input_tx, threshold, batch_size, &mut consecutive_empty, metrics)
                    .await;
            if pulled == 0 {
                break;
            }
        }

        if driver.is_finished() {
            return Ok(());
        }
    }
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
            let mut dropped = 0usize;
            for path_str in paths {
                match input_tx.try_send(PathBuf::from(&path_str)) {
                    Ok(()) => sent += 1,
                    Err(_) => { dropped += 1; break; }
                }
            }
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
pub fn spawn_pipeline_b_demand_pull(
    store: Arc<GraphStore>,
    database_url: String,
    b1_inbox_tx: Sender<super::stage_b1::B1InboxItem>,
    threshold: usize,
    batch_size: usize,
) -> Arc<DemandPullMetrics> {
    let metrics = DemandPullMetrics::new();
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match demand_pull_b_loop(&store, &database_url, &b1_inbox_tx, threshold, batch_size, &metrics_clone)
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
    b1_inbox_tx: &Sender<super::stage_b1::B1InboxItem>,
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

    pull_and_feed_b(store, b1_inbox_tx, threshold, batch_size, &mut consecutive_empty, metrics).await;

    loop {
        let woke_by_notify = tokio::select! {
            biased;
            Some(_) = notify_rx.recv() => {
                while notify_rx.try_recv().is_ok() {}
                true
            }
            _ = tokio::time::sleep(safety_interval) => {
                false
            }
        };

        if woke_by_notify {
            consecutive_empty = 0;
        }

        loop {
            let pulled = pull_and_feed_b(
                store,
                b1_inbox_tx,
                threshold,
                batch_size,
                &mut consecutive_empty,
                metrics,
            )
            .await;
            if pulled == 0 {
                break;
            }
        }

        if driver.is_finished() {
            return Ok(());
        }
    }
}

async fn pull_and_feed_b(
    store: &Arc<GraphStore>,
    b1_inbox_tx: &Sender<super::stage_b1::B1InboxItem>,
    threshold: usize,
    batch_size: usize,
    consecutive_empty: &mut u32,
    metrics: &Arc<DemandPullMetrics>,
) -> usize {
    let in_flight = b1_inbox_tx.max_capacity() - b1_inbox_tx.capacity();
    if in_flight >= threshold {
        metrics.skipped_above_threshold.fetch_add(1, Ordering::Relaxed);
        return 0;
    }

    metrics.pulls_total.fetch_add(1, Ordering::Relaxed);
    let store_clone = store.clone();
    let result = tokio::task::spawn_blocking(move || {
        store_clone.select_chunks_needing_embedding(batch_size)
    })
    .await;

    match result {
        Ok(Ok(chunk_ids)) if !chunk_ids.is_empty() => {
            let count = chunk_ids.len();
            let mut sent = 0usize;
            let mut dropped = 0usize;
            for cid in chunk_ids {
                match b1_inbox_tx.try_send(super::stage_b1::B1InboxItem::FetchById(cid)) {
                    Ok(()) => sent += 1,
                    Err(_) => { dropped += 1; break; }
                }
            }
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
