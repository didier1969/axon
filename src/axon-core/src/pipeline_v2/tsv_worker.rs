//! TSV builder worker — REQ-AXO-901624 P4 Lazy Async TSV Build via pgmq.
//!
//! Drains the `pgmq.tsv_pending` queue and back-fills
//! `public.Chunk.content_tsv` out of band of the A3 critical write path.
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

use anyhow::{Context, Result};
use serde_json::Value;
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

/// Parse the JSON array returned by `SELECT msg_id::text,
/// (message->>'chunk_id') AS chunk_id FROM pgmq.read(...)` into
/// parallel vectors of (msg_id, chunk_id). Rows that lack a `chunk_id`
/// extraction (NULL or missing key) are silently dropped — defensive
/// against future fan-out of the queue to other payload shapes.
///
/// Review-fix REQ-AXO-901624 : on prend `message->>'chunk_id'`
/// directement côté SQL pour bypass le double JSON parsing (le cast
/// `message::text` → re-parse Rust est fragile vis-à-vis du shape de
/// sérialisation jsonb retourné par `query_json`).
pub(crate) fn extract_chunk_ids(rows_json: &str) -> Result<(Vec<String>, Vec<String>)> {
    let parsed: Value = serde_json::from_str(rows_json)
        .with_context(|| format!("tsv_worker: pgmq.read returned non-JSON: {rows_json}"))?;
    let Some(arr) = parsed.as_array() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut msg_ids = Vec::with_capacity(arr.len());
    let mut chunk_ids = Vec::with_capacity(arr.len());
    for row in arr {
        let msg_id = row
            .get("msg_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let chunk_id = row
            .get("chunk_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if msg_id.is_empty() || chunk_id.is_empty() {
            continue;
        }
        msg_ids.push(msg_id);
        chunk_ids.push(chunk_id);
    }
    Ok((msg_ids, chunk_ids))
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
        "UPDATE public.Chunk \
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

/// One drain pass. Reads up to `qty` messages, UPDATEs the
/// corresponding Chunk rows, archives the messages. Called once per
/// worker loop iteration. Synchronous : the caller wraps in
/// `spawn_blocking` because `GraphStore::execute` / `query_json_writer`
/// are FFI-blocking.
///
/// Review-fix REQ-AXO-901624 : on utilise `query_json_writer`
/// (graph_query.rs:490) parce que `pgmq.read` est sémantiquement une
/// mutation (incrémente `read_ct` + UPDATE `vt`) ; le reader_ctx
/// embedded-test backend sert un snapshot stale entre commits writer
/// et ne devrait jamais voir des lignes pgmq fraîchement enqueued.
pub fn drain_once(store: &GraphStore, qty: usize, vt_s: u32) -> Result<DrainStats> {
    let mut stats = DrainStats::default();
    let read_sql = format!(
        "SELECT msg_id::text AS msg_id, (message->>'chunk_id') AS chunk_id \
         FROM pgmq.read('tsv_pending', {vt_s}, {qty})"
    );
    let rows = store.query_json_writer(&read_sql)?;
    let (msg_ids, chunk_ids) = extract_chunk_ids(&rows)?;
    stats.fetched = msg_ids.len();
    if msg_ids.is_empty() {
        return Ok(stats);
    }

    store.execute(&build_update_sql(&chunk_ids))?;
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
        store.execute(&archive_sql)?;
    }
    stats.archived = parsed_count;

    Ok(stats)
}

/// Probe the running PG for the `pgmq` extension. Used at worker boot
/// to gate the spawn — if the extension is missing (devenv not yet
/// rebuilt with `exts.pgmq`), spawning workers would just flood the
/// logs with "relation pgmq.tsv_pending does not exist" errors at
/// `poll_interval_ms` cadence. Defer until the extension shows up.
pub fn pgmq_extension_present(store: &GraphStore) -> bool {
    let sql = "SELECT count(*)::text AS c FROM pg_extension WHERE extname = 'pgmq'";
    let Ok(rows) = store.query_json_writer(sql) else {
        return false;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&rows) else {
        return false;
    };
    parsed
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .map(|n| n >= 1)
        .unwrap_or(false)
}

/// Cap pour le backoff exponentiel sur erreur permanente (e.g.
/// extension manquante au runtime). 30 secondes = compromis entre
/// "réactif quand l'extension arrive" et "pas de flood logs".
const ERROR_BACKOFF_MAX_MS: u64 = 30_000;

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
    if !pgmq_extension_present(&store) {
        warn!(
            "pgmq extension absent — tsv worker pool not spawned. \
             Run `devenv up -d` after adding `exts.pgmq` to devenv.nix \
             and restart axon-brain to activate REQ-AXO-901624 P4."
        );
        return Vec::new();
    }
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
            loop {
                let s = store.clone();
                let qty = cfg.batch_size;
                let vt = cfg.visibility_timeout_s;
                let drain_join =
                    tokio::task::spawn_blocking(move || drain_once(&s, qty, vt)).await;
                match drain_join {
                    Ok(Ok(stats)) => {
                        error_backoff_ms = cfg.poll_interval_ms;
                        if stats.fetched == 0 {
                            tokio::time::sleep(Duration::from_millis(cfg.poll_interval_ms))
                                .await;
                        } else {
                            debug!(
                                worker_idx,
                                fetched = stats.fetched,
                                updated = stats.updated,
                                archived = stats.archived,
                                "tsv worker drained batch"
                            );
                        }
                    }
                    Ok(Err(err)) => {
                        warn!(
                            worker_idx,
                            error = ?err,
                            backoff_ms = error_backoff_ms,
                            "tsv worker drain failed"
                        );
                        tokio::time::sleep(Duration::from_millis(error_backoff_ms)).await;
                        error_backoff_ms = (error_backoff_ms.saturating_mul(2))
                            .min(ERROR_BACKOFF_MAX_MS);
                    }
                    Err(join_err) => {
                        warn!(
                            worker_idx,
                            error = ?join_err,
                            backoff_ms = error_backoff_ms,
                            "tsv worker spawn_blocking joined with error"
                        );
                        tokio::time::sleep(Duration::from_millis(error_backoff_ms)).await;
                        error_backoff_ms = (error_backoff_ms.saturating_mul(2))
                            .min(ERROR_BACKOFF_MAX_MS);
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
    fn extract_chunk_ids_parses_pgmq_read_output() {
        // Review-fix : SQL retourne désormais (msg_id, chunk_id) directement
        // via `message->>'chunk_id'`, plus de double-parsing.
        let rows_json = r#"[
            {"msg_id":"1","chunk_id":"AXO:file.rs:fn:1-10:abc"},
            {"msg_id":"2","chunk_id":"AXO:file.rs:fn:11-20:def"}
        ]"#;
        let (msg_ids, chunk_ids) = extract_chunk_ids(rows_json).unwrap();
        assert_eq!(msg_ids, vec!["1", "2"]);
        assert_eq!(
            chunk_ids,
            vec!["AXO:file.rs:fn:1-10:abc", "AXO:file.rs:fn:11-20:def"]
        );
    }

    #[test]
    fn extract_chunk_ids_drops_rows_missing_chunk_id() {
        let rows_json = r#"[
            {"msg_id":"1","chunk_id":null},
            {"msg_id":"2","chunk_id":"ok"},
            {"msg_id":"3"}
        ]"#;
        let (msg_ids, chunk_ids) = extract_chunk_ids(rows_json).unwrap();
        assert_eq!(msg_ids, vec!["2"]);
        assert_eq!(chunk_ids, vec!["ok"]);
    }

    #[test]
    fn extract_chunk_ids_handles_empty_array() {
        let (msg_ids, chunk_ids) = extract_chunk_ids("[]").unwrap();
        assert!(msg_ids.is_empty());
        assert!(chunk_ids.is_empty());
    }

    #[test]
    fn build_update_sql_escapes_single_quotes() {
        let ids = vec!["plain".to_string(), "with'quote".to_string()];
        let sql = build_update_sql(&ids);
        assert!(sql.contains("'plain'"));
        assert!(sql.contains("'with''quote'"));
        assert!(sql.contains("axon.compute_chunk_tsv"));
        assert!(sql.contains("UPDATE public.Chunk"));
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
