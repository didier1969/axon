use serde_json::{json, Value};

use super::tools_framework_support::{
    cache_read, cache_write, diff_metric_summaries, metric_value,
};
use super::McpServer;

impl McpServer {
    pub(super) fn derive_conception_view(&self, project_code: &str) -> Value {
        let escaped_project = project_code.replace('\'', "''");
        let modules_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT f.path, count(rel.target_id) AS symbol_count
                 FROM File f
                 LEFT JOIN CONTAINS rel
                   ON rel.source_id = f.path
                  AND rel.project_code = '{project}'
                 WHERE f.project_code = '{}'
                 GROUP BY 1
                 ORDER BY symbol_count DESC, f.path ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
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

        let interfaces_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, f.path
                 FROM Symbol s
                 LEFT JOIN CONTAINS rel
                   ON rel.target_id = s.id
                  AND rel.project_code = '{project}'
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND s.kind = 'interface'
                 ORDER BY s.name ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
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

        let contracts_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT s.name, s.kind, f.path
                 FROM Symbol s
                 LEFT JOIN CONTAINS rel
                   ON rel.target_id = s.id
                  AND rel.project_code = '{project}'
                 LEFT JOIN File f ON f.path = rel.source_id
                 WHERE s.project_code = '{}'
                   AND COALESCE(s.is_public, false) = true
                   AND s.kind IN ('interface', 'module', 'class', 'struct', 'function', 'method')
                 ORDER BY s.kind ASC, s.name ASC
                 LIMIT 5",
                escaped_project,
                project = escaped_project
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

        let flows_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT src.name, src_rel.source_id, dst.name, dst_rel.source_id
                 FROM CALLS c
                 JOIN Symbol src ON src.id = c.source_id
                 JOIN Symbol dst ON dst.id = c.target_id
                 JOIN CONTAINS src_rel
                   ON src_rel.target_id = src.id
                  AND src_rel.project_code = '{project}'
                 JOIN CONTAINS dst_rel
                   ON dst_rel.target_id = dst.id
                  AND dst_rel.project_code = '{project}'
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
                   AND c.project_code = '{project}'
                   AND src_rel.source_id != dst_rel.source_id
                 ORDER BY src.name ASC, dst.name ASC
                 LIMIT 5",
                project = escaped_project
            ))
            .unwrap_or_else(|_| "[]".to_string());
        let flow_rows: Vec<Vec<Value>> = serde_json::from_str(&flows_raw).unwrap_or_default();
        let flows = flow_rows
            .iter()
            .filter_map(|row| {
                Some(json!({
                    "from_symbol": row.first()?.as_str()?.to_string(),
                    "from_path": row.get(1).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                    "to_symbol": row.get(2).and_then(|value| value.as_str()).unwrap_or("").to_string(),
                    "to_path": row.get(3).and_then(|value| value.as_str()).unwrap_or("").to_string()
                }))
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
        let flow_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*)
                 FROM CALLS c
                 JOIN CONTAINS src_rel
                   ON src_rel.target_id = c.source_id
                  AND src_rel.project_code = '{project}'
                 JOIN CONTAINS dst_rel
                   ON dst_rel.target_id = c.target_id
                  AND dst_rel.project_code = '{project}'
                 JOIN Symbol src ON src.id = c.source_id
                 JOIN Symbol dst ON dst.id = c.target_id
                 WHERE src.project_code = '{project}'
                   AND dst.project_code = '{project}'
                   AND c.project_code = '{project}'
                   AND src_rel.source_id != dst_rel.source_id",
                project = escaped_project
            ))
            .unwrap_or(0);

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
