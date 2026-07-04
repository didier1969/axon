//! REQ-AXO-902184 / CPT-AXO-90055 — Structural Health Index (SHI): a RAM-native
//! aggregate of normalized structural-quality sub-scores.
//!
//! This module is the PURE math layer — normalization (a raw graph metric → a [0,1]
//! sub-score, 1 = healthy) plus the weighted GEOMETRIC aggregate — so every scoring
//! decision is unit-testable without a runtime, a clock, or disk (same discipline as
//! `release_reconciler`). Feeding the REAL IST/SOLL metrics in (the warm-snapshot
//! plumbing) and exposing the MCP `structural_health_index` tool are later slices
//! (REQ-AXO-902185 dimensions, REQ-AXO-902184 tool surface).
//!
//! GEOMETRIC, not arithmetic (CPT-AXO-90055 anti-Goodhart): one rotten axis (sub-score
//! → 0) drags the whole index → 0, so a brilliant axis CANNOT mask a broken one. The
//! sub-scores are ALWAYS surfaced individually — the aggregate is a compass, never the
//! sole verdict.

/// One normalized structural dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct SubScore {
    /// Stable machine name, e.g. "acyclicity", "duplication", "main_sequence".
    pub name: &'static str,
    /// Health value in [0,1] — 1.0 = healthy, 0.0 = worst. Clamped on construction.
    pub value: f64,
    /// Aggregation weight (relative). Non-negative; a zero weight excludes the axis.
    pub weight: f64,
    /// The target this axis should reach (for the worklist / display), in [0,1].
    pub target: f64,
    /// Human one-liner explaining the raw measurement behind `value`.
    pub detail: String,
}

impl SubScore {
    /// Build a sub-score, clamping `value`/`target` into [0,1] and `weight` to >= 0.
    pub fn new(name: &'static str, value: f64, weight: f64, target: f64, detail: impl Into<String>) -> Self {
        SubScore {
            name,
            value: clamp01(value),
            weight: weight.max(0.0),
            target: clamp01(target),
            detail: detail.into(),
        }
    }

    /// True when the axis meets or beats its target (drives the worklist: below-target
    /// axes are the remediation candidates).
    pub fn meets_target(&self) -> bool {
        self.value >= self.target
    }
}

/// Clamp a float into the unit interval; NaN maps to 0.0 (a missing/undefined measure is
/// treated as worst, never silently healthy).
fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// The computed index: the individual sub-scores + their weighted geometric aggregate.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuralHealthIndex {
    pub sub_scores: Vec<SubScore>,
    /// Weighted geometric mean of the sub-scores' values, in [0,1].
    pub aggregate: f64,
}

impl StructuralHealthIndex {
    pub fn compute(sub_scores: Vec<SubScore>) -> Self {
        let aggregate = geometric_aggregate(&sub_scores);
        StructuralHealthIndex {
            sub_scores,
            aggregate,
        }
    }

    /// The axes below their target, worst-first — the raw material of the remediation
    /// worklist (REQ-AXO-902186 ranks these by ROI = expected ΔSHI ÷ blast radius).
    pub fn below_target(&self) -> Vec<&SubScore> {
        let mut v: Vec<&SubScore> = self.sub_scores.iter().filter(|s| !s.meets_target()).collect();
        v.sort_by(|a, b| a.value.partial_cmp(&b.value).unwrap_or(std::cmp::Ordering::Equal));
        v
    }
}

