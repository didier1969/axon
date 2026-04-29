use crate::embedding_profile::{
    runtime_chunk_profile, token_count_for_text, EmbeddingChunkProfile,
};
use crate::parser::Symbol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCodeChunk {
    pub content: String,
    pub part_index: usize,
    pub part_count: usize,
    pub chunk_path: String,
    pub estimated_tokens: usize,
    pub start_line: usize,
    pub end_line: usize,
}

pub fn active_chunk_profile() -> EmbeddingChunkProfile {
    runtime_chunk_profile()
}

pub fn should_accept_symbol_fast_path(profile: EmbeddingChunkProfile, content: &str) -> bool {
    content.chars().count() <= profile.small_symbol_char_fast_path
}

pub fn should_measure_symbol_tokens(profile: EmbeddingChunkProfile, content: &str) -> bool {
    let chars = content.chars().count();
    chars > profile.small_symbol_char_fast_path && chars <= profile.gray_zone_char_threshold
}

pub fn measured_symbol_token_count(content: &str) -> Option<usize> {
    token_count_for_text(content).ok()
}

fn fallback_estimated_token_count(content: &str) -> usize {
    content.chars().count().div_ceil(3).max(1)
}

fn estimated_token_count(content: &str) -> usize {
    measured_symbol_token_count(content).unwrap_or_else(|| fallback_estimated_token_count(content))
}

fn parse_property_usize(symbol: &Symbol, key: &str) -> Option<usize> {
    symbol.properties.get(key)?.parse::<usize>().ok()
}

