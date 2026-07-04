//! REQ-AXO-157 — declarative structural-invariants validator.
//!
//! DEC-AXO-901649: this is a **minimal SOLL-anchored predicate schema**, NOT a
//! general graph-query DSL (re-implementing CodeQL/semgrep is the trap the
//! requirement body named). A rule is
//! `{mode: forbidden|required, source, target, relations[]}` where `source`
//! and `target` select nodes by architectural *layer* (canonical-id prefix) or
//! by `NodeKind`. Rules are evaluated against the RAM IST graph using the
//! primitives that already back `architectural_drift` (`layer_violations` family)
//! — `node_meta`, `forward_neighbors`, id-prefix matching — in O(N + M).
//!
//! The differentiated value over an external linter is the IST×SOLL join: a rule
//! *lives in SOLL* (it carries its governing node id), so a violation is
//! traceable back to the intent that mandated it via `why`. This module is the
//! pure evaluator; the MCP handler (`tools_governance::axon_structural_invariants`)
//! loads rules from SOLL, warms the snapshot, and renders the envelope.

use super::snapshot::{IstGraph, NodeKind, RelationType};

/// Whether the predicate forbids the edge pattern or requires it to exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvariantMode {
    /// No `source` node may have a qualifying edge to a `target` node.
    /// Each offending edge is one violation.
    Forbidden,
    /// Every `source` node MUST have at least one qualifying edge to a
    /// `target` node. A source with none is one violation.
    Required,
}

impl InvariantMode {
    /// Parse the canonical `mode` string from a SOLL rule (case-insensitive).
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "forbidden" | "forbid" | "deny" => Some(Self::Forbidden),
            "required" | "require" | "must" => Some(Self::Required),
            _ => None,
        }
    }
}

/// How a rule selects one endpoint of the edge. `Layer` is the coarse
/// architectural axis (canonical-id prefix, e.g. `core/`, `mcp/`, `db/`);
/// `Kind` matches a `NodeKind`; `Any` matches every node (when only the other
/// side is constrained).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeMatcher {
    Layer(String),
    Kind(NodeKind),
    Any,
}

impl NodeMatcher {
    /// Build a matcher from the `(layer, kind)` fields a rule carries in its
    /// SOLL metadata. `layer` wins when both are present — the DEC names the
    /// layer axis first and it is the coarser architectural constraint. Empty
    /// or absent on both sides → [`NodeMatcher::Any`].
    pub fn from_fields(layer: Option<&str>, kind: Option<&str>) -> Self {
        if let Some(prefix) = layer.map(str::trim).filter(|s| !s.is_empty()) {
            return Self::Layer(prefix.to_string());
        }
        if let Some(k) = kind.map(str::trim).filter(|s| !s.is_empty()) {
            return Self::Kind(NodeKind::from_db(k));
        }
        Self::Any
    }

    fn matches(&self, id: &str, kind: NodeKind) -> bool {
        match self {
            Self::Layer(prefix) => id.starts_with(prefix.as_str()),
            Self::Kind(k) => kind == *k,
            Self::Any => true,
        }
    }
}

/// One declarative structural invariant. `id`/`title` carry the governing SOLL
/// node so a violation is joinable back to its intent (`why <id>`).
#[derive(Clone, Debug)]
pub struct StructuralInvariant {
    pub id: String,
    pub title: String,
    pub mode: InvariantMode,
    pub source: NodeMatcher,
    pub target: NodeMatcher,
    /// Relation types the rule constrains. Empty = any relation.
    pub relations: Vec<RelationType>,
}

/// One detected violation, carrying the rule id so the caller can render the
/// IST×SOLL join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvariantViolation {
    pub rule_id: String,
    pub source_id: String,
    /// The offending edge target for `forbidden`. `None` for `required`
    /// (the source node has NO qualifying edge).
    pub target_id: Option<String>,
    pub relation: Option<RelationType>,
}

