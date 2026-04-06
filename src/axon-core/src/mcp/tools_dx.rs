use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryIntent {
    Generic,
    ConfigLookupExact,
}

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
    pub(crate) fn project_scope_summary(
        &self,
        project: Option<&str>,
    ) -> Option<ProjectScopeSummary> {
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

    fn resolve_scoped_symbol_id_dx(&self, symbol: &str, project: Option<&str>) -> Option<String> {
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

    fn classify_query_intent(query_text: &str) -> QueryIntent {
        let trimmed = query_text.trim();
        let token_count = trimmed.split_whitespace().count();
        let dot_count = trimmed.matches('.').count();
        let looks_structured = !trimmed.is_empty()
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-' | '/'));
        if token_count == 1 && dot_count >= 2 && looks_structured {
            QueryIntent::ConfigLookupExact
        } else {
            QueryIntent::Generic
        }
    }

    fn query_intent_label(intent: QueryIntent) -> &'static str {
        match intent {
            QueryIntent::Generic => "generic",
            QueryIntent::ConfigLookupExact => "config_lookup_exact",
        }
    }

    fn is_operational_file_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.ends_with("/mix.exs")
            || lower.ends_with("/mix.lock")
            || lower.ends_with("devenv.yaml")
            || lower.ends_with("devenv.nix")
            || lower.ends_with(".exs")
            || lower.ends_with(".yml")
            || lower.ends_with(".yaml")
            || lower.ends_with(".json")
            || lower.ends_with(".toml")
            || lower.contains("/config/")
            || lower.contains("/.github/workflows/")
            || lower.contains("docker-compose")
    }

    fn is_documentary_file_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        lower.ends_with(".md")
            || lower.contains("/docs/")
            || lower.contains("/plans/")
            || lower.contains("audit")
            || lower.ends_with("readme.md")
    }

    fn result_category_for_path(path: &str) -> &'static str {
        if Self::is_operational_file_path(path) {
            "source operatoire"
        } else if Self::is_documentary_file_path(path) {
            "documentaire"
        } else {
            "code general"
        }
    }

    fn query_diagnostic_block(
        intent: QueryIntent,
        query_path: &str,
        result_category: &str,
        semantic_fallback_reason: Option<&str>,
    ) -> String {
        let fallback = semantic_fallback_reason
            .map(|reason| format!("**Fallback semantique:** {}\n", reason))
            .unwrap_or_default();
        format!(
            "**Type de resultat:** {}\n**Diagnostic:** query_intent={} ; query_path={}\n{}\n",
            result_category,
            Self::query_intent_label(intent),
            query_path,
            fallback
        )
    }

    fn exact_match_rank(value: Option<&str>, query_lower: &str) -> usize {
        let Some(value) = value else {
            return 2;
        };
        let value_lower = value.to_ascii_lowercase();
        if value_lower == query_lower {
            0
        } else if value_lower.contains(query_lower) {
            1
        } else {
            2
        }
    }

    fn operational_rank(path: &str) -> usize {
        if Self::is_operational_file_path(path) {
            0
        } else if Self::is_documentary_file_path(path) {
            2
        } else {
            1
        }
    }

    fn rerank_symbol_rows(
        rows: Vec<Vec<Value>>,
        query_text: &str,
        intent: QueryIntent,
    ) -> Vec<Vec<Value>> {
        if intent != QueryIntent::ConfigLookupExact {
            return rows;
        }

        let query_lower = query_text.to_ascii_lowercase();
        let mut indexed = rows.into_iter().enumerate().collect::<Vec<_>>();
        indexed.sort_by_key(|(original_index, row)| {
            let name = row.first().and_then(Value::as_str).unwrap_or_default();
            let uri = row.get(2).and_then(Value::as_str).unwrap_or_default();
            (
                Self::operational_rank(uri),
                Self::exact_match_rank(Some(name), &query_lower),
                Self::exact_match_rank(Some(uri), &query_lower),
                uri.len(),
                uri.to_ascii_lowercase(),
                *original_index,
            )
        });
        indexed.into_iter().map(|(_, row)| row).collect()
    }

    fn chunk_match_rank(reason: &str) -> usize {
        match reason {
            "docstring" => 0,
            "chunk body" => 1,
            "chunk metadata" => 2,
            "file path" => 3,
            _ => 4,
        }
    }

    fn rerank_chunk_rows(
        rows: Vec<Vec<Value>>,
        query_text: &str,
        intent: QueryIntent,
    ) -> Vec<Vec<Value>> {
        if intent != QueryIntent::ConfigLookupExact {
            return rows;
        }

        let query_lower = query_text.to_ascii_lowercase();
        let mut indexed = rows.into_iter().enumerate().collect::<Vec<_>>();
        indexed.sort_by_key(|(original_index, row)| {
            let uri = row.get(2).and_then(Value::as_str).unwrap_or_default();
            let match_reason = row.get(3).and_then(Value::as_str).unwrap_or_default();
            let evidence = row.get(4).and_then(Value::as_str).unwrap_or_default();
            (
                Self::operational_rank(uri),
                Self::exact_match_rank(Some(evidence), &query_lower),
                Self::exact_match_rank(Some(uri), &query_lower),
                Self::chunk_match_rank(match_reason),
                uri.len(),
                uri.to_ascii_lowercase(),
                *original_index,
            )
        });
        indexed.into_iter().map(|(_, row)| row).collect()
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
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let query_intent = Self::classify_query_intent(query_text);
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let degraded_note =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)));

        let embedding_attempt = crate::embedder::batch_embed(vec![query_text.to_string()]);
        let semantic_fallback_reason = embedding_attempt.as_ref().err().map(|err| err.to_string());
        let embedding = embedding_attempt.ok().and_then(|v| v.into_iter().next());
        let query_limit = if query_intent == QueryIntent::ConfigLookupExact {
            25
        } else {
            10
        };

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
                         ORDER BY score ASC LIMIT {}",
                        vec_str, base_predicate, vec_str, query_limit
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
                         ORDER BY score ASC LIMIT {}",
                        vec_str, base_predicate, vec_str, query_limit
                    ),
                    Self::build_symbol_search_params(query_text, project),
                )
            }
        } else if project == "*" {
            (
                format!(
                    "SELECT s.name, s.kind, f.path AS uri \
                     FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                     WHERE {} \
                     LIMIT {}",
                    base_predicate, query_limit
                ),
                Self::build_symbol_search_params(query_text, project),
            )
        } else {
            (
                format!(
                    "SELECT s.name, s.kind, f.path AS uri \
                     FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                     WHERE f.project_slug = $proj AND ( {} ) LIMIT {}",
                    base_predicate, query_limit
                ),
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
                let rows: Vec<Vec<Value>> =
                    Self::rerank_symbol_rows(serde_json::from_str(&res).unwrap_or_default(), query_text, query_intent);
                if rows.is_empty() {
                    return self.axon_query_from_chunks(
                        query_text,
                        project,
                        &params,
                        query_intent,
                        semantic_fallback_reason.as_deref(),
                    );
                }
                let headers = if sql.contains("score") {
                    vec!["Nom", "Type", "URI (Chemin)", "Distance Sémantique"]
                } else {
                    vec!["Nom", "Type", "URI (Chemin)"]
                };
                let table_json = serde_json::to_string(&rows).unwrap_or(res);
                let table = format_table_from_json(&table_json, &headers);
                let scope = if project == "*" {
                    "workspace:*".to_string()
                } else {
                    format!("project:{}", project)
                };
                let result_category = rows
                    .first()
                    .and_then(|row| row.get(2))
                    .and_then(Value::as_str)
                    .map(Self::result_category_for_path)
                    .unwrap_or("inconnu");
                let diagnostic = Self::query_diagnostic_block(
                    query_intent,
                    if sql.contains("score") {
                        "symbol_index_semantic"
                    } else {
                        "symbol_index_structural"
                    },
                    result_category,
                    semantic_fallback_reason.as_deref(),
                );
                let evidence = format!(
                    "**Mode:** {}\n{}\n{}{}{}",
                    mode_label,
                    diagnostic,
                    project_note.clone().unwrap_or_default(),
                    degraded_note.clone().unwrap_or_default(),
                    table
                );
                let evidence = evidence_by_mode(&evidence, mode);
                let report = format!(
                    "### 🔎 Resultats de recherche : '{}'\n\n{}",
                    query_text,
                    format_standard_contract(
                        "ok",
                        "semantic query resolved",
                        &scope,
                        &evidence,
                        &["use `inspect` on a returned symbol", "use `impact` for blast radius"],
                        "high",
                    )
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(_) => self.axon_query_from_chunks(
                query_text,
                project,
                &params,
                query_intent,
                semantic_fallback_reason.as_deref(),
            ),
        }
    }

    fn axon_query_from_chunks(
        &self,
        query_text: &str,
        project: &str,
        params: &Value,
        query_intent: QueryIntent,
        semantic_fallback_reason: Option<&str>,
    ) -> Option<Value> {
        let predicate = Self::chunk_search_predicate();
        let docstring_match = Self::chunk_docstring_match_expression();
        let body_match = Self::chunk_body_match_expression();
        let path_match = Self::chunk_path_match_expression();
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let degraded_note =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)));
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
                 LIMIT {limit}",
                docstring_match = docstring_match,
                body_match = body_match,
                path_match = path_match,
                predicate = predicate,
                limit = if query_intent == QueryIntent::ConfigLookupExact {
                    25
                } else {
                    10
                },
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
                 LIMIT {limit}",
                docstring_match = docstring_match,
                body_match = body_match,
                path_match = path_match,
                predicate = predicate,
                limit = if query_intent == QueryIntent::ConfigLookupExact {
                    25
                } else {
                    10
                },
            )
        };

        match self.graph_store.query_json_param(&sql, params) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> =
                    Self::rerank_chunk_rows(serde_json::from_str(&res).unwrap_or_default(), query_text, query_intent);
                if rows.is_empty() {
                    return self.axon_query_without_contains(
                        query_text,
                        project,
                        params,
                        query_intent,
                        semantic_fallback_reason,
                    );
                }
                let result_category = rows
                    .first()
                    .and_then(|row| row.get(2))
                    .and_then(Value::as_str)
                    .map(Self::result_category_for_path)
                    .unwrap_or("inconnu");
                let diagnostic = Self::query_diagnostic_block(
                    query_intent,
                    "chunk_fallback",
                    result_category,
                    semantic_fallback_reason,
                );
                let table_json = serde_json::to_string(&rows).unwrap_or(res);
                Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** fallback lexical sur chunk derive\n{}\n**Provenance:** chaque resultat precise sa source de match (`docstring`, `chunk body`, `chunk metadata`, `file path`) et reste ancre sur un fichier structurel.\n\n{}{}{}",
                            query_text,
                            diagnostic,
                            project_note.unwrap_or_default(),
                            degraded_note.unwrap_or_default(),
                            format_table_from_json(&table_json, &["Nom", "Type", "URI (Chemin)", "Why it matched", "Evidence"])
                        )
                    }]
                }))
            }
            Err(_) => self.axon_query_without_contains(
                query_text,
                project,
                params,
                query_intent,
                semantic_fallback_reason,
            ),
        }
    }

    fn axon_query_without_contains(
        &self,
        query_text: &str,
        project: &str,
        params: &Value,
        query_intent: QueryIntent,
        semantic_fallback_reason: Option<&str>,
    ) -> Option<Value> {
        let degraded_files = self.degraded_file_count((project != "*").then_some(project));
        let degraded_note = self.degraded_truth_note(degraded_files);
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let contains_count = self
            .graph_store
            .query_count("SELECT count(*) FROM CONTAINS")
            .unwrap_or(0);
        println!("axon_query_without_contains: contains_count={} in DB {:?}", contains_count, self.graph_store.db_path);
        if contains_count > 0 {
            let diagnostic =
                Self::query_diagnostic_block(query_intent, "structure_only_empty", "aucun", semantic_fallback_reason);
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** structurel\n{}\n{}{}{}\n",
                        query_text,
                        diagnostic,
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
            let diagnostic =
                Self::query_diagnostic_block(query_intent, "structure_only_empty", "aucun", semantic_fallback_reason);
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** degrade structurel sans ancrage fichier\n{}\n{}{}{}\n",
                        query_text,
                        diagnostic,
                        project_note.unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        "Aucun résultat trouvé."
                    )
                }]
            }))
        } else {
            let rows = Self::rerank_symbol_rows(fallback_rows, query_text, query_intent);
            let result_category = rows
                .first()
                .and_then(|row| row.get(2))
                .and_then(Value::as_str)
                .map(Self::result_category_for_path)
                .unwrap_or("inconnu");
            let diagnostic = Self::query_diagnostic_block(
                query_intent,
                "structure_only_unanchored",
                result_category,
                semantic_fallback_reason,
            );
            let project_note = if project == "*" {
                "portee projet non contrainte"
            } else {
                "contrainte projet non fiable tant que CONTAINS est vide"
            };
            let table_json = serde_json::to_string(&rows).unwrap_or(fallback_res);
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### 🔎 Resultats de recherche : '{}'\n\n**Mode:** degrade structurel sans ancrage fichier\n{}\n**Etat:** le graphe de containment n'est pas encore disponible; les symboles ci-dessous restent exploitables mais sans URI verifiee ({})\n{}{}\n{}",
                        query_text,
                        diagnostic,
                        project_note,
                        self.project_scope_truth_note((project != "*").then_some(project))
                            .unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        format_table_from_json(&table_json, &["Nom", "Type", "Projet"])
                    )
                }]
            }))
        }
    }

    pub(crate) fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
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
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        match self.graph_store.query_json_param(query, &params) {
            Ok(res) => {
                let table =
                    format_table_from_json(&res, &["Nom", "Type", "Testé", "Appelants", "Appelés"]);
                let scope = project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string());
                let evidence = format!(
                    "{}{}{}",
                    project_note.unwrap_or_default(),
                    degraded_note.unwrap_or_default(),
                    table
                );
                let evidence = evidence_by_mode(&evidence, mode);
                let report = format!(
                    "### 🔍 Inspection du Symbole : {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "symbol inspection computed",
                        &scope,
                        &evidence,
                        &["run `impact` for dependency blast radius", "run `bidi_trace` for topology"],
                        "high",
                    )
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(_) => None,
        }
    }

    pub(crate) fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(100);
        let scope = project
            .map(|p| format!("project:{}", p))
            .unwrap_or_else(|| "workspace:*".to_string());
        let Some(target_id) = self.resolve_scoped_symbol_id_dx(symbol, project) else {
            let (sugg_query, sugg_params) = if let Some(project) = project {
                (
                    "SELECT name, kind, project_slug \
                     FROM Symbol \
                     WHERE project_slug = $project AND lower(name) LIKE lower($pat) \
                     ORDER BY name \
                     LIMIT 8",
                    json!({ "project": project, "pat": format!("%{}%", symbol) }),
                )
            } else {
                (
                    "SELECT name, kind, COALESCE(project_slug, 'unknown') \
                     FROM Symbol \
                     WHERE lower(name) LIKE lower($pat) \
                     ORDER BY name \
                     LIMIT 8",
                    json!({ "pat": format!("%{}%", symbol) }),
                )
            };
            let suggestions = self
                .graph_store
                .query_json_param(sugg_query, &sugg_params)
                .unwrap_or_else(|_| "[]".to_string());
            let evidence = format!(
                "{}{}",
                self.project_scope_truth_note(project).unwrap_or_default(),
                format_table_from_json(&suggestions, &["Symbole suggéré", "Type", "Projet"])
            );
            let report = format!(
                "## ↕️ Trace Bidirectionnelle : {}\n\n{}",
                symbol,
                format_standard_contract(
                    "warn_input_not_found",
                    "symbol not found in current scope",
                    &scope,
                    &evidence_by_mode(&evidence, mode),
                    &["pick one suggested symbol", "or pass the exact symbol id"],
                    "low",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        };

        let up_query = format!(
            "WITH RECURSIVE callers(sym, depth) AS (
                SELECT source_id, 1 FROM CALLS WHERE target_id = $target_id
                UNION ALL
                SELECT c.source_id, callers.depth + 1
                FROM CALLS c
                JOIN callers ON c.target_id = callers.sym
                WHERE callers.depth < {}
            )
            SELECT DISTINCT s.name, s.kind, COALESCE(s.project_slug, 'unknown') FROM callers
            JOIN Symbol s ON s.id = callers.sym{}",
            depth,
            if project.is_some() {
                " WHERE s.project_slug = $project"
            } else {
                ""
            }
        );

        let down_query = format!(
            "WITH RECURSIVE callees(sym, depth) AS (
                SELECT target_id, 1 FROM CALLS WHERE source_id = $target_id
                UNION ALL
                SELECT c.target_id, callees.depth + 1
                FROM CALLS c
                JOIN callees ON c.source_id = callees.sym
                WHERE callees.depth < {}
            )
            SELECT DISTINCT s.name, s.kind, COALESCE(s.project_slug, 'unknown') FROM callees
            JOIN Symbol s ON s.id = callees.sym{}",
            depth,
            if project.is_some() {
                " WHERE s.project_slug = $project"
            } else {
                ""
            }
        );

        let params = if let Some(project) = project {
            json!({"target_id": target_id, "project": project})
        } else {
            json!({"target_id": target_id})
        };
        let up_res = self
            .graph_store
            .query_json_param(&up_query, &params)
            .unwrap_or_else(|_| "[]".to_string());
        let down_res = self
            .graph_store
            .query_json_param(&down_query, &params)
            .unwrap_or_else(|_| "[]".to_string());

        let up_rows: Vec<Vec<Value>> = serde_json::from_str(&up_res).unwrap_or_default();
        let down_rows: Vec<Vec<Value>> = serde_json::from_str(&down_res).unwrap_or_default();
        let status = if up_rows.is_empty() && down_rows.is_empty() {
            "warn_empty_result"
        } else {
            "ok"
        };
        let confidence = if up_rows.len() + down_rows.len() >= 5 {
            "high"
        } else if up_rows.is_empty() && down_rows.is_empty() {
            "low"
        } else {
            "medium"
        };
        let mut evidence = String::new();
        if let Some(note) = self.project_scope_truth_note(project) {
            evidence.push_str(&note);
            evidence.push('\n');
        }
        if let Some(note) = self.degraded_truth_note(self.degraded_symbol_count(symbol, project)) {
            evidence.push_str(&note);
            evidence.push('\n');
        }
        evidence.push_str("### ↑ Appelants / Entry Points\n");
        evidence.push_str(&format_table_from_json(&up_res, &["Nom", "Type", "Projet"]));
        evidence.push_str("\n\n### ↓ Appels Profonds\n");
        evidence.push_str(&format_table_from_json(&down_res, &["Nom", "Type", "Projet"]));

        let report = format!(
            "## ↕️ Trace Bidirectionnelle : {}\n\n{}",
            symbol,
            format_standard_contract(
                status,
                "bidirectional call trace computed",
                &scope,
                &evidence_by_mode(&evidence, mode),
                &["run `impact` for blast-radius summary", "run `inspect` on one critical neighbor"],
                confidence,
            )
        );

        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_api_break_check(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let scope = project
            .map(|p| format!("project:{}", p))
            .unwrap_or_else(|| "workspace:*".to_string());
        let Some(target_id) = self.resolve_scoped_symbol_id_dx(symbol, project) else {
            let report = format!(
                "## 🧯 Vérification rupture API : {}\n\n{}",
                symbol,
                format_standard_contract(
                    "warn_input_not_found",
                    "symbol not found in current scope",
                    &scope,
                    "",
                    &["run `query` to discover the exact symbol id/name", "retry with `project` when relevant"],
                    "low",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        };

        let query = if project.is_some() {
            "
            SELECT DISTINCT f.path AS consumer, s.name, s.kind, COALESCE(s.project_slug, 'unknown')
            FROM CALLS c
            JOIN Symbol s ON s.id = c.source_id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id
            JOIN Symbol target ON target.id = c.target_id
            WHERE target.id = $target_id AND target.is_public = true AND s.project_slug = $project
        "
        } else {
            "
            SELECT DISTINCT f.path AS consumer, s.name, s.kind, COALESCE(s.project_slug, 'unknown')
            FROM CALLS c
            JOIN Symbol s ON s.id = c.source_id
            LEFT JOIN CONTAINS con ON s.id = con.target_id
            LEFT JOIN File f ON f.path = con.source_id
            JOIN Symbol target ON target.id = c.target_id
            WHERE target.id = $target_id AND target.is_public = true
        "
        };
        let params = if let Some(project) = project {
            json!({ "target_id": target_id, "project": project })
        } else {
            json!({ "target_id": target_id })
        };

        match self.graph_store.query_json_param(query, &params) {
            Ok(res) => {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
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
                if rows.is_empty() {
                    let report = format!(
                        "## 🧯 Vérification rupture API : {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "ok",
                            "no external consumers detected for the resolved public symbol",
                            &scope,
                            &evidence_by_mode(&evidence, mode),
                            &["run `impact` for broader dependency view"],
                            "high",
                        )
                    );
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                } else {
                    evidence.push_str(
                        "Modifier ce symbole public impactera directement les consommateurs suivants:\n\n",
                    );
                    evidence.push_str(&format_table_from_json(
                        &res,
                        &["Consommateur", "Symbole", "Type", "Projet"],
                    ));
                    let report = format!(
                        "## 🧯 Vérification rupture API : {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "warn_api_break_risk",
                            "public api consumer impact detected",
                            &scope,
                            &evidence_by_mode(&evidence, mode),
                            &["inspect top consumers", "run `simulate_mutation` before changing signature"],
                            "high",
                        )
                    );
                    Some(json!({ "content": [{ "type": "text", "text": report }] }))
                }
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true }),
            ),
        }
    }
}
