use super::*;

impl McpServer {
    /// Batched broken-file-evidence count map keyed by requirement_id.
    ///
    /// REQ-AXO-320 — Reads from `soll.Traceability.artifact_status` (sweeper
    /// column) instead of `Path::exists()` syscalls in app code. Lazy
    /// refresh: artifacts with NULL status or `artifact_checked_at` older
    /// than `BROKEN_FILE_TTL` are re-checked in a batch (single stat() per
    /// unique path) and persisted via one UPDATE. Subsequent calls within
    /// the TTL window are pure SQL and read from index
    /// `soll_traceability_status_idx`.
    fn broken_file_evidence_counts_by_requirement(
        &self,
        project_code: &str,
    ) -> HashMap<String, usize> {
        // 5-min TTL: balances staleness (artifacts referenced from SOLL rarely
        // disappear between minutes) against refresh cost (single batched
        // sweep per window).
        const BROKEN_FILE_TTL_SECS: i64 = 300;

        let query = format!(
            "SELECT id, soll_entity_id, COALESCE(artifact_ref, ''), \
                    COALESCE(artifact_status, ''), \
                    COALESCE(EXTRACT(EPOCH FROM artifact_checked_at)::BIGINT, 0) \
             FROM soll.Traceability \
             WHERE lower(soll_entity_type) = 'requirement' \
               AND soll_entity_id LIKE 'REQ-{}-%' \
               AND lower(artifact_type) IN ('file', 'document')",
            escape_sql(project_code)
        );
        let raw = match self.graph_store.query_json(&query) {
            Ok(s) => s,
            Err(_) => return HashMap::new(),
        };
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Phase 1: collect rows + identify stale ones needing refresh.
        struct Row {
            traceability_id: String,
            req_id: String,
            artifact_ref: String,
            status: String,
            stale: bool,
        }
        let mut all_rows: Vec<Row> = Vec::with_capacity(rows.len());
        let mut stale_refs: HashSet<String> = HashSet::new();
        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let artifact_ref = row[2].trim().to_string();
            if artifact_ref.is_empty() {
                continue;
            }
            let status = row[3].clone();
            let checked_at = row[4].parse::<i64>().unwrap_or(0);
            let stale = status.is_empty() || (now_secs - checked_at) > BROKEN_FILE_TTL_SECS;
            if stale {
                stale_refs.insert(artifact_ref.clone());
            }
            all_rows.push(Row {
                traceability_id: row[0].clone(),
                req_id: row[1].clone(),
                artifact_ref,
                status,
                stale,
            });
        }

