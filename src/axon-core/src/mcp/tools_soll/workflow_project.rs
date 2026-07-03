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

/// REQ-AXO-901909 — collapse a guideline body to a single-line digest so
/// `axon_init_project` can advertise the rule catalogue without re-dumping
/// every full body on every init (GUI-PRO-100 token-economy). Newlines are
/// flattened, the result is bounded to `GUIDELINE_DIGEST_MAX` characters,
/// truncated back to the last word boundary, and marked with `…`. The full
/// body stays one `sql SELECT description FROM soll.Node WHERE id=…` away.
const GUIDELINE_DIGEST_MAX: usize = 100;
fn guideline_digest(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= GUIDELINE_DIGEST_MAX {
        return flat;
    }
    let mut out: String = flat.chars().take(GUIDELINE_DIGEST_MAX).collect();
    if let Some(idx) = out.rfind(' ') {
        out.truncate(idx);
    }
    out.push('…');
    out
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

/// REQ-AXO-159 — extract canonical `REQ-<PROJ>-<N>` ids from a commit message
/// for auto-evidence attachment. Deduplicates, preserves first-seen order. No
/// regex dependency: scans `REQ-` anchors then validates `<UPPER>+-<DIGITS>+`.
pub(super) fn parse_commit_req_ids(message: &str) -> Vec<String> {
    let bytes = message.as_bytes();
    let mut ids: Vec<String> = Vec::new();
    for (start, _) in message.match_indices("REQ-") {
        let mut j = start + 4;
        let proj_start = j;
        while j < bytes.len() && bytes[j].is_ascii_uppercase() {
            j += 1;
        }
        if j == proj_start || j >= bytes.len() || bytes[j] != b'-' {
            continue;
        }
        j += 1; // skip the '-' between PROJ and the number
        let dig_start = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == dig_start {
            continue;
        }
        let id = message[start..j].to_string();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
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

        // REQ-AXO-902169 — refuse an EMPTY diff_paths for a REAL commit. The staging
        // below is `git add -A -- <diff_paths...>`; with no pathspec, `git add -A --`
        // stages the ENTIRE working tree, so a caller that passes `diff_paths:[]`
        // (observed s94: a qualify smoke calling `{diff_paths:[], message:"x"}`) would
        // sweep arbitrary untracked/modified files into a junk commit (6df4b383).
        // A commit must NAME what it commits; a qualification run must never mutate git.
        // dry_run above still validates an empty diff_paths without touching git.
        if diff_paths.is_empty() {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text":
                    "axon_commit_work refuses an empty `diff_paths` for a real commit: `git add -A --` with no pathspec would stage the entire working tree. List the modified/created files this commit should contain (pre-stage deletions via `git rm`), or pass `dry_run:true` to validate only." }],
                "isError": true,
                "structuredContent": {
                    "status": "input_invalid",
                    "operator_guidance": {
                        "tool": "axon_commit_work",
                        "problem_class": "empty_diff_paths_refused",
                        "invalid_field": "diff_paths",
                        "hint": "list the files this commit should contain; use dry_run=true to validate without committing"
                    }
                }
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
        // REQ-AXO-902062 (llm_feedback id14, FSF) — `-A -- <paths>` stages
        // additions, modifications AND deletions for the given pathspecs. Plain
        // `git add <deleted_path>` fails 'pathspec did not match any files' once
        // the file (and its directory) are gone from the worktree, which blocked
        // committing a pure file deletion. `-A` still errors on a genuinely
        // never-tracked path, preserving the REQ-AXO-138 missing-path guard.
        add_cmd.arg("add").arg("-A").arg("--");
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
                    let mut s = format!(
                        "Commit succeeded.\n{}",
                        String::from_utf8_lossy(&output.stdout)
                    );
                    // REQ-AXO-159 — auto-attach commit evidence (Traceability) to
                    // each EXISTING Requirement referenced in the message, so a
                    // delivered REQ accrues its proof without a manual
                    // soll_attach_evidence round-trip (feeds the REQ-902041 gap).
                    let note = self.auto_attach_commit_evidence(
                        message,
                        diff_paths,
                        resolved_project_path.as_deref(),
                    );
                    if !note.is_empty() {
                        s.push('\n');
                        s.push_str(&note);
                    }
                    s
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

    /// REQ-AXO-159 — after a successful commit, attach a Traceability "Commit"
    /// artifact (the HEAD sha) to each EXISTING Requirement referenced in the
    /// message. confidence=0.7 (inferred from the message, not manually
    /// verified). No FK on soll.Traceability → existence is checked first to
    /// avoid dangling rows. Returns a one-line note for the response, or "".
    fn auto_attach_commit_evidence(
        &self,
        message: &str,
        diff_paths: &[serde_json::Value],
        project_dir: Option<&std::path::Path>,
    ) -> String {
        let ids = parse_commit_req_ids(message);
        if ids.is_empty() {
            return String::new();
        }
        let mut sha_cmd = std::process::Command::new("git");
        if let Some(dir) = project_dir {
            sha_cmd.current_dir(dir);
        }
        let sha = match sha_cmd.arg("rev-parse").arg("HEAD").output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => return String::new(),
        };
        if sha.is_empty() {
            return String::new();
        }
        let now = now_unix_ms();
        let subject = message.lines().next().unwrap_or("").to_string();
        let files: Vec<&str> = diff_paths.iter().filter_map(|p| p.as_str()).collect();
        let metadata = serde_json::json!({
            "source": "auto_commit_work",
            "subject": subject,
            "files": files,
        })
        .to_string();
        let mut attached: Vec<String> = Vec::new();
        for (idx, id) in ids.iter().enumerate() {
            // No FK on soll.Traceability → only attach to an EXISTING Requirement.
            let exists = self
                .graph_store
                .query_count_param(
                    "SELECT COUNT(*) FROM soll.Node WHERE id = ? AND type = 'Requirement'",
                    &serde_json::json!([id]),
                )
                .unwrap_or(0)
                > 0;
            if !exists {
                continue;
            }
            let trace_id = format!("TRC-{}-{}-auto{}", id, now, idx);
            if self
                .graph_store
                .execute_param(
                    "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    &serde_json::json!([trace_id, "requirement", id, "Commit", sha, 0.7_f64, metadata, now]),
                )
                .is_ok()
            {
                attached.push(id.clone());
            }
        }
        if attached.is_empty() {
            String::new()
        } else {
            let short = &sha[..sha.len().min(8)];
            format!(
                "REQ-AXO-159 auto-evidence : commit {} attach\u{00e9} \u{00e0} {}",
                short,
                attached.join(", ")
            )
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
    pub(crate) fn lookup_project_path_by_code(&self, code: &str) -> Option<std::path::PathBuf> {
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

    /// REQ-AXO-902078 — init context economy. The kickoff bundle previously
    /// risked diluting a cold-start LLM's context by leaving it to discover
    /// macro intent through several follow-up reads. `soll_skeleton` applies a
    /// PUSH/PULL split that keeps the bundle small while front-loading the
    /// Phase-B-critical material (GUI-PRO-102):
    ///   - Vision + Pillars are PUSHED with full bodies (few nodes, mandatory
    ///     for Phase B reasoning) — descriptions read straight from soll.Node.
    ///   - Decisions + Guidelines are INDEXED (id + title only, status=current)
    ///     with a `pull_with` hint so the LLM fetches a body on demand via
    ///     `soll_query_context`. Their bodies are deliberately NOT inlined —
    ///     that bulk is the real dilution this split removes.
    fn soll_skeleton(&self, project_code: &str) -> serde_json::Value {
        let Ok(snapshot) = self.soll_cache().snapshot(project_code) else {
            return serde_json::json!({
                "status": "unavailable",
                "note": "SOLL snapshot not resolvable for this project",
            });
        };

        // PUSH: full bodies for the few, Phase-B-critical macro nodes.
        let push_bodies = |entity_type: &str| -> serde_json::Value {
            let mut ids: Vec<&String> = snapshot.node_ids_of_type(entity_type).iter().collect();
            ids.sort();
            serde_json::Value::Array(
                ids.into_iter()
                    .filter_map(|id| snapshot.nodes.get(id).map(|n| (id, n)))
                    .map(|(id, n)| {
                        serde_json::json!({
                            "id": id,
                            "title": n.title,
                            "status": n.status,
                            "body": self.read_soll_node_description(id),
                        })
                    })
                    .collect(),
            )
        };

        // PULL: id + title index only, restricted to status='current'.
        let index_current = |entity_type: &str| -> serde_json::Value {
            let mut ids: Vec<&String> = snapshot.node_ids_of_type(entity_type).iter().collect();
            ids.sort();
            serde_json::Value::Array(
                ids.into_iter()
                    .filter_map(|id| snapshot.nodes.get(id).map(|n| (id, n)))
                    .filter(|(_, n)| n.status == "current")
                    .map(|(id, n)| {
                        serde_json::json!({
                            "id": id,
                            "title": n.title,
                        })
                    })
                    .collect(),
            )
        };

        serde_json::json!({
            "vision": push_bodies("Vision"),
            "pillars": push_bodies("Pillar"),
            "decisions_index": index_current("Decision"),
            "guidelines_index": index_current("Guideline"),
            // Bodies for the indexed Decisions/Guidelines are intentionally
            // omitted (PULL on demand) to keep the bundle lean.
            "pull_with": "soll_query_context",
            "pull_note": "decisions_index/guidelines_index list id+title only — fetch a body on demand via soll_query_context(question=<ID>) or sql SELECT description FROM soll.Node WHERE id='<ID>'.",
        })
    }

    /// REQ-AXO-902078 — capabilities_map. Derived at RUNTIME from
    /// `tools_catalog(false)['tools']` (name + first sentence of description)
    /// so it can never drift from the live tool surface. Hard-coding it would
    /// re-introduce the non-conformity this REQ closes: a stale list misleads
    /// the cold-start LLM about which tools exist.
    fn capabilities_map() -> serde_json::Value {
        let catalog = crate::mcp::catalog::tools_catalog(false);
        let tools = catalog
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        serde_json::Value::Array(
            tools
                .into_iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(|v| v.as_str())?.to_string();
                    let summary = tool
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(Self::first_sentence)
                        .unwrap_or_default();
                    Some(serde_json::json!({ "name": name, "summary": summary }))
                })
                .collect(),
        )
    }

    /// First sentence of a tool description (up to and including the first
    /// `.`), trimmed. Falls back to the whole trimmed string when no period
    /// is present.
    fn first_sentence(description: &str) -> String {
        let trimmed = description.trim();
        match trimmed.find('.') {
            Some(idx) => trimmed[..=idx].trim().to_string(),
            None => trimmed.to_string(),
        }
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

    /// REQ-AXO-287 / REQ-AXO-902160 — run a `git` command bounded by a wall-clock
    /// timeout so a slow repo (large pack, lock contention, fs latency) can't trip
    /// the MCP gateway 30-s timeout on `axon_init_project`. Over-budget or failure →
    /// `None` so the caller degrades gracefully (empty bundle field). Single source
    /// of the spawn+timeout+kill dance, shared by `read_recent_req_commits` and
    /// `read_git_head`.
    fn bounded_git_stdout(
        project_path: &str,
        args: &[&str],
        timeout: std::time::Duration,
    ) -> Option<String> {
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(project_path);
        for a in args {
            cmd.arg(a);
        }
        let child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<std::process::Output>>();
        // Move `child` into the wait thread ; if the timeout fires we kill the
        // process via its pid (captured before the move) for a signal-based kill.
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(o)) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(_) => None,
            Err(_) => {
                // Timed out — best-effort kill ; the dangling thread exits when
                // `git` finishes or the kill takes effect.
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .output();
                None
            }
        }
    }

    fn read_recent_req_commits(project_path: &str, limit: usize) -> serde_json::Value {
        // REQ-AXO-287 — 2-second budget is generous for a `--max-count` filtered
        // log ; over-budget = empty so the rest of the kickoff bundle completes.
        let max = format!("--max-count={}", limit * 4);
        let stdout = match Self::bounded_git_stdout(
            project_path,
            &["log", "--oneline", &max, "-E", "--grep=REQ-[A-Z]+-[0-9]+"],
            std::time::Duration::from_secs(2),
        ) {
            Some(s) => s,
            None => return serde_json::json!([]),
        };
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

    /// REQ-AXO-902160 — current git HEAD as a tab-joined `"<short-sha>\t<refnames>\t<subject>"`
    /// line (`%h\t%D\t%s`), bounded like the commit read. `None` on an absent repo or
    /// timeout. Parsed by [`Self::derive_session_pointer`] (kept pure).
    fn read_git_head(project_path: &str) -> Option<String> {
        let out = Self::bounded_git_stdout(
            project_path,
            &["log", "-1", "--format=%h\t%D\t%s"],
            std::time::Duration::from_secs(2),
        )?;
        let line = out.lines().next()?.trim();
        if line.is_empty() {
            None
        } else {
            Some(line.to_string())
        }
    }

    /// REQ-AXO-902160 — auto-derive a compact session-orientation pointer from
    /// signals the kickoff bundle ALREADY computes (git HEAD line `"sha\trefs\tsubject"`,
    /// in-progress REQs, recent REQ commits). A fresh session gets oriented WITHOUT
    /// anyone hand-writing a ~2500-char session_pointer node (the OPV friction,
    /// REQ-AXO-902160). COMPLEMENTS an explicit operator-set pointer (surfaced under
    /// `explicit`), never replaces it. Pure + unit-testable (bounded git I/O stays in
    /// the caller per the REQ-AXO-287 pattern).
    fn derive_session_pointer(
        head_line: Option<&str>,
        in_progress: &serde_json::Value,
        recent_commits: &serde_json::Value,
        explicit: &serde_json::Value,
    ) -> serde_json::Value {
        // git HEAD line: "sha\trefs\tsubject" ; refs like "HEAD -> main, origin/main".
        let (head_sha, head_branch, head_subject) = match head_line {
            Some(l) => {
                let mut p = l.splitn(3, '\t');
                let sha = p.next().unwrap_or("").trim().to_string();
                let refs = p.next().unwrap_or("").trim().to_string();
                let subject = p.next().unwrap_or("").trim().to_string();
                let branch = refs
                    .split(',')
                    .map(str::trim)
                    .find_map(|r| r.strip_prefix("HEAD -> "))
                    .unwrap_or("")
                    .to_string();
                (sha, branch, subject)
            }
            None => (String::new(), String::new(), String::new()),
        };

        let ip = in_progress.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let ip_brief: Vec<String> = ip
            .iter()
            .take(3)
            .map(|r| {
                let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let prio = r
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .map(|p| format!(" ({p})"))
                    .unwrap_or_default();
                let title: String = r
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(48)
                    .collect();
                format!("{id}{prio} {title}")
            })
            .collect();

        let commits = recent_commits.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let commit_brief: Vec<String> = commits
            .iter()
            .take(3)
            .filter_map(|c| {
                let sha = c.get("sha").and_then(|v| v.as_str())?;
                let subj: String = c
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect();
                Some(format!("{sha} {subj}"))
            })
            .collect();

        let explicit_ref = match explicit.get("kind").and_then(|v| v.as_str()) {
            Some("none") | None => serde_json::Value::Null,
            Some(_) => explicit.clone(),
        };

        let mut summary = String::from("Auto-derived orientation (git+SOLL) — ");
        if head_sha.is_empty() {
            summary.push_str("no git HEAD");
        } else {
            summary.push_str(&format!("HEAD {head_sha}"));
            if !head_branch.is_empty() {
                summary.push_str(&format!(" ({head_branch})"));
            }
            if !head_subject.is_empty() {
                let s: String = head_subject.chars().take(52).collect();
                summary.push_str(&format!(" {s}"));
            }
        }
        if ip_brief.is_empty() {
            summary.push_str(" · no REQ in progress");
        } else {
            summary.push_str(&format!(" · {} REQ in progress: {}", ip.len(), ip_brief.join("; ")));
        }
        if !commit_brief.is_empty() {
            summary.push_str(&format!(" · recent REQ commits: {}", commit_brief.join(", ")));
        }
        if let Some(id) = explicit_ref.get("value").and_then(|v| v.as_str()) {
            summary.push_str(&format!(" · explicit pointer: {id}"));
        }

        serde_json::json!({
            "summary": summary,
            "head": if head_sha.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!({"sha": head_sha, "branch": head_branch, "subject": head_subject})
            },
            "in_progress_count": ip.len(),
            "explicit": explicit_ref,
            "sources": ["git_head", "in_progress_requirements", "recent_req_commits"],
            "note": "Derived automatically from live signals (REQ-AXO-902160); no hand-write. `explicit` is the operator-set session_pointer when present.",
        })
    }

    /// REQ-AXO-902172 — render the essential Continuation block for `content.text`.
    /// The kickoff bundle is rich but lived ONLY in `data.kickoff_bundle`; an LLM client
    /// reading content alone saw the pointer sentence, never the orientation itself
    /// (mcp_feedback #41, LLL 2026-07-02). Renders the operator/derived session pointer,
    /// the wave-1 unblockers, and the 3 most recent REQ commits — bounded, token-safe.
    /// Pure over the already-assembled bundle (no I/O, no timeout risk).
    fn render_continuation_block(bundle: &serde_json::Value) -> String {
        let mut out = String::from("## 🧭 Continuation — orientation immédiate\n");

        // Session pointer: explicit operator-set wins; else fall back to the derived summary.
        let sp = bundle.get("session_pointer");
        let sp_kind = sp
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        if sp_kind != "none" {
            let val = sp.and_then(|v| v.get("value")).and_then(|v| v.as_str()).unwrap_or("");
            let label = sp.and_then(|v| v.get("label")).and_then(|v| v.as_str()).unwrap_or("");
            let label = if label.is_empty() { String::new() } else { format!(" — {label}") };
            out.push_str(&format!("**Session pointer** (`{sp_kind}`): `{val}`{label}\n"));
        }
        if let Some(summary) = bundle
            .get("derived_session_pointer")
            .and_then(|v| v.get("summary"))
            .and_then(|v| v.as_str())
        {
            out.push_str(&format!("**Auto-orienté:** {summary}\n"));
        }

        // Wave-1 unblockers (id — priority — title), the 3 kickoff fast-path leaves.
        if let Some(wave) = bundle.get("wave_1_unblockers").and_then(|v| v.as_array()) {
            let items: Vec<String> = wave
                .iter()
                .take(3)
                .map(|w| {
                    let id = w.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let prio = w
                        .get("priority")
                        .and_then(|v| v.as_str())
                        .map(|p| format!(" [{p}]"))
                        .unwrap_or_default();
                    let title: String = w
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(64)
                        .collect();
                    format!("{id}{prio} {title}")
                })
                .collect();
            if !items.is_empty() {
                out.push_str(&format!("**Wave-1 (déblocages):** {}\n", items.join(" · ")));
            }
        }

        // 3 most recent REQ commits (already bounded by read_recent_req_commits).
        if let Some(commits) = bundle.get("recent_req_commits").and_then(|v| v.as_array()) {
            let items: Vec<String> = commits
                .iter()
                .take(3)
                .filter_map(|c| {
                    let sha = c.get("sha").and_then(|v| v.as_str())?;
                    let subj: String = c
                        .get("subject")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(56)
                        .collect();
                    Some(format!("`{sha}` {subj}"))
                })
                .collect();
            if !items.is_empty() {
                out.push_str(&format!("**Derniers commits REQ:** {}\n", items.join(" · ")));
            }
        }

        out
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
        // REQ-AXO-902160 — auto-derive a session-orientation pointer from the live
        // signals above (git HEAD + in-progress REQs + recent REQ commits) so a fresh
        // session is oriented without anyone hand-writing a session_pointer node.
        let derived_session_pointer = Self::derive_session_pointer(
            Self::read_git_head(project_path).as_deref(),
            &in_progress_requirements,
            &recent_req_commits,
            &session_pointer,
        );
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
        // REQ-AXO-901963 — PUSH code-intel availability into the kickoff bundle so
        // a cold-start LLM knows query/inspect/impact are live (N/N indexed) and
        // does not default to grep.
        let code_intel = match self.project_scope_summary(Some(project_code)) {
            Some(s) if s.total_files > 0 => serde_json::json!({
                "status": if s.backlog_files == 0 { "live" } else { "degraded" },
                "files_indexed": s.completed_files,
                "files_total": s.total_files,
                "backlog": s.backlog_files,
                "tools": ["query", "inspect", "impact", "why", "path", "anomalies"],
                "signal": format!(
                    "CODE-INTEL {} — {}/{} files indexed, backlog {} (query/inspect/impact/why operational; prefer structural tools over grep/cat)",
                    if s.backlog_files == 0 { "LIVE" } else { "DEGRADED" },
                    s.completed_files, s.total_files, s.backlog_files,
                ),
            }),
            _ => serde_json::json!({
                "status": "unknown",
                "signal": "Code-intel scope not resolvable for this project",
            }),
        };
        // REQ-AXO-902078 — init context economy: PUSH macro bodies (Vision +
        // Pillars), INDEX Decisions/Guidelines (id+title, pull-on-demand),
        // expose a runtime-derived capabilities_map and a session_toolset_hint
        // so the cold-start LLM provisions its tool surface in one move.
        let soll_skeleton = self.soll_skeleton(project_code);
        let capabilities_map = Self::capabilities_map();
        let session_toolset_hint = "select:query,inspect,retrieve_context,impact,soll_query_context,soll_work_plan,soll_manager,document_intent,axon_pre_flight_check,axon_commit_work";
        serde_json::json!({
            "kickoff_prompt": kickoff_prompt,
            "kickoff_prompt_source": "soll://Node/DEC-PRO-001",
            "methodology_summary": methodology_summary,
            "methodology_summary_source": "soll://Node/CPT-AXO-019",
            "entry_points": Self::cold_start_entry_points(),
            "session_pointer": session_pointer,
            "derived_session_pointer": derived_session_pointer,
            "active_handoff": active_handoff_alias,
            "in_progress_requirements": in_progress_requirements,
            "wave_1_unblockers": wave_1_unblockers,
            // REQ-AXO-902114 (MBX-2) — unread mailbox at wake: a session onboarding
            // sees pending inter-project messages alongside its SOLL backlog.
            "inbox_unread": self.mailbox_unread_count(project_code),
            "recent_req_commits": recent_req_commits,
            "recent_soll_writes": recent_soll_writes,
            "bootstrap_required": bootstrap_required,
            "input_documents": input_documents,
            "code_intel": code_intel,
            // REQ-AXO-902078 — init context economy (PUSH/PULL skeleton +
            // runtime capabilities map + toolset hint).
            "soll_skeleton": soll_skeleton,
            "capabilities_map": capabilities_map,
            "session_toolset_hint": session_toolset_hint,
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
                // REQ-AXO-901909 — advertise the rule catalogue with a terse
                // one-line digest, NOT the full body. Some bodies run multiple
                // KB (e.g. GUI-PRO-102) and re-dumping every one re-bills the
                // cold-start token budget on every init, in every session
                // (GUI-PRO-100 token-economy). Full bodies stay one `sql` read
                // away (pointer printed below).
                rules_text.push_str(&format!(
                    "- **{}**: {} — {}\n",
                    row[0],
                    row[1],
                    guideline_digest(&row[2])
                ));
            }
        }

        let mut response_text = format!(
            "Project '{}' ({}) initialized in Axon.\n\n",
            project_name, project_code
        );

        // REQ-AXO-901985 — be contract-honest about LIVE indexing. The registry
        // write emits `axon_registry_changed`; a running indexer enrols the new
        // project and indexes it live. But if the runtime is brain_only there is
        // NO indexer to consume the signal — say so explicitly so the operator
        // isn't surprised that nothing got indexed (the observed bug).
        response_text.push_str(
            "📡 **Live indexing** : an enrolment signal (`axon_registry_changed`) was emitted. \
             If an indexer is running, this project is being indexed live now. \
             If the runtime is `brain_only` (no indexer — check `status`), start one with \
             `./scripts/axon-live start --indexer-full`; the project enrols via the registry signal (REQ-AXO-901985).\n\n",
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
        response_text.push_str("Available global rules (digest — read any body in full via `sql SELECT description FROM soll.Node WHERE id='<ID>'`). Which ones do you want to activate, ignore, or specialize for this project?\n");
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
            "\n\nKickoff bundle attached in `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, session_pointer, derived_session_pointer, active_handoff, in_progress_requirements, wave_1_unblockers, recent_req_commits, recent_soll_writes, soll_skeleton, capabilities_map, session_toolset_hint). derived_session_pointer (REQ-AXO-902160) auto-orients a fresh session from git HEAD + in-progress REQs + recent REQ commits — no hand-write ; `.explicit` carries the operator-set session_pointer when present. soll_skeleton PUSHES Vision+Pillar bodies and INDEXES Decisions/Guidelines (id+title, pull via soll_query_context); capabilities_map lists the live tool surface; session_toolset_hint is a ready ToolSearch select. Use it to onboard yourself or any future LLM session before doing project-specific work.",
        );

        // REQ-AXO-902172 — lead with the essential Continuation block INLINE so a client
        // reading content.text alone is oriented without cracking data.kickoff_bundle
        // (mcp_feedback #41). The rich bundle stays in `data` for programmatic use.
        let continuation = Self::render_continuation_block(&bundle);
        let response_text = format!("{continuation}\n---\n\n{response_text}");

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

#[cfg(test)]
mod guideline_digest_tests {
    //! REQ-AXO-901909 — `axon_init_project` must advertise the rule
    //! catalogue as a terse digest, never re-dump every full body on
    //! every init (GUI-PRO-100 token-economy). These lock the pure
    //! digest contract: single-line, bounded, word-boundary truncation,
    //! short bodies passed through verbatim.
    use super::{guideline_digest, GUIDELINE_DIGEST_MAX};

    #[test]
    fn long_multiline_body_collapses_to_single_bounded_line() {
        // Filler well past the digest bound, then a sentinel that only
        // appears deep in the body (well beyond GUIDELINE_DIGEST_MAX).
        let body = format!(
            "## Phase A\n{}\nDEEP_BODY_SENTINEL must never reach the LLM at init.\n",
            "step does a thing. ".repeat(30)
        );
        let digest = guideline_digest(&body);
        assert!(
            !digest.contains('\n'),
            "digest must be single-line, got: {digest}"
        );
        // bounded to MAX chars + the ellipsis marker
        assert!(
            digest.chars().count() <= GUIDELINE_DIGEST_MAX + 1,
            "digest must be bounded, got {} chars",
            digest.chars().count()
        );
        assert!(
            digest.ends_with('…'),
            "truncated digest must carry the ellipsis marker, got: {digest}"
        );
        // the deep-body sentinel must NOT survive into the digest
        assert!(
            !digest.contains("DEEP_BODY_SENTINEL"),
            "full body must not leak into the digest, got: {digest}"
        );
    }

    #[test]
    fn short_body_passes_through_verbatim_without_ellipsis() {
        let body = "Tests written before or with the source code.";
        assert_eq!(guideline_digest(body), body);
    }

    #[test]
    fn embedded_newlines_in_short_body_are_flattened() {
        let digest = guideline_digest("line one\nline two");
        assert_eq!(digest, "line one line two");
    }
}

#[cfg(test)]
mod derive_session_pointer_tests {
    //! REQ-AXO-902160 — auto-derived session pointer: pure assembly from git HEAD +
    //! in-progress REQs + recent REQ commits, complementing (not replacing) an
    //! explicit operator-set pointer.
    use super::McpServer;
    use serde_json::json;

    #[test]
    fn full_signals_produce_oriented_summary() {
        let head = "65c69de\tHEAD -> main, origin/main\trefactor(practice): supprime le prédicat";
        let in_progress = json!([
            {"id": "REQ-AXO-902160", "title": "session_pointer auto-derive", "priority": "P1"},
            {"id": "REQ-AXO-902161", "title": "SOLL patch/append", "priority": "P1"},
        ]);
        let commits = json!([
            {"sha": "65c69de", "subject": "refactor(practice): should_fuse"},
            {"sha": "d2f5e6c", "subject": "fix(practice): write-gate"},
        ]);
        let explicit = json!({"kind": "soll_node", "value": "CPT-AXO-052"});
        let d = McpServer::derive_session_pointer(Some(head), &in_progress, &commits, &explicit);

        let summary = d.get("summary").and_then(|v| v.as_str()).unwrap();
        assert!(summary.contains("HEAD 65c69de"), "summary: {summary}");
        assert!(summary.contains("(main)"), "branch extracted: {summary}");
        assert!(summary.contains("2 REQ in progress"), "in-progress count: {summary}");
        assert!(summary.contains("REQ-AXO-902160"), "first REQ id: {summary}");
        assert!(summary.contains("recent REQ commits"), "commits: {summary}");
        assert!(summary.contains("explicit pointer: CPT-AXO-052"), "explicit: {summary}");
        assert_eq!(d["head"]["branch"], "main");
        assert_eq!(d["in_progress_count"], 2);
        assert_eq!(d["explicit"]["value"], "CPT-AXO-052");
    }

    #[test]
    fn absent_signals_degrade_gracefully() {
        // No git HEAD, no open REQ, explicit pointer declared absent (kind=none).
        let d = McpServer::derive_session_pointer(
            None,
            &json!([]),
            &json!([]),
            &json!({"kind": "none"}),
        );
        let summary = d.get("summary").and_then(|v| v.as_str()).unwrap();
        assert!(summary.contains("no git HEAD"), "summary: {summary}");
        assert!(summary.contains("no REQ in progress"), "summary: {summary}");
        assert!(d["head"].is_null());
        assert!(d["explicit"].is_null());
        assert_eq!(d["in_progress_count"], 0);
    }
}

#[cfg(test)]
mod continuation_block_tests {
    //! REQ-AXO-902172 — the Continuation block must surface the orientation essentials
    //! INLINE in content.text (mcp_feedback #41: bundle lived only in data).
    use super::McpServer;
    use serde_json::json;

    #[test]
    fn renders_pointer_wave_and_commits() {
        let bundle = json!({
            "session_pointer": {"kind": "soll_node", "value": "CPT-AXO-052", "label": "active pointer"},
            "derived_session_pointer": {"summary": "HEAD 78145526 (main) feat cutover"},
            "wave_1_unblockers": [
                {"id": "REQ-AXO-902165", "title": "cutover in-place", "priority": "P1"},
                {"id": "REQ-AXO-902170", "title": "process_exists zombie", "priority": "P2"},
            ],
            "recent_req_commits": [
                {"sha": "78145526", "subject": "feat(reconciler): run_cutover_loop"},
                {"sha": "44b992e9", "subject": "feat(axonctl): liveness"},
            ],
        });
        let b = McpServer::render_continuation_block(&bundle);
        assert!(b.contains("Continuation"), "header: {b}");
        assert!(b.contains("CPT-AXO-052"), "explicit pointer: {b}");
        assert!(b.contains("active pointer"), "pointer label: {b}");
        assert!(b.contains("HEAD 78145526"), "derived summary: {b}");
        assert!(b.contains("REQ-AXO-902165") && b.contains("[P1]"), "wave-1 + priority: {b}");
        assert!(b.contains("`78145526`"), "recent commit sha: {b}");
    }

    #[test]
    fn degrades_without_explicit_pointer() {
        // kind=none → no explicit line, but the derived summary + header still render.
        let bundle = json!({
            "session_pointer": {"kind": "none"},
            "derived_session_pointer": {"summary": "no git HEAD · no REQ in progress"},
        });
        let b = McpServer::render_continuation_block(&bundle);
        assert!(b.contains("Continuation"), "header present: {b}");
        assert!(!b.contains("Session pointer** (`none`"), "no explicit pointer line: {b}");
        assert!(b.contains("no git HEAD"), "derived summary shown: {b}");
    }
}

#[cfg(test)]
mod commit_req_id_tests {
    //! REQ-AXO-159 — lock the commit-message REQ-id parser used for
    //! auto-evidence attachment: canonical TYPE-PROJ-N only, deduped, in order.
    use super::parse_commit_req_ids;

    #[test]
    fn extracts_multiple_distinct_ids_in_order() {
        let msg = "feat(x): do thing (REQ-AXO-159)\n\nAlso closes REQ-AXO-902041 and REQ-AXO-159 again.";
        assert_eq!(
            parse_commit_req_ids(msg),
            vec!["REQ-AXO-159".to_string(), "REQ-AXO-902041".to_string()]
        );
    }

    #[test]
    fn ignores_malformed_and_non_req_tokens() {
        // no digits, no proj, lowercase, and a non-REQ prefix must all be skipped
        let msg = "REQ- REQ-AXO- REQ-axo-1 DEC-AXO-085 fixes REQ-PRO-12 ok";
        assert_eq!(parse_commit_req_ids(msg), vec!["REQ-PRO-12".to_string()]);
    }

    #[test]
    fn empty_when_no_ids() {
        assert!(parse_commit_req_ids("chore: tidy up, no refs").is_empty());
    }
}