/// Evaluate a single rule against the snapshot, scoped to `project` (empty
/// string = all projects). O(N + out-degree·matches); reuses the same RAM
/// primitives as `architectural_drift`.
pub fn evaluate_invariant(
    graph: &IstGraph,
    project: &str,
    rule: &StructuralInvariant,
) -> Vec<InvariantViolation> {
    let rel_ok = |rel: RelationType| rule.relations.is_empty() || rule.relations.contains(&rel);
    let proj_ok = |p: &str| project.is_empty() || p == project;

    let mut out: Vec<InvariantViolation> = Vec::new();
    let n = graph.node_count();
    for src in 0..n as u32 {
        let (skind_b, sproj, _flags) = graph.node_meta(src);
        if !proj_ok(sproj) {
            continue;
        }
        let src_id = graph.id_of(src);
        if !rule.source.matches(src_id, NodeKind::from_u8(skind_b)) {
            continue;
        }

        match rule.mode {
            InvariantMode::Forbidden => {
                for (tgt, rel) in graph.forward_neighbors(src) {
                    if !rel_ok(rel) {
                        continue;
                    }
                    let tgt_id = graph.id_of(tgt);
                    let (tkind_b, _tp, _tf) = graph.node_meta(tgt);
                    if rule.target.matches(tgt_id, NodeKind::from_u8(tkind_b)) {
                        out.push(InvariantViolation {
                            rule_id: rule.id.clone(),
                            source_id: src_id.to_string(),
                            target_id: Some(tgt_id.to_string()),
                            relation: Some(rel),
                        });
                    }
                }
            }
            InvariantMode::Required => {
                let satisfied = graph.forward_neighbors(src).any(|(tgt, rel)| {
                    if !rel_ok(rel) {
                        return false;
                    }
                    let (tkind_b, _tp, _tf) = graph.node_meta(tgt);
                    rule.target.matches(graph.id_of(tgt), NodeKind::from_u8(tkind_b))
                });
                if !satisfied {
                    out.push(InvariantViolation {
                        rule_id: rule.id.clone(),
                        source_id: src_id.to_string(),
                        target_id: None,
                        relation: None,
                    });
                }
            }
        }
    }
    out
}

