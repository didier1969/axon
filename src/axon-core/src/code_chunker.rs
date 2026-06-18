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

/// REQ-AXO-901902/901906 — bytes above which the keep-single check refuses the
/// precise full-content tokenizer encode. `tokenizer.encode` pre-tokenizes the
/// ENTIRE body before truncation, so its cost scales with body size: on a
/// whole-file `document_body` symbol (TextParser on a 644 KB `LOG.txt`) the
/// single keep-single encode spun >90 s — *before* the `chunk_budget_ms`
/// deadline was even armed, so no downstream defense could interrupt it
/// (proven by `axon-diag-chunk-spin`). A body past this ceiling is provably
/// multi-chunk for any sane target (32 KiB ⇒ ~8k tokens ≫ `target_chunk_tokens`),
/// so we split it via the cheap byte estimate. Encodes up to the ceiling are
/// sub-10 ms, so exact keep-single behavior is preserved for every real symbol.
const MAX_PRECISE_ENCODE_BYTES: usize = 32 * 1024;

/// REQ-AXO-901902/901906 — maximum number of emitted chunks for which
/// `build_symbol_chunks` still computes a precise per-chunk `estimated_token_count`
/// (full tokenizer encode). Each encode is ~ms; a body fanning out into
/// thousands of chunks (giant-line fallback on a log, or a huge DP split) would
/// run thousands of un-budgeted encodes in the emission loop — the spin proven
/// on a 644 KB `LOG.txt` (~4.5k segments ⇒ >40 s, deadline-uncovered). Past the
/// cap the bodies are degenerate (data/log/minified) and the byte estimate is
/// sufficient. ~256 precise encodes stay well under the chunk wall-clock budget.
const MAX_PRECISE_ENCODE_CHUNKS: usize = 256;

/// REQ-AXO-901921 — hard ceiling on the number of chunks a SINGLE symbol may
/// emit. A sub-`DEGENERATE_BODY_BYTES` body cannot legitimately exceed
/// ~`DEGENERATE_BODY_BYTES / (target_chunk_tokens × SAFE_CHARS_PER_TOKEN)` ≈ 333
/// model-window chunks at full budget; a fan-out beyond this ceiling is proof
/// that `body_budget` collapsed (degenerate data/log/markdown body), so the
/// symbol is re-chunked coarsely to bound the COUNT, not just the encode time.
/// Set at 4× the precise-encode cap so legitimate large code symbols keep their
/// precise structural boundaries while true collapse (thousands of chunks) is
/// always caught.
const MAX_CHUNKS_PER_SYMBOL: usize = 4 * MAX_PRECISE_ENCODE_CHUNKS;

/// Conservative chars-per-token proxy for byte-based chunk sizing on the
/// degenerate fast path (BGE on source/log text rarely exceeds ~4 chars/token).
const SAFE_CHARS_PER_TOKEN: usize = 4;

/// REQ-AXO-901902/901906 — body byte length above which `build_symbol_chunks`
/// abandons the precise code-oriented path entirely (see
/// [`coarse_byte_window_chunks`]). A body this large cannot fit
/// `MAX_PRECISE_ENCODE_CHUNKS` model-window chunks, so it is provably degenerate
/// — a whole-file `document_body` text symbol, a giant log, generated data. The
/// precise path would touch the tokenizer on huge content at several sites (the
/// keep-single encode, the per-line DP cost loop, the per-chunk emission) and
/// collapse `body_budget` to its floor, fanning out into tens of thousands of
/// micro-chunks. Threshold = `MAX_PRECISE_ENCODE_CHUNKS` × a model window ×
/// `SAFE_CHARS_PER_TOKEN` (≈ 512 KiB): a body that cannot fit even 256 full
/// model-window chunks is not worth (or safe for) precise per-symbol chunking.
/// Real code symbols — even a 5 000-line function — sit below this and keep
/// precise chunking.
const DEGENERATE_BODY_BYTES: usize = 512 * 1024;

/// REQ-AXO-901902/901906 — cap on the `repeated_context` prepended to EVERY
/// emitted chunk. A context larger than this is both useless (it would alone
/// overflow the model window) and a per-chunk encode hazard: a `document_body`
/// whose header heuristic swallowed most of the file made the overhead encode
/// run on hundreds of KB. Real structural headers (a function signature, a class
/// preamble) are well under this; only a degenerate "header" is truncated.
const MAX_REPEATED_CONTEXT_CHARS: usize = 8 * 1024;

