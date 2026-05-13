//! NOTIFY listener for `chunk_pending_embed` (DEC-AXO-086 slice 1B).
//!
//! Opens a dedicated `tokio_postgres` connection (outside the deadpool),
//! issues `LISTEN chunk_pending_embed`, and forwards every received
//! notification's payload (a chunk_id) into the B1 inbox channel.
//!
//! Pairs with the trigger `trg_chunk_notify_pending` in
//! `db/ddl/03_ist_schema.sql` which fires `pg_notify` post-commit on
//! every `public.Chunk` INSERT or `content_hash` UPDATE.
//!
//! Resilience : on connection drop / channel close, loops forever with
//! exponential backoff (200ms → 30s cap). The cold-start poll task
//! continues to act as the safety net for any NOTIFY lost between
//! reconnects.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

const LISTEN_CHANNEL: &str = "chunk_pending_embed";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
const NOTIFY_FORWARD_BUFFER: usize = 2048;
const LOG_EVERY_N_NOTIFICATIONS: u64 = 1000;

/// Spawn the supervised listener loop. Returns immediately.
///
/// `database_url` is consumed (cloned) for the loop body. The listener
/// reconnects on any error using exponential backoff. `b1_inbox_tx` is
/// the same channel A3 try-sends to and `b1_cold_start_poll` forwards
/// to — three independent producers, single consumer pool (pipeline B).
pub fn spawn_chunk_pending_listener(database_url: String, b1_inbox_tx: Sender<String>) {
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&database_url, &b1_inbox_tx).await {
                Ok(()) => {
                    warn!(
                        channel = LISTEN_CHANNEL,
                        "LISTEN loop exited cleanly; reconnecting"
                    );
                    backoff_ms = BACKOFF_INITIAL_MS;
                }
                Err(err) => {
                    warn!(
                        channel = LISTEN_CHANNEL,
                        backoff_ms,
                        error = %err,
                        "LISTEN errored; backing off"
                    );
                    backoff_ms = (backoff_ms * 2).min(BACKOFF_MAX_MS);
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
    });
}

async fn listen_once(database_url: &str, b1_inbox_tx: &Sender<String>) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("LISTEN connect failed")?;

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
                    warn!(error = %e, "LISTEN connection driver error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN command failed")?;
    info!(channel = LISTEN_CHANNEL, "PG NOTIFY listener active");

    let mut received_total: u64 = 0;
    while let Some(notification) = notify_rx.recv().await {
        let chunk_id = notification.payload();
        if chunk_id.is_empty() {
            continue;
        }
        if b1_inbox_tx.send(chunk_id.to_string()).await.is_err() {
            driver.abort();
            return Err(anyhow!("b1_inbox closed; stopping listener"));
        }
        received_total += 1;
        if received_total % LOG_EVERY_N_NOTIFICATIONS == 0 {
            info!(received_total, "NOTIFY forwarder cumulative count");
        }
    }

    driver.abort();
    Err(anyhow!("LISTEN connection closed (driver exited)"))
}
