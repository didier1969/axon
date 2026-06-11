//! TSV builder worker — REQ-AXO-901624 P4 Lazy Async TSV Build via pgmq.
//!
//! Drains the `pgmq.tsv_pending` queue and back-fills
//! `ist.Chunk.content_tsv` out of band of the A3 critical write path.
//! Decouples the ~95 % CPU cost of the 4-setweight tsvector computation
//! (P1 EXPLAIN ANALYZE session 48 ; was a GENERATED ALWAYS STORED column
//! synchronous to A3 INSERT) from A3's transaction latency.
//!
//! # Lifecycle
//!
//! Spawned once per orchestrator boot via [`spawn_tsv_workers`]. Each
//! worker task runs a polling loop : `pgmq.read(vt=30s, qty=BATCH)` →
//! batch UPDATE → `pgmq.archive`. On idle (empty read) the worker
//! sleeps for `AXON_TSV_POLL_INTERVAL_MS` (100 ms default).
//!
//! # Tuning
//!
//! - `AXON_TSV_WORKER_CONCURRENCY` (default 2)
//! - `AXON_TSV_BATCH_SIZE` (default 256)
//! - `AXON_TSV_VISIBILITY_TIMEOUT_S` (default 30)
//! - `AXON_TSV_POLL_INTERVAL_MS` (default 100)
//!
//! # Crash recovery
//!
//! Visibility-timeout based : if a worker dies between `read` and
//! `archive`, pgmq re-delivers the messages after VT seconds. The
//! worker UPDATE is idempotent (`axon.compute_chunk_tsv` produces the
//! same tsvector for identical content) so double-processing is
//! harmless beyond a duplicate UPDATE cost.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::graph::GraphStore;

/// Configuration knobs for the TSV worker pool. Constructed via
/// [`TsvWorkerConfig::from_env`] which honors the four AXON_TSV_*
/// environment variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsvWorkerConfig {
    pub concurrency: usize,
    pub batch_size: usize,
    pub visibility_timeout_s: u32,
    pub poll_interval_ms: u64,
}

impl Default for TsvWorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 2,
            batch_size: 256,
            visibility_timeout_s: 30,
            poll_interval_ms: 100,
        }
    }
}

impl TsvWorkerConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        // `concurrency=0` is intentional : disables the worker pool for
        // A/B benches against the pre-P4 baseline. Other knobs reject 0
        // as nonsensical (zero batch, zero timeout).
        if let Some(v) = std::env::var("AXON_TSV_WORKER_CONCURRENCY")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
        {
            cfg.concurrency = v;
        }
        if let Some(v) = std::env::var("AXON_TSV_BATCH_SIZE")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
        {
            cfg.batch_size = v;
        }
        if let Some(v) = std::env::var("AXON_TSV_VISIBILITY_TIMEOUT_S")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|n| *n > 0)
        {
            cfg.visibility_timeout_s = v;
        }
        if let Some(v) = std::env::var("AXON_TSV_POLL_INTERVAL_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            cfg.poll_interval_ms = v;
        }
        cfg
    }
}

/// Per-iteration drain outcome. Surfaced for tests so they can assert
/// throughput without timing dependencies.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainStats {
    pub fetched: usize,
    pub updated: usize,
    pub archived: usize,
}

/// Build the UPDATE statement that fills `content_tsv` for the given
/// chunk_ids. Chunk IDs are escaped SQL-string style (`'` → `''`)
/// before interpolation. The expression delegates to the
/// `axon.compute_chunk_tsv` function so the 4-setweight semantics stay
/// centralized.
pub(crate) fn build_update_sql(chunk_ids: &[String]) -> String {
    let id_list = chunk_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "UPDATE ist.Chunk \
         SET content_tsv = axon.compute_chunk_tsv(chunk_path, kind, content, file_path) \
         WHERE id IN ({id_list})"
    )
}

/// Build the `SELECT pgmq.archive(...)` statement for a batch of
/// msg_ids. Returns `(sql, parsed_count)` where `parsed_count` is how
/// many msg_ids survived the i64 parse — caller can compare against
/// `msg_ids.len()` to surface a malformed-msg-id condition. In
/// practice pgmq.read returns bigint msg_ids stringified by
/// `msg_id::text`, so a drop here signals a contract regression
/// upstream worth a warn!.
pub(crate) fn build_archive_sql(msg_ids: &[String]) -> (String, usize) {
    let parsed: Vec<String> = msg_ids
        .iter()
        .filter_map(|m| m.parse::<i64>().ok().map(|n| n.to_string()))
        .collect();
    let parsed_count = parsed.len();
    let msgs = parsed.join(",");
    (
        format!("SELECT pgmq.archive('tsv_pending', ARRAY[{msgs}]::bigint[])"),
        parsed_count,
    )
}

