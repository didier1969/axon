use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    fn impact_target_filter() -> &'static str {
        "target_id IN (SELECT id FROM Symbol WHERE name = $sym OR id = $sym)"
    }

    pub(crate) fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);

        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS (
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE {}
                UNION ALL
                SELECT c.source_id, c.target_id, t.depth + 1
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller
                WHERE t.depth < {}
            )
            SELECT DISTINCT COALESCE(f.path, 'Unknown') AS origin, s.name, s.kind
            FROM traverse t
            JOIN Symbol s ON t.caller = s.id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id",
            Self::impact_target_filter(),
            depth
        );
        let params = json!({ "sym": symbol });

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let table = format_table_from_json(&res, &["Fichier / Projet", "Symbole Impacté", "Type"]);
                let count_query = format!(
                    "WITH RECURSIVE traverse(caller, callee, depth) AS (
                        SELECT source_id, target_id, 1 as depth FROM CALLS WHERE {}
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1
                        FROM CALLS c JOIN traverse t ON c.target_id = t.caller
                        WHERE t.depth < {}
                    )
                    SELECT count(DISTINCT caller) FROM traverse",
                    Self::impact_target_filter(),
                    depth
                );
                let impact_radius = self
                    .graph_store
                    .query_count_param(&count_query, &params)
                    .unwrap_or(0);

                let mut report = format!("## 💥 Analyse d'Impact Transversale : {}\n\n", symbol);
                report.push_str(&format!(
                    "**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n",
                    depth, impact_radius
                ));
                report.push_str(&table);

                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }], "isError": true })),
        }
    }

    pub(crate) fn axon_diff(&self, args: &Value) -> Option<Value> {
        let diff = args.get("diff_content")?.as_str()?;
        let mut files = std::collections::HashSet::new();
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.insert(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/") {
                if path != "/dev/null" {
                    files.insert(path.to_string());
                }
            }
        }

        let mut all_results = Vec::new();
        for file in files {
            let query = format!(
                "SELECT s.name, s.kind FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id WHERE f.path LIKE '%{}%'",
                file.replace("'", "''")
            );
            if let Ok(res) = self.graph_store.query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        Some(json!({ "content": [{ "type": "text", "text": all_results.join("\n\n") }] }))
    }

    pub(crate) fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE {} \
                UNION ALL \
                SELECT c.source_id, c.target_id, t.depth + 1 \
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller \
                WHERE t.depth < {} \
            ) \
            SELECT count(DISTINCT caller) FROM traverse",
            Self::impact_target_filter(),
            depth
        );
        let params = json!({"sym": symbol});

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let count: i64 = res.trim().parse().unwrap_or(0);
                let report = format!(
                    "🔮 Dry-Run Mutation : Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.",
                    symbol, count
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }], "isError": true })),
        }
    }
}
