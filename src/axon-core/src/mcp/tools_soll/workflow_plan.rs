use super::*;

impl McpServer {
    pub(crate) fn axon_soll_apply_plan(&self, args: &Value) -> Option<Value> {
        let project_code = match self.require_registered_mutation_project_code(
            args.get("project_code").and_then(|v| v.as_str()),
            "soll_apply_plan",
        ) {
            Ok(code) => code,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let _plan = args.get("plan")?;

        let (canonical_project_code, _) = match self
            .resolve_canonical_project_identity_for_mutation(&project_code)
        {
            Ok(identity) => identity,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };

        let operations = self.build_plan_operations(&canonical_project_code, args);
        let preview_id = if let Some(reserved_preview_id) = args
            .get("reserved_preview_id")
            .and_then(|value| value.as_str())
        {
            reserved_preview_id.to_string()
        } else {
            let (_, project_code, _, next_preview) = match self
                .next_server_numeric_id(&canonical_project_code, "preview")
            {
                Ok(parts) => parts,
                Err(e) => {
                    return Some(json!({
                        "content": [{"type":"text","text": format!("SOLL apply_plan preview id error: {}", e)}],
                        "isError": true
                    }))
                }
            };
            format!("PRV-{}-{:03}", project_code, next_preview)
        };
        let payload = json!({
            "project_code": canonical_project_code,
            "author": author,
            "dry_run": dry_run,
            "operations": operations
        });

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.RevisionPreview (preview_id, author, project_code, payload, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (preview_id) DO UPDATE SET author = EXCLUDED.author, project_code = EXCLUDED.project_code, payload = EXCLUDED.payload, created_at = EXCLUDED.created_at",
            &json!([preview_id, author, canonical_project_code, payload.to_string(), now_unix_ms()]),
        ) {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan error: {}", e)}],
                "isError": true
            }));
        }

        let counts = summarize_ops(&operations);
        let result_contract = apply_plan_operation_contract(&operations);
        if dry_run {
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan DRY-RUN ready. preview_id={} (create={}, update={})", preview_id, counts.0, counts.1)}],
                "data": {
                    "preview_id": preview_id,
                    "counts": {"create": counts.0, "update": counts.1},
                    "operations": operations,
                    "result_contract": result_contract
                }
            }));
        }

        self.axon_soll_commit_revision(&json!({ "preview_id": preview_id, "author": author }))
    }

    pub(crate) fn axon_soll_commit_revision(&self, args: &Value) -> Option<Value> {
        let preview_id = match args.get("preview_id").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{"type":"text","text":"Missing required argument: preview_id"}],
                    "isError": true
                }));
            }
        };
        let author = args
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let preview_raw = match query_first_sql_cell(
            self,
            &format!(
                "SELECT payload FROM soll.RevisionPreview WHERE preview_id = '{}'",
                escape_sql(preview_id)
            ),
        ) {
            Some(v) => v,
            None => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Preview not found: {}", preview_id)}],
                    "isError": true
                }));
            }
        };
        let payload: Value = match serde_json::from_str(&preview_raw) {
            Ok(v) => v,
            Err(e) => {
                return Some(json!({
                    "content": [{"type":"text","text": format!("Invalid preview payload JSON: {}", e)}],
                    "isError": true
                }));
            }
        };
        let operations = payload
            .get("operations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let project_code = payload
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");

        let revision_id = if let Some(reserved_revision_id) = args
            .get("reserved_revision_id")
            .and_then(|value| value.as_str())
        {
            reserved_revision_id.to_string()
        } else {
            let (_, project_code, _, next_revision) = match self
                .next_server_numeric_id(project_code, "revision")
            {
                Ok(parts) => parts,
                Err(e) => {
                    return Some(json!({
                        "content": [{"type":"text","text": format!("SOLL commit error (revision id): {}", e)}],
                        "isError": true
                    }))
                }
            };
            format!("REV-{}-{:03}", project_code, next_revision)
        };
        let now = now_unix_ms();
        let _ = self.graph_store.execute("BEGIN TRANSACTION");

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([revision_id, author, "mcp", "SOLL plan commit", "committed", now, now]),
        ) {
            let _ = self.graph_store.execute("ROLLBACK");
            return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (revision row): {}", e)}],"isError": true}));
        }

        let mut identity_mapping = std::collections::HashMap::new();
        let mut linked_results = Vec::new();
        for op in &operations {
            match self.apply_operation_with_audit(&revision_id, op, &mut identity_mapping) {
                Ok(generated_id) => {
                    let kind = op
                        .get("kind")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    if kind == "link" {
                        // REQ-AXO-137: surface CANONICAL ids in data.linked[]
                        // so callers can immediately query the resulting Edges
                        // without re-resolving logical_keys themselves. The
                        // payload field still references the original logical_key
                        // (or canonical, when caller supplied one); we resolve
                        // both endpoints against identity_mapping for the
                        // response. Falls through to the original value when
                        // already canonical (not a logical_key).
                        let payload = op.get("payload").cloned().unwrap_or_else(|| json!({}));
                        let raw_source = payload
                            .get("source_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let raw_target = payload
                            .get("target_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let resolved_source = identity_mapping
                            .get(raw_source)
                            .cloned()
                            .unwrap_or_else(|| raw_source.to_string());
                        let resolved_target = identity_mapping
                            .get(raw_target)
                            .cloned()
                            .unwrap_or_else(|| raw_target.to_string());
                        linked_results.push(json!({
                            "source_id": resolved_source,
                            "target_id": resolved_target,
                            "raw_source_id": raw_source,
                            "raw_target_id": raw_target,
                            "relation_type": payload.get("relation_type").cloned().unwrap_or(Value::Null),
                            "status": "linked"
                        }));
                    } else if !generated_id.is_empty() {
                        if let Some(lk) = op.get("logical_key").and_then(|v| v.as_str()) {
                            identity_mapping.insert(lk.to_string(), generated_id);
                        }
                    }
                }
                Err(e) => {
                    let _ = self.graph_store.execute("ROLLBACK");
                    return Some(
                        json!({"content":[{"type":"text","text": format!("SOLL commit error (operation): {}", e)}],"isError": true}),
                    );
                }
            }
        }

        let _ = self.graph_store.execute("COMMIT");
        let _ = self.graph_store.execute(&format!(
            "DELETE FROM soll.RevisionPreview WHERE preview_id = '{}'",
            escape_sql(preview_id)
        ));

        let mut result_contract = apply_plan_operation_contract(&operations);
        if let Some(items) = result_contract
            .get_mut("created")
            .and_then(|value| value.as_array_mut())
        {
            for item in items.iter_mut() {
                if let Some(logical_key) = item.get("logical_key").and_then(|value| value.as_str())
                {
                    if let Some(actual_id) = identity_mapping.get(logical_key) {
                        item["id"] = Value::from(actual_id.clone());
                        item["status"] = Value::from("created");
                    }
                }
            }
        }
        if let Some(items) = result_contract
            .get_mut("updated")
            .and_then(|value| value.as_array_mut())
        {
            for item in items.iter_mut() {
                if let Some(logical_key) = item.get("logical_key").and_then(|value| value.as_str())
                {
                    if let Some(actual_id) = identity_mapping.get(logical_key) {
                        item["id"] = Value::from(actual_id.clone());
                    }
                }
                item["status"] = Value::from("updated");
            }
        }
        result_contract["linked"] = Value::Array(linked_results);

        Some(json!({
            "content": [{"type":"text","text": format!("SOLL revision committed: {} ({} operations)", revision_id, operations.len())}],
            "data": {
                "revision_id": revision_id,
                "operations": operations.len(),
                "identity_mapping": identity_mapping,
                "created": result_contract.get("created").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "updated": result_contract.get("updated").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "linked": result_contract.get("linked").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "skipped": result_contract.get("skipped").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "errors": result_contract.get("errors").cloned().unwrap_or_else(|| Value::Array(vec![]))
            }
        }))
    }

    pub(crate) fn axon_soll_query_context(&self, args: &Value) -> Option<Value> {
        let project_code_input = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .unwrap_or("AXO");
        // REQ-AXO-043 — wrong_project_scope contract via shared helper.
        let project_code = match self.resolve_project_code(project_code_input) {
            Ok(code) => code,
            Err(_) => {
                return Some(self.wrong_project_scope_response(project_code_input, "soll_query_context"));
            }
        };
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(25)
            .max(1);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let cache_key = format!("{}|{}", project_code, limit);
        if let Some(cached) = Self::read_soll_context_cache(&cache_key, now_ms) {
            return Some(cached);
        }

        let escaped_project = escape_sql(&project_code);
        let reqs = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Requirement'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let visions = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'') || '|' || COALESCE(description,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Vision'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let decisions = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Decision'
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                limit = limit
            ))
            .unwrap_or_default();
        let revisions = self
            .query_single_column(&format!(
                "SELECT revision_id || '|' || COALESCE(summary,'') || '|' || COALESCE(author,'')
             FROM soll.Revision
             ORDER BY committed_at DESC
             LIMIT {}",
                limit
            ))
            .unwrap_or_default();
        let completeness_snapshot = self.soll_completeness_snapshot(Some(&project_code)).ok();
        let entity_counts_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT type, count(*)
                 FROM soll.Node
                 WHERE project_code = '{}'
                 GROUP BY type
                 ORDER BY type",
                escaped_project
            ))
            .ok()?;
        let entity_count_rows: Vec<Vec<String>> =
            serde_json::from_str(&entity_counts_raw).unwrap_or_default();
        let entity_counts = entity_count_rows
            .into_iter()
            .filter_map(|row| {
                let entity_type = row.first()?.clone();
                let count = row.get(1)?.parse::<usize>().ok()?;
                Some(json!({
                    "entity_type": entity_type,
                    "count": count
                }))
            })
            .collect::<Vec<_>>();
        let last_revision_metadata = self
            .graph_store
            .query_json(&format!(
                "SELECT r.revision_id,
                        COALESCE(r.summary,''),
                        COALESCE(r.author,''),
                        COALESCE(r.status,''),
                        COALESCE(r.committed_at, r.created_at)
                 FROM soll.Revision r
                 JOIN soll.RevisionChange c
                   ON c.revision_id = r.revision_id
                 WHERE c.entity_id LIKE '%-{}-%'
                 GROUP BY r.revision_id, r.summary, r.author, r.status, r.committed_at, r.created_at
                 ORDER BY COALESCE(r.committed_at, r.created_at) DESC
                 LIMIT 1",
                escaped_project
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
            .and_then(|rows| rows.into_iter().next())
            .map(|row| {
                json!({
                    "revision_id": row.first().cloned().unwrap_or_default(),
                    "summary": row.get(1).cloned().unwrap_or_default(),
                    "author": row.get(2).cloned().unwrap_or_default(),
                    "status": row.get(3).cloned().unwrap_or_default(),
                    "committed_at": row.get(4).cloned().unwrap_or_default()
                })
            })
            .unwrap_or(json!({
                "available": false
            }));
        let operational_digest = query_context::build_operational_digest(
            completeness_snapshot.as_ref(),
            entity_counts,
            last_revision_metadata,
        );

        let response = json!({
            "content": [{"type":"text","text": format!("SOLL context for {} loaded.", project_code)}],
            "data": {
                "project_code": project_code,
                "visions": visions,
                "requirements": reqs,
                "decisions": decisions,
                "revisions": revisions,
                "operational_digest": operational_digest
            }
        });
        Self::write_soll_context_cache(cache_key, now_ms, &response);
        Some(response)
    }
}

