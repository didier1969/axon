use super::*;

fn path_satisfies_required_path(path: &str, required_path: &str) -> bool {
    if path.contains(required_path) {
        return true;
    }

    if required_path == "tests.rs" {
        let normalized = path.replace('\\', "/");
        return normalized.ends_with("_test.rs")
            || normalized.ends_with("_tests.rs")
            || normalized.ends_with("/tests.rs")
            || normalized.contains("/tests/")
            || normalized.contains("/test/");
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

        let export_args = serde_json::json!({});
        let export_res = self.axon_export_soll(&export_args);
        let mut export_report = String::new();
        if let Some(res) = export_res {
            if soll_tool_is_error(Some(&res)) {
                return Some(res);
            }
            if let Some(txt) = soll_tool_text(Some(&res)) {
                export_report = txt;
            }
        }

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
                    "content": [{ "type": "text", "text": format!("Validation passed.\n\n{}\n\nExport Report (not auto-staged):\n{}", status, export_report) }]
                }))
            }
            Err(e) => Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Git commit failed: {}", e) }],
                "isError": true
            })),
        }
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

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": response_text }],
            "data": {
                "project_code": project_code,
                "project_name": project_name,
                "project_path": project_path,
                "path_exists_on_disk": path_exists_on_disk,
                "warnings": warnings
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

        let mut applied = Vec::new();
        for id_val in accepted_ids {
            let global_id = id_val.as_str().unwrap_or("");
            let row_raw = self.graph_store.query_json(&format!(
                "SELECT title, description, metadata FROM soll.Node WHERE id = '{}' AND type='Guideline'",
                escape_sql(global_id)
            )).unwrap_or_else(|_| "[]".to_string());

            let rows: Vec<Vec<String>> = serde_json::from_str(&row_raw).unwrap_or_default();
            if let Some(row) = rows.first() {
                if row.len() < 3 {
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
            }
        }

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": format!("Inheritance applied. New local rules created: {:?}", applied) }]
        }))
    }
}