/// Weighted geometric mean of the sub-scores' values: `exp( Σ wᵢ·ln(vᵢ) / Σ wᵢ )`.
///
/// - A single `value == 0` forces the aggregate to 0 (the anti-masking property —
///   `ln(0) = -∞`, handled explicitly so no NaN escapes).
/// - Zero-weight axes are ignored. All-zero weight (or empty input) → 1.0 (vacuously
///   healthy: nothing was measured to be wrong).
/// - The result is clamped to [0,1] against float rounding.
pub fn geometric_aggregate(scores: &[SubScore]) -> f64 {
    let total_weight: f64 = scores.iter().map(|s| s.weight).sum();
    if total_weight <= 0.0 {
        return 1.0;
    }
    // Any zero-valued (positively-weighted) axis zeroes the whole index.
    if scores.iter().any(|s| s.weight > 0.0 && s.value <= 0.0) {
        return 0.0;
    }
    let weighted_ln_sum: f64 = scores
        .iter()
        .filter(|s| s.weight > 0.0)
        .map(|s| s.weight * s.value.ln())
        .sum();
    clamp01((weighted_ln_sum / total_weight).exp())
}

// ---------------------------------------------------------------------------
// Normalization functions — raw structural metric → [0,1] sub-score value (1 = healthy).
// Each is pure and independently unit-tested; the MCP tool feeds them real snapshot
// numbers and wraps the result in a `SubScore` with a weight + target.
// ---------------------------------------------------------------------------

/// Acyclicity: the fraction of nodes NOT inside any cycle. 1.0 = perfect DAG.
/// `nodes_in_cycles` = Σ sizes of SCCs with size > 1 (from `ist_structural_sccs`).
pub fn acyclicity_score(nodes_in_cycles: usize, total_nodes: usize) -> f64 {
    if total_nodes == 0 {
        return 1.0;
    }
    clamp01(1.0 - (nodes_in_cycles as f64) / (total_nodes as f64))
}

/// Duplication: `1 - clone_pairs_over_threshold / total_functions`. 1.0 = no duplication.
/// A corpus-wide clone RATE (REQ-AXO-902185), not a single-symbol sample.
pub fn duplication_score(clone_pairs_over_threshold: usize, total_functions: usize) -> f64 {
    if total_functions == 0 {
        return 1.0;
    }
    clamp01(1.0 - (clone_pairs_over_threshold as f64) / (total_functions as f64))
}

/// Layering integrity: `1 - drift_violations / total_edges`. 1.0 = no architectural drift.
pub fn layering_score(drift_violations: usize, total_edges: usize) -> f64 {
    if total_edges == 0 {
        return 1.0;
    }
    clamp01(1.0 - (drift_violations as f64) / (total_edges as f64))
}

/// Main-sequence health from Martin's distance `D = |A + I − 1|` (0 = on the sequence,
/// 1 = zone of pain/uselessness). `mean_distance` is the module-averaged D → score
/// `1 − mean_D`. 1.0 = every module sits on the main sequence.
pub fn main_sequence_score(mean_distance: f64) -> f64 {
    clamp01(1.0 - mean_distance)
}

/// Intent↔code alignment: `1 − (orphan_intent_frac + orphan_code_frac) / 2`, where each
/// fraction is in [0,1] (orphan intent = SOLL nodes with no code; orphan code = symbols
/// with no governing intent). 1.0 = perfect SOLL↔IST alignment.
pub fn intent_alignment_score(orphan_intent_frac: f64, orphan_code_frac: f64) -> f64 {
    clamp01(1.0 - (clamp01(orphan_intent_frac) + clamp01(orphan_code_frac)) / 2.0)
}

/// Centrality-weighted test coverage: `Σ(tested_pagerank) / Σ(pagerank)`. Weighting by
/// PageRank asks the load-bearing question — are the HUBS tested? — not the flat symbol
/// ratio. 1.0 = all centrality sits on tested symbols.
pub fn weighted_coverage_score(tested_pagerank_sum: f64, total_pagerank_sum: f64) -> f64 {
    if total_pagerank_sum <= 0.0 {
        return 1.0;
    }
    clamp01(tested_pagerank_sum / total_pagerank_sum)
}

