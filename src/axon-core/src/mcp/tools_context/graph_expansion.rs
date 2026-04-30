use serde_json::{json, Value};
use std::collections::HashSet;
use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn collect_structural_neighbors(&self, entry_candidates: &[EntryCandidate], route: RetrievalRoute) -> Vec<Value> {
        let radius = if matches!(route, RetrievalRoute::Impact) { 2 } else { 1 };
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for anchor in entry_candidates.iter().take(2) {
            if anchor.kind == "file" { continue; }
            let Ok(Some(anchor_id)) = self.graph_store.refresh_symbol_projection(&anchor.id, radius) else { continue; };
            let raw = self.graph_store.query_graph_projection("symbol", &anchor_id, radius).unwrap_or_else(|_| "[]".to_string());
            let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                let Some(target_id) = row.get(1).and_then(|v| v.as_str()) else { continue; };
                let edge_kind = row.get(2).and_then(|v| v.as_str()).unwrap_or("unknown");
                if target_id == anchor.id || edge_kind == "anchor" { continue; }
                let key = format!("{}:{target_id}", anchor.id);
                if !seen.insert(key) { continue; }
                selected.push(json!({
                    "anchor_symbol": anchor.name,
                    "target_type": row.first().and_then(|v| v.as_str()).unwrap_or("unknown"),
                    "target_id": target_id, "edge_kind": edge_kind,
                    "distance": row.get(3).and_then(|v| v.as_i64()).unwrap_or(0),
                    "label": row.get(4).and_then(|v| v.as_str()).unwrap_or(target_id),
                    "uri": row.get(5).and_then(|v| v.as_str()).unwrap_or(""),
                    "evidence_class": "derived_graph_projection",
                }));
                if selected.len() >= 2 { return selected; }
            }
        }
        selected
    }
}
