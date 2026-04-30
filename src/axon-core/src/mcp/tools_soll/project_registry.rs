use super::*;

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
                "aucun code connu".to_string()
            } else {
                codes.join(", ")
            }
        })
        .unwrap_or_else(|_| "aucun code connu".to_string())
    }

    pub(super) fn ensure_soll_registry_row(&self, project_code: &str) -> anyhow::Result<()> {
        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev)
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
             ON CONFLICT (project_code) DO NOTHING",
            &json!([project_code]),
        )?;
        Ok(())
    }

    pub(super) fn validate_explicit_canonical_project_code(
        &self,
        project_code: Option<&str>,
        action_label: &str,
    ) -> anyhow::Result<String> {
        let raw = project_code.unwrap_or("").trim();
        if raw.is_empty() {
            return Err(anyhow!(
                "`project_code` est obligatoire pour {}. Utilisez un code canonique de 3 caractères alphanumériques majuscules, par exemple `AXO`.",
                action_label
            ));
        }

        if !is_valid_project_code(raw) || raw != raw.to_ascii_uppercase() {
            return Err(anyhow!(
                "Identifiant projet non canonique `{}` pour {}. Les mutations SOLL acceptent uniquement `project_code` au format canonique de 3 caractères alphanumériques majuscules, par exemple `AXO`. Codes connus: {}",
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
            "Code projet canonique `{}` introuvable dans soll.ProjectCodeRegistry ou `.axon/meta.json`. Codes connus: {}",
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
                    "Impossible de dériver le nom projet depuis le chemin `{}`",
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
            "Impossible d'attribuer un `project_code` canonique unique pour `{}` depuis `{}`. Codes connus: {}",
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
            .require_registered_mutation_project_code(Some(project_code), "cette mutation SOLL")?;
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
            "Projet canonique `{}` introuvable dans `.axon/meta.json` ou soll.ProjectCodeRegistry",
            project_code
        ))
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
                "isError": true
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
                "Projet canonique trouvé: {} ({})",
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
            "Aucun projet canonique trouvé dans ProjectCodeRegistry pour les critères fournis."
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
