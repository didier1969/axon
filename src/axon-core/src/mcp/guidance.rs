use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SollGuidance {
    pub(crate) recommended_action: String,
    pub(crate) update_kind: String,
    pub(crate) reason: String,
    pub(crate) requires_authorization: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuidanceOutcome {
    pub(crate) problem_class: Option<String>,
    pub(crate) likely_cause: Option<String>,
    pub(crate) next_best_actions: Vec<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) soll: Option<SollGuidance>,
}

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

pub(crate) fn build_guided_response(
    mut payload: Value,
    outcome: GuidanceOutcome,
) -> Value {
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
        if let Some(soll) = outcome.soll.filter(|soll| soll.requires_authorization.is_some()) {
            object.insert(
                "soll".to_string(),
                serde_json::to_value(soll).unwrap_or(Value::Null),
            );
        }
    }

    payload
}
