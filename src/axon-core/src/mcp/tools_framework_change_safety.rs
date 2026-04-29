use serde_json::{json, Value};

pub(super) fn summarize_change_safety(
    coverage_signals: &Value,
    traceability_signals: &Value,
    validation_signals: &Value,
) -> (&'static str, Vec<String>, Vec<String>, &'static str) {
    let tested = coverage_signals
        .get("tested")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let traceability_links = traceability_signals
        .get("traceability_links")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let validation_nodes = validation_signals
        .get("validation_nodes")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let verifies_edges = validation_signals
        .get("verifies_edges")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    let mut reasoning = Vec::new();
    if tested {
        reasoning.push("target has direct test coverage".to_string());
    } else {
        reasoning.push("target lacks direct test coverage".to_string());
    }
    if traceability_links > 0 {
        reasoning.push(format!(
            "target has {} traceability link(s)",
            traceability_links
        ));
    } else {
        reasoning.push("target has no traceability links".to_string());
    }
    if validation_nodes > 0 || verifies_edges > 0 {
        reasoning.push("target is linked to validation evidence".to_string());
    } else {
        reasoning.push("target has no linked validation proof".to_string());
    }

    let guardrails = if tested {
        vec![
            "run focused tests before and after change".to_string(),
            "confirm rationale still holds with `why`".to_string(),
        ]
    } else if traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
        vec![
            "review `impact` before mutation".to_string(),
            "add or refresh validation evidence after change".to_string(),
        ]
    } else {
        vec![
            "do not mutate without human review".to_string(),
            "establish traceability or tests before refactor".to_string(),
        ]
    };

    let safety = if tested {
        "safe"
    } else if traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
        "caution"
    } else {
        "unsafe"
    };
    let confidence = if tested && traceability_links > 0 {
        "high"
    } else if tested || traceability_links > 0 || validation_nodes > 0 || verifies_edges > 0 {
        "medium"
    } else {
        "low"
    };
    (safety, reasoning, guardrails, confidence)
}

pub(super) fn change_safety_operator_guidance(
    change_safety: &str,
    coverage_signals: &Value,
    traceability_signals: &Value,
    validation_signals: &Value,
) -> Value {
    let tested = coverage_signals
        .get("tested")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let traceability_links = traceability_signals
        .get("traceability_links")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let validation_nodes = validation_signals
        .get("validation_nodes")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let verifies_edges = validation_signals
        .get("verifies_edges")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    let mut blocking_factors = Vec::<Value>::new();
    if !tested {
        blocking_factors.push(json!({
            "factor": "missing_direct_test_coverage",
            "severity": "high",
            "recommended_action": "add or refresh direct tests before mutation"
        }));
    }
    if traceability_links == 0 {
        blocking_factors.push(json!({
            "factor": "missing_traceability_links",
            "severity": "high",
            "recommended_action": "link the target to canonical SOLL intent or evidence before mutation"
        }));
    }
    if validation_nodes == 0 && verifies_edges == 0 {
        blocking_factors.push(json!({
            "factor": "missing_validation_proof",
            "severity": "medium",
            "recommended_action": "create or attach validation proof before high-risk mutation"
        }));
    }

    let mutation_class_recommendation = match change_safety {
        "safe" => "safe_for_direct_mutation",
        "caution" if traceability_links > 0 => "safe_for_guarded_mutation",
        "caution" => "prefer_small_guarded_mutation",
        _ => "defer_or_prepare_evidence_first",
    };
    let recommended_next_step = match change_safety {
        "safe" => "proceed_with_mutation_after_impact_check",
        "caution" if !tested => "add_targeted_tests_then_reassess",
        "caution" if traceability_links == 0 => "link_traceability_then_reassess",
        "caution" => "use_small_guarded_mutation_with_explicit_validation_plan",
        _ => "stop_and_prepare_traceability_tests_and_validation_first",
    };
    let remediation_actions = blocking_factors
        .iter()
        .filter_map(|factor| {
            factor
                .get("recommended_action")
                .and_then(|value| value.as_str())
                .map(|value| Value::from(value.to_string()))
        })
        .collect::<Vec<_>>();

    json!({
        "mutation_class_recommendation": mutation_class_recommendation,
        "recommended_next_step": recommended_next_step,
        "actionable_now": change_safety == "safe",
        "blocking_factors": blocking_factors,
        "remediation_actions": remediation_actions
    })
}
