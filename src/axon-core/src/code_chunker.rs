use crate::embedding_profile::{
    content_token_count, runtime_chunk_profile, token_count_for_text, EmbeddingChunkProfile,
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

/// REQ-AXO-901894 — A segment of a symbol body the chunker will emit.
///
/// Either a contiguous run of whole lines (`LineRange`, the normal case) or a
/// char-window slice of a single over-long physical line (`CharWindow`, the
/// minified/generated giant-line fallback). Carrying both in one type lets the
/// emit tail produce the exact snippet text — line-joined vs char-sliced —
/// without re-deriving it from line numbers (which would duplicate a giant
/// line N times instead of slicing it).
#[derive(Debug, Clone, PartialEq, Eq)]
enum BodySegment {
    /// Whole lines `[start, end)` (0-based indices into `lines`).
    LineRange { start: usize, end: usize },
    /// Char window `[char_start, char_end)` of the single line `line` (0-based
    /// index into `lines`).
    CharWindow {
        line: usize,
        char_start: usize,
        char_end: usize,
    },
}

impl BodySegment {
    /// 1-based start line for `DerivedCodeChunk::start_line`.
    fn start_line(&self) -> usize {
        match self {
            BodySegment::LineRange { start, .. } => start + 1,
            BodySegment::CharWindow { line, .. } => line + 1,
        }
    }
    /// 1-based end line for `DerivedCodeChunk::end_line`.
    fn end_line(&self) -> usize {
        match self {
            BodySegment::LineRange { start, end } => (*end).max(start + 1),
            BodySegment::CharWindow { line, .. } => line + 1,
        }
    }
    /// The snippet text this segment contributes (pre-prefix).
    fn snippet(&self, lines: &[&str]) -> String {
        match self {
            BodySegment::LineRange { start, end } => lines[*start..*end].join("\n"),
            BodySegment::CharWindow {
                line,
                char_start,
                char_end,
            } => lines[*line]
                .chars()
                .skip(*char_start)
                .take(char_end - char_start)
                .collect(),
        }
    }
}

/// REQ-AXO-901894 — Optimal, linear-cost body segmentation (Knuth-Plass DP).
///
/// Replaces the divide-and-conquer `recursive_symbol_ranges` whose
/// `estimated_token_count` (full HuggingFace tokenizer encode) at *every*
/// recursion node was O(N²) full-tokenizer encodes for a large body span —
/// the gdb-proven cause of the 4-core, hours-long pipeline-A stall.
///
/// Strategy:
///  1. Cost every body line ONCE via `content_token_count` — O(N) encodes.
///  2. Prefix-sum → O(1) body-cost queries for any line window.
///  3. Windowed Knuth-Plass DP picks the segmentation minimizing total
///     "badness" (under-fill² + a boundary penalty for cutting at a line
///     that is NOT a preferred structural boundary). The inner scan is
///     bounded by the body budget (monotone cost ⇒ early break), so the DP
///     is O(N · window) ≈ O(N) in practice — never super-linear, no deep
///     recursion, no stack-overflow risk regardless of symbol size.
///  4. GIANT-LINE fallback: a single physical line that alone blows the
///     budget (minified/generated) is char-windowed instead.
///
/// REQ-AXO-901906 — wall-clock budget (ms) for chunking ONE symbol body before
/// the DP bails to cheap fixed line-windows. The DP is normally far under this
/// (200k lines ≈ 4.5 s), so only a genuine pathology trips it. Override via
/// `AXON_CHUNK_BUDGET_MS`.
const CHUNK_BUDGET_MS_DEFAULT: u64 = 15_000;
fn chunk_budget_ms() -> u64 {
    std::env::var("AXON_CHUNK_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(CHUNK_BUDGET_MS_DEFAULT)
}
/// Lines per chunk in the cheap fixed-window fallback (on budget bail).
const CHEAP_WINDOW_LINES: usize = 200;

/// Cheap O(N) fixed line-window segmentation (no tokenizer encodes). Contiguous,
/// gap-free `[body_start, end)` in `window`-line slabs. The defense fallback for
/// pathological bodies that slipped the upstream directory/minified/size filters.
fn cheap_line_window_segments(body_start: usize, end: usize, window: usize) -> Vec<BodySegment> {
    let window = window.max(1);
    let mut segments = Vec::new();
    let mut cursor = body_start;
    while cursor < end {
        let next = (cursor + window).min(end);
        segments.push(BodySegment::LineRange { start: cursor, end: next });
        cursor = next;
    }
    segments
}

/// Returns `BodySegment`s covering `[body_start, body_end)` in order.
/// Returns `(segments, bailed)` — `bailed` is true when the wall-clock
/// `deadline` was hit and we fell back to cheap fixed line-windows.
fn dp_segment_body(
    profile: EmbeddingChunkProfile,
    symbol: &Symbol,
    lines: &[&str],
    repeated_context: &str,
    body_start: usize,
    body_end: usize,
    deadline: std::time::Instant,
) -> (Vec<BodySegment>, bool) {
    if body_start >= body_end {
        return (Vec::new(), false);
    }

    let body_lines = &lines[body_start..body_end];
    let n = body_lines.len();

    // --- Fixed per-chunk prefix overhead (symbol/kind/docstring/part/context,
    // empty snippet). Subtracted from the model budget so the BODY itself
    // fits with the prefix that will be prepended to every chunk. ---
    let overhead = content_token_count(&format_chunk_content(symbol, repeated_context, "", 1, 2));
    let body_budget = profile.target_chunk_tokens.saturating_sub(overhead).max(8);

    // --- GIANT-LINE fallback (minified / generated code): if the body is a
    // single line, or any single line is so long that on its own it blows the
    // budget, the DP's per-line granularity cannot help. Char-window the
    // offending lines. Heuristic: ~3 chars/token (conservative for BGE on
    // source), so a body_budget*3-char window stays under budget. ---
    let char_per_token: usize = 3;
    let giant_char_threshold = body_budget.saturating_mul(4).max(1);
    let any_giant_line = body_lines
        .iter()
        .any(|line| line.chars().count() > giant_char_threshold);
    if n == 1 || any_giant_line {
        return (
            split_giant_lines(body_lines, body_start, body_budget, char_per_token),
            false,
        );
    }

    // --- Per-line token costs: O(N) encodes (the core fix). The wall-clock
    // deadline is checked here because this is where the tokenizer time is
    // spent; on a pathological body we bail to cheap fixed line-windows. ---
    let mut line_costs: Vec<usize> = Vec::with_capacity(n);
    for (i, line) in body_lines.iter().enumerate() {
        if i % 2048 == 0 && std::time::Instant::now() >= deadline {
            return (
                cheap_line_window_segments(body_start, body_end, CHEAP_WINDOW_LINES),
                true,
            );
        }
        line_costs.push(content_token_count(line));
    }

    // Prefix sum P[k] = sum of line_costs[0..k]. body_cost(a,b) ≈
    // (P[b]-P[a]) + (b-a-1) — the trailing term is the ~1 token per
    // inter-line newline that the joined snippet contributes.
    let mut prefix = vec![0usize; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + line_costs[i];
    }
    let body_cost = |a: usize, b: usize| -> usize {
        debug_assert!(a < b && b <= n);
        (prefix[b] - prefix[a]) + (b - a).saturating_sub(1)
    };

    // preferred_cut_before(i) for i in 1..=n: is a cut placed *before* body
    // line i a structurally clean boundary? (blank previous line / explicit
    // parser-provided split / dedent). i==n (end of body) is always clean.
    let explicit_splits: std::collections::HashSet<usize> = parse_split_lines(symbol)
        .into_iter()
        // parser lines are 1-based absolute; convert to 0-based body-local idx.
        .filter_map(|abs1| abs1.checked_sub(1))
        .filter(|abs0| *abs0 >= body_start && *abs0 < body_end)
        .map(|abs0| abs0 - body_start)
        .collect();
    let preferred_cut_before = |i: usize| -> bool {
        if i == 0 || i >= n {
            return true;
        }
        if body_lines[i - 1].trim().is_empty() {
            return true;
        }
        if explicit_splits.contains(&i) {
            return true;
        }
        indentation_depth(body_lines[i]) < indentation_depth(body_lines[i - 1])
    };

    // --- Windowed Knuth-Plass DP. dp[e] = min badness segmenting lines
    // [0..e); prev[e] = chosen segment start. ---
    let budget_f = body_budget as f64;
    let boundary_penalty = budget_f.powi(2) * 0.25;
    // Oversized single line (cost > budget) is unavoidable — it is emitted
    // whole and the embedder truncates it. Cost it with a large-but-finite
    // penalty so the DP still prefers feasible cuts elsewhere.
    let large_finite = budget_f.powi(2) * 16.0;

    let mut dp = vec![f64::INFINITY; n + 1];
    let mut prev = vec![0usize; n + 1];
    dp[0] = 0.0;

    for e in 1..=n {
        // Scan candidate segment starts s from e-1 downward; stop when the
        // window [s, e) exceeds budget (cost is monotone non-decreasing as s
        // shrinks, so the first overflow ends the feasible window).
        let mut s = e;
        let mut found = false;
        while s > 0 {
            s -= 1;
            let cost = body_cost(s, e);
            if cost > body_budget {
                break;
            }
            if dp[s].is_infinite() {
                continue;
            }
            found = true;
            let under = (body_budget - cost) as f64;
            let mut badness = under * under;
            if !(e == n || preferred_cut_before(e)) {
                badness += boundary_penalty;
            }
            let candidate = dp[s] + badness;
            if candidate < dp[e] {
                dp[e] = candidate;
                prev[e] = s;
            }
        }
        if !found {
            // Single line at e-1 alone exceeds budget (giant line that slipped
            // past the char-proxy gate). Emit it as its own oversized chunk.
            dp[e] = dp[e - 1] + large_finite;
            prev[e] = e - 1;
        }
    }

    // Reconstruct body-local ranges from n via prev, then map to absolute.
    let mut ranges = Vec::new();
    let mut e = n;
    while e > 0 {
        let s = prev[e];
        ranges.push(BodySegment::LineRange {
            start: body_start + s,
            end: body_start + e,
        });
        e = s;
    }
    ranges.reverse();
    (ranges, false)
}

/// REQ-AXO-901894 — Char-window fallback for minified / generated lines that
/// individually exceed the body budget. Splits each over-long line's TEXT into
/// `~budget*char_per_token`-char windows; short lines pass through whole.
fn split_giant_lines(
    body_lines: &[&str],
    body_start: usize,
    body_budget: usize,
    char_per_token: usize,
) -> Vec<BodySegment> {
    let window_chars = body_budget.saturating_mul(char_per_token).max(1);
    let mut segments = Vec::new();
    for (offset, line) in body_lines.iter().enumerate() {
        let abs = body_start + offset;
        let char_len = line.chars().count();
        if char_len <= window_chars {
            segments.push(BodySegment::LineRange {
                start: abs,
                end: abs + 1,
            });
        } else {
            let mut cs = 0usize;
            while cs < char_len {
                let ce = (cs + window_chars).min(char_len);
                segments.push(BodySegment::CharWindow {
                    line: abs,
                    char_start: cs,
                    char_end: ce,
                });
                cs = ce;
            }
        }
    }
    if segments.is_empty() {
        segments.push(BodySegment::LineRange {
            start: body_start,
            end: body_start + 1,
        });
    }
    segments
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
    let (context_start, context_end, body_start) = explicit_symbol_chunk_layout(symbol, end)
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

    // REQ-AXO-901906 — defense-in-depth TEMPORAL safety. The Knuth-Plass DP is
    // O(N) and fast (200k lines ≈ 4.5 s release), but an as-yet-unidentified
    // pathology (a generated/minified file that slipped the directory + minified
    // + size filters) could spin the per-line tokenizer on the A3 spawn_blocking
    // thread and stall the whole pipeline. If the wall-clock budget is exceeded
    // we bail to cheap fixed line-windows with BYTE-BASED token estimates (no
    // further tokenizer encodes) and LOG it so the offending file can be analysed.
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(chunk_budget_ms());
    let (mut segments, bailed) =
        dp_segment_body(profile, symbol, &lines, &repeated_context, body_start, end, deadline);
    if segments.is_empty() {
        segments.push(BodySegment::LineRange { start, end });
    }
    if bailed {
        tracing::warn!(
            target: "pipeline_v2::chunk",
            symbol = %symbol.name,
            kind = %symbol.kind,
            start_line = symbol.start_line,
            body_lines = end.saturating_sub(body_start),
            content_bytes = file_content.len(),
            budget_ms = chunk_budget_ms(),
            chunks = segments.len(),
            "REQ-AXO-901906 chunk budget exceeded — bailed to cheap byte-windowed chunking; INVESTIGATE this file/symbol"
        );
    }
    let part_count = segments.len().max(1);
    segments
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let part_index = index + 1;
            let snippet = segment.snippet(&lines);
            let content =
                format_chunk_content(symbol, &repeated_context, &snippet, part_index, part_count);
            DerivedCodeChunk {
                // On bail, byte-based estimate avoids re-entering the tokenizer
                // (the very thing that may be spinning) for every emitted chunk.
                estimated_tokens: if bailed {
                    fallback_estimated_token_count(&content)
                } else {
                    estimated_token_count(&content)
                },
                content,
                part_index,
                part_count,
                chunk_path: format!("{}/{}", part_index, part_count),
                start_line: segment.start_line(),
                end_line: segment.end_line(),
            }
        })
        .collect()
}

