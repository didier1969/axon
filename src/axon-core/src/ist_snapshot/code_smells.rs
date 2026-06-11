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
const TEST_PATH_FRAGMENTS: &[&str] = &["/tests/", "/test/"];
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
        let (kind_byte, _, _) = graph.node_meta(idx);
        if !is_callable(kind_byte) {
            continue;
        }
        let name = name_from_id(graph.id_of(idx));
        if name.len() < 3 {
            continue;
        }
        let empty = String::new();
        let path = file_map.get(&idx).unwrap_or(&empty);
        if looks_like_minified_or_vendor(name, path) {
            continue;
        }

        // REQ-AXO-901924 — fan-OUT (distinct collaborators this callable
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
        NodeKind::Other => "other",
    }
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
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::new(false, public, false, false),
        }
    }

    fn file(id: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
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
                project_code: "AXO".to_string(),
                kind: NodeKind::Function,
                flags: NodeFlags::default(),
            },
            NodeRecord {
                id: "OPT::src/b.rs::foo".to_string(),
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
}
