use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(not(test))]
pub(super) fn cache_read(
    cache: &'static Mutex<HashMap<String, (i64, Value)>>,
    key: &str,
    now_ms: i64,
    ttl_ms: i64,
) -> Option<Value> {
    let guard = cache.lock().ok()?;
    let (stored_at, value) = guard.get(key)?;
    if now_ms.saturating_sub(*stored_at) > ttl_ms {
        return None;
    }
    Some(value.clone())
}

#[cfg(test)]
pub(super) fn cache_read(
    _cache: &'static Mutex<HashMap<String, (i64, Value)>>,
    _key: &str,
    _now_ms: i64,
    _ttl_ms: i64,
) -> Option<Value> {
    None
}

#[cfg(not(test))]
pub(super) fn cache_write(
    cache: &'static Mutex<HashMap<String, (i64, Value)>>,
    key: String,
    now_ms: i64,
    value: &Value,
) {
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, (now_ms, value.clone()));
    }
}

#[cfg(test)]
pub(super) fn cache_write(
    _cache: &'static Mutex<HashMap<String, (i64, Value)>>,
    _key: String,
    _now_ms: i64,
    _value: &Value,
) {
}

pub(super) fn structural_history_dir() -> PathBuf {
    std::env::var("AXON_STRUCTURAL_HISTORY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".axon/structural-history"))
}

pub(super) fn structural_history_path(project_code: &str) -> PathBuf {
    structural_history_dir().join(format!("{project_code}.jsonl"))
}

fn load_snapshots_at(path: &std::path::Path) -> Vec<Value> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect()
}

fn persist_snapshot_at(path: &std::path::Path, snapshot: &Value) -> Result<(), String> {
    fs::create_dir_all(structural_history_dir()).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let rendered = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    writeln!(file, "{rendered}").map_err(|error| error.to_string())
}

pub(super) fn load_structural_snapshots(project_code: &str) -> Vec<Value> {
    load_snapshots_at(&structural_history_path(project_code))
}

pub(super) fn persist_structural_snapshot(
    project_code: &str,
    snapshot: &Value,
) -> Result<(), String> {
    persist_snapshot_at(&structural_history_path(project_code), snapshot)
}

/// REQ-AXO-902187 — SHI-specific history file, distinct from the anomaly-count
/// `structural_history_path` (different schema: aggregate + named sub-scores, not
/// wrapper/feature-envy/orphan counts). Same jsonl-append pattern (DRY via
/// `load_snapshots_at`/`persist_snapshot_at`), separate file so the two schemas never mix.
pub(super) fn shi_history_path(project_code: &str) -> PathBuf {
    structural_history_dir().join(format!("{project_code}-shi.jsonl"))
}

pub(super) fn load_shi_snapshots(project_code: &str) -> Vec<Value> {
    load_snapshots_at(&shi_history_path(project_code))
}

pub(super) fn persist_shi_snapshot(project_code: &str, snapshot: &Value) -> Result<(), String> {
    persist_snapshot_at(&shi_history_path(project_code), snapshot)
}

/// REQ-AXO-902187 — per-dimension delta between two SHI snapshots (`sub_scores` name→value
/// maps), plus the aggregate delta. Positive = improved, negative = regressed/stagnated (the
/// re-surfacing signal: a fix only counts once the NEXT measurement confirms the gain, never
/// the LLM's own claim). A dimension absent from `previous` reads as 0.0 (first appearance,
/// no delta to report yet).
pub(super) fn diff_shi_snapshots(current: &Value, previous: &Value) -> Value {
    let sub_score_value = |snap: &Value, name: &str| -> f64 {
        snap.get("sub_scores")
            .and_then(|v| v.as_object())
            .and_then(|obj| obj.get(name))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };
    let aggregate_delta = current.get("aggregate").and_then(|v| v.as_f64()).unwrap_or(0.0)
        - previous.get("aggregate").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut per_dimension = serde_json::Map::new();
    if let Some(obj) = current.get("sub_scores").and_then(|v| v.as_object()) {
        for name in obj.keys() {
            let delta = sub_score_value(current, name) - sub_score_value(previous, name);
            per_dimension.insert(name.clone(), json!(delta));
        }
    }
    json!({
        "aggregate_delta": aggregate_delta,
        "per_dimension_delta": per_dimension,
        "previous_snapshot_id": previous.get("snapshot_id").cloned().unwrap_or(Value::Null)
    })
}

