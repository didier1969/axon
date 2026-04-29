use serde_json::{json, Value};

pub(super) fn project_status_operator_guidance(
    degraded_notes: &[String],
    snapshot_storage: &Value,
    vision: &Value,
) -> Value {
    let mut blocking_factors = Vec::<Value>::new();

    for note in degraded_notes {
        blocking_factors.push(json!({
            "factor": "runtime_degraded_note",
            "severity": "high",
            "detail": note,
            "recommended_action": "inspect `status` and clear degraded runtime conditions before relying on project-wide conclusions"
        }));
    }

    if snapshot_storage
        .get("persisted")
        .and_then(|value| value.as_bool())
        == Some(false)
    {
        blocking_factors.push(json!({
            "factor": "snapshot_persistence_failed",
            "severity": "medium",
            "recommended_action": "repair structural snapshot persistence before depending on historical delta tracking"
        }));
    }

    if vision
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("unavailable")
        == "unavailable"
    {
        blocking_factors.push(json!({
            "factor": "vision_unavailable",
            "severity": "medium",
            "recommended_action": "refresh SOLL context so project steering is anchored on a canonical vision"
        }));
    }

    blocking_factors.push(json!({
        "factor": "anomalies_decoupled",
        "severity": "low",
        "recommended_action": "run `anomalies` explicitly when you need the full structural findings payload"
    }));

    let remediation_actions = blocking_factors
        .iter()
        .filter_map(|factor| {
            factor
                .get("recommended_action")
                .and_then(|value| value.as_str())
                .map(|value| Value::from(value.to_string()))
        })
        .collect::<Vec<_>>();

    let recommended_next_step = if !degraded_notes.is_empty() {
        "inspect_runtime_status_then_refresh_project_status"
    } else if snapshot_storage
        .get("persisted")
        .and_then(|value| value.as_bool())
        == Some(false)
    {
        "repair_snapshot_storage_then_refresh_project_status"
    } else if vision
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("unavailable")
        == "unavailable"
    {
        "refresh_soll_context_then_reassess_project_status"
    } else {
        "run_anomalies_explicitly_then_follow_with_why_or_path"
    };

    let next_action = match recommended_next_step {
        "inspect_runtime_status_then_refresh_project_status" => json!({
            "kind": "inspect_runtime_status",
            "tool": "status",
            "when": "now"
        }),
        "repair_snapshot_storage_then_refresh_project_status" => json!({
            "kind": "repair_snapshot_storage",
            "tool": "project_status",
            "when": "after_storage_fix"
        }),
        "refresh_soll_context_then_reassess_project_status" => json!({
            "kind": "refresh_soll_context",
            "tool": "soll_query_context",
            "when": "now"
        }),
        _ => json!({
            "kind": "expand_structural_findings",
            "tool": "anomalies",
            "when": "now"
        }),
    };

    json!({
        "recommended_next_step": recommended_next_step,
        "actionable_now": degraded_notes.is_empty(),
        "blocking_factors": blocking_factors,
        "remediation_actions": remediation_actions,
        "follow_up_tools": ["anomalies", "why", "path"],
        "next_action": next_action
    })
}
