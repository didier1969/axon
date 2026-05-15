// REQ-AXO-91485 — IstGraph CSR snapshot.
//
// Compressed Sparse Row representation of the IST containment + call graph
// for a single project. Forward + reverse adjacency are built together so
// in-degree and out-degree probes are both O(1) and neighbor traversals are
// O(deg). NodePack carries one byte per categorical attribute (kind / project
// / flags) to keep cache lines warm during BFS-style traversals.

use std::collections::HashMap;

/// IST relation_type domain (mirrors public.edge.relation_type post-AGE
/// retirement, before REQ-AXO-91505 broadens it). Stored as u8 in the CSR
/// edge arrays for compactness.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RelationType {
    Contains = 0,
    Calls = 1,
    CallsNif = 2,
    Other = 255,
}

impl RelationType {
    pub fn from_db(s: &str) -> Self {
        match s {
            "CONTAINS" => Self::Contains,
            "CALLS" => Self::Calls,
            "CALLS_NIF" => Self::CallsNif,
            _ => Self::Other,
        }
    }

    pub fn as_db(self) -> &'static str {
        match self {
            Self::Contains => "CONTAINS",
            Self::Calls => "CALLS",
            Self::CallsNif => "CALLS_NIF",
            Self::Other => "OTHER",
        }
    }
}

/// CPT-AXO-90003 packed node kind table (subset surfaced today ; the full
/// 19-variant set listed in CPT-AXO-90003 will grow as parsers emit new
/// kinds — Other absorbs the unknown variants without losing the node).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NodeKind {
    File = 0,
    Function = 1,
    Method = 2,
    Class = 3,
    Struct = 4,
    Module = 5,
    Trait = 6,
    Enum = 7,
    Field = 8,
    Section = 9,
    Element = 10,
    ConfigKey = 11,
    Other = 255,
}

impl NodeKind {
    pub fn from_db(s: &str) -> Self {
        match s {
            "file" => Self::File,
            "function" => Self::Function,
            "method" => Self::Method,
            "class" => Self::Class,
            "struct" => Self::Struct,
            "module" => Self::Module,
            "trait" => Self::Trait,
            "enum" => Self::Enum,
            "field" => Self::Field,
            "section" => Self::Section,
            "element" => Self::Element,
            "config_key" => Self::ConfigKey,
            _ => Self::Other,
        }
    }
}

/// Bitfield matching public.symbol bool columns (tested / is_public / is_nif
/// / is_unsafe). Stored as a single u8 ; 4 bits free for future flags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeFlags(pub u8);

impl NodeFlags {
    pub const TESTED: u8 = 1 << 0;
    pub const PUBLIC: u8 = 1 << 1;
    pub const NIF: u8 = 1 << 2;
    pub const UNSAFE: u8 = 1 << 3;

    pub fn new(tested: bool, public: bool, nif: bool, unsafe_: bool) -> Self {
        let mut bits: u8 = 0;
        if tested {
            bits |= Self::TESTED;
        }
        if public {
            bits |= Self::PUBLIC;
        }
        if nif {
            bits |= Self::NIF;
        }
        if unsafe_ {
            bits |= Self::UNSAFE;
        }
        Self(bits)
    }

    pub fn tested(self) -> bool {
        self.0 & Self::TESTED != 0
    }
    pub fn public(self) -> bool {
        self.0 & Self::PUBLIC != 0
    }
    pub fn nif(self) -> bool {
        self.0 & Self::NIF != 0
    }
    pub fn unsafe_(self) -> bool {
        self.0 & Self::UNSAFE != 0
    }
}

/// Triple consumed by [`IstGraph::build`]. Source/target ids are full canonical
/// IST identifiers (`PRJ::path::sym`) so the graph stays addressable by the
/// same keys the SQL surfaces use.
#[derive(Clone, Debug)]
pub struct EdgeTriple {
    pub source: String,
    pub target: String,
    pub rel: RelationType,
}

/// Attribute record paired with the id when building the graph. `project`
/// stores the project_code as a u8 index into [`IstGraph::project_codes`]
/// (resolved at build time so the snapshot owns the small string set).
#[derive(Clone, Debug)]
pub struct NodeRecord {
    pub id: String,
    pub project_code: String,
    pub kind: NodeKind,
    pub flags: NodeFlags,
}