/// Evaluate a batch of rules, concatenating their violations (rule order
/// preserved). Convenience for the MCP handler.
pub fn evaluate_all(
    graph: &IstGraph,
    project: &str,
    rules: &[StructuralInvariant],
) -> Vec<InvariantViolation> {
    rules
        .iter()
        .flat_map(|r| evaluate_invariant(graph, project, r))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ist_snapshot::snapshot::{EdgeTriple, NodeFlags, NodeRecord};

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

    fn rule(
        id: &str,
        mode: InvariantMode,
        source: NodeMatcher,
        target: NodeMatcher,
        relations: Vec<RelationType>,
    ) -> StructuralInvariant {
        StructuralInvariant {
            id: id.to_string(),
            title: format!("test rule {id}"),
            mode,
            source,
            target,
            relations,
        }
    }

    #[test]
    fn mode_parses_case_insensitively() {
        assert_eq!(InvariantMode::from_str_ci("Forbidden"), Some(InvariantMode::Forbidden));
        assert_eq!(InvariantMode::from_str_ci("REQUIRE"), Some(InvariantMode::Required));
        assert_eq!(InvariantMode::from_str_ci(" must "), Some(InvariantMode::Required));
        assert_eq!(InvariantMode::from_str_ci("whatever"), None);
    }

    #[test]
    fn matcher_layer_wins_over_kind_then_any() {
        assert_eq!(
            NodeMatcher::from_fields(Some("core/"), Some("function")),
            NodeMatcher::Layer("core/".into())
        );
        assert_eq!(
            NodeMatcher::from_fields(Some(""), Some("struct")),
            NodeMatcher::Kind(NodeKind::Struct)
        );
        assert_eq!(NodeMatcher::from_fields(None, None), NodeMatcher::Any);
    }

    fn layered_graph() -> IstGraph {
        // core/a -> mcp/b (CALLS): an upward call across the forbidden boundary.
        // mcp/b -> core/a (CALLS): the allowed downward direction.
        let nodes = vec![
            node("AXO::core/a.rs::foo", "AXO", NodeKind::Function),
            node("AXO::mcp/b.rs::bar", "AXO", NodeKind::Function),
        ];
        let edges = vec![
            edge("AXO::core/a.rs::foo", "AXO::mcp/b.rs::bar", RelationType::Calls),
            edge("AXO::mcp/b.rs::bar", "AXO::core/a.rs::foo", RelationType::Calls),
        ];
        IstGraph::build(nodes, edges)
    }

    #[test]
    fn forbidden_layer_flags_only_the_wrong_direction() {
        let g = layered_graph();
        let r = rule(
            "GUI-AXO-test1",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("AXO::core/".into()),
            NodeMatcher::Layer("AXO::mcp/".into()),
            vec![RelationType::Calls],
        );
        let v = evaluate_invariant(&g, "AXO", &r);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].source_id, "AXO::core/a.rs::foo");
        assert_eq!(v[0].target_id.as_deref(), Some("AXO::mcp/b.rs::bar"));
        assert_eq!(v[0].relation, Some(RelationType::Calls));
        assert_eq!(v[0].rule_id, "GUI-AXO-test1");
    }

    #[test]
    fn forbidden_respects_relation_filter() {
        let g = layered_graph();
        // The edges are CALLS; constrain to IMPORTS → nothing matches.
        let r = rule(
            "GUI-AXO-test2",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("AXO::core/".into()),
            NodeMatcher::Layer("AXO::mcp/".into()),
            vec![RelationType::Imports],
        );
        assert!(evaluate_invariant(&g, "AXO", &r).is_empty());
    }

    #[test]
    fn forbidden_empty_relations_matches_any() {
        let g = layered_graph();
        let r = rule(
            "GUI-AXO-test3",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("AXO::core/".into()),
            NodeMatcher::Layer("AXO::mcp/".into()),
            vec![],
        );
        assert_eq!(evaluate_invariant(&g, "AXO", &r).len(), 1);
    }

    #[test]
    fn required_flags_source_without_qualifying_edge() {
        // ClassA IMPLEMENTS IfaceX (satisfied) ; ClassB implements nothing.
        let nodes = vec![
            node("AXO::m/a.rs::ClassA", "AXO", NodeKind::Class),
            node("AXO::m/b.rs::ClassB", "AXO", NodeKind::Class),
            node("AXO::m/i.rs::IfaceX", "AXO", NodeKind::Interface),
        ];
        let edges = vec![edge(
            "AXO::m/a.rs::ClassA",
            "AXO::m/i.rs::IfaceX",
            RelationType::Implements,
        )];
        let g = IstGraph::build(nodes, edges);
        let r = rule(
            "GUI-AXO-test4",
            InvariantMode::Required,
            NodeMatcher::Kind(NodeKind::Class),
            NodeMatcher::Kind(NodeKind::Interface),
            vec![RelationType::Implements],
        );
        let v = evaluate_invariant(&g, "AXO", &r);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].source_id, "AXO::m/b.rs::ClassB");
        assert_eq!(v[0].target_id, None);
        assert_eq!(v[0].relation, None);
    }

    #[test]
    fn project_scope_excludes_other_projects() {
        let nodes = vec![
            node("PRO::core/x.rs::foo", "PRO", NodeKind::Function),
            node("PRO::mcp/y.rs::bar", "PRO", NodeKind::Function),
        ];
        let edges = vec![edge(
            "PRO::core/x.rs::foo",
            "PRO::mcp/y.rs::bar",
            RelationType::Calls,
        )];
        let g = IstGraph::build(nodes, edges);
        let r = rule(
            "GUI-AXO-test5",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("PRO::core/".into()),
            NodeMatcher::Layer("PRO::mcp/".into()),
            vec![RelationType::Calls],
        );
        // Scoped to AXO → the PRO edge is invisible.
        assert!(evaluate_invariant(&g, "AXO", &r).is_empty());
        // Unscoped (empty) → visible.
        assert_eq!(evaluate_invariant(&g, "", &r).len(), 1);
    }

    #[test]
    fn evaluate_all_concatenates_rule_violations() {
        let g = layered_graph();
        let r1 = rule(
            "GUI-AXO-a",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("AXO::core/".into()),
            NodeMatcher::Layer("AXO::mcp/".into()),
            vec![],
        );
        let r2 = rule(
            "GUI-AXO-b",
            InvariantMode::Forbidden,
            NodeMatcher::Layer("AXO::mcp/".into()),
            NodeMatcher::Layer("AXO::core/".into()),
            vec![],
        );
        let v = evaluate_all(&g, "AXO", &[r1, r2]);
        // One violation per direction = 2 total, tagged by their rule ids.
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|x| x.rule_id == "GUI-AXO-a"));
        assert!(v.iter().any(|x| x.rule_id == "GUI-AXO-b"));
    }
}
