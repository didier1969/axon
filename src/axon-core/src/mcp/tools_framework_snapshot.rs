use serde_json::{json, Value};

use super::tools_framework_support::{
    diff_metric_summaries, load_structural_snapshots, structural_history_path,
};
use super::McpServer;

impl McpServer {
    pub(super) fn axon_snapshot_history_impl(&self, args: &Value) -> Value {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let limit = args
            .get("limit")
            .and_then(|value| value.as_u64())
            .unwrap_or(10) as usize;
        let snapshots = load_structural_snapshots(project_code);
        let count = snapshots.len();
        let start = count.saturating_sub(limit);
        json!({
            "content": [{ "type": "text", "text": format!("snapshot_history returned {} snapshot(s) for {}", count.saturating_sub(start), project_code) }],
            "data": {
                "project_code": project_code,
                "snapshots": snapshots.into_iter().skip(start).collect::<Vec<_>>(),
                "storage": {
                    "scope": "derived_non_canonical",
                    "path": structural_history_path(project_code).to_string_lossy().to_string()
                },
                "provenance": "aggregated",
                "confidence": "high",
                "evidence_sources": ["project_status_snapshots"],
                "safe_to_act": false,
                "needs_human_confirmation": false
            }
        })
    }

    pub(super) fn axon_snapshot_diff_impl(&self, args: &Value) -> Value {
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("AXO");
        let snapshots = load_structural_snapshots(project_code);
        if snapshots.is_empty() {
            return json!({
                "content": [{ "type": "text", "text": format!("No structural snapshots found for {}", project_code) }],
                "isError": true
            });
        }
        let from_snapshot_id = args
            .get("from_snapshot_id")
            .and_then(|value| value.as_str());
        let to_snapshot_id = args.get("to_snapshot_id").and_then(|value| value.as_str());

        let resolve = |snapshot_id: Option<&str>, prefer_last: bool| -> Option<Value> {
            snapshot_id
                .and_then(|id| {
                    snapshots
                        .iter()
                        .find(|item| {
                            item.get("snapshot_id").and_then(|value| value.as_str()) == Some(id)
                        })
                        .cloned()
                })
                .or_else(|| {
                    if prefer_last {
                        snapshots.last().cloned()
                    } else if snapshots.len() >= 2 {
                        snapshots.get(snapshots.len() - 2).cloned()
                    } else {
                        snapshots.first().cloned()
                    }
                })
        };

        let from_snapshot = match resolve(from_snapshot_id, false) {
            Some(value) => value,
            None => {
                return json!({
                    "content": [{ "type": "text", "text": format!("No structural snapshots found for {}", project_code) }],
                    "isError": true
                });
            }
        };
        let to_snapshot = match resolve(to_snapshot_id, true) {
            Some(value) => value,
            None => {
                return json!({
                    "content": [{ "type": "text", "text": format!("No structural snapshots found for {}", project_code) }],
                    "isError": true
                });
            }
        };
        let from_summary = from_snapshot
            .get("anomaly_summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let to_summary = to_snapshot
            .get("anomaly_summary")
            .cloned()
            .unwrap_or_else(|| json!({}));
        json!({
            "content": [{ "type": "text", "text": format!(
                "snapshot_diff compared {} -> {}",
                from_snapshot.get("snapshot_id").and_then(|value| value.as_str()).unwrap_or("unknown"),
                to_snapshot.get("snapshot_id").and_then(|value| value.as_str()).unwrap_or("unknown")
            ) }],
            "data": {
                "project_code": project_code,
                "from_snapshot_id": from_snapshot.get("snapshot_id").cloned().unwrap_or(Value::Null),
                "to_snapshot_id": to_snapshot.get("snapshot_id").cloned().unwrap_or(Value::Null),
                "metric_delta": diff_metric_summaries(&to_summary, &from_summary),
                "storage": {
                    "scope": "derived_non_canonical",
                    "path": structural_history_path(project_code).to_string_lossy().to_string()
                },
                "provenance": "aggregated",
                "confidence": "high",
                "evidence_sources": ["project_status_snapshots"],
                "safe_to_act": false,
                "needs_human_confirmation": false
            }
        })
    }
}
