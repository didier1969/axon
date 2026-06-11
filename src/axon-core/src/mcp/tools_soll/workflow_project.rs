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
/// REQ-AXO-91569 — true when the Conventional-Commits message starts
/// with the `refactor` type token (`refactor:` or `refactor(<scope>):`).
/// Leading whitespace and a single optional `!` (breaking-change
/// marker, e.g. `refactor(api)!:`) are tolerated. The check is
/// case-sensitive per Conventional-Commits 1.0.
fn commit_message_is_refactor(message: &str) -> bool {
    let trimmed = message.trim_start();
    if !trimmed.starts_with("refactor") {
        return false;
    }
    let after = &trimmed["refactor".len()..];
    let after = after.strip_prefix('!').unwrap_or(after);
    match after.chars().next() {
        Some(':') => true,
        Some('(') => after.contains(')'),
        _ => false,
    }
}

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
        // REQ-AXO-191 (Fiscaly P1.2) — when `project_path` is supplied
        // (or resolvable from `project_code` via the registry), run
        // every git command with that path as `current_dir`. Without
        // this guard the commands run against whatever cwd the brain
        // process happens to hold (usually the Axon repo), so a
        // cross-project commit silently lands in the wrong tree.
        let explicit_project_path = args
            .get("project_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from);
        let project_code_arg = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let resolved_project_path: Option<std::path::PathBuf> = explicit_project_path
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| project_code_arg.and_then(|code| self.lookup_project_path_by_code(code)));

        // REQ-AXO-91571 — gate scope to `PRO` (cross-project canonical
        // guidelines) + the effective project's own guidelines. Without
        // this filter, every other project's duplicates of GUI-PRO-001
        // / GUI-PRO-002 (TDD / Documentation MCP) would fire on every
        // AXO commit (observed session 43 : GUI-FSF-002, GUI-MLD-002,
        // GUI-NEX-001, GUI-TE2-001 leaking from sibling projects).
        let effective_project_code: Option<String> =
            project_code_arg.map(str::to_string).or_else(|| {
                resolved_project_path
                    .as_ref()
                    .and_then(|p| self.lookup_project_code_by_path(p))
            });
        let guideline_scope_filter = match effective_project_code.as_deref() {
            Some(code) => format!("AND project_code IN ('PRO', '{}')", escape_sql(code)),
            None => "AND project_code = 'PRO'".to_string(),
        };
        let rows_raw = self
            .graph_store
            .query_json(&format!(
                "SELECT id, title, description, metadata FROM soll.Node \
                 WHERE type='Guideline' AND status='active' {}",
                guideline_scope_filter
            ))
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

            // REQ-AXO-91569 — diff-aware exemption: when the guideline
            // metadata sets `exempt_for_refactor: true` and the commit
            // message starts with the Conventional-Commits `refactor`
            // type (with or without scope), the gate steps aside. The
            // signal is intentionally narrow: only the `refactor:` /
            // `refactor(<scope>):` prefix qualifies. `feat:` / `fix:` /
            // `chore:` / etc. stay strictly gated.
            let exempt_for_refactor = meta
                .get("exempt_for_refactor")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if exempt_for_refactor && commit_message_is_refactor(message) {
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
        if let Some(dir) = resolved_project_path.as_ref() {
            add_cmd.current_dir(dir);
        }
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

        let mut commit_cmd = std::process::Command::new("git");
        if let Some(dir) = resolved_project_path.as_ref() {
            commit_cmd.current_dir(dir);
        }
        let commit_out = commit_cmd.arg("commit").arg("-m").arg(message).output();

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

    /// REQ-AXO-91571 — reverse of `lookup_project_path_by_code`. Maps
    /// an absolute repo path back to its canonical project_code via
    /// the registry. Used by the pre-flight gate to scope guideline
    /// queries to (`PRO`, effective_project) so sibling-project
    /// duplicates don't leak into commit validation.
    fn lookup_project_code_by_path(&self, path: &std::path::Path) -> Option<String> {
        let path_str = path.to_str()?;
        let escaped = escape_sql(path_str);
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT project_code FROM {} WHERE project_path = '{}'",
                self.graph_store.soll_table("ProjectCodeRegistry"),
                escaped
            ))
            .ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let code = rows.into_iter().next()?.into_iter().next()?;
        if code.trim().is_empty() {
            None
        } else {
            Some(code)
        }
    }

    /// REQ-AXO-191 — resolve a project_code to its absolute
    /// `project_path` via the registry. Used by `axon_commit_work` to
    /// set the git command's `current_dir` so cross-project commits
    /// land in the correct tree.
    fn lookup_project_path_by_code(&self, code: &str) -> Option<std::path::PathBuf> {
        let escaped = escape_sql(code);
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT project_path FROM {} WHERE project_code = '{}'",
                self.graph_store.soll_table("ProjectCodeRegistry"),
                escaped
            ))
            .ok()?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).ok()?;
        let path = rows.into_iter().next()?.into_iter().next()?;
        if path.trim().is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(path))
        }
    }

    fn read_soll_node_description(&self, id: &str) -> Option<String> {
        let escaped = escape_sql(id);
        let raw = self
            .graph_store
            .query_json(&format!(
                "SELECT description FROM {} WHERE id = '{}'",
                self.graph_store.soll_table("Node"),
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
            { "step": 6, "kind": "sql", "target": "SELECT id, title, description FROM soll.Node WHERE project_code = '<CODE>' AND type = 'Vision'", "purpose": "project Vision in full" },
            { "step": 7, "kind": "sql", "target": "SELECT id, title, description FROM soll.Node WHERE project_code = '<CODE>' AND type = 'Pillar' ORDER BY id", "purpose": "every Pillar description in full" },
            { "step": 8, "kind": "sql", "target": "SELECT id, title FROM soll.Node WHERE project_code = '<CODE>' AND type IN ('Decision','Milestone') AND status IN ('accepted','delivered','completed') ORDER BY id DESC LIMIT 30", "purpose": "already-completed work" },
            { "step": 9, "kind": "mcp", "target": "mcp__axon__soll_validate project_code=<CODE>", "purpose": "current SOLL invariant violations (target zero)" },
            { "step": 10, "kind": "mcp", "target": "mcp__axon__soll_work_plan project_code=<CODE> format=brief top=5 limit=15", "purpose": "scored topological order of unblockers; wave 1 score is authoritative" }
        ])
    }

    fn default_methodology_summary() -> &'static str {
        "Observe -> Log to SOLL -> Link to Pillar/Concept -> Re-plan via soll_work_plan -> Execute the highest-score wave-1 unblocker. Repeat. Interrupt the user only for destructive irreversible actions, architectural decisions needing human authority, hard blockers, or external-impact milestones (deploy/release/fix unblocking another human). Canonical reference: CPT-AXO-019 in soll.Node."
    }

    fn default_kickoff_prompt() -> &'static str {
        "Bootstrap prompt seed not yet in SOLL. Run mcp__axon__sql with SELECT description FROM soll.Node WHERE id = 'DEC-PRO-001' once it is seeded. In the meantime, follow entry_points in the bundle in order, then enter the operational loop in methodology_summary."
    }

    /// REQ-AXO-143 — validate a session_pointer JSON object supplied via
    /// `axon_init_project` arg. Returns the canonical normalized form
    /// `{kind, value, label?}` or an error message describing the
    /// rejection. `kind` ∈ `file|url|soll_node|none`. `value` is required
    /// for the first three kinds; ignored for `none`.
    fn validate_session_pointer(pointer: &serde_json::Value) -> Result<serde_json::Value, String> {
        let object = pointer
            .as_object()
            .ok_or_else(|| "session_pointer must be a JSON object".to_string())?;
        let kind = object
            .get("kind")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .ok_or_else(|| {
                "session_pointer.kind is required (file|url|soll_node|none)".to_string()
            })?;
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
        let dir = std::path::Path::new(project_path)
            .join("docs")
            .join("working-notes");
        if !dir.is_dir() {
            return None;
        }
        let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf)> =
            std::fs::read_dir(&dir)
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

    // REQ-AXO-176 — bundle enrichment helpers. Aggregate the recent
    // activity an LLM would otherwise discover via 3-4 separate MCP/Bash
    // calls. The four fields are scoped per project; empty arrays are
    // returned when no rows match. Bounded LIMITs guarantee a sparse
    // project does not bloat the response.

    fn read_in_progress_requirements(&self, project_code: &str, limit: usize) -> serde_json::Value {
        // DEC-AXO-091 / REQ-AXO-322 (v3) — snapshot-driven kickoff
        // helper: iterate Requirement nodes, filter status=in_progress,
        // sort by metadata.updated_at DESC, take limit.
        let Ok(snapshot) = self.soll_cache().snapshot(project_code) else {
            return serde_json::json!([]);
        };
        let mut rows: Vec<(&String, &String, i64, String)> = snapshot
            .node_ids_of_type("Requirement")
            .iter()
            .filter_map(|id| snapshot.nodes.get(id).map(|n| (id, n)))
            .filter(|(_, n)| n.status == "in_progress")
            .map(|(id, n)| {
                let meta: serde_json::Value =
                    serde_json::from_str(&n.metadata_raw).unwrap_or(serde_json::json!({}));
                let updated_at = meta
                    .get("updated_at")
                    .and_then(|v| match v {
                        serde_json::Value::String(s) => s.parse::<i64>().ok(),
                        serde_json::Value::Number(num) => num.as_i64(),
                        _ => None,
                    })
                    .unwrap_or(i64::MIN);
                let priority = meta
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (id, &n.title, updated_at, priority)
            })
            .collect();
        rows.sort_by(|a, b| b.2.cmp(&a.2));
        rows.truncate(limit);
        serde_json::Value::Array(
            rows.into_iter()
                .map(|(id, title, _, priority)| {
                    serde_json::json!({
                        "id": id,
                        "title": title,
                        "priority": if priority.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::from(priority)
                        },
                    })
                })
                .collect(),
        )
    }

    fn read_recent_soll_writes(&self, project_code: &str, limit: usize) -> serde_json::Value {
        // DEC-AXO-091 / REQ-AXO-322 (v3) — snapshot-driven: rank all
        // SOLL nodes by metadata.updated_at DESC, take limit.
        let Ok(snapshot) = self.soll_cache().snapshot(project_code) else {
            return serde_json::json!([]);
        };
        let mut rows: Vec<(&String, &String, &String, i64)> = snapshot
            .nodes
            .values()
            .map(|n| {
                let meta: serde_json::Value =
                    serde_json::from_str(&n.metadata_raw).unwrap_or(serde_json::json!({}));
                let updated_at = meta
                    .get("updated_at")
                    .and_then(|v| match v {
                        serde_json::Value::String(s) => s.parse::<i64>().ok(),
                        serde_json::Value::Number(num) => num.as_i64(),
                        _ => None,
                    })
                    .unwrap_or(i64::MIN);
                (&n.id, &n.entity_type, &n.title, updated_at)
            })
            .collect();
        rows.sort_by(|a, b| b.3.cmp(&a.3));
        rows.truncate(limit);
        serde_json::Value::Array(
            rows.into_iter()
                .map(|(id, entity_type, title, updated_at)| {
                    let updated_value = if updated_at == i64::MIN {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::from(updated_at.to_string())
                    };
                    serde_json::json!({
                        "id": id,
                        "type": entity_type,
                        "title": title,
                        "updated_at": updated_value,
                    })
                })
                .collect(),
        )
    }

    fn read_recent_req_commits(project_path: &str, limit: usize) -> serde_json::Value {
        // REQ-AXO-287 — bound the git shell-out so a slow repo (large
        // pack, lock contention, fs latency) can't trip the MCP
        // gateway 30-s timeout on `axon_init_project`. 2-second budget
        // is generous for a `--max-count=20` filtered log on any
        // reasonable repo ; over-budget = return empty so the rest of
        // the kickoff bundle still completes.
        let child = match std::process::Command::new("git")
            .arg("-C")
            .arg(project_path)
            .arg("log")
            .arg("--oneline")
            .arg(format!("--max-count={}", limit * 4))
            .arg("-E")
            .arg("--grep=REQ-[A-Z]+-[0-9]+")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return serde_json::json!([]),
        };

        let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<std::process::Output>>();
        // Move `child` into the wait thread ; if the timeout fires we
        // kill the process via a separate handle obtained before the
        // move. `Child::id()` gives us the pid for a signal-based kill.
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });

        let output = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(o)) if o.status.success() => o,
            Ok(_) => return serde_json::json!([]),
            Err(_) => {
                // Timed out — best-effort kill via shell. The dangling
                // thread will eventually exit when `git log` finishes
                // or the kill takes effect.
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .output();
                return serde_json::json!([]);
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::Value::Array(
            stdout
                .lines()
                .take(limit)
                .filter_map(|line| {
                    let mut parts = line.splitn(2, ' ');
                    let sha = parts.next()?.to_string();
                    let subject = parts.next()?.to_string();
                    Some(serde_json::json!({
                        "sha": sha,
                        "subject": subject,
                    }))
                })
                .collect(),
        )
    }

    fn read_wave_1_unblockers(&self, project_code: &str) -> serde_json::Value {
        // DEC-AXO-091 / REQ-AXO-322 (v3) — cold-start fast path is now
        // entirely snapshot-driven. Original SQL ordered by priority
        // CASE + updated_at DESC; same logic applied to the in-memory
        // Requirement set. The full prioritization (cycle / descendant
        // analysis) remains in `soll_work_plan` for LLMs that ask for
        // it after onboarding.
        let Ok(snapshot) = self.soll_cache().snapshot(project_code) else {
            return serde_json::json!([]);
        };
        fn priority_rank(p: &str) -> u8 {
            match p {
                "critical" => 0,
                "high" => 1,
                "medium" => 2,
                "low" => 3,
                _ => 4,
            }
        }
        let mut rows: Vec<(&String, &String, &String, i64, String)> = snapshot
            .node_ids_of_type("Requirement")
            .iter()
            .filter_map(|id| snapshot.nodes.get(id).map(|n| (id, n)))
            .filter(|(_, n)| matches!(n.status.as_str(), "proposed" | "in_progress"))
            .map(|(id, n)| {
                let meta: serde_json::Value =
                    serde_json::from_str(&n.metadata_raw).unwrap_or(serde_json::json!({}));
                let updated_at = meta
                    .get("updated_at")
                    .and_then(|v| match v {
                        serde_json::Value::String(s) => s.parse::<i64>().ok(),
                        serde_json::Value::Number(num) => num.as_i64(),
                        _ => None,
                    })
                    .unwrap_or(i64::MIN);
                let priority = meta
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (id, &n.title, &n.status, updated_at, priority)
            })
            .collect();
        rows.sort_by(|a, b| {
            priority_rank(&a.4)
                .cmp(&priority_rank(&b.4))
                .then_with(|| b.3.cmp(&a.3))
        });
        rows.truncate(3);
        serde_json::Value::Array(
            rows.into_iter()
                .map(|(id, title, status, _, priority)| {
                    let priority_value = if priority.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::from(priority)
                    };
                    serde_json::json!({
                        "id": id,
                        "entity_type": "Requirement",
                        "title": title,
                        "score": serde_json::Value::Null,
                        "wave_index": serde_json::Value::Null,
                        "kind": status,
                        "reason": "kickoff fast path: ranked by priority + recency. Call `soll_work_plan` for cycle/blocker/validation analysis.",
                        "validation_gates": serde_json::json!({}),
                        "priority": priority_value,
                    })
                })
                .collect(),
        )
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
        // REQ-AXO-176 — kickoff bundle enrichment: aggregate recent project
        // activity inline so a fresh LLM session reaches productive state
        // from a single MCP call.
        let in_progress_requirements = self.read_in_progress_requirements(project_code, 10);
        let wave_1_unblockers = self.read_wave_1_unblockers(project_code);
        let recent_req_commits = Self::read_recent_req_commits(project_path, 5);
        let recent_soll_writes = self.read_recent_soll_writes(project_code, 8);
        // REQ-AXO-278 — Bootstrap-vs-Continuation phase detection (GUI-PRO-026).
        // When VIS-{project_code}-001 is absent, a Vision-less project is in
        // Bootstrap phase: the consumer-side LLM (via /axon-driven-development
        // skill) must invoke /bootstrap-soll to derive macro structure from
        // input_documents[]. When VIS exists, project is in Continuation phase.
        let bootstrap_required = self.bootstrap_required(project_code);
        let input_documents = if bootstrap_required {
            Self::scan_input_documents(project_path)
        } else {
            serde_json::json!([])
        };
        serde_json::json!({
            "kickoff_prompt": kickoff_prompt,
            "kickoff_prompt_source": "soll://Node/DEC-PRO-001",
            "methodology_summary": methodology_summary,
            "methodology_summary_source": "soll://Node/CPT-AXO-019",
            "entry_points": Self::cold_start_entry_points(),
            "session_pointer": session_pointer,
            "active_handoff": active_handoff_alias,
            "in_progress_requirements": in_progress_requirements,
            "wave_1_unblockers": wave_1_unblockers,
            "recent_req_commits": recent_req_commits,
            "recent_soll_writes": recent_soll_writes,
            "bootstrap_required": bootstrap_required,
            "input_documents": input_documents,
        })
    }

    fn bootstrap_required(&self, project_code: &str) -> bool {
        // DEC-AXO-091 / REQ-AXO-322 (v3) — snapshot-driven: bootstrap
        // phase = no Vision node yet. Replaces SQL existence probe.
        match self.soll_cache().snapshot(project_code) {
            Ok(snapshot) => snapshot.node_ids_of_type("Vision").is_empty(),
            Err(_) => false,
        }
    }

    fn scan_input_documents(project_path: &str) -> serde_json::Value {
        let root = std::path::Path::new(project_path);
        if !root.is_dir() {
            return serde_json::json!([]);
        }
        let patterns = ["README", "vision", "brief", "PRD", "CONTEXT"];
        let mut hits: Vec<serde_json::Value> = Vec::new();
        let read_dir = match std::fs::read_dir(root) {
            Ok(rd) => rd,
            Err(_) => return serde_json::json!([]),
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let lower = name.to_lowercase();
            let is_md = lower.ends_with(".md");
            let matches_pattern = patterns
                .iter()
                .any(|p| lower.starts_with(&p.to_lowercase()));
            if !(is_md || matches_pattern) {
                continue;
            }
            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime_unix = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            hits.push(serde_json::json!({
                "path": path.to_string_lossy(),
                "size_bytes": metadata.len(),
                "mtime_unix_secs": mtime_unix,
            }));
        }
        // stable order: by path
        hits.sort_by(|a, b| {
            a.get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .cmp(b.get("path").and_then(|v| v.as_str()).unwrap_or(""))
        });
        serde_json::Value::Array(hits)
    }

    /// REQ-AXO-901606 — auto-seed minimal Vision + Pillar so the bootstrap
    /// flow is not blocked by `soll_manager(action=create, entity=vision)`
    /// rejection (manager.rs:334 reserves Vision creation to this path).
    ///
    /// Idempotent : returns `false` (no-op) when the project already has at
    /// least one Vision row. Otherwise allocates canonical IDs via
    /// `soll.allocate_node_id`, inserts Vision + Pillar with
    /// `status='planned'` (canonical DEC-PRO-100 vocabulary ; closer to
    /// « draft » semantically while honoring the soll_node_status_canonical
    /// check constraint) so the operator/LLM can refine via
    /// `soll_manager(action=update)` without a special bootstrap codepath.
    /// The Pillar EPITOMIZES the Vision (canonical PIL→VIS edge per
    /// soll_relation_schema).
    ///
    /// Failure modes are best-effort : individual write errors log via
    /// `tracing::warn` and the function returns `false`. The init flow does
    /// NOT abort on seed failure since the project registry entry is the
    /// real critical resource.
    fn seed_default_vision_and_pillar(
        &self,
        project_code: &str,
        concept_text: Option<&str>,
    ) -> bool {
        let escaped_code = escape_sql(project_code);
        let vision_count = self
            .graph_store
            .query_count(&format!(
                "SELECT COUNT(*) FROM soll.Node WHERE type='Vision' AND project_code='{}'",
                escaped_code
            ))
            .unwrap_or(0);
        if vision_count > 0 {
            return false;
        }

        // Allocate canonical Vision id via PG function (REQ-AXO-90006 gap-skip aware)
        let vis_id = match self
            .graph_store
            .query_json(&format!(
                "SELECT soll.allocate_node_id('Vision', '{}')",
                escaped_code
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
        {
            Some(id) if !id.is_empty() => id,
            _ => {
                tracing::warn!(project_code = %project_code, "Vision id allocation failed; skipping auto-seed");
                return false;
            }
        };

        let description = concept_text.unwrap_or(
            "North-star draft — auto-seeded by axon_init_project. Edit via `soll_manager(action=update, entity=vision, data={id, description, ...})` to populate. Resolved REQ-AXO-901606 bootstrap blocker."
        );
        let metadata = serde_json::json!({
            "auto_seeded": true,
            "seeded_by": "axon_init_project",
            "req_ref": "REQ-AXO-901606"
        })
        .to_string();

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
             VALUES (?, 'Vision', ?, ?, ?, 'planned', ?)",
            &serde_json::json!([
                vis_id.clone(),
                project_code,
                "Project north-star (draft)",
                description,
                metadata.clone()
            ]),
        ) {
            tracing::warn!(project_code = %project_code, vis_id = %vis_id, error = %e, "Vision auto-seed insert failed");
            return false;
        }

        // Allocate canonical Pillar id
        let pil_id = match self
            .graph_store
            .query_json(&format!(
                "SELECT soll.allocate_node_id('Pillar', '{}')",
                escaped_code
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
        {
            Some(id) if !id.is_empty() => id,
            _ => {
                tracing::warn!(project_code = %project_code, "Pillar id allocation failed; Vision created without companion Pillar");
                return true; // Vision is the critical artefact, Pillar best-effort
            }
        };

        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
             VALUES (?, 'Pillar', ?, ?, ?, 'planned', ?)",
            &serde_json::json!([
                pil_id.clone(),
                project_code,
                "Project north-star pillar (draft)",
                "Default Pillar EPITOMIZES the Vision. Split into multiple Pillars via soll_manager(action=create) once project intent crystallizes. Auto-seeded by axon_init_project (REQ-AXO-901606).",
                metadata
            ]),
        ) {
            tracing::warn!(project_code = %project_code, pil_id = %pil_id, error = %e, "Pillar auto-seed insert failed");
            return true;
        }

        // Pillar EPITOMIZES Vision (canonical per soll_relation_schema)
        if let Err(e) = self.graph_store.execute_param(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code)
             VALUES (?, ?, 'EPITOMIZES', ?)",
            &serde_json::json!([pil_id, vis_id, project_code]),
        ) {
            tracing::warn!(project_code = %project_code, error = %e, "EPITOMIZES edge insert failed");
        }

        true
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

        // REQ-AXO-901606 — auto-seed Vision + Pillar (draft) so a fresh
        // project is immediately usable for Pillar/REQ creation via
        // soll_manager. Without this, callers hit the rejection
        // « soll_manager cannot create a Vision » (manager.rs:334) AND
        // « Pillars exigent EPITOMIZES → Vision » constraint, blocking
        // bootstrap. Idempotent : no-op if project already has a Vision.
        let vision_auto_seeded = self.seed_default_vision_and_pillar(&project_code, concept_text);

        // IST tables are multi-project under PG (post-CPT-AXO-039
        // supersedure 2026-05-08), provisioned once by
        // `bootstrap_global_pg_schema`. `generate_project_schema` emits
        // zero DDL — it only validates project_code shape — and is
        // preserved as an extension point for future per-project setup.
        if let Err(e) = crate::postgres::ddl::generate_project_schema(&project_code) {
            tracing::warn!(
                project_code = %project_code,
                error = %e,
                "axon_init_project: project_code validation failed; registry entry created"
            );
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

        // REQ-AXO-901606 — Vision auto-seed messaging. The legacy text
        // « Extract the Vision and Pillars, then use `soll_manager` to
        // create them » was misleading because soll_manager.create vision
        // is rejected. Now we either confirm the auto-seed (with the
        // exact draft IDs) or note that a pre-existing Vision was kept.
        if vision_auto_seeded {
            response_text.push_str(&format!(
                "🌟 Vision + Pillar auto-seeded (status=planned).\n\
                 - `VIS-{code}-001` Project north-star — edit via `soll_manager(action=update, entity=vision, data={{id:'VIS-{code}-001', description:'...', status:'current'}})`\n\
                 - `PIL-{code}-001` Project north-star pillar EPITOMIZES Vision — split into specific Pillars via `soll_manager(action=create, entity=pillar, data={{project_code:'{code}', attach_to:'VIS-{code}-001', relation_type:'EPITOMIZES', ...}})`\n\n",
                code = project_code
            ));
        } else if concept_text.is_some() {
            response_text.push_str(&format!(
                "📄 A concept document was provided but Vision auto-seed was skipped (project already has a Vision). Inspect existing intent via `soll_query_context project_code={}` then edit via `soll_manager(action=update)`.\n\n",
                project_code
            ));
        }

        response_text.push_str(&format!(
            "Server-assigned project code: `{}`.\n\n",
            project_code
        ));
        response_text.push_str("Available global rules. Which ones do you want to activate, ignore, or specialize for this project?\n");
        response_text.push_str(&rules_text);
        response_text.push_str("\n(Use `axon_apply_guidelines` to apply these choices).");

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
            "\n\nKickoff bundle attached in `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, session_pointer, active_handoff, in_progress_requirements, wave_1_unblockers, recent_req_commits, recent_soll_writes). Use it to onboard yourself or any future LLM session before doing project-specific work.",
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
        let supplied_project = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .unwrap_or("");
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
        // REQ-AXO-901613 bug #2 — idempotence ledger (already INHERITS_FROM)
        let mut already_applied: Vec<String> = Vec::new();
        // REQ-AXO-901613 bug #1 — node INSERT failures surfaced, never silent
        let mut failed: Vec<String> = Vec::new();
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

                // REQ-AXO-901613 bug #2 — idempotence gate BEFORE id allocation.
                // soll.allocate_node_id increments last_gui irreversibly, so a
                // duplicate must short-circuit here and never consume an id.
                let existing = self
                    .graph_store
                    .query_count_param(
                        "SELECT count(*) FROM soll.Edge WHERE relation_type = 'INHERITS_FROM' AND target_id = ? AND project_code = ?",
                        &serde_json::json!([global_id, project_code]),
                    )
                    .unwrap_or(0);
                if existing > 0 {
                    already_applied.push(global_id.to_string());
                    continue;
                }

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

                // REQ-AXO-901613 bug #1 — status='planned' is the canonical
                // draft vocabulary (DEC-PRO-100) honoured by the
                // soll_node_status_canonical CHECK ; legacy 'active' violated it
                // and the swallowed error left a phantom edge with no node.
                if let Err(e) = self.graph_store.execute_param(
                    "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                     VALUES (?, 'Guideline', ?, ?, ?, 'planned', ?)",
                    &serde_json::json!([local_id, p_code, title, desc, meta])
                ) {
                    tracing::warn!(project_code = %p_code, local_id = %local_id, global_id = %global_id, error = %e, "guideline node insert failed; skipping edge");
                    failed.push(global_id.to_string());
                    continue;
                }

                // REQ-AXO-152: project_code on INSERT. p_code is in scope (the
                // local tenant the Guideline is being inherited into), so use it
                // directly rather than re-deriving from canonical ID prefixes.
                if let Err(e) = self.graph_store.execute_param(
                    "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) VALUES (?, ?, 'INHERITS_FROM', '{}', ?)",
                    &serde_json::json!([local_id, global_id, p_code])
                ) {
                    // Edge failed after the node committed — roll back the
                    // just-created orphan node (best-effort) so we never leave a
                    // node without its inheritance edge (mirror of the bug).
                    tracing::warn!(project_code = %p_code, local_id = %local_id, global_id = %global_id, error = %e, "inheritance edge insert failed; rolling back node");
                    let _ = self.graph_store.execute_param(
                        "DELETE FROM soll.Node WHERE id = ?",
                        &serde_json::json!([local_id]),
                    );
                    failed.push(global_id.to_string());
                    continue;
                }

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
        // valid IDs (discoverable via `sql SELECT id, title FROM
        // soll.Node WHERE type='Guideline' AND project_code='PRO'`).
        // REQ-AXO-341 — hint retargeted PG canonical post-MIL-AXO-017.
        let empty_input = accepted_ids.is_empty();
        let nothing_applied = applied.is_empty();
        let recovery_hint = "discover valid IDs via sql SELECT id, title FROM soll.Node WHERE type='Guideline' AND project_code='PRO'";
        // REQ-AXO-901613 — already_applied (idempotent no-op) is a SUCCESS
        // outcome, not an error ; failed (node/edge insert error) is.
        let suffix_already = if already_applied.is_empty() {
            String::new()
        } else {
            format!(" Already applied (idempotent no-op): {already_applied:?}.")
        };
        let suffix_failed = if failed.is_empty() {
            String::new()
        } else {
            format!(" Failed (node/edge write error, see logs): {failed:?}.")
        };
        let text = if empty_input {
            format!(
                "axon_apply_guidelines requires at least one canonical Guideline ID in `accepted_global_rule_ids`. {recovery_hint}.")
        } else if !applied.is_empty() && !unknown.is_empty() {
            format!(
                "Inheritance applied. New local rules created: {applied:?}. Unknown global rule IDs (skipped): {unknown:?}.{suffix_already}{suffix_failed}")
        } else if !applied.is_empty() {
            format!("Inheritance applied. New local rules created: {applied:?}.{suffix_already}{suffix_failed}")
        } else if !already_applied.is_empty() && failed.is_empty() && unknown.is_empty() {
            format!("No new rules applied — all requested guidelines were already inherited (idempotent no-op): {already_applied:?}.")
        } else {
            format!(
                "No rules applied. Unknown global rule IDs: {unknown:?}.{suffix_already}{suffix_failed} {recovery_hint}.")
        };

        let mut data = serde_json::json!({
            "applied": applied,
            "unknown_global_rule_ids": unknown,
            "already_applied": already_applied,
            "failed": failed,
        });
        if empty_input {
            data["empty_input"] = serde_json::json!(true);
        }
        // already_applied with no failures is a successful idempotent no-op.
        let pure_already_applied = nothing_applied
            && failed.is_empty()
            && unknown.is_empty()
            && !already_applied.is_empty();
        let is_error =
            empty_input || (nothing_applied && !pure_already_applied) || !failed.is_empty();
        if empty_input || (nothing_applied && !pure_already_applied) {
            data["recovery_hint"] = serde_json::json!(recovery_hint);
        }
        let mut response = serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "data": data,
        });
        if is_error {
            response["isError"] = serde_json::json!(true);
        }
        Some(response)
    }
}

