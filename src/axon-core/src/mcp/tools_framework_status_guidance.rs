use serde_json::{json, Value};

pub(super) fn project_status_operator_guidance(
    degraded_notes: &[String],
    snapshot_storage: &Value,
    vision: &Value,
    project_code: &str,
) -> Value {
    let mut blocking_factors = Vec::<Value>::new();

    for note in degraded_notes {
        let (factor, action) = if note == "indexed_projections_not_fresh" {
            (
                "indexed_projections_not_fresh",
                "start indexer via `axon-live start --indexer-graph` to maintain realtime freshness (CPT-AXO-029)".to_string(),
            )
        } else if note.contains("aucun fichier indexe") || note == "no_indexed_files_for_project" {
            (
                "no_indexed_files",
                format!("run `diagnose_indexing project={project_code}` to inspect ingestion state"),
            )
        } else if note.starts_with("ist_writer_") {
            (
                "ist_writer_degraded",
                "inspect `status mode=verbose` and verify ist_writer subsystem health".to_string(),
            )
        } else {
            (
                "runtime_degraded_note",
                format!("address runtime degradation: {note}"),
            )
        };

        blocking_factors.push(json!({
            "factor": factor,
            "severity": "high",
            "detail": note,
            "recommended_action": action
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

    // REQ-AXO-902546 — next_action must be concrete and actionable, NEVER recursively
    // pointing back to `status` without parameters (which led LLMs into an infinite loop).
    let (recommended_next_step, next_action) = if let Some(ist_note) = degraded_notes.iter().find(|n| n.starts_with("ist_writer_")) {
        (
            "inspect_ist_writer",
            json!({
                "kind": "inspect_ist_writer",
                "tool": "status",
                "arguments": { "mode": "verbose" },
                "reason": ist_note,
                "when": "now"
            }),
        )
    } else if degraded_notes.iter().any(|n| n.contains("aucun fichier indexe") || n == "no_indexed_files_for_project") {
        (
            "diagnose_indexing",
            json!({
                "kind": "diagnose_indexing",
                "tool": "diagnose_indexing",
                "arguments": { "project": project_code },
                "when": "now"
            }),
        )
    } else if degraded_notes.iter().any(|n| n == "indexed_projections_not_fresh") {
        (
            "start_indexer",
            json!({
                "kind": "start_indexer",
                "tool": "axon-live",
                "arguments": { "command": "start --indexer-graph" },
                "when": "now"
            }),
        )
    } else if snapshot_storage
        .get("persisted")
        .and_then(|value| value.as_bool())
        == Some(false)
    {
        (
            "repair_snapshot_storage_then_refresh_project_status",
            json!({
                "kind": "repair_snapshot_storage",
                "tool": "project_status",
                "when": "after_storage_fix"
            }),
        )
    } else if vision
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("unavailable")
        == "unavailable"
    {
        (
            "refresh_soll_context_then_reassess_project_status",
            json!({
                "kind": "refresh_soll_context",
                "tool": "soll_query_context",
                "when": "now"
            }),
        )
    } else if !degraded_notes.is_empty() {
        (
            "inspect_project_indexing",
            json!({
                "kind": "inspect_project_indexing",
                "tool": "diagnose_indexing",
                "arguments": { "project": project_code },
                "when": "now"
            }),
        )
    } else {
        (
            "run_anomalies_explicitly_then_follow_with_why_or_path",
            json!({
                "kind": "expand_structural_findings",
                "tool": "anomalies",
                "when": "now"
            }),
        )
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
