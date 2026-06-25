//! REQ-AXO-219 — repo-literal candidate helpers extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! functions on `McpServer`; behavior-preserving move, `Self::…` call sites
//! unchanged. They rank/filter on-disk paths and cut snippets for the
//! repo-literal fallback lane of `retrieve_context`.

use super::super::McpServer;
use super::util::truncate;

impl McpServer {
    pub(super) fn project_repo_root(project: Option<&str>) -> Option<String> {
        let project = project.map(str::trim).filter(|value| !value.is_empty())?;
        let identity = crate::project_meta::resolve_canonical_project_identity(project).ok()?;
        let repo_root = identity.meta_path.parent()?.parent()?;
        Some(repo_root.to_string_lossy().into_owned())
    }

    pub(super) fn is_strong_identifier_term(term: &str) -> bool {
        term.len() >= 4
            && term
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.'))
    }

    pub(super) fn repo_literal_file_rank(path: &str) -> i32 {
        let lower = path.to_ascii_lowercase();
        let mut score = 0i32;
        if lower.ends_with(".rs")
            || lower.ends_with(".ex")
            || lower.ends_with(".exs")
            || lower.ends_with(".py")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
        {
            score += 4;
        }
        if lower.contains("/src/") {
            score += 3;
        }
        if lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.starts_with("test/")
            || lower.starts_with("tests/")
        {
            score -= 4;
        }
        if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") {
            score -= 3;
        }
        score
    }

    pub(super) fn should_consider_repo_literal_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        if lower.contains("/.git/")
            || lower.contains("/target/")
            || lower.contains("/.axon/")
            || lower.contains("/node_modules/")
            || lower.contains("/dist/")
            || lower.contains("/build/")
            || lower.contains("/_build/")
            || lower.contains("/deps/")
            || lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.ends_with("/tests.rs")
            || lower.ends_with("_test.exs")
            || lower.ends_with("_test.ex")
            || lower.ends_with("_test.rs")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".test.js")
            || lower.contains("/docs/")
            || lower.ends_with(".md")
        {
            return false;
        }

        lower.ends_with(".rs")
            || lower.ends_with(".ex")
            || lower.ends_with(".exs")
            || lower.ends_with(".py")
            || lower.ends_with(".ts")
            || lower.ends_with(".tsx")
            || lower.ends_with(".js")
            || lower.ends_with(".jsx")
    }

    pub(super) fn snippet_around_term(content: &str, term: &str) -> Option<String> {
        let lower = content.to_ascii_lowercase();
        let needle = term.to_ascii_lowercase();
        let offset = lower.find(&needle)?;
        let start = offset.saturating_sub(100);
        let end = (offset + needle.len() + 120).min(content.len());
        Some(truncate(
            content.get(start..end).unwrap_or(content).trim(),
            220,
        ))
    }
}
