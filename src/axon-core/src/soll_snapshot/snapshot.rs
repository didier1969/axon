//! Data structures mirrored from the SOLL graph (REQ-AXO-322).
//!
//! The snapshot ships a `petgraph::Graph` alongside the raw rows so SOLL
//! reads that need real graph algorithms (cycle detection, descendant
//! counting, neighbor iteration) run natively in RAM without re-deriving
//! adjacency on every call.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::graph::{Graph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

#[derive(Clone, Debug)]
pub struct SnapshotNode {
    pub id: String,
    pub entity_type: String,
    pub title: String,
    pub status: String,
    pub metadata_raw: String,
}

#[derive(Clone, Debug)]
pub struct SnapshotEdge {
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
}

#[derive(Clone, Debug)]
pub struct SnapshotTraceability {
    pub id: String,
    pub soll_entity_type: String,
    pub soll_entity_id: String,
    pub artifact_type: String,
    pub artifact_ref: String,
    pub artifact_status: String,
}

#[derive(Clone, Debug)]
pub struct SollSnapshot {
    pub project_code: String,
    pub loaded_at_ms: i64,
    pub generation: u64,

    pub nodes: HashMap<String, SnapshotNode>,
    pub edges: Vec<SnapshotEdge>,
    pub traceability: Vec<SnapshotTraceability>,

    nodes_by_type_prefix: HashMap<String, Vec<String>>,
    traceability_count_by_entity: HashMap<String, usize>,
    // "type::id" (lowercased type) -> indices into self.traceability
    traceability_by_entity: HashMap<String, Vec<usize>>,

    // Native graph view. Node weight = canonical SOLL id; edge weight =
    // relation_type. Built once at load time; reads use the petgraph
    // algorithms (tarjan_scc, BFS, neighbor iteration) directly.
    graph: Graph<String, String>,
    node_index: HashMap<String, NodeIndex>,
}

impl SollSnapshot {
    pub fn empty(project_code: impl Into<String>, generation: u64) -> Self {
        Self {
            project_code: project_code.into(),
            loaded_at_ms: now_unix_ms(),
            generation,
            nodes: HashMap::new(),
            edges: Vec::new(),
            traceability: Vec::new(),
            nodes_by_type_prefix: HashMap::new(),
            traceability_count_by_entity: HashMap::new(),
            traceability_by_entity: HashMap::new(),
            graph: Graph::new(),
            node_index: HashMap::new(),
        }
    }

    pub fn build(
        project_code: impl Into<String>,
        generation: u64,
        nodes: HashMap<String, SnapshotNode>,
        edges: Vec<SnapshotEdge>,
        traceability: Vec<SnapshotTraceability>,
    ) -> Self {
        let mut nodes_by_type_prefix: HashMap<String, Vec<String>> = HashMap::new();
        for node in nodes.values() {
            nodes_by_type_prefix
                .entry(node.entity_type.clone())
                .or_default()
                .push(node.id.clone());
        }
        for ids in nodes_by_type_prefix.values_mut() {
            ids.sort();
        }

        let mut traceability_count_by_entity: HashMap<String, usize> = HashMap::new();
        let mut traceability_by_entity: HashMap<String, Vec<usize>> = HashMap::new();
        for (idx, t) in traceability.iter().enumerate() {
            let key = format!(
                "{}::{}",
                t.soll_entity_type.to_ascii_lowercase(),
                t.soll_entity_id
            );
            *traceability_count_by_entity.entry(key.clone()).or_insert(0) += 1;
            traceability_by_entity.entry(key).or_default().push(idx);
        }

        // Build the petgraph view. Every node from `nodes` becomes a
        // graph node (so isolated nodes are still discoverable for the
        // orphan check); every edge becomes a directed graph edge.
        // Endpoints that are absent from `nodes` are still added so
        // cross-project edges don't get dropped silently.
        let mut graph: Graph<String, String> = Graph::with_capacity(nodes.len(), edges.len());
        let mut node_index: HashMap<String, NodeIndex> = HashMap::with_capacity(nodes.len());
        for id in nodes.keys() {
            let idx = graph.add_node(id.clone());
            node_index.insert(id.clone(), idx);
        }
        for e in &edges {
            let src_idx = *node_index
                .entry(e.source_id.clone())
                .or_insert_with(|| graph.add_node(e.source_id.clone()));
            let tgt_idx = *node_index
                .entry(e.target_id.clone())
                .or_insert_with(|| graph.add_node(e.target_id.clone()));
            graph.add_edge(src_idx, tgt_idx, e.relation_type.clone());
        }

        Self {
            project_code: project_code.into(),
            loaded_at_ms: now_unix_ms(),
            generation,
            nodes,
            edges,
            traceability,
            nodes_by_type_prefix,
            traceability_count_by_entity,
            traceability_by_entity,
            graph,
            node_index,
        }
    }

