//! REQ-AXO-219 — entry-candidate finders extracted from the `tools_context.rs`
//! god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` call sites unchanged. They find symbol /
//! exact-symbol / file entry candidates from PG (Symbol/Chunk) for
//! `retrieve_context` anchoring.

use super::super::McpServer;
use super::retrieval_model::EntryCandidate;
use serde_json::Value;
use std::collections::HashSet;

impl McpServer {
    pub(super) fn find_symbol_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        let mut candidates = self.find_exact_symbol_candidates(project, terms, limit);
        let name_match = Self::term_match_sql(terms, "s.name");
        let path_match = Self::path_match_sql(path_hints, "ch.file_path");
        let uri_term_match = Self::term_match_sql(terms, "ch.file_path");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(ch.file_path, '') \
             FROM Symbol s \
             LEFT JOIN Chunk ch ON ch.source_id = s.id AND ch.source_type = 'symbol' \
             WHERE ({name_match} OR {uri_term_match} OR {path_match}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "ch.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        candidates.extend(rows.into_iter().filter_map(|row| {
            let id = row.first()?.as_str()?.to_string();
            let name = row.get(1)?.as_str()?.to_string();
            let kind = row.get(2)?.as_str()?.to_string();
            let project_code = row.get(3)?.as_str()?.to_string();
            let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
            let lexical_hits = terms
                .iter()
                .filter(|term| {
                    name.to_ascii_lowercase().contains(term.as_str())
                        || uri.to_ascii_lowercase().contains(term.as_str())
                })
                .count();
            let exact_match = terms.iter().any(|term| name.eq_ignore_ascii_case(term))
                || path_hints.iter().any(|hint| uri.eq_ignore_ascii_case(hint));
            Some(EntryCandidate {
                id,
                name,
                kind,
                project_code,
                uri,
                lexical_hits,
                exact_match,
                score: 0.0,
                reasons: Vec::new(),
                semantic_distance: None,
            })
        }));
        candidates
    }

    pub(super) fn find_exact_symbol_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        let exact_terms = terms
            .iter()
            .filter(|term| Self::is_strong_identifier_term(term))
            .map(|term| term.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        if exact_terms.is_empty() {
            return Vec::new();
        }

        let exact_values = exact_terms
            .iter()
            .map(|term| format!("'{}'", Self::escape_sql(term)))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT s.id, s.name, s.kind, COALESCE(s.project_code, 'unknown'), COALESCE(ch.file_path, '') \
             FROM Symbol s \
             LEFT JOIN Chunk ch ON ch.source_id = s.id AND ch.source_type = 'symbol' \
             WHERE lower(s.name) IN ({exact_values}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["s.project_code", "ch.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                let id = row.first()?.as_str()?.to_string();
                let name = row.get(1)?.as_str()?.to_string();
                let kind = row.get(2)?.as_str()?.to_string();
                let project_code = row.get(3)?.as_str()?.to_string();
                let uri = row.get(4)?.as_str().unwrap_or_default().to_string();
                Some(EntryCandidate {
                    id,
                    name,
                    kind,
                    project_code,
                    uri,
                    lexical_hits: 1,
                    exact_match: true,
                    score: 0.0,
                    reasons: vec!["exact_symbol_lookup".to_string()],
                    semantic_distance: None,
                })
            })
            .collect()
    }

    pub(super) fn find_file_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
    ) -> Vec<EntryCandidate> {
        // REQ-AXO-901653 slice-5d — public.File dropped ; project_code per
        // file derives from ist.Chunk (canonical pivot post pipeline).
        let path_match = Self::path_match_sql(path_hints, "c.file_path");
        let term_match = Self::term_match_sql(terms, "c.file_path");
        let query = format!(
            "SELECT DISTINCT c.file_path, COALESCE(c.project_code, 'unknown') \
             FROM ist.Chunk c \
             WHERE c.file_path IS NOT NULL \
               AND ({path_match} OR {term_match}){project_filter} \
             LIMIT {limit}",
            project_filter = Self::sql_project_filter_for_fields(project, &["c.project_code"]),
        );

        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| {
                let path = row.first()?.as_str()?.to_string();
                let project_code = row.get(1)?.as_str()?.to_string();
                let lexical_hits = terms
                    .iter()
                    .filter(|term| path.to_ascii_lowercase().contains(term.as_str()))
                    .count();
                let exact_match = path_hints
                    .iter()
                    .any(|hint| path.eq_ignore_ascii_case(hint))
                    || terms.iter().any(|term| path.eq_ignore_ascii_case(term));
                Some(EntryCandidate {
                    id: path.clone(),
                    name: path.clone(),
                    kind: "file".to_string(),
                    project_code,
                    uri: path,
                    lexical_hits,
                    exact_match,
                    score: 0.0,
                    reasons: Vec::new(),
                    semantic_distance: None,
                })
            })
            .collect()
    }
}
