use super::*;

impl McpServer {
    pub(crate) fn axon_soll_rollback_revision(&self, args: &Value) -> Option<Value> {
        let revision_id = args.get("revision_id")?.as_str()?;
        let query = format!(
            "SELECT entity_type, entity_id, action, before_json, after_json
             FROM soll.RevisionChange
             WHERE revision_id = '{}'
             ORDER BY created_at DESC",
            escape_sql(revision_id)
        );
        let rows_raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();

        // REQ-AXO-254: skip BEGIN/COMMIT pairing under PG (FFI
        // connection-pinning bug, see workflow_plan.rs commit notes).
        let use_explicit_transaction = !self.graph_store.is_postgres_backend();
        if use_explicit_transaction {
            let _ = self.graph_store.execute("BEGIN TRANSACTION");
        }
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let entity_type = &row[0];
            let entity_id = &row[1];
            let action = &row[2];
            let before_json = &row[3];

            let op = if action == "create" {
                json!({"kind":"delete", "entity": entity_type, "entity_id": entity_id})
            } else {
                let before_val: Value = serde_json::from_str(before_json).unwrap_or(json!({}));
                json!({"kind":"restore", "entity": entity_type, "entity_id": entity_id, "before": before_val})
            };

            if let Err(e) = self.apply_rollback_operation(&op) {
                if use_explicit_transaction {
                    let _ = self.graph_store.execute("ROLLBACK");
                }
                return Some(json!({
                    "content":[{"type":"text","text": format!("Rollback failed: {}", e)}],
                    "isError": true,
                    "data": {
                        "status": "internal_error",
                        "parameter_repair": {
                            "invalid_field": "revision_id",
                            "stage": "rollback_operation",
                            "supplied_revision_id": revision_id,
                            "follow_up_tools": ["soll_validate", "sql"],
                            "hint": "rollback operation failed mid-transaction; verify revision integrity via `sql SELECT * FROM soll.Revision WHERE revision_id='<id>'` and `soll_validate` post-rollback"
                        },
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                    }
                }));
            }
        }

