// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use crate::embedding_contract::GRAPH_MODEL_ID;

impl McpServer {
    fn json_to_i64(value: &Value) -> Option<i64> {
        match value {
            Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_u64().and_then(|v| i64::try_from(v).ok()))
                .or_else(|| n.as_f64().map(|v| v.round() as i64)),
            Value::String(s) => s
                .parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|v| v.round() as i64)),
            _ => None,
        }
    }

    fn sql_scalar(&self, query: &str) -> i64 {
        let raw = match self.graph_store.execute_raw_sql_gateway(query) {
            Ok(raw) => raw,
            Err(_) => return 0,
        };
        let rows: Vec<Vec<Value>> = match serde_json::from_str(&raw) {
            Ok(rows) => rows,
            Err(_) => return 0,
        };
        rows.first()
            .and_then(|row| row.first())
            .and_then(Self::json_to_i64)
            .unwrap_or(0)
    }

    fn sql_rows(&self, query: &str) -> Vec<Vec<Value>> {
        self.graph_store
            .execute_raw_sql_gateway(query)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
            .unwrap_or_default()
    }

    fn project_filter(project: &str, column: &str) -> String {
        if project == "*" {
            "1=1".to_string()
        } else {
            format!("{} = '{}'", column, project.replace('\'', "''"))
        }
    }

    pub(crate) fn indexing_diagnosis_markdown(&self, project: &str) -> String {
        let file_filter = Self::project_filter(project, "project_code");
        let symbol_filter = Self::project_filter(project, "project_code");
        let known = self.sql_scalar(&format!("SELECT count(*) FROM File WHERE {}", file_filter));
        let global_known = self.sql_scalar("SELECT count(*) FROM File");
        let pending = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status = 'pending'",
            file_filter
        ));
        let indexing = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status = 'indexing'",
            file_filter
        ));
        let completed = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status IN ('indexed','indexed_degraded','skipped','deleted')",
            file_filter
        ));
        let symbols = self.sql_scalar(&format!(
            "SELECT count(*) FROM Symbol WHERE {}",
            symbol_filter
        ));
        // REQ-AXO-251: under PG age-only-relations, the SQL CALLS / CALLS_NIF
        // tables are empty/dropped — these governance counts return 0 cleanly
        // (canonical edge counts live in AGE post-Stop A; this diagnostic is
        // a SQL-storage health probe).
        let skip_legacy_relations = self.graph_store.skip_legacy_relations();
        let calls_direct = if skip_legacy_relations {
            0
        } else {
            self.sql_scalar(&format!(
                "SELECT count(*) FROM CALLS c JOIN Symbol s ON c.source_id = s.id WHERE {}",
                Self::project_filter(project, "s.project_code")
            ))
        };
        let calls_nif = if skip_legacy_relations {
            0
        } else {
            self.sql_scalar(&format!(
                "SELECT count(*) FROM CALLS_NIF c JOIN Symbol s ON c.source_id = s.id WHERE {}",
                Self::project_filter(project, "s.project_code")
            ))
        };
        let top_reasons = self.sql_rows(&format!(
            "SELECT COALESCE(status_reason, 'unknown'), count(*) \
             FROM File \
             WHERE {} AND status IN ('pending','indexing','indexed_degraded','oversized_for_current_budget') \
             GROUP BY 1 ORDER BY 2 DESC, 1 ASC LIMIT 5",
            file_filter
        ));
        let top_errors = self.sql_rows(&format!(
            "SELECT COALESCE(last_error_reason, 'unknown'), count(*) \
             FROM File \
             WHERE {} AND last_error_reason IS NOT NULL \
             GROUP BY 1 ORDER BY 2 DESC, 1 ASC LIMIT 5",
            file_filter
        ));

        // REQ-AXO-212 — sub-causes carry ADR-2026-04-18-aligned
        // vocabulary so the LLM gets a single actionable next step
        // instead of the historical generic "scope_mismatch" message.
        // Each cause is (machine_id, human_explanation, remediation).
        let mut causes: Vec<(&'static str, String, &'static str)> = Vec::new();
        let runtime_mode = std::env::var("AXON_RUNTIME_MODE").unwrap_or_default();
        let watch_root_set = std::env::var("AXON_WATCH_DIR")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);

        if known == 0 {
            if !watch_root_set {
                causes.push((
                    "watch_root_unconfigured",
                    "no AXON_WATCH_DIR configured for this runtime; the indexer has no roots to scan"
                        .to_string(),
                    "set AXON_WATCH_DIR or watch_root in .axon/config.json then restart axon-indexer",
                ));
            } else if runtime_mode == "brain_only" {
                causes.push((
                    "runtime_mode_excludes_indexing",
                    "current runtime mode is brain_only; the indexer process is intentionally not running"
                        .to_string(),
                    "switch to indexer_full (or indexer_graph) via `axon-{live,dev} start --indexer-full`",
                ));
            } else if project != "*" && global_known > 0 {
                causes.push((
                    "path_not_in_runtime_registry",
                    "the workspace contains indexed files, but none for this project_code; \
                     the project may not be registered yet"
                        .to_string(),
                    "run `axon_init_project(project_path=<absolute path of the project>)` to register \
                     the project_code in the runtime registry",
                ));
            } else {
                causes.push((
                    "discovery_absent_or_filtered",
                    "no files discovered under the configured watch root \
                     (filter, .axonignore, .gitignore, or permissions)"
                        .to_string(),
                    "edit .axonignore to re-include relevant paths via `+pattern`, or verify \
                     filesystem permissions on the watch root",
                ));
            }
        }
        if known > 0 && completed == 0 && (pending + indexing) > 0 {
            causes.push((
                "ingestion_not_completed",
                "files in pending/indexing; pipeline possibly blocked or still running".to_string(),
                "wait one or two indexer cycles, then re-run diagnose_indexing; if still stuck, \
                 inspect `last_error_reason` and `status_reason` columns",
            ));
        }
        // file_too_large_for_budget surfaces the oversized_for_current_budget status.
        let oversized = self.sql_scalar(&format!(
            "SELECT count(*) FROM File WHERE {} AND status = 'oversized_for_current_budget'",
            file_filter
        ));
        if oversized > 0 {
            causes.push((
                "file_too_large_for_budget",
                format!(
                    "{} file(s) exceed the indexer queue memory budget for the current runtime profile",
                    oversized
                ),
                "increase AXON_QUEUE_MEMORY_BUDGET_BYTES, or split the offending file(s) before \
                 reindexing",
            ));
        }
        if known > 0 && symbols == 0 {
            causes.push((
                "parser_extraction_gap",
                "files known but 0 symbols extracted (unsupported language or parse failure)"
                    .to_string(),
                "verify tree-sitter grammar coverage for the file extensions; inspect \
                 `last_error_reason` for parser-side failures",
            ));
        }
        if symbols > 0 && (calls_direct + calls_nif) == 0 {
            causes.push((
                "call_graph_gap",
                "symbols present but call graph empty for this scope".to_string(),
                "run bridge refinement; inspect FFI / NIF boundaries for cross-module calls",
            ));
        }
        if causes.is_empty() {
            causes.push((
                "no_blocker_detected",
                "no major blocker detected by this diagnostic".to_string(),
                "no remediation needed; verify expected counts against `audit` for the project",
            ));
        }

        let reason_lines = if top_reasons.is_empty() {
            "* no dominant reason".to_string()
        } else {
            top_reasons
                .iter()
                .filter_map(|row| {
                    let reason = row.first()?.as_str()?;
                    let count = row
                        .get(1)?
                        .as_i64()
                        .or_else(|| row.get(1)?.as_u64().map(|v| v as i64))?;
                    Some(format!("* `{}`: {}", reason, count))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let error_lines = if top_errors.is_empty() {
            "* no parser/commit error reported in `last_error_reason`".to_string()
        } else {
            top_errors
                .iter()
                .filter_map(|row| {
                    let reason = row.first()?.as_str()?;
                    let count = row
                        .get(1)?
                        .as_i64()
                        .or_else(|| row.get(1)?.as_u64().map(|v| v as i64))?;
                    Some(format!("* `{}`: {}", reason, count))
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let cause_lines = causes
            .iter()
            .map(|(id, explain, remediation)| format_diagnose_cause_line(id, explain, remediation))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "### 🔎 Day-1 Indexing Diagnosis ({})\n\n\
             **Scope facts**\n\
             * known files: {}\n\
             * completed files: {}\n\
             * pending: {}\n\
             * indexing: {}\n\
             * symbols: {}\n\
             * calls (direct): {}\n\
             * calls (nif): {}\n\n\
             **Likely root causes**\n{}\n\n\
             **Top status reasons**\n{}\n\n\
             **Top parser/runtime errors**\n{}\n\n\
             **Remediation hints**\n\
             * validate project code and scope (`project_code`) used in calls\n\
             * check watch root and ignored paths\n\
             * inspect parser support and `last_error_reason`\n\
             * if symbols > 0 but calls = 0, run bridge refinement and inspect FFI boundaries",
            project,
            known,
            completed,
            pending,
            indexing,
            symbols,
            calls_direct,
            calls_nif,
            cause_lines,
            reason_lines,
            error_lines
        )
    }

    pub(crate) fn axon_diagnose_indexing(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let report = self.indexing_diagnosis_markdown(project);
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn file_count_for_project(&self, project: &str) -> i64 {
        let query = if project == "*" {
            "SELECT count(*) FROM File".to_string()
        } else {
            format!(
                "SELECT count(*) FROM File WHERE project_code = '{}'",
                project.replace('\'', "''")
            )
        };

        self.graph_store
            .execute_raw_sql_gateway(&query)
            .ok()
            .and_then(|raw| {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
                Self::json_to_i64(rows.first()?.first()?)
            })
            .unwrap_or(0)
    }

    fn build_graph_clone_section(&self, symbol: &str) -> Option<String> {
        let anchor_res = self
            .graph_store
            .query_json_param(
                "SELECT id FROM Symbol WHERE id = $sym OR name = $sym LIMIT 1",
                &json!({"sym": symbol}),
            )
            .ok()?;
        let anchor_rows: Vec<Vec<Value>> = serde_json::from_str(&anchor_res).unwrap_or_default();
        let anchor_id = anchor_rows.first()?.first()?.as_str()?;
        // REQ-AXO-271 slice 2b : PG canonical only.
        // pgvector `<=>` returns cosine distance on `vector(N)` columns.
        let cosine_expr = "(anchor.embedding <=> peer.embedding)";
        let query = format!(
            "
            SELECT other.name, other.kind, {cosine_expr} AS score
            FROM GraphEmbedding anchor
            JOIN GraphProjectionState anchor_state
              ON anchor_state.anchor_type = anchor.anchor_type
             AND anchor_state.anchor_id = anchor.anchor_id
             AND anchor_state.radius = anchor.radius
             AND anchor_state.source_signature = anchor.source_signature
             AND anchor_state.projection_version = anchor.projection_version
            JOIN GraphEmbedding peer
              ON peer.anchor_type = anchor.anchor_type
             AND peer.radius = anchor.radius
             AND peer.model_id = anchor.model_id
             AND peer.anchor_id <> anchor.anchor_id
            JOIN GraphProjectionState peer_state
              ON peer_state.anchor_type = peer.anchor_type
             AND peer_state.anchor_id = peer.anchor_id
             AND peer_state.radius = peer.radius
             AND peer_state.source_signature = peer.source_signature
             AND peer_state.projection_version = peer.projection_version
            JOIN Symbol other
              ON other.id = peer.anchor_id
            WHERE anchor.anchor_type = 'symbol'
              AND anchor.anchor_id = $anchor
              AND anchor.model_id = '{GRAPH_MODEL_ID}'
              AND {cosine_expr} < 0.05
            ORDER BY score ASC
            LIMIT 5"
        );
        let res = self
            .graph_store
            .query_json_param(&query, &json!({"anchor": anchor_id}))
            .ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        if rows.is_empty() {
            return None;
        }

        Some(format!(
            "\n\n### Similar Graph Neighborhoods\n\n**Status:** graph-derived context via `GraphEmbedding`, useful for spotting nearby neighborhoods; not a canonical architecture truth.\n\n{}",
            format_table_from_json(&res, &["Name", "Type", "Neighborhood Distance"])
        ))
    }

    pub(crate) fn axon_audit(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = requested_project;

        let file_count = self.file_count_for_project(project);

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            let diagnostic = self.indexing_diagnosis_markdown(project);
            let report = format!(
                "## 🛡️ Compliance Audit: {}\n\n{}",
                project,
                format_standard_contract(
                    "warn_input_not_ready",
                    "project appears unindexed for audit scope",
                    &format!("project:{}", project),
                    &format!("{}\n\n{}", warning, diagnostic),
                    &[
                        "run indexing and retry",
                        "check project code and ignore filters"
                    ],
                    "low",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }

        let (sec_score, paths) = self
            .graph_store
            .get_security_audit(project)
            .unwrap_or((100, "[]".to_string()));
        let cov_score = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let tech_debt = self
            .graph_store
            .get_technical_debt(project)
            .unwrap_or_default();
        let god_objects = self
            .graph_store
            .get_god_objects(project)
            .unwrap_or_default();
        let telemetry_score = self.graph_store.get_telemetry_score(project).unwrap_or(100);
        let dead_code = self.graph_store.get_dead_code_count(project).unwrap_or(0);
        let hygiene_score = (100 - (god_objects.len() as i64 * 10) - (dead_code * 2)).max(0);

        let mut evidence = String::new();
        if let Some(note) = self.project_scope_truth_note((project != "*").then_some(project)) {
            evidence.push_str(&note);
            evidence.push('\n');
        }
        if let Some(note) =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)))
        {
            evidence.push_str(&note);
            evidence.push('\n');
        }
        evidence.push_str(&format!("### 🔒 Security: {}/100\n", sec_score));

        if sec_score < 100 {
            evidence.push_str("🚨 **Potential vulnerabilities detected.**\n");
            evidence.push_str(&format!("Critical paths found: {}\n", paths));
        } else {
            evidence.push_str("✅ No critical path to dangerous functions detected.\n");
        }

        if !tech_debt.is_empty() {
            evidence.push_str("\n### ⚠️ Technical Debt & Panic Points\n");
            evidence.push_str("The following points present crash risks (panic) or poor error handling:\n\n");
            for (file, issue) in tech_debt.iter().take(10) {
                evidence.push_str(&format!("*   `{}` dans `{}`\n", issue, file));
            }
            if tech_debt.len() > 10 {
                evidence.push_str(&format!(
                    "*... and {} more points detected.*\n",
                    tech_debt.len() - 10
                ));
            }
        }

        evidence.push_str(&format!("\n### 🧪 Quality & Tests: {}%\n", cov_score));

        evidence.push_str(&format!(
            "\n### 🧹 Code Hygiene (Clean-As-You-Go): {}/100\n",
            hygiene_score
        ));
        if god_objects.is_empty() && dead_code == 0 {
            evidence.push_str("✅ Healthy codebase: zero God Objects and zero dead code detected.\n");
        } else {
            if !god_objects.is_empty() {
                evidence.push_str(&format!(
                    "* 🚨 {} God Objects (monolithic files/functions) detected.\n",
                    god_objects.len()
                ));
            }
            if dead_code > 0 {
                evidence.push_str(&format!("* 🗑️ {} dead functions (non-public with no callers) detected. Please remove them.\n", dead_code));
            }
        }

        evidence.push_str(&format!(
            "\n### 📡 Telemetry & Observability: {}/100\n",
            telemetry_score
        ));
        if telemetry_score < 100 {
            evidence.push_str("🚨 Raw text logging calls (`println!`, `console.log`, etc.) detected. Use structured telemetry.\n");
        } else {
            evidence.push_str("✅ Observability compliant (zero raw log calls detected).\n");
        }

        let circular_deps = self
            .graph_store
            .get_circular_dependencies(project)
            .unwrap_or_default();
        let domain_leaks = self
            .graph_store
            .get_domain_leakage(project, "domain", "infrastructure")
            .unwrap_or_default();
        let unsafe_exposure = self
            .graph_store
            .get_unsafe_exposure(project)
            .unwrap_or_default();
        let nif_blocking_risks = self
            .graph_store
            .get_nif_blocking_risks(project)
            .unwrap_or_default();

        evidence.push_str("\n### 🌪️ Architectural Anti-Patterns\n");
        if circular_deps.is_empty() {
            evidence.push_str("✅ No circular dependencies detected.\n");
        } else {
            evidence.push_str(&format!(
                "🚨 [{}] Circular dependencies detected:\n",
                circular_deps.len()
            ));
            for path in circular_deps.iter().take(5) {
                evidence.push_str(&format!("*   `{}`\n", path));
            }
            if circular_deps.len() > 5 {
                evidence.push_str(&format!(
                    "*   ... and {} more loops.\n",
                    circular_deps.len() - 5
                ));
            }
        }

        if domain_leaks.is_empty() {
            evidence.push_str("✅ No domain leakage detected.\n");
        } else {
            evidence.push_str(&format!(
                "🚨 [{}] Domain leaks detected:\n",
                domain_leaks.len()
            ));
            for leak in domain_leaks.iter().take(5) {
                evidence.push_str(&format!("*   `{}`\n", leak));
            }
            if domain_leaks.len() > 5 {
                evidence.push_str(&format!(
                    "*   ... and {} more leaks.\n",
                    domain_leaks.len() - 5
                ));
            }
        }

        if unsafe_exposure.is_empty() {
            evidence.push_str("✅ No unsafe exposure detected.\n");
        } else {
            evidence.push_str(&format!(
                "🚨 [{}] Unsafe exposures detected:\n",
                unsafe_exposure.len()
            ));
            for exp in unsafe_exposure.iter().take(5) {
                evidence.push_str(&format!("*   `{}`\n", exp));
            }
            if unsafe_exposure.len() > 5 {
                evidence.push_str(&format!(
                    "*   ... and {} more exposures.\n",
                    unsafe_exposure.len() - 5
                ));
            }
        }

        if nif_blocking_risks.is_empty() {
            evidence.push_str("✅ No NIF blocking risk (Scheduler Starvation) detected.\n");
        } else {
            evidence.push_str(&format!(
                "🚨 [{}] NIF blocking risks detected (critical call depth):\n",
                nif_blocking_risks.len()
            ));
            for risk in nif_blocking_risks.iter().take(5) {
                evidence.push_str(&format!("*   `{}`\n", risk));
            }
            if nif_blocking_risks.len() > 5 {
                evidence.push_str(&format!(
                    "*   ... and {} more risks.\n",
                    nif_blocking_risks.len() - 5
                ));
            }
        }

        let overall_score = if !circular_deps.is_empty()
            || !domain_leaks.is_empty()
            || !unsafe_exposure.is_empty()
            || !nif_blocking_risks.is_empty()
        {
            0
        } else {
            (sec_score + cov_score + hygiene_score + telemetry_score) / 4
        };

        let report = format!(
            "## 🛡️ Compliance Audit: {}\n\n{}",
            project,
            format_standard_contract(
                "ok",
                "governance audit computed",
                &format!("project:{}", project),
                &evidence_by_mode(&evidence, mode),
                &[
                    "review critical security paths first",
                    "delete dead code",
                    "triage top technical debt items"
                ],
                if overall_score >= 90 {
                    "high"
                } else {
                    "medium"
                },
            )
        );
        // REQ-AXO-91525 (MIL-AXO-019 Tier A aggregator) — tri-modal
        // envelope. `audit` composes security audit, coverage,
        // technical debt, god objects, telemetry, dead code, hygiene —
        // all from `graph_store` SQL aggregators today. Adding the
        // RAM cognitive overlay (bridges, SCCs, articulation points)
        // is a follow-up slice ; the algos are ready
        // (`ist_snapshot::algorithms`, vague 1c) and the wiring
        // pattern is the same as `anomalies` REQ-AXO-91517 commit
        // `6ac5e3cc`.
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project": project,
                "security_score": sec_score,
                "coverage_score": cov_score,
                "telemetry_score": telemetry_score,
                "hygiene_score": hygiene_score,
                "overall_score": overall_score,
                "surfaces_used": ["graph_pg"],
                "total_available": 1,
                "next_call_hint": "anomalies project=<code> mode=verbose for structural findings + cognitive_signals"
            }
        }))
    }

    pub(crate) fn axon_health(&self, args: &Value) -> Option<Value> {
        let requested_project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = requested_project;

        let file_count = self.file_count_for_project(project);

        if file_count < 1 {
            let warning = format!(
                "⚠️ Warning: Project '{}' seems unindexed or parser failed (Found {} files). Health metrics are invalid.",
                project, file_count
            );
            let diagnostic = self.indexing_diagnosis_markdown(project);
            let report = format!(
                "## 🏥 Health Report: {}\n\n{}",
                project,
                format_standard_contract(
                    "warn_input_not_ready",
                    "project appears unindexed for health scope",
                    &format!("project:{}", project),
                    &format!("{}\n\n{}", warning, diagnostic),
                    &[
                        "run indexing and retry",
                        "validate parser coverage for project languages"
                    ],
                    "low",
                )
            );
            return Some(json!({ "content": [{ "type": "text", "text": report }] }));
        }

        let coverage = self.graph_store.get_coverage_score(project).unwrap_or(0);
        let god_objects = self
            .graph_store
            .get_god_objects(project)
            .unwrap_or_default();

        let mut evidence = format!("Coverage {}%. Stability high.", coverage);
        if let Some(note) = self.project_scope_truth_note((project != "*").then_some(project)) {
            evidence.push_str(&format!("\n{}", note));
        }
        if let Some(note) =
            self.degraded_truth_note(self.degraded_file_count((project != "*").then_some(project)))
        {
            evidence.push_str(&format!("\n{}", note));
        }
        if !god_objects.is_empty() {
            let god_list: Vec<String> = god_objects
                .iter()
                .map(|(name, count)| format!("{} ({} lines)", name, count))
                .collect();
            evidence.push_str(&format!("\nGod Objects detected: {}", god_list.join(", ")));
        }

        let report = format!(
            "## 🏥 Health Report: {}\n\n{}",
            project,
            format_standard_contract(
                "ok",
                "health metrics computed",
                &format!("project:{}", project),
                &evidence_by_mode(&evidence, mode),
                &[
                    "inspect god objects first",
                    "run audit for governance details"
                ],
                "medium",
            )
        );
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    pub(crate) fn axon_semantic_clones(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        // REQ-AXO-271 slice 2b : PG canonical only.
        // Symbol.embedding is `vector(N)` ; pgvector `<=>` returns cosine distance.
        let cosine_expr = "(s.embedding <=> other.embedding)";
        let query = format!(
            "SELECT other.name, other.kind, {cosine_expr} as score \
             FROM Symbol s, Symbol other \
             WHERE s.name = '{}' AND s.name <> other.name AND {cosine_expr} < 0.05 \
             ORDER BY score ASC LIMIT 5",
            symbol.replace("'", "''")
        );
        match self.graph_store.query_json(&query) {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                let mut report = if !rows.is_empty() {
                    format!(
                        "### 👯 Semantic Clones detected for '{}'\n\n{}",
                        symbol,
                        format_table_from_json(&res, &["Name", "Type", "Similarity"])
                    )
                } else {
                    format!(
                        "✅ No obvious semantic clone (similarity > 95%) found for '{}'.",
                        symbol
                    )
                };
                if let Some(section) = self.build_graph_clone_section(symbol) {
                    report.push_str(&section);
                }
                Some(json!({ "content": [{ "type": "text", "text": report }] }))
            }
            Err(e) => Some(json!({
                "content": [{ "type": "text", "text": format!("Cloning Error: {}", e) }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "parameter_repair": {
                        "invalid_field": "symbol",
                        "follow_up_tools": ["inspect", "query", "status"],
                        "hint": "semantic-clones computation failed; verify the symbol resolves via `inspect` and runtime is healthy"
                    },
                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                }
            })),
        }
    }

    pub(crate) fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
        let source_layer = args.get("source_layer")?.as_str()?;
        let target_layer = args.get("target_layer")?.as_str()?;

        // REQ-AXO-271 slice 2d invariant : the legacy SQL
        // CALLS / CONTAINS tables are dropped under PG canonical so
        // the `WITH RECURSIVE call_paths` translation that powered
        // this tool pre-MIL-AXO-017 is dead.
        //
        // REQ-AXO-91516 (planned) will rewire the surface to the
        // in-memory `IstGraph` via the new `layer_violations`
        // algorithm (MIL-AXO-019 vague 1c, commit `787ac797`) — that
        // migration ships in a follow-up slice. Until then surface a
        // structured `not_implemented` envelope so the LLM sees the
        // truth instead of the previous silent "✅ no drift"
        // (which was the always-empty result of the dead SQL path).
        let report = format!(
            "🛠️ **architectural_drift not yet wired to RAM graph**\n\nSource layer: `{}`\nTarget layer: `{}`\n\nThe PG-canonical migration is REQ-AXO-91516 (Tier A, MIL-AXO-019). Until it ships, use `impact` + `path` for ad-hoc layer-crossing analysis.",
            source_layer, target_layer
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "not_implemented",
                "source_layer": source_layer,
                "target_layer": target_layer,
                "surfaces_used": ["pending_ram_migration"],
                "next_call_hint": "impact symbol=<source-layer-entry>",
                "tracking_req": "REQ-AXO-91516",
            }
        }))
    }
}

