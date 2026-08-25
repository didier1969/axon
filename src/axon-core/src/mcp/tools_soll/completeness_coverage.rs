use super::*;

/// Verdict of one `file`/`document` artifact ref against the project root.
///
/// REQ-AXO-902436 — FOUR states, not three. `unresolved_root` is the one that
/// was missing, and its absence is what made the sweep destructive: when the
/// root could not be resolved, a RELATIVE ref was stat()ed against whatever
/// directory the brain happened to run in, missed, and was reported `broken`.
/// `soll_remove_evidence(broken_only=true)` then deletes it. An unmeasurable
/// ref is not a broken ref — it is a ref the tool could not judge, and saying
/// so is the whole point (PIL-AXO-002: no verdict on an unmeasured quantity).
///
/// An ABSOLUTE ref stays judgeable with no root at all, so it keeps its
/// `broken` verdict — the root only ever mattered for relative refs.
pub(crate) fn classify_evidence_ref_against_root(
    raw_ref: &str,
    project_root: Option<&Path>,
) -> &'static str {
    // REQ-AXO-902457 — `fichier:45-54` désigne un emplacement DANS un fichier ;
    // c'est le fichier qu'il faut stat(), pas la chaîne entière.
    let path = Path::new(super::shared::evidence_path_without_line_anchor(raw_ref));
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match project_root {
            Some(root) => root.join(path),
            None => return "unresolved_root",
        }
    };
    match std::fs::symlink_metadata(&candidate) {
        Ok(meta) if meta.is_dir() => "directory",
        Ok(_) => "present",
        Err(_) => "broken",
    }
}

