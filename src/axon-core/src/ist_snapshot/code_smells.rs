// REQ-AXO-901595 / REQ-AXO-901596 — RAM-first analytics + lexical search.
//
// PIL-AXO-9002 fast paths for the structural code-smell heuristics that
// previously round-tripped to PG (graph_analytics.rs: get_wrapper_candidates,
// get_feature_envy_candidates, get_god_objects, get_orphan_code_symbols)
// plus the lexical regex search consumed by the `query` tool.
//
// Each function expects an IstGraph filtered to a single project (the loader
// already scopes by project_code, so this is implicit in CSR contents).
// File-path filtering uses the reverse CONTAINS edge to discover the file
// node id and matches the same test-path patterns the SQL queries use.
//
// SOLL Traceability orphan-code filter is NOT applied here (the IST cache
// does not carry SOLL relations) — callers requiring the full canonical
// orphan_code definition (no callers AND no soll.Traceability link) must
// keep the PG path.

use std::collections::HashMap;

use crate::ist_snapshot::snapshot::{IstGraph, NodeFlags, NodeKind, RelationType};

const ANALYTICS_LIMIT: usize = 20;
const GOD_OBJECT_FAN_OUT_THRESHOLD: usize = 20;
const FEATURE_ENVY_MIN_TOTAL: usize = 3;
const FEATURE_ENVY_MIN_FOREIGN: usize = 2;
// REQ-AXO-901958 — `/scripts/` joins the test-path exclusion: script helpers are
// invoked from module-level top-level code (not captured as CALLS edges) or run
// directly, so an uncalled private fn in scripts/ is an entry-point-like utility,
// not dead code. This kills the script-dir slice of the ~85% false-positive set.
const TEST_PATH_FRAGMENTS: &[&str] = &["/tests/", "/test/", "/scripts/"];
const TEST_PATH_SUFFIXES: &[&str] = &["_test.rs", "_test.exs", ".test.ts", ".test.js"];

fn is_callable(kind_byte: u8) -> bool {
    matches!(
        NodeKind::from_u8(kind_byte),
        NodeKind::Function | NodeKind::Method
    )
}

fn is_test_path(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();
    if TEST_PATH_FRAGMENTS
        .iter()
        .any(|frag| lowered.contains(frag))
    {
        return true;
    }
    TEST_PATH_SUFFIXES.iter().any(|suf| lowered.ends_with(suf))
}

fn looks_like_minified_or_vendor(name: &str, path: &str) -> bool {
    let lowered_name = name.to_ascii_lowercase();
    if lowered_name.starts_with("__webpack") || lowered_name.contains("minified") {
        return true;
    }
    let lowered_path = path.to_ascii_lowercase();
    lowered_path.contains("/node_modules/")
        || lowered_path.contains("/dist/")
        || lowered_path.contains("/_build/")
        || lowered_path.contains("/priv/static/")
}

/// Map each symbol idx to its containing file node id (canonical file path
/// in the IST). O(N+M) ; lookups via reverse CONTAINS. Symbols without a
/// containing file (rare ; e.g. test fixtures) get an empty string.
fn build_file_path_map(graph: &IstGraph) -> HashMap<u32, String> {
    let mut map: HashMap<u32, String> = HashMap::with_capacity(graph.node_count());
    for idx in 0..(graph.node_count() as u32) {
        for (src, rel) in graph.reverse_neighbors(idx) {
            if matches!(rel, RelationType::Contains) {
                map.insert(idx, graph.id_of(src).to_string());
                break;
            }
        }
    }
    map
}

fn name_from_id(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}

fn project_matches(graph: &IstGraph, idx: u32, project: &str) -> bool {
    if project == "*" {
        return true;
    }
    let (_, proj, _) = graph.node_meta(idx);
    proj == project
}

/// REQ-AXO-901595 — RAM equivalent of `GraphStore::get_wrapper_candidates`.
/// A wrapper is a non-public function/method with exactly one outgoing CALLS
/// edge ; result format mirrors the PG path : `"source_name -> target_name"`.
pub fn wrapper_candidates(graph: &IstGraph, project: &str, limit: usize) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let mut scored: Vec<(usize, String, String)> = Vec::new();

    for src_idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, src_idx, project) {
            continue;
        }
        let (kind_byte, _, flags) = graph.node_meta(src_idx);
        if !is_callable(kind_byte) {
            continue;
        }
        if NodeFlags(flags.0).public() {
            continue;
        }
        let empty = String::new();
        let path = file_map.get(&src_idx).unwrap_or(&empty);
        if !path.is_empty() && is_test_path(path) {
            continue;
        }

        let mut call_targets: Vec<u32> = Vec::new();
        for (tgt, rel) in graph.forward_neighbors(src_idx) {
            if matches!(rel, RelationType::Calls) {
                call_targets.push(tgt);
                if call_targets.len() > 1 {
                    break;
                }
            }
        }
        if call_targets.len() != 1 {
            continue;
        }
        let tgt_idx = call_targets[0];
        let target_callers = graph
            .reverse_neighbors(tgt_idx)
            .filter(|(_, rel)| matches!(rel, RelationType::Calls))
            .count();

        let source_name = name_from_id(graph.id_of(src_idx)).to_string();
        let target_name = name_from_id(graph.id_of(tgt_idx)).to_string();
        scored.push((target_callers, source_name, target_name));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(limit.max(1).min(ANALYTICS_LIMIT))
        .map(|(_, src, tgt)| format!("{} -> {}", src, tgt))
        .collect()
}

/// REQ-AXO-901595 — RAM equivalent of `GraphStore::get_feature_envy_candidates`.
/// Returns entries formatted as `"source -> dominant_foreign_path (foreign/total)"`
/// matching the PG output shape.
pub fn feature_envy_candidates(graph: &IstGraph, project: &str, limit: usize) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let mut scored: Vec<(usize, usize, String, String)> = Vec::new();

    for src_idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, src_idx, project) {
            continue;
        }
        let (kind_byte, _, _) = graph.node_meta(src_idx);
        if !is_callable(kind_byte) {
            continue;
        }
        let Some(source_path) = file_map.get(&src_idx) else {
            continue;
        };
        if is_test_path(source_path) {
            continue;
        }

        let mut total_calls: usize = 0;
        let mut foreign_calls: usize = 0;
        let mut per_target_path: HashMap<String, usize> = HashMap::new();
        for (tgt, rel) in graph.forward_neighbors(src_idx) {
            if !matches!(rel, RelationType::Calls) {
                continue;
            }
            let Some(target_path) = file_map.get(&tgt) else {
                continue;
            };
            total_calls += 1;
            if target_path != source_path {
                foreign_calls += 1;
                *per_target_path.entry(target_path.clone()).or_insert(0) += 1;
            }
        }
        if total_calls < FEATURE_ENVY_MIN_TOTAL
            || foreign_calls < FEATURE_ENVY_MIN_FOREIGN
            || foreign_calls <= total_calls - foreign_calls
        {
            continue;
        }
        let Some((dominant_path, _)) = per_target_path
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        else {
            continue;
        };

        let source_name = name_from_id(graph.id_of(src_idx)).to_string();
        scored.push((foreign_calls, total_calls, source_name, dominant_path));
    }

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(limit.max(1).min(ANALYTICS_LIMIT))
        .map(|(foreign, total, name, path)| format!("{} -> {} ({}/{})", name, path, foreign, total))
        .collect()
}

