// REQ-AXO-902678 — Horizontal CSR Sharding and Federated In-RAM Sub-graphs.
//
// Partitioning of the monolithic IstGraph CSR matrix across ShardId domains,
// enabling horizontal scaling to tens of millions of symbols, cross-shard
// traversal without unified contiguous allocation, and partial re-indexing
// of isolated shards.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use anyhow::Result;

use crate::ist_snapshot::snapshot::{EdgeTriple, IstGraph, NodeRecord, RelationType};

pub type ShardId = u16;

/// Deterministic routing strategy for symbol sharding.
#[derive(Clone, Debug)]
pub enum ShardingStrategy {
    /// Legacy/Single-shard: all symbols map to Shard 0.
    Single,
    /// Prefix-based routing (e.g. subsystem / namespace partitioning).
    PrefixRule(Vec<(String, ShardId)>),
    /// Hash-based modulo routing across N shards.
    HashModulo(u16),
}

impl ShardingStrategy {
    pub fn assign_shard(&self, symbol_id: &str) -> ShardId {
        match self {
            Self::Single => 0,
            Self::PrefixRule(rules) => {
                for (prefix, shard) in rules {
                    if symbol_id.starts_with(prefix) {
                        return *shard;
                    }
                }
                0
            }
            Self::HashModulo(n) => {
                if *n <= 1 {
                    return 0;
                }
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                symbol_id.hash(&mut hasher);
                (hasher.finish() % (*n as u64)) as ShardId
            }
        }
    }
}

/// An edge crossing shard boundaries (source on `source_shard`, target on `target_shard`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossShardEdge {
    pub source_symbol: String,
    pub target_symbol: String,
    pub source_shard: ShardId,
    pub target_shard: ShardId,
    pub rel: RelationType,
}

/// A partition of the CSR graph holding local nodes and edges.
pub struct CsrShard {
    id: ShardId,
    graph: Arc<IstGraph>,
    local_symbol_set: HashSet<String>,
    outgoing_cross_edges: Vec<CrossShardEdge>,
}

impl CsrShard {
    pub fn id(&self) -> ShardId {
        self.id
    }

    pub fn local_node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn graph(&self) -> &Arc<IstGraph> {
        &self.graph
    }

    pub fn contains_symbol(&self, symbol: &str) -> bool {
        self.local_symbol_set.contains(symbol)
    }

    pub fn outgoing_cross_edges(&self) -> &[CrossShardEdge] {
        &self.outgoing_cross_edges
    }
}

/// A federated sharded graph composed of multiple `CsrShard` instances.
pub struct ShardedIstGraph {
    project_code: String,
    shards: HashMap<ShardId, Arc<CsrShard>>,
    symbol_to_shard: HashMap<String, ShardId>,
    cross_shard_edges: Vec<CrossShardEdge>,
    strategy: ShardingStrategy,
}

impl ShardedIstGraph {
    pub fn builder(project_code: &str, strategy: ShardingStrategy) -> ShardedIstGraphBuilder {
        ShardedIstGraphBuilder {
            project_code: project_code.to_string(),
            strategy,
        }
    }

