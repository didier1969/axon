// REQ-AXO-91489 (MIL-AXO-019 slice 5) — Reciprocal Rank Fusion (Cormack 2009).
//
// Pure scoring primitive : given N ranked lists (vector, FTS, graph), fuse
// them into a single canonical ranking via :
//
//     score(d) = Σ_{i ∈ lists} 1 / (k + rank_i(d))     k = 60
//
// Documents that appear in more than one list dominate. Missing entries in
// a list contribute 0 (no penalty). Optional centrality boost
// (`× (1 + α × pagerank_norm)`) and reachability filter live behind their
// own helpers so callers gate them per route (CPT-AXO-90007).
//
// The integration of this primitive into the entry/chunk candidate paths
// inside `candidates.rs` is staged behind `AXON_RRF_ENABLED` and remains a
// follow-up : the algorithm itself ships standalone first so tests pin its
// behaviour before any path-rewrite lands.

use std::collections::HashMap;

pub const RRF_K_CORMACK: f64 = 60.0;

#[derive(Clone, Debug)]
pub struct RankedDoc {
    pub id: String,
    pub vec_rank: Option<usize>,
    pub fts_rank: Option<usize>,
    pub graph_rank: Option<usize>,
    pub centrality_boost: f64,
    pub reachable_from_anchor: bool,
}

#[derive(Clone, Debug)]
pub struct FusedDoc {
    pub id: String,
    pub vec_rank: Option<usize>,
    pub fts_rank: Option<usize>,
    pub graph_rank: Option<usize>,
    pub rrf_score: f64,
    pub centrality_boost: f64,
    pub reachable_from_anchor: bool,
    pub fusion_method: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct RrfInputs {
    pub vec_order: Vec<String>,
    pub fts_order: Vec<String>,
    pub graph_order: Vec<String>,
    pub centrality_boost: HashMap<String, f64>,
    pub anchor_reachable: HashMap<String, bool>,
}

/// Build the per-id rank map from a ranked list (1-based ranks).
fn rank_map(order: &[String]) -> HashMap<String, usize> {
    order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i + 1))
        .collect()
}

