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
use crate::ist_snapshot::{process_view, IstGraph, IstSnapshotCache, NodeKind};
use crate::mcp::tools_framework_support::{diff_shi_snapshots, load_shi_snapshots, persist_shi_snapshot};
use crate::mcp::McpServer;
use std::collections::{HashMap, HashSet};

use crate::structural_health::{
    acyclicity_score, duplication_score, geometric_aggregate, impact_radius_score, martin_distance,
    main_sequence_score, module_depth_score, resilience_score, weighted_coverage_score,
    StructuralHealthIndex, SubScore,
};

/// REQ-AXO-902185 (impact radius) — bounded reverse-BFS depth: how many hops of "who
/// depends on this" we walk before stopping. 3 matches the existing `impact` tool's
/// convention (blast radius display depth).
const IMPACT_RADIUS_MAX_DEPTH: u32 = 3;
/// REQ-AXO-902185 (impact radius) — per-symbol cap on the reverse-BFS frontier so one
/// extreme hub can't blow up the whole-corpus scan cost; the count saturates at this cap
/// for genuine super-hubs, which is fine for a percentile (they land in the tail either way).
const IMPACT_RADIUS_MAX_NEIGHBORS: usize = 200;

/// REQ-AXO-902185 (impact radius) — nearest-rank percentile over an already-sorted slice.
fn percentile(sorted: &[usize], pct: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

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

/// REQ-AXO-902193 — is this id a TESTABLE symbol = a real source definition
/// (`is_real_source_symbol`) AND a function/method (the only kinds a test exercises via a
/// CALLS edge)? The id-only filter is necessary but NOT sufficient: `…embedder.rs::assert_eq!`
/// embeds a file component so `is_real_source_symbol` accepts it, yet it is a `NodeKind::Other`
/// macro/keyword CALL-TARGET, never a definition. Gating on kind removes the
/// `assert_eq!`/`Ok`/`Some`/`vec!`/`format!` noise from the worklist and stops them from
/// understating weighted_coverage (they were counted as uncovered denominator mass).
fn is_testable_symbol(id: &str, kind: Option<NodeKind>) -> bool {
    is_real_source_symbol(id)
        && matches!(kind, Some(NodeKind::Function) | Some(NodeKind::Method))
        // REQ-AXO-902202 — only Rust carries a coverage model (#[test] → `covered`
        // propagation). A `.py` ops script (`runtime_contracts.py::mode_contract`,
        // `qualify_ingestion_run.py::…`) is a function in a file, so it passes the two
        // gates above, yet it can NEVER be `#[test]`-covered — counting it understates
        // weighted_coverage (the denominator) AND pollutes the worklist with
        // un-actionable targets (the cross-tenant LLL finding: distinguish
        // "not-tested" from "not-MEASURABLE"). Gate on the file being `.rs`.
        && module_of(id).ends_with(".rs")
}

/// REQ-AXO-902185 (dimension 5, intent→code half) — count the ORPHAN-INTENT governed SOLL
/// nodes over their total. A governed node = Requirement/Decision/Concept/Validation; it is
/// orphaned when it carries NO traceability row (no code/test/file/symbol artifact) — intent
/// that claims to be implemented but points at nothing. Mirrors the validated
/// `get_orphan_intent_nodes` / anomalies definition. Pure over the SOLL snapshot so the
/// counting logic unit-tests without a live cache. Returns `(orphan, total)`.
/// REQ-AXO-902201 — concise, disambiguating label for a canonical symbol id in a text
/// summary: `file::name` (the module-file tail + the symbol short name), e.g.
/// `nli.rs::load`. Falls back to the bare name when there is no distinct file segment.
fn short_symbol_label(id: &str) -> String {
    let name = id.rsplit("::").next().unwrap_or(id);
    let file = module_of(id).rsplit("::").next().unwrap_or("");
    if file.is_empty() || file == name {
        name.to_string()
    } else {
        format!("{file}::{name}")
    }
}

fn orphan_intent_over_snapshot(snap: &crate::soll_snapshot::SollSnapshot) -> (usize, usize) {
    let mut orphan = 0usize;
    let mut total = 0usize;
    for ty in ["Requirement", "Decision", "Concept", "Validation"] {
        let lower = ty.to_ascii_lowercase();
        for id in snap.node_ids_of_type(ty) {
            total += 1;
            if snap.traceability_count_for(&lower, id) == 0 {
                orphan += 1;
            }
        }
    }
    (orphan, total)
}

/// REQ-AXO-902186 — the raw structural measurements behind the 5 SHI sub-scores,
/// extracted ONCE so `structural_health_index` (final index) and
/// `structural_health_worklist` (per-candidate "what if I fix just THIS one" deltas)
/// compute against the IDENTICAL baseline. Before this, the worklist re-derived its own
/// copy of the Martin-distance-per-module pass — a DRY fork that could silently diverge
/// from the index's numbers (GUI-PRO-013).
struct ShiRawMetrics {
    total_nodes: usize,
    sccs: Vec<Vec<String>>,
    articulation: Vec<String>,
    covered_pr: f64,
    total_pr: f64,
    /// module → (Martin distance D, afferent count, efferent count).
    mod_d: HashMap<String, (f64, usize, usize)>,
    mean_distance: f64,
    d_count: usize,
    orphan_intent: usize,
    total_intent: usize,
    /// REQ-AXO-902185 — near-duplicate (semantic clone) pairs, RAM-native via
    /// `SIMILAR_TO` edges persisted out-of-band by `reconcile_duplication_edges`
    /// (pgvector HNSW scan, never inline — see that fn's docs for why). Reading
    /// this is a plain CSR relation-type count, zero PG cost per call.
    clone_pairs: usize,
    total_testable_symbols: usize,
    /// REQ-AXO-902185 (module depth) — mean `public/total` symbol ratio across modules
    /// that have at least one real source symbol. See `module_depth_score`.
    mean_public_ratio: f64,
    /// Count of modules contributing to `mean_public_ratio` (NOT the same set as
    /// `d_count` — this includes every module with ≥1 real source symbol, whether or
    /// not it has cross-module coupling).
    mod_pub_total_count: usize,
    /// REQ-AXO-902185 (impact radius) — median + p95 bounded blast radius (reverse BFS,
    /// depth `IMPACT_RADIUS_MAX_DEPTH`, cap `IMPACT_RADIUS_MAX_NEIGHBORS`) over testable
    /// symbols. See `impact_radius_score`.
    median_impact_radius: usize,
    p95_impact_radius: usize,
}

fn compute_shi_raw_metrics(
    snapshot: &IstGraph,
    orphan_intent: usize,
    total_intent: usize,
) -> ShiRawMetrics {
    let total_nodes = snapshot.node_count();
    let sccs = structural_sccs(snapshot);
    let (_bridges, articulation) = bridges_and_articulation(snapshot);

    let ranked = pagerank_top(snapshot, 0.85, 50, total_nodes.max(1));
    let mut covered_pr = 0.0_f64;
    let mut total_pr = 0.0_f64;
    let mut total_testable_symbols = 0usize;
    // REQ-AXO-902185 (impact radius) — bounded reverse-BFS blast radius per testable
    // symbol, collected alongside the coverage pass (same filter, one iteration).
    let mut impact_radii: Vec<usize> = Vec::new();
    for (id, score) in &ranked {
        if !is_testable_symbol(id, snapshot.node_kind(id)) {
            continue;
        }
        total_testable_symbols += 1;
        let s = *score as f64;
        total_pr += s;
        let covered = snapshot
            .index_of(id)
            .map(|idx| snapshot.node_meta(idx).2.covered())
            .unwrap_or(false);
        if covered {
            covered_pr += s;
        }
        let radius = snapshot
            .bfs_reverse(id, IMPACT_RADIUS_MAX_DEPTH, IMPACT_RADIUS_MAX_NEIGHBORS, &[])
            .len();
        impact_radii.push(radius);
    }
    impact_radii.sort_unstable();
    let median_impact_radius = percentile(&impact_radii, 50.0);
    let p95_impact_radius = percentile(&impact_radii, 95.0);

    // REQ-AXO-902185 — near-duplicate pairs, RAM-native via SIMILAR_TO edges. These
    // are NEVER computed here (would reintroduce the PG-per-call cost this whole
    // struct exists to avoid) — `reconcile_duplication_edges` persists them
    // out-of-band via a pgvector HNSW scan, and `ist_snapshot_warm` loads them into
    // the CSR exactly like CALLS/CONTAINS. A plain relation-type count is O(E).
    let clone_pairs = snapshot.count_edges_with_relation(&[crate::ist_snapshot::RelationType::SimilarTo]);

    // REQ-AXO-902186 (dogfood finding, dev-tested against real AXO data) — restrict
    // module-coupling attribution to REAL source symbols. Without this gate, a documentary
    // or external-reference id with no file component (a markdown heading like `AXO::Risque
    // 3. Nettoyer…`, a CSS selector `AXO::.stack-title`, a stdlib call-target
    // `AXO::shutil.which`) falls back to a bogus single-node "module" via `module_of`'s
    // rfind-`::` fallback, which trivially scores Martin-D=1.0 (a single incidental edge) and
    // dominated the worklist's top-ROI slot with un-actionable noise. Same anti-pollution
    // principle already applied to weighted_coverage (REQ-AXO-902193's `is_testable_symbol`);
    // here the kind restriction is dropped (`is_real_source_symbol` only) because
    // traits/structs/enums — excluded by `is_testable_symbol` — are exactly what feed the
    // abstractness (A) side of Martin's distance.
    let mut mod_types: HashMap<String, (usize, usize)> = HashMap::new();
    // REQ-AXO-902185 (module depth) — (public_count, total_count) per module over ALL
    // real source symbols (any kind), the "interface(nb pub)/impl(taille corps)" ratio's
    // raw inputs. Kept separate from `mod_types` (trait/struct/enum only, feeds
    // abstractness) since depth is about the whole module surface, not just its types.
    let mut mod_pub_total: HashMap<String, (usize, usize)> = HashMap::new();
    let mut efferent: HashMap<String, HashSet<String>> = HashMap::new();
    let mut afferent: HashMap<String, HashSet<String>> = HashMap::new();
    for i in 0..total_nodes as u32 {
        let id = snapshot.id_of(i);
        if !is_real_source_symbol(id) {
            continue;
        }
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
        let pub_total = mod_pub_total.entry(m.clone()).or_insert((0, 0));
        pub_total.1 += 1;
        if snapshot.node_meta(i).2.public() {
            pub_total.0 += 1;
        }
        for (t, _rel) in snapshot.forward_neighbors(i) {
            let target_id = snapshot.id_of(t);
            if !is_real_source_symbol(target_id) {
                continue;
            }
            let tm = module_of(target_id).to_string();
            if tm != m {
                efferent.entry(m.clone()).or_default().insert(tm.clone());
                afferent.entry(tm).or_default().insert(m.clone());
            }
        }
    }
    let mut mod_d: HashMap<String, (f64, usize, usize)> = HashMap::new();
    let mut d_sum = 0.0_f64;
    let mut d_count = 0usize;
    for m in mod_types.keys() {
        let ca = afferent.get(m).map(|s| s.len()).unwrap_or(0);
        let ce = efferent.get(m).map(|s| s.len()).unwrap_or(0);
        if ca + ce == 0 {
            continue;
        }
        let (traits, types) = mod_types.get(m).copied().unwrap_or((0, 0));
        let abstractness = if types == 0 { 0.0 } else { traits as f64 / types as f64 };
        let d = martin_distance(ca, ce, abstractness);
        d_sum += d;
        d_count += 1;
        mod_d.insert(m.clone(), (d, ca, ce));
    }
    let mean_distance = if d_count == 0 { 0.0 } else { d_sum / d_count as f64 };

    // REQ-AXO-902185 (module depth) — mean public/total ratio over modules that carry at
    // least one real source symbol (empty modules can't happen here since mod_pub_total
    // is only populated inside the is_real_source_symbol-gated loop above).
    let mod_pub_total_count = mod_pub_total.len();
    let mean_public_ratio = if mod_pub_total.is_empty() {
        0.0
    } else {
        let sum: f64 = mod_pub_total
            .values()
            .map(|(pub_count, total)| if *total == 0 { 0.0 } else { *pub_count as f64 / *total as f64 })
            .sum();
        sum / mod_pub_total_count as f64
    };

    ShiRawMetrics {
        total_nodes,
        sccs,
        articulation,
        covered_pr,
        total_pr,
        mod_d,
        mean_distance,
        d_count,
        orphan_intent,
        total_intent,
        clone_pairs,
        total_testable_symbols,
        mean_public_ratio,
        mod_pub_total_count,
        median_impact_radius,
        p95_impact_radius,
    }
}

fn build_sub_scores(raw: &ShiRawMetrics) -> Vec<SubScore> {
    let nodes_in_cycles: usize = raw.sccs.iter().map(|c| c.len()).sum();
    let orphan_intent_frac = if raw.total_intent == 0 {
        0.0
    } else {
        raw.orphan_intent as f64 / raw.total_intent as f64
    };
    vec![
        SubScore::new(
            "acyclicity",
            acyclicity_score(nodes_in_cycles, raw.total_nodes),
            1.0,
            0.99,
            format!(
                "{} node(s) in {} cycle(s) / {} total",
                nodes_in_cycles,
                raw.sccs.len(),
                raw.total_nodes
            ),
        ),
        SubScore::new(
            "resilience",
            resilience_score(raw.articulation.len(), raw.total_nodes),
            1.0,
            0.95,
            format!(
                "{} articulation point(s) (SPOF) / {} total",
                raw.articulation.len(),
                raw.total_nodes
            ),
        ),
        SubScore::new(
            "weighted_coverage",
            weighted_coverage_score(raw.covered_pr, raw.total_pr),
            1.0,
            0.80,
            format!(
                "{:.1}% of the PageRank mass is covered (are the hubs exercised by a test?)",
                if raw.total_pr > 0.0 { 100.0 * raw.covered_pr / raw.total_pr } else { 100.0 }
            ),
        ),
        SubScore::new(
            "main_sequence",
            main_sequence_score(raw.mean_distance),
            1.0,
            0.75,
            format!(
                "mean Martin distance D={:.3} over {} coupled module(s)",
                raw.mean_distance, raw.d_count
            ),
        ),
        SubScore::new(
            "intent_alignment",
            1.0 - orphan_intent_frac,
            1.0,
            0.85,
            format!(
                "{}/{} governed SOLL node(s) orphaned — no code trace",
                raw.orphan_intent, raw.total_intent
            ),
        ),
        SubScore::new(
            "duplication",
            duplication_score(raw.clone_pairs, raw.total_testable_symbols),
            1.0,
            0.90,
            format!(
                "{} near-duplicate pair(s) / {} testable symbol(s) (SIMILAR_TO edges, pgvector HNSW, threshold<0.10)",
                raw.clone_pairs, raw.total_testable_symbols
            ),
        ),
        SubScore::new(
            "module_depth",
            module_depth_score(raw.mean_public_ratio),
            1.0,
            0.70,
            format!(
                "mean public/total symbol ratio={:.3} across {} module(s) (interface/impl, APoSD)",
                raw.mean_public_ratio,
                raw.mod_pub_total_count
            ),
        ),
        SubScore::new(
            "impact_radius",
            impact_radius_score(raw.p95_impact_radius, raw.total_nodes),
            1.0,
            0.85,
            format!(
                "median impact radius={} / p95={} / {} total node(s) (bounded reverse BFS, depth {})",
                raw.median_impact_radius, raw.p95_impact_radius, raw.total_nodes, IMPACT_RADIUS_MAX_DEPTH
            ),
        ),
    ]
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
            // REQ-AXO-902201 — list the ranked ids (concise file::name) in the TEXT, not
            // just the top-1, so an LLM client can act on the full ranking.
            let ranked_list = pairs
                .iter()
                .map(|(id, _)| short_symbol_label(id))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "ist_centrality_pagerank {} top {} (damping={}, iter={}) — ranked: {}",
                project, top, damping, iterations, ranked_list
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
        // REQ-AXO-902186 — raw metrics extracted via the SHARED helper (also used by
        // `structural_health_worklist` for its per-candidate "what if" deltas), so the
        // index and the worklist can never silently diverge on the same baseline.
        let (orphan_intent, total_intent) = self
            .soll_cache()
            .snapshot(&project)
            .map(|snap| orphan_intent_over_snapshot(&snap))
            .unwrap_or((0, 0));
        let raw = compute_shi_raw_metrics(&snapshot, orphan_intent, total_intent);
        let d_count = raw.d_count;
        let index = StructuralHealthIndex::compute(build_sub_scores(&raw));

        // REQ-AXO-902187 — closed loop: persist this measurement + diff against the
        // PREVIOUS one (loaded BEFORE the append) so the tool's own response carries the
        // verdict — a below-target axis whose delta is <= 0 (no improvement / regression)
        // RE-SURFACES explicitly instead of silently accepting an LLM's unverified "fixed
        // it" claim. Snapshot id = AXON_BUILD_ID (already the authoritative release
        // identity, REQ-AXO-902205/902064) — reused rather than shelling out to git.
        let build_id = std::env::var("AXON_BUILD_ID").unwrap_or_else(|_| "unknown".to_string());
        let sub_scores_map: serde_json::Map<String, Value> = index
            .sub_scores
            .iter()
            .map(|s| (s.name.to_string(), json!(s.value)))
            .collect();
        let shi_snapshot = json!({
            "snapshot_id": build_id,
            "aggregate": index.aggregate,
            "sub_scores": sub_scores_map,
        });
        let previous_snapshots = load_shi_snapshots(&project);
        let delta_vs_previous =
            previous_snapshots.last().map(|prev| diff_shi_snapshots(&shi_snapshot, prev));
        if let Err(err) = persist_shi_snapshot(&project, &shi_snapshot) {
            tracing::warn!(error = %err, project = %project, "REQ-AXO-902187: failed to persist SHI snapshot (non-fatal, index still returned)");
        }
        let per_dimension_delta = delta_vs_previous
            .as_ref()
            .and_then(|d| d.get("per_dimension_delta"))
            .cloned();
        let dimension_delta = |name: &str| -> Option<f64> {
            per_dimension_delta.as_ref()?.get(name)?.as_f64()
        };

        let below: Vec<Value> = index
            .below_target()
            .iter()
            .map(|s| {
                let delta = dimension_delta(s.name);
                json!({
                    "name": s.name,
                    "value": s.value,
                    "target": s.target,
                    "detail": s.detail,
                    "delta_vs_previous": delta,
                    // re_surfaced = still below target AND did not improve since the last
                    // measurement (delta absent on first-ever measurement → not flagged,
                    // there is nothing to have regressed against yet).
                    "re_surfaced": delta.is_some_and(|d| d <= 0.0)
                })
            })
            .collect();
        let re_surfaced_count = below
            .iter()
            .filter(|b| b.get("re_surfaced").and_then(|v| v.as_bool()).unwrap_or(false))
            .count();
        let summary = format!(
            "structural_health_index {} : SHI={:.4} ({} dimension(s), {} below target{})",
            project,
            index.aggregate,
            index.sub_scores.len(),
            below.len(),
            if re_surfaced_count > 0 {
                format!(", {re_surfaced_count} RE-SURFACED (no improvement since last measurement)")
            } else {
                String::new()
            }
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
                "dimensions_wired": 8,
                "coupled_modules": d_count,
                "orphan_intent": orphan_intent,
                "total_intent_nodes": total_intent,
                "snapshot_id": build_id,
                "delta_vs_previous": delta_vs_previous,
                "history_depth": previous_snapshots.len() + 1,
                "note": "acyclicity + resilience + coverage×centrality + main_sequence (Martin-D) + intent_alignment (intent→code half) + duplication (SIMILAR_TO edges, pgvector HNSW scan — see reconcile_duplication_edges, run out-of-band, NOT per-call) + module_depth (interface/impl ratio, APoSD) + impact_radius (bounded reverse-BFS p95); remaining (code→intent orphan half, god-objects) via REQ-AXO-902185. Δ per-call persisted (REQ-AXO-902187) — re_surfaced=true means a below-target axis did not improve since the last measurement."
            }
        }))
    }

    /// REQ-AXO-902192 (volet 1a, CPT-AXO-90056) — WIRING orphans: defined callables that no
    /// PRODUCTION caller reaches via CALLS (only `#[test]`s, or nothing). `test_only` =
    /// delivered + green test but never wired into prod — the recurring OPV cost. Mirror of
    /// `covered` (REQ-AXO-902187): reachable ONLY from a `#[test]`. Requires `ist_snapshot_warm`.
    pub(crate) fn axon_wiring(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "wiring") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("wiring", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("wiring", &project)),
        };
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(20).clamp(1, 200) as usize;
        // REQ-AXO-902192 S2 — SOLL-declared symbols are exempt: a traceability edge means the
        // symbol is wired to INTENT, so a dispatch-dynamic / lazy-import / hook entry the static
        // CALLS graph can't reach is not an orphan (the OPV blind spots). RAM-first via the SOLL
        // snapshot (PIL-AXO-9002); cold snapshot → empty set → no exemption (safe default).
        let declared: std::collections::HashSet<String> = self
            .soll_cache()
            .snapshot(&project)
            .map(|snap| {
                snap.traceability
                    .iter()
                    .filter(|t| t.artifact_type == "Symbol")
                    .map(|t| t.artifact_ref.to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let orphans =
            crate::ist_snapshot::code_smells::wiring_orphans(&snapshot, &project, &declared, top);
        let test_only = orphans.iter().filter(|o| o.category == "test_only").count();
        let isolated = orphans.iter().filter(|o| o.category == "isolated").count();
        let items: Vec<Value> = orphans
            .iter()
            .map(|o| {
                json!({
                    "id": o.id,
                    "name": o.name,
                    "kind": o.kind,
                    "test_callers": o.test_callers,
                    "category": o.category
                })
            })
            .collect();
        let summary = format!(
            "wiring {} : {} orphan(s) — {} test_only (delivered+tested but NO prod caller — the OPV class) + {} isolated (no caller at all, advisory). A test_only symbol tagged deliverable = must be wired before delivery (gate S3, axon_pre_flight_check).",
            project, orphans.len(), test_only, isolated
        );
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "orphans": items,
                "test_only_count": test_only,
                "isolated_count": isolated,
                "soll_declared_symbols": declared.len(),
                "note": "REQ-AXO-902192 volet 1a+S2 — test_only = high-confidence unwired deliverable (0 prod caller, ≥1 test); isolated = advisory (may be an undetected entry). Symbols with a SOLL traceability edge are EXEMPT (declared intent — covers dispatch-dynamic/lazy-import/hook entries the static CALLS graph misses). Gate in axon_pre_flight_check = slice S3."
            }
        }))
    }

    /// REQ-AXO-902186 slice 2 — Structural Health WORKLIST: turns EVERY below-target-capable
    /// SHI axis into concrete remediation candidates, ranked by TRUE ROI = expected ΔSHI ÷
    /// blast-radius (not "worst first" — a catastrophic but cheap-to-fix offender beats a
    /// mild one buried under 200 callers). Four categories, one unified ranking: coverage
    /// (untested hubs), coupling (worst Martin-D modules), resilience (articulation
    /// points/SPOF), acyclicity (cycles/SCCs). `expected_delta_shi` simulates "if ONLY this
    /// one candidate were fixed" by swapping that axis's value in the SAME baseline
    /// (`compute_shi_raw_metrics`/`build_sub_scores`, shared with `structural_health_index` —
    /// no divergent duplicate math) and re-running the pure `geometric_aggregate`.
    /// `blast_radius` is a direct-dependency proxy (callers / module coupling degree / SCC
    /// size) — cheap and RAM-native, not a full multi-hop impact simulation. Requires
    /// `ist_snapshot_warm`. Pair with `structural_health_index`: after fixing, re-run the
    /// index — the ΔSHI it reports is the verdict (REQ-AXO-902187), never the LLM's claim.
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

        let (orphan_intent, total_intent) = self
            .soll_cache()
            .snapshot(&project)
            .map(|snap| orphan_intent_over_snapshot(&snap))
            .unwrap_or((0, 0));
        let raw = compute_shi_raw_metrics(&snapshot, orphan_intent, total_intent);
        let base_scores = build_sub_scores(&raw);
        let base_aggregate = geometric_aggregate(&base_scores);

        // Direct-caller count — the blast-radius proxy: more callers = riskier/costlier to
        // touch. O(V+E), computed once and reused across every candidate category.
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for i in 0..total_nodes as u32 {
            for (t, _rel) in snapshot.forward_neighbors(i) {
                *in_degree.entry(snapshot.id_of(t)).or_insert(0) += 1;
            }
        }

        // "If only THIS one candidate were fixed" — swap one named axis's value into a CLONE
        // of the baseline sub-scores and re-run the pure geometric aggregate. The delta is
        // this candidate's isolated contribution, holding every other axis fixed.
        let delta_for = |name: &str, new_value: f64| -> f64 {
            let mut scores = base_scores.clone();
            if let Some(s) = scores.iter_mut().find(|s| s.name == name) {
                s.value = new_value.clamp(0.0, 1.0);
            }
            geometric_aggregate(&scores) - base_aggregate
        };

        struct Candidate {
            category: &'static str,
            target: Value,
            expected_delta_shi: f64,
            blast_radius: usize,
        }
        let scan_cap = top.saturating_mul(3).max(top);
        let mut candidates: Vec<Candidate> = Vec::new();

        // 1) Coverage — untested hubs (REQ-AXO-902187: gate on `covered`, not raw `tested` —
        // a prod hub never carries #[test]).
        let ranked = pagerank_top(&snapshot, 0.85, 50, total_nodes.max(1));
        let mut hub_scanned = 0usize;
        for (id, score) in &ranked {
            if hub_scanned >= scan_cap {
                break;
            }
            if !is_testable_symbol(id, snapshot.node_kind(id)) {
                continue;
            }
            let covered = snapshot
                .index_of(id)
                .map(|idx| snapshot.node_meta(idx).2.covered())
                .unwrap_or(false);
            if covered {
                continue;
            }
            hub_scanned += 1;
            let s = *score as f64;
            let new_value = weighted_coverage_score(raw.covered_pr + s, raw.total_pr);
            let blast = in_degree.get(id.as_str()).copied().unwrap_or(0).max(1);
            candidates.push(Candidate {
                category: "coverage",
                target: json!({
                    "id": id,
                    "label": short_symbol_label(id),
                    "pagerank": score,
                    "kind": snapshot.node_kind(id).map(|k| k.as_db()).unwrap_or("")
                }),
                expected_delta_shi: delta_for("weighted_coverage", new_value),
                blast_radius: blast,
            });
        }

        // 2) Coupling — worst modules by Martin distance D.
        let mut coupled: Vec<(&String, &(f64, usize, usize))> = raw.mod_d.iter().collect();
        coupled.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal));
        for (m, (d, ca, ce)) in coupled.into_iter().take(scan_cap) {
            // Simulate this ONE module fixed to D=0 (perfectly on the main sequence).
            let new_mean = if raw.d_count == 0 {
                0.0
            } else {
                ((raw.mean_distance * raw.d_count as f64) - d) / raw.d_count as f64
            };
            candidates.push(Candidate {
                category: "coupling",
                target: json!({"module": m, "martin_distance": d, "afferent": ca, "efferent": ce}),
                expected_delta_shi: delta_for("main_sequence", main_sequence_score(new_mean)),
                blast_radius: (ca + ce).max(1),
            });
        }

        // 3) Resilience — articulation points (single points of failure).
        let nodes_in_cycles: usize = raw.sccs.iter().map(|c| c.len()).sum();
        for node_id in raw.articulation.iter().take(scan_cap) {
            let new_value =
                resilience_score(raw.articulation.len().saturating_sub(1), raw.total_nodes);
            let degree = in_degree.get(node_id.as_str()).copied().unwrap_or(0)
                + snapshot
                    .index_of(node_id)
                    .map(|i| snapshot.forward_neighbors(i).count())
                    .unwrap_or(0);
            candidates.push(Candidate {
                category: "resilience",
                target: json!({
                    "id": node_id,
                    "label": short_symbol_label(node_id),
                    "kind": snapshot.node_kind(node_id).map(|k| k.as_db()).unwrap_or("")
                }),
                expected_delta_shi: delta_for("resilience", new_value),
                blast_radius: degree.max(1),
            });
        }

        // 4) Acyclicity — cycles (SCC size > 1), largest first.
        let mut sccs_sorted = raw.sccs.clone();
        sccs_sorted.sort_by(|a, b| b.len().cmp(&a.len()));
        for scc in sccs_sorted.iter().take(scan_cap) {
            let new_value =
                acyclicity_score(nodes_in_cycles.saturating_sub(scc.len()), raw.total_nodes);
            candidates.push(Candidate {
                category: "acyclicity",
                target: json!({"cycle_nodes": scc, "size": scc.len()}),
                expected_delta_shi: delta_for("acyclicity", new_value),
                blast_radius: scc.len().max(1),
            });
        }

        candidates.sort_by(|a, b| {
            let roi_a = a.expected_delta_shi / a.blast_radius as f64;
            let roi_b = b.expected_delta_shi / b.blast_radius as f64;
            roi_b.partial_cmp(&roi_a).unwrap_or(std::cmp::Ordering::Equal)
        });
        let ranked_candidates: Vec<Value> = candidates
            .iter()
            .take(top)
            .map(|c| {
                json!({
                    "category": c.category,
                    "target": c.target,
                    "expected_delta_shi": c.expected_delta_shi,
                    "blast_radius": c.blast_radius,
                    "roi": c.expected_delta_shi / c.blast_radius as f64
                })
            })
            .collect();
        let count_of = |cat: &str| ranked_candidates.iter().filter(|c| c["category"] == cat).count();

        // REQ-AXO-902201 — surface the ranked targets IN THE TEXT so an LLM client can act.
        let target_list = ranked_candidates
            .iter()
            .filter_map(|c| {
                let cat = c["category"].as_str()?;
                let label = c["target"]["label"]
                    .as_str()
                    .or_else(|| c["target"]["module"].as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{}-node cycle", c["target"]["size"]));
                Some(format!("{cat}:{label}"))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let summary = format!(
            "structural_health_worklist {} : {} target(s) ranked by ROI (ΔSHI÷blast-radius) — {} coverage, {} coupling, {} resilience, {} acyclicity. Fix the top first, then re-run structural_health_index — ΔSHI confirms (REQ-AXO-902187).\nRanked: {}",
            project,
            ranked_candidates.len(),
            count_of("coverage"),
            count_of("coupling"),
            count_of("resilience"),
            count_of("acyclicity"),
            if target_list.is_empty() { "—".to_string() } else { target_list }
        );
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "worklist": ranked_candidates,
                "note": "REQ-AXO-902186 slice 2: unified ranking by ROI = expected ΔSHI ÷ blast-radius across coverage/coupling/resilience/acyclicity (not 'worst-first'). blast_radius proxy = direct callers (coverage/resilience) or coupling degree (coupling) or SCC size (acyclicity). Re-run structural_health_index after fixing — ΔSHI is the verdict (REQ-AXO-902187)."
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
    use super::{is_real_source_symbol, is_testable_symbol, module_of, NodeKind};

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

    #[test]
    fn is_testable_symbol_gates_macro_targets_and_non_callables_by_kind() {
        // REQ-AXO-902193: a real definition that IS a function/method passes.
        assert!(is_testable_symbol(
            "AXO::x::embedder.rs::warm",
            Some(NodeKind::Method)
        ));
        assert!(is_testable_symbol(
            "AXO::x::parser.rs::parse",
            Some(NodeKind::Function)
        ));
        // Macro/keyword call-target attributed to a file: is_real_source_symbol accepts it
        // (file segment present) but kind Other/None gates it out — the s96 worklist noise.
        assert!(!is_testable_symbol(
            "AXO::x::embedder.rs::assert_eq!",
            Some(NodeKind::Other)
        ));
        assert!(!is_testable_symbol("AXO::x::graph_ingestion.rs::Ok", None));
        // Trait/struct definitions aren't execution-coverage targets.
        assert!(!is_testable_symbol(
            "AXO::x::pipeline.rs::B2Embedder",
            Some(NodeKind::Trait)
        ));
        // External call-target (no file segment) excluded regardless of kind.
        assert!(!is_testable_symbol("AXO::unwrap", Some(NodeKind::Function)));
        // REQ-AXO-902202 — a function in a NON-Rust file has no #[test] coverage model:
        // excluded from both the worklist and the weighted_coverage denominator.
        assert!(!is_testable_symbol(
            "AXO::x::runtime_contracts.py::mode_contract",
            Some(NodeKind::Function)
        ));
        assert!(!is_testable_symbol(
            "AXO::x::qualify_ingestion_run.py::current_graph_root",
            Some(NodeKind::Method)
        ));
        // Rust file still passes.
        assert!(is_testable_symbol("AXO::x::view.rs::try_snapshot", Some(NodeKind::Method)));
    }

    #[test]
    fn orphan_intent_counts_governed_nodes_without_a_code_trace() {
        use crate::soll_snapshot::{SnapshotNode, SnapshotTraceability, SollSnapshot};
        use std::collections::HashMap;

        let node = |id: &str, ty: &str| SnapshotNode {
            id: id.to_string(),
            entity_type: ty.to_string(),
            title: String::new(),
            status: "current".to_string(),
            metadata_raw: String::new(),
        };
        let trace = |ty: &str, entity: &str| SnapshotTraceability {
            id: format!("t-{entity}"),
            soll_entity_type: ty.to_string(),
            soll_entity_id: entity.to_string(),
            artifact_type: "Symbol".to_string(),
            artifact_ref: "AXO::x::y.rs::f".to_string(),
            artifact_status: "current".to_string(),
        };

        let mut nodes: HashMap<String, SnapshotNode> = HashMap::new();
        for (id, ty) in [
            ("REQ-1", "Requirement"), // traced
            ("REQ-2", "Requirement"), // orphan
            ("DEC-1", "Decision"),    // orphan
            ("CPT-1", "Concept"),     // traced
            ("VAL-1", "Validation"),  // orphan
            ("PIL-1", "Pillar"),      // NOT a governed type — ignored even if orphan
        ] {
            nodes.insert(id.to_string(), node(id, ty));
        }
        // Only REQ-1 and CPT-1 carry a traceability row.
        let traceability = vec![trace("Requirement", "REQ-1"), trace("Concept", "CPT-1")];

        let snap = SollSnapshot::build("AXO", 1, nodes, Vec::new(), traceability);
        // total = 2 Req + 1 Dec + 1 Concept + 1 Validation = 5 (Pillar excluded);
        // orphan = REQ-2 + DEC-1 + VAL-1 = 3.
        assert_eq!(super::orphan_intent_over_snapshot(&snap), (3, 5));
    }
}
