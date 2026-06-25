//! REQ-AXO-219 — retrieval URI/doc scoring helpers extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! functions on `McpServer` (no `&self`); behavior-preserving move, `Self::…`
//! call sites unchanged. They rank/penalize candidate URIs + chunks during
//! `retrieve_context` hybrid scoring.

use super::super::McpServer;
use super::retrieval_model::ChunkCandidate;

impl McpServer {
    pub(crate) fn project_intent_doc_weight(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") {
            score += 4.0;
        }
        if lower.contains("concept-foundation") {
            score += 3.0;
        }
        if lower.contains("implementation-plan") {
            score += 3.0;
        }
        if lower.contains("feedback-axon") {
            score -= 6.0;
        }
        if lower.contains("operator") || lower.contains("retrospective") {
            score -= 2.0;
        }
        score
    }

    pub(super) fn canonical_project_doc_weight(uri: &str, project_scope_variants: &[String]) -> f64 {
        let lower = uri.to_ascii_lowercase();
        let mut score = 0.0;
        if lower.contains("/docs/plans/") || lower.starts_with("docs/plans/") {
            score += 4.5;
        }
        if lower.contains("/docs/vision/") || lower.starts_with("docs/vision/") {
            score += 4.0;
        }
        if lower.contains("/docs/derived/soll/") || lower.starts_with("docs/derived/soll/") {
            score += 3.0;
        }
        if lower.ends_with("readme.md") || lower == "readme.md" {
            score += 1.5;
        }
        if project_scope_variants.iter().any(|variant| {
            let variant = variant.to_ascii_lowercase();
            lower.contains(&format!("/{variant}/")) || lower.contains(&format!("-{variant}-"))
        }) {
            score += 1.0;
        }
        score
    }

    pub(super) fn workspace_noise_penalty(uri: &str) -> f64 {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/.axon/")
            || lower.starts_with(".axon/")
            || lower.contains("/target/")
            || lower.starts_with("target/")
            || lower.contains("/tmp/")
            || lower.starts_with("/tmp/")
            || lower.contains("/scripts/")
            || lower.starts_with("scripts/")
        {
            -3.0
        } else if lower.contains("feedback-") || lower.contains("/feedback/") {
            -2.5
        } else {
            0.0
        }
    }

    pub(super) fn uri_penalty_reason(uri: &str) -> Option<&'static str> {
        let lower = uri.to_ascii_lowercase();
        if lower.contains("/tests/")
            || lower.contains("/test/")
            || lower.starts_with("tests/")
            || lower.starts_with("test/")
            || lower.ends_with("/tests.rs")
            || lower.ends_with("_test.exs")
            || lower.ends_with("_test.ex")
            || lower.ends_with("_test.rs")
        {
            Some("test_file_penalty")
        } else if lower.contains("/docs/") || lower.starts_with("docs/") || lower.ends_with(".md") {
            Some("docs_file_penalty")
        } else if lower.contains("/examples/") || lower.starts_with("examples/") {
            Some("non_operational_chunk_penalized")
        } else if lower.contains("/fixtures/") || lower.starts_with("fixtures/") {
            Some("non_operational_chunk_penalized")
        } else {
            None
        }
    }

    pub(super) fn chunk_penalty_reason(candidate: &ChunkCandidate) -> Option<&'static str> {
        if let Some(reason) = Self::uri_penalty_reason(&candidate.uri) {
            return Some(reason);
        }
        let source_lower = candidate.source_id.to_ascii_lowercase();
        let content_lower = candidate.content.to_ascii_lowercase();
        if source_lower.ends_with("::tests")
            || source_lower.contains("::test_")
            || content_lower.contains("fn test_")
            || content_lower.contains("mod tests")
            || content_lower.contains("#[test]")
        {
            Some("test_file_penalty")
        } else {
            None
        }
    }
}
