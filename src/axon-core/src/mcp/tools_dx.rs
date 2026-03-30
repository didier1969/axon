use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

impl McpServer {
    fn build_symbol_search_params(query_text: &str, project: &str) -> Value {
        let normalized_query = query_text.to_lowercase();
        let wildcard_query = normalized_query.replace([' ', '-', ':'], "%");
        let compact_query = normalized_query.replace([' ', '-', '_', ':'], "");

        if project == "*" {
            json!({
                "needle": query_text,
                "normalized": normalized_query,
                "wildcard": wildcard_query,
                "compact": compact_query
            })
        } else {
            json!({
                "needle": query_text,
                "normalized": normalized_query,
                "wildcard": wildcard_query,
                "compact": compact_query,
                "proj": project
            })
        }
    }

    pub(crate) fn axon_fs_read(&self, args: &Value) -> Option<Value> {
        let uri = args.get("uri")?.as_str()?;
        let start_line = args.get("start_line").and_then(|v| v.as_u64());
        let end_line = args.get("end_line").and_then(|v| v.as_u64());

        let file_path = std::path::Path::new(uri);
        if !file_path.exists() || !file_path.is_file() {
            return Some(json!({ "content": [{ "type": "text", "text": format!("Erreur: Le fichier '{}' n'existe pas ou n'est pas lisible.", uri) }], "isError": true }));
        }

        match std::fs::read_to_string(file_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();
                let start = start_line.unwrap_or(1).saturating_sub(1) as usize;
                let end = end_line.unwrap_or(total_lines as u64) as usize;
                let start = start.min(total_lines);
                let end = end.min(total_lines).max(start);
                let sliced_content = lines[start..end].join("\n");
                let report = format!("📄 L2 Detail : {}\n(Lignes {} à {} sur {})\n\n```\n{}\n```", uri, start + 1, end, total_lines, sliced_content);
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Erreur de lecture: {}", e) }], "isError": true })),
        }
    }

    pub(crate) fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");

        let embedding = crate::embedder::batch_embed(vec![query_text.to_string()])
            .ok()
            .and_then(|v| v.into_iter().next());

