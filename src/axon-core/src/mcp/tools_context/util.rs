//! REQ-AXO-219 — leaf parsing/formatting helpers extracted from the
//! tools_context god-file. Pure, self-less, used only within tools_context.
use super::ChunkCandidate;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(super) fn confidence_label(score: f64) -> &'static str {
    if score >= 0.8 {
        "high"
    } else if score >= 0.55 {
        "medium"
    } else {
        "low"
    }
}
pub(super) fn parse_usize_value(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|raw| usize::try_from(raw).ok())
        .or_else(|| {
            value
                .as_i64()
                .and_then(|raw| usize::try_from(raw.max(0)).ok())
        })
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<usize>().ok()))
}

/// REQ-AXO-901883 — the native PG reader (`render_pg_value`) renders every
/// column as a JSON string (rows are serialised `Vec<Vec<String>>`), so a
/// `FLOAT8` cosine distance arrives as `"0.1778"`, not a JSON number, and a
/// SQL `NULL` arrives as the literal string `"null"`. A bare `Value::as_f64`
/// therefore returns `None` for a perfectly good distance — silently dropping
/// the semantic top-k's `semantic_distance`. This mirrors `parse_usize_value`:
/// accept a JSON number, or a numeric string, and treat the `"null"` sentinel
/// (and an empty string) as absent.
pub(super) fn parse_f64_value(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value.as_str().and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
                None
            } else {
                trimmed.parse::<f64>().ok()
            }
        })
    })
}

pub(super) fn can_reuse_uri_for_multipart(
    candidate: &ChunkCandidate,
    seen_uris: &HashSet<String>,
    selected_source_parts: &HashMap<String, Vec<usize>>,
) -> bool {
    if !seen_uris.contains(&candidate.uri) {
        return true;
    }
    if !candidate.anchored_to_entry && !candidate.same_file_as_entry {
        return false;
    }
    if candidate.chunk_part_count <= 1 {
        return false;
    }
    let Some(existing_parts) = selected_source_parts.get(&candidate.source_id) else {
        return false;
    };
    if existing_parts.len() >= 2 {
        return false;
    }
    !existing_parts.contains(&candidate.chunk_part_index)
        && existing_parts
            .iter()
            .any(|existing| existing.abs_diff(candidate.chunk_part_index) <= 1)
}

pub(super) fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.replace('\n', " ");
    }
    let mut end = value.len();
    for (count, (idx, _)) in value.char_indices().enumerate() {
        if count == max_chars {
            end = idx;
            break;
        }
    }
    format!("{}...", value[..end].replace('\n', " "))
}

pub(super) fn estimate_tokens(parts: &[&str]) -> usize {
    parts.iter().map(|part| part.chars().count() / 4 + 1).sum()
}
