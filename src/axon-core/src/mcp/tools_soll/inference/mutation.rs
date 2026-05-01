use super::*;

#[derive(Clone, Debug)]
struct SollMutationCandidate {
    id: String,
    entity_type: String,
    title: String,
    score: usize,
    reasons: Vec<String>,
}

#[derive(Clone, Debug)]
struct SollMutationInference {
    project_code: String,
    statement: String,
    candidate_entity_type: String,
    confidence: String,
    impacted_candidates: Vec<SollMutationCandidate>,
    target_ids: Vec<String>,
    ambiguity_warnings: Vec<String>,
    proposed_operation_kind: String,
}

fn tokenize_inference_text(input: &str) -> Vec<String> {
    input
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_string())
        .collect()
}

fn preferred_entity_type_for_statement(statement: &str) -> &'static str {
    let lower = statement.to_ascii_lowercase();
    if ["constraint", "must", "should", "need", "requires", "rule"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Requirement"
    } else if [
        "decision",
        "choose",
        "adopt",
        "use ",
        "switch",
        "architecture",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "Decision"
    } else if ["guideline", "policy", "convention", "standard"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Guideline"
    } else if ["concept", "means", "term", "definition"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        "Concept"
    } else {
        "Requirement"
    }
}

fn canonical_blocker_ids(snapshot: &SollCompletenessSnapshot) -> BTreeSet<String> {
    snapshot.canonical_orphan_intent_ids()
}

impl McpServer {
    pub(crate) fn canonical_next_link_hints(&self, entity_type_cap: &str) -> Vec<Value> {
        let outgoing = allowed_relation_targets_from_source(entity_type_cap);
        if !outgoing.is_empty() {
            outgoing
        } else {
            incoming_relation_sources_for_target(entity_type_cap)
        }
    }

    fn derive_next_best_actions_from_snapshot(
        &self,
        snapshot: &SollCompletenessSnapshot,
    ) -> Vec<String> {
        if !snapshot.orphan_requirements.is_empty() {
            vec![
                "link each orphan requirement to its pillar or guideline".to_string(),
                "use `soll_relation_schema` before retrying if canonical edges are unclear"
                    .to_string(),
            ]
        } else if !snapshot.validations_without_verifies.is_empty() {
            vec![
                "attach each validation to a requirement with `VERIFIES`".to_string(),
                "rerun `soll_validate` after adding the missing proof links".to_string(),
            ]
        } else if !snapshot.uncovered_requirements.is_empty() {
            vec![
                "add acceptance criteria or evidence to uncovered requirements".to_string(),
                "use `soll_attach_evidence` or update requirement metadata".to_string(),
            ]
        } else {
            vec![
                "rerun `soll_work_plan` to open the next delivery wave".to_string(),
                "use `soll_verify_requirements` for requirement-level proof status".to_string(),
            ]
        }
    }

    pub(crate) fn mutation_feedback_payload(
        &self,
        before: &SollCompletenessSnapshot,
        after: &SollCompletenessSnapshot,
        changed_entities: Vec<Value>,
        topology_delta: Value,
    ) -> Value {
        let before_blockers = canonical_blocker_ids(before);
        let after_blockers = canonical_blocker_ids(after);
        let newly_unblocked = before_blockers
            .difference(&after_blockers)
            .cloned()
            .collect::<Vec<_>>();
        let remaining_blockers = after_blockers.into_iter().collect::<Vec<_>>();

        json!({
            "changed_entities": changed_entities,
            "topology_delta": topology_delta,
            "newly_unblocked": newly_unblocked,
            "remaining_blockers": remaining_blockers,
            "next_best_actions": self.derive_next_best_actions_from_snapshot(after),
            "completeness_before": {
                "concept_completeness": before.concept_complete(),
                "implementation_completeness": before.implementation_complete(),
                "structurally_connected": before.structurally_connected(),
                "evidence_ready": before.evidence_ready(),
                "duplicate_free": before.duplicate_free()
            },
            "completeness_after": {
                "concept_completeness": after.concept_complete(),
                "implementation_completeness": after.implementation_complete(),
                "structurally_connected": after.structurally_connected(),
                "evidence_ready": after.evidence_ready(),
                "duplicate_free": after.duplicate_free()
            },
            "guidance_source": "server-side canonical soll mutation feedback"
        })
    }

