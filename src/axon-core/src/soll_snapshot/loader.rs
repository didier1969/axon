//! Load a full project snapshot from the SOLL backing store (REQ-AXO-322).
//!
//! Three round-trips against `soll.Node`, `soll.Edge`, `soll.Traceability`
//! filtered by `project_code`. Cost on the live AXO project (~920 nodes /
//! ~2-3k edges / ~900 traceability rows) is well under 100 ms even on a
//! cold cache; subsequent reads hit RAM only.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::graph::GraphStore;

use super::snapshot::{SnapshotEdge, SnapshotNode, SnapshotTraceability, SollSnapshot};

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

pub fn load_snapshot(
    store: &Arc<GraphStore>,
    project_code: &str,
    generation: u64,
) -> Result<SollSnapshot> {
    let project_code_escaped = escape_sql(project_code);

    let node_query = format!(
        "SELECT id, type, COALESCE(title, ''), COALESCE(status, ''), COALESCE(metadata::text, '{{}}') \
         FROM soll.Node \
         WHERE project_code = '{}'",
        project_code_escaped
    );
    let node_rows_raw = store
        .query_json(&node_query)
        .with_context(|| format!("snapshot: load nodes for project={}", project_code))?;
    let node_rows: Vec<Vec<String>> = serde_json::from_str(&node_rows_raw).unwrap_or_default();
    let mut nodes: HashMap<String, SnapshotNode> = HashMap::with_capacity(node_rows.len());
    for row in node_rows {
        if row.len() < 5 {
            continue;
        }
        let id = row[0].clone();
        nodes.insert(
            id.clone(),
            SnapshotNode {
                id,
                entity_type: row[1].clone(),
                title: row[2].clone(),
                status: row[3].clone(),
                metadata_raw: row[4].clone(),
            },
        );
    }

    // Edges where either endpoint is anchored in this project. The
    // `project_code` columns on soll.Edge would be cleaner but the
    // existing schema relies on LIKE prefixes; mirror that to avoid
    // missing cross-type edges.
    let edge_query = format!(
        "SELECT source_id, target_id, relation_type \
         FROM soll.Edge \
         WHERE source_id LIKE '%-{}-%' OR target_id LIKE '%-{}-%'",
        project_code_escaped, project_code_escaped
    );
    let edge_rows_raw = store
        .query_json(&edge_query)
        .with_context(|| format!("snapshot: load edges for project={}", project_code))?;
    let edge_rows: Vec<Vec<String>> = serde_json::from_str(&edge_rows_raw).unwrap_or_default();
    let mut edges: Vec<SnapshotEdge> = Vec::with_capacity(edge_rows.len());
    for row in edge_rows {
        if row.len() < 3 {
            continue;
        }
        edges.push(SnapshotEdge {
            source_id: row[0].clone(),
            target_id: row[1].clone(),
            relation_type: row[2].clone(),
        });
    }

    // Traceability: filter by entity_id prefix matching project code
    // (e.g. 'REQ-AXO-%', 'DEC-AXO-%', ...). The `artifact_status`
    // column was added by REQ-AXO-320 (additive DDL, idempotent on
    // bootstrap). Fall back to empty string if a row predates that
    // migration and has NULL.
    let trace_query = format!(
        "SELECT id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, \
                COALESCE(artifact_status, '') \
         FROM soll.Traceability \
         WHERE soll_entity_id LIKE '%-{}-%'",
        project_code_escaped
    );
    let trace_rows_raw = store
        .query_json(&trace_query)
        .with_context(|| format!("snapshot: load traceability for project={}", project_code))?;
    let trace_rows: Vec<Vec<String>> = serde_json::from_str(&trace_rows_raw).unwrap_or_default();
    let mut traceability: Vec<SnapshotTraceability> = Vec::with_capacity(trace_rows.len());
    for row in trace_rows {
        if row.len() < 6 {
            continue;
        }
        traceability.push(SnapshotTraceability {
            id: row[0].clone(),
            soll_entity_type: row[1].clone(),
            soll_entity_id: row[2].clone(),
            artifact_type: row[3].clone(),
            artifact_ref: row[4].clone(),
            artifact_status: row[5].clone(),
        });
    }

    Ok(SollSnapshot::build(
        project_code,
        generation,
        nodes,
        edges,
        traceability,
    ))
}