/// Token estimate for the *single-chunk keep/split* decision that NEVER feeds an
/// arbitrarily large body to the tokenizer (see [`MAX_PRECISE_ENCODE_BYTES`]).
/// Past the ceiling we floor the byte estimate at `target_chunk_tokens + 1` so a
/// token-dense body (`chars/3` can *undershoot* real tokens) is never mis-kept
/// as a single oversized chunk — past the ceiling the answer is always "split".
fn single_chunk_token_estimate(profile: EmbeddingChunkProfile, content: &str) -> usize {
    if content.len() > MAX_PRECISE_ENCODE_BYTES {
        fallback_estimated_token_count(content).max(profile.target_chunk_tokens + 1)
    } else {
        estimated_token_count(content)
    }
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

/// REQ-AXO-902024 fix C — wall-clock budget for chunking ALL of a file's symbols
/// (see [`build_file_chunks`]). `chunk_budget_ms` guards a single huge symbol; this
/// guards the symbol COUNT — a file that explodes into thousands of symbols (a
/// SOLL export's phantom refs) drowns the per-symbol tokenizer encode without any
/// single symbol tripping its own budget. Default 15 s: a real file chunks in
/// well under it; a pathological one degrades to coarse chunks instead of wedging.
const FILE_CHUNK_BUDGET_MS_DEFAULT: u64 = 15_000;
fn file_chunk_budget_ms() -> u64 {
    std::env::var("AXON_FILE_CHUNK_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(FILE_CHUNK_BUDGET_MS_DEFAULT)
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
        segments.push(BodySegment::LineRange {
            start: cursor,
            end: next,
        });
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
    // single line, or any single line is so long (cheap char proxy) that on its
    // own it blows the budget, the DP's per-line granularity cannot help.
    // Char-window the offending lines. The char proxy is a fast pre-filter; the
    // measured-cost gate below catches the dense lines it misses. ---
    let giant_char_threshold = body_budget.saturating_mul(4).max(1);
    let any_giant_line = body_lines
        .iter()
        .any(|line| line.chars().count() > giant_char_threshold);
    if n == 1 || any_giant_line {
        return (split_giant_lines(body_lines, body_start, body_budget), false);
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

    // REQ-AXO-901895 item #2 — a physical line whose MEASURED token cost alone
    // exceeds `body_budget` cannot be placed by the line-granular DP: it would
    // fall into the `!found` branch and be emitted WHOLE, then the embedder
    // silently truncates it (content dropped from the index). The cheap
    // char-proxy gate above misses DENSE lines (~1 token/char) shorter than
    // `giant_char_threshold`. Now that per-line costs are measured (and cheap —
    // the char gate already excluded the truly huge lines), route the body
    // through the data-driven char-windower, which guarantees every emitted
    // window fits the budget. This makes the DP `!found` branch below pure
    // defense-in-depth (it can no longer fire for a reachable input).
    if line_costs.iter().any(|cost| *cost > body_budget) {
        return (split_giant_lines(body_lines, body_start, body_budget), false);
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
        // REQ-AXO-901906 — defense-in-depth: the DP itself is checked against
        // the wall-clock deadline (the per-line cost loop above already is). A
        // pathological line-cost distribution could otherwise spin the inner
        // window scan with no bail. Cheap fixed line-windows on overrun.
        if e % 4096 == 0 && std::time::Instant::now() >= deadline {
            return (
                cheap_line_window_segments(body_start, body_end, CHEAP_WINDOW_LINES),
                true,
            );
        }
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
            // Defense-in-depth (REQ-AXO-901895 item #2): the measured-cost gate
            // above already routed any over-budget line to the char-windower, so
            // this is now unreachable for real input. Kept as a non-panicking
            // floor — emit the line as its own oversized chunk — in case a future
            // change weakens the gate.
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

/// REQ-AXO-901894 / REQ-AXO-901895 item #1 — char-window fallback for minified
/// / generated / dense lines that individually blow the body budget. Splits
/// each over-long line into windows of at most `body_budget` CHARS; shorter
/// lines pass through whole.
///
/// CORRECT-BY-CONSTRUCTION (kills the silent-truncation class): the WordPiece
/// tokenizer used by BGE never emits more tokens than characters — every
/// subword token spans >= 1 char — so a `body_budget`-char window can never
/// exceed `body_budget` tokens. No measurement, no per-window tokenizer encode
/// (this fallback path stays encode-free, so it never slows the B1 chunk-prep
/// stage), and no heuristic to mis-tune. The pre-fix code sized windows at
/// `body_budget * char_per_token` chars (~3 chars/token assumption) and emitted
/// shorter lines whole; on a DENSE line (~1 token/char: hex, CJK, dense
/// punctuation) that window encoded to up to `char_per_token × body_budget`
/// tokens, overflowed the model window, and the embedder SILENTLY truncated it
/// — content dropped from the index, hurting retrieval.
///
/// Trade-off: on SPARSE minified content (~3-4 chars/token) this emits more,
/// smaller windows than the old heuristic. That is the deliberate cost of
/// losslessness on a rare, low-retrieval-value fallback; the common code path
/// (the line-granular DP) is untouched.
fn split_giant_lines(
    body_lines: &[&str],
    body_start: usize,
    body_budget: usize,
) -> Vec<BodySegment> {
    let window_chars = body_budget.max(1);
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

/// REQ-AXO-901902/901906 — degenerate-body fast path. Splits `[start, end)` into
/// at most `MAX_PRECISE_ENCODE_CHUNKS` contiguous, gap-free coarse byte-windows
/// with byte-based token estimates and NO tokenizer encode, NO Knuth-Plass DP,
/// NO repeated structural context. Used for bodies past `DEGENERATE_BODY_BYTES`
/// (whole-file text/log/data symbols) whose per-symbol retrieval value is ~nil
/// and whose precise chunking spins the tokenizer for minutes.
fn coarse_byte_window_chunks(
    symbol: &Symbol,
    lines: &[&str],
    start: usize,
    end: usize,
    profile: EmbeddingChunkProfile,
) -> Vec<DerivedCodeChunk> {
    if start >= end {
        return Vec::new();
    }
    let body_bytes: usize = lines[start..end].iter().map(|l| l.len() + 1).sum();
    // Target ~one model window of bytes per chunk, but never exceed the cap.
    let by_window = profile
        .target_chunk_tokens
        .saturating_mul(SAFE_CHARS_PER_TOKEN)
        .max(1);
    let by_cap = body_bytes.div_ceil(MAX_PRECISE_ENCODE_CHUNKS).max(1);
    let bytes_per_chunk = by_window.max(by_cap);

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut seg_start = start;
    let mut acc = 0usize;
    for cursor in start..end {
        acc += lines[cursor].len() + 1;
        if acc >= bytes_per_chunk {
            ranges.push((seg_start, cursor + 1));
            seg_start = cursor + 1;
            acc = 0;
        }
    }
    if seg_start < end {
        ranges.push((seg_start, end));
    }
    if ranges.is_empty() {
        ranges.push((start, end));
    }

    let part_count = ranges.len();
    ranges
        .into_iter()
        .enumerate()
        .map(|(i, (s, e))| {
            let part_index = i + 1;
            let snippet = lines[s..e].join("\n");
            let content = format_chunk_content(symbol, "", &snippet, part_index, part_count);
            DerivedCodeChunk {
                estimated_tokens: fallback_estimated_token_count(&content),
                content,
                part_index,
                part_count,
                chunk_path: format!("{}/{}", part_index, part_count),
                start_line: s + 1,
                end_line: e,
            }
        })
        .collect()
}

/// REQ-AXO-902024 fix B+C — build the chunks for ALL of a file's symbols in one
/// pass. Splits `file_content` into lines ONCE (B: was re-split per symbol →
/// O(symbols × N)), and enforces a per-FILE wall-clock budget (C): once spent,
/// the remaining symbols fall back to coarse byte-window chunks (tokenizer-free)
/// and a single WARN names the file for investigation — so a file dense in
/// matched ids (a SOLL export → thousands of phantom symbols) or otherwise
/// drowning the per-symbol encodes can NEVER wedge plane A. Returns
/// `(symbol_index, chunk)` so the caller maps each chunk back to its symbol id.
pub fn build_file_chunks(
    symbols: &[&Symbol],
    file_content: &str,
) -> Vec<(usize, DerivedCodeChunk)> {
    build_file_chunks_with_budget(symbols, file_content, file_chunk_budget_ms())
}

/// Testable seam for [`build_file_chunks`]: `budget_ms = 0` forces the coarse
/// fallback for EVERY symbol deterministically (deadline already passed), so the
/// never-wedge path is exercised without depending on wall-clock timing.
pub fn build_file_chunks_with_budget(
    symbols: &[&Symbol],
    file_content: &str,
    budget_ms: u64,
) -> Vec<(usize, DerivedCodeChunk)> {
    let profile = active_chunk_profile();
    let lines: Vec<&str> = file_content.lines().collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
    let mut out: Vec<(usize, DerivedCodeChunk)> = Vec::new();
    let mut bailed = false;
    for (i, sym) in symbols.iter().enumerate() {
        if !bailed && std::time::Instant::now() >= deadline {
            bailed = true;
            tracing::warn!(
                target: "pipeline_v2::chunk",
                symbols = symbols.len(),
                chunked_fine = i,
                content_bytes = file_content.len(),
                budget_ms = file_chunk_budget_ms(),
                "REQ-AXO-902024 per-FILE chunk budget exceeded — remaining symbols fall back to coarse byte-window chunks (no tokenizer). INVESTIGATE this file (likely dense in matched ids / generated)."
            );
        }
        if bailed {
            let start = sym.start_line.saturating_sub(1).min(lines.len());
            let end = sym.end_line.min(lines.len()).max(start);
            let mut coarse = coarse_byte_window_chunks(sym, &lines, start, end, profile);
            if coarse.is_empty() {
                // single-line / empty-body symbol: emit one cheap byte-estimated
                // chunk so the content is still indexed (coarse, not skipped).
                let snippet = if start < lines.len() {
                    lines[start].to_string()
                } else {
                    String::new()
                };
                let content = format_chunk_content(sym, "", &snippet, 1, 1);
                coarse.push(DerivedCodeChunk {
                    estimated_tokens: fallback_estimated_token_count(&content),
                    content,
                    part_index: 1,
                    part_count: 1,
                    chunk_path: "1/1".to_string(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                });
            }
            for chunk in coarse {
                out.push((i, chunk));
            }
        } else {
            for chunk in build_symbol_chunks_with_lines(sym, &lines, file_content) {
                out.push((i, chunk));
            }
        }
    }
    out
}

pub fn build_symbol_chunks(symbol: &Symbol, file_content: &str) -> Vec<DerivedCodeChunk> {
    let lines: Vec<&str> = file_content.lines().collect();
    build_symbol_chunks_with_lines(symbol, &lines, file_content)
}

/// REQ-AXO-902024 fix B — lines-aware variant. `build_symbol_chunks` re-split the
/// whole file (`file_content.lines().collect()`, O(N)) on EVERY call, and it is
/// called once per symbol → O(symbols × N) for the file. Callers that chunk many
/// symbols of one file (see [`build_file_chunks`]) split once and reuse the slice.
/// `file_content` is kept only for its O(1) byte length in the budget WARN.
pub fn build_symbol_chunks_with_lines(
    symbol: &Symbol,
    lines: &[&str],
    file_content: &str,
) -> Vec<DerivedCodeChunk> {
    let profile = active_chunk_profile();
    let start = symbol.start_line.saturating_sub(1).min(lines.len());
    let end = symbol.end_line.min(lines.len()).max(start);

    // REQ-AXO-901902/901906 — DEGENERATE-BODY FAST PATH (root fix). Short-circuit
    // BEFORE building any large intermediate string (snippet/repeated_context)
    // or touching the tokenizer. A 644 KB `LOG.txt` `document_body` symbol drove
    // every precise-path encode site on huge content (>90 s) and collapsed
    // `body_budget` to its floor (28 769 micro-chunks, then `fuse_small_chunks`
    // choked). Coarse byte-windows bound BOTH time and fan-out for every parser.
    let body_byte_len: usize = if start < end {
        lines[start..end].iter().map(|l| l.len() + 1).sum()
    } else {
        0
    };
    if body_byte_len > DEGENERATE_BODY_BYTES {
        return coarse_byte_window_chunks(symbol, lines, start, end, profile);
    }

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
    // REQ-AXO-901902/901906 — bound the repeated context: it is prepended to
    // EVERY emitted chunk AND fed to the `overhead` token encode below. A header
    // heuristic that swallowed most of a degenerate body would otherwise run
    // that encode on hundreds of KB. Truncate on a char boundary; real headers
    // are far under the cap.
    let repeated_context = if repeated_context.len() > MAX_REPEATED_CONTEXT_CHARS {
        repeated_context
            .chars()
            .take(MAX_REPEATED_CONTEXT_CHARS)
            .collect()
    } else {
        repeated_context
    };
    let single_content = format_chunk_content(symbol, "", &snippet, 1, 1);
    let single_estimated_tokens = single_chunk_token_estimate(profile, &single_content);
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(chunk_budget_ms());
    let (mut segments, bailed) = dp_segment_body(
        profile,
        symbol,
        lines,
        &repeated_context,
        body_start,
        end,
        deadline,
    );
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
    // REQ-AXO-901921 — bound the chunk COUNT (not just the encode time). A body
    // UNDER `DEGENERATE_BODY_BYTES` (so it took the precise path) whose
    // `body_budget` collapses toward its floor — repeated_context overhead, or
    // a giant-line fallback with a tiny window — fans out into THOUSANDS of
    // ~tiny segments. The 901902 cap byte-estimated the encode past
    // `MAX_PRECISE_ENCODE_CHUNKS` (time bounded) but still EMITTED every
    // micro-chunk: pure embedding noise (GUI-PRO-107 L8) + wasted B-stage work.
    // A sub-512 KiB body cannot legitimately exceed ~333 model-window chunks at
    // full budget, so a fan-out past `MAX_CHUNKS_PER_SYMBOL` is proof of budget
    // collapse on a low-value data/log/markdown body: re-chunk it coarsely into
    // bounded (≤ MAX_PRECISE_ENCODE_CHUNKS) model-window byte windows, the same
    // treatment >512 KiB bodies already get. Threshold sits well above any
    // legitimate large code symbol so precise structural boundaries are kept
    // for real code.
    if part_count > MAX_CHUNKS_PER_SYMBOL {
        tracing::warn!(
            target: "pipeline_v2::chunk",
            symbol = %symbol.name,
            kind = %symbol.kind,
            start_line = symbol.start_line,
            content_bytes = file_content.len(),
            fan_out = part_count,
            ceiling = MAX_CHUNKS_PER_SYMBOL,
            "REQ-AXO-901921 chunk fan-out exceeded per-symbol ceiling — coarse byte-window re-chunk to bound COUNT (budget-collapsed non-code data/log/markdown body)"
        );
        return coarse_byte_window_chunks(symbol, lines, start, end, profile);
    }
    // REQ-AXO-901902 — bound the encode TIME for legitimate-but-large bodies in
    // the `MAX_PRECISE_ENCODE_CHUNKS`..`MAX_CHUNKS_PER_SYMBOL` band: keep their
    // precise segment boundaries but byte-estimate tokens to avoid an encode
    // storm (the per-chunk tokenizer encode is the cost, and the DP wall-clock
    // deadline does NOT cover this loop).
    let use_byte_estimate = bailed || part_count > MAX_PRECISE_ENCODE_CHUNKS;
    if use_byte_estimate && !bailed {
        tracing::warn!(
            target: "pipeline_v2::chunk",
            symbol = %symbol.name,
            kind = %symbol.kind,
            start_line = symbol.start_line,
            content_bytes = file_content.len(),
            chunks = part_count,
            cap = MAX_PRECISE_ENCODE_CHUNKS,
            "REQ-AXO-901902 chunk fan-out exceeded precise-encode cap — byte-estimating tokens; INVESTIGATE this file/symbol (likely non-code data/log/config)"
        );
    }
    segments
        .into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let part_index = index + 1;
            let snippet = segment.snippet(lines);
            let content =
                format_chunk_content(symbol, &repeated_context, &snippet, part_index, part_count);
            DerivedCodeChunk {
                // Byte-based estimate avoids re-entering the tokenizer (the very
                // thing that spins) for every emitted chunk on bail OR fan-out.
                estimated_tokens: if use_byte_estimate {
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
/// REQ-AXO-901895 item #4 — only genuinely tiny SINGLE-part symbols are fused.
/// Multi-part chunks (`part_count > 1`, the output of the Knuth-Plass body DP)
/// are deliberately NOT fused: each is already sized to the budget, and merging
/// them would re-create the oversized spans the DP exists to split. Earlier
/// notes that fusion "rescues tiny DP parts" were inaccurate — DP parts carry
/// `part_count > 1` and never enter the fuse candidate set below.
///
/// Returns `(chunk_id_suffix, content, estimated_tokens, start_line, end_line, source_symbol_id)`.
pub fn fuse_small_chunks(mut tagged: Vec<TaggedChunk>, target_tokens: usize) -> Vec<TaggedChunk> {
    if tagged.is_empty() {
        return tagged;
    }
    // REQ-AXO-901902/901917 — defense: the per-group `estimated_token_count`
    // below is bounded in CONTENT size (groups flush at `target_tokens`) but NOT
    // in COUNT. A file that fans out into thousands of fusable chunks (phantom-
    // heavy source, or a degenerate body before the upstream cap) would run one
    // encode per group — the second spin site observed on the 644 KB LOG.txt
    // (28 769 chunks ⇒ `fuse` choked after `build`). Past the cap, byte-estimate
    // the fused groups too; precise counts add no value at that fan-out.
    let byte_estimate_fused = tagged.len() > MAX_PRECISE_ENCODE_CHUNKS;
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
        let file_prefix = first
            .symbol_id
            .rsplit_once("::")
            .map(|(prefix, _)| prefix)
            .unwrap_or(&first.symbol_id);
        let seq = *fused_seq;
        *fused_seq += 1;
        let fused = TaggedChunk {
            symbol_id: format!("{file_prefix}::fused_L{start_line}_{end_line}_{seq}"),
            symbol_name: "fused_group".to_string(),
            chunk: DerivedCodeChunk {
                estimated_tokens: if byte_estimate_fused {
                    fallback_estimated_token_count(&combined_content)
                } else {
                    estimated_token_count(&combined_content)
                },
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

    /// REQ-AXO-902024 fix B+C — `build_file_chunks` chunks every symbol of a file
    /// (the lines-once / per-file-budget path that replaced the per-symbol
    /// file re-split). Each chunk is tagged with its originating symbol index.
    #[test]
    fn build_file_chunks_covers_every_symbol() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let content = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mk = |name: &str, line: usize| Symbol {
            name: name.to_string(),
            kind: "function".to_string(),
            start_line: line,
            end_line: line,
            docstring: None,
            is_entry_point: false,
            is_public: false,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        };
        let syms = [mk("a", 1), mk("b", 2), mk("c", 3)];
        let refs: Vec<&Symbol> = syms.iter().collect();
        let chunks = build_file_chunks(&refs, content);
        let covered: std::collections::HashSet<usize> = chunks.iter().map(|(i, _)| *i).collect();
        assert_eq!(covered.len(), 3, "every symbol index produced ≥1 chunk: {chunks:?}");
        assert!(covered.contains(&0) && covered.contains(&1) && covered.contains(&2));
    }

    /// REQ-AXO-902024 fix C — the never-wedge guarantee: with the per-file budget
    /// spent (budget_ms = 0), EVERY symbol still yields ≥1 chunk via the coarse,
    /// tokenizer-free fallback — none is dropped, nothing spins. This is the
    /// deterministic stand-in for a dense file that would otherwise wedge plane A.
    #[test]
    fn build_file_chunks_budget_zero_falls_back_coarse_for_all() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let content = "line one\nline two\nline three\nline four\n";
        let mk = |name: &str, s: usize, e: usize| Symbol {
            name: name.to_string(),
            kind: "soll_ref".to_string(),
            start_line: s,
            end_line: e,
            docstring: None,
            is_entry_point: false,
            is_public: false,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: Default::default(),
            embedding: None,
        };
        // Mix of single-line (phantom-like) and multi-line symbols.
        let syms = [mk("ref1", 1, 1), mk("ref2", 2, 2), mk("block", 1, 4)];
        let refs: Vec<&Symbol> = syms.iter().collect();
        let chunks = build_file_chunks_with_budget(&refs, content, 0);
        let covered: std::collections::HashSet<usize> = chunks.iter().map(|(i, _)| *i).collect();
        assert_eq!(
            covered.len(),
            3,
            "budget=0 must still emit a coarse chunk for EVERY symbol (never drop/wedge): {chunks:?}"
        );
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

        // REQ-AXO-901895 item #3 — DETERMINISTIC O(N²) tripwire. The pre-fix
        // wall-clock bound flaked under load and was a weak super-linearity
        // signal. Count the actual tokenizer encodes instead: the linear DP
        // costs each unique body line once (~N encodes via the memo), whereas
        // the old divide-and-conquer re-encoded growing spans at every recursion
        // node (~N²/2 ≈ 12.5M for N=5000). A `< 4·N` ceiling sits far above the
        // linear cost and far below any super-linear reintroduction.
        // Deterministic only under `--test-threads=1` (required for lib tests).
        crate::embedding_profile::encode_counter::reset();
        let t0 = std::time::Instant::now();
        let chunks = build_symbol_chunks(&symbol, &content);
        let elapsed = t0.elapsed();
        let encodes = crate::embedding_profile::encode_counter::get();

        assert!(
            encodes < n * 4,
            "tokenizer encode count {encodes} for N={n} lines suggests super-linear \
             re-encoding (linear cost is ~N; O(N²) regression would be ~{}M)",
            (n * n) / 2 / 1_000_000
        );

        // Coarse wall-clock net kept as defense-in-depth (the encode count is
        // the authoritative guard). Profile-aware: the unoptimized cargo-test
        // build is ~2-3x slower than the RELEASE "sub-second class" target.
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
        assert!(
            chunks.len() > 1,
            "expected multi-part split, got {}",
            chunks.len()
        );
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

    /// REQ-AXO-901902/901906 — a whole-file `document_body` symbol (TextParser
    /// on a large `.txt`/`.log`/`.conf`) must NEVER trigger a full-content
    /// tokenizer encode at the keep-single check. `axon-diag-chunk-spin` proved
    /// a 644 KB `LOG.txt` span spun `build_symbol_chunks` >90 s exactly there,
    /// blowing past the 15 s chunk budget (armed only AFTER that encode). Past
    /// the gray zone the estimate must be byte-based — and therefore sub-ms.
    #[test]
    fn oversized_single_symbol_estimate_skips_tokenizer_and_is_fast() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile = active_chunk_profile();
        // ~1 MB of varied log-like content — far past gray_zone_char_threshold,
        // so a real tokenizer encode would take seconds; byte estimate is sub-ms.
        let mut content = String::with_capacity(1_100_000);
        let mut i = 0u64;
        while content.len() < 1_000_000 {
            content.push_str(&format!(
                "2026-06-08T12:00:{:02} INFO event id={} val={}\n",
                i % 60,
                i,
                i.wrapping_mul(2654435761)
            ));
            i += 1;
        }
        assert!(
            content.chars().count() > profile.gray_zone_char_threshold,
            "test content must exceed the gray zone to exercise the byte-guard"
        );

        let t0 = std::time::Instant::now();
        let est = single_chunk_token_estimate(profile, &content);
        let elapsed = t0.elapsed();

        assert_eq!(
            est,
            fallback_estimated_token_count(&content),
            "past the gray zone the estimate must be byte-based, not a tokenizer encode"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "oversized single-chunk estimate must skip the tokenizer (took {elapsed:?})"
        );
    }

    /// REQ-AXO-901902/901906 — end-to-end regression for the EXACT `LOG.txt`
    /// pathology proven by `axon-diag-chunk-spin`: a ~640 KB whole-file
    /// `document_body` symbol made of thousands of short lines PLUS one ~8 KB
    /// line. The giant line trips the giant-line fallback, fanning the body into
    /// thousands of segments; pre-fix the emission loop ran a precise tokenizer
    /// encode per segment (~4.5k encodes ⇒ >40 s, NOT covered by the DP
    /// deadline). The fan-out cap byte-estimates past `MAX_PRECISE_ENCODE_CHUNKS`,
    /// so this completes in well under a second.
    #[test]
    fn log_file_shape_with_giant_line_does_not_encode_storm() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut content = String::with_capacity(660_000);
        let mut i = 0u64;
        // ~4500 short log lines (p50≈62 chars, like the real LOG.txt).
        while content.len() < 620_000 {
            content.push_str(&format!(
                "2026-06-08T12:00:{:02}.123 INFO trader: order id={} px={} qty={}\n",
                i % 60,
                i,
                i.wrapping_mul(2654435761) % 100000,
                i % 1000
            ));
            i += 1;
        }
        // One ~8 KB physical line (a serialized blob / stack dump) — trips the
        // giant-line fallback exactly as the real file does.
        let mut giant = String::with_capacity(8200);
        let mut j = 0u64;
        while giant.len() < 8100 {
            giant.push_str(&format!("k{j}=v{};", j.wrapping_mul(40503)));
            j += 1;
        }
        content.push_str(&giant);
        content.push('\n');

        let line_count = content.lines().count();
        let symbol = synthetic_symbol("document_body", line_count.max(1));

        let t0 = std::time::Instant::now();
        let chunks = build_symbol_chunks(&symbol, &content);
        let elapsed = t0.elapsed();

        // Pre-fix: >40 s. Post-fix (byte estimates past the fan-out cap): sub-s.
        // Generous ceiling so cargo-test debug timing never flakes, yet a true
        // per-chunk encode storm (thousands × ms) cannot fit.
        let ceiling = if cfg!(debug_assertions) {
            std::time::Duration::from_secs(8)
        } else {
            std::time::Duration::from_secs(3)
        };
        assert!(
            elapsed < ceiling,
            "640 KB log-shaped document_body chunking took {elapsed:?} (limit {ceiling:?}; pre-fix encode-storm spun >40 s)"
        );
        // Root fix bounds the fan-out: pre-fix this body produced ~28 769
        // micro-chunks (body_budget floored), then `fuse_small_chunks` choked.
        assert!(
            chunks.len() <= MAX_PRECISE_ENCODE_CHUNKS + 1,
            "degenerate body fan-out must be capped, got {} chunks",
            chunks.len()
        );
        assert!(
            chunks.len() > 1,
            "a 640 KB body must still split into multiple chunks"
        );
        // Contiguous, gap-free coverage.
        for w in chunks.windows(2) {
            assert_eq!(
                w[0].end_line + 1,
                w[1].start_line,
                "coarse chunks must be contiguous: {:?} then {:?}",
                (w[0].start_line, w[0].end_line),
                (w[1].start_line, w[1].end_line)
            );
        }
    }

    /// REQ-AXO-901921 — a body UNDER `DEGENERATE_BODY_BYTES` whose `body_budget`
    /// collapses (an ~8 KB first line becomes the capped `repeated_context`,
    /// whose token overhead floors the budget) takes the precise path and,
    /// pre-fix, windowed into thousands of ~tiny chunks (pure embedding noise).
    /// The COUNT must now be bounded by the coarse re-chunk fallback.
    #[test]
    fn sub_degenerate_body_with_collapsed_budget_is_count_bounded() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        // ~8 KB first line → capped repeated_context → overhead floors budget.
        let mut header = String::with_capacity(8400);
        let mut j = 0u64;
        while header.len() < 8200 {
            header.push_str(&format!("hk{j}=hv{}-", j.wrapping_mul(2654435761)));
            j += 1;
        }
        let mut content = String::with_capacity(420_000);
        content.push_str(&header);
        content.push('\n');
        // ~400 KB of short body rows: UNDER the 512 KiB degenerate threshold,
        // so it takes the precise path where the floored budget would fan out.
        let mut i = 0u64;
        while content.len() < 400_000 {
            content.push_str(&format!(
                "body token row {} value {}\n",
                i,
                i.wrapping_mul(40503) % 100000
            ));
            i += 1;
        }
        assert!(
            content.len() < DEGENERATE_BODY_BYTES,
            "must stay on the precise (sub-degenerate) path to exercise the collapse"
        );

        let line_count = content.lines().count();
        let symbol = synthetic_symbol("document_body", line_count.max(1));

        let t0 = std::time::Instant::now();
        let chunks = build_symbol_chunks(&symbol, &content);
        let elapsed = t0.elapsed();

        // Pre-fix this floored-budget body fanned out into thousands of micro
        // chunks; the coarse fallback now bounds the COUNT.
        assert!(
            chunks.len() <= MAX_PRECISE_ENCODE_CHUNKS + 1,
            "REQ-AXO-901921: a budget-collapsed sub-512KB body must be count-bounded, got {} chunks",
            chunks.len()
        );
        assert!(
            chunks.len() > 1,
            "a 400 KB body must still split into multiple chunks"
        );
        let ceiling = if cfg!(debug_assertions) {
            std::time::Duration::from_secs(8)
        } else {
            std::time::Duration::from_secs(3)
        };
        assert!(
            elapsed < ceiling,
            "collapsed-budget chunking took {elapsed:?} (limit {ceiling:?})"
        );
        // Coarse fallback guarantees contiguous, gap-free coverage.
        for w in chunks.windows(2) {
            assert_eq!(
                w[0].end_line + 1,
                w[1].start_line,
                "coarse chunks must be contiguous: {:?} then {:?}",
                (w[0].start_line, w[0].end_line),
                (w[1].start_line, w[1].end_line)
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
        assert!(
            content.lines().count() == 1,
            "must be a single physical line"
        );

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

    /// REQ-AXO-901895 item #1 — every giant-line window holds at most
    /// `body_budget` CHARS, so by the WordPiece invariant (tokens <= chars) it
    /// can never exceed `body_budget` tokens. The pre-fix code sized windows at
    /// `body_budget * char_per_token` chars and emitted shorter lines whole, so
    /// a dense (~1 token/char) line overflowed the model window and the embedder
    /// silently truncated it. Hermetic — proves the bound structurally, no
    /// tokenizer needed.
    #[test]
    fn giant_line_windows_never_exceed_budget_chars() {
        let budget = 50usize;
        // A long line (windowed into many slabs) + a short line the pre-fix code
        // emitted whole (<= the old body_budget*3 window) — both must end up in
        // windows of <= budget chars.
        let long_line = "x".repeat(1000);
        let short_line = "y".repeat(120);
        let lines = [long_line.as_str(), short_line.as_str()];

        let segments = split_giant_lines(&lines, 0, budget);

        for seg in &segments {
            let chars = seg.snippet(&lines).chars().count();
            assert!(
                chars <= budget,
                "window holds {chars} chars > budget {budget}: {seg:?}"
            );
        }
        // No content lost: each line's windows concatenate back to the line.
        let reassembled = |line_1based: usize| -> String {
            segments
                .iter()
                .filter(|s| s.start_line() == line_1based)
                .map(|s| s.snippet(&lines))
                .collect()
        };
        assert_eq!(reassembled(1), long_line, "line 0 windows must cover the line");
        assert_eq!(reassembled(2), short_line, "line 1 windows must cover the line");
    }

    /// REQ-AXO-901895 item #2 — the measured-cost gate. A physical line SHORTER
    /// than the char-proxy giant gate (so the cheap pre-filter misses it) but
    /// whose MEASURED token cost exceeds the body budget must be char-windowed,
    /// NOT left to the DP `!found` branch that emits it whole. Driven directly
    /// against `dp_segment_body` with `body_budget` computed exactly as the code
    /// does, so the line is sized deterministically into the (2×budget, 4×budget)
    /// char band — over the budget, under the cheap gate — regardless of model.
    /// Density is tokenizer-agnostic: space-separated single chars = 1 token each.
    #[test]
    fn measured_cost_gate_windows_dense_dp_line() {
        let _env = env_test_lock().lock().unwrap_or_else(|p| p.into_inner());
        let profile = active_chunk_profile();
        let symbol = synthetic_symbol("dense_dp", 100);
        // Mirror dp_segment_body's budget math (empty repeated_context).
        let overhead = content_token_count(&format_chunk_content(&symbol, "", "", 1, 2));
        let body_budget = profile.target_chunk_tokens.saturating_sub(overhead).max(8);
        // 1.5×budget single-char tokens → cost = 1.5×budget (> budget) and char
        // length = 3×budget-1 (between 2×budget and the 4×budget cheap gate).
        let n_tokens = body_budget + body_budget / 2;
        let dense_line: String = (0..n_tokens)
            .map(|i| ((b'a' + (i as u8 % 26)) as char).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            dense_line.chars().count() < body_budget * 4,
            "dense line must stay under the cheap char gate"
        );
        // Multi-line body (n>1, no line over the cheap gate) so the cheap proxy
        // and the n==1 path are both bypassed — only the measured-cost gate fires.
        let lines_owned = vec![
            "    let x = 1;".to_string(),
            dense_line.clone(),
            "    let y = 2;".to_string(),
        ];
        let lines: Vec<&str> = lines_owned.iter().map(|s| s.as_str()).collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let (segments, bailed) =
            dp_segment_body(profile, &symbol, &lines, "", 0, lines.len(), deadline);

        assert!(!bailed, "must not hit the wall-clock bail");
        // The dense line (index 1) is split into CharWindows — proof the gate
        // routed the body to the windower instead of the DP whole-line emit.
        let windows: Vec<&BodySegment> = segments
            .iter()
            .filter(|s| matches!(s, BodySegment::CharWindow { line, .. } if *line == 1))
            .collect();
        assert!(
            windows.len() >= 2,
            "dense DP line must be char-windowed, got segments {segments:?}"
        );
        // Every window holds at most body_budget chars (=> <= body_budget tokens).
        for w in &windows {
            if let BodySegment::CharWindow {
                char_start,
                char_end,
                ..
            } = w
            {
                assert!(
                    char_end - char_start <= body_budget,
                    "window spans {} chars > budget {body_budget}",
                    char_end - char_start
                );
            }
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
