use super::*;

impl McpServer {
    pub(crate) fn axon_export_soll(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        // REQ-AXO-126 — `soll_export` is now snapshot-per-release: the
        // automatic hook that fired on every `axon_commit_work` is
        // removed (PIL-AXO-005 alignment — exports are part of the
        // qualified-release lineage, not a side-effect of routine
        // commits). The MCP tool stays available on demand: the
        // promotion pipeline (`scripts/release/promote_live_safe.sh`)
        // calls it once per live promotion, and operators can call it
        // manually for ad-hoc snapshots. No env-var gate is needed
        // because the per-call rate is now bounded by promotion
        // frequency, not commit frequency.
        let project_code = args.get("project_code").and_then(|v| v.as_str());
        let project_code = match project_code
            .map(|code| self.resolve_project_code(code))
            .transpose()
        {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        let mut markdown = String::from("# SOLL Extraction\n\n");

        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Local> = now.into();
        let timestamp_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        markdown.push_str(&format!("*Generated on: {}*\n\n", timestamp_str));

        if let Some(ref code) = project_code {
            markdown.push_str(&format!("*Scope: project `{}`*\n\n", code));
        }

        markdown.push_str("## Topologie (Mermaid)\n```mermaid\ngraph TD;\n");
        if let Ok(res) = self.graph_store.query_json(&format!(
            "SELECT source_id, target_id, relation_type FROM soll.Edge{}",
            project_scope_clause_for_relation(project_code.as_deref())
        )) {
            let edges: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            for edge in edges {
                if edge.len() >= 3 {
                    markdown.push_str(&format!("  {} -- {} --> {};\n", edge[0], edge[2], edge[1]));
                }
            }
        }
        markdown.push_str("```\n\n");

        if let Ok(res) = self.graph_store.query_json(&format!(
            "SELECT id, type, title, description, status, metadata FROM soll.Node{} ORDER BY type, id",
            project_scope_clause_for_table("id", project_code.as_deref())
        )) {
            let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
            let mut current_type = String::new();
            for r in rows {
                let n_id = &r[0];
                let n_type = &r[1];
                let title = &r[2];
                let desc = &r[3];
                let status = &r[4];
                let meta = r.get(5).cloned().unwrap_or_default();

                if n_type != &current_type {
                    markdown.push_str(&format!("## Entities: {}\n", n_type));
                    current_type = n_type.clone();
                }

                markdown.push_str(&format!("### {} - {}\n", n_id, title));
                if !desc.is_empty() {
                    markdown.push_str(&format!("**Description:** {}\n", desc));
                }
                if !status.is_empty() {
                    markdown.push_str(&format!("**Status:** {}\n", status));
                }
                if meta != "{}" {
                    markdown.push_str(&format!("**Meta:** `{}`\n", meta));
                }
                markdown.push('\n');
            }
        }

        let export_dir = match canonical_soll_export_dir() {
            Some(path) => path,
            None => {
                return Some(serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": "Write error: cannot resolve canonical docs/vision directory"
                    }],
                    "isError": true
                }))
            }
        };

        let file_name = format!("SOLL_EXPORT_{}.md", datetime.format("%Y-%m-%d_%H%M%S_%3f"));
        let file_path = export_dir.join(file_name);

        let _ = std::fs::create_dir_all(&export_dir);
        match std::fs::write(&file_path, &markdown) {
            Ok(_) => {
                // REQ-AXO-103 — auto-rotate to keep the most recent N
                // exports. Without this, every axon_commit_work
                // accumulates one SOLL_EXPORT file in docs/vision/
                // (already gitignored, but the disk count grows
                // unbounded — observed 700+ files within weeks of
                // routine usage). Honors AXON_SOLL_EXPORT_RETAIN env
                // override; defaults to 20 most-recent exports.
                let retain: usize = std::env::var("AXON_SOLL_EXPORT_RETAIN")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(20);
                Self::prune_old_soll_exports(&export_dir, retain);

                let report = format!(
                    "✅ Exported to {}\n\n---\n\n{}",
                    file_path.display(),
                    markdown.chars().take(300).collect::<String>()
                );
                Some(serde_json::json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(
                serde_json::json!({ "content": [{ "type": "text", "text": format!("Write error: {}", e) }], "isError": true }),
            ),
        }
    }

    pub(crate) fn prune_old_soll_exports(export_dir: &std::path::Path, keep: usize) {
        // Best-effort cleanup; swallow filesystem errors so a transient
        // permission issue never blocks the SOLL export.
        let entries = match std::fs::read_dir(export_dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy().to_string();
                if !(name.starts_with("SOLL_EXPORT_") && name.ends_with(".md")) {
                    return None;
                }
                let mtime = entry.metadata().ok()?.modified().ok()?;
                Some((mtime, path))
            })
            .collect();
        if candidates.len() <= keep {
            return;
        }
        // Newest first.
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in candidates.into_iter().skip(keep) {
            let _ = std::fs::remove_file(path);
        }
    }

    pub(crate) fn axon_validate_soll(&self, args: &Value) -> Option<Value> {
        let project_code = args.get("project_code").and_then(|v| v.as_str());
        // REQ-AXO-043 — when project_code is supplied but unregistered,
        // surface the structured wrong_project_scope contract via the
        // shared helper (matches soll_query_context / soll_work_plan /
        // anomalies / entrench_nuance) instead of a bare
        // "Canonical project error: <anyhow>" string.
        if let Some(code) = project_code {
            if self.resolve_project_code(code).is_err() {
                return Some(self.wrong_project_scope_response(code, "soll_validate"));
            }
        }
        let snapshot = match self.soll_completeness_snapshot(project_code) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        let violation_count = snapshot.orphan_requirements.len()
            + snapshot.validations_without_verifies.len()
            + snapshot.decisions_without_links.len()
            + snapshot.uncovered_requirements.len()
            + snapshot.duplicate_title_rows.len()
            + snapshot.relation_policy_violations.len();

        let mut repair_guidance = Vec::new();
        if !snapshot.orphan_requirements.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "orphan_requirements",
                &snapshot.orphan_requirements,
                "Requirements should be structurally attached to the graph.",
                &[
                    "link each requirement to its pillar or guideline with `soll_manager`",
                    "call `soll_relation_schema` with `source_id` or `target_id` before retrying if the valid edge is unclear",
                ],
            ));
        }
        if !snapshot.validations_without_verifies.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "validations_without_verifies",
                &snapshot.validations_without_verifies,
                "Validation nodes should verify at least one requirement.",
                &[
                    "add a `VERIFIES` edge from each validation to the requirement it proves",
                    "use `soll_relation_schema` on the validation id to inspect canonical targets if needed",
                ],
            ));
        }
        if !snapshot.decisions_without_links.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "decisions_without_solves_or_impacts",
                &snapshot.decisions_without_links,
                "Decision nodes should solve a requirement or impact an artifact.",
                &[
                    "link each decision to a requirement with `SOLVES` or `REFINES` when it addresses a need",
                    "link each decision to an artifact with `IMPACTS` or `SUBSTANTIATES` when it changes implementation reality",
                ],
            ));
        }
        if !snapshot.uncovered_requirements.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "requirements_without_evidence_or_criteria",
                &snapshot.uncovered_requirements,
                "Requirements should have acceptance criteria or explicit supporting evidence.",
                &[
                    "update requirement metadata with `acceptance_criteria`",
                    "attach evidence refs or add concept / decision / validation nodes that explain, solve, or verify the requirement",
                ],
            ));
        }
        if !snapshot.duplicate_ids.is_empty() {
            repair_guidance.push(repair_guidance_entry(
                "duplicate_titles",
                &snapshot.duplicate_ids,
                "Duplicate SOLL titles usually signal overlapping concepts, requirements, or decisions.",
                &[
                    "merge or supersede duplicates instead of keeping parallel semantic copies",
                    "prefer stable logical keys or update existing ids rather than creating near-identical nodes",
                ],
            ));
        }
        if !snapshot.relation_policy_violations.is_empty() {
            repair_guidance.push(json!({
                "category": "relation_policy_violations",
                "summary": "Some edges violate the canonical SOLL relation policy.",
                "ids": [],
                "details": snapshot.relation_policy_violations,
                "next_steps": [
                    "remove or replace invalid edges with canonical pairs from `soll_relation_schema`",
                    "retry the link only after the source/target kinds and default relation are confirmed"
                ],
                "guidance_source": "server-side canonical soll validation"
            }));
        }

        let completeness = json!({
            "populated": snapshot.total_nodes > 0,
            "structurally_connected": snapshot.structurally_connected(),
            "evidence_ready": snapshot.evidence_ready(),
            "duplicate_free": snapshot.duplicate_free(),
            "concept_completeness": snapshot.concept_complete(),
            "implementation_completeness": snapshot.implementation_complete()
        });

        let mut evidence = format!(
            "SOLL validation: {} minimal coherence violation(s) detected.\n",
            violation_count
        );
        evidence.push_str("Mode: read-only, no auto-repair.\n");

        if !snapshot.orphan_requirements.is_empty() {
            evidence.push_str("\n- Orphan requirements:\n");
            for id in &snapshot.orphan_requirements {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }
        if !snapshot.validations_without_verifies.is_empty() {
            evidence.push_str("\n- Validations without VERIFIES link:\n");
            for id in &snapshot.validations_without_verifies {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }
        if !snapshot.decisions_without_links.is_empty() {
            evidence.push_str("\n- Decisions without SOLVES/IMPACTS link:\n");
            for id in &snapshot.decisions_without_links {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }
        if !snapshot.uncovered_requirements.is_empty() {
            evidence.push_str("\n- Requirements without criteria/evidence:\n");
            for id in &snapshot.uncovered_requirements {
                evidence.push_str(&format!("  - {}\n", id));
            }
        }
        if !snapshot.duplicate_title_rows.is_empty() {
            evidence.push_str("\n- Duplicate titles (potential semantic duplicates):\n");
            for row in &snapshot.duplicate_title_rows {
                if row.len() < 3 {
                    continue;
                }
                evidence.push_str(&format!("  - {} :: {} -> {}\n", row[0], row[1], row[2]));
            }
        }
        if !snapshot.relation_policy_violations.is_empty() {
            evidence.push_str("\n- Invalid relations:\n");
            for violation in &snapshot.relation_policy_violations {
                evidence.push_str(&format!("  - {}\n", violation));
            }
        }

        let status = if violation_count == 0 {
            "ok"
        } else {
            "warn_soll_invariants"
        };
        let confidence = if violation_count == 0 {
            "high"
        } else {
            "medium"
        };
        let summary = if violation_count == 0 {
            "minimal soll invariants verified"
        } else {
            "minimal soll invariants violations detected"
        };
        let report = format!(
            "### 🧭 Validation SOLL\n\n{}",
            format_standard_contract(
                status,
                summary,
                &snapshot.project_scope,
                &evidence,
                &[
                    "run `soll_verify_requirements` for requirement-level coverage",
                    "apply targeted SOLL links with `soll_manager` if needed",
                    "deduplicate by updating existing nodes or using stable `logical_key` in `soll_apply_plan`"
                ],
                confidence,
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": status,
                "summary": summary,
                "scope": snapshot.project_scope,
                "violations": {
                    "orphan_requirements": snapshot.orphan_requirements,
                    "validations_without_verifies": snapshot.validations_without_verifies,
                    "decisions_without_links": snapshot.decisions_without_links,
                    "uncovered_requirements": snapshot.uncovered_requirements,
                    "duplicate_title_rows": snapshot.duplicate_title_rows,
                    "relation_policy_violations": snapshot.relation_policy_violations
                },
                "repair_guidance": repair_guidance,
                "completeness": completeness,
                "requirement_coverage": {
                    "done": snapshot.requirement_coverage.done,
                    "partial": snapshot.requirement_coverage.partial,
                    "missing": snapshot.requirement_coverage.missing
                },
                "guidance_source": "server-side canonical soll validation"
            }
        }))
    }

    pub(crate) fn axon_restore_soll(&self, args: &Value) -> Option<Value> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(find_latest_soll_export)?;

        let markdown = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore read error: {}", e) }],
                    "isError": true
                }))
            }
        };

        let restore = match parse_soll_export(&markdown) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore parse error: {}", e) }],
                    "isError": true
                }))
            }
        };

        if let Err(e) = self.graph_store.execute(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_prv, last_rev)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING"
        ) {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("SOLL restore registry error: {}", e) }],
                "isError": true
            }));
        }

        let mut restored = SollRestoreCounts::default();

        for vision in restore.vision {
            let mut meta_out: serde_json::Value = vision
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if !vision.goal.is_empty() {
                let goal = vision.goal.clone();
                if let Some(obj) = meta_out.as_object_mut() {
                    obj.insert("goal".to_string(), serde_json::Value::String(goal));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('VIS-AXO-001', 'Vision', 'AXO', $title, $description, NULL, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "title": vision.title,
                    "description": vision.description,
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore vision error: {}", e) }], "isError": true }));
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            let metadata = pillar.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, metadata)
                 VALUES ($id, 'Pillar', $title, $description, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": pillar.id,
                    "title": pillar.title,
                    "description": pillar.description,
                    "metadata": metadata
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore pillar error: {}", e) }], "isError": true }));
            }
            restored.pillars += 1;
        }

        for req in restore.requirements {
            let mut meta_out: serde_json::Value = req
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if !req.priority.is_empty() {
                    let priority = req.priority.clone();
                    obj.insert("priority".to_string(), serde_json::Value::String(priority));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, status, metadata)
                 VALUES ($id, 'Requirement', $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": req.id,
                    "title": req.title,
                    "description": req.description,
                    "status": req.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore requirement error: {}", e) }], "isError": true }));
            }
            restored.requirements += 1;
        }

        for dec in restore.decisions {
            let meta_out: serde_json::Value = dec
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, description, status, metadata)
                 VALUES ($id, 'Decision', $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": dec.id,
                    "title": dec.title,
                    "description": dec.description,
                    "status": dec.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore decision error: {}", e) }], "isError": true }));
            }
            restored.decisions += 1;
        }

        for mil in restore.milestones {
            let metadata = mil.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, title, status, metadata)
                 VALUES ($id, 'Milestone', $title, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": mil.id,
                    "title": mil.title,
                    "status": mil.status.clone(),
                    "metadata": metadata
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore milestone error: {}", e) }], "isError": true }));
            }
            restored.milestones += 1;
        }

        for val in restore.validations {
            let meta_out: serde_json::Value = val
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, status, metadata)
                 VALUES ($id, 'Validation', $result, $metadata)
                 ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": val.id,
                    "result": val.result.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore validation error: {}", e) }], "isError": true }));
            }
            restored.validations += 1;
        }

        for cpt in restore.concepts {
            let mut meta_out: serde_json::Value = cpt
                .metadata
                .unwrap_or_else(|| "{}".to_string())
                .parse()
                .unwrap_or(serde_json::json!({}));
            if let Some(obj) = meta_out.as_object_mut() {
                if !cpt.rationale.is_empty() {
                    let rat = cpt.rationale.clone();
                    obj.insert("rationale".to_string(), serde_json::Value::String(rat));
                }
            }
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, metadata)
                 VALUES ($id, 'Concept', $project_code, $name, $explanation, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": cpt.id,
                    "project_code": "AXO".to_string(),
                    "name": cpt.name,
                    "explanation": cpt.explanation,
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore concept error: {}", e) }], "isError": true }));
            }
            restored.concepts += 1;
        }

        for rel in restore.relations {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES (?, ?, ?, '{}') ON CONFLICT DO NOTHING",
                &serde_json::json!([rel.source_id, rel.target_id, rel.relation_type])
            ) {
                return Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("SOLL restore relation error: {}", e) }], "isError": true }));
            }
            restored.relations += 1;
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "### SOLL restore complete\n\nSource: `{}`\n\nRestored in merge mode:\n- Vision: {}\n- Pillars: {}\n- Concepts: {}\n- Milestones: {}\n- Requirements: {}\n- Decisions: {}\n- Validations: {}\n- Relations: {}\n\nNote: this restore path rebuilds conceptual entities from the official Markdown export format. Metadata and links present in the export are replayed in merge mode; absent fields retain historical tolerant behavior.",
                    path,
                    restored.vision,
                    restored.pillars,
                    restored.concepts,
                    restored.milestones,
                    restored.requirements,
                    restored.decisions,
                    restored.validations,
                    restored.relations
                )
            }]
        }))
    }
}
