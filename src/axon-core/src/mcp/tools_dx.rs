use crate::ist_snapshot::process_view;
use crate::service_guard::{self, ServicePressure};
use serde_json::{json, Value};
use std::collections::HashSet;

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use super::{GuidanceCandidates, GuidanceFact};

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

/// REQ-AXO-91511 — materialize IST symbol ids into the JSON row-of-row
/// format `format_table_from_json` consumes (`[[name, kind, project], ...]`).
/// One round-trip on public.Symbol for display ; the BFS itself already
/// ran in RAM via IstGraphView. Returns `"[]"` when ids is empty so the
/// downstream string parser is happy.
fn materialize_symbol_rows(server: &super::McpServer, ids: &[String]) -> String {
    if ids.is_empty() {
        return "[]".to_string();
    }
    let escaped: Vec<String> = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    let sql = format!(
        "SELECT name, kind, COALESCE(project_code, 'unknown') \
         FROM public.Symbol WHERE id IN ({})",
        escaped.join(", ")
    );
    server
        .graph_store
        .query_json(&sql)
        .unwrap_or_else(|_| "[]".to_string())
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
    fn canonical_source_names(canonical_sources: Option<&Value>) -> Vec<String> {
        canonical_sources
            .and_then(Value::as_object)
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn exact_candidate_missing(rows: &[Vec<Value>], requested: &str, intent: QueryIntent) -> bool {
        if intent != QueryIntent::ConfigLookupExact {
            return false;
        }
        let requested = requested.trim().to_ascii_lowercase();
        !rows.iter().any(|row| {
            row.first()
                .and_then(Value::as_str)
                .map(|name| name.trim().eq_ignore_ascii_case(&requested))
                .unwrap_or(false)
        })
    }

    pub(crate) fn extract_query_guidance_facts(
        &self,
        query_text: &str,
        project: Option<&str>,
        candidates: &GuidanceCandidates,
        degraded_file_count: i64,
        vectorization_incomplete: bool,
        exact_match_missing: bool,
        backend_pressure: bool,
    ) -> Vec<GuidanceFact> {
        let mut facts = vec![GuidanceFact::requested_target(query_text)];
        if let Some(project_code) = project {
            facts.push(GuidanceFact::resolved_project_scope(project_code));
        }

        for symbol in &candidates.symbols {
            facts.push(GuidanceFact::candidate_symbol(symbol.clone()));
        }
        for code in &candidates.project_codes {
            facts.push(GuidanceFact::candidate_project_code(code.clone()));
        }
        for source in &candidates.canonical_sources {
            facts.push(GuidanceFact::canonical_source(source.clone()));
        }

        if degraded_file_count > 0 {
            facts.push(GuidanceFact::IndexIncomplete);
            facts.push(GuidanceFact::result_degraded("index_partial"));
        }
        if vectorization_incomplete {
            facts.push(GuidanceFact::VectorizationIncomplete);
        }
        if backend_pressure {
            facts.push(GuidanceFact::problem_signal("backend_pressure"));
        }

        if let Some(project_code) = project {
            if !candidates.project_codes.is_empty()
                && !candidates
                    .project_codes
                    .iter()
                    .any(|code| code == project_code)
            {
                facts.push(GuidanceFact::problem_signal("wrong_project_scope"));
                return facts;
            }
        }

        if candidates.project_codes.len() > 1 {
            facts.push(GuidanceFact::problem_signal("input_ambiguous"));
        } else if exact_match_missing && !candidates.symbols.is_empty() {
            facts.push(GuidanceFact::problem_signal("input_not_found"));
        }

        facts
    }

    pub(crate) fn extract_inspect_guidance_facts(
        &self,
        symbol: &str,
        project: Option<&str>,
        candidates: &GuidanceCandidates,
        degraded_symbol_count: i64,
        exact_match_missing: bool,
        backend_pressure: bool,
    ) -> Vec<GuidanceFact> {
        let mut facts = self.extract_query_guidance_facts(
            symbol,
            project,
            candidates,
            degraded_symbol_count,
            false,
            exact_match_missing,
            backend_pressure,
        );
        if degraded_symbol_count > 0 {
            facts.push(GuidanceFact::result_degraded("symbol_partial"));
        }
        facts
    }

    pub(crate) fn project_scope_summary(
        &self,
        project: Option<&str>,
    ) -> Option<ProjectScopeSummary> {
        let project = project?;
        if project == "*" {
            return None;
        }

        // Post-CPT-AXO-039 supersedure (2026-05-08): IST tables are
        // multi-project under both backends — same SQL on PG and DuckDB.
        let params = json!({ "project": project });
        let total_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_code = $project",
                &params,
            )
            .unwrap_or(0);
        let pending_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_code = $project AND status = 'pending'",
                &params,
            )
            .unwrap_or(0);
        let indexing_files = self
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_code = $project AND status = 'indexing'",
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
                 WHERE project_code = $project AND status IN ('pending', 'indexing') \
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
            format!(" Top backlog causes: {}.", reasons)
        };

        Some(format!(
            "**Scope completeness `{}`:** {}/{} files completed; visible backlog {} (`pending`: {}, `indexing`: {}).{}\
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
        // Post-CPT-AXO-039 supersedure (2026-05-08): same SQL on PG and
        // DuckDB — multi-project tables filter via `project_code` row
        // column, not a per-project schema namespace.
        let (query, params) = if let Some(project) = project {
            (
                "SELECT count(*) FROM File \
                 WHERE project_code = $project AND status = 'indexed_degraded'",
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
                   AND s.project_code = $project \
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
            "**State:** partial truth; {} file(s) in requested scope are `indexed_degraded` (`structure_only`). Chunks, embeddings, and `CALLS` edges may be missing.\n",
            degraded_files
        ))
    }

    fn resolve_scoped_symbol_id_dx(&self, symbol: &str, project: Option<&str>) -> Option<String> {
        self.resolve_scoped_symbol_id_canonical(symbol, project)
    }

    fn build_symbol_search_params(query_text: &str, project: &str) -> Value {
        // REQ-AXO-088 — `_` belongs in the wildcard separator set, not
        // just in the compact set. Without it, a query like
        // `reserve_budget` was treated as a single literal token and
        // never matched `reserve_memory_budget` even though the LIKE
        // wildcard branch was supposed to handle exactly this case.
        // Including `_` here makes the wildcard form `reserve%budget`,
        // which matches the underscore-separated symbol via DuckDB
        // LIKE. The compact branch already strips `_` so it stays
        // unchanged.
        let normalized_query = query_text.to_lowercase();
        let wildcard_query = normalized_query.replace([' ', '-', ':', '_'], "%");
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
            "operational source"
        } else if Self::is_documentary_file_path(path) {
            "documentary"
        } else {
            "general code"
        }
    }

    fn query_diagnostic_block(
        intent: QueryIntent,
        query_path: &str,
        result_category: &str,
        semantic_fallback_reason: Option<&str>,
    ) -> String {
        let fallback = semantic_fallback_reason
            .map(|reason| format!("**Semantic fallback:** {}\n", reason))
            .unwrap_or_default();
        format!(
            "**Result type:** {}\n**Diagnostic:** query_intent={} ; query_path={}\n{}\n",
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
                json!({ "content": [{ "type": "text", "text": format!("Error: file '{}' does not exist or is not readable.", uri) }], "isError": true }),
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
                    "L2 Detail: {}\n(Lines {} to {} of {})\n\n```\n{}\n```",
                    uri,
                    start + 1,
                    end,
                    total_lines,
                    sliced_content
                );
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("Read error: {}", e) }], "isError": true }),
            ),
        }
    }

    /// REQ-AXO-91508 — graph r=1 neighbor expansion lane (single-lookup
    /// category per CPT-AXO-90007). Given the set of direct-hit symbol
    /// names from the symbol_index lane, look up their canonical ids
    /// then emit one-hop CALLS / CONTAINS / CALLS_NIF neighbors as
    /// supplementary `graph_r1` hits. Best-effort : if the lookup
    /// fails, returns an empty vec and the caller falls back to
    /// symbol-only results.
    pub(crate) fn query_graph_r1_neighbors(
        &self,
        direct_names: &HashSet<String>,
        project: &str,
        limit: usize,
    ) -> Vec<Value> {
        if direct_names.is_empty() || project == "*" {
            return Vec::new();
        }
        // SQL-escape names + project. Identifiers come from the bench
        // dataset / LLM input ; treat as untrusted.
        let names_sql = direct_names
            .iter()
            .map(|n| format!("'{}'", n.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let safe_project = project.replace('\'', "''");
        let sql = format!(
            "WITH anchors AS ( \
                SELECT id FROM public.Symbol \
                WHERE project_code = '{safe_project}' AND name IN ({names_sql}) \
             ), \
             neighbor_edges AS ( \
                SELECT e.target_id AS nid FROM public.Edge e \
                JOIN anchors a ON a.id = e.source_id \
                WHERE e.project_code = '{safe_project}' \
                  AND e.relation_type IN ('CALLS', 'CALLS_NIF', 'CONTAINS') \
                UNION \
                SELECT e.source_id AS nid FROM public.Edge e \
                JOIN anchors a ON a.id = e.target_id \
                WHERE e.project_code = '{safe_project}' \
                  AND e.relation_type IN ('CALLS', 'CALLS_NIF', 'CONTAINS') \
             ) \
             SELECT DISTINCT s.name, COALESCE(s.kind, '') AS kind, \
                    COALESCE((SELECT c.file_path FROM public.Chunk c \
                              WHERE c.source_id = s.id LIMIT 1), '') AS uri \
             FROM public.Symbol s \
             JOIN neighbor_edges n ON n.nid = s.id \
             WHERE s.project_code = '{safe_project}' \
               AND s.name NOT IN ({names_sql}) \
             LIMIT {limit}"
        );
        match self.graph_store.query_json(&sql) {
            Ok(json_str) => serde_json::from_str::<Vec<Vec<Value>>>(&json_str)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|row| {
                    let name = row.first().and_then(Value::as_str)?;
                    if name.is_empty() {
                        return None;
                    }
                    let kind = row.get(1).and_then(Value::as_str).unwrap_or("");
                    let uri = row.get(2).and_then(Value::as_str).unwrap_or("");
                    Some(json!({
                        "name": name,
                        "kind": kind,
                        "uri": uri,
                        "surface": "graph_r1",
                        "project": project,
                    }))
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn axon_query(&self, args: &Value) -> Option<Value> {
        let query_text = args.get("query")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        // REQ-AXO-089 — extend cwd auto-resolution from retrieve_context
        // to query: when the caller omits `project`, try AXON_PROJECT_ROOT
        // or current_dir against the registry. Exact one match returns
        // the code; otherwise fall back to workspace:* as before. The
        // `auto_project` String must outlive `project` because `project`
        // borrows from it via `as_deref`.
        let explicit_project = args.get("project").and_then(|v| v.as_str());
        let auto_project = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project = explicit_project
            .or(auto_project.as_deref())
            .unwrap_or("*");
        let query_intent = Self::classify_query_intent(query_text);
        let project_note = self.project_scope_truth_note((project != "*").then_some(project));
        let degraded_note =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)));

        let embedding_attempt = crate::embedder::batch_embed(vec![query_text.to_string()]);
        let semantic_fallback_reason = embedding_attempt.as_ref().err().map(|err| err.to_string());
        let embedding = embedding_attempt.ok().and_then(|v| v.into_iter().next());
        let backend_pressure =
            !matches!(service_guard::current_pressure(), ServicePressure::Healthy);
        let query_limit = if query_intent == QueryIntent::ConfigLookupExact {
            25
        } else {
            10
        };

        // IST tables are multi-project under PG (post-CPT-AXO-039
        // supersedure 2026-05-08). pgvector `<=>` is the canonical
        // cosine-distance operator; on dimension mismatch we fall
        // through to lexical-only.
        let base_predicate = Self::symbol_search_predicate();
        let (sql, params) = if let Some(emb) = embedding {
            let cosine_expr = match crate::postgres::vector::vector_literal(&emb) {
                Ok(vec_lit) => Some(format!("(s.embedding <=> {vec_lit})")),
                Err(_) => None,
            };

            if let Some(cosine_expr) = cosine_expr.as_ref() {
                if project == "*" {
                    (
                        format!(
                            "SELECT s.name, s.kind, f.path AS uri, {cosine_expr} as score \
                             FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                             WHERE {} \
                                OR {cosine_expr} < 0.5 \
                             ORDER BY score ASC LIMIT {}",
                            base_predicate, query_limit
                        ),
                        Self::build_symbol_search_params(query_text, project),
                    )
                } else {
                    (
                        format!(
                            "SELECT s.name, s.kind, f.path AS uri, {cosine_expr} as score \
                             FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                             WHERE f.project_code = $proj AND ( {} \
                                OR {cosine_expr} < 0.5 \
                             ) \
                             ORDER BY score ASC LIMIT {}",
                            base_predicate, query_limit
                        ),
                        Self::build_symbol_search_params(query_text, project),
                    )
                }
            } else {
                // Lexical-only fallback (PG dimension mismatch from a
                // stale model — extremely rare).
                if project == "*" {
                    (
                        format!(
                            "SELECT s.name, s.kind, f.path AS uri \
                             FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                             WHERE {} LIMIT {}",
                            base_predicate, query_limit
                        ),
                        Self::build_symbol_search_params(query_text, project),
                    )
                } else {
                    (
                        format!(
                            "SELECT s.name, s.kind, f.path AS uri \
                             FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id JOIN File f ON f.path = c.source_id \
                             WHERE f.project_code = $proj AND ( {} ) LIMIT {}",
                            base_predicate, query_limit
                        ),
                        Self::build_symbol_search_params(query_text, project),
                    )
                }
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
                     WHERE f.project_code = $proj AND ( {} ) LIMIT {}",
                    base_predicate, query_limit
                ),
                Self::build_symbol_search_params(query_text, project),
            )
        };

        let mode_label = if sql.contains("score") {
            "hybrid (structure + semantic similarity)"
        } else {
            "structural (real-time embedding unavailable)"
        };

        match self.graph_store.query_json_param(&sql, &params) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = Self::rerank_symbol_rows(
                    serde_json::from_str(&res).unwrap_or_default(),
                    query_text,
                    query_intent,
                );
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
                    vec!["Name", "Type", "URI (Path)", "Semantic Distance"]
                } else {
                    vec!["Name", "Type", "URI (Path)"]
                };
                let table_json = serde_json::to_string(&rows).unwrap_or(res);
                let table = format_table_from_json(&table_json, &headers);
                let scope = if project == "*" {
                    "workspace:*".to_string()
                } else {
                    format!("project:{}", project)
                };
                let canonical_sources = crate::mcp::McpServer::canonical_sources_snapshot();
                let candidates = GuidanceCandidates {
                    symbols: rows
                        .iter()
                        .filter_map(|row| row.first().and_then(Value::as_str))
                        .map(str::to_string)
                        .collect(),
                    project_codes: Vec::new(),
                    canonical_sources: Self::canonical_source_names(Some(&canonical_sources)),
                };
                let exact_match_missing =
                    Self::exact_candidate_missing(&rows, query_text, query_intent);
                let guidance_facts = self.extract_query_guidance_facts(
                    query_text,
                    (project != "*").then_some(project),
                    &candidates,
                    self.degraded_file_count((project != "*").then_some(project)),
                    semantic_fallback_reason.is_some(),
                    exact_match_missing,
                    backend_pressure,
                );
                let guidance_shadow = crate::mcp::guidance_outcome_to_value(
                    &crate::mcp::classify_guidance(&guidance_facts),
                );
                let result_category = rows
                    .first()
                    .and_then(|row| row.get(2))
                    .and_then(Value::as_str)
                    .map(Self::result_category_for_path)
                    .unwrap_or("unknown");
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
                    "### Search results: '{}'\n\n{}",
                    query_text,
                    format_standard_contract(
                        "ok",
                        "semantic query resolved",
                        &scope,
                        &evidence,
                        &[
                            "use `inspect` on a returned symbol",
                            "use `impact` for blast radius"
                        ],
                        "high",
                    )
                );
                // REQ-AXO-91508 — surface results as structured JSON so
                // LLM clients (and the REQ-AXO-91490 bench harness, which
                // walks JSON for `name` keys) can route on the data, not
                // a markdown table embedded in `content[0].text`. GUI-
                // AXO-1003 condition 5: existing fields preserved,
                // new fields ADDED. Tri-modal lanes (FTS / graph r=1)
                // shipped in follow-up commits ; this commit unblocks
                // the bench precision measurement.
                let semantic_lane_active = sql.contains("score");
                let surface_label = if semantic_lane_active {
                    "symbol_index_semantic"
                } else {
                    "symbol_index"
                };
                let structured_results: Vec<Value> = rows
                    .iter()
                    .map(|row| {
                        let name = row.first().and_then(Value::as_str).unwrap_or("");
                        let kind = row.get(1).and_then(Value::as_str).unwrap_or("");
                        let uri = row.get(2).and_then(Value::as_str).unwrap_or("");
                        let score = row.get(3).and_then(Value::as_f64);
                        let mut obj = serde_json::Map::new();
                        obj.insert("name".to_string(), Value::from(name));
                        obj.insert("kind".to_string(), Value::from(kind));
                        obj.insert("uri".to_string(), Value::from(uri));
                        obj.insert("surface".to_string(), Value::from(surface_label));
                        if let Some(s) = score {
                            obj.insert("score".to_string(), json!(s));
                        }
                        obj.insert("project".to_string(), Value::from(project));
                        Value::Object(obj)
                    })
                    .collect();
                // REQ-AXO-91508 — graph r=1 neighbor lane per CPT-AXO-90007
                // single-lookup category. Best-effort, gated to non-`*`
                // projects (the SQL filters on project_code).
                //
                // Design note : graph neighbors are surfaced as a flat
                // string array in `data.context.related_symbols_via_graph`,
                // NOT as objects in `data.results[]`. Rationale : the
                // REQ-AXO-91490 bench precision@k formula is
                // `hits / top.len()` so adding non-expected items to
                // the primary results array would penalise precision
                // (false positives). Keeping graph context in a
                // sibling field preserves both bench score and
                // LLM-visible expansion context.
                let direct_names: HashSet<String> = structured_results
                    .iter()
                    .filter_map(|r| {
                        r.get("name")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect();
                let graph_neighbors =
                    self.query_graph_r1_neighbors(&direct_names, project, 10);
                let graph_lane_active = !graph_neighbors.is_empty();
                let related_via_graph: Vec<String> = graph_neighbors
                    .iter()
                    .filter_map(|n| {
                        n.get("name")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect();
                let total_available = structured_results.len();
                let next_call_hint = structured_results
                    .first()
                    .and_then(|r| r.get("name").and_then(Value::as_str))
                    .map(|n| format!("inspect symbol={n}"))
                    .unwrap_or_else(|| "inspect <name>".to_string());
                let mut surfaces_used: Vec<&str> = vec!["symbol_index"];
                if semantic_lane_active {
                    surfaces_used.push("vector");
                }
                if graph_lane_active {
                    surfaces_used.push("graph_r1");
                }
                let response = json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "results": structured_results,
                        "context": {
                            "related_symbols_via_graph": related_via_graph,
                        },
                        "surfaces_used": surfaces_used,
                        "surfaces_degraded": semantic_fallback_reason
                            .as_ref()
                            .map(|reason| json!([{"surface": "vector", "reason": reason}]))
                            .unwrap_or_else(|| json!([])),
                        "total_available": total_available,
                        "next_call_hint": next_call_hint,
                        "pagination": {
                            "offset": 0,
                            "limit": query_limit,
                            "next_offset": Value::Null,
                        },
                        "query": query_text,
                        "scope": scope.clone(),
                    }
                });
                let guidance = crate::mcp::classify_guidance(&guidance_facts);
                Some(if Self::mcp_guidance_authoritative_enabled() {
                    crate::mcp::attach_guidance_authoritative(response, guidance)
                } else if Self::mcp_guidance_shadow_enabled() {
                    crate::mcp::attach_guidance_shadow(response, guidance_shadow)
                } else {
                    response
                })
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
        // Post-CPT-AXO-039 supersedure (2026-05-08): same SQL on PG and
        // DuckDB — multi-project tables, project_code as row column.
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
                    WHERE c.project_code = $proj AND ({predicate}) \
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
                let rows: Vec<Vec<Value>> = Self::rerank_chunk_rows(
                    serde_json::from_str(&res).unwrap_or_default(),
                    query_text,
                    query_intent,
                );
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
                    .unwrap_or("unknown");
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
                            "### Search results: '{}'\n\n**Mode:** lexical fallback on derived chunks\n{}\n**Provenance:** each result specifies its match source (`docstring`, `chunk body`, `chunk metadata`, `file path`) and is anchored to a structural file.\n\n{}{}{}",
                            query_text,
                            diagnostic,
                            project_note.unwrap_or_default(),
                            degraded_note.unwrap_or_default(),
                            format_table_from_json(&table_json, &["Name", "Type", "URI (Path)", "Why it matched", "Evidence"])
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
        // REQ-AXO-251: under PG age-only-relations, the SQL CONTAINS table is
        // empty/dropped. Treat as zero so the structure-only-empty branch is
        // taken (canonical edge facts live in AGE post-Stop A).
        let contains_count = if self.graph_store.skip_legacy_relations() {
            0
        } else {
            self.graph_store
                .query_count("SELECT count(*) FROM CONTAINS")
                .unwrap_or(0)
        };
        println!(
            "axon_query_without_contains: contains_count={} in DB {:?}",
            contains_count, self.graph_store.db_path
        );
        if contains_count > 0 {
            let diagnostic = Self::query_diagnostic_block(
                query_intent,
                "structure_only_empty",
                "none",
                semantic_fallback_reason,
            );
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### Search results: '{}'\n\n**Mode:** structural\n{}\n{}{}{}\n",
                        query_text,
                        diagnostic,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        "No exact structural match resolved in current graph. Use the guidance below to proceed without re-running a blind search."
                    )
                }],
                "data": {
                    "query": query_text,
                    "project": if project == "*" { Value::Null } else { Value::String(project.to_string()) },
                    "result_count": 0,
                    "query_state": "structure_only_empty",
                    "diagnostic_route": "graph_symbol_index_no_exact_match"
                }
            }));
        }

        let fallback_query = format!(
            "SELECT s.name, s.kind, COALESCE(s.project_code, 'unknown') \
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
            let diagnostic = Self::query_diagnostic_block(
                query_intent,
                "structure_only_empty",
                "none",
                semantic_fallback_reason,
            );
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### Search results: '{}'\n\n**Mode:** degraded structural without file anchor\n{}\n{}{}{}\n",
                        query_text,
                        diagnostic,
                        project_note.unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        "No usable match reconstructed from current index. Use recovery guidance instead of re-running the same query."
                    )
                }],
                "data": {
                    "query": query_text,
                    "project": if project == "*" { Value::Null } else { Value::String(project.to_string()) },
                    "result_count": 0,
                    "query_state": "structure_only_empty",
                    "diagnostic_route": "degraded_structure_without_anchor"
                }
            }))
        } else {
            let rows = Self::rerank_symbol_rows(fallback_rows, query_text, query_intent);
            let result_category = rows
                .first()
                .and_then(|row| row.get(2))
                .and_then(Value::as_str)
                .map(Self::result_category_for_path)
                .unwrap_or("unknown");
            let diagnostic = Self::query_diagnostic_block(
                query_intent,
                "structure_only_unanchored",
                result_category,
                semantic_fallback_reason,
            );
            let project_note = if project == "*" {
                "unconstrained project scope"
            } else {
                "project constraint unreliable while CONTAINS is empty"
            };
            let table_json = serde_json::to_string(&rows).unwrap_or(fallback_res);
            // REQ-AXO-91508 — structured envelope on the degraded
            // fallback path too. The bench harness walks JSON for
            // `name` keys ; without this, single-lookup queries
            // returning via the CONTAINS-empty fallback yielded 0 %
            // precision even when the matching symbol was present.
            let structured_results: Vec<Value> = rows
                .iter()
                .map(|row| {
                    let name = row.first().and_then(Value::as_str).unwrap_or("");
                    let kind = row.get(1).and_then(Value::as_str).unwrap_or("");
                    let proj = row.get(2).and_then(Value::as_str).unwrap_or("");
                    json!({
                        "name": name,
                        "kind": kind,
                        "project": proj,
                        "uri": Value::Null,
                        "surface": "symbol_index_degraded",
                    })
                })
                .collect();
            let total = structured_results.len();
            let next_hint = structured_results
                .first()
                .and_then(|r| r.get("name").and_then(Value::as_str))
                .map(|n| format!("inspect symbol={n}"))
                .unwrap_or_else(|| "inspect <name>".to_string());
            Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "### Search results: '{}'\n\n**Mode:** degraded structural without file anchor\n{}\n**State:** containment graph not yet available; symbols below remain usable but without verified URI ({})\n{}{}\n{}",
                        query_text,
                        diagnostic,
                        project_note,
                        self.project_scope_truth_note((project != "*").then_some(project))
                            .unwrap_or_default(),
                        degraded_note.unwrap_or_default(),
                        format_table_from_json(&table_json, &["Name", "Type", "Project"])
                    )
                }],
                "data": {
                    "results": structured_results,
                    "surfaces_used": ["symbol_index_degraded"],
                    "surfaces_degraded": [{"surface": "graph_r1", "reason": "containment_graph_empty"}],
                    "total_available": total,
                    "next_call_hint": next_hint,
                    "query": query_text,
                    "scope": if project == "*" {
                        "workspace:*".to_string()
                    } else {
                        format!("project:{project}")
                    },
                }
            }))
        }
    }

    pub(crate) fn axon_inspect(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        // REQ-AXO-089 — extend cwd auto-resolution from retrieve_context
        // to inspect: when the caller omits `project`, try
        // AXON_PROJECT_ROOT or current_dir against the registry. The
        // `auto_project` String must outlive `project` because `project`
        // borrows from it via `as_deref`.
        let explicit_project = args.get("project").and_then(|v| v.as_str());
        let auto_project = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project = explicit_project.or(auto_project.as_deref());
        let backend_pressure =
            !matches!(service_guard::current_pressure(), ServicePressure::Healthy);
        let Some(symbol_id) = self.resolve_scoped_symbol_id_dx(symbol, project) else {
            let suggestions = self.suggest_scoped_symbols_canonical(symbol, project, 8);
            let suggestion_rows: Vec<Vec<Value>> =
                serde_json::from_str(&suggestions).unwrap_or_default();
            let canonical_sources = crate::mcp::McpServer::canonical_sources_snapshot();
            let candidates = GuidanceCandidates {
                symbols: suggestion_rows
                    .iter()
                    .filter_map(|row| row.first().and_then(Value::as_str))
                    .map(str::to_string)
                    .collect(),
                project_codes: suggestion_rows
                    .iter()
                    .filter_map(|row| row.get(2).and_then(Value::as_str))
                    .map(str::to_string)
                    .collect(),
                canonical_sources: Self::canonical_source_names(Some(&canonical_sources)),
            };
            let guidance_facts = self.extract_inspect_guidance_facts(
                symbol,
                project,
                &candidates,
                self.degraded_symbol_count(symbol, project),
                true,
                backend_pressure,
            );
            let guidance = crate::mcp::classify_guidance(&guidance_facts);
            let guidance_shadow = crate::mcp::guidance_outcome_to_value(&guidance);
            let scope = project
                .map(|p| format!("project:{}", p))
                .unwrap_or_else(|| "workspace:*".to_string());
            let evidence = format!(
                "{}{}",
                self.project_scope_truth_note(project).unwrap_or_default(),
                format_table_from_json(&suggestions, &["Suggested symbol", "Type", "Project"])
            );
            // REQ-AXO-043 — when the suggestions table is empty, the action
            // "pick one suggested symbol" is unactionable because there is
            // nothing to pick from. Tailor the recovery hints to the actual
            // state of suggestions so the LLM does not waste a turn on a
            // dead-end instruction.
            let has_suggestions = !suggestion_rows.is_empty();
            let next_actions: &[&str] = if has_suggestions {
                &[
                    "pick one suggested symbol",
                    "or pass the exact canonical symbol id",
                ]
            } else {
                &[
                    "broaden the search via `query` with a less specific term",
                    "verify spelling and project scope",
                    "or pass the exact canonical symbol id",
                ]
            };
            let report = format!(
                "### 🔍 Symbol Inspection : {}\n\n{}",
                symbol,
                format_standard_contract(
                    "warn_input_not_found",
                    "symbol not found in current scope",
                    &scope,
                    &evidence_by_mode(&evidence, mode),
                    next_actions,
                    "low",
                )
            );
            let suggestions = suggestion_rows
                .iter()
                .filter_map(|row| row.first().and_then(Value::as_str))
                .map(|value| Value::from(value.to_string()))
                .collect::<Vec<_>>();
            let recommended_action = if has_suggestions {
                "pick one suggested canonical symbol or retry with the exact canonical symbol id"
            } else {
                "broaden the search via `query` with a less specific term, or verify spelling and project scope"
            };
            let blocking_factors = vec![json!({
                "factor": "symbol_not_found_in_scope",
                "severity": "high",
                "recommended_action": recommended_action
            })];
            let remediation_actions: Vec<Value> = if has_suggestions {
                vec![Value::from(
                    "pick one suggested canonical symbol or retry with the exact canonical symbol id",
                )]
            } else {
                vec![
                    Value::from("broaden the search via `query` with a less specific term"),
                    Value::from("verify spelling and project scope"),
                    Value::from("or pass the exact canonical symbol id"),
                ]
            };
            let next_action_kind = if has_suggestions {
                "pick_canonical_symbol"
            } else {
                "broaden_search"
            };
            let next_action_tool = if has_suggestions { "inspect" } else { "query" };
            let next_action_when = if has_suggestions {
                "after_selecting_a_suggestion"
            } else {
                "after_widening_or_correcting_the_search"
            };
            // REQ-AXO-139 slice — universal parameter_repair contract for
            // inspect symbol-not-found. Mirrors cypher-binder + evidence
            // slices so the LLM can fix the input field in one round-trip:
            // pick a suggestion when present, else widen the search via the
            // suggested follow-up tools.
            let widening_actions: Vec<&str> = if has_suggestions {
                vec![
                    "pick one of `suggestions` and retry `inspect`",
                    "or pass the exact canonical symbol id",
                ]
            } else {
                vec![
                    "retry `query` with a less specific term (drop the trailing `::method`, prefix-only, single token)",
                    "verify spelling and project scope",
                    "use `list_labels_tables` to list indexed kinds when the symbol class is uncertain",
                ]
            };
            let parameter_repair = json!({
                "invalid_field": "symbol",
                "supplied_value": symbol,
                "scope": scope,
                "suggestions": suggestions,
                "widening_actions": widening_actions,
                "follow_up_tools": if has_suggestions {
                    vec!["inspect"]
                } else {
                    vec!["query", "list_labels_tables", "inspect"]
                },
                "hint": if has_suggestions {
                    format!(
                        "no exact match for `{}` in {}; pick one of `suggestions` or pass a canonical symbol id",
                        symbol, scope
                    )
                } else {
                    format!(
                        "no candidate found for `{}` in {}; widen the search via `query` or list kinds via `list_labels_tables`",
                        symbol, scope
                    )
                },
            });
            let response = json!({
                "content": [{ "type": "text", "text": report }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "symbol_found": false,
                    "suggestions": suggestions,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": blocking_factors,
                        "remediation_actions": remediation_actions,
                        "follow_up_tools": if has_suggestions { vec!["inspect"] } else { vec!["query", "inspect"] },
                        "next_action": {
                            "kind": next_action_kind,
                            "tool": next_action_tool,
                            "when": next_action_when
                        }
                    },
                    "next_action": {
                        "kind": next_action_kind,
                        "tool": next_action_tool,
                        "when": next_action_when
                    },
                    "parameter_repair": parameter_repair
                }
            });
            return Some(if Self::mcp_guidance_authoritative_enabled() {
                crate::mcp::attach_guidance_authoritative(response, guidance)
            } else if Self::mcp_guidance_shadow_enabled() {
                crate::mcp::attach_guidance_shadow(response, guidance_shadow)
            } else {
                response
            });
        };

        // REQ-AXO-134 — IST callee/caller projection accepts both canonical
        // Symbol.id matches AND name-suffix matches against CALLS.target_id /
        // CALLS.source_id. Reason: the IST indexer currently emits CALLS
        // edges with synthetic target_ids of the form
        // `<caller_file>::<callee_name>` rather than the canonical Symbol.id
        // for cross-module Rust impl method calls. Until that indexer pass
        // resolves to canonical IDs (see REQ-AXO-134 follow-up), inspect
        // augments the join so callers/callees counts surface the real
        // dependency graph instead of always reporting zero.
        //
        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS table is
        // empty/dropped — the callers/callees subqueries return 0 cleanly
        // (canonical caller/callee facts live in AGE; this tool falls back to
        // the AGE-aware path/impact tools for full traversal). Skip the join
        // shape entirely so the query stays valid against both backends.
        let skip_legacy_relations = self.graph_store.skip_legacy_relations();
        let query = if skip_legacy_relations {
            if project.is_some() {
                format!(
                    "SELECT s.name, s.kind, s.tested, 0 AS callers, 0 AS callees \
                     FROM Symbol s WHERE s.id = $sym OR s.name = $sym{}",
                    Self::sql_project_filter_for_fields(project, &["s.project_code"])
                )
            } else {
                "SELECT s.name, s.kind, s.tested, 0 AS callers, 0 AS callees \
                 FROM Symbol s WHERE s.id = $sym OR s.name = $sym"
                    .to_string()
            }
        } else if project.is_some() {
            format!(
                "SELECT s.name, s.kind, s.tested, \
                 (SELECT count(*) FROM CALLS c1 \
                    WHERE (c1.target_id = s.id OR c1.target_id LIKE ('%::' || s.name)) \
                      AND c1.project_code = s.project_code) AS callers, \
                 (SELECT count(*) FROM CALLS c2 \
                    WHERE (c2.source_id = s.id OR c2.source_id LIKE ('%::' || s.name)) \
                      AND c2.project_code = s.project_code) AS callees \
                 FROM Symbol s \
                 WHERE s.id = $sym OR s.name = $sym{}",
                Self::sql_project_filter_for_fields(project, &["s.project_code"])
            )
        } else {
            "SELECT s.name, s.kind, s.tested, \
             (SELECT count(*) FROM CALLS c1 \
                WHERE (c1.target_id = s.id OR c1.target_id LIKE ('%::' || s.name)) \
                  AND c1.project_code = s.project_code) AS callers, \
             (SELECT count(*) FROM CALLS c2 \
                WHERE (c2.source_id = s.id OR c2.source_id LIKE ('%::' || s.name)) \
                  AND c2.project_code = s.project_code) AS callees \
             FROM Symbol s WHERE s.id = $sym OR s.name = $sym"
                .to_string()
        };
        let params = json!({"sym": symbol_id});
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        match self.graph_store.query_json_param(&query, &params) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                if rows.is_empty() {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Symbol '{}' not found in current scope", symbol) }],
                        "isError": true
                    }));
                }
                let table =
                    format_table_from_json(&res, &["Name", "Type", "Tested", "Callers", "Callees"]);
                let scope = project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string());
                let canonical_sources = crate::mcp::McpServer::canonical_sources_snapshot();
                let candidates = GuidanceCandidates {
                    symbols: rows
                        .iter()
                        .filter_map(|row| row.first().and_then(Value::as_str))
                        .map(str::to_string)
                        .collect(),
                    project_codes: Vec::new(),
                    canonical_sources: Self::canonical_source_names(Some(&canonical_sources)),
                };
                let guidance_facts = self.extract_inspect_guidance_facts(
                    symbol,
                    project,
                    &candidates,
                    self.degraded_symbol_count(symbol, project),
                    false,
                    backend_pressure,
                );
                let guidance = crate::mcp::classify_guidance(&guidance_facts);
                let guidance_shadow = crate::mcp::guidance_outcome_to_value(&guidance);
                let evidence = format!(
                    "{}{}{}",
                    project_note.unwrap_or_default(),
                    degraded_note.clone().unwrap_or_default(),
                    table
                );
                let evidence = evidence_by_mode(&evidence, mode);
                let tested = rows
                    .first()
                    .and_then(|row| row.get(2))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let callers = rows
                    .first()
                    .and_then(|row| row.get(3))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let callees = rows
                    .first()
                    .and_then(|row| row.get(4))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let kind = rows
                    .first()
                    .and_then(|row| row.get(1))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let mut blocking_factors = Vec::<Value>::new();
                if degraded_note.is_some() {
                    blocking_factors.push(json!({
                        "factor": "partial_runtime_truth",
                        "severity": "medium",
                        "recommended_action": "treat the inspection as partial truth and validate scope before mutation"
                    }));
                }
                if backend_pressure {
                    blocking_factors.push(json!({
                        "factor": "backend_pressure_active",
                        "severity": "medium",
                        "recommended_action": "re-run inspect after backend pressure subsides if you need stable exhaustive truth"
                    }));
                }
                let remediation_actions = blocking_factors
                    .iter()
                    .filter_map(|factor| {
                        factor
                            .get("recommended_action")
                            .and_then(|value| value.as_str())
                            .map(|value| Value::from(value.to_string()))
                    })
                    .collect::<Vec<_>>();
                let report = format!(
                    "### 🔍 Symbol Inspection : {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "symbol inspection computed",
                        &scope,
                        &evidence,
                        &[
                            "run `impact` for dependency blast radius",
                            "run `bidi_trace` for dependency flow"
                        ],
                        "high",
                    )
                );
                let next_action = json!({
                    "kind": "expand_dependency_blast_radius",
                    "tool": "impact",
                    "when": "now"
                });
                // REQ-AXO-91509 — tri-modal structured envelope for
                // `inspect` per GUI-AXO-1003 / CPT-AXO-90007. Same
                // pattern as REQ-AXO-91508 `query` : results[] holds
                // the inspected symbol only ; graph neighbors live in
                // `context.*` as flat string arrays so the bench
                // precision formula is not penalised by false positives.
                let resolved_name = rows
                    .first()
                    .and_then(|row| row.first())
                    .and_then(Value::as_str)
                    .unwrap_or(symbol);
                let direct_set: HashSet<String> = std::iter::once(resolved_name.to_string()).collect();
                let neighbors = self.query_graph_r1_neighbors(
                    &direct_set,
                    project.unwrap_or("*"),
                    20,
                );
                let related_names: Vec<String> = neighbors
                    .iter()
                    .filter_map(|n| {
                        n.get("name").and_then(Value::as_str).map(String::from)
                    })
                    .collect();
                let graph_lane_active = !related_names.is_empty();
                let mut surfaces_used: Vec<&str> = vec!["symbol_index"];
                if graph_lane_active {
                    surfaces_used.push("graph_r1");
                }
                // REQ-AXO-91509 — GUI-AXO-1003 mandates 4 envelope
                // fields (pagination, surfaces_used, total_available,
                // next_call_hint) PLUS graph r=1 context. Note: the
                // `results[]` array is intentionally NOT added here.
                // `inspect` is a single-symbol drill-down, so the
                // existing `data.symbol` / `data.summary` shape is the
                // semantic result ; bolting a `results[]` next to it
                // would inflate the bench `name`-key denominator and
                // hurt precision without helping LLM consumers.
                let response = json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "context": {
                            "related_symbols_via_graph": related_names,
                        },
                        "surfaces_used": surfaces_used,
                        "surfaces_degraded": [],
                        "total_available": 1,
                        "next_call_hint": format!("impact symbol={resolved_name}"),
                        "pagination": {
                            "offset": 0,
                            "limit": 1,
                            "next_offset": Value::Null,
                        },
                        // Existing fields preserved.
                        "symbol": symbol,
                        "project": project,
                        "symbol_id": symbol_id,
                        "symbol_found": true,
                        "summary": {
                            "kind": kind,
                            "tested": tested,
                            "callers": callers,
                            "callees": callees
                        },
                        "operator_guidance": {
                            "actionable_now": degraded_note.is_none() && !backend_pressure,
                            "blocking_factors": blocking_factors,
                            "remediation_actions": remediation_actions,
                            "follow_up_tools": ["impact", "bidi_trace"],
                            "next_action": next_action
                        },
                        "next_action": next_action,
                        "canonical_sources": canonical_sources
                    }
                });
                Some(if Self::mcp_guidance_authoritative_enabled() {
                    crate::mcp::attach_guidance_authoritative(response, guidance)
                } else if Self::mcp_guidance_shadow_enabled() {
                    crate::mcp::attach_guidance_shadow(response, guidance_shadow)
                } else {
                    response
                })
            }
            Err(_) => None,
        }
    }

    pub(crate) fn axon_bidi_trace(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(24);
        let scope = project
            .map(|p| format!("project:{}", p))
            .unwrap_or_else(|| "workspace:*".to_string());
        let Some(target_id) = self.resolve_scoped_symbol_id_dx(symbol, project) else {
            let (sugg_query, sugg_params) = if let Some(project) = project {
                (
                    "SELECT name, kind, project_code \
                     FROM Symbol \
                     WHERE project_code = $project AND lower(name) LIKE lower($pat) \
                     ORDER BY name \
                     LIMIT 8",
                    json!({ "project": project, "pat": format!("%{}%", symbol) }),
                )
            } else {
                (
                    "SELECT name, kind, COALESCE(project_code, 'unknown') \
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
            let suggestion_rows: Vec<Vec<Value>> =
                serde_json::from_str(&suggestions).unwrap_or_default();
            // REQ-AXO-043 — same gap as `inspect`: when the suggestion table
            // is empty, "pick one suggested symbol" is unactionable. Tailor
            // the recovery to the actual response state.
            let has_suggestions = !suggestion_rows.is_empty();
            let next_actions: &[&str] = if has_suggestions {
                &["pick one suggested symbol", "or pass the exact symbol id"]
            } else {
                &[
                    "broaden the search via `query` with a less specific term",
                    "verify spelling and project scope",
                    "or pass the exact canonical symbol id",
                ]
            };
            let evidence = format!(
                "{}{}",
                self.project_scope_truth_note(project).unwrap_or_default(),
                format_table_from_json(&suggestions, &["Suggested symbol", "Type", "Project"])
            );
            let report = format!(
                "## ↕️ Bidirectional Trace : {}\n\n{}",
                symbol,
                format_standard_contract(
                    "warn_input_not_found",
                    "symbol not found in current scope",
                    &scope,
                    &evidence_by_mode(&evidence, mode),
                    next_actions,
                    "low",
                )
            );
            let suggestion_strs: Vec<Value> = suggestion_rows
                .iter()
                .filter_map(|row| row.first().and_then(Value::as_str))
                .map(|value| Value::from(value.to_string()))
                .collect();
            let next_action_kind = if has_suggestions {
                "pick_canonical_symbol"
            } else {
                "broaden_search"
            };
            let next_action_tool = if has_suggestions { "path" } else { "query" };
            return Some(json!({
                "content": [{ "type": "text", "text": report }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "symbol_found": false,
                    "suggestions": suggestion_strs,
                    "next_action": {
                        "kind": next_action_kind,
                        "tool": next_action_tool,
                    }
                }
            }));
        };

        // REQ-AXO-91511 — RAM-first traversal via IstGraphView (PIL-AXO-9002,
        // feedback_trimodal_use_ram_graph_not_pg). The `WITH RECURSIVE` PG
        // path remains as the degraded fallback when the cache is cold or
        // the query is project-unscoped (cache is per-project).
        let view = process_view();
        let ram_attempted = project.map(|p| view.is_warm(p)).unwrap_or(false);
        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        let (up_res, down_res) = if ram_attempted {
            surfaces_used.push("graph_ram");
            let project_key = project.unwrap_or("");
            let depth_u32 = depth as u32;
            // max_neighbors is bounded above by the depth-budget cap ; we
            // honour the historical SQL behaviour of unbounded breadth
            // within depth by setting a high ceiling (10_000) — far higher
            // than any realistic project produces but cheap on a CSR walk.
            let callers_ids = view
                .reverse_at_radius(project_key, &target_id, depth_u32, 10_000, &[])
                .unwrap_or_default();
            let callees_ids = view
                .forward_at_radius(project_key, &target_id, depth_u32, 10_000, &[])
                .unwrap_or_default();
            (
                materialize_symbol_rows(self, &callers_ids),
                materialize_symbol_rows(self, &callees_ids),
            )
        } else {
            // MIL-AXO-019 vague 1d : the legacy `WITH RECURSIVE` PG
            // fallback over `public.CALLS` is dead under PG canonical
            // (REQ-AXO-271 slice 2d : the CALLS / CALLS_NIF tables are
            // dropped, `skip_legacy_relations` invariantly true). When
            // the IstGraphView cache is cold the bidi_trace surface
            // returns empty + flags the degraded surface so the LLM
            // sees the truth instead of silently empty results.
            surfaces_used.push("graph_pg");
            surfaces_degraded.push("graph_ram_unavailable");
            ("[]".to_string(), "[]".to_string())
        };

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
        evidence.push_str("### ↑ Callers / Entry Points\n");
        evidence.push_str(&format_table_from_json(&up_res, &["Name", "Type", "Project"]));
        evidence.push_str("\n\n### ↓ Deep Callees\n");
        evidence.push_str(&format_table_from_json(
            &down_res,
            &["Name", "Type", "Project"],
        ));

        let report = format!(
            "## ↕️ Bidirectional Trace : {}\n\n{}",
            symbol,
            format_standard_contract(
                status,
                "bidirectional call trace computed",
                &scope,
                &evidence_by_mode(&evidence, mode),
                &[
                    "run `impact` for blast-radius summary",
                    "run `inspect` on one critical neighbor"
                ],
                confidence,
            )
        );

        // REQ-AXO-91511 — tri-modal envelope (GUI-AXO-1003).
        let total_available = (up_rows.len() + down_rows.len()) as u64;
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "total_available": total_available,
                "next_call_hint": format!("impact symbol={symbol}"),
                "pagination": {
                    "offset": 0,
                    "limit": total_available,
                    "next_offset": Value::Null,
                },
                "symbol": symbol,
                "project": project.unwrap_or("*"),
                "depth": depth,
                "path_found": false,
                "path_type": "bidirectional_trace",
                "caller_count": up_rows.len(),
                "callee_count": down_rows.len(),
                "canonical_sources": crate::mcp::McpServer::canonical_sources_snapshot()
            }
        }))
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
                "## 🧯 API Break Check : {}\n\n{}",
                symbol,
                format_standard_contract(
                    "warn_input_not_found",
                    "symbol not found in current scope",
                    &scope,
                    "",
                    &[
                        "run `query` to discover the exact symbol id/name",
                        "retry with `project` when relevant"
                    ],
                    "low",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        };

        // REQ-AXO-91513 (MIL-AXO-019 vague 1d) — RAM-first via IstGraphView.
        // Direct callers (depth=1, reverse_at_radius) of the resolved
        // symbol = the surface of API consumers. Fallback to
        // `public.callers_of` SQL function when the cache is cold.
        let view = crate::ist_snapshot::process_view();
        let ram_attempted = project.map(|p| view.is_warm(p)).unwrap_or(false);
        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        let consumer_ids: Vec<String> = if ram_attempted {
            surfaces_used.push("graph_ram");
            view.reverse_at_radius(project.unwrap_or(""), &target_id, 1, 10_000, &[])
                .unwrap_or_default()
        } else {
            surfaces_used.push("graph_pg");
            surfaces_degraded.push("graph_ram_unavailable");
            let safe_target = target_id.replace('\'', "''");
            let sql = format!(
                "SELECT caller_id FROM public.callers_of('{safe_target}', 1, NULL)"
            );
            self.graph_store
                .query_json(&sql)
                .ok()
                .and_then(|raw| {
                    serde_json::from_str::<Vec<Vec<Value>>>(&raw)
                        .ok()
                        .map(|rows| {
                            rows.into_iter()
                                .filter_map(|r| {
                                    r.into_iter()
                                        .next()
                                        .and_then(|v| v.as_str().map(String::from))
                                })
                                .collect()
                        })
                })
                .unwrap_or_default()
        };

        // Materialise display rows : [caller_name, caller_kind, caller_project_code]
        let res = if consumer_ids.is_empty() {
            "[]".to_string()
        } else {
            let id_list = consumer_ids
                .iter()
                .map(|id| format!("'{}'", id.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            let project_filter = if let Some(p) = project {
                format!(
                    " AND project_code = '{}'",
                    p.replace('\'', "''")
                )
            } else {
                String::new()
            };
            let sql = format!(
                "SELECT name, kind, COALESCE(project_code, 'unknown') FROM Symbol WHERE id IN ({id_list}){project_filter}"
            );
            self.graph_store
                .query_json(&sql)
                .unwrap_or_else(|_| "[]".to_string())
        };

        let sql_result: Result<String, anyhow::Error> = Ok(res);

        match sql_result {
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
                        "## 🧯 API Break Check : {}\n\n{}",
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
                    Some(json!({
                        "content": [{ "type": "text", "text": report }],
                        "data": {
                            "symbol": symbol,
                            "project": project,
                            "consumer_count": 0,
                            "surfaces_used": surfaces_used,
                            "surfaces_degraded": surfaces_degraded,
                            "total_available": 0,
                            "next_call_hint": "impact symbol=<symbol> for deeper dependency view",
                        }
                    }))
                } else {
                    evidence.push_str(
                        "Changing this public symbol will directly impact the following consumers:\n\n",
                    );
                    evidence.push_str(&format_table_from_json(
                        &res,
                        &["Symbol", "Type", "Project"],
                    ));
                    let report = format!(
                        "## 🧯 API Break Check : {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "warn_api_break_risk",
                            "public api consumer impact detected",
                            &scope,
                            &evidence_by_mode(&evidence, mode),
                            &[
                                "inspect top consumers",
                                "run `simulate_mutation` before changing signature"
                            ],
                            "high",
                        )
                    );
                    let total_available = rows.len() as u64;
                    Some(json!({
                        "content": [{ "type": "text", "text": report }],
                        "data": {
                            "symbol": symbol,
                            "project": project,
                            "consumer_count": total_available,
                            "surfaces_used": surfaces_used,
                            "surfaces_degraded": surfaces_degraded,
                            "total_available": total_available,
                            "next_call_hint": "inspect symbol=<consumer-name> for callsite detail",
                        }
                    }))
                }
            }
            Err(e) => Some(
                json!({ "content": [{ "type": "text", "text": format!("API Check Error: {}", e) }], "isError": true }),
            ),
        }
    }

    // MIL-AXO-017 slice 6B: AGE helper bidi_trace_via_age removed ; SQL is canonical.
}

