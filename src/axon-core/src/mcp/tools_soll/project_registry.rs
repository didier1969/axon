use super::*;

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
        ("last_ski", "SKI"), // REQ-AXO-91578 — Skill entity counter
        ("last_prt", "PRT"), // REQ-AXO-91579 — PromptTemplate entity counter
        ("last_prv", "PRV"),
        ("last_rev", "REV"),
    ]
    .iter()
    .map(|(col, prefix)| {
        format!(
            "{col} = GREATEST({col}, {expr})",
            col = col,
            expr = max_for(prefix)
        )
    })
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

    /// REQ-AXO-902467 — `pub(crate)` : le refus canonique `unresolved_project_error`
    /// (guidance.rs) doit nommer les candidats depuis N'IMPORTE quel outil, pas
    /// seulement depuis `tools_soll`. Elargir la visibilite d'un helper existant
    /// plutot que d'en recopier un second (GUI-PRO-013).
    pub(crate) fn known_project_codes_hint(&self) -> String {
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
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_ski, last_prt, last_prv, last_rev)
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
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
                let codes: Vec<String> =
                    codes.into_iter().filter(|v| !v.trim().is_empty()).collect();
                if codes.len() == 1 {
                    return Ok(codes.into_iter().next().unwrap());
                }
                if codes.len() > 1 {
                    // REQ-AXO-902286 — match the CALLER's directory (per-request client
                    // cwd from the tunnel header, else AXON_PROJECT_ROOT / server cwd)
                    // against registered project paths, so a SOLL mutation lands in the
                    // project the agent is working in, not the shared brain's own (AXO).
                    // REQ-AXO-902312 — the MOST SPECIFIC registered path wins, it is not
                    // an ambiguity. Ancestor projects are legitimately registered
                    // (/home/dstadel, /home/dstadel/projects, /home/dstadel/projects/axon):
                    // a cwd sits inside ALL of them, so the old `len() == 1` guard bailed
                    // out on the NORMAL case and demanded a `project_code` the server could
                    // already derive — 69 occurrences of `wrong_project_scope`.
                    //
                    // REQ-AXO-902128 fixed exactly this in `auto_resolve_project_code_str`
                    // (tools_framework_runtime_status.rs). It was never carried here, so the
                    // two functions resolving ONE rule disagreed — the same divergence
                    // pattern as the measurement defects of REQ-AXO-902309/902310. Same
                    // ORDER BY, deliberately, so a future reader sees one rule twice rather
                    // than two rules.
                    let search_path = crate::mcp::effective_project_search_path();
                    if !search_path.is_empty() {
                        let cwd_escaped = escape_sql(&search_path);
                        if let Ok(cwd_matches) = self.query_single_column(&format!(
                            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_path IS NOT NULL AND (project_path = '{}' OR starts_with('{}', project_path || '/')) ORDER BY length(project_path) DESC LIMIT 1",
                            cwd_escaped, cwd_escaped
                        )) {
                            if let Some(code) =
                                cwd_matches.into_iter().find(|v| !v.trim().is_empty())
                            {
                                return Ok(code);
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

    /// REQ-AXO-902453 — les parents que ce kind peut LÉGALEMENT atteindre, avec
    /// la relation à employer : `(id, titre, relation)`.
    ///
    /// Signalé par TE2 (`llm_feedback` #224) : le refus `attach_required`
    /// proposait les six Pillars du projet à un `milestone`. **Aucun n'est
    /// atteignable depuis un MIL** — et le message SUIVANT l'expliquait. Deux
    /// appels perdus, et une hésitation sur laquelle des deux réponses croire.
    ///
    /// La matrice existait déjà et était correcte (`relation_policy_for_pair`) ;
    /// c'est une jointure, pas une fonctionnalité. Puisque le kind source est
    /// connu, la relation est nommée avec chaque parent : le second appel est
    /// correct du premier coup.
    ///
    /// `SUPERSEDES` est écarté des propositions : il MUTE la cible en
    /// `superseded`. Proposer une relation destructive à quelqu'un qui cherche
    /// juste un parent serait la pire des suggestions (même raison qu'en
    /// `completeness_relations.rs`).
    pub(super) fn candidate_parents_for_source(
        &self,
        project_code: &str,
        source_prefix: &str,
    ) -> Vec<(String, String, String)> {
        /// Assez pour choisir, trop peu pour noyer.
        const MAX_CANDIDATES: usize = 12;

        // Quels TYPES ce kind peut-il atteindre, et par quelle relation ?
        let mut relation_by_type: HashMap<String, String> = HashMap::new();
        for route in allowed_relation_targets_from_source(source_prefix) {
            let Some(target_prefix) = route.get("target_kind").and_then(Value::as_str) else {
                continue;
            };
            let relations: Vec<&str> = route
                .get("allowed_relations")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let chosen = route
                .get("default_relation")
                .and_then(Value::as_str)
                .or_else(|| relations.iter().copied().find(|r| *r != "SUPERSEDES"));
            let Some(relation) = chosen.filter(|r| *r != "SUPERSEDES") else {
                continue;
            };
            if let Some(node_type) = node_type_for_prefix(target_prefix) {
                relation_by_type.insert(node_type.to_string(), relation.to_string());
            }
        }
        if relation_by_type.is_empty() {
            return Vec::new();
        }

        let escaped = escape_sql(project_code);
        let type_list = relation_by_type
            .keys()
            .map(|t| format!("'{}'", escape_sql(t)))
            .collect::<Vec<_>>()
            .join(", ");
        self.graph_store
            .query_json(&format!(
                "SELECT id, COALESCE(title, ''), type FROM soll.Node \
                 WHERE type IN ({type_list}) AND project_code = '{escaped}' \
                   AND COALESCE(status, '') NOT IN ('superseded', 'rejected', 'archived') \
                 ORDER BY type ASC, id ASC LIMIT {MAX_CANDIDATES}"
            ))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                let mut it = row.into_iter();
                let id = it.next()?;
                let title = it.next().unwrap_or_default();
                let node_type = it.next().unwrap_or_default();
                let relation = relation_by_type.get(&node_type)?.clone();
                Some((id, title, relation))
            })
            .collect()
    }

    pub(super) fn require_registered_mutation_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let canonical_code =
            self.validate_explicit_canonical_project_code(project_code, action_label)?;

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
            .ok_or_else(|| anyhow!("Cannot derive project name from path `{}`", project_path))
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
            .query_json("SELECT project_code FROM soll.ProjectCodeRegistry ORDER BY project_code")
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
        let mut rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();

        // REQ-AXO-902368 — `project_path` matched by EQUALITY only, so a path INSIDE a
        // project resolved to nothing. Verified first-hand:
        //   /home/dstadel/projects/aps3d            -> APS
        //   /home/dstadel/projects/aps3d/lib/aps3d  -> "No canonical project found"
        // An agent working in a subdirectory — the normal case — could not resolve its
        // own project, while `detect_project` walks ancestors for exactly this reason.
        // The two resolvers disagreed on what "this path belongs to project X" means.
        //
        // Fall back to the DEEPEST registered ancestor: with nested projects the most
        // specific one wins, which is the only answer that can be right. The trailing
        // separator matters — `/proj/axon-tools` must not resolve to `/proj/axon`.
        let mut path_resolved_by_ancestor: Option<String> = None;
        if rows.is_empty() {
            if let Some(path) = project_path {
                let ancestor_sql = format!(
                    "SELECT project_code, COALESCE(project_name,''), COALESCE(project_path,'') \
                     FROM soll.ProjectCodeRegistry \
                     WHERE project_path <> '' \
                       AND ('{p}' = project_path OR '{p}' LIKE project_path || '/%') \
                     ORDER BY length(project_path) DESC \
                     LIMIT 1",
                    p = escape_sql(path)
                );
                let ancestor_raw = self
                    .graph_store
                    .query_json(&ancestor_sql)
                    .unwrap_or_else(|_| "[]".to_string());
                let ancestor_rows: Vec<Vec<String>> =
                    serde_json::from_str(&ancestor_raw).unwrap_or_default();
                if let Some(row) = ancestor_rows.into_iter().next() {
                    path_resolved_by_ancestor = row.get(2).cloned();
                    rows = vec![row];
                }
            }
        }
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
            // Say WHICH path answered when it is not the one asked for — a silent
            // ancestor match would read as an exact hit.
            let ancestor_note = path_resolved_by_ancestor
                .as_deref()
                .filter(|root| Some(*root) != project_path)
                .map(|root| format!(" — résolu par le projet englobant `{root}`"))
                .unwrap_or_default();
            format!(
                "Canonical project found: {} ({}){ancestor_note}",
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
            "No canonical project found in ProjectCodeRegistry for the given criteria.".to_string()
        };

        Some(serde_json::json!({
            "content": [{ "type": "text", "text": content }],
            "data": {
                "found": found,
                "resolved_by_ancestor_path": path_resolved_by_ancestor,
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

    /// REQ-AXO-901618 — expose the `soll.Registry` allocation counters and the
    /// NEXT id the server would assign per entity type, so an LLM can reference a
    /// canonical id in a doc/memo BEFORE the `soll_manager(create)` call that
    /// allocates it. Read-only projection of the registry row (the actual
    /// allocation still goes through `soll.allocate_node_id`, which gap-skips —
    /// `next_id` is therefore the lower bound the next create will land on or
    /// after, never below).
    /// REQ-AXO-902369 / REQ-AXO-902507 — RETIRER un projet du registre.
    ///
    /// On pouvait entrer, jamais sortir. `VIS-ELE-001` l'écrivait noir sur blanc : *« La
    /// ligne de `soll.ProjectCodeRegistry` n'a pas pu être retirée : aucun outil MCP ne le
    /// permet. »* Résultat : `ELE` a survécu **six jours** à une demande de suppression
    /// explicite de l'opérateur, et un tenant (VPC) a dû câbler une table de correspondance
    /// `ELE → FSF` pour contourner un code qui n'aurait plus dû exister.
    ///
    /// Un registre où l'on peut entrer sans pouvoir sortir accumule les fantômes — quatre à
    /// ce jour, qui détenaient 90 % des fichiers du parc.
    ///
    /// Trois gardes, dans cet ordre. Elles refusent par défaut ; c'est l'inverse d'une
    /// suppression qui demanderait confirmation après coup.
    pub(crate) fn axon_project_registry_remove(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let code = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| v.to_ascii_uppercase());
        let Some(code) = code else {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text":
                    "`project_registry_remove` attend `project_code`." }],
                "isError": true,
                "data": { "status": "input_invalid",
                          "parameter_repair": { "invalid_field": "project_code",
                                                "follow_up_tools": ["project_registry_lookup"] } }
            }));
        };
        let esc = code.replace('\'', "''");
        let compte = |sql: &str| -> i64 {
            self.graph_store
                .query_json(sql)
                .ok()
                .and_then(|r| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&r).ok())
                .and_then(|rows| rows.into_iter().next())
                .and_then(|row| row.into_iter().next())
                .and_then(|c| c.as_i64().or_else(|| c.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(-1)
        };
        let refus = |raison: String, remede: &str| -> Option<serde_json::Value> {
            Some(serde_json::json!({
                "content": [{ "type": "text", "text": raison }],
                "isError": true,
                "data": { "status": "refused", "project_code": code,
                          "next_action": { "kind": "operator_decision", "detail": remede } }
            }))
        };

        let chemin: String = self
            .graph_store
            .query_json(&format!(
                "SELECT project_path FROM soll.ProjectCodeRegistry WHERE project_code = '{esc}'"
            ))
            .ok()
            .and_then(|r| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&r).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if chemin.is_empty() {
            return refus(
                format!("`{code}` n'est pas dans le registre — rien à retirer."),
                "vérifier le code via `project_registry_lookup`",
            );
        }

        // GARDE 1 — de l'intention réelle : ce n'est pas une coquille.
        // Vision/Pillar auto-seedés ne comptent pas : `axon_init_project` les crée seul.
        let fond = compte(&format!(
            "SELECT count(*)::bigint FROM soll.Node WHERE project_code = '{esc}' \
             AND type IN ('Requirement','Decision','Milestone','Concept','Guideline')"
        ));
        if fond > 0 {
            return refus(
                format!(
                    "⛔ `{code}` porte {fond} nœud(s) SOLL de fond (REQ/DEC/MIL/CPT/GUI). \
                     Ce n'est pas une coquille : quelqu'un y a écrit de l'intention.\n\n\
                     Reporter cette intention vers un projet légitime AVANT tout retrait — \
                     `soll_manager(action=append_section)` sur la Vision qui l'absorbe. \
                     Effacer un nœud sans reporter ce qu'il dit, c'est perdre la seule chose \
                     qui ne se régénère pas."
                ),
                "reporter l'intention, puis relancer",
            );
        }

        // GARDE 2 — des fichiers qu'un autre projet n'a pas encore repris.
        let a_reprendre = compte(&format!(
            "SELECT count(*)::bigint FROM ist.IndexedFile f WHERE f.project_code = '{esc}' \
             AND EXISTS (SELECT 1 FROM soll.ProjectCodeRegistry r \
                          WHERE r.project_code <> '{esc}' AND r.project_path IS NOT NULL \
                            AND starts_with(f.path, r.project_path || '/'))"
        ));
        if a_reprendre > 0 {
            return refus(
                format!(
                    "⛔ {a_reprendre} fichier(s) de `{code}` appartiennent en réalité à un projet \
                     plus spécifique qui ne les a pas encore repris.\n\n\
                     Les retirer maintenant perdrait leur index. Réindexer d'abord les projets \
                     concernés (`rescan_project full=true`) : l'attribution se corrige seule \
                     depuis REQ-AXO-902506, et ce compte tombera à 0."
                ),
                "réindexer les projets qui doivent reprendre ces fichiers, puis relancer",
            );
        }

        // GARDE 3 — confirmation explicite. Un retrait est irréversible côté registre.
        if args.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
            let orphelins = compte(&format!(
                "SELECT count(*)::bigint FROM ist.IndexedFile WHERE project_code = '{esc}'"
            ));
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!(
                    "✅ `{code}` ({chemin}) est retirable — les deux gardes passent.\n\n\
                     Ce que le retrait effacera : {orphelins} fichier(s) indexés qu'AUCUN autre \
                     projet ne réclame, plus les symboles/chunks associés et les coquilles SOLL \
                     (Vision/Pillar auto-seedés). Les messages ENVOYÉS par `{code}` sont \
                     conservés : ils vivent dans la boîte du destinataire.\n\n\
                     Relancer avec `confirm=true` pour exécuter."
                )}],
                "data": { "status": "ok", "dry_run": true, "project_code": code,
                          "project_path": chemin, "indexed_files_dropped": orphelins }
            }));
        }

        let sql = format!(
            "DELETE FROM ist.Edge WHERE project_code='{esc}'; \
             DELETE FROM ist.ChunkEmbedding WHERE project_code='{esc}'; \
             DELETE FROM ist.Chunk WHERE project_code='{esc}'; \
             DELETE FROM ist.Symbol WHERE project_code='{esc}'; \
             DELETE FROM ist.IndexedFile WHERE project_code='{esc}'; \
             DELETE FROM axon.mailbox_message WHERE to_project='{esc}'; \
             DELETE FROM axon.mailbox_cursor WHERE project_code='{esc}'; \
             DELETE FROM soll.Edge WHERE source_id IN (SELECT id FROM soll.Node WHERE project_code='{esc}') \
                                      OR target_id IN (SELECT id FROM soll.Node WHERE project_code='{esc}'); \
             DELETE FROM soll.Node WHERE project_code='{esc}'; \
             DELETE FROM soll.ProjectCodeRegistry WHERE project_code='{esc}';"
        );
        if let Err(e) = self.graph_store.execute(&sql) {
            return Some(serde_json::json!({
                "content": [{ "type": "text", "text": format!("Retrait de `{code}` échoué : {e}") }],
                "isError": true,
                "data": { "status": "writer_failed", "diagnostic_excerpt": e.to_string() }
            }));
        }
        let reste = compte(&format!(
            "SELECT count(*)::bigint FROM soll.ProjectCodeRegistry WHERE project_code='{esc}'"
        ));
        Some(serde_json::json!({
            "content": [{ "type": "text", "text": format!(
                "🗑️ `{code}` retiré du registre ({chemin}).\n\n\
                 Entrées restantes portant ce code : {reste} (attendu 0). Les messages ENVOYÉS \
                 par `{code}` sont conservés chez leurs destinataires."
            )}],
            "data": { "status": "ok", "project_code": code, "removed": true,
                      "registry_rows_left": reste }
        }))
    }

    pub(crate) fn axon_soll_id_registry(
        &self,
        args: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        let _ = self.sync_project_code_registry_from_meta();
        let project_code = args
            .get("project_code")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let project_code = match project_code {
            Some(code) => code,
            None => {
                return Some(serde_json::json!({
                    "content": [{ "type": "text", "text": "`soll_id_registry` attend `project_code`." }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "invalid_field": "project_code",
                            "follow_up_tools": ["project_registry_lookup", "status"],
                            "hint": "supply the canonical project_code (e.g. \"AXO\")"
                        }
                    }
                }));
            }
        };
        const COUNTERS: &[(&str, &str)] = &[
            ("last_vis", "VIS"),
            ("last_pil", "PIL"),
            ("last_req", "REQ"),
            ("last_cpt", "CPT"),
            ("last_dec", "DEC"),
            ("last_mil", "MIL"),
            ("last_val", "VAL"),
            ("last_stk", "STK"),
            ("last_gui", "GUI"),
            ("last_ski", "SKI"),
            ("last_prt", "PRT"),
        ];
        let cols = COUNTERS
            .iter()
            .map(|(col, _)| format!("COALESCE({col}, 0)"))
            .collect::<Vec<_>>()
            .join(", ");
        let query = format!(
            "SELECT {cols} FROM soll.Registry WHERE project_code = '{}'",
            escape_sql(project_code)
        );
        let raw = self
            .graph_store
            .query_json(&query)
            .unwrap_or_else(|_| "[]".to_string());
        // BIGINT cells come back from query_json as either a JSON number OR a
        // JSON string depending on the codec path — mirror the canonical
        // `query_single_i64_writer` extraction (number-or-parsed-string).
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let found = !rows.is_empty();
        let row = rows.first().cloned().unwrap_or_default();
        let cell_i64 = |v: &serde_json::Value| -> i64 {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                .unwrap_or(0)
        };
        let counters: Vec<serde_json::Value> = COUNTERS
            .iter()
            .enumerate()
            .map(|(i, (_, prefix))| {
                let last = row.get(i).map(cell_i64).unwrap_or(0);
                serde_json::json!({
                    "type": prefix,
                    "last": last,
                    "next_id": format!("{}-{}-{}", prefix, project_code, last + 1)
                })
            })
            .collect();
        let preview = counters
            .iter()
            .filter(|c| c.get("last").and_then(|v| v.as_i64()).unwrap_or(0) > 0)
            .filter_map(|c| c.get("next_id").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        Some(serde_json::json!({
            "content": [{ "type": "text", "text": if found {
                format!("ID registry {project_code} — next allocatable: {preview}")
            } else {
                format!("No soll.Registry row for `{project_code}` (project not initialized).")
            } }],
            "data": {
                "status": if found { "ok" } else { "input_not_found" },
                "project_code": project_code,
                "counters": counters,
                "note": "next_id is a lower bound; soll.allocate_node_id gap-skips, so the real id may be higher.",
                "follow_up_tools": ["soll_manager", "soll_query_context"]
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
            "last_vis", "last_pil", "last_req", "last_cpt", "last_dec", "last_mil", "last_val",
            "last_stk", "last_gui", "last_prv", "last_rev",
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
        assert!(
            sql.contains("project_code = 'AXO'"),
            "project_code scope missing: {sql}"
        );
        assert!(
            sql.contains("id ~ '^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]+$'"),
            "canonical id regex missing: {sql}"
        );
        for prefix in [
            "VIS", "PIL", "REQ", "CPT", "DEC", "MIL", "VAL", "STK", "GUI", "PRV", "REV",
        ] {
            let like = format!("id LIKE '{prefix}-%'");
            assert!(
                sql.contains(&like),
                "missing prefix filter for {prefix}: {sql}"
            );
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

#[cfg(test)]
mod req_902368_ancestor_path_tests {
    /// REQ-AXO-902368 — la règle de préfixe, isolée de PG pour être falsifiable.
    /// Miroir exact du prédicat SQL : `'{p}' = project_path OR '{p}' LIKE project_path || '/%'`.
    fn path_belongs_to(candidate: &str, project_root: &str) -> bool {
        candidate == project_root || candidate.starts_with(&format!("{project_root}/"))
    }

    /// Le plus PROFOND ancêtre l'emporte (projets imbriqués) — miroir de
    /// `ORDER BY length(project_path) DESC LIMIT 1`.
    fn deepest<'a>(candidate: &str, roots: &[&'a str]) -> Option<&'a str> {
        roots
            .iter()
            .filter(|r| path_belongs_to(candidate, r))
            .max_by_key(|r| r.len())
            .copied()
    }

    #[test]
    fn a_subdirectory_resolves_to_its_project() {
        // Le cas constaté : le chemin racine résolvait, le sous-répertoire non.
        assert!(path_belongs_to(
            "/home/dstadel/projects/aps3d/lib/aps3d",
            "/home/dstadel/projects/aps3d"
        ));
        assert!(path_belongs_to(
            "/home/dstadel/projects/aps3d",
            "/home/dstadel/projects/aps3d"
        ));
    }

    #[test]
    fn a_sibling_sharing_a_prefix_is_not_a_match() {
        // LA falsification. Un préfixe nu ferait résoudre `axon-tools` vers `axon` :
        // le séparateur est ce qui sépare « à l'intérieur de » de « commence pareil ».
        assert!(!path_belongs_to(
            "/home/dstadel/projects/axon-tools",
            "/home/dstadel/projects/axon"
        ));
        assert!(!path_belongs_to(
            "/home/dstadel/projects/axonium/src",
            "/home/dstadel/projects/axon"
        ));
    }

    #[test]
    fn the_deepest_ancestor_wins_for_nested_projects() {
        // Un dépôt imbriqué dans un autre : seul le plus spécifique peut être juste.
        let roots = ["/home/dstadel/projects", "/home/dstadel/projects/axon"];
        assert_eq!(
            deepest("/home/dstadel/projects/axon/src/axon-core", &roots),
            Some("/home/dstadel/projects/axon")
        );
        assert_eq!(
            deepest("/home/dstadel/projects/autre/lib", &roots),
            Some("/home/dstadel/projects")
        );
    }

    #[test]
    fn an_unregistered_path_still_resolves_to_nothing() {
        // La branche « pas trouvé » reste ATTEIGNABLE : le repli élargit la
        // résolution, il ne la rend pas complaisante.
        let roots = ["/home/dstadel/projects/axon"];
        assert_eq!(deepest("/var/tmp/ailleurs", &roots), None);
    }
}
