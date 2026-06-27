use super::*;

impl McpServer {
    pub(super) fn query_single_column(&self, query: &str) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect())
    }

    pub(super) fn canonical_project_root_for_entity(&self, entity_id: &str) -> Option<PathBuf> {
        let project_code = project_code_from_canonical_entity_id(entity_id)?;
        // REQ-AXO-901971 — registre DB AUTORITATIF d'abord : `project_path` déterministe (peuplé par
        // axon_init_project). Immunise la résolution contre le scan filesystem fragile de
        // `resolve_canonical_project_identity` (candidate_directories → `fs::read_dir` avalé sous pression
        // ressources → un projet pourtant ENREGISTRÉ devient transitoirement « introuvable » → un
        // artifact_ref RELATIF est rejeté à tort alors que l'entity_id nomme le projet sans ambiguïté).
        if let Some(path) = self.lookup_project_path_by_code(&project_code) {
            return Some(path);
        }
        // Fallback découverte filesystem (.axon/meta.json) : bootstrap / projet pas encore enregistré.
        resolve_canonical_project_identity(&project_code)
            .ok()
            .map(|identity| identity.project_path)
    }

    pub(super) fn query_named_row(
        &self,
        query: &str,
        expected_columns: usize,
    ) -> anyhow::Result<Vec<String>> {
        let res = self.graph_store.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("SOLL entity not found"))?;
        if row.len() < expected_columns {
            return Err(anyhow!("Incomplete SOLL result for update"));
        }
        Ok(row)
    }

    pub(crate) fn next_server_numeric_id(
        &self,
        project_code: &str,
        kind: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        let (canonical_code, project_code) =
            self.resolve_canonical_project_identity_for_mutation(project_code)?;
        let (prefix, reg_col, node_type) = match kind {
            "vision" => ("VIS", "last_vis", Some("Vision")),
            "pillar" => ("PIL", "last_pil", Some("Pillar")),
            "requirement" => ("REQ", "last_req", Some("Requirement")),
            "concept" => ("CPT", "last_cpt", Some("Concept")),
            "decision" => ("DEC", "last_dec", Some("Decision")),
            "milestone" => ("MIL", "last_mil", Some("Milestone")),
            "validation" => ("VAL", "last_val", Some("Validation")),
            "stakeholder" => ("STK", "last_stk", Some("Stakeholder")),
            "guideline" => ("GUI", "last_gui", Some("Guideline")),
            "skill" => ("SKI", "last_ski", Some("Skill")), // REQ-AXO-91578
            "prompt_template" => ("PRT", "last_prt", Some("PromptTemplate")), // REQ-AXO-91579
            "technology_migration" => ("TMG", "last_tmg", Some("TechnologyMigration")), // REQ-AXO-901727
            "preview" => ("PRV", "last_prv", None),
            "revision" => ("REV", "last_rev", None),
            _ => return Err(anyhow!("Unknown id kind")),
        };

        // MIL-AXO-020 slice 1 — DDL function ensures atomic
        // per-(type, project_code) increment in a single round trip and
        // surfaces unregistered-project errors via RAISE.
        if let Some(node_type) = node_type {
            let allocated = self
                .graph_store
                .query_json_param(
                    "SELECT soll.allocate_node_id(?, ?)",
                    &json!([node_type, canonical_code]),
                )
                .map_err(|e| {
                    let msg = e.to_string();
                    if msg.contains("project_code_not_registered") {
                        anyhow!("project_code_not_registered:{}", canonical_code)
                    } else {
                        anyhow!("allocate_node_id failed: {}", msg)
                    }
                })?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&allocated).unwrap_or_default();
            let id_str = rows
                .into_iter()
                .next()
                .and_then(|r| r.into_iter().next())
                .ok_or_else(|| anyhow!("allocate_node_id returned no row"))?;
            let next = id_str
                .rsplit('-')
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .ok_or_else(|| anyhow!("allocate_node_id returned non-numeric suffix: {id_str}"))?;
            return Ok((canonical_code, project_code, prefix, next));
        }

        // `preview` / `revision` have no Node row → they don't go through
        // `soll.allocate_node_id` ; allocate from the Registry directly.
        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_ski, last_prt, last_prv, last_rev) \
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) ON CONFLICT (project_code) DO NOTHING",
            &json!([canonical_code]),
        )?;

        let current_query = format!(
            "SELECT COALESCE({}, 0) FROM soll.Registry WHERE project_code = '{}'",
            reg_col,
            escape_sql(&canonical_code)
        );
        let current = self
            .query_single_column(&current_query)?
            .into_iter()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        // REQ-AXO-902086 (feedback #26) : pour les révisions, le compteur Registry
        // `last_rev` peut se désynchroniser de `soll.Revision` — des lignes
        // `REV-{code}-NNN` migrées/seedées remplissent la table sans avancer le
        // compteur, si bien que `current+1` collisionne sur `revision_pkey`
        // (observé : compteur=1 alors que max=35 → ~34 retries). On ancre donc
        // `next` sur le vrai max du suffixe numérique. L'ancre `^REV-{code}-([0-9]+)$`
        // ne matche QUE les ids canoniques : les révisions `unlink-<ts>` (dont les
        // chiffres de fin empoisonneraient MAX) sont exclues. Aligne sur le chemin
        // soll_manager. L'UPDATE resynchronise le compteur du même coup.
        let next = if kind == "revision" {
            let reconcile = format!(
                "SELECT COALESCE(MAX(CAST(substring(revision_id FROM '^REV-{code}-([0-9]+)$') AS INTEGER)), 0) \
                 FROM soll.Revision WHERE project_code = '{code}'",
                code = escape_sql(&canonical_code)
            );
            let max_existing = self
                .query_single_column(&reconcile)?
                .into_iter()
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            current.max(max_existing) + 1
        } else {
            current + 1
        };
        self.graph_store.execute(&format!(
            "UPDATE soll.Registry SET {} = {} WHERE project_code = '{}'",
            reg_col,
            next,
            escape_sql(&canonical_code)
        ))?;

        Ok((canonical_code, project_code, prefix, next))
    }

    pub(crate) fn next_soll_numeric_id(
        &self,
        project_code: &str,
        entity: &str,
    ) -> anyhow::Result<(String, String, &'static str, u64)> {
        self.next_server_numeric_id(project_code, entity)
    }

    #[allow(dead_code)]
    pub(super) fn restore_soll_relation(
        &self,
        relation_type: &str,
        source_id: &str,
        target_id: &str,
    ) -> anyhow::Result<()> {
        let normalized = relation_type.to_uppercase();
        let (selected, policy) =
            self.select_relation_type_for_link(source_id, target_id, Some(&normalized))?;
        self.insert_validated_relation(selected, source_id, target_id, policy)?;
        Ok(())
    }
}