/// REQ-AXO-212 — render one diagnose_indexing cause as a two-line
/// markdown bullet: the machine-stable id + 1-line remediation. Pure
/// helper factored out so tests can exercise the rendering contract
/// without booting a full McpServer.
fn format_diagnose_cause_line(id: &str, explain: &str, remediation: &str) -> String {
    format!("* **{id}**: {explain}\n  * remediation: {remediation}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cause_line_renders_machine_id_and_remediation() {
        let line = format_diagnose_cause_line(
            "watch_root_unconfigured",
            "no AXON_WATCH_DIR configured",
            "set AXON_WATCH_DIR or watch_root in .axon/config.json then restart axon-indexer",
        );
        assert!(line.starts_with("* **watch_root_unconfigured**:"));
        assert!(line.contains("\n  * remediation: set AXON_WATCH_DIR"));
        assert!(line.contains("axon-indexer"));
    }

    #[test]
    fn cause_line_preserves_distinct_id_and_explanation() {
        let line = format_diagnose_cause_line("path_not_in_runtime_registry", "explain", "fix");
        let lines: Vec<&str> = line.lines().collect();
        assert_eq!(lines.len(), 2, "cause renders on exactly two lines");
        assert!(lines[0].contains("path_not_in_runtime_registry"));
        assert!(lines[0].contains("explain"));
        assert!(lines[1].trim_start().starts_with("* remediation: fix"));
    }
}
