//! REQ-AXO-219 — retrieval routing + SQL-fragment helpers extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). Pure associated
//! functions on `McpServer` (no `&self`); behavior-preserving move, `Self::…`
//! call sites unchanged. They build LIKE-fragments and decide route/intent bias
//! for `retrieve_context` hybrid retrieval.

use super::super::McpServer;
use super::retrieval_model::RetrievalRoute;

impl McpServer {
    pub(super) fn term_match_sql(terms: &[String], column: &str) -> String {
        if terms.is_empty() {
            return "1=1".to_string();
        }
        terms
            .iter()
            .map(|term| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(term)))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    pub(super) fn path_match_sql(path_hints: &[String], column: &str) -> String {
        if path_hints.is_empty() {
            return "1=0".to_string();
        }
        path_hints
            .iter()
            .map(|hint| format!("lower({column}) LIKE '%{}%'", Self::escape_sql(hint)))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    pub(super) fn has_rationale_language(question: &str) -> bool {
        let lower = question.to_ascii_lowercase();
        [
            "why",
            "rationale",
            "decision",
            "requirement",
            "constraint",
            "intent",
            "designed this way",
            "design choice",
            "architectural intent",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    pub(super) fn route_prefers_operational_code(route: RetrievalRoute) -> bool {
        matches!(
            route,
            RetrievalRoute::ExactLookup | RetrievalRoute::Wiring | RetrievalRoute::Impact
        )
    }

    pub(super) fn prefer_project_intent(question: &str, mode: Option<&str>) -> bool {
        if mode.is_some_and(|value| value.eq_ignore_ascii_case("intent")) {
            return true;
        }
        let lower = question.to_ascii_lowercase();
        [
            "soll mutation",
            "what soll mutation",
            "implementation plan",
            "concept foundation",
            "must support",
            "weekly plan",
            "project intent",
            "entrench",
            "recipe creation",
            "normalization",
            "attachment",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }
}