/// Reciprocal Rank Fusion. `k = 60` matches the Cormack 2009 default and
/// petgraph-style search literature. Optional `centrality_alpha` scales
/// the boost contribution (set 0.0 to disable centrality). Optional
/// `require_reachable` filters out candidates whose `reachable_from_anchor`
/// is false — used by route `Impact`/`Wiring` per CPT-AXO-90007.
pub fn rrf_fuse(
    inputs: &RrfInputs,
    k: f64,
    centrality_alpha: f64,
    require_reachable: bool,
    top: usize,
) -> Vec<FusedDoc> {
    let vec_rank = rank_map(&inputs.vec_order);
    let fts_rank = rank_map(&inputs.fts_order);
    let graph_rank = rank_map(&inputs.graph_order);

    let mut union: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for id in inputs.vec_order.iter().chain(inputs.fts_order.iter()).chain(inputs.graph_order.iter())
    {
        union.insert(id.as_str());
    }

    let mut out: Vec<FusedDoc> = Vec::with_capacity(union.len());
    for id in union {
        let v = vec_rank.get(id).copied();
        let f = fts_rank.get(id).copied();
        let g = graph_rank.get(id).copied();
        let mut score = 0.0_f64;
        if let Some(r) = v {
            score += 1.0 / (k + r as f64);
        }
        if let Some(r) = f {
            score += 1.0 / (k + r as f64);
        }
        if let Some(r) = g {
            score += 1.0 / (k + r as f64);
        }
        let boost = inputs.centrality_boost.get(id).copied().unwrap_or(0.0);
        if centrality_alpha > 0.0 && boost > 0.0 {
            score *= 1.0 + centrality_alpha * boost;
        }
        let reachable = inputs
            .anchor_reachable
            .get(id)
            .copied()
            .unwrap_or(false);
        if require_reachable && !reachable {
            continue;
        }
        out.push(FusedDoc {
            id: id.to_string(),
            vec_rank: v,
            fts_rank: f,
            graph_rank: g,
            rrf_score: score,
            centrality_boost: boost,
            reachable_from_anchor: reachable,
            fusion_method: "rrf_cormack_k60",
        });
    }
    out.sort_by(|a, b| b.rrf_score.partial_cmp(&a.rrf_score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(top);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rrf_doc_appearing_in_all_three_lists_ranks_first() {
        // d1 appears in all three lists, d2 in two, d3 in one
        let inputs = RrfInputs {
            vec_order: ids(&["d2", "d1", "d3"]),
            fts_order: ids(&["d1", "d2"]),
            graph_order: ids(&["d1"]),
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 10);
        assert_eq!(fused[0].id, "d1");
        assert_eq!(fused[1].id, "d2");
        assert_eq!(fused[2].id, "d3");
    }

    #[test]
    fn rrf_uses_k60_cormack_constant() {
        // single-list d at rank 1 should score exactly 1/(60+1).
        let inputs = RrfInputs {
            vec_order: ids(&["d"]),
            fts_order: vec![],
            graph_order: vec![],
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 1);
        let expected = 1.0 / (60.0 + 1.0);
        assert!((fused[0].rrf_score - expected).abs() < 1e-12);
    }

    #[test]
    fn rrf_missing_entries_contribute_zero_not_penalty() {
        let inputs = RrfInputs {
            vec_order: ids(&["d1"]),
            fts_order: vec![],
            graph_order: vec![],
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 10);
        assert_eq!(fused.len(), 1);
        assert!(fused[0].rrf_score > 0.0);
    }

    #[test]
    fn rrf_centrality_boost_scales_score() {
        let inputs_no_boost = RrfInputs {
            vec_order: ids(&["d"]),
            fts_order: ids(&["d"]),
            graph_order: vec![],
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let baseline = rrf_fuse(&inputs_no_boost, RRF_K_CORMACK, 0.0, false, 1)[0].rrf_score;
        let mut boost = HashMap::new();
        boost.insert("d".to_string(), 1.0);
        let inputs_boost = RrfInputs {
            vec_order: ids(&["d"]),
            fts_order: ids(&["d"]),
            graph_order: vec![],
            centrality_boost: boost,
            anchor_reachable: HashMap::new(),
        };
        let boosted = rrf_fuse(&inputs_boost, RRF_K_CORMACK, 0.5, false, 1)[0].rrf_score;
        assert!(boosted > baseline);
        // alpha=0.5, boost=1.0 → score × (1 + 0.5) = 1.5×
        assert!((boosted / baseline - 1.5).abs() < 1e-12);
    }

    #[test]
    fn rrf_reachability_filter_drops_unreachable() {
        let mut reach = HashMap::new();
        reach.insert("d1".to_string(), true);
        reach.insert("d2".to_string(), false);
        let inputs = RrfInputs {
            vec_order: ids(&["d1", "d2"]),
            fts_order: ids(&["d2", "d1"]),
            graph_order: vec![],
            centrality_boost: HashMap::new(),
            anchor_reachable: reach,
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, true, 10);
        let ids_returned: Vec<&str> = fused.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids_returned, vec!["d1"]);
    }

    #[test]
    fn rrf_diagnostics_carry_per_modality_rank() {
        let inputs = RrfInputs {
            vec_order: ids(&["d", "x"]),
            fts_order: ids(&["d"]),
            graph_order: ids(&["x", "d"]),
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 10);
        let d = fused.iter().find(|f| f.id == "d").unwrap();
        assert_eq!(d.vec_rank, Some(1));
        assert_eq!(d.fts_rank, Some(1));
        assert_eq!(d.graph_rank, Some(2));
        assert_eq!(d.fusion_method, "rrf_cormack_k60");
    }

    #[test]
    fn rrf_top_truncates_output() {
        let inputs = RrfInputs {
            vec_order: ids(&["a", "b", "c", "d"]),
            fts_order: vec![],
            graph_order: vec![],
            centrality_boost: HashMap::new(),
            anchor_reachable: HashMap::new(),
        };
        let fused = rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 2);
        assert_eq!(fused.len(), 2);
    }

    #[test]
    fn rrf_empty_inputs_returns_empty() {
        let inputs = RrfInputs::default();
        assert!(rrf_fuse(&inputs, RRF_K_CORMACK, 0.0, false, 10).is_empty());
    }
}