pub(super) fn metric_value(summary: &Value, key: &str) -> i64 {
    summary
        .get(key)
        .and_then(|value| value.as_i64())
        .unwrap_or(0)
}

pub(super) fn diff_metric_summaries(current_summary: &Value, previous_summary: &Value) -> Value {
    let delta_for = |key: &str| -> i64 {
        metric_value(current_summary, key) - metric_value(previous_summary, key)
    };

    json!({
        "wrapper_count_delta": delta_for("wrapper_count"),
        "feature_envy_count_delta": delta_for("feature_envy_count"),
        "detour_count_delta": delta_for("detour_count"),
        "abstraction_detour_count_delta": delta_for("abstraction_detour_count"),
        "orphan_code_count_delta": delta_for("orphan_code_count"),
        "orphan_intent_count_delta": delta_for("orphan_intent_count"),
        "cycle_count_delta": delta_for("cycle_count"),
        "god_object_count_delta": delta_for("god_object_count")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-AXO-902187 — SHI history path is a DISTINCT file from the anomaly-summary
    // history (different schema); only the filename suffix is asserted (the directory
    // prefix depends on AXON_STRUCTURAL_HISTORY_DIR, which other tests may be mutating
    // concurrently — asserting on the env-independent suffix keeps this hermetic).
    #[test]
    fn shi_history_path_is_distinct_from_structural_history_path() {
        let shi = shi_history_path("ZZZ");
        let anomaly = structural_history_path("ZZZ");
        assert_ne!(shi, anomaly);
        assert_eq!(shi.file_name().unwrap().to_str().unwrap(), "ZZZ-shi.jsonl");
        assert_eq!(anomaly.file_name().unwrap().to_str().unwrap(), "ZZZ.jsonl");
    }

    // Exercises the private path-parameterized helpers directly against a tempdir —
    // zero env-var mutation, so no race with sibling tests touching
    // AXON_STRUCTURAL_HISTORY_DIR (GUI-PRO-004: real filesystem I/O, no mock).
    #[test]
    fn persist_and_load_snapshot_round_trips_via_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AXO-shi.jsonl");
        assert!(load_snapshots_at(&path).is_empty());

        let first = json!({"snapshot_id": "v1", "aggregate": 0.5, "sub_scores": {"acyclicity": 0.9}});
        persist_snapshot_at(&path, &first).unwrap();
        let second = json!({"snapshot_id": "v2", "aggregate": 0.6, "sub_scores": {"acyclicity": 0.95}});
        persist_snapshot_at(&path, &second).unwrap();

        let loaded = load_snapshots_at(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0]["snapshot_id"], json!("v1"));
        assert_eq!(loaded[1]["snapshot_id"], json!("v2"));
    }

    #[test]
    fn diff_shi_snapshots_reports_improvement_and_regression_per_dimension() {
        let previous = json!({
            "snapshot_id": "v1",
            "aggregate": 0.5,
            "sub_scores": {"acyclicity": 0.9, "weighted_coverage": 0.4}
        });
        let current = json!({
            "snapshot_id": "v2",
            "aggregate": 0.55,
            "sub_scores": {"acyclicity": 0.85, "weighted_coverage": 0.5}
        });
        let delta = diff_shi_snapshots(&current, &previous);
        assert!((delta["aggregate_delta"].as_f64().unwrap() - 0.05).abs() < 1e-9);
        assert!(delta["per_dimension_delta"]["weighted_coverage"].as_f64().unwrap() > 0.0);
        assert!(
            delta["per_dimension_delta"]["acyclicity"].as_f64().unwrap() < 0.0,
            "acyclicity regressed 0.9 -> 0.85, delta must be negative"
        );
        assert_eq!(delta["previous_snapshot_id"], json!("v1"));
    }

    #[test]
    fn diff_shi_snapshots_first_appearance_of_a_dimension_reads_as_its_own_value() {
        let previous = json!({"snapshot_id": "v1", "aggregate": 0.5, "sub_scores": {}});
        let current = json!({
            "snapshot_id": "v2",
            "aggregate": 0.5,
            "sub_scores": {"intent_alignment": 0.8}
        });
        let delta = diff_shi_snapshots(&current, &previous);
        assert_eq!(delta["per_dimension_delta"]["intent_alignment"], json!(0.8));
    }
}
