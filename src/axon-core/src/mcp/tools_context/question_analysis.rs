//! REQ-AXO-219 — NL question-analysis helpers extracted from the
//! `tools_context.rs` god-file (APoSD deep-module split). These are pure
//! associated functions on `McpServer` (no `&self`); the move is
//! behavior-preserving — call sites keep using `Self::…` / `McpServer::…`
//! unchanged. They cover retrieval routing, tokenization, path-hint extraction,
//! and composed-question splitting for `retrieve_context` / `_layered`.

use super::super::McpServer;
use super::retrieval_model::RetrievalRoute;
use std::collections::HashSet;

impl McpServer {
    pub(super) fn plan_retrieval_route(question: &str) -> RetrievalRoute {
        let lower = question.to_ascii_lowercase();
        if lower.contains("what breaks if")
            || lower.contains("blast radius")
            || lower.contains("impact of")
            || lower.contains("if ") && (lower.contains(" changes") || lower.contains(" changed"))
        {
            RetrievalRoute::Impact
        } else if lower.contains("why ")
            || lower.contains("rationale")
            || lower.contains("decision")
            || lower.contains("requirement")
            || lower.contains("architectural intent")
        {
            RetrievalRoute::SollHybrid
        } else if lower.contains("where is")
            || lower.contains("wired")
            || lower.contains("hooked")
            || lower.contains("connected")
        {
            RetrievalRoute::Wiring
        } else if Self::looks_like_exact_lookup(question) {
            RetrievalRoute::ExactLookup
        } else {
            RetrievalRoute::Hybrid
        }
    }

    pub(crate) fn looks_like_exact_lookup(question: &str) -> bool {
        let trimmed = question.trim();
        let token_count = trimmed.split_whitespace().count();
        token_count <= 3
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.' | '-' | '/'))
    }

    pub(crate) fn question_terms(question: &str) -> Vec<String> {
        let stopwords = [
            "what",
            "breaks",
            "if",
            "why",
            "does",
            "use",
            "the",
            "where",
            "is",
            "wired",
            "hooked",
            "connected",
            "changes",
            "changed",
            "and",
            "for",
            "with",
            "this",
            "that",
            "from",
            "into",
            "how",
            "say",
            "about",
        ];
        let stopwords = stopwords.into_iter().collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        question
            .split(|ch: char| {
                !ch.is_ascii_alphanumeric()
                    && ch != '_'
                    && ch != ':'
                    && ch != '-'
                    && ch != '/'
                    && ch != '.'
            })
            .filter_map(|token| {
                let normalized = token.trim().to_ascii_lowercase();
                if normalized.len() < 3 || stopwords.contains(normalized.as_str()) {
                    return None;
                }
                if !seen.insert(normalized.clone()) {
                    return None;
                }
                Some(normalized)
            })
            .collect()
    }

    /// REQ-AXO-902023 tier C.2 — wh-interrogative cues that strongly OPEN a
    /// question (EN + FR). Deliberately excludes weak mid-sentence words
    /// (`is`/`are`/`do`) so the coordinator split stays high-precision.
    const INTERROGATIVE_CUES: [&'static str; 19] = [
        "how", "what", "why", "where", "when", "which", "who", "whose", "whom", "comment",
        "pourquoi", "où", "quand", "quel", "quelle", "quels", "quelles", "qui", "combien",
    ];

    /// REQ-AXO-902023 tier C.2 — detect a composed question carrying ≥2 distinct
    /// sub-questions and split it. Conservative (high precision): a false split
    /// degrades a single question, so we only split on strong signals —
    ///   1. ≥2 `?` terminators, each closing a substantive (≥2-word) clause; or
    ///   2. a coordinator (` and ` / ` et ` / `; `) whose right side OPENS with an
    ///      interrogative cue — a genuine second question, not a noun list
    ///      ("list X and Y" never splits; "...work and why is Y slow" does).
    /// Returns None when the question reads as a single ask.
    pub(crate) fn split_composed_question(question: &str) -> Option<Vec<String>> {
        let trimmed = question.trim();
        if trimmed.len() < 12 {
            return None;
        }
        // Rule 1 — multiple '?' terminators.
        if trimmed.matches('?').count() >= 2 {
            let parts: Vec<String> = trimmed
                .split_inclusive('?')
                .map(|segment| segment.trim())
                .filter(|segment| {
                    segment.ends_with('?')
                        && segment.trim_end_matches('?').split_whitespace().count() >= 2
                })
                .map(|segment| segment.to_string())
                .collect();
            if parts.len() >= 2 {
                return Some(parts);
            }
        }
        // Rule 2 — coordinator + interrogative right side.
        let parts = Self::split_on_interrogative_coordinator(trimmed);
        if parts.len() >= 2 {
            return Some(parts);
        }
        None
    }

    fn opens_with_interrogative(text: &str) -> bool {
        let first = text
            .trim_start()
            .split(|ch: char| !ch.is_alphanumeric() && ch != '\'')
            .find(|tok| !tok.is_empty())
            .unwrap_or("")
            .to_ascii_lowercase();
        Self::INTERROGATIVE_CUES.iter().any(|cue| *cue == first)
    }

    /// REQ-AXO-902023 tier C.2 — cut the question at each coordinator whose right
    /// side opens with an interrogative cue. Substantive parts only (≥2 words).
    pub(crate) fn split_on_interrogative_coordinator(question: &str) -> Vec<String> {
        const COORDINATORS: [&str; 3] = [" and ", " et ", "; "];
        let mut parts: Vec<String> = Vec::new();
        let mut rest = question;
        loop {
            let mut best: Option<(usize, usize)> = None; // (cut byte index, coord len)
            for coord in COORDINATORS {
                let mut from = 0;
                while let Some(pos) = rest[from..].find(coord) {
                    let idx = from + pos;
                    if Self::opens_with_interrogative(&rest[idx + coord.len()..]) {
                        if best.map_or(true, |(b, _)| idx < b) {
                            best = Some((idx, coord.len()));
                        }
                        break;
                    }
                    from = idx + coord.len();
                }
            }
            match best {
                Some((idx, coord_len)) => {
                    let left = rest[..idx].trim();
                    if left.split_whitespace().count() >= 2 {
                        parts.push(left.to_string());
                    }
                    rest = rest[idx + coord_len..].trim();
                }
                None => break,
            }
        }
        if !parts.is_empty() && rest.split_whitespace().count() >= 2 {
            parts.push(rest.to_string());
        }
        parts
    }

    pub(crate) fn question_path_hints(question: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        question
            .split_whitespace()
            .filter_map(|token| {
                let normalized = token
                    .trim_matches(|ch: char| {
                        matches!(ch, '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '(' | ')')
                    })
                    .trim();
                if normalized.is_empty() {
                    return None;
                }
                if !(normalized.contains('/') || normalized.contains('.')) {
                    return None;
                }
                let value = normalized.to_ascii_lowercase();
                if !seen.insert(value.clone()) {
                    return None;
                }
                Some(value)
            })
            .collect()
    }
}
