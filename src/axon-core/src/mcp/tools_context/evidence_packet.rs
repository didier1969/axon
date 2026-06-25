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
}
