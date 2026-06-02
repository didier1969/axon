use crate::bridge::RuntimeTruthFeed;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// REQ-AXO-901859 — peer info now carries ONLY version/telemetry metadata.
/// Indexer liveness (feed + ready) was removed from here: it is derived
/// exclusively from the PG heartbeat via `resolve_indexer_liveness`, so a
/// single source of truth feeds every consumer (PIL-AXO-001).
#[derive(Debug, Clone)]
pub(crate) struct SplitPeerRuntimeInfo {
    pub(crate) release_version: Option<String>,
    pub(crate) build_id: Option<String>,
    pub(crate) install_generation: Option<String>,
    pub(crate) runtime_mode: Option<String>,
    pub(crate) runtime_telemetry: Option<Value>,
    // REQ-AXO-901836 — indexer's lane_parameters (vector_workers,
    // graph_workers, batch sizes) surfaced via heartbeat. Brain composer
    // overrides its own local config with these when paired.
    pub(crate) lane_parameters: Option<Value>,
}

fn split_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

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
                source: if fresh { "pg_heartbeat" } else { "pg_heartbeat_stale" },
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

fn split_runtime_heartbeat_path(
    project_root: &str,
    instance_kind: &str,
    role_slug: &str,
) -> PathBuf {
    split_run_root(project_root, instance_kind, role_slug).join("runtime-heartbeat.json")
}

fn split_runtime_truth_feed_from_heartbeat(path: &PathBuf) -> Option<(RuntimeTruthFeed, Value)> {
    let payload = fs::read_to_string(path).ok()?;
    let payload: Value = serde_json::from_str(&payload).ok()?;
    let runtime_truth_feed = payload
        .get("runtime_truth_feed")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .or_else(|| {
            let now_ms = split_now_ms();
            Some(RuntimeTruthFeed::from_observed_times(
                now_ms,
                payload.get("last_heartbeat_at_ms").and_then(Value::as_u64),
                payload
                    .get("last_good_payload_at_ms")
                    .and_then(Value::as_u64),
                payload
                    .get("stale_after_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS),
                payload
                    .get("degraded_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .as_deref(),
            ))
        })?;
    Some((runtime_truth_feed, payload))
}

fn split_pid_file_path(project_root: &str, instance_kind: &str, role_slug: &str) -> PathBuf {
    split_run_root(project_root, instance_kind, role_slug).join(format!("axon-{role_slug}.pid"))
}

fn pid_is_live(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

fn file_mtime_ms(path: &PathBuf) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(elapsed.as_millis() as u64)
}

fn split_runtime_truth_feed_from_runtime_state(
    runtime_state_path: &PathBuf,
    pid_path: &PathBuf,
) -> Option<RuntimeTruthFeed> {
    let runtime_state = split_runtime_state_from_file(runtime_state_path)?;
    let pid = fs::read_to_string(pid_path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    let now_ms = split_now_ms();
    let last_heartbeat_at_ms = file_mtime_ms(runtime_state_path)
        .or_else(|| file_mtime_ms(pid_path))
        .or(Some(now_ms));
    let degraded_reason = match pid {
        Some(pid) if pid_is_live(pid) => None,
        Some(_) => Some("indexer_process_not_live"),
        None => Some("indexer_pid_missing"),
    };
    let stale_after_ms = RuntimeTruthFeed::DEFAULT_STALE_AFTER_MS;
    let mut feed = RuntimeTruthFeed::from_observed_times(
        now_ms,
        last_heartbeat_at_ms,
        last_heartbeat_at_ms,
        stale_after_ms,
        degraded_reason,
    );
    if degraded_reason.is_none() {
        feed.stale = false;
        feed.degraded_reason = None;
    }
    let _ = runtime_state;
    Some(feed)
}

pub(crate) fn split_peer_runtime_info(
    project_root: &str,
    instance_kind: &str,
    role_slug: &str,
) -> Option<SplitPeerRuntimeInfo> {
    let run_root = split_run_root(project_root, instance_kind, role_slug);
    let runtime_state_path = run_root.join("runtime.env");
    let runtime_state = split_runtime_state_from_file(&runtime_state_path);
    let pid_path = split_pid_file_path(project_root, instance_kind, role_slug);
    let runtime_heartbeat_path =
        split_runtime_heartbeat_path(project_root, instance_kind, role_slug);
    // REQ-AXO-901859 — only the metadata payload is kept; the file-derived
    // truth feed is no longer surfaced (liveness = PG heartbeat alone). The
    // `?` on the runtime-state branch preserves the prior "peer present iff a
    // heartbeat export OR a readable runtime.env feed exists" semantics.
    let payload = match split_runtime_truth_feed_from_heartbeat(&runtime_heartbeat_path) {
        Some((_feed, payload)) => payload,
        None => {
            split_runtime_truth_feed_from_runtime_state(&runtime_state_path, &pid_path)?;
            json!({})
        }
    };
    let release_version = payload
        .get("release_version")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| runtime_state.as_ref()?.get("AXON_RELEASE_VERSION").cloned());
    let build_id = payload
        .get("build_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| runtime_state.as_ref()?.get("AXON_BUILD_ID").cloned());
    let install_generation = payload
        .get("install_generation")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            runtime_state
                .as_ref()?
                .get("AXON_INSTALL_GENERATION")
                .cloned()
        });
    let runtime_mode = payload
        .get("runtime_mode")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| runtime_state.as_ref()?.get("AXON_RUNTIME_MODE").cloned());
    let runtime_telemetry = payload.get("runtime_telemetry").cloned();
    // REQ-AXO-901836 — lane_parameters block published by indexer's heartbeat.
    // Contains the indexer's effective vector_workers / graph_workers /
    // batch sizes. Brain forwards these so resource_policy + lane_parameters
    // surfaces reflect the paired indexer's truth, not the brain's local
    // (brain_only, vector_workers=0) config.
    let lane_parameters = payload
        .get("lane_parameters")
        .cloned()
        .filter(|value| !value.is_null());

    Some(SplitPeerRuntimeInfo {
        release_version,
        build_id,
        install_generation,
        runtime_mode,
        runtime_telemetry,
        lane_parameters,
    })
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
        let live =
            resolve_indexer_liveness(now, Some(now - 3_000), EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS);
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
        assert_eq!(live.feed.degraded_reason.as_deref(), Some("indexer_heartbeat_stale"));
        assert!(!live.ready);
        assert_eq!(live.source, "pg_heartbeat_stale");
    }

    #[test]
    fn absent_heartbeat_is_loud_not_silent() {
        let live = resolve_indexer_liveness(1_000_000, None, EMBEDDER_LIFECYCLE_HEARTBEAT_FRESHNESS_MS);
        assert!(live.feed.stale);
        assert_eq!(live.feed.degraded_reason.as_deref(), Some("indexer_heartbeat_absent"));
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
        assert!(live.ready, "a just-written (skewed) heartbeat is still proof of life");
        assert_eq!(live.source, "pg_heartbeat");
    }
}
