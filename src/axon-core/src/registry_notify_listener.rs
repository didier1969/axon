// REQ-AXO-901675 (PIL-AXO-008) — LISTEN axon_registry_changed.
//
// Opens a dedicated `tokio_postgres` connection (outside the deadpool),
// issues `LISTEN axon_registry_changed`, and pushes a subtree hint into
// the shared `IngressBuffer` so the scanner discovers and indexes the
// newly registered project's files **without** an indexer restart.
//
// PG trigger (`soll.fn_registry_notify` in `db/ddl/07_registry_notify.sql`)
// fires `pg_notify('axon_registry_changed', json{op,project_code,project_path})`
// on every INSERT/UPDATE to `soll.ProjectCodeRegistry`.
//
// Payload shape:
//   {"op":"insert|update","project_code":"AXO","project_path":"/path/to/proj"}
//
// Resilience : on connection drop / channel close, loops forever with
// exponential backoff (200ms → 30s cap) — same shape as the existing
// `ist_mutated` listener in `ist_snapshot/notify_listener.rs`.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use serde::Deserialize;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::ingress_buffer::{IngressSource, SharedIngressBuffer};

const LISTEN_CHANNEL: &str = "axon_registry_changed";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
const COALESCE_WINDOW_MS: u64 = 50;
/// Subtree hint priority for registry-driven scans. Mid-range — registry
/// mutations are operator-intentional (axon_init_project) so they deserve
/// prompt processing, but should not preempt active watcher events for
/// existing projects.
const REGISTRY_SUBTREE_HINT_PRIORITY: i64 = 100;

#[derive(Debug, Deserialize, Clone)]
struct RegistryNotifyPayload {
    #[serde(default)]
    op: String,
    #[serde(default)]
    project_code: String,
    #[serde(default)]
    project_path: String,
}

/// Supervised listener loop. Returns immediately ; reconnects forever on
/// errors. The PG trigger fires regardless ; this listener is the
/// no-op short-circuit when the indexer is not running.
pub fn spawn_registry_change_listener(
    database_url: String,
    ingress_buffer: SharedIngressBuffer,
) {
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&database_url, ingress_buffer.clone()).await {
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

async fn listen_once(
    database_url: &str,
    ingress_buffer: SharedIngressBuffer,
) -> Result<()> {
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
                    warn!(error = %err, "axon_registry_changed stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN axon_registry_changed failed")?;
    info!(
        channel = LISTEN_CHANNEL,
        "axon_registry_changed listener attached (REQ-AXO-901675)"
    );

    loop {
        let first = match notify_rx.recv().await {
            Some(n) => n,
            None => break,
        };
        let mut payloads: Vec<RegistryNotifyPayload> = Vec::new();
        push_payload(first.payload(), &mut payloads);
        // Coalesce bursts : an operator running `axon_init_project` against
        // multiple projects in quick succession lands here as a small
        // burst. Drain the queue for COALESCE_WINDOW_MS before pushing hints.
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(COALESCE_WINDOW_MS);
        while let Ok(maybe) = tokio::time::timeout_at(deadline, notify_rx.recv()).await {
            match maybe {
                Some(n) => push_payload(n.payload(), &mut payloads),
                None => break,
            }
        }
        push_hints_to_ingress(&payloads, &ingress_buffer);
    }
    drop(client);
    let _ = driver.await;
    Ok(())
}

fn push_payload(raw: &str, out: &mut Vec<RegistryNotifyPayload>) {
    match serde_json::from_str::<RegistryNotifyPayload>(raw) {
        Ok(p) if !p.project_path.is_empty() => out.push(p),
        Ok(_) => {
            // No project_path means nothing for the indexer to scan ;
            // silently skip.
        }
        Err(_) => {
            // Malformed payload — skip silently ; logging here is
            // noisy because misconfigured triggers would flood logs.
        }
    }
}

fn push_hints_to_ingress(
    payloads: &[RegistryNotifyPayload],
    ingress_buffer: &SharedIngressBuffer,
) {
    if payloads.is_empty() {
        return;
    }
    let mut guard = ingress_buffer
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    for payload in payloads {
        info!(
            project_code = payload.project_code.as_str(),
            project_path = payload.project_path.as_str(),
            op = payload.op.as_str(),
            "axon_registry_changed → enqueue subtree hint (REQ-AXO-901675)"
        );
        guard.record_subtree_hint(
            payload.project_path.clone(),
            REGISTRY_SUBTREE_HINT_PRIORITY,
            IngressSource::Scan,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_payload_extracts_registry_mutation() {
        let mut out = Vec::new();
        push_payload(
            r#"{"op":"insert","project_code":"AXO","project_path":"/home/x/projects/axon"}"#,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].project_code, "AXO");
        assert_eq!(out[0].project_path, "/home/x/projects/axon");
        assert_eq!(out[0].op, "insert");
    }

    #[test]
    fn push_payload_skips_missing_project_path() {
        let mut out = Vec::new();
        push_payload(
            r#"{"op":"insert","project_code":"AXO","project_path":""}"#,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn push_payload_skips_malformed_json() {
        let mut out = Vec::new();
        push_payload("not json", &mut out);
        push_payload("{}", &mut out);
        push_payload("null", &mut out);
        assert!(out.is_empty());
    }
}
