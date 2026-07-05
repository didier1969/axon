// REQ-AXO-91485 — IstGraph CSR snapshot.
//
// Compressed Sparse Row representation of the IST containment + call graph
// for a single project. Forward + reverse adjacency are built together so
// in-degree and out-degree probes are both O(1) and neighbor traversals are
// O(deg). NodePack carries one byte per categorical attribute (kind / project
// / flags) to keep cache lines warm during BFS-style traversals.

use std::collections::{HashMap, HashSet, VecDeque};

/// REQ-AXO-902028 — max implementors expanded from a single contract node in the
/// dynamic-dispatch bridge, bounding path-search fan-out at high-fan-in traits
/// (e.g. a marker trait with hundreds of impls). `pub(crate)` since REQ-AXO-902211
/// reuses the exact same cap for the analogous bridge in a forward reachability
/// walk (`algorithms::dead_clusters`) — one tuning knob, not two divergent copies.
pub(crate) const CONTRACT_BRIDGE_FANOUT_CAP: usize = 32;

/// IST relation_type domain (mirrors ist.edge.relation_type post-AGE
/// retirement, before REQ-AXO-91505 broadens it). Stored as u8 in the CSR
/// edge arrays for compactness.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RelationType {
    Contains = 0,
    Calls = 1,
    CallsNif = 2,
    Implements = 3,
    Imports = 4,
    Uses = 5,
    // REQ-AXO-901970 — Reads / Declares back the phantom-reference analytics.
    Reads = 6,
    Declares = 7,
    // REQ-AXO-902017 — a code symbol READS a data artifact (CSV lake / manifest).
    // Appended (not inserted) so existing CSR u8 encodings stay stable.
    ReadsArtifact = 8,
    // REQ-AXO-902185 — near-duplicate (semantic clone) pair, persisted out-of-band
    // by a periodic pgvector HNSW scan (never by the parser). Appended (not
    // inserted) so existing CSR u8 encodings stay stable.
    SimilarTo = 9,
    Other = 255,
}

impl RelationType {
    /// REQ-AXO-91505 — accept canonical DB strings case-insensitively.
    /// Parsers emit lowercase (`imports`, `implements`, `uses`, `calls`)
    /// while legacy code paths emit uppercase. Both must map to the
    /// canonical variant so IstGraph reads cleanly from ist.edge.
    pub fn from_db(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "CONTAINS" => Self::Contains,
            "CALLS" => Self::Calls,
            "CALLS_NIF" => Self::CallsNif,
            "IMPLEMENTS" => Self::Implements,
            "IMPORTS" => Self::Imports,
            "USES" => Self::Uses,
            "READS" => Self::Reads,
            "DECLARES" => Self::Declares,
            "READS_ARTIFACT" => Self::ReadsArtifact,
            "SIMILAR_TO" => Self::SimilarTo,
            _ => Self::Other,
        }
    }

    pub fn as_db(self) -> &'static str {
        match self {
            Self::Contains => "CONTAINS",
            Self::Calls => "CALLS",
            Self::CallsNif => "CALLS_NIF",
            Self::Implements => "IMPLEMENTS",
            Self::Imports => "IMPORTS",
            Self::Uses => "USES",
            Self::Reads => "READS",
            Self::Declares => "DECLARES",
            Self::ReadsArtifact => "READS_ARTIFACT",
            Self::SimilarTo => "SIMILAR_TO",
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
    // REQ-AXO-901970 — Interface kind backs the abstraction-detour analytic.
    Interface = 12,
    // REQ-AXO-902017 — a data artifact (CSV lake / fixture / manifest) indexed
    // as an IST node so data-centric projects answer "what data exists + who
    // reads it" structurally. Appended (not inserted) for CSR u8 stability.
    DataArtifact = 13,
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
            "interface" => Self::Interface,
            "data_artifact" => Self::DataArtifact,
            _ => Self::Other,
        }
    }

    pub fn from_u8(byte: u8) -> Self {
        match byte {
            0 => Self::File,
            1 => Self::Function,
            2 => Self::Method,
            3 => Self::Class,
            4 => Self::Struct,
            5 => Self::Module,
            6 => Self::Trait,
            7 => Self::Enum,
            8 => Self::Field,
            9 => Self::Section,
            10 => Self::Element,
            11 => Self::ConfigKey,
            12 => Self::Interface,
            13 => Self::DataArtifact,
            _ => Self::Other,
        }
    }

    /// REQ-AXO-901970 — inverse of `from_db`: the canonical lowercase kind
    /// string the `ist.symbol.kind` column stores (and that `query`/`inspect`
    /// surface). `Other` maps to `""` to match `COALESCE(s.kind, '')`.
    pub fn as_db(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Module => "module",
            Self::Trait => "trait",
            Self::Enum => "enum",
            Self::Field => "field",
            Self::Section => "section",
            Self::Element => "element",
            Self::ConfigKey => "config_key",
            Self::Interface => "interface",
            Self::DataArtifact => "data_artifact",
            Self::Other => "",
        }
    }
}

/// Bitfield matching ist.symbol bool columns (tested / is_public / is_nif
/// / is_unsafe) plus the RAM-derived `covered` bit. Stored as a single u8 ;
/// 3 bits free for future flags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeFlags(pub u8);

impl NodeFlags {
    pub const TESTED: u8 = 1 << 0;
    pub const PUBLIC: u8 = 1 << 1;
    pub const NIF: u8 = 1 << 2;
    pub const UNSAFE: u8 = 1 << 3;
    /// REQ-AXO-902187 — RAM-DERIVED (not a DB column): the node is reachable via
    /// CALLS/CALLS_NIF forward-edges from a `#[test]` node. `tested` is the
    /// syntactic parse fact ("carries #[test]") ; `covered` is the wiring fact
    /// ("exercised by a test") the SHI weighted_coverage actually needs. Set by
    /// [`IstGraph::build`]'s propagation pass, never by the parser/loader.
    pub const COVERED: u8 = 1 << 4;

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
    /// REQ-AXO-902187 — reachable from a `#[test]` via CALLS forward-edges
    /// (derived at build time). Distinct from [`tested`](Self::tested).
    pub fn covered(self) -> bool {
        self.0 & Self::COVERED != 0
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
    /// REQ-AXO-901970 — canonical display name (`ist.symbol.name`). The RAM graph
    /// stores it explicitly rather than re-deriving from the id suffix, because
    /// some symbols (TODO/FIXME comments, secret findings, macro call targets)
    /// carry a `name` that is NOT the last `::` segment of the id. Name-based
    /// analytics (technical_debt, security taint, telemetry) match against this.
    pub name: String,
    pub project_code: String,
    pub kind: NodeKind,
    pub flags: NodeFlags,
    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity computed by
    /// the parser at index time (`ist.symbol.cyclomatic_complexity`). `None` =
    /// not yet measured (pre-migration row, or a language whose counting slice
    /// hasn't landed) — the SHI classifier treats this as "not a god-object",
    /// never as complexity 0 (which would be a false "simple" reading).
    pub complexity: Option<i32>,
}

/// In-memory CSR snapshot of one or more projects' IST. Build once via
/// [`IstGraph::build`], then traverse via [`IstGraph::forward_neighbors`] /
/// [`IstGraph::reverse_neighbors`]. The CSR arrays are immutable post-build ;
/// the snapshot is swapped atomically via [`crate::ist_snapshot::IstSnapshotCache`]
/// when a fresh load lands.
pub struct IstGraph {
    ids: Vec<String>,
    /// REQ-AXO-901970 — canonical display names, index-aligned with `ids`.
    /// Edge-implied phantom endpoints (no NodeRecord) fall back to the id suffix.
    names: Vec<String>,
    id_to_idx: HashMap<String, u32>,
    project_indices: Vec<u8>,
    project_codes: Vec<String>,
    kinds: Vec<u8>,
    flags: Vec<u8>,
    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, index-aligned
    /// with `ids`. Sentinel `-1` = unmeasured (see `NodeRecord::complexity`);
    /// real values are always >= 1 (a function has at least one path).
    complexity: Vec<i32>,
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

