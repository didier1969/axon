//! Data structures mirrored from the SOLL graph (REQ-AXO-322).

use std::collections::HashMap;

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
        for t in &traceability {
            let key = format!(
                "{}::{}",
                t.soll_entity_type.to_ascii_lowercase(),
                t.soll_entity_id
            );
            *traceability_count_by_entity.entry(key).or_insert(0) += 1;
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
        }
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
        nodes.insert("REQ-AXO-001".to_string(), mk_node("REQ-AXO-001", "Requirement"));
        nodes.insert("REQ-AXO-002".to_string(), mk_node("REQ-AXO-002", "Requirement"));
        nodes.insert("DEC-AXO-001".to_string(), mk_node("DEC-AXO-001", "Decision"));
        nodes.insert("MIL-AXO-001".to_string(), mk_node("MIL-AXO-001", "Milestone"));

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
