use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};

use super::retrieval_model::{
    ChunkCandidate, EntryCandidate, RetrievalDiagnostics, RetrievalRoute, RetrievalRuntimeState,
};
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn find_entry_candidates(
        &self, project: Option<&str>, terms: &[String], path_hints: &[String],
        limit: usize, diagnostics: &mut RetrievalDiagnostics,
    ) -> Vec<EntryCandidate> {
        let mut entries = self.find_symbol_candidates(project, terms, path_hints, limit);
        diagnostics.symbol_candidates_considered = entries.len();
        let file_candidates = self.find_file_candidates(project, terms, path_hints, limit);
        diagnostics.file_candidates_considered = file_candidates.len();
        entries.extend(file_candidates);
        if entries.is_empty() {
            if let Some(repo_root) = Self::project_repo_root(project) {
                let mut fallback = self.find_symbol_candidates(None, terms, path_hints, limit);
                fallback.retain(|c| c.uri.starts_with(&repo_root));
                diagnostics.symbol_candidates_considered += fallback.len();
                let mut fallback_files = self.find_file_candidates(None, terms, path_hints, limit);
                fallback_files.retain(|c| c.uri.starts_with(&repo_root));
                diagnostics.file_candidates_considered += fallback_files.len();
                fallback.extend(fallback_files);
                entries.extend(fallback);
            }
        }
        if entries.is_empty() {
            let (repo_entries, _) = self.repo_literal_fallback_candidates(project, terms, limit);
            diagnostics.file_candidates_considered += repo_entries.len();
            entries.extend(repo_entries);
        }
        entries
    }

    pub(super) fn project_repo_root(project: Option<&str>) -> Option<String> {
        let project = project.map(str::trim).filter(|v| !v.is_empty())?;
        let identity = crate::project_meta::resolve_canonical_project_identity(project).ok()?;
        let repo_root = identity.meta_path.parent()?.parent()?;
        Some(repo_root.to_string_lossy().into_owned())
    }

    fn is_strong_identifier_term(term: &str) -> bool {
        term.len() >= 4 && term.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.'))
    }

    fn repo_literal_file_rank(path: &str) -> i32 {
        let lower = path.to_ascii_lowercase();
        let mut score = 0i32;
        if lower.ends_with(".rs") || lower.ends_with(".ex") || lower.ends_with(".exs")
            || lower.ends_with(".py") || lower.ends_with(".ts") || lower.ends_with(".tsx")
            || lower.ends_with(".js") || lower.ends_with(".jsx") { score += 4; }
        if lower.contains("/src/") { score += 3; }
        if lower.contains("/test/") || lower.contains("/tests/")
            || lower.starts_with("test/") || lower.starts_with("tests/") { score -= 4; }
        if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") { score -= 3; }
        score
    }

    fn should_consider_repo_literal_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        if lower.contains("/.git/") || lower.contains("/target/") || lower.contains("/.axon/")
            || lower.contains("/node_modules/") || lower.contains("/dist/") || lower.contains("/build/")
            || lower.contains("/_build/") || lower.contains("/deps/") || lower.contains("/test/")
            || lower.contains("/tests/") || lower.ends_with("/tests.rs") || lower.ends_with("_test.exs")
            || lower.ends_with("_test.ex") || lower.ends_with("_test.rs") || lower.ends_with(".test.ts")
            || lower.ends_with(".test.js") || lower.contains("/docs/") || lower.ends_with(".md")
        { return false; }
        lower.ends_with(".rs") || lower.ends_with(".ex") || lower.ends_with(".exs")
            || lower.ends_with(".py") || lower.ends_with(".ts") || lower.ends_with(".tsx")
            || lower.ends_with(".js") || lower.ends_with(".jsx")
    }

    fn snippet_around_term(content: &str, term: &str) -> Option<String> {
        let lower = content.to_ascii_lowercase();
        let needle = term.to_ascii_lowercase();
        let offset = lower.find(&needle)?;
        let start = offset.saturating_sub(100);
        let end = (offset + needle.len() + 120).min(content.len());
        Some(Self::truncate(content.get(start..end).unwrap_or(content).trim(), 220))
    }

    pub(super) fn repo_literal_fallback_candidates(&self, project: Option<&str>, terms: &[String], limit: usize) -> (Vec<EntryCandidate>, Vec<ChunkCandidate>) {
        let Some(repo_root) = Self::project_repo_root(project) else { return (Vec::new(), Vec::new()); };
        let repo_root_path = Path::new(&repo_root);
        if !repo_root_path.exists() { return (Vec::new(), Vec::new()); }
        let strong_terms = terms.iter().filter(|t| Self::is_strong_identifier_term(t)).cloned().collect::<Vec<_>>();
        if strong_terms.is_empty() { return (Vec::new(), Vec::new()); }
        let project_code = project
            .and_then(|v| crate::project_meta::resolve_canonical_project_identity(v).ok())
            .map(|i| i.code)
            .or_else(|| project.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        let mut matches = Vec::new();
        for entry in WalkBuilder::new(repo_root_path).hidden(false).standard_filters(true).build() {
            let Ok(entry) = entry else { continue; };
            if !entry.file_type().map(|ty| ty.is_file()).unwrap_or(false) { continue; }
            let path = entry.path();
            let path_str = path.to_string_lossy().into_owned();
            if !Self::should_consider_repo_literal_path(&path_str) { continue; }
            let metadata = match entry.metadata() { Ok(m) => m, Err(_) => continue };
            if metadata.len() > 512 * 1024 { continue; }
            let Ok(content) = fs::read_to_string(path) else { continue; };
            let content_lower = content.to_ascii_lowercase();
            let mut matched_terms = strong_terms.iter().filter(|t| content_lower.contains(t.as_str())).cloned().collect::<Vec<_>>();
            if matched_terms.is_empty() { continue; }
            matched_terms.sort();
            let match_term = matched_terms[0].clone();
            let lexical_hits = matched_terms.len();
            let snippet = Self::snippet_around_term(&content, &match_term)
                .unwrap_or_else(|| Self::truncate(content.lines().next().unwrap_or_default(), 220));
            matches.push((Self::repo_literal_file_rank(&path_str), lexical_hits, match_term, path_str, snippet, matched_terms));
        }
        matches.sort_by(|l, r| r.0.cmp(&l.0).then_with(|| r.1.cmp(&l.1)).then_with(|| l.3.cmp(&r.3)));
        let mut entries = Vec::new();
        let mut chunks = Vec::new();
        let mut seen_paths = HashSet::new();
        for (base_rank, lexical_hits, match_term, path_str, snippet, matched_terms) in matches {
            if !seen_paths.insert(path_str.clone()) { continue; }
            let reasons = vec!["repo_literal_fallback".to_string(), "repo_root_match".to_string(), "content_term_match".to_string()];
            entries.push(EntryCandidate { id: path_str.clone(), name: match_term.clone(), kind: "repo_literal".to_string(),
                project_code: project_code.clone(), uri: path_str.clone(), lexical_hits, exact_match: true,
                score: 4.0 + f64::from(base_rank.max(0)), reasons: reasons.clone() });
            chunks.push(ChunkCandidate { chunk_id: format!("repo_literal::{path_str}::{match_term}"),
                source_id: path_str.clone(), project_code: project_code.clone(), uri: path_str.clone(),
                content: snippet, match_reason: "repo_literal".to_string(), lexical_hits: matched_terms.len(),
                semantic_distance: None, chunk_part_index: 1, chunk_part_count: 1, chunk_path: "1/1".to_string(),
                anchored_to_entry: true, same_file_as_entry: true,
                score: 4.0 + f64::from(base_rank.max(0)), reasons });
            if entries.len() >= limit.min(2) { break; }
        }
        (entries, chunks)
    }

    pub(super) fn find_symbol_candidates(&self, project: Option<&str>, terms: &[String], path_hints: &[String], limit: usize) -> Vec<EntryCandidate> {
        let mut candidates = self.find_exact_symbol_candidates(project, terms, limit);
        let name_match = Self::term_match_sql(terms, "s.name");
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let uri_term_match = Self::term_match_sql(terms, "f.path");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(f.path, '') \
             FROM Symbol s LEFT JOIN CONTAINS c ON c.target_id = s.id LEFT JOIN File f ON f.path = c.source_id \
             WHERE ({name_match} OR {uri_term_match} OR {path_match}){project_filter} LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "f.project_code"]));
        let raw = self.graph_store.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        candidates.extend(rows.into_iter().filter_map(|row| {
            let id = row.first()?.as_str()?.to_string();
            let name = row.get(1)?.as_str()?.to_string();
            let kind = row.get(2)?.as_str()?.to_string();
            let project_code = row.get(3)?.as_str()?.to_string();
            let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
            let lexical_hits = terms.iter().filter(|t| name.to_ascii_lowercase().contains(t.as_str()) || uri.to_ascii_lowercase().contains(t.as_str())).count();
            let exact_match = terms.iter().any(|t| name.eq_ignore_ascii_case(t)) || path_hints.iter().any(|h| uri.eq_ignore_ascii_case(h));
            Some(EntryCandidate { id, name, kind, project_code, uri, lexical_hits, exact_match, score: 0.0, reasons: Vec::new() })
        }));
        candidates
    }

    fn find_exact_symbol_candidates(&self, project: Option<&str>, terms: &[String], limit: usize) -> Vec<EntryCandidate> {
        let exact_terms = terms.iter().filter(|t| Self::is_strong_identifier_term(t)).map(|t| t.to_ascii_lowercase()).collect::<HashSet<_>>();
        if exact_terms.is_empty() { return Vec::new(); }
        let exact_values = exact_terms.iter().map(|t| format!("'{}'", Self::escape_sql(t))).collect::<Vec<_>>().join(", ");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(f.path, '') \
             FROM Symbol s LEFT JOIN CONTAINS c ON c.target_id = s.id LEFT JOIN File f ON f.path = c.source_id \
             WHERE lower(s.name) IN ({exact_values}){project_filter} LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "f.project_code"]));
        let raw = self.graph_store.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter().filter_map(|row| {
            let id = row.first()?.as_str()?.to_string();
            let name = row.get(1)?.as_str()?.to_string();
            let kind = row.get(2)?.as_str()?.to_string();
            let project_code = row.get(3)?.as_str()?.to_string();
            let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
            Some(EntryCandidate { id, name, kind, project_code, uri, lexical_hits: 1, exact_match: true, score: 0.0, reasons: vec!["exact_symbol_lookup".to_string()] })
        }).collect()
    }

    pub(super) fn find_file_candidates(&self, project: Option<&str>, terms: &[String], path_hints: &[String], limit: usize) -> Vec<EntryCandidate> {
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let term_match = Self::term_match_sql(terms, "f.path");
        let query = format!(
            "SELECT f.path, COALESCE(f.project_code, 'unknown') FROM File f WHERE ({path_match} OR {term_match}){project_filter} LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["f.project_code"]));
        let raw = self.graph_store.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter().filter_map(|row| {
            let path = row.first()?.as_str()?.to_string();
            let project_code = row.get(1)?.as_str()?.to_string();
            let lexical_hits = terms.iter().filter(|t| path.to_ascii_lowercase().contains(t.as_str())).count();
            let exact_match = path_hints.iter().any(|h| path.eq_ignore_ascii_case(h)) || terms.iter().any(|t| path.eq_ignore_ascii_case(t));
            Some(EntryCandidate { id: path.clone(), name: path.clone(), kind: "file".to_string(), project_code, uri: path, lexical_hits, exact_match, score: 0.0, reasons: Vec::new() })
        }).collect()
    }

    pub(super) fn resolve_file_symbol_bindings(&self, project: Option<&str>, file_paths: &[String]) -> Vec<(String, String)> {
        if file_paths.is_empty() { return Vec::new(); }
        // REQ-AXO-251: under PG age-only-relations, the SQL CONTAINS table is
        // empty/dropped — return no bindings gracefully.
        if self.graph_store.skip_legacy_relations() { return Vec::new(); }
        let values = file_paths.iter().map(|p| format!("'{}'", Self::escape_sql(p))).collect::<Vec<_>>().join(", ");
        let query = format!("SELECT c.target_id, c.source_id FROM CONTAINS c WHERE c.source_id IN ({values}){project_filter}",
            project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]));
        let raw = self.graph_store.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter().filter_map(|row| Some((row.first()?.as_str()?.to_string(), row.get(1)?.as_str()?.to_string()))).collect()
    }

    pub(super) fn rerank_entry_candidates(&self, candidates: &mut [EntryCandidate], route: RetrievalRoute,
        terms: &[String], path_hints: &[String], project_scope_variants: &[String], prefer_project_intent: bool) {
        let scope_lc = project_scope_variants.iter().map(|v| v.to_ascii_lowercase()).collect::<HashSet<_>>();
        for candidate in candidates.iter_mut() {
            let mut score = (candidate.lexical_hits as f64) * 2.0;
            if candidate.exact_match { score += 5.0; candidate.reasons.push("exact_anchor_match".to_string()); }
            if !candidate.uri.is_empty() { score += 1.0; candidate.reasons.push("file_anchored".to_string()); }
            if scope_lc.contains(&candidate.project_code.to_ascii_lowercase()) { score += 1.5; candidate.reasons.push("project_scope_match".to_string()); }
            if matches!(route, RetrievalRoute::Wiring | RetrievalRoute::Impact) && matches!(candidate.kind.as_str(), "function" | "method") {
                score += 1.5; candidate.reasons.push("route_prefers_callable_anchor".to_string());
            }
            if candidate.kind == "file" { score += 1.0; candidate.reasons.push("file_entrypoint".to_string()); }
            if prefer_project_intent {
                let intent_weight = Self::project_intent_doc_weight(&candidate.uri);
                if intent_weight > 0.0 { score += intent_weight; candidate.reasons.push("intent_canonical_plan_bonus".to_string()); }
                else if intent_weight < 0.0 { score += intent_weight; candidate.reasons.push("intent_feedback_penalty".to_string()); }
            }
            if path_hints.iter().any(|h| candidate.uri.to_ascii_lowercase().contains(h)) { score += 2.0; candidate.reasons.push("path_hint_match".to_string()); }
            if terms.iter().any(|t| candidate.uri.to_ascii_lowercase().contains(t)) { score += 1.0; candidate.reasons.push("uri_term_match".to_string()); }
            candidate.score = score;
        }
        candidates.sort_by(|l, r| r.score.partial_cmp(&l.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| l.uri.cmp(&r.uri)));
    }

    pub(super) fn select_entry_candidates(&self, candidates: &[EntryCandidate], top_k: usize) -> Vec<EntryCandidate> {
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        for candidate in candidates.iter().take(top_k * 2) {
            let key = format!("{}:{}", candidate.kind, candidate.id);
            if !seen.insert(key) { continue; }
            selected.push(candidate.clone());
            if selected.len() >= top_k.min(2) { break; }
        }
        selected
    }

    pub(super) fn is_strong_anchor(candidate: &EntryCandidate) -> bool {
        candidate.exact_match || candidate.lexical_hits > 0
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn find_chunk_candidates(
        &self, project: Option<&str>, question: &str, terms: &[String], path_hints: &[String],
        entry_candidates: &[EntryCandidate], route: RetrievalRoute, limit: usize,
        excluded_because: &mut Vec<String>, semantic_allowed: bool, runtime: &mut RetrievalRuntimeState,
    ) -> Vec<ChunkCandidate> {
        let entry_ids = entry_candidates.iter().map(|c| c.id.clone()).collect::<HashSet<_>>();
        let entry_uris = entry_candidates.iter().map(|c| c.uri.clone()).collect::<HashSet<_>>();
        let entry_id_match = if entry_ids.is_empty() { "1=0".to_string() }
            else { entry_ids.iter().map(|id| format!("c.source_id = '{}'", Self::escape_sql(id))).collect::<Vec<_>>().join(" OR ") };
        let entry_uri_match = if entry_uris.is_empty() { "1=0".to_string() }
            else { entry_uris.iter().map(|u| format!("f.path = '{}'", Self::escape_sql(u))).collect::<Vec<_>>().join(" OR ") };
        let lexical_predicate = Self::term_match_sql(terms, "c.content");
        let path_match = Self::path_match_sql(path_hints, "f.path");
        let lexical_uri_match = Self::term_match_sql(terms, "f.path");

        let semantic = if semantic_allowed {
            match crate::embedder::batch_embed(vec![question.to_string()]) {
                Ok(vectors) => { runtime.semantic_search_used = true; vectors.into_iter().next() }
                Err(err) => {
                    excluded_because.push("semantic_chunk_search_unavailable".to_string());
                    excluded_because.push(format!("semantic_chunk_search_error:{}", Self::truncate(&err.to_string(), 120)));
                    None
                }
            }
        } else { excluded_because.push("semantic_chunk_search_skipped".to_string()); None };

        if Self::route_prefers_operational_code(route) && (!entry_ids.is_empty() || !entry_uris.is_empty()) {
            let file_bindings = self.resolve_file_symbol_bindings(project, &entry_uris.iter().cloned().collect::<Vec<_>>());
            let mut source_to_uri = entry_candidates.iter().filter(|c| !c.uri.is_empty())
                .map(|c| (c.id.clone(), c.uri.clone())).collect::<HashMap<_, _>>();
            let mut same_file_source_ids = Vec::new();
            for (source_id, file_path) in file_bindings {
                source_to_uri.entry(source_id.clone()).or_insert(file_path.clone());
                same_file_source_ids.push(source_id);
            }
            let fast_path_ids = entry_ids.iter().cloned().chain(same_file_source_ids.iter().cloned()).collect::<HashSet<_>>();
            let fast_path_filter = if fast_path_ids.is_empty() { String::new() }
                else { let values = fast_path_ids.iter().map(|v| format!("'{}'", Self::escape_sql(v))).collect::<Vec<_>>().join(", ");
                    format!("c.source_id IN ({values})") };
            let anchored_query = format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                 CASE WHEN ({entry_id_match}) THEN 'entry_anchor' ELSE 'same_file' END \
                 FROM Chunk c WHERE ({fast_path_filter}){project_filter} LIMIT {limit}",
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]),
                limit = limit.min(12));
            let anchored_raw = self.graph_store.query_json(&anchored_query).unwrap_or_else(|_| "[]".to_string());
            let anchored_rows: Vec<Vec<Value>> = serde_json::from_str(&anchored_raw).unwrap_or_default();
            let anchored_candidates = anchored_rows.into_iter().filter_map(|row| {
                let chunk_id = row.first()?.as_str()?.to_string();
                let source_id = row.get(1)?.as_str()?.to_string();
                let project_code = row.get(2)?.as_str()?.to_string();
                let content = row.get(3)?.as_str()?.to_string();
                let chunk_part_index = Self::parse_usize_value(row.get(4)?).unwrap_or(1).max(1);
                let chunk_part_count = Self::parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
                let chunk_path = row.get(6)?.as_str().unwrap_or("1/1").to_string();
                let match_reason = row.get(7)?.as_str()?.to_string();
                let uri = source_to_uri.get(&source_id).cloned()
                    .or_else(|| if source_id.contains('/') { Some(source_id.clone()) } else { None }).unwrap_or_default();
                let lexical_hits = terms.iter().filter(|t| content.to_ascii_lowercase().contains(t.as_str()) || uri.to_ascii_lowercase().contains(t.as_str())).count();
                let anchored_to_entry = entry_ids.contains(&source_id);
                let same_file_as_entry = entry_uris.contains(&uri);
                Some(ChunkCandidate { chunk_id, source_id, project_code, uri, content, match_reason, lexical_hits,
                    semantic_distance: None, chunk_part_index, chunk_part_count, chunk_path, anchored_to_entry,
                    same_file_as_entry, score: 0.0, reasons: Vec::new() })
            }).collect::<Vec<_>>();
            if !anchored_candidates.is_empty() { return anchored_candidates; }
        }

        let query = if let Some(embedding) = semantic {
            // MIL-AXO-015 P6 read-side: cosine dialect swap. Under PG,
            // ChunkEmbedding.embedding is `vector(N)` (pgvector) and the
            // distance operator is `<=>`. Under DuckDB the legacy
            // `array_cosine_distance(... CAST(... AS FLOAT[N]))` form is
            // kept. (REQ-AXO-271 slice 1, 2026-05-10: the Parquet
            // side-store DuckDB-only fast path was removed.)
            let is_pg = self.graph_store.is_postgres_backend();
            let (embed_join, cosine_expr) = if is_pg {
                let join = format!(
                    "JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{model_id}' AND ce.source_hash = c.content_hash",
                    model_id = CHUNK_MODEL_ID
                );
                let vec_lit = match crate::postgres::vector::vector_literal(&embedding) {
                    Ok(lit) => lit,
                    Err(e) => {
                        // Dimension mismatch / non-finite: extremely
                        // rare. Drop semantic candidates and let the
                        // caller fall back to entry / lexical paths.
                        log::warn!(
                            "find_chunk_candidates: skipping pgvector literal under PG: {}",
                            e
                        );
                        excluded_because.push(format!("pgvector_literal_unavailable:{e}"));
                        return Vec::new();
                    }
                };
                (join, format!("(ce.embedding <=> {vec_lit})"))
            } else {
                let vector = format!("{embedding:?}");
                let join = format!(
                    "JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{model_id}' AND ce.source_hash = c.content_hash",
                    model_id = CHUNK_MODEL_ID
                );
                (
                    join,
                    format!(
                        "array_cosine_distance(ce.embedding, CAST({vector} AS FLOAT[{DIMENSION}]))"
                    ),
                )
            };
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                        CASE WHEN ({entry_id_match}) THEN 'entry_anchor' WHEN ({entry_uri_match}) THEN 'same_file' \
                             WHEN ({path_match}) THEN 'file_path' WHEN ({lexical_predicate}) THEN 'lexical+semantic' \
                             ELSE 'semantic' END, \
                        {cosine_expr} \
                 FROM Chunk c \
                 {embed_join} \
                 LEFT JOIN CONTAINS rel ON rel.target_id = c.source_id LEFT JOIN File f ON f.path = rel.source_id \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match}) \
                        OR {cosine_expr} < 0.55){project_filter} \
                 ORDER BY {cosine_expr} ASC LIMIT {limit}",
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]))
        } else {
            format!(
                "SELECT c.id, c.source_id, COALESCE(c.project_code, 'unknown'), COALESCE(f.path, ''), c.content, \
                        COALESCE(c.chunk_part_index, 1), COALESCE(c.chunk_part_count, 1), COALESCE(c.chunk_path, '1/1'), \
                        CASE WHEN ({entry_id_match}) THEN 'entry_anchor' WHEN ({entry_uri_match}) THEN 'same_file' \
                             WHEN ({path_match}) THEN 'file_path' ELSE 'lexical' END, NULL \
                 FROM Chunk c LEFT JOIN CONTAINS rel ON rel.target_id = c.source_id LEFT JOIN File f ON f.path = rel.source_id \
                 WHERE (({entry_id_match}) OR ({entry_uri_match}) OR ({lexical_predicate}) OR ({lexical_uri_match}) OR ({path_match})){project_filter} LIMIT {limit}",
                project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]))
        };

        let raw = self.graph_store.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let mut rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            if let Some(repo_root) = Self::project_repo_root(project) {
                let fallback_query = query.replacen(&Self::sql_project_filter_for_fields(project, &["c.project_code", "f.project_code"]), "", 1);
                let fallback_raw = self.graph_store.query_json(&fallback_query).unwrap_or_else(|_| "[]".to_string());
                let fallback_rows: Vec<Vec<Value>> = serde_json::from_str(&fallback_raw).unwrap_or_default();
                rows = fallback_rows.into_iter().filter(|row| row.get(3).and_then(|v| v.as_str()).map(|uri| uri.starts_with(&repo_root)).unwrap_or(false)).collect();
            }
        }
        let mut candidates = rows.into_iter().filter_map(|row| {
            let chunk_id = row.first()?.as_str()?.to_string();
            let source_id = row.get(1)?.as_str()?.to_string();
            let project_code = row.get(2)?.as_str()?.to_string();
            let uri = row.get(3)?.as_str().unwrap_or_default().to_string();
            let content = row.get(4)?.as_str()?.to_string();
            let chunk_part_index = Self::parse_usize_value(row.get(5)?).unwrap_or(1).max(1);
            let chunk_part_count = Self::parse_usize_value(row.get(6)?).unwrap_or(1).max(1);
            let chunk_path = row.get(7)?.as_str().unwrap_or("1/1").to_string();
            let match_reason = row.get(8)?.as_str()?.to_string();
            let semantic_distance = row.get(9).and_then(|v| v.as_f64());
            let lexical_hits = terms.iter().filter(|t| content.to_ascii_lowercase().contains(t.as_str()) || uri.to_ascii_lowercase().contains(t.as_str())).count();
            let anchored_to_entry = entry_ids.contains(&source_id);
            let same_file_as_entry = entry_uris.contains(&uri);
            Some(ChunkCandidate { chunk_id, source_id, project_code, uri, content, match_reason, lexical_hits,
                semantic_distance, chunk_part_index, chunk_part_count, chunk_path, anchored_to_entry,
                same_file_as_entry, score: 0.0, reasons: Vec::new() })
        }).collect::<Vec<_>>();
        if candidates.is_empty() {
            let (_, repo_chunks) = self.repo_literal_fallback_candidates(project, terms, limit);
            candidates.extend(repo_chunks);
        }
        candidates
    }

    pub(super) fn rerank_chunk_candidates(&self, candidates: &mut [ChunkCandidate], route: RetrievalRoute,
        terms: &[String], entry_candidates: &[EntryCandidate], project_scope_variants: &[String],
        prefer_project_intent: bool, linked_evidence_first: bool) {
        let entry_uris = entry_candidates.iter().map(|c| c.uri.to_ascii_lowercase()).collect::<HashSet<_>>();
        let scope_lc = project_scope_variants.iter().map(|v| v.to_ascii_lowercase()).collect::<HashSet<_>>();
        for candidate in candidates.iter_mut() {
            let mut score = (candidate.lexical_hits as f64) * 1.5;
            if candidate.anchored_to_entry { score += 5.0; candidate.reasons.push("anchored_to_entry".to_string()); }
            else if candidate.same_file_as_entry { score += 3.5; candidate.reasons.push("same_file_as_entry".to_string()); }
            if let Some(distance) = candidate.semantic_distance { score += (1.0 - distance).max(0.0) * 3.0; candidate.reasons.push("semantic_chunk_match".to_string()); }
            if candidate.chunk_part_count > 1 {
                score += 0.25; candidate.reasons.push("multipart_symbol_chunk".to_string());
                if candidate.chunk_part_index == 1 { score += 0.6; candidate.reasons.push("multipart_lead_chunk".to_string()); }
                else if candidate.chunk_part_index == 2 { score += 0.3; candidate.reasons.push("multipart_adjacent_continuation_bonus".to_string()); }
                else { score -= 0.35; candidate.reasons.push("multipart_late_chunk_penalty".to_string()); }
            }
            if scope_lc.contains(&candidate.project_code.to_ascii_lowercase()) { score += 1.0; candidate.reasons.push("project_scope_match".to_string()); }
            if matches!(route, RetrievalRoute::Hybrid | RetrievalRoute::SollHybrid) { score += 0.5; }
            if terms.iter().any(|t| candidate.content.to_ascii_lowercase().contains(t)) { score += 0.5; candidate.reasons.push("content_term_match".to_string()); }
            if entry_uris.contains(&candidate.uri.to_ascii_lowercase()) { score += 1.0; }
            if prefer_project_intent {
                let intent_weight = Self::project_intent_doc_weight(&candidate.uri);
                if intent_weight > 0.0 { score += intent_weight; candidate.reasons.push("intent_canonical_plan_bonus".to_string()); }
                else if intent_weight < 0.0 { score += intent_weight; candidate.reasons.push("intent_feedback_penalty".to_string()); }
            }
            if linked_evidence_first && !candidate.anchored_to_entry && !candidate.same_file_as_entry {
                let canonical_doc_weight = Self::canonical_project_doc_weight(&candidate.uri, project_scope_variants);
                if canonical_doc_weight > 0.0 { score += canonical_doc_weight; candidate.reasons.push("canonical_project_doc_bonus".to_string()); }
                let workspace_noise_penalty = Self::workspace_noise_penalty(&candidate.uri);
                if workspace_noise_penalty < 0.0 { score += workspace_noise_penalty; candidate.reasons.push("workspace_noise_penalty".to_string()); }
            }
            if Self::route_prefers_operational_code(route) {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) { score -= 2.0; candidate.reasons.push(reason.to_string()); }
            }
            if !candidate.anchored_to_entry && !candidate.same_file_as_entry && candidate.semantic_distance.is_some() && candidate.lexical_hits == 0 {
                score -= 1.0; candidate.reasons.push("generic_semantic_only_penalty".to_string());
            }
            candidate.score = score;
        }
        candidates.sort_by(|l, r| r.score.partial_cmp(&l.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| l.uri.cmp(&r.uri)));
    }

    pub(super) fn select_supporting_chunks(&self, candidates: &[ChunkCandidate], entry_candidates: &[EntryCandidate],
        route: RetrievalRoute, top_k: usize, token_budget: usize,
        excluded_because: &mut Vec<String>, diagnostics: &mut RetrievalDiagnostics) -> Vec<Value> {
        let mut selected = Vec::new();
        let mut selected_ids = HashSet::new();
        let mut seen_uris = HashSet::new();
        let mut selected_source_parts: HashMap<String, Vec<usize>> = HashMap::new();
        let mut consumed_tokens = 0usize;
        let chunk_cap = top_k.min(4);
        let has_anchor = entry_candidates.iter().any(Self::is_strong_anchor);
        let prefers_operational_code = Self::route_prefers_operational_code(route);
        let mut broader_selected = 0usize;
        let mut non_operational_selected = 0usize;

        let anchored = candidates.iter().filter(|c| c.anchored_to_entry).cloned().collect::<Vec<_>>();
        let same_file = candidates.iter().filter(|c| !c.anchored_to_entry && c.same_file_as_entry).cloned().collect::<Vec<_>>();
        let broader = candidates.iter().filter(|c| !c.anchored_to_entry && !c.same_file_as_entry).cloned().collect::<Vec<_>>();

        let ingest = |candidate: &ChunkCandidate, selected: &mut Vec<Value>, selected_ids: &mut HashSet<String>,
            seen_uris: &mut HashSet<String>, selected_source_parts: &mut HashMap<String, Vec<usize>>,
            consumed_tokens: &mut usize, diagnostics: &mut RetrievalDiagnostics| {
            if selected.len() >= chunk_cap { return; }
            if !selected_ids.insert(candidate.chunk_id.clone()) { return; }
            if !Self::can_reuse_uri_for_multipart(candidate, seen_uris, selected_source_parts) { return; }
            let snippet = Self::truncate(&candidate.content, 220);
            let estimated = Self::estimate_tokens(&[&snippet]);
            if *consumed_tokens + estimated > token_budget / 2 { return; }
            *consumed_tokens += estimated;
            seen_uris.insert(candidate.uri.clone());
            selected_source_parts.entry(candidate.source_id.clone()).or_default().push(candidate.chunk_part_index);
            if candidate.anchored_to_entry || candidate.same_file_as_entry { diagnostics.anchored_chunks_selected += 1; }
            else { diagnostics.unanchored_chunks_selected += 1; }
            if candidate.chunk_part_count > 1 { diagnostics.multipart_chunks_selected += 1; }
            selected.push(json!({
                "chunk_id": candidate.chunk_id, "source_id": candidate.source_id,
                "project_code": candidate.project_code, "uri": candidate.uri,
                "match_reason": candidate.match_reason, "evidence_class": "derived_chunk",
                "chunk_path": candidate.chunk_path, "chunk_part_index": candidate.chunk_part_index,
                "chunk_part_count": candidate.chunk_part_count, "anchored_to_entry": candidate.anchored_to_entry,
                "same_file_as_entry": candidate.same_file_as_entry, "snippet": snippet,
                "score": candidate.score, "ranking_reasons": candidate.reasons,
            }));
        };

        for candidate in &anchored { ingest(candidate, &mut selected, &mut selected_ids, &mut seen_uris, &mut selected_source_parts, &mut consumed_tokens, diagnostics); }
        for candidate in &same_file { ingest(candidate, &mut selected, &mut selected_ids, &mut seen_uris, &mut selected_source_parts, &mut consumed_tokens, diagnostics); }

        let anchored_selected = diagnostics.anchored_chunks_selected > 0;
        if has_anchor && !anchored.is_empty() && !anchored_selected {
            excluded_because.push("anchored_chunks_over_budget".to_string());
            return selected;
        }

        for candidate in &broader {
            if has_anchor && !anchored_selected { excluded_because.push("not_anchor_affine".to_string()); continue; }
            if has_anchor && prefers_operational_code {
                if let Some(reason) = Self::chunk_penalty_reason(candidate) {
                    excluded_because.push(reason.to_string());
                    if reason != "test_file_penalty" && reason != "docs_file_penalty" { excluded_because.push("non_operational_chunk_penalized".to_string()); }
                    continue;
                }
            }
            if broader_selected >= 1 { excluded_because.push("broader_semantic_dropped_due_to_anchor".to_string()); continue; }
            if candidate.semantic_distance.is_some() && candidate.lexical_hits == 0 { excluded_because.push("generic_semantic_only".to_string()); }
            if prefers_operational_code && Self::chunk_penalty_reason(candidate).is_some() {
                if non_operational_selected >= 1 { excluded_because.push("non_operational_chunk_penalized".to_string()); continue; }
                non_operational_selected += 1;
            }
            ingest(candidate, &mut selected, &mut selected_ids, &mut seen_uris, &mut selected_source_parts, &mut consumed_tokens, diagnostics);
            broader_selected += 1;
        }

        if prefers_operational_code && !same_file.is_empty() && broader_selected > 0 && diagnostics.anchored_chunks_selected > 0 {
            excluded_because.push("same_file_preferred".to_string());
        }
        diagnostics.multipart_symbol_groups_selected = selected_source_parts.values().filter(|parts| parts.len() > 1).count();
        selected
    }
}
