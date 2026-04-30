use serde_json::{json, Value};
use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn compute_confidence(&self, route: RetrievalRoute, entry_candidates: &[EntryCandidate],
        supporting_chunks: &[Value], structural_neighbors: &[Value], relevant_soll_entities: &[Value]) -> Value {
        let mut score: f64 = 0.20;
        if !entry_candidates.is_empty() { score += 0.35; }
        if !supporting_chunks.is_empty() { score += 0.20; }
        if matches!(route, RetrievalRoute::Impact | RetrievalRoute::Wiring) && !structural_neighbors.is_empty() { score += 0.15; }
        if matches!(route, RetrievalRoute::SollHybrid) && !relevant_soll_entities.is_empty() { score += 0.10; }
        score = score.min(0.95);
        json!({ "score": score, "label": Self::confidence_label(score) })
    }

    pub(super) fn confidence_label(score: f64) -> &'static str {
        if score >= 0.8 { "high" } else if score >= 0.55 { "medium" } else { "low" }
    }

    pub(super) fn render_evidence_packet(&self, packet: &Value, route: RetrievalRoute) -> String {
        let answer_sketch = packet.get("answer_sketch").and_then(|v| v.as_str()).unwrap_or_default();
        let direct = packet.get("direct_evidence").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let neighbors = packet.get("structural_neighbors").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let confidence = packet.get("confidence").and_then(|v| v.get("label")).and_then(|v| v.as_str()).unwrap_or("low");
        let evidence_states = packet.get("evidence_states").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let governing_requirements = packet.get("governing_requirements").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let governing_decisions = packet.get("governing_decisions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let supporting_guidelines = packet.get("supporting_guidelines").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let supporting_docs = packet.get("supporting_docs").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let supporting_code_context = packet.get("supporting_code_context").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let rationale_quality = packet.get("rationale_quality").cloned().unwrap_or_else(|| json!({}));

        let mut rendered = format!("**Planner route:** `{}`\n**Evidence confidence:** `{}`\n\n### Answer sketch\n{}\n", route.as_str(), confidence, answer_sketch);

        if !evidence_states.is_empty() {
            rendered.push_str("\n### Evidence states\n");
            for row in evidence_states.iter().take(4) {
                rendered.push_str(&format!("- `{}`: {}\n", row.get("state").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("detail").and_then(|v| v.as_str()).unwrap_or("")));
            }
        }
        if !governing_requirements.is_empty() {
            rendered.push_str("\n### Governing requirements\n");
            for row in governing_requirements.iter().take(2) {
                rendered.push_str(&format!("- `{}` [{} / {}]\n", row.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("link_mode").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("evidence_provenance").and_then(|v| v.as_str()).unwrap_or("unknown")));
            }
        }
        if !governing_decisions.is_empty() {
            rendered.push_str("\n### Governing decisions\n");
            for row in governing_decisions.iter().take(2) {
                rendered.push_str(&format!("- `{}` [{} / {}]\n", row.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("link_mode").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("evidence_provenance").and_then(|v| v.as_str()).unwrap_or("unknown")));
            }
        }
        if !supporting_guidelines.is_empty() {
            rendered.push_str("\n### Supporting guidelines\n");
            for row in supporting_guidelines.iter().take(2) {
                rendered.push_str(&format!("- `{}` [{}]\n", row.get("id").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("link_mode").and_then(|v| v.as_str()).unwrap_or("unknown")));
            }
        }
        if !direct.is_empty() {
            rendered.push_str("\n### Direct evidence\n");
            for row in direct.iter().take(2) {
                rendered.push_str(&format!("- `{}` ({}) in `{}` [{}]\n", row.get("name").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("uri").and_then(|v| v.as_str()).unwrap_or(""), row.get("evidence_class").and_then(|v| v.as_str()).unwrap_or("canonical")));
            }
        }
        if !supporting_docs.is_empty() {
            rendered.push_str("\n### Supporting docs\n");
            for row in supporting_docs.iter().take(2) {
                rendered.push_str(&format!("- `{}` [{} / {}]: {}\n", row.get("uri").and_then(|v| v.as_str()).unwrap_or(""), row.get("link_mode").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("evidence_provenance").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("snippet").and_then(|v| v.as_str()).unwrap_or("")));
            }
        }
        if !supporting_code_context.is_empty() {
            rendered.push_str("\n### Supporting code context\n");
            for row in supporting_code_context.iter().take(4) {
                rendered.push_str(&format!("- `{}` [{} / {}]: {}\n", row.get("uri").or_else(|| row.get("label")).and_then(|v| v.as_str()).unwrap_or(""), row.get("link_mode").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("evidence_provenance").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("snippet").or_else(|| row.get("label")).and_then(|v| v.as_str()).unwrap_or("")));
            }
        }
        if !neighbors.is_empty() {
            rendered.push_str("\n### Structural neighbors\n");
            for row in neighbors.iter().take(2) {
                rendered.push_str(&format!("- `{}` via `{}` at distance {}\n", row.get("label").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("edge_kind").and_then(|v| v.as_str()).unwrap_or("unknown"), row.get("distance").and_then(|v| v.as_i64()).unwrap_or(0)));
            }
        }
        if rationale_quality.get("level").is_some() {
            rendered.push_str("\n### Rationale quality\n");
            rendered.push_str(&format!("- level: `{}`\n- reason: {}\n- contract: `{}`\n", rationale_quality.get("level").and_then(|v| v.as_str()).unwrap_or("unknown"), rationale_quality.get("confidence_reason").and_then(|v| v.as_str()).unwrap_or(""), rationale_quality.get("automation_contract").and_then(|v| v.as_str()).unwrap_or("unknown")));
        }
        if let Some(diag) = packet.get("retrieval_diagnostics") {
            rendered.push_str("\n### Retrieval diagnostics\n");
            rendered.push_str(&format!("- symbol candidates: {}\n- file candidates: {}\n- chunk candidates: {}\n- anchored chunks selected: {}\n- unanchored chunks selected: {}\n- multipart chunks selected: {}\n- multipart symbol groups selected: {}\n",
                diag.get("symbol_candidates_considered").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("file_candidates_considered").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("chunk_candidates_considered").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("anchored_chunks_selected").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("unanchored_chunks_selected").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("multipart_chunks_selected").and_then(|v| v.as_u64()).unwrap_or(0),
                diag.get("multipart_symbol_groups_selected").and_then(|v| v.as_u64()).unwrap_or(0)));
        }
        rendered
    }
}