/// One drain pass. Reads up to `qty` messages, UPDATEs the corresponding
/// Chunk rows, archives the messages. Called once per worker loop iteration.
///
/// REQ-AXO-901884 stage 1 — fully async on the native core (`query_rows` /
/// `execute_async`); the worker `.await`s it directly (no `spawn_blocking`).
/// `pgmq.read` is semantically a mutation (bumps `read_ct` + the visibility
/// timeout), so it stays writer-routed via the single canonical pool.
pub async fn drain_once(store: &GraphStore, qty: usize, vt_s: u32) -> Result<DrainStats> {
    let mut stats = DrainStats::default();
    let read_sql = format!(
        "SELECT msg_id::text AS msg_id, (message->>'chunk_id') AS chunk_id \
         FROM pgmq.read('tsv_pending', {vt_s}, {qty})"
    );
    // REQ-AXO-901884 stage 1 — async-native drain: typed `tokio_postgres::Row`s
    // straight from the core (no run_blocking hop, no Vec<Vec<String>> render +
    // re-parse — which is exactly the object-vs-array trap that left FTS dead).
    // pgmq.read column order is (msg_id::text, chunk_id::text); a NULL chunk_id
    // is dropped.
    let rows = store.query_rows(&read_sql).await?;
    let mut msg_ids = Vec::with_capacity(rows.len());
    let mut chunk_ids = Vec::with_capacity(rows.len());
    for row in &rows {
        let msg_id: String = row
            .try_get::<_, Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default();
        let chunk_id: String = row
            .try_get::<_, Option<String>>(1)
            .ok()
            .flatten()
            .unwrap_or_default();
        if msg_id.is_empty() || chunk_id.is_empty() {
            continue;
        }
        msg_ids.push(msg_id);
        chunk_ids.push(chunk_id);
    }
    stats.fetched = msg_ids.len();
    if msg_ids.is_empty() {
        return Ok(stats);
    }

    store.execute_async(&build_update_sql(&chunk_ids)).await?;
    stats.updated = chunk_ids.len();

    let (archive_sql, parsed_count) = build_archive_sql(&msg_ids);
    if parsed_count < msg_ids.len() {
        warn!(
            received = msg_ids.len(),
            parsed = parsed_count,
            "tsv worker dropped non-i64 msg_ids during archive — pgmq contract regression?"
        );
    }
    if parsed_count > 0 {
        store.execute_async(&archive_sql).await?;
    }
    stats.archived = parsed_count;

    Ok(stats)
}

/// Probe the running PG for the `pgmq` extension. Used at worker boot
/// to gate the spawn — if the extension is missing (devenv not yet
/// rebuilt with `exts.pgmq`), spawning workers would just flood the
/// logs with "relation pgmq.tsv_pending does not exist" errors at
/// `poll_interval_ms` cadence. Defer until the extension shows up.
pub async fn pgmq_extension_present(store: &GraphStore) -> bool {
    // REQ-AXO-901884 stage 1 — async-native + typed: `query_count_async` reads
    // the native bigint count directly (no ::text cast, no JSON render/parse —
    // which is what silently returned a false-negative and kept FTS dead).
    store
        .query_count_async("SELECT count(*) FROM pg_extension WHERE extname = 'pgmq'")
        .await
        .map(|n| n >= 1)
        .unwrap_or(false)
}

/// Cap pour le backoff exponentiel sur erreur permanente (e.g.
/// extension manquante au runtime). 30 secondes = compromis entre
/// "réactif quand l'extension arrive" et "pas de flood logs".
const ERROR_BACKOFF_MAX_MS: u64 = 30_000;

/// REQ-AXO-901930 — cap on the *idle* poll backoff. When `tsv_pending`
/// is empty, the worker doubles its sleep from `poll_interval_ms` up to
/// this ceiling instead of hammering pgmq every `poll_interval_ms`
/// forever (40 PG round-trips/s across 2 workers at idle = the observed
/// ~30% indexer CPU spin). 2 s keeps worst-case FTS back-fill latency
/// after an idle spell bounded while killing the busy-loop. The instant
/// a drain returns rows the backoff resets to `poll_interval_ms`, so a
/// real backlog still drains at full speed (no FTS throughput regression).
const IDLE_BACKOFF_MAX_MS: u64 = 2_000;

