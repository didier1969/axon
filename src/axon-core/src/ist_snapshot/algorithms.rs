// REQ-AXO-91488 (MIL-AXO-019 slice 4) — petgraph algorithms over IstGraph.
//
// IstGraph holds a CSR representation tuned for the 12 B / node target
// (CPT-AXO-90003). Implementing petgraph::visit::* traits directly on it
// would be ~200 LOC. v1 takes a pragmatic shortcut : copy CSR ↔ petgraph
// graph for each algorithm call. Cost : one O(N + M) pass with ~80 B / node
// temporary allocation. For AXO scale (~11k nodes, ~32k edges) that is
// 1-2 MB transient ; still 30× cheaper than the equivalent SQL recursion.
// Full trait impl is a follow-up REQ when call rate justifies it.

use std::collections::{HashMap, HashSet, VecDeque};

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
/// REQ-AXO-901923 — sparse power-iteration PageRank directly on the CSR
/// snapshot, O(iterations · (V + E)).
///
/// The previous implementation delegated to `petgraph::algo::page_rank`, which
/// is O(V² · iterations) (it probes edge existence across every node pair per
/// iteration). On the AXO IST (~22.5k nodes) that is ~25 billion ops over 50
/// iterations and blew the MCP gateway timeout every call. This native sparse
/// walk over `forward_neighbors` finishes in milliseconds on the same graph,
/// with standard dangling-node mass redistribution so the rank vector stays a
/// proper probability distribution.
pub fn pagerank_top(
    graph: &IstGraph,
    damping: f32,
    iterations: usize,
    top: usize,
) -> Vec<(String, f32)> {
    let n = graph.node_count();
    if n == 0 {
        return Vec::new();
    }
    let nf = n as f32;

    // Out-degree per node (distinct forward edges in the CSR).
    let out_deg: Vec<u32> = (0..n as u32)
        .map(|u| graph.forward_neighbors(u).count() as u32)
        .collect();

    let mut rank = vec![1.0f32 / nf; n];
    let teleport = (1.0 - damping) / nf;

    for _ in 0..iterations {
        // Mass held by dangling nodes (no out-edges) is redistributed
        // uniformly so total rank is conserved.
        let dangling: f32 = (0..n).filter(|&u| out_deg[u] == 0).map(|u| rank[u]).sum();
        let dangling_share = damping * dangling / nf;

        let mut next = vec![teleport + dangling_share; n];
        for u in 0..n as u32 {
            let d = out_deg[u as usize];
            if d == 0 {
                continue;
            }
            let contribution = damping * rank[u as usize] / d as f32;
            for (target, _rel) in graph.forward_neighbors(u) {
                next[target as usize] += contribution;
            }
        }
        rank = next;
    }

    let mut pairs: Vec<(String, f32)> = rank
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
    // REQ-AXO-901928 — petgraph 0.6.5 `tarjan_scc` is RECURSIVE (its own doc
    // says "This implementation is recursive"); on a deep call-chain it
    // recurses to depth = chain length and stack-overflows (SIGABRT observed on
    // a synthetic 20k-node deep graph). Use an explicit-stack iterative Tarjan
    // so the tool degrades gracefully instead of aborting the process
    // (PIL-AXO-002 no dead-end). Same pattern as the iterative DFS in
    // `bridges_and_articulation`.
    let raw = tarjan_scc_iterative(&pg);
    let mut out: Vec<Vec<String>> = raw
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|component| component.into_iter().map(|i| pg[i].clone()).collect())
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()));
    out
}

