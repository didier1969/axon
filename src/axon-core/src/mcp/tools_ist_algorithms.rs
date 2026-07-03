// REQ-AXO-91488 (MIL-AXO-019 slice 4) — MCP tools for advanced IST algos.
//
// Three tools expose petgraph-backed algorithms over the in-memory CSR
// snapshot. All three reuse the process IstSnapshotCache (slice 1) and
// dispatch on it. Cache miss / disabled → structured error with hint to
// run `ist_snapshot_warm` first.

use serde_json::{json, Value};

use crate::ist_snapshot::algorithms::{
    bridges_and_articulation, pagerank_top, shortest_path, structural_sccs,
};
use crate::ist_snapshot::{process_view, IstSnapshotCache};
use crate::mcp::McpServer;
use std::collections::{HashMap, HashSet};

use crate::structural_health::{
    acyclicity_score, martin_distance, main_sequence_score, resilience_score,
    weighted_coverage_score, StructuralHealthIndex, SubScore,
};

/// The MODULE (file) a canonical IST id belongs to. The id embeds the path:
/// `PROJ::path::to::file.rs::Symbol[::method]`. The module is the file — everything up to
/// and INCLUDING the first path component that carries an extension (`.rs`, `.ex`, …), so
/// nested symbols (`file.rs::Type::method`) still map to their file. Ids with no
/// file-like component fall back to stripping the last `::`-component (the symbol name).
fn module_of(id: &str) -> &str {
    let mut offset = 0usize;
    for part in id.split("::") {
        let end = offset + part.len();
        if part.contains('.') {
            return &id[..end];
        }
        offset = end + 2; // skip the "::" separator
    }
    match id.rfind("::") {
        Some(p) => &id[..p],
        None => id,
    }
}

/// REQ-AXO-902185 — is this id a REAL source-code symbol (a definition in a file), as
/// opposed to an external CALL-TARGET node the IST records for a std/library/macro call
/// (e.g. `AXO::unwrap`, `AXO::body.encode`, `AXO::json.loads`)? Real symbols embed a file
/// component: some `::`-segment BEFORE the last one carries a `.` (`…mailbox.rs::message_id`).
/// External call-targets don't (`AXO::unwrap`) — they carry high PageRank (everything calls
/// them) + tested=false, and WITHOUT this filter they pollute weighted_coverage down to a
/// misleading ~0.05 and fill the worklist with untestable targets (discovered s95).
fn is_real_source_symbol(id: &str) -> bool {
    let parts: Vec<&str> = id.split("::").collect();
    parts.len() >= 2 && parts[..parts.len() - 1].iter().any(|p| p.contains('.'))
}