/// Martin's distance from the main sequence for ONE module: `D = |A + I − 1|`, where the
/// instability `I = Ce / (Ca + Ce)` (0 when the module has no coupling). D=0 = balanced
/// (on the sequence); D→1 = the "zone of pain" (concrete + stable → rigid) or the "zone of
/// uselessness" (abstract + unstable). `abstractness` A ∈ [0,1] = abstract types / total
/// types. The SHI sub-score is `main_sequence_score(mean_D)` over the modules.
pub fn martin_distance(afferent: usize, efferent: usize, abstractness: f64) -> f64 {
    let coupling = afferent + efferent;
    let instability = if coupling == 0 {
        0.0
    } else {
        efferent as f64 / coupling as f64
    };
    ((clamp01(abstractness) + instability - 1.0).abs()).min(1.0)
}

/// Resilience: `1 - articulation_points / total_nodes`. An articulation point is a node
/// whose removal disconnects the graph — a single point of failure. 1.0 = no SPOF.
pub fn resilience_score(articulation_points: usize, total_nodes: usize) -> f64 {
    if total_nodes == 0 {
        return 1.0;
    }
    clamp01(1.0 - (articulation_points as f64) / (total_nodes as f64))
}

/// Migration completeness: the mean `migrated_fraction` across ACTIVE tech migrations
/// (from `tech_debt_inventory`). 1.0 = no incomplete migration residue.
pub fn migration_completeness_score(migrated_fractions: &[f64]) -> f64 {
    if migrated_fractions.is_empty() {
        return 1.0;
    }
    let sum: f64 = migrated_fractions.iter().map(|f| clamp01(*f)).sum();
    clamp01(sum / migrated_fractions.len() as f64)
}

/// Module depth (REQ-AXO-902185, APoSD/GUI-PRO-018): `interface(nb pub) / impl(taille
/// corps)` per module, averaged across coupled modules. A LOW ratio = a small public
/// interface hiding a large implementation (a "deep" module — healthy, per APoSD's
/// thesis that deep modules with simple interfaces beat shallow ones). A HIGH ratio =
/// most of the module's symbols are public relative to its total size — internals
/// leaking out as surface area (shallow — unhealthy). Score inverts the mean ratio so
/// 1.0 = deep/healthy, 0.0 = every symbol in every module is public (maximally shallow).
pub fn module_depth_score(mean_public_ratio: f64) -> f64 {
    clamp01(1.0 - mean_public_ratio)
}