fn parse_split_lines(symbol: &Symbol) -> Vec<usize> {
    symbol
        .properties
        .get("body_split_lines")
        .map(|value| {
            value
                .split(',')
                .filter_map(|part| part.trim().parse::<usize>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn explicit_symbol_chunk_layout(symbol: &Symbol, end: usize) -> Option<(usize, usize, usize)> {
    let body_start_line = parse_property_usize(symbol, "body_start_line")?;
    let body_end_line = parse_property_usize(symbol, "body_end_line")?;
    let header_end_line = parse_property_usize(symbol, "header_end_line")
        .unwrap_or(body_start_line.saturating_sub(1));

    let body_start = body_start_line.saturating_sub(1).min(end);
    let body_end = body_end_line.min(end).max(body_start);
    let header_end = header_end_line.min(body_start_line).min(end);
    let header_end_idx = header_end.min(body_start_line).min(end);

    if body_start >= body_end || header_end_idx < symbol.start_line.saturating_sub(1) {
        return None;
    }

    Some((
        symbol.start_line.saturating_sub(1),
        header_end_idx,
        body_start,
    ))
}

fn structural_header_line_count(lines: &[&str]) -> usize {
    let max_probe = lines.len().min(6);
    let mut count = 0usize;

    for line in lines.iter().take(max_probe) {
        let trimmed = line.trim();
        count += 1;
        if trimmed.is_empty() {
            break;
        }
        if trimmed.ends_with('{')
            || trimmed.ends_with(':')
            || trimmed == "do"
            || trimmed.ends_with(" do")
        {
            break;
        }
        if trimmed.contains('{') && !trimmed.starts_with("//") {
            break;
        }
    }

    count.min(lines.len())
}

fn indentation_depth(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

fn brace_depth_delta(line: &str) -> isize {
    let mut delta = 0isize;
    for ch in line.chars() {
        match ch {
            '{' | '(' | '[' => delta += 1,
            '}' | ')' | ']' => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn choose_structural_split_point(lines: &[&str], start: usize, end: usize) -> usize {
    let mid = start + ((end - start) / 2);
    let mut best: Option<(usize, i64)> = None;
    let mut depth_before = 0isize;

    for idx in start + 1..end {
        let prev = lines[idx - 1];
        depth_before += brace_depth_delta(prev);
        let next = lines[idx];
        let prev_trimmed = prev.trim();
        let next_trimmed = next.trim();
        let blank_boundary = prev_trimmed.is_empty() || next_trimmed.is_empty();
        let dedent_boundary = !prev_trimmed.is_empty()
            && !next_trimmed.is_empty()
            && indentation_depth(next) < indentation_depth(prev);
        let block_close_boundary =
            prev_trimmed.ends_with('}') || prev_trimmed == "end" || prev_trimmed.ends_with("end");
        let distance_penalty = (idx as i64 - mid as i64).abs() * 4;
        let score = (blank_boundary as i64 * 200)
            + (dedent_boundary as i64 * 120)
            + (block_close_boundary as i64 * 90)
            - ((depth_before.max(0) as i64) * 15)
            - distance_penalty;

        match best {
            Some((_, best_score)) if score <= best_score => {}
            _ => best = Some((idx, score)),
        }
    }

    best.map(|(idx, _)| idx)
        .unwrap_or_else(|| choose_split_point(lines, start, end))
        .clamp(start + 1, end.saturating_sub(1))
}

fn choose_explicit_split_point(symbol: &Symbol, start: usize, end: usize) -> Option<usize> {
    let mid = start + ((end - start) / 2);
    parse_split_lines(symbol)
        .into_iter()
        .map(|line| line.saturating_sub(1))
        .filter(|idx| *idx > start && *idx < end)
        .min_by_key(|idx| ((*idx as isize) - (mid as isize)).abs())
}

fn format_chunk_content(
    symbol: &Symbol,
    repeated_context: &str,
    snippet: &str,
    part_index: usize,
    part_count: usize,
) -> String {
    let docstring = symbol
        .docstring
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("docstring: {}\n", value))
        .unwrap_or_default();
    let part = if part_count > 1 {
        format!("part: {}/{}\n", part_index, part_count)
    } else {
        String::new()
    };
    let repeated_context = if repeated_context.trim().is_empty() {
        String::new()
    } else {
        format!("context:\n{}\n\n", repeated_context.trim_end())
    };

    format!(
        "symbol: {}\nkind: {}\n{}{}\
\n{}{}",
        symbol.name, symbol.kind, docstring, part, repeated_context, snippet
    )
}

fn choose_split_point(lines: &[&str], start: usize, end: usize) -> usize {
    let mid = start + ((end - start) / 2);
    for distance in 0..(end - start) {
        if mid >= start + distance {
            let idx = mid - distance;
            if idx > start && idx < end && lines[idx].trim().is_empty() {
                return idx;
            }
        }
        let idx = mid + distance;
        if idx > start && idx < end && idx < lines.len() && lines[idx].trim().is_empty() {
            return idx;
        }
    }
    mid.clamp(start + 1, end.saturating_sub(1))
}

fn recursive_symbol_ranges(
    profile: EmbeddingChunkProfile,
    symbol: &Symbol,
    lines: &[&str],
    repeated_context: &str,
    start: usize,
    end: usize,
    out: &mut Vec<(usize, usize)>,
) {
    if start >= end {
        return;
    }

    let snippet = lines[start..end].join("\n");
    let content = format_chunk_content(symbol, repeated_context, &snippet, 1, 1);
    let estimated_tokens = estimated_token_count(&content);
    let line_count = end - start;
    if estimated_tokens <= profile.target_chunk_tokens || line_count <= 1 {
        out.push((start, end));
        return;
    }

    let split = choose_explicit_split_point(symbol, start, end)
        .unwrap_or_else(|| choose_structural_split_point(lines, start, end));
    if split <= start || split >= end {
        out.push((start, end));
        return;
    }

    recursive_symbol_ranges(profile, symbol, lines, repeated_context, start, split, out);
    recursive_symbol_ranges(profile, symbol, lines, repeated_context, split, end, out);
}

pub fn build_symbol_chunks(symbol: &Symbol, file_content: &str) -> Vec<DerivedCodeChunk> {
    let profile = active_chunk_profile();
    let lines: Vec<&str> = file_content.lines().collect();
    let start = symbol.start_line.saturating_sub(1).min(lines.len());
    let end = symbol.end_line.min(lines.len()).max(start);
    let snippet = if start < end {
        lines[start..end].join("\n")
    } else {
        String::new()
    };
    let (context_start, context_end, recursive_start) = explicit_symbol_chunk_layout(symbol, end)
        .unwrap_or_else(|| {
            let repeated_context_end = if start < end {
                let symbol_lines = &lines[start..end];
                let header_count = structural_header_line_count(symbol_lines);
                if header_count >= symbol_lines.len() {
                    start
                } else {
                    (start + header_count).min(end)
                }
            } else {
                start
            };
            (start, repeated_context_end, repeated_context_end)
        });
    let repeated_context = if context_start < context_end {
        lines[context_start..context_end].join("\n")
    } else if start < end {
        let symbol_lines = &lines[start..end];
        let header_count = structural_header_line_count(symbol_lines);
        if header_count >= symbol_lines.len() {
            String::new()
        } else {
            symbol_lines[..header_count].join("\n")
        }
    } else {
        String::new()
    };
    let single_content = format_chunk_content(symbol, "", &snippet, 1, 1);
    let single_estimated_tokens = estimated_token_count(&single_content);
    let should_keep_single = if should_accept_symbol_fast_path(profile, &single_content) {
        true
    } else {
        single_estimated_tokens <= profile.target_chunk_tokens
    };

    if should_keep_single {
        return vec![DerivedCodeChunk {
            content: single_content,
            part_index: 1,
            part_count: 1,
            chunk_path: "1/1".to_string(),
            estimated_tokens: single_estimated_tokens,
            start_line: symbol.start_line,
            end_line: symbol.end_line,
        }];
    }

    let mut ranges = Vec::new();
    recursive_symbol_ranges(
        profile,
        symbol,
        &lines,
        &repeated_context,
        recursive_start,
        end,
        &mut ranges,
    );
    if ranges.is_empty() {
        ranges.push((start, end));
    }
    let part_count = ranges.len().max(1);
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, (range_start, range_end))| {
            let part_index = index + 1;
            let snippet = lines[range_start..range_end].join("\n");
            let content =
                format_chunk_content(symbol, &repeated_context, &snippet, part_index, part_count);
            DerivedCodeChunk {
                estimated_tokens: estimated_token_count(&content),
                content,
                part_index,
                part_count,
                chunk_path: format!("{}/{}", part_index, part_count),
                start_line: range_start + 1,
                end_line: range_end.max(range_start + 1),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Symbol;

    #[test]
    fn fast_path_and_gray_zone_follow_active_profile() {
        let profile = active_chunk_profile();
        let small = "a".repeat(profile.small_symbol_char_fast_path.saturating_sub(1));
        let gray = "b".repeat(profile.small_symbol_char_fast_path.saturating_add(8));
        assert!(should_accept_symbol_fast_path(profile, &small));
        assert!(!should_accept_symbol_fast_path(profile, &gray));
        assert!(should_measure_symbol_tokens(profile, &gray));
    }

    #[test]
    fn oversized_symbol_is_split_into_multiple_chunks() {
        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "32");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "64");
        }
        let symbol = Symbol {
            name: "huge_fn".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 9,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        };
        let content = [
            "fn huge_fn() {",
            "    let alpha = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let beta = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let gamma = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let delta = very_long_identifier_name_for_a_large_symbol_payload();",
            "}",
        ]
        .join("\n");
        let chunks = build_symbol_chunks(&symbol, &content);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.part_count == chunks.len()));
        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    #[test]
    fn multipart_chunks_repeat_structural_context() {
        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "32");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "64");
        }
        let symbol = Symbol {
            name: "shape_batches".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 10,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        };
        let content = [
            "fn shape_batches(",
            "    input: &[Chunk],",
            ") -> Vec<Batch> {",
            "    let alpha = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let beta = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let gamma = very_long_identifier_name_for_a_large_symbol_payload();",
            "    let delta = very_long_identifier_name_for_a_large_symbol_payload();",
            "}",
        ]
        .join("\n");
        let chunks = build_symbol_chunks(&symbol, &content);
        assert!(chunks.len() > 1);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.content.contains("context:\nfn shape_batches(")));
        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    #[test]
    fn explicit_body_bounds_override_header_guessing() {
        let mut properties = std::collections::HashMap::new();
        properties.insert("header_end_line".to_string(), "3".to_string());
        properties.insert("body_start_line".to_string(), "3".to_string());
        properties.insert("body_end_line".to_string(), "8".to_string());
        let symbol = Symbol {
            name: "shape_batches".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 8,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties,
            embedding: None,
        };
        let content = [
            "fn shape_batches(",
            "    input: &[Chunk],",
            ") -> Vec<Batch> {",
            "    let alpha = very_long_identifier_name_for_a_large_symbol_payload();",
            "",
            "    let beta = very_long_identifier_name_for_a_large_symbol_payload();",
            "    let gamma = very_long_identifier_name_for_a_large_symbol_payload();",
            "}",
        ]
        .join("\n");
        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "32");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "64");
        }
        let chunks = build_symbol_chunks(&symbol, &content);
        assert!(chunks.len() > 1);
        assert!(chunks[0].content.contains("context:\nfn shape_batches("));
        assert!(chunks[0].content.contains(") -> Vec<Batch> {"));
        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    #[test]
    fn explicit_body_split_lines_are_preferred() {
        let mut properties = std::collections::HashMap::new();
        properties.insert("body_split_lines".to_string(), "4,9".to_string());
        let symbol = Symbol {
            name: "shape_batches".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 12,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties,
            embedding: None,
        };
        assert_eq!(choose_explicit_split_point(&symbol, 0, 12), Some(8));
    }
}