pub(super) fn query_first_sql_cell(server: &McpServer, query: &str) -> Option<String> {
    let raw = server.execute_raw_sql(query).ok()?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
    let first = rows.first()?;
    let value = first.first()?;
    if let Some(text) = value.as_str() {
        Some(text.to_string())
    } else {
        Some(value.to_string())
    }
}

impl McpServer {
    fn soll_node_type_for_entity(entity: &str) -> Option<&'static str> {
        match entity {
            "vision" => Some("Vision"),
            "pillar" => Some("Pillar"),
            "requirement" => Some("Requirement"),
            "concept" => Some("Concept"),
            "decision" => Some("Decision"),
            "milestone" => Some("Milestone"),
            "stakeholder" => Some("Stakeholder"),
            "validation" => Some("Validation"),
            "guideline" => Some("Guideline"),
            "skill" => Some("Skill"),                    // REQ-AXO-91578
            "prompt_template" => Some("PromptTemplate"), // REQ-AXO-91579
            "technology_migration" => Some("TechnologyMigration"), // REQ-AXO-901727
            _ => None,
        }
    }

    pub(super) fn resolve_soll_id(
        &self,
        entity: &str,
        project_code: &str,
        title: &str,
        logical_key: &str,
    ) -> Option<String> {
        let node_type = Self::soll_node_type_for_entity(entity)?;

        let by_metadata = format!(
            "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND metadata->>'logical_key' = '{}' ORDER BY id DESC LIMIT 1",
            escape_sql(node_type),
            escape_sql(project_code),
            escape_sql(logical_key)
        );
        if let Some(found) = query_first_sql_cell(self, &by_metadata) {
            return Some(found);
        }

        if !title.trim().is_empty() {
            let by_title = format!(
                "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND title = '{}' ORDER BY id DESC LIMIT 1",
                escape_sql(node_type),
                escape_sql(project_code),
                escape_sql(title)
            );
            if let Some(found) = query_first_sql_cell(self, &by_title) {
                return Some(found);
            }
        }

        None
    }
}