impl McpServer {
    pub(crate) fn axon_ist_centrality_pagerank(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_centrality_pagerank") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let top = args
            .get("top")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(20);
        let damping = args
            .get("damping")
            .and_then(|v| v.as_f64())
            .map(|d| d as f32)
            .unwrap_or(0.85);
        let iterations = args
            .get("iterations")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(50);

        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_centrality_pagerank", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => {
                return Some(ist_cache_miss_error("ist_centrality_pagerank", &project));
            }
        };

        let pairs = pagerank_top(&snapshot, damping, iterations, top);
        let rows: Vec<Value> = pairs
            .iter()
            .enumerate()
            .map(|(rank, (id, score))| {
                json!({
                    "rank": rank + 1,
                    "id": id,
                    "score": score,
                })
            })
            .collect();
        let summary = if pairs.is_empty() {
            format!("ist_centrality_pagerank {} : empty snapshot", project)
        } else {
            format!(
                "ist_centrality_pagerank {} top {} (damping={}, iter={}) — top: {}",
                project, top, damping, iterations, pairs[0].0
            )
        };
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "node_count": snapshot.node_count(),
                "edge_count": snapshot.edge_count(),
                "top_n": top,
                "damping": damping,
                "iterations": iterations,
                "results": rows
            }
        }))
    }

    pub(crate) fn axon_ist_structural_sccs(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_structural_sccs") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_structural_sccs", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => {
                return Some(ist_cache_miss_error("ist_structural_sccs", &project));
            }
        };

        let sccs = structural_sccs(&snapshot);
        let payload: Vec<Value> = sccs
            .iter()
            .map(|c| {
                json!({
                    "size": c.len(),
                    "nodes": c
                })
            })
            .collect();
        let summary = if sccs.is_empty() {
            format!(
                "ist_structural_sccs {} : 0 SCC>1 detected ({} nodes, {} edges)",
                project,
                snapshot.node_count(),
                snapshot.edge_count()
            )
        } else {
            format!(
                "ist_structural_sccs {} : {} SCC>1 (largest size = {})",
                project,
                sccs.len(),
                sccs[0].len()
            )
        };
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": if sccs.is_empty() { "ok" } else { "cycles_detected" },
                "project_code": project,
                "node_count": snapshot.node_count(),
                "edge_count": snapshot.edge_count(),
                "scc_count": sccs.len(),
                "sccs": payload
            }
        }))
    }

    /// REQ-AXO-902184 / CPT-AXO-90055 — Structural Health Index: a RAM-native aggregate
    /// of normalized structural-quality sub-scores over the warm IST snapshot. Slice 2a
    /// wires the two zero-config graph dimensions — acyclicity (Tarjan SCC) + resilience
    /// (articulation points = single points of failure) — into the pure GEOMETRIC
    /// aggregate (`structural_health`), so one rotten axis drags the index down (a
    /// brilliant axis can't mask a broken one). More dimensions (Martin distance,
    /// coverage×centrality, duplication rate, intent alignment) land via REQ-AXO-902185.
    /// Sub-scores are ALWAYS returned individually (anti-Goodhart); the aggregate is a
    /// compass. Supersedes the unweighted `health` aggregate.
    pub(crate) fn axon_structural_health_index(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "structural_health_index") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("structural_health_index", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("structural_health_index", &project)),
        };

        let total_nodes = snapshot.node_count();
        // Acyclicity: Σ sizes of SCCs with size>1 = the nodes trapped in a cycle.
        let sccs = structural_sccs(&snapshot);
        let nodes_in_cycles: usize = sccs.iter().map(|c| c.len()).sum();
        // Resilience: articulation points whose removal would disconnect the graph.
        let (_bridges, articulation) = bridges_and_articulation(&snapshot);

        // Centrality-weighted coverage: Σ(pagerank of COVERED nodes) / Σ(pagerank). Weighting
        // by PageRank asks the load-bearing question — are the HUBS exercised by a test? — not
        // the flat symbol ratio. REQ-AXO-902187: read the RAM-derived `covered` flag (reachable
        // from a #[test] via CALLS), NOT the raw `tested` bit (= "carries #[test]", a leaf with
        // ≈0 PageRank → a structurally false ~0.06). Stays RAM-native. top = node_count → the
        // full ranking (no truncation).
        let ranked = pagerank_top(&snapshot, 0.85, 50, total_nodes.max(1));
        let mut covered_pr = 0.0_f64;
        let mut total_pr = 0.0_f64;
        let mut covered_hub_count = 0usize;
        for (id, score) in &ranked {
            // Exclude external call-target nodes (std/macro/library) — they carry huge
            // PageRank but aren't AXO code to test (would drag coverage to a false ~0.05).
            if !is_real_source_symbol(id) {
                continue;
            }
            let s = *score as f64;
            total_pr += s;
            let covered = snapshot
                .index_of(id)
                .map(|idx| snapshot.node_meta(idx).2.covered())
                .unwrap_or(false);
            if covered {
                covered_pr += s;
                covered_hub_count += 1;
            }
        }

        // Coupling health — Martin's distance from the main sequence per MODULE (file):
        // I = Ce/(Ca+Ce), A = traits/types, D = |A+I−1|. Modules with no coupling are
        // excluded (Martin's metric is defined on the coupling graph). All RAM: module from
        // the id, kind from node_kind, coupling from the CSR edges.
        let mut mod_types: HashMap<String, (usize, usize)> = HashMap::new(); // module → (traits, types)
        let mut efferent: HashMap<String, HashSet<String>> = HashMap::new(); // module → dep-on modules
        let mut afferent: HashMap<String, HashSet<String>> = HashMap::new(); // module → depended-on-by
        for i in 0..total_nodes as u32 {
            let id = snapshot.id_of(i);
            let m = module_of(id).to_string();
            let entry = mod_types.entry(m.clone()).or_insert((0, 0));
            if let Some(kind) = snapshot.node_kind(id) {
                match kind.as_db() {
                    "trait" => {
                        entry.0 += 1;
                        entry.1 += 1;
                    }
                    "struct" | "enum" => entry.1 += 1,
                    _ => {}
                }
            }
            for (t, _rel) in snapshot.forward_neighbors(i) {
                let tm = module_of(snapshot.id_of(t)).to_string();
                if tm != m {
                    efferent.entry(m.clone()).or_default().insert(tm.clone());
                    afferent.entry(tm).or_default().insert(m.clone());
                }
            }
        }
        let mut d_sum = 0.0_f64;
        let mut d_count = 0usize;
        let mut worst_module = String::new();
        let mut worst_d = -1.0_f64;
        for m in mod_types.keys() {
            let ca = afferent.get(m).map(|s| s.len()).unwrap_or(0);
            let ce = efferent.get(m).map(|s| s.len()).unwrap_or(0);
            if ca + ce == 0 {
                continue; // isolated module — not on the coupling graph.
            }
            let (traits, types) = mod_types.get(m).copied().unwrap_or((0, 0));
            let abstractness = if types == 0 { 0.0 } else { traits as f64 / types as f64 };
            let d = martin_distance(ca, ce, abstractness);
            d_sum += d;
            d_count += 1;
            if d > worst_d {
                worst_d = d;
                worst_module = m.clone();
            }
        }
        let mean_distance = if d_count == 0 { 0.0 } else { d_sum / d_count as f64 };

        let index = StructuralHealthIndex::compute(vec![
            SubScore::new(
                "acyclicity",
                acyclicity_score(nodes_in_cycles, total_nodes),
                1.0,
                0.99,
                format!(
                    "{} node(s) in {} cycle(s) / {} total",
                    nodes_in_cycles,
                    sccs.len(),
                    total_nodes
                ),
            ),
            SubScore::new(
                "resilience",
                resilience_score(articulation.len(), total_nodes),
                1.0,
                0.95,
                format!(
                    "{} articulation point(s) (SPOF) / {} total",
                    articulation.len(),
                    total_nodes
                ),
            ),
            SubScore::new(
                "weighted_coverage",
                weighted_coverage_score(covered_pr, total_pr),
                1.0,
                0.80,
                format!(
                    "{} covered node(s) carry {:.1}% of the PageRank mass (are the hubs exercised by a test?)",
                    covered_hub_count,
                    if total_pr > 0.0 { 100.0 * covered_pr / total_pr } else { 100.0 }
                ),
            ),
            SubScore::new(
                "main_sequence",
                main_sequence_score(mean_distance),
                1.0,
                0.75,
                format!(
                    "mean Martin distance D={:.3} over {} coupled module(s); worst: {} (D={:.2})",
                    mean_distance,
                    d_count,
                    if worst_module.is_empty() { "—" } else { worst_module.as_str() },
                    worst_d.max(0.0)
                ),
            ),
        ]);

        let below: Vec<Value> = index
            .below_target()
            .iter()
            .map(|s| json!({"name": s.name, "value": s.value, "target": s.target, "detail": s.detail}))
            .collect();
        let summary = format!(
            "structural_health_index {} : SHI={:.4} ({} dimension(s), {} below target)",
            project,
            index.aggregate,
            index.sub_scores.len(),
            below.len()
        );
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "aggregate": index.aggregate,
                "sub_scores": index.sub_scores.iter().map(|s| json!({
                    "name": s.name,
                    "value": s.value,
                    "weight": s.weight,
                    "target": s.target,
                    "meets_target": s.meets_target(),
                    "detail": s.detail
                })).collect::<Vec<_>>(),
                "below_target": below,
                "node_count": total_nodes,
                "edge_count": snapshot.edge_count(),
                "dimensions_wired": 4,
                "coupled_modules": d_count,
                "note": "acyclicity + resilience + coverage×centrality + main_sequence (Martin-D); remaining (duplication rate, intent alignment) via REQ-AXO-902185"
            }
        }))
    }

    /// REQ-AXO-902186 / CPT-AXO-90055 — Structural Health WORKLIST: turns the below-target
    /// SHI axes into CONCRETE ranked remediation targets. Slice 1 surfaces the two most
    /// actionable: the untested HUBS (top PageRank nodes with tested=false — the load-bearing
    /// code that drags weighted_coverage to 0.05) and the worst-COUPLED modules (top Martin
    /// distance D — the debt behind main_sequence). Ranked by centrality / D so the highest-
    /// leverage fix is first (the ROI seed). Requires `ist_snapshot_warm`. Pair with
    /// `structural_health_index`: after fixing, re-run the index — the ΔSHI is the verdict
    /// (REQ-AXO-902187), not the LLM's judgment.
    pub(crate) fn axon_structural_health_worklist(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "structural_health_worklist") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("structural_health_worklist", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("structural_health_worklist", &project)),
        };
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(15).clamp(1, 200) as usize;
        let total_nodes = snapshot.node_count();

        // Uncovered hubs: full PageRank (sorted desc), keep the covered=false ones = the
        // load-bearing code no test reaches — the highest-ROI remediation for coverage.
        // REQ-AXO-902187: gate on the RAM-derived `covered` flag (reachable from a #[test]
        // via CALLS), NOT the raw `tested` bit — a prod hub NEVER carries #[test], so the old
        // `tested=false` filter surfaced EVERY hub regardless of whether a test exercised it.
        let ranked = pagerank_top(&snapshot, 0.85, 50, total_nodes.max(1));
        let mut untested_hubs: Vec<Value> = Vec::new();
        for (id, score) in &ranked {
            if untested_hubs.len() >= top {
                break;
            }
            // Only REAL AXO code is a testable target — skip external call-targets
            // (std/macro nodes that carry PageRank but can't be tested here).
            if !is_real_source_symbol(id) {
                continue;
            }
            let covered = snapshot
                .index_of(id)
                .map(|idx| snapshot.node_meta(idx).2.covered())
                .unwrap_or(false);
            if !covered {
                let kind = snapshot.node_kind(id).map(|k| k.as_db()).unwrap_or("");
                untested_hubs.push(json!({"id": id, "pagerank": score, "kind": kind}));
            }
        }

        // Worst-coupled modules: Martin distance D per module (same extraction as the index).
        let mut mod_types: HashMap<String, (usize, usize)> = HashMap::new();
        let mut efferent: HashMap<String, HashSet<String>> = HashMap::new();
        let mut afferent: HashMap<String, HashSet<String>> = HashMap::new();
        for i in 0..total_nodes as u32 {
            let id = snapshot.id_of(i);
            let m = module_of(id).to_string();
            let entry = mod_types.entry(m.clone()).or_insert((0, 0));
            if let Some(kind) = snapshot.node_kind(id) {
                match kind.as_db() {
                    "trait" => {
                        entry.0 += 1;
                        entry.1 += 1;
                    }
                    "struct" | "enum" => entry.1 += 1,
                    _ => {}
                }
            }
            for (t, _rel) in snapshot.forward_neighbors(i) {
                let tm = module_of(snapshot.id_of(t)).to_string();
                if tm != m {
                    efferent.entry(m.clone()).or_default().insert(tm.clone());
                    afferent.entry(tm).or_default().insert(m.clone());
                }
            }
        }
        let mut coupled: Vec<(String, f64, usize, usize, f64)> = Vec::new();
        for m in mod_types.keys() {
            let ca = afferent.get(m).map(|s| s.len()).unwrap_or(0);
            let ce = efferent.get(m).map(|s| s.len()).unwrap_or(0);
            if ca + ce == 0 {
                continue;
            }
            let (traits, types) = mod_types.get(m).copied().unwrap_or((0, 0));
            let a = if types == 0 { 0.0 } else { traits as f64 / types as f64 };
            coupled.push((m.clone(), martin_distance(ca, ce, a), ca, ce, a));
        }
        coupled.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        let worst_modules: Vec<Value> = coupled
            .iter()
            .take(top)
            .map(|(m, d, ca, ce, a)| {
                json!({"module": m, "martin_distance": d, "afferent": ca, "efferent": ce, "abstractness": a})
            })
            .collect();

        let summary = format!(
            "structural_health_worklist {} : {} untested hub(s) + {} coupled module(s) ranked — attack the top first, then re-run structural_health_index to verify ΔSHI",
            project,
            untested_hubs.len(),
            worst_modules.len()
        );
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "untested_hubs": untested_hubs,
                "worst_coupled_modules": worst_modules,
                "note": "slice 1 (REQ-AXO-902186): coverage + coupling offenders ranked. After fixing, re-run structural_health_index — ΔSHI is the verdict (REQ-AXO-902187)."
            }
        }))
    }

    pub(crate) fn axon_ist_shortest_path(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "ist_shortest_path") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let from = args
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let to = args
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if from.is_empty() || to.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "ist_shortest_path requires `from` and `to` canonical ids." }],
                "isError": true,
                "data": {
                    "status": "missing_endpoints",
                    "parameter_repair": {
                        "invalid_field": if from.is_empty() { "from" } else { "to" },
                        "tool": "ist_shortest_path",
                        "follow_up_tools": ["query", "inspect"]
                    }
                }
            }));
        }
        let max_radius = args
            .get("max_radius")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(20);

        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("ist_shortest_path", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("ist_shortest_path", &project)),
        };

        let path_opt = shortest_path(&snapshot, &from, &to, max_radius, &[]);
        match path_opt {
            None => Some(json!({
                "content": [{ "type": "text", "text": format!("ist_shortest_path {} : no path from {} to {} within radius {}", project, from, to, max_radius) }],
                "data": {
                    "status": "no_path",
                    "project_code": project,
                    "from": from,
                    "to": to,
                    "max_radius": max_radius,
                    "path": Value::Null
                }
            })),
            Some(path) => {
                let hops = path.len().saturating_sub(1);
                Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!("ist_shortest_path {} : {} → {} via {} hop(s)", project, from, to, hops)
                    }],
                    "data": {
                        "status": "ok",
                        "project_code": project,
                        "from": from,
                        "to": to,
                        "max_radius": max_radius,
                        "hops": hops,
                        "path": path
                    }
                }))
            }
        }
    }

    fn ist_resolve_project(&self, args: &Value, tool: &str) -> Result<String, Value> {
        let raw = args.get("project_code").and_then(|v| v.as_str());
        match raw {
            Some(code) => self
                .resolve_project_code(code)
                .map_err(|_| self.wrong_project_scope_response(code, tool)),
            None => Err(json!({
                "content": [{ "type": "text", "text": format!("{} requires project_code", tool) }],
                "isError": true,
                "data": {
                    "status": "missing_project_code",
                    "parameter_repair": {
                        "invalid_field": "project_code",
                        "tool": tool,
                        "follow_up_tools": ["project_registry_lookup", "help"]
                    }
                }
            })),
        }
    }
}

