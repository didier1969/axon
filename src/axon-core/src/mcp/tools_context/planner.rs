use std::collections::HashSet;

use super::retrieval_model::RetrievalRoute;
use crate::mcp::McpServer;

impl McpServer {
    pub(super) fn plan_retrieval_route(question: &str) -> RetrievalRoute {
        let lower = question.to_ascii_lowercase();
        if lower.contains("what breaks if") || lower.contains("blast radius") || lower.contains("impact of")
            || lower.contains("if ") && (lower.contains(" changes") || lower.contains(" changed"))
        { RetrievalRoute::Impact }
        else if lower.contains("why ") || lower.contains("rationale") || lower.contains("decision")
            || lower.contains("requirement") || lower.contains("architectural intent")
        { RetrievalRoute::SollHybrid }
        else if lower.contains("where is") || lower.contains("wired") || lower.contains("hooked") || lower.contains("connected")
        { RetrievalRoute::Wiring }
        else if Self::looks_like_exact_lookup(question) { RetrievalRoute::ExactLookup }
        else { RetrievalRoute::Hybrid }
    }

    pub(super) fn looks_like_exact_lookup(question: &str) -> bool {
        let trimmed = question.trim();
        let token_count = trimmed.split_whitespace().count();
        token_count <= 3 && trimmed.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '-' | '/'))
    }

    pub(super) fn question_terms(question: &str) -> Vec<String> {
        let stopwords = ["what","breaks","if","why","does","use","the","where","is","wired","hooked","connected","changes","changed","and","for","with","this","that","from","into","how","say","about"];
        let stopwords = stopwords.into_iter().collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        question.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != ':' && ch != '-' && ch != '/' && ch != '.')
            .filter_map(|token| {
                let normalized = token.trim().to_ascii_lowercase();
                if normalized.len() < 3 || stopwords.contains(normalized.as_str()) { return None; }
                if !seen.insert(normalized.clone()) { return None; }
                Some(normalized)
            }).collect()
    }

    pub(super) fn question_path_hints(question: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        question.split_whitespace().filter_map(|token| {
            let normalized = token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '(' | ')')).trim();
            if normalized.is_empty() { return None; }
            if !(normalized.contains('/') || normalized.contains('.')) { return None; }
            let value = normalized.to_ascii_lowercase();
            if !seen.insert(value.clone()) { return None; }
            Some(value)
        }).collect()
    }

    pub(super) fn has_rationale_language(question: &str) -> bool {
        let lower = question.to_ascii_lowercase();
        ["why","rationale","decision","requirement","constraint","intent","designed this way","design choice","architectural intent"]
            .iter().any(|needle| lower.contains(needle))
    }

    pub(super) fn route_prefers_operational_code(route: RetrievalRoute) -> bool {
        matches!(route, RetrievalRoute::ExactLookup | RetrievalRoute::Wiring | RetrievalRoute::Impact)
    }

    pub(super) fn prefer_project_intent(question: &str, mode: Option<&str>) -> bool {
        if mode.is_some_and(|v| v.eq_ignore_ascii_case("intent")) { return true; }
        let lower = question.to_ascii_lowercase();
        ["soll mutation","what soll mutation","implementation plan","concept foundation","must support","weekly plan","project intent","entrench","recipe creation","normalization","attachment"]
            .iter().any(|needle| lower.contains(needle))
    }
}
