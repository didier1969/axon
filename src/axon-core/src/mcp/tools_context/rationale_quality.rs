//! REQ-AXO-219 — rationale-quality verdict helper extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! function on `McpServer`; behavior-preserving move, `Self::…` call sites
//! unchanged. Grades the evidence packet (strong / mixed / weak) and names the
//! proof-gap remediation (PIL-AXO-002 machine-actionable).

use super::super::McpServer;
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn build_rationale_quality(
        evidence_states: &[Value],
        governing_requirements: &[Value],
        governing_decisions: &[Value],
        supporting_guidelines: &[Value],
        question_terms: &[String],
    ) -> Value {
        let has_governing = !governing_requirements.is_empty() || !governing_decisions.is_empty();
        // REQ-AXO-901976 critère #3 — `strong` requires not just the PRESENCE of
        // governing intent but its RELEVANCE to the question. A governing entity
        // with zero overlap (term/anchor) downgrades the verdict to `mixed`.
        let has_relevant_governing = governing_requirements
            .iter()
            .chain(governing_decisions.iter())
            .any(|entity| Self::governing_overlaps_question(entity, question_terms));
        let has_missing_governing = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("missing_governing_intent")
        });
        let has_no_direct_traceability = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("no_direct_traceability")
        });
        let has_correlation_only = evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("correlation_only")
        });
        let level = if has_governing && evidence_states.is_empty() && has_relevant_governing {
            "strong"
        } else if has_missing_governing || has_no_direct_traceability || has_correlation_only {
            "weak"
        } else if has_governing || !supporting_guidelines.is_empty() {
            "mixed"
        } else {
            "weak"
        };
        let confidence_reason = if has_missing_governing {
            "governing intent is missing, so the packet should be read as non-canonical rationale"
        } else if has_no_direct_traceability {
            "supporting evidence exists, but no direct traceability was found for the current anchor"
        } else if has_governing && !has_relevant_governing {
            "governing intent is present but shares no overlap with the question, so read it as partial support, not a direct answer"
        } else if has_governing {
            "governing intent is present, but downstream support may still be partial"
        } else if has_correlation_only {
            "only correlated support artifacts were found"
        } else {
            "no governing intent was found; only local support evidence is available"
        };
        // REQ-AXO-901989 / NEX client report — a `weak` verdict caused by
        // missing governing intent or no direct traceability is a PROOF_GAP
        // (evidence not yet linked to this anchor), NOT a tool limitation. The
        // client read low why/change_safety scores as an Axon defect when in
        // fact no evidence had been attached. Surface the gap as fixable + name
        // the exact remediation tools so the verdict is self-explanatory
        // (PIL-AXO-002 machine-actionable, REQ-AXO-088 guidance-must-deliver).
        let proof_gap = has_missing_governing || has_no_direct_traceability;
        let remediation = if proof_gap {
            json!("proof_gap, not a tool limitation: link governing intent with soll_manager(action=link, relation_type=SOLVES|REFINES) and/or attach evidence with soll_attach_evidence to this anchor — confidence rises mechanically once intent/evidence is wired.")
        } else {
            Value::Null
        };
        json!({
            "level": level,
            "confidence_reason": confidence_reason,
            "proof_gap": proof_gap,
            "remediation": remediation,
            "automation_contract": "informational_only"
        })
    }
}
