// REQ-AXO-902234 VOLET 1 — LISTEN embedder_control (desired-state consumer).
//
// The idle-drop watchdog (`pipeline/embedder_gpu.rs::spawn_idle_watchdog`) runs
// inside `axon-indexer`; the `idle_drop` MCP tool that flips it runs inside
// `axon-brain`. Two OS processes, so the `embed_provider` in-process AtomicU8
// pattern cannot reach it. This module closes the loop: the brain writes
// `axon.EmbedderControl`, the PG trigger (db/ddl/24_embedder_control.sql) fires
// `pg_notify('embedder_control', …)`, and this listener flips the process-global
// atomics the watchdog re-reads on every tick — no restart, therefore no GPU
// teardown (the operation the unstable NVIDIA driver makes risky).
//
// Shape mirrors `ist_snapshot/notify_listener.rs`: dedicated `tokio_postgres`
// connection outside the deadpool, forever-reconnect with exponential backoff
// (200 ms → 30 s cap).
//
// Boot order (D1, operator decision): `seed_and_load` runs BEFORE the LISTEN
// loop. It INSERTs the env-derived defaults with `ON CONFLICT DO NOTHING` —
// never an UPDATE, or every restart would clobber a value set at runtime — then
// reads the row back and applies it. Net effect: the env is the boot SEED (so a
// fresh DB still honours `.env.worktree` and an activation can never silently
// vanish, the original 902234 defect), while the row is authoritative once it
// exists.

use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use serde::Deserialize;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::pipeline::embedder_gpu::{
    apply_idle_drop_control, idle_drop_enabled_from_env, idle_drop_seconds_from_env,
};

const LISTEN_CHANNEL: &str = "embedder_control";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;

/// The `process_role` key this process owns in `axon.EmbedderControl`. The
/// watchdog is an indexer-side concern, so a brain-targeted row is ignored.
pub const ROLE_INDEXER: &str = "indexer";

#[derive(Debug, Deserialize)]
struct ControlPayload {
    #[serde(default)]
    process_role: String,
    #[serde(default)]
    idle_drop_enabled: bool,
    #[serde(default)]
    idle_seconds: u64,
}

/// Supervised listener. Returns immediately; seeds + loads the row once, then
/// reconnects forever on error. `role` is the row this process obeys
/// ([`ROLE_INDEXER`]).
pub fn spawn_embedder_control_listener(database_url: String, role: String) {
    tokio::spawn(async move {
        match seed_and_load(&database_url, &role).await {
            Ok((enabled, seconds)) => {
                apply_idle_drop_control(enabled, seconds);
                info!(
                    role = %role,
                    enabled,
                    t_idle_s = seconds,
                    "embedder_control: desired state loaded from PG (REQ-AXO-902234)"
                );
            }
            Err(err) => {
                // Non-fatal: the watchdog keeps using the env seed. Degrading to
                // "env only" is exactly the pre-902234 behaviour, never a
                // dead-end (PIL-AXO-002).
                warn!(
                    error = %err,
                    "embedder_control: seed/load failed; falling back to the env seed"
                );
            }
        }

        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&database_url, &role).await {
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

/// Seed the control row from the env (once, never overwriting) and return the
/// authoritative values.
async fn seed_and_load(database_url: &str, role: &str) -> Result<(bool, u64)> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("embedder_control seed connect failed")?;
    let driver = tokio::spawn(async move {
        if let Err(err) = connection.await {
            warn!(error = %err, "embedder_control seed connection closed");
        }
    });

    let now_ms = chrono::Utc::now().timestamp_millis();
    client
        .execute(
            "INSERT INTO axon.EmbedderControl \
             (process_role, idle_drop_enabled, idle_seconds, updated_ms, updated_by) \
             VALUES ($1, $2, $3, $4, 'boot_seed:env') \
             ON CONFLICT (process_role) DO NOTHING",
            &[
                &role,
                &idle_drop_enabled_from_env(),
                &(idle_drop_seconds_from_env() as i32),
                &now_ms,
            ],
        )
        .await
        .context("embedder_control seed insert failed")?;

    let row = client
        .query_one(
            "SELECT idle_drop_enabled, idle_seconds FROM axon.EmbedderControl \
             WHERE process_role = $1",
            &[&role],
        )
        .await
        .context("embedder_control read-back failed")?;

    let enabled: bool = row.get(0);
    let seconds: i32 = row.get(1);
    drop(client);
    let _ = driver.await;
    Ok((enabled, (seconds.max(1)) as u64))
}

async fn listen_once(database_url: &str, role: &str) -> Result<()> {
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
                    warn!(error = %err, "embedder_control stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN embedder_control failed")?;
    info!(channel = LISTEN_CHANNEL, role = %role, "embedder_control listener attached");

    while let Some(n) = notify_rx.recv().await {
        // No coalescing window (unlike ist_mutated): control flips are rare,
        // operator-driven, and last-write-wins is the correct semantic.
        if let Some((enabled, seconds)) = parse_payload_for_role(&n.payload(), role) {
            apply_idle_drop_control(enabled, seconds);
            info!(
                enabled,
                t_idle_s = seconds,
                "embedder_control: idle-drop policy updated at RUNTIME (no restart, REQ-AXO-902234)"
            );
        }
    }

    drop(client);
    let _ = driver.await;
    Ok(())
}

/// Returns the new policy when the payload targets `role`, else `None`.
/// Malformed payloads are ignored (a misconfigured trigger must not flood logs
/// nor flip a GPU policy).
fn parse_payload_for_role(raw: &str, role: &str) -> Option<(bool, u64)> {
    let payload: ControlPayload = serde_json::from_str(raw).ok()?;
    if payload.process_role != role {
        return None;
    }
    Some((payload.idle_drop_enabled, payload.idle_seconds.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_payload_for_matching_role() {
        let got = parse_payload_for_role(
            r#"{"process_role":"indexer","idle_drop_enabled":true,"idle_seconds":20}"#,
            ROLE_INDEXER,
        );
        assert_eq!(got, Some((true, 20)));
    }

    #[test]
    fn parses_disable_as_false_not_absence() {
        // The whole point of the tri-state override: an explicit `false` must
        // reach the watchdog as DISABLED, not as "unset → fall back to env".
        let got = parse_payload_for_role(
            r#"{"process_role":"indexer","idle_drop_enabled":false,"idle_seconds":20}"#,
            ROLE_INDEXER,
        );
        assert_eq!(got, Some((false, 20)));
    }

    #[test]
    fn ignores_payload_for_another_role() {
        assert!(parse_payload_for_role(
            r#"{"process_role":"brain","idle_drop_enabled":true,"idle_seconds":5}"#,
            ROLE_INDEXER,
        )
        .is_none());
    }

    #[test]
    fn clamps_zero_seconds_to_one() {
        let got = parse_payload_for_role(
            r#"{"process_role":"indexer","idle_drop_enabled":true,"idle_seconds":0}"#,
            ROLE_INDEXER,
        );
        assert_eq!(got, Some((true, 1)), "0 s would make the gate meaningless");
    }

    #[test]
    fn ignores_malformed_payloads() {
        for raw in ["not json", "{}", "null", r#"{"process_role":123}"#] {
            assert!(
                parse_payload_for_role(raw, ROLE_INDEXER).is_none(),
                "{raw} must not flip a GPU policy"
            );
        }
    }
}
