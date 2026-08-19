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
    acyclicity_score, duplication_score, geometric_aggregate, god_objects_score, impact_radius_score,
    martin_distance, main_sequence_score, module_depth_score, resilience_score,
    weighted_coverage_score, StructuralHealthIndex, SubScore,
};

/// REQ-AXO-902279 (feedback #46, NEX — blocking) — sample up to `cap` identifiers into a
/// tool's TEXT summary (`content[0].text`), the channel an LLM actually reads. The names
/// are ALREADY in `data`; this only echoes a bounded sample so a bare count like "77 dead
/// clusters" stops being unactionable. Truncation is ALWAYS disclosed ("showing N of M") —
/// a summary that silently drops names is the class of defect rejected for the S4 wiring
/// image (a caveat carried in prose, never hidden). Returns "" for an empty slice so the
/// caller can append it unconditionally.
fn sample_identities(label: &str, names: &[String], cap: usize) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown = names.len().min(cap);
    let list = names[..shown].join(", ");
    if names.len() > shown {
        format!(" · {} (showing {} of {}): {}, …", label, shown, names.len(), list)
    } else {
        format!(" · {}: {}", label, list)
    }
}

/// REQ-AXO-902185 (impact radius) — bounded reverse-BFS depth: how many hops of "who
/// depends on this" we walk before stopping. 3 matches the existing `impact` tool's
/// convention (blast radius display depth).
const IMPACT_RADIUS_MAX_DEPTH: u32 = 3;
/// REQ-AXO-902185 (impact radius) — per-symbol cap on the reverse-BFS frontier so one
/// extreme hub can't blow up the whole-corpus scan cost; the count saturates at this cap
/// for genuine super-hubs, which is fine for a percentile (they land in the tail either way).
const IMPACT_RADIUS_MAX_NEIGHBORS: usize = 200;

/// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity threshold above
/// which a function is "complex" (industry-standard >10, not a measurement-swept
/// value — a health index needs a fixed line so a clean codebase can hit 1.0).
const GOD_OBJECT_COMPLEXITY_THRESHOLD: i32 = 10;
/// REQ-AXO-902185 (god-objects) — fan-out (distinct outbound CALLS/CALLS_NIF)
/// threshold. ANDed with complexity (not OR'd) so neither axis alone flags a
/// false positive — the lesson from the earlier LOC-threshold attempt (20
/// lines alone gave 81 false positives on a single axis).
const GOD_OBJECT_FANOUT_THRESHOLD: usize = 10;

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
        // REQ-AXO-902202 / REQ-AXO-902214 — only a language with a NATIVE coverage model
        // (#[test] → `covered` propagation) belongs in the weighted_coverage denominator. A
        // `.py` ops script (`runtime_contracts.py::mode_contract`) is a function in a file, so
        // it passes the two gates above, yet it can NEVER be `#[test]`-covered — counting it
        // understates the axis AND pollutes the worklist with un-actionable targets (the
        // cross-tenant LLL finding: distinguish "not-tested" from "not-MEASURABLE"). The
        // capability lives in ONE parser-registry-adjacent home, not an inline `== ".rs"`:
        && crate::parser::language_has_coverage_model(module_of(id))
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
    /// REQ-AXO-902185 (god-objects) — count of Function/Method symbols classified
    /// as god-objects (complexity AND fan-out both over threshold).
    god_objects: usize,
    /// REQ-AXO-902185 (god-objects) — total real Function/Method symbols (the
    /// denominator), regardless of whether complexity has been measured yet.
    total_real_functions: usize,
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
    // principle already applied to weighted_coverage (REQ-AXO-902193's `is_testable_symbol`).
    // REQ-AXO-902230 extends 902186: `is_real_source_symbol` still accepts doc/config files
    // WITH a real path (`…inventory.md::Purpose`, `docker-compose.yml`), so both endpoints are
    // ALSO gated by `NodeKind::can_form_code_module` (below) — which deliberately KEEPS
    // trait/struct/enum (they feed the abstractness A side) and drops only the four non-code
    // kinds; the edge loop is further restricted to `RelationType::is_dependency` (drops
    // SIMILAR_TO clone-pairs + intra-module CONTAINS, the relations that carried .md pollution).
    let mut mod_types: HashMap<String, (usize, usize)> = HashMap::new();
    // REQ-AXO-902185 (module depth) — (public_count, total_count) per module over ALL
    // real source symbols (any kind), the "interface(nb pub)/impl(taille corps)" ratio's
    // raw inputs. Kept separate from `mod_types` (trait/struct/enum only, feeds
    // abstractness) since depth is about the whole module surface, not just its types.
    let mut mod_pub_total: HashMap<String, (usize, usize)> = HashMap::new();
    let mut efferent: HashMap<String, HashSet<String>> = HashMap::new();
    let mut afferent: HashMap<String, HashSet<String>> = HashMap::new();
    // REQ-AXO-902185 (god-objects) — real Function/Method symbols, the population
    // this axis classifies. `total_real_functions` counts ALL of them regardless
    // of whether their language's complexity-counting has landed yet (so the
    // score stays honest: a not-yet-instrumented language never inflates it,
    // it just can't contribute to the numerator — same "None ≠ 0" discipline as
    // `NodeRecord::complexity`). `god_objects` counts complexity AND fan-out both
    // over threshold (AND, not OR — a single-axis threshold gave 81 false
    // positives on a prior LOC-only attempt).
    let mut total_real_functions = 0usize;
    let mut god_objects = 0usize;
    for i in 0..total_nodes as u32 {
        let id = snapshot.id_of(i);
        if !is_real_source_symbol(id) {
            continue;
        }
        // REQ-AXO-902230 — a Martin module is a CODE module. `is_real_source_symbol`
        // only asserts the id carries a file segment, so a doc/config/data file WITH a
        // real path (`…inventory.md::Purpose` = Section, `docker-compose.yml::x` =
        // ConfigKey, `schema.sql::t`) still passes it and 902186's earlier gate — yet
        // carries no code semantics. Exclude the unambiguous non-code kinds from BOTH the
        // coupling and module-depth attribution (`Other` stays — see can_form_code_module).
        let node_kind = snapshot.node_kind(id);
        if !node_kind.map(|k| k.can_form_code_module()).unwrap_or(false) {
            continue;
        }
        let m = module_of(id).to_string();
        let entry = mod_types.entry(m.clone()).or_insert((0, 0));
        if let Some(kind) = node_kind {
            match kind.as_db() {
                "trait" => {
                    entry.0 += 1;
                    entry.1 += 1;
                }
                "struct" | "enum" => entry.1 += 1,
                _ => {}
            }
            if matches!(kind, NodeKind::Function | NodeKind::Method) {
                total_real_functions += 1;
                if let Some(complexity) = snapshot.complexity_of(i) {
                    let fan_out = snapshot
                        .forward_neighbors(i)
                        .filter(|(_, rel)| {
                            matches!(rel, crate::ist_snapshot::RelationType::Calls | crate::ist_snapshot::RelationType::CallsNif)
                        })
                        .count();
                    if complexity > GOD_OBJECT_COMPLEXITY_THRESHOLD
                        && fan_out > GOD_OBJECT_FANOUT_THRESHOLD
                    {
                        god_objects += 1;
                    }
                }
            }
        }
        let pub_total = mod_pub_total.entry(m.clone()).or_insert((0, 0));
        pub_total.1 += 1;
        if snapshot.node_meta(i).2.public() {
            pub_total.0 += 1;
        }
        for (t, rel) in snapshot.forward_neighbors(i) {
            // REQ-AXO-902230 — Martin Ca/Ce is DEPENDENCY coupling. Skip non-dependency
            // relations: `SIMILAR_TO` (out-of-band pgvector clone pairs, 2886 on AXO —
            // the .md-domination dogfood flowed entirely through these) and `CONTAINS`
            // (intra-module structural, never a cross-module dependency).
            if !rel.is_dependency() {
                continue;
            }
            let target_id = snapshot.id_of(t);
            if !is_real_source_symbol(target_id) {
                continue;
            }
            if !snapshot
                .node_kind(target_id)
                .map(|k| k.can_form_code_module())
                .unwrap_or(false)
            {
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
        god_objects,
        total_real_functions,
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
        // REQ-AXO-902214 — NEUTRALIZE (not_applicable), don't score, when the scope has NO
        // coverage-capable symbol at all (`total_testable_symbols == 0` ⟺ no language here
        // carries a native #[test]→covered model — see `is_testable_symbol` /
        // `parser::language_has_coverage_model`). Gating on the CAPABILITY count, never on
        // `covered_pr == 0`: a real Rust project at 0% coverage still has testable symbols, so
        // it stays a measured 0.0 (below target, on the worklist) — the session-100 revert
        // landmine. A `.lll`/Python-only corpus has zero testable symbols → the axis is excluded
        // from the geometric aggregate (weight 0) rather than mislabeled 1.0 "100% covered"
        // (which inflated SHI). REQ-AXO-902202 removed the ~0 penalty; this removes the 1.0 lie.
        if raw.total_testable_symbols == 0 {
            SubScore::not_applicable(
                "weighted_coverage",
                0.80,
                "not_applicable: no coverage-capable language in scope (no native #[test]→covered model — e.g. Python/Elixir/.lll) — axis neutralized (weight 0), NOT counted as 0 or 100%",
            )
        } else {
            SubScore::new(
                "weighted_coverage",
                weighted_coverage_score(raw.covered_pr, raw.total_pr),
                1.0,
                0.80,
                format!(
                    "{:.1}% of the PageRank mass is covered (are the hubs exercised by a test?)",
                    if raw.total_pr > 0.0 { 100.0 * raw.covered_pr / raw.total_pr } else { 100.0 }
                ),
            )
        },
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
        SubScore::new(
            "god_objects",
            god_objects_score(raw.god_objects, raw.total_real_functions),
            1.0,
            0.95,
            format!(
                "{} god-object(s) (complexity>{} AND fan-out>{}) / {} real function(s)/method(s) — languages without complexity-counting yet read as unmeasured, not clean",
                raw.god_objects, GOD_OBJECT_COMPLEXITY_THRESHOLD, GOD_OBJECT_FANOUT_THRESHOLD, raw.total_real_functions
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
        // REQ-AXO-902279 (feedback #46) — name the members of the largest SCC in the TEXT
        // channel: "62 SCC>1, largest = 35" gives no cycle to break until an LLM knows WHICH
        // symbols form it. Names are already in `data.sccs[].nodes` (size-desc); echo a
        // bounded sample of the biggest with truncation disclosure.
        let scc_phrase = match sccs.first() {
            Some(largest) => sample_identities(
                &format!("largest SCC ({} symbols)", largest.len()),
                largest,
                12,
            ),
            None => String::new(),
        };
        let summary = if sccs.is_empty() {
            format!(
                "ist_structural_sccs {} : 0 SCC>1 detected ({} nodes, {} edges)",
                project,
                snapshot.node_count(),
                snapshot.edge_count()
            )
        } else {
            format!(
                "ist_structural_sccs {} : {} SCC>1 (largest size = {}){}",
                project,
                sccs.len(),
                sccs[0].len(),
                scc_phrase
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
            // REQ-AXO-902214 — never persist a not_applicable axis's display-only 1.0. If the
            // scope later BECOMES coverage-capable (a Python repo adds Rust), a persisted 1.0
            // would make the first real measurement (e.g. 0.2) read as a regression → a false
            // "re_surfaced" flag. Absent instead: the diff then treats that first measurement as
            // its own positive value (diff_shi_snapshots_first_appearance_of_a_dimension_reads_as_its_own_value).
            .filter(|s| !s.not_applicable)
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
        let not_applicable_count = index.sub_scores.iter().filter(|s| s.not_applicable).count();
        let summary = format!(
            "structural_health_index {} : SHI={:.4} ({} dimension(s), {} below target{}{})",
            project,
            index.aggregate,
            index.sub_scores.len(),
            below.len(),
            if not_applicable_count > 0 {
                // REQ-AXO-902214 — flag neutralized axes so a non-Rust corpus reads honestly.
                format!(", {not_applicable_count} not_applicable (neutralized)")
            } else {
                String::new()
            },
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
                    // REQ-AXO-902214 — true = axis neutralized (no measurable signal for this
                    // scope, weight 0, excluded from the aggregate); NOT a 0-or-100% score.
                    "not_applicable": s.not_applicable,
                    "detail": s.detail
                })).collect::<Vec<_>>(),
                "below_target": below,
                "node_count": total_nodes,
                "edge_count": snapshot.edge_count(),
                "dimensions_wired": 9,
                "coupled_modules": d_count,
                "orphan_intent": orphan_intent,
                "total_intent_nodes": total_intent,
                "snapshot_id": build_id,
                "delta_vs_previous": delta_vs_previous,
                "history_depth": previous_snapshots.len() + 1,
                "note": "acyclicity + resilience + coverage×centrality + main_sequence (Martin-D) + intent_alignment (intent→code half) + duplication (SIMILAR_TO edges, pgvector HNSW scan — see reconcile_duplication_edges, run out-of-band, NOT per-call) + module_depth (interface/impl ratio, APoSD) + impact_radius (bounded reverse-BFS p95) + god_objects (complexity AND fan-out, Rust counting landed — other languages read as unmeasured until their parser slice lands, REQ-AXO-902185); remaining (code→intent orphan half) via REQ-AXO-902185. Δ per-call persisted (REQ-AXO-902187) — re_surfaced=true means a below-target axis did not improve since the last measurement."
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
        // REQ-AXO-902279 (feedback #46) — name the orphans in the TEXT channel, not just
        // the count. `orphans` is already capped at `top` upstream, so this samples the
        // returned set (each annotated with its category) with truncation disclosure.
        let orphan_names: Vec<String> =
            orphans.iter().map(|o| format!("{} [{}]", o.name, o.category)).collect();
        let orphan_phrase = sample_identities("orphans", &orphan_names, 12);
        let summary = format!(
            "wiring {} : {} orphan(s) — {} test_only (delivered+tested but NO prod caller — the OPV class) + {} isolated (no caller at all, advisory). A test_only symbol tagged deliverable = must be wired before delivery (gate S3, axon_pre_flight_check).{}",
            project, orphans.len(), test_only, isolated, orphan_phrase
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

    /// REQ-AXO-902211 (REFINES CPT-AXO-90056) — DEAD CLUSTERS: groups of callable symbols
    /// reachable from NO root (main/handler/nif/SOLL role=entry), grouped by mutual
    /// connectivity. Complements `wiring` (per-SYMBOL: does X have >=1 non-test caller?),
    /// which is BLIND to a cluster of N functions calling ONLY each other — each has a
    /// caller (another dead member) so none is flagged, yet the whole group never runs.
    /// `roots` uses the SAME `role='entry'` SOLL convention as the S3 gate
    /// (`workflow_project.rs`) — deliberately NARROWER than `wiring`'s own `declared` set
    /// (which exempts ANY traceability row): a blanket exemption here would treat half the
    /// SOLL-tracked codebase as a root and mask real dead clusters. Requires `ist_snapshot_warm`.
    pub(crate) fn axon_orphan_clusters(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "orphan_clusters") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("orphan_clusters", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("orphan_clusters", &project)),
        };
        // REQ-AXO-902211 — same role='entry' query as the S3 gate (workflow_project.rs),
        // NOT `wiring`'s broad `declared` (any traceability row) — see doc comment above.
        let entry_raw = self
            .graph_store
            .query_json(
                "SELECT artifact_ref FROM soll.Traceability \
                 WHERE artifact_type = 'Symbol' AND metadata->>'role' = 'entry'",
            )
            .unwrap_or_else(|_| "[]".to_string());
        let declared_entries: std::collections::HashSet<String> =
            serde_json::from_str::<Vec<Vec<String>>>(&entry_raw)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|row| row.into_iter().next())
                .map(|s| s.to_ascii_lowercase())
                .collect();
        let report = crate::ist_snapshot::code_smells::orphan_clusters(
            &snapshot,
            &project,
            &declared_entries,
        );
        let clusters_json: Vec<Value> = report
            .clusters
            .iter()
            .map(|c| json!({ "size": c.len(), "nodes": c }))
            .collect();
        // REQ-AXO-902244 — coverage in the TEXT channel, which is the one an LLM actually
        // reads. S1 of REQ-AXO-902192 promised `wiring_coverage` and shipped only
        // `orphans[]`: the tool gave a list of dead things with no denominator, so
        // "3 dead clusters" was unreadable without knowing whether the project has 30
        // callables or 3000.
        let coverage_pct = report.wiring_coverage() * 100.0;
        let coverage_note = format!(
            " · wiring coverage {:.1}% ({}/{} candidates reachable from {} root(s)), {} leaf/leaves",
            coverage_pct,
            report.reached_count,
            report.candidate_count,
            report.root_count,
            report.leaves.len()
        );
        // REQ-AXO-902279 (feedback #46) — name the members of the largest dead cluster in
        // the TEXT channel: "77 dead clusters" is unactionable until an LLM knows WHICH
        // symbols are dead. Names are already in `data.clusters[].nodes`; this echoes a
        // bounded sample of the biggest (clusters are size-desc) with truncation disclosure.
        let cluster_phrase = match report.clusters.first() {
            Some(largest) => sample_identities(
                &format!("largest dead cluster ({} symbols)", largest.len()),
                largest,
                12,
            ),
            None => String::new(),
        };
        let summary = if report.clusters.is_empty() {
            format!(
                "orphan_clusters {} : 0 dead cluster(s) ({} unreached singleton(s) out of {} candidate(s)){}",
                project, report.unreached_count, report.candidate_count, coverage_note
            )
        } else {
            format!(
                "orphan_clusters {} : {} dead cluster(s), largest = {} symbols ({} total unreached out of {} candidate(s)){}{}",
                project,
                report.clusters.len(),
                report.clusters[0].len(),
                report.unreached_count,
                report.candidate_count,
                coverage_note,
                cluster_phrase
            )
        };
        // Cap the identity lists: `roots` is small by nature (79 on AXO) but `leaves` can
        // run to thousands, and this response is read into an LLM context. Full counts stay
        // available above, so truncation never hides the magnitude — only the enumeration.
        const IDENTITY_CAP: usize = 200;
        let roots_truncated = report.roots.len() > IDENTITY_CAP;
        let leaves_truncated = report.leaves.len() > IDENTITY_CAP;
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": if report.clusters.is_empty() { "ok" } else { "dead_clusters_detected" },
                "project_code": project,
                "candidate_count": report.candidate_count,
                "root_count": report.root_count,
                "unreached_count": report.unreached_count,
                "cluster_count": report.clusters.len(),
                "clusters": clusters_json,
                "soll_declared_entries": declared_entries.len(),
                // REQ-AXO-902244 — ADDITIVE only: every field above keeps its name and
                // meaning, because this response shape is consumed by the S3 gate of
                // axon_commit_work AND by the /wiring LiveView.
                "reached_count": report.reached_count,
                "wiring_coverage": report.wiring_coverage(),
                "roots": report.roots.iter().take(IDENTITY_CAP).collect::<Vec<_>>(),
                "roots_truncated": roots_truncated,
                "leaves": report.leaves.iter().take(IDENTITY_CAP).collect::<Vec<_>>(),
                "leaves_truncated": leaves_truncated,
                "leaves_note": "A LEAF is REACHED and calls no other candidate — a live endpoint, the OPPOSITE of dead code. Do not read it as a problem.",
                "note": "REQ-AXO-902211 — a lone unreached symbol (no dead neighbour) is NOT reported here (see `wiring`'s isolated category instead); this tool exists specifically for MUTUALLY-wired groups invisible to the per-symbol check. Advisory only — no gate."
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

    /// REQ-AXO-902360 — `debt_digest`: ONE RAM-native call for the worst structural debt,
    /// across categories, ranked by centrality (a central offender outweighs a leaf). DRY by
    /// construction — it ORCHESTRATES the existing engines (SIMILAR_TO edges / SOLL orphan
    /// intent / `wiring_orphans` / `orphan_clusters`), never re-deriving their logic
    /// (GUI-PRO-013). Extensible: add a category to `collect_debt_sections` — no new tool, no
    /// contract change. Surfaced at every init (`kickoff_bundle.debt_digest`, counts+pointer)
    /// and handoff (`axon_handoff_check`). Distinct from `tech_debt_inventory` (migration
    /// remnants) and `structural_health_worklist` (SHI axes). Requires `ist_snapshot_warm`.
    pub(crate) fn axon_debt_digest(&self, args: &Value) -> Option<Value> {
        let project = match self.ist_resolve_project(args, "debt_digest") {
            Ok(p) => p,
            Err(e) => return Some(e),
        };
        let view = process_view();
        if !view.is_warm(&project) {
            return Some(ist_cache_miss_error("debt_digest", &project));
        }
        let snapshot = match view.cache_handle().get(&project) {
            Some(s) => s,
            None => return Some(ist_cache_miss_error("debt_digest", &project)),
        };
        let top = args.get("top").and_then(|v| v.as_u64()).unwrap_or(10).clamp(1, 200) as usize;
        let wanted: Option<HashSet<String>> = args
            .get("sections")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
        let (counts, sections) =
            self.collect_debt_sections(&snapshot, &project, top, wanted.as_ref());

        let count_of = |k: &str| counts.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let summary = format!(
            "debt_digest {} : {} dry (SIMILAR_TO pairs) · {} unlinked_soll (intent w/o evidence) · {} unlinked_code (unwired symbols/clusters). Top {} per section ranked by centrality in data.sections.",
            project,
            count_of("dry"),
            count_of("unlinked_soll"),
            count_of("unlinked_code"),
            top
        );
        Some(json!({
            "content": [{ "type": "text", "text": summary }],
            "data": {
                "status": "ok",
                "project_code": project,
                "counts": counts,
                "sections": sections,
                "surfaces_used": ["graph_ram", "soll_ram"],
                "note": "REQ-AXO-902360 — RAM-native debt digest with an extensible section registry (add a category in collect_debt_sections). dry = SIMILAR_TO near-duplicate pairs (semantic_clones for the token diff); unlinked_soll = intent with 0 traceability (soll_attach_evidence); unlinked_code = wiring_orphans (0 prod caller) + dead orphan_clusters. Ranked by PageRank centrality. Distinct from tech_debt_inventory (migration remnants) and structural_health_worklist (SHI axes)."
            }
        }))
    }

    /// REQ-AXO-902360 — the shared RAM-native debt collection behind `debt_digest` AND its
    /// counts-only surfacing at init/handoff (single source — GUI-PRO-013). `top` bounds each
    /// section's offender list; `counts` are always the TRUE totals (full RAM scan). `wanted`
    /// = optional section-key filter. The `if want(..)` blocks ARE the registry: add a
    /// category here and it appears everywhere the digest is consumed (tool + kickoff +
    /// handoff), no contract change.
    pub(crate) fn collect_debt_sections(
        &self,
        snapshot: &IstGraph,
        project: &str,
        top: usize,
        wanted: Option<&HashSet<String>>,
    ) -> (serde_json::Map<String, Value>, Vec<Value>) {
        let want = |k: &str| wanted.map_or(true, |w| w.contains(k));
        let total_nodes = snapshot.node_count();
        // PageRank (O(V+E)) is the severity weight — only needed when we actually emit ranked
        // offenders. Counts-only callers (init/handoff surfacing, top=0) skip it so the
        // digest never lengthens init/handoff (REQ-AXO-902360 "must not slow init").
        let ranked = if top > 0 {
            pagerank_top(snapshot, 0.85, 50, total_nodes.max(1))
        } else {
            Vec::new()
        };
        let pr: HashMap<&str, f64> = ranked.iter().map(|(id, s)| (id.as_str(), *s as f64)).collect();
        let soll_snap = self.soll_cache().snapshot(project);

        let mut counts = serde_json::Map::new();
        let mut sections: Vec<Value> = Vec::new();

        // 1) dry — near-duplicate SIMILAR_TO pairs, ranked by the more-central endpoint.
        if want("dry") {
            let mut seen: HashSet<(u32, u32)> = HashSet::new();
            let mut pairs: Vec<(String, String, f64)> = Vec::new();
            for i in 0..total_nodes as u32 {
                for (t, rel) in snapshot.forward_neighbors(i) {
                    if matches!(rel, crate::ist_snapshot::RelationType::SimilarTo) {
                        let key = if i <= t { (i, t) } else { (t, i) };
                        if seen.insert(key) {
                            let a = snapshot.id_of(i).to_string();
                            let b = snapshot.id_of(t).to_string();
                            let score = pr
                                .get(a.as_str())
                                .copied()
                                .unwrap_or(0.0)
                                .max(pr.get(b.as_str()).copied().unwrap_or(0.0));
                            pairs.push((a, b, score));
                        }
                    }
                }
            }
            let total = pairs.len();
            pairs.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal));
            let offenders: Vec<Value> = pairs
                .iter()
                .take(top)
                .map(|(a, b, s)| {
                    json!({
                        "pair": [a, b],
                        "score": s,
                        "remediation": "factor the shared logic; `semantic_clones` for the token-level diff"
                    })
                })
                .collect();
            counts.insert("dry".into(), json!(total));
            sections.push(json!({
                "key": "dry",
                "description": "near-duplicate SIMILAR_TO pairs (semantic clones), most-central first",
                "total_available": total,
                "offenders": offenders
            }));
        }

        // 2) unlinked_soll — intent nodes (REQ/DEC/CPT/VAL) with ZERO traceability evidence.
        if want("unlinked_soll") {
            let mut orphans: Vec<Value> = Vec::new();
            let mut total = 0usize;
            if let Ok(snap) = soll_snap.as_ref() {
                for ty in ["Requirement", "Decision", "Concept", "Validation"] {
                    let lower = ty.to_ascii_lowercase();
                    for id in snap.node_ids_of_type(ty) {
                        if snap.traceability_count_for(&lower, id) == 0 {
                            total += 1;
                            if orphans.len() < top {
                                orphans.push(json!({
                                    "id": id,
                                    "type": ty,
                                    "remediation": "soll_attach_evidence, or link the intent (soll_manager link)"
                                }));
                            }
                        }
                    }
                }
            }
            counts.insert("unlinked_soll".into(), json!(total));
            sections.push(json!({
                "key": "unlinked_soll",
                "description": "SOLL intent nodes with zero traceability evidence",
                "total_available": total,
                "offenders": orphans
            }));
        }

        // 3) unlinked_code — unwired symbols (wiring_orphans) + dead clusters (orphan_clusters).
        if want("unlinked_code") {
            let declared: HashSet<String> = soll_snap
                .as_ref()
                .map(|snap| {
                    snap.traceability
                        .iter()
                        .filter(|t| t.artifact_type == "Symbol")
                        .map(|t| t.artifact_ref.to_ascii_lowercase())
                        .collect()
                })
                .unwrap_or_default();
            let orphans =
                crate::ist_snapshot::code_smells::wiring_orphans(snapshot, project, &declared, 1000);
            let n_symbols = orphans.len();
            let mut symbol_offenders: Vec<(Value, f64)> = orphans
                .iter()
                .map(|o| {
                    let s = pr.get(o.id.as_str()).copied().unwrap_or(0.0);
                    (
                        json!({
                            "kind": "unwired_symbol",
                            "id": o.id,
                            "name": o.name,
                            "category": o.category,
                            "score": s,
                            "remediation": "wire into a prod caller, or declare the entry (soll_attach_evidence role=entry)"
                        }),
                        s,
                    )
                })
                .collect();
            symbol_offenders
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            // Dead clusters use the NARROW role='entry' convention (same as the S3 gate /
            // `orphan_clusters` tool), not the broad `declared` set — a small indexed query.
            let entry_raw = self
                .graph_store
                .query_json(
                    "SELECT artifact_ref FROM soll.Traceability \
                     WHERE artifact_type = 'Symbol' AND metadata->>'role' = 'entry'",
                )
                .unwrap_or_else(|_| "[]".to_string());
            let declared_entries: HashSet<String> = serde_json::from_str::<Vec<Vec<String>>>(&entry_raw)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|r| r.into_iter().next())
                .map(|s| s.to_ascii_lowercase())
                .collect();
            let report = crate::ist_snapshot::code_smells::orphan_clusters(
                snapshot,
                project,
                &declared_entries,
            );
            let n_clusters = report.clusters.len();

            let mut offenders: Vec<Value> =
                symbol_offenders.into_iter().take(top).map(|(v, _)| v).collect();
            for c in report.clusters.iter().take(top.saturating_sub(offenders.len())) {
                offenders.push(json!({
                    "kind": "dead_cluster",
                    "size": c.len(),
                    "nodes": c.iter().take(8).collect::<Vec<_>>(),
                    "remediation": "the whole group is unreachable from any root — delete it, or wire an entry"
                }));
            }
            counts.insert("unlinked_code".into(), json!(n_symbols + n_clusters));
            sections.push(json!({
                "key": "unlinked_code",
                "description": "unwired symbols (0 prod caller) + dead clusters (unreachable from any root)",
                "total_available": n_symbols + n_clusters,
                "unwired_symbols": n_symbols,
                "dead_clusters": n_clusters,
                "offenders": offenders
            }));
        }

        (counts, sections)
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

    // REQ-AXO-902279 (feedback #46, NEX) — the TEXT summary of wiring/orphan_clusters/
    // ist_structural_sccs must NAME symbols and, when it caps the list, DISCLOSE the
    // truncation with the true magnitude. sample_identities is the shared renderer.
    #[test]
    fn sample_identities_lists_all_names_when_under_cap() {
        let names = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let out = super::sample_identities("orphans", &names, 12);
        assert!(
            out.contains("alpha") && out.contains("beta") && out.contains("gamma"),
            "all names present under cap: {out}"
        );
        // Not truncated → no "showing N of M" disclosure clause.
        assert!(!out.contains("showing"), "no truncation clause when under cap: {out}");
    }

    #[test]
    fn sample_identities_discloses_truncation_and_magnitude() {
        let names: Vec<String> = (0..80).map(|i| format!("sym{i}")).collect();
        let out = super::sample_identities("largest dead cluster (80 symbols)", &names, 12);
        // The first `cap` names are present; the (cap+1)-th is not.
        assert!(out.contains("sym0") && out.contains("sym11"), "capped names present: {out}");
        assert!(!out.contains("sym12"), "name past the cap must not appear: {out}");
        // Magnitude is NEVER hidden — the class of defect rejected for the S4 wiring image.
        assert!(out.contains("showing 12 of 80"), "truncation disclosed with magnitude: {out}");
    }

    #[test]
    fn sample_identities_empty_is_blank() {
        assert_eq!(super::sample_identities("orphans", &[], 12), "");
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
    fn martin_coupling_excludes_noncode_kinds_and_nondependency_edges() {
        // REQ-AXO-902230 — regression guard for the .md-domination dogfood. Build a tiny IST:
        // two .rs modules wired by a real CALLS dependency, a .rs module wired ONLY by
        // SIMILAR_TO, and two .md doc sections wired to each other by SIMILAR_TO (the exact
        // pollution pattern). Only the real dependency-coupled code modules reach mod_d.
        use crate::ist_snapshot::snapshot::{EdgeTriple, NodeRecord};
        use crate::ist_snapshot::{IstGraph, NodeFlags, RelationType};
        let n = |id: &str, kind: NodeKind| NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind,
            flags: NodeFlags::default(),
            complexity: None,
        };
        let e = |s: &str, t: &str, rel: RelationType| EdgeTriple {
            source: s.to_string(),
            target: t.to_string(),
            rel,
        };
        let nodes = vec![
            n("AXO::src::a.rs::foo", NodeKind::Function),
            n("AXO::src::b.rs::bar", NodeKind::Function),
            n("AXO::src::c.rs::baz", NodeKind::Function),
            n("AXO::docs::x.md::Purpose", NodeKind::Section),
            n("AXO::docs::y.md::Purpose", NodeKind::Section),
        ];
        let edges = vec![
            e("AXO::src::a.rs::foo", "AXO::src::b.rs::bar", RelationType::Calls),
            e("AXO::src::c.rs::baz", "AXO::src::a.rs::foo", RelationType::SimilarTo),
            e(
                "AXO::docs::x.md::Purpose",
                "AXO::docs::y.md::Purpose",
                RelationType::SimilarTo,
            ),
        ];
        let raw = super::compute_shi_raw_metrics(&IstGraph::build(nodes, edges), 0, 0);
        let keys: Vec<&str> = raw.mod_d.keys().map(String::as_str).collect();
        // Real cross-module dependency survives on BOTH endpoints.
        assert!(raw.mod_d.contains_key("AXO::src::a.rs"), "caller module present: {keys:?}");
        assert!(raw.mod_d.contains_key("AXO::src::b.rs"), "callee module present: {keys:?}");
        // Doc modules never appear (non-code kind AND non-dependency edge).
        assert!(!keys.iter().any(|k| k.contains(".md")), "no .md coupling module: {keys:?}");
        // A code module wired ONLY by SIMILAR_TO carries no Martin coupling (relation gate).
        assert!(!raw.mod_d.contains_key("AXO::src::c.rs"), "SIMILAR_TO-only excluded: {keys:?}");
    }

    // REQ-AXO-902214 — build_sub_scores wires the capability signal into the coverage axis.
    fn raw_metrics(total_testable_symbols: usize, covered_pr: f64, total_pr: f64) -> super::ShiRawMetrics {
        super::ShiRawMetrics {
            total_nodes: 100,
            sccs: vec![],
            articulation: vec![],
            covered_pr,
            total_pr,
            mod_d: std::collections::HashMap::new(),
            mean_distance: 0.0,
            d_count: 0,
            orphan_intent: 0,
            total_intent: 0,
            clone_pairs: 0,
            total_testable_symbols,
            mean_public_ratio: 0.0,
            mod_pub_total_count: 0,
            median_impact_radius: 0,
            p95_impact_radius: 0,
            god_objects: 0,
            total_real_functions: 0,
        }
    }

    #[test]
    fn build_sub_scores_neutralizes_weighted_coverage_when_no_coverage_capable_symbol() {
        // A corpus with ZERO coverage-capable symbols (pure .lll / Python / Elixir) must
        // NEUTRALIZE the axis — not read "100% covered" (the pre-902214 mislabel that inflated
        // SHI). total_testable_symbols == 0 is the capability trigger.
        let scores = super::build_sub_scores(&raw_metrics(0, 0.0, 0.0));
        let cov = scores.iter().find(|s| s.name == "weighted_coverage").expect("axis present");
        assert!(cov.not_applicable, "neutralized when no coverage-capable symbol exists");
        assert_eq!(cov.weight, 0.0, "weight 0 → excluded from the geometric aggregate");
    }

    #[test]
    fn build_sub_scores_keeps_weighted_coverage_measured_at_real_zero_coverage() {
        // The session-100 revert landmine: a REAL Rust project at 0% coverage (covered_pr==0
        // but testable symbols EXIST) must stay a MEASURED 0.0 below target — NEVER neutralized
        // (else it would inflate SHI + vanish from the worklist). Trigger on the capability
        // count, never on covered_pr==0.
        let scores = super::build_sub_scores(&raw_metrics(42, 0.0, 100.0));
        let cov = scores.iter().find(|s| s.name == "weighted_coverage").expect("axis present");
        assert!(!cov.not_applicable, "0% real coverage is a measured failure, not not_applicable");
        assert_eq!(cov.weight, 1.0);
        assert_eq!(cov.value, 0.0, "0 covered / 100 total → measured 0.0, below the 0.80 target");
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
