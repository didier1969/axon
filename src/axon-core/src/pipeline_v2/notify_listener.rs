//! State-only NOTIFY listener for `chunk_pending_embed` (REQ-AXO-90009 Slice 2).
//!
//! Brain-side variant : keeps `EmbedderRuntimeState` in sync so
//! `retrieve_context`'s freshness gate reflects what the indexer just
//! wrote. Used when brain + indexer run in separate processes
//! (canonical PIL-AXO-008 dual-product topology).
//!
//! Pairs with the trigger `trg_chunk_notify_pending` in
//! `db/ddl/03_ist_schema.sql` which fires `pg_notify` post-commit on
//! every `ist.Chunk` INSERT or `content_hash` UPDATE.
//!
//! Resilience : on connection drop / channel close, loops forever with
//! exponential backoff (200ms → 30s cap).

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::stream::StreamExt;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::embedder::lifecycle::process_state as embedder_state;

pub const LISTEN_CHANNEL: &str = "chunk_pending_embed";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
const NOTIFY_FORWARD_BUFFER: usize = 2048;

/// REQ-AXO-90009 Slice 2 — brain-side variant of the listener. No B1
/// inbox, no Pipeline B in this process : the listener exists solely
/// to keep `EmbedderRuntimeState` in sync so `retrieve_context`'s
/// freshness gate reflects what the indexer just wrote. Useful when
/// brain + indexer run in separate processes (canonical PIL-AXO-008
/// dual-product topology).
pub fn spawn_chunk_pending_state_listener(database_url: String) {
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_state_only(&database_url).await {
                Ok(()) => {
                    warn!(
                        channel = LISTEN_CHANNEL,
                        "state-only LISTEN loop exited cleanly; reconnecting"
                    );
                    backoff_ms = BACKOFF_INITIAL_MS;
                }
                Err(err) => {
                    warn!(
                        channel = LISTEN_CHANNEL,
                        backoff_ms,
                        error = %err,
                        "state-only LISTEN errored; backing off"
                    );
                    backoff_ms = (backoff_ms * 2).min(BACKOFF_MAX_MS);
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    });
}

async fn listen_state_only(database_url: &str) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("LISTEN state-only connect failed")?;
    let (notify_tx, mut notify_rx) =
        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(NOTIFY_FORWARD_BUFFER);
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
                Err(e) => {
                    warn!(error = %e, "state-only LISTEN connection driver error");
                    return;
                }
            }
        }
    });
    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN command failed")?;
    info!(channel = LISTEN_CHANNEL, "state-only NOTIFY listener active");

    while let Some(notification) = notify_rx.recv().await {
        let chunk_id = notification.payload();
        if !chunk_id.is_empty() {
            embedder_state().mark_pending(chunk_id);
        }
    }
    driver.abort();
    Err(anyhow!(
        "state-only LISTEN connection closed (driver exited)"
    ))
}

/// REQ-AXO-90009 Slice 2 — reconcile loop. Every `interval` (with
/// jitter), re-hydrate the pending set from PG (`SELECT chunk_id FROM
/// Chunk WHERE NOT EXISTS (matching ChunkEmbedding row)`) to recover
/// from NOTIFY drops or LISTEN reconnect gaps. Strictly **additive** :
/// re-hydration unions with the in-memory set ; never removes (B3 is
/// the only authority that clears chunk_ids via `mark_embedded`).
///
/// Caller passes a closure that returns the orphan chunk_ids. Keeping
/// the SQL out of this module makes it unit-testable without a live PG
/// connection. The closure is invoked inside `spawn_blocking` so a
/// slow PG call doesn't stall the tokio runtime.
pub fn spawn_pending_reconcile_loop<F>(interval: Duration, jitter: Duration, fetch_orphans: F)
where
    F: Fn() -> anyhow::Result<Vec<String>> + Send + Sync + 'static,
{
    tokio::spawn(async move {
        // First tick fires immediately so the boot-time hydration runs
        // without waiting `interval`. Subsequent ticks honour interval.
        let mut next_tick = tokio::time::interval(interval);
        next_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let fetch_arc = std::sync::Arc::new(fetch_orphans);
        loop {
            next_tick.tick().await;
            // Random jitter ∈ [0, jitter) so concurrent indexer
            // restarts don't reconcile in lock-step. Uses the
            // monotonic clock's sub-millisecond noise as entropy source
            // — good enough for de-syncing two processes, no need for
            // a real RNG dependency.
            let jitter_ms = if jitter.is_zero() {
                0
            } else {
                let entropy = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64)
                    .unwrap_or(0);
                entropy % (jitter.as_millis().max(1) as u64)
            };
            if jitter_ms > 0 {
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
            }
            let fetch_for_blocking = fetch_arc.clone();
            let result = tokio::task::spawn_blocking(move || (fetch_for_blocking)()).await;
            match result {
                Ok(Ok(orphans)) => {
                    let state = embedder_state();
                    for cid in &orphans {
                        state.mark_pending(cid.clone());
                    }
                    info!(
                        orphans = orphans.len(),
                        pending_count = state.pending_count(),
                        "pending reconcile tick"
                    );
                }
                Ok(Err(err)) => {
                    warn!(error = %err, "pending reconcile fetch failed");
                }
                Err(join_err) => {
                    warn!(error = ?join_err, "pending reconcile blocking task joined with error");
                }
            }
        }
    });
}