/// Next idle-poll backoff: double, capped at [`IDLE_BACKOFF_MAX_MS`].
/// Pure so the progression is unit-testable (REQ-AXO-901930).
fn next_idle_backoff_ms(current_ms: u64) -> u64 {
    current_ms.saturating_mul(2).min(IDLE_BACKOFF_MAX_MS)
}

/// Spawn `cfg.concurrency` worker tasks. Returns the JoinHandles so the
/// orchestrator (or tests) can await teardown. Workers loop forever ;
/// the de-facto shutdown is process kill, matching the cadence of A3
/// batched workers (no graceful drain channel in pipeline v2 yet).
///
/// Review-fix REQ-AXO-901624 : si l'extension pgmq n'est pas présente,
/// on log un warning unique et on n'attache PAS de workers. Évite le
/// flood logs de 20 errors/s qui surviendrait sinon. L'opérateur doit
/// rebuilder devenv + restart brain pour activer.
pub fn spawn_tsv_workers(store: Arc<GraphStore>, cfg: TsvWorkerConfig) -> Vec<JoinHandle<()>> {
    // REQ-AXO-901884 — DEFER, do NOT one-shot-gate, on pgmq presence. The old
    // `if !pgmq_extension_present { return Vec::new() }` gate disabled FTS for
    // the ENTIRE process on a single boot-time false-negative (transient pool
    // hiccup / DDL-ordering race before pgmq is reachable): content_tsv stayed
    // empty while tsv_pending grew unbounded (2.7M msgs observed in dev). We now
    // spawn unconditionally and re-probe pgmq at the top of each loop iteration
    // with capped backoff — the worker self-heals the moment pgmq is reachable,
    // logging the wait ONCE per dry spell (no flood). Matches the documented
    // "defer until the extension shows up" intent on pgmq_extension_present.
    let mut handles = Vec::with_capacity(cfg.concurrency);
    for worker_idx in 0..cfg.concurrency {
        let store = store.clone();
        let handle = tokio::spawn(async move {
            info!(
                worker_idx,
                concurrency = cfg.concurrency,
                batch_size = cfg.batch_size,
                vt_s = cfg.visibility_timeout_s,
                poll_interval_ms = cfg.poll_interval_ms,
                "tsv worker spawned"
            );
            let mut error_backoff_ms: u64 = cfg.poll_interval_ms;
            let mut defer_backoff_ms: u64 = cfg.poll_interval_ms;
            // REQ-AXO-901930 — idle poll backoff (resets to poll_interval_ms
            // on any non-empty drain). Kills the ~30% indexer CPU spin at idle.
            let mut idle_backoff_ms: u64 = cfg.poll_interval_ms;
            let mut deferred_logged = false;
            loop {
                // REQ-AXO-901884 — deferred pgmq readiness probe (own backoff,
                // independent of the drain-failure backoff below). Skip the drain
                // without error spam until the extension/queue is reachable.
                {
                    let present = pgmq_extension_present(&store).await;
                    if !present {
                        if !deferred_logged {
                            warn!(
                                worker_idx,
                                "pgmq not reachable yet — tsv worker deferring (will retry; \
                                 content_tsv back-fill paused until pgmq is up)"
                            );
                            deferred_logged = true;
                        }
                        tokio::time::sleep(Duration::from_millis(defer_backoff_ms)).await;
                        defer_backoff_ms =
                            (defer_backoff_ms.saturating_mul(2)).min(ERROR_BACKOFF_MAX_MS);
                        continue;
                    }
                    if deferred_logged {
                        info!(worker_idx, "pgmq now reachable — tsv worker resuming");
                        deferred_logged = false;
                    }
                    defer_backoff_ms = cfg.poll_interval_ms;
                }
                let qty = cfg.batch_size;
                let vt = cfg.visibility_timeout_s;
                // REQ-AXO-901884 stage 1 — drain directly on the async core (no
                // spawn_blocking hop; the join-error arm is gone with it).
                match drain_once(&store, qty, vt).await {
                    Ok(stats) => {
                        error_backoff_ms = cfg.poll_interval_ms;
                        if stats.fetched == 0 {
                            // REQ-AXO-901930 — empty queue: back off (capped)
                            // instead of polling every poll_interval_ms forever.
                            tokio::time::sleep(Duration::from_millis(idle_backoff_ms)).await;
                            idle_backoff_ms = next_idle_backoff_ms(idle_backoff_ms);
                        } else {
                            // Work present → poll fast again so a backlog drains
                            // at full speed (no FTS throughput regression).
                            idle_backoff_ms = cfg.poll_interval_ms;
                            debug!(
                                worker_idx,
                                fetched = stats.fetched,
                                updated = stats.updated,
                                archived = stats.archived,
                                "tsv worker drained batch"
                            );
                        }
                    }
                    Err(err) => {
                        warn!(
                            worker_idx,
                            error = ?err,
                            backoff_ms = error_backoff_ms,
                            "tsv worker drain failed"
                        );
                        tokio::time::sleep(Duration::from_millis(error_backoff_ms)).await;
                        error_backoff_ms =
                            (error_backoff_ms.saturating_mul(2)).min(ERROR_BACKOFF_MAX_MS);
                    }
                }
            }
        });
        handles.push(handle);
    }
    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_design_doc() {
        let cfg = TsvWorkerConfig::default();
        assert_eq!(cfg.concurrency, 2);
        assert_eq!(cfg.batch_size, 256);
        assert_eq!(cfg.visibility_timeout_s, 30);
        assert_eq!(cfg.poll_interval_ms, 100);
    }

    #[test]
    fn idle_backoff_doubles_then_caps() {
        // REQ-AXO-901930 — from 100ms it should climb 100→200→400→800→1600
        // and then saturate at the 2s ceiling, never spinning faster again.
        assert_eq!(next_idle_backoff_ms(100), 200);
        assert_eq!(next_idle_backoff_ms(200), 400);
        assert_eq!(next_idle_backoff_ms(800), 1_600);
        assert_eq!(next_idle_backoff_ms(1_600), IDLE_BACKOFF_MAX_MS);
        assert_eq!(
            next_idle_backoff_ms(IDLE_BACKOFF_MAX_MS),
            IDLE_BACKOFF_MAX_MS
        );
        // Overflow-safe.
        assert_eq!(next_idle_backoff_ms(u64::MAX), IDLE_BACKOFF_MAX_MS);
    }

    #[test]
    fn build_update_sql_escapes_single_quotes() {
        let ids = vec!["plain".to_string(), "with'quote".to_string()];
        let sql = build_update_sql(&ids);
        assert!(sql.contains("'plain'"));
        assert!(sql.contains("'with''quote'"));
        assert!(sql.contains("axon.compute_chunk_tsv"));
        assert!(sql.contains("UPDATE ist.Chunk"));
    }

    #[test]
    fn build_archive_sql_keeps_only_numeric_msg_ids() {
        let msg_ids = vec!["1".to_string(), "abc".to_string(), "42".to_string()];
        let (sql, parsed_count) = build_archive_sql(&msg_ids);
        assert!(sql.contains("ARRAY[1,42]::bigint[]"));
        assert_eq!(parsed_count, 2, "non-i64 'abc' should be filtered out");
    }

    #[test]
    fn build_archive_sql_keeps_all_when_all_numeric() {
        let msg_ids = vec!["10".to_string(), "20".to_string()];
        let (sql, parsed_count) = build_archive_sql(&msg_ids);
        assert!(sql.contains("ARRAY[10,20]::bigint[]"));
        assert_eq!(parsed_count, 2);
    }

    #[test]
    fn build_archive_sql_handles_empty() {
        let (sql, parsed_count) = build_archive_sql(&[]);
        assert!(sql.contains("ARRAY[]::bigint[]"));
        assert_eq!(parsed_count, 0);
    }

    #[test]
    fn config_env_concurrency_override() {
        // SAFETY: env writes inside a test mutate process-global state.
        // The harness lock used by embedder tests isn't applicable here
        // (no GPU). Conflict surface is small (only AXON_TSV_*) and the
        // assertion lives within the same scope.
        std::env::set_var("AXON_TSV_WORKER_CONCURRENCY", "7");
        let cfg = TsvWorkerConfig::from_env();
        assert_eq!(cfg.concurrency, 7);
        std::env::remove_var("AXON_TSV_WORKER_CONCURRENCY");
    }

    #[test]
    fn config_env_rejects_zero() {
        std::env::set_var("AXON_TSV_BATCH_SIZE", "0");
        let cfg = TsvWorkerConfig::from_env();
        assert_eq!(cfg.batch_size, 256); // default preserved
        std::env::remove_var("AXON_TSV_BATCH_SIZE");
    }
}