pub(super) fn soll_tool_text(resp: Option<&Value>) -> Option<String> {
    resp.and_then(|v| {
        v.get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|entry| entry.get("text"))
            .and_then(|text| text.as_str())
            .map(|s| s.to_string())
    })
}

pub(super) fn soll_tool_is_error(resp: Option<&Value>) -> bool {
    resp.and_then(|v| v.get("isError"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub(super) fn extract_soll_id_from_message(text: String) -> Option<String> {
    let start = text.find('`')?;
    let end = text[start + 1..].find('`')?;
    Some(text[start + 1..start + 1 + end].to_string())
}

pub(super) fn project_scope_clause_for_table(
    id_column: &str,
    project_code: Option<&str>,
) -> String {
    project_code
        .map(|code| format!(" WHERE {} LIKE '%-{}-%'", id_column, escape_sql(code)))
        .unwrap_or_default()
}

pub(super) fn project_scope_clause_for_relation(project_code: Option<&str>) -> String {
    project_code
        .map(|code| {
            let escaped = escape_sql(code);
            format!(
                " WHERE source_id LIKE '%-{}-%' OR target_id LIKE '%-{}-%'",
                escaped, escaped
            )
        })
        .unwrap_or_default()
}

pub(super) fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
pub(super) fn format_canonical_id(prefix: &str, project_code: &str, next: u64) -> String {
    format!("{prefix}-{project_code}-{next:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn canonical_regex() -> Regex {
        Regex::new(r"^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]{3,}$").unwrap()
    }

    #[test]
    fn format_canonical_id_zero_pads_below_one_hundred() {
        assert_eq!(format_canonical_id("REQ", "AXO", 1), "REQ-AXO-001");
        assert_eq!(format_canonical_id("REQ", "AXO", 42), "REQ-AXO-042");
        assert_eq!(format_canonical_id("CPT", "BKS", 99), "CPT-BKS-099");
    }

    #[test]
    fn format_canonical_id_keeps_natural_length_above_three_digits() {
        assert_eq!(format_canonical_id("REQ", "AXO", 100), "REQ-AXO-100");
        assert_eq!(format_canonical_id("REQ", "AXO", 999), "REQ-AXO-999");
        assert_eq!(format_canonical_id("REQ", "AXO", 1_000), "REQ-AXO-1000");
        assert_eq!(format_canonical_id("REQ", "AXO", 9_999), "REQ-AXO-9999");
        assert_eq!(format_canonical_id("REQ", "AXO", 10_000), "REQ-AXO-10000");
        assert_eq!(
            format_canonical_id("REQ", "AXO", 1_000_000),
            "REQ-AXO-1000000"
        );
    }

    #[test]
    fn format_canonical_id_outputs_match_canonical_regex() {
        let re = canonical_regex();
        for n in [1u64, 42, 99, 100, 999, 1_000, 9_999, 10_000, 99_999] {
            let id = format_canonical_id("REQ", "AXO", n);
            assert!(re.is_match(&id), "{id} must match canonical pattern");
        }
        // Project code with alphanumeric (TE2) — canonical project_code regex allows it.
        assert!(re.is_match(&format_canonical_id("DEC", "TE2", 1)));
    }

    #[test]
    fn canonical_regex_rejects_malformed_ids() {
        let re = canonical_regex();
        assert!(!re.is_match("REQ-AXO-01"), "<3 digit suffix forbidden");
        assert!(!re.is_match("REQU-AXO-001"), "4-letter type forbidden");
        assert!(!re.is_match("REQ-AX-001"), "2-char project_code forbidden");
        assert!(
            !re.is_match("REQ-1AX-001"),
            "project_code first char must be alpha"
        );
        assert!(
            !re.is_match("REQ-axo-001"),
            "project_code must be uppercase"
        );
        assert!(!re.is_match("REQ-AXO-01a"), "suffix must be digits only");
        assert!(!re.is_match("REQ-AXO-"), "suffix cannot be empty");
        assert!(!re.is_match("R3Q-AXO-001"), "type must be alphabetic");
    }
}
