// REQ-AXO-91488 (MIL-AXO-019 slice 4) — petgraph algorithms over IstGraph.
//
// IstGraph holds a CSR representation tuned for the 12 B / node target
// (CPT-AXO-90003). Implementing petgraph::visit::* traits directly on it
// would be ~200 LOC. v1 takes a pragmatic shortcut : copy CSR ↔ petgraph
// graph for each algorithm call. Cost : one O(N + M) pass with ~80 B / node
// temporary allocation. For AXO scale (~11k nodes, ~32k edges) that is
// 1-2 MB transient ; still 30× cheaper than the equivalent SQL recursion.
// Full trait impl is a follow-up REQ when call rate justifies it.

use std::collections::HashMap;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::ist_snapshot::snapshot::{IstGraph, RelationType};

/// Convert an IstGraph CSR view into a `petgraph::DiGraph`. The petgraph
/// node weight stores the canonical IST id (so callers can map results back
/// without holding the IstGraph reference) ; the edge weight stores the
/// relation_type as u8 to keep the structure compact.
pub fn to_petgraph(graph: &IstGraph) -> (DiGraph<String, u8>, Vec<NodeIndex>) {
    let n = graph.node_count();
    let mut pg: DiGraph<String, u8> = DiGraph::with_capacity(n, graph.edge_count());
    let mut node_idx: Vec<NodeIndex> = Vec::with_capacity(n);
    for i in 0..n as u32 {
        node_idx.push(pg.add_node(graph.id_of(i).to_string()));
    }
    for src in 0..n as u32 {
        let src_pg = node_idx[src as usize];
        for (tgt, rel) in graph.forward_neighbors(src) {
            pg.add_edge(src_pg, node_idx[tgt as usize], rel as u8);
        }
    }
    (pg, node_idx)
}

/// REQ-AXO-91488 — PageRank centrality. Returns `(id, score)` pairs sorted
/// by descending score. `damping` is the standard 0.85 used in the original
/// Brin/Page paper ; `iterations` should be ≥30 for convergence on graphs
/// up to 1M nodes (petgraph 0.6 does not implement an early-stop).
pub fn pagerank_top(
    graph: &IstGraph,
    damping: f32,
    iterations: usize,
    top: usize,
) -> Vec<(String, f32)> {
    if graph.node_count() == 0 {
        return Vec::new();
    }
    let (pg, _) = to_petgraph(graph);
    let scores = petgraph::algo::page_rank(&pg, damping, iterations);
    let mut pairs: Vec<(String, f32)> = scores
        .into_iter()
        .enumerate()
        .map(|(i, s)| (graph.id_of(i as u32).to_string(), s))
        .collect();
    pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    pairs.truncate(top);
    pairs
}

/// REQ-AXO-91488 — Tarjan strongly-connected components on IST (not SOLL ;
/// for SOLL use `soll_acyclic_audit` REQ-AXO-91492). Returns SCCs with > 1
/// node (true cycles) ordered by descending size.
pub fn structural_sccs(graph: &IstGraph) -> Vec<Vec<String>> {
    if graph.node_count() == 0 {
        return Vec::new();
    }
    let (pg, _) = to_petgraph(graph);
    let raw = petgraph::algo::tarjan_scc(&pg);
    let mut out: Vec<Vec<String>> = raw
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|component| component.into_iter().map(|i| pg[i].clone()).collect())
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()));
    out
}

/// REQ-AXO-91488 — single-direction BFS shortest path with parent tracking.
/// Returns the canonical id sequence (source → target inclusive) or `None`
/// if no path exists or either endpoint is unknown. Respects `rel_filter`
/// (empty ⇒ all). The bidirectional variant is a follow-up optimization
/// once the call rate exceeds 100 path queries / sec.
pub fn shortest_path(
    graph: &IstGraph,
    from_id: &str,
    to_id: &str,
    max_radius: u32,
    rel_filter: &[RelationType],
) -> Option<Vec<String>> {
    let start = graph.index_of(from_id)?;
    let end = graph.index_of(to_id)?;
    if start == end {
        return Some(vec![from_id.to_string()]);
    }

    let rel_allowed =
        |rel: RelationType| -> bool { rel_filter.is_empty() || rel_filter.contains(&rel) };

    let mut parent: HashMap<u32, Option<u32>> = HashMap::new();
    parent.insert(start, None);
    let mut frontier: Vec<u32> = vec![start];
    for _ in 0..max_radius {
        let mut next: Vec<u32> = Vec::new();
        for node in frontier.iter().copied() {
            for (target, rel) in graph.forward_neighbors(node) {
                if !rel_allowed(rel) {
                    continue;
                }
                if parent.contains_key(&target) {
                    continue;
                }
                parent.insert(target, Some(node));
                if target == end {
                    return Some(reconstruct_path(target, &parent, graph));
                }
                next.push(target);
            }
        }
        if next.is_empty() {
            return None;
        }
        frontier = next;
    }
    None
}

