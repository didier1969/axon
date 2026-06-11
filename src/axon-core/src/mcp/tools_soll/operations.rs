use super::*;

// REQ-AXO-147 — universal parameter_repair contract rollout for
// operations.rs (REQ-AXO-139 follow-up). The restore loop has 8 distinct
// per-entity-kind failure paths that previously emitted bare
// "SOLL restore <kind> error: <e>" strings without structured recovery.
// `restore_step_error_response` standardises them so an LLM can route on
// `data.parameter_repair.{step, entity_kind, hint, follow_up_tools}` in a
// single round-trip.
fn restore_step_error_response(step: &str, entity_kind: &str, err: &str) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": format!(
                "SOLL restore {} error: {}",
                entity_kind, err
            )
        }],
        "isError": true,
        "data": {
            "status": "internal_error",
            "operator_guidance": {
                "problem_class": "internal_error",
                "follow_up_tools": ["soll_validate", "soll_query_context"],
            },
            "parameter_repair": {
                "invalid_field": "path",
                "step": step,
                "entity_kind": entity_kind,
                "follow_up_tools": ["soll_validate", "soll_query_context"],
                "hint": format!(
                    "{step} on entity_kind=`{entity_kind}` failed; verify the source SOLL export is well-formed and matches the canonical schema. Run `soll_validate` after partial restore to surface remaining gaps."
                )
            },
            "diagnostic_excerpt": err.chars().take(240).collect::<String>()
        }
    })
}

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
        let project_code_input = args.get("project_code").and_then(|v| v.as_str());
        // REQ-AXO-147 — surface wrong_project_scope contract for unregistered
        // project_code (matches soll_validate / soll_query_context / soll_work_plan).
        if let Some(code) = project_code_input {
            if self.resolve_project_code(code).is_err() {
                return Some(self.wrong_project_scope_response(code, "soll_export"));
            }
        }
        let project_code = project_code_input
            .map(|code| self.resolve_project_code(code))
            .transpose()
            .ok()
            .flatten();
        let mut markdown = String::from("# SOLL Extraction\n\n");

        let now = std::time::SystemTime::now();
        let datetime: chrono::DateTime<chrono::Local> = now.into();
        let timestamp_str = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        markdown.push_str(&format!("*Generated on: {}*\n\n", timestamp_str));

        if let Some(ref code) = project_code {
            markdown.push_str(&format!("*Scope: project `{}`*\n\n", code));
        }

        // DEC-AXO-091 / REQ-AXO-322 (v3) — when scoped to a project,
        // walk the snapshot for both the Mermaid topology and the
        // node listing. Workspace-wide (no project_code) falls back
        // to SQL since the snapshot is per-project. Export still
        // needs `description` which the snapshot doesn't carry; pull
        // descriptions in a single batched SQL when needed.
        markdown.push_str("## Topologie (Mermaid)\n```mermaid\ngraph TD;\n");
        let snapshot_opt = project_code
            .as_deref()
            .and_then(|code| self.soll_cache().snapshot(code).ok());
        if let Some(snapshot) = snapshot_opt.as_deref() {
            for edge in &snapshot.edges {
                markdown.push_str(&format!(
                    "  {} -- {} --> {};\n",
                    edge.source_id, edge.relation_type, edge.target_id
                ));
            }
        } else if let Ok(res) = self.graph_store.query_json(&format!(
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

        if let Some(snapshot) = snapshot_opt.as_deref() {
            // Snapshot doesn't include description (kept out of the
            // hot-read footprint). Fetch descriptions in one batched
            // SQL keyed by id list — single round-trip regardless of
            // node count.
            let descriptions = if !snapshot.nodes.is_empty() {
                let ids = snapshot
                    .nodes
                    .keys()
                    .map(|id| format!("'{}'", escape_sql(id)))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.graph_store
                    .query_json(&format!(
                        "SELECT id, COALESCE(description, '') FROM soll.Node WHERE id IN ({ids})"
                    ))
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
                    .map(|rows| {
                        rows.into_iter()
                            .filter_map(|r| {
                                let mut it = r.into_iter();
                                let id = it.next()?;
                                let desc = it.next().unwrap_or_default();
                                Some((id, desc))
                            })
                            .collect::<std::collections::HashMap<String, String>>()
                    })
                    .unwrap_or_default()
            } else {
                std::collections::HashMap::new()
            };
            let mut sorted_ids: Vec<&String> = snapshot.nodes.keys().collect();
            sorted_ids.sort_by(|a, b| {
                let na = &snapshot.nodes[*a];
                let nb = &snapshot.nodes[*b];
                na.entity_type.cmp(&nb.entity_type).then_with(|| a.cmp(b))
            });
            let mut current_type = String::new();
            for id in sorted_ids {
                let node = &snapshot.nodes[id];
                if node.entity_type != current_type {
                    markdown.push_str(&format!("## Entities: {}\n", node.entity_type));
                    current_type = node.entity_type.clone();
                }
                markdown.push_str(&format!("### {} - {}\n", node.id, node.title));
                if let Some(desc) = descriptions.get(&node.id) {
                    if !desc.is_empty() {
                        markdown.push_str(&format!("**Description:** {}\n", desc));
                    }
                }
                if !node.status.is_empty() {
                    markdown.push_str(&format!("**Status:** {}\n", node.status));
                }
                if node.metadata_raw != "{}" {
                    markdown.push_str(&format!("**Meta:** `{}`\n", node.metadata_raw));
                }
                markdown.push('\n');
            }
        } else if let Ok(res) = self.graph_store.query_json(&format!(
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
                    "isError": true,
                    "data": {
                        "status": "internal_error",
                        "operator_guidance": {
                            "problem_class": "internal_error",
                            "follow_up_tools": ["status"],
                        },
                        "parameter_repair": {
                            "invalid_field": "canonical_soll_export_dir",
                            "follow_up_tools": ["status"],
                            "hint": "axon runtime cannot resolve docs/vision directory; verify project_path layout via `status mode=verbose` and the `instance_identity.data_root_absolute` field"
                        }
                    }
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
            Err(e) => Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Write error: {}", e) }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "operator_guidance": {
                        "problem_class": "internal_error",
                        "follow_up_tools": ["status"],
                    },
                    "parameter_repair": {
                        "invalid_field": "filesystem",
                        "supplied_path": file_path.display().to_string(),
                        "follow_up_tools": ["status"],
                        "hint": "fs::write to docs/vision failed; check disk space, permissions on the project root, and AXON_SOLL_EXPORT_RETAIN if pruning was attempted"
                    },
                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                }
            })),
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
        self.axon_validate_soll_with_cached_coverage(args, None)
    }

    /// Memoized variant — accepts a precomputed
    /// `RequirementCoverageSummary` so callers like `axon_soll_work_plan`
    /// can avoid the repeated heavy recomputation.
    pub(crate) fn axon_validate_soll_with_cached_coverage(
        &self,
        args: &Value,
        cached_coverage: Option<&RequirementCoverageSummary>,
    ) -> Option<Value> {
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
        // REQ-AXO-901602 — accept optional `statuses_to_check` array. The
        // default (`["current","planned"]`) suppresses the ~75
        // terminal-status false positives the validator previously raised
        // on AXO. Operators can opt into the full audit via `["*"]`.
        let statuses_to_check: Vec<String> = args
            .get("statuses_to_check")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_else(|| vec!["current".to_string(), "planned".to_string()]);
        let snapshot = match self.soll_completeness_snapshot_filtered(
            project_code,
            Some(&statuses_to_check),
            cached_coverage,
        ) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true,
                    "data": {
                        "status": "internal_error",
                        "operator_guidance": {
                            "problem_class": "internal_error",
                            "follow_up_tools": ["status", "soll_query_context"],
                        },
                        "parameter_repair": {
                            "invalid_field": "soll_completeness_snapshot",
                            "follow_up_tools": ["status", "soll_query_context"],
                            "hint": "graph snapshot computation failed; runtime may be degraded — check `status` and retry, or fall back to `soll_query_context` for a partial view"
                        },
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                    }
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
        // REQ-AXO-91528 (MIL-AXO-019 Tier B) — tri-modal envelope. The
        // completeness snapshot today reads from PG (`soll.Node`+`Edge`)
        // ; a follow-up slice can move cycle/orphan/relation-policy
        // detection to the SOLL petgraph snapshot (REQ-AXO-322) +
        // `petgraph::algo::is_cyclic_directed` for sub-ms runs.
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
                "guidance_source": "server-side canonical soll validation",
                "surfaces_used": ["soll_pg"],
                "total_available": violation_count as u64,
                "next_call_hint": "soll_manager action=link <fix one violation at a time>"
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
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "follow_up_tools": ["status"],
                        },
                        "parameter_repair": {
                            "invalid_field": "path",
                            "supplied_value": path.clone(),
                            "follow_up_tools": ["status"],
                            "hint": "supply a path to a SOLL_EXPORT_*.md file under docs/vision/ (or omit `path` to restore from the latest export)"
                        },
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                    }
                }))
            }
        };

        let restore = match parse_soll_export(&markdown) {
            Ok(parsed) => parsed,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("SOLL restore parse error: {}", e) }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "operator_guidance": {
                            "problem_class": "input_invalid",
                            "follow_up_tools": ["soll_export"],
                        },
                        "parameter_repair": {
                            "invalid_field": "path",
                            "supplied_value": path.clone(),
                            "follow_up_tools": ["soll_export"],
                            "hint": "the file does not match the canonical SOLL_EXPORT Markdown schema (sections: Vision / Pillars / Concepts / Milestones / Requirements / Decisions / Validations / Relations). Generate a fresh canonical export via `soll_export` and restore from that"
                        },
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                    }
                }))
            }
        };

        if let Err(e) = self.graph_store.execute(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_prv, last_rev)
             VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING"
        ) {
            return Some(restore_step_error_response(
                "registry_seed",
                "soll.Registry",
                &e.to_string(),
            ));
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
                return Some(restore_step_error_response("insert_node", "Vision", &e.to_string()));
            }
            restored.vision += 1;
        }

        for pillar in restore.pillars {
            let metadata = pillar.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, metadata)
                 VALUES ($id, 'Pillar', $project_code, $title, $description, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": pillar.id,
                    "project_code": super::shared::project_code_from_canonical_entity_id(&pillar.id).unwrap_or_else(|| "AXO".to_string()),
                    "title": pillar.title,
                    "description": pillar.description,
                    "metadata": metadata
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Pillar", &e.to_string()));
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
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ($id, 'Requirement', $project_code, $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": req.id,
                    "project_code": super::shared::project_code_from_canonical_entity_id(&req.id).unwrap_or_else(|| "AXO".to_string()),
                    "title": req.title,
                    "description": req.description,
                    "status": req.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Requirement", &e.to_string()));
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
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ($id, 'Decision', $project_code, $title, $description, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": dec.id,
                    "project_code": super::shared::project_code_from_canonical_entity_id(&dec.id).unwrap_or_else(|| "AXO".to_string()),
                    "title": dec.title,
                    "description": dec.description,
                    "status": dec.status.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Decision", &e.to_string()));
            }
            restored.decisions += 1;
        }

        for mil in restore.milestones {
            let metadata = mil.metadata.unwrap_or_else(|| "{}".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, status, metadata)
                 VALUES ($id, 'Milestone', $project_code, $title, $status, $metadata)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": mil.id,
                    "project_code": super::shared::project_code_from_canonical_entity_id(&mil.id).unwrap_or_else(|| "AXO".to_string()),
                    "title": mil.title,
                    "status": mil.status.clone(),
                    "metadata": metadata
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Milestone", &e.to_string()));
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
                "INSERT INTO soll.Node (id, type, project_code, status, metadata)
                 VALUES ($id, 'Validation', $project_code, $result, $metadata)
                 ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status, metadata = EXCLUDED.metadata",
                &serde_json::json!({
                    "id": val.id,
                    "project_code": super::shared::project_code_from_canonical_entity_id(&val.id).unwrap_or_else(|| "AXO".to_string()),
                    "result": val.result.clone(),
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Validation", &e.to_string()));
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
                    "project_code": super::shared::project_code_from_canonical_entity_id(&cpt.id).unwrap_or_else(|| "AXO".to_string()),
                    "name": cpt.name,
                    "explanation": cpt.explanation,
                    "metadata": meta_out.to_string()
                }),
            ) {
                return Some(restore_step_error_response("insert_node", "Concept", &e.to_string()));
            }
            restored.concepts += 1;
        }

        for rel in restore.relations {
            // REQ-AXO-152: derive project_code from canonical ID prefix and write
            // it on INSERT. NULL project_code rows brick brain boot via WAL replay.
            let project_code = super::shared::project_code_from_canonical_entity_id(&rel.source_id)
                .or_else(|| super::shared::project_code_from_canonical_entity_id(&rel.target_id))
                .unwrap_or_else(|| "AXO".to_string());
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) VALUES (?, ?, ?, '{}', ?) ON CONFLICT DO NOTHING",
                &serde_json::json!([rel.source_id, rel.target_id, rel.relation_type, project_code])
            ) {
                return Some(restore_step_error_response("insert_edge", "Edge", &e.to_string()));
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