#[cfg(test)]
mod commit_work_cwd_tests {
    //! REQ-AXO-191 (Fiscaly P1.2) — verify the args→cwd-resolution
    //! contract on `axon_commit_work`. The integration with
    //! `Command::current_dir` is covered by the existing
    //! commit-pipeline tests; here we test the pure parsing and
    //! escape behaviour so the regression is locked.
    use serde_json::json;

    fn extract_project_path_arg(args: &serde_json::Value) -> Option<String> {
        args.get("project_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from)
    }

    #[test]
    fn project_path_arg_extracted_when_present() {
        let args = json!({"project_path": "/abs/path", "diff_paths": [], "message": "msg"});
        assert_eq!(
            extract_project_path_arg(&args),
            Some("/abs/path".to_string())
        );
    }

    #[test]
    fn project_path_arg_rejects_empty_and_whitespace() {
        let args = json!({"project_path": "   ", "diff_paths": [], "message": "msg"});
        assert!(extract_project_path_arg(&args).is_none());
        let args = json!({"diff_paths": [], "message": "msg"});
        assert!(extract_project_path_arg(&args).is_none());
    }

    #[test]
    fn project_path_arg_handles_quote_in_value() {
        // The path is later sql-escaped via `escape_sql` before
        // hitting the registry lookup; here we only check that the
        // parser preserves the quote so the escape can apply.
        let args =
            json!({"project_path": "/path/with'quote/in_it", "diff_paths": [], "message": "msg"});
        assert_eq!(
            extract_project_path_arg(&args),
            Some("/path/with'quote/in_it".to_string())
        );
    }
}

#[cfg(test)]
mod commit_message_refactor_tests {
    //! REQ-AXO-91569 — diff-aware pre-flight exemption. The helper
    //! `commit_message_is_refactor` is the single signal the gate
    //! consults when a guideline carries `exempt_for_refactor: true`.
    //! Locking the parsing here prevents the gate from being
    //! accidentally widened (`refactoring`, `refactor without colon`)
    //! or narrowed (rejecting valid scoped breaking-change prefixes).
    use super::commit_message_is_refactor;

