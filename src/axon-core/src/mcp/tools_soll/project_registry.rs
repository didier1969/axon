use super::*;

/// REQ-AXO-91577 — Sentinel project_codes represent cross-tenant logical
/// scopes that have no concrete `project_path`. They must remain usable for
/// SOLL mutations even when the registry row is missing (live regression
/// recovery). The allowlist is closed: only these codes self-heal.
///
/// - `PRO` : cross-project methodology surface (Pillar PIL-AXO-9003
///   Two-Sided Identity). Holds `GUI-PRO-*`, `SKI-PRO-*`, `PRT-PRO-*` etc.
fn is_sentinel_project_code(code: &str) -> bool {
    matches!(code, "PRO")
}

/// Sentinel registry metadata: (project_path placeholder, project_name).
fn sentinel_project_metadata(code: &str) -> (&'static str, &'static str) {
    match code {
        "PRO" => (
            "(sentinel:cross-project-methodology)",
            "System Global Namespace",
        ),
        _ => ("(sentinel)", "sentinel"),
    }
}

/// REQ-AXO-323 Fault 3 — registry counter seed.
///
/// Returns an idempotent UPDATE that bumps each `last_*` counter in
/// `soll.Registry` to `GREATEST(current, MAX(numeric_suffix))` over the
/// project's existing `soll.Node` rows. Safe to run on every call —
/// counters never go down. Called from `ensure_soll_registry_row` so a
/// project whose registry row was added post-hoc (after nodes already
/// exist) does not allocate colliding ids starting from 0.
///
/// `project_code` is interpolated directly (validated upstream as `^[A-Z]{3}$`
/// by `validate_explicit_canonical_project_code`). Pure formatter so the
/// SQL contract is unit testable.
fn seed_registry_counters_sql(project_code: &str) -> String {
    let max_for = |prefix: &str| {
        format!(
            "COALESCE((SELECT MAX(CAST(SUBSTRING(id FROM '[0-9]+$') AS INTEGER)) \
             FROM soll.Node \
             WHERE project_code = '{project_code}' \
               AND id LIKE '{prefix}-%' \
               AND id ~ '^[A-Z]{{3}}-[A-Z][A-Z0-9]{{2}}-[0-9]+$'), 0)",
            project_code = project_code,
            prefix = prefix
        )
    };
    let assignments: Vec<String> = [
        ("last_vis", "VIS"),
        ("last_pil", "PIL"),
        ("last_req", "REQ"),
        ("last_cpt", "CPT"),
        ("last_dec", "DEC"),
        ("last_mil", "MIL"),
        ("last_val", "VAL"),
        ("last_stk", "STK"),
        ("last_gui", "GUI"),
        ("last_prv", "PRV"),
        ("last_rev", "REV"),
    ]
    .iter()
    .map(|(col, prefix)| format!("{col} = GREATEST({col}, {expr})", col = col, expr = max_for(prefix)))
    .collect();
    format!(
        "UPDATE soll.Registry SET {assignments} WHERE project_code = '{project_code}'",
        assignments = assignments.join(", "),
        project_code = project_code,
    )
}

impl McpServer {
    pub(super) fn sync_project_code_registry_from_meta(&self) -> anyhow::Result<()> {
        for identity in discover_project_identities() {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
        }
        Ok(())
    }

    pub(super) fn known_project_codes_hint(&self) -> String {
        self.query_single_column(
            "SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code ASC",
        )
        .map(|codes| {
            let codes: Vec<String> = codes
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .collect();
            if codes.is_empty() {
                "no known code".to_string()
            } else {
                codes.join(", ")
            }
        })
        .unwrap_or_else(|_| "no known code".to_string())
    }

