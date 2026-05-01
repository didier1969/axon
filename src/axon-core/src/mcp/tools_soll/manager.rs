use super::*;

impl McpServer {
    /// REQ-AXO-125 — normalize writer errors so the LLM-visible text
    /// contains only the action kind, category, and a recovery hint —
    /// never the raw SQL or DuckDB internals (which previously leaked
    /// the partially-substituted INSERT statement and bound metadata
    /// JSON to the caller). The full error is surfaced under
    /// `data.diagnostic_excerpt` (truncated to keep response small)
    /// for clients that explicitly want to inspect it.
    fn normalized_soll_writer_error(
        action: &'static str,
        e: anyhow::Error,
    ) -> serde_json::Value {
        let raw = format!("{}", e);
        let category = if raw.contains("Writer Error") || raw.contains("INSERT INTO") {
            "duckdb_writer"
        } else if raw.contains("forbidden_relation") || raw.contains("No canonical relation allowed") {
            "forbidden_relation"
        } else if raw.contains("not found") {
            "target_not_found"
        } else if raw.contains("Unknown id kind") {
            "registry_unknown_id_kind"
        } else {
            "unknown"
        };
        let recovery = match category {
            "duckdb_writer" => {
                "If your title/description contains literal `?` characters, strip them and retry — REQ-AXO-091 placeholder bug is fixed in the dev tree but the live brain still ships the pre-fix binary until promotion. Otherwise check that the data fits the column constraints (id collision, schema drift, missing project_code)."
            }
            "forbidden_relation" => {
                "Use a canonical relation type: REQ -BELONGS_TO-> PIL, CPT -EXPLAINS-> REQ, DEC -SOLVES/IMPACTS-> REQ, PIL -EPITOMIZES-> VIS. Run `soll_relation_schema` to discover allowed pairs."
            }
            "target_not_found" => {
                "Verify the target id exists via `cypher SELECT id FROM soll.main.Node WHERE id = '<id>'`. If the id was just created, ensure it was committed."
            }
            "registry_unknown_id_kind" => {
                "Use one of the canonical entity types: vision, pillar, requirement, concept, decision, milestone, stakeholder, validation, guideline."
            }
            _ => "Inspect data.diagnostic_excerpt for the underlying writer error.",
        };
        let mut excerpt = raw.clone();
        if excerpt.len() > 240 {
            excerpt.truncate(240);
            excerpt.push_str("...");
        }
        // Strip newlines so the excerpt fits inline.
        let excerpt = excerpt.replace('\n', " ").replace("  ", " ");
        let kind = format!("{}_failed", action);
        let visible_text = format!("soll_manager {action} failed ({category}). {recovery}");
        serde_json::json!({
            "content": [{ "type": "text", "text": visible_text }],
            "isError": true,
            "data": {
                "kind": kind,
                "category": category,
                "next_action": recovery,
                "diagnostic_excerpt": excerpt,
                "operator_guidance": {
                    "problem_class": kind,
                    "follow_up_tools": ["cypher", "soll_relation_schema", "project_registry_lookup"],
                    "confidence": "high"
                }
            }
        })
    }

    /// REQ-AXO-043 / REQ-AXO-125 — strip SQL leakage from link errors
    /// while preserving the existing flat `data.relation_guidance` shape
    /// that MCP callers depend on. The `link` path historically returned
    /// `format!("Link error: {}", e)` which exposed DuckDB writer error
    /// text including raw INSERT statements. Here we detect the writer
    /// pattern and substitute a classified message; non-SQL errors (e.g.
    /// `Cardinality conflict`, `Relation X not found in canonical
    /// policy`) keep their human-readable form.
    pub(super) fn sanitized_link_error_text(e: &anyhow::Error) -> String {
        let raw = format!("{}", e);
        if raw.contains("Writer Error") || raw.contains("INSERT INTO") {
            return "Link error: duckdb writer rejected the edge insert. Verify both endpoints exist via `cypher SELECT id FROM soll.main.Node WHERE id IN ('<src>','<tgt>')` and that the relation_type is allowed for the pair via `soll_relation_schema`.".to_string();
        }
        format!("Link error: {}", raw)
    }

