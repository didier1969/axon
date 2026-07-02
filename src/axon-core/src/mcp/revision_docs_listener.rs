//! REQ-AXO-309 (DEC-AXO-901640) — SOLL revision-committed journal subscriber.
//!
//! Opens a dedicated `tokio_postgres` connection, `LISTEN soll_revision_committed`,
//! and on each commit (debounced so a burst triggers one pass) performs TWO duties
//! for the affected project:
//!   1. REQ-AXO-902176 — invalidate the RAM `SollSnapshotCache` so the next hot read
//!      reloads fresh from PG. Journal-driven (trigger-reliable, cross-process),
//!      unlike the per-tool in-process hook which misses cross-process writes.
//!   2. Regenerate the derived autodoc site.
//! ONE emitter (the `trg_soll_revision_notify` trigger on `soll.Revision`,
//! `db/ddl/13_*.sql`), N fire-and-forget subscribers (this one; the SOLL-embedding
//! sweep next), replacing the per-tool derived-docs hooks (dependency inversion —
//! `soll_manager` no longer needs to know its consumers).
//!
//! The render reuses `McpServer::regenerate_derived_docs_for` →
//! `schedule_background_derived_docs_refresh`, so it shares the SAME
//! inflight-coalescing set + render-lock as the legacy per-tool hook: the two
//! paths never double-render or race the non-atomic site `fs::write`.
//!
//! Resilience: reconnect-forever with exponential backoff (200ms → 30s cap),
//! same shape as the `ist_mutated` listener.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::stream::StreamExt;
use serde::Deserialize;
use tokio_postgres::{AsyncMessage, NoTls};
use tracing::{info, warn};

use crate::mcp::McpServer;

const LISTEN_CHANNEL: &str = "soll_revision_committed";
const BACKOFF_INITIAL_MS: u64 = 200;
const BACKOFF_MAX_MS: u64 = 30_000;
// Autodoc regen is heavier than an IST snapshot refresh, so coalesce a wider
// window than the 50ms ist_mutated one: a burst of SOLL mutations (e.g.
// soll_apply_plan writing N nodes → N revisions) regenerates the site once, a
// few seconds after the burst settles.
const COALESCE_WINDOW_MS: u64 = 3_000;

#[derive(Debug, Deserialize)]
struct RevisionNotifyPayload {
    #[serde(default)]
    project_code: String,
}

/// Supervised listener loop. Returns immediately; reconnects forever on errors.
pub(crate) fn spawn(server: Arc<McpServer>, database_url: String) {
    tokio::spawn(async move {
        let mut backoff_ms = BACKOFF_INITIAL_MS;
        loop {
            match listen_once(&server, &database_url).await {
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

async fn listen_once(server: &Arc<McpServer>, database_url: &str) -> Result<()> {
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
                    warn!(error = %err, "soll_revision_committed stream error");
                    return;
                }
            }
        }
    });

    client
        .batch_execute(&format!("LISTEN {}", LISTEN_CHANNEL))
        .await
        .context("LISTEN soll_revision_committed failed")?;
    info!(
        channel = LISTEN_CHANNEL,
        "soll_revision_committed listener attached"
    );

    loop {
        // Coalesce bursts: wait COALESCE_WINDOW_MS after the first notification
        // and drain the queue, so a transactional bulk commit (N revisions)
        // regenerates each affected project's site exactly once.
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
        // REQ-AXO-902176 — freshness FIRST: drop the RAM SOLL snapshot for every
        // affected project BEFORE the (heavier, debounced) autodoc regen, so a
        // concurrent hot read reloads fresh from PG immediately.
        invalidate_soll_snapshots(server, &projects);

        // REQ-AXO-902053 P2 — feed the viz-freshness signal so `status`/`health`
        // and the dashboard event (`compose_dashboard_state_v1`) surface "when
        // did SOLL last change" without a shell. Fire-and-forget alongside the
        // autodoc regen (the existing subscriber duty).
        let now_ms = chrono::Utc::now().timestamp_millis();
        for project in &projects {
            crate::viz_freshness::record_soll_revision(project.clone(), now_ms);
            server.regenerate_derived_docs_for(project.clone());
        }
    }
    drop(client);
    let _ = driver.await;
    Ok(())
}

/// REQ-AXO-902176 — drop the RAM SOLL snapshot for every project touched by a
/// committed revision. The `trg_soll_revision_notify` trigger fires on EVERY commit
/// (in-process OR cross-process), so this journal-driven invalidation makes RAM
/// freshness reliable *independently* of the per-tool in-process hook
/// (`attach_derived_docs_refresh_metadata`), which misses cross-process writes AND
/// any mutation whose `project_code` derivation fails (root of the stale
/// `soll_acyclic_audit` + `soll_work_plan`-evidence desync, mcp_feedback #38/#39/#40).
/// Invalidate (not eager reload): the next hot read lazily reloads the project from
/// PG, matching the cache's demand-driven design.
fn invalidate_soll_snapshots(server: &Arc<McpServer>, projects: &HashSet<String>) {
    for project in projects {
        server.soll_cache().invalidate(project);
    }
}