/// REQ-AXO-901595 / REQ-AXO-901924 — RAM equivalent of
/// `GraphStore::get_god_objects`. Returns `(symbol_name, fan_out_count)` for
/// callables whose OUTGOING CALLS count meets `GOD_OBJECT_FAN_OUT_THRESHOLD`,
/// excluding minified / vendor paths.
///
/// A god object/function is one that *does too much* — it orchestrates many
/// collaborators (high fan-OUT). The previous heuristic counted fan-IN, which
/// flags widely-*called* tiny utilities (`now_ms`, `build`) — popular hubs, the
/// exact opposite of a god object (REQ-AXO-901924 false positives).
pub fn god_objects(graph: &IstGraph, project: &str) -> Vec<(String, usize)> {
    let file_map = build_file_path_map(graph);
    let mut out: Vec<(String, usize)> = Vec::new();

    for idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, idx, project) {
            continue;
        }
        // REQ-AXO-901970 — match PG `get_god_objects`: no kind filter. A god
        // object is ANY symbol with high CALLS fan-out — classes/structs that
        // orchestrate many collaborators, not just functions/methods (the old
        // is_callable gate dropped `GodClass`-style types). File nodes carry no
        // CALLS edges → never reach the threshold, so no kind guard is needed.
        let name = name_from_id(graph.id_of(idx));
        if name.len() < 3 {
            continue;
        }
        let empty = String::new();
        let path = file_map.get(&idx).unwrap_or(&empty);
        if looks_like_minified_or_vendor(name, path) {
            continue;
        }

        // REQ-AXO-901924 — fan-OUT (distinct collaborators this symbol
        // invokes), not fan-in. High fan-out = does-too-much = god object.
        let mut callees: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (target, rel) in graph.forward_neighbors(idx) {
            if matches!(rel, RelationType::Calls) {
                callees.insert(target);
            }
        }
        let fan_out = callees.len();
        if fan_out >= GOD_OBJECT_FAN_OUT_THRESHOLD {
            out.push((name.to_string(), fan_out));
        }
    }

    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// REQ-AXO-901595 — RAM structural variant of `GraphStore::get_orphan_code_symbols`.
/// Returns non-public callables with zero incoming CALLS edges, excluding
/// test paths. The PG canonical query ALSO excludes symbols carrying a
/// soll.Traceability link ; that filter requires SOLL state outside the
/// IstGraph and is left to the caller (this method is therefore a
/// strict superset of the PG candidate set).
pub fn orphan_code_symbols(graph: &IstGraph, project: &str, limit: usize) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let mut names: Vec<String> = Vec::new();

    for idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, idx, project) {
            continue;
        }
        let (kind_byte, _, flags) = graph.node_meta(idx);
        if !is_callable(kind_byte) {
            continue;
        }
        if NodeFlags(flags.0).public() {
            continue;
        }
        // REQ-AXO-901958 — a `#[test]`/fixture function is a private callable with
        // no inbound CALLS edge (the harness invokes it via attribute, not a call
        // expression), so it would be mis-reported as orphan/dead. The `tested`
        // flag is already persisted; skip these (kills the dominant false-positive
        // class — ~71% of AXO's reported dead set were the test fns themselves).
        if NodeFlags(flags.0).tested() {
            continue;
        }
        let empty = String::new();
        let path = file_map.get(&idx).unwrap_or(&empty);
        if !path.is_empty() && is_test_path(path) {
            continue;
        }
        let has_caller = graph
            .reverse_neighbors(idx)
            .any(|(_, rel)| matches!(rel, RelationType::Calls));
        if has_caller {
            continue;
        }
        names.push(name_from_id(graph.id_of(idx)).to_string());
    }

    names.sort();
    names.dedup();
    names.truncate(limit.max(1).min(ANALYTICS_LIMIT));
    names
}

/// REQ-AXO-901596 — RAM lexical match over symbol names. Implements the
/// same fuzzy matching the PG `symbol_search_predicate` runs : direct
/// substring, separator-normalized substring, wildcard form, and compact
/// form (separators stripped). Returns `(name, kind_str, file_path)` so
/// callers can render the same evidence table the PG path produces.
pub fn lexical_symbol_search(
    graph: &IstGraph,
    project: &str,
    query_text: &str,
    limit: usize,
) -> Vec<(String, &'static str, String)> {
    if query_text.is_empty() {
        return Vec::new();
    }
    let normalized = query_text.to_lowercase();
    let wildcard = normalized.replace([' ', '-', ':', '_'], "%");
    let compact = normalized.replace([' ', '-', '_', ':'], "");
    let file_map = build_file_path_map(graph);

    let matches_predicate = |name: &str| -> bool {
        let lowered = name.to_ascii_lowercase();
        if lowered.contains(&normalized) {
            return true;
        }
        let separator_normalized = lowered.replace(['_', '-', ':'], " ");
        if separator_normalized.contains(&normalized) {
            return true;
        }
        if matches_wildcard(&lowered, &wildcard) {
            return true;
        }
        let compact_name = lowered.replace([' ', '_', '-', ':'], "");
        compact_name.contains(&compact)
    };

    let mut out: Vec<(String, &'static str, String)> = Vec::new();
    for idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, idx, project) {
            continue;
        }
        let (kind_byte, _, _) = graph.node_meta(idx);
        let kind = NodeKind::from_u8(kind_byte);
        if matches!(kind, NodeKind::File | NodeKind::Other) {
            continue;
        }
        let name = name_from_id(graph.id_of(idx));
        if !matches_predicate(name) {
            continue;
        }
        let path = file_map.get(&idx).cloned().unwrap_or_default();
        out.push((name.to_string(), kind_label(kind), path));
        if out.len() >= limit.max(1).min(50) {
            break;
        }
    }
    out
}

fn matches_wildcard(haystack: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    // Implements substring match of a `%`-separated pattern : every segment
    // must appear in order within haystack. Empty leading / trailing
    // segments behave like SQL `%` (no anchor).
    let mut cursor: usize = 0;
    let segments: Vec<&str> = pattern.split('%').collect();
    for seg in &segments {
        if seg.is_empty() {
            continue;
        }
        match haystack[cursor..].find(seg) {
            Some(found) => cursor += found + seg.len(),
            None => return false,
        }
    }
    true
}

fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Function => "function",
        NodeKind::Method => "method",
        NodeKind::Class => "class",
        NodeKind::Struct => "struct",
        NodeKind::Module => "module",
        NodeKind::Trait => "trait",
        NodeKind::Enum => "enum",
        NodeKind::Field => "field",
        NodeKind::Section => "section",
        NodeKind::Element => "element",
        NodeKind::ConfigKey => "config_key",
        NodeKind::Interface => "interface",
        NodeKind::DataArtifact => "data_artifact",
        NodeKind::Other => "other",
    }
}

const UNSAFE_EXPOSURE_MAX_DEPTH: usize = 10;
const NIF_BLOCKING_MAX_DEPTH: usize = 20;
const NIF_BLOCKING_DEPTH_THRESHOLD: usize = 5;