    /// REQ-AXO-901970 — count directed edges whose relation_type is in `rels`.
    /// O(E) over the forward CSR relation column.
    pub fn count_edges_with_relation(&self, rels: &[RelationType]) -> usize {
        self.fwd_rel
            .iter()
            .filter(|&&byte| rels.contains(&relation_from_u8(byte)))
            .count()
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

    /// REQ-AXO-901970 — canonical display name of a node (`ist.symbol.name`),
    /// distinct from the id suffix for TODO/secret/macro symbols. Phantom
    /// endpoints fall back to the id suffix at build time.
    pub fn name_of(&self, idx: u32) -> &str {
        &self.names[idx as usize]
    }

    /// REQ-AXO-901970 — canonical ids matching `name`, by the canonical `name`
    /// field OR the id's last `::` segment. Linear scan ; used by
    /// `query_graph_r1_neighbors` and `resolve_scoped_symbol_id_canonical` to
    /// resolve anchor names to ALL matching symbols.
    ///
    /// REQ-AXO-902075 — must match the `name` field, not only the id-suffix:
    /// the PG resolver this RAM lane mirrors is `WHERE name = $sym OR id = $sym`,
    /// and REQ-AXO-901970 explicitly stores `name` because some symbols carry a
    /// name that is NOT the last `::` segment of the id (TODO/secret/macro
    /// targets). Matching only the suffix silently diverged the RAM mirror from
    /// PG for those symbols (PIL-AXO-9002 requires the RAM mirror to agree).
    pub fn ids_with_short_name(&self, name: &str) -> Vec<&str> {
        self.ids
            .iter()
            .enumerate()
            .filter(|(i, id)| {
                id.rsplit("::").next() == Some(name) || self.names[*i].as_str() == name
            })
            .map(|(_, s)| s.as_str())
            .collect()
    }

    /// REQ-AXO-901970 — `NodeKind` for a canonical id, if present.
    pub fn node_kind(&self, id: &str) -> Option<NodeKind> {
        let idx = self.index_of(id)?;
        Some(NodeKind::from_u8(self.kinds[idx as usize]))
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

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity for a node,
    /// `None` if unmeasured (sentinel `-1` — see `NodeRecord::complexity`).
    pub fn complexity_of(&self, idx: u32) -> Option<i32> {
        match self.complexity[idx as usize] {
            n if n < 0 => None,
            n => Some(n),
        }
    }

    /// Forward neighbors of `idx` as `(target_idx, relation_type)` pairs.
    /// O(out-degree) ; no allocation.
    pub fn forward_neighbors(&self, idx: u32) -> impl Iterator<Item = (u32, RelationType)> + '_ {
        let start = self.fwd_offsets[idx as usize] as usize;
        let end = self.fwd_offsets[idx as usize + 1] as usize;
        (start..end).map(move |slot| (self.fwd_targets[slot], relation_from_u8(self.fwd_rel[slot])))
    }

    /// Reverse neighbors of `idx` (in-edges) as `(source_idx, relation_type)`.
    pub fn reverse_neighbors(&self, idx: u32) -> impl Iterator<Item = (u32, RelationType)> + '_ {
        let start = self.rev_offsets[idx as usize] as usize;
        let end = self.rev_offsets[idx as usize + 1] as usize;
        (start..end).map(move |slot| (self.rev_sources[slot], relation_from_u8(self.rev_rel[slot])))
    }