fn ist_cache_miss_error(tool: &str, project: &str) -> Value {
    // REQ-AXO-901952 — RAM is unconditional (no opt-out) ; `is_enabled()` is a
    // status reporter that is always true. A cache miss means the snapshot is
    // cold, not disabled : the only remedy is to warm it.
    let enabled = IstSnapshotCache::is_enabled();
    let hint = format!(
        "call `ist_snapshot_warm project_code={}` first ; then retry",
        project
    );
    json!({
        "content": [{
            "type": "text",
            "text": format!(
                "{} : IST RAM snapshot not warm for {}. {}",
                tool, project, hint
            )
        }],
        "isError": true,
        "data": {
            "status": "ist_cache_miss",
            "project_code": project,
            "ram_enabled": enabled,
            "parameter_repair": {
                "invalid_field": "ist_cache_snapshot",
                "tool": tool,
                "follow_up_tools": ["ist_snapshot_warm", "status"],
                "hint": hint
            }
        }
    })
}

#[cfg(test)]
mod structural_health_helpers_tests {
    use super::{is_real_source_symbol, module_of};

    #[test]
    fn module_of_extracts_file_from_canonical_id() {
        assert_eq!(
            module_of("AXO::axon::src::axon-core::src::release_reconciler.rs::run_cutover_loop"),
            "AXO::axon::src::axon-core::src::release_reconciler.rs"
        );
        // Nested symbol (impl method) still maps to the file.
        assert_eq!(
            module_of("AXO::a::b::snapshot.rs::IstGraph::node_meta"),
            "AXO::a::b::snapshot.rs"
        );
        // No file-like component → strip the last segment.
        assert_eq!(module_of("AXO::unwrap"), "AXO");
    }

    #[test]
    fn is_real_source_symbol_excludes_external_call_targets() {
        // REQ-AXO-902185 pollution fix: real defs carry a file segment; external
        // call-targets (std/macro/library) do not.
        assert!(is_real_source_symbol("AXO::a::b::mailbox.rs::message_id"));
        assert!(is_real_source_symbol("AXO::x::parser::elixir.rs::new"));
        // External call-targets — no file segment before the last part.
        assert!(!is_real_source_symbol("AXO::unwrap"));
        assert!(!is_real_source_symbol("AXO::Some"));
        assert!(!is_real_source_symbol("AXO::body.encode")); // '.' only in the LAST segment
        assert!(!is_real_source_symbol("AXO::json.loads"));
        assert!(!is_real_source_symbol("bare"));
    }
}
