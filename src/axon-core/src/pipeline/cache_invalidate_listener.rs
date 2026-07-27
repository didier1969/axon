// REQ-AXO-902262 — LISTEN ist_cache_invalidate: purge the indexer's in-RAM dedup cache
// on demand.
//
// The defect this closes, measured in production
// ----------------------------------------------
// `rescan_project full=true` promises to "wipe IndexedFile rows … so every file is forced
// through A1/A2/A3 + B1/B2/B3 again", and answers `cache_invalidation: "wiped (full mode)"`.
// Both statements are true about POSTGRES and irrelevant to the outcome, because the cache
// that DECIDES whether a file is re-read is `IndexedFileCache` — a DashMap living in the
// INDEXER's RAM, hydrated once at boot (`pipeline_runtime.rs`, `load_all_indexed_files`).
// The MCP tool runs in the BRAIN. Two processes: the tool could not reach the thing that
// decides.
//
// Observed sequence on LLL (434/434 files chunked → 2/438, no automatic recovery):
//   1. full=true DELETEs the chunks and the IndexedFile rows.
//   2. The walk re-enrols the 438 rows with the REAL on-disk mtime/size.
//   3. A1 asks the RAM cache, which still holds the identical (mtime, size) → "unchanged"
//      → SKIP. The 15-minute reconciliation walk then replays that skip forever.
// Net: a tool that destroys data and is structurally unable to rebuild it, while reporting
// `status: ok`.
//
// Same two-process shape as REQ-AXO-902234 (idle-drop), so the same mechanism: the brain
// emits a `pg_notify`, this listener applies it inside the indexer. No restart needed —
// which matters, since "restart the indexer" was the only manual workaround and it costs a
// GPU teardown.
//
// Shape mirrors `embedder_control_listener.rs`: dedicated `tokio_postgres` connection
// outside the deadpool, forever-reconnect with exponential backoff.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::pipeline::IndexedFileCache;

pub const LISTEN_CHANNEL: &str = "ist_cache_invalidate";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;

/// Apply one invalidation payload. PURE apart from the cache mutation, so the payload
/// contract is unit-testable without a database.
///
/// The payload is the absolute path PREFIX to forget (the project root). An empty or
/// whitespace-only payload is REFUSED rather than treated as "everything": a malformed
/// notification must not be able to blank the whole dedup cache and trigger a full
/// re-index of every project on the host.
pub fn apply_invalidate_payload(payload: &str) -> Option<usize> {
    let prefix = payload.trim();
    if prefix.is_empty() || prefix == "/" {
        warn!(
            payload = %payload,
            "ist_cache_invalidate: refusing an empty/root prefix — that would re-index every project"
        );
        return None;
    }
    let cache = IndexedFileCache::global()?;
    let forgotten = cache.forget_prefix(prefix);
    // REQ-AXO-902268 — purging only makes the files ELIGIBLE to be re-read; the re-read
    // happens on the reconciliation walk. Wake it now instead of waiting out its period
    // (900 s default), which used to leave the project at zero coverage for up to 15 min.
    crate::pipeline::indexed_file_cache::walk_wake_signal().notify_one();
    Some(forgotten)
}

/// Supervised listener. Returns immediately, then reconnects forever on error.
pub fn spawn_cache_invalidate_listener(database_url: String) {
    tokio::spawn(async move {
        let mut backoff = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&database_url).await {
                Ok(()) => {
                    warn!(channel = LISTEN_CHANNEL, "LISTEN loop exited cleanly; reconnecting");
                    backoff = BACKOFF_INITIAL_MS;
                }
                Err(err) => {
                    warn!(
                        channel = LISTEN_CHANNEL,
                        error = %err,
                        backoff_ms = backoff,
                        "LISTEN errored; backing off"
                    );
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff)).await;
            backoff = (backoff * 2).min(BACKOFF_MAX_MS);
        }
    });
}

async fn listen_once(database_url: &str) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("LISTEN connect failed")?;

    let (notify_tx, mut notify_rx) =
        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(256);

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
                    warn!(error = %err, "ist_cache_invalidate stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN ist_cache_invalidate failed")?;
    info!(channel = LISTEN_CHANNEL, "ist_cache_invalidate listener attached");

    while let Some(n) = notify_rx.recv().await {
        if let Some(forgotten) = apply_invalidate_payload(n.payload()) {
            info!(
                prefix = n.payload(),
                forgotten,
                "ist_cache_invalidate: dedup cache purged + reconciliation walk woken — re-read starts now, not at the next period (REQ-AXO-902262 / REQ-AXO-902268)"
            );
        }
    }

    drop(client);
    let _ = driver.await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::apply_invalidate_payload;

    /// The blast-radius guard. A malformed or empty notification must NEVER be read as
    /// "forget everything": that would silently trigger a full re-index (and re-embed) of
    /// every project on the host. Returning None here means "did nothing", which is the
    /// correct response to an instruction that cannot be trusted.
    #[test]
    fn empty_or_root_prefix_is_refused() {
        assert!(apply_invalidate_payload("").is_none());
        assert!(apply_invalidate_payload("   ").is_none());
        assert!(apply_invalidate_payload("/").is_none());
        assert!(apply_invalidate_payload("\n\t ").is_none());
    }

    /// With no global cache published (the brain, or before the indexer finishes booting)
    /// a well-formed payload is a clean no-op, not an error. The listener must never make
    /// a process that has nothing to invalidate look broken.
    #[test]
    fn well_formed_prefix_without_a_published_cache_is_a_noop() {
        // The indexer runtime is what publishes the global; no pipeline runs under the
        // unit-test harness, so this exercises exactly that path.
        assert!(apply_invalidate_payload("/home/dstadel/projects/llmlang").is_none());
    }
}
