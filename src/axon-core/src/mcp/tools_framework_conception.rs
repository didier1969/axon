use serde_json::{json, Value};

use super::tools_framework_support::{
    cache_read, cache_write, diff_metric_summaries, metric_value,
};
use super::McpServer;

impl McpServer {
    pub(super) fn derive_conception_view(&self, project_code: &str) -> Value {
        let escaped_project = project_code.replace('\'', "''");
        // REQ-AXO-901653 slice-5c — public.File retired ; group by Chunk.file_path
        // which is the canonical per-file pivot post pipeline.
        let modules_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT c.file_path AS path, count(DISTINCT c.source_id) AS symbol_count
                 FROM ist.Chunk c
                 WHERE c.project_code = '{}'
                   AND c.file_path IS NOT NULL
                 GROUP BY c.file_path
                 ORDER BY symbol_count DESC, c.file_path ASC
                 LIMIT 5",
                escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let module_rows: Vec<Vec<Value>> = serde_json::from_str(&modules_raw).unwrap_or_default();
        let modules = module_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "path": row.first()?.as_str()?.to_string(),
                    "symbol_count": row.get(1).and_then(|value| value.as_u64()).unwrap_or(0)
                }))
            })
            .collect::<Vec<_>>();

        // REQ-AXO-901653 slice-5c — interface listing now derives path from
        // Chunk.file_path (Chunk → Symbol via source_id). No File join.
        let interfaces_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, c.file_path AS path
                 FROM ist.Symbol s
                 LEFT JOIN ist.Chunk c
                   ON c.source_id = s.id
                  AND c.project_code = s.project_code
                 WHERE s.project_code = '{}'
                   AND s.kind = 'interface'
                 GROUP BY s.name, c.file_path
                 ORDER BY s.name ASC
                 LIMIT 5",
                escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let interface_rows: Vec<Vec<Value>> =
            serde_json::from_str(&interfaces_raw).unwrap_or_default();
        let interfaces = interface_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "name": row.first()?.as_str()?.to_string(),
                    "path": row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
            })
            .collect::<Vec<_>>();

        // REQ-AXO-901653 slice-5c — path now derived from Chunk.file_path
        // (Chunk.source_id = Symbol.id) ; public.File join removed.
        let contracts_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, s.kind, c.file_path AS path
                 FROM ist.Symbol s
                 LEFT JOIN ist.Chunk c
                   ON c.source_id = s.id
                  AND c.project_code = s.project_code
                 WHERE s.project_code = '{}'
                   AND COALESCE(s.is_public, false) = true
                   AND s.kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')
                 GROUP BY s.name, s.kind, c.file_path
                 ORDER BY s.kind ASC, s.name ASC
                 LIMIT 5",
                escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let contract_rows: Vec<Vec<Value>> =
            serde_json::from_str(&contracts_raw).unwrap_or_default();
        let contracts = contract_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "name": row.first()?.as_str()?.to_string(),
                    "kind": row.get(1).and_then(|value| value.as_str()).unwrap_or("unknown").to_string(),
                    "path": row.get(2).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
            })
            .collect::<Vec<_>>();

        // REQ-AXO-901970 — cross-file CALLS flows + count RAM-only (forward CALLS
        // over the process snapshot, file via reverse CONTAINS). Replaces the PG
        // `ist.Edge` join. Cold cache → empty flows + 0 count (no PG fallback).
        let (flow_tuples, flow_count) = if self.ensure_ram_snapshot_warm(project_code) {
            crate::ist_snapshot::process_view()
                .cross_file_call_flows(project_code, 5)
                .unwrap_or_default()
        } else {
            (Vec::new(), 0)
        };
        let flows = flow_tuples
            .iter()
            .map(|(from_symbol, from_path, to_symbol, to_path)| {
                json!({
                    "from_symbol": from_symbol,
                    "from_path": from_path,
                    "to_symbol": to_symbol,
                    "to_path": to_path,
                })
            })
            .collect::<Vec<_>>();

        let interface_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND kind = 'interface'",
                escaped_project
            ))
            .unwrap_or(0);
        let contract_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM Symbol
                 WHERE project_code = '{}'
                   AND COALESCE(is_public, false) = true
                   AND kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')",
                escaped_project
            ))
            .unwrap_or(0);
        // flow_count computed RAM-only above alongside the flows list.

        json!({
            "module_count": modules.len(),
            "modules": modules,
            "interface_count": interface_count,
            "interfaces": interfaces,
            "flow_count": flow_count,
            "flows": flows,
            "contract_count": contract_count,
            "contracts": contracts,
            "boundaries": [],
            "owners": [],
            "confidence": "medium",
            "provenance": "derived_read_only_view"
        })
    }

    pub(super) fn cached_conception_view(&self, project_code: &str) -> Value {
        let now_ms = Self::now_unix_ms();
        let cache_key = project_code.to_string();
        if let Some(cached) = cache_read(
            Self::conception_cache(),
            &cache_key,
            now_ms,
            super::tools_framework::CONCEPTION_CACHE_TTL_MS,
        ) {
            return cached;
        }

        let conception = self.derive_conception_view(project_code);
        cache_write(Self::conception_cache(), cache_key, now_ms, &conception);
        conception
    }

    pub(super) fn build_project_status_delta(
        previous_summary: Option<&Value>,
        current_summary: &Value,
    ) -> Value {
        let Some(previous) = previous_summary else {
            return json!({ "available": false });
        };
        json!({
            "available": true,
            "metric_delta": diff_metric_summaries(current_summary, previous),
            "wrapper_count_delta": metric_value(current_summary, "wrapper_count") - metric_value(previous, "wrapper_count"),
            "feature_envy_count_delta": metric_value(current_summary, "feature_envy_count") - metric_value(previous, "feature_envy_count"),
            "detour_count_delta": metric_value(current_summary, "detour_count") - metric_value(previous, "detour_count"),
            "abstraction_detour_count_delta": metric_value(current_summary, "abstraction_detour_count") - metric_value(previous, "abstraction_detour_count"),
            "orphan_code_count_delta": metric_value(current_summary, "orphan_code_count") - metric_value(previous, "orphan_code_count"),
            "orphan_intent_count_delta": metric_value(current_summary, "orphan_intent_count") - metric_value(previous, "orphan_intent_count"),
            "cycle_count_delta": metric_value(current_summary, "cycle_count") - metric_value(previous, "cycle_count"),
            "god_object_count_delta": metric_value(current_summary, "god_object_count") - metric_value(previous, "god_object_count")
        })
    }
}