    fn classify_attach_status_from_error(&self, error_text: &str) -> &'static str {
        if error_text.contains("Explicit relation required") {
            "needs_relation_hint"
        } else if error_text.contains("not found") {
            "invalid_target_id"
        } else if error_text.contains("\"error\":\"forbidden_relation\"")
            || error_text.contains("No canonical relation allowed")
        {
            "forbidden_relation"
        } else {
            "attach_failed"
        }
    }

    pub(crate) fn axon_soll_manager(&self, args: &Value) -> Option<Value> {
        let action = args.get("action")?.as_str()?;
        let entity = args.get("entity")?.as_str()?;
        let data = args.get("data")?;

        match action {
            "create" => {
                // REQ-AXO-092 / REQ-AXO-043 — validate entity BEFORE the registry
                // resolves an ID. Otherwise an unknown entity surfaces as a generic
                // "Registry error: Unknown id kind" which buries the schema mismatch.
                let entity_type_cap = match entity {
                    "vision" => "Vision",
                    "pillar" => "Pillar",
                    "requirement" => "Requirement",
                    "concept" => "Concept",
                    "decision" => "Decision",
                    "milestone" => "Milestone",
                    "stakeholder" => "Stakeholder",
                    "validation" => "Validation",
                    "guideline" => "Guideline",
                    other => {
                        let accepted = [
                            "vision", "pillar", "requirement", "concept", "decision",
                            "milestone", "stakeholder", "validation", "guideline",
                        ];
                        return Some(json!({
                            "content": [{
                                "type": "text",
                                "text": format!(
                                    "Unknown entity: `{}`. Use one of: {}.",
                                    other,
                                    accepted.join(", "),
                                ),
                            }],
                            "isError": true,
                            "data": {
                                "status": "input_invalid",
                                "rejected_entity": other,
                                "accepted_entities": accepted,
                                "next_action": format!(
                                    "retry with one of: {}",
                                    accepted.join(", "),
                                ),
                                "operator_guidance": {
                                    "problem_class": "input_invalid",
                                    "likely_cause": "entity_not_in_schema_enum",
                                    "next_best_actions": [
                                        format!("retry with `entity` in: {}", accepted.join(", ")),
                                    ],
                                    "confidence": "high",
                                },
                            },
                        }));
                    }
                };

                let project_code_raw = args
                    .get("project_code")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("project_code").and_then(|v| v.as_str()))
                    .map(str::trim);
                let project_code = match self.require_registered_mutation_project_code(
                    project_code_raw,
                    "soll_manager create",
                ) {
                    Ok(code) => code,
                    Err(e) => {
                        return Some(json!({
                            "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                            "isError": true
                        }))
                    }
                };
                let before_snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok();
                let reserved_id = args.get("reserved_id").and_then(|value| value.as_str());
                let (_requested_code, canonical_code, formatted_id) = if let Some(reserved_id) =
                    reserved_id
                {
                    match self.resolve_canonical_project_identity_for_mutation(&project_code) {
                        Ok((canonical_code, project_code)) => {
                            (canonical_code, project_code, reserved_id.to_string())
                        }
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Registry error: {}", e) }], "isError": true }),
                            )
                        }
                    }
                } else {
                    match self.next_soll_numeric_id(&project_code, entity) {
                        Ok((canonical_code, project_code, prefix, next_num)) => (
                            canonical_code,
                            project_code.clone(),
                            format!("{}-{}-{:03}", prefix, project_code, next_num),
                        ),
                        Err(e) => {
                            return Some(
                                json!({ "content": [{ "type": "text", "text": format!("Registry error: {}", e) }], "isError": true }),
                            )
                        }
                    }
                };

                let mut meta = data.get("metadata").cloned().unwrap_or(json!({}));
                let title = data
                    .get("title")
                    .or_else(|| data.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let description = data
                    .get("description")
                    .or_else(|| data.get("explanation"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let status = if entity == "validation" {
                    data.get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or(status)
                } else {
                    status
                };

                if let Some(goal) = data.get("goal") {
                    meta["goal"] = goal.clone();
                }
                if let Some(priority) = data.get("priority") {
                    meta["priority"] = priority.clone();
                }
                if let Some(owner) = data.get("owner") {
                    meta["owner"] = owner.clone();
                }
                if let Some(ac) = data.get("acceptance_criteria") {
                    meta["acceptance_criteria"] = ac.clone();
                }
                if let Some(er) = data.get("evidence_refs") {
                    meta["evidence_refs"] = er.clone();
                }
                if let Some(rat) = data.get("rationale") {
                    meta["rationale"] = rat.clone();
                }
                if let Some(ctx) = data.get("context") {
                    meta["context"] = ctx.clone();
                }
                if let Some(sup) = data.get("supersedes_decision_id") {
                    meta["supersedes_decision_id"] = sup.clone();
                }
                if let Some(imp) = data.get("impact_scope") {
                    meta["impact_scope"] = imp.clone();
                }
                if let Some(role) = data.get("role") {
                    meta["role"] = role.clone();
                }
                if let Some(method) = data.get("method") {
                    meta["method"] = method.clone();
                }
                if let Some(result) = data.get("result") {
                    meta["result"] = result.clone();
                }

                meta["updated_at"] = json!(now_unix_ms());

                let q = "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET project_code = EXCLUDED.project_code, title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata";
                let attach_to = data.get("attach_to").and_then(|v| v.as_str());
                let relation_hint = data.get("relation_hint").and_then(|v| v.as_str());

                let insert_res = self.graph_store.execute_param(
                    q,
                    &json!([
                        formatted_id,
                        entity_type_cap,
                        canonical_code,
                        title,
                        description,
                        status,
                        meta.to_string()
                    ]),
                );

                match insert_res {
                    Ok(_) => {
                        let created_id = formatted_id.clone();
                        let mut report = format!("SOLL entity created: `{}`", created_id);
                        let mut response_data = json!({
                            "created_id": created_id,
                            "entity_type": entity_type_cap,
                            "project_code": canonical_code,
                            "canonical_next_links": self.canonical_next_link_hints(entity_type_cap),
                            "attach_attempted": attach_to.is_some(),
                            "attached": false,
                            "attached_to": attach_to.map(Value::from).unwrap_or(Value::Null),
                            "applied_relation": Value::Null,
                            "attach_status": if attach_to.is_some() { Value::from("not_attempted") } else { Value::Null }
                        });

                        if let Some(target_id) = attach_to {
                            match self.select_relation_type_for_link(
                                &formatted_id,
                                target_id,
                                relation_hint,
                            ) {
                                Ok((relation_type, policy)) => {
                                    match self.insert_validated_relation(
                                        relation_type,
                                        &formatted_id,
                                        target_id,
                                        policy,
                                    ) {
                                        Ok(inserted) => {
                                            response_data["attached"] = Value::from(true);
                                            response_data["attached_to"] = Value::from(target_id);
                                            response_data["applied_relation"] =
                                                Value::from(relation_type);
                                            response_data["attach_status"] =
                                                Value::from(if inserted {
                                                    "attached"
                                                } else {
                                                    "already_present"
                                                });
                                            report.push_str(&format!(
                                                "\nCanonical link applied: `{}` -> `{}` via `{}`",
                                                formatted_id, target_id, relation_type
                                            ));
                                        }
                                        Err(error) => {
                                            let error_text = error.to_string();
                                            response_data["attach_status"] = Value::from(
                                                self.classify_attach_status_from_error(&error_text),
                                            );
                                            response_data["attach_guidance"] = self
                                                .relation_guidance_for_link(
                                                    &formatted_id,
                                                    target_id,
                                                    relation_hint,
                                                );
                                            report.push_str(&format!(
                                                "\nCanonical attach rejected: {}",
                                                error_text
                                            ));
                                        }
                                    }
                                }
                                Err(error) => {
                                    let error_text = error.to_string();
                                    response_data["attach_status"] = Value::from(
                                        self.classify_attach_status_from_error(&error_text),
                                    );
                                    response_data["attach_guidance"] = self
                                        .relation_guidance_for_link(
                                            &formatted_id,
                                            target_id,
                                            relation_hint,
                                        );
                                    report.push_str(&format!(
                                        "\nCanonical attach rejected: {}",
                                        error_text
                                    ));
                                }
                            }
                        }

                        if let (Some(before), Ok(after)) = (
                            before_snapshot.as_ref(),
                            self.soll_completeness_snapshot(Some(&canonical_code)),
                        ) {
                            response_data["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": formatted_id,
                                    "change_kind": "created",
                                    "entity_type": entity_type_cap
                                })],
                                json!({
                                    "nodes_created": 1,
                                    "nodes_updated": 0,
                                    "edges_created": usize::from(response_data["attached"].as_bool().unwrap_or(false))
                                }),
                            );
                        }

                        Some(json!({
                            "content": [{ "type": "text", "text": report }],
                            "data": response_data
                        }))
                    }
                    Err(e) => Some(Self::normalized_soll_writer_error("insert", e)),
                }
            }
            "update" => {
                let id = data.get("id")?.as_str()?;
                let project_code = project_code_from_canonical_entity_id(id);
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());

                let update_res: anyhow::Result<()> = (|| {
                    let current = self.query_named_row(
                        &format!("SELECT title, description, status, metadata FROM soll.Node WHERE id = '{}'", escape_sql(id)),
                        4,
                    )?;
                    let mut meta: Value = serde_json::from_str(&current[3]).unwrap_or(json!({}));

                    let title = data
                        .get("title")
                        .or_else(|| data.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[0]);
                    let description = data
                        .get("description")
                        .or_else(|| data.get("explanation"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[1]);
                    let status = data
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&current[2]);

                    if let Some(m) = data.get("metadata") {
                        if let Some(obj) = m.as_object() {
                            for (k, v) in obj {
                                meta[k] = v.clone();
                            }
                        }
                    }
                    if let Some(goal) = data.get("goal") {
                        meta["goal"] = goal.clone();
                    }
                    if let Some(priority) = data.get("priority") {
                        meta["priority"] = priority.clone();
                    }
                    if let Some(owner) = data.get("owner") {
                        meta["owner"] = owner.clone();
                    }
                    if let Some(ac) = data.get("acceptance_criteria") {
                        meta["acceptance_criteria"] = ac.clone();
                    }
                    if let Some(er) = data.get("evidence_refs") {
                        meta["evidence_refs"] = er.clone();
                    }
                    if let Some(rat) = data.get("rationale") {
                        meta["rationale"] = rat.clone();
                    }
                    if let Some(ctx) = data.get("context") {
                        meta["context"] = ctx.clone();
                    }
                    if let Some(sup) = data.get("supersedes_decision_id") {
                        meta["supersedes_decision_id"] = sup.clone();
                    }
                    if let Some(imp) = data.get("impact_scope") {
                        meta["impact_scope"] = imp.clone();
                    }
                    if let Some(role) = data.get("role") {
                        meta["role"] = role.clone();
                    }
                    if let Some(method) = data.get("method") {
                        meta["method"] = method.clone();
                    }
                    if let Some(result) = data.get("result") {
                        meta["result"] = result.clone();
                    }

                    meta["updated_at"] = json!(now_unix_ms());

                    let q = "UPDATE soll.Node SET title = ?, description = ?, status = ?, metadata = ? WHERE id = ?";
                    self.graph_store.execute_param(
                        q,
                        &json!([title, description, status, meta.to_string(), id]),
                    )
                })();

                match update_res {
                    Ok(_) => {
                        let mut payload = json!({
                            "content": [{ "type": "text", "text": format!("Update succeeded for `{}`", id) }],
                            "data": {}
                        });
                        if let (Some(code), Some(before), Ok(after)) = (
                            project_code.as_deref(),
                            before_snapshot.as_ref(),
                            project_code
                                .as_deref()
                                .ok_or_else(|| anyhow!("missing project"))
                                .and_then(|value| self.soll_completeness_snapshot(Some(value))),
                        ) {
                            let _ = code;
                            payload["data"]["mutation_feedback"] = self.mutation_feedback_payload(
                                before,
                                &after,
                                vec![json!({
                                    "id": id,
                                    "change_kind": "updated",
                                    "fields": ["title", "description", "status", "metadata"]
                                })],
                                json!({
                                    "nodes_created": 0,
                                    "nodes_updated": 1,
                                    "edges_created": 0
                                }),
                            );
                        }
                        Some(payload)
                    }
                    Err(e) => Some(Self::normalized_soll_writer_error("update", e)),
                }
            }
            "link" => {
                let src = data.get("source_id")?.as_str()?;
                let tgt = data.get("target_id")?.as_str()?;
                let explicit_rel = data.get("relation_type").and_then(|v| v.as_str());
                let project_code = project_code_from_canonical_entity_id(src)
                    .or_else(|| project_code_from_canonical_entity_id(tgt));
                let before_snapshot = project_code
                    .as_deref()
                    .and_then(|code| self.soll_completeness_snapshot(Some(code)).ok());
                match self.select_relation_type_for_link(src, tgt, explicit_rel) {
                    Ok((relation_type, policy)) => {
                        let rel_table = relation_table_name(relation_type).unwrap_or(relation_type);
                        match self.insert_validated_relation(relation_type, src, tgt, policy) {
                            Ok(inserted) => {
                                let mut payload = json!({
                                    "content": [{ "type": "text", "text": if inserted {
                                        format!("Link created: `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    } else {
                                        format!("Link already present: `{}` -> `{}` (via {})", src, tgt, rel_table)
                                    }}],
                                    "data": {}
                                });
                                if inserted {
                                    if let (Some(before), Some(code), Ok(after)) = (
                                        before_snapshot.as_ref(),
                                        project_code.as_deref(),
                                        project_code
                                            .as_deref()
                                            .ok_or_else(|| anyhow!("missing project"))
                                            .and_then(|value| {
                                                self.soll_completeness_snapshot(Some(value))
                                            }),
                                    ) {
                                        let _ = code;
                                        payload["data"]["mutation_feedback"] =
                                            self.mutation_feedback_payload(
                                                before,
                                                &after,
                                                vec![json!({
                                                    "id": format!("{}:{}:{}", src, relation_type, tgt),
                                                    "change_kind": "edge_created",
                                                    "source_id": src,
                                                    "target_id": tgt,
                                                    "relation_type": relation_type
                                                })],
                                                json!({
                                                    "nodes_created": 0,
                                                    "nodes_updated": 0,
                                                    "edges_created": 1
                                                }),
                                            );
                                    }
                                }
                                Some(payload)
                            }
                            Err(e) => Some(json!({
                                "content": [{ "type": "text", "text": Self::sanitized_link_error_text(&e) }],
                                "isError": true,
                                "data": self.relation_guidance_for_link(src, tgt, explicit_rel)
                            })),
                        }
                    }
                    Err(e) => Some(json!({
                        "content": [{ "type": "text", "text": Self::sanitized_link_error_text(&e) }],
                        "isError": true,
                        "data": self.relation_guidance_for_link(src, tgt, explicit_rel)
                    })),
                }
            }
            _ => None,
        }
    }
}
