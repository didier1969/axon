//! REQ-AXO-902053 — Visualization-surface freshness signals (PIL-AXO-009,
//! DEC-AXO-901640 G3).
//!
//! The human-facing visualization surfaces (Memgraph projection, SOLL autodoc,
//! dashboard) are **non-canonical, reconstructible** projections of PG truth.
//! Without a freshness signal in MCP, a skipped publish (Docker down) or a
//! stale projection drifts silently. This module surfaces two anti-drift
//! signals so `status`/`health` (and the dashboard event) expose them in one
//! call — no shell, no guessing:
//!
//!   1. **Memgraph last-publish marker** — `.axon/memgraph/last_publish.json`,
//!      written by `scripts/publish-memgraph.sh` (status/detail/at_unix/
//!      source_commit). Read here, aged, and given a verdict.
//!   2. **Last SOLL revision** — a process-global signal updated by the
//!      `soll_revision_committed` listener (the dashboard's autodoc-regen
//!      subscriber). Surfaces "when did the SOLL knowledge last change" so the
//!      operator sees the dashboard/autodoc is tracking live mutations, and the
//!      dashboard event (`compose_dashboard_state_v1`, P2) carries the same
//!      signal.
//!
//! Both are best-effort: a missing marker or never-fired listener degrades to
//! an explicit `absent` verdict, never an error.

use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};

/// A Memgraph publication older than this (and otherwise `ok`) is judged
/// `stale` — the DEC-AXO-901640 cadence target is "refresh post-promote OR
/// ≥1×/day", so 24 h is the drift ceiling for a healthy projection.
pub const MEMGRAPH_STALE_AFTER_SECONDS: i64 = 86_400;

/// Process-global record of the most recent `soll_revision_committed` NOTIFY
/// the brain observed. `count` is the lifetime number of project autodoc-regen
/// events seen since boot (one per affected project per coalesced listener
/// wake), not a PG row count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SollRevisionSignal {
    pub project_code: String,
    pub at_ms: i64,
    pub count: u64,
}

static LAST_SOLL_REVISION: OnceLock<Mutex<Option<SollRevisionSignal>>> = OnceLock::new();

fn soll_revision_slot() -> &'static Mutex<Option<SollRevisionSignal>> {
    LAST_SOLL_REVISION.get_or_init(|| Mutex::new(None))
}

/// Record a SOLL revision burst. Called by the `soll_revision_committed`
/// listener on every coalesced wake so the freshness signal tracks live
/// mutations. Increments the lifetime burst count and stamps the time.
pub fn record_soll_revision(project_code: impl Into<String>, at_ms: i64) {
    if let Ok(mut guard) = soll_revision_slot().lock() {
        let prev_count = guard.as_ref().map(|s| s.count).unwrap_or(0);
        *guard = Some(SollRevisionSignal {
            project_code: project_code.into(),
            at_ms,
            count: prev_count.saturating_add(1),
        });
    }
}

/// The latest observed SOLL revision signal, or `None` if the listener has
/// never fired this process.
pub fn latest_soll_revision() -> Option<SollRevisionSignal> {
    soll_revision_slot().lock().ok().and_then(|g| g.clone())
}

/// Resolve the Memgraph last-publish marker path from the runtime env. The
/// marker lives at `<AXON_PROJECT_ROOT>/.axon/memgraph/last_publish.json`
/// (written by `publish-memgraph.sh`); falls back to a repo-relative path when
/// the env var is absent (e.g. a test or a bare CLI invocation).
fn memgraph_marker_path() -> std::path::PathBuf {
    let base = std::env::var("AXON_PROJECT_ROOT").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base)
        .join(".axon")
        .join("memgraph")
        .join("last_publish.json")
}

/// Parse a marker JSON body into a freshness snapshot, aging it against
/// `now_unix` and deriving a verdict. Pure (path/clock injected) so it is
/// unit-testable without filesystem or wall-clock access.
///
/// Verdict:
/// * `absent`  — no marker on disk yet (never published).
/// * `fresh`   — last publish `ok` and within [`MEMGRAPH_STALE_AFTER_SECONDS`].
/// * `stale`   — last publish `ok` but older than the drift ceiling.
/// * `skipped` — clean skip (Docker down, throttled, missing tool) — `detail`.
/// * `failed`  — a publish step errored — `detail` carries which.
/// * `unknown` — marker present but unparseable.
fn memgraph_snapshot_from_body(body: Option<&str>, now_unix: i64) -> Value {
    let Some(body) = body else {
        return json!({ "verdict": "absent", "detail": "no publication marker yet" });
    };
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return json!({ "verdict": "unknown", "detail": "marker present but unparseable" });
    };
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("unknown");
    let detail = parsed.get("detail").and_then(Value::as_str).unwrap_or("");
    let at_unix = parsed.get("at_unix").and_then(Value::as_i64).unwrap_or(0);
    let source_commit = parsed
        .get("source_commit")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let age_seconds = (now_unix - at_unix).max(0);
    let verdict = match status {
        "ok" => {
            if age_seconds > MEMGRAPH_STALE_AFTER_SECONDS {
                "stale"
            } else {
                "fresh"
            }
        }
        "skipped" => "skipped",
        "failed" => "failed",
        _ => "unknown",
    };
    json!({
        "verdict": verdict,
        "status": status,
        "detail": detail,
        "at_unix": at_unix,
        "age_seconds": age_seconds,
        "source_commit": source_commit,
    })
}