fn push_payload(raw: &str, out: &mut HashSet<String>) {
    if let Ok(p) = serde_json::from_str::<RevisionNotifyPayload>(raw) {
        if !p.project_code.is_empty() {
            out.insert(p.project_code);
        }
    }
    // Malformed payload → skip silently (a misconfigured trigger would otherwise
    // flood logs).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_payload_extracts_project_code() {
        let mut set = HashSet::new();
        push_payload(
            r#"{"project_code":"AXO","revision_id":"REV-AXO-1"}"#,
            &mut set,
        );
        assert!(set.contains("AXO"));
    }

    #[test]
    fn push_payload_dedups_across_calls() {
        let mut set = HashSet::new();
        push_payload(r#"{"project_code":"AXO","revision_id":"r1"}"#, &mut set);
        push_payload(r#"{"project_code":"AXO","revision_id":"r2"}"#, &mut set);
        push_payload(r#"{"project_code":"OPT","revision_id":"r3"}"#, &mut set);
        assert_eq!(set.len(), 2);
        assert!(set.contains("AXO"));
        assert!(set.contains("OPT"));
    }

    #[test]
    fn push_payload_ignores_empty_and_malformed() {
        let mut set = HashSet::new();
        push_payload(r#"{"project_code":""}"#, &mut set);
        push_payload("not json", &mut set);
        push_payload("{}", &mut set);
        push_payload("null", &mut set);
        assert!(set.is_empty());
    }

    // REQ-AXO-902176 — the journal-driven freshness duty: a committed revision must
    // drop the affected project's RAM snapshot so the next read reloads from PG.
    // Zero mock I/O (GUI-PRO-004): real ephemeral PG + real McpServer + real cache.
    #[test]
    fn invalidate_soll_snapshots_drops_warm_cache_902176() {
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let server = Arc::new(McpServer::new(store));
        // Warm AXO: first read = PG load, second = RAM hit.
        let _ = server.soll_cache().snapshot("AXO").expect("warm");
        let _ = server.soll_cache().snapshot("AXO").expect("ram hit");
        let (ram_before, pg_before) = server.soll_cache().read_stats();
        assert!(ram_before >= 1, "second read should be a RAM hit: {ram_before}");

        // Journal duty: invalidate every project touched by the revision burst.
        let mut projects = HashSet::new();
        projects.insert("AXO".to_string());
        invalidate_soll_snapshots(&server, &projects);

        // Next read must reload from PG — one additional PG load proves the drop.
        let _ = server.soll_cache().snapshot("AXO").expect("reload");
        let (_ram_after, pg_after) = server.soll_cache().read_stats();
        assert_eq!(
            pg_after,
            pg_before + 1,
            "post-invalidation read must reload from PG"
        );
    }

    // REQ-AXO-309 E2E — real PG round-trip (zero mock I/O): a soll.Revision
    // INSERT fires the trg_soll_revision_notify trigger (db/ddl/13_*.sql), and a
    // LISTEN connection receives the soll_revision_committed payload carrying the
    // project_code. Proves the emit point on real data, not a shell.
    #[tokio::test]
    async fn revision_insert_fires_soll_revision_committed_notify() {
        let test_db = crate::test_support::test_db::TestDb::create();
        let url = test_db.url();
        let (client, mut connection) = tokio_postgres::connect(&url, NoTls)
            .await
            .expect("connect to ephemeral test db");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<tokio_postgres::Notification>(16);
        let driver = tokio::spawn(async move {
            let stream = futures_util::stream::poll_fn(move |cx| connection.poll_message(cx));
            tokio::pin!(stream);
            while let Some(msg) = stream.next().await {
                if let Ok(AsyncMessage::Notification(n)) = msg {
                    let _ = tx.send(n).await;
                }
            }
        });
        client
            .batch_execute("LISTEN soll_revision_committed")
            .await
            .expect("LISTEN");
        client
            .batch_execute(
                "INSERT INTO soll.Revision (revision_id, project_code) \
                 VALUES ('REV-TST-309', 'AXO')",
            )
            .await
            .expect("insert revision");

        let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a NOTIFY within 5s")
            .expect("a notification");
        let mut projects = HashSet::new();
        push_payload(&got.payload(), &mut projects);
        assert!(
            projects.contains("AXO"),
            "trigger must emit project_code; payload was {}",
            got.payload()
        );
        driver.abort();
    }
}
