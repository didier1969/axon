use super::*;

// REQ-AXO-147 — universal parameter_repair contract rollout for
// workflow_project.rs (REQ-AXO-139 follow-up). 12 bare isError sites in
// axon_init_project / axon_apply_guidelines / commit_work share the same
// recovery shape so the LLM can route on `data.parameter_repair`.
fn project_workflow_error(
    invalid_field: &str,
    supplied_value: Option<&str>,
    follow_up: &[&str],
    visible_text: String,
    hint: &str,
    err: Option<&str>,
) -> serde_json::Value {
    let follow_up_vec: Vec<String> = follow_up.iter().map(|s| s.to_string()).collect();
    serde_json::json!({
        "content": [{ "type": "text", "text": visible_text }],
        "isError": true,
        "data": {
            "status": "input_invalid",
            "operator_guidance": {
                "problem_class": "input_invalid",
                "follow_up_tools": follow_up_vec.clone(),
            },
            "parameter_repair": {
                "invalid_field": invalid_field,
                "supplied_value": supplied_value,
                "follow_up_tools": follow_up_vec,
                "hint": hint,
            },
            "diagnostic_excerpt": err.map(|e| e.chars().take(240).collect::<String>()).unwrap_or_default()
        }
    })
}

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

        // REQ-AXO-138 — auto-stage every diff_path so Edit/Write modifications
        // never silently drop out of the commit. The previous implementation
        // ran `git add` but only failed if the process couldn't spawn — exit-
        // code failures (path missing, ignore conflict, etc.) passed through
        // silently and the subsequent `git commit` captured only whatever
        // was pre-staged elsewhere (e.g. earlier `git rm`). The result was
        // commits that referenced symbols absent from HEAD, breaking bisect.
        //
        // Fix: status-check the add invocation and return a structured error
        // listing the offending stderr so the caller can repair before retry.
        let mut add_cmd = std::process::Command::new("git");
        add_cmd.arg("add");
        for p in diff_paths {
            if let Some(path_str) = p.as_str() {
                add_cmd.arg(path_str);
            }
        }
        let add_out = match add_cmd.output() {
            Ok(output) => output,
            Err(e) => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": format!("Git add spawn failed: {}", e) }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "next_action": {
                            "kind": "verify_paths_then_retry",
                            "tool": "axon_commit_work",
                            "when": "after_paths_resolved"
                        },
                        "operator_guidance": {
                            "problem_class": "git_invocation_failure",
                            "follow_up_tools": ["axon_pre_flight_check"],
                        }
                    }
                }));
            }
        };
        if !add_out.status.success() {
            let stderr = String::from_utf8_lossy(&add_out.stderr);
            let stdout = String::from_utf8_lossy(&add_out.stdout);
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!(
                    "Git add failed (exit {}). Refusing to commit a partial diff. stderr: {}",
                    add_out.status.code().unwrap_or(-1),
                    stderr.trim()
                )}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "git_add_exit_code": add_out.status.code(),
                    "git_add_stderr": stderr,
                    "git_add_stdout": stdout,
                    "next_action": {
                        "kind": "fix_path_then_retry",
                        "tool": "axon_commit_work",
                        "when": "after_paths_resolved"
                    },
                    "operator_guidance": {
                        "problem_class": "git_add_rejected_paths",
                        "follow_up_tools": ["axon_pre_flight_check"],
                    },
                    "parameter_repair": {
                        "invalid_field": "diff_paths",
                        "hint": "verify each path exists relative to repo root and is not gitignored, then retry"
                    }
                }
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
            Err(e) => Some(project_workflow_error(
                "git_environment",
                None,
                &["axon_pre_flight_check", "status"],
                format!("Git commit failed: {}", e),
                "git commit invocation failed; verify the git binary is on PATH and the repo is in a valid state, then retry `axon_pre_flight_check`",
                Some(&e.to_string()),
            )),
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

    /// REQ-AXO-143 — validate a session_pointer JSON object supplied via
    /// `axon_init_project` arg. Returns the canonical normalized form
    /// `{kind, value, label?}` or an error message describing the
    /// rejection. `kind` ∈ `file|url|soll_node|none`. `value` is required
    /// for the first three kinds; ignored for `none`.
    fn validate_session_pointer(
        pointer: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let object = pointer
            .as_object()
            .ok_or_else(|| "session_pointer must be a JSON object".to_string())?;
        let kind = object
            .get("kind")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .ok_or_else(|| "session_pointer.kind is required (file|url|soll_node|none)".to_string())?;
        if !matches!(kind, "file" | "url" | "soll_node" | "none") {
            return Err(format!(
                "session_pointer.kind must be one of file|url|soll_node|none (got `{}`)",
                kind
            ));
        }
        let label = object
            .get("label")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if kind == "none" {
            let mut canonical = serde_json::Map::new();
            canonical.insert("kind".to_string(), serde_json::Value::from("none"));
            canonical.insert("value".to_string(), serde_json::Value::Null);
            if let Some(label_value) = label {
                canonical.insert("label".to_string(), serde_json::Value::from(label_value));
            }
            return Ok(serde_json::Value::Object(canonical));
        }
        let value = object
            .get("value")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "session_pointer.value is required when kind=`{}` (non-empty string)",
                    kind
                )
            })?;
        let mut canonical = serde_json::Map::new();
        canonical.insert("kind".to_string(), serde_json::Value::from(kind));
        canonical.insert("value".to_string(), serde_json::Value::from(value));
        if let Some(label_value) = label {
            canonical.insert("label".to_string(), serde_json::Value::from(label_value));
        }
        Ok(serde_json::Value::Object(canonical))
    }

    /// REQ-AXO-143 — surface the per-project session pointer for the
    /// kickoff bundle and `status` instance_identity. Reads the persisted
    /// JSON from the project registry first; falls back to synthesizing
    /// a `{kind:"file", value:<path>}` shape from `find_active_handoff`
    /// so legacy AXO-style projects keep working without explicit
    /// configuration.
    pub(crate) fn resolve_session_pointer(
        &self,
        project_code: &str,
        project_path: Option<&str>,
    ) -> serde_json::Value {
        if let Ok(Some(value)) = self.graph_store.read_session_pointer(project_code) {
            return value;
        }
        if let Some(path) = project_path {
            if let Some(handoff) = Self::find_active_handoff(path) {
                return serde_json::json!({
                    "kind": "file",
                    "value": handoff,
                    "label": "synthesized from docs/working-notes/<date>-handoff-*.md (legacy fallback)"
                });
            }
        }
        serde_json::Value::Null
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

    fn axon_init_project_bundle(
        &self,
        project_code: &str,
        project_path: &str,
    ) -> serde_json::Value {
        let kickoff_prompt = self
            .read_soll_node_description("DEC-PRO-001")
            .unwrap_or_else(|| Self::default_kickoff_prompt().to_string());
        let methodology_summary = self
            .read_soll_node_description("CPT-AXO-019")
            .unwrap_or_else(|| Self::default_methodology_summary().to_string());
        // REQ-AXO-143 — `session_pointer` is the canonical workflow-agnostic
        // onboarding pointer. `active_handoff` is retained as a backward-compat
        // alias for one release cycle (LLM clients that already read
        // `bundle.active_handoff` keep working). When session_pointer.kind is
        // `file`, active_handoff mirrors session_pointer.value; otherwise null.
        let session_pointer = self.resolve_session_pointer(project_code, Some(project_path));
        let active_handoff_alias = match session_pointer.get("kind").and_then(|v| v.as_str()) {
            Some("file") => session_pointer
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| serde_json::Value::from(s.to_string()))
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        serde_json::json!({
            "kickoff_prompt": kickoff_prompt,
            "kickoff_prompt_source": "soll://Node/DEC-PRO-001",
            "methodology_summary": methodology_summary,
            "methodology_summary_source": "soll://Node/CPT-AXO-019",
            "entry_points": Self::cold_start_entry_points(),
            "session_pointer": session_pointer,
            "active_handoff": active_handoff_alias,
        })
    }

    pub(crate) fn axon_init_project(&self, args: &serde_json::Value) -> Option<serde_json::Value> {
        let project_path = match args.get("project_path").and_then(|value| value.as_str()) {
            Some(path) if !path.trim().is_empty() => path.trim(),
            _ => {
                return Some(project_workflow_error(
                    "project_path",
                    None,
                    &["help"],
                    "`project_path` is required for `axon_init_project`.".to_string(),
                    "supply an absolute project path; example: `/home/user/projects/myrepo`",
                    None,
                ))
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
                return Some(project_workflow_error(
                    "project_path",
                    Some(project_path),
                    &["help"],
                    format!("Project error: {}", e),
                    "could not derive project_name from the supplied path; verify the path resolves to a directory with a sensible last segment",
                    Some(&e.to_string()),
                ))
            }
        };
        let project_code = match self.assign_project_code_for_init(&project_name, project_path) {
            Ok(code) => code,
            Err(e) => {
                return Some(project_workflow_error(
                    "project_path",
                    Some(project_path),
                    &["project_registry_lookup"],
                    format!("Canonical project error: {}", e),
                    "automatic project_code assignment failed; the registry may already contain an incompatible entry. Run `project_registry_lookup` to inspect, or supply an explicit `project_code`",
                    Some(&e.to_string()),
                ))
            }
        };
        if let Some(requested_code) = args.get("project_code").and_then(|value| value.as_str()) {
            let requested = match self
                .validate_explicit_canonical_project_code(Some(requested_code), "axon_init_project")
            {
                Ok(code) => code,
                Err(e) => {
                    return Some(project_workflow_error(
                        "project_code",
                        Some(requested_code),
                        &["help"],
                        format!("Canonical project error: {}", e),
                        "the supplied `project_code` is malformed (must be 3 ASCII alphanumerics, conventionally uppercase); omit it to let the server assign one",
                        Some(&e.to_string()),
                    ))
                }
            };
            if requested != project_code {
                return Some(project_workflow_error(
                    "project_code",
                    Some(requested_code),
                    &["axon_init_project"],
                    format!("Canonical project error: `project_code` is server-assigned. Omit it or use `{}` for this project.", project_code),
                    &format!("omit `project_code` (server-assigned) or pass `{}` for this project", project_code),
                    None,
                ));
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
            return Some(project_workflow_error(
                "project_path",
                Some(project_path),
                &["status", "project_registry_lookup"],
                format!("Project registration error: {}", e),
                "writing to soll.ProjectCodeRegistry failed; verify runtime is healthy via `status` and inspect the registry via `project_registry_lookup`",
                Some(&e.to_string()),
            ));
        }
        if let Err(e) = self.ensure_soll_registry_row(&project_code) {
            return Some(project_workflow_error(
                "project_code",
                Some(&project_code),
                &["status"],
                format!("SOLL initialization error for project: {}", e),
                "seeding soll.Registry counters failed; verify runtime is healthy via `status` and retry",
                Some(&e.to_string()),
            ));
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

        // REQ-AXO-143 — accept and persist an optional session_pointer arg
        // BEFORE building the kickoff bundle so the bundle reads back the
        // freshly-stored value. See `validate_session_pointer` for the
        // canonical shape `{kind: file|url|soll_node|none, value, label?}`.
        if let Some(pointer_arg) = args.get("session_pointer") {
            if !pointer_arg.is_null() {
                let canonical = match Self::validate_session_pointer(pointer_arg) {
                    Ok(value) => value,
                    Err(message) => {
                        return Some(project_workflow_error(
                            "session_pointer",
                            None,
                            &["help", "axon_init_project"],
                            format!("Session pointer error: {}", message),
                            "session_pointer must be {kind: file|url|soll_node|none, value, label?}; omit or set null to clear",
                            None,
                        ));
                    }
                };
                if let Err(e) = self
                    .graph_store
                    .write_session_pointer(&project_code, Some(&canonical))
                {
                    return Some(project_workflow_error(
                        "session_pointer",
                        None,
                        &["status"],
                        format!("Session pointer write error: {}", e),
                        "writing session_pointer to soll.ProjectCodeRegistry failed; verify runtime is healthy via `status` and retry",
                        Some(&e.to_string()),
                    ));
                }
            } else {
                // Explicit null clears any prior pointer.
                let _ = self.graph_store.write_session_pointer(&project_code, None);
            }
        }

        // REQ-AXO-119 — append the kickoff bundle pointer to the
        // human-readable response so an LLM scanning content alone
        // sees that the structured bundle is available in data.
        let bundle = self.axon_init_project_bundle(&project_code, project_path);
        response_text.push_str(
            "\n\nKickoff bundle attached in `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, session_pointer, active_handoff). Use it to onboard yourself or any future LLM session before doing project-specific work.",
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
        let supplied_project = args.get("project_code").and_then(|value| value.as_str()).unwrap_or("");
        let project_code = match self.require_registered_mutation_project_code(
            args.get("project_code").and_then(|value| value.as_str()),
            "axon_apply_guidelines",
        ) {
            Ok(code) => code,
            Err(e) => {
                return Some(project_workflow_error(
                    "project_code",
                    Some(supplied_project),
                    &["project_registry_lookup", "axon_init_project"],
                    format!("Canonical project error: {}", e),
                    "supply a registered `project_code`; call `project_registry_lookup` to list registered codes or `axon_init_project` first",
                    Some(&e.to_string()),
                ))
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
                        return Some(project_workflow_error(
                            "project_code",
                            Some(&project_code),
                            &["status", "project_registry_lookup"],
                            format!("SOLL registry error: {}", e),
                            "soll.Registry id-reservation failed during guideline import; verify runtime health via `status`",
                            Some(&e.to_string()),
                        ))
                    }
                };
                let local_id = format!("{}-{}-{:03}", prefix, p_code, num);

                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                     VALUES (?, 'Guideline', ?, ?, ?, 'active', ?)",
                    &serde_json::json!([local_id, p_code, title, desc, meta])
                );

                // REQ-AXO-152: project_code on INSERT. p_code is in scope (the
                // local tenant the Guideline is being inherited into), so use it
                // directly rather than re-deriving from canonical ID prefixes.
                let _ = self.graph_store.execute_param(
                    "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) VALUES (?, ?, 'INHERITS_FROM', '{}', ?)",
                    &serde_json::json!([local_id, global_id, p_code])
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