    /// REQ-AXO-91518 — Extract the `depth`-bounded neighborhood of `root_id`
    /// as a self-contained `IstGraph` (both forward + reverse directions).
    /// Used by VF2 isomorphism on per-symbol sub-graphs (semantic_clones
    /// slice 2) without copying the full snapshot. Returns `None` when the
    /// root id is unknown.
    pub fn neighborhood_subgraph(&self, root_id: &str, depth: u32) -> Option<IstGraph> {
        let root_idx = self.index_of(root_id)?;
        let mut visited: HashSet<u32> = HashSet::from([root_idx]);
        let mut frontier: Vec<u32> = vec![root_idx];

        for _ in 0..depth {
            let mut next: Vec<u32> = Vec::new();
            for &idx in &frontier {
                for (n, _) in self.forward_neighbors(idx) {
                    if visited.insert(n) {
                        next.push(n);
                    }
                }
                for (n, _) in self.reverse_neighbors(idx) {
                    if visited.insert(n) {
                        next.push(n);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        let mut nodes: Vec<NodeRecord> = Vec::with_capacity(visited.len());
        for &idx in &visited {
            let (kind_byte, project, flags) = self.node_meta(idx);
            nodes.push(NodeRecord {
                id: self.id_of(idx).to_string(),
                name: self.name_of(idx).to_string(),
                project_code: project.to_string(),
                kind: NodeKind::from_u8(kind_byte),
                flags,
                complexity: self.complexity_of(idx),
            });
        }

        let mut edges: Vec<EdgeTriple> = Vec::new();
        for &src_idx in &visited {
            for (tgt, rel) in self.forward_neighbors(src_idx) {
                if visited.contains(&tgt) {
                    edges.push(EdgeTriple {
                        source: self.id_of(src_idx).to_string(),
                        target: self.id_of(tgt).to_string(),
                        rel,
                    });
                }
            }
        }

        Some(IstGraph::build(nodes, edges))
    }

    /// Build a CSR snapshot from `nodes` + `edges`. Edge endpoints not present
    /// in `nodes` are auto-registered with `NodeKind::Other` and inherit the
    /// project code of the edge endpoint that introduced them, falling back
    /// to `""` when none is supplied. Stable ordering : nodes are indexed in
    /// the order they are first observed (declared records first, then
    /// edge-implied endpoints).
    pub fn build(nodes: Vec<NodeRecord>, edges: Vec<EdgeTriple>) -> Self {
        let mut ids: Vec<String> = Vec::with_capacity(nodes.len());
        let mut names: Vec<String> = Vec::with_capacity(nodes.len());
        let mut id_to_idx: HashMap<String, u32> = HashMap::with_capacity(nodes.len());
        let mut kinds: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut flags: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut complexity: Vec<i32> = Vec::with_capacity(nodes.len());
        let mut project_indices: Vec<u8> = Vec::with_capacity(nodes.len());
        let mut project_codes: Vec<String> = Vec::new();
        let mut project_to_idx: HashMap<String, u8> = HashMap::new();

        let intern_project =
            |code: String, codes: &mut Vec<String>, map: &mut HashMap<String, u8>| -> u8 {
                if let Some(&idx) = map.get(&code) {
                    return idx;
                }
                let idx = u8::try_from(codes.len()).unwrap_or(u8::MAX);
                map.insert(code.clone(), idx);
                codes.push(code);
                idx
            };

        // REQ-AXO-140 — name → UNIQUE function/method node, used to resolve a
        // synthetic CALLS target (`caller_file::name`, no node of its own) to the
        // canonical callee node at projection-build time. PG keeps the raw parse
        // (PIL-AXO-9004 immutable journal); the RAM graph does the interpretation
        // (PIL-AXO-9002). `(idx, ambiguous)` — a name with ≥2 defs stays
        // unresolved (phantom), exactly like the retired PG name-suffix workaround
        // (REQ-AXO-134) which only matched when the callee name was unique.
        let mut name_to_func: HashMap<String, (u32, bool)> = HashMap::new();
        // REQ-AXO-901986 — short name → all is_nif function/method nodes of that
        // name. Used ONLY to resolve a CALLS_NIF edge across the Elixir↔Rust NIF
        // boundary: the Elixir stub `Mod.foo` (is_nif) and the Rust
        // `#[rustler::nif] fn foo` (is_nif) share the bare name `foo`, so the
        // generic unique-name resolver (name_to_func) bails as ambiguous. The NIF
        // edge resolves to the is_nif node of that name that ISN'T the caller.
        let mut name_to_nif: HashMap<String, Vec<u32>> = HashMap::new();

        for record in nodes {
            let proj_idx = intern_project(
                record.project_code.clone(),
                &mut project_codes,
                &mut project_to_idx,
            );
            let idx = u32::try_from(ids.len()).expect("ist_snapshot exceeds u32 capacity");
            id_to_idx.insert(record.id.clone(), idx);
            if matches!(record.kind, NodeKind::Function | NodeKind::Method) {
                if let Some(name) = record.id.rsplit("::").next() {
                    name_to_func
                        .entry(name.to_string())
                        .and_modify(|e| e.1 = true)
                        .or_insert((idx, false));
                    if record.flags.nif() {
                        name_to_nif.entry(name.to_string()).or_default().push(idx);
                    }
                }
            }
            ids.push(record.id);
            names.push(record.name);
            kinds.push(record.kind as u8);
            flags.push(record.flags.0);
            complexity.push(record.complexity.unwrap_or(-1));
            project_indices.push(proj_idx);
        }

        let mut sources: Vec<u32> = Vec::with_capacity(edges.len());
        let mut targets: Vec<u32> = Vec::with_capacity(edges.len());
        let mut rels: Vec<u8> = Vec::with_capacity(edges.len());
        // REQ-AXO-140 — dedupe (source, target, rel) so a canonical edge and a
        // synthetic edge that resolves to the SAME canonical target collapse to
        // one. The PG name-suffix workaround returned both shapes → duplicates.
        let mut seen_edges: HashSet<(u32, u32, u8)> = HashSet::new();

        for edge in edges {
            let src_idx = match id_to_idx.get(&edge.source) {
                Some(&i) => i,
                None => {
                    let idx = u32::try_from(ids.len()).expect("ist_snapshot exceeds u32 capacity");
                    id_to_idx.insert(edge.source.clone(), idx);
                    let phantom_name = edge
                        .source
                        .rsplit("::")
                        .next()
                        .unwrap_or(&edge.source)
                        .to_string();
                    ids.push(edge.source);
                    names.push(phantom_name);
                    kinds.push(NodeKind::Other as u8);
                    flags.push(0);
                    complexity.push(-1);
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
                    // REQ-AXO-140 — resolve a synthetic target to the UNIQUE
                    // canonical function/method node of that name before falling
                    // back to a phantom. This is the whole REQ-AXO-134 workaround,
                    // moved from per-query PG SQL into the canonical RAM projection.
                    let resolved = edge
                        .target
                        .rsplit("::")
                        .next()
                        .and_then(|name| name_to_func.get(name))
                        .and_then(|&(idx, ambiguous)| (!ambiguous).then_some(idx))
                        // REQ-AXO-901986 — CALLS_NIF cross-language fallback: when
                        // the bare nif name is ambiguous (Elixir stub + Rust
                        // #[rustler::nif] share it), resolve to the is_nif node of
                        // that name that isn't the caller (the stub→impl hop).
                        .or_else(|| {
                            if edge.rel != RelationType::CallsNif {
                                return None;
                            }
                            let name = edge.target.rsplit("::").next()?;
                            name_to_nif
                                .get(name)?
                                .iter()
                                .copied()
                                .find(|&i| i != src_idx)
                        });
                    match resolved {
                        Some(i) => i,
                        None => {
                            let idx = u32::try_from(ids.len())
                                .expect("ist_snapshot exceeds u32 capacity");
                            id_to_idx.insert(edge.target.clone(), idx);
                            let phantom_name = edge
                                .target
                                .rsplit("::")
                                .next()
                                .unwrap_or(&edge.target)
                                .to_string();
                            ids.push(edge.target);
                            names.push(phantom_name);
                            kinds.push(NodeKind::Other as u8);
                            flags.push(0);
                            complexity.push(-1);
                            project_indices.push(intern_project(
                                String::new(),
                                &mut project_codes,
                                &mut project_to_idx,
                            ));
                            idx
                        }
                    }
                }
            };
            let rel_u8 = edge.rel as u8;
            if seen_edges.insert((src_idx, tgt_idx, rel_u8)) {
                sources.push(src_idx);
                targets.push(tgt_idx);
                rels.push(rel_u8);
            }
        }

        let node_count = ids.len();
        let (fwd_offsets, fwd_targets, fwd_rel) = build_csr(node_count, &sources, &targets, &rels);
        let (rev_offsets, rev_sources, rev_rel) = build_csr(node_count, &targets, &sources, &rels);

        // REQ-AXO-902187 — derive the `covered` flag: a node is COVERED iff it is
        // reachable via CALLS/CALLS_NIF forward-edges from a `#[test]` node (the
        // `tested` bit). The parser/DB `tested` flag NEVER propagated (RCA s96:
        // `tested` = syntactic "#[test]", weighted_coverage wrongly summed test
        // PageRank ≈ 6%). Doing it here — once per build — makes it RAM-native and
        // recomputed on EVERY snapshot warm, so a freshly-committed test flips its
        // production callees `covered` with ZERO delta lag. Multi-source forward
        // BFS over the just-built forward CSR ; O(V+E). Seeds (the tests) are
        // covered by definition (they are exercised by the runner).
        {
            let calls = RelationType::Calls as u8;
            let calls_nif = RelationType::CallsNif as u8;
            let mut covered = vec![false; node_count];
            let mut queue: VecDeque<u32> = VecDeque::new();
            for (i, &f) in flags.iter().enumerate() {
                if NodeFlags(f).tested() {
                    covered[i] = true;
                    queue.push_back(i as u32);
                }
            }
            while let Some(n) = queue.pop_front() {
                let start = fwd_offsets[n as usize] as usize;
                let end = fwd_offsets[n as usize + 1] as usize;
                for slot in start..end {
                    let rel = fwd_rel[slot];
                    if rel != calls && rel != calls_nif {
                        continue;
                    }
                    let t = fwd_targets[slot];
                    if !covered[t as usize] {
                        covered[t as usize] = true;
                        queue.push_back(t);
                    }
                }
            }
            for (i, &c) in covered.iter().enumerate() {
                if c {
                    flags[i] |= NodeFlags::COVERED;
                }
            }
        }

        Self {
            ids,
            names,
            id_to_idx,
            project_indices,
            project_codes,
            kinds,
            flags,
            complexity,
            fwd_offsets,
            fwd_targets,
            fwd_rel,
            rev_offsets,
            rev_sources,
            rev_rel,
        }
    }

    /// REQ-AXO-91486 — Bounded-radius BFS forward from `source_id`. Returns
    /// the set of canonical ids reached (excluding the seed). Aborts on
    /// `max_neighbors` (returning the partial frontier). Relation filter
    /// `rel_filter` short-circuits edges whose relation_type is not in the
    /// set — when empty, all relations are traversed.
    pub fn bfs_forward(
        &self,
        source_id: &str,
        max_radius: u32,
        max_neighbors: usize,
        rel_filter: &[RelationType],
    ) -> Vec<String> {
        let Some(start) = self.index_of(source_id) else {
            return Vec::new();
        };
        let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
        visited.insert(start);
        let mut frontier: Vec<u32> = vec![start];
        let mut out: Vec<String> = Vec::new();
        for _ in 0..max_radius {
            let mut next_frontier: Vec<u32> = Vec::new();
            for node in &frontier {
                for (target, rel) in self.forward_neighbors(*node) {
                    if !rel_filter.is_empty() && !rel_filter.contains(&rel) {
                        continue;
                    }
                    if visited.insert(target) {
                        out.push(self.id_of(target).to_string());
                        if out.len() >= max_neighbors {
                            return out;
                        }
                        next_frontier.push(target);
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }
        out
    }

    /// REQ-AXO-91486 — Bounded-radius BFS reverse (in-edges). Same contract
    /// as [`bfs_forward`] but traverses [`reverse_neighbors`] ; used by
    /// `impact` style queries (who calls X transitively).
    pub fn bfs_reverse(
        &self,
        source_id: &str,
        max_radius: u32,
        max_neighbors: usize,
        rel_filter: &[RelationType],
    ) -> Vec<String> {
        let Some(start) = self.index_of(source_id) else {
            return Vec::new();
        };
        let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
        visited.insert(start);
        let mut frontier: Vec<u32> = vec![start];
        let mut out: Vec<String> = Vec::new();
        for _ in 0..max_radius {
            let mut next_frontier: Vec<u32> = Vec::new();
            for node in &frontier {
                for (source, rel) in self.reverse_neighbors(*node) {
                    if !rel_filter.is_empty() && !rel_filter.contains(&rel) {
                        continue;
                    }
                    if visited.insert(source) {
                        out.push(self.id_of(source).to_string());
                        if out.len() >= max_neighbors {
                            return out;
                        }
                        next_frontier.push(source);
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }
        out
    }

    /// REQ-AXO-91510 — Bounded-radius BFS shortest path source→sink.
    /// Returns `Some((node_ids, relation_types))` where `node_ids[0] == source`
    /// and `node_ids.last() == sink`, walking the predecessor chain. The
    /// relation_types vector is aligned with the edge taken to reach each
    /// node, with `RelationType::Calls` placeholder at index 0 (the source
    /// has no incoming edge inside the path). Returns `None` when either
    /// endpoint is unknown or no path exists within `max_depth`. Honors
    /// `rel_filter` (empty ⇒ all relations).
    pub fn bfs_shortest_path(
        &self,
        source_id: &str,
        sink_id: &str,
        max_depth: u32,
        rel_filter: &[RelationType],
    ) -> Option<(Vec<String>, Vec<RelationType>)> {
        let start = self.index_of(source_id)?;
        let goal = self.index_of(sink_id)?;
        self.bfs_shortest_path_excluding(
            start,
            goal,
            max_depth,
            rel_filter,
            &std::collections::HashSet::new(),
        )
    }

    /// REQ-AXO-902019 — up to `max_paths` NODE-DISJOINT routes source→sink
    /// (pairwise sharing no intermediate node). `len >= 2` independent routes is
    /// a REDUNDANCY signal (GUI-PRO-107 lens L1) unless deliberate (perf
    /// fast-path). Greedy Yen-lite: take the shortest, exclude its intermediates,
    /// repeat. A direct edge (no intermediates) is the unique route → stops there.
    pub fn bfs_disjoint_paths(
        &self,
        source_id: &str,
        sink_id: &str,
        max_depth: u32,
        rel_filter: &[RelationType],
        max_paths: usize,
    ) -> Vec<(Vec<String>, Vec<RelationType>)> {
        let (Some(start), Some(goal)) = (self.index_of(source_id), self.index_of(sink_id)) else {
            return Vec::new();
        };
        let mut excluded: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut paths: Vec<(Vec<String>, Vec<RelationType>)> = Vec::new();
        for _ in 0..max_paths.max(1) {
            let Some((names, rels)) =
                self.bfs_shortest_path_excluding(start, goal, max_depth, rel_filter, &excluded)
            else {
                break;
            };
            // A direct edge (source, sink) has no intermediate to exclude — it is
            // the single route, so record it and stop enumerating alternates.
            let is_direct = names.len() <= 2;
            // Exclude this route's intermediate nodes so the next search yields a
            // node-disjoint alternative (not the same path again).
            for intermediate in names.iter().skip(1).rev().skip(1) {
                if let Some(idx) = self.index_of(intermediate) {
                    excluded.insert(idx);
                }
            }
            paths.push((names, rels));
            if is_direct {
                break;
            }
        }
        paths
    }

    /// Core BFS shared by `bfs_shortest_path` and `bfs_disjoint_paths`. Skips any
    /// `excluded` intermediate node (start/goal are never excluded).
    fn bfs_shortest_path_excluding(
        &self,
        start: u32,
        goal: u32,
        max_depth: u32,
        rel_filter: &[RelationType],
        excluded: &std::collections::HashSet<u32>,
    ) -> Option<(Vec<String>, Vec<RelationType>)> {
        if start == goal {
            return Some((
                vec![self.id_of(start).to_string()],
                vec![RelationType::Calls],
            ));
        }
        // parents[idx] = (predecessor_idx, edge_relation)
        let mut parents: std::collections::HashMap<u32, (u32, RelationType)> =
            std::collections::HashMap::new();
        let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
        visited.insert(start);
        let mut frontier: Vec<u32> = vec![start];
        for _ in 0..max_depth {
            let mut next_frontier: Vec<u32> = Vec::new();
            for node in &frontier {
                for (target, rel) in self.forward_neighbors(*node) {
                    if !rel_filter.is_empty() && !rel_filter.contains(&rel) {
                        continue;
                    }
                    // REQ-AXO-902019 — skip excluded intermediates (never the goal).
                    if target != goal && excluded.contains(&target) {
                        continue;
                    }
                    if visited.insert(target) {
                        parents.insert(target, (*node, rel));
                        if target == goal {
                            return Some(self.reconstruct_bfs_path(goal, start, &parents));
                        }
                        next_frontier.push(target);
                    }
                }
                // REQ-AXO-902028 — contract-dispatch bridge. A node that is the
                // TARGET of IMPLEMENTS edges is a contract (trait / Protocol /
                // ABC); follow those edges in REVERSE to reach the concrete
                // implementors, so a path landing on the contract continues to
                // the impl instead of dead-ending (dynamic-dispatch hole). The
                // fan-out is capped to bound search blow-up at high-fan-in
                // contracts, and it is gated on IMPLEMENTS being in the relation
                // filter (so callers can opt out via an explicit filter). The
                // bridged hop is recorded as an IMPLEMENTS relation traversed in
                // reverse.
                if rel_filter.is_empty() || rel_filter.contains(&RelationType::Implements) {
                    let mut bridged = 0usize;
                    for (implementor, rel) in self.reverse_neighbors(*node) {
                        if rel != RelationType::Implements {
                            continue;
                        }
                        if bridged >= CONTRACT_BRIDGE_FANOUT_CAP {
                            break;
                        }
                        bridged += 1;
                        if implementor != goal && excluded.contains(&implementor) {
                            continue;
                        }
                        if visited.insert(implementor) {
                            parents.insert(implementor, (*node, RelationType::Implements));
                            if implementor == goal {
                                return Some(self.reconstruct_bfs_path(goal, start, &parents));
                            }
                            next_frontier.push(implementor);
                        }
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }
        None
    }

    /// Reconstruct a BFS path from `goal` back to `start` via the `parents`
    /// predecessor map. Each `parents[c] = (pred, edge_rel)` exposes the edge
    /// `pred -> c`, so one relation accumulates per hop; a placeholder `Calls`
    /// is prepended at index 0 to align lengths with the source slot (which has
    /// no incoming edge inside the path).
    fn reconstruct_bfs_path(
        &self,
        goal: u32,
        start: u32,
        parents: &std::collections::HashMap<u32, (u32, RelationType)>,
    ) -> (Vec<String>, Vec<RelationType>) {
        let mut chain_idx: Vec<u32> = vec![goal];
        let mut chain_rel: Vec<RelationType> = Vec::new();
        let mut cursor = goal;
        while let Some((pred, edge_rel)) = parents.get(&cursor) {
            chain_idx.push(*pred);
            chain_rel.push(*edge_rel);
            if *pred == start {
                break;
            }
            cursor = *pred;
        }
        chain_idx.reverse();
        chain_rel.reverse();
        chain_rel.insert(0, RelationType::Calls);
        let names: Vec<String> = chain_idx
            .iter()
            .map(|i| self.id_of(*i).to_string())
            .collect();
        (names, chain_rel)
    }

    /// REQ-AXO-91486 — count reciprocal CALLS cycles (A→B + B→A) used by
    /// `get_circular_dependency_count_fast`. Linear in edges, dedup via
    /// canonical pair ordering. Self-loops (A→A) are excluded.
    pub fn reciprocal_calls_cycle_count(&self) -> usize {
        let mut pairs: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for source_idx in 0..(self.ids.len() as u32) {
            for (target_idx, rel) in self.forward_neighbors(source_idx) {
                if !matches!(rel, RelationType::Calls) {
                    continue;
                }
                if source_idx == target_idx {
                    continue;
                }
                // Look for the reverse edge (target -> source) with CALLS.
                let has_reciprocal = self
                    .forward_neighbors(target_idx)
                    .any(|(t, r)| t == source_idx && matches!(r, RelationType::Calls));
                if has_reciprocal {
                    let pair = if source_idx < target_idx {
                        (source_idx, target_idx)
                    } else {
                        (target_idx, source_idx)
                    };
                    pairs.insert(pair);
                }
            }
        }
        pairs.len()
    }

    /// Approximate resident memory (bytes) — sum of CSR + arena + index
    /// overhead. Used by the bench binary and ist_snapshot diagnostics.
    pub fn approximate_bytes(&self) -> usize {
        let ids_bytes: usize = self.ids.iter().map(String::len).sum();
        let names_bytes: usize = self.names.iter().map(String::len).sum();
        let names_overhead = self.names.capacity() * std::mem::size_of::<String>();
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
        ids_bytes
            + names_bytes
            + names_overhead
            + ids_overhead
            + id_to_idx_overhead
            + csr_bytes
            + attr_bytes
            + projects_bytes
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
        3 => RelationType::Implements,
        4 => RelationType::Imports,
        5 => RelationType::Uses,
        6 => RelationType::Reads,
        7 => RelationType::Declares,
        8 => RelationType::ReadsArtifact,
        9 => RelationType::SimilarTo,
        _ => RelationType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, project: &str, kind: NodeKind) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: project.to_string(),
            kind,
            flags: NodeFlags::default(),
            complexity: None,
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

    // REQ-AXO-902187 — the `covered` propagation: a #[test] node covers itself and
    // everything reachable via CALLS forward-edges (transitively). Non-CALLS edges
    // (Contains) and unreachable nodes stay uncovered. Proves weighted_coverage now
    // measures wiring ("are the hubs exercised by a test?"), not the raw #[test] bit.
    #[test]
    fn covered_propagates_forward_from_tests_via_calls_only() {
        let test_fn = NodeRecord {
            id: "AXO::m::a_test".to_string(),
            name: "a_test".to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            flags: NodeFlags::new(true, false, false, false), // tested = carries #[test]
            complexity: None,
        };
        let nodes = vec![
            test_fn,
            node("AXO::m::hub", "AXO", NodeKind::Function), // called by the test
            node("AXO::m::deep", "AXO", NodeKind::Function), // called by hub (transitive)
            node("AXO::m::isolated", "AXO", NodeKind::Function), // only reached via Contains
            node("AXO::m::file", "AXO", NodeKind::File),   // not reached from any test
        ];
        let edges = vec![
            edge("AXO::m::a_test", "AXO::m::hub", RelationType::Calls),
            edge("AXO::m::hub", "AXO::m::deep", RelationType::Calls),
            edge("AXO::m::file", "AXO::m::isolated", RelationType::Contains),
        ];
        let g = IstGraph::build(nodes, edges);
        let covered = |id: &str| g.node_meta(g.index_of(id).unwrap()).2.covered();
        assert!(covered("AXO::m::a_test"), "a #[test] seeds itself as covered");
        assert!(covered("AXO::m::hub"), "direct CALLS callee is covered");
        assert!(covered("AXO::m::deep"), "transitive CALLS callee is covered");
        assert!(
            !covered("AXO::m::isolated"),
            "reached only via Contains → not covered"
        );
        assert!(
            !covered("AXO::m::file"),
            "not reachable from any test → not covered"
        );
    }

    fn nif_node(id: &str) -> NodeRecord {
        NodeRecord {
            id: id.to_string(),
            name: id.rsplit("::").next().unwrap_or(id).to_string(),
            project_code: "AXO".to_string(),
            kind: NodeKind::Function,
            // tested=false, public=true, nif=true, unsafe=false
            flags: NodeFlags::new(false, true, true, false),
            complexity: None,
        }
    }

    #[test]
    fn build_resolves_calls_nif_across_elixir_rust_boundary() {
        // REQ-AXO-901986 — the Elixir stub `Native.add` (is_nif) calls the Rust
        // `#[rustler::nif] fn add` (is_nif). They share the bare name `add`, so
        // the generic unique-name resolver bails as ambiguous. The CALLS_NIF edge
        // must still resolve across the language boundary to the Rust impl —
        // never a phantom, never the stub itself.
        let nodes = vec![
            nif_node("AXO::lib/native.ex::Native.add"),
            nif_node("AXO::native/src/lib.rs::add"),
        ];
        // Elixir parser emits calls_nif with a synthetic target under the Elixir
        // file path (bare callee name), exactly like the local-call shape.
        let edges = vec![edge(
            "AXO::lib/native.ex::Native.add",
            "AXO::lib/native.ex::add",
            RelationType::CallsNif,
        )];
        let g = IstGraph::build(nodes, edges);
        let stub = g.index_of("AXO::lib/native.ex::Native.add").unwrap();
        let rust = g.index_of("AXO::native/src/lib.rs::add").unwrap();
        // The synthetic target must NOT become a phantom node.
        assert_eq!(
            g.index_of("AXO::lib/native.ex::add"),
            None,
            "synthetic NIF target must not become a phantom"
        );
        let fwd: Vec<_> = g.forward_neighbors(stub).map(|(t, _)| t).collect();
        assert_eq!(
            fwd,
            vec![rust],
            "CALLS_NIF must resolve cross-language to the Rust impl (not phantom/self)"
        );
        // And impact (reverse) on the Rust impl now sees the Elixir caller.
        let rev: Vec<_> = g.reverse_neighbors(rust).map(|(s, _)| s).collect();
        assert_eq!(rev, vec![stub]);
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
    fn synthetic_call_target_resolves_to_unique_canonical_node() {
        // REQ-AXO-140 — a CALLS edge whose target is a synthetic `caller::name`
        // (no node of its own) resolves to the UNIQUE canonical function node of
        // that name instead of spawning a phantom. The PG name-suffix workaround
        // (REQ-AXO-134) now lives in this RAM projection.
        let nodes = vec![
            node("p::a.rs::caller", "P", NodeKind::Function),
            node("p::b.rs::callee", "P", NodeKind::Function),
        ];
        // Indexer emitted the caller-file id `p::a.rs::callee` (no such node).
        let edges = vec![edge("p::a.rs::caller", "p::a.rs::callee", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let caller = g.index_of("p::a.rs::caller").unwrap();
        let callee = g.index_of("p::b.rs::callee").unwrap();
        assert_eq!(
            g.index_of("p::a.rs::callee"),
            None,
            "synthetic id must NOT become a phantom node"
        );
        let fwd: Vec<_> = g.forward_neighbors(caller).map(|(t, _)| t).collect();
        assert_eq!(fwd, vec![callee], "synthetic target resolved to the canonical callee");
        let rev: Vec<_> = g.reverse_neighbors(callee).map(|(s, _)| s).collect();
        assert_eq!(rev, vec![caller], "callee now sees its real caller");
    }

    #[test]
    fn ambiguous_synthetic_target_stays_phantom() {
        // Two `dup` defs → ambiguous → never guess; the synthetic stays a phantom.
        let nodes = vec![
            node("p::a.rs::caller", "P", NodeKind::Function),
            node("p::b.rs::dup", "P", NodeKind::Function),
            node("p::c.rs::dup", "P", NodeKind::Function),
        ];
        let edges = vec![edge("p::a.rs::caller", "p::a.rs::dup", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        assert!(
            g.index_of("p::a.rs::dup").is_some(),
            "ambiguous name stays a phantom, never guessed"
        );
    }

    #[test]
    fn canonical_and_resolved_synthetic_edge_dedupe() {
        // REQ-AXO-140 — a canonical edge AND a synthetic edge that resolves to the
        // SAME (source, target, rel) collapse to ONE (no duplicate neighbor — the
        // duplication the PG workaround produced).
        let nodes = vec![
            node("p::a.rs::caller", "P", NodeKind::Function),
            node("p::b.rs::callee", "P", NodeKind::Function),
        ];
        let edges = vec![
            edge("p::a.rs::caller", "p::b.rs::callee", RelationType::Calls), // canonical
            edge("p::a.rs::caller", "p::a.rs::callee", RelationType::Calls), // synthetic → same
        ];
        let g = IstGraph::build(nodes, edges);
        let caller = g.index_of("p::a.rs::caller").unwrap();
        let fwd: Vec<_> = g.forward_neighbors(caller).map(|(t, _)| t).collect();
        assert_eq!(fwd.len(), 1, "canonical + resolved-synthetic dedupe to one edge");
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
        for s in [
            "CONTAINS",
            "CALLS",
            "CALLS_NIF",
            "IMPLEMENTS",
            "IMPORTS",
            "USES",
            "READS",
            "DECLARES",
            "READS_ARTIFACT",
            "SIMILAR_TO",
        ] {
            assert_eq!(RelationType::from_db(s).as_db(), s);
        }
        // REQ-AXO-902017 — ReadsArtifact survives the CSR u8 round-trip (=8).
        assert_eq!(relation_from_u8(8), RelationType::ReadsArtifact);
        assert_eq!(RelationType::ReadsArtifact as u8, 8);
        // REQ-AXO-902185 — SimilarTo survives the CSR u8 round-trip (=9).
        assert_eq!(relation_from_u8(9), RelationType::SimilarTo);
        assert_eq!(RelationType::SimilarTo as u8, 9);
        assert_eq!(RelationType::from_db("UNKNOWN"), RelationType::Other);
    }

    #[test]
    fn relation_type_from_db_accepts_lowercase_parser_output() {
        // REQ-AXO-91505 — parsers emit lowercase ("imports", "implements",
        // "uses", "calls") ; reading from ist.edge must canonicalize.
        assert_eq!(RelationType::from_db("imports"), RelationType::Imports);
        assert_eq!(
            RelationType::from_db("implements"),
            RelationType::Implements
        );
        assert_eq!(RelationType::from_db("uses"), RelationType::Uses);
        assert_eq!(RelationType::from_db("calls"), RelationType::Calls);
        assert_eq!(RelationType::from_db("calls_nif"), RelationType::CallsNif);
    }

    #[test]
    fn relation_type_round_trips_through_csr_u8_storage() {
        // CSR stores the relation_type as u8 ; the round-trip via
        // relation_from_u8 must preserve the new IMPLEMENTS/IMPORTS/USES
        // variants alongside the legacy three.
        let nodes = vec![
            node("a", "AXO", NodeKind::Module),
            node("b", "AXO", NodeKind::Trait),
            node("c", "AXO", NodeKind::Module),
            node("d", "AXO", NodeKind::Module),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Implements),
            edge("a", "c", RelationType::Imports),
            edge("a", "d", RelationType::Uses),
        ];
        let g = IstGraph::build(nodes, edges);
        let a = g.index_of("a").unwrap();
        let rels: Vec<RelationType> = g.forward_neighbors(a).map(|(_, r)| r).collect();
        assert!(rels.contains(&RelationType::Implements));
        assert!(rels.contains(&RelationType::Imports));
        assert!(rels.contains(&RelationType::Uses));
    }

    #[test]
    fn bfs_forward_returns_descendants_up_to_radius() {
        // a -> b -> c -> d ; b -> e
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
            node("e", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
            edge("c", "d", RelationType::Calls),
            edge("b", "e", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let reach = g.bfs_forward("a", 2, 100, &[]);
        let set: std::collections::HashSet<&str> = reach.iter().map(String::as_str).collect();
        assert!(set.contains("b"));
        assert!(set.contains("c"));
        assert!(set.contains("e"));
        assert!(!set.contains("d"), "radius 2 should NOT reach d");
    }

    #[test]
    fn bfs_forward_honors_max_neighbors_cap() {
        let nodes = (0..10)
            .map(|i| node(&format!("n{}", i), "AXO", NodeKind::Function))
            .collect::<Vec<_>>();
        let mut edges: Vec<EdgeTriple> = Vec::new();
        for i in 1..10 {
            edges.push(edge("n0", &format!("n{}", i), RelationType::Calls));
        }
        let g = IstGraph::build(nodes, edges);
        let reach = g.bfs_forward("n0", 5, 3, &[]);
        assert_eq!(reach.len(), 3);
    }

    // REQ-AXO-901970 — count_edges_with_relation counts only the requested
    // relation types over the forward CSR.
    #[test]
    fn count_edges_with_relation_counts_requested_types() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
            edge("a", "c", RelationType::CallsNif),
            edge("a", "b", RelationType::Contains),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.count_edges_with_relation(&[RelationType::Calls]), 2);
        assert_eq!(g.count_edges_with_relation(&[RelationType::CallsNif]), 1);
        assert_eq!(
            g.count_edges_with_relation(&[RelationType::Calls, RelationType::CallsNif]),
            3
        );
        assert_eq!(g.count_edges_with_relation(&[RelationType::Implements]), 0);
    }

    #[test]
    fn bfs_forward_filters_by_relation_type() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Contains),
            edge("a", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let reach_calls = g.bfs_forward("a", 3, 100, &[RelationType::Calls]);
        assert_eq!(reach_calls, vec!["c"]);
        let reach_contains = g.bfs_forward("a", 3, 100, &[RelationType::Contains]);
        assert_eq!(reach_contains, vec!["b"]);
    }

    #[test]
    fn bfs_reverse_collects_ancestors() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "c", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let callers = g.bfs_reverse("c", 1, 100, &[RelationType::Calls]);
        let set: std::collections::HashSet<&str> = callers.iter().map(String::as_str).collect();
        assert!(set.contains("a"));
        assert!(set.contains("b"));
    }

    #[test]
    fn bfs_shortest_path_three_node_chain() {
        // REQ-AXO-91510 — a→b→c, shortest path a→c is [a,b,c].
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let (names, rels) = g
            .bfs_shortest_path("a", "c", 6, &[])
            .expect("path must exist");
        assert_eq!(
            names,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        // rels has one slot per node: source placeholder + 2 edge rels.
        assert_eq!(rels.len(), 3);
        assert!(matches!(rels[0], RelationType::Calls)); // placeholder
        assert!(matches!(rels[1], RelationType::Calls)); // a→b
        assert!(matches!(rels[2], RelationType::Calls)); // b→c
    }

    #[test]
    fn bfs_bridges_contract_to_implementor_via_reverse_implements() {
        // REQ-AXO-902028 — dynamic dispatch: `caller` CALLS the contract `Trait`;
        // `Foo` IMPLEMENTS `Trait` (edge Foo→Trait). A forward-only BFS dead-ends
        // at `Trait`; the contract bridge follows the IMPLEMENTS edge in reverse
        // so caller→Foo resolves to [caller, Trait, Foo].
        let nodes = vec![
            node("caller", "AXO", NodeKind::Function),
            node("Trait", "AXO", NodeKind::Function),
            node("Foo", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("caller", "Trait", RelationType::Calls),
            edge("Foo", "Trait", RelationType::Implements),
        ];
        let g = IstGraph::build(nodes, edges);

        let (names, rels) = g
            .bfs_shortest_path("caller", "Foo", 6, &[])
            .expect("contract bridge must connect caller→Foo");
        assert_eq!(
            names,
            vec!["caller".to_string(), "Trait".to_string(), "Foo".to_string()]
        );
        assert!(matches!(rels[1], RelationType::Calls)); // caller→Trait
        assert!(matches!(rels[2], RelationType::Implements)); // bridged Trait→Foo

        // An explicit relation filter that omits IMPLEMENTS opts out of the
        // bridge — the caller can still restrict traversal to pure call edges.
        assert!(
            g.bfs_shortest_path("caller", "Foo", 6, &[RelationType::Calls])
                .is_none(),
            "a CALLS-only filter must disable the implements bridge"
        );
    }

    #[test]
    fn bfs_shortest_path_unreachable_returns_none() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
        ];
        // No edges → no path.
        let g = IstGraph::build(nodes, Vec::new());
        assert!(g.bfs_shortest_path("a", "b", 6, &[]).is_none());
    }

    #[test]
    fn bfs_shortest_path_picks_shorter_when_two_routes_exist() {
        // a→b→d (len 3) vs a→c→x→d (len 4). BFS must return the 3-node path.
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
            node("x", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "d", RelationType::Calls),
            edge("a", "c", RelationType::Calls),
            edge("c", "x", RelationType::Calls),
            edge("x", "d", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let (names, _) = g.bfs_shortest_path("a", "d", 6, &[]).expect("path");
        assert_eq!(
            names,
            vec!["a".to_string(), "b".to_string(), "d".to_string()]
        );
    }

    #[test]
    fn bfs_shortest_path_respects_max_depth() {
        // a→b→c→d, max_depth=2 ⇒ cannot reach d.
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
            edge("c", "d", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(g.bfs_shortest_path("a", "d", 2, &[]).is_none());
        assert!(g.bfs_shortest_path("a", "d", 3, &[]).is_some());
    }

    #[test]
    fn bfs_shortest_path_filters_relation_types() {
        // a-(CONTAINS)→b-(CALLS)→c. With CALLS-only filter, no path.
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Contains),
            edge("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert!(g
            .bfs_shortest_path("a", "c", 6, &[RelationType::Calls])
            .is_none());
        // Without filter, path exists.
        assert!(g.bfs_shortest_path("a", "c", 6, &[]).is_some());
    }

    // REQ-AXO-902019 — node-disjoint route enumeration (multiplicity signal).
    #[test]
    fn bfs_disjoint_paths_finds_two_independent_routes() {
        // a→b→d and a→c→d : two node-disjoint routes (b/c distinct).
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "d", RelationType::Calls),
            edge("a", "c", RelationType::Calls),
            edge("c", "d", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let routes = g.bfs_disjoint_paths("a", "d", 6, &[], 3);
        assert_eq!(routes.len(), 2, "two node-disjoint routes a→{{b,c}}→d");
        // The two routes share no intermediate node.
        let mids: Vec<&String> = routes
            .iter()
            .flat_map(|(ids, _)| ids.iter().skip(1).rev().skip(1))
            .collect();
        let unique: std::collections::HashSet<&&String> = mids.iter().collect();
        assert_eq!(unique.len(), mids.len(), "intermediates are disjoint");
    }

    #[test]
    fn bfs_disjoint_paths_single_linear_route_is_one() {
        // a→b→c : only one route, no independent alternate.
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "c", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.bfs_disjoint_paths("a", "c", 6, &[], 3).len(), 1);
    }

    #[test]
    fn bfs_disjoint_paths_direct_edge_is_single_route() {
        // a→b direct : one route, enumeration stops (no intermediate to exclude).
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
        ];
        let edges = vec![edge("a", "b", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let routes = g.bfs_disjoint_paths("a", "b", 6, &[], 5);
        assert_eq!(routes.len(), 1, "a direct edge is the single route");
    }

    #[test]
    fn bfs_disjoint_paths_respects_max_paths_cap() {
        // Three independent routes a→{x,y,z}→d ; cap at 2.
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("x", "AXO", NodeKind::Function),
            node("y", "AXO", NodeKind::Function),
            node("z", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "x", RelationType::Calls),
            edge("x", "d", RelationType::Calls),
            edge("a", "y", RelationType::Calls),
            edge("y", "d", RelationType::Calls),
            edge("a", "z", RelationType::Calls),
            edge("z", "d", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.bfs_disjoint_paths("a", "d", 6, &[], 2).len(), 2);
    }

    #[test]
    fn reciprocal_calls_cycle_count_matches_pairs() {
        // a<->b (1 cycle) ; c<->d (1 cycle) ; e->f one-way (0)
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
            node("c", "AXO", NodeKind::Function),
            node("d", "AXO", NodeKind::Function),
            node("e", "AXO", NodeKind::Function),
            node("f", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("a", "b", RelationType::Calls),
            edge("b", "a", RelationType::Calls),
            edge("c", "d", RelationType::Calls),
            edge("d", "c", RelationType::Calls),
            edge("e", "f", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.reciprocal_calls_cycle_count(), 2);
    }

    #[test]
    fn reciprocal_calls_cycle_count_excludes_self_loops() {
        let nodes = vec![node("a", "AXO", NodeKind::Function)];
        let edges = vec![edge("a", "a", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        assert_eq!(g.reciprocal_calls_cycle_count(), 0);
    }

    #[test]
    fn node_flags_round_trip_known_combinations() {
        let f = NodeFlags::new(true, false, true, false);
        assert!(f.tested());
        assert!(!f.public());
        assert!(f.nif());
        assert!(!f.unsafe_());
    }

    #[test]
    fn node_kind_from_u8_round_trip_for_canonical_variants() {
        assert_eq!(NodeKind::from_u8(0), NodeKind::File);
        assert_eq!(NodeKind::from_u8(1), NodeKind::Function);
        assert_eq!(NodeKind::from_u8(2), NodeKind::Method);
        assert_eq!(NodeKind::from_u8(3), NodeKind::Class);
        assert_eq!(NodeKind::from_u8(11), NodeKind::ConfigKey);
        assert_eq!(NodeKind::from_u8(12), NodeKind::Interface);
        // REQ-AXO-902017 — DataArtifact round-trips through db + u8 (=13).
        assert_eq!(NodeKind::from_u8(13), NodeKind::DataArtifact);
        assert_eq!(NodeKind::DataArtifact as u8, 13);
        assert_eq!(NodeKind::from_db("data_artifact"), NodeKind::DataArtifact);
        assert_eq!(NodeKind::DataArtifact.as_db(), "data_artifact");
        assert_eq!(NodeKind::from_u8(42), NodeKind::Other);
        assert_eq!(NodeKind::from_u8(255), NodeKind::Other);
    }

    #[test]
    fn neighborhood_subgraph_returns_none_for_unknown_root() {
        let nodes = vec![node("a", "AXO", NodeKind::Function)];
        let g = IstGraph::build(nodes, vec![]);
        assert!(g.neighborhood_subgraph("missing", 1).is_none());
    }

    #[test]
    fn neighborhood_subgraph_depth_0_returns_singleton_no_edges() {
        let nodes = vec![
            node("a", "AXO", NodeKind::Function),
            node("b", "AXO", NodeKind::Function),
        ];
        let edges = vec![edge("a", "b", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let sub = g.neighborhood_subgraph("a", 0).expect("root exists");
        assert_eq!(sub.node_count(), 1);
        assert_eq!(sub.edge_count(), 0);
        assert_eq!(sub.index_of("a"), Some(0));
    }

    #[test]
    fn neighborhood_subgraph_depth_1_captures_both_directions() {
        let nodes = vec![
            node("caller", "AXO", NodeKind::Function),
            node("root", "AXO", NodeKind::Function),
            node("callee", "AXO", NodeKind::Function),
            node("unrelated", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("caller", "root", RelationType::Calls),
            edge("root", "callee", RelationType::Calls),
        ];
        let g = IstGraph::build(nodes, edges);
        let sub = g.neighborhood_subgraph("root", 1).expect("root exists");
        assert_eq!(sub.node_count(), 3, "caller + root + callee");
        assert_eq!(sub.edge_count(), 2);
        assert!(sub.index_of("caller").is_some());
        assert!(sub.index_of("callee").is_some());
        assert!(
            sub.index_of("unrelated").is_none(),
            "depth=1 must not include unrelated"
        );
    }

    #[test]
    fn neighborhood_subgraph_preserves_node_kind_via_from_u8() {
        let nodes = vec![
            node("root", "AXO", NodeKind::Method),
            node("callee", "AXO", NodeKind::Function),
        ];
        let edges = vec![edge("root", "callee", RelationType::Calls)];
        let g = IstGraph::build(nodes, edges);
        let sub = g.neighborhood_subgraph("root", 1).expect("root exists");
        let root_idx = sub.index_of("root").unwrap();
        let (kind, _, _) = sub.node_meta(root_idx);
        assert_eq!(kind, NodeKind::Method as u8);
    }
}