    /// Borrow the underlying petgraph graph for direct algorithm access.
    pub fn graph(&self) -> &Graph<String, String> {
        &self.graph
    }

    /// Resolve a SOLL id to its `NodeIndex` in the petgraph view.
    pub fn node_index(&self, id: &str) -> Option<NodeIndex> {
        self.node_index.get(id).copied()
    }

    /// All edges where this node is the SOURCE.
    pub fn outgoing_edges<'a>(
        &'a self,
        source_id: &str,
    ) -> Box<dyn Iterator<Item = (&'a str, &'a str)> + 'a> {
        let Some(idx) = self.node_index.get(source_id).copied() else {
            return Box::new(std::iter::empty());
        };
        Box::new(
            self.graph
                .edges_directed(idx, Direction::Outgoing)
                .map(move |e| (self.graph[e.target()].as_str(), e.weight().as_str())),
        )
    }

    /// All edges where this node is the TARGET.
    pub fn incoming_edges<'a>(
        &'a self,
        target_id: &str,
    ) -> Box<dyn Iterator<Item = (&'a str, &'a str)> + 'a> {
        let Some(idx) = self.node_index.get(target_id).copied() else {
            return Box::new(std::iter::empty());
        };
        Box::new(
            self.graph
                .edges_directed(idx, Direction::Incoming)
                .map(move |e| (self.graph[e.source()].as_str(), e.weight().as_str())),
        )
    }

    /// True if the node has at least one edge as source OR target.
    pub fn has_any_edge(&self, node_id: &str) -> bool {
        let Some(idx) = self.node_index.get(node_id).copied() else {
            return false;
        };
        self.graph
            .edges_directed(idx, Direction::Outgoing)
            .next()
            .is_some()
            || self
                .graph
                .edges_directed(idx, Direction::Incoming)
                .next()
                .is_some()
    }

    /// Count incoming edges with a given relation_type, optionally restricted
    /// to sources matching a prefix (e.g. "VAL-AXO-" for VERIFIES from VAL).
    pub fn count_incoming_edges_with(
        &self,
        target_id: &str,
        relation_type: &str,
        source_prefix: Option<&str>,
    ) -> usize {
        self.incoming_edges(target_id)
            .filter(|(_src, rel)| *rel == relation_type)
            .filter(|(src, _)| match source_prefix {
                Some(p) => src.starts_with(p),
                None => true,
            })
            .count()
    }

    /// Strongly-connected components containing more than one node.
    /// Returned as sets of SOLL ids. Self-loops on a single node are
    /// also reported.
    ///
    /// Uses [`petgraph::algo::tarjan_scc`] — linear in nodes+edges.
    pub fn cycle_sets(&self) -> Vec<HashSet<String>> {
        let sccs = petgraph::algo::tarjan_scc(&self.graph);
        let mut out = Vec::new();
        for component in sccs {
            if component.len() > 1 {
                out.push(
                    component
                        .into_iter()
                        .map(|nidx| self.graph[nidx].clone())
                        .collect(),
                );
            } else if let Some(&nidx) = component.first() {
                // Single-node SCC counts only if it has a self-loop.
                let has_self_loop = self
                    .graph
                    .edges_directed(nidx, Direction::Outgoing)
                    .any(|e| e.target() == nidx);
                if has_self_loop {
                    let mut set = HashSet::new();
                    set.insert(self.graph[nidx].clone());
                    out.push(set);
                }
            }
        }
        out
    }

    /// BFS forward from `source_id` over the subgraph induced by
    /// `allowed`. Returns the count of *open* descendants reachable.
    /// The seed node itself is not counted.
    pub fn count_descendants_in(&self, source_id: &str, allowed: &HashSet<String>) -> usize {
        let Some(start) = self.node_index.get(source_id).copied() else {
            return 0;
        };
        let mut visited: HashSet<NodeIndex> = HashSet::new();
        let mut queue: VecDeque<NodeIndex> = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);
        let mut count = 0usize;
        while let Some(node) = queue.pop_front() {
            for e in self.graph.edges_directed(node, Direction::Outgoing) {
                let nxt = e.target();
                if visited.contains(&nxt) {
                    continue;
                }
                let nxt_id = &self.graph[nxt];
                if !allowed.contains(nxt_id) {
                    continue;
                }
                visited.insert(nxt);
                queue.push_back(nxt);
                count += 1;
            }
        }
        count
    }

    /// Traceability rows for a given (lowercase entity_type, entity_id).
    pub fn traceability_rows_for<'a>(
        &'a self,
        lower_entity_type: &str,
        entity_id: &str,
    ) -> impl Iterator<Item = &'a SnapshotTraceability> + 'a {
        let key = format!("{}::{}", lower_entity_type, entity_id);
        self.traceability_by_entity
            .get(&key)
            .map(|idxs| idxs.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(move |&i| &self.traceability[i])
    }

    /// Return all node ids with the given canonical type (e.g. "Requirement").
    pub fn node_ids_of_type(&self, entity_type: &str) -> &[String] {
        self.nodes_by_type_prefix
            .get(entity_type)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Traceability rows attached to a given (lowercase entity_type, entity_id).
    pub fn traceability_count_for(&self, lower_entity_type: &str, entity_id: &str) -> usize {
        let key = format!("{}::{}", lower_entity_type, entity_id);
        self.traceability_count_by_entity
            .get(&key)
            .copied()
            .unwrap_or(0)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_node(id: &str, ty: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: ty.to_string(),
            title: format!("title-{}", id),
            status: "current".to_string(),
            metadata_raw: "{}".to_string(),
        }
    }

    fn mk_trace(entity_type: &str, entity_id: &str, idx: usize) -> SnapshotTraceability {
        SnapshotTraceability {
            id: format!("T-{}-{}", entity_id, idx),
            soll_entity_type: entity_type.to_string(),
            soll_entity_id: entity_id.to_string(),
            artifact_type: "file".to_string(),
            artifact_ref: format!("/tmp/x-{}", idx),
            artifact_status: "ok".to_string(),
        }
    }

    #[test]
    fn build_indexes_nodes_by_type() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "REQ-AXO-001".to_string(),
            mk_node("REQ-AXO-001", "Requirement"),
        );
        nodes.insert(
            "REQ-AXO-002".to_string(),
            mk_node("REQ-AXO-002", "Requirement"),
        );
        nodes.insert(
            "DEC-AXO-001".to_string(),
            mk_node("DEC-AXO-001", "Decision"),
        );
        nodes.insert(
            "MIL-AXO-001".to_string(),
            mk_node("MIL-AXO-001", "Milestone"),
        );

        let snap = SollSnapshot::build("AXO", 1, nodes, vec![], vec![]);
        assert_eq!(snap.node_ids_of_type("Requirement").len(), 2);
        assert_eq!(snap.node_ids_of_type("Decision").len(), 1);
        assert_eq!(snap.node_ids_of_type("Milestone").len(), 1);
        assert_eq!(snap.node_ids_of_type("Vision").len(), 0);
        let reqs = snap.node_ids_of_type("Requirement");
        assert_eq!(reqs[0], "REQ-AXO-001");
        assert_eq!(reqs[1], "REQ-AXO-002");
    }

    #[test]
    fn build_pre_aggregates_traceability_counts() {
        let nodes = HashMap::new();
        let trace = vec![
            mk_trace("Decision", "DEC-AXO-001", 1),
            mk_trace("Decision", "DEC-AXO-001", 2),
            mk_trace("decision", "DEC-AXO-002", 3),
            mk_trace("Milestone", "MIL-AXO-001", 1),
            mk_trace("Requirement", "REQ-AXO-001", 1),
        ];
        let snap = SollSnapshot::build("AXO", 1, nodes, vec![], trace);
        assert_eq!(snap.traceability_count_for("decision", "DEC-AXO-001"), 2);
        assert_eq!(snap.traceability_count_for("decision", "DEC-AXO-002"), 1);
        assert_eq!(snap.traceability_count_for("milestone", "MIL-AXO-001"), 1);
        assert_eq!(snap.traceability_count_for("requirement", "REQ-AXO-001"), 1);
        assert_eq!(snap.traceability_count_for("decision", "DEC-AXO-999"), 0);
    }

    #[test]
    fn empty_snapshot_is_self_consistent() {
        let snap = SollSnapshot::empty("AXO", 0);
        assert_eq!(snap.node_count(), 0);
        assert_eq!(snap.edge_count(), 0);
        assert_eq!(snap.node_ids_of_type("Requirement").len(), 0);
        assert_eq!(snap.traceability_count_for("decision", "DEC-AXO-001"), 0);
        assert_eq!(snap.project_code, "AXO");
    }
}