        // Phase 2: refresh stale entries via one batched stat+UPDATE.
        let fresh_status: HashMap<String, &'static str> = if stale_refs.is_empty() {
            HashMap::new()
        } else {
            let project_root = resolve_canonical_project_identity(project_code)
                .ok()
                .map(|identity| identity.project_path);
            // One stat() per unique stale path.
            let mut fresh: HashMap<String, &'static str> =
                HashMap::with_capacity(stale_refs.len());
            for raw_ref in &stale_refs {
                let path = Path::new(raw_ref);
                let candidate = if path.is_absolute() {
                    path.to_path_buf()
                } else if let Some(root) = project_root.as_ref() {
                    root.join(path)
                } else {
                    path.to_path_buf()
                };
                let status: &'static str = match std::fs::symlink_metadata(&candidate) {
                    Ok(meta) if meta.is_dir() => "directory",
                    Ok(_) => "present",
                    Err(_) => "broken",
                };
                fresh.insert(raw_ref.clone(), status);
            }
            // Batch UPDATE via VALUES list (one round-trip).
            let mut values: Vec<String> = Vec::new();
            for row in &all_rows {
                if row.stale {
                    if let Some(&status) = fresh.get(&row.artifact_ref) {
                        values.push(format!(
                            "('{}', '{}')",
                            escape_sql(&row.traceability_id),
                            escape_sql(status)
                        ));
                    }
                }
            }
            if !values.is_empty() {
                let sql = format!(
                    "UPDATE soll.Traceability AS t \
                     SET artifact_status = v.status, \
                         artifact_checked_at = to_timestamp({}) \
                     FROM (VALUES {}) AS v(id, status) \
                     WHERE t.id = v.id",
                    now_secs,
                    values.join(", ")
                );
                // best-effort: swallow errors so a write failure doesn't kill
                // the whole coverage computation.
                let _ = self.graph_store.execute_param(&sql, &serde_json::json!([]));
            }
            fresh
        };

        // Phase 3: fold per-requirement broken counts using freshest status.
        let mut by_req: HashMap<String, usize> = HashMap::new();
        for row in &all_rows {
            let effective_status: &str = if row.stale {
                fresh_status
                    .get(&row.artifact_ref)
                    .copied()
                    .unwrap_or("unknown")
            } else {
                row.status.as_str()
            };
            if effective_status == "broken" {
                *by_req.entry(row.req_id.clone()).or_insert(0) += 1;
            } else {
                by_req.entry(row.req_id.clone()).or_insert(0);
            }
        }
        by_req
    }

    pub(crate) fn requirement_coverage_summary(
        &self,
        project_code: &str,
    ) -> anyhow::Result<RequirementCoverageSummary> {
        let project_code = self.resolve_project_code(project_code)?;
        // Use the (project_code, type) composite index instead of `id LIKE
        // 'REQ-{code}-%'` which forces a sequential scan on soll.Node — same
        // selectivity, indexable. The same swap on soll.Edge would require a
        // project_code column on Edge (it exists; see soll_edge_project_*_idx)
        // but we keep the LIKE on e.source_id since the prefix is selective
        // enough and the pkey already covers (source_id, target_id, relation_type).
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
             WHERE r.type='Requirement' AND r.project_code='{}'
             GROUP BY 1,2,3
             ORDER BY r.id",
            escape_sql(&project_code),
            escape_sql(&project_code)
        );
        let rows_raw = self.graph_store.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&rows_raw).unwrap_or_default();
        let mut summary = RequirementCoverageSummary::default();

        // Single batched query for ALL broken-file-evidence counts in this project,
        // replacing the per-requirement N+1 (was 328 SQL round-trips + 328 stat()
        // syscalls per call to `requirement_coverage_summary`, and this function
        // is invoked 3-4× per `soll_work_plan` invocation).
        let broken_counts = self.broken_file_evidence_counts_by_requirement(&project_code);

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
            let broken_file_evidence_count = broken_counts.get(&id).copied().unwrap_or(0);
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
        self.soll_completeness_snapshot_with_cached_coverage(project_code, None)
    }

    /// Memoized variant: when the caller has already computed
    /// `requirement_coverage_summary` for this project, pass it via
    /// `cached_coverage` to skip the redundant heavy recomputation.
    /// `axon_soll_work_plan` calls this with Some(&coverage) — the public
    /// wrapper above keeps the original semantics with None.
    pub(crate) fn soll_completeness_snapshot_with_cached_coverage(
        &self,
        project_code: Option<&str>,
        cached_coverage: Option<&RequirementCoverageSummary>,
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
                "SELECT count(*) FROM soll.Node n WHERE 1=1{}",
                scoped_query_filter(resolved_project_code.as_deref(), "n.")
            ))
            .unwrap_or(0) as usize;

        // REQ-AXO-319 Phase 3 — fuse the 4 independent ID-list queries
        // (orphan_requirements, validations_without_verifies,
        // decisions_without_links, uncovered_requirements) into ONE
        // UNION ALL round-trip with a `category` tag. PostgreSQL plans
        // each branch independently and the result is partitioned in
        // Rust. 4 round-trips → 1.
        let r_scope = project_scope_predicate("r.id", resolved_project_code.as_deref());
        let v_scope = project_scope_predicate("v.id", resolved_project_code.as_deref());
        let d_scope = project_scope_predicate("d.id", resolved_project_code.as_deref());
        let fused_sql = format!(
            "SELECT 'orphan_requirement' AS category, id FROM soll.Node r \
             WHERE type = 'Requirement' \
               AND COALESCE(r.status, '') <> 'archived' \
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE source_id = r.id OR target_id = r.id) \
               {r_scope1} \
             UNION ALL \
             SELECT 'validation_without_verifies' AS category, id FROM soll.Node v \
             WHERE type = 'Validation' \
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = v.id OR target_id = v.id) AND relation_type = 'VERIFIES') \
               {v_scope} \
             UNION ALL \
             SELECT 'decision_without_links' AS category, id FROM soll.Node d \
             WHERE type = 'Decision' \
               AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = d.id OR target_id = d.id) AND relation_type IN ('SOLVES', 'IMPACTS')) \
               {d_scope} \
             UNION ALL \
             SELECT 'uncovered_requirement' AS category, r.id FROM soll.Node r \
             LEFT JOIN soll.Traceability t \
               ON lower(t.soll_entity_type) = lower(r.type) \
              AND t.soll_entity_id = r.id \
             WHERE r.type = 'Requirement' \
               AND COALESCE(r.status, '') <> 'archived' \
               {r_scope2} \
             GROUP BY r.id, r.status, r.metadata \
             HAVING COUNT(t.id) = 0 \
                AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') IN ('', '[]') \
             ORDER BY 1, 2",
            r_scope1 = r_scope,
            v_scope = v_scope,
            d_scope = d_scope,
            r_scope2 = r_scope,
        );
        let fused_raw = self.graph_store.query_json(&fused_sql)?;
        let fused_rows: Vec<Vec<String>> = serde_json::from_str(&fused_raw).unwrap_or_default();
        let mut orphan_requirements: Vec<String> = Vec::new();
        let mut validations_without_verifies: Vec<String> = Vec::new();
        let mut decisions_without_links: Vec<String> = Vec::new();
        let mut uncovered_requirements: Vec<String> = Vec::new();
        for row in fused_rows {
            if row.len() < 2 {
                continue;
            }
            let id = row[1].clone();
            match row[0].as_str() {
                "orphan_requirement" => orphan_requirements.push(id),
                "validation_without_verifies" => validations_without_verifies.push(id),
                "decision_without_links" => decisions_without_links.push(id),
                "uncovered_requirement" => uncovered_requirements.push(id),
                _ => {}
            }
        }

        let duplicate_title_rows_raw = self.graph_store.query_json(&format!(
            "SELECT type, title, string_agg(id, ', ' ORDER BY id)
             FROM soll.Node
             WHERE type IN ('Requirement', 'Decision', 'Concept')
               AND COALESCE(title, '') <> ''{}
             GROUP BY type, title
             HAVING COUNT(*) > 1
             ORDER BY type, title",
            scoped_query_filter(resolved_project_code.as_deref(), "")
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
        let requirement_coverage = match (resolved_project_code.as_deref(), cached_coverage) {
            (Some(_), Some(cached)) => cached.clone(),
            (Some(code), None) => self.requirement_coverage_summary(code)?,
            (None, _) => RequirementCoverageSummary::default(),
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
