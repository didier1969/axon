use super::*;

impl McpServer {
    fn broken_file_evidence_count_for_requirement(&self, requirement_id: &str) -> usize {
        let project_root = self.canonical_project_root_for_entity(requirement_id);
        let query = format!(
            "SELECT COALESCE(artifact_ref, '') FROM soll.Traceability
             WHERE lower(soll_entity_type) = 'requirement'
               AND soll_entity_id = '{}'
               AND lower(artifact_type) IN ('file', 'document')",
            escape_sql(requirement_id)
        );
        let refs = self.query_single_column(&query).unwrap_or_default();
        refs.into_iter()
            .filter(|artifact_ref| {
                let raw = artifact_ref.trim();
                if raw.is_empty() {
                    return false;
                }
                let path = Path::new(raw);
                let candidate = if path.is_absolute() {
                    path.to_path_buf()
                } else if let Some(root) = project_root.as_ref() {
                    root.join(path)
                } else {
                    path.to_path_buf()
                };
                !candidate.exists()
            })
            .count()
    }

    pub(crate) fn requirement_coverage_summary(
        &self,
        project_code: &str,
    ) -> anyhow::Result<RequirementCoverageSummary> {
        let project_code = self.resolve_project_code(project_code)?;
        let query = format!(
            "SELECT r.id,
                    COALESCE(r.status,''),
                    COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), ''),
                    COUNT(DISTINCT t.id),
                    COUNT(DISTINCT CASE WHEN e.relation_type = 'VERIFIES' THEN e.source_id END)
             FROM soll.Node r
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(r.type)
              AND t.soll_entity_id = r.id
             LEFT JOIN soll.Edge e
               ON e.target_id = r.id
              AND e.relation_type = 'VERIFIES'
              AND e.source_id LIKE 'VAL-{}-%'
             WHERE r.type='Requirement' AND r.id LIKE 'REQ-{}-%'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(&project_code),
            escape_sql(&project_code)
        );
        let rows_raw = self.graph_store.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut summary = RequirementCoverageSummary::default();

        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let id = row[0].clone();
            let status = row[1].clone();
            let criteria = row[2].clone();
            let evidence_count = row[3].parse::<usize>().unwrap_or(0);
            let validation_count = row[4].parse::<usize>().unwrap_or(0);
            let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";
            let broken_file_evidence_count = self.broken_file_evidence_count_for_requirement(&id);
            let state = requirement_state_from(
                status.as_str(),
                &criteria,
                evidence_count,
                broken_file_evidence_count,
            );
            let missing_dimensions = requirement_missing_dimensions(
                status.as_str(),
                has_criteria,
                evidence_count,
                validation_count,
                broken_file_evidence_count,
            );
            let suggested_next_actions = requirement_next_actions(&missing_dimensions);

            match state {
                "done" => summary.done += 1,
                "partial" => summary.partial += 1,
                _ => summary.missing += 1,
            }

            summary.entries.push(RequirementCoverageEntry {
                id,
                status,
                evidence_count,
                validation_count,
                has_criteria,
                broken_file_evidence_count,
                state: state.to_string(),
                missing_dimensions,
                suggested_next_actions,
            });
        }

        Ok(summary)
    }

    pub(crate) fn soll_completeness_snapshot(
        &self,
        project_code: Option<&str>,
    ) -> anyhow::Result<SollCompletenessSnapshot> {
        let resolved_project_code = match project_code {
            Some(code) => Some(self.resolve_project_code(code)?),
            None => None,
        };
        let project_scope = resolved_project_code
            .clone()
            .map(|code| format!("project:{code}"))
            .unwrap_or_else(|| "workspace:*".to_string());
        let project_scope_predicate = |id_column: &str, project_code: Option<&str>| {
            project_code
                .map(|code| format!("AND {id_column} LIKE '%-{}-%'", escape_sql(code)))
                .unwrap_or_default()
        };

        let total_nodes = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node n WHERE 1=1 {}",
                resolved_project_code
                    .as_deref()
                    .map(|code| format!("AND n.project_code = '{}'", escape_sql(code)))
                    .unwrap_or_default()
            ))
            .unwrap_or(0) as usize;

        let orphan_requirements = self.query_single_column(&format!(
            "SELECT id FROM soll.Node r
             WHERE type = 'Requirement'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE source_id = r.id OR target_id = r.id)
               {}
             ORDER BY id",
            project_scope_predicate("r.id", resolved_project_code.as_deref())
        ))?;

        let validations_without_verifies = self.query_single_column(&format!(
            "SELECT id FROM soll.Node v
             WHERE type = 'Validation'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = v.id OR target_id = v.id) AND relation_type = 'VERIFIES')
               {}
             ORDER BY id",
            project_scope_predicate("v.id", resolved_project_code.as_deref())
        ))?;

        let decisions_without_links = self.query_single_column(&format!(
            "SELECT id FROM soll.Node d
             WHERE type = 'Decision'
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = d.id OR target_id = d.id) AND relation_type IN ('SOLVES', 'IMPACTS'))
               {}
             ORDER BY id",
            project_scope_predicate("d.id", resolved_project_code.as_deref())
        ))?;

        let uncovered_requirements = self.query_single_column(&format!(
            "SELECT r.id FROM soll.Node r
             LEFT JOIN soll.Traceability t
               ON lower(t.soll_entity_type) = lower(r.type)
              AND t.soll_entity_id = r.id
             WHERE r.type = 'Requirement'
               {}
             GROUP BY r.id, r.status, r.metadata
             HAVING COUNT(t.id) = 0
                AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') IN ('', '[]')
             ORDER BY r.id",
            project_scope_predicate("r.id", resolved_project_code.as_deref())
        ))?;

        let duplicate_title_rows_raw = self.graph_store.query_json(&format!(
            "SELECT type, title, string_agg(id, ', ' ORDER BY id)
             FROM soll.Node
             WHERE type IN ('Requirement', 'Decision', 'Concept')
               AND COALESCE(title, '') <> ''
               {}
             GROUP BY type, title
             HAVING COUNT(*) > 1
             ORDER BY type, title",
            resolved_project_code
                .as_deref()
                .map(|code| format!("AND project_code = '{}'", escape_sql(code)))
                .unwrap_or_default()
        ))?;
        let duplicate_title_rows: Vec<Vec<String>> =
            serde_json::from_str(&duplicate_title_rows_raw).unwrap_or_default();

        let duplicate_ids = duplicate_title_rows
            .iter()
            .filter_map(|row| row.get(2).cloned())
            .flat_map(|ids| {
                ids.split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let relation_policy_violations =
            self.collect_relation_policy_violations(resolved_project_code.as_deref())?;
        let requirement_coverage = match resolved_project_code.as_deref() {
            Some(code) => self.requirement_coverage_summary(code)?,
            None => RequirementCoverageSummary::default(),
        };

        Ok(SollCompletenessSnapshot {
            project_scope,
            total_nodes,
            orphan_requirements,
            validations_without_verifies,
            decisions_without_links,
            uncovered_requirements,
            duplicate_title_rows,
            duplicate_ids,
            relation_policy_violations,
            requirement_coverage,
        })
    }
}
