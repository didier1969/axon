// Tech-debt tracking — TechnologyMigration HAS_REMNANT cross-graph edge +
// queryable inventory + pre-flight residue lookup.
//
// REQ-AXO-901727 (Option A) umbrella; concept docs/concepts/soll-tech-debt-
// tracking-evolution.md. N1 (entity) landed in 8c1ae70a. This module ships:
//   - N2 (REQ-AXO-902030): `link_has_remnant` — the ONLY SOLL→IST edge. A
//     TechnologyMigration (soll.Node, TMG-…) points at IST artifacts
//     (symbol / indexed_file / chunk) that are leftover residue of the
//     migration. Stored in soll.Edge with a `target_kind` discriminator in
//     metadata (concept §4.3) so no new graph table is needed.
//   - N3 (REQ-AXO-902031): `axon_tech_debt_inventory` — queryable inventory of
//     migrations + their remnants + progression. p95 < 100 ms (AC7).
//   - N4 helper (REQ-AXO-902032): `migrations_with_remnant_path` — pre-flight /
//     sub-agent residue lookup for a set of edited paths.
use super::*;
use super::storage::escape_sql;

/// IST target-kind discriminators for HAS_REMNANT cross-graph edges.
/// Tuple: (`target_kind` label stored in edge metadata, IST table, PK column).
pub(super) const REMNANT_KINDS: &[(&str, &str, &str)] = &[
    ("ist:symbol", "ist.Symbol", "id"),
    ("ist:indexed_file", "ist.IndexedFile", "path"),
    ("ist:chunk", "ist.Chunk", "id"),
];