/// REQ-AXO-901928 — iterative Tarjan strongly-connected-components over a
/// `petgraph::DiGraph`, with an explicit work stack instead of recursion so it
/// cannot overflow the call stack on pathologically deep graphs. Returns every
/// SCC (callers filter by size). Classic Tarjan with `index`/`lowlink`/on-stack
/// state held in flat `Vec`s keyed by `NodeIndex::index()`.
fn tarjan_scc_iterative(pg: &DiGraph<String, u8>) -> Vec<Vec<NodeIndex>> {
    let n = pg.node_count();
    const UNVISITED: u32 = u32::MAX;
    let mut index_of: Vec<u32> = vec![UNVISITED; n];
    let mut lowlink: Vec<u32> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut comp_stack: Vec<NodeIndex> = Vec::new();
    let mut sccs: Vec<Vec<NodeIndex>> = Vec::new();
    let mut counter: u32 = 0;

    for root in pg.node_indices() {
        if index_of[root.index()] != UNVISITED {
            continue;
        }
        // Each frame = (node, its successors, next-successor cursor). Indexing
        // `work[..]` directly (not holding `last_mut()`) keeps the borrow
        // checker happy across the `work.push` for a tree edge.
        let mut work: Vec<(NodeIndex, Vec<NodeIndex>, usize)> = Vec::new();
        index_of[root.index()] = counter;
        lowlink[root.index()] = counter;
        counter += 1;
        comp_stack.push(root);
        on_stack[root.index()] = true;
        work.push((root, pg.neighbors(root).collect(), 0));

        while let Some(frame_idx) = work.len().checked_sub(1) {
            let v = work[frame_idx].0;
            let pos = work[frame_idx].2;
            if pos < work[frame_idx].1.len() {
                let w = work[frame_idx].1[pos];
                work[frame_idx].2 += 1;
                if index_of[w.index()] == UNVISITED {
                    index_of[w.index()] = counter;
                    lowlink[w.index()] = counter;
                    counter += 1;
                    comp_stack.push(w);
                    on_stack[w.index()] = true;
                    work.push((w, pg.neighbors(w).collect(), 0));
                } else if on_stack[w.index()] {
                    lowlink[v.index()] = lowlink[v.index()].min(index_of[w.index()]);
                }
            } else {
                // All successors of v explored: if v is an SCC root, pop it.
                if lowlink[v.index()] == index_of[v.index()] {
                    let mut component = Vec::new();
                    loop {
                        let w = comp_stack.pop().expect("comp_stack non-empty");
                        on_stack[w.index()] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    sccs.push(component);
                }
                work.pop();
                if let Some(parent) = work.last() {
                    let p = parent.0.index();
                    lowlink[p] = lowlink[p].min(lowlink[v.index()]);
                }
            }
        }
    }
    sccs
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

fn reconstruct_path(end: u32, parent: &HashMap<u32, Option<u32>>, graph: &IstGraph) -> Vec<String> {
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

/// REQ-AXO-91488 follow-up — bidirectional BFS (meet-in-the-middle).
/// Returns the shortest path source → target inclusive ; usually 2-4×
/// faster than `shortest_path` for long radii because frontier growth
/// is O(b^(d/2)) instead of O(b^d). Returns `None` when no path exists
/// or either endpoint is unknown. Used by `bidi_trace` tool.
pub fn bidi_bfs(
    graph: &IstGraph,
    from_id: &str,
    to_id: &str,
    max_radius: u32,
) -> Option<Vec<String>> {
    let start = graph.index_of(from_id)?;
    let end = graph.index_of(to_id)?;
    if start == end {
        return Some(vec![from_id.to_string()]);
    }

    // Two BFS frontiers, parent maps anchored at start/end respectively.
    let mut fwd_parent: HashMap<u32, Option<u32>> = HashMap::new();
    fwd_parent.insert(start, None);
    let mut rev_parent: HashMap<u32, Option<u32>> = HashMap::new();
    rev_parent.insert(end, None);

    let mut fwd_frontier: Vec<u32> = vec![start];
    let mut rev_frontier: Vec<u32> = vec![end];

    for _ in 0..max_radius {
        // Expand the smaller frontier first (balanced search).
        let (expand_fwd, expand_frontier, expand_parent, opposite_parent) =
            if fwd_frontier.len() <= rev_frontier.len() {
                (true, &mut fwd_frontier, &mut fwd_parent, &rev_parent)
            } else {
                (false, &mut rev_frontier, &mut rev_parent, &fwd_parent)
            };
        let mut next: Vec<u32> = Vec::new();
        for node in expand_frontier.iter().copied() {
            let neighbours: Vec<(u32, RelationType)> = if expand_fwd {
                graph.forward_neighbors(node).collect()
            } else {
                graph.reverse_neighbors(node).collect()
            };
            for (nbr, _rel) in neighbours {
                if expand_parent.contains_key(&nbr) {
                    continue;
                }
                expand_parent.insert(nbr, Some(node));
                if opposite_parent.contains_key(&nbr) {
                    // Meet point — splice both halves.
                    return Some(splice_bidi_path(nbr, &fwd_parent, &rev_parent, graph));
                }
                next.push(nbr);
            }
        }
        if next.is_empty() {
            return None;
        }
        *expand_frontier = next;
    }
    None
}

fn splice_bidi_path(
    meet: u32,
    fwd_parent: &HashMap<u32, Option<u32>>,
    rev_parent: &HashMap<u32, Option<u32>>,
    graph: &IstGraph,
) -> Vec<String> {
    let mut fwd_chain: Vec<u32> = Vec::new();
    let mut cursor = Some(meet);
    while let Some(node) = cursor {
        fwd_chain.push(node);
        cursor = fwd_parent.get(&node).and_then(|x| *x);
    }
    fwd_chain.reverse();

    let mut rev_chain: Vec<u32> = Vec::new();
    let mut cursor = rev_parent.get(&meet).and_then(|x| *x);
    while let Some(node) = cursor {
        rev_chain.push(node);
        cursor = rev_parent.get(&node).and_then(|x| *x);
    }

    fwd_chain
        .into_iter()
        .chain(rev_chain)
        .map(|i| graph.id_of(i).to_string())
        .collect()
}

/// REQ-AXO-91488 follow-up — Tarjan low-link bridges + articulation points
/// over the **undirected projection** of IstGraph (a bridge in a directed
/// call graph is meaningful only when treating CALLS as bidirectional
/// connectivity). Returns `(bridges, articulation_points)` as canonical
/// id pairs / nodes. Used by `anomalies` (91517) + `audit` (91525) +
/// `impact` cognitive layer.
pub fn bridges_and_articulation(graph: &IstGraph) -> (Vec<(String, String)>, Vec<String>) {
    let n = graph.node_count();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    // Build deduplicated undirected adjacency : both directions of any
    // directed edge collapse to a single undirected link. Without this,
    // a pair `a→b` + `b→a` would create 2 parallel edges and break
    // Tarjan's `low[u_idx] > disc[p_idx]` bridge predicate.
    let mut adj_sets: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    for src in 0..n as u32 {
        for (tgt, _) in graph.forward_neighbors(src) {
            if src == tgt {
                continue;
            }
            adj_sets[src as usize].insert(tgt);
            adj_sets[tgt as usize].insert(src);
        }
    }
    let adj: Vec<Vec<u32>> = adj_sets
        .into_iter()
        .map(|s| {
            let mut v: Vec<u32> = s.into_iter().collect();
            v.sort_unstable();
            v
        })
        .collect();

    let mut visited: Vec<bool> = vec![false; n];
    let mut disc: Vec<u32> = vec![0; n];
    let mut low: Vec<u32> = vec![0; n];
    let mut parent: Vec<Option<u32>> = vec![None; n];
    let mut bridges: Vec<(u32, u32)> = Vec::new();
    let mut ap: HashSet<u32> = HashSet::new();
    let mut timer: u32 = 0;

    // Iterative DFS — recursion would blow the stack on a 100k-node graph.
    for root in 0..n as u32 {
        if visited[root as usize] {
            continue;
        }
        let mut stack: Vec<(u32, usize)> = vec![(root, 0)];
        let mut root_children = 0u32;
        visited[root as usize] = true;
        disc[root as usize] = timer;
        low[root as usize] = timer;
        timer += 1;

        while let Some(&(u, ci)) = stack.last() {
            let u_idx = u as usize;
            if ci < adj[u_idx].len() {
                let v = adj[u_idx][ci];
                stack.last_mut().unwrap().1 += 1;
                if !visited[v as usize] {
                    visited[v as usize] = true;
                    parent[v as usize] = Some(u);
                    disc[v as usize] = timer;
                    low[v as usize] = timer;
                    timer += 1;
                    if u == root {
                        root_children += 1;
                    }
                    stack.push((v, 0));
                } else if parent[u_idx] != Some(v) {
                    low[u_idx] = low[u_idx].min(disc[v as usize]);
                }
            } else {
                stack.pop();
                if let Some(p) = parent[u_idx] {
                    let p_idx = p as usize;
                    low[p_idx] = low[p_idx].min(low[u_idx]);
                    if low[u_idx] > disc[p_idx] {
                        bridges.push((p, u));
                    }
                    if p != root && low[u_idx] >= disc[p_idx] {
                        ap.insert(p);
                    }
                }
            }
        }
        if root_children > 1 {
            ap.insert(root);
        }
    }

    let bridge_ids: Vec<(String, String)> = bridges
        .into_iter()
        .map(|(a, b)| (graph.id_of(a).to_string(), graph.id_of(b).to_string()))
        .collect();
    let ap_ids: Vec<String> = ap.into_iter().map(|i| graph.id_of(i).to_string()).collect();
    (bridge_ids, ap_ids)
}

/// REQ-AXO-91488 follow-up — Level-by-level BFS layers. Used by
/// `retrieve_context_layered` (91524) to materialise the per-depth
/// neighbourhood independently. Layer 0 = source itself ; layer k =
/// nodes reachable in exactly k hops. Stops at `max_depth` inclusive.
pub fn bfs_layers(
    graph: &IstGraph,
    source: &str,
    max_depth: u32,
    rel_filter: &[RelationType],
) -> Vec<Vec<String>> {
    let Some(start) = graph.index_of(source) else {
        return Vec::new();
    };
    let rel_allowed =
        |rel: RelationType| -> bool { rel_filter.is_empty() || rel_filter.contains(&rel) };

    let mut visited: HashSet<u32> = HashSet::from([start]);
    let mut layers: Vec<Vec<String>> = vec![vec![graph.id_of(start).to_string()]];
    let mut frontier: Vec<u32> = vec![start];
    for _ in 0..max_depth {
        let mut next: Vec<u32> = Vec::new();
        for node in frontier.iter().copied() {
            for (tgt, rel) in graph.forward_neighbors(node) {
                if !rel_allowed(rel) {
                    continue;
                }
                if visited.insert(tgt) {
                    next.push(tgt);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        layers.push(next.iter().map(|&i| graph.id_of(i).to_string()).collect());
        frontier = next;
    }
    layers
}

/// REQ-AXO-91488 follow-up — Layer violations. Each node is assigned a
/// layer based on the prefix of its canonical id (e.g. `core/`, `mcp/`,
/// `db/`). A violation is an edge crossing layers in the wrong
/// direction (higher → lower index). Used by `architectural_drift`
/// (91516). `layer_def` maps prefix → priority (lower priority = lower
/// layer, may not call higher).
pub fn layer_violations(
    graph: &IstGraph,
    layer_def: &[(&str, u32)],
) -> Vec<(String, String, u32, u32)> {
    let layer_for = |id: &str| -> Option<u32> {
        layer_def
            .iter()
            .find(|(prefix, _)| id.starts_with(prefix))
            .map(|&(_, prio)| prio)
    };

    let n = graph.node_count();
    let mut violations: Vec<(String, String, u32, u32)> = Vec::new();
    for src in 0..n as u32 {
        let src_id = graph.id_of(src);
        let Some(src_layer) = layer_for(src_id) else {
            continue;
        };
        for (tgt, _rel) in graph.forward_neighbors(src) {
            let tgt_id = graph.id_of(tgt);
            let Some(tgt_layer) = layer_for(tgt_id) else {
                continue;
            };
            if src_layer < tgt_layer {
                violations.push((src_id.to_string(), tgt_id.to_string(), src_layer, tgt_layer));
            }
        }
    }
    violations
}

/// REQ-AXO-91488 follow-up — Snapshot diff. Compute the symmetric edge
/// difference between two IstGraphs. Returns `(added, removed)` where
/// each tuple is `(source_id, target_id, relation_type)`. Used by
/// `snapshot_diff` (91519) and `diff` (91520) tools. Symbol-set diff
/// is left to the caller (cheaper HashSet diff on canonical ids).
pub fn snapshot_edge_diff(
    before: &IstGraph,
    after: &IstGraph,
) -> (
    Vec<(String, String, RelationType)>,
    Vec<(String, String, RelationType)>,
) {
    let edges = |g: &IstGraph| -> HashSet<(String, String, u8)> {
        let mut out: HashSet<(String, String, u8)> = HashSet::new();
        for src in 0..g.node_count() as u32 {
            for (tgt, rel) in g.forward_neighbors(src) {
                out.insert((
                    g.id_of(src).to_string(),
                    g.id_of(tgt).to_string(),
                    rel as u8,
                ));
            }
        }
        out
    };
    let before_set = edges(before);
    let after_set = edges(after);

    let rel_from_u8 = |code: u8| -> RelationType {
        // Mirror the canonical u8 ↔ RelationType mapping in
        // `snapshot::relation_from_u8` ; defensive fallback to Calls
        // because the only consumer is a user-facing report.
        match code {
            0 => RelationType::Contains,
            1 => RelationType::Calls,
            2 => RelationType::CallsNif,
            _ => RelationType::Other,
        }
    };

    let added: Vec<(String, String, RelationType)> = after_set
        .difference(&before_set)
        .cloned()
        .map(|(s, t, r)| (s, t, rel_from_u8(r)))
        .collect();
    let removed: Vec<(String, String, RelationType)> = before_set
        .difference(&after_set)
        .cloned()
        .map(|(s, t, r)| (s, t, rel_from_u8(r)))
        .collect();
    (added, removed)
}

/// REQ-AXO-91488 follow-up — Minimal VF2-style subgraph isomorphism.
/// Searches for occurrences of `query` (small graph) within `host`
/// (typically the full IST snapshot). Returns mappings query_id →
/// host_id (canonical ids). Used by `semantic_clones` (91518) on
/// candidate clusters pre-filtered by BGE-Large vector similarity ;
/// not designed for unbounded host scale.
///
/// Algorithm : backtracking with degree-pruning. Cordella et al. 2004
/// VF2 with simplified feasibility checks (degree + already-mapped
/// edge consistency). Stops at `max_matches` to bound runtime.
pub fn vf2_subgraph_match(
    query: &IstGraph,
    host: &IstGraph,
    max_matches: usize,
) -> Vec<HashMap<String, String>> {
    let q_n = query.node_count();
    let h_n = host.node_count();
    if q_n == 0 || h_n == 0 || q_n > h_n {
        return Vec::new();
    }

    // Pre-compute degree signatures for pruning.
    let q_degree: Vec<usize> = (0..q_n as u32)
        .map(|i| query.forward_neighbors(i).count() + query.reverse_neighbors(i).count())
        .collect();
    let h_degree: Vec<usize> = (0..h_n as u32)
        .map(|i| host.forward_neighbors(i).count() + host.reverse_neighbors(i).count())
        .collect();

    let mut matches: Vec<HashMap<String, String>> = Vec::new();
    let mut mapping: HashMap<u32, u32> = HashMap::new();
    let mut used_host: HashSet<u32> = HashSet::new();

    vf2_recurse(
        query,
        host,
        &q_degree,
        &h_degree,
        &mut mapping,
        &mut used_host,
        &mut matches,
        max_matches,
    );

    matches
}

fn vf2_recurse(
    query: &IstGraph,
    host: &IstGraph,
    q_degree: &[usize],
    h_degree: &[usize],
    mapping: &mut HashMap<u32, u32>,
    used_host: &mut HashSet<u32>,
    matches: &mut Vec<HashMap<String, String>>,
    max_matches: usize,
) {
    if matches.len() >= max_matches {
        return;
    }
    if mapping.len() == query.node_count() {
        let mapped: HashMap<String, String> = mapping
            .iter()
            .map(|(&q, &h)| (query.id_of(q).to_string(), host.id_of(h).to_string()))
            .collect();
        matches.push(mapped);
        return;
    }

    // Pick the next query node (smallest unmapped index — deterministic).
    let q_next: u32 = (0..query.node_count() as u32)
        .find(|i| !mapping.contains_key(i))
        .expect("non-empty unmapped set");

    let q_deg = q_degree[q_next as usize];
    for h_candidate in 0..host.node_count() as u32 {
        if used_host.contains(&h_candidate) {
            continue;
        }
        // Degree pruning : host node must have ≥ query node degree.
        if h_degree[h_candidate as usize] < q_deg {
            continue;
        }
        // Consistency check : every already-mapped edge in query must
        // exist in host with same direction.
        if !vf2_consistent(query, host, mapping, q_next, h_candidate) {
            continue;
        }
        mapping.insert(q_next, h_candidate);
        used_host.insert(h_candidate);
        vf2_recurse(
            query,
            host,
            q_degree,
            h_degree,
            mapping,
            used_host,
            matches,
            max_matches,
        );
        mapping.remove(&q_next);
        used_host.remove(&h_candidate);
        if matches.len() >= max_matches {
            return;
        }
    }
}

fn vf2_consistent(
    query: &IstGraph,
    host: &IstGraph,
    mapping: &HashMap<u32, u32>,
    q_new: u32,
    h_new: u32,
) -> bool {
    // For every existing query → q_new edge, the mapped host node
    // must have a corresponding host → h_new edge.
    for (q_pred, _) in query.reverse_neighbors(q_new) {
        if let Some(&h_pred) = mapping.get(&q_pred) {
            if !host.forward_neighbors(h_pred).any(|(t, _)| t == h_new) {
                return false;
            }
        }
    }
    // For every q_new → existing query edge, host must have h_new → host.
    for (q_succ, _) in query.forward_neighbors(q_new) {
        if let Some(&h_succ) = mapping.get(&q_succ) {
            if !host.forward_neighbors(h_new).any(|(t, _)| t == h_succ) {
                return false;
            }
        }
    }
    true
}

/// Silence unused-import warnings on `VecDeque` until consumers land.
#[allow(dead_code)]
fn _vec_deque_anchor() -> VecDeque<u32> {
    VecDeque::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeKind, NodeRecord};

    fn n(id: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
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
    fn latency_bench_ram_algorithms_on_large_graph() {
        // REQ-AXO-901923 — self-measured latency guard at ≈ AXO IST scale
        // (20k nodes / 40k edges). Proves the RAM algorithms stay far under
        // the MCP gateway budget. The old petgraph O(V²) PageRank took tens of
        // seconds here; the sparse CSR impl is single-digit ms. Run with
        // `--nocapture` to print the measured timings.
        let count: u32 = 20_000;
        let mut nodes = Vec::with_capacity(count as usize);
        let mut edges = Vec::with_capacity(count as usize * 2);
        for i in 0..count {
            nodes.push(n(&format!("AXO::s{i}")));
        }
        for i in 0..count {
            // Deterministic ~2 edges/node (no RNG — resume-safe).
            edges.push(e(
                &format!("AXO::s{i}"),
                &format!("AXO::s{}", i.wrapping_mul(7).wrapping_add(1) % count),
                RelationType::Calls,
            ));
            edges.push(e(
                &format!("AXO::s{i}"),
                &format!("AXO::s{}", i.wrapping_mul(13).wrapping_add(3) % count),
                RelationType::Calls,
            ));
        }
        let g = IstGraph::build(nodes, edges);

        // PageRank is the worst-case latency tool (the old petgraph impl was
        // O(V²·iter)); it is iterative so the deep synthetic chains are safe.
        let t = std::time::Instant::now();
        let pr = pagerank_top(&g, 0.85, 50, 10);
        let pr_ms = t.elapsed().as_millis();

        // Reverse-radius RAM traversal (the primitive behind impact/bidi_trace),
        // bounded depth so it stays iterative-cheap.
        let t = std::time::Instant::now();
        let callers = g.bfs_reverse("AXO::s7", 4, 10_000, &[]);
        let bfs_ms = t.elapsed().as_millis();

        println!(
            "LATENCY_BENCH 20k nodes / 40k edges -> pagerank(50it)={pr_ms}ms bfs_reverse(d4)={bfs_ms}ms (pr_top={}, callers={})",
            pr.len(),
            callers.len()
        );
        assert_eq!(pr.len(), 10);
        assert!(pr_ms < 5_000, "pagerank latency regressed: {pr_ms}ms");
        assert!(bfs_ms < 5_000, "bfs_reverse latency regressed: {bfs_ms}ms");
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
    fn structural_sccs_deep_chain_does_not_stack_overflow() {
        // REQ-AXO-901928 — a long acyclic call-chain. The recursive petgraph
        // `tarjan_scc` recursed to depth = chain length and SIGABRT'd here; the
        // iterative implementation must complete and (a DAG) report no SCC > 1.
        const DEPTH: usize = 60_000;
        let nodes: Vec<_> = (0..DEPTH).map(|i| n(&format!("c{i}"))).collect();
        let edges: Vec<_> = (0..DEPTH - 1)
            .map(|i| e(&format!("c{i}"), &format!("c{}", i + 1), RelationType::Calls))
            .collect();
        let g = IstGraph::build(nodes, edges);
        assert!(
            structural_sccs(&g).is_empty(),
            "a deep acyclic chain has no SCC > 1"
        );
    }

    #[test]
    fn structural_sccs_detects_large_deep_cycle() {
        // REQ-AXO-901928 — a deep cycle (chain + back edge) is ONE big SCC. The
        // iterative Tarjan must find it without recursing chain-deep.
        const SIZE: usize = 5_000;
        let nodes: Vec<_> = (0..SIZE).map(|i| n(&format!("k{i}"))).collect();
        let mut edges: Vec<_> = (0..SIZE - 1)
            .map(|i| e(&format!("k{i}"), &format!("k{}", i + 1), RelationType::Calls))
            .collect();
        edges.push(e(&format!("k{}", SIZE - 1), "k0", RelationType::Calls));
        let g = IstGraph::build(nodes, edges);
        let sccs = structural_sccs(&g);
        assert_eq!(sccs.len(), 1, "the whole cycle is one SCC");
        assert_eq!(sccs[0].len(), SIZE, "every node is in the cycle");
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
        assert_eq!(
            path,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
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

    // ── REQ-AXO-91488 follow-up — new algos ────────────────────────

    #[test]
    fn bidi_bfs_finds_path_meeting_in_middle() {
        let nodes = vec![n("a"), n("b"), n("c"), n("d"), n("e")];
        let edges = vec![
            e("a", "b", RelationType::Calls),
            e("b", "c", RelationType::Calls),
            e("c", "d", RelationType::Calls),
            e("d", "e", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let path = bidi_bfs(&g, "a", "e", 10).expect("path exists");
        assert_eq!(path, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn bidi_bfs_same_endpoint_returns_singleton() {
        let nodes = vec![n("a")];
        let g = IstGraph::build(nodes, vec![]);
        assert_eq!(bidi_bfs(&g, "a", "a", 5), Some(vec!["a".to_string()]));
    }

    #[test]
    fn bidi_bfs_disconnected_returns_none() {
        let nodes = vec![n("a"), n("b")];
        let g = IstGraph::build(nodes, vec![]);
        assert!(bidi_bfs(&g, "a", "b", 5).is_none());
    }

    #[test]
    fn bridges_finds_isthmus_edge() {
        // Two triangles (a-b-c) and (d-e-f) connected by a single
        // bridge c-d. The undirected projection has 7 edges total ;
        // only c-d is a bridge (removing it disconnects {a,b,c} from
        // {d,e,f}). Bidirectional edges in CALLS form the triangle
        // cycles ; the single c→d directed edge is the isthmus.
        let nodes = vec![n("a"), n("b"), n("c"), n("d"), n("e"), n("f")];
        let edges = vec![
            // Triangle 1
            e("a", "b", RelationType::Calls),
            e("b", "c", RelationType::Calls),
            e("c", "a", RelationType::Calls),
            // Triangle 2
            e("d", "e", RelationType::Calls),
            e("e", "f", RelationType::Calls),
            e("f", "d", RelationType::Calls),
            // Single bridge connecting the triangles
            e("c", "d", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let (bridges, _) = bridges_and_articulation(&g);
        assert_eq!(
            bridges.len(),
            1,
            "expected exactly one bridge (c-d), got: {bridges:?}"
        );
        let pair: HashSet<&str> = [bridges[0].0.as_str(), bridges[0].1.as_str()]
            .into_iter()
            .collect();
        assert!(pair.contains("c") && pair.contains("d"));
    }

    #[test]
    fn bridges_empty_graph_returns_empty() {
        let g = IstGraph::build(vec![], vec![]);
        let (bridges, ap) = bridges_and_articulation(&g);
        assert!(bridges.is_empty());
        assert!(ap.is_empty());
    }

    #[test]
    fn bfs_layers_groups_nodes_by_depth() {
        let nodes = vec![n("root"), n("l1a"), n("l1b"), n("l2")];
        let edges = vec![
            e("root", "l1a", RelationType::Calls),
            e("root", "l1b", RelationType::Calls),
            e("l1a", "l2", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let layers = bfs_layers(&g, "root", 3, &[]);
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["root".to_string()]);
        // Layer 1 contains both children (order depends on adjacency).
        let l1: HashSet<&str> = layers[1].iter().map(String::as_str).collect();
        assert!(l1.contains("l1a") && l1.contains("l1b"));
        assert_eq!(layers[2], vec!["l2".to_string()]);
    }

    #[test]
    fn bfs_layers_unknown_source_returns_empty() {
        let g = IstGraph::build(vec![n("a")], vec![]);
        assert!(bfs_layers(&g, "missing", 5, &[]).is_empty());
    }

    #[test]
    fn layer_violations_detects_upward_edges() {
        // db (prio 0) <-- core (prio 1) <-- mcp (prio 2). Edges going
        // upward (db → core or core → mcp) violate ; downward OK.
        let nodes = vec![n("db/foo"), n("core/bar"), n("mcp/baz")];
        let edges = vec![
            // OK : downward.
            e("mcp/baz", "core/bar", RelationType::Calls),
            e("core/bar", "db/foo", RelationType::Calls),
            // Violation : core calls mcp (upward).
            e("core/bar", "mcp/baz", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let v = layer_violations(&g, &[("db/", 0), ("core/", 1), ("mcp/", 2)]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "core/bar");
        assert_eq!(v[0].1, "mcp/baz");
    }

    #[test]
    fn snapshot_diff_detects_added_and_removed() {
        let nodes = vec![n("a"), n("b"), n("c")];
        let before = IstGraph::build(
            nodes.clone(),
            vec![
                e("a", "b", RelationType::Calls),
                e("b", "c", RelationType::Calls),
            ],
        );
        let after = IstGraph::build(
            nodes,
            vec![
                e("a", "b", RelationType::Calls),
                e("a", "c", RelationType::Calls), // added
                                                  // b → c removed
            ],
        );
        let (added, removed) = snapshot_edge_diff(&before, &after);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].0, "a");
        assert_eq!(added[0].1, "c");
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "b");
        assert_eq!(removed[0].1, "c");
    }

    #[test]
    fn vf2_finds_simple_chain_match() {
        // Query : a → b → c. Host : x → y → z → w with extra y → q.
        let q_nodes = vec![n("a"), n("b"), n("c")];
        let q_edges = vec![
            e("a", "b", RelationType::Calls),
            e("b", "c", RelationType::Calls),
        ];
        let query = IstGraph::build(q_nodes, q_edges);

        let h_nodes = vec![n("x"), n("y"), n("z"), n("w"), n("q")];
        let h_edges = vec![
            e("x", "y", RelationType::Calls),
            e("y", "z", RelationType::Calls),
            e("z", "w", RelationType::Calls),
            e("y", "q", RelationType::Calls),
        ];
        let host = IstGraph::build(h_nodes, h_edges);

        let matches = vf2_subgraph_match(&query, &host, 10);
        // We expect at least one match (x → y → z) and possibly (y → z → w).
        assert!(!matches.is_empty(), "VF2 should find chain subgraph");
        let chain_match = matches.iter().any(|m| {
            m.get("a").map(String::as_str) == Some("x")
                && m.get("b").map(String::as_str) == Some("y")
                && m.get("c").map(String::as_str) == Some("z")
        });
        assert!(chain_match, "expected (a,b,c) → (x,y,z) mapping in matches");
    }

    #[test]
    fn vf2_returns_empty_when_query_larger_than_host() {
        let q = IstGraph::build(vec![n("a"), n("b"), n("c")], vec![]);
        let h = IstGraph::build(vec![n("x"), n("y")], vec![]);
        assert!(vf2_subgraph_match(&q, &h, 5).is_empty());
    }
}