/// REQ-AXO-901970 — RAM equivalent of `GraphStore::get_unsafe_exposure`. From
/// each PUBLIC callable, BFS forward over CALLS (depth ≤ 10, cycle-avoiding); if
/// any reachable node is unsafe (`NodeFlags::unsafe_` OR name == "unwrap"), emit
/// `"initial_name -> ... -> unsafe_name"`. Deduplicated + sorted — mirrors the
/// PG `WITH RECURSIVE` + `SELECT DISTINCT` shape.
pub fn unsafe_exposure(graph: &IstGraph, project: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut out: HashSet<String> = HashSet::new();

    for start in 0..(graph.node_count() as u32) {
        if !project_matches(graph, start, project) {
            continue;
        }
        let (kind_byte, _, flags) = graph.node_meta(start);
        if !is_callable(kind_byte) || !flags.public() {
            continue;
        }
        let start_name = name_from_id(graph.id_of(start)).to_string();
        let mut visited: HashSet<u32> = HashSet::from([start]);
        let mut frontier: Vec<u32> = vec![start];
        for _depth in 1..=UNSAFE_EXPOSURE_MAX_DEPTH {
            let mut next: Vec<u32> = Vec::new();
            for &node in &frontier {
                for (tgt, rel) in graph.forward_neighbors(node) {
                    if !matches!(rel, RelationType::Calls) || !visited.insert(tgt) {
                        continue;
                    }
                    let (_, _, tflags) = graph.node_meta(tgt);
                    let tname = name_from_id(graph.id_of(tgt));
                    if tflags.unsafe_() || tname.eq_ignore_ascii_case("unwrap") {
                        out.insert(format!("{} -> ... -> {}", start_name, tname));
                    }
                    next.push(tgt);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }

    let mut sorted: Vec<String> = out.into_iter().collect();
    sorted.sort();
    sorted
}

/// REQ-AXO-901970 — RAM equivalent of `GraphStore::get_nif_blocking_risks`.
/// From each CALLS_NIF target (a NIF function), BFS forward over CALLS (depth ≤
/// 20, cycle-avoiding) tracking the deepest reachable chain; emit
/// `"nif_name (profondeur: D)"` for NIFs whose max depth exceeds 5 — mirrors the
/// PG `WITH RECURSIVE` + `GROUP BY ... HAVING max(depth) > 5`.
pub fn nif_blocking_risks(graph: &IstGraph, project: &str) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let mut max_depth: HashMap<u32, usize> = HashMap::new();
    let mut nif_names: HashMap<u32, String> = HashMap::new();

    for caller in 0..(graph.node_count() as u32) {
        for (nif, rel) in graph.forward_neighbors(caller) {
            if !matches!(rel, RelationType::CallsNif) || !project_matches(graph, nif, project) {
                continue;
            }
            nif_names
                .entry(nif)
                .or_insert_with(|| name_from_id(graph.id_of(nif)).to_string());
            // depth 1 = the CALLS_NIF edge itself; each forward CALLS hop adds 1.
            let mut visited: HashSet<u32> = HashSet::from([caller, nif]);
            let mut frontier: Vec<u32> = vec![nif];
            let mut depth = 1usize;
            while !frontier.is_empty() && depth < NIF_BLOCKING_MAX_DEPTH {
                let mut next: Vec<u32> = Vec::new();
                for &node in &frontier {
                    for (tgt, r) in graph.forward_neighbors(node) {
                        if matches!(r, RelationType::Calls) && visited.insert(tgt) {
                            next.push(tgt);
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                depth += 1;
                frontier = next;
            }
            let entry = max_depth.entry(nif).or_insert(0);
            if depth > *entry {
                *entry = depth;
            }
        }
    }

    let mut out: Vec<String> = max_depth
        .into_iter()
        .filter(|(_, d)| *d > NIF_BLOCKING_DEPTH_THRESHOLD)
        .map(|(nif, d)| format!("{} (profondeur: {})", nif_names[&nif], d))
        .collect();
    out.sort();
    out
}

/// REQ-AXO-901970 — RAM equivalent of `conception_view`'s cross-file CALLS
/// flow query. Returns `(flows, total_count)` where each flow is
/// `(src_name, src_file, dst_name, dst_file)` for CALLS edges whose source and
/// target live in different files, sorted by `(src_name, dst_name)` and capped
/// at `limit`; `total_count` is the unbounded number of such edges. Mirrors the
/// PG `JOIN ist.Edge ... WHERE COALESCE(src_file,'') != COALESCE(dst_file,'')`.
#[allow(clippy::type_complexity)]
pub fn cross_file_call_flows(
    graph: &IstGraph,
    project: &str,
    limit: usize,
) -> (Vec<(String, String, String, String)>, usize) {
    let file_map = build_file_path_map(graph);
    let mut flows: Vec<(String, String, String, String)> = Vec::new();

    for src_idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, src_idx, project) {
            continue;
        }
        let src_file = file_map.get(&src_idx).cloned().unwrap_or_default();
        for (dst_idx, rel) in graph.forward_neighbors(src_idx) {
            if !matches!(rel, RelationType::Calls) {
                continue;
            }
            let dst_file = file_map.get(&dst_idx).cloned().unwrap_or_default();
            if src_file == dst_file {
                continue;
            }
            flows.push((
                name_from_id(graph.id_of(src_idx)).to_string(),
                src_file.clone(),
                name_from_id(graph.id_of(dst_idx)).to_string(),
                dst_file,
            ));
        }
    }

    let total = flows.len();
    flows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    flows.truncate(limit);
    (flows, total)
}

const DANGEROUS_NAMES: &[&str] = &["eval", "unwrap"];
const LOG_CALL_NAMES: &[&str] = &[
    "println!", "dbg!", "console.log", "io.puts", "print", "printf",
];
const DEBT_NAME_FRAGMENTS: &[&str] = &["todo", "fixme", "secret", "hardcoded credential"];

fn is_dangerous(graph: &IstGraph, idx: u32) -> bool {
    let (_, _, flags) = graph.node_meta(idx);
    if flags.unsafe_() {
        return true;
    }
    // REQ-AXO-901970 — match the canonical name (ist.symbol.name), not the id
    // suffix : a macro/method call target's display name is authoritative.
    let name = graph.name_of(idx).to_ascii_lowercase();
    DANGEROUS_NAMES.contains(&name.as_str())
}

/// REQ-AXO-901970 — RAM equivalent of `GraphStore::get_security_audit` taint
/// walk. `(caller_name, dangerous_name)` pairs where a project symbol reaches a
/// dangerous symbol (unsafe / `eval` / `unwrap`) via 1 or 2 CALLS/CALLS_NIF hops
/// (reverse walk from the small dangerous set, mirroring the PG target-first
/// query). Capped at 100 (the score saturates at 5 findings anyway).
pub fn security_audit_paths(graph: &IstGraph, project: &str) -> Vec<(String, String)> {
    let rels = |r: &RelationType| matches!(r, RelationType::Calls | RelationType::CallsNif);
    let mut pairs: Vec<(String, String)> = Vec::new();
    for d in 0..(graph.node_count() as u32) {
        if !is_dangerous(graph, d) {
            continue;
        }
        let dname = graph.name_of(d).to_string();
        for (src, rel) in graph.reverse_neighbors(d) {
            if !rels(&rel) {
                continue;
            }
            if project_matches(graph, src, project) {
                pairs.push((graph.name_of(src).to_string(), dname.clone()));
                if pairs.len() >= 100 {
                    return pairs;
                }
            }
            // indirect (2-hop): callers of the direct caller.
            for (src2, rel2) in graph.reverse_neighbors(src) {
                if rels(&rel2) && project_matches(graph, src2, project) {
                    pairs.push((graph.name_of(src2).to_string(), dname.clone()));
                    if pairs.len() >= 100 {
                        return pairs;
                    }
                }
            }
        }
    }
    pairs
}

/// REQ-AXO-901970 — RAM equivalent of `GraphStore::get_technical_debt`.
/// `(file_path, symbol_name)` for project symbols whose name carries a debt
/// fragment (todo/fixme/secret/hardcoded credential) OR that CALL `unwrap`/`eval`.
pub fn technical_debt(graph: &IstGraph, project: &str) -> Vec<(String, String)> {
    let file_map = build_file_path_map(graph);
    let mut out: Vec<(String, String)> = Vec::new();
    for idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, idx, project) {
            continue;
        }
        let Some(file) = file_map.get(&idx).filter(|p| !p.is_empty()) else {
            continue;
        };
        // REQ-AXO-901970 — match + return the canonical name (ist.symbol.name) :
        // a TODO/secret symbol's name is the comment/finding text, NOT the id
        // suffix (which is a slug). Same for the CALLS-target dangerous check.
        let name = graph.name_of(idx);
        let name_lower = name.to_ascii_lowercase();
        let name_hit = DEBT_NAME_FRAGMENTS.iter().any(|f| name_lower.contains(f));
        let calls_dangerous = name_hit
            || graph.forward_neighbors(idx).any(|(tgt, rel)| {
                matches!(rel, RelationType::Calls) && {
                    let t = graph.name_of(tgt).to_ascii_lowercase();
                    t == "unwrap" || t == "eval"
                }
            });
        if calls_dangerous {
            out.push((file.clone(), name.to_string()));
        }
    }
    out
}

/// REQ-AXO-901970 — RAM equivalent of `GraphStore::get_telemetry_score`'s count:
/// number of CALLS edges (from a project symbol) targeting a raw-logging
/// function (println!/dbg!/console.log/...).
pub fn telemetry_log_call_count(graph: &IstGraph, project: &str) -> usize {
    let mut count = 0usize;
    for src in 0..(graph.node_count() as u32) {
        if !project_matches(graph, src, project) {
            continue;
        }
        for (tgt, rel) in graph.forward_neighbors(src) {
            if matches!(rel, RelationType::Calls) {
                // REQ-AXO-901970 — canonical name, not the id suffix.
                let t = graph.name_of(tgt).to_ascii_lowercase();
                if LOG_CALL_NAMES.iter().any(|l| l.to_ascii_lowercase() == t) {
                    count += 1;
                }
            }
        }
    }
    count
}

/// REQ-AXO-901970 — RAM detour candidates: `src -> mid -> dst`, all same file,
/// `mid` private with EXACTLY one inbound + one outbound CALLS, `src != dst`.
pub fn detour_candidates(graph: &IstGraph, project: &str, limit: usize) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let mut out: Vec<(String, String, String)> = Vec::new();
    for mid in 0..(graph.node_count() as u32) {
        if !project_matches(graph, mid, project) {
            continue;
        }
        let (kind, _, flags) = graph.node_meta(mid);
        if !is_callable(kind) || flags.public() {
            continue;
        }
        let empty = String::new();
        let mid_file = file_map.get(&mid).unwrap_or(&empty);
        if mid_file.is_empty() || is_test_path(mid_file) {
            continue;
        }
        let inbound: Vec<u32> = graph
            .reverse_neighbors(mid)
            .filter(|(_, r)| matches!(r, RelationType::Calls))
            .map(|(s, _)| s)
            .collect();
        let outbound: Vec<u32> = graph
            .forward_neighbors(mid)
            .filter(|(_, r)| matches!(r, RelationType::Calls))
            .map(|(t, _)| t)
            .collect();
        if inbound.len() != 1 || outbound.len() != 1 {
            continue;
        }
        let (src, dst) = (inbound[0], outbound[0]);
        if src == dst {
            continue;
        }
        if file_map.get(&src) != Some(mid_file) || file_map.get(&dst) != Some(mid_file) {
            continue;
        }
        out.push((
            name_from_id(graph.id_of(src)).to_string(),
            name_from_id(graph.id_of(mid)).to_string(),
            name_from_id(graph.id_of(dst)).to_string(),
        ));
    }
    out.sort();
    out.into_iter()
        .take(limit.max(1).min(ANALYTICS_LIMIT))
        .map(|(s, m, d)| format!("{} -> {} -> {}", s, m, d))
        .collect()
}

