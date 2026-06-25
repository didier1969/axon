//! REQ-AXO-219 — semantic SOLL retrieval method extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Method on `McpServer`;
//! behavior-preserving move, `self.collect_soll_entities_via_ann` call site
//! unchanged. Cross-module `Self::escape_sql` / `Self::project_scope_variants`
//! and `self.graph_store` resolve via the shared McpServer type + descendant
//! access.

use super::super::McpServer;
use serde_json::{json, Value};

impl McpServer {
    /// REQ-AXO-901757 slice B3b — semantic SOLL retrieval arm. ANN over SOLL
    /// description embeddings (populated by the slice-B2 sweep) surfaces governing
    /// intent (decisions / requirements / concepts) that is semantically relevant
    /// to the question even when there is NO graph traceability from the entry
    /// symbols — fusing the *why* with retrieval (VIS-AXO-001). The HNSW index is
    /// global; project scoping happens on the `soll.Node` join. A distinct
    /// `evidence_class` (`soll_semantic_ann`) lets the intent band and rationale
    /// quality tell a semantic match from a traceability one.
    pub(super) fn collect_soll_entities_via_ann(
        &self,
        question_vector: &[f32],
        project: Option<&str>,
        limit: usize,
    ) -> Vec<Value> {
        if limit == 0 {
            return Vec::new();
        }
        let hits = match self
            .graph_store
            .select_soll_nodes_by_ann(question_vector, limit.saturating_mul(4).max(8))
        {
            Ok(hits) if !hits.is_empty() => hits,
            _ => return Vec::new(),
        };
        let dist_by_id: std::collections::HashMap<String, f64> = hits.iter().cloned().collect();
        let id_list = hits
            .iter()
            .map(|(id, _)| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(",");
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
        let query = format!(
            "SELECT n.id, n.type, COALESCE(n.title,''), COALESCE(n.description,''), \
                    COALESCE(n.status,'') \
             FROM soll.Node n \
             WHERE n.id IN ({id_list}){project_filter}",
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut entities = rows
            .into_iter()
            .filter_map(|row| {
                let id = row.first()?.as_str()?.to_string();
                let dist = dist_by_id.get(&id).copied().unwrap_or(f64::MAX);
                // cosine distance ∈ [0,2]; map to 0..90 so a semantic match sits
                // just below the strongest traceability tiers (Symbol/File 95-100).
                let score = (((2.0 - dist) / 2.0) * 90.0).round().clamp(0.0, 90.0) as i64;
                Some((
                    dist,
                    json!({
                        "id": id,
                        "type": row.get(1)?.as_str()?.to_string(),
                        "title": row.get(2).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        "description": row.get(3).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        "status": row.get(4).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                        "relation_type": "",
                        "source_symbol": "",
                        "artifact_type": "",
                        "ranking_reasons": [format!("semantic_ann (cosine_distance={dist:.3})")],
                        "ranking_score": score,
                        "evidence_class": "soll_semantic_ann",
                    }),
                ))
            })
            .collect::<Vec<_>>();
        entities.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        entities.truncate(limit);
        entities.into_iter().map(|(_, entity)| entity).collect()
    }
}