    #[test]
    fn plain_refactor_colon_matches() {
        assert!(commit_message_is_refactor("refactor: tighten loop"));
    }

    #[test]
    fn scoped_refactor_matches() {
        assert!(commit_message_is_refactor(
            "refactor(governance): collapse cosine_expr"
        ));
    }

    #[test]
    fn scoped_breaking_refactor_matches() {
        assert!(commit_message_is_refactor("refactor(api)!: rename method"));
    }

    #[test]
    fn plain_breaking_refactor_matches() {
        assert!(commit_message_is_refactor("refactor!: hot-path rewrite"));
    }

    #[test]
    fn leading_whitespace_tolerated() {
        assert!(commit_message_is_refactor("   refactor: with leading ws"));
    }

    #[test]
    fn feat_prefix_rejected() {
        assert!(!commit_message_is_refactor("feat: add API"));
    }

    #[test]
    fn refactoring_rejected() {
        // `refactoring` is not a Conventional-Commits type token ; the
        // gate must stay strict on near-misses.
        assert!(!commit_message_is_refactor("refactoring the loop"));
    }

    #[test]
    fn refactor_without_separator_rejected() {
        assert!(!commit_message_is_refactor("refactor the loop"));
    }

    #[test]
    fn unclosed_scope_rejected() {
        assert!(!commit_message_is_refactor("refactor(governance"));
    }

    #[test]
    fn empty_message_rejected() {
        assert!(!commit_message_is_refactor(""));
    }

    #[test]
    fn uppercase_rejected() {
        // Conventional-Commits 1.0 is case-sensitive — `Refactor:` is
        // not a valid type token, so the gate stays strict.
        assert!(!commit_message_is_refactor("Refactor: tighten loop"));
    }
}
