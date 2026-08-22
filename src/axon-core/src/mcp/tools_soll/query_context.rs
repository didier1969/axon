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
                    // REQ-AXO-902455 — les trois listes de rattachement sont
                    // remplacées par les violations de règles, qui portent en
                    // plus le `rule_id` : le lecteur sait POURQUOI un nœud est
                    // signalé, et `soll_get(rule_id)` rend l'intention.
                    "declarative_rule_violation_count": snapshot.declarative_rule_violations.len(),
                    "declarative_rule_violations": snapshot
                        .declarative_rule_violations
                        .iter()
                        .map(|v| v.render())
                        .collect::<Vec<_>>(),
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
