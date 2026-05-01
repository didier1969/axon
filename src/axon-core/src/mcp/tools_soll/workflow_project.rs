use super::*;

/// REQ-AXO-121 — recognize inline `#[cfg(test)] mod tests { … }` blocks
/// inside the modified `.rs` file itself when looking for a `tests.rs`
/// satisfier. Without this, GUI-PRO-001 forced a sibling `_tests.rs`
/// file for binaries (whose canonical idiom is inline tests) and
/// blocked trivial library hygiene fixes (one-line attribute changes
/// in files that already have inline tests). The sibling-file
/// patterns from the original matcher are preserved so existing
/// commits keep passing.
fn path_satisfies_required_path(path: &str, required_path: &str) -> bool {
    if path.contains(required_path) {
        return true;
    }

    if required_path == "tests.rs" {
        let normalized = path.replace('\\', "/");
        if normalized.ends_with("_test.rs")
            || normalized.ends_with("_tests.rs")
            || normalized.ends_with("/tests.rs")
            || normalized.contains("/tests/")
            || normalized.contains("/test/")
        {
            return true;
        }
        // Inline-test recognition: open the file and look for the
        // `#[cfg(test)]` attribute. The check is deliberately
        // conservative — substring-only — so it does not require AST
        // parsing and works on any Rust source. Files that do not
        // exist or are not readable fall through to the negative
        // branch (the gate stays strict on truly untested diffs).
        if normalized.ends_with(".rs") {
            if let Ok(content) = std::fs::read_to_string(path) {
                if content.contains("#[cfg(test)]") {
                    return true;
                }
            }
        }
    }

    false
}