/// In-memory CSR snapshot of one or more projects' IST. Build once via
/// [`IstGraph::build`], then traverse via [`IstGraph::forward_neighbors`] /
/// [`IstGraph::reverse_neighbors`]. The CSR arrays are immutable post-build ;
/// the snapshot is swapped atomically via [`crate::ist_snapshot::IstSnapshotCache`]
/// when a fresh load lands.
pub struct IstGraph {
    ids: Vec<String>,
    id_to_idx: HashMap<String, u32>,
    project_indices: Vec<u8>,
    project_codes: Vec<String>,
    kinds: Vec<u8>,
    flags: Vec<u8>,
    fwd_offsets: Vec<u32>,
    fwd_targets: Vec<u32>,
    fwd_rel: Vec<u8>,
    rev_offsets: Vec<u32>,
    rev_sources: Vec<u32>,
    rev_rel: Vec<u8>,
}

impl IstGraph {
    /// Number of nodes resolved (sum of declared records + edge endpoints that
    /// did not appear as records).
    pub fn node_count(&self) -> usize {
        self.ids.len()
    }

    /// Total directed edges in the snapshot (one entry per CSR forward slot).
    pub fn edge_count(&self) -> usize {
        self.fwd_targets.len()
    }

    /// Resolve a canonical id to its CSR index, if present.
    pub fn index_of(&self, id: &str) -> Option<u32> {
        self.id_to_idx.get(id).copied()
    }

    /// Return the canonical id of a node, panicking on out-of-range indices
    /// (callers must derive indices via [`index_of`] or by iterating).
    pub fn id_of(&self, idx: u32) -> &str {
        &self.ids[idx as usize]
    }

    /// `(kind, project_code, flags)` for a node.
    pub fn node_meta(&self, idx: u32) -> (u8, &str, NodeFlags) {
        let i = idx as usize;
        let proj_idx = self.project_indices[i] as usize;
        (
            self.kinds[i],
            self.project_codes[proj_idx].as_str(),
            NodeFlags(self.flags[i]),
        )
    }

