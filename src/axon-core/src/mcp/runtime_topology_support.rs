use crate::bridge::RuntimeTruthFeed;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// REQ-AXO-901859 — canonical freshness window for the indexer lifecycle
/// heartbeat (`axon_runtime.EmbedderLifecycleHeartbeat`). Shared by the
/// runtime status composer and the topology snapshot so both judge indexer
/// liveness against the SAME threshold (PIL-AXO-001 single source of truth,
/// no duplicated value). Tick is ~5 s; 30 s tolerates a few missed ticks.
pub(crate) const EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS: i64 = 30_000;

/// REQ-AXO-901859 — the SINGLE canonical indexer liveness verdict, derived
/// solely from the PG heartbeat (`axon_runtime.EmbedderLifecycleHeartbeat`).
/// There is intentionally NO file/shadow-role fallback: under separate
/// brain/indexer processes the file feed false-negatives, and that second
/// source is exactly what let `status` and `embedding_status` disagree
/// (PIL-AXO-001). If the indexer has not published a fresh heartbeat it is
/// not provably alive — say so loudly rather than infer from launch mode.
pub(crate) struct IndexerLiveness {
    pub(crate) feed: RuntimeTruthFeed,
    pub(crate) ready: bool,
    /// Fail-loud provenance: `pg_heartbeat` (fresh row), `pg_heartbeat_stale`
    /// (row present but past the window), `no_heartbeat` (row absent).
    pub(crate) source: &'static str,
}

/// Pure so the verdict is unit-tested without a live `GraphStore`.
pub(crate) fn resolve_indexer_liveness(
    now_ms: i64,
    indexer_heartbeat_ms: Option<i64>,
    freshness_window_ms: i64,
) -> IndexerLiveness {
    let window = freshness_window_ms.max(0) as u64;
    match indexer_heartbeat_ms {
        Some(heartbeat_ms) => {
            let now_u = now_ms.max(0) as u64;
            let heartbeat_u = heartbeat_ms.max(0) as u64;
            // saturating_sub folds clock skew (future-dated heartbeat) to
            // age 0 — a just-written row counts as fresh, not distrusted.
            let fresh = now_u.saturating_sub(heartbeat_u) <= window;
            let feed = RuntimeTruthFeed::from_observed_times(
                now_u,
                Some(heartbeat_u),
                Some(heartbeat_u),
                window,
                if fresh {
                    None::<String>
                } else {
                    Some("indexer_heartbeat_stale".to_string())
                },
            );
            IndexerLiveness {
                ready: fresh,
                source: if fresh {
                    "pg_heartbeat"
                } else {
                    "pg_heartbeat_stale"
                },
                feed,
            }
        }
        None => IndexerLiveness {
            feed: RuntimeTruthFeed::from_observed_times(
                0,
                None,
                None,
                window,
                Some("indexer_heartbeat_absent".to_string()),
            ),
            ready: false,
            source: "no_heartbeat",
        },
    }
}

pub(crate) fn split_run_root(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
    let mut path = PathBuf::from(project_root);
    if instance_kind == "dev" {
        path.push(".axon-dev");
    } else {
        path.push(".axon");
    }
    path.push(format!("run-{role_slug}"));
    path
}

pub(crate) fn split_runtime_state_from_file(path: &PathBuf) -> Option<HashMap<String, String>> {
    let file = OpenOptions::new().read(true).open(path).ok()?;
    let reader = BufReader::new(file);
    let mut values = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    Some(values)
}

pub(crate) fn runtime_truth_feed_snapshot(feed: &RuntimeTruthFeed) -> Value {
    let state = if feed.stale {
        "stale"
    } else if feed.degraded_reason.is_some() {
        "degraded"
    } else {
        "fresh"
    };

    json!({
        "state": state,
        "stale": feed.stale,
        "observed_age_ms": feed.observed_age_ms,
        "stale_after_ms": feed.stale_after_ms,
        "last_heartbeat_at_ms": feed.last_heartbeat_at_ms,
        "last_good_payload_at_ms": feed.last_good_payload_at_ms,
        "degraded_reason": feed.degraded_reason
    })
}

#[cfg(test)]
mod resolve_indexer_liveness_tests {
    use super::*;

    #[test]
    fn fresh_heartbeat_is_ready_and_canonical() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now - 3_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        );
        assert!(!live.feed.stale, "fresh heartbeat yields a non-stale feed");
        assert!(live.feed.degraded_reason.is_none());
        assert!(live.ready);
        assert_eq!(live.source, "pg_heartbeat");
    }

    #[test]
    fn stale_heartbeat_is_degraded_not_ready() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now - 60_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        );
        assert!(live.feed.stale);
        assert_eq!(
            live.feed.degraded_reason.as_deref(),
            Some("indexer_heartbeat_stale")
        );
        assert!(!live.ready);
        assert_eq!(live.source, "pg_heartbeat_stale");
    }

    #[test]
    fn absent_heartbeat_is_loud_not_silent() {
        let live =
            resolve_indexer_liveness(1_000_000, None, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS);
        assert!(live.feed.stale);
        assert_eq!(
            live.feed.degraded_reason.as_deref(),
            Some("indexer_heartbeat_absent")
        );
        assert!(!live.ready);
        assert_eq!(live.source, "no_heartbeat");
    }

    #[test]
    fn future_heartbeat_clock_skew_counts_fresh() {
        let now = 1_000_000;
        let live = resolve_indexer_liveness(
            now,
            Some(now + 10_000),
            EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS,
        );
        assert!(
            live.ready,
            "a just-written (skewed) heartbeat is still proof of life"
        );
        assert_eq!(live.source, "pg_heartbeat");
    }
}
