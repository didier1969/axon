//! Strategic Relevance Signal (SRS) — REQ-AXO-901751 slice 1.
//!
//! Core detection engine: given an IST artifact (symbol/file), traverse
//! evidence edges to SOLL. If any linked SOLL node is superseded, infer
//! the replacement strategy and return a structured `LegacyProximity`.

use crate::soll_snapshot::SollSnapshot;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegacyStrategy {
    RadicalComplete,
    ProgressiveActive,
    DeprecatedRetained,
    Abandoned,
}

impl LegacyStrategy {
    pub fn direction_hint(&self) -> &'static str {
        match self {
            Self::RadicalComplete => "do not resurrect",
            Self::ProgressiveActive => "work toward successor",
            Self::DeprecatedRetained => "do not modify, orient new usage toward successor",
            Self::Abandoned => "dead code, delete if touched",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LegacyNode {
    pub id: String,
    pub strategy: LegacyStrategy,
    pub successor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LegacyProximity {
    pub nodes: Vec<LegacyNode>,
    pub direction: String,
    pub confidence: String,
}

/// Given an IST artifact reference (file path or symbol name), traverse
/// the SOLL traceability table to find linked superseded nodes and infer
/// the replacement strategy for each.
///
/// Operates entirely on the in-memory `SollSnapshot` — no DB round-trip.
/// Designed for < 5 ms latency on typical project sizes.
pub fn detect_legacy_proximity(
    artifact_ref: &str,
    snapshot: &SollSnapshot,
) -> Option<LegacyProximity> {
    let artifact_lower = artifact_ref.to_ascii_lowercase();

    // Step 1: find SOLL entities linked to this artifact via traceability.
    let mut linked_entity_ids: Vec<String> = Vec::new();
    for trace in &snapshot.traceability {
        if trace.artifact_ref.to_ascii_lowercase().contains(&artifact_lower)
            || artifact_lower.contains(&trace.artifact_ref.to_ascii_lowercase())
        {
            if !linked_entity_ids.contains(&trace.soll_entity_id) {
                linked_entity_ids.push(trace.soll_entity_id.clone());
            }
        }
    }

    // Step 2: filter to superseded SOLL nodes only.
    let mut legacy_nodes: Vec<LegacyNode> = Vec::new();
    for entity_id in &linked_entity_ids {
        let Some(node) = snapshot.nodes.get(entity_id) else {
            continue;
        };
        if node.status != "superseded" {
            continue;
        }

        let strategy = infer_strategy(entity_id, node, snapshot);
        let successor = find_successor(entity_id, snapshot);
        let superseded_at = extract_updated_at(&node.metadata_raw);

        legacy_nodes.push(LegacyNode {
            id: entity_id.clone(),
            strategy,
            successor,
            superseded_at,
        });
    }

    if legacy_nodes.is_empty() {
        return None;
    }

    let confidence = if legacy_nodes.iter().all(|n| n.successor.is_some()) {
        "high"
    } else {
        "medium"
    };
    let direction = legacy_nodes
        .first()
        .map(|n| n.strategy.direction_hint())
        .unwrap_or("review legacy linkage")
        .to_string();

    Some(LegacyProximity {
        nodes: legacy_nodes,
        direction,
        confidence: confidence.to_string(),
    })
}

fn find_successor(superseded_id: &str, snapshot: &SollSnapshot) -> Option<String> {
    // SUPERSEDES edge: source SUPERSEDES target.
    // The superseded node is the TARGET. The successor is the SOURCE.
    snapshot
        .incoming_edges(superseded_id)
        .find(|(_src, rel)| *rel == "SUPERSEDES")
        .map(|(src, _)| src.to_string())
}

fn infer_strategy(
    entity_id: &str,
    node: &crate::soll_snapshot::SnapshotNode,
    snapshot: &SollSnapshot,
) -> LegacyStrategy {
    let successor_id = find_successor(entity_id, snapshot);

    let Some(ref succ_id) = successor_id else {
        return LegacyStrategy::Abandoned;
    };

    if has_deprecated_tag(&node.metadata_raw) {
        return LegacyStrategy::DeprecatedRetained;
    }

    let successor_status = snapshot
        .nodes
        .get(succ_id.as_str())
        .map(|n| n.status.as_str())
        .unwrap_or("");
    let successor_terminal =
        successor_status == "delivered" || successor_status == "completed";

    // IST residual: count traceability rows for the superseded entity.
    let entity_type_lower = node.entity_type.to_ascii_lowercase();
    let residual_count = snapshot.traceability_count_for(&entity_type_lower, entity_id);

    if successor_terminal && residual_count == 0 {
        LegacyStrategy::RadicalComplete
    } else {
        LegacyStrategy::ProgressiveActive
    }
}

fn has_deprecated_tag(metadata_raw: &str) -> bool {
    let Ok(meta) = serde_json::from_str::<serde_json::Value>(metadata_raw) else {
        return false;
    };
    if let Some(tags) = meta.get("tags").and_then(|v| v.as_str()) {
        return tags
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("deprecated"));
    }
    if let Some(tags) = meta.get("tags").and_then(|v| v.as_array()) {
        return tags.iter().any(|t| {
            t.as_str()
                .map(|s| s.eq_ignore_ascii_case("deprecated"))
                .unwrap_or(false)
        });
    }
    false
}

fn extract_updated_at(metadata_raw: &str) -> Option<i64> {
    let meta: serde_json::Value = serde_json::from_str(metadata_raw).ok()?;
    meta.get("updated_at")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soll_snapshot::{SnapshotEdge, SnapshotNode, SnapshotTraceability, SollSnapshot};
    use std::collections::HashMap;

    fn mk_node(id: &str, ty: &str, status: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: ty.to_string(),
            title: format!("title-{}", id),
            status: status.to_string(),
            metadata_raw: "{}".to_string(),
        }
    }

    fn mk_node_with_meta(id: &str, ty: &str, status: &str, meta: &str) -> SnapshotNode {
        SnapshotNode {
            id: id.to_string(),
            entity_type: ty.to_string(),
            title: format!("title-{}", id),
            status: status.to_string(),
            metadata_raw: meta.to_string(),
        }
    }

    fn mk_edge(src: &str, tgt: &str, rel: &str) -> SnapshotEdge {
        SnapshotEdge {
            source_id: src.to_string(),
            target_id: tgt.to_string(),
            relation_type: rel.to_string(),
        }
    }

    fn mk_trace(entity_type: &str, entity_id: &str, artifact_ref: &str) -> SnapshotTraceability {
        SnapshotTraceability {
            id: format!("T-{}-{}", entity_id, artifact_ref.len()),
            soll_entity_type: entity_type.to_string(),
            soll_entity_id: entity_id.to_string(),
            artifact_type: "file".to_string(),
            artifact_ref: artifact_ref.to_string(),
            artifact_status: "ok".to_string(),
        }
    }

    fn build_snapshot(
        nodes: Vec<SnapshotNode>,
        edges: Vec<SnapshotEdge>,
        traces: Vec<SnapshotTraceability>,
    ) -> SollSnapshot {
        let node_map: HashMap<String, SnapshotNode> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        SollSnapshot::build("AXO", 1, node_map, edges, traces)
    }

    #[test]
    fn no_traceability_returns_none() {
        let snapshot = build_snapshot(
            vec![mk_node("DEC-AXO-001", "Decision", "superseded")],
            vec![],
            vec![],
        );
        assert!(detect_legacy_proximity("src/old_module.rs", &snapshot).is_none());
    }

    #[test]
    fn current_node_returns_none() {
        let snapshot = build_snapshot(
            vec![mk_node("DEC-AXO-001", "Decision", "current")],
            vec![],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/module.rs")],
        );
        assert!(detect_legacy_proximity("src/module.rs", &snapshot).is_none());
    }

    #[test]
    fn abandoned_no_successor() {
        let snapshot = build_snapshot(
            vec![mk_node("DEC-AXO-001", "Decision", "superseded")],
            vec![],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/old_backend.rs")],
        );
        let result = detect_legacy_proximity("src/old_backend.rs", &snapshot);
        assert!(result.is_some());
        let prox = result.unwrap();
        assert_eq!(prox.nodes.len(), 1);
        assert_eq!(prox.nodes[0].strategy, LegacyStrategy::Abandoned);
        assert!(prox.nodes[0].successor.is_none());
        assert_eq!(prox.confidence, "medium");
    }

    #[test]
    fn progressive_active_successor_current_with_residual() {
        let snapshot = build_snapshot(
            vec![
                mk_node("DEC-AXO-001", "Decision", "superseded"),
                mk_node("DEC-AXO-002", "Decision", "current"),
            ],
            vec![mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES")],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/old_backend.rs")],
        );
        let result = detect_legacy_proximity("src/old_backend.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].strategy, LegacyStrategy::ProgressiveActive);
        assert_eq!(result.nodes[0].successor.as_deref(), Some("DEC-AXO-002"));
        assert_eq!(result.confidence, "high");
    }

    #[test]
    fn progressive_active_successor_delivered_but_residual_exists() {
        // Successor is delivered but the superseded node still has traceability
        // (IST residual) → progressive_active, not radical_complete.
        let snapshot = build_snapshot(
            vec![
                mk_node("DEC-AXO-001", "Decision", "superseded"),
                mk_node("DEC-AXO-002", "Decision", "delivered"),
            ],
            vec![mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES")],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/old_backend.rs")],
        );
        let result = detect_legacy_proximity("src/old_backend.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].strategy, LegacyStrategy::ProgressiveActive);
    }

    #[test]
    fn radical_complete_successor_delivered_zero_residual() {
        // Successor is delivered and superseded node has no traceability
        // (zero IST residual). We find it via a different artifact that
        // matches but the superseded entity itself has no trace rows.
        let snapshot = build_snapshot(
            vec![
                mk_node("DEC-AXO-001", "Decision", "superseded"),
                mk_node("DEC-AXO-002", "Decision", "delivered"),
                mk_node("REQ-AXO-010", "Requirement", "current"),
            ],
            vec![
                mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES"),
                mk_edge("DEC-AXO-001", "REQ-AXO-010", "SOLVES"),
            ],
            // Traceability links the *requirement* to the artifact, not
            // the superseded decision directly. The decision has zero
            // traceability rows.
            vec![mk_trace(
                "Requirement",
                "REQ-AXO-010",
                "src/old_backend.rs",
            )],
        );
        // REQ-AXO-010 is current → no legacy proximity from it.
        // DEC-AXO-001 is superseded but has no traceability to this
        // artifact → not found via IST→SOLL. Returns None.
        let result = detect_legacy_proximity("src/old_backend.rs", &snapshot);
        assert!(result.is_none());
    }

    #[test]
    fn radical_complete_via_direct_trace_zero_sibling_residual() {
        // Edge case: artifact linked to superseded node via traceability,
        // but no OTHER traceability rows exist for the superseded node.
        // The match itself counts as 1 row in traceability_count_for.
        // So radical_complete requires exactly 0 traceability rows for
        // the superseded entity — meaning the artifact we found was
        // linked to a *different* entity, not the superseded one.
        //
        // For IST→SOLL direction: if we found the artifact linked to a
        // superseded node, by definition there IS residual (count ≥ 1).
        // Radical_complete is unreachable in pure IST→SOLL traversal.
        // It's exposed for SOLL→IST panoramic use (slice 5).
        let snapshot = build_snapshot(
            vec![
                mk_node("DEC-AXO-001", "Decision", "superseded"),
                mk_node("DEC-AXO-002", "Decision", "delivered"),
            ],
            vec![mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES")],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/removed_file.rs")],
        );
        // The artifact IS linked → count=1 → progressive_active
        let result = detect_legacy_proximity("src/removed_file.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].strategy, LegacyStrategy::ProgressiveActive);
    }

    #[test]
    fn deprecated_retained_with_tag() {
        let snapshot = build_snapshot(
            vec![
                mk_node_with_meta(
                    "DEC-AXO-001",
                    "Decision",
                    "superseded",
                    r#"{"tags": "deprecated, compat"}"#,
                ),
                mk_node("DEC-AXO-002", "Decision", "current"),
            ],
            vec![mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES")],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/compat_layer.rs")],
        );
        let result = detect_legacy_proximity("src/compat_layer.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].strategy, LegacyStrategy::DeprecatedRetained);
        assert_eq!(result.nodes[0].successor.as_deref(), Some("DEC-AXO-002"));
    }

    #[test]
    fn deprecated_retained_with_array_tags() {
        let snapshot = build_snapshot(
            vec![
                mk_node_with_meta(
                    "DEC-AXO-001",
                    "Decision",
                    "superseded",
                    r#"{"tags": ["deprecated", "compat"]}"#,
                ),
                mk_node("DEC-AXO-002", "Decision", "current"),
            ],
            vec![mk_edge("DEC-AXO-002", "DEC-AXO-001", "SUPERSEDES")],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/compat_layer.rs")],
        );
        let result = detect_legacy_proximity("src/compat_layer.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].strategy, LegacyStrategy::DeprecatedRetained);
    }

    #[test]
    fn multiple_superseded_nodes() {
        let snapshot = build_snapshot(
            vec![
                mk_node("DEC-AXO-001", "Decision", "superseded"),
                mk_node("DEC-AXO-002", "Decision", "superseded"),
                mk_node("DEC-AXO-003", "Decision", "current"),
            ],
            vec![
                mk_edge("DEC-AXO-003", "DEC-AXO-001", "SUPERSEDES"),
                // DEC-002 has no successor → abandoned
            ],
            vec![
                mk_trace("Decision", "DEC-AXO-001", "src/shared_module.rs"),
                mk_trace("Decision", "DEC-AXO-002", "src/shared_module.rs"),
            ],
        );
        let result = detect_legacy_proximity("src/shared_module.rs", &snapshot).unwrap();
        assert_eq!(result.nodes.len(), 2);
        let strategies: Vec<&LegacyStrategy> =
            result.nodes.iter().map(|n| &n.strategy).collect();
        assert!(strategies.contains(&&LegacyStrategy::ProgressiveActive));
        assert!(strategies.contains(&&LegacyStrategy::Abandoned));
    }

    #[test]
    fn case_insensitive_artifact_match() {
        let snapshot = build_snapshot(
            vec![mk_node("DEC-AXO-001", "Decision", "superseded")],
            vec![],
            vec![mk_trace(
                "Decision",
                "DEC-AXO-001",
                "src/OldBackend.rs",
            )],
        );
        let result = detect_legacy_proximity("src/oldbackend.rs", &snapshot);
        assert!(result.is_some());
    }

    #[test]
    fn partial_path_match() {
        let snapshot = build_snapshot(
            vec![mk_node("DEC-AXO-001", "Decision", "superseded")],
            vec![],
            vec![mk_trace(
                "Decision",
                "DEC-AXO-001",
                "src/old_backend/mod.rs",
            )],
        );
        // Symbol query "old_backend" should match the traceability ref
        let result = detect_legacy_proximity("old_backend", &snapshot);
        assert!(result.is_some());
    }

    #[test]
    fn superseded_at_from_metadata() {
        let snapshot = build_snapshot(
            vec![mk_node_with_meta(
                "DEC-AXO-001",
                "Decision",
                "superseded",
                r#"{"updated_at": 1779000000000}"#,
            )],
            vec![],
            vec![mk_trace("Decision", "DEC-AXO-001", "src/old.rs")],
        );
        let result = detect_legacy_proximity("src/old.rs", &snapshot).unwrap();
        assert_eq!(result.nodes[0].superseded_at, Some(1779000000000));
    }

    #[test]
    fn latency_within_budget() {
        // Construct a moderately sized snapshot (~1000 nodes, ~2000 edges,
        // ~500 traceability rows) and verify detection completes in < 5ms.
        let mut nodes = HashMap::new();
        let mut edges = Vec::new();
        let mut traces = Vec::new();

        for i in 0..500 {
            let id = format!("REQ-AXO-{:04}", i);
            let status = if i % 50 == 0 { "superseded" } else { "current" };
            nodes.insert(
                id.clone(),
                mk_node(&id, "Requirement", status),
            );
        }
        for i in 0..500 {
            let id = format!("DEC-AXO-{:04}", i);
            nodes.insert(id.clone(), mk_node(&id, "Decision", "current"));
        }
        for i in 0..1000 {
            edges.push(mk_edge(
                &format!("REQ-AXO-{:04}", i % 500),
                &format!("DEC-AXO-{:04}", i % 500),
                "SOLVES",
            ));
        }
        for i in 0..500 {
            traces.push(mk_trace(
                "Requirement",
                &format!("REQ-AXO-{:04}", i),
                &format!("src/module_{:04}.rs", i),
            ));
        }

        let snapshot = SollSnapshot::build("AXO", 1, nodes, edges, traces);
        let start = std::time::Instant::now();
        let _result = detect_legacy_proximity("src/module_0000.rs", &snapshot);
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 5,
            "detection took {}ms, budget is 5ms",
            elapsed.as_millis()
        );
    }
}
