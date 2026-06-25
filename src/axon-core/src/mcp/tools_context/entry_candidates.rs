//! REQ-AXO-219 — entry-candidate finders extracted from the `tools_context.rs`
//! god-file (APoSD deep-module split). Methods on `McpServer`;
//! behavior-preserving move, `self.…` call sites unchanged. They find symbol /
//! exact-symbol / file entry candidates from PG (Symbol/Chunk) for
//! `retrieve_context` anchoring.

use super::super::McpServer;
use super::retrieval_model::{ChunkCandidate, EntryCandidate, RetrievalDiagnostics};
use super::util::truncate;
use super::CONTAINS_SYMBOL_CAP;
use ignore::WalkBuilder;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

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

    pub(super) fn find_entry_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        path_hints: &[String],
        limit: usize,
        diagnostics: &mut RetrievalDiagnostics,
    ) -> Vec<EntryCandidate> {
        let mut entries = self.find_symbol_candidates(project, terms, path_hints, limit);
        diagnostics.symbol_candidates_considered = entries.len();
        let file_candidates = self.find_file_candidates(project, terms, path_hints, limit);
        diagnostics.file_candidates_considered = file_candidates.len();
        entries.extend(file_candidates);
        if entries.is_empty() {
            if let Some(repo_root) = Self::project_repo_root(project) {
                let mut fallback = self.find_symbol_candidates(None, terms, path_hints, limit);
                fallback.retain(|candidate| candidate.uri.starts_with(&repo_root));
                diagnostics.symbol_candidates_considered += fallback.len();
                let mut fallback_files = self.find_file_candidates(None, terms, path_hints, limit);
                fallback_files.retain(|candidate| candidate.uri.starts_with(&repo_root));
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

    pub(super) fn repo_literal_fallback_candidates(
        &self,
        project: Option<&str>,
        terms: &[String],
        limit: usize,
    ) -> (Vec<EntryCandidate>, Vec<ChunkCandidate>) {
        let Some(repo_root) = Self::project_repo_root(project) else {
            return (Vec::new(), Vec::new());
        };
        let repo_root_path = Path::new(&repo_root);
        if !repo_root_path.exists() {
            return (Vec::new(), Vec::new());
        }

        let strong_terms = terms
            .iter()
            .filter(|term| Self::is_strong_identifier_term(term))
            .cloned()
            .collect::<Vec<_>>();
        if strong_terms.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let project_code = project
            .and_then(|value| crate::project_meta::resolve_canonical_project_identity(value).ok())
            .map(|identity| identity.code)
            .or_else(|| {
                project
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string());
        let mut matches = Vec::new();
        for entry in WalkBuilder::new(repo_root_path)
            .hidden(false)
            .standard_filters(true)
            .build()
        {
            let Ok(entry) = entry else {
                continue;
            };
            if !entry.file_type().map(|ty| ty.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy().into_owned();
            if !Self::should_consider_repo_literal_path(&path_str) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.len() > 512 * 1024 {
                continue;
            }
            let Ok(content) = fs::read_to_string(path) else {
                continue;
            };
            let content_lower = content.to_ascii_lowercase();
            let mut matched_terms = strong_terms
                .iter()
                .filter(|term| content_lower.contains(term.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if matched_terms.is_empty() {
                continue;
            }
            matched_terms.sort();
            let match_term = matched_terms[0].clone();
            let lexical_hits = matched_terms.len();
            let snippet = Self::snippet_around_term(&content, &match_term)
                .unwrap_or_else(|| truncate(content.lines().next().unwrap_or_default(), 220));
            matches.push((
                Self::repo_literal_file_rank(&path_str),
                lexical_hits,
                match_term,
                path_str,
                snippet,
                matched_terms,
            ));
        }

        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.3.cmp(&right.3))
        });

        let mut entries = Vec::new();
        let mut chunks = Vec::new();
        let mut seen_paths = HashSet::new();
        for (base_rank, lexical_hits, match_term, path_str, snippet, matched_terms) in matches {
            if !seen_paths.insert(path_str.clone()) {
                continue;
            }
            let reasons = vec![
                "repo_literal_fallback".to_string(),
                "repo_root_match".to_string(),
                "content_term_match".to_string(),
            ];
            entries.push(EntryCandidate {
                id: path_str.clone(),
                name: match_term.clone(),
                kind: "repo_literal".to_string(),
                project_code: project_code.clone(),
                uri: path_str.clone(),
                lexical_hits,
                exact_match: true,
                score: 4.0 + f64::from(base_rank.max(0)),
                reasons: reasons.clone(),
                semantic_distance: None,
            });
            chunks.push(ChunkCandidate {
                chunk_id: format!("repo_literal::{path_str}::{match_term}"),
                source_id: path_str.clone(),
                project_code: project_code.clone(),
                uri: path_str.clone(),
                content: snippet,
                match_reason: "repo_literal".to_string(),
                lexical_hits: matched_terms.len(),
                semantic_distance: None,
                chunk_part_index: 1,
                chunk_part_count: 1,
                chunk_path: "1/1".to_string(),
                anchored_to_entry: true,
                same_file_as_entry: true,
                score: 4.0 + f64::from(base_rank.max(0)),
                reasons,
                fts_rank: None,
            });
            if entries.len() >= limit.min(2) {
                break;
            }
        }

        (entries, chunks)
    }

    /// REQ-AXO-901952 — RAM-only forward CONTAINS (file → contained symbols).
    /// `file_project_pairs` carries `(file_path, project_code)` so the lookup
    /// scopes to the file's own per-project snapshot (derive-project pattern),
    /// never the legacy unscoped `ist.Edge` SQL. Returns `(symbol_id, file_path)`
    /// — same shape the superseded SQL emitted as `(target_id, source_id)`.
    /// Cold snapshot → that file is skipped (best-effort: retrieve_context
    /// still has FTS + vector arms), never a silent PG fallback.
    pub(super) fn resolve_file_symbol_bindings(
        &self,
        file_project_pairs: &[(String, String)],
    ) -> Vec<(String, String)> {
        if file_project_pairs.is_empty() {
            return Vec::new();
        }
        let ram_view = crate::ist_snapshot::process_view();
        let mut bindings = Vec::new();
        let mut seen_files = HashSet::new();
        for (file_path, project_code) in file_project_pairs {
            if file_path.is_empty()
                || project_code.is_empty()
                || !seen_files.insert((file_path.clone(), project_code.clone()))
                || !self.ensure_ram_snapshot_warm(project_code)
            {
                continue;
            }
            let Some(symbols) = ram_view.forward_at_radius(
                project_code,
                file_path,
                1,
                CONTAINS_SYMBOL_CAP,
                &[crate::ist_snapshot::snapshot::RelationType::Contains],
            ) else {
                continue;
            };
            for symbol_id in symbols {
                if symbol_id == *file_path {
                    continue;
                }
                bindings.push((symbol_id, file_path.clone()));
            }
        }
        bindings
    }

    /// REQ-AXO-901952 — RAM-only reverse CONTAINS (symbol → containing file).
    /// Replaces the per-row `(SELECT ce.source_id FROM ist.Edge … CONTAINS)`
    /// SQL fallback used when `Chunk.file_path` is NULL. Resolves from the
    /// row's own project snapshot; empty when cold / absent (display-only
    /// enrichment, so a miss is non-fatal — never a silent PG fallback).
    pub(super) fn resolve_containing_file_ram(&self, project_code: &str, symbol_id: &str) -> String {
        if project_code.is_empty()
            || symbol_id.is_empty()
            || !self.ensure_ram_snapshot_warm(project_code)
        {
            return String::new();
        }
        crate::ist_snapshot::process_view()
            .reverse_at_radius(
                project_code,
                symbol_id,
                1,
                1,
                &[crate::ist_snapshot::snapshot::RelationType::Contains],
            )
            .and_then(|files| files.into_iter().next())
            .unwrap_or_default()
    }
}
