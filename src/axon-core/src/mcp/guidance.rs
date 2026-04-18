use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GuidanceFact {
    RequestedTarget(String),
    ResolvedProjectScope(String),
    CandidateSymbol(String),
    CandidateProjectCode(String),
    ProblemSignal(String),
    CanonicalSource(String),
    ResultDegraded(String),
    IndexIncomplete,
    VectorizationIncomplete,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GuidanceCandidates {
    pub(crate) symbols: Vec<String>,
    pub(crate) project_codes: Vec<String>,
    pub(crate) canonical_sources: Vec<String>,
}

impl GuidanceFact {
    pub(crate) fn requested_target(value: impl Into<String>) -> Self {
        Self::RequestedTarget(value.into())
    }

    pub(crate) fn resolved_project_scope(value: impl Into<String>) -> Self {
        Self::ResolvedProjectScope(value.into())
    }

    pub(crate) fn candidate_symbol(value: impl Into<String>) -> Self {
        Self::CandidateSymbol(value.into())
    }

    pub(crate) fn candidate_project_code(value: impl Into<String>) -> Self {
        Self::CandidateProjectCode(value.into())
    }

    pub(crate) fn problem_signal(value: impl Into<String>) -> Self {
        Self::ProblemSignal(value.into())
    }

    pub(crate) fn canonical_source(value: impl Into<String>) -> Self {
        Self::CanonicalSource(value.into())
    }

    pub(crate) fn result_degraded(value: impl Into<String>) -> Self {
        Self::ResultDegraded(value.into())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SollGuidance {
    pub(crate) recommended_action: String,
    pub(crate) update_kind: String,
    pub(crate) reason: String,
    pub(crate) requires_authorization: Option<bool>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuidanceOutcome {
    pub(crate) problem_class: Option<String>,
    pub(crate) likely_cause: Option<String>,
    pub(crate) next_best_actions: Vec<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) soll: Option<SollGuidance>,
}

#[allow(dead_code)]
impl GuidanceOutcome {
    pub(crate) fn none() -> Self {
        Self {
            problem_class: None,
            likely_cause: None,
            next_best_actions: Vec::new(),
            confidence: None,
            soll: None,
        }
    }
}

#[allow(dead_code)]
pub(crate) fn project_authoritative_phase1_guidance(outcome: GuidanceOutcome) -> GuidanceOutcome {
    let Some(problem_class) = outcome.problem_class.as_deref() else {
        return outcome;
    };

    match problem_class {
        "input_not_found"
        | "input_ambiguous"
        | "wrong_project_scope"
        | "tool_unavailable"
        | "degraded" => outcome,
        _ => GuidanceOutcome::none(),
    }
}

#[allow(dead_code)]
pub(crate) fn classify_guidance(facts: &[GuidanceFact]) -> GuidanceOutcome {
    // Precedence must stay aligned with the frozen taxonomy document.
    let has_signal = |needle: &str| {
        facts
            .iter()
            .any(|fact| matches!(fact, GuidanceFact::ProblemSignal(signal) if signal == needle))
    };
    let has_fact = |expected: &GuidanceFact| facts.iter().any(|fact| fact == expected);

    if has_signal("tool_unavailable") {
        return GuidanceOutcome {
            problem_class: Some("tool_unavailable".to_string()),
            likely_cause: Some("runtime_profile_does_not_allow_tool".to_string()),
            next_best_actions: vec![
                "switch_to_supported_runtime_profile".to_string(),
                "retry_tool_after_runtime_change".to_string(),
            ],
            confidence: Some("high".to_string()),
            soll: None,
        };
    }

    if has_signal("wrong_project_scope") {
        return GuidanceOutcome {
            problem_class: Some("wrong_project_scope".to_string()),
            likely_cause: Some("non_canonical_or_incorrect_project_code".to_string()),
            next_best_actions: vec![
                "use_canonical_project_code".to_string(),
                "run_project_status".to_string(),
            ],
            confidence: Some("high".to_string()),
            soll: None,
        };
    }

    if has_signal("input_ambiguous") {
        return GuidanceOutcome {
            problem_class: Some("input_ambiguous".to_string()),
            likely_cause: Some("multiple_plausible_targets".to_string()),
            next_best_actions: vec![
                "pick_exact_symbol".to_string(),
                "narrow_project_scope".to_string(),
            ],
            confidence: Some("medium".to_string()),
            soll: None,
        };
    }

    if has_signal("input_not_found") {
        return GuidanceOutcome {
            problem_class: Some("input_not_found".to_string()),
            likely_cause: Some("exact_symbol_mismatch".to_string()),
            next_best_actions: vec![
                "retry_with_suggested_symbol".to_string(),
                "use_query_to_broaden_recall".to_string(),
            ],
            confidence: Some("low".to_string()),
            soll: None,
        };
    }

    if has_signal("backend_pressure")
        || has_fact(&GuidanceFact::IndexIncomplete)
        || has_fact(&GuidanceFact::VectorizationIncomplete)
    {
        let likely_cause = if has_signal("backend_pressure") {
            "runtime_pressure_reduces_reliability"
        } else if has_fact(&GuidanceFact::IndexIncomplete) {
            "graph_index_not_fully_ready"
        } else {
            "semantic_layer_not_fully_ready"
        };

        return GuidanceOutcome {
            problem_class: Some("degraded".to_string()),
            likely_cause: Some(likely_cause.to_string()),
            next_best_actions: vec![
                "treat_result_as_partial".to_string(),
                "retry_after_runtime_stabilizes".to_string(),
            ],
            confidence: Some("medium".to_string()),
            soll: None,
        };
    }

    // Deferred SOLL-gap classes remain available for shadow experiments only.
    if has_signal("intent_missing_in_soll") {
        return GuidanceOutcome {
            problem_class: Some("intent_missing_in_soll".to_string()),
            likely_cause: Some("code_evidence_without_maintained_intent".to_string()),
            next_best_actions: vec![
                "review_current_soll_context".to_string(),
                "update_requirement_or_decision_if_authorized".to_string(),
            ],
            confidence: Some("medium".to_string()),
            soll: Some(SollGuidance {
                recommended_action: "recommend_update".to_string(),
                update_kind: "requirement_or_decision".to_string(),
                reason: "missing_intent_evidence".to_string(),
                requires_authorization: Some(true),
            }),
        };
    }

    if has_signal("missing_rationale_in_soll") {
        return GuidanceOutcome {
            problem_class: Some("missing_rationale_in_soll".to_string()),
            likely_cause: Some("code_evidence_without_maintained_rationale".to_string()),
            next_best_actions: vec![
                "review_current_soll_context".to_string(),
                "update_decision_or_requirement_if_authorized".to_string(),
            ],
            confidence: Some("medium".to_string()),
            soll: Some(SollGuidance {
                recommended_action: "recommend_update".to_string(),
                update_kind: "decision_or_requirement".to_string(),
                reason: "missing_rationale_evidence".to_string(),
                requires_authorization: Some(true),
            }),
        };
    }

    GuidanceOutcome::none()
}

#[allow(dead_code)]
pub(crate) fn guidance_outcome_to_value(outcome: &GuidanceOutcome) -> Value {
    if outcome.problem_class.is_none()
        && outcome.next_best_actions.is_empty()
        && outcome.soll.is_none()
        && outcome.likely_cause.is_none()
        && outcome.confidence.is_none()
    {
        return Value::Null;
    }

    json!({
        "problem_class": outcome.problem_class,
        "likely_cause": outcome.likely_cause,
        "next_best_actions": outcome.next_best_actions,
        "confidence": outcome.confidence,
        "soll": outcome.soll,
    })
}

#[allow(dead_code)]
pub(crate) fn attach_guidance_authoritative(
    mut response: Value,
    outcome: GuidanceOutcome,
) -> Value {
    let projected = project_authoritative_phase1_guidance(outcome);
    if projected == GuidanceOutcome::none() {
        return response;
    }

    let Some(object) = response.as_object_mut() else {
        return response;
    };

    let data = object
        .entry("data".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    if !data.is_object() {
        *data = json!({ "value": data.clone() });
    }

    let next = build_guided_response(data.clone(), projected);
    *data = next;
    response
}

#[allow(dead_code)]
pub(crate) fn attach_guidance_shadow(mut response: Value, guidance_shadow: Value) -> Value {
    if guidance_shadow.is_null() {
        return response;
    }
    let Some(object) = response.as_object_mut() else {
        return response;
    };

    let data = object
        .entry("data".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    if !data.is_object() {
        *data = json!({ "value": data.clone() });
    }

    if let Some(data_object) = data.as_object_mut() {
        let shadow = data_object
            .entry("_shadow".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !shadow.is_object() {
            *shadow = Value::Object(serde_json::Map::new());
        }
        if let Some(shadow_object) = shadow.as_object_mut() {
            shadow_object.insert("guidance".to_string(), guidance_shadow);
        }
    }

    response
}

#[allow(dead_code)]
pub(crate) fn build_guided_response(mut payload: Value, outcome: GuidanceOutcome) -> Value {
    if outcome.problem_class.is_none()
        && outcome.next_best_actions.is_empty()
        && outcome.soll.is_none()
        && outcome.likely_cause.is_none()
        && outcome.confidence.is_none()
    {
        return payload;
    }

    if let Some(object) = payload.as_object_mut() {
        if let Some(problem_class) = outcome.problem_class {
            object.insert("problem_class".to_string(), Value::String(problem_class));
        }
        if let Some(likely_cause) = outcome.likely_cause {
            object.insert("likely_cause".to_string(), Value::String(likely_cause));
        }
        if !outcome.next_best_actions.is_empty() {
            object.insert(
                "next_best_actions".to_string(),
                Value::Array(
                    outcome
                        .next_best_actions
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if let Some(confidence) = outcome.confidence {
            object.insert("confidence".to_string(), Value::String(confidence));
        }
        if let Some(soll) = outcome
            .soll
            .filter(|soll| soll.requires_authorization.is_some())
        {
            object.insert(
                "soll".to_string(),
                serde_json::to_value(soll).unwrap_or(Value::Null),
            );
        }
    }

    payload
}