        let (sql, params) = if let Some(emb) = embedding {
            let vec_str = format!("{:?}", emb);
            if project == "*" {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE lower(s.name) LIKE '%' || $normalized || '%' \
                            OR lower(replace(replace(replace(s.name, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
                            OR lower(s.name) LIKE '%' || $wildcard || '%' \
                            OR lower(replace(replace(replace(replace(s.name, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%' \
                            OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5 \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, vec_str
                    ),
                    Self::build_symbol_search_params(query_text, project),
                )
            } else {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE f.path LIKE '%' || $proj || '%' AND ( \
                            lower(s.name) LIKE '%' || $normalized || '%' \
                            OR lower(replace(replace(replace(s.name, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
                            OR lower(s.name) LIKE '%' || $wildcard || '%' \
                            OR lower(replace(replace(replace(replace(s.name, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%' \
                            OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5 \
                         ) \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, vec_str
                    ),
                    Self::build_symbol_search_params(query_text, project),
                )
            }
        } else if project == "*" {
            (
                "SELECT s.name, s.kind, f.path AS uri \
                 FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                 WHERE lower(s.name) LIKE '%' || $normalized || '%' \
                    OR lower(replace(replace(replace(s.name, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
                    OR lower(s.name) LIKE '%' || $wildcard || '%' \
                    OR lower(replace(replace(replace(replace(s.name, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%' \
                 LIMIT 10"
                    .to_string(),
                Self::build_symbol_search_params(query_text, project),
            )
        } else {
            (
                "SELECT s.name, s.kind, f.path AS uri \
                 FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                 WHERE f.path LIKE '%' || $proj || '%' AND ( \
                    lower(s.name) LIKE '%' || $normalized || '%' \
                    OR lower(replace(replace(replace(s.name, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
                    OR lower(s.name) LIKE '%' || $wildcard || '%' \
                    OR lower(replace(replace(replace(replace(s.name, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%' \
                 ) LIMIT 10"
                    .to_string(),
                Self::build_symbol_search_params(query_text, project),
            )
        };

        let mode_label = if sql.contains("score") {
            "hybride (structure + similarite semantique)"
        } else {
            "structurel (embedding temps reel indisponible)"
        };

        match self.graph_store.query_json_param(&sql, &params) {
            Ok(res) => {
                let headers = if sql.contains("score") {
                    vec!["Nom", "Type", "URI (Chemin)", "Distance Sémantique"]
                } else {
                    vec!["Nom", "Type", "URI (Chemin)"]
                };
                let table = format_table_from_json(&res, &headers);
                Some(json!({ "content": [{ "type": "text", "text": format!("### 🔎 Resultats de recherche : '{}'\n\n**Mode:** {}\n\n{}", query_text, mode_label, table) }] }))
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("Search Error: {}", e) }], "isError": true })),
        }
    }

    pub(crate) fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "SELECT s.name, s.kind, s.tested, \
                     (SELECT count(*) FROM CALLS c1 WHERE c1.target_id = s.id) AS callers, \
                     (SELECT count(*) FROM CALLS c2 WHERE c2.source_id = s.id) AS callees \
                     FROM Symbol s WHERE s.name = $sym";

        match self.graph_store.query_json_param(query, &json!({"sym": symbol})) {
            Ok(res) => {
                let table = format_table_from_json(&res, &["Nom", "Type", "Testé", "Appelants", "Appelés"]);
                Some(json!({ "content": [{ "type": "text", "text": format!("### 🔍 Inspection du Symbole : {}\n\n{}", symbol, table) }] }))
            }
            Err(_) => None,
        }
    }

    pub(crate) fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(100);

        let up_query = format!(
            "WITH RECURSIVE callers(sym, depth) AS (
                SELECT source_id, 1 FROM CALLS WHERE target_id = $sym
                UNION ALL
                SELECT c.source_id, callers.depth + 1
                FROM CALLS c
                JOIN callers ON c.target_id = callers.sym
                WHERE callers.depth < {}
            )
            SELECT DISTINCT s.name, s.kind FROM callers
            JOIN Symbol s ON s.id = callers.sym",
            depth
        );

        let down_query = format!(
            "WITH RECURSIVE callees(sym, depth) AS (
                SELECT target_id, 1 FROM CALLS WHERE source_id = $sym
                UNION ALL
                SELECT c.target_id, callees.depth + 1
                FROM CALLS c
                JOIN callees ON c.source_id = callees.sym
                WHERE callees.depth < {}
            )
            SELECT DISTINCT s.name, s.kind FROM callees
            JOIN Symbol s ON s.id = callees.sym",
            depth
        );

        let params = json!({"sym": symbol});
        let up_res = self.graph_store.query_json_param(&up_query, &params).unwrap_or_else(|_| "[]".to_string());
        let down_res = self.graph_store.query_json_param(&down_query, &params).unwrap_or_else(|_| "[]".to_string());

        let report = format!(
            "## ↕️ Trace Bidirectionnelle : {}\n\n### ↑ Appelants / Entry Points\n{}\n\n### ↓ Appels Profonds\n{}",
            symbol,
            format_table_from_json(&up_res, &["Nom", "Type"]),
            format_table_from_json(&down_res, &["Nom", "Type"])
        );

        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_api_break_check(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let query = "
            SELECT DISTINCT f.path AS consumer, s.name, s.kind
            FROM CALLS c
            JOIN Symbol s ON s.id = c.source_id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id
            JOIN Symbol target ON target.id = c.target_id
            WHERE target.name = $sym AND target.is_public = true
        ";

        match self.graph_store.query_json_param(query, &json!({"sym": symbol})) {
            Ok(res) => {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                if rows.is_empty() {
                    Some(json!({ "content": [{ "type": "text", "text": format!("✅ Aucun consommateur externe détecté pour '{}'.", symbol) }] }))
                } else {
                    Some(json!({ "content": [{ "type": "text", "text": format!("⚠️ **RISQUE DE RUPTURE D'API**\n\nModifier '{}' impactera directement les consommateurs suivants :\n\n{}", symbol, format_table_from_json(&res, &["Consommateur", "Symbole", "Type"])) }] }))
                }
            }
            Err(e) => Some(json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true })),
        }
    }
}
