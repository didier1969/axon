// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    fn resolve_scoped_symbol_id(&self, symbol: &str, project: Option<&str>) -> Option<String> {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT id FROM Symbol \
                 WHERE (name = $sym OR id = $sym) AND project_slug = $project \
                 LIMIT 1",
                json!({ "sym": symbol, "project": project }),
            )
        } else {
            (
                "SELECT id FROM Symbol WHERE name = $sym OR id = $sym LIMIT 1",
                json!({ "sym": symbol }),
            )
        };
        let res = self.graph_store.query_json_param(query, &params).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        rows.first()?
            .first()?
            .as_str()
            .map(|value| value.to_string())
    }

    fn build_local_projection_section(
        &self,
        _symbol: &str,
        anchor: &str,
        depth: u64,
    ) -> Option<String> {
        let radius = depth.max(1).min(2);
        let anchor_id = self
            .graph_store
            .refresh_symbol_projection(anchor, radius)
            .ok()??;
        let projection_res = self
            .graph_store
            .query_graph_projection("symbol", &anchor_id, radius)
            .ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&projection_res).unwrap_or_default();
        if rows.len() <= 1 {
            return None;
        }

        Some(format!(
            "\n\n### Projection locale derivee\n\n**Etat:** vue de voisinage derivee, utile pour le contexte local; elle ne remplace pas la verite canonique de `CALLS`.\n\n{}",
            format_table_from_json(
                &projection_res,
                &["Type cible", "ID cible", "Type de lien", "Distance", "Label", "URI"]
            )
        ))
    }

    pub(crate) fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let Some(target_id) = self.resolve_scoped_symbol_id(symbol, project) else {
            return self.axon_impact_without_calls(symbol, project, depth);
        };

        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS (
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $target_id
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
            depth
        );
        let params = json!({ "target_id": target_id });

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                let table =
                    format_table_from_json(&res, &["Fichier / Projet", "Symbole Impacté", "Type"]);
                let count_query = format!(
                    "WITH RECURSIVE traverse(caller, callee, depth) AS (
                        SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $target_id
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1
                        FROM CALLS c JOIN traverse t ON c.target_id = t.caller
                        WHERE t.depth < {}
                    )
                    SELECT count(DISTINCT caller) FROM traverse",
                    depth
                );
                let impact_radius = self
                    .graph_store
                    .query_count_param(&count_query, &params)
                    .unwrap_or(0);

                if rows.is_empty() && impact_radius == 0 {
                    return self.axon_impact_without_calls(symbol, project, depth);
                }

                let mut report = format!("## 💥 Analyse d'Impact Transversale : {}\n\n", symbol);
                if let Some(note) = self.project_scope_truth_note(project) {
                    report.push_str(&note);
                    report.push('\n');
                }
                if let Some(note) =
                    self.degraded_truth_note(self.degraded_symbol_count(symbol, project))
                {
                    report.push_str(&note);
                    report.push('\n');
                }
                report.push_str(&format!(
                    "**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n",
                    depth, impact_radius
                ));
                report.push_str(&table);
                if let Some(section) =
                    self.build_local_projection_section(symbol, &target_id, depth)
                {
                    report.push_str(&section);
                }

                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }], "isError": true }),
            ),
        }
    }

    fn axon_impact_without_calls(
        &self,
        symbol: &str,
        project: Option<&str>,
        depth: u64,
    ) -> Option<Value> {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT name, kind, COALESCE(project_slug, 'unknown') \
                 FROM Symbol \
                 WHERE (name = $sym OR id = $sym) AND project_slug = $project \
                 LIMIT 5",
                json!({ "sym": symbol, "project": project }),
            )
        } else {
            (
                "SELECT name, kind, COALESCE(project_slug, 'unknown') \
                 FROM Symbol \
                 WHERE name = $sym OR id = $sym \
                 LIMIT 5",
                json!({ "sym": symbol }),
            )
        };
        let symbol_res = self
            .graph_store
            .query_json_param(query, &params)
            .unwrap_or_else(|_| "[]".to_string());
        let symbol_rows: Vec<Vec<Value>> = serde_json::from_str(&symbol_res).unwrap_or_default();
        if symbol_rows.is_empty() {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!("## 💥 Analyse d'Impact Transversale : {}\n\nAucun symbole correspondant n'a ete trouve.", symbol)
                }]
            }));
        }
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        let calls_count = self
            .graph_store
            .query_count("SELECT count(*) FROM CALLS")
            .unwrap_or(0);
        if calls_count > 0 {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Analyse d'Impact Transversale : {}\n\n{}{}Aucun impact n'a ete calcule a la profondeur {}.",
                        symbol,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        depth
                    )
                }]
            }));
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "## 💥 Analyse d'Impact Transversale : {}\n\n{}{}Le symbole existe, mais le graphe d'appel n'est pas encore disponible dans cette base live.\n\n{}\n\n**Etat:** CALLS est vide; le rayon d'impact ne peut pas encore etre calcule de maniere fiable.",
                    symbol,
                    project_note.unwrap_or_default(),
                    degraded_note.unwrap_or_default(),
                    format_table_from_json(&symbol_res, &["Nom", "Type", "Projet"])
                )
            }]
        }))
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
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let target_id = self.resolve_scoped_symbol_id(symbol, project)?;
        let query = format!(
            "WITH RECURSIVE traverse(caller, callee, depth) AS ( \
                SELECT source_id, target_id, 1 as depth FROM CALLS WHERE target_id = $target_id \
                UNION ALL \
                SELECT c.source_id, c.target_id, t.depth + 1 \
                FROM CALLS c JOIN traverse t ON c.target_id = t.caller \
                WHERE t.depth < {} \
            ) \
            SELECT count(DISTINCT caller) FROM traverse",
            depth
        );
        let params = json!({"target_id": target_id});

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let count: i64 = res.trim().parse().unwrap_or(0);
                let report = format!(
                    "🔮 Dry-Run Mutation : Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.",
                    symbol, count
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }], "isError": true }),
            ),
        }
    }
}
