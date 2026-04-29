use super::*;

pub(super) fn build_operational_digest(
    completeness_snapshot: Option<&SollCompletenessSnapshot>,
    entity_counts: Vec<Value>,
    last_revision_metadata: Value,
) -> Value {
    completeness_snapshot
        .map(|snapshot| {
            json!({
                "project_scope": snapshot.project_scope,
                "entity_counts": entity_counts,
                "topology_summary": {
                    "total_nodes": snapshot.total_nodes,
                    "structurally_connected": snapshot.structurally_connected(),
                    "orphan_requirement_count": snapshot.orphan_requirements.len(),
                    "orphan_requirements": snapshot.orphan_requirements,
                    "validations_without_verifies_count": snapshot.validations_without_verifies.len(),
                    "validations_without_verifies": snapshot.validations_without_verifies,
                    "decisions_without_links_count": snapshot.decisions_without_links.len(),
                    "decisions_without_links": snapshot.decisions_without_links,
                    "relation_policy_violation_count": snapshot.relation_policy_violations.len(),
                    "relation_policy_violations": snapshot.relation_policy_violations
                },
                "requirement_coverage_summary": {
                    "done": snapshot.requirement_coverage.done,
                    "partial": snapshot.requirement_coverage.partial,
                    "missing": snapshot.requirement_coverage.missing,
                    "total": snapshot.requirement_coverage.entries.len(),
                    "uncovered_requirements": snapshot.uncovered_requirements
                },
                "last_meaningful_revision": last_revision_metadata
            })
        })
        .unwrap_or(json!({
            "entity_counts": entity_counts,
            "topology_summary": Value::Null,
            "requirement_coverage_summary": Value::Null,
            "last_meaningful_revision": last_revision_metadata
        }))
}