/// Read + age the Memgraph publication marker from disk. Best-effort: a missing
/// or unreadable marker yields the `absent` verdict.
pub fn memgraph_last_publish_snapshot(now_unix: i64) -> Value {
    let body = std::fs::read_to_string(memgraph_marker_path()).ok();
    memgraph_snapshot_from_body(body.as_deref(), now_unix)
}

/// The SOLL-revision side of the viz-freshness block, aged against `now_ms`.
/// Public so the dashboard event (`compose_dashboard_state_v1`, P2) reuses the
/// exact same shape without re-reading the Memgraph marker file each 1 Hz tick.
pub fn soll_revision_snapshot(now_ms: i64) -> Value {
    match latest_soll_revision() {
        Some(sig) => json!({
            "verdict": "observed",
            "project_code": sig.project_code,
            "at_ms": sig.at_ms,
            "age_ms": (now_ms - sig.at_ms).max(0),
            "bursts_since_boot": sig.count,
        }),
        None => json!({ "verdict": "none_since_boot" }),
    }
}

/// Combined visualization-freshness block for `status`/`health` (and reused as
/// the `soll_revision` source for the dashboard event). `now_ms` is the
/// caller's clock; the Memgraph marker uses unix seconds derived from it.
pub fn viz_freshness_snapshot(now_ms: i64) -> Value {
    json!({
        "memgraph_publication": memgraph_last_publish_snapshot(now_ms / 1000),
        "soll_revision": soll_revision_snapshot(now_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_marker_yields_absent_verdict() {
        let snap = memgraph_snapshot_from_body(None, 1_000);
        assert_eq!(snap["verdict"], "absent");
    }

    #[test]
    fn unparseable_marker_yields_unknown_verdict() {
        let snap = memgraph_snapshot_from_body(Some("not json"), 1_000);
        assert_eq!(snap["verdict"], "unknown");
    }

    #[test]
    fn ok_recent_publish_is_fresh_with_age() {
        let body = r#"{"status":"ok","detail":"published pub-99","at_unix":900,"source_commit":"abc1234"}"#;
        let snap = memgraph_snapshot_from_body(Some(body), 1_000);
        assert_eq!(snap["verdict"], "fresh");
        assert_eq!(snap["age_seconds"], 100);
        assert_eq!(snap["source_commit"], "abc1234");
        assert_eq!(snap["status"], "ok");
    }

    #[test]
    fn ok_old_publish_is_stale() {
        let body = r#"{"status":"ok","detail":"published pub-1","at_unix":0,"source_commit":"abc"}"#;
        let snap = memgraph_snapshot_from_body(Some(body), MEMGRAPH_STALE_AFTER_SECONDS + 10);
        assert_eq!(snap["verdict"], "stale");
    }

    #[test]
    fn skipped_publish_surfaces_detail() {
        let body = r#"{"status":"skipped","detail":"missing tool: psql","at_unix":500,"source_commit":"abc"}"#;
        let snap = memgraph_snapshot_from_body(Some(body), 1_000);
        assert_eq!(snap["verdict"], "skipped");
        assert_eq!(snap["detail"], "missing tool: psql");
    }

    #[test]
    fn failed_publish_yields_failed_verdict() {
        let body = r#"{"status":"failed","detail":"export step failed","at_unix":500,"source_commit":"abc"}"#;
        let snap = memgraph_snapshot_from_body(Some(body), 1_000);
        assert_eq!(snap["verdict"], "failed");
        assert_eq!(snap["detail"], "export step failed");
    }

    #[test]
    fn soll_revision_records_and_ages() {
        // Isolated from the process-global by asserting monotonic behaviour:
        // record twice, the count must rise and the latest project/time win.
        record_soll_revision("AXO", 10_000);
        let first = latest_soll_revision().expect("a signal after record");
        assert_eq!(first.project_code, "AXO");
        assert_eq!(first.at_ms, 10_000);
        record_soll_revision("OPT", 20_000);
        let second = latest_soll_revision().expect("a signal after second record");
        assert_eq!(second.project_code, "OPT");
        assert_eq!(second.at_ms, 20_000);
        assert!(
            second.count > first.count,
            "burst count must increase across records ({} !> {})",
            second.count,
            first.count
        );
    }

    #[test]
    fn soll_revision_snapshot_shape_when_observed() {
        record_soll_revision("AXO", 5_000);
        let snap = soll_revision_snapshot(8_000);
        assert_eq!(snap["verdict"], "observed");
        assert_eq!(snap["project_code"], "AXO");
        assert_eq!(snap["age_ms"], 3_000);
    }

    #[test]
    fn viz_freshness_snapshot_has_both_blocks() {
        let snap = viz_freshness_snapshot(1_000_000);
        assert!(snap.get("memgraph_publication").is_some());
        assert!(snap.get("soll_revision").is_some());
    }
}
