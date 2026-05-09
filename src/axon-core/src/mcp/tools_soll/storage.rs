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
        let (prefix, reg_col, table, id_expr) = match kind {
            "vision" => ("VIS", "last_vis", "soll.Node", "id"),
            "pillar" => ("PIL", "last_pil", "soll.Node", "id"),
            "requirement" => ("REQ", "last_req", "soll.Node", "id"),
            "concept" => ("CPT", "last_cpt", "soll.Node", "id"),
            "decision" => ("DEC", "last_dec", "soll.Node", "id"),
            "milestone" => ("MIL", "last_mil", "soll.Node", "id"),
            "validation" => ("VAL", "last_val", "soll.Node", "id"),
            "stakeholder" => ("STK", "last_stk", "soll.Node", "id"),
            "guideline" => ("GUI", "last_gui", "soll.Node", "id"),
            "preview" => ("PRV", "last_prv", "soll.RevisionPreview", "preview_id"),
            "revision" => ("REV", "last_rev", "soll.Revision", "revision_id"),
            _ => return Err(anyhow!("Unknown id kind")),
        };

        self.graph_store.execute_param(
            "INSERT INTO soll.Registry (project_code, id, last_vis, last_pil, last_req, last_cpt, last_dec, last_mil, last_val, last_stk, last_gui, last_prv, last_rev) \
             VALUES (?, 'AXON_GLOBAL', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) ON CONFLICT (project_code) DO NOTHING",
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

        let ids_query = format!(
            "SELECT {} FROM {} WHERE {} LIKE '{}-{}-%'",
            id_expr,
            table,
            id_expr,
            prefix,
            escape_sql(&project_code)
        );
        let observed_max = self
            .query_single_column(&ids_query)?
            .into_iter()
            .filter_map(|value| parse_numeric_suffix(&value))
            .max()
            .unwrap_or(0);

        let next = current.max(observed_max) + 1;
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

        let by_metadata = if self.graph_store.is_postgres_backend() {
            format!(
                "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND metadata->>'logical_key' = '{}' ORDER BY id DESC LIMIT 1",
                escape_sql(node_type),
                escape_sql(project_code),
                escape_sql(logical_key)
            )
        } else {
            format!(
                "SELECT id FROM soll.Node WHERE type = '{}' AND project_code = '{}' AND metadata LIKE '%\"logical_key\":\"{}\"%' ORDER BY id DESC LIMIT 1",
                escape_sql(node_type),
                escape_sql(project_code),
                escape_sql(logical_key)
            )
        };
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

pub(super) fn parse_numeric_suffix(value: &str) -> Option<u64> {
    let head = value.split(':').next()?.trim();
    head.rsplit('-').next()?.parse::<u64>().ok()
}

pub(super) fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