    fn infer_soll_mutation_internal(
        &self,
        project_code: &str,
        statement: &str,
    ) -> anyhow::Result<SollMutationInference> {
        let project_code = self.resolve_project_code(project_code)?;
        let preferred_type = preferred_entity_type_for_statement(statement);
        let tokens = tokenize_inference_text(statement);
        let rows_raw = self.graph_store.query_json(&format!(
            "SELECT id, type, COALESCE(title,''), COALESCE(description,'')
             FROM soll.Node
             WHERE project_code = '{}'
             ORDER BY type, id",
            escape_sql(&project_code)
        ))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        let mut candidates = rows
            .into_iter()
            .filter(|row| row.len() >= 4)
            .map(|row| {
                let haystack = format!("{} {}", row[2], row[3]).to_ascii_lowercase();
                let token_hits = tokens
                    .iter()
                    .filter(|token| haystack.contains(token.as_str()))
                    .count();
                let type_bonus = usize::from(row[1] == preferred_type) * 2;
                let score = token_hits + type_bonus;
                let mut reasons = Vec::new();
                if token_hits > 0 {
                    reasons.push(format!("matched {} statement token(s)", token_hits));
                }
                if row[1] == preferred_type {
                    reasons.push(format!("preferred entity type `{}`", preferred_type));
                }
                SollMutationCandidate {
                    id: row[0].clone(),
                    entity_type: row[1].clone(),
                    title: row[2].clone(),
                    score,
                    reasons,
                }
            })
            .filter(|candidate| candidate.score > 0)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.id.cmp(&right.id))
        });
        candidates.truncate(5);

        let target_ids = candidates
            .iter()
            .take(2)
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let confidence = if candidates
            .first()
            .is_some_and(|candidate| candidate.score >= 4)
        {
            "high"
        } else if !candidates.is_empty() {
            "medium"
        } else {
            "low"
        };
        let mut ambiguity_warnings = Vec::new();
        if candidates.is_empty() {
            ambiguity_warnings.push(
                "No existing canonical nodes matched strongly; wave 1 entrenchment will not create new entities automatically.".to_string(),
            );
        } else if candidates.len() > 1
            && candidates
                .get(1)
                .is_some_and(|candidate| candidate.score == candidates[0].score)
        {
            ambiguity_warnings.push(
                "Multiple candidate nodes scored equally; confirm target_ids explicitly before write mode.".to_string(),
            );
        }

        Ok(SollMutationInference {
            project_code,
            statement: statement.to_string(),
            candidate_entity_type: preferred_type.to_string(),
            confidence: confidence.to_string(),
            impacted_candidates: candidates.clone(),
            target_ids,
            ambiguity_warnings,
            proposed_operation_kind: if candidates.is_empty() {
                "needs_manual_scope".to_string()
            } else {
                "update_existing_entities".to_string()
            },
        })
    }

    pub(crate) fn axon_infer_soll_mutation(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|value| value.as_str())?;
        let statement = args.get("statement").and_then(|value| value.as_str())?;
        match self.infer_soll_mutation_internal(project_code, statement) {
            Ok(inference) => Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Assistive SOLL inference for `{}` suggests `{}` with {} impacted candidate(s).",
                        inference.project_code,
                        inference.proposed_operation_kind,
                        inference.impacted_candidates.len()
                    )
                }],
                "data": {
                    "project_code": inference.project_code,
                    "statement": inference.statement,
                    "candidate_entity_type": inference.candidate_entity_type,
                    "proposed_operation_kind": inference.proposed_operation_kind,
                    "confidence": inference.confidence,
                    "target_ids": inference.target_ids,
                    "impacted_candidates": inference.impacted_candidates.iter().map(|candidate| json!({
                        "id": candidate.id,
                        "entity_type": candidate.entity_type,
                        "title": candidate.title,
                        "score": candidate.score,
                        "reasons": candidate.reasons
                    })).collect::<Vec<_>>(),
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": if inference.impacted_candidates.is_empty() {
                        vec![
                            "inspect the current SOLL context and choose explicit target_ids".to_string(),
                            "create or update canonical nodes manually with `soll_manager` if the nuance truly requires a new entity".to_string()
                        ]
                    } else {
                        vec![
                            "confirm the target_ids and call `entrench_nuance` with `confirm=true`".to_string(),
                            "override target_ids explicitly if the proposed scope is too broad".to_string()
                        ]
                    }
                }
            })),
            Err(error) => Some(json!({
                "content": [{ "type": "text", "text": format!("Inference failed: {}", error) }],
                "isError": true
            })),
        }
    }

    pub(crate) fn axon_entrench_nuance(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|value| value.as_str())?;
        let statement = args.get("statement").and_then(|value| value.as_str())?;
        let confirm = args
            .get("confirm")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        // REQ-AXO-043 — pre-validate project_code so we surface the
        // structured wrong_project_scope contract (matching
        // soll_query_context / soll_work_plan / anomalies) instead of a
        // bare "Entrenchment failed: <anyhow>" message.
        if self.resolve_project_code(project_code).is_err() {
            let registered: Vec<String> = self
                .graph_store
                .query_json(
                    "SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code",
                )
                .ok()
                .and_then(|s| serde_json::from_str::<Vec<Vec<String>>>(&s).ok())
                .map(|rows| rows.into_iter().filter_map(|r| r.into_iter().next()).collect())
                .unwrap_or_default();
            let registered_values: Vec<Value> =
                registered.iter().map(|c| Value::from(c.clone())).collect();
            let next_action = if registered.is_empty() {
                "no projects registered yet — use axon_init_project to register one".to_string()
            } else {
                format!(
                    "use one of the registered project_codes: {}",
                    registered.join(", ")
                )
            };
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Project `{}` not found in registry for entrench_nuance. {}",
                        project_code, next_action,
                    ),
                }],
                "isError": true,
                "data": {
                    "status": "wrong_project_scope",
                    "rejected_project_code": project_code,
                    "registered_project_codes": registered_values,
                    "next_action": next_action,
                    "operator_guidance": {
                        "problem_class": "wrong_project_scope",
                        "likely_cause": "project_code_not_in_registry",
                        "next_best_actions": [
                            "retry with a registered project_code",
                            "or call axon_init_project to register a new project",
                        ],
                        "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                        "confidence": "high",
                    },
                }
            }));
        }

        let inference = match self.infer_soll_mutation_internal(project_code, statement) {
            Ok(inference) => inference,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let explicit_target_ids = args
            .get("target_ids")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let target_ids = if explicit_target_ids.is_empty() {
            inference.target_ids.clone()
        } else {
            explicit_target_ids.clone()
        };

        if !confirm {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": "Entrenchment proposal only. Re-run with `confirm=true` to apply bounded updates on the selected canonical entities."
                }],
                "data": {
                    "project_code": inference.project_code,
                    "statement": inference.statement,
                    "confirm_required": true,
                    "candidate_entity_type": inference.candidate_entity_type,
                    "proposed_operation_kind": inference.proposed_operation_kind,
                    "target_ids": target_ids,
                    "impacted_candidates": inference.impacted_candidates.iter().map(|candidate| json!({
                        "id": candidate.id,
                        "entity_type": candidate.entity_type,
                        "title": candidate.title,
                        "score": candidate.score,
                        "reasons": candidate.reasons
                    })).collect::<Vec<_>>(),
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": vec![
                        "review the proposed target_ids".to_string(),
                        "rerun with `confirm=true` once the scope is explicit".to_string()
                    ]
                }
            }));
        }

        if target_ids.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Wave 1 entrenchment cannot write without explicit or inferred existing target_ids." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": [],
                    "next_best_actions": [
                        "call `infer_soll_mutation` to inspect impacted nodes",
                        "provide `target_ids` explicitly or use `soll_manager` for manual graph changes"
                    ]
                }
            }));
        }

        if explicit_target_ids.is_empty() && !inference.ambiguity_warnings.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Entrenchment confirmation refused because the inferred scope is still ambiguous. Provide explicit `target_ids` first." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": target_ids,
                    "ambiguity_warnings": inference.ambiguity_warnings,
                    "next_best_actions": [
                        "review the impacted_candidates returned by `infer_soll_mutation`",
                        "rerun `entrench_nuance` with explicit `target_ids` once the scope is fully explicit"
                    ]
                }
            }));
        }

        let cross_project_targets = target_ids
            .iter()
            .filter(|target_id| {
                project_code_from_canonical_entity_id(target_id)
                    .is_none_or(|candidate_project| candidate_project != inference.project_code)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !cross_project_targets.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "Entrenchment confirmation refused because some target_ids do not belong to the requested project_code." }],
                "isError": true,
                "data": {
                    "project_code": inference.project_code,
                    "confirm_required": false,
                    "target_ids": target_ids,
                    "invalid_target_ids": cross_project_targets,
                    "next_best_actions": [
                        "use only canonical IDs that belong to the requested project_code",
                        "re-run `infer_soll_mutation` if the intended scope is uncertain"
                    ]
                }
            }));
        }

        let before = match self.soll_completeness_snapshot(Some(&inference.project_code)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment baseline failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let mut changed_entities = Vec::new();
        for target_id in &target_ids {
            let row = match self.query_named_row(
                &format!(
                    "SELECT title, description, status, metadata FROM soll.Node WHERE id = '{}'",
                    escape_sql(target_id)
                ),
                4,
            ) {
                Ok(row) => row,
                Err(error) => {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!("Entrenchment target lookup failed for `{}`: {}", target_id, error) }],
                        "isError": true
                    }))
                }
            };
            let mut metadata: Value = serde_json::from_str(&row[3]).unwrap_or(json!({}));
            let entry = json!({
                "statement": statement,
                "source": "entrench_nuance",
                "entrenched_at": now_unix_ms()
            });
            if !metadata
                .get("nuances")
                .is_some_and(|value| value.is_array())
            {
                metadata["nuances"] = json!([]);
            }
            if let Some(items) = metadata
                .get_mut("nuances")
                .and_then(|value| value.as_array_mut())
            {
                items.push(entry);
            }
            metadata["updated_at"] = json!(now_unix_ms());

            if let Err(error) = self.graph_store.execute_param(
                "UPDATE soll.Node SET metadata = ? WHERE id = ?",
                &json!([metadata.to_string(), target_id]),
            ) {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment update failed for `{}`: {}", target_id, error) }],
                    "isError": true
                }));
            }

            changed_entities.push(json!({
                "id": target_id,
                "change_kind": "metadata_update",
                "fields": ["metadata.nuances", "metadata.updated_at"]
            }));
        }

        let after = match self.soll_completeness_snapshot(Some(&inference.project_code)) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Entrenchment follow-up failed: {}", error) }],
                    "isError": true
                }))
            }
        };

        let mutation_feedback = self.mutation_feedback_payload(
            &before,
            &after,
            changed_entities.clone(),
            json!({
                "nodes_created": 0,
                "nodes_updated": changed_entities.len(),
                "edges_created": 0
            }),
        );

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Nuance entrenched across {} canonical node(s).",
                    changed_entities.len()
                )
            }],
            "data": {
                "project_code": inference.project_code,
                "statement": inference.statement,
                "target_ids": target_ids,
                "changed_entities": changed_entities,
                "mutation_feedback": mutation_feedback
            }
        }))
    }
}
