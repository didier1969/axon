//! REQ-AXO-219 — PG SOLL-entity collection extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Method on `McpServer`;
//! behavior-preserving move, `self.collect_soll_entities_pg` call site
//! unchanged. PG traceability + lexical-fallback lane (the cold/unscoped path of
//! the RAM-first `collect_soll_entities` dispatcher).

use super::super::McpServer;
use super::retrieval_model::EntryCandidate;
use serde_json::{json, Value};
use std::collections::HashSet;

impl McpServer {
    /// REQ-AXO-902524 — canonical SOLL ids written in the question are not
    /// search terms: they are explicit intent anchors. Keep this extractor
    /// deliberately strict so ordinary hyphenated prose cannot become an id.
    pub(super) fn explicit_soll_ids(question: &str) -> Vec<String> {
        let Ok(pattern) = regex::Regex::new(r"(?i)\b[A-Z]{3}-[A-Z0-9]{3}-[0-9]+\b") else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        pattern
            .find_iter(question)
            .map(|hit| hit.as_str().to_ascii_uppercase())
            .filter(|id| seen.insert(id.clone()))
            .collect()
    }

    /// Resolve explicit anchors exactly and tenant-safely, together with the
    /// evidence rows actually attached to them. Missing/cross-tenant ids stay
    /// missing; the caller exposes that fact instead of substituting a lexical
    /// or ANN correlation.
    pub(super) fn collect_explicit_soll_entities(
        &self,
        ids: &[String],
        project: Option<&str>,
    ) -> Vec<Value> {
        if ids.is_empty() {
            return Vec::new();
        }
        let id_list = ids
            .iter()
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let project_filter = project
            .map(|value| {
                format!(
                    " AND lower(project_code) IN ({})",
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
        let node_query = format!(
            "SELECT id, type, COALESCE(title, ''), COALESCE(description, ''), status \
             FROM soll.Node WHERE id IN ({id_list}){project_filter}"
        );
        let raw = self
            .graph_store
            .query_json(&node_query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let resolved_id_list = rows
            .iter()
            .filter_map(|row| row.first().and_then(Value::as_str))
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(", ");
        if resolved_id_list.is_empty() {
            return Vec::new();
        }

        let evidence_query = format!(
            "SELECT soll_entity_id, artifact_type, artifact_ref, \
                    confidence, COALESCE(artifact_status, 'unknown') \
             FROM soll.Traceability WHERE soll_entity_id IN ({resolved_id_list}) \
             ORDER BY soll_entity_id, artifact_type, artifact_ref"
        );
        let evidence_raw = self
            .graph_store
            .query_json(&evidence_query)
            .unwrap_or_else(|_| "[]".to_string());
        let evidence_rows: Vec<Vec<Value>> =
            serde_json::from_str(&evidence_raw).unwrap_or_default();

        ids.iter()
            .filter_map(|requested_id| {
                let row = rows.iter().find(|row| {
                    row.first().and_then(Value::as_str) == Some(requested_id.as_str())
                })?;
                let attached_evidence = evidence_rows
                    .iter()
                    .filter(|evidence| {
                        evidence.first().and_then(Value::as_str) == Some(requested_id.as_str())
                    })
                    .map(|evidence| {
                        json!({
                            "artifact_type": evidence.get(1).cloned().unwrap_or(Value::Null),
                            "artifact_ref": evidence.get(2).cloned().unwrap_or(Value::Null),
                            "confidence": evidence.get(3).cloned().unwrap_or(Value::Null),
                            "artifact_status": evidence.get(4).cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect::<Vec<_>>();
                Some(json!({
                    "id": requested_id,
                    "type": row.get(1).cloned().unwrap_or(Value::Null),
                    "title": row.get(2).cloned().unwrap_or(Value::Null),
                    "description": row.get(3).cloned().unwrap_or(Value::Null),
                    "status": row.get(4).cloned().unwrap_or(Value::Null),
                    "relation_type": "",
                    "source_symbol": "",
                    "artifact_type": "SOLL-ID",
                    "ranking_reasons": ["explicit_soll_id"],
                    "ranking_score": 1_000,
                    "evidence_class": "soll_explicit_anchor",
                    "attached_evidence": attached_evidence,
                }))
            })
            .collect()
    }

    pub(super) fn collect_soll_entities_pg(
        &self,
        entry_candidates: &[EntryCandidate],
        project: Option<&str>,
        terms: &[String],
        top_k: usize,
    ) -> Vec<Value> {
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
        if symbol_names.is_empty() && file_paths.is_empty() {
            return Vec::new();
        }

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
        let query = format!(
            "SELECT n.id, n.type, COALESCE(n.title, ''), COALESCE(e.relation_type, ''), \
                    COALESCE(t.artifact_ref, ''), t.artifact_type, \
                    CASE \
                        WHEN t.artifact_type = 'Symbol' THEN 'direct_symbol_traceability' \
                        WHEN t.artifact_type = 'File' THEN 'direct_file_traceability' \
                        WHEN e.relation_type = 'SOLVES' THEN 'requirement_support' \
                        ELSE 'decision_proximity' \
                    END AS ranking_reason, \
                    CASE \
                        WHEN t.artifact_type = 'Symbol' THEN 100 \
                        WHEN t.artifact_type = 'File' THEN 95 \
                        WHEN n.type = 'Decision' THEN 80 \
                        WHEN n.type = 'Requirement' THEN 70 \
                        ELSE 50 \
                    END AS ranking_score \
             FROM soll.Traceability t \
             JOIN soll.Node n ON n.id = t.soll_entity_id \
             LEFT JOIN soll.Edge e ON e.source_id = n.id \
             WHERE ({predicates}){project_filter} \
             ORDER BY ranking_score DESC, n.type DESC, n.id ASC \
             LIMIT {limit}",
            predicates = predicates.join(" OR "),
            limit = top_k.min(2),
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut selected = rows
            .into_iter()
            .filter_map(|row| {
                Some(json!({
                    "id": row.first()?.as_str()?.to_string(),
                    "type": row.get(1)?.as_str()?.to_string(),
                    "title": row.get(2)?.as_str().unwrap_or_default().to_string(),
                    "relation_type": row.get(3)?.as_str().unwrap_or_default().to_string(),
                    "source_symbol": row.get(4)?.as_str().unwrap_or_default().to_string(),
                    "artifact_type": row.get(5)?.as_str().unwrap_or_default().to_string(),
                    "ranking_reasons": [row.get(6)?.as_str().unwrap_or_default().to_string()],
                    "ranking_score": row.get(7)?.as_i64().unwrap_or_default(),
                    "evidence_class": "soll_traceability",
                }))
            })
            .collect::<Vec<_>>();

        self.expand_concept_governing_entities(&mut selected, project, top_k);
        if !selected.is_empty() {
            return selected;
        }

        let filtered_terms = terms
            .iter()
            .filter(|term| term.len() >= 4)
            .cloned()
            .collect::<Vec<_>>();
        if filtered_terms.is_empty() {
            return Vec::new();
        }

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
        let lexical_predicate = filtered_terms
            .iter()
            .map(|term| {
                format!(
                    "(lower(n.title) LIKE '%{t}%' OR lower(COALESCE(n.description, '')) LIKE '%{t}%')",
                    t = Self::escape_sql(term)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let fallback_query = format!(
            "SELECT n.id, n.type, COALESCE(n.title, ''), \
                    CASE \
                        WHEN n.type = 'Requirement' THEN 'direct_requirement_match' \
                        WHEN n.type = 'Decision' THEN 'direct_decision_match' \
                        ELSE 'direct_intent_match' \
                    END AS ranking_reason, \
                    CASE \
                        WHEN n.type = 'Requirement' THEN 95 \
                        WHEN n.type = 'Decision' THEN 90 \
                        WHEN n.type = 'Concept' THEN 80 \
                        ELSE 70 \
                    END AS ranking_score
             FROM soll.Node n
             WHERE ({lexical_predicate}){project_filter}
             ORDER BY ranking_score DESC, n.id ASC
             LIMIT {limit}",
            limit = top_k.min(4),
        );
        let fallback_raw = self
            .graph_store
            .query_json(&fallback_query)
            .unwrap_or_else(|_| "[]".to_string());
        let fallback_rows: Vec<Vec<Value>> =
            serde_json::from_str(&fallback_raw).unwrap_or_default();
        selected.extend(fallback_rows.into_iter().filter_map(|row| {
            Some(json!({
                "id": row.first()?.as_str()?.to_string(),
                "type": row.get(1)?.as_str()?.to_string(),
                "title": row.get(2)?.as_str().unwrap_or_default().to_string(),
                "relation_type": "",
                "source_symbol": "",
                "artifact_type": "",
                "ranking_reasons": [row.get(3)?.as_str().unwrap_or_default().to_string()],
                "ranking_score": row.get(4)?.as_i64().unwrap_or_default(),
                "evidence_class": "soll_lexical_fallback",
            }))
        }));
        self.expand_concept_governing_entities(&mut selected, project, top_k);
        selected
    }

    pub(super) fn expand_concept_governing_entities(
        &self,
        selected: &mut Vec<Value>,
        project: Option<&str>,
        top_k: usize,
    ) {
        let concept_ids = selected
            .iter()
            .filter(|row| row.get("type").and_then(|value| value.as_str()) == Some("Concept"))
            .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if concept_ids.is_empty() {
            return;
        }

        let mut seen_ids = selected
            .iter()
            .filter_map(|row| row.get("id").and_then(|value| value.as_str()))
            .map(str::to_string)
            .collect::<HashSet<_>>();
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
        let concept_ids_sql = concept_ids
            .iter()
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(", ");

        let requirement_query = format!(
            "SELECT DISTINCT n.id, n.type, COALESCE(n.title, ''), COALESCE(e.relation_type, ''), \
                    c.id AS source_symbol, '' AS artifact_type, \
                    'concept_requirement_bridge' AS ranking_reason, \
                    88 AS ranking_score \
             FROM soll.Node c \
             JOIN soll.Edge e ON e.source_id = c.id \
             JOIN soll.Node n ON n.id = e.target_id \
             WHERE c.id IN ({concept_ids_sql}) AND n.type = 'Requirement'{project_filter} \
             ORDER BY ranking_score DESC, n.id ASC \
             LIMIT {limit}",
            limit = top_k.min(4),
        );
        let decision_project_filter = project
            .map(|value| {
                format!(
                    " AND lower(d.project_code) IN ({})",
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
        let decision_query = format!(
            "SELECT DISTINCT d.id, d.type, COALESCE(d.title, ''), COALESCE(de.relation_type, ''), \
                    c.id AS source_symbol, '' AS artifact_type, \
                    'concept_decision_bridge' AS ranking_reason, \
                    84 AS ranking_score \
             FROM soll.Node c \
             JOIN soll.Edge ce ON ce.source_id = c.id \
             JOIN soll.Node r ON r.id = ce.target_id AND r.type = 'Requirement' \
             JOIN soll.Edge de ON de.target_id = r.id \
             JOIN soll.Node d ON d.id = de.source_id \
             WHERE c.id IN ({concept_ids_sql}) AND d.type = 'Decision'{decision_project_filter} \
             ORDER BY ranking_score DESC, d.id ASC \
             LIMIT {limit}",
            limit = top_k.min(4),
        );

        for query in [requirement_query, decision_query] {
            let raw = self
                .graph_store
                .query_json(&query)
                .unwrap_or_else(|_| "[]".to_string());
            let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                let Some(id) = row.first().and_then(|value| value.as_str()) else {
                    continue;
                };
                if !seen_ids.insert(id.to_string()) {
                    continue;
                }
                selected.push(json!({
                    "id": id.to_string(),
                    "type": row.get(1).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "title": row.get(2).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "relation_type": row.get(3).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "source_symbol": row.get(4).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "artifact_type": row.get(5).and_then(|value| value.as_str()).unwrap_or_default().to_string(),
                    "ranking_reasons": [row.get(6).and_then(|value| value.as_str()).unwrap_or_default().to_string()],
                    "ranking_score": row.get(7).and_then(|value| value.as_i64()).unwrap_or_default(),
                    "evidence_class": "soll_concept_bridge",
                }));
            }
        }
    }
}