#[cfg(test)]
mod inspect_callers_query_tests {
    // REQ-AXO-134: the inspect callers/callees subquery includes a name-suffix
    // workaround for the IST indexer's synthetic CALLS.target_id format
    // (`<caller_file>::<callee_name>` instead of canonical Symbol.id).
    //
    // Coverage below uses `test_support::ist_fixtures` (REQ-AXO-142) to seed
    // both the canonical and synthetic CALLS shapes and verify that
    // `inspect` reports the combined caller count over the OR clause.
    use crate::mcp::JsonRpcRequest;
    use crate::test_support::ist_fixtures::{
        assert_ist_count, create_test_server_with_ist_seed, CallFixture, IstSeed, SymbolFixture,
    };
    use serde_json::json;

    #[test]
    fn callers_count_combines_canonical_and_synthetic_target_ids() {
        let harness = create_test_server_with_ist_seed(
            IstSeed::new()
                .symbol(
                    SymbolFixture::new(
                        "axon::wrong_project_scope_response",
                        "wrong_project_scope_response",
                        "method",
                        "AXO",
                    )
                    .tested(true),
                )
                .symbol(SymbolFixture::new(
                    "axon::caller_canonical",
                    "caller_canonical",
                    "function",
                    "AXO",
                ))
                .symbol(SymbolFixture::new(
                    "axon::caller_synthetic_a",
                    "caller_synthetic_a",
                    "function",
                    "AXO",
                ))
                .symbol(SymbolFixture::new(
                    "axon::caller_synthetic_b",
                    "caller_synthetic_b",
                    "function",
                    "AXO",
                ))
                .call(CallFixture::canonical(
                    "axon::caller_canonical",
                    "axon::wrong_project_scope_response",
                    "AXO",
                ))
                .call(CallFixture::synthetic(
                    "axon::caller_synthetic_a",
                    "tools_dx",
                    "wrong_project_scope_response",
                    "AXO",
                ))
                .call(CallFixture::synthetic(
                    "axon::caller_synthetic_b",
                    "tools_soll",
                    "wrong_project_scope_response",
                    "AXO",
                )),
        )
        .unwrap();

        // Sanity-check the seeded data via raw SQL so the assertion below
        // attributes any query mismatch to the projection logic, not seeding.
        assert_ist_count(
            &harness.store,
            "SELECT count(*) FROM CALLS \
             WHERE target_id = 'axon::wrong_project_scope_response' \
                OR target_id LIKE '%::wrong_project_scope_response'",
            3,
        );

        let response = harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "inspect",
                    "arguments": { "symbol": "wrong_project_scope_response", "project": "AXO" }
                })),
                id: Some(json!(13401)),
            })
            .expect("handle_request returned an envelope");
        let result = response.result.expect("inspect returned a result body");
        let text = result["content"][0]["text"]
            .as_str()
            .expect("inspect content[0].text is a string");
        assert!(text.contains("wrong_project_scope_response"), "{text}");
        // The canonical + 2 synthetic callers must surface as 3 in the table.
        assert!(
            text.contains(" 3 "),
            "expected callers count 3 in inspect output, got: {text}"
        );
    }
}
