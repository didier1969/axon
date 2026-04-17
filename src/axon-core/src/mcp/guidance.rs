use serde::{Deserialize, Serialize};
use serde_json::Value;

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
