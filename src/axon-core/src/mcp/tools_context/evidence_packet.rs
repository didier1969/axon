//! REQ-AXO-219 — evidence-packet builder methods extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` call sites unchanged. They assemble the
//! direct-evidence / why-these / missing-evidence sections of the
//! `retrieve_context` packet.

use super::super::McpServer;
use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn build_direct_evidence(&self, entry_candidates: &[EntryCandidate]) -> Vec<Value> {
        entry_candidates
            .iter()
            .map(|candidate| {
                let evidence_class = if candidate.kind == "file" {
                    "canonical_file"
                } else if candidate.kind == "repo_literal" {
                    "repo_literal_file"
                } else {
                    "canonical_symbol"
                };
                json!({
                    "symbol_id": candidate.id,
                    "name": candidate.name,
                    "kind": candidate.kind,
                    "project_code": candidate.project_code,
                    "uri": candidate.uri,
                    "evidence_class": evidence_class,
                    "score": candidate.score,
                    "ranking_reasons": candidate.reasons,
                })
            })
            .collect()
    }

    pub(super) fn build_why_these_items(
        &self,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        relevant_soll_entities: &[Value],
    ) -> Vec<Value> {
        let mut items = Vec::new();
        if !entry_candidates.is_empty() {
            items.push(json!({
                "reason": "entrypoints_selected",
                "detail": format!("{} entrypoint(s) selected for route {}", entry_candidates.len(), route.as_str())
            }));
        }
        if !supporting_chunks.is_empty() {
            items.push(json!({
                "reason": "grounding_chunks_selected",
                "detail": format!("{} supporting chunk(s) chosen under diversity and budget constraints", supporting_chunks.len())
            }));
        }
        if !structural_neighbors.is_empty() {
            items.push(json!({
                "reason": "bounded_graph_expansion",
                "detail": format!("{} structural neighbor(s) retained from derived graph projection", structural_neighbors.len())
            }));
        }
        if !relevant_soll_entities.is_empty() {
            items.push(json!({
                "reason": "soll_join_materially_helpful",
                "detail": format!("{} SOLL item(s) joined because the route requested rationale/intent", relevant_soll_entities.len())
            }));
        }
        items
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_missing_evidence(
        &self,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        relevant_soll_entities: &[Value],
        rationale_requested: bool,
        has_direct_traceability: bool,
        semantic_search_used: bool,
        degraded_reason: Option<&str>,
    ) -> Vec<Value> {
        let mut missing = Vec::new();
        if entry_candidates.is_empty() {
            missing.push(json!({"type": "entrypoint", "detail": "No strong symbol or file entrypoint was found"}));
        } else if supporting_chunks.is_empty() {
            missing.push(json!({"type": "supporting_chunks", "detail": "An anchor was found but no anchored chunk-level grounding evidence was retained"}));
        }
        if !semantic_search_used {
            if let Some(reason) = degraded_reason {
                missing.push(json!({"type": "semantic_search", "detail": format!("Semantic chunk search was skipped or unavailable: {}", reason)}));
            }
        }
        if !entry_candidates.is_empty()
            && !has_direct_traceability
            && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested)
        {
            missing.push(json!({"type": "anchor_found_but_no_traceability", "detail": "A structural anchor was found but no direct Symbol/File traceability matched it"}));
        }
        if matches!(route, RetrievalRoute::SollHybrid) && relevant_soll_entities.is_empty() {
            missing.push(json!({"type": "soll_intent", "detail": "SOLL rationale was requested but no direct traceability or intentional evidence was found"}));
        } else if rationale_requested && relevant_soll_entities.is_empty() {
            missing.push(json!({"type": "rationale_requested_but_no_intent_evidence", "detail": "The question requested rationale, but no intentional evidence was available after anchored retrieval"}));
        }
        missing
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_answer_sketch(
        &self,
        question: &str,
        route: RetrievalRoute,
        entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value],
        structural_neighbors: &[Value],
        governing_requirements: &[Value],
        governing_decisions: &[Value],
        supporting_guidelines: &[Value],
        evidence_states: &[Value],
    ) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "Route `{}` selected for `{}`.",
            route.as_str(),
            question
        ));
        if let Some(anchor) = entry_candidates.first() {
            lines.push(format!(
                "Primary entrypoint: `{}` ({}) in `{}`.",
                anchor.name, anchor.kind, anchor.uri
            ));
        }
        if !structural_neighbors.is_empty() {
            let labels = structural_neighbors
                .iter()
                .filter_map(|row| row.get("label").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Bounded structural expansion found: {}.", labels));
        }
        if !supporting_chunks.is_empty() {
            lines.push(format!(
                "{} supporting chunk(s) added for grounded detail.",
                supporting_chunks.len()
            ));
        }
        if !governing_requirements.is_empty() {
            let ids = governing_requirements
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            let label = if governing_requirements
                .iter()
                .all(|row| row.get("link_mode").and_then(|value| value.as_str()) == Some("direct"))
            {
                "Direct governing requirement(s)"
            } else {
                "Governing requirement(s) inferred from supporting intent"
            };
            lines.push(format!("{label}: {}.", ids));
        }
        if !governing_decisions.is_empty() {
            let ids = governing_decisions
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            let label = if governing_decisions
                .iter()
                .all(|row| row.get("link_mode").and_then(|value| value.as_str()) == Some("direct"))
            {
                "Direct governing decision(s)"
            } else {
                "Governing decision(s) inferred from supporting intent"
            };
            lines.push(format!("{label}: {}.", ids));
        }
        if !supporting_guidelines.is_empty() {
            let ids = supporting_guidelines
                .iter()
                .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
                .take(2)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("Supporting guideline(s): {}.", ids));
        }
        if evidence_states.iter().any(|row| {
            row.get("state").and_then(|value| value.as_str()) == Some("missing_governing_intent")
        }) {
            lines.push("No direct governing intent was found for this symbol.".to_string());
        }
        if evidence_states
            .iter()
            .any(|row| row.get("state").and_then(|value| value.as_str()) == Some("support_only"))
        {
            lines.push("Current rationale is supported by local evidence only and should not be treated as canonical intent.".to_string());
        }
        lines.join(" ")
    }
}