/// REQ-AXO-901970 — RAM abstraction-detour: an interface with EXACTLY one
/// same-file impl (class/struct/module) named `<iface>impl|_impl|…adapter…`.
pub fn abstraction_detour_candidates(graph: &IstGraph, project: &str, limit: usize) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let is_impl_kind = |k: u8| {
        matches!(
            NodeKind::from_u8(k),
            NodeKind::Class | NodeKind::Struct | NodeKind::Module
        )
    };
    let name_matches = |impl_lower: &str, iface_lower: &str| -> bool {
        impl_lower == format!("{iface_lower}impl")
            || impl_lower == format!("{iface_lower}_impl")
            || (impl_lower.starts_with(iface_lower) && impl_lower.contains("adapter"))
    };
    let mut by_file: HashMap<String, Vec<u32>> = HashMap::new();
    for idx in 0..(graph.node_count() as u32) {
        if let Some(path) = file_map.get(&idx) {
            if !path.is_empty() && !is_test_path(path) {
                by_file.entry(path.clone()).or_default().push(idx);
            }
        }
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for iface in 0..(graph.node_count() as u32) {
        if !project_matches(graph, iface, project) {
            continue;
        }
        let (kind, _, _) = graph.node_meta(iface);
        if !matches!(NodeKind::from_u8(kind), NodeKind::Interface) {
            continue;
        }
        let Some(iface_file) = file_map.get(&iface).filter(|p| !p.is_empty()) else {
            continue;
        };
        let iface_lower = name_from_id(graph.id_of(iface)).to_ascii_lowercase();
        let siblings = by_file.get(iface_file).map(Vec::as_slice).unwrap_or(&[]);
        let impls: Vec<u32> = siblings
            .iter()
            .copied()
            .filter(|&i| {
                i != iface
                    && is_impl_kind(graph.node_meta(i).0)
                    && name_matches(
                        &name_from_id(graph.id_of(i)).to_ascii_lowercase(),
                        &iface_lower,
                    )
            })
            .collect();
        if impls.len() == 1 {
            out.push((
                name_from_id(graph.id_of(iface)).to_string(),
                name_from_id(graph.id_of(impls[0])).to_string(),
            ));
        }
    }
    out.sort();
    out.into_iter()
        .take(limit.max(1).min(ANALYTICS_LIMIT))
        .map(|(i, m)| format!("{} -> {}", i, m))
        .collect()
}

