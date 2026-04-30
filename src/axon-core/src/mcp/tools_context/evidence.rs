use serde_json::{json, Value};

use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn build_direct_evidence(&self, entry_candidates: &[EntryCandidate]) -> Vec<Value> {
        entry_candidates.iter().map(|candidate| {
            let evidence_class = if candidate.kind == "file" { "canonical_file" }
                else if candidate.kind == "repo_literal" { "repo_literal_file" }
                else { "canonical_symbol" };
            json!({
                "symbol_id": candidate.id, "name": candidate.name, "kind": candidate.kind,
                "project_code": candidate.project_code, "uri": candidate.uri,
                "evidence_class": evidence_class, "score": candidate.score,
                "ranking_reasons": candidate.reasons,
            })
        }).collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_answer_sketch(
        &self, question: &str, route: RetrievalRoute, entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value], structural_neighbors: &[Value],
        governing_requirements: &[Value], governing_decisions: &[Value],
        supporting_guidelines: &[Value], evidence_states: &[Value],
    ) -> String {
        let mut lines = Vec::new();
        lines.push(format!("Route `{}` selected for `{}`.", route.as_str(), question));
        if let Some(anchor) = entry_candidates.first() {
            lines.push(format!("Primary entrypoint: `{}` ({}) in `{}`.", anchor.name, anchor.kind, anchor.uri));
        }
        if !structural_neighbors.is_empty() {
            let labels = structural_neighbors.iter()
                .filter_map(|row| row.get("label").and_then(|v| v.as_str())).take(2)
                .collect::<Vec<_>>().join(", ");
            lines.push(format!("Bounded structural expansion found: {}.", labels));
        }
        if !supporting_chunks.is_empty() {
            lines.push(format!("{} supporting chunk(s) added for grounded detail.", supporting_chunks.len()));
        }
        if !governing_requirements.is_empty() {
            let ids = governing_requirements.iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_str())).take(2)
                .collect::<Vec<_>>().join(", ");
            let label = if governing_requirements.iter().all(|row| row.get("link_mode").and_then(|v| v.as_str()) == Some("direct"))
                { "Direct governing requirement(s)" } else { "Governing requirement(s) inferred from supporting intent" };
            lines.push(format!("{label}: {}.", ids));
        }
        if !governing_decisions.is_empty() {
            let ids = governing_decisions.iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_str())).take(2)
                .collect::<Vec<_>>().join(", ");
            let label = if governing_decisions.iter().all(|row| row.get("link_mode").and_then(|v| v.as_str()) == Some("direct"))
                { "Direct governing decision(s)" } else { "Governing decision(s) inferred from supporting intent" };
            lines.push(format!("{label}: {}.", ids));
        }
        if !supporting_guidelines.is_empty() {
            let ids = supporting_guidelines.iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_str())).take(2)
                .collect::<Vec<_>>().join(", ");
            lines.push(format!("Supporting guideline(s): {}.", ids));
        }
        if evidence_states.iter().any(|row| row.get("state").and_then(|v| v.as_str()) == Some("missing_governing_intent")) {
            lines.push("No direct governing intent was found for this symbol.".to_string());
        }
        if evidence_states.iter().any(|row| row.get("state").and_then(|v| v.as_str()) == Some("support_only")) {
            lines.push("Current rationale is supported by local evidence only and should not be treated as canonical intent.".to_string());
        }
        lines.join(" ")
    }

    pub(super) fn build_why_these_items(
        &self, route: RetrievalRoute, entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value], structural_neighbors: &[Value], relevant_soll_entities: &[Value],
    ) -> Vec<Value> {
        let mut items = Vec::new();
        if !entry_candidates.is_empty() {
            items.push(json!({"reason": "entrypoints_selected", "detail": format!("{} entrypoint(s) selected for route {}", entry_candidates.len(), route.as_str())}));
        }
        if !supporting_chunks.is_empty() {
            items.push(json!({"reason": "grounding_chunks_selected", "detail": format!("{} supporting chunk(s) chosen under diversity and budget constraints", supporting_chunks.len())}));
        }
        if !structural_neighbors.is_empty() {
            items.push(json!({"reason": "bounded_graph_expansion", "detail": format!("{} structural neighbor(s) retained from derived graph projection", structural_neighbors.len())}));
        }
        if !relevant_soll_entities.is_empty() {
            items.push(json!({"reason": "soll_join_materially_helpful", "detail": format!("{} SOLL item(s) joined because the route requested rationale/intent", relevant_soll_entities.len())}));
        }
        items
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_missing_evidence(
        &self, route: RetrievalRoute, entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value], relevant_soll_entities: &[Value],
        rationale_requested: bool, has_direct_traceability: bool,
        semantic_search_used: bool, degraded_reason: Option<&str>,
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
        if !entry_candidates.is_empty() && !has_direct_traceability
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

    pub(super) fn classify_governing_entities(entities: &[Value], expected_type: &str, provenance: &str) -> Vec<Value> {
        entities.iter()
            .filter(|row| row.get("type").and_then(|v| v.as_str()) == Some(expected_type))
            .map(|row| {
                let evidence_class = row.get("evidence_class").and_then(|v| v.as_str()).unwrap_or("unknown");
                let ranking_reason = row.get("ranking_reasons").and_then(|v| v.as_array())
                    .and_then(|items| items.first()).and_then(|v| v.as_str()).unwrap_or("unknown");
                let link_mode = if evidence_class == "soll_traceability"
                    && (ranking_reason.starts_with("direct_") || ranking_reason.starts_with("requirement_"))
                    { "direct" }
                    else if evidence_class == "soll_traceability" || evidence_class == "soll_concept_bridge" { "inferred" }
                    else { "weak_correlation" };
                let authority_class = if link_mode == "weak_correlation" { "correlated" }
                    else if expected_type == "Guideline" { "supporting" } else { "governing" };
                let mut enriched = row.clone();
                if let Some(object) = enriched.as_object_mut() {
                    object.insert("authority_class".to_string(), Value::String(authority_class.to_string()));
                    object.insert("evidence_provenance".to_string(), Value::String(provenance.to_string()));
                    object.insert("link_mode".to_string(), Value::String(link_mode.to_string()));
                    object.insert("inclusion_reason".to_string(), Value::String(ranking_reason.to_string()));
                }
                enriched
            }).collect()
    }

    pub(super) fn evidence_provenance_for_uri(uri: &str) -> &'static str {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("benchmark") { "benchmark" }
        else if matches!(Self::uri_penalty_reason(uri), Some("test_file_penalty")) { "test" }
        else if lower.contains("/scripts/") || lower.starts_with("scripts/") { "script" }
        else if matches!(Self::uri_penalty_reason(uri), Some("docs_file_penalty")) { "doc" }
        else { "code_chunk" }
    }

    pub(super) fn classify_direct_code_evidence(direct_evidence: &[Value]) -> Vec<Value> {
        direct_evidence.iter().map(|row| {
            let mut enriched = row.clone();
            let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
            let uri = row.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            let evidence_class = row.get("evidence_class").and_then(|v| v.as_str()).unwrap_or("unknown");
            let uri_provenance = Self::evidence_provenance_for_uri(uri);
            let provenance = if evidence_class == "repo_literal_file" { uri_provenance }
                else if matches!(uri_provenance, "benchmark" | "test" | "script" | "doc") { uri_provenance }
                else if kind == "file" { "code_file" } else { "code_symbol" };
            let authority_class = match provenance { "benchmark" | "test" | "script" | "doc" => "correlated", _ => "supporting" };
            let link_mode = if evidence_class == "repo_literal_file" { "weak_correlation" } else { "direct" };
            if let Some(object) = enriched.as_object_mut() {
                object.insert("authority_class".to_string(), Value::String(authority_class.to_string()));
                object.insert("evidence_provenance".to_string(), Value::String(provenance.to_string()));
                object.insert("link_mode".to_string(), Value::String(link_mode.to_string()));
                object.insert("inclusion_reason".to_string(), Value::String(
                    row.get("evidence_class").and_then(|v| v.as_str()).unwrap_or("direct_evidence").to_string()));
            }
            enriched
        }).collect()
    }

    pub(super) fn classify_supporting_chunks_by_provenance(chunks: &[Value], provenance: &str, authority_class: &str) -> Vec<Value> {
        chunks.iter().filter_map(|row| {
            let uri = row.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            let row_provenance = Self::evidence_provenance_for_uri(uri);
            (row_provenance == provenance).then(|| {
                let mut enriched = row.clone();
                let link_mode = match row.get("anchored_to_entry").and_then(|v| v.as_bool()) { Some(true) => "direct", _ => "inferred" };
                if let Some(object) = enriched.as_object_mut() {
                    object.insert("authority_class".to_string(), Value::String(authority_class.to_string()));
                    object.insert("evidence_provenance".to_string(), Value::String(provenance.to_string()));
                    object.insert("link_mode".to_string(), Value::String(link_mode.to_string()));
                    object.insert("inclusion_reason".to_string(), Value::String(
                        row.get("match_reason").and_then(|v| v.as_str()).unwrap_or("supporting_chunk").to_string()));
                }
                enriched
            })
        }).collect()
    }

    pub(super) fn classify_supporting_code_context(chunks: &[Value], neighbors: &[Value]) -> Vec<Value> {
        let mut items = chunks.iter().filter_map(|row| {
            let uri = row.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            let provenance = Self::evidence_provenance_for_uri(uri);
            (provenance != "doc").then(|| {
                let mut enriched = row.clone();
                let link_mode = if matches!(provenance, "benchmark" | "test" | "script") { "weak_correlation" }
                    else if row.get("anchored_to_entry").and_then(|v| v.as_bool()) == Some(true) { "direct" }
                    else { "inferred" };
                let authority_class = if link_mode == "weak_correlation" { "correlated" } else { "supporting" };
                if let Some(object) = enriched.as_object_mut() {
                    object.insert("authority_class".to_string(), Value::String(authority_class.to_string()));
                    object.insert("evidence_provenance".to_string(), Value::String(provenance.to_string()));
                    object.insert("link_mode".to_string(), Value::String(link_mode.to_string()));
                    object.insert("inclusion_reason".to_string(), Value::String(
                        row.get("match_reason").and_then(|v| v.as_str()).unwrap_or("supporting_chunk").to_string()));
                }
                enriched
            })
        }).collect::<Vec<_>>();
        for neighbor in neighbors {
            let mut enriched = neighbor.clone();
            if let Some(object) = enriched.as_object_mut() {
                object.insert("authority_class".to_string(), Value::String("supporting".to_string()));
                object.insert("evidence_provenance".to_string(), Value::String("code_chunk".to_string()));
                object.insert("link_mode".to_string(), Value::String("inferred".to_string()));
                object.insert("inclusion_reason".to_string(), Value::String(
                    neighbor.get("edge_kind").and_then(|v| v.as_str()).unwrap_or("structural_neighbor").to_string()));
            }
            items.push(enriched);
        }
        items
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_evidence_states(
        route: RetrievalRoute, rationale_requested: bool, has_direct_traceability: bool,
        degraded_reason: Option<&str>, governing_requirements: &[Value], governing_decisions: &[Value],
        supporting_guidelines: &[Value], direct_code_evidence: &[Value],
        supporting_docs: &[Value], supporting_code_context: &[Value],
    ) -> Vec<Value> {
        let mut states = Vec::new();
        let has_governing = !governing_requirements.is_empty() || !governing_decisions.is_empty();
        let has_support = !supporting_guidelines.is_empty() || !direct_code_evidence.is_empty()
            || !supporting_docs.is_empty() || !supporting_code_context.is_empty();
        if (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested) && !has_governing {
            states.push(json!({"state": "missing_governing_intent", "severity": "medium",
                "detail": "No direct governing requirement or decision was found for this rationale request"}));
        }
        if !has_direct_traceability && (matches!(route, RetrievalRoute::SollHybrid) || rationale_requested) {
            states.push(json!({"state": "no_direct_traceability", "severity": "medium",
                "detail": "No direct Symbol/File traceability was found for the current anchor"}));
        }
        if degraded_reason.is_some() {
            states.push(json!({"state": "retrieval_degraded", "severity": "low",
                "detail": degraded_reason.map(|v| format!("Retrieval ran under degraded conditions: {v}"))
                    .unwrap_or_else(|| "Retrieval ran under degraded conditions".to_string())}));
        }
        if !has_governing && has_support {
            let only_correlated = direct_code_evidence.iter().chain(supporting_docs.iter())
                .chain(supporting_code_context.iter()).chain(supporting_guidelines.iter())
                .all(|row| row.get("authority_class").and_then(|v| v.as_str()) == Some("correlated"));
            states.push(json!({
                "state": if only_correlated { "correlation_only" } else { "support_only" },
                "severity": "medium",
                "detail": if only_correlated { "Only correlated support artifacts were available for this rationale packet" }
                    else { "Only supporting local evidence was available for this rationale packet" }
            }));
        }
        states
    }

    pub(super) fn build_rationale_quality(
        evidence_states: &[Value], governing_requirements: &[Value],
        governing_decisions: &[Value], supporting_guidelines: &[Value],
    ) -> Value {
        let has_governing = !governing_requirements.is_empty() || !governing_decisions.is_empty();
        let has_missing_governing = evidence_states.iter().any(|row| row.get("state").and_then(|v| v.as_str()) == Some("missing_governing_intent"));
        let has_no_direct_traceability = evidence_states.iter().any(|row| row.get("state").and_then(|v| v.as_str()) == Some("no_direct_traceability"));
        let has_correlation_only = evidence_states.iter().any(|row| row.get("state").and_then(|v| v.as_str()) == Some("correlation_only"));
        let level = if has_governing && evidence_states.is_empty() { "strong" }
            else if has_missing_governing || has_no_direct_traceability || has_correlation_only { "weak" }
            else if has_governing || !supporting_guidelines.is_empty() { "mixed" }
            else { "weak" };
        let confidence_reason = if has_missing_governing { "governing intent is missing, so the packet should be read as non-canonical rationale" }
            else if has_no_direct_traceability { "supporting evidence exists, but no direct traceability was found for the current anchor" }
            else if has_governing { "governing intent is present, but downstream support may still be partial" }
            else if has_correlation_only { "only correlated support artifacts were found" }
            else { "no governing intent was found; only local support evidence is available" };
        json!({"level": level, "confidence_reason": confidence_reason, "automation_contract": "informational_only"})
    }
}