fn summarize_ops(ops: &[Value]) -> (usize, usize) {
    let mut creates = 0usize;
    let mut updates = 0usize;
    for op in ops {
        match op.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "create" => creates += 1,
            "update" => updates += 1,
            _ => {}
        }
    }
    (creates, updates)
}

fn apply_plan_operation_contract(operations: &[Value]) -> Value {
    let mut created = Vec::new();
    let mut updated = Vec::new();
    let mut linked = Vec::new();
    let skipped = Vec::<Value>::new();
    let errors = Vec::<Value>::new();

    for op in operations {
        let kind = op
            .get("kind")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let entity = op
            .get("entity")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let payload = op.get("payload").cloned().unwrap_or_else(|| json!({}));
        match kind {
            "create" | "update" => {
                let record = json!({
                    "logical_key": op.get("logical_key").cloned().unwrap_or(Value::Null),
                    "entity": entity,
                    "title": payload.get("title").cloned().unwrap_or(Value::Null),
                    "predicted_id": op.get("entity_id").cloned().unwrap_or(Value::Null),
                    "status": if kind == "create" { "pending_create" } else { "pending_update" }
                });
                if kind == "create" {
                    created.push(record);
                } else {
                    updated.push(record);
                }
            }
            "link" => linked.push(json!({
                "source_id": payload.get("source_id").cloned().unwrap_or(Value::Null),
                "target_id": payload.get("target_id").cloned().unwrap_or(Value::Null),
                "relation_type": payload.get("relation_type").cloned().unwrap_or(Value::Null),
                "status": "pending_link"
            })),
            _ => {}
        }
    }

    json!({
        "created": created,
        "updated": updated,
        "linked": linked,
        "skipped": skipped,
        "errors": errors
    })
}
