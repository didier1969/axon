use std::collections::{HashMap, HashSet};
use serde_json::Value;
use super::retrieval_model::ChunkCandidate;
use crate::mcp::McpServer;

impl McpServer {
    pub(crate) fn project_intent_doc_weight(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") { score += 4.0; }
        if lower.contains("concept-foundation") { score += 3.0; }
        if lower.contains("implementation-plan") { score += 3.0; }
        if lower.contains("feedback-axon") { score -= 6.0; }
        if lower.contains("operator") || lower.contains("retrospective") { score -= 2.0; }
        score
    }

    pub(super) fn canonical_project_doc_weight(uri: &str, project_scope_variants: &[String]) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") { score += 4.5; }
        if lower.contains("/docs/vision/") || lower.starts_with("docs/vision/") { score += 4.0; }
        if lower.contains("/docs/derived/soll/") || lower.starts_with("docs/derived/soll/") { score += 3.0; }
        if lower.ends_with("readme.md") || lower == "readme.md" { score += 1.5; }
        if project_scope_variants.iter().any(|variant| {
            let variant = variant.to_ascii_lowercase();
            lower.contains(&format!("/{variant}/")) || lower.contains(&format!("-{variant}-"))
        }) { score += 1.0; }
        score
    }

    pub(super) fn workspace_noise_penalty(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/.axon/") || lower.starts_with(".axon/") || lower.contains("/target/") || lower.starts_with("target/")
            || lower.contains("/tmp/") || lower.starts_with("/tmp/") || lower.contains("/scripts/") || lower.starts_with("scripts/") { -3.0 }
        else if lower.contains("feedback-") || lower.contains("/feedback/") { -2.5 }
        else { 0.0 }
    }

    pub(super) fn uri_penalty_reason(uri: &str) -> Option<&'static str> {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/tests/") || lower.contains("/test/") || lower.starts_with("tests/") || lower.starts_with("test/")
            || lower.ends_with("/tests.rs") || lower.ends_with("_test.exs") || lower.ends_with("_test.ex") || lower.ends_with("_test.rs")
        { Some("test_file_penalty") }
        else if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") { Some("docs_file_penalty") }
        else if lower.contains("/examples/") || lower.starts_with("examples/") { Some("non_operational_chunk_penalized") }
        else if lower.contains("/fixtures/") || lower.starts_with("fixtures/") { Some("non_operational_chunk_penalized") }
        else { None }
    }

    pub(super) fn chunk_penalty_reason(candidate: &ChunkCandidate) -> Option<&'static str> {
        if let Some(reason) = Self::uri_penalty_reason(&candidate.uri) { return Some(reason); }
        let source_lower = candidate.source_id.to_ascii_lowercase();
        let content_lower = candidate.content.to_ascii_lowercase();
        if source_lower.ends_with("::tests") || source_lower.contains("::test_")
            || content_lower.contains("fn test_") || content_lower.contains("mod tests") || content_lower.contains("#[test]")
        { Some("test_file_penalty") } else { None }
    }

    pub(super) fn term_match_sql(terms: &[String], column: &str) -> String {
        if terms.is_empty() { return "1=1".to_string(); }
        terms.iter().map(|term| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(term))).collect::<Vec<_>>().join(" OR ")
    }

    pub(super) fn path_match_sql(path_hints: &[String], column: &str) -> String {
        if path_hints.is_empty() { return "1=0".to_string(); }
        path_hints.iter().map(|hint| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(hint))).collect::<Vec<_>>().join(" OR ")
    }

    pub(crate) fn project_scope_variants(project: Option<&str>) -> Vec<String> {
        let Some(project) = project.map(str::trim).filter(|v| !v.is_empty()) else { return Vec::new(); };
        let mut values = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |value: String| { if !value.is_empty() && seen.insert(value.to_ascii_lowercase()) { values.push(value); } };
        push(project.to_string());
        push(project.to_ascii_lowercase());
        if let Ok(identity) = crate::project_meta::resolve_canonical_project_identity(project) {
            push(identity.code.clone());
            push(identity.code.to_ascii_lowercase());
            if let Some(repo_root) = identity.project_path.file_name().and_then(|name| name.to_str()) {
                push(repo_root.to_string());
                push(repo_root.to_ascii_lowercase());
            }
        }
        values
    }

    pub(crate) fn sql_project_filter_for_fields(project: Option<&str>, fields: &[&str]) -> String {
        let variants = Self::project_scope_variants(project);
        if variants.is_empty() || fields.is_empty() { return String::new(); }
        let values = variants.iter().map(|v| format!("'{}'", Self::escape_sql(&v.to_ascii_lowercase()))).collect::<Vec<_>>().join(", ");
        let predicates = fields.iter().map(|field| format!("lower({field}) IN ({values})")).collect::<Vec<_>>().join(" OR ");
        format!(" AND ({predicates})")
    }

    pub(super) fn truncate(value: &str, max_chars: usize) -> String {
        if value.chars().count() <= max_chars { return value.replace('\n', " "); }
        let mut end = value.len();
        for (count, (idx, _)) in value.char_indices().enumerate() { if count == max_chars { end = idx; break; } }
        format!("{}...", value[..end].replace('\n', " "))
    }

    pub(super) fn estimate_tokens(parts: &[&str]) -> usize {
        parts.iter().map(|part| part.chars().count() / 4 + 1).sum()
    }

    pub(super) fn escape_sql(value: &str) -> String { value.replace('\'', "''") }

    pub(super) fn parse_usize_value(value: &Value) -> Option<usize> {
        value.as_u64().and_then(|raw| usize::try_from(raw).ok())
            .or_else(|| value.as_i64().and_then(|raw| usize::try_from(raw.max(0)).ok()))
            .or_else(|| value.as_str().and_then(|raw| raw.parse::<usize>().ok()))
    }

    pub(super) fn can_reuse_uri_for_multipart(candidate: &ChunkCandidate, seen_uris: &HashSet<String>, selected_source_parts: &HashMap<String, Vec<usize>>) -> bool {
        if !seen_uris.contains(&candidate.uri) { return true; }
        if !candidate.anchored_to_entry && !candidate.same_file_as_entry { return false; }
        if candidate.chunk_part_count <= 1 { return false; }
        let Some(existing_parts) = selected_source_parts.get(&candidate.source_id) else { return false; };
        if existing_parts.len() >= 2 { return false; }
        !existing_parts.contains(&candidate.chunk_part_index)
            && existing_parts.iter().any(|existing| existing.abs_diff(candidate.chunk_part_index) <= 1)
    }
}