    pub fn project_code(&self) -> &str {
        &self.project_code
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    pub fn get_shard(&self, id: ShardId) -> Option<&Arc<CsrShard>> {
        self.shards.get(&id)
    }

    pub fn shard_for_symbol(&self, symbol: &str) -> Option<ShardId> {
        self.symbol_to_shard.get(symbol).copied()
    }

    pub fn cross_shard_edge_count(&self) -> usize {
        self.cross_shard_edges.len()
    }

    pub fn cross_shard_edges(&self) -> &[CrossShardEdge] {
        &self.cross_shard_edges
    }

    pub fn strategy(&self) -> &ShardingStrategy {
        &self.strategy
    }

    pub fn total_node_count(&self) -> usize {
        self.symbol_to_shard.len()
    }

    pub fn total_edge_count(&self) -> usize {
        let local_edges: usize = self.shards.values().map(|s| s.graph.edge_count()).sum();
        local_edges + self.cross_shard_edges.len()
    }

    /// Return a copy of this graph with the given shard invalidated/removed.
    pub fn invalidate_shard(&self, shard_id: ShardId) -> Self {
        let mut new_shards = self.shards.clone();
        new_shards.remove(&shard_id);
        let mut new_symbol_to_shard = self.symbol_to_shard.clone();
        new_symbol_to_shard.retain(|_, s_id| *s_id != shard_id);
        let mut new_cross_edges = self.cross_shard_edges.clone();
        new_cross_edges.retain(|e| e.source_shard != shard_id && e.target_shard != shard_id);

        Self {
            project_code: self.project_code.clone(),
            shards: new_shards,
            symbol_to_shard: new_symbol_to_shard,
            cross_shard_edges: new_cross_edges,
            strategy: self.strategy.clone(),
        }
    }
}

pub struct ShardedIstGraphBuilder {
    project_code: String,
    strategy: ShardingStrategy,
}

impl ShardedIstGraphBuilder {
    pub fn new(project_code: String, strategy: ShardingStrategy) -> Self {
        Self {
            project_code,
            strategy,
        }
    }

    pub fn build(
        &mut self,
        nodes: Vec<NodeRecord>,
        edges: Vec<EdgeTriple>,
    ) -> Result<ShardedIstGraph> {
        let mut symbol_to_shard: HashMap<String, ShardId> = HashMap::with_capacity(nodes.len());
        let mut shard_nodes: HashMap<ShardId, Vec<NodeRecord>> = HashMap::new();

        for node in nodes {
            let shard_id = self.strategy.assign_shard(&node.id);
            symbol_to_shard.insert(node.id.clone(), shard_id);
            shard_nodes.entry(shard_id).or_default().push(node);
        }

        let mut shard_local_edges: HashMap<ShardId, Vec<EdgeTriple>> = HashMap::new();
        let mut cross_shard_edges: Vec<CrossShardEdge> = Vec::new();
        let mut outgoing_per_shard: HashMap<ShardId, Vec<CrossShardEdge>> = HashMap::new();

        for edge in edges {
            let src_shard = symbol_to_shard
                .get(&edge.source)
                .copied()
                .unwrap_or_else(|| self.strategy.assign_shard(&edge.source));
            let tgt_shard = symbol_to_shard
                .get(&edge.target)
                .copied()
                .unwrap_or_else(|| self.strategy.assign_shard(&edge.target));

            if src_shard == tgt_shard {
                shard_local_edges.entry(src_shard).or_default().push(edge);
            } else {
                let cross = CrossShardEdge {
                    source_symbol: edge.source.clone(),
                    target_symbol: edge.target.clone(),
                    source_shard: src_shard,
                    target_shard: tgt_shard,
                    rel: edge.rel,
                };
                cross_shard_edges.push(cross.clone());
                outgoing_per_shard.entry(src_shard).or_default().push(cross);
            }
        }

        let mut shards: HashMap<ShardId, Arc<CsrShard>> = HashMap::new();

        for (shard_id, nodes) in shard_nodes {
            let local_edges = shard_local_edges.remove(&shard_id).unwrap_or_default();
            let mut local_symbol_set = HashSet::with_capacity(nodes.len());
            for n in &nodes {
                local_symbol_set.insert(n.id.clone());
            }

            let graph = Arc::new(IstGraph::build(nodes, local_edges));
            let outgoing = outgoing_per_shard.remove(&shard_id).unwrap_or_default();

            shards.insert(
                shard_id,
                Arc::new(CsrShard {
                    id: shard_id,
                    graph,
                    local_symbol_set,
                    outgoing_cross_edges: outgoing,
                }),
            );
        }

        Ok(ShardedIstGraph {
            project_code: self.project_code.clone(),
            shards,
            symbol_to_shard,
            cross_shard_edges,
            strategy: self.strategy.clone(),
        })
    }
}

/// Unified traversal view over a `ShardedIstGraph`.
pub struct ShardedIstView<'a> {
    graph: &'a ShardedIstGraph,
    reverse_cross_edges: HashMap<String, Vec<(String, RelationType)>>,
}