/// REQ-AXO-901746 — Minimum token count below which a single-part
/// chunk is a candidate for fusion with its neighbors.
const MIN_FUSE_TOKENS: usize = 100;

/// A chunk tagged with its originating symbol_id, ready for fusion.
#[derive(Debug, Clone)]
pub struct TaggedChunk {
    pub symbol_id: String,
    pub symbol_name: String,
    pub chunk: DerivedCodeChunk,
}

/// Fuse small adjacent single-part chunks into larger context groups.
///
/// Chunks with < `MIN_FUSE_TOKENS` tokens (and `part_count == 1`) are
/// merged with their neighbors until the group reaches
/// `target_tokens`. Large or multi-part chunks pass through unchanged.
/// Within each fused group, contents are joined with `\n\n` and the
/// chunk_id is derived from the first symbol in the group.
///
/// Returns `(chunk_id_suffix, content, estimated_tokens, start_line, end_line, source_symbol_id)`.
pub fn fuse_small_chunks(
    mut tagged: Vec<TaggedChunk>,
    target_tokens: usize,
) -> Vec<TaggedChunk> {
    if tagged.is_empty() {
        return tagged;
    }
    tagged.sort_by_key(|t| t.chunk.start_line);

    let mut result: Vec<TaggedChunk> = Vec::with_capacity(tagged.len());
    let mut group: Vec<TaggedChunk> = Vec::new();
    let mut group_tokens: usize = 0;
    // REQ-AXO-901846 — monotonic per-file fused-group counter. The line
    // range alone is NOT a unique key: duplicate-span symbols (macros,
    // decorators, re-exports, generated code) can split into several fused
    // groups that share (start_line, end_line); without the counter they
    // collapse to one id and the writer's ON CONFLICT (id) silently drops
    // all but one distinct content. Stable for an unchanged file (group
    // order is fixed by the start_line sort above).
    let mut fused_seq: usize = 0;

    let flush = |group: &mut Vec<TaggedChunk>,
                 group_tokens: &mut usize,
                 result: &mut Vec<TaggedChunk>,
                 fused_seq: &mut usize| {
        if group.is_empty() {
            return;
        }
        if group.len() == 1 {
            result.push(group.pop().unwrap());
            *group_tokens = 0;
            return;
        }
        let combined_content = group
            .iter()
            .map(|t| t.chunk.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let start_line = group.first().unwrap().chunk.start_line;
        let end_line = group.last().unwrap().chunk.end_line;
        // F-08: line-range-based ID (stable across minor symbol
        // additions/renames). REQ-AXO-901846: disambiguated by a per-file
        // fused-group sequence so identical spans never collide.
        let first = &group[0];
        let file_prefix = first.symbol_id
            .rsplit_once("::")
            .map(|(prefix, _)| prefix)
            .unwrap_or(&first.symbol_id);
        let seq = *fused_seq;
        *fused_seq += 1;
        let fused = TaggedChunk {
            symbol_id: format!("{file_prefix}::fused_L{start_line}_{end_line}_{seq}"),
            symbol_name: "fused_group".to_string(),
            chunk: DerivedCodeChunk {
                estimated_tokens: estimated_token_count(&combined_content),
                content: combined_content,
                part_index: 1,
                part_count: 1,
                chunk_path: "1/1".to_string(),
                start_line,
                end_line,
            },
        };
        result.push(fused);
        group.clear();
        *group_tokens = 0;
    };

    for item in tagged {
        let is_fusable =
            item.chunk.part_count == 1 && item.chunk.estimated_tokens < MIN_FUSE_TOKENS;

        if !is_fusable {
            flush(&mut group, &mut group_tokens, &mut result, &mut fused_seq);
            result.push(item);
            continue;
        }

        if group_tokens + item.chunk.estimated_tokens > target_tokens && !group.is_empty() {
            flush(&mut group, &mut group_tokens, &mut result, &mut fused_seq);
        }
        group_tokens += item.chunk.estimated_tokens;
        group.push(item);
    }
    flush(&mut group, &mut group_tokens, &mut result, &mut fused_seq);

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Symbol;
    use crate::test_support::env_test_lock;

    #[test]
    fn fast_path_and_gray_zone_follow_active_profile() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile = active_chunk_profile();
        let small = "a".repeat(profile.small_symbol_char_fast_path.saturating_sub(1));
        let gray = "b".repeat(profile.small_symbol_char_fast_path.saturating_add(8));
        assert!(should_accept_symbol_fast_path(profile, &small));
        assert!(!should_accept_symbol_fast_path(profile, &gray));
        assert!(should_measure_symbol_tokens(profile, &gray));
    }

    /// REQ-AXO-901846 — root cause regression: two fused groups in the
    /// same file that share a (start_line, end_line) span (duplicate-span
    /// symbols from macros / decorators / re-exports / generated code,
    /// split across groups by the token budget) MUST NOT collapse to the
    /// same fused chunk id. A collision means two DISTINCT chunk contents
    /// map to one id → silent loss via the writer's ON CONFLICT (id).
    #[test]
    fn fused_groups_with_identical_span_get_unique_ids() {
        let mk = |content: &str| TaggedChunk {
            symbol_id: "AXO::src__dup_rs::sym".to_string(),
            symbol_name: "sym".to_string(),
            chunk: DerivedCodeChunk {
                estimated_tokens: 1,
                content: content.to_string(),
                part_index: 1,
                part_count: 1,
                chunk_path: "1/1".to_string(),
                // identical span for every chunk — the pathological case.
                start_line: 10,
                end_line: 10,
            },
        };
        // 6 tiny fusable chunks + target_tokens=2 → forces ≥2 fused groups,
        // all sharing span L10_10 but carrying different content.
        let tagged: Vec<TaggedChunk> = (0..6).map(|i| mk(&format!("body-{i}"))).collect();
        let fused = fuse_small_chunks(tagged, 2);

        let ids: Vec<String> = fused.iter().map(|t| t.symbol_id.clone()).collect();
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            ids.len(),
            "fused chunk ids must be unique even for identical spans; got collisions: {ids:?}"
        );
    }

    #[test]
    fn oversized_symbol_is_split_into_multiple_chunks() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
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
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
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
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
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

    /// Helper: a minimal synthetic Symbol covering lines [1, end_line].
    fn synthetic_symbol(name: &str, end_line: usize) -> Symbol {
        Symbol {
            name: name.to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        }
    }

    /// REQ-AXO-901894 — parser-provided `body_split_lines` are honored as
    /// preferred cut boundaries by the DP. We feed a long body whose ONLY
    /// clean cut (no blank lines, uniform indentation) is the explicit split,
    /// and assert a chunk boundary lands there. (Re-anchored from the deleted
    /// `choose_explicit_split_point` unit — the new DP has no per-call split
    /// helper; the boundary preference is observable end-to-end.)
    #[test]
    fn explicit_body_split_lines_are_preferred() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "64");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "128");
        }
        // body lines 2..=13 (1-based); declare an explicit split before line 8.
        let mut properties = std::collections::HashMap::new();
        properties.insert("body_split_lines".to_string(), "8".to_string());
        let mut symbol = synthetic_symbol("shape_batches", 14);
        symbol.properties = properties;

        let mut content_lines = vec!["fn shape_batches() {".to_string()];
        for i in 0..12 {
            content_lines.push(format!(
                "    let v{i} = very_long_identifier_name_for_a_large_symbol_payload_{i}();"
            ));
        }
        content_lines.push("}".to_string());
        let content = content_lines.join("\n");

        let chunks = build_symbol_chunks(&symbol, &content);
        assert!(chunks.len() > 1, "expected a multi-part split");
        // A chunk boundary should land exactly at the explicit split (line 8
        // begins a new part). end_line of some chunk == 7, start_line of the
        // next == 8.
        let starts: Vec<usize> = chunks.iter().map(|c| c.start_line).collect();
        assert!(
            starts.contains(&8),
            "explicit split line 8 should begin a chunk; got starts {starts:?}"
        );
        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    /// REQ-AXO-901894 (a) — the class-killer: a 5000-line body chunks in well
    /// under a second (the old O(N²) tokenizer-encode recursion took hours),
    /// yields >1 contiguous gap-free chunk covering the whole body, and every
    /// chunk fits the model window.
    #[test]
    fn large_body_chunks_fast_contiguous_and_bounded() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile = active_chunk_profile();
        let n = 5000usize;
        let symbol = synthetic_symbol("giant_fn", n + 2);
        let mut content_lines = vec!["fn giant_fn() {".to_string()];
        for i in 0..n {
            // distinct code-like lines so per-line costs vary realistically.
            content_lines.push(format!(
                "    let value_{i} = compute_step_{i}(accumulator, factor_{i}, offset);"
            ));
        }
        content_lines.push("}".to_string());
        let content = content_lines.join("\n");

        let t0 = std::time::Instant::now();
        let chunks = build_symbol_chunks(&symbol, &content);
        let elapsed = t0.elapsed();

        // O(N²)-regression guard. The Knuth-Plass DP is near-linear: a true
        // O(N²) blowup over 5000 lines (~25M tokenizer-encode ops, the old
        // recursion "took hours") cannot fit even the generous debug bound.
        // The tight "sub-second class" target from REQ-AXO-901894 is a RELEASE
        // claim; the unoptimized cargo-test build is ~2-3x slower, so make the
        // ceiling profile-aware instead of flaking in debug.
        let max_elapsed = if cfg!(debug_assertions) {
            std::time::Duration::from_secs(12)
        } else {
            std::time::Duration::from_secs(2)
        };
        assert!(
            elapsed < max_elapsed,
            "5000-line body chunking took {elapsed:?} (limit {max_elapsed:?}; \
             O(N²) regression would be far slower)"
        );
        assert!(chunks.len() > 1, "expected multi-part split, got {}", chunks.len());
        // Contiguous, gap-free, in order.
        for w in chunks.windows(2) {
            assert_eq!(
                w[0].end_line + 1,
                w[1].start_line,
                "chunks must be contiguous: {:?} then {:?}",
                (w[0].start_line, w[0].end_line),
                (w[1].start_line, w[1].end_line)
            );
        }
        // Coverage: first chunk starts at the body, last ends at body end.
        assert_eq!(chunks.first().unwrap().start_line, 2);
        assert_eq!(chunks.last().unwrap().end_line, n + 2);
        // Every chunk fits the model window.
        for c in &chunks {
            let toks = token_count_for_text(&c.content).unwrap();
            assert!(
                toks <= profile.model_max_tokens,
                "chunk exceeds model window: {toks} > {}",
                profile.model_max_tokens
            );
        }
    }

    /// REQ-AXO-901894 (b) — a single ~200k-char minified line is char-windowed
    /// into multiple bounded chunks with no panic and no hang. The content is
    /// deliberately NON-repetitive: a repeated character (e.g. "xxxx") BPE-
    /// merges into a handful of tokens and legitimately fits one chunk, which
    /// would not exercise the windowing path.
    #[test]
    fn giant_single_line_is_windowed_and_bounded() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile = active_chunk_profile();
        let symbol = synthetic_symbol("minified", 1);
        // ~200k chars of varied tokens so the tokenizer cannot compress it —
        // simulates a real minified/generated single-line payload.
        let mut payload = String::with_capacity(220_000);
        let mut i = 0u64;
        while payload.len() < 200_000 {
            payload.push_str(&format!("k{i}=v{};", i.wrapping_mul(2654435761)));
            i += 1;
        }
        let content = format!("var data = {{{payload}}};");
        assert!(content.lines().count() == 1, "must be a single physical line");

        let t0 = std::time::Instant::now();
        let chunks = build_symbol_chunks(&symbol, &content);
        let elapsed = t0.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "giant-line chunking took {elapsed:?}"
        );
        assert!(
            chunks.len() > 1,
            "giant line must split into windows, got {}",
            chunks.len()
        );
        // Every chunk maps to the single physical line and fits the model window.
        for c in &chunks {
            assert_eq!(c.start_line, 1);
            assert_eq!(c.end_line, 1);
            let toks = token_count_for_text(&c.content).unwrap();
            assert!(
                toks <= profile.model_max_tokens,
                "windowed chunk exceeds model window: {toks} > {}",
                profile.model_max_tokens
            );
        }
    }

    /// REQ-AXO-901894 (c) — when a blank line sits near the budget boundary,
    /// the DP prefers cutting there (boundary penalty steers it to the clean
    /// structural seam over an arbitrary mid-block cut). Budget is sized so
    /// several lines fit per chunk and the natural fill point lands on/near
    /// the blank seam separating two blocks.
    #[test]
    fn cut_prefers_blank_line_near_boundary() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "128");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "64");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "128");
        }
        // Short, uniform-indent lines (~6 tokens each) so a ~100-token body
        // budget fits ~7 lines — one full block — and the cleanest cut is the
        // blank seam between the two blocks.
        let symbol = synthetic_symbol("two_blocks", 18);
        let block = |tag: &str| {
            (0..7)
                .map(|i| format!("    {tag}_{i} = step({i});"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let content = format!(
            "fn two_blocks() {{\n{}\n\n{}\n}}",
            block("alpha"),
            block("beta")
        );
        let chunks = build_symbol_chunks(&symbol, &content);
        assert!(chunks.len() > 1, "expected a split, got {}", chunks.len());

        let lines: Vec<&str> = content.lines().collect();
        let blank_1based = lines
            .iter()
            .position(|l| l.trim().is_empty())
            .map(|i| i + 1)
            .expect("a blank line exists");
        // A clean cut straddles the blank line: some chunk ends at/before the
        // blank and the next starts at/after it (blank consumed as boundary,
        // never split mid-block).
        let cut_at_blank = chunks
            .windows(2)
            .any(|w| w[0].end_line <= blank_1based && w[1].start_line >= blank_1based);
        assert!(
            cut_at_blank,
            "cut should straddle blank line {blank_1based}; chunk bounds: {:?}",
            chunks
                .iter()
                .map(|c| (c.start_line, c.end_line))
                .collect::<Vec<_>>()
        );
        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }
}