fn reconstruct_path(
    end: u32,
    parent: &HashMap<u32, Option<u32>>,
    graph: &IstGraph,
) -> Vec<String> {
    let mut chain: Vec<u32> = Vec::new();
    let mut cursor = Some(end);
    while let Some(node) = cursor {
        chain.push(node);
        cursor = parent.get(&node).and_then(|x| *x);
    }
    chain.reverse();
    chain
        .into_iter()
        .map(|i| graph.id_of(i).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord};

    fn n(id: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        }
    }

    fn e(s: &str, t: &str, r: RelationType) -> EdgeTriple {
        EdgeTriple {
            source: s.to_string(),
            target: t.to_string(),
            rel: r,
        }
    }

    #[test]
    fn pagerank_returns_top_n_sorted_descending() {
        // hub a is pointed-to by b, c, d ; d has no callers => lower score
        let nodes = vec![n("a"), n("b"), n("c"), n("d")];
        let edges = vec![
            e("b", "a", RelationType::Calls),
            e("c", "a", RelationType::Calls),
            e("d", "a", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let top = pagerank_top(&g, 0.85, 50, 4);
        assert_eq!(top.len(), 4);
        assert_eq!(top[0].0, "a", "hub should rank first");
        // Scores monotonically descending.
        for w in top.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
    }

    #[test]
    fn pagerank_empty_graph_returns_empty() {
        let g = IstGraph::build(vec![], vec![]);
        assert!(pagerank_top(&g, 0.85, 10, 5).is_empty());
    }

    #[test]
    fn structural_sccs_detects_two_node_cycle() {
        let nodes = vec![n("a"), n("b"), n("c")];
        let edges = vec![
            e("a", "b", RelationType::Calls),
            e("b", "a", RelationType::Calls),
            e("c", "a", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let sccs = structural_sccs(&g);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
        let set: std::collections::HashSet<&str> = sccs[0].iter().map(String::as_str).collect();
        assert!(set.contains("a"));
        assert!(set.contains("b"));
    }

    #[test]
    fn structural_sccs_ignores_dag() {
        let nodes = vec![n("a"), n("b"), n("c")];
        let edges = vec![
            e("a", "b", RelationType::Calls),
            e("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(structural_sccs(&g).is_empty());
    }

    #[test]
    fn shortest_path_finds_direct_route() {
        let nodes = vec![n("a"), n("b"), n("c")];
        let edges = vec![
            e("a", "b", RelationType::Calls),
            e("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let path = shortest_path(&g, "a", "c", 5, &[]).expect("path exists");
        assert_eq!(path, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn shortest_path_same_endpoint_returns_singleton() {
        let nodes = vec![n("a")];
        let g = IstGraph::build(nodes, vec![]);
        let path = shortest_path(&g, "a", "a", 5, &[]).unwrap();
        assert_eq!(path, vec!["a".to_string()]);
    }

    #[test]
    fn shortest_path_disconnected_returns_none() {
        let nodes = vec![n("a"), n("b")];
        let g = IstGraph::build(nodes, vec![]);
        assert!(shortest_path(&g, "a", "b", 5, &[]).is_none());
    }

    #[test]
    fn shortest_path_unknown_endpoint_returns_none() {
        let nodes = vec![n("a")];
        let g = IstGraph::build(nodes, vec![]);
        assert!(shortest_path(&g, "a", "zz", 5, &[]).is_none());
        assert!(shortest_path(&g, "zz", "a", 5, &[]).is_none());
    }

    #[test]
    fn shortest_path_respects_relation_filter() {
        let nodes = vec![n("a"), n("b")];
        let edges = vec![e("a", "b", RelationType::Contains)];
        let g = IstGraph::build(nodes, edges);
        assert!(shortest_path(&g, "a", "b", 5, &[RelationType::Calls]).is_none());
        assert!(shortest_path(&g, "a", "b", 5, &[RelationType::Contains]).is_some());
    }
}