    pub(super) fn ensure_soll_registry_row(&self, project_code: &str) -> anyhow::Result<()> {
        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev)
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING",
            &json!([project_code]),
        )?;
        // REQ-AXO-323 Fault 3 — seed counters from MAX(numeric_suffix) per
        // type when the project already has nodes (e.g. registry row created
        // post-hoc to recover from an unregistered-project workaround).
        // Idempotent via GREATEST — counters never go down. Safe to run on
        // every call. project_code is validated upstream as ^[A-Z]{3}$ so
        // direct interpolation is safe.
        if is_valid_project_code(project_code) {
            self.graph_store
                .execute_param(&seed_registry_counters_sql(project_code), &json!([]))?;
        }
        Ok(())
    }

    pub(super) fn validate_explicit_canonical_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let raw = project_code.unwrap_or("").trim();
        if raw.is_empty() {
            // Auto-detect from registry: single project or cwd match.
            let _ = self.sync_project_code_registry_from_meta();
            if let Ok(codes) = self.query_single_column(
                "SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code ASC",
            ) {
                let codes: Vec<String> = codes
                    .into_iter()
                    .filter(|v| !v.trim().is_empty())
                    .collect();
                if codes.len() == 1 {
                    return Ok(codes.into_iter().next().unwrap());
                }
                if codes.len() > 1 {
                    // Try matching AXON_PROJECT_ROOT or cwd against registered project paths.
                    let search_path = std::env::var("AXON_PROJECT_ROOT")
                        .or_else(|_| std::env::current_dir().map(|p| p.to_string_lossy().to_string()))
                        .unwrap_or_default();
                    if !search_path.is_empty() {
                        let cwd_escaped = escape_sql(&search_path);
                        if let Ok(cwd_matches) = self.query_single_column(&format!(
                            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_path IS NOT NULL AND (project_path = '{}' OR starts_with('{}', project_path || '/'))",
                            cwd_escaped, cwd_escaped
                        )) {
                            let cwd_matches: Vec<String> = cwd_matches
                                .into_iter()
                                .filter(|v| !v.trim().is_empty())
                                .collect();
                            if cwd_matches.len() == 1 {
                                return Ok(cwd_matches.into_iter().next().unwrap());
                            }
                        }
                    }
                    return Err(anyhow!(
                        "`project_code` is required for {} when multiple projects exist. Known: {}. Provide the canonical code (e.g. `AXO`).",
                        action_label,
                        codes.join(", ")
                    ));
                }
            }
            return Err(anyhow!(
                "`project_code` is required for {}. Use a canonical 3-character uppercase code, e.g. `AXO`. Call `status` to discover your project.",
                action_label
            ));
        }

        if !is_valid_project_code(raw) || raw != raw.to_ascii_uppercase() {
            return Err(anyhow!(
                "Non-canonical project_code `{}` for {}. SOLL mutations require 3-char uppercase canonical codes (e.g. `AXO`). Known: {}",
                raw,
                action_label,
                self.known_project_codes_hint()
            ));
        }

        Ok(raw.to_string())
    }

    pub(super) fn require_registered_mutation_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let canonical_code =
            self.validate_explicit_canonical_project_code(project_code, action_label)?;

        // REQ-AXO-91577 — Sentinel project_codes (PRO, ...) self-heal when the
        // registry row is missing. Without this, a live regression that drops
        // the row (or a deployment that never seeded it) makes the whole
        // cross-tenant methodology surface uncreatable via the canonical API
        // while grandfathered nodes remain. Idempotent: `sync_project_registry_entry`
        // uses ON CONFLICT DO UPDATE, so calling on every sentinel mutation
        // is cheap and keeps metadata canonical.
        if is_sentinel_project_code(&canonical_code) {
            let (path, name) = sentinel_project_metadata(&canonical_code);
            self.graph_store
                .sync_project_registry_entry(&canonical_code, Some(name), Some(path))?;
            self.ensure_soll_registry_row(&canonical_code)?;
            return Ok(canonical_code);
        }

        let _ = self.sync_project_code_registry_from_meta();
        let escaped = escape_sql(&canonical_code);
        let rows = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = rows.into_iter().next() {
            self.ensure_soll_registry_row(&code)?;
            return Ok(code);
        }

        if let Ok(identity) = resolve_canonical_project_identity(&canonical_code) {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
            self.ensure_soll_registry_row(&identity.code)?;
            return Ok(identity.code);
        }

        Err(anyhow!(
            "Canonical project_code `{}` not found in ProjectCodeRegistry or .axon/meta.json. Known: {}",
            canonical_code,
            self.known_project_codes_hint()
        ))
    }

    pub(super) fn derive_project_name_from_path(
        &self,
        project_path: &str,
    ) -> anyhow::Result<String> {
        Path::new(project_path)
            .file_name()
            .map(|value| value.to_string_lossy().trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "Cannot derive project name from path `{}`",
                    project_path
                )
            })
    }

    fn split_project_name_parts(&self, raw: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut previous_is_lowercase = false;

        for ch in raw.chars() {
            if !ch.is_ascii_alphanumeric() {
                if !current.is_empty() {
                    parts.push(current.clone());
                    current.clear();
                }
                previous_is_lowercase = false;
                continue;
            }

            let is_uppercase = ch.is_ascii_uppercase();
            if is_uppercase && previous_is_lowercase && !current.is_empty() {
                parts.push(current.clone());
                current.clear();
            }
            current.push(ch.to_ascii_uppercase());
            previous_is_lowercase = ch.is_ascii_lowercase();
        }

        if !current.is_empty() {
            parts.push(current);
        }

        parts
    }

    fn candidate_project_codes_for_name(&self, project_name: &str) -> Vec<String> {
        fn is_consonant(ch: char) -> bool {
            matches!(
                ch,
                'B' | 'C'
                    | 'D'
                    | 'F'
                    | 'G'
                    | 'H'
                    | 'J'
                    | 'K'
                    | 'L'
                    | 'M'
                    | 'N'
                    | 'P'
                    | 'Q'
                    | 'R'
                    | 'S'
                    | 'T'
                    | 'V'
                    | 'W'
                    | 'X'
                    | 'Y'
                    | 'Z'
            )
        }

        let normalized: String = project_name
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase())
            .collect();
        let parts = self.split_project_name_parts(project_name);
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        let mut push_candidate = |candidate: String| {
            if is_valid_project_code(&candidate) && seen.insert(candidate.clone()) {
                candidates.push(candidate);
            }
        };

        if let Some(first) = parts.first() {
            let mut heuristic = String::new();
            if let Some(ch) = first.chars().next() {
                heuristic.push(ch);
            }
            for ch in first.chars().skip(1).filter(|ch| is_consonant(*ch)) {
                if heuristic.len() >= 2 {
                    break;
                }
                heuristic.push(ch);
            }
            for ch in parts.iter().skip(1).filter_map(|part| part.chars().next()) {
                if heuristic.len() >= 3 {
                    break;
                }
                heuristic.push(ch);
            }
            for ch in normalized.chars() {
                if heuristic.len() >= 3 {
                    break;
                }
                heuristic.push(ch);
            }
            push_candidate(heuristic);
        }

        if normalized.len() >= 3 {
            push_candidate(normalized.chars().take(3).collect());
        }

        let chars: Vec<char> = normalized.chars().collect();
        if chars.len() >= 3 {
            for window in chars.windows(3) {
                push_candidate(window.iter().collect());
            }
            push_candidate(format!(
                "{}{}{}",
                chars[0],
                chars[1],
                chars[chars.len() - 1]
            ));
            push_candidate(format!(
                "{}{}{}",
                chars[0],
                chars[chars.len() / 2],
                chars[chars.len() - 1]
            ));
        }

        candidates
    }

    pub(super) fn assign_project_code_for_init(
        &self,
        project_name: &str,
        project_path: &str,
    ) -> anyhow::Result<String> {
        let _ = self.sync_project_code_registry_from_meta();
        let escaped_path = escape_sql(project_path);
        if let Some(existing) = self
            .query_single_column(&format!(
                "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_path = '{}'",
                escaped_path
            ))?
            .into_iter()
            .next()
        {
            return Ok(existing);
        }

        let known_codes: HashSet<String> = self
            .query_single_column("SELECT project_code FROM soll.ProjectCodeRegistry")?
            .into_iter()
            .collect();
        for candidate in self.candidate_project_codes_for_name(project_name) {
            if !known_codes.contains(&candidate) {
                return Ok(candidate);
            }
        }

        Err(anyhow!(
            "Cannot assign a unique canonical `project_code` for `{}` from `{}`. Known codes: {}",
            project_name,
            project_path,
            self.known_project_codes_hint()
        ))
    }

    pub(super) fn resolve_canonical_project_identity_for_mutation(
        &self,
        project_code: &str,
    ) -> anyhow::Result<(String, String)> {
        let canonical_code = self
            .require_registered_mutation_project_code(Some(project_code), "this SOLL mutation")?;
        Ok((canonical_code.clone(), canonical_code))
    }

    pub(crate) fn resolve_project_code(&self, project_code: &str) -> anyhow::Result<String> {
        let escaped = escape_sql(project_code);
        let by_code = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = by_code.into_iter().next() {
            return Ok(code);
        }

        let _ = self.sync_project_code_registry_from_meta();
        let by_code_after_sync = self.query_single_column(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            escaped
        ))?;
        if let Some(code) = by_code_after_sync.into_iter().next() {
            return Ok(code);
        }

        if let Ok(identity) = resolve_canonical_project_identity(project_code) {
            let project_path = identity.project_path.to_string_lossy().to_string();
            self.graph_store.sync_project_registry_entry(
                &identity.code,
                identity.name.as_deref(),
                Some(&project_path),
            )?;
            return Ok(identity.code);
        }

        if let Err(e) = resolve_canonical_project_identity(project_code) {
            return Err(e);
        }

        Err(anyhow!(
            "Canonical project `{}` not found in .axon/meta.json or ProjectCodeRegistry",
            project_code
        ))
    }

    /// REQ-AXO-043 — shared helper for the wrong_project_scope contract.
    /// Used by every tool that takes a `project_code` and rejects it when
    /// the registry has no matching entry. Returns the structured error
    /// payload (with `isError=true`, `data.status="wrong_project_scope"`,
    /// `data.registered_project_codes`, `data.next_action`,
    /// `data.operator_guidance.{problem_class,likely_cause,
    /// next_best_actions,follow_up_tools,confidence}`) for the caller to
    /// `return Some(value)` directly.
    pub(crate) fn wrong_project_scope_response(
        &self,
        rejected_project_code: &str,
        tool_name: &str,
    ) -> serde_json::Value {
        self.wrong_project_scope_response_with_extras(rejected_project_code, tool_name, &[])
    }

    /// Variant of [`wrong_project_scope_response`] that lets a tool append
    /// tool-specific recovery hints to `next_best_actions` (e.g., the
    /// anomalies tool can advise "or omit `project` to scope to workspace:*").
    pub(crate) fn wrong_project_scope_response_with_extras(
        &self,
        rejected_project_code: &str,
        tool_name: &str,
        extra_actions: &[&str],
    ) -> serde_json::Value {
        let registered: Vec<String> = self
            .graph_store
            .query_json(
                "SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code",
            )
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Vec<String>>>(&s).ok())
            .map(|rows| {
                rows.into_iter()
                    .filter_map(|r| r.into_iter().next())
                    .collect()
            })
            .unwrap_or_default();
        let registered_values: Vec<serde_json::Value> = registered
            .iter()
            .map(|c| serde_json::Value::from(c.clone()))
            .collect();
        let next_action = if registered.is_empty() {
            "no projects registered yet — use axon_init_project to register one".to_string()
        } else {
            format!(
                "use one of the registered project_codes: {}",
                registered.join(", ")
            )
        };
        let mut next_best_actions: Vec<serde_json::Value> = vec![
            serde_json::Value::from("retry with a registered project_code"),
            serde_json::Value::from("or call axon_init_project to register a new project"),
        ];
        for extra in extra_actions {
            next_best_actions.push(serde_json::Value::from(*extra));
        }
        serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Project `{}` not found in registry for {}. {}",
                    rejected_project_code, tool_name, next_action,
                ),
            }],
            "isError": true,
            "data": {
                "status": "wrong_project_scope",
                "rejected_project_code": rejected_project_code,
                "registered_project_codes": registered_values.clone(),
                "next_action": next_action,
                "operator_guidance": {
                    "problem_class": "wrong_project_scope",
                    "likely_cause": "project_code_not_in_registry",
                    "next_best_actions": next_best_actions,
                    "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                    "confidence": "high",
                },
                "parameter_repair": {
                    "invalid_field": "project_code",
                    "supplied_value": rejected_project_code,
                    "registered_project_codes": registered_values,
                    "follow_up_tools": ["project_registry_lookup", "axon_init_project"],
                    "hint": format!("`{}` is not in the project registry; pick one of `registered_project_codes`, or call `axon_init_project` to register a new one", rejected_project_code),
                }
            }
        })
    }

    pub(crate) fn axon_project_registry_lookup(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let _ = self.sync_project_code_registry_from_meta();

        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let project_name = args
            .get("project_name")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let project_path = args
            .get("project_path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if project_code.is_none() && project_name.is_none() && project_path.is_none() {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": "`project_registry_lookup` attend au moins un de: `project_code`, `project_name`, `project_path`." }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "project_code|project_name|project_path",
                        "accepted_aliases": ["project_code", "project_name", "project_path"],
                        "follow_up_tools": ["help"],
                        "hint": "supply at least one of `project_code` / `project_name` / `project_path` to scope the lookup"
                    }
                }
            }));
        }

        let mut clauses = Vec::new();
        if let Some(code) = project_code {
            clauses.push(format!("project_code = '{}'", escape_sql(code)));
        }
        if let Some(name) = project_name {
            clauses.push(format!("project_name = '{}'", escape_sql(name)));
        }
        if let Some(path) = project_path {
            clauses.push(format!("project_path = '{}'", escape_sql(path)));
        }

        let query = format!(
            "SELECT project_code, COALESCE(project_name,''), COALESCE(project_path,'')
             FROM soll.ProjectCodeRegistry
             WHERE {}
             ORDER BY project_code ASC",
            clauses.join(" OR ")
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let matches: Vec<serde_json::Value> = rows
            .iter()
            .filter(|row| row.len() >= 3)
            .map(|row| {
                serde_json::json!({
                    "project_code": row[0],
                    "project_name": row[1],
                    "project_path": row[2]
                })
            })
            .collect();

        let first = matches
            .first()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let found = !matches.is_empty();
        let content = if found {
            format!(
                "Canonical project found: {} ({})",
                first
                    .get("project_name")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                first
                    .get("project_code")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
            )
        } else {
            "No canonical project found in ProjectCodeRegistry for the given criteria."
                .to_string()
        };

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": content }],
            "data": {
                "found": found,
                "ambiguous": matches.len() > 1,
                "project_code": first.get("project_code").cloned().unwrap_or(serde_json::json!(null)),
                "project_name": first.get("project_name").cloned().unwrap_or(serde_json::json!(null)),
                "project_path": first.get("project_path").cloned().unwrap_or(serde_json::json!(null)),
                "matches": matches,
                "operator_guidance": if found {
                    serde_json::json!({
                        "actionable_now": true,
                        "blocking_factors": if matches.len() > 1 {
                            vec![serde_json::json!({
                                "factor": "registry_match_ambiguous",
                                "severity": "medium",
                                "recommended_action": "prefer the exact canonical project_code from the returned matches before mutating"
                            })]
                        } else {
                            Vec::<serde_json::Value>::new()
                        },
                        "remediation_actions": if matches.len() > 1 {
                            vec!["prefer the exact canonical project_code from the returned matches before mutating"]
                        } else {
                            Vec::<&str>::new()
                        },
                        "follow_up_tools": ["project_status", "soll_query_context"],
                        "next_action": {
                            "kind": "use_canonical_project_code",
                            "tool": "project_status",
                            "when": "now"
                        }
                    })
                } else {
                    serde_json::json!({
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "project_not_found_in_registry",
                            "severity": "high",
                            "recommended_action": "use axon_init_project or retry with the exact canonical code, name, or path"
                        }],
                        "remediation_actions": [
                            "use axon_init_project or retry with the exact canonical code, name, or path"
                        ],
                        "follow_up_tools": ["axon_init_project", "project_registry_lookup"],
                        "next_action": {
                            "kind": "initialize_or_retry_project_identity",
                            "tool": "axon_init_project",
                            "when": "after_identity_confirmation"
                        }
                    })
                },
                "next_action": if found {
                    serde_json::json!({
                        "kind": "use_canonical_project_code",
                        "tool": "project_status",
                        "when": "now"
                    })
                } else {
                    serde_json::json!({
                        "kind": "initialize_or_retry_project_identity",
                        "tool": "axon_init_project",
                        "when": "after_identity_confirmation"
                    })
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests_req_axo_323 {
    use super::seed_registry_counters_sql;

    #[test]
    fn seed_sql_targets_all_eleven_counters_with_greatest_idempotence() {
        let sql = seed_registry_counters_sql("AXO");
        for col in [
            "last_vis", "last_pil", "last_req", "last_cpt", "last_dec", "last_mil",
            "last_val", "last_stk", "last_gui", "last_prv", "last_rev",
        ] {
            let pattern = format!("{col} = GREATEST({col},");
            assert!(
                sql.contains(&pattern),
                "missing idempotent GREATEST assignment for {col}: {sql}"
            );
        }
    }

    #[test]
    fn seed_sql_filters_by_canonical_id_regex_and_scoped_project_code() {
        let sql = seed_registry_counters_sql("AXO");
        assert!(sql.contains("project_code = 'AXO'"), "project_code scope missing: {sql}");
        assert!(
            sql.contains("id ~ '^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]+$'"),
            "canonical id regex missing: {sql}"
        );
        for prefix in ["VIS", "PIL", "REQ", "CPT", "DEC", "MIL", "VAL", "STK", "GUI", "PRV", "REV"] {
            let like = format!("id LIKE '{prefix}-%'");
            assert!(sql.contains(&like), "missing prefix filter for {prefix}: {sql}");
        }
    }

    #[test]
    fn seed_sql_targets_correct_registry_row() {
        let sql = seed_registry_counters_sql("PRO");
        assert!(
            sql.contains("UPDATE soll.Registry SET"),
            "must update soll.Registry: {sql}"
        );
        assert!(
            sql.ends_with("WHERE project_code = 'PRO'"),
            "must scope WHERE to the project's registry row: {sql}"
        );
    }
}
