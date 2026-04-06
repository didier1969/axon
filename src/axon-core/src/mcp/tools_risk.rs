// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
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

    fn suggest_scoped_symbols(
        &self,
        symbol: &str,
        project: Option<&str>,
        limit: usize,
    ) -> String {
        let needle = symbol.trim();
        if needle.is_empty() {
            return "[]".to_string();
        }
        let (query, params) = if let Some(project) = project {
            (
                "SELECT name, kind, COALESCE(project_slug, 'unknown') \
                 FROM Symbol \
                 WHERE project_slug = $project \
                   AND lower(name) LIKE lower($pat) \
                 ORDER BY name \
                 LIMIT $limit",
                json!({ "project": project, "pat": format!("%{}%", needle), "limit": limit as u64 }),
            )
        } else {
            (
                "SELECT name, kind, COALESCE(project_slug, 'unknown') \
                 FROM Symbol \
                 WHERE lower(name) LIKE lower($pat) \
                 ORDER BY name \
                 LIMIT $limit",
                json!({ "pat": format!("%{}%", needle), "limit": limit as u64 }),
            )
        };
        self.graph_store
            .query_json_param(query, &params)
            .unwrap_or_else(|_| "[]".to_string())
    }

    fn build_local_projection_section(
        &self,
        _symbol: &str,
        anchor: &str,
        depth: u64,
    ) -> Option<String> {
        let radius = depth.clamp(1, 2);
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
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let Some(target_id) = self.resolve_scoped_symbol_id(symbol, project) else {
            return self.axon_impact_without_calls(symbol, project, depth);
        };

        let query = format!(
            "WITH RECURSIVE bridge_edges AS (
                SELECT s1.id AS source_id, s2.id AS target_id
                FROM Symbol s1
                JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                WHERE (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
            ),
            all_edges AS (
                SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                UNION ALL
                SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                UNION ALL
                SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
            ),
            traverse(caller, callee, depth, edge_type) AS (
                SELECT source_id, target_id, 1 as depth, edge_type FROM all_edges WHERE target_id = $target_id
                UNION ALL
                SELECT c.source_id, c.target_id, t.depth + 1, c.edge_type
                FROM all_edges c JOIN traverse t ON c.target_id = t.caller
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
                let mut table = if rows.len() > 15 {
                    format!("_Le rapport de code a été agrégé car {} symboles sont impactés. Seuls les impacts architecturaux majeurs sont détaillés ci-dessous._

", rows.len())
                } else {
                    format_table_from_json(&res, &["Fichier / Projet", "Symbole Impacté", "Type"])
                };

                let soll_query = format!(
                    "WITH RECURSIVE bridge_edges AS (
                        SELECT s1.id AS source_id, s2.id AS target_id
                        FROM Symbol s1
                        JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                        WHERE (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
                    ),
                    all_edges AS (
                        SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                        UNION ALL
                        SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                        UNION ALL
                        SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
                    ),
                    code_traverse(caller, callee, depth) AS (
                        SELECT source_id, target_id, 1 as depth FROM all_edges WHERE target_id = $target_id
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1
                        FROM all_edges c JOIN code_traverse t ON c.target_id = t.caller
                        WHERE t.depth < {code_depth}
                    ),
                    impacted_symbols AS (
                        SELECT caller AS id FROM code_traverse
                        UNION SELECT $target_id
                    ),
                    soll_entry_points AS (
                        SELECT t.soll_entity_id as id
                        FROM soll.Traceability t
                        JOIN Symbol s ON s.name = t.artifact_ref
                        JOIN impacted_symbols i ON i.id = s.id
                        WHERE t.artifact_type = 'Symbol'
                    ),
                    soll_traverse(id, depth) AS (
                        SELECT id, 1 as depth FROM soll_entry_points
                        UNION ALL
                        SELECT e.target_id, st.depth + 1
                        FROM soll.Edge e
                        JOIN soll_traverse st ON e.source_id = st.id
                        WHERE st.depth < 10
                    )
                    SELECT DISTINCT n.id, n.type, n.title
                    FROM soll_traverse st
                    JOIN soll.Node n ON st.id = n.id
                    ORDER BY n.type DESC, n.id",
                    code_depth = depth
                );

                let soll_raw = self
                    .graph_store
                    .query_json_param(&soll_query, &params)
                    .unwrap_or_else(|_| "[]".to_string());
                let soll_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&soll_raw).unwrap_or_default();
                    
                if !soll_rows.is_empty() {
                    table.push_str("\n### 🏛️ SOLL Impact (Architecture Compromise)\n\n| Entité | Type | Titre |\n| --- | --- | --- |\n");
                    for row in soll_rows {
                        let id = row.first().and_then(|v| v.as_str()).unwrap_or("-");
                        let t = row.get(1).and_then(|v| v.as_str()).unwrap_or("-");
                        let title = row.get(2).and_then(|v| v.as_str()).unwrap_or("-");
                        table.push_str(&format!("| `{}` | `{}` | {} |\n", id, t, title));
                    }
                    table.push('\n');
                }
                
                let count_query = format!(
                    "WITH RECURSIVE bridge_edges AS (
                        SELECT s1.id AS source_id, s2.id AS target_id
                        FROM Symbol s1
                        JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                        WHERE (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
                    ),
                    all_edges AS (
                        SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                        UNION ALL
                        SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                        UNION ALL
                        SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
                    ),
                    traverse(caller, callee, depth, edge_type) AS (
                        SELECT source_id, target_id, 1 as depth, edge_type FROM all_edges WHERE target_id = $target_id
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1, c.edge_type
                        FROM all_edges c JOIN traverse t ON c.target_id = t.caller
                        WHERE t.depth < {}
                    )
                    SELECT count(DISTINCT caller) FROM traverse",
                    depth
                );
                let confidence_query = format!(
                    "WITH RECURSIVE bridge_edges AS (
                        SELECT s1.id AS source_id, s2.id AS target_id
                        FROM Symbol s1
                        JOIN Symbol s2 ON s1.name = s2.name AND s1.id <> s2.id
                        WHERE (COALESCE(s1.is_nif, FALSE) = TRUE OR COALESCE(s2.is_nif, FALSE) = TRUE)
                    ),
                    all_edges AS (
                        SELECT source_id, target_id, 'calls' AS edge_type FROM CALLS
                        UNION ALL
                        SELECT source_id, target_id, 'calls_nif' AS edge_type FROM CALLS_NIF
                        UNION ALL
                        SELECT source_id, target_id, 'bridge_name' AS edge_type FROM bridge_edges
                    ),
                    traverse(caller, callee, depth, edge_type) AS (
                        SELECT source_id, target_id, 1 as depth, edge_type FROM all_edges WHERE target_id = $target_id
                        UNION ALL
                        SELECT c.source_id, c.target_id, t.depth + 1, c.edge_type
                        FROM all_edges c JOIN traverse t ON c.target_id = t.caller
                        WHERE t.depth < {}
                    )
                    SELECT edge_type, count(*) FROM traverse GROUP BY 1 ORDER BY 2 DESC",
                    depth
                );
                let impact_radius = self
                    .graph_store
                    .query_count_param(&count_query, &params)
                    .unwrap_or(0);
                let confidence_raw = self
                    .graph_store
                    .query_json_param(&confidence_query, &params)
                    .unwrap_or_else(|_| "[]".to_string());
                let confidence_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&confidence_raw).unwrap_or_default();
                let mut direct_edges = 0_i64;
                let mut nif_edges = 0_i64;
                let mut inferred_edges = 0_i64;
                for row in confidence_rows {
                    let edge_type = row
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let count = row
                        .get(1)
                        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|x| x as i64)))
                        .unwrap_or(0);
                    match edge_type {
                        "calls" => direct_edges += count,
                        "calls_nif" => nif_edges += count,
                        "bridge_name" => inferred_edges += count,
                        _ => {}
                    }
                }
                let confidence_label = if direct_edges + nif_edges > 0 {
                    "high"
                } else if inferred_edges > 0 {
                    "medium"
                } else {
                    "low"
                };

                if rows.is_empty() && impact_radius == 0 {
                    return self.axon_impact_without_calls(symbol, project, depth);
                }

                let mut evidence = String::new();
                if let Some(note) = self.project_scope_truth_note(project) {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                if let Some(note) =
                    self.degraded_truth_note(self.degraded_symbol_count(symbol, project))
                {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                evidence.push_str(&format!(
                    "**Rayon d'Impact (profondeur {}) :** {} composants affectés à travers le Treillis.\n\n",
                    depth, impact_radius
                ));
                evidence.push_str(&format!(
                    "**Coverage:** confidence={} (direct_calls={}, calls_nif={}, inferred_bridge={})\n\n",
                    confidence_label, direct_edges, nif_edges, inferred_edges
                ));
                evidence.push_str(&table);
                if let Some(section) =
                    self.build_local_projection_section(symbol, &target_id, depth)
                {
                    evidence.push_str(&section);
                }
                let scope = project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string());
                let report = format!(
                    "## 💥 Analyse d'Impact Transversale : {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "impact analysis computed",
                        &scope,
                        &evidence_by_mode(&evidence, mode),
                        &["review top impacted symbols", "run simulate_mutation before editing"],
                        confidence_label,
                    )
                );

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
            let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
            let suggestions_table = format_table_from_json(
                &suggestions,
                &["Symbole suggéré", "Type", "Projet"],
            );
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Analyse d'Impact Transversale : {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "warn_input_not_found",
                            "symbol not found in current scope",
                            &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                            &format!("Aucun symbole exact correspondant n'a ete trouve dans le scope courant.\n\n### Suggestions\n\n{}", suggestions_table),
                            &["retry with one suggested symbol", "use query/inspect to validate exact name"],
                            "medium",
                        )
                    )
                }]
            }));
        }
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        let calls_count = self
            .graph_store
            .query_count("SELECT (SELECT count(*) FROM CALLS) + (SELECT count(*) FROM CALLS_NIF)")
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
        let mode = args.get("mode").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(10, 500) as usize)
            .unwrap_or(120);
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
                "SELECT s.name, s.kind FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id WHERE f.path LIKE '%{}%' LIMIT {}",
                file.replace("'", "''"),
                limit
            );
            if let Ok(res) = self.graph_store.query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        let mut joined = all_results.join("\n\n");
        let truncated = if joined.len() > 60_000 {
            joined.truncate(60_000);
            true
        } else {
            false
        };
        let evidence = if truncated {
            format!("{}\n\n[truncated=true, max_chars=60000]", joined)
        } else {
            joined
        };
        let report = format!(
            "## 🧬 Diff Impact\n\n{}",
            format_standard_contract(
                "ok",
                "diff symbol extraction completed",
                "workspace:*",
                &evidence_by_mode(&evidence, mode),
                &["increase `limit` if needed", "run impact on selected symbols for blast radius"],
                if truncated { "medium" } else { "high" },
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{ "type": "text", "text": "Missing required argument: symbol" }],
                    "isError": true
                }));
            }
        };
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let target_id = match self.resolve_scoped_symbol_id(symbol, project) {
            Some(id) => id,
            None => {
                let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
                let suggestions_table =
                    format_table_from_json(&suggestions, &["Symbole suggéré", "Type", "Projet"]);
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "## 🔮 Dry-Run Mutation : {}\n\n{}",
                            symbol,
                            format_standard_contract(
                                "warn_input_not_found",
                                "symbol not found in current scope",
                                &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                                &evidence_by_mode(
                                    &format!("Aucun symbole exact n'a ete trouve dans le scope courant.\n\n### Suggestions\n\n{}", suggestions_table),
                                    mode,
                                ),
                                &["retry with one suggested symbol", "run inspect to validate symbol name"],
                                "medium",
                            )
                        )
                    }]
                }));
            }
        };
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
                    "## 🔮 Dry-Run Mutation : {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "mutation blast-radius estimated",
                        &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                        &evidence_by_mode(
                            &format!("Modifier '{}' va impacter en cascade ~{} composants dans l'architecture.", symbol, count),
                            mode,
                        ),
                        &["review impact output for precise affected components"],
                        "high",
                    )
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Simulation Error: {}", e) }], "isError": true }),
            ),
        }
    }
}
