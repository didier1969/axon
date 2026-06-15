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

    fn project_filter(project: &str, column: &str) -> String {
        if project == "*" {
            "1=1".to_string()
        } else {
            format!("{} = '{}'", column, project.replace('\'', "''"))
        }
    }

    pub(crate) fn indexing_diagnosis_markdown(&self, project: &str) -> String {
        // Canonical projection (REQ-AXO-901865): diagnose_indexing reads the
        // SAME ist.project_telemetry view as the dashboard + embedding_status,
        // so its file/symbol counts reconcile exactly (byte-for-byte). Coverage
        // is REAL — `known` = files_chunked (enrolled files with >=1 chunk).
        // IndexedFile DOES carry project_code ; the old "no project_code by
        // design" note and the status pending/indexing machine were retired
        // (REQ-AXO-289 / REQ-AXO-901860), so those concepts collapse to 0.
        let where_project = if project == "*" {
            String::new()
        } else {
            format!(" WHERE project_code = '{}'", project.replace('\'', "''"))
        };
        // REQ-AXO-901905 — ::BIGINT cast: SUM(bigint) is promoted to `numeric`
        // by PG, which the SQL-gateway renders as `<unsupported type numeric>`
        // → json_to_i64 fails → silent 0. file_count_for_project already got
        // this fix; diagnose's three SUM()s had been missed (returned `known
        // files: 0` despite a populated IndexedFile).
        let known = self.sql_scalar(&format!(
            "SELECT COALESCE(SUM(files_chunked), 0)::BIGINT FROM axon.project_telemetry{}",
            where_project
        ));
        let global_known = self
            .sql_scalar("SELECT COALESCE(SUM(files_total), 0)::BIGINT FROM axon.project_telemetry");
        let pending = 0i64;
        let indexing = 0i64;
        let completed = known;
        let symbols = self.sql_scalar(&format!(
            "SELECT COALESCE(SUM(symbols), 0)::BIGINT FROM axon.project_telemetry{}",
            where_project
        ));
        // Post-MIL-AXO-017: edge counts from canonical ist.Edge table.
        let calls_direct = self.sql_scalar(&format!(
            "SELECT count(*) FROM ist.Edge e JOIN Symbol s ON e.source_id = s.id WHERE e.relation_type = 'CALLS' AND {}",
            Self::project_filter(project, "s.project_code")
        ));
        let calls_nif = self.sql_scalar(&format!(
            "SELECT count(*) FROM ist.Edge e JOIN Symbol s ON e.source_id = s.id WHERE e.relation_type = 'CALLS_NIF' AND {}",
            Self::project_filter(project, "s.project_code")
        ));
        // REQ-AXO-901653 slice-5c — `status_reason` + `last_error_reason`
        // were public.File columns ; pipeline_v2 doesn't carry equivalent
        // diagnostic data (failures are logged via tracing, not row state).
        let top_reasons: Vec<Vec<Value>> = Vec::new();
        let top_errors: Vec<Vec<Value>> = Vec::new();

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
        // REQ-AXO-901653 slice-5c — `oversized_for_current_budget` status was
        // a public.File enum ; pipeline_v2 enforces budget via in-line stage
        // back-pressure (no persisted oversized flag).
        let oversized = 0i64;
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

        // REQ-AXO-901893 (LEGACY FEED PURGE) — the ingress drain + periodic
        // sweep diagnosis blocks were ripped with the ingress_buffer. Watchman
        // feeds pipeline A directly; DBQ-A drains the backlog. Use
        // `pipeline_status` / `stock_a` (discovered backlog) for feed health.
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
             ### File source — Watchman + DBQ-A (REQ-AXO-901893 / REQ-AXO-901897)\n\
             * Watchman clock/cursor deltas feed pipeline A directly (legacy ingress drain + periodic sweep RIPPED)\n\
             * DBQ-A claim feeder drains the 'discovered' backlog by construction\n\n\
             **Remediation hints**\n\
             * validate project code and scope (`project_code`) used in calls\n\
             * check watch root and ignored paths\n\
             * inspect parser support and `last_error_reason`\n\
             * if symbols > 0 but calls = 0, run bridge refinement and inspect FFI boundaries\n\
             * if the 'discovered' backlog (stock_a in `pipeline_status`) stays high, check the indexer is running and the Watchman daemon is reachable",
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
            error_lines,
        )
    }

    pub(crate) fn axon_diagnose_indexing(&self, args: &Value) -> Option<Value> {
        let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("*");
        let report = self.indexing_diagnosis_markdown(project);
        Some(json!({ "content": [{ "type": "text", "text": report }] }))
    }

    fn file_count_for_project(&self, project: &str) -> i64 {
        // Canonical projection (REQ-AXO-901865) — enrolled file count from the
        // single ist.project_telemetry view, consistent for global + scoped.
        // The old path mixed semantics (global = IndexedFile count, scoped =
        // chunk-distinct file_path) ; both now read files_total from the view.
        let where_project = if project == "*" {
            String::new()
        } else {
            format!(" WHERE project_code = '{}'", project.replace('\'', "''"))
        };
        // REQ-AXO-901905 — cast the aggregate to BIGINT. PG `SUM(bigint)`
        // returns `numeric`, which the SQL-gateway value renderer
        // (postgres/native.rs render_pg_value) does NOT decode (no decimal
        // crate) → it emits the sentinel string "<unsupported type numeric>",
        // which json_to_i64 cannot parse → the count silently collapsed to 0,
        // so EVERY audit/health call was gated as "unindexed" regardless of
        // real enrollment. The ::BIGINT cast renders as INT8 (a plain integer)
        // and counts correctly. (The broader gateway numeric-render gap is
        // tracked separately for a class-level renderer fix.)
        let query = format!(
            "SELECT COALESCE(SUM(files_total), 0)::BIGINT FROM axon.project_telemetry{}",
            where_project
        );

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
        // REQ-AXO-901869 A3 / REQ-AXO-901634 — honour the canonical
        // graph-embedding disable flag. When the lane is turned off the
        // `GraphEmbedding` table is not maintained, so any rows still
        // present are stale ; surfacing them as "Similar Graph
        // Neighborhoods" would be a lie. Emit an explicit disabled note
        // instead of querying.
        if !crate::embedder::graph_embeddings_enabled_from_env() {
            return Some(
                "\n\n### Similar Graph Neighborhoods\n\n**Status:** graph-embedding derivation is temporarily disabled (`AXON_GRAPH_EMBEDDINGS_ENABLED=false`); neighbourhood similarity is not computed. Structural clones above remain canonical.".to_string(),
            );
        }
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

        // REQ-AXO-901970 — warm the RAM snapshot so the structural anti-pattern
        // checks (circular / domain_leakage / unsafe_exposure / nif / dead_code /
        // god_objects) resolve RAM-only. Cold / "*" → those checks surface empty.
        let ram_warm = project != "*" && self.ensure_ram_snapshot_warm(project);

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
        // REQ-AXO-901970 — RAM-only god-objects (warm the per-project CSR, no PG
        // fallback; cold / "*" → empty).
        let god_objects = if project != "*" && self.ensure_ram_snapshot_warm(project) {
            crate::ist_snapshot::process_view()
                .god_objects(project)
                .map(|pairs| {
                    pairs
                        .into_iter()
                        .map(|(name, count)| {
                            (name, serde_json::Value::Number((count as i64).into()))
                        })
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                })
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };
        let telemetry_score = self.graph_store.get_telemetry_score(project).unwrap_or(100);
        // REQ-AXO-901970 — RAM-only dead-code count (no PG fallback; cold → 0).
        let dead_code = if ram_warm {
            crate::ist_snapshot::process_view()
                .dead_code_count(project)
                .unwrap_or(0) as i64
        } else {
            0
        };
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
            evidence.push_str(
                "The following points present crash risks (panic) or poor error handling:\n\n",
            );
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
            evidence
                .push_str("✅ Healthy codebase: zero God Objects and zero dead code detected.\n");
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
        // REQ-AXO-901970 — RAM-only domain leakage (no PG fallback; cold → empty).
        let domain_leaks = if ram_warm {
            crate::ist_snapshot::process_view()
                .domain_leakage(project, "domain", "infrastructure")
                .unwrap_or_default()
        } else {
            Vec::new()
        };
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
        // REQ-AXO-901970 — RAM-only god-objects (warm the per-project CSR, no PG
        // fallback; cold / "*" → empty).
        let god_objects = if project != "*" && self.ensure_ram_snapshot_warm(project) {
            crate::ist_snapshot::process_view()
                .god_objects(project)
                .map(|pairs| {
                    pairs
                        .into_iter()
                        .map(|(name, count)| {
                            (name, serde_json::Value::Number((count as i64).into()))
                        })
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                })
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };

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
        let project = args.get("project").and_then(|v| v.as_str());
        // GUI-AXO-1004 pagination contract.
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 1000) as usize;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .clamp(1, 3) as u32;
        // Over-fetch from vector layer so VF2 has more candidates to confirm.
        let pre_filter_k = (limit + offset).saturating_mul(4).max(20);

        // REQ-AXO-901977 / REQ-AXO-901952 — `ist.Symbol.embedding` is NEVER
        // populated by the canonical pipeline (only chunks are embedded), so the
        // historical `s.embedding <=> other.embedding` arm was permanently dead
        // and semantic_clones always returned nothing. Rank candidate symbols by
        // the MIN cosine distance between their embedded chunks and the SOURCE
        // symbol's representative chunk embedding (ANN over ist.ChunkEmbedding →
        // owning symbol) — the same chunk signal `query`/`retrieve_context` use.
        // Empty source embedding (no chunk) → no rows (graceful, never an error).
        let name_sql = symbol.replace('\'', "''");
        let proj_pred_other = match project {
            Some(p) if !p.is_empty() && p != "*" => {
                format!(" AND other.project_code = '{}'", p.replace('\'', "''"))
            }
            _ => String::new(),
        };
        let proj_pred_src = match project {
            Some(p) if !p.is_empty() && p != "*" => {
                format!(" AND project_code = '{}'", p.replace('\'', "''"))
            }
            _ => String::new(),
        };
        let query = format!(
            "WITH src AS (SELECT id FROM Symbol WHERE name = '{name}'{proj_pred_src} LIMIT 1), \
             src_emb AS ( \
                 SELECT ce.embedding AS emb FROM ist.Chunk c \
                 JOIN ist.ChunkEmbedding ce ON ce.chunk_id = c.id \
                 WHERE c.source_type = 'symbol' AND c.source_id = (SELECT id FROM src) \
                 LIMIT 1 \
             ), \
             ann AS ( \
                 SELECT ce.chunk_id, (ce.embedding <=> (SELECT emb FROM src_emb)) AS dist \
                 FROM ist.ChunkEmbedding ce \
                 WHERE (SELECT emb FROM src_emb) IS NOT NULL \
                 ORDER BY ce.embedding <=> (SELECT emb FROM src_emb) \
                 LIMIT 400 \
             ) \
             SELECT other.id, other.name, other.kind, MIN(a.dist)::float8 AS score \
             FROM ann a \
             JOIN ist.Chunk c ON c.id = a.chunk_id AND c.source_type = 'symbol' \
                 AND c.source_id <> (SELECT id FROM src) \
             JOIN Symbol other ON other.id = c.source_id \
                 AND other.name <> '{name}'{proj_pred_other} \
             GROUP BY other.id, other.name, other.kind \
             HAVING MIN(a.dist) < 0.10 \
             ORDER BY score ASC LIMIT {pre_filter_k}",
            name = name_sql,
        );
        let raw = match self.graph_store.query_json(&query) {
            Ok(r) => r,
            Err(e) => {
                return Some(json!({
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
                }));
            }
        };
        let candidates: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();

        // REQ-AXO-91518 slice 2 — structural confirmation via VF2 on each
        // candidate's neighborhood sub-graph. Requires a warm IstGraphView
        // for `project` ; falls back to vector-only ranking otherwise.
        let view = crate::ist_snapshot::process_view();
        let project_for_graph = project.unwrap_or("");
        let ram_warm = !project_for_graph.is_empty() && view.is_warm(project_for_graph);

        let mut surfaces_used: Vec<&'static str> = vec!["vector_pgvector"];
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        // Resolve the source symbol's canonical IST id so we can extract its
        // own neighborhood. Use the same name + optional project filter as
        // the vector pre-filter.
        let source_id: Option<String> = {
            let proj_pred = match project {
                Some(p) if !p.is_empty() && p != "*" => {
                    format!(" AND project_code = '{}'", p.replace('\'', "''"))
                }
                _ => String::new(),
            };
            let sql = format!(
                "SELECT id FROM Symbol WHERE name = '{}'{proj_pred} LIMIT 1",
                symbol.replace('\'', "''")
            );
            self.graph_store
                .query_json(&sql)
                .ok()
                .and_then(|raw| serde_json::from_str::<Vec<Vec<Value>>>(&raw).ok())
                .and_then(|rows| {
                    rows.into_iter()
                        .next()
                        .and_then(|r| r.into_iter().next())
                        .and_then(|v| v.as_str().map(String::from))
                })
        };

        let (structural_flags, vf2_attempted) = if ram_warm && source_id.is_some() {
            surfaces_used.push("graph_vf2_isomorphism");
            let snap_opt = view.cache_handle().get(project_for_graph);
            if let Some(snap) = snap_opt {
                let src_nbhd =
                    snap.neighborhood_subgraph(source_id.as_deref().unwrap_or(""), max_depth);
                let flags: Vec<bool> = candidates
                    .iter()
                    .map(|c| {
                        let cand_id = c.first().and_then(|v| v.as_str()).unwrap_or("");
                        if cand_id.is_empty() {
                            return false;
                        }
                        let Some(src) = src_nbhd.as_ref() else {
                            return false;
                        };
                        let Some(cand_nbhd) = snap.neighborhood_subgraph(cand_id, max_depth) else {
                            return false;
                        };
                        let matches =
                            crate::ist_snapshot::algorithms::vf2_subgraph_match(src, &cand_nbhd, 1);
                        !matches.is_empty()
                    })
                    .collect();
                (flags, true)
            } else {
                surfaces_degraded.push("graph_ram_unavailable");
                (vec![false; candidates.len()], false)
            }
        } else {
            if !ram_warm {
                surfaces_degraded.push("graph_ram_unavailable");
            }
            (vec![false; candidates.len()], false)
        };

        // Sort: structural matches first (VF2 confirmed), then by cosine score ascending.
        let mut scored: Vec<(usize, bool, f64)> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let score = c.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0);
                let structural = structural_flags.get(i).copied().unwrap_or(false);
                (i, structural, score)
            })
            .collect();
        scored.sort_by(|a, b| match b.1.cmp(&a.1) {
            std::cmp::Ordering::Equal => a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal),
            other => other,
        });

        let total_available = scored.len() as u64;
        let paginated: Vec<&(usize, bool, f64)> = scored.iter().skip(offset).take(limit).collect();
        let returned = paginated.len() as u64;
        let has_more = (offset as u64).saturating_add(returned) < total_available;

        // Build the human-readable section.
        let display_rows: Vec<Vec<Value>> = paginated
            .iter()
            .map(|(idx, structural, _)| {
                let row = &candidates[*idx];
                let name = row.get(1).cloned().unwrap_or(Value::Null);
                let kind = row.get(2).cloned().unwrap_or(Value::Null);
                let score = row.get(3).cloned().unwrap_or(Value::Null);
                let marker = if *structural {
                    "✓ VF2"
                } else if vf2_attempted {
                    "—"
                } else {
                    "n/a"
                };
                vec![name, kind, score, Value::String(marker.to_string())]
            })
            .collect();
        let display_json =
            serde_json::to_string(&display_rows).unwrap_or_else(|_| "[]".to_string());

        let mut report = if !display_rows.is_empty() {
            format!(
                "### 👯 Semantic Clones detected for '{}'\n\n{}",
                symbol,
                format_table_from_json(&display_json, &["Name", "Type", "Cosine", "Structural"])
            )
        } else {
            format!(
                "✅ No obvious semantic clone (cosine < 0.10) found for '{}'.",
                symbol
            )
        };
        if let Some(section) = self.build_graph_clone_section(symbol) {
            report.push_str(&section);
        }

        let next_call_hint = if has_more {
            json!({
                "params": { "symbol": symbol, "limit": limit, "offset": offset + paginated.len(), "max_depth": max_depth },
                "reason": "more candidates available; bump offset to paginate"
            })
        } else {
            json!({
                "params": { "follow_up": "impact symbol=<clone>" },
                "reason": "review blast radius of confirmed clone before dedup"
            })
        };

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "symbol": symbol,
                "project": project.unwrap_or("*"),
                "clone_count": display_rows.len(),
                "data": display_rows,
                "total_available": total_available,
                "returned": returned,
                "has_more": has_more,
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "structural_confirmed": structural_flags.iter().filter(|f| **f).count(),
                "vf2_attempted": vf2_attempted,
                "truncation_strategy": "pgvector_topk_then_vf2",
                "truncation_applied": pre_filter_k < total_available as usize,
                "next_call_hint": next_call_hint,
                "sort_by": "structural_then_cosine"
            }
        }))
    }

    pub(crate) fn axon_architectural_drift(&self, args: &Value) -> Option<Value> {
        let source_layer = args.get("source_layer")?.as_str()?;
        let target_layer = args.get("target_layer")?.as_str()?;
        let project = args.get("project").and_then(|v| v.as_str());
        // GUI-AXO-1004 pagination contract.
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .clamp(1, 1000) as usize;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let sort_by = args
            .get("sort_by")
            .and_then(|v| v.as_str())
            .unwrap_or("severity");

        // REQ-AXO-91516 (MIL-AXO-019 vague 1d) — RAM-first via
        // IstGraphView. `layer_violations` (vague 1c, commit `787ac797`)
        // scans the in-memory CSR for edges that cross the user-declared
        // forbidden boundary in O(N + M). Falls back to a structured
        // not_warm envelope when the snapshot for `project` is cold so
        // the LLM sees the truth instead of the previous silent stub.
        let view = crate::ist_snapshot::process_view();
        let project_for_graph = project.unwrap_or("");
        let ram_warm = !project_for_graph.is_empty() && view.is_warm(project_for_graph);

        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        if !ram_warm {
            surfaces_used.push("graph_ram_pending");
            surfaces_degraded.push("graph_ram_unavailable");
            let report = format!(
                "🛠️ **architectural_drift requires a warm IstGraph snapshot for project `{}`**\n\nSource layer: `{source_layer}`\nTarget layer: `{target_layer}`\n\nLoad the snapshot via `ist_snapshot_warm project={}` then retry. Pre-warm path uses `ist.symbol` + `ist.edge` (PG canonical).",
                project_for_graph,
                project_for_graph
            );
            return Some(json!({
                "content": [{ "type": "text", "text": report }],
                "data": {
                    "status": "warn_ram_cold",
                    "source_layer": source_layer,
                    "target_layer": target_layer,
                    "project": project_for_graph,
                    "surfaces_used": surfaces_used,
                    "surfaces_degraded": surfaces_degraded,
                    "total_available": 0u64,
                    "returned": 0u64,
                    "has_more": false,
                    "next_call_hint": {
                        "params": { "tool": "ist_snapshot_warm", "project": project_for_graph },
                        "reason": "warm the RAM snapshot before calling architectural_drift"
                    },
                }
            }));
        }

        // Resolve snapshot via cache_handle (view.is_warm guarantees Some).
        let snap = match view.cache_handle().get(project_for_graph) {
            Some(s) => s,
            None => {
                surfaces_used.push("graph_ram_pending");
                surfaces_degraded.push("graph_ram_unavailable");
                return Some(json!({
                    "content": [{ "type": "text", "text": "architectural_drift: snapshot race — RAM warm but absent at fetch; retry."  }],
                    "data": {
                        "status": "internal_race",
                        "surfaces_used": surfaces_used,
                        "surfaces_degraded": surfaces_degraded
                    }
                }));
            }
        };

        surfaces_used.push("graph_ram");
        let layer_def: Vec<(&str, u32)> = vec![(source_layer, 0), (target_layer, 1)];
        let mut violations = crate::ist_snapshot::algorithms::layer_violations(&snap, &layer_def);

        // sort_by selectors per GUI-AXO-1004 ; default "severity" =
        // edges where the layer gap is biggest, then alphabetic for
        // determinism.
        match sort_by {
            "alphabetical" => violations
                .sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str()))),
            _ => violations.sort_by(|a, b| {
                let sev_b = (b.3 as i64 - b.2 as i64).abs();
                let sev_a = (a.3 as i64 - a.2 as i64).abs();
                sev_b.cmp(&sev_a).then_with(|| a.0.cmp(&b.0))
            }),
        }

        let total_available = violations.len() as u64;
        let paginated: Vec<&(String, String, u32, u32)> =
            violations.iter().skip(offset).take(limit).collect();
        let returned = paginated.len() as u64;
        let has_more = (offset as u64).saturating_add(returned) < total_available;

        let display_rows: Vec<Vec<Value>> = paginated
            .iter()
            .map(|(src, tgt, src_l, tgt_l)| {
                vec![
                    Value::String(src.clone()),
                    Value::String(tgt.clone()),
                    Value::String(format!("{src_l}")),
                    Value::String(format!("{tgt_l}")),
                ]
            })
            .collect();
        let display_json =
            serde_json::to_string(&display_rows).unwrap_or_else(|_| "[]".to_string());

        let report = if display_rows.is_empty() {
            format!(
                "✅ No architectural drift detected from `{source_layer}` → `{target_layer}` in project `{project_for_graph}`.",
            )
        } else {
            format!(
                "### 🚨 Architectural drift `{source_layer}` → `{target_layer}` ({total_available} violations)\n\n{}",
                format_table_from_json(
                    &display_json,
                    &["Source", "Target", "Src Layer", "Tgt Layer"]
                )
            )
        };

        let next_call_hint = if has_more {
            json!({
                "params": {
                    "source_layer": source_layer,
                    "target_layer": target_layer,
                    "project": project_for_graph,
                    "limit": limit,
                    "offset": offset + paginated.len(),
                    "sort_by": sort_by
                },
                "reason": "more violations available; bump offset to paginate"
            })
        } else if display_rows.is_empty() {
            json!({
                "params": { "follow_up": "architectural_drift source_layer=<other> target_layer=<other>" },
                "reason": "boundary is clean ; consider checking adjacent layers"
            })
        } else {
            json!({
                "params": { "follow_up": "impact symbol=<first-violation-source>" },
                "reason": "drill into a specific drift edge to assess blast radius"
            })
        };

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "source_layer": source_layer,
                "target_layer": target_layer,
                "project": project_for_graph,
                "data": display_rows,
                "total_available": total_available,
                "returned": returned,
                "has_more": has_more,
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "truncation_strategy": "topk_then_paginate",
                "truncation_applied": false,
                "sort_by": sort_by,
                "next_call_hint": next_call_hint
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