impl McpServer {
    /// Batched broken-file-evidence map keyed by requirement_id, carrying the
    /// NAMED offenders (traceability id + artifact path), not just a count.
    ///
    /// REQ-AXO-320 — Reads from `soll.Traceability.artifact_status` (sweeper
    /// column) instead of `Path::exists()` syscalls in app code. Lazy
    /// refresh: artifacts with NULL status or `artifact_checked_at` older
    /// than `BROKEN_FILE_TTL` are re-checked in a batch (single stat() per
    /// unique path) and persisted via one UPDATE. Subsequent calls within
    /// the TTL window are pure SQL and read from index
    /// `soll_traceability_status_idx`.
    ///
    /// REQ-AXO-902337 piste 1 — the sweep already knows each broken path;
    /// return the offenders so `soll_verify_requirements` can name them
    /// instead of forcing raw SQL on `soll.Traceability`. Requirements with
    /// no broken reference are absent from the map (callers use
    /// `.get(id)` with a `unwrap_or_default`).
    /// Returns `(offenders_by_requirement, unresolvable_ref_count)`. The second
    /// member is REQ-AXO-902436: refs the sweep could not judge at all. It is
    /// returned rather than folded into the first, because "I could not
    /// measure this" and "this is broken" must never share a bucket.
    fn broken_file_evidence_by_requirement(
        &self,
        project_code: &str,
    ) -> (HashMap<String, Vec<BrokenFileEvidence>>, usize) {
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
            Err(_) => return (HashMap::new(), 0),
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
            // REQ-AXO-902390 — the SQL filter above trusts `artifact_type`, and
            // `Document` is where every unrecognised ref used to land. Measured on
            // axon_live: 493 commit hashes + 113 SOLL ids typed `Document`, all
            // stat()ed as paths and reported as missing files. A ref whose SHAPE is
            // not a path is a TYPING defect, never a broken file.
            if !evidence_ref_is_disk_checkable("file", &artifact_ref) {
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
            // REQ-AXO-902436 — resolve the root through BOTH sources, not just
            // `.axon/meta.json` on disk. Measured on axon_live: the disk scan
            // knows 13 project roots, `soll.ProjectCodeRegistry` knows 75, and
            // the 62 it misses had EVERY relative artifact_ref stat()ed against
            // the brain's own cwd — 126 of 156 relative refs marked `broken`
            // while present under their real root (TE2 78/82, OPV 47/47).
            // `resolve_project_identity` is the composed resolver that already
            // existed in project_meta.rs (disk first, registry fallback); this
            // sweep was calling the disk-only half.
            let project_root = resolve_project_identity(&self.graph_store, project_code)
                .ok()
                .map(|identity| identity.project_path);
            // One stat() per unique stale path.
            let mut fresh: HashMap<String, &'static str> = HashMap::with_capacity(stale_refs.len());
            for raw_ref in &stale_refs {
                fresh.insert(
                    raw_ref.clone(),
                    classify_evidence_ref_against_root(raw_ref, project_root.as_deref()),
                );
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

        // Phase 3: fold the NAMED broken offenders per requirement using the
        // freshest status. Requirements with only healthy evidence never
        // enter the map (REQ-AXO-902337 contract) — callers unwrap_or_default.
        let mut by_req: HashMap<String, Vec<BrokenFileEvidence>> = HashMap::new();
        let mut unresolvable = 0usize;
        for row in &all_rows {
            let effective_status: &str = if row.stale {
                fresh_status
                    .get(&row.artifact_ref)
                    .copied()
                    .unwrap_or("unknown")
            } else {
                row.status.as_str()
            };
            match effective_status {
                "broken" => {
                    by_req
                        .entry(row.req_id.clone())
                        .or_default()
                        .push(BrokenFileEvidence {
                            traceability_id: row.traceability_id.clone(),
                            artifact_ref: row.artifact_ref.clone(),
                        });
                }
                "unresolved_root" => unresolvable += 1,
                _ => {}
            }
        }
        (by_req, unresolvable)
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

        // broken_file_evidence_by_requirement still drives the filesystem
        // freshness sweep (REQ-AXO-320) — keep that SQL path since it owns
        // the stat() + UPDATE flow. Hot-path callers already pay this only
        // once per work_plan invocation (cached upstream by REQ-AXO-319).
        // REQ-AXO-902337 — it now returns the named offenders per requirement.
        let (broken_by_req, unresolvable_file_evidence) =
            self.broken_file_evidence_by_requirement(&project_code);

        // Stable iteration order by id so callers comparing snapshots
        // across calls (tests, diff tooling) see deterministic output.
        let mut req_ids: Vec<&String> = snapshot.node_ids_of_type("Requirement").iter().collect();
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
            let broken_file_evidence = broken_by_req.get(id).cloned().unwrap_or_default();
            let broken_file_evidence_count = broken_file_evidence.len();

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

            // REQ-AXO-902501 — un critère volontairement non tenu doit être LISIBLE
            // par la machine. VPC l'avait écrit trois fois en prose faute de champ.
            let criteres_resume = resume_criteres(&parse_acceptance_criteria(&criteria));

            summary.entries.push(RequirementCoverageEntry {
                id: id.clone(),
                status,
                evidence_count,
                validation_count,
                has_criteria,
                criteres_resume,
                broken_file_evidence_count,
                broken_file_evidence,
                state: state.to_string(),
                missing_dimensions,
                suggested_next_actions,
            });
        }

        summary.unresolvable_file_evidence_count = unresolvable_file_evidence;
        Ok(summary)
    }

    pub(crate) fn soll_completeness_snapshot(
        &self,
        project_code: Option<&str>,
    ) -> anyhow::Result<SollCompletenessSnapshot> {
        self.soll_completeness_snapshot_filtered(project_code, None, None)
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
        self.soll_completeness_snapshot_filtered(project_code, None, cached_coverage)
    }

    /// REQ-AXO-901602 — filtered variant supporting `statuses_to_check`.
    ///
    /// When `statuses_to_check` is `None`, behaviour matches the original
    /// snapshot (only `archived` excluded for orphan/uncovered checks ; no
    /// status filter on decisions/duplicate-titles). This preserves
    /// backward compatibility for the dozens of existing callers (anomalies,
    /// inference/mutation before/after deltas, work_plan, manager, etc.).
    ///
    /// When `Some(&statuses)` is provided, every status-scoped check
    /// (orphan_requirements, decisions_without_links, uncovered_requirements,
    /// duplicate_title_rows) only considers nodes whose `status` is in the
    /// allow-list. The sentinel value `"*"` inside the list disables the
    /// filter entirely (back-compat full sweep).
    ///
    /// `axon_validate_soll` opts in with `["current","planned"]` by default,
    /// suppressing the 75+ terminal-status false positives observed on AXO
    /// post-session-46 (cf. CPT-AXO-052 session 47 audit).
    pub(crate) fn soll_completeness_snapshot_filtered(
        &self,
        project_code: Option<&str>,
        statuses_to_check: Option<&[String]>,
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

        // REQ-AXO-901602 — closure used by every status-scoped check below.
        // Returns true if the node should be included in the coherence audit.
        // - filter=None  → legacy behaviour (only `archived` excluded for
        //   orphan/uncovered, no filter on decisions/duplicates).
        // - filter=Some(["*"]) → no filter (back-compat full sweep).
        // - filter=Some(list) → only nodes whose `status` is in the list.
        let status_allowed = |status: &str, legacy_archived_only: bool| -> bool {
            match statuses_to_check {
                None if legacy_archived_only => status != "archived",
                None => true,
                Some(allowed) if allowed.iter().any(|s| s == "*") => true,
                Some(allowed) => allowed.iter().any(|s| s == status),
            }
        };

        // DEC-AXO-091 / REQ-AXO-322 (v2) — when a project_code is in
        // scope, derive total_nodes and the 4 ID lists (orphan_req,
        // validation_without_verifies, decision_without_links,
        // uncovered_req) from the in-memory snapshot. The UNION ALL
        // round-trip is gone. For workspace-wide calls (no project
        // scope), fall back to SQL because the snapshot is per-project.
        // REQ-AXO-902455 — `orphan_requirements`, `validations_without_verifies`
        // et `decisions_without_links` ne sont plus calculés ici : ce sont les
        // règles-données `GUI-PRO-127/128/129`, évaluées par le moteur
        // déclaratif. Équivalence prouvée sur le même fixture avant retrait par
        // `the_three_attachment_rules_match_the_hardcoded_checks_before_they_are_removed`.
        //
        // `uncovered_requirements` reste : c'est une CONJONCTION — ni preuve NI
        // critère — que `parse_soll_rule` refuse par construction.
        let mut uncovered_requirements: Vec<String> = Vec::new();

        let total_nodes = if let Some(code) = resolved_project_code.as_deref() {
            let snapshot = self.soll_cache().snapshot(code)?;

            // uncovered_requirement: Requirement, status filter,
            // no traceability AND no acceptance_criteria. The legacy
            // SQL grouped on metadata; we evaluate the same predicate
            // on the in-memory metadata_raw JSON.
            for id in snapshot.node_ids_of_type("Requirement") {
                let Some(node) = snapshot.nodes.get(id) else {
                    continue;
                };
                if !status_allowed(&node.status, true) {
                    continue;
                }
                if snapshot
                    .traceability_rows_for("requirement", id)
                    .next()
                    .is_some()
                {
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

            uncovered_requirements.sort();

            snapshot.nodes.len()
        } else {
            // Workspace-wide (no project_code) — keep SQL since the
            // snapshot is per-project. This branch is rare (only the
            // unscoped public wrapper).
            //
            // REQ-AXO-901602 — derive a SQL status filter from the
            // optional `statuses_to_check` list. Legacy default (None)
            // keeps the original `<> 'archived'` semantics for
            // orphan/uncovered; no filter on decision/validation.
            // Opt-in (Some) applies a positive IN-list to all of
            // orphan/decision/uncovered (validations excluded — VAL
            // nodes are proofs, not subject to status hygiene).
            let req_status_sql = match statuses_to_check {
                None => "COALESCE(r.status, '') <> 'archived'".to_string(),
                Some(allowed) if allowed.iter().any(|s| s == "*") => "1=1".to_string(),
                Some(allowed) => {
                    let parts: Vec<String> = allowed
                        .iter()
                        .map(|s| format!("'{}'", escape_sql(s)))
                        .collect();
                    format!("COALESCE(r.status, '') IN ({})", parts.join(", "))
                }
            };
            let total = self
                .graph_store
                .query_count("SELECT count(*) FROM soll.Node")
                .unwrap_or(0) as usize;
            // REQ-AXO-902455 — les trois catégories de rattachement ont migré
            // vers `GUI-PRO-127/128/129`. Il ne reste que la conjonction que le
            // moteur déclaratif refuse par construction.
            let fused_sql = format!(
                "SELECT 'uncovered_requirement' AS category, r.id FROM soll.Node r \
                 LEFT JOIN soll.Traceability t \
                   ON lower(t.soll_entity_type) = lower(r.type) \
                  AND t.soll_entity_id = r.id \
                 WHERE r.type = 'Requirement' \
                   AND {req_status_sql} \
                 GROUP BY r.id, r.status, r.metadata \
                 HAVING COUNT(t.id) = 0 \
                    AND COALESCE((r.metadata->>'acceptance_criteria'), '') IN ('', '[]') \
                 ORDER BY 1, 2"
            );
            let fused_raw = self.graph_store.query_json(&fused_sql)?;
            let fused_rows: Vec<Vec<String>> = serde_json::from_str(&fused_raw).unwrap_or_default();
            for row in fused_rows {
                if row.len() < 2 {
                    continue;
                }
                let id = row[1].clone();
                if row[0] == "uncovered_requirement" {
                    uncovered_requirements.push(id);
                }
            }
            total
        };

        // REQ-AXO-902455 — la détection de titres dupliqués n'est plus ici :
        // c'est la règle-donnée `GUI-PRO-121`, évaluée par le moteur déclaratif
        // sous `collect_relation_policy_violations`. Le SQL retiré ne couvrait
        // que Requirement/Decision/Concept — aveugle aux Skill/PromptTemplate,
        // où vivent les 41 doublons de `PRO` (résidus de tests dans le
        // namespace produit). Équivalence prouvée avant retrait par
        // `the_declarative_title_rule_covers_everything_the_hardcoded_check_did_and_more`.
        let (relation_policy_violations, declarative_rule_violations) =
            self.collect_relation_policy_violations(resolved_project_code.as_deref())?;
        let requirement_coverage = match (resolved_project_code.as_deref(), cached_coverage) {
            (Some(_), Some(cached)) => cached.clone(),
            (Some(code), None) => self.requirement_coverage_summary(code)?,
            (None, _) => RequirementCoverageSummary::default(),
        };

        Ok(SollCompletenessSnapshot {
            project_scope,
            total_nodes,
            uncovered_requirements,
            relation_policy_violations,
            declarative_rule_violations,
            requirement_coverage,
        })
    }
}