        if use_explicit_transaction {
            let _ = self.graph_store.execute("COMMIT");
        }
        let _ = self.graph_store.execute(&format!(
            "UPDATE soll.Revision SET status = 'rolled_back' WHERE revision_id = '{}'",
            escape_sql(revision_id)
        ));
        Some(
            json!({"content":[{"type":"text","text": format!("Revision rolled back: {}", revision_id)}]}),
        )
    }

    pub(crate) fn apply_operation_with_audit(
        &self,
        revision_id: &str,
        op: &Value,
        identity_mapping: &mut std::collections::HashMap<String, String>,
    ) -> anyhow::Result<String> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("create");
        let entity = op
            .get("entity")
            .and_then(|v| v.as_str())
            .unwrap_or("requirement");
        let mut payload = op.get("payload").cloned().unwrap_or(serde_json::json!({}));
        let project_code = op
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        if kind == "link" {
            if let Some(obj) = payload.as_object_mut() {
                if let Some(sid) = obj.get("source_id").and_then(|v| v.as_str()) {
                    if let Some(canon) = identity_mapping.get(sid) {
                        obj.insert("source_id".to_string(), serde_json::json!(canon));
                    }
                }
                if let Some(tid) = obj.get("target_id").and_then(|v| v.as_str()) {
                    if let Some(canon) = identity_mapping.get(tid) {
                        obj.insert("target_id".to_string(), serde_json::json!(canon));
                    }
                }
            }

            let result = self.axon_soll_manager(
                &serde_json::json!({"action":"link","entity":"relation","data":payload}),
            );
            if soll_tool_is_error(result.as_ref()) {
                return Err(anyhow::anyhow!(
                    "{}",
                    soll_tool_text(result.as_ref()).unwrap_or_else(|| "link error".to_string())
                ));
            }
            return Ok(String::new());
        }

        let entity_id_hint = op
            .get("entity_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let before = if let Some(id) = entity_id_hint.clone() {
            self.snapshot_entity(entity, &id)
                .unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let result = if kind == "update" && entity_id_hint.is_some() {
            let mut data = payload.clone();
            data["id"] = serde_json::json!(entity_id_hint.clone().unwrap_or_default());
            self.axon_soll_manager(
                &serde_json::json!({"action":"update","entity":entity,"data":data}),
            )
        } else {
            let mut data = payload.clone();
            data["project_code"] = serde_json::json!(project_code);
            self.axon_soll_manager(
                &serde_json::json!({"action":"create","entity":entity,"data":data}),
            )
        };

        if soll_tool_is_error(result.as_ref()) {
            return Err(anyhow::anyhow!(
                "{}",
                soll_tool_text(result.as_ref()).unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        let entity_id = if let Some(id) = entity_id_hint {
            id
        } else {
            soll_tool_text(result.as_ref())
                .and_then(extract_soll_id_from_message)
                .unwrap_or_else(|| "unknown".to_string())
        };

        let after = self
            .snapshot_entity(entity, &entity_id)
            .unwrap_or(serde_json::json!({}));
        self.graph_store.execute_param(
            "INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &serde_json::json!([
                revision_id,
                entity,
                entity_id,
                kind,
                before.to_string(),
                after.to_string(),
                now_unix_ms()
            ]),
        )?;

        Ok(entity_id)
    }

    fn apply_rollback_operation(&self, op: &Value) -> anyhow::Result<()> {
        let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let entity = op.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let entity_id = op.get("entity_id").and_then(|v| v.as_str()).unwrap_or("");

        match (kind, entity) {
            ("delete", "pillar") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Pillar' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "requirement") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Requirement' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "decision") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Decision' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("delete", "milestone") => self.graph_store.execute(&format!(
                "DELETE FROM soll.Node WHERE type='Milestone' AND id = '{}'",
                escape_sql(entity_id)
            ))?,
            ("restore", _) => {
                let before = op.get("before").cloned().unwrap_or(json!({}));
                let mut data = before;
                data["id"] = json!(entity_id);
                let resp =
                    self.axon_soll_manager(&json!({"action":"update","entity":entity,"data":data}));
                if soll_tool_is_error(resp.as_ref()) {
                    return Err(anyhow!(
                        "{}",
                        soll_tool_text(resp.as_ref()).unwrap_or_default()
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot_entity(&self, entity: &str, entity_id: &str) -> Option<Value> {
        let query = match entity {
            "pillar" => format!(
                "SELECT title, description, metadata FROM soll.Node WHERE type='Pillar' AND id = '{}'",
                escape_sql(entity_id)
            ),
            "requirement" => format!(
                "SELECT title, description, status, metadata FROM soll.Node WHERE type='Requirement' AND id = '{}'",
                escape_sql(entity_id)
            ),
            "decision" => format!(
                "SELECT title, description, status, metadata FROM soll.Node WHERE type='Decision' AND id = '{}'",
                escape_sql(entity_id)
            ),
            "milestone" => format!(
                "SELECT title, status, metadata FROM soll.Node WHERE type='Milestone' AND id = '{}'",
                escape_sql(entity_id)
            ),
            "guideline" => format!(
                "SELECT title, description, status, metadata FROM soll.Node WHERE type='Guideline' AND id = '{}'",
                escape_sql(entity_id)
            ),
            _ => return None,
        };
        let raw = self.graph_store.query_json(&query).ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let first = rows.first()?;
        match entity {
            "pillar" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            "requirement" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "status": first.get(2).cloned().unwrap_or_default(),
                "priority": first.get(3).cloned().unwrap_or_default(),
                "metadata": first.get(4).cloned().unwrap_or_else(|| "{}".to_string()),
                "owner": first.get(5).cloned().unwrap_or_default(),
                "acceptance_criteria": first.get(6).cloned().unwrap_or_else(|| "[]".to_string()),
                "evidence_refs": first.get(7).cloned().unwrap_or_else(|| "[]".to_string())
            })),
            "decision" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "description": first.get(1).cloned().unwrap_or_default(),
                "context": first.get(2).cloned().unwrap_or_default(),
                "rationale": first.get(3).cloned().unwrap_or_default(),
                "status": first.get(4).cloned().unwrap_or_default(),
                "metadata": first.get(5).cloned().unwrap_or_else(|| "{}".to_string()),
                "supersedes_decision_id": first.get(6).cloned().unwrap_or_default(),
                "impact_scope": first.get(7).cloned().unwrap_or_default()
            })),
            "milestone" => Some(json!({
                "title": first.first().cloned().unwrap_or_default(),
                "status": first.get(1).cloned().unwrap_or_default(),
                "metadata": first.get(2).cloned().unwrap_or_else(|| "{}".to_string())
            })),
            _ => None,
        }
    }
}