/// REQ-AXO-901970 — RAM domain leakage: CALLS from a `domain` file into an
/// `infra` file. Output `"src (src_file) -> dst (dst_file)"`.
pub fn domain_leakage(graph: &IstGraph, project: &str, domain: &str, infra: &str) -> Vec<String> {
    let file_map = build_file_path_map(graph);
    let mut out: Vec<String> = Vec::new();
    for src in 0..(graph.node_count() as u32) {
        if !project_matches(graph, src, project) {
            continue;
        }
        let Some(src_file) = file_map.get(&src) else {
            continue;
        };
        if !src_file.contains(domain) {
            continue;
        }
        for (dst, rel) in graph.forward_neighbors(src) {
            if !matches!(rel, RelationType::Calls) {
                continue;
            }
            let Some(dst_file) = file_map.get(&dst) else {
                continue;
            };
            if dst_file.contains(infra) {
                out.push(format!(
                    "{} ({}) -> {} ({})",
                    name_from_id(graph.id_of(src)),
                    src_file,
                    name_from_id(graph.id_of(dst)),
                    dst_file
                ));
            }
        }
    }
    out
}

/// REQ-AXO-901970 — RAM dead-code count: private callables in a non-test file
/// with NO inbound CALLS and NO inbound CALLS_NIF.
pub fn dead_code_count(graph: &IstGraph, project: &str) -> usize {
    let file_map = build_file_path_map(graph);
    let mut count = 0usize;
    for idx in 0..(graph.node_count() as u32) {
        if !project_matches(graph, idx, project) {
            continue;
        }
        let (kind, _, flags) = graph.node_meta(idx);
        // REQ-AXO-901958 — exclude `#[test]`/fixture callables (the `tested` flag
        // is persisted): they are private + have no inbound CALLS edge, so they
        // were the dominant dead-code false positive.
        if !is_callable(kind) || flags.public() || flags.tested() {
            continue;
        }
        let empty = String::new();
        let path = file_map.get(&idx).unwrap_or(&empty);
        if path.is_empty() || is_test_path(path) {
            continue;
        }
        let has_inbound_call = graph
            .reverse_neighbors(idx)
            .any(|(_, r)| matches!(r, RelationType::Calls | RelationType::CallsNif));
        if !has_inbound_call {
            count += 1;
        }
    }
    count
}

/// REQ-AXO-901970 — RAM phantom dead refs: phantom-namespaced symbols READS-d
/// but never DECLARES-d. Sorted, capped at 20.
pub fn phantom_dead_refs(graph: &IstGraph, _project: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut read_targets: HashSet<u32> = HashSet::new();
    let mut declared: HashSet<u32> = HashSet::new();
    for src in 0..(graph.node_count() as u32) {
        for (tgt, rel) in graph.forward_neighbors(src) {
            match rel {
                RelationType::Reads if graph.id_of(tgt).contains("::phantom::") => {
                    read_targets.insert(tgt);
                }
                RelationType::Declares => {
                    declared.insert(tgt);
                }
                _ => {}
            }
        }
    }
    let mut out: Vec<String> = read_targets
        .into_iter()
        .filter(|t| !declared.contains(t))
        .map(|t| graph.id_of(t).to_string())
        .collect();
    out.sort();
    out.truncate(20);
    out
}

/// REQ-AXO-901970 — RAM phantom multi-declare: phantom symbols DECLARES-d from
/// >1 distinct source. Output `"target (N sources)"`, by N desc.
pub fn phantom_multi_declare(graph: &IstGraph, _project: &str) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let mut sources: HashMap<u32, HashSet<u32>> = HashMap::new();
    for src in 0..(graph.node_count() as u32) {
        for (tgt, rel) in graph.forward_neighbors(src) {
            if matches!(rel, RelationType::Declares) && graph.id_of(tgt).contains("::phantom::") {
                sources.entry(tgt).or_default().insert(src);
            }
        }
    }
    let mut scored: Vec<(usize, String)> = sources
        .into_iter()
        .map(|(t, s)| (s.len(), graph.id_of(t).to_string()))
        .filter(|(n, _)| *n > 1)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(20)
        .map(|(n, t)| format!("{} ({} sources)", t, n))
        .collect()
}

