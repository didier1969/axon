use super::*;

impl McpServer {
    pub(crate) fn axon_soll_verify_requirements(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        let project_code = self.resolve_project_code(project_code).ok()?;
        let summary = self.requirement_coverage_summary(&project_code).ok()?;
        let snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok()?;
        let details = summary
            .entries
            .iter()
            .map(|entry| {
                let missing_dimensions_detailed = entry
                    .missing_dimensions
                    .iter()
                    .map(|dimension| requirement_dimension_descriptor(dimension))
                    .collect::<Vec<_>>();
                let next_actions_detailed = entry
                    .missing_dimensions
                    .iter()
                    .map(|dimension| {
                        let descriptor = requirement_dimension_descriptor(dimension);
                        json!({
                            "dimension": requirement_dimension_canonical_name(dimension),
                            "legacy_dimension": dimension,
                            "action": descriptor.get("next_action").cloned().unwrap_or(Value::Null),
                            "mutation_class": match dimension.as_ref() {
                                "status" | "criteria" => "update_requirement",
                                "evidence" => "attach_evidence",
                                "validation" => "link_validation",
                                "broken_file_evidence" => "repair_evidence",
                                _ => "inspect_requirement"
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "id": entry.id,
                    "state": entry.state,
                    "completion_state": entry.state,
                    "coverage_reason": requirement_state_reason(&entry.state, &entry.missing_dimensions),
                    "status": entry.status,
                    "evidence_count": entry.evidence_count,
                    "validation_count": entry.validation_count,
                    "has_criteria": entry.has_criteria,
                    "broken_file_evidence_count": entry.broken_file_evidence_count,
                    "missing_dimensions": entry.missing_dimensions,
                    "missing_dimensions_detailed": missing_dimensions_detailed,
                    "suggested_next_actions": entry.suggested_next_actions,
                    "next_actions_detailed": next_actions_detailed
                })
            })
            .collect::<Vec<_>>();
        let completion_model = json!({
            "required_dimensions": [
                requirement_dimension_descriptor("status"),
                requirement_dimension_descriptor("criteria"),
                requirement_dimension_descriptor("evidence"),
                requirement_dimension_descriptor("validation")
            ],
            "warning_dimensions": [
                requirement_dimension_descriptor("broken_file_evidence")
            ],
            "done_rule": "status is current|accepted, acceptance criteria exist, supporting evidence exists, and a validation node VERIFIES the requirement",
            "partial_rule": "some required dimensions exist but not all required dimensions are satisfied",
            "missing_rule": "required dimensions are mostly absent or requirement status is not yet operationally accepted"
        });

        Some(json!({
            "content": [{"type":"text","text": format!("Requirement verification: done={}, partial={}, missing={}", summary.done, summary.partial, summary.missing)}],
            "data": {
                "project_code": project_code,
                "done": summary.done,
                "partial": summary.partial,
                "missing": summary.missing,
                "summary": {
                    "done": summary.done,
                    "partial": summary.partial,
                    "missing": summary.missing,
                    "total": summary.entries.len()
                },
                "details": details,
                "requirements": details,
                "completion_model": completion_model,
                "completeness_axes": {
                    "concept_completeness": snapshot.concept_complete(),
                    "implementation_completeness": snapshot.implementation_complete(),
                    "evidence_ready": snapshot.evidence_ready()
                },
                "guidance_source": "server-side canonical soll completeness evaluator"
            }
        }))
    }

    pub(crate) fn build_plan_operations(&self, project_code: &str, args: &Value) -> Vec<Value> {
        let mut operations = Vec::new();

        if let Some(plan) = args.get("plan") {
            for entity in [
                "pillar",
                "requirement",
                "decision",
                "milestone",
                "vision",
                "concept",
            ] {
                if let Some(items) = plan.get(format!("{}s", entity)).and_then(|v| v.as_array()) {
                    for item in items {
                        if let Some(obj) = item.as_object() {
                            let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("");
                            let logical_key = obj
                                .get("logical_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or(title);
                            if logical_key.is_empty() {
                                continue;
                            }
                            let existing_id =
                                self.resolve_soll_id(entity, project_code, title, logical_key);
                            let kind = if existing_id.is_some() {
                                "update"
                            } else {
                                "create"
                            };
                            operations.push(json!({
                                "kind": kind,
                                "entity": entity,
                                "project_code": project_code,
                                "logical_key": logical_key,
                                "entity_id": existing_id,
                                "payload": Value::Object(obj.clone())
                            }));
                        }
                    }
                }
            }
        }

        if let Some(relations) = args.get("relations").and_then(|v| v.as_array()) {
            for rel in relations {
                if let Some(obj) = rel.as_object() {
                    operations.push(json!({
                        "kind": "link",
                        "entity": "relation",
                        "project_code": project_code,
                        "payload": Value::Object(obj.clone())
                    }));
                }
            }
        }

        operations
    }
}
