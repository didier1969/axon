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
        // REQ-AXO-901625 — default switched from `true` to `false`. The
        // LLM-facing contract is "succeeded means applied" (CPT-AXO-025
        // Branch 2). With the previous default, a caller that omitted
        // `dry_run` got a successful preview that left soll.Node /
        // soll.Edge untouched — perfect silent-success failure mode.
        // Callers that want a preview must now opt in explicitly.
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let _plan = args.get("plan")?;

        // REQ-AXO-901625 — detect a frequent LLM mistake : nesting
        // `relations` inside `plan` instead of at the top level. The
        // documented schema places `relations` next to `plan`, but the
        // collection name reads naturally as part of the plan object so
        // callers slip the array inside. Before this guard the misplaced
        // array was silently dropped (parsed neither by build_plan_operations
        // nor by the top-level relations loop), producing a "succeeded"
        // job that materialised zero edges. Surface the misplacement
        // explicitly so the operator can correct the call in one round-trip.
        if let Some(plan_obj) = args.get("plan").and_then(|v| v.as_object()) {
            if let Some(misplaced) = plan_obj.get("relations") {
                let len = misplaced.as_array().map(|a| a.len()).unwrap_or(0);
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "soll_apply_plan rejected: `relations` ({} item(s)) is nested inside `plan` but the schema places it at the top level next to `plan`. Move the array out of `plan` and retry.",
                            len
                        )
                    }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "relations_misplaced_inside_plan",
                            "likely_cause": "schema_drift_relations_under_plan",
                            "follow_up_tools": ["help", "soll_apply_plan"],
                            "confidence": "high",
                        },
                        "parameter_repair": {
                            "tool": "soll_apply_plan",
                            "category": "relations_misplaced_inside_plan",
                            "invalid_field": "plan.relations",
                            "expected_field": "relations",
                            "items_silently_dropped": len,
                            "hint": "move `relations: [...]` out of `plan` so the call looks like `{project_code, plan:{requirements:[...]}, relations:[...]}`",
                            "follow_up_tools": ["help", "soll_apply_plan"],
                        },
                        "canonical_source": "REQ-AXO-901625",
                    },
                }));
            }
        }

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

        // MIL-AXO-020 slice 2 (REQ-AXO-91542) — reject plan items carrying
        // an explicit `id`. Server allocates canonical ids via
        // soll.allocate_node_id; logical_key is the right idempotence
        // mechanism. Visions are exempt because `axon_init_project`
        // restore flows may legitimately re-insert a known VIS id.
        if let Some(plan) = args.get("plan").and_then(|v| v.as_object()) {
            for (collection, items) in plan {
                if collection == "visions" {
                    continue;
                }
                if let Some(arr) = items.as_array() {
                    for (index, item) in arr.iter().enumerate() {
                        let supplied_id = item
                            .as_object()
                            .and_then(|obj| obj.get("id"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty());
                        if let Some(id) = supplied_id {
                            return Some(json!({
                                "content": [{
                                    "type": "text",
                                    "text": format!(
                                        "soll_apply_plan rejected: plan.{}[{}] carries explicit id `{}`. Use logical_key for idempotence; the server allocates canonical ids.",
                                        collection, index, id
                                    )
                                }],
                                "isError": true,
                                "data": {
                                    "status": "input_invalid",
                                    "operator_guidance": {
                                        "problem_class": "id_field_forbidden",
                                        "likely_cause": "caller_provided_id_in_plan",
                                        "follow_up_tools": ["soll_apply_plan"],
                                        "confidence": "high",
                                    },
                                    "parameter_repair": {
                                        "tool": "soll_apply_plan",
                                        "category": "id_field_forbidden",
                                        "invalid_field": format!("plan.{}[{}].id", collection, index),
                                        "supplied_value": id,
                                        "accepted_fields": [
                                            "logical_key",
                                            "title",
                                            "description",
                                            "status",
                                            "metadata"
                                        ],
                                        "hint": "remove the id field; supply logical_key + title and the server returns the allocated id in the result_contract entry",
                                        "follow_up_tools": ["soll_apply_plan"],
                                    },
                                    "canonical_source": "MIL-AXO-020",
                                },
                            }));
                        }
                    }
                }
            }
        }

        let operations = self.build_plan_operations(&canonical_project_code, args);

        // REQ-AXO-901625 — empty-plan guard. If neither plan.* collections
        // nor top-level relations[] produced any operation, the call is a
        // no-op : the previous silent-success path is the original symptom
        // logged by the operator. Surface this as an explicit input error
        // so the caller diagnoses the malformed payload in one round-trip.
        if operations.is_empty() {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": "soll_apply_plan rejected: plan produced zero operations. Provide at least one entry under `plan.{pillars|requirements|decisions|milestones|concepts|guidelines|stakeholders|validations}` or top-level `relations`."
                }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "operator_guidance": {
                        "problem_class": "empty_plan",
                        "likely_cause": "malformed_plan_payload_or_missing_collections",
                        "follow_up_tools": ["help", "soll_apply_plan"],
                        "confidence": "high",
                    },
                    "parameter_repair": {
                        "tool": "soll_apply_plan",
                        "category": "empty_plan",
                        "invalid_field": "plan",
                        "accepted_collections": [
                            "pillars", "requirements", "decisions", "milestones",
                            "concepts", "guidelines", "stakeholders", "validations"
                        ],
                        "top_level_field": "relations",
                        "hint": "ensure each plan entry includes `logical_key` (or `title`) and is nested under one of the accepted collection names",
                        "follow_up_tools": ["help"],
                    },
                    "canonical_source": "REQ-AXO-901625",
                },
            }));
        }

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
            // REQ-AXO-901625 — explicit `applied=false` + `dry_run=true`
            // flags so a caller polling `job_status` can distinguish a
            // preview from a real commit without re-reading the
            // human-readable content blob. Includes the next-step tool
            // call to flip the preview into a revision.
            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan DRY-RUN ready (NO mutations applied). preview_id={} (create={}, update={}). To commit, call soll_commit_revision(preview_id=\"{}\") or re-call soll_apply_plan with dry_run=false.", preview_id, counts.0, counts.1, preview_id)}],
                "data": {
                    "preview_id": preview_id,
                    "applied": false,
                    "dry_run": true,
                    "counts": {"create": counts.0, "update": counts.1},
                    "operations": operations,
                    "result_contract": result_contract,
                    "next_action": {
                        "tool": "soll_commit_revision",
                        "arguments": {"preview_id": preview_id},
                        "hint": "preview persisted in soll.RevisionPreview ; commit_revision materialises nodes + edges. Alternatively re-call soll_apply_plan with dry_run=false to commit in one shot."
                    },
                    "canonical_source": "REQ-AXO-901625"
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
        // REQ-AXO-254: deadpool serves a fresh connection per `pg_execute`,
        // so a wrapping BEGIN/COMMIT lands on different sessions and leaves
        // the first one "idle in transaction" with row locks held. Each
        // INSERT auto-commits; on partial failure the operator cleans up
        // via `soll_rollback_revision` (which inverts the captured
        // RevisionChange rows). A `with_pinned_connection` primitive that
        // restores real txn semantics is tracked by REQ-AXO-254 AC#1.

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Revision (revision_id, project_code, author, source, summary, status, created_at, committed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!([revision_id, project_code, author, "mcp", "SOLL plan commit", "committed", now, now]),
        ) {
            return Some(json!({"content":[{"type":"text","text": format!("SOLL commit error (revision row): {}", e)}],"isError": true}));
        }

        let mut identity_mapping = std::collections::HashMap::new();
        let mut linked_results = Vec::new();
        // REQ-AXO-139 slice — surface unresolved logical_keys in link
        // operations so the LLM can fix the inputs in one round-trip instead
        // of inspecting every Edge insert silently passing through bad keys.
        let mut link_errors: Vec<Value> = Vec::new();
        for (op_index, op) in operations.iter().enumerate() {
            let kind = op
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap_or("");

            // REQ-AXO-139 slice — pre-check link operations for unresolved
            // logical_keys BEFORE attempting the insert so the failure mode
            // is structured (errors[] + parameter_repair) rather than the
            // generic SQL error path that rolls back the whole transaction.
            if kind == "link" {
                let payload = op.get("payload").cloned().unwrap_or_else(|| json!({}));
                let raw_source = payload
                    .get("source_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let raw_target = payload
                    .get("target_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut unresolved: Vec<String> = Vec::new();
                if !raw_source.is_empty()
                    && !identity_mapping.contains_key(raw_source)
                    && project_code_from_canonical_entity_id(raw_source).is_none()
                {
                    unresolved.push(raw_source.to_string());
                }
                if !raw_target.is_empty()
                    && !identity_mapping.contains_key(raw_target)
                    && project_code_from_canonical_entity_id(raw_target).is_none()
                {
                    unresolved.push(raw_target.to_string());
                }
                if !unresolved.is_empty() {
                    let available: Vec<String> = identity_mapping.keys().cloned().collect();
                    link_errors.push(json!({
                        "operation_index": op_index,
                        "kind": "unresolved_logical_key",
                        "operation": "link",
                        "raw_source_id": raw_source,
                        "raw_target_id": raw_target,
                        "relation_type": payload.get("relation_type").cloned().unwrap_or(Value::Null),
                        "unresolved_keys": unresolved,
                        "available_logical_keys": available,
                        "hint": "supply a canonical TYPE-CODE-NNN id, or ensure the same `logical_key` was created earlier in this `operations` batch"
                    }));
                    continue;
                }
            }

            match self.apply_operation_with_audit(&revision_id, op, &mut identity_mapping) {
                Ok(generated_id) => {
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
                    return Some(
                        json!({"content":[{"type":"text","text": format!("SOLL commit error (operation): {}", e)}],"isError": true}),
                    );
                }
            }
        }

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

        // REQ-AXO-139 slice — surface unresolved logical_keys (and a
        // top-level parameter_repair shortcut) when present, mirroring
        // cypher-binder / inspect / dispatch slices for one-round-trip
        // recovery.
        let parameter_repair = if link_errors.is_empty() {
            Value::Null
        } else {
            let first = &link_errors[0];
            let unresolved: Vec<String> = first
                .get("unresolved_keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let available: Vec<String> = first
                .get("available_logical_keys")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "invalid_field": "operations[].payload.source_id|target_id",
                "operation_index": first.get("operation_index").cloned().unwrap_or(Value::Null),
                "unresolved_keys": unresolved,
                "available_logical_keys": available,
                "follow_up_tools": ["soll_apply_plan", "soll_manager"],
                "hint": "either reuse a `logical_key` declared as `kind=create|update` earlier in the same `operations` batch, or pass a canonical TYPE-CODE-NNN id directly"
            })
        };
        let mut errors = result_contract
            .get("errors")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![]));
        if let Some(arr) = errors.as_array_mut() {
            arr.extend(link_errors);
        }

        // REQ-AXO-901625 — explicit `applied=true` + `dry_run=false`
        // flags on the commit branch mirror the dry-run envelope so a
        // caller can branch on a single boolean instead of parsing the
        // human-readable content blob.
        Some(json!({
            "content": [{"type":"text","text": format!("SOLL revision committed: {} ({} operations)", revision_id, operations.len())}],
            "data": {
                "revision_id": revision_id,
                "applied": true,
                "dry_run": false,
                "operations": operations.len(),
                "identity_mapping": identity_mapping,
                "created": result_contract.get("created").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "updated": result_contract.get("updated").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "linked": result_contract.get("linked").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "skipped": result_contract.get("skipped").cloned().unwrap_or_else(|| Value::Array(vec![])),
                "errors": errors,
                "parameter_repair": parameter_repair,
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
                return Some(
                    self.wrong_project_scope_response(project_code_input, "soll_query_context"),
                );
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

        // REQ-AXO-91526 (MIL-AXO-019 Tier B) — `soll_query_context` runs
        // against the live PG SOLL tables (`soll.Node`, `soll.Revision`).
        // The SOLL petgraph snapshot (REQ-AXO-322, ~1 MB RAM) is the
        // analytic surface for `soll_work_plan`/`soll_verify_requirements`
        // ; this surface returns raw paginated rows. Surface flagged as
        // `soll_pg` until the snapshot exposes pagination by entity_type.
        let total_available =
            (visions.len() + reqs.len() + decisions.len() + revisions.len()) as u64;
        // REQ-AXO-901616 — surface a structured compact summary in the
        // text response so MCP clients that only display content[].text
        // (no data.*) still get actionable bootstrap info. The previous
        // text "SOLL context for {project} loaded." was a dead-end for
        // every LLM that didn't know to inspect `data.*`.
        let text = format_soll_query_context_summary(
            &project_code,
            &visions,
            &reqs,
            &decisions,
            &revisions,
        );
        let response = json!({
            "content": [{"type":"text","text": text}],
            "data": {
                "project_code": project_code,
                "visions": visions,
                "requirements": reqs,
                "decisions": decisions,
                "revisions": revisions,
                "operational_digest": operational_digest,
                "surfaces_used": ["soll_pg"],
                "total_available": total_available,
                "next_call_hint": "soll_work_plan project_code=<code> top=8 for scored execution order"
            }
        });
        Self::write_soll_context_cache(cache_key, now_ms, &response);
        Some(response)
    }
}

/// REQ-AXO-901616 — render a token-thrifty multi-line summary of the SOLL
/// context query result, surfacing canonical IDs + status counts in the
/// `content[].text` response so MCP clients that only display text still
/// get an actionable bootstrap view.
///
/// Row formats (built by axon_soll_query_context above) :
///   - visions  : "id|title|status|description"
///   - reqs     : "id|title|status"
///   - decisions: "id|title|status"
///   - revisions: "revision_id|summary|author"
pub(super) fn format_soll_query_context_summary(
    project_code: &str,
    visions: &[String],
    reqs: &[String],
    decisions: &[String],
    revisions: &[String],
) -> String {
    fn split_row(row: &str, max_parts: usize) -> Vec<&str> {
        row.splitn(max_parts, '|').collect()
    }

    fn status_counts<F>(rows: &[String], status_at: F) -> std::collections::BTreeMap<String, usize>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut counts = std::collections::BTreeMap::new();
        for row in rows {
            if let Some(status) = status_at(row) {
                *counts.entry(status).or_insert(0) += 1;
            }
        }
        counts
    }

    fn status_breakdown(counts: &std::collections::BTreeMap<String, usize>) -> String {
        if counts.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        format!(" ({})", parts.join(", "))
    }

    let mut out = String::new();
    out.push_str(&format!("SOLL context for {} :\n", project_code));

    // Visions — print id + title for each (typically very few).
    if visions.is_empty() {
        out.push_str("- Vision: none\n");
    } else {
        for row in visions.iter().take(3) {
            let parts = split_row(row, 4);
            let id = parts.first().copied().unwrap_or("?");
            let title = parts.get(1).copied().unwrap_or("");
            let status = parts.get(2).copied().unwrap_or("");
            out.push_str(&format!("- Vision: {} ({}) — {}\n", id, status, title));
        }
        if visions.len() > 3 {
            out.push_str(&format!("  ... +{} more vision(s)\n", visions.len() - 3));
        }
    }

    // Requirements — show top-3 ids + status breakdown.
    let req_counts = status_counts(reqs, |row| split_row(row, 3).get(2).map(|s| s.to_string()));
    let top_reqs: Vec<&str> = reqs
        .iter()
        .take(3)
        .filter_map(|row| split_row(row, 3).first().copied())
        .collect();
    out.push_str(&format!(
        "- REQs: {} total{}",
        reqs.len(),
        status_breakdown(&req_counts)
    ));
    if !top_reqs.is_empty() {
        out.push_str(&format!(" | top: {}", top_reqs.join(", ")));
    }
    out.push('\n');

    // Decisions — same shape.
    let dec_counts = status_counts(decisions, |row| {
        split_row(row, 3).get(2).map(|s| s.to_string())
    });
    let top_decs: Vec<&str> = decisions
        .iter()
        .take(3)
        .filter_map(|row| split_row(row, 3).first().copied())
        .collect();
    out.push_str(&format!(
        "- DECs: {} total{}",
        decisions.len(),
        status_breakdown(&dec_counts)
    ));
    if !top_decs.is_empty() {
        out.push_str(&format!(" | top: {}", top_decs.join(", ")));
    }
    out.push('\n');

    // Revisions — last revision id + summary.
    if let Some(first) = revisions.first() {
        let parts = split_row(first, 3);
        let id = parts.first().copied().unwrap_or("?");
        let summary = parts.get(1).copied().unwrap_or("");
        let truncated_summary = if summary.len() > 80 {
            format!("{}...", &summary[..80])
        } else {
            summary.to_string()
        };
        out.push_str(&format!(
            "- Last revision: {} — {} ({} more)\n",
            id,
            truncated_summary,
            revisions.len().saturating_sub(1)
        ));
    } else {
        out.push_str("- Revisions: none\n");
    }

    out.push_str("Use `soll_work_plan top=8` for scored execution order ; `soll_query_context` returns full rows in `data.*`.");
    out
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

#[cfg(test)]
mod soll_query_context_summary_tests {
    use super::format_soll_query_context_summary;

    /// REQ-AXO-901616 — the text payload must surface canonical IDs + status
    /// counts so a fresh LLM that can only see content[].text gets actionable
    /// bootstrap info (the previous "SOLL context for AXO loaded." was a
    /// dead-end).
    #[test]
    fn summary_surfaces_canonical_ids_and_status_counts() {
        let visions = vec!["VIS-AXO-001|Axon vision|current|desc".to_string()];
        let reqs = vec![
            "REQ-AXO-101|first|current".to_string(),
            "REQ-AXO-102|second|planned".to_string(),
            "REQ-AXO-103|third|delivered".to_string(),
        ];
        let decisions = vec!["DEC-AXO-001|d1|current".to_string()];
        let revisions = vec!["REV-001|migrated AGE→PG|author".to_string()];

        let text =
            format_soll_query_context_summary("AXO", &visions, &reqs, &decisions, &revisions);

        // Canonical id surfaces (vision + REQ top + DEC top + revision id).
        assert!(text.contains("VIS-AXO-001"), "missing vision id: {text}");
        assert!(text.contains("REQ-AXO-101"), "missing top REQ id: {text}");
        assert!(text.contains("DEC-AXO-001"), "missing DEC id: {text}");
        assert!(text.contains("REV-001"), "missing revision id: {text}");
        // Status breakdown counts each REQ status.
        assert!(
            text.contains("1 current"),
            "missing 'current' count: {text}"
        );
        assert!(
            text.contains("1 planned"),
            "missing 'planned' count: {text}"
        );
        assert!(
            text.contains("1 delivered"),
            "missing 'delivered' count: {text}"
        );
        // Hint anchors the next call.
        assert!(
            text.contains("soll_work_plan"),
            "missing next-call hint: {text}"
        );
    }

    /// REQ-AXO-901616 — empty payloads produce the friendly fallback.
    #[test]
    fn summary_handles_empty_payload() {
        let text = format_soll_query_context_summary("EMPTY", &[], &[], &[], &[]);
        assert!(text.contains("SOLL context for EMPTY"));
        assert!(text.contains("Vision: none"));
        assert!(text.contains("REQs: 0 total"));
        assert!(text.contains("DECs: 0 total"));
        assert!(text.contains("Revisions: none"));
    }
}
