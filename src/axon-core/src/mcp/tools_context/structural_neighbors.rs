//! REQ-AXO-219 — structural-neighbour expansion extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Method on `McpServer`;
//! behavior-preserving move, `self.collect_structural_neighbors` call site
//! unchanged. RAM-only CSR expansion (PIL-AXO-9002, REQ-AXO-901952), no PG
//! projection fallback.

use super::super::McpServer;
use super::retrieval_model::{EntryCandidate, RetrievalRoute};
use serde_json::{json, Value};
use std::collections::HashSet;

impl McpServer {
    pub(super) fn collect_structural_neighbors(
        &self,
        entry_candidates: &[EntryCandidate],
        route: RetrievalRoute,
    ) -> Vec<Value> {
        // REQ-AXO-91486 slice 2 — RAM fast-path : when the cache is warm for
        // the anchor's project (RAM unconditional, REQ-AXO-901952), expand to radius
        // 5-10 (Impact: 10) with neighbor cap 20-50 (Impact: 50), sub-µs
        // CSR traversal. Cache miss / disabled → silent fallback to the
        // legacy radius 1-2 / cap 2 SQL CTE path below.
        let ram_view = crate::ist_snapshot::process_view();
        let cap_per_anchor: usize = if matches!(route, RetrievalRoute::Impact) {
            50
        } else {
            20
        };
        let total_cap: usize = cap_per_anchor * 2;
        let mut selected = Vec::new();
        let mut seen = HashSet::new();

        for anchor in entry_candidates.iter().take(2) {
            if anchor.kind == "file" {
                continue;
            }
            // REQ-AXO-901952 (gap D) — RAM-only structural neighbours. Warm
            // the anchor's per-project snapshot ; if it can't warm, skip this
            // anchor (best-effort — retrieve_context still has FTS + vector).
            // No query_graph_projection PG fallback.
            if anchor.project_code.is_empty()
                || !self.ensure_ram_snapshot_warm(&anchor.project_code)
            {
                continue;
            }
            let ram_radius: u32 = if matches!(route, RetrievalRoute::Impact) {
                10
            } else {
                5
            };
            // Impact route wants callers (reverse adjacency — who breaks if the
            // anchor changes) ; other routes want forward dependencies (callees).
            let ram_ids = if matches!(route, RetrievalRoute::Impact) {
                ram_view.reverse_at_radius(
                    &anchor.project_code,
                    &anchor.id,
                    ram_radius,
                    cap_per_anchor,
                    &[],
                )
            } else {
                ram_view.forward_at_radius(
                    &anchor.project_code,
                    &anchor.id,
                    ram_radius,
                    cap_per_anchor,
                    &[],
                )
            };
            if let Some(ids) = ram_ids {
                for target_id in ids {
                    if target_id == anchor.id {
                        continue;
                    }
                    let key = format!("{}:{target_id}", anchor.id);
                    if !seen.insert(key) {
                        continue;
                    }
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
                    if selected.len() >= total_cap {
                        return selected;
                    }
                }
            }
        }

        selected
    }
}
