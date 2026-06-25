//! REQ-AXO-219 — direct-SOLL-traceability checks extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` / `Self::…` call sites unchanged. RAM-first
//! (SollSnapshot) with PG fallback, per PIL-AXO-9002.

use super::super::McpServer;
use super::retrieval_model::EntryCandidate;
use serde_json::{json, Value};

impl McpServer {
    pub(super) fn has_direct_soll_traceability(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
    ) -> bool {
        // REQ-AXO-902039 element 2 — RAM-first via SollSnapshot (PIL-AXO-9002).
        // The PG form is `count(*) FROM soll.Traceability JOIN soll.Node`; the
        // RAM equivalent scans the per-project snapshot's traceability rows for a
        // Symbol/File artifact whose governing node is present in this project's
        // snapshot (the JOIN + project_filter are implicit: the snapshot is
        // scoped to one project). Project unscoped or snapshot cold ⇒ PG fallback.
        if let Some(proj) = project {
            if let Ok(snap) = self.soll_cache().snapshot(proj) {
                crate::soll_snapshot::record_fusion_read(true);
                return Self::snapshot_has_direct_traceability(&snap, entry_candidates);
            }
        }
        crate::soll_snapshot::record_fusion_read(false);
        let symbol_names = entry_candidates
            .iter()
            .filter(|candidate| candidate.kind != "file")
            .map(|candidate| {
                format!(
                    "'{}'",
                    Self::escape_sql(&candidate.name.to_ascii_lowercase())
                )
            })
            .collect::<Vec<_>>();
        let file_paths = entry_candidates
            .iter()
            .filter(|candidate| !candidate.uri.is_empty())
            .map(|candidate| format!("'{}'", Self::escape_sql(&candidate.uri)))
            .collect::<Vec<_>>();
        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(n.project_code) IN ({})",
                    Self::project_scope_variants(Some(value))
                        .iter()
                        .map(|variant| format!(
                            "'{}'",
                            Self::escape_sql(&variant.to_ascii_lowercase())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        let mut predicates = Vec::new();
        if !symbol_names.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'Symbol' AND lower(t.artifact_ref) IN ({}))",
                symbol_names.join(",")
            ));
        }
        if !file_paths.is_empty() {
            predicates.push(format!(
                "(t.artifact_type = 'File' AND t.artifact_ref IN ({}))",
                file_paths.join(",")
            ));
        }
        if predicates.is_empty() {
            return false;
        }
        let query = format!(
            "SELECT count(*) FROM soll.Traceability t \
             JOIN soll.Node n ON n.id = t.soll_entity_id \
             WHERE ({predicates}){project_filter}",
            predicates = predicates.join(" OR "),
        );
        self.graph_store.query_count(&query).unwrap_or(0) > 0
    }

    /// REQ-AXO-902039 element 2 — RAM form of `has_direct_soll_traceability`.
    /// Any Symbol/File traceability row whose governing node is present in this
    /// project's snapshot (the snapshot scopes the JOIN + project_filter).
    pub(super) fn snapshot_has_direct_traceability(
        snap: &crate::soll_snapshot::SollSnapshot,
        entry_candidates: &[EntryCandidate],
    ) -> bool {
        use std::collections::HashSet;
        let symbol_names: HashSet<String> = entry_candidates
            .iter()
            .filter(|c| c.kind != "file")
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
        let file_paths: HashSet<&str> = entry_candidates
            .iter()
            .filter(|c| !c.uri.is_empty())
            .map(|c| c.uri.as_str())
            .collect();
        if symbol_names.is_empty() && file_paths.is_empty() {
            return false;
        }
        snap.traceability.iter().any(|t| {
            let matches = (t.artifact_type == "Symbol"
                && symbol_names.contains(&t.artifact_ref.to_ascii_lowercase()))
                || (t.artifact_type == "File" && file_paths.contains(t.artifact_ref.as_str()));
            matches && snap.nodes.contains_key(&t.soll_entity_id)
        })
    }

    pub(super) fn collect_soll_entities(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
        terms: &[String],
        top_k: usize,
    ) -> Vec<Value> {
        // REQ-AXO-902039 element 2 — RAM-first fusion (PIL-AXO-9002). When the
        // project is scoped and the SOLL snapshot is warm, serve the
        // symbol→governing-intent traceability + concept-bridge reads from RAM.
        // The lexical title+description fallback stays on PG: `description` is not
        // mirrored in SollSnapshot, so a RAM-empty result falls through to the
        // explicit PG path below (annotated fallback the REQ permits).
        if let Some(proj) = project {
            if let Ok(snap) = self.soll_cache().snapshot(proj) {
                let mut selected =
                    Self::collect_soll_traceability_ram(&snap, entry_candidates, top_k);
                Self::expand_concept_governing_entities_ram(&snap, &mut selected, top_k);
                if !selected.is_empty() {
                    crate::soll_snapshot::record_fusion_read(true);
                    return selected;
                }
                // RAM traceability empty → PG lexical fallback (description join).
            }
        }
        crate::soll_snapshot::record_fusion_read(false);
        self.collect_soll_entities_pg(entry_candidates, project, terms, top_k)
    }

    /// REQ-AXO-902039 element 2 — RAM reimplementation of the traceability branch
    /// of `collect_soll_entities`. Faithful to the PG query (Symbol→100 /
    /// File→95 ranking; ORDER score DESC, type DESC, id ASC; LIMIT min(top_k,2))
    /// but deterministic: the PG `LEFT JOIN soll.Edge` multiplied rows per
    /// outgoing edge with a score independent of the edge, so here each governing
    /// node yields one row, preferring a `SOLVES` relation_type when present.
    pub(super) fn collect_soll_traceability_ram(
        snap: &crate::soll_snapshot::SollSnapshot,
        entry_candidates: &[EntryCandidate],
        top_k: usize,
    ) -> Vec<Value> {
        use std::collections::HashSet;
        let symbol_names: HashSet<String> = entry_candidates
            .iter()
            .filter(|c| c.kind != "file")
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
        let file_paths: HashSet<&str> = entry_candidates
            .iter()
            .filter(|c| !c.uri.is_empty())
            .map(|c| c.uri.as_str())
            .collect();
        if symbol_names.is_empty() && file_paths.is_empty() {
            return Vec::new();
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut scored: Vec<(i64, String, String, Value)> = Vec::new();
        for t in &snap.traceability {
            let (artifact_type, score, reason) = if t.artifact_type == "Symbol"
                && symbol_names.contains(&t.artifact_ref.to_ascii_lowercase())
            {
                ("Symbol", 100i64, "direct_symbol_traceability")
            } else if t.artifact_type == "File" && file_paths.contains(t.artifact_ref.as_str()) {
                ("File", 95i64, "direct_file_traceability")
            } else {
                continue;
            };
            let Some(node) = snap.nodes.get(&t.soll_entity_id) else {
                continue; // mirrors PG `JOIN soll.Node` (+ implicit project scope)
            };
            if !seen.insert(node.id.clone()) {
                continue;
            }
            let mut relation_type = String::new();
            for (_tgt, rel) in snap.outgoing_edges(&node.id) {
                if rel == "SOLVES" {
                    relation_type = rel.to_string();
                    break;
                }
                if relation_type.is_empty() {
                    relation_type = rel.to_string();
                }
            }
            scored.push((
                score,
                node.entity_type.clone(),
                node.id.clone(),
                json!({
                    "id": node.id.clone(),
                    "type": node.entity_type.clone(),
                    "title": node.title.clone(),
                    "relation_type": relation_type,
                    "source_symbol": t.artifact_ref.clone(),
                    "artifact_type": artifact_type,
                    "ranking_reasons": [reason],
                    "ranking_score": score,
                    "evidence_class": "soll_traceability",
                }),
            ));
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        scored
            .into_iter()
            .take(top_k.min(2))
            .map(|(_, _, _, v)| v)
            .collect()
    }

    /// REQ-AXO-902039 element 2 — RAM reimplementation of
    /// `expand_concept_governing_entities`: concept→Requirement (score 88) and
    /// concept→Requirement→Decision (score 84) bridges traversed over the SOLL
    /// petgraph snapshot instead of PG `soll.Edge` joins.
    pub(super) fn expand_concept_governing_entities_ram(
        snap: &crate::soll_snapshot::SollSnapshot,
        selected: &mut Vec<Value>,
        top_k: usize,
    ) {
        use std::collections::HashSet;
        let concept_ids: Vec<String> = selected
            .iter()
            .filter(|row| row.get("type").and_then(|v| v.as_str()) == Some("Concept"))
            .filter_map(|row| row.get("id").and_then(|v| v.as_str()))
            .map(str::to_string)
            .collect();
        if concept_ids.is_empty() {
            return;
        }
        let mut seen_ids: HashSet<String> = selected
            .iter()
            .filter_map(|row| row.get("id").and_then(|v| v.as_str()))
            .map(str::to_string)
            .collect();
        let limit = top_k.min(4);

        let mut req_rows: Vec<(String, Value)> = Vec::new();
        for concept_id in &concept_ids {
            for (tgt, rel) in snap.outgoing_edges(concept_id) {
                let Some(node) = snap.nodes.get(tgt) else { continue };
                if node.entity_type != "Requirement" {
                    continue;
                }
                req_rows.push((
                    node.id.clone(),
                    json!({
                        "id": node.id.clone(),
                        "type": "Requirement",
                        "title": node.title.clone(),
                        "relation_type": rel.to_string(),
                        "source_symbol": concept_id.clone(),
                        "artifact_type": "",
                        "ranking_reasons": ["concept_requirement_bridge"],
                        "ranking_score": 88,
                        "evidence_class": "soll_traceability",
                    }),
                ));
            }
        }
        let mut dec_rows: Vec<(String, Value)> = Vec::new();
        for concept_id in &concept_ids {
            for (req_tgt, _rel) in snap.outgoing_edges(concept_id) {
                let Some(req_node) = snap.nodes.get(req_tgt) else { continue };
                if req_node.entity_type != "Requirement" {
                    continue;
                }
                for (dec_src, de_rel) in snap.incoming_edges(&req_node.id) {
                    let Some(dec_node) = snap.nodes.get(dec_src) else { continue };
                    if dec_node.entity_type != "Decision" {
                        continue;
                    }
                    dec_rows.push((
                        dec_node.id.clone(),
                        json!({
                            "id": dec_node.id.clone(),
                            "type": "Decision",
                            "title": dec_node.title.clone(),
                            "relation_type": de_rel.to_string(),
                            "source_symbol": concept_id.clone(),
                            "artifact_type": "",
                            "ranking_reasons": ["concept_decision_bridge"],
                            "ranking_score": 84,
                            "evidence_class": "soll_traceability",
                        }),
                    ));
                }
            }
        }
        req_rows.sort_by(|a, b| a.0.cmp(&b.0));
        dec_rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, row) in req_rows
            .into_iter()
            .take(limit)
            .chain(dec_rows.into_iter().take(limit))
        {
            if seen_ids.insert(id) {
                selected.push(row);
            }
        }
    }
}
