// REQ-AXO-91487 (MIL-AXO-019 slice 3) — LISTEN ist_mutated.
//
// Opens a dedicated `tokio_postgres` connection (outside the deadpool),
// issues `LISTEN ist_mutated`, and evicts the affected project from the
// process IstSnapshotCache on each notification. The next read for that
// project triggers a fresh cold-load via `ist_snapshot_warm` (operator)
// or whatever caller next hits a migrated call-site. v1 eviction-only ;
// the LSM overlay path (CSR + Vec + tombstones) is a follow-up REQ once
// mutation rate exceeds 5 % of the snapshot per cold-load window.
//
// Resilience : on connection drop / channel close, loops forever with
// exponential backoff (200ms → 30s cap) — same shape as the existing
// `chunk_pending_embed` listener in pipeline_v2/notify_listener.rs.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use serde::Deserialize;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::ist_snapshot::evict_process_snapshot;

const LISTEN_CHANNEL: &str = "ist_mutated";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
const COALESCE_WINDOW_MS: u64 = 50;

#[derive(Debug, Deserialize)]
struct IstNotifyPayload {
    #[serde(default)]
    project_code: String,
    #[serde(default)]
    _op: String,
    #[serde(default, rename = "table")]
    _table_name: String,
}

/// Supervised listener loop. Returns immediately ; reconnects forever on
/// errors. Activates only when [`IstSnapshotCache::is_enabled`] reports
/// true at startup (the trigger fires regardless, the listener is the
/// no-op short-circuit when RAM dispatch is off).
pub fn spawn_ist_mutation_listener(database_url: String) {
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&database_url).await {
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

async fn listen_once(database_url: &str) -> Result<()> {
    let (client, mut connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("LISTEN connect failed")?;

    let (notify_tx, mut notify_rx) =
        tokio::sync::mpsc::channel::<tokio_postgres::Notification>(2048);

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
                    warn!(error = %err, "ist_mutated stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN ist_mutated failed")?;
    info!(channel = LISTEN_CHANNEL, "ist_mutated listener attached");

    loop {
        // Coalesce bursts : if a notification lands, wait COALESCE_WINDOW_MS
        // and drain the queue before evicting, so a transactional bulk
        // INSERT (e.g. parser-driven 500 edges per file) evicts the
        // project exactly once instead of 500 times.
        let first = match notify_rx.recv().await {
            Some(n) => n,
            None => break,
        };
        let mut projects: HashSet<String> = HashSet::new();
        push_payload(&first.payload(), &mut projects);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(COALESCE_WINDOW_MS);
        while let Ok(maybe) = tokio::time::timeout_at(deadline, notify_rx.recv()).await {
            match maybe {
                Some(n) => push_payload(&n.payload(), &mut projects),
                None => break,
            }
        }
        for project in &projects {
            evict_process_snapshot(project);
        }
    }
    drop(client);
    let _ = driver.await;
    Ok(())
}

fn push_payload(raw: &str, out: &mut HashSet<String>) {
    match serde_json::from_str::<IstNotifyPayload>(raw) {
        Ok(p) => {
            if !p.project_code.is_empty() {
                out.insert(p.project_code);
            }
        }
        Err(_) => {
            // Malformed payload — skip silently ; logging here is
            // noisy because misconfigured triggers would flood logs.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_payload_extracts_project_code() {
        let mut set = HashSet::new();
        push_payload(
            r#"{"op":"insert","project_code":"AXO","table":"edge"}"#,
            &mut set,
        );
        assert!(set.contains("AXO"));
    }

    #[test]
    fn push_payload_dedup_across_calls() {
        let mut set = HashSet::new();
        push_payload(
            r#"{"op":"insert","project_code":"AXO","table":"edge"}"#,
            &mut set,
        );
        push_payload(
            r#"{"op":"insert","project_code":"AXO","table":"symbol"}"#,
            &mut set,
        );
        push_payload(
            r#"{"op":"update","project_code":"OPT","table":"edge"}"#,
            &mut set,
        );
        assert_eq!(set.len(), 2);
        assert!(set.contains("AXO"));
        assert!(set.contains("OPT"));
    }

    #[test]
    fn push_payload_ignores_empty_project_code() {
        let mut set = HashSet::new();
        push_payload(
            r#"{"op":"insert","project_code":"","table":"edge"}"#,
            &mut set,
        );
        assert!(set.is_empty());
    }

    #[test]
    fn push_payload_ignores_malformed_json() {
        let mut set = HashSet::new();
        push_payload("not json", &mut set);
        push_payload("{}", &mut set);
        push_payload("null", &mut set);
        assert!(set.is_empty());
    }
}