impl McpServer {
    /// REQ-AXO-902030 (N2) — create a HAS_REMNANT cross-graph edge from a
    /// TechnologyMigration (soll.Node, TMG-…) to an IST artifact.
    ///
    /// Bypasses the SOLL↔SOLL endpoint classifier + cycle guard on purpose: a
    /// cross-graph edge can never close a SOLL cycle, and the target is not a
    /// SOLL node so `classify_existing_link_endpoint` would reject it. The
    /// `target_kind` discriminator is auto-detected (probe the 3 IST tables)
    /// unless the caller passes an explicit hint, which is then validated.
    pub(crate) fn link_has_remnant(
        &self,
        source_id: &str,
        target_id: &str,
        target_kind_hint: Option<&str>,
    ) -> Option<Value> {
        // 1. Source must be an existing TechnologyMigration node.
        let src_is_migration = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE id = '{}' AND type = 'TechnologyMigration'",
                escape_sql(source_id)
            ))
            .unwrap_or(0);
        if src_is_migration == 0 {
            return Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "HAS_REMNANT source `{}` is not a TechnologyMigration node. The source of a remnant edge must be a TMG-… node (create one via `soll_manager action=create entity=technology_migration`).",
                    source_id
                )}],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "tool": "soll_manager",
                        "category": "source_not_a_migration",
                        "invalid_field": "data.source_id",
                        "source_id": source_id,
                        "follow_up_tools": ["sql", "soll_query_context"],
                        "hint": "HAS_REMNANT edges originate from a TechnologyMigration (TMG-…). Verify via `sql SELECT id FROM soll.Node WHERE type='TechnologyMigration'`."
                    }
                }
            }));
        }

        // 2. Resolve target_kind: trust an explicit valid hint whose target
        //    actually exists; otherwise auto-detect by probing IST tables.
        let resolved_kind: Option<&'static str> = match target_kind_hint {
            Some(hint) => {
                let Some(entry) = REMNANT_KINDS.iter().find(|(label, _, _)| *label == hint)
                else {
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "Unknown `target_kind` `{}`. Allowed: ist:symbol, ist:indexed_file, ist:chunk (omit to auto-detect).",
                            hint
                        )}],
                        "isError": true,
                        "data": {
                            "status": "input_invalid",
                            "parameter_repair": {
                                "tool": "soll_manager",
                                "category": "unknown_target_kind",
                                "invalid_field": "data.target_kind",
                                "allowed": ["ist:symbol", "ist:indexed_file", "ist:chunk"],
                                "hint": "omit data.target_kind to auto-detect from the IST tables, or set it to one of the allowed discriminators"
                            }
                        }
                    }));
                };
                let (label, table, pk) = *entry;
                let exists = self
                    .graph_store
                    .query_count(&format!(
                        "SELECT count(*) FROM {} WHERE {} = '{}'",
                        table,
                        pk,
                        escape_sql(target_id)
                    ))
                    .unwrap_or(0);
                if exists > 0 {
                    Some(label)
                } else {
                    None
                }
            }
            None => REMNANT_KINDS.iter().find_map(|(label, table, pk)| {
                let exists = self
                    .graph_store
                    .query_count(&format!(
                        "SELECT count(*) FROM {} WHERE {} = '{}'",
                        table,
                        pk,
                        escape_sql(target_id)
                    ))
                    .unwrap_or(0);
                if exists > 0 {
                    Some(*label)
                } else {
                    None
                }
            }),
        };

        let Some(kind) = resolved_kind else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!(
                    "HAS_REMNANT target `{}` was not found in the IST (no matching ist.Symbol.id, ist.IndexedFile.path, or ist.Chunk.id). The target of a remnant edge must be a real indexed artifact.",
                    target_id
                )}],
                "isError": true,
                "data": {
                    "status": "input_not_found",
                    "parameter_repair": {
                        "tool": "soll_manager",
                        "category": "target_not_in_ist",
                        "invalid_field": "data.target_id",
                        "target_id": target_id,
                        "follow_up_tools": ["query", "sql"],
                        "hint": "resolve the IST id first via `query symbol=<name>` (symbol id) or `sql SELECT path FROM ist.IndexedFile WHERE path LIKE '%<frag>%'` (file path)"
                    }
                }
            }));
        };

        // 3. Idempotent insert into soll.Edge with the target_kind discriminator.
        let already = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = 'HAS_REMNANT'",
                escape_sql(source_id),
                escape_sql(target_id)
            ))
            .unwrap_or(0);

        let project_code = super::shared::project_code_from_canonical_entity_id(source_id)
            .unwrap_or_else(|| "AXO".to_string());
        let metadata = json!({ "target_kind": kind }).to_string();

        if already == 0 {
            if let Err(e) = self.graph_store.execute_param(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code) \
                 VALUES (?, ?, 'HAS_REMNANT', ?::jsonb, ?) ON CONFLICT DO NOTHING",
                &json!([source_id, target_id, metadata, project_code]),
            ) {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("HAS_REMNANT insert failed: {}", e) }],
                    "isError": true,
                    "data": {
                        "status": "internal_error",
                        "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                    }
                }));
            }
        }

        Some(json!({
            "content": [{ "type": "text", "text": if already == 0 {
                format!("Remnant edge created: `{}` -HAS_REMNANT-> `{}` ({})", source_id, target_id, kind)
            } else {
                format!("Remnant edge already present: `{}` -HAS_REMNANT-> `{}` ({})", source_id, target_id, kind)
            }}],
            "data": {
                "status": "ok",
                "source_id": source_id,
                "target_id": target_id,
                "relation_type": "HAS_REMNANT",
                "target_kind": kind,
                "edges_created": if already == 0 { 1 } else { 0 },
                "project_code": project_code
            }
        }))
    }

    /// REQ-AXO-902031 (N3) — `tech_debt_inventory` MCP tool.
    ///
    /// Lists TechnologyMigration nodes + their HAS_REMNANT remnants + progress.
    /// Filters: `migration_id`, `from_tech`/`to_tech` (matched against metadata),
    /// `status` (all|active|complete|…), `group_by` (file|symbol|chunk).
    pub(crate) fn axon_tech_debt_inventory(&self, args: &Value) -> Option<Value> {
        let project_code = args
            .get("project_code")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("AXO");
        let migration_id = args
            .get("migration_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let from_tech = args
            .get("from_tech")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let to_tech = args
            .get("to_tech")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let status_filter = args
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "all");
        let group_by = args
            .get("group_by")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());

        // 1. Fetch migration nodes (optionally scoped to one id).
        let node_sql = if let Some(id) = migration_id {
            format!(
                "SELECT id, title, status, metadata FROM soll.Node WHERE type = 'TechnologyMigration' AND id = '{}'",
                escape_sql(id)
            )
        } else {
            format!(
                "SELECT id, title, status, metadata FROM soll.Node WHERE type = 'TechnologyMigration' AND project_code = '{}' ORDER BY id",
                escape_sql(project_code)
            )
        };
        let raw = match self.graph_store.query_json(&node_sql) {
            Ok(r) => r,
            Err(e) => {
                return Some(json!({
                    "content": [{ "type": "text", "text": format!("tech_debt_inventory: migration query failed: {}", e) }],
                    "isError": true,
                    "data": { "status": "internal_error", "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>() }
                }));
            }
        };
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();

        let mut migrations = Vec::new();
        let mut total_remnants_all = 0i64;
        for row in &rows {
            if row.len() < 4 {
                continue;
            }
            let id = row[0].as_str().unwrap_or("").to_string();
            let title = row[1].as_str().unwrap_or("").to_string();
            let status = row[2].as_str().unwrap_or("").to_string();
            let metadata: Value = match &row[3] {
                Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                other => other.clone(),
            };

            if let Some(want) = status_filter {
                if status != want {
                    continue;
                }
            }
            let meta_from = metadata.get("from_tech").and_then(Value::as_str);
            let meta_to = metadata.get("to_tech").and_then(Value::as_str);
            if let Some(ft) = from_tech {
                if meta_from.map(|v| v.eq_ignore_ascii_case(ft)) != Some(true) {
                    continue;
                }
            }
            if let Some(tt) = to_tech {
                if meta_to.map(|v| v.eq_ignore_ascii_case(tt)) != Some(true) {
                    continue;
                }
            }

            // 2. Remnant edges for this migration.
            let edge_raw = self
                .graph_store
                .query_json(&format!(
                    "SELECT target_id, metadata FROM soll.Edge WHERE source_id = '{}' AND relation_type = 'HAS_REMNANT'",
                    escape_sql(&id)
                ))
                .unwrap_or_else(|_| "[]".to_string());
            let edge_rows: Vec<Vec<Value>> = serde_json::from_str(&edge_raw).unwrap_or_default();

            let mut by_kind: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            let mut by_file: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            let mut remnant_targets: Vec<Value> = Vec::new();
            for er in &edge_rows {
                if er.is_empty() {
                    continue;
                }
                let target_id = er[0].as_str().unwrap_or("").to_string();
                let emeta: Value = er
                    .get(1)
                    .map(|m| match m {
                        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                        other => other.clone(),
                    })
                    .unwrap_or(Value::Null);
                let tkind = emeta
                    .get("target_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                *by_kind.entry(tkind.clone()).or_insert(0) += 1;
                if tkind == "ist:indexed_file" {
                    *by_file.entry(target_id.clone()).or_insert(0) += 1;
                }
                if group_by.is_none()
                    || group_by == Some("file") && tkind == "ist:indexed_file"
                    || group_by == Some("symbol") && tkind == "ist:symbol"
                    || group_by == Some("chunk") && tkind == "ist:chunk"
                {
                    remnant_targets.push(json!({ "target_id": target_id, "target_kind": tkind }));
                }
            }
            let remnant_count = edge_rows.len() as i64;
            total_remnants_all += remnant_count;

            // Progress: honest only when a baseline was recorded at migration
            // creation (metadata.baseline_remnants). Otherwise null (no
            // fabricated %). debt_budget surfaces breach when declared.
            let baseline = metadata
                .get("baseline_remnants")
                .and_then(Value::as_i64);
            let progress_pct = baseline.and_then(|b| {
                if b > 0 {
                    Some((((b - remnant_count).max(0) as f64) / (b as f64) * 100.0).round())
                } else {
                    None
                }
            });
            let budget = metadata.get("debt_budget").cloned();
            let max_residue = budget
                .as_ref()
                .and_then(|b| b.get("max_residue_files"))
                .and_then(Value::as_i64);
            let breached = max_residue.map(|m| remnant_count > m);

            migrations.push(json!({
                "id": id,
                "title": title,
                "status": status,
                "from_tech": meta_from,
                "to_tech": meta_to,
                "remnant_count": remnant_count,
                "by_target_kind": by_kind,
                "by_file": by_file.iter().map(|(p, c)| json!({ "path": p, "count": c })).collect::<Vec<_>>(),
                "remnants": remnant_targets,
                "baseline_remnants": baseline,
                "progress_pct": progress_pct,
                "debt_budget": budget,
                "budget_breached": breached,
            }));
        }

        let text = if migrations.is_empty() {
            "No TechnologyMigration nodes match the filter.".to_string()
        } else {
            let mut lines = vec![format!(
                "### 🧬 Tech-Debt Inventory ({} migration(s), {} remnant(s) total)",
                migrations.len(),
                total_remnants_all
            )];
            for m in &migrations {
                let breach = match m.get("budget_breached") {
                    Some(Value::Bool(true)) => " ⚠️ BUDGET BREACHED",
                    _ => "",
                };
                let prog = m
                    .get("progress_pct")
                    .and_then(Value::as_f64)
                    .map(|p| format!(", {}% migrated", p))
                    .unwrap_or_default();
                lines.push(format!(
                    "- {} [{}] {} → {} : {} remnant(s){}{}",
                    m.get("id").and_then(Value::as_str).unwrap_or(""),
                    m.get("status").and_then(Value::as_str).unwrap_or(""),
                    m.get("from_tech").and_then(Value::as_str).unwrap_or("?"),
                    m.get("to_tech").and_then(Value::as_str).unwrap_or("?"),
                    m.get("remnant_count").and_then(Value::as_i64).unwrap_or(0),
                    prog,
                    breach
                ));
            }
            lines.join("\n")
        };

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": "ok",
                "project_code": project_code,
                "migration_count": migrations.len(),
                "total_remnants": total_remnants_all,
                "migrations": migrations,
            }
        }))
    }

    /// REQ-AXO-902032 (N4) — work-plan weighting signal. Returns active
    /// TechnologyMigrations that still carry HAS_REMNANT residue, ranked by
    /// remnant_count DESC (debt magnitude). `None` when there are zero such
    /// migrations (AC5: zero overhead when idle). The planner reads this to
    /// weight incomplete migrations alongside the scored REQ waves.
    pub(crate) fn tech_debt_work_plan_signal(&self, project_code: &str) -> Option<Value> {
        // Aggregate remnant counts per migration in one grouped scan, joined to
        // the migration node for status/title. Terminal-status migrations
        // (complete / abandoned) are excluded — their residue is not actionable.
        let sql = format!(
            "SELECT n.id, n.title, n.status, n.metadata, count(e.target_id) AS remnants \
             FROM soll.Node n \
             JOIN soll.Edge e ON e.source_id = n.id AND e.relation_type = 'HAS_REMNANT' \
             WHERE n.type = 'TechnologyMigration' \
             AND n.project_code = '{}' \
             AND n.status NOT IN ('complete', 'abandoned') \
             GROUP BY n.id, n.title, n.status, n.metadata \
             ORDER BY remnants DESC, n.id",
            escape_sql(project_code)
        );
        let raw = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            return None;
        }
        let mut migrations = Vec::new();
        let mut total = 0i64;
        for r in &rows {
            if r.len() < 5 {
                continue;
            }
            let remnants = r[4].as_i64().or_else(|| r[4].as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
            total += remnants;
            let metadata: Value = match &r[3] {
                Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                other => other.clone(),
            };
            let max_residue = metadata
                .get("debt_budget")
                .and_then(|b| b.get("max_residue_files"))
                .and_then(Value::as_i64);
            migrations.push(json!({
                "id": r[0].as_str().unwrap_or(""),
                "title": r[1].as_str().unwrap_or(""),
                "status": r[2].as_str().unwrap_or(""),
                "remnant_count": remnants,
                "budget_breached": max_residue.map(|m| remnants > m),
            }));
        }
        if migrations.is_empty() {
            return None;
        }
        Some(json!({
            "active_migrations": migrations.len(),
            "total_remnants": total,
            "migrations": migrations,
            "advice": "incomplete technology migrations carry unresolved residue — weight cleanup work; run `tech_debt_inventory` for the per-file set"
        }))
    }

    /// REQ-AXO-902032 (N4) — pre-flight / sub-agent residue lookup.
    ///
    /// Given a set of edited file paths, returns the migrations whose
    /// HAS_REMNANT set targets those files (via the `ist:indexed_file`
    /// discriminator). Each hit: (migration_id, title, status, from→to, path).
    /// Empty input or zero active migrations → empty Vec (AC5: zero overhead).
    pub(crate) fn migrations_with_remnant_path(&self, paths: &[String]) -> Vec<Value> {
        let cleaned: Vec<&str> = paths
            .iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Vec::new();
        }
        // IST stores absolute paths; pre-flight diff_paths are often
        // repo-relative. Match exact OR path-suffix so both forms resolve.
        let path_match = cleaned
            .iter()
            .map(|p| {
                let esc = escape_sql(p);
                format!("e.target_id = '{esc}' OR e.target_id LIKE '%/{esc}'")
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        // Join HAS_REMNANT edges (file-kind) against migration nodes. Small
        // table (soll.Edge ~hundreds of rows) — single indexed scan.
        let sql = format!(
            "SELECT e.target_id, n.id, n.title, n.status, n.metadata \
             FROM soll.Edge e \
             JOIN soll.Node n ON n.id = e.source_id AND n.type = 'TechnologyMigration' \
             WHERE e.relation_type = 'HAS_REMNANT' \
             AND ({}) \
             AND COALESCE(e.metadata->>'target_kind', '') = 'ist:indexed_file'",
            path_match
        );
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        rows.iter()
            .filter_map(|r| {
                if r.len() < 5 {
                    return None;
                }
                let metadata: Value = match &r[4] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
                    other => other.clone(),
                };
                Some(json!({
                    "path": r[0].as_str().unwrap_or(""),
                    "migration_id": r[1].as_str().unwrap_or(""),
                    "migration_title": r[2].as_str().unwrap_or(""),
                    "migration_status": r[3].as_str().unwrap_or(""),
                    "from_tech": metadata.get("from_tech").and_then(Value::as_str),
                    "to_tech": metadata.get("to_tech").and_then(Value::as_str),
                    "debt_policy": metadata.get("debt_policy").and_then(Value::as_str),
                }))
            })
            .collect()
    }
}