impl McpServer {
    pub(crate) fn axon_commit_work(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let diff_paths = args.get("diff_paths")?.as_array()?;
        let message = args.get("message")?.as_str()?;
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let rows_raw = self
            .graph_store
            .query_json(
                "SELECT id, title, description, metadata FROM soll.Node WHERE type='Guideline' AND status='active'",
            )
            .unwrap_or_else(|_| "[]".to_string());

        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut violations = Vec::new();

        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let id = &row[0];
            let meta: serde_json::Value =
                serde_json::from_str(&row[3]).unwrap_or_else(|_| serde_json::json!({}));

            let trigger_path = meta
                .get("trigger_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let required_path = meta
                .get("required_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let enforcement = meta
                .get("enforcement")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if trigger_path.is_empty() || required_path.is_empty() || enforcement != "strict" {
                continue;
            }

            let trigger_clean = trigger_path.replace("*", "");
            let triggered = diff_paths.iter().any(|p| {
                p.as_str()
                    .map(|path_str| path_str.contains(&trigger_clean))
                    .unwrap_or(false)
            });

            if triggered {
                let satisfied = diff_paths.iter().any(|p| {
                    p.as_str()
                        .map(|path_str| path_satisfies_required_path(path_str, required_path))
                        .unwrap_or(false)
                });

                if !satisfied {
                    let phase = meta.get("phase").and_then(|v| v.as_str()).unwrap_or("");
                    let phase_str = if phase.is_empty() {
                        "".to_string()
                    } else {
                        format!(" [Phase: {}]", phase)
                    };
                    violations.push(serde_json::json!({
                        "rule": format!("{} - {}", id, row[1]),
                        "diagnostic": format!("{}{} requires matching test coverage: '{}'.", id, phase_str, required_path),
                        "remediation_plan": format!("Add/update '{}' coverage, then retry axon_commit_work.", required_path)
                    }));
                }
            }
        }

        if !violations.is_empty() {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Violation: {}\nRemediation: {}", violations[0]["rule"], violations[0]["remediation_plan"]) }],
                "isError": true,
                "data": { "violations": violations }
            }));
        }

        if dry_run {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Validation passed (Dry Run). No commit performed. Message '{}' is valid.", message) }]
            }));
        }

        // REQ-AXO-126 — SOLL export no longer auto-fires on every commit.
        // The release-promotion pipeline owns the snapshot moment now.
        // See scripts/release/promote_live_safe.sh for the canonical
        // call site and `axon_export_soll` for the rationale.

        let mut add_cmd = std::process::Command::new("git");
        add_cmd.arg("add");
        for p in diff_paths {
            if let Some(path_str) = p.as_str() {
                add_cmd.arg(path_str);
            }
        }
        let add_out = add_cmd.output();
        if let Err(e) = add_out {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git add failed: {}", e) }],
                "isError": true
            }));
        }

        let commit_out = std::process::Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .output();

        match commit_out {
            Ok(output) => {
                let status = if output.status.success() {
                    format!(
                        "Commit succeeded.\n{}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                } else {
                    format!(
                        "Commit failed.\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                };
                Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Validation passed.\n\n{}", status) }]
                }))
            }
            Err(e) => Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git commit failed: {}", e) }],
                "isError": true
            })),
        }
    }

    // REQ-AXO-119 — kickoff bundle helpers. axon_init_project now returns
    // a stable bundle on every call (first-init AND re-init) so an LLM
    // that has only Axon MCP access can call axon_init_project once and
    // have everything it needs to begin productive work without
    // re-discovering the bootstrap protocol from scratch.

    fn read_soll_node_description(&self, id: &str) -> Option<String> {
        let escaped = escape_sql(id);
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT description FROM soll.main.Node WHERE id = '{}'",
                escaped
            ))
            .ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        rows.into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .filter(|s| !s.is_empty())
    }

    fn cold_start_entry_points() -> serde_json::Value {
        serde_json::json!([
            { "step": 1, "kind": "file", "target": "~/.claude/CLAUDE.md", "purpose": "cross-project standing rules" },
            { "step": 2, "kind": "file", "target": "<project_root>/CLAUDE.md", "purpose": "project-specific discipline" },
            { "step": 3, "kind": "file", "target": "<persistent_memory>/MEMORY.md", "purpose": "accumulated session memory and active handoff pointer" },
            { "step": 4, "kind": "mcp", "target": "mcp__axon__help", "purpose": "confirm MCP reachable, return Axon identity and tool routing" },
            { "step": 5, "kind": "mcp", "target": "mcp__axon__status mode=brief", "purpose": "runtime instance, profile, freshness, vector backlog" },
            { "step": 6, "kind": "cypher", "target": "SELECT id, title, description FROM soll.main.Node WHERE project_code = '<CODE>' AND type = 'Vision'", "purpose": "project Vision in full" },
            { "step": 7, "kind": "cypher", "target": "SELECT id, title, description FROM soll.main.Node WHERE project_code = '<CODE>' AND type = 'Pillar' ORDER BY id", "purpose": "every Pillar description in full" },
            { "step": 8, "kind": "cypher", "target": "SELECT id, title FROM soll.main.Node WHERE project_code = '<CODE>' AND type IN ('Decision','Milestone') AND status IN ('accepted','delivered','completed') ORDER BY id DESC LIMIT 30", "purpose": "already-completed work" },
            { "step": 9, "kind": "mcp", "target": "mcp__axon__soll_validate project_code=<CODE>", "purpose": "current SOLL invariant violations (target zero)" },
            { "step": 10, "kind": "mcp", "target": "mcp__axon__soll_work_plan project_code=<CODE> format=brief top=5 limit=15", "purpose": "scored topological order of unblockers; wave 1 score is authoritative" }
        ])
    }

    fn default_methodology_summary() -> &'static str {
        "Observe -> Log to SOLL -> Link to Pillar/Concept -> Re-plan via soll_work_plan -> Execute the highest-score wave-1 unblocker. Repeat. Interrupt the user only for destructive irreversible actions, architectural decisions needing human authority, hard blockers, or external-impact milestones (deploy/release/fix unblocking another human). Canonical reference: CPT-AXO-019 in soll.main.Node."
    }

    fn default_kickoff_prompt() -> &'static str {
        "Bootstrap prompt seed not yet in SOLL. Run mcp__axon__cypher with SELECT description FROM soll.main.Node WHERE id = 'DEC-PRO-001' once it is seeded. In the meantime, follow entry_points in the bundle in order, then enter the operational loop in methodology_summary."
    }

    fn find_active_handoff(project_path: &str) -> Option<String> {
        let dir = std::path::Path::new(project_path).join("docs").join("working-notes");
        if !dir.is_dir() {
            return None;
        }
        let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy().to_string();
                // Match docs/working-notes/<date>-handoff-*.md pattern.
                if !name.ends_with(".md") || !name.contains("-handoff-") {
                    return None;
                }
                let mtime = entry.metadata().ok()?.modified().ok()?;
                Some((mtime, path))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        candidates
            .into_iter()
            .next()
            .map(|(_, path)| path.to_string_lossy().to_string())
    }

    fn axon_init_project_bundle(&self, project_path: &str) -> serde_json::Value {
        let kickoff_prompt = self
            .read_soll_node_description("DEC-PRO-001")
            .unwrap_or_else(|| Self::default_kickoff_prompt().to_string());
        let methodology_summary = self
            .read_soll_node_description("CPT-AXO-019")
            .unwrap_or_else(|| Self::default_methodology_summary().to_string());
        serde_json::json!({
            "kickoff_prompt": kickoff_prompt,
            "kickoff_prompt_source": "soll://Node/DEC-PRO-001",
            "methodology_summary": methodology_summary,
            "methodology_summary_source": "soll://Node/CPT-AXO-019",
            "entry_points": Self::cold_start_entry_points(),
            "active_handoff": Self::find_active_handoff(project_path),
        })
    }

    pub(crate) fn axon_init_project(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_path = match args.get("project_path").and_then(|value| value.as_str()) {
            Some(path) if !path.trim().is_empty() => path.trim(),
            _ => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": "`project_path` is required for `axon_init_project`." }],
                    "isError": true
                }))
            }
        };
        // REQ-AXO-118 — flag (but do not reject) project_path values that do
        // not resolve to a real directory on disk. Earlier sessions accidentally
        // registered bogus paths via typos and the registry silently accepted
        // them, leading to hard-to-diagnose failures downstream when indexer/
        // qualify tried to use the path. Surfacing the condition in
        // data.warnings lets the LLM client (or operator) catch the typo at
        // registration time without breaking the legitimate "register a future
        // project" workflow.
        let path_exists_on_disk = std::path::Path::new(project_path).is_dir();
        let project_name = match self.derive_project_name_from_path(project_path) {
            Ok(name) => name,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        let project_code = match self.assign_project_code_for_init(&project_name, project_path) {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        if let Some(requested_code) = args.get("project_code").and_then(|value| value.as_str()) {
            let requested = match self
                .validate_explicit_canonical_project_code(Some(requested_code), "axon_init_project")
            {
                Ok(code) => code,
                Err(e) => {
                    return Some(serde_json::json!({
                        "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                        "isError": true
                    }))
                }
            };
            if requested != project_code {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: `project_code` is server-assigned. Omit it or use `{}` for this project.", project_code) }],
                    "isError": true
                }));
            }
        }
        let concept_text = args
            .get("concept_document_url_or_text")
            .and_then(|v| v.as_str());

        if let Err(e) = self.graph_store.sync_project_registry_entry(
            &project_code,
            Some(&project_name),
            Some(project_path),
        ) {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Project registration error: {}", e) }],
                "isError": true
            }));
        }
        if let Err(e) = self.ensure_soll_registry_row(&project_code) {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("SOLL initialization error for project: {}", e) }],
                "isError": true
            }));
        }

        let rows_raw = self.graph_store.query_json(
            "SELECT id, title, description, metadata FROM soll.Node WHERE type='Guideline' AND project_code='PRO'",
        ).unwrap_or_else(|_| "[]".to_string());

        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut rules_text = String::new();
        for row in rows {
            if row.len() >= 3 {
                rules_text.push_str(&format!("- **{}**: {} ({})\n", row[0], row[1], row[2]));
            }
        }

        let mut response_text = format!(
            "Project '{}' ({}) initialized in Axon.\n\n",
            project_name, project_code
        );

        if concept_text.is_some() {
            response_text.push_str(&format!(
                "📄 A concept document was detected. Extract the Vision and Pillars, then use `soll_manager` to create them under project {}.\n\n",
                project_code
            ));
        }

        response_text.push_str(&format!(
            "Server-assigned project code: `{}`.\n\n",
            project_code
        ));
        response_text.push_str("Available global rules. Which ones do you want to activate, ignore, or specialize for this project?\n");
        response_text.push_str(&rules_text);
        response_text
            .push_str("\n(Use `axon_apply_guidelines` to apply these choices).");

        let warnings: Vec<serde_json::Value> = if path_exists_on_disk {
            Vec::new()
        } else {
            // REQ-AXO-118 — non-blocking warning surfaced in data.warnings.
            // Also append to the LLM-visible content so a one-shot read
            // catches the typo at registration time.
            response_text.push_str(&format!(
                "\n\n⚠️  project_path `{}` does not currently exist on disk. The registry entry was created anyway; if this was a typo, run `axon_init_project` again with the corrected path or `mkdir -p` the directory.",
                project_path
            ));
            vec![serde_json::json!({
                "kind": "path_does_not_exist_on_disk",
                "path": project_path,
                "next_action": "verify the path or `mkdir -p` before relying on this project for indexer / qualify operations"
            })]
        };

        // REQ-AXO-119 — append the kickoff bundle pointer to the
        // human-readable response so an LLM scanning content alone
        // sees that the structured bundle is available in data.
        let bundle = self.axon_init_project_bundle(project_path);
        response_text.push_str(
            "\n\nKickoff bundle attached in `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, active_handoff). Use it to onboard yourself or any future LLM session before doing project-specific work.",
        );

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": response_text }],
            "data": {
                "project_code": project_code,
                "project_name": project_name,
                "project_path": project_path,
                "path_exists_on_disk": path_exists_on_disk,
                "warnings": warnings,
                "kickoff_bundle": bundle
            }
        }))
    }

    pub(crate) fn axon_apply_guidelines(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let project_code = match self.require_registered_mutation_project_code(
            args.get("project_code").and_then(|value| value.as_str()),
            "axon_apply_guidelines",
        ) {
            Ok(code) => code,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Canonical project error: {}", e) }],
                    "isError": true
                }))
            }
        };
        let accepted_ids = args.get("accepted_global_rule_ids")?.as_array()?;

        let mut applied: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for id_val in accepted_ids {
            let global_id = id_val.as_str().unwrap_or("").trim();
            if global_id.is_empty() {
                continue;
            }
            let row_raw = self.graph_store.query_json(&format!(
                "SELECT title, description, metadata FROM soll.Node WHERE id = '{}' AND type='Guideline'",
                escape_sql(global_id)
            )).unwrap_or_else(|_| "[]".to_string());

            let rows: Vec<Vec<String>> = serde_json::from_str(&row_raw).unwrap_or_default();
            if let Some(row) = rows.first() {
                if row.len() < 3 {
                    unknown.push(global_id.to_string());
                    continue;
                }
                let title = &row[0];
                let desc = &row[1];
                let meta = &row[2];

                let (_scope_code, p_code, prefix, num) = match self
                    .next_soll_numeric_id(&project_code, "guideline")
                {
                    Ok(parts) => parts,
                    Err(e) => {
                        return Some(serde_json::json!({
                            "content": [{ "type": "text", "text": format!("SOLL registry error: {}", e) }],
                            "isError": true
                        }))
                    }
                };
                let local_id = format!("{}-{}-{:03}", prefix, p_code, num);

                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                     VALUES (?, 'Guideline', ?, ?, ?, 'active', ?)",
                    &serde_json::json!([local_id, p_code, title, desc, meta])
                );

                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES (?, ?, 'INHERITS_FROM', '{}')",
                    &serde_json::json!([local_id, global_id])
                );

                applied.push(local_id);
            } else {
                unknown.push(global_id.to_string());
            }
        }

        // REQ-AXO-043 — silent-success contract gap. Previous behaviour
        // returned "Inheritance applied. New local rules created: []" for
        // both an empty input AND a list of all-unknown rule IDs, which
        // misled the LLM caller into thinking work happened. Surface the
        // unknown IDs as a recovery contract so the caller can retry with
        // valid IDs (discoverable via `cypher SELECT id, title FROM
        // soll.main.Node WHERE type='Guideline' AND project_code='PRO'`).
        let empty_input = accepted_ids.is_empty();
        let nothing_applied = applied.is_empty();
        let recovery_hint = "discover valid IDs via cypher SELECT id, title FROM soll.main.Node WHERE type='Guideline' AND project_code='PRO'";
        let text = if empty_input {
            format!(
                "axon_apply_guidelines requires at least one canonical Guideline ID in `accepted_global_rule_ids`. {recovery_hint}.")
        } else if !applied.is_empty() && !unknown.is_empty() {
            format!(
                "Inheritance applied. New local rules created: {applied:?}. Unknown global rule IDs (skipped): {unknown:?}.")
        } else if !applied.is_empty() {
            format!("Inheritance applied. New local rules created: {applied:?}")
        } else {
            format!(
                "No rules applied. All requested global rule IDs were unknown: {unknown:?}. {recovery_hint}.")
        };

        let mut data = serde_json::json!({
            "applied": applied,
            "unknown_global_rule_ids": unknown,
        });
        if empty_input {
            data["empty_input"] = serde_json::json!(true);
        }
        if empty_input || nothing_applied {
            data["recovery_hint"] = serde_json::json!(recovery_hint);
        }
        let mut response = serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "data": data,
        });
        if empty_input || nothing_applied {
            response["isError"] = serde_json::json!(true);
        }
        Some(response)
    }
}
