use serde_json::{json, Value};
use std::collections::HashSet;
use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn collect_structural_neighbors(&self, entry_candidates: &[EntryCandidate], route: RetrievalRoute) -> Vec<Value> {
        // REQ-AXO-91486 slice 2 — radius raised from 1-2 to 5-10 when the
        // RAM view is warm (sub-microsecond CSR traversal makes the wider
        // sweep cheap). The legacy SQL CTE path keeps radius 1-2 below
        // because the projection cache becomes expensive at higher radii.
        let ram_view = crate::ist_snapshot::process_view();
        let cap_per_anchor: usize = if matches!(route, RetrievalRoute::Impact) { 50 } else { 20 };
        let total_cap: usize = cap_per_anchor * 2;
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for anchor in entry_candidates.iter().take(2) {
            if anchor.kind == "file" { continue; }
            // RAM fast-path : when warm, expand to radius 5-10 with higher
            // neighbor caps so structural retrieval surfaces more context
            // per question (REQ-AXO-91486 invariant).
            if !anchor.project_code.is_empty() && ram_view.is_warm(&anchor.project_code) {
                let project = &anchor.project_code;
                {
                    let ram_radius: u32 = if matches!(route, RetrievalRoute::Impact) { 10 } else { 5 };
                    if let Some(ids) = ram_view.forward_at_radius(project, &anchor.id, ram_radius, cap_per_anchor, &[]) {
                        for target_id in ids {
                            if target_id == anchor.id { continue; }
                            let key = format!("{}:{target_id}", anchor.id);
                            if !seen.insert(key) { continue; }
                            selected.push(json!({
                                "anchor_symbol": anchor.name,
                                "target_type": "symbol",
                                "target_id": target_id,
                                "edge_kind": "ram_csr",
                                "distance": 0,
                                "label": target_id,
                                "uri": "",
                                "evidence_class": "derived_ist_ram_snapshot",
                            }));
                            if selected.len() >= total_cap { return selected; }
                        }
                        continue;
                    }
                }
            }
            // SQL fallback (legacy radius 1-2 cap 2 per REQ-AXO-91486 invariant).
            let radius = if matches!(route, RetrievalRoute::Impact) { 2 } else { 1 };
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