impl<'a> ShardedIstView<'a> {
    pub fn new(graph: &'a ShardedIstGraph) -> Self {
        let mut reverse_cross_edges: HashMap<String, Vec<(String, RelationType)>> = HashMap::new();
        for edge in &graph.cross_shard_edges {
            reverse_cross_edges
                .entry(edge.target_symbol.clone())
                .or_default()
                .push((edge.source_symbol.clone(), edge.rel));
        }
        Self {
            graph,
            reverse_cross_edges,
        }
    }

    pub fn forward_neighbors(&self, symbol: &str) -> Vec<(String, RelationType)> {
        let mut results = Vec::new();
        if let Some(shard_id) = self.graph.shard_for_symbol(symbol) {
            if let Some(shard) = self.graph.get_shard(shard_id) {
                if let Some(idx) = shard.graph.index_of(symbol) {
                    for (target_idx, rel) in shard.graph.forward_neighbors(idx) {
                        results.push((shard.graph.id_of(target_idx).to_string(), rel));
                    }
                }
                for edge in shard.outgoing_cross_edges() {
                    if edge.source_symbol == symbol {
                        results.push((edge.target_symbol.clone(), edge.rel));
                    }
                }
            }
        }
        results
    }

    pub fn reverse_neighbors(&self, symbol: &str) -> Vec<(String, RelationType)> {
        let mut results = Vec::new();
        if let Some(shard_id) = self.graph.shard_for_symbol(symbol) {
            if let Some(shard) = self.graph.get_shard(shard_id) {
                if let Some(idx) = shard.graph.index_of(symbol) {
                    for (src_idx, rel) in shard.graph.reverse_neighbors(idx) {
                        results.push((shard.graph.id_of(src_idx).to_string(), rel));
                    }
                }
            }
        }
        if let Some(cross_callers) = self.reverse_cross_edges.get(symbol) {
            for (caller, rel) in cross_callers {
                results.push((caller.clone(), *rel));
            }
        }
        results
    }

    pub fn callers_of(&self, symbol: &str) -> Vec<String> {
        self.reverse_neighbors(symbol)
            .into_iter()
            .filter(|(_, rel)| rel.is_dependency())
            .map(|(s, _)| s)
            .collect()
    }

    pub fn find_path(&self, source: &str, target: &str) -> Option<Vec<String>> {
        if source == target {
            return Some(vec![source.to_string()]);
        }
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut parent: HashMap<String, String> = HashMap::new();

        visited.insert(source.to_string());
        queue.push_back(source.to_string());

        while let Some(curr) = queue.pop_front() {
            if curr == target {
                let mut path = Vec::new();
                let mut p = target.to_string();
                path.push(p.clone());
                while let Some(prev) = parent.get(&p) {
                    path.push(prev.clone());
                    p = prev.clone();
                }
                path.reverse();
                return Some(path);
            }

            for (next, rel) in self.forward_neighbors(&curr) {
                if rel.is_dependency() && visited.insert(next.clone()) {
                    parent.insert(next.clone(), curr.clone());
                    queue.push_back(next);
                }
            }
        }

        None
    }

    pub fn transitive_callers_of(&self, target: &str, max_depth: usize) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((target.to_string(), 0));

        while let Some((curr, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for (caller, rel) in self.reverse_neighbors(&curr) {
                if rel.is_dependency() && visited.insert(caller.clone()) {
                    queue.push_back((caller, depth + 1));
                }
            }
        }

        visited.into_iter().collect()
    }
}
