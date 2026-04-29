use serde_json::Value;

pub(super) fn recommend_effort_and_risk(
    kind: &str,
    validation_signals: &Value,
) -> (&'static str, &'static str) {
    match kind {
        "wrapper" => {
            let tested = validation_signals
                .get("tested")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if tested {
                ("low", "low")
            } else {
                ("low", "medium")
            }
        }
        "orphan_code" => {
            let tested = validation_signals
                .get("tested")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if tested {
                ("medium", "medium")
            } else {
                ("medium", "high")
            }
        }
        "feature_envy" => ("medium", "medium"),
        "detour" => ("medium", "medium"),
        "abstraction_detour" => ("medium", "medium"),
        "orphan_intent" => ("medium", "high"),
        "cycle" => ("high", "high"),
        "god_object" => ("high", "medium"),
        _ => ("unknown", "unknown"),
    }
}

pub(super) fn sequencing_dependencies_for_anomaly(anomaly_type: &str) -> Vec<&'static str> {
    match anomaly_type {
        "wrapper" => vec![
            "confirm target API stability",
            "check callers with `impact`",
        ],
        "feature_envy" => vec![
            "confirm natural owning module",
            "review move impact with `path`",
        ],
        "detour" => vec![
            "confirm hop is not policy-bearing",
            "review direct caller/callee contract",
        ],
        "abstraction_detour" => vec![
            "confirm abstraction has no second implementation planned",
            "inspect public API commitments",
        ],
        "orphan_code" => vec!["search SOLL rationale with `why`", "decide link vs delete"],
        "orphan_intent" => vec!["inspect `soll_work_plan`", "attach implementation or proof"],
        "heuristic_intent_gap" => vec![
            "compare with `soll_validate` completeness axes",
            "defer if concept baseline is already complete",
        ],
        "cycle" => vec!["confirm if cycle is intentional", "review module boundary"],
        "god_object" => vec!["inspect fan-in consumers", "stage decomposition carefully"],
        _ => vec!["review manually"],
    }
}
