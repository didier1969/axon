use super::*;

impl McpServer {
    /// REQ-AXO-902300 — append a disclosure line to a tool response's text channel.
    ///
    /// Mirrors `disclose_cwd_provenance`: `content[0].text` is the channel an LLM
    /// actually reads, and a silent input normalisation would be a loss of trust,
    /// not a convenience. Applied to the response WHATEVER path produced it — the
    /// dry-run envelope built here, or the one `soll_commit_revision` returns when
    /// the call commits directly.
    fn append_text_note(response: Option<Value>, note: &str) -> Option<Value> {
        let mut response = response?;
        if let Some(text) = response
            .get_mut("content")
            .and_then(|c| c.get_mut(0))
            .and_then(|entry| entry.get_mut("text"))
        {
            if let Some(existing) = text.as_str() {
                *text = Value::from(format!("{existing}{note}"));
            }
        }
        Some(response)
    }

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
        // REQ-AXO-902300 — when only ONE placement is filled, the correction is
        // deterministic (same content, wrong slot): hoist it and carry on, rather
        // than spend a round-trip prescribing what we can already do. Same frontier
        // as REQ-AXO-902288 for `relation_type`: unambiguous → auto-canonicalise,
        // ambiguous → refuse. The refusal below now fires ONLY when both slots are
        // filled, where picking one (or merging into duplicates) would be guessing.
        if let Some(plan_obj) = args.get("plan").and_then(|v| v.as_object()) {
            let both_filled = plan_obj.contains_key("relations")
                && args
                    .get("relations")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty());
            if plan_obj.contains_key("relations") && !both_filled {
                let hoisted_len = plan_obj
                    .get("relations")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let mut patched = args.clone();
                if let Some(obj) = patched.as_object_mut() {
                    let moved = obj
                        .get_mut("plan")
                        .and_then(|p| p.as_object_mut())
                        .and_then(|p| p.remove("relations"))
                        .unwrap_or_else(|| json!([]));
                    obj.insert("relations".to_string(), moved);
                }
                tracing::debug!(
                    hoisted_len,
                    "REQ-AXO-902300 — hoisted plan.relations to top level"
                );
                let note = format!(
                    "\n\n_↳ `relations` ({hoisted_len} item(s)) était imbriqué dans `plan` ; \
                     hissé au niveau attendu et appliqué (REQ-AXO-902300). Le schéma le place \
                     à côté de `plan`, pas dedans._"
                );
                return Self::append_text_note(self.axon_soll_apply_plan(&patched), &note);
            }
            if let Some(misplaced) = plan_obj.get("relations") {
                let len = misplaced.as_array().map(|a| a.len()).unwrap_or(0);
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "soll_apply_plan rejected: `relations` is filled in BOTH places — {} item(s) nested inside `plan`, and a non-empty top-level `relations`. A nested-only array is hoisted automatically (REQ-AXO-902300); with both filled, picking one would drop relations and merging would duplicate them. Keep a single list, at the top level next to `plan`.",
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
                            "nested_items": len,
                            "hint": "both placements are filled: merge them into ONE top-level `relations` array so the call looks like `{project_code, plan:{requirements:[...]}, relations:[...]}`. A nested-ONLY array needs no fix — it is hoisted for you.",
                            // REQ-AXO-902055 — inline minimal valid call so the LLM
                            // corrects in one round-trip (pattern: evidence repair).
                            "corrected_call": {
                                "project_code": "<CODE>",
                                "plan": { "requirements": [{ "logical_key": "k1", "title": "…" }] },
                                "relations": [{ "source_logical_key": "k1", "target_id": "<PARENT-ID>", "relation_type": "BELONGS_TO" }]
                            },
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
                                        "soll_apply_plan rejected: plan.{}[{}] carries explicit id `{}`. apply_plan is create-only (idempotent by logical_key). To UPDATE an EXISTING node by its canonical id, use soll_manager(action=update, data={{id:\"{}\", …}}) — supplying a known id as logical_key here would CREATE A DUPLICATE.",
                                        collection, index, id, id
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
                                        "hint": "To CREATE: remove the id, supply logical_key + title (server allocates the id). To UPDATE this existing node: call soll_manager(action=update, data={id, …}) — do NOT pass the id as logical_key (creates a duplicate).",
                                        // REQ-AXO-902055 — both corrected calls inline (one round-trip).
                                        "corrected_call_create": {
                                            "project_code": "<CODE>",
                                            "plan": { "requirements": [{ "logical_key": "k1", "title": "…" }] }
                                        },
                                        "corrected_call_update": {
                                            "action": "update",
                                            "entity": "requirement",
                                            "data": { "id": id, "status": "delivered" }
                                        },
                                        "update_path": "soll_manager(action=update, data={id, status|description|…})",
                                        "follow_up_tools": ["soll_manager", "soll_apply_plan"],
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
            &json!([preview_id, author, canonical_project_code, payload.to_string(), crate::clock::now_unix_ms()]),
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
            // REQ-AXO-901992 B2 — surface the commit invariants (attach_to +
            // relation_type, parent existence) the bare preview used to hide, so
            // dry-run is honest about what will fail at commit.
            let commit_blockers = self.plan_commit_blockers(&operations);
            let blocker_note = if commit_blockers.is_empty() {
                String::new()
            } else {
                format!(
                    " ⚠️ {} item(s) WILL FAIL at commit (missing attach_to/relation_type, non-existent parent, or duplicate title) — see data.commit_blockers; fix before dry_run=false.",
                    commit_blockers.len()
                )
            };

            // REQ-AXO-902474 — rend, pour chaque nœud créé du plan, les nœuds vivants les plus proches (titre + distance)
            let mut plan_nearby_nodes = Vec::new();
            for op in &operations {
                let entity = op.get("entity").and_then(Value::as_str).unwrap_or("");
                if entity == "relation" {
                    continue;
                }
                let logical_key = op.get("logical_key").and_then(Value::as_str).unwrap_or("");
                let payload = op.get("payload");
                let title = payload
                    .and_then(|p| p.get("title").or_else(|| p.get("name")))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if title.is_empty() {
                    continue;
                }
                let nearby_query = format!(
                    "SELECT id, title, type, status, similarity(LOWER(title), LOWER('{}')) AS sim \
                     FROM soll.Node \
                     WHERE project_code = '{}' \
                       AND status NOT IN ('superseded', 'rejected', 'archived') \
                       AND similarity(LOWER(title), LOWER('{}')) >= 0.3 \
                     ORDER BY sim DESC, length(title), title \
                     LIMIT 5",
                    escape_sql(title),
                    escape_sql(&canonical_project_code),
                    escape_sql(title)
                );
                let (status, candidates) = match self.graph_store.query_json(&nearby_query) {
                    Ok(raw) => {
                        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
                        let mut cand = Vec::new();
                        for row in rows {
                            if let (Some(id), Some(t), Some(tp), Some(st)) = (
                                row.get(0).and_then(|v| v.as_str()),
                                row.get(1).and_then(|v| v.as_str()),
                                row.get(2).and_then(|v| v.as_str()),
                                row.get(3).and_then(|v| v.as_str()),
                            ) {
                                let sim = row.get(4).and_then(|v| {
                                    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                                }).unwrap_or(0.0);
                                cand.push(json!({
                                    "id": id,
                                    "title": t,
                                    "type": tp,
                                    "status": st,
                                    "similarity": sim
                                }));
                            }
                        }
                        if cand.is_empty() {
                            ("none", vec![])
                        } else {
                            ("found", cand)
                        }
                    }
                    Err(_) => ("search_unavailable", vec![])
                };
                plan_nearby_nodes.push(json!({
                    "logical_key": logical_key,
                    "title": title,
                    "status": status,
                    "candidates": candidates
                }));
            }

            return Some(json!({
                "content": [{"type":"text","text": format!("SOLL apply_plan DRY-RUN ready (NO mutations applied). preview_id={} (create={}, update={}, link={}).{} To commit, call soll_commit_revision(preview_id=\"{}\") or re-call soll_apply_plan with dry_run=false.", preview_id, counts.0, counts.1, counts.2, blocker_note, preview_id)}],
                "data": {
                    "preview_id": preview_id,
                    "applied": false,
                    "dry_run": true,
                    "counts": {"create": counts.0, "update": counts.1, "link": counts.2},
                    "commit_blockers": commit_blockers,
                    "nearby_nodes": plan_nearby_nodes,
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

    /// REQ-AXO-901992 B2 — the invariants the COMMIT enforces (via composed
    /// soll_manager(create)) that a bare dry-run preview did NOT surface: every
    /// non-Vision create needs `attach_to` (an EXISTING canonical id) +
    /// `relation_type`. The HYC consumer got "DRY-RUN ready" then a cascade of
    /// commit failures. Surfacing these as `commit_blockers` in the dry-run keeps
    /// the REQ-AXO-901625 preview contract intact while making the dry-run honest
    /// about what will fail at commit.
    pub(crate) fn plan_commit_blockers(&self, operations: &[Value]) -> Vec<Value> {
        let mut blockers = Vec::new();
        // (logical_key, entity, attach_to) for ops that passed the presence check
        // — checked for existence in a second pass.
        let mut to_check: Vec<(String, String, String)> = Vec::new();
        for op in operations {
            if op.get("kind").and_then(Value::as_str) != Some("create") {
                continue;
            }
            let entity = op.get("entity").and_then(Value::as_str).unwrap_or("");
            if entity == "vision" || entity == "relation" {
                continue;
            }
            let logical_key = op.get("logical_key").and_then(Value::as_str).unwrap_or("");
            let payload = op.get("payload");
            let attach_to = payload
                .and_then(|p| p.get("attach_to"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let relation_type = payload
                .and_then(|p| p.get("relation_type"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let mut missing: Vec<&str> = Vec::new();
            if attach_to.is_none() {
                missing.push("attach_to");
            }
            if relation_type.is_none() {
                missing.push("relation_type");
            }
            if !missing.is_empty() {
                blockers.push(json!({
                    "logical_key": logical_key,
                    "entity": entity,
                    "missing": missing,
                    "reason": "non-Vision create requires attach_to (an EXISTING canonical id, not a same-plan logical_key) + relation_type"
                }));
            } else if let Some(a) = attach_to {
                to_check.push((logical_key.to_string(), entity.to_string(), a.to_string()));
            }
        }
        // Existence pass: attach_to must point at an already-persisted node
        // (the 3rd failure HYC hit: "attach_to <logical_key> does not exist").
        for (logical_key, entity, attach_to) in to_check {
            let exists = self
                .graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM soll.Node WHERE id = '{}'",
                    attach_to.replace('\'', "''")
                ))
                .unwrap_or(0)
                > 0;
            if !exists {
                blockers.push(json!({
                    "logical_key": logical_key,
                    "entity": entity,
                    "missing": ["attach_to_target"],
                    "attach_to": attach_to,
                    "reason": "attach_to target does not exist — it must be an already-persisted canonical id (persist the parent first, or wire same-plan nodes via top-level `relations`)"
                }));
            }
        }

        // REQ-AXO-902474 / GUI-PRO-121: Reject duplicate titles at plan validation time
        let mut seen_titles: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for op in operations {
            let entity = op.get("entity").and_then(Value::as_str).unwrap_or("");
            if entity == "relation" {
                continue;
            }
            let logical_key = op.get("logical_key").and_then(Value::as_str).unwrap_or("");
            let payload = op.get("payload");
            let title = payload
                .and_then(|p| p.get("title").or_else(|| p.get("name")))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if title.is_empty() {
                continue;
            }
            let norm_title = title.to_lowercase();
            // 1) Intra-plan duplicate
            if let Some(other_key) = seen_titles.get(&norm_title) {
                if other_key != logical_key {
                    blockers.push(json!({
                        "logical_key": logical_key,
                        "entity": entity,
                        "title": title,
                        "problem_class": "duplicate_title_rejected",
                        "conflicting_logical_key": other_key,
                        "reason": format!("Duplicate title within same plan rejected by GUI-PRO-121: logical_key `{}` shares the same title", other_key)
                    }));
                }
            } else {
                seen_titles.insert(norm_title, logical_key.to_string());
            }

            // 2) DB live-node duplicate
            let check_sql = format!(
                "SELECT id, title, type, status, metadata->>'logical_key' FROM soll.Node \
                 WHERE status NOT IN ('superseded', 'rejected', 'archived') \
                   AND LOWER(TRIM(title)) = LOWER(TRIM('{}')) \
                 LIMIT 1",
                escape_sql(title)
            );
            if let Ok(raw) = self.graph_store.query_json(&check_sql) {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
                if let Some(row) = rows.first() {
                    let existing_id = row.get(0).and_then(|v| v.as_str()).unwrap_or("");
                    let existing_lk = row.get(4).and_then(|v| v.as_str());
                    let is_same_node = match existing_lk {
                        Some(lk) => lk == logical_key,
                        None => false,
                    };
                    if !is_same_node {
                        blockers.push(json!({
                            "logical_key": logical_key,
                            "entity": entity,
                            "title": title,
                            "conflicting_node": existing_id,
                            "problem_class": "duplicate_title_rejected",
                            "reason": format!("Duplicate title rejected by GUI-PRO-121: live node `{}` already carries this title", existing_id)
                        }));
                    }
                }
            }
        }
        blockers
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
        // REQ-AXO-902467 — ne plus deviner le projet courant.
        let resolved = payload
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.auto_resolve_project_code_str());
        let Some(project_code) = resolved.as_deref() else {
            return Some(crate::mcp::guidance::unresolved_project_error(
                "soll_work_plan",
                &self.known_project_codes_hint(),
            ));
        };

        let revision_id = if let Some(reserved_revision_id) = args
            .get("reserved_revision_id")
            .and_then(|value| value.as_str())
        {
            reserved_revision_id.to_string()
        } else {
            // REQ-AXO-902086 (fix définitif) — revision_id timestamp+nonce,
            // collision-free sous écritures concurrentes. L'ancien compteur
            // `REV-{code}-{NNN}` (soll.Registry.last_rev) se désynchronisait de
            // soll.Revision ET courait entre MAX() et INSERT : observé en prod
            // (brain 1215) → duplicate_key répété sur REV-AXO-036/037 même après
            // le patch max+1. Les révisions sont des lignes d'AUDIT (pas des nœuds
            // canoniques, DEC-AXO-085 ne s'y applique pas) → le format numérique
            // n'est pas requis. Aligne sur la voie soll_manager/unlink
            // (`unlink-{ts}-{src}`), qui n'a jamais collisionné.
            use std::sync::atomic::{AtomicU64, Ordering};
            static REV_NONCE: AtomicU64 = AtomicU64::new(0);
            format!(
                "REV-{}-{}-{}",
                project_code,
                crate::clock::now_unix_ms(),
                REV_NONCE.fetch_add(1, Ordering::Relaxed)
            )
        };
        let now = crate::clock::now_unix_ms();
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

        // REQ-AXO-902446 — Post-allocation pass: resolve remaining placeholders
        // (forward references) in created/updated node descriptions now that all
        // logical_keys in the plan have their canonical IDs allocated.
        if !identity_mapping.is_empty() {
            for op in &operations {
                let kind = op.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                if kind == "link" {
                    continue;
                }
                let logical_key = op.get("logical_key").and_then(|v| v.as_str()).unwrap_or("");
                let entity_id = match identity_mapping.get(logical_key) {
                    Some(id) => id.clone(),
                    None => {
                        if let Some(id) = op.get("entity_id").and_then(|v| v.as_str()) {
                            id.to_string()
                        } else {
                            continue;
                        }
                    }
                };
                let current_raw = self.graph_store.query_json(&format!(
                    "SELECT description FROM soll.Node WHERE id = '{}' LIMIT 1",
                    escape_sql(&entity_id)
                ));
                if let Ok(raw) = current_raw {
                    let rows: Vec<Vec<Option<String>>> = serde_json::from_str(&raw).unwrap_or_default();
                    if let Some(row) = rows.first() {
                        if let Some(Some(desc)) = row.first() {
                            if desc.contains("{{") {
                                let substituted = substitute_logical_keys_in_str(desc, &identity_mapping);
                                if &substituted != desc {
                                    let _ = self.graph_store.execute_param(
                                        "UPDATE soll.Node SET description = ? WHERE id = ?",
                                        &json!([substituted, entity_id]),
                                    );
                                    // REQ-AXO-902446 — Update audit row after_json if present
                                    if let Ok(rev_raw) = self.graph_store.query_json(&format!(
                                        "SELECT after_json FROM soll.RevisionChange WHERE revision_id = '{}' AND entity_id = '{}' LIMIT 1",
                                        escape_sql(&revision_id),
                                        escape_sql(&entity_id)
                                    )) {
                                        let rev_rows: Vec<Vec<Option<String>>> = serde_json::from_str(&rev_raw).unwrap_or_default();
                                        if let Some(rev_row) = rev_rows.first() {
                                            if let Some(Some(after_str)) = rev_row.first() {
                                                if let Ok(mut after_val) = serde_json::from_str::<Value>(after_str) {
                                                    substitute_logical_keys_in_value(&mut after_val, &identity_mapping);
                                                    let _ = self.graph_store.execute_param(
                                                        "UPDATE soll.RevisionChange SET after_json = ? WHERE revision_id = ? AND entity_id = ?",
                                                        &json!([after_val.to_string(), revision_id, entity_id]),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
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
        // REQ-AXO-902403 — RENDER the logical_key → canonical id mapping.
        //
        // Reported by KKI (llm_feedback #176): four Requirements created with
        // `logical_key`, and the whole answer was "SOLL revision committed:
        // REV-… (9 operations)". The ids WERE assigned — `identity_mapping`
        // carries them — but only into `data.*`, which the Claude Code client
        // does not expose to the LLM (the cause REQ-AXO-902355 closed for the
        // kickoff_bundle). So they called `soll_work_plan` and INFERRED that
        // 047/048/049/050 were their four items *in plan order* — a guess
        // nothing guarantees, then hard-wired into a milestone, a decision and
        // five edges. A wrong order would have wired the graph crooked in
        // silence. And `soll_manager(action=create)` — the lower-level
        // primitive — does print its id: the wrapper said LESS than the tool it
        // wraps.
        let mut mapping_lines: Vec<String> = identity_mapping
            .iter()
            .map(|(logical, canonical): (&String, &String)| {
                format!("| {logical} | {canonical} |")
            })
            .collect();
        mapping_lines.sort();
        let mapping_block = if mapping_lines.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n| logical_key | id canonique |\n|---|---|\n{}\n",
                mapping_lines.join("\n")
            )
        };

        Some(json!({
            "content": [{"type":"text","text": format!(
                "SOLL revision committed: {} ({} operations){}",
                revision_id,
                operations.len(),
                mapping_block
            )}],
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

    /// REQ-AXO-902249 — traverse SOLL edges from one node.
    ///
    /// The second most common raw-SQL shape after `soll_get`: listing an
    /// umbrella's `REFINES` children, or climbing to a parent, meant hand-writing
    /// a `JOIN soll.Edge e ON ... soll.Node n` every single time. Session 104 got
    /// it wrong on the first try (`column e.src does not exist` — the real columns
    /// are `source_id` / `target_id`). A tool removes that class of error by
    /// construction, which is the whole point of the operator's ask: MCP commands
    /// rather than LLMs querying the database themselves.
    pub(crate) fn axon_soll_children(&self, args: &Value) -> Option<Value> {
        let Some(id) = args
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Some(json!({
                "content": [{ "type": "text", "text": "soll_children requires `id` (canonical SOLL id, e.g. REQ-AXO-902192)." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": {
                    "invalid_field": "id",
                    "corrected_call": { "name": "soll_children", "arguments": { "id": "REQ-AXO-902192" } }
                } }
            }));
        };

        let direction_str = args
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or("children");
        let direction = match direction_str {
            "children" => "children",
            "parents" => "parents",
            "incoming" => "incoming",
            "outgoing" => "outgoing",
            other => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Invalid direction `{other}`. Expected `children`, `parents`, `incoming`, or `outgoing`.") }],
                    "isError": true,
                    "data": { "status": "input_invalid", "parameter_repair": {
                        "invalid_field": "direction",
                        "corrected_call": { "name": "soll_children", "arguments": { "id": id, "direction": "children" } }
                    } }
                }));
            }
        };

        let rel = args
            .get("relation_type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // REQ-AXO-902642: If relation_type is specified and is non-hierarchical,
        // it cannot be traversed under semantic filiation (`children` / `parents`).
        if let Some(r) = rel {
            if relation_traversal_semantics(r) == RelationTraversalSemantics::NonHierarchical
                && (direction == "children" || direction == "parents")
            {
                let notice = format!(
                    "`{r}` is a non-hierarchical relation (dependency or lateral link, not filiation). \
                     For non-hierarchical relations, use `direction=\"incoming\"` or `direction=\"outgoing\"`."
                );
                return Some(json!({
                    "content": [{ "type": "text", "text": format!(
                        "{} of {} via {}: 0 found\n(none)\n\n_{}_",
                        if direction == "parents" { "Parents" } else { "Children" },
                        id, r, notice
                    ) }],
                    "data": {
                        "status": "ok",
                        "id": id,
                        "direction": direction,
                        "relation_type": rel,
                        "count": 0,
                        "capped": false,
                        "nodes": []
                    }
                }));
            }
        }

        let (sql, params) = match direction {
            "incoming" => {
                let sql = format!(
                    "SELECT n.id, n.type, COALESCE(n.status,''), COALESCE(n.title,''), e.relation_type, e.source_id, e.target_id \
                     FROM soll.Edge e JOIN soll.Node n ON n.id = e.source_id \
                     WHERE e.target_id = ?{} ORDER BY n.id LIMIT 200",
                    if rel.is_some() { " AND e.relation_type = ?" } else { "" }
                );
                let params = match rel {
                    Some(r) => json!([id, r]),
                    None => json!([id]),
                };
                (sql, params)
            }
            "outgoing" => {
                let sql = format!(
                    "SELECT n.id, n.type, COALESCE(n.status,''), COALESCE(n.title,''), e.relation_type, e.source_id, e.target_id \
                     FROM soll.Edge e JOIN soll.Node n ON n.id = e.target_id \
                     WHERE e.source_id = ?{} ORDER BY n.id LIMIT 200",
                    if rel.is_some() { " AND e.relation_type = ?" } else { "" }
                );
                let params = match rel {
                    Some(r) => json!([id, r]),
                    None => json!([id]),
                };
                (sql, params)
            }
            "children" => {
                match rel {
                    Some(r) => {
                        let sem = relation_traversal_semantics(r);
                        let (match_col, other_col) = match sem {
                            RelationTraversalSemantics::ParentToChild => ("source_id", "target_id"),
                            RelationTraversalSemantics::ChildToParent => ("target_id", "source_id"),
                            RelationTraversalSemantics::NonHierarchical => unreachable!(),
                        };
                        let sql = format!(
                            "SELECT n.id, n.type, COALESCE(n.status,''), COALESCE(n.title,''), e.relation_type, e.source_id, e.target_id \
                             FROM soll.Edge e JOIN soll.Node n ON n.id = e.{other_col} \
                             WHERE e.{match_col} = ? AND e.relation_type = ? ORDER BY n.id LIMIT 200"
                        );
                        (sql, json!([id, r]))
                    }
                    None => {
                        // Semantic children union:
                        // 1. Incoming edges where relation is ChildToParent (BELONGS_TO, REFINES, EPITOMIZES), neighbor = source_id.
                        // 2. Outgoing edges where relation is ParentToChild (TARGETS, SOLVES), neighbor = target_id.
                        let sql = "SELECT id, type, status, title, relation_type, source_id, target_id FROM (\
                                       SELECT n.id AS id, n.type AS type, COALESCE(n.status,'') AS status, COALESCE(n.title,'') AS title, e.relation_type AS relation_type, e.source_id AS source_id, e.target_id AS target_id \
                                       FROM soll.Edge e JOIN soll.Node n ON n.id = e.source_id \
                                       WHERE e.target_id = ? AND e.relation_type IN ('BELONGS_TO', 'REFINES', 'EPITOMIZES') \
                                       UNION ALL \
                                       SELECT n.id AS id, n.type AS type, COALESCE(n.status,'') AS status, COALESCE(n.title,'') AS title, e.relation_type AS relation_type, e.source_id AS source_id, e.target_id AS target_id \
                                       FROM soll.Edge e JOIN soll.Node n ON n.id = e.target_id \
                                       WHERE e.source_id = ? AND e.relation_type IN ('TARGETS', 'SOLVES') \
                                   ) sub ORDER BY id LIMIT 200".to_string();
                        (sql, json!([id, id]))
                    }
                }
            }
            "parents" => {
                match rel {
                    Some(r) => {
                        let sem = relation_traversal_semantics(r);
                        let (match_col, other_col) = match sem {
                            RelationTraversalSemantics::ParentToChild => ("target_id", "source_id"),
                            RelationTraversalSemantics::ChildToParent => ("source_id", "target_id"),
                            RelationTraversalSemantics::NonHierarchical => unreachable!(),
                        };
                        let sql = format!(
                            "SELECT n.id, n.type, COALESCE(n.status,''), COALESCE(n.title,''), e.relation_type, e.source_id, e.target_id \
                             FROM soll.Edge e JOIN soll.Node n ON n.id = e.{other_col} \
                             WHERE e.{match_col} = ? AND e.relation_type = ? ORDER BY n.id LIMIT 200"
                        );
                        (sql, json!([id, r]))
                    }
                    None => {
                        // Semantic parents union:
                        // 1. Outgoing edges where relation is ChildToParent (BELONGS_TO, REFINES, EPITOMIZES), neighbor = target_id.
                        // 2. Incoming edges where relation is ParentToChild (TARGETS, SOLVES), neighbor = source_id.
                        let sql = "SELECT id, type, status, title, relation_type, source_id, target_id FROM (\
                                       SELECT n.id AS id, n.type AS type, COALESCE(n.status,'') AS status, COALESCE(n.title,'') AS title, e.relation_type AS relation_type, e.source_id AS source_id, e.target_id AS target_id \
                                       FROM soll.Edge e JOIN soll.Node n ON n.id = e.target_id \
                                       WHERE e.source_id = ? AND e.relation_type IN ('BELONGS_TO', 'REFINES', 'EPITOMIZES') \
                                       UNION ALL \
                                       SELECT n.id AS id, n.type AS type, COALESCE(n.status,'') AS status, COALESCE(n.title,'') AS title, e.relation_type AS relation_type, e.source_id AS source_id, e.target_id AS target_id \
                                       FROM soll.Edge e JOIN soll.Node n ON n.id = e.source_id \
                                       WHERE e.target_id = ? AND e.relation_type IN ('TARGETS', 'SOLVES') \
                                   ) sub ORDER BY id LIMIT 200".to_string();
                        (sql, json!([id, id]))
                    }
                }
            }
            _ => unreachable!(),
        };

        let raw = match self.graph_store.query_json_param(&sql, &params) {
            Ok(r) => r,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Database error while querying soll_children for `{id}`: {e}") }],
                    "isError": true,
                    "data": { "status": "backend_error", "id": id, "error": e.to_string() }
                }));
            }
        };

        let rows: Vec<Vec<Value>> = match serde_json::from_str(&raw) {
            Ok(r) => r,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Failed to decode query results for `{id}`: {e}") }],
                    "isError": true,
                    "data": { "status": "decode_error", "id": id, "error": e.to_string() }
                }));
            }
        };

        let cell = |r: &[Value], i: usize| r.get(i).and_then(Value::as_str).unwrap_or("").to_string();
        let items: Vec<Value> = rows
            .iter()
            .map(|r| json!({
                "id": cell(r, 0), "type": cell(r, 1),
                "status": cell(r, 2), "title": cell(r, 3),
                "relation_type": cell(r, 4),
                "source_id": cell(r, 5), "target_id": cell(r, 6),
            }))
            .collect();

        let lines: Vec<String> = rows
            .iter()
            .map(|r| format!("- {} [{}] {} ({} -> {}) — {}", cell(r, 0), cell(r, 2), cell(r, 4), cell(r, 5), cell(r, 6), cell(r, 3)))
            .collect();

        let capped = items.len() >= 200;

        // REQ-AXO-902401, REQ-AXO-902642 — Opposite direction check.
        // Takes relation_type into account when provided.
        let opposite_hint = if items.is_empty() {
            let (other_dir, other_sql, other_params) = match direction {
                "children" => {
                    let other = "parents";
                    let (q, p) = match rel {
                        Some(r) => match relation_traversal_semantics(r) {
                            RelationTraversalSemantics::ParentToChild => (
                                "SELECT count(*) FROM soll.Edge WHERE target_id = ? AND relation_type = ?".to_string(),
                                json!([id, r]),
                            ),
                            RelationTraversalSemantics::ChildToParent => (
                                "SELECT count(*) FROM soll.Edge WHERE source_id = ? AND relation_type = ?".to_string(),
                                json!([id, r]),
                            ),
                            RelationTraversalSemantics::NonHierarchical => (
                                "SELECT 0".to_string(),
                                json!([]),
                            ),
                        },
                        None => (
                            "SELECT count(*) FROM (\
                                 SELECT source_id FROM soll.Edge WHERE source_id = ? AND relation_type IN ('BELONGS_TO', 'REFINES', 'EPITOMIZES') \
                                 UNION ALL \
                                 SELECT target_id FROM soll.Edge WHERE target_id = ? AND relation_type IN ('TARGETS', 'SOLVES')\
                             ) sub".to_string(),
                            json!([id, id]),
                        ),
                    };
                    (other, q, p)
                }
                "parents" => {
                    let other = "children";
                    let (q, p) = match rel {
                        Some(r) => match relation_traversal_semantics(r) {
                            RelationTraversalSemantics::ParentToChild => (
                                "SELECT count(*) FROM soll.Edge WHERE source_id = ? AND relation_type = ?".to_string(),
                                json!([id, r]),
                            ),
                            RelationTraversalSemantics::ChildToParent => (
                                "SELECT count(*) FROM soll.Edge WHERE target_id = ? AND relation_type = ?".to_string(),
                                json!([id, r]),
                            ),
                            RelationTraversalSemantics::NonHierarchical => (
                                "SELECT 0".to_string(),
                                json!([]),
                            ),
                        },
                        None => (
                            "SELECT count(*) FROM (\
                                 SELECT target_id FROM soll.Edge WHERE source_id = ? AND relation_type IN ('TARGETS', 'SOLVES') \
                                 UNION ALL \
                                 SELECT source_id FROM soll.Edge WHERE target_id = ? AND relation_type IN ('BELONGS_TO', 'REFINES', 'EPITOMIZES')\
                             ) sub".to_string(),
                            json!([id, id]),
                        ),
                    };
                    (other, q, p)
                }
                "incoming" => {
                    let other = "outgoing";
                    let q = format!(
                        "SELECT count(*) FROM soll.Edge WHERE source_id = ?{}",
                        if rel.is_some() { " AND relation_type = ?" } else { "" }
                    );
                    let p = match rel {
                        Some(r) => json!([id, r]),
                        None => json!([id]),
                    };
                    (other, q, p)
                }
                "outgoing" => {
                    let other = "incoming";
                    let q = format!(
                        "SELECT count(*) FROM soll.Edge WHERE target_id = ?{}",
                        if rel.is_some() { " AND relation_type = ?" } else { "" }
                    );
                    let p = match rel {
                        Some(r) => json!([id, r]),
                        None => json!([id]),
                    };
                    (other, q, p)
                }
                _ => unreachable!(),
            };

            let count: i64 = self
                .graph_store
                .query_json_param(&other_sql, &other_params)
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .and_then(|rows| {
                    rows.first()?.first()?.as_i64().or_else(|| {
                        rows.first()?.first()?.as_str().and_then(|s| s.parse().ok())
                    })
                })
                .unwrap_or(0);

            if count > 0 {
                let rel_arg = rel.map(|r| format!(", relation_type=\"{r}\"")).unwrap_or_default();
                Some(format!(
                    "\n\n_0 in this direction, but {count} edge(s) exist the other way: \
                     `soll_children(id=\"{id}\", direction=\"{other_dir}\"{rel_arg})`. SOLL orientation is not \
                     uniform — `BELONGS_TO`/`REFINES` point child→parent, `TARGETS`/`SOLVES` point \
                     parent→child._"
                ))
            } else {
                None
            }
        } else {
            None
        };

        // REQ-AXO-902429 / REQ-AXO-902579 — Check if the queried node is superseded and has living replacement(s).
        let superseded_replacements: Vec<(String, String)> = {
            let sql = "SELECT e.source_id, COALESCE(n.title, '') \
                       FROM soll.Edge e \
                       JOIN soll.Node n ON n.id = e.source_id \
                       WHERE e.target_id = ? AND e.relation_type = 'SUPERSEDES' \
                       ORDER BY e.source_id ASC";
            self.graph_store
                .query_json_param(sql, &json!([id]))
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .map(|rows| {
                    rows.into_iter()
                        .filter_map(|r| {
                            let rep_id = r.get(0)?.as_str()?.to_string();
                            let rep_title = r.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                            Some((rep_id, rep_title))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        let direction_title = match direction {
            "parents" => "Parents",
            "incoming" => "Incoming edges",
            "outgoing" => "Outgoing edges",
            _ => "Children",
        };
        let capping_text = if capped {
            "\n_Results capped at 200 nodes._"
        } else {
            ""
        };

        let mut data_obj = json!({
            "status": "ok",
            "id": id,
            "direction": direction,
            "relation_type": rel,
            "count": items.len(),
            "capped": capped,
            "nodes": items,
        });

        let mut superseded_notice = String::new();
        if !superseded_replacements.is_empty() {
            let first_rep = &superseded_replacements[0];
            data_obj["superseded_by"] = json!(first_rep.0);
            data_obj["superseded_by_title"] = json!(first_rep.1);
            let all_ids: Vec<String> = superseded_replacements.iter().map(|(r_id, _)| r_id.clone()).collect();
            data_obj["superseded_by_all"] = json!(all_ids);

            if superseded_replacements.len() == 1 {
                superseded_notice = format!(
                    "ℹ Note: `{id}` has been superseded by `{}` ({}).\n\n",
                    first_rep.0,
                    if first_rep.1.is_empty() { "replacement node" } else { &first_rep.1 }
                );
            } else {
                let reps_str = superseded_replacements
                    .iter()
                    .map(|(rep_id, rep_title)| {
                        if rep_title.is_empty() {
                            format!("`{rep_id}`")
                        } else {
                            format!("`{rep_id}` ({rep_title})")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                superseded_notice = format!(
                    "ℹ Note: `{id}` has been superseded by: {reps_str}.\n\n"
                );
            }
        }

        Some(json!({
            "content": [{ "type": "text", "text": format!(
                "{}{} of {}{}: {} found\n{}{}{}",
                superseded_notice,
                direction_title,
                id,
                rel.map(|r| format!(" via {r}")).unwrap_or_default(),
                items.len(),
                if lines.is_empty() { "(none)".to_string() } else { lines.join("\n") },
                capping_text,
                opposite_hint.unwrap_or_default(),
            ) }],
            "data": data_obj
        }))
    }

    /// REQ-AXO-902248 — return the BODY of one SOLL node, by id.
    ///
    /// This is the single most-prescribed raw-SQL pattern in the whole system:
    /// `~/.claude/CLAUDE.md` (global, loaded in every session of every project)
    /// tells every LLM, twice, to run
    /// `sql SELECT description FROM soll.Node WHERE id='<ID>'`. It says so because
    /// no tool did the job — `soll_query_context` IGNORES an id and returns a
    /// project overview. `sql` is 63.5 % of all MCP traffic (71 424 calls); this
    /// pattern is plausibly its single largest contributor.
    ///
    /// Terse by default (GUI-AXO-1026 inv.4): the body IS the answer, since that
    /// is what the procedures reach for. Identity/status ride along in `data`.
/// REQ-AXO-902621 — au-delà de ce nombre de caractères, `soll_get` sans `sections`
/// ni `section` ne sert plus le corps entier. Calé sur la même valeur que le bornage
/// de `re_anchor` : les deux servent le même objet, un session pointer de plusieurs
/// centaines de milliers de caractères, et deux seuils différents pour un même corps
/// seraient une source de surprise pour rien.
const SEUIL_CORPS_ENTIER_CHARS: usize = 8_000;
/// REQ-AXO-902449 — budget global de caractères pour le multi-get soll_get(ids=[...]).
const MULTI_GET_TEXT_BUDGET: usize = 32_000;

    fn soll_get_multiple(&self, req_ids: &[String]) -> Value {
        if req_ids.is_empty() {
            return json!({
                "content": [{ "type": "text", "text": "soll_get: `ids` array must not be empty." }],
                "isError": true,
                "data": { "status": "input_invalid" }
            });
        }

        let in_clause = req_ids
            .iter()
            .map(|id| format!("'{}'", escape_sql(id)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, type, COALESCE(title,''), COALESCE(description,''), \
                    COALESCE(status,''), COALESCE(project_code,'') \
             FROM soll.Node WHERE id IN ({in_clause})"
        );
        let rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json(&sql)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        let mut row_map: HashMap<String, (String, String, String, String, String)> = HashMap::new();
        for r in rows {
            let cell = |i: usize| r.get(i).and_then(Value::as_str).unwrap_or("").to_string();
            let id = cell(0);
            row_map.insert(id.clone(), (cell(1), cell(2), cell(3), cell(4), cell(5)));
        }

        let mut missing_ids: Vec<String> = Vec::new();
        let mut deferred_ids: Vec<String> = Vec::new();
        let mut nodes_json: Vec<Value> = Vec::new();
        let mut rendered_blocks: Vec<String> = Vec::new();
        let mut cumulated_chars = 0;

        for id in req_ids {
            let Some((node_type, title, body, status, project)) = row_map.get(id) else {
                missing_ids.push(id.clone());
                continue;
            };

            let titres: Vec<&str> = body
                .lines()
                .filter(|l| l.trim_start().starts_with("## "))
                .map(|l| l.trim_start().trim_start_matches("## ").trim())
                .collect();

            let corps_rendu: String = if body.chars().count() > Self::SEUIL_CORPS_ENTIER_CHARS {
                let derniere = body
                    .match_indices("\n## ")
                    .last()
                    .map(|(i, _)| &body[i + 1..])
                    .or_else(|| body.starts_with("## ").then_some(body.as_str()));
                let table = if titres.is_empty() {
                    "_(ce corps ne porte aucun titre `##`)_".to_string()
                } else {
                    titres
                        .iter()
                        .map(|t| format!("- {t}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                match derniere {
                    Some(sec) => format!(
                        "_corps de {} caractères — au-delà du seuil de {}, seule la DERNIÈRE section \
                         est rendue. Les {} titres sont ci-dessous ; `section=\"<fragment>\"` en tire \
                         une autre._\n{}\n\n---\n\n{}",
                        body.chars().count(),
                        Self::SEUIL_CORPS_ENTIER_CHARS,
                        titres.len(),
                        table,
                        sec
                    ),
                    None => {
                        let saut = body.chars().count().saturating_sub(Self::SEUIL_CORPS_ENTIER_CHARS);
                        let octet = body.char_indices().nth(saut).map(|(i, _)| i).unwrap_or(0);
                        format!(
                            "_corps de {} caractères sans titre `##` — au-delà du seuil de {}, seule \
                             la FIN est rendue._\n\n{}",
                            body.chars().count(),
                            Self::SEUIL_CORPS_ENTIER_CHARS,
                            &body[octet..]
                        )
                    }
                }
            } else {
                body.to_string()
            };

            let rendered_node_text = format!("## {id} — {title}\n_{node_type} · {status} · {project}_\n\n{corps_rendu}");
            let block_len = rendered_node_text.len();

            if !rendered_blocks.is_empty() && cumulated_chars + block_len > Self::MULTI_GET_TEXT_BUDGET {
                deferred_ids.push(id.clone());
                continue;
            }

            cumulated_chars += block_len;
            rendered_blocks.push(rendered_node_text);
            nodes_json.push(json!({
                "id": id,
                "type": node_type,
                "title": title,
                "node_status": status,
                "status": status,
                "project_code": project,
                "section_titles": titres,
                "description": corps_rendu
            }));
        }

        let mut full_text = rendered_blocks.join("\n\n---\n\n");
        if !missing_ids.is_empty() {
            if !full_text.is_empty() {
                full_text.push_str("\n\n---\n\n");
            }
            full_text.push_str(&format!(
                "**Missing node(s)** (not found in SOLL): {}",
                missing_ids.join(", ")
            ));
        }
        if !deferred_ids.is_empty() {
            if !full_text.is_empty() {
                full_text.push_str("\n\n---\n\n");
            }
            full_text.push_str(&format!(
                "**Deferred node(s)** (text budget reached, call soll_get with these ids to read): {}",
                deferred_ids.join(", ")
            ));
        }

        json!({
            "content": [{ "type": "text", "text": full_text }],
            "data": {
                "status": "ok",
                "nodes": nodes_json,
                "missing_ids": missing_ids,
                "deferred_ids": deferred_ids,
                "total_found": nodes_json.len(),
                "total_requested": req_ids.len(),
                "next_action": { "kind": "continue_with_follow_up_tool", "tool": "soll_query_context", "when": "if_more_context_needed" }
            }
        })
    }

    pub(crate) fn axon_soll_get(&self, args: &Value) -> Option<Value> {
        // REQ-AXO-902449 — prise en charge de `ids` en mode batch.
        if let Some(ids_val) = args.get("ids") {
            let req_ids: Vec<String> = if let Some(arr) = ids_val.as_array() {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            } else if let Some(s) = ids_val.as_str() {
                s.split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(String::from)
                    .collect()
            } else {
                Vec::new()
            };

            if !req_ids.is_empty() {
                return Some(self.soll_get_multiple(&req_ids));
            }
        }

        let Some(id) = args
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Some(json!({
                "content": [{ "type": "text", "text": "soll_get requires `id` (canonical SOLL id, e.g. GUI-PRO-028) or `ids` (array of canonical ids)." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": {
                    "invalid_field": "id",
                    "corrected_call": { "name": "soll_get", "arguments": { "id": "GUI-PRO-028" } }
                } }
            }));
        };

        let rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json_param(
                "SELECT id, type, COALESCE(title,''), COALESCE(description,''), \
                        COALESCE(status,''), COALESCE(project_code,'') \
                 FROM soll.Node WHERE id = ? LIMIT 1",
                &json!([id]),
            )
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();

        let Some(row) = rows.first() else {
            // Repair AS DATA (inv.5): an unknown id is nearly always a typo or a
            // wrong family prefix, so hand back real neighbours rather than a bare
            // "not found" the caller must then go hunting for.
            let prefix = id.rsplit_once('-').map(|(p, _)| p).unwrap_or(id);
            let near: Vec<Value> = self
                .graph_store
                .query_json_param(
                    "SELECT id FROM soll.Node WHERE id LIKE ? ORDER BY id DESC LIMIT 8",
                    &json!([format!("{prefix}-%")]),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|r| r.into_iter().next())
                .collect();
            return Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "No SOLL node `{id}`. Closest ids in the `{prefix}` family: {}.",
                    if near.is_empty() { "(none)".to_string() }
                    else { near.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ") }
                ) }],
                "isError": true,
                "data": { "status": "not_found", "parameter_repair": {
                    "invalid_field": "id", "supplied_value": id, "nearby_ids": near,
                    "follow_up_tools": ["soll_id_registry", "soll_query_context"]
                } }
            }));
        };

        let cell = |i: usize| row.get(i).and_then(Value::as_str).unwrap_or("");
        let (node_type, title, body, status, project) =
            (cell(1), cell(2), cell(3), cell(4), cell(5));

        // REQ-AXO-902496 (doléance DVM #262) — la contrepartie en LECTURE d'`append_section`.
        //
        // « `soll_get(id="CPT-DVM-009")` rend d'un bloc le corps entier — ici environ
        // 6 000 jetons. À cet appel-là je ne voulais que la section "Les trois prochaines
        // actions" (~120 jetons), et il n'existe aucun moyen de la demander. » Sur un corps
        // relu à CHAQUE reprise de session — la définition d'un session pointer — l'écart
        // se paie à chaque fois. L'écriture avait été optimisée (`append_section`,
        // REQ-AXO-902161), la lecture non, alors qu'elle est le côté le plus fréquent.
        //
        // Sans paramètre, comportement STRICTEMENT inchangé.
        let titres: Vec<&str> = body
            .lines()
            .filter(|l| l.trim_start().starts_with("## "))
            .map(|l| l.trim_start().trim_start_matches("## ").trim())
            .collect();

        let veut_sommaire = args
            .get("sections")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let fragment = args
            .get("section")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|f| !f.is_empty());

        let corps_rendu: String = if veut_sommaire {
            format!(
                "**{} section(s)** — passe `section=\"<fragment du titre>\"` pour en lire une :\n{}",
                titres.len(),
                if titres.is_empty() {
                    "_(ce corps ne porte aucun titre `##`)_".to_string()
                } else {
                    titres
                        .iter()
                        .map(|t| format!("- {t}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            )
        } else if let Some(frag) = fragment {
            let bas = frag.to_lowercase();
            let mut trouve: Option<String> = None;
            let mut courant: Option<(String, Vec<&str>)> = None;
            for l in body.lines() {
                let t = l.trim_start();
                if t.starts_with("## ") {
                    if let Some((titre, lignes)) = courant.take() {
                        if titre.to_lowercase().contains(&bas) {
                            trouve = Some(format!("## {titre}\n{}", lignes.join("\n")));
                            break;
                        }
                    }
                    courant = Some((t.trim_start_matches("## ").trim().to_string(), Vec::new()));
                } else if let Some((_, lignes)) = courant.as_mut() {
                    lignes.push(l);
                }
            }
            if trouve.is_none() {
                if let Some((titre, lignes)) = courant {
                    if titre.to_lowercase().contains(&bas) {
                        trouve = Some(format!("## {titre}\n{}", lignes.join("\n")));
                    }
                }
            }
            match trouve {
                Some(sec) => sec,
                // Un fragment qui ne matche pas ne doit PAS coûter un aller-retour à
                // l'aveugle : on rend les titres disponibles avec le refus.
                None => format!(
                    "_aucune section dont le titre contient « {frag} ». {} titre(s) \
                     disponible(s) :_\n{}",
                    titres.len(),
                    titres
                        .iter()
                        .map(|t| format!("- {t}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }
        } else if body.chars().count() > Self::SEUIL_CORPS_ENTIER_CHARS {
            // REQ-AXO-902621 — un corps qui dépasse le seuil n'est plus servi entier
            // par défaut. Mesuré le 2026-09-05 : `soll_get` a rendu **115 227 octets
            // pour UN appel** (le session pointer, 330 k caractères), sur un total de
            // 260 279 pour huit appels. La sous-commande existe depuis
            // REQ-AXO-902496 ; c'est son EMPLOI qui n'était pas systématique, et un
            // défaut qu'il faut penser à éviter n'est pas un défaut évité.
            //
            // On rend la table des titres plus la DERNIÈRE section — sur un journal
            // append-only, c'est le récent qui oriente, et c'est ce que ces pointeurs
            // ordonnent eux-mêmes dans leur titre (« LIRE LA DERNIÈRE SECTION »).
            //
            // JAMAIS EN SILENCE (REQ-AXO-902583) : le rendu dit ce qu'il a retenu, ce
            // qu'il a écarté, et comment obtenir le reste. Un tronquage muet ferait
            // lire un corps amputé comme un corps complet.
            let derniere = body
                .match_indices("\n## ")
                .last()
                .map(|(i, _)| &body[i + 1..])
                .or_else(|| body.starts_with("## ").then_some(body));
            let table = if titres.is_empty() {
                "_(ce corps ne porte aucun titre `##`)_".to_string()
            } else {
                titres
                    .iter()
                    .map(|t| format!("- {t}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            match derniere {
                Some(sec) => format!(
                    "_corps de {} caractères — au-delà du seuil de {}, seule la DERNIÈRE section \
                     est rendue. Les {} titres sont ci-dessous ; `section=\"<fragment>\"` en tire \
                     une autre, `sections=true` ne rend que la table._\n{}\n\n---\n\n{}",
                    body.chars().count(),
                    Self::SEUIL_CORPS_ENTIER_CHARS,
                    titres.len(),
                    table,
                    sec
                ),
                // Aucune section : on garde la QUEUE, jamais la tête — sur un journal
                // append-only, la tête est le plus ancien.
                None => {
                    let saut = body.chars().count().saturating_sub(Self::SEUIL_CORPS_ENTIER_CHARS);
                    let octet = body.char_indices().nth(saut).map(|(i, _)| i).unwrap_or(0);
                    format!(
                        "_corps de {} caractères sans titre `##` — au-delà du seuil de {}, seule \
                         la FIN est rendue._\n\n{}",
                        body.chars().count(),
                        Self::SEUIL_CORPS_ENTIER_CHARS,
                        &body[octet..]
                    )
                }
            }
        } else {
            body.to_string()
        };

        // REQ-AXO-902580 — la sélection doit atteindre `data.description`, pas
        // seulement `content[0].text`. C'est `data` que le client sérialise vers
        // le LLM : rendre la découpe dans le texte tout en laissant le corps
        // ENTIER dans `data.description` n'économisait rien, et faisait payer
        // 224 000 caractères à un appel qui en demandait 120. Sans paramètre,
        // `corps_rendu` VAUT `body` — le comportement reste strictement inchangé.
        let texte =
            format!("## {id} — {title}\n_{node_type} · {status} · {project}_\n\n{corps_rendu}");

        Some(json!({
            "content": [{ "type": "text", "text": texte }],
            "data": {
                "status": "ok",
                "id": id, "type": node_type, "title": title,
                "node_status": status, "project_code": project,
                "section_titles": titres,
                "description": corps_rendu,
                "next_action": { "kind": "continue_with_follow_up_tool", "tool": "soll_query_context", "when": "if_more_context_needed" }
            }
        }))
    }

    pub(crate) fn axon_soll_query_context(&self, args: &Value) -> Option<Value> {
        // REQ-AXO-902467 — ne plus deviner le projet courant.
        let resolved_input = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.auto_resolve_project_code_str());
        let Some(project_code_input) = resolved_input.as_deref() else {
            return Some(crate::mcp::guidance::unresolved_project_error(
                "soll_roadmap",
                &self.known_project_codes_hint(),
            ));
        };
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
        let kind = args
            .get("kind")
            .or_else(|| args.get("type"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // REQ-AXO-901757 slice A — FTS search mode. When `search` is supplied,
        // return SOLL nodes ranked by ts_rank over title+description (served by
        // the soll_node_fts_idx GIN) instead of the project overview.
        if let Some(search) = args
            .get("search")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(self.soll_fts_search(&project_code, search, kind, limit));
        }
        // REQ-AXO-902449 — filtrage par TYPE/KIND sans recherche plein texte.
        if let Some(k) = kind {
            return Some(self.soll_query_by_kind(&project_code, k, limit));
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        // REQ-AXO-901941 — `changed_since` cursor (epoch ms). When supplied,
        // return ONLY nodes whose `metadata.updated_at` is newer than the
        // cursor, so a session re-checks state without re-paying the whole
        // graph in context. The response returns a fresh `cursor` (now) to
        // pass on the next call. `changed_since` is an integer, inlined
        // safely. Nodes predating updated_at tracking are excluded from a
        // delta (treated as unchanged-since-cursor).
        let changed_since = args.get("changed_since").and_then(|v| v.as_i64());
        let since_clause = changed_since
            .map(|c| format!(" AND (metadata->>'updated_at')::bigint > {c}"))
            .unwrap_or_default();
        let cache_key = format!("{}|{}|{}", project_code, limit, changed_since.unwrap_or(-1));
        if let Some(cached) = Self::read_soll_context_cache(&cache_key, now_ms) {
            return Some(cached);
        }

        let escaped_project = escape_sql(&project_code);
        let reqs = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Requirement'{since}
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                since = since_clause,
                limit = limit
            ))
            .unwrap_or_default();
        let visions = self
            .query_single_column(&format!(
                // REQ-AXO-901935 — a list surface renders {id, title, digest},
                // never the full body. The Vision body (often >1 KB) was dumped
                // verbatim here on every call; bound it to a digest. The full
                // Vision is read on demand (cold-start step 5 / `sql`).
                "SELECT id || '|' || title || '|' || COALESCE(status,'') || '|' || left(COALESCE(description,''), 200)
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Vision'{since}
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                since = since_clause,
                limit = limit
            ))
            .unwrap_or_default();
        let decisions = self
            .query_single_column(&format!(
                "SELECT id || '|' || title || '|' || COALESCE(status,'')
                 FROM soll.Node
                 WHERE project_code = '{project}'
                   AND type = 'Decision'{since}
                 ORDER BY id DESC
                 LIMIT {limit}",
                project = escaped_project,
                since = since_clause,
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
        // REQ-AXO-902305 — les mêmes comptes, gardés sous forme typée pour le rendu
        // TEXTE. Ils n'étaient calculés que pour `data.*` et absents du résumé,
        // d'où un projet fait de Guidelines/Skills qui se lisait « vide ».
        let entity_count_pairs: Vec<(String, i64)> = entity_count_rows
            .iter()
            .filter_map(|row| Some((row.first()?.clone(), row.get(1)?.parse::<i64>().ok()?)))
            .collect();
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
            &entity_count_pairs,
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
                // REQ-AXO-901941 — pass `cursor` back on the next call as
                // `changed_since` to receive ONLY nodes changed since now.
                "cursor": now_ms,
                "changed_since": changed_since,
                "next_call_hint": "soll_work_plan project_code=<code> top=8 for scored execution order ; or re-call with changed_since=<cursor> for a delta"
            }
        });
        Self::write_soll_context_cache(cache_key, now_ms, &response);
        Some(response)
    }

    /// REQ-AXO-902449 — query SOLL nodes by exact type/kind (e.g. "guideline", "requirement").
    fn soll_query_by_kind(&self, project_code: &str, kind: &str, limit: i64) -> Value {
        let escaped_project = escape_sql(project_code);
        let escaped_kind = escape_sql(kind);
        let rows = self
            .query_single_column(&format!(
                "SELECT id || '|' || type || '|' || COALESCE(title,'') || '|' || COALESCE(status,'') \
                 FROM soll.Node \
                 WHERE project_code = '{escaped_project}' AND lower(type) = lower('{escaped_kind}') \
                 ORDER BY id DESC \
                 LIMIT {limit}"
            ))
            .unwrap_or_default();
        let nodes: Vec<Value> = rows
            .iter()
            .map(|row| {
                let p: Vec<&str> = row.splitn(4, '|').collect();
                json!({
                    "id": p.first().copied().unwrap_or(""),
                    "type": p.get(1).copied().unwrap_or(""),
                    "title": p.get(2).copied().unwrap_or(""),
                    "status": p.get(3).copied().unwrap_or("")
                })
            })
            .collect();
        let text = if nodes.is_empty() {
            format!("No SOLL node of type \"{kind}\" in {project_code}.")
        } else {
            let lines: Vec<String> = nodes
                .iter()
                .map(|m| {
                    format!(
                        "- {} [{}] {}",
                        m["id"].as_str().unwrap_or(""),
                        m["status"].as_str().unwrap_or(""),
                        m["title"].as_str().unwrap_or("")
                    )
                })
                .collect();
            format!(
                "SOLL nodes of type \"{kind}\" in {project_code} ({} returned, limit {limit}):\n{}\n\nRead a node body via `soll_get(id='<ID>')` or multiple bodies via `soll_get(ids=['<ID1>', '<ID2>'])`.",
                nodes.len(),
                lines.join("\n")
            )
        };
        json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "project_code": project_code,
                "kind": kind,
                "nodes": nodes,
                "count": nodes.len(),
                "limit": limit,
                "next_call_hint": "read a node body via `soll_get(id='<ID>')` or multiple bodies via `soll_get(ids=['<ID1>', '<ID2>'])`"
            }
        })
    }

    /// REQ-AXO-901757 slice A — Full-Text Search over `soll.Node`
    /// (title+description), ranked by `ts_rank`. The `to_tsvector('simple', …)`
    /// expression is byte-identical to `soll_node_fts_idx` so the planner uses
    /// the GIN. `plainto_tsquery` is injection-safe for the operator (parses raw
    /// words into AND-tokens); the literal is still escaped defensively.
    ///
    /// REQ-AXO-902449 — supports optional `kind` filter to restrict FTS to a
    /// specific node type.
    fn soll_fts_search(&self, project_code: &str, query: &str, kind: Option<&str>, limit: i64) -> Value {
        let escaped_project = escape_sql(project_code);
        let escaped_query = escape_sql(query);
        let kind_clause = if let Some(k) = kind {
            format!(" AND lower(type) = lower('{}')", escape_sql(k))
        } else {
            String::new()
        };
        let tsv = "to_tsvector('simple', COALESCE(title,'') || ' ' || COALESCE(description,''))";
        let tsq = format!("plainto_tsquery('simple', '{escaped_query}')");
        let rows = self
            .query_single_column(&format!(
                "SELECT id || '|' || type || '|' || COALESCE(title,'') || '|' \
                     || COALESCE(status,'') || '|' || ts_rank({tsv}, {tsq})::text \
                 FROM soll.Node \
                 WHERE project_code = '{escaped_project}' AND {tsv} @@ {tsq}{kind_clause} \
                 ORDER BY ts_rank({tsv}, {tsq}) DESC, id DESC \
                 LIMIT {limit}"
            ))
            .unwrap_or_default();
        let matches: Vec<Value> = rows
            .iter()
            .map(|row| {
                let p: Vec<&str> = row.splitn(5, '|').collect();
                json!({
                    "id": p.first().copied().unwrap_or(""),
                    "type": p.get(1).copied().unwrap_or(""),
                    "title": p.get(2).copied().unwrap_or(""),
                    "status": p.get(3).copied().unwrap_or(""),
                    "rank": p.get(4).copied().unwrap_or("0")
                })
            })
            .collect();
        let kind_label = kind.map(|k| format!(" [{k}]")).unwrap_or_default();
        let text = if matches.is_empty() {
            format!("No SOLL node matches FTS \"{query}\"{kind_label} in {project_code}.")
        } else {
            let lines: Vec<String> = matches
                .iter()
                .map(|m| {
                    format!(
                        "- {} [{}] {}",
                        m["id"].as_str().unwrap_or(""),
                        m["status"].as_str().unwrap_or(""),
                        m["title"].as_str().unwrap_or("")
                    )
                })
                .collect();
            format!(
                "SOLL FTS \"{query}\"{kind_label} in {project_code} ({} match(es)):\n{}",
                matches.len(),
                lines.join("\n")
            )
        };
        let mut data = json!({
            "project_code": project_code,
            "search": query,
            "matches": matches,
            "surfaces_used": ["soll_fts"],
            "total_available": matches.len() as u64,
            "next_call_hint": "read a match body via `soll_get(id='<ID>')` or multiple bodies via `soll_get(ids=['<ID1>', '<ID2>'])`"
        });
        if let Some(k) = kind {
            data["kind"] = json!(k);
        }
        json!({
            "content": [{"type": "text", "text": text}],
            "data": data
        })
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
    entity_counts: &[(String, i64)],
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

    // REQ-AXO-902305 — the whole graph, by type, BEFORE the Vision/REQ/DEC detail.
    //
    // Those three were the only types rendered, so a project whose content is
    // anything else read as empty. `PRO` — the namespace carrying the entire
    // delivered methodology — reported "Vision: none, REQs: 0, DECs: 3" while
    // holding 111 nodes: 41 Guidelines, 30 Skills, 26 PromptTemplates. An LLM
    // discovering the product through this tool concluded there was nothing to
    // inherit.
    //
    // The counts were ALREADY computed by the caller and dropped on the floor —
    // same shape as REQ-AXO-902292 (`mcp_friction_report` announced a count and
    // named nothing). Counts only, never bodies: REQ-AXO-901935 removed a full
    // Vision dump from this very surface, and that lesson holds.
    if !entity_counts.is_empty() {
        let total: i64 = entity_counts.iter().map(|(_, n)| *n).sum();
        let breakdown: Vec<String> = entity_counts
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(kind, n)| format!("{n} {kind}"))
            .collect();
        out.push_str(&format!(
            "- Graph: {total} node(s) — {}\n",
            breakdown.join(", ")
        ));
    }

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

/// REQ-AXO-902411 — compter AUSSI les liens.
///
/// Le dry-run rendait `create=0, update=0` pour un lot de 15 arêtes `TARGETS`
/// soumises : un appelant qui prévisualise un lot de liens lisait « 0, 0 » et
/// en concluait raisonnablement que son lot était vide ou mal formé. Le commit,
/// lui, annonçait bien « 15 operations » — le défaut n'était que sur la branche
/// de PRÉVISUALISATION.
///
/// C'est la classe de REQ-AXO-902409 sur le chemin qui n'écrit pas, et c'est
/// le pire des deux : un aperçu muet précède la décision d'appliquer.
fn summarize_ops(ops: &[Value]) -> (usize, usize, usize) {
    let mut creates = 0usize;
    let mut updates = 0usize;
    let mut links = 0usize;
    for op in ops {
        match op.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
            "create" => creates += 1,
            "update" => updates += 1,
            "link" => links += 1,
            _ => {}
        }
    }
    (creates, updates, links)
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
            format_soll_query_context_summary("AXO", &visions, &reqs, &decisions, &revisions, &[]);

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
        let text = format_soll_query_context_summary("EMPTY", &[], &[], &[], &[], &[]);
        assert!(text.contains("SOLL context for EMPTY"));
        assert!(text.contains("Vision: none"));
        assert!(text.contains("REQs: 0 total"));
        assert!(text.contains("DECs: 0 total"));
        assert!(text.contains("Revisions: none"));
    }

    /// REQ-AXO-902305 — un projet sans Vision/REQ/DEC n'est pas un projet vide.
    ///
    /// `PRO` — le namespace qui porte TOUTE la méthodologie livrée au client —
    /// rendait « Vision: none, REQs: 0, DECs: 3 » alors qu'il compte 111 nœuds :
    /// 41 Guidelines, 30 Skills, 26 PromptTemplates. Un LLM découvrant le produit
    /// par cet outil concluait qu'il n'y avait rien à hériter. Les comptes étaient
    /// déjà calculés pour `data.*` et jetés avant le rendu — même forme que
    /// REQ-AXO-902292.
    #[test]
    fn summary_surfaces_a_project_made_of_guidelines_and_skills() {
        let counts = vec![
            ("Guideline".to_string(), 47i64),
            ("Skill".to_string(), 30),
            ("PromptTemplate".to_string(), 26),
            ("Decision".to_string(), 3),
        ];
        let text = format_soll_query_context_summary("PRO", &[], &[], &[], &[], &counts);

        assert!(
            text.contains("106 node(s)"),
            "le total doit être affiché, sinon « Vision: none / REQs: 0 » se lit \
             comme un projet vide : {text}"
        );
        for kind in ["47 Guideline", "30 Skill", "26 PromptTemplate"] {
            assert!(
                text.contains(kind),
                "le type porteur du contenu doit être nommé (`{kind}`) : {text}"
            );
        }
    }
}