    /// Forward neighbors of `idx` as `(target_idx, relation_type)` pairs.
    /// O(out-degree) ; no allocation.
    pub fn forward_neighbors(&self, idx: u32) -> impl Iterator<Item = (u32, RelationType)> + '_ {
        let start = self.fwd_offsets[idx as usize] as usize;
        let end = self.fwd_offsets[idx as usize + 1] as usize;
        (start..end).map(move |slot| {
            (
                self.fwd_targets[slot],
                relation_from_u8(self.fwd_rel[slot]),
            )
        })
    }

    /// Reverse neighbors of `idx` (in-edges) as `(source_idx, relation_type)`.
    pub fn reverse_neighbors(&self, idx: u32) -> impl Iterator<Item = (u32, RelationType)> + '_ {
        let start = self.rev_offsets[idx as usize] as usize;
        let end = self.rev_offsets[idx as usize + 1] as usize;
        (start..end).map(move |slot| {
            (
                self.rev_sources[slot],
                relation_from_u8(self.rev_rel[slot]),
            )
        })
    }

    /// Build a CSR snapshot from `nodes` + `edges`. Edge endpoints not present
    /// in `nodes` are auto-registered with `NodeKind::Other` and inherit the
    /// project code of the edge endpoint that introduced them, falling back
    /// to `""` when none is supplied. Stable ordering : nodes are indexed in
    /// the order they are first observed (declared records first, then
    /// edge-implied endpoints).
    pub fn build(nodes: Vec<NodeRecord>, edges: Vec<EdgeTriple>) -> Self {
        let mut ids: Vec<String> = Vec::with_capacity(nodes.len());
        let mut id_to_idx: HashMap<String, u32> = HashMap::with_capacity(nodes.len());
        let mut kinds: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut flags: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut project_indices: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut project_codes: Vec<String> = Vec::new();
        let mut project_to_idx: HashMap<String, u8> = HashMap::new();

        let intern_project = |code: String,
                                  codes: &mut Vec<String>,
                                  map: &mut HashMap<String, u8>|
         -> u8 {
            if let Some(&idx) = map.get(&code) {
                return idx;
            }
            let idx = u8::try_from(codes.len()).unwrap_or(u8::MAX);
            map.insert(code.clone(), idx);
            codes.push(code);
            idx
        };

        for record in nodes {
            let proj_idx = intern_project(
                record.project_code.clone(),
                &mut project_codes,
                &mut project_to_idx,
            );
            let idx = u32::try_from(ids.len()).expect("ist_snapshot exceeds u32 capacity");
            id_to_idx.insert(record.id.clone(), idx);
            ids.push(record.id);
            kinds.push(record.kind as u8);
            flags.push(record.flags.0);
            project_indices.push(proj_idx);
        }

        let mut sources: Vec<u32> = Vec::with_capacity(edges.len());
        let mut targets: Vec<u32> = Vec::with_capacity(edges.len());
        let mut rels: Vec<u8> = Vec::with_capacity(edges.len());

        for edge in edges {
            let src_idx = match id_to_idx.get(&edge.source) {
                Some(&i) => i,
                None => {
                    let idx = u32::try_from(ids.len()).expect("ist_snapshot exceeds u32 capacity");
                    id_to_idx.insert(edge.source.clone(), idx);
                    ids.push(edge.source);
                    kinds.push(NodeKind::Other as u8);
                    flags.push(0);
                    project_indices.push(intern_project(
                        String::new(),
                        &mut project_codes,
                        &mut project_to_idx,
                    ));
                    idx
                }
            };
            let tgt_idx = match id_to_idx.get(&edge.target) {
                Some(&i) => i,
                None => {
                    let idx = u32::try_from(ids.len()).expect("ist_snapshot exceeds u32 capacity");
                    id_to_idx.insert(edge.target.clone(), idx);
                    ids.push(edge.target);
                    kinds.push(NodeKind::Other as u8);
                    flags.push(0);
                    project_indices.push(intern_project(
                        String::new(),
                        &mut project_codes,
                        &mut project_to_idx,
                    ));
                    idx
                }
            };
            sources.push(src_idx);
            targets.push(tgt_idx);
            rels.push(edge.rel as u8);
        }

        let node_count = ids.len();
        let (fwd_offsets, fwd_targets, fwd_rel) =
            build_csr(node_count, &sources, &targets, &rels);
        let (rev_offsets, rev_sources, rev_rel) =
            build_csr(node_count, &targets, &sources, &rels);

        Self {
            ids,
            id_to_idx,
            project_indices,
            project_codes,
            kinds,
            flags,
            fwd_offsets,
            fwd_targets,
            fwd_rel,
            rev_offsets,
            rev_sources,
            rev_rel,
        }
    }

    /// Approximate resident memory (bytes) — sum of CSR + arena + index
    /// overhead. Used by the bench binary and ist_snapshot diagnostics.
    pub fn approximate_bytes(&self) -> usize {
        let ids_bytes: usize = self.ids.iter().map(String::len).sum();
        let ids_overhead = self.ids.capacity() * std::mem::size_of::<String>();
        let id_to_idx_overhead = self.id_to_idx.capacity()
            * (std::mem::size_of::<String>() + std::mem::size_of::<u32>() + 16);
        let csr_bytes = self.fwd_offsets.len() * std::mem::size_of::<u32>() * 2
            + self.fwd_targets.len() * std::mem::size_of::<u32>()
            + self.fwd_rel.len()
            + self.rev_offsets.len() * std::mem::size_of::<u32>() * 2
            + self.rev_sources.len() * std::mem::size_of::<u32>()
            + self.rev_rel.len();
        let attr_bytes = self.kinds.len() + self.flags.len() + self.project_indices.len();
        let projects_bytes: usize = self.project_codes.iter().map(String::len).sum();
        ids_bytes + ids_overhead + id_to_idx_overhead + csr_bytes + attr_bytes + projects_bytes
    }
}

fn build_csr(
    node_count: usize,
    sources: &[u32],
    targets: &[u32],
    rels: &[u8],
) -> (Vec<u32>, Vec<u32>, Vec<u8>) {
    let mut offsets: Vec<u32> = vec![0; node_count + 1];
    for &src in sources {
        offsets[src as usize + 1] += 1;
    }
    for i in 1..=node_count {
        offsets[i] += offsets[i - 1];
    }
    let mut targets_out: Vec<u32> = vec![0; sources.len()];
    let mut rel_out: Vec<u8> = vec![0; sources.len()];
    let mut cursor: Vec<u32> = offsets[..node_count].to_vec();
    for i in 0..sources.len() {
        let src = sources[i] as usize;
        let slot = cursor[src] as usize;
        targets_out[slot] = targets[i];
        rel_out[slot] = rels[i];
        cursor[src] += 1;
    }
    (offsets, targets_out, rel_out)
}

