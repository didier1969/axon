use serde_json::{json, Value};

use super::format::format_table_from_json;
use super::McpServer;

pub(crate) struct ProjectScopeSummary {
    pub(crate) total_files: i64,
    pub(crate) completed_files: i64,
    pub(crate) pending_files: i64,
    pub(crate) indexing_files: i64,
    pub(crate) backlog_files: i64,
    pub(crate) pending_reasons: Vec<(String, i64)>,
}

fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            if let Some(v) = number.as_i64() {
                Some(v)
            } else if let Some(v) = number.as_u64() {
                i64::try_from(v).ok()
            } else {
                number.as_f64().map(|v| v.round() as i64)
            }
        }
        Value::String(s) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().map(|v| v.round() as i64)),
        _ => None,
    }
}

impl McpServer {
    pub(crate) fn project_scope_summary(&self, project: Option<&str>) -> Option<ProjectScopeSummary> {
        let project = project?;
        if project == "*" {
            return None;
        }

        let params = json!({ "project": project });
        let total_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_slug = $project",
                &params,
            )
            .unwrap_or(0);
        let pending_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_slug = $project AND status = 'pending'",
                &params,
            )
            .unwrap_or(0);
        let indexing_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_slug = $project AND status = 'indexing'",
                &params,
            )
            .unwrap_or(0);
        let backlog_files = pending_files + indexing_files;
        let completed_files = (total_files - backlog_files).max(0);

        let reasons_res = self
            .graph_store
            .query_json_param(
                "SELECT COALESCE(status_reason, 'unknown'), count(*) \
                 FROM File \
                 WHERE project_slug = $project AND status IN ('pending', 'indexing') \
                 GROUP BY 1 \
                 ORDER BY count(*) DESC, 1 ASC \
                 LIMIT 3",
                &params,
            )
            .unwrap_or_else(|_| "[]".to_string());
        let pending_reasons = serde_json::from_str::<Vec<Vec<Value>>>(&reasons_res)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                let reason = row.first()?.as_str()?.to_string();
                let count = json_i64(row.get(1)?)?;
                Some((reason, count))
            })
            .collect();

        Some(ProjectScopeSummary {
            total_files,
            completed_files,
            pending_files,
            indexing_files,
            backlog_files,
            pending_reasons,
        })
    }

    pub(crate) fn project_scope_truth_note(&self, project: Option<&str>) -> Option<String> {
        let project = project?;
        let summary = self.project_scope_summary(Some(project))?;
        if summary.total_files <= 0 {
            return None;
        }

        let reason_note = if summary.pending_reasons.is_empty() {
            String::new()
        } else {
            let reasons = summary
                .pending_reasons
                .iter()
                .map(|(reason, count)| format!("`{reason}`: {count}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(" Causes backlog dominantes: {}.", reasons)
        };

        Some(format!(
            "**Completude du scope `{}`:** {}/{} fichiers termines; backlog visible {} (`pending`: {}, `indexing`: {}).{}\
\n",
            project,
            summary.completed_files,
            summary.total_files,
            summary.backlog_files,
            summary.pending_files,
            summary.indexing_files,
            reason_note
        ))
    }

    pub(crate) fn degraded_file_count(&self, project: Option<&str>) -> i64 {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT count(*) FROM File \
                 WHERE project_slug = $project AND status = 'indexed_degraded'",
                json!({ "project": project }),
            )
        } else {
            (
                "SELECT count(*) FROM File WHERE status = 'indexed_degraded'",
                json!({}),
            )
        };
        self.graph_store
            .query_count_param(query, &params)
            .unwrap_or(0)
    }

    pub(crate) fn degraded_symbol_count(&self, symbol: &str, project: Option<&str>) -> i64 {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT count(*) \
                 FROM File f \
                 JOIN CONTAINS c ON c.source_id = f.path \
                 JOIN Symbol s ON s.id = c.target_id \
                 WHERE (s.name = $sym OR s.id = $sym) \
                   AND s.project_slug = $project \
                   AND f.status = 'indexed_degraded'",
                json!({ "sym": symbol, "project": project }),
            )
        } else {
            (
                "SELECT count(*) \
                 FROM File f \
                 JOIN CONTAINS c ON c.source_id = f.path \
                 JOIN Symbol s ON s.id = c.target_id \
                 WHERE (s.name = $sym OR s.id = $sym) \
                   AND f.status = 'indexed_degraded'",
                json!({ "sym": symbol }),
            )
        };
        self.graph_store
            .query_count_param(query, &params)
            .unwrap_or(0)
    }

    pub(crate) fn degraded_truth_note(&self, degraded_files: i64) -> Option<String> {
        if degraded_files <= 0 {
            return None;
        }

        Some(format!(
            "**Etat:** verite partielle; {} fichier(s) du scope demande sont en `indexed_degraded` (`structure_only`). Les chunks, embeddings et aretes `CALLS` peuvent manquer.\n",
            degraded_files
        ))
    }

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

    fn symbol_search_predicate() -> &'static str {
        "lower(s.name) LIKE '%' || $normalized || '%' \
         OR lower(replace(replace(replace(s.name, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
         OR lower(s.name) LIKE '%' || $wildcard || '%' \
         OR lower(replace(replace(replace(replace(s.name, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%'"
    }

    fn chunk_search_predicate() -> &'static str {
        "lower(c.content) LIKE '%' || $normalized || '%' \
         OR lower(replace(replace(replace(c.content, '_', ' '), '-', ' '), ':', ' ')) LIKE '%' || $normalized || '%' \
         OR lower(c.content) LIKE '%' || $wildcard || '%' \
         OR lower(replace(replace(replace(replace(c.content, '_', ''), '-', ''), ':', ''), ' ', '')) LIKE '%' || $compact || '%' \
         OR lower(f.path) LIKE '%' || $wildcard || '%'"
    }

    fn chunk_docstring_match_expression() -> &'static str {
        "position('docstring:' in lower(c.content)) > 0 \
         AND position($normalized in lower(c.content)) > position('docstring:' in lower(c.content)) \
         AND (position('\n\n' in c.content) = 0 OR position($normalized in lower(c.content)) < position('\n\n' in c.content))"
    }

    fn chunk_body_match_expression() -> &'static str {
        "position('\n\n' in c.content) > 0 \
         AND position($normalized in lower(c.content)) > position('\n\n' in c.content)"
    }

    fn chunk_path_match_expression() -> &'static str {
        "lower(f.path) LIKE '%' || $wildcard || '%' \
         OR lower(f.path) LIKE '%' || $normalized || '%'"
    }

    pub(crate) fn axon_fs_read(&self, args: &Value) -> Option<Value> {
        let uri = args.get("uri")?.as_str()?;
        let start_line = args.get("start_line").and_then(|v| v.as_u64());
        let end_line = args.get("end_line").and_then(|v| v.as_u64());

        let file_path = std::path::Path::new(uri);
        if !file_path.exists() || !file_path.is_file() {
            return Some(
                json!({ "content": [{ "type": "text", "text": format!("Erreur: Le fichier '{}' n'existe pas ou n'est pas lisible.", uri) }], "isError": true }),
            );
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
                let report = format!(
                    "📄 L2 Detail : {}\n(Lignes {} à {} sur {})\n\n```\n{}\n```",
                    uri,
                    start + 1,
                    end,
                    total_lines,
                    sliced_content
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Erreur de lecture: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let degraded_note = self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)));

        let embedding = crate::embedder::batch_embed(vec![query_text.to_string()])
            .ok()
            .and_then(|v| v.into_iter().next());

        let base_predicate = Self::symbol_search_predicate();
        let (sql, params) = if let Some(emb) = embedding {
            let vec_str = format!("{:?}", emb);
            if project == "*" {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE {} \
                            OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5 \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, base_predicate, vec_str
                    ),
                    Self::build_symbol_search_params(query_text, project),
                )
            } else {
                (
                    format!(
                        "SELECT s.name, s.kind, f.path AS uri, array_cosine_distance(s.embedding, {}::FLOAT[384]) as score \
                         FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                         WHERE f.project_slug = $proj AND ( {} \
                            OR array_cosine_distance(s.embedding, {}::FLOAT[384]) < 0.5 \
                         ) \
                         ORDER BY score ASC LIMIT 10",
                        vec_str, base_predicate, vec_str
                    ),
                    Self::build_symbol_search_params(query_text, project),
                )
            }
        } else if project == "*" {
            (
                "SELECT s.name, s.kind, f.path AS uri \
                 FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                 WHERE {} \
                 LIMIT 10"
                    .replace("{}", base_predicate),
                Self::build_symbol_search_params(query_text, project),
            )
        } else {
            (
                "SELECT s.name, s.kind, f.path AS uri \
                 FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                 WHERE f.project_slug = $proj AND ( {} ) LIMIT 10"
                    .replace("{}", base_predicate),
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
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                if rows.is_empty() {
                    return self.axon_query_from_chunks(query_text, project, &params);
                }
                let headers = if sql.contains("score") {
                    vec!["Nom", "Type", "URI (Chemin)", "Distance Sémantique"]
                } else {
                    vec!["Nom", "Type", "URI (Chemin)"]
                };
                let table = format_table_from_json(&res, &headers);
                Some(
                    json!({ "content": [{ "type": "text", "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** {}\n\n{}{}{}",
                        query_text,
                        mode_label,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        table
                    ) }] }),
                )
            }
            Err(_) => self.axon_query_from_chunks(query_text, project, &params),
        }
    }

    fn axon_query_from_chunks(
        &self,
        query_text: &str,
        project: &str,
        params: &Value,
    ) -> Option<Value> {
        let predicate = Self::chunk_search_predicate();
        let docstring_match = Self::chunk_docstring_match_expression();
        let body_match = Self::chunk_body_match_expression();
        let path_match = Self::chunk_path_match_expression();
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let degraded_note = self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)));
        let sql = if project == "*" {
            format!(
                "WITH chunk_matches AS ( \
                    SELECT s.name, s.kind, f.path AS uri, \
                           CASE \
                               WHEN {docstring_match} THEN 'docstring' \
                               WHEN {body_match} THEN 'chunk body' \
                               WHEN {path_match} THEN 'file path' \
                               ELSE 'chunk metadata' \
                           END AS match_reason, \
                           CASE \
                               WHEN {docstring_match} THEN 0 \
                               WHEN {body_match} THEN 1 \
                               WHEN {path_match} THEN 3 \
                               ELSE 2 \
                           END AS match_rank, \
                           CASE \
                               WHEN {path_match} THEN f.path \
                               ELSE replace(replace(substr(c.content, 1, 220), '\n', ' '), '\r', ' ') \
                           END AS evidence \
                    FROM Chunk c \
                    JOIN Symbol s ON s.id = c.source_id \
                    JOIN CONTAINS rel ON rel.target_id = s.id \
                    JOIN File f ON f.path = rel.source_id \
                    WHERE {predicate} \
                 ) \
                 SELECT name, kind, uri, match_reason, evidence \
                 FROM chunk_matches \
                 ORDER BY match_rank ASC, uri ASC, name ASC \
                 LIMIT 10",
                docstring_match = docstring_match,
                body_match = body_match,
                path_match = path_match,
                predicate = predicate,
            )
        } else {
            format!(
                "WITH chunk_matches AS ( \
                    SELECT s.name, s.kind, f.path AS uri, \
                           CASE \
                               WHEN {docstring_match} THEN 'docstring' \
                               WHEN {body_match} THEN 'chunk body' \
                               WHEN {path_match} THEN 'file path' \
                               ELSE 'chunk metadata' \
                           END AS match_reason, \
                           CASE \
                               WHEN {docstring_match} THEN 0 \
                               WHEN {body_match} THEN 1 \
                               WHEN {path_match} THEN 3 \
                               ELSE 2 \
                           END AS match_rank, \
                           CASE \
                               WHEN {path_match} THEN f.path \
                               ELSE replace(replace(substr(c.content, 1, 220), '\n', ' '), '\r', ' ') \
                           END AS evidence \
                    FROM Chunk c \
                    JOIN Symbol s ON s.id = c.source_id \
                    JOIN CONTAINS rel ON rel.target_id = s.id \
                    JOIN File f ON f.path = rel.source_id \
                    WHERE c.project_slug = $proj AND ({predicate}) \
                 ) \
                 SELECT name, kind, uri, match_reason, evidence \
                 FROM chunk_matches \
                 ORDER BY match_rank ASC, uri ASC, name ASC \
                 LIMIT 10",
                docstring_match = docstring_match,
                body_match = body_match,
                path_match = path_match,
                predicate = predicate,
            )
        };

        match self.graph_store.query_json_param(&sql, params) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                if rows.is_empty() {
                    return self.axon_query_without_contains(query_text, project, params);
                }
                Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** fallback lexical sur chunk derive\n**Provenance:** chaque resultat precise sa source de match (`docstring`, `chunk body`, `chunk metadata`, `file path`) et reste ancre sur un fichier structurel.\n\n{}{}{}",
                            query_text,
                            project_note.unwrap_or_default(),
                            degraded_note.unwrap_or_default(),
                            format_table_from_json(&res, &["Nom", "Type", "URI (Chemin)", "Why it matched", "Evidence"])
                        )
                    }]
                }))
            }
            Err(_) => self.axon_query_without_contains(query_text, project, params),
        }
    }

    fn axon_query_without_contains(
        &self,
        query_text: &str,
        project: &str,
        params: &Value,
    ) -> Option<Value> {
        let degraded_files = self.degraded_file_count((project != "*").then_some(project));
        let degraded_note = self.degraded_truth_note(degraded_files);
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let contains_count = self
            .graph_store
            .query_count("SELECT count(*) FROM CONTAINS")
            .unwrap_or(0);
        if contains_count > 0 {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** structurel\n\n{}{}{}\n",
                        query_text,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        "Aucun résultat trouvé."
                    )
                }]
            }));
        }

        let fallback_query = format!(
            "SELECT s.name, s.kind, COALESCE(s.project_slug, 'unknown') \
             FROM Symbol s \
             WHERE {} \
             LIMIT 10",
            Self::symbol_search_predicate()
        );
        let fallback_res = self
            .graph_store
            .query_json_param(&fallback_query, params)
            .unwrap_or_else(|_| "[]".to_string());
        let fallback_rows: Vec<Vec<Value>> =
            serde_json::from_str(&fallback_res).unwrap_or_default();

        if fallback_rows.is_empty() {
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** degrade structurel sans ancrage fichier\n\n{}{}{}\n",
                        query_text,
                        project_note.unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        "Aucun résultat trouvé."
                    )
                }]
            }))
        } else {
            let project_note = if project == "*" {
                "portee projet non contrainte"
            } else {
                "contrainte projet non fiable tant que CONTAINS est vide"
            };
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** degrade structurel sans ancrage fichier\n**Etat:** le graphe de containment n'est pas encore disponible; les symboles ci-dessous restent exploitables mais sans URI verifiee ({})\n{}{}\n{}",
                        query_text,
                        project_note,
                        self.project_scope_truth_note((project != "*").then_some(project))
                            .unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        format_table_from_json(&fallback_res, &["Nom", "Type", "Projet"])
                    )
                }]
            }))
        }
    }

    pub(crate) fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str());
        let (query, params) = if let Some(project) = project {
            (
                "SELECT s.name, s.kind, s.tested, \
                 (SELECT count(*) FROM CALLS c1 WHERE c1.target_id = s.id) AS callers, \
                 (SELECT count(*) FROM CALLS c2 WHERE c2.source_id = s.id) AS callees \
                 FROM Symbol s \
                 WHERE s.name = $sym AND s.project_slug = $project",
                json!({"sym": symbol, "project": project}),
            )
        } else {
            (
                "SELECT s.name, s.kind, s.tested, \
                 (SELECT count(*) FROM CALLS c1 WHERE c1.target_id = s.id) AS callers, \
                 (SELECT count(*) FROM CALLS c2 WHERE c2.source_id = s.id) AS callees \
                 FROM Symbol s WHERE s.name = $sym",
                json!({"sym": symbol}),
            )
        };
        let degraded_note =
            self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        match self.graph_store.query_json_param(query, &params) {
            Ok(res) => {
                let table =
                    format_table_from_json(&res, &["Nom", "Type", "Testé", "Appelants", "Appelés"]);
                Some(
                    json!({ "content": [{ "type": "text", "text": format!(
                        "### 🔍 Inspection du Symbole : {}\n\n{}{}{}",
                        symbol,
                        project_note.unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        table
                    ) }] }),
                )
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
        let up_res = self
            .graph_store
            .query_json_param(&up_query, &params)
            .unwrap_or_else(|_| "[]".to_string());
        let down_res = self
            .graph_store
            .query_json_param(&down_query, &params)
            .unwrap_or_else(|_| "[]".to_string());

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

        match self
            .graph_store
            .query_json_param(query, &json!({"sym": symbol}))
        {
            Ok(res) => {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                if rows.is_empty() {
                    Some(
                        json!({ "content": [{ "type": "text", "text": format!("✅ Aucun consommateur externe détecté pour '{}'.", symbol) }] }),
                    )
                } else {
                    Some(
                        json!({ "content": [{ "type": "text", "text": format!("⚠️ **RISQUE DE RUPTURE D'API**\n\nModifier '{}' impactera directement les consommateurs suivants :\n\n{}", symbol, format_table_from_json(&res, &["Consommateur", "Symbole", "Type"])) }] }),
                    )
                }
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true }),
            ),
        }
    }
}
