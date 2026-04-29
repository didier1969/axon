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

pub(super) fn load_structural_snapshots(project_code: &str) -> Vec<Value> {
    let path = structural_history_path(project_code);
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

pub(super) fn persist_structural_snapshot(
    project_code: &str,
    snapshot: &Value,
) -> Result<(), String> {
    let dir = structural_history_dir();
    fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = structural_history_path(project_code);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let rendered = serde_json::to_string(snapshot).map_err(|error| error.to_string())?;
    writeln!(file, "{rendered}").map_err(|error| error.to_string())
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