fn relation_from_u8(value: u8) -> RelationType {
    match value {
        0 => RelationType::Contains,
        1 => RelationType::Calls,
        2 => RelationType::CallsNif,
        _ => RelationType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, project: &str, kind: NodeKind) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            project_code: project.to_string(),
            kind,
            flags: NodeFlags::default(),
        }
    }

    fn edge(src: &str, tgt: &str, rel: RelationType) -> EdgeTriple {
        EdgeTriple {
            source: src.to_string(),
            target: tgt.to_string(),
            rel,
        }
    }

    #[test]
    fn build_empty_graph_has_zero_nodes_zero_edges() {
        let g = IstGraph::build(vec![], vec![]);
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
    }

    #[test]
    fn build_indexes_declared_nodes_first_then_edge_implied() {
        let nodes = vec![node("AXO::a", "AXO", NodeKind::File)];
        let edges = vec![edge("AXO::a", "AXO::b", RelationType::Contains)];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert_eq!(g.index_of("AXO::a"), Some(0));
        assert_eq!(g.index_of("AXO::b"), Some(1));
        assert_eq!(g.id_of(0), "AXO::a");
        let (kind_a, _, _) = g.node_meta(0);
        assert_eq!(kind_a, NodeKind::File as u8);
        let (kind_b, _, _) = g.node_meta(1);
        assert_eq!(kind_b, NodeKind::Other as u8);
    }

    #[test]
    fn forward_and_reverse_neighbors_round_trip() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("a", "c", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let a = g.index_of("a").unwrap();
        let b = g.index_of("b").unwrap();
        let c = g.index_of("c").unwrap();
        let fwd_a: Vec<_> = g.forward_neighbors(a).map(|(t, _)| t).collect();
        assert_eq!(fwd_a.len(), 2);
        assert!(fwd_a.contains(&b));
        assert!(fwd_a.contains(&c));
        let rev_c: Vec<_> = g.reverse_neighbors(c).map(|(s, _)| s).collect();
        assert_eq!(rev_c.len(), 2);
        assert!(rev_c.contains(&a));
        assert!(rev_c.contains(&b));
        let rev_a: Vec<_> = g.reverse_neighbors(a).map(|(s, _)| s).collect();
        assert!(rev_a.is_empty());
    }

    #[test]
    fn self_loop_recorded_in_both_directions() {
        let nodes = vec![node("a", "AXO", NodeKind::Function)];
        let edges = vec![edge("a", "a", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let a = g.index_of("a").unwrap();
        assert_eq!(g.forward_neighbors(a).count(), 1);
        assert_eq!(g.reverse_neighbors(a).count(), 1);
    }

    #[test]
    fn relation_types_preserved_in_forward_csr() {
        let nodes = vec![
            node("file", "AXO", NodeKind::File),
            node("file::fn", "AXO", NodeKind::Function),
            node("file::fn::callee", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("file", "file::fn", RelationType::Contains),
            edge("file::fn", "file::fn::callee", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let file = g.index_of("file").unwrap();
        let r#fn = g.index_of("file::fn").unwrap();
        let rels_from_file: Vec<_> = g.forward_neighbors(file).map(|(_, r)| r).collect();
        assert_eq!(rels_from_file, vec![RelationType::Contains]);
        let rels_from_fn: Vec<_> = g.forward_neighbors(r#fn).map(|(_, r)| r).collect();
        assert_eq!(rels_from_fn, vec![RelationType::Calls]);
    }

    #[test]
    fn project_codes_interned_once_per_distinct_value() {
        let nodes = vec![
            node("a1", "AXO", NodeKind::Function),
            node("a2", "AXO", NodeKind::Function),
            node("o1", "OPT", NodeKind::Function),
        ];
        let g = IstGraph::build(nodes, vec![]);
        let (_, proj_a1, _) = g.node_meta(g.index_of("a1").unwrap());
        let (_, proj_a2, _) = g.node_meta(g.index_of("a2").unwrap());
        let (_, proj_o1, _) = g.node_meta(g.index_of("o1").unwrap());
        assert_eq!(proj_a1, "AXO");
        assert_eq!(proj_a2, "AXO");
        assert_eq!(proj_o1, "OPT");
        assert_eq!(g.project_codes.len(), 2);
    }

    #[test]
    fn relation_type_from_db_round_trips_known_values() {
        for s in ["CONTAINS", "CALLS", "CALLS_NIF"] {
            assert_eq!(RelationType::from_db(s).as_db(), s);
        }
        assert_eq!(RelationType::from_db("UNKNOWN"), RelationType::Other);
    }

    #[test]
    fn node_flags_round_trip_known_combinations() {
        let f = NodeFlags::new(true, false, true, false);
        assert!(f.tested());
        assert!(!f.public());
        assert!(f.nif());
        assert!(!f.unsafe_());
    }
}