/// Impact radius (REQ-AXO-902185): normalizes the tail (p95) blast radius — the count of
/// transitive dependents reached by a bounded reverse BFS from a symbol — against the
/// total node count. A change whose ripple reaches a LARGE fraction of the graph is
/// structurally risky (poor modularity / too many transitive dependents); one contained
/// to a small slice is healthy. 1.0 = p95 radius is a negligible slice of the graph,
/// 0.0 = it covers (nearly) the whole graph.
pub fn impact_radius_score(p95_radius: usize, total_nodes: usize) -> f64 {
    if total_nodes == 0 {
        return 1.0;
    }
    clamp01(1.0 - (p95_radius as f64 / total_nodes as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &'static str, value: f64, weight: f64) -> SubScore {
        SubScore::new(name, value, weight, 0.9, "")
    }

    // --- geometric aggregate -------------------------------------------------

    #[test]
    fn aggregate_all_perfect_is_one() {
        let scores = vec![s("a", 1.0, 1.0), s("b", 1.0, 2.0), s("c", 1.0, 0.5)];
        assert!((geometric_aggregate(&scores) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_zero_axis_zeroes_the_index_anti_masking() {
        // THE anti-Goodhart property: a brilliant axis cannot mask a broken one.
        let scores = vec![s("acyclicity", 0.9996, 1.0), s("coupling", 0.0, 1.0)];
        assert_eq!(geometric_aggregate(&scores), 0.0);
    }

    #[test]
    fn aggregate_is_geometric_not_arithmetic() {
        // Geometric mean of 0.25 and 1.0 (equal weight) = 0.5, below the arithmetic 0.625.
        let scores = vec![s("a", 0.25, 1.0), s("b", 1.0, 1.0)];
        let g = geometric_aggregate(&scores);
        assert!((g - 0.5).abs() < 1e-9, "geometric mean should be 0.5, got {g}");
        assert!(g < 0.625, "must be below the arithmetic mean (which would mask the weak axis)");
    }

    #[test]
    fn aggregate_respects_weights() {
        // Heavier weight on the strong axis pulls the mean up vs equal weights.
        let equal = geometric_aggregate(&[s("a", 0.5, 1.0), s("b", 1.0, 1.0)]);
        let heavy_strong = geometric_aggregate(&[s("a", 0.5, 1.0), s("b", 1.0, 3.0)]);
        assert!(heavy_strong > equal);
    }

    #[test]
    fn aggregate_empty_or_zero_weight_is_vacuously_one() {
        assert_eq!(geometric_aggregate(&[]), 1.0);
        assert_eq!(geometric_aggregate(&[s("a", 0.2, 0.0)]), 1.0);
    }

    #[test]
    fn subscore_clamps_and_nan_is_worst() {
        assert_eq!(SubScore::new("x", 1.5, 1.0, 2.0, "").value, 1.0);
        assert_eq!(SubScore::new("x", -0.3, 1.0, 0.9, "").value, 0.0);
        assert_eq!(SubScore::new("x", f64::NAN, 1.0, 0.9, "").value, 0.0);
        assert_eq!(SubScore::new("x", 0.5, -2.0, 0.9, "").weight, 0.0);
    }

    #[test]
    fn below_target_lists_failing_axes_worst_first() {
        let idx = StructuralHealthIndex::compute(vec![
            SubScore::new("good", 0.95, 1.0, 0.9, ""),
            SubScore::new("bad", 0.30, 1.0, 0.9, ""),
            SubScore::new("worse", 0.10, 1.0, 0.9, ""),
        ]);
        let failing: Vec<&str> = idx.below_target().iter().map(|s| s.name).collect();
        assert_eq!(failing, vec!["worse", "bad"], "worst-first, target-meeting axis excluded");
    }

    // --- normalization functions --------------------------------------------

    #[test]
    fn acyclicity_matches_measured_axo() {
        // s95 measured: 1 SCC of 4 nodes out of 11393 → ~0.99965.
        let sc = acyclicity_score(4, 11393);
        assert!((sc - 0.9996489).abs() < 1e-6, "got {sc}");
        assert_eq!(acyclicity_score(0, 100), 1.0);
        assert_eq!(acyclicity_score(0, 0), 1.0);
    }

    #[test]
    fn duplication_and_layering_are_one_minus_rate() {
        assert!((duplication_score(1, 100) - 0.99).abs() < 1e-9);
        assert_eq!(duplication_score(0, 500), 1.0);
        assert_eq!(duplication_score(5, 0), 1.0);
        assert!((layering_score(2, 50) - 0.96).abs() < 1e-9);
    }

    #[test]
    fn main_sequence_score_inverts_distance() {
        assert_eq!(main_sequence_score(0.0), 1.0); // on the sequence
        assert_eq!(main_sequence_score(1.0), 0.0); // zone of pain
        assert!((main_sequence_score(0.3) - 0.7).abs() < 1e-9);
    }

    #[test]
    fn intent_alignment_penalizes_both_orphan_kinds() {
        assert_eq!(intent_alignment_score(0.0, 0.0), 1.0);
        assert!((intent_alignment_score(0.2, 0.4) - 0.7).abs() < 1e-9);
        assert_eq!(intent_alignment_score(1.0, 1.0), 0.0);
    }

    #[test]
    fn weighted_coverage_asks_are_hubs_tested() {
        // 14% flat coverage but the hubs (high pagerank) are tested → high weighted score.
        assert!((weighted_coverage_score(9.0, 10.0) - 0.9).abs() < 1e-9);
        assert_eq!(weighted_coverage_score(0.0, 0.0), 1.0);
    }

    #[test]
    fn martin_distance_zones() {
        // On the main sequence (D=0): unstable+concrete (I=1,A=0) OR stable+abstract (I=0,A=1).
        assert!((martin_distance(0, 5, 0.0)).abs() < 1e-9); // I=1, A=0 → |0+1-1|=0
        assert!((martin_distance(5, 0, 1.0)).abs() < 1e-9); // I=0, A=1 → |1+0-1|=0
        // Zone of pain (D=1): concrete + stable (many depend on it, it depends on nothing).
        assert!((martin_distance(5, 0, 0.0) - 1.0).abs() < 1e-9); // I=0, A=0 → |0+0-1|=1
        // Zone of uselessness (D=1): abstract + unstable.
        assert!((martin_distance(0, 5, 1.0) - 1.0).abs() < 1e-9); // I=1, A=1 → |1+1-1|=1
        // No coupling → I=0; concrete isolated module sits at D=1 (dead weight).
        assert!((martin_distance(0, 0, 0.0) - 1.0).abs() < 1e-9);
        // Balanced middle.
        assert!((martin_distance(1, 1, 0.0) - 0.5).abs() < 1e-9); // I=0.5, A=0 → 0.5
    }

    #[test]
    fn resilience_score_penalizes_articulation_points() {
        assert_eq!(resilience_score(0, 100), 1.0);
        assert!((resilience_score(5, 100) - 0.95).abs() < 1e-9);
        assert_eq!(resilience_score(3, 0), 1.0);
    }

    #[test]
    fn migration_completeness_is_mean_migrated() {
        assert!((migration_completeness_score(&[1.0, 0.78, 0.0]) - 0.5933333).abs() < 1e-6);
        assert_eq!(migration_completeness_score(&[]), 1.0);
    }

    #[test]
    fn module_depth_inverts_public_ratio() {
        assert_eq!(module_depth_score(0.0), 1.0); // nothing public — maximally deep
        assert_eq!(module_depth_score(1.0), 0.0); // everything public — maximally shallow
        assert!((module_depth_score(0.2) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn impact_radius_score_penalizes_wide_ripple() {
        assert_eq!(impact_radius_score(0, 100), 1.0);
        assert!((impact_radius_score(20, 100) - 0.8).abs() < 1e-9);
        assert_eq!(impact_radius_score(100, 100), 0.0);
        assert_eq!(impact_radius_score(5, 0), 1.0);
    }

    #[test]
    fn end_to_end_index_from_measured_axo_snapshot() {
        // Compose the two s95-measured axes + a couple of placeholders into an index and
        // assert the aggregate reflects the weakest axis (coverage), not the strongest.
        let idx = StructuralHealthIndex::compute(vec![
            SubScore::new("acyclicity", acyclicity_score(4, 11393), 1.0, 0.99, "1 SCC/4 of 11393"),
            SubScore::new("duplication", duplication_score(0, 900), 1.0, 0.98, "no clone pairs"),
            SubScore::new("layering", layering_score(0, 50578), 1.0, 1.0, "no drift"),
            SubScore::new("weighted_coverage", weighted_coverage_score(14.0, 100.0), 1.0, 0.8, "14% weighted"),
        ]);
        assert_eq!(idx.sub_scores.len(), 4);
        // The 0.14 coverage axis drags the geometric aggregate well below the ~1.0 others.
        assert!(idx.aggregate < 0.65, "weak coverage must drag the index down: {}", idx.aggregate);
        assert!(idx.aggregate > 0.0);
        // Coverage is the sole below-target axis.
        assert_eq!(idx.below_target().iter().map(|s| s.name).collect::<Vec<_>>(), vec!["weighted_coverage"]);
    }
}
