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

        // DEC-AXO-091 / REQ-AXO-322 (v2) — entirely snapshot-driven:
        // iterate Requirement nodes from the in-memory snapshot, count
        // traceability rows from the snapshot's pre-built index, and
        // count VERIFIES edges from VAL-{code}-* via the incoming-edge
        // index. The expensive multi-JOIN SQL is gone.
        let snapshot = self.soll_cache().snapshot(&project_code)?;
        let val_prefix = format!("VAL-{}-", project_code);
        let mut summary = RequirementCoverageSummary::default();

        // broken_file_evidence_counts_by_requirement still drives the
        // filesystem freshness sweep (REQ-AXO-320) — keep that SQL path
        // since it owns the stat() + UPDATE flow. Hot-path callers
        // already pay this only once per work_plan invocation (cached
        // upstream by REQ-AXO-319).
        let broken_counts = self.broken_file_evidence_counts_by_requirement(&project_code);

        // Stable iteration order by id so callers comparing snapshots
        // across calls (tests, diff tooling) see deterministic output.
        let mut req_ids: Vec<&String> = snapshot
            .node_ids_of_type("Requirement")
            .iter()
            .collect();
        req_ids.sort();

        for id in req_ids {
            let Some(node) = snapshot.nodes.get(id) else {
                continue;
            };
            let status = node.status.clone();
            let meta: serde_json::Value =
                serde_json::from_str(&node.metadata_raw).unwrap_or(serde_json::json!({}));
            let criteria = meta
                .get("acceptance_criteria")
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";

            let evidence_count = snapshot.traceability_rows_for("requirement", id).count();
            let validation_count =
                snapshot.count_incoming_edges_with(id, "VERIFIES", Some(&val_prefix));
            let broken_file_evidence_count = broken_counts.get(id).copied().unwrap_or(0);

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
                id: id.clone(),
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
        // DEC-AXO-091 / REQ-AXO-322 (v2) — when a project_code is in
        // scope, derive total_nodes and the 4 ID lists (orphan_req,
        // validation_without_verifies, decision_without_links,
        // uncovered_req) from the in-memory snapshot. The UNION ALL
        // round-trip is gone. For workspace-wide calls (no project
        // scope), fall back to SQL because the snapshot is per-project.
        let mut orphan_requirements: Vec<String> = Vec::new();
        let mut validations_without_verifies: Vec<String> = Vec::new();
        let mut decisions_without_links: Vec<String> = Vec::new();
        let mut uncovered_requirements: Vec<String> = Vec::new();

        let total_nodes = if let Some(code) = resolved_project_code.as_deref() {
            let snapshot = self.soll_cache().snapshot(code)?;

            // orphan_requirement: Requirement, status <> archived, no edges.
            for id in snapshot.node_ids_of_type("Requirement") {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                if node.status == "archived" {
                    continue;
                }
                if !snapshot.has_any_edge(id) {
                    orphan_requirements.push(id.clone());
                }
            }

            // validation_without_verifies: Validation with no VERIFIES
            // edge (in either direction).
            for id in snapshot.node_ids_of_type("Validation") {
                let has_verifies = snapshot
                    .outgoing_edges(id)
                    .any(|(_, rel)| rel == "VERIFIES")
                    || snapshot
                        .incoming_edges(id)
                        .any(|(_, rel)| rel == "VERIFIES");
                if !has_verifies {
                    validations_without_verifies.push(id.clone());
                }
            }

            // decision_without_links: Decision with no SOLVES/IMPACTS.
            for id in snapshot.node_ids_of_type("Decision") {
                let has_links = snapshot
                    .outgoing_edges(id)
                    .any(|(_, rel)| matches!(rel, "SOLVES" | "IMPACTS"))
                    || snapshot
                        .incoming_edges(id)
                        .any(|(_, rel)| matches!(rel, "SOLVES" | "IMPACTS"));
                if !has_links {
                    decisions_without_links.push(id.clone());
                }
            }

            // uncovered_requirement: Requirement, status <> archived,
            // no traceability AND no acceptance_criteria. The legacy
            // SQL grouped on metadata; we evaluate the same predicate
            // on the in-memory metadata_raw JSON.
            for id in snapshot.node_ids_of_type("Requirement") {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                if node.status == "archived" {
                    continue;
                }
                if snapshot.traceability_rows_for("requirement", id).next().is_some() {
                    continue;
                }
                let meta: serde_json::Value =
                    serde_json::from_str(&node.metadata_raw).unwrap_or(serde_json::json!({}));
                let criteria = meta
                    .get("acceptance_criteria")
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                let has_criteria = !criteria.trim().is_empty() && criteria.trim() != "[]";
                if !has_criteria {
                    uncovered_requirements.push(id.clone());
                }
            }

            orphan_requirements.sort();
            validations_without_verifies.sort();
            decisions_without_links.sort();
            uncovered_requirements.sort();

            snapshot.nodes.len()
        } else {
            // Workspace-wide (no project_code) — keep SQL since the
            // snapshot is per-project. This branch is rare (only the
            // unscoped public wrapper).
            let total = self
                .graph_store
                .query_count("SELECT count(*) FROM soll.Node")
                .unwrap_or(0) as usize;
            let fused_sql =
                "SELECT 'orphan_requirement' AS category, id FROM soll.Node r \
                 WHERE type = 'Requirement' \
                   AND COALESCE(r.status, '') <> 'archived' \
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE source_id = r.id OR target_id = r.id) \
                 UNION ALL \
                 SELECT 'validation_without_verifies' AS category, id FROM soll.Node v \
                 WHERE type = 'Validation' \
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = v.id OR target_id = v.id) AND relation_type = 'VERIFIES') \
                 UNION ALL \
                 SELECT 'decision_without_links' AS category, id FROM soll.Node d \
                 WHERE type = 'Decision' \
                   AND NOT EXISTS (SELECT 1 FROM soll.Edge WHERE (source_id = d.id OR target_id = d.id) AND relation_type IN ('SOLVES', 'IMPACTS')) \
                 UNION ALL \
                 SELECT 'uncovered_requirement' AS category, r.id FROM soll.Node r \
                 LEFT JOIN soll.Traceability t \
                   ON lower(t.soll_entity_type) = lower(r.type) \
                  AND t.soll_entity_id = r.id \
                 WHERE r.type = 'Requirement' \
                   AND COALESCE(r.status, '') <> 'archived' \
                 GROUP BY r.id, r.status, r.metadata \
                 HAVING COUNT(t.id) = 0 \
                    AND COALESCE(CAST(json_extract(r.metadata, '$.acceptance_criteria') AS VARCHAR), '') IN ('', '[]') \
                 ORDER BY 1, 2";
            let fused_raw = self.graph_store.query_json(fused_sql)?;
            let fused_rows: Vec<Vec<String>> = serde_json::from_str(&fused_raw).unwrap_or_default();
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
            total
        };

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