/// REQ-AXO-901970 — symbol ids whose CONTAINING FILE name matches the query,
/// either as a `%`-separated wildcard or a plain substring (lowercased). Used by
/// `query`'s chunk-search `path_match` to find content-chunks (NULL file_path)
/// whose file NAME matches — replacing the PG `EXISTS(CONTAINS …)` subquery with
/// a precomputed `c.source_id IN (…)` arm. Empty `normalized` ⇒ no matches
/// (avoids matching every file).
pub fn symbols_in_matching_files(
    graph: &IstGraph,
    project: &str,
    normalized: &str,
    wildcard: &str,
) -> Vec<String> {
    use std::collections::HashSet;
    if normalized.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for file_idx in 0..(graph.node_count() as u32) {
        let file_lower = graph.id_of(file_idx).to_ascii_lowercase();
        if !(file_lower.contains(normalized) || matches_wildcard(&file_lower, wildcard)) {
            continue;
        }
        for (sym, rel) in graph.forward_neighbors(file_idx) {
            if matches!(rel, RelationType::Contains) && project_matches(graph, sym, project) {
                let id = graph.id_of(sym);
                if seen.insert(id.to_string()) {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{
        EdgeTriple, NodeFlags, NodeKind, NodeRecord, RelationType,
    };

    fn func(id: &str, public: bool) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::new(false, public, false, false),
        }
    }

    fn file(id: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::File,
            flags: NodeFlags::default(),
        }
    }

    fn edge(s: &str, t: &str, rel: RelationType) -> EdgeTriple {
        EdgeTriple {
            source: s.to_string(),
            target: t.to_string(),
            rel,
        }
    }

    #[test]
    fn wrapper_candidates_returns_single_call_non_public_callable() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::wrap", false),
            func("AXO::src/lib.rs::real", true),
        ];
        let edges = vec![
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::wrap",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::real",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs::wrap",
                "AXO::src/lib.rs::real",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        let wrappers = wrapper_candidates(&g, "AXO", 5);
        assert_eq!(wrappers, vec!["wrap -> real".to_string()]);
    }

    #[test]
    fn wrapper_candidates_excludes_public_source() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::wrap_pub", true),
            func("AXO::src/lib.rs::real", false),
        ];
        let edges = vec![
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::wrap_pub",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::real",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs::wrap_pub",
                "AXO::src/lib.rs::real",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(wrapper_candidates(&g, "AXO", 5).is_empty());
    }

    #[test]
    fn wrapper_candidates_excludes_test_paths() {
        let nodes = vec![
            file("AXO::src/tests/lib.rs"),
            func("AXO::src/tests/lib.rs::wrap", false),
            func("AXO::src/tests/lib.rs::real", false),
        ];
        let edges = vec![
            edge(
                "AXO::src/tests/lib.rs",
                "AXO::src/tests/lib.rs::wrap",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/tests/lib.rs",
                "AXO::src/tests/lib.rs::real",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/tests/lib.rs::wrap",
                "AXO::src/tests/lib.rs::real",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(wrapper_candidates(&g, "AXO", 5).is_empty());
    }

    #[test]
    fn wrapper_candidates_skips_multi_call_source() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::orchestrator", false),
            func("AXO::src/lib.rs::a", false),
            func("AXO::src/lib.rs::b", false),
        ];
        let edges = vec![
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::orchestrator",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::a",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::b",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs::orchestrator",
                "AXO::src/lib.rs::a",
                RelationType::Calls,
            ),
            edge(
                "AXO::src/lib.rs::orchestrator",
                "AXO::src/lib.rs::b",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(wrapper_candidates(&g, "AXO", 5).is_empty());
    }

    #[test]
    fn god_objects_returns_high_fan_out_callables() {
        // REQ-AXO-901924 — a god function ORCHESTRATES many collaborators
        // (high fan-out). `god` calls THRESHOLD distinct callees.
        let mut nodes = vec![
            file("AXO::src/core.rs"),
            func("AXO::src/core.rs::god", false),
        ];
        let mut edges = vec![edge(
            "AXO::src/core.rs",
            "AXO::src/core.rs::god",
            RelationType::Contains,
        )];
        for i in 0..GOD_OBJECT_FAN_OUT_THRESHOLD {
            let callee = format!("AXO::src/core.rs::callee_{i:02}");
            nodes.push(func(&callee, false));
            edges.push(edge("AXO::src/core.rs", &callee, RelationType::Contains));
            edges.push(edge("AXO::src/core.rs::god", &callee, RelationType::Calls));
        }
        let g = IstGraph::build(nodes, edges);
        let gods = god_objects(&g, "AXO");
        assert_eq!(gods.len(), 1);
        assert_eq!(gods[0].0, "god");
        assert_eq!(gods[0].1, GOD_OBJECT_FAN_OUT_THRESHOLD);
    }

    #[test]
    fn god_objects_does_not_flag_high_fan_in_utility() {
        // REQ-AXO-901924 regression guard — a widely-CALLED tiny helper
        // (high fan-IN, e.g. now_ms / build) is a hub, NOT a god object.
        let mut nodes = vec![
            file("AXO::src/core.rs"),
            func("AXO::src/core.rs::now_ms", false),
        ];
        let mut edges = vec![edge(
            "AXO::src/core.rs",
            "AXO::src/core.rs::now_ms",
            RelationType::Contains,
        )];
        for i in 0..(GOD_OBJECT_FAN_OUT_THRESHOLD + 5) {
            let caller = format!("AXO::src/core.rs::caller_{i:02}");
            nodes.push(func(&caller, false));
            edges.push(edge("AXO::src/core.rs", &caller, RelationType::Contains));
            edges.push(edge(
                &caller,
                "AXO::src/core.rs::now_ms",
                RelationType::Calls,
            ));
        }
        let g = IstGraph::build(nodes, edges);
        assert!(
            god_objects(&g, "AXO").is_empty(),
            "high fan-in utility must not be flagged as a god object"
        );
    }

    #[test]
    fn god_objects_excludes_below_threshold() {
        let nodes = vec![
            file("AXO::src/core.rs"),
            func("AXO::src/core.rs::hub", false),
            func("AXO::src/core.rs::caller", false),
        ];
        let edges = vec![
            edge(
                "AXO::src/core.rs",
                "AXO::src/core.rs::hub",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/core.rs",
                "AXO::src/core.rs::caller",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/core.rs::caller",
                "AXO::src/core.rs::hub",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(god_objects(&g, "AXO").is_empty());
    }

    #[test]
    fn feature_envy_detects_dominant_foreign_path() {
        let nodes = vec![
            file("AXO::src/a.rs"),
            file("AXO::src/b.rs"),
            func("AXO::src/a.rs::source", false),
            func("AXO::src/b.rs::callee_1", false),
            func("AXO::src/b.rs::callee_2", false),
            func("AXO::src/a.rs::local", false),
        ];
        let edges = vec![
            edge(
                "AXO::src/a.rs",
                "AXO::src/a.rs::source",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/a.rs",
                "AXO::src/a.rs::local",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/b.rs",
                "AXO::src/b.rs::callee_1",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/b.rs",
                "AXO::src/b.rs::callee_2",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/a.rs::source",
                "AXO::src/b.rs::callee_1",
                RelationType::Calls,
            ),
            edge(
                "AXO::src/a.rs::source",
                "AXO::src/b.rs::callee_2",
                RelationType::Calls,
            ),
            edge(
                "AXO::src/a.rs::source",
                "AXO::src/a.rs::local",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        let envy = feature_envy_candidates(&g, "AXO", 5);
        assert_eq!(envy.len(), 1);
        assert!(envy[0].starts_with("source -> AXO::src/b.rs"));
        assert!(envy[0].ends_with("(2/3)"));
    }

    #[test]
    fn orphan_code_returns_uncalled_non_public_callables() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::orphan_one", false),
            func("AXO::src/lib.rs::orphan_two", false),
            func("AXO::src/lib.rs::called", false),
            func("AXO::src/lib.rs::public_orphan", true),
        ];
        let edges = vec![
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::orphan_one",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::orphan_two",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::called",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::public_orphan",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs::orphan_one",
                "AXO::src/lib.rs::called",
                RelationType::Calls,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        let orphans = orphan_code_symbols(&g, "AXO", 10);
        // orphan_one calls something but has zero callers itself → orphan
        // orphan_two has zero callers → orphan
        // called has a caller → not orphan
        // public_orphan is public → excluded
        assert_eq!(
            orphans,
            vec!["orphan_one".to_string(), "orphan_two".to_string()]
        );
    }

    #[test]
    fn lexical_search_substring_match() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::reserve_memory_budget", true),
            func("AXO::src/lib.rs::unrelated", true),
        ];
        let edges = vec![
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::reserve_memory_budget",
                RelationType::Contains,
            ),
            edge(
                "AXO::src/lib.rs",
                "AXO::src/lib.rs::unrelated",
                RelationType::Contains,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        let hits = lexical_symbol_search(&g, "AXO", "memory", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "reserve_memory_budget");
        assert_eq!(hits[0].1, "function");
        assert_eq!(hits[0].2, "AXO::src/lib.rs");
    }

    #[test]
    fn lexical_search_wildcard_via_underscore_segments() {
        let nodes = vec![
            file("AXO::src/lib.rs"),
            func("AXO::src/lib.rs::reserve_memory_budget", true),
        ];
        let edges = vec![edge(
            "AXO::src/lib.rs",
            "AXO::src/lib.rs::reserve_memory_budget",
            RelationType::Contains,
        )];
        let g = IstGraph::build(nodes, edges);
        // Caller types "reserve_budget" → wildcard "reserve%budget" must
        // match the underscore-separated symbol via the segmented form.
        let hits = lexical_symbol_search(&g, "AXO", "reserve_budget", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "reserve_memory_budget");
    }

    #[test]
    fn lexical_search_workspace_scope_matches_all_projects() {
        let nodes = vec![
            file("AXO::src/a.rs"),
            file("OPT::src/b.rs"),
            NodeRecord {
                id: "AXO::src/a.rs::foo".to_string(),
                name: "foo".to_string(),
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
            NodeRecord {
                id: "OPT::src/b.rs::foo".to_string(),
                name: "foo".to_string(),
                project_code: "OPT".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
        ];
        let edges = vec![
            edge(
                "AXO::src/a.rs",
                "AXO::src/a.rs::foo",
                RelationType::Contains,
            ),
            edge(
                "OPT::src/b.rs",
                "OPT::src/b.rs::foo",
                RelationType::Contains,
            ),
        ];
        let g = IstGraph::build(nodes, edges);
        let workspace_hits = lexical_symbol_search(&g, "*", "foo", 10);
        assert_eq!(workspace_hits.len(), 2);
        let scoped_hits = lexical_symbol_search(&g, "OPT", "foo", 10);
        assert_eq!(scoped_hits.len(), 1);
        assert_eq!(scoped_hits[0].2, "OPT::src/b.rs");
    }

    #[test]
    fn lexical_search_skips_file_and_other_nodes() {
        let nodes = vec![file("AXO::src/foo.rs")];
        let edges = vec![];
        let g = IstGraph::build(nodes, edges);
        assert!(lexical_symbol_search(&g, "AXO", "foo", 10).is_empty());
    }

    fn func_flags(id: &str, public: bool, unsafe_: bool) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::new(false, public, false, unsafe_),
        }
    }

    // REQ-AXO-901970 — unsafe_exposure: public fn reaching an unsafe target
    // (flag) OR a function named `unwrap`, via a transitive CALLS chain.
    #[test]
    fn unsafe_exposure_traces_public_to_unsafe_and_unwrap() {
        let nodes = vec![
            func_flags("AXO::f.rs::pub_fn", true, false),
            func_flags("AXO::f.rs::mid", false, false),
            func_flags("AXO::f.rs::danger", false, true),
            func_flags("AXO::f.rs::unwrap", false, false),
            // a private root must NOT seed exposure.
            func_flags("AXO::f.rs::priv_root", false, false),
        ];
        let edges = vec![
            edge("AXO::f.rs::pub_fn", "AXO::f.rs::mid", RelationType::Calls),
            edge("AXO::f.rs::mid", "AXO::f.rs::danger", RelationType::Calls),
            edge("AXO::f.rs::pub_fn", "AXO::f.rs::unwrap", RelationType::Calls),
            edge("AXO::f.rs::priv_root", "AXO::f.rs::danger", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let out = unsafe_exposure(&g, "AXO");
        assert!(out.contains(&"pub_fn -> ... -> danger".to_string()), "{out:?}");
        assert!(out.contains(&"pub_fn -> ... -> unwrap".to_string()), "{out:?}");
        // The private root never appears as an initial.
        assert!(!out.iter().any(|s| s.starts_with("priv_root")), "{out:?}");
    }

    // REQ-AXO-901970 — nif_blocking_risks: a NIF whose downstream CALLS chain
    // exceeds depth 5 is flagged with its max depth; a shallow NIF is not.
    #[test]
    fn nif_blocking_risks_flags_deep_chain_only() {
        let mut nodes = vec![
            func_flags("AXO::f.rs::caller", false, false),
            func_flags("AXO::f.rs::deep_nif", false, false),
            func_flags("AXO::f.rs::shallow_nif", false, false),
            func_flags("AXO::f.rs::caller2", false, false),
        ];
        // deep chain: deep_nif -> a -> b -> c -> d -> e -> f  (depth 1..7)
        let chain = ["a", "b", "c", "d", "e", "f"];
        for n in chain {
            nodes.push(func_flags(&format!("AXO::f.rs::{n}"), false, false));
        }
        let mut edges = vec![
            edge("AXO::f.rs::caller", "AXO::f.rs::deep_nif", RelationType::CallsNif),
            edge("AXO::f.rs::caller2", "AXO::f.rs::shallow_nif", RelationType::CallsNif),
        ];
        let mut prev = "AXO::f.rs::deep_nif".to_string();
        for n in chain {
            let cur = format!("AXO::f.rs::{n}");
            edges.push(edge(&prev, &cur, RelationType::Calls));
            prev = cur;
        }
        let g = IstGraph::build(nodes, edges);
        let out = nif_blocking_risks(&g, "AXO");
        assert!(
            out.iter().any(|s| s.starts_with("deep_nif (profondeur: 7)")),
            "deep nif must be flagged at depth 7: {out:?}"
        );
        assert!(
            !out.iter().any(|s| s.starts_with("shallow_nif")),
            "shallow nif (depth 1) must not be flagged: {out:?}"
        );
    }

    // REQ-AXO-901970 — symbols_in_matching_files: symbols whose containing file
    // name matches the query (substring or wildcard); empty query → no matches.
    fn typed_node(id: &str, kind: NodeKind) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind,
            flags: NodeFlags::default(),
        }
    }

    #[test]
    fn detour_candidates_flags_single_passthrough() {
        let nodes = vec![
            file("src/m.rs"),
            func("src/m.rs::src", true),
            func("src/m.rs::mid", false),
            func("src/m.rs::dst", true),
        ];
        let edges = vec![
            edge("src/m.rs", "src/m.rs::src", RelationType::Contains),
            edge("src/m.rs", "src/m.rs::mid", RelationType::Contains),
            edge("src/m.rs", "src/m.rs::dst", RelationType::Contains),
            edge("src/m.rs::src", "src/m.rs::mid", RelationType::Calls),
            edge("src/m.rs::mid", "src/m.rs::dst", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(detour_candidates(&g, "AXO", 10), vec!["src -> mid -> dst".to_string()]);
    }

    #[test]
    fn abstraction_detour_flags_single_impl() {
        let nodes = vec![
            file("src/svc.rs"),
            typed_node("src/svc.rs::Service", NodeKind::Interface),
            typed_node("src/svc.rs::ServiceImpl", NodeKind::Struct),
        ];
        let edges = vec![
            edge("src/svc.rs", "src/svc.rs::Service", RelationType::Contains),
            edge("src/svc.rs", "src/svc.rs::ServiceImpl", RelationType::Contains),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(
            abstraction_detour_candidates(&g, "AXO", 10),
            vec!["Service -> ServiceImpl".to_string()]
        );
    }

    #[test]
    fn domain_leakage_flags_cross_layer_call() {
        let nodes = vec![
            file("src/domain/order.rs"),
            func("src/domain/order.rs::place", true),
            file("src/infrastructure/db.rs"),
            func("src/infrastructure/db.rs::write", true),
        ];
        let edges = vec![
            edge("src/domain/order.rs", "src/domain/order.rs::place", RelationType::Contains),
            edge("src/infrastructure/db.rs", "src/infrastructure/db.rs::write", RelationType::Contains),
            edge("src/domain/order.rs::place", "src/infrastructure/db.rs::write", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let leaks = domain_leakage(&g, "AXO", "domain", "infrastructure");
        assert_eq!(leaks.len(), 1);
        assert!(leaks[0].contains("place") && leaks[0].contains("write"), "{leaks:?}");
    }

    #[test]
    fn dead_code_count_counts_uncalled_private() {
        let nodes = vec![
            file("src/d.rs"),
            func("src/d.rs::dead", false),
            func("src/d.rs::live", false),
            func("src/d.rs::caller", true),
        ];
        let edges = vec![
            edge("src/d.rs", "src/d.rs::dead", RelationType::Contains),
            edge("src/d.rs", "src/d.rs::live", RelationType::Contains),
            edge("src/d.rs", "src/d.rs::caller", RelationType::Contains),
            edge("src/d.rs::caller", "src/d.rs::live", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(dead_code_count(&g, "AXO"), 1);
    }

    #[test]
    fn phantom_analytics_use_reads_and_declares() {
        let nodes = vec![func("src/p.rs::reader", false), func("src/p.rs::decl_a", false)];
        let edges = vec![
            edge("src/p.rs::reader", "ENV::phantom::MISSING", RelationType::Reads),
            edge("src/p.rs::reader", "ENV::phantom::DUP", RelationType::Declares),
            edge("src/p.rs::decl_a", "ENV::phantom::DUP", RelationType::Declares),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(phantom_dead_refs(&g, "AXO"), vec!["ENV::phantom::MISSING".to_string()]);
        let multi = phantom_multi_declare(&g, "AXO");
        assert_eq!(multi.len(), 1);
        assert!(multi[0].starts_with("ENV::phantom::DUP (2 sources)"), "{multi:?}");
    }

    #[test]
    fn symbols_in_matching_files_matches_by_containing_file_name() {
        let nodes = vec![
            file("src/overlay.rs"),
            func("src/overlay.rs::render", true),
            file("src/other.rs"),
            func("src/other.rs::helper", true),
        ];
        let edges = vec![
            edge("src/overlay.rs", "src/overlay.rs::render", RelationType::Contains),
            edge("src/other.rs", "src/other.rs::helper", RelationType::Contains),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(
            symbols_in_matching_files(&g, "AXO", "overlay", "overlay"),
            vec!["src/overlay.rs::render".to_string()]
        );
        // wildcard form (separator → %) still matches the file name.
        assert_eq!(
            symbols_in_matching_files(&g, "AXO", "over lay", "over%lay"),
            vec!["src/overlay.rs::render".to_string()]
        );
        assert!(symbols_in_matching_files(&g, "AXO", "nomatch", "nomatch").is_empty());
        assert!(symbols_in_matching_files(&g, "AXO", "", "").is_empty());
    }

    // REQ-AXO-901970 — cross_file_call_flows: only CALLS edges crossing a file
    // boundary count; same-file calls are excluded; total is unbounded, list is
    // sorted+capped.
    #[test]
    fn cross_file_call_flows_excludes_same_file_calls() {
        let nodes = vec![
            file("AXO::a.rs"),
            file("AXO::b.rs"),
            func("AXO::a.rs::af", true),
            func("AXO::a.rs::a2", true),
            func("AXO::b.rs::bf", true),
        ];
        let edges = vec![
            edge("AXO::a.rs", "AXO::a.rs::af", RelationType::Contains),
            edge("AXO::a.rs", "AXO::a.rs::a2", RelationType::Contains),
            edge("AXO::b.rs", "AXO::b.rs::bf", RelationType::Contains),
            // cross-file: af -> bf
            edge("AXO::a.rs::af", "AXO::b.rs::bf", RelationType::Calls),
            // same-file: af -> a2 (excluded)
            edge("AXO::a.rs::af", "AXO::a.rs::a2", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let (flows, count) = cross_file_call_flows(&g, "AXO", 5);
        assert_eq!(count, 1, "only the cross-file call counts: {flows:?}");
        assert_eq!(flows.len(), 1);
        assert_eq!(
            flows[0],
            (
                "af".to_string(),
                "AXO::a.rs".to_string(),
                "bf".to_string(),
                "AXO::b.rs".to_string()
            )
        );
    }

    // REQ-AXO-901970 — NodeRecord with a canonical `name` DECOUPLED from the id
    // suffix : the regression these three tests lock in. The indexer slugs the id
    // (`file::todo_7`) but keeps the real text in `ist.symbol.name`. Name-based
    // analytics must read `name_of`, never `name_from_id(id_of(..))`.
    fn named(id: &str, name: &str, kind: NodeKind) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: name.to_string(),
            project_code: "AXO".to_string(),
            kind,
            flags: NodeFlags::new(false, true, false, false),
        }
    }

    #[test]
    fn technical_debt_matches_canonical_name_not_id_suffix() {
        // id suffix is an opaque slug ("todo_7") ; the TODO text lives in `name`.
        let nodes = vec![
            file("src/parser.rs"),
            named("AXO::src/parser.rs::todo_7", "// TODO: fix the parser", NodeKind::Other),
            named("AXO::src/cfg.rs::sec_1", "SECRET_API_KEY hardcoded credential", NodeKind::Other),
            file("src/cfg.rs"),
        ];
        let edges = vec![
            edge("src/parser.rs", "AXO::src/parser.rs::todo_7", RelationType::Contains),
            edge("src/cfg.rs", "AXO::src/cfg.rs::sec_1", RelationType::Contains),
        ];
        let g = IstGraph::build(nodes, edges);
        let debt = technical_debt(&g, "AXO");
        // The returned name is the canonical comment/secret text, NOT the slug.
        assert!(
            debt.iter().any(|(f, n)| f == "src/parser.rs" && n == "// TODO: fix the parser"),
            "TODO text must be matched + returned via name_of: {debt:?}"
        );
        assert!(
            debt.iter().any(|(f, n)| f == "src/cfg.rs" && n.contains("hardcoded credential")),
            "secret finding must be matched via name_of: {debt:?}"
        );
    }

    #[test]
    fn security_audit_matches_dangerous_by_canonical_name() {
        // The dangerous callee's id suffix is a slug ; its NAME is "eval".
        let nodes = vec![
            named("AXO::f.rs::caller", "caller", NodeKind::Function),
            named("AXO::f.rs::n42", "eval", NodeKind::Function),
        ];
        let edges = vec![edge("AXO::f.rs::caller", "AXO::f.rs::n42", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let pairs = security_audit_paths(&g, "AXO");
        assert!(
            pairs.iter().any(|(c, d)| c == "caller" && d == "eval"),
            "dangerous callee must be recognised by name_of, not id suffix: {pairs:?}"
        );
    }

    #[test]
    fn telemetry_counts_log_calls_by_canonical_name() {
        // The log target's id suffix is a slug ; its NAME is "println!".
        let nodes = vec![
            named("AXO::f.rs::worker", "worker", NodeKind::Function),
            named("AXO::std::m99", "println!", NodeKind::Function),
            named("AXO::f.rs::quiet", "compute", NodeKind::Function),
        ];
        let edges = vec![
            edge("AXO::f.rs::worker", "AXO::std::m99", RelationType::Calls),
            edge("AXO::f.rs::quiet", "AXO::std::m99", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(
            telemetry_log_call_count(&g, "AXO"),
            2,
            "both CALLS to the println!-named target count via name_of"
        );
    }
}
