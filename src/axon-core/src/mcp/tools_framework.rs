use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::format::{evidence_by_mode, format_standard_contract};
use super::tools_framework_change_safety::{
    change_safety_operator_guidance, summarize_change_safety,
};
use super::tools_framework_surface::axon_mcp_surface_diagnostics_impl;
use super::tools_framework_validation::linked_validations_from_intentions;
use super::McpServer;

type FrameworkCache = HashMap<String, (i64, Value)>;

static ANOMALIES_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static CONCEPTION_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static STATUS_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();
static WHY_CACHE: OnceLock<Mutex<FrameworkCache>> = OnceLock::new();

#[allow(dead_code)]
const FRAMEWORK_CACHE_TTL_MS: i64 = 5_000;
pub(super) const CONCEPTION_CACHE_TTL_MS: i64 = 60_000;
pub(super) const STATUS_CACHE_TTL_MS: i64 = 180_000;
pub(super) const STATUS_FULL_CACHE_TTL_MS: i64 = 1_000;
pub(super) const WHY_CACHE_TTL_MS: i64 = 180_000;
pub(super) const ANOMALIES_CACHE_TTL_MS: i64 = 180_000;

impl McpServer {
    pub(super) fn anomalies_cache() -> &'static Mutex<FrameworkCache> {
        ANOMALIES_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn conception_cache() -> &'static Mutex<FrameworkCache> {
        CONCEPTION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn status_cache() -> &'static Mutex<FrameworkCache> {
        STATUS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn why_cache() -> &'static Mutex<FrameworkCache> {
        WHY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn axon_mcp_surface_diagnostics(&self, _args: &Value) -> Option<Value> {
        Some(axon_mcp_surface_diagnostics_impl())
    }

    pub(super) fn compact_runtime_path(path: String) -> String {
        let current_dir = std::env::current_dir().ok();
        let current_dir = current_dir.as_ref().map(|dir| dir.as_path());
        let as_path = PathBuf::from(&path);
        if let Some(root) = current_dir {
            if let Ok(stripped) = as_path.strip_prefix(root) {
                let display = stripped.display().to_string();
                return if display.is_empty() {
                    ".".to_string()
                } else {
                    format!("./{}", display)
                };
            }
        }
        if let Some(name) = as_path.file_name().and_then(|value| value.to_str()) {
            return format!("<{}>", name);
        }
        path
    }

    pub(crate) fn canonical_sources_snapshot() -> Value {
        json!({
            "soll_export": {
                "role": "canonical_intention_backup",
                "reimportable": true
            }
        })
    }

    pub(super) fn parse_soll_vision_entry(raw: &str) -> Value {
        let parts = raw.splitn(4, '|').collect::<Vec<_>>();
        json!({
            "id": parts.first().copied().unwrap_or("unknown"),
            "title": parts.get(1).copied().unwrap_or("unknown"),
            "status": parts.get(2).copied().unwrap_or("unknown"),
            "description": parts.get(3).copied().unwrap_or("unavailable"),
            "source": "SOLL"
        })
    }

    pub(super) fn summarize_why_response(args: &Value, response: &mut Value) {
        let Some(data) = response
            .get_mut("data")
            .and_then(|value| value.as_object_mut())
        else {
            return;
        };
        let planner = data.get("planner").cloned().unwrap_or_else(|| json!({}));
        let packet = data.get("packet").cloned().unwrap_or_else(|| json!({}));
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let brief_mode = mode == "brief";

        let mut governing_requirements = packet
            .get("governing_requirements")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut governing_decisions = packet
            .get("governing_decisions")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut supporting_guidelines = packet
            .get("supporting_guidelines")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut supporting_docs = packet
            .get("supporting_docs")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut direct_code_evidence = packet
            .get("direct_code_evidence")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut supporting_code_context = packet
            .get("supporting_code_context")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut evidence_states = packet
            .get("evidence_states")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let rationale_quality = packet.get("rationale_quality").cloned().unwrap_or_else(|| {
            json!({
                "level": "weak",
                "confidence_reason": "no structured rationale quality available",
                "automation_contract": "informational_only"
            })
        });
        let mut direct_evidence = packet
            .get("direct_evidence")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut supporting_chunks = packet
            .get("supporting_chunks")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut structural_neighbors = packet
            .get("structural_neighbors")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let missing_evidence = packet
            .get("missing_evidence")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let excluded_because = packet
            .get("excluded_because")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let confidence = packet
            .get("confidence")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let mut relevant_soll_entities = governing_requirements
            .iter()
            .chain(governing_decisions.iter())
            .chain(supporting_guidelines.iter())
            .cloned()
            .collect::<Vec<_>>();

        if brief_mode {
            governing_requirements.truncate(3);
            governing_decisions.truncate(3);
            supporting_guidelines.truncate(3);
            supporting_docs.truncate(3);
            direct_code_evidence.truncate(3);
            supporting_code_context.truncate(3);
            evidence_states.truncate(4);
            relevant_soll_entities.truncate(4);
            direct_evidence.truncate(3);
            supporting_chunks.truncate(3);
            structural_neighbors.truncate(3);
        }

        let linked_validations = linked_validations_from_intentions(&relevant_soll_entities);

        let summary = json!({
            "target": {
                "question": args.get("question").and_then(|value| value.as_str()),
                "symbol": args.get("symbol").and_then(|value| value.as_str()),
                "project": args.get("project").and_then(|value| value.as_str()).unwrap_or("*")
            },
            "route": planner.get("route").and_then(|value| value.as_str()).unwrap_or("unknown"),
            "linked_intentions": relevant_soll_entities,
            "linked_validations": linked_validations,
            "governing_requirements": governing_requirements,
            "governing_decisions": governing_decisions,
            "supporting_guidelines": supporting_guidelines,
            "supporting_docs": supporting_docs,
            "direct_code_evidence": direct_code_evidence,
            "supporting_code_context": supporting_code_context,
            "evidence_states": evidence_states,
            "rationale_quality": rationale_quality,
            "supporting_artifacts": {
                "direct_evidence": direct_evidence,
                "supporting_chunks": supporting_chunks,
                "structural_neighbors": structural_neighbors
            },
            "missing_evidence": missing_evidence,
            "confidence": confidence,
            "provenance": "aggregated",
            "evidence_sources": ["retrieve_context", "soll_query_context", "traceability"],
            "safe_to_act": false,
            "needs_human_confirmation": true,
            "degradation": {
                "service_pressure": planner.get("service_pressure").cloned().unwrap_or(Value::Null),
                "degraded_reason": planner.get("degraded_reason").cloned().unwrap_or(Value::Null),
                "excluded_because": excluded_because
            },
            "canonical_sources": Self::canonical_sources_snapshot()
        });
        data.insert("why".to_string(), summary);
        if brief_mode {
            data.remove("planner");
            data.remove("packet");
        }
    }

    pub(crate) fn axon_pre_flight_check(&self, args: &Value) -> Option<Value> {
        let diff_paths = args.get("diff_paths")?.as_array()?.clone();
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("pre-flight-check");
        let incremental = args
            .get("incremental")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        // REQ-AXO-901754 — SRS slice 4: detect legacy proximity across
        // all diff_paths. Non-blocking warning injected into the response.
        let legacy_proximity_value = self.detect_diff_paths_legacy_proximity(&diff_paths);

        // REQ-AXO-902032 (N4) — tech-debt residue warning. If an edited file is
        // a known HAS_REMNANT of an active TechnologyMigration, surface the
        // migration + policy so the LLM resolves (or consciously keeps) the
        // residue instead of re-discovering it by accident. Non-blocking.
        let tech_debt_residue_value = {
            let paths: Vec<String> = diff_paths
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let hits = self.migrations_with_remnant_path(&paths);
            if hits.is_empty() {
                None
            } else {
                Some(json!({
                    "hits": hits,
                    "warning": "diff touches files flagged as residue of an incomplete technology migration — replace, document the hybrid policy, or run `tech_debt_inventory` for the full set"
                }))
            }
        };

        if !incremental {
            let mut response = self.axon_commit_work(&json!({
                "diff_paths": diff_paths,
                "message": message,
                "project_code": args.get("project_code"),
                "project_path": args.get("project_path"),
                "dry_run": true
            }))?;
            if let Some(data) = response.get_mut("data") {
                if let Some(lp) = &legacy_proximity_value {
                    data["legacy_proximity"] = lp.clone();
                }
                if let Some(td) = &tech_debt_residue_value {
                    data["tech_debt_residue"] = td.clone();
                }
            }
            return Some(response);
        }

        // REQ-AXO-145 — per-file incremental dry-run. Re-runs axon_commit_work
        // for each diff_path individually so an LLM authoring N files
        // sequentially detects a TDD-gate failure on file 1 without first
        // authoring files 2..N.
        let mut per_file = serde_json::Map::new();
        let mut total_violations = 0usize;
        let mut failing_files = 0usize;
        let mut first_failing_path: Option<String> = None;
        for path_value in diff_paths.iter() {
            let Some(path_str) = path_value.as_str() else {
                continue;
            };
            let result = self.axon_commit_work(&json!({
                "diff_paths": [path_value.clone()],
                "message": message,
                "project_code": args.get("project_code"),
                "project_path": args.get("project_path"),
                "dry_run": true
            }));
            let mut entry = json!({ "ok": true, "violations": [] });
            if let Some(value) = result.as_ref() {
                if value
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    let violations = value
                        .pointer("/data/violations")
                        .cloned()
                        .unwrap_or_else(|| json!([]));
                    let count = violations.as_array().map(|a| a.len()).unwrap_or(0);
                    total_violations += count;
                    failing_files += 1;
                    if first_failing_path.is_none() {
                        first_failing_path = Some(path_str.to_string());
                    }
                    entry = json!({ "ok": false, "violations": violations });
                }
            }
            per_file.insert(path_str.to_string(), entry);
        }

        let summary_text = if total_violations == 0 {
            format!(
                "Validation passed (Dry Run, incremental). {} file(s) checked individually.",
                per_file.len()
            )
        } else {
            format!(
                "Incremental dry run found {} violation(s) across {} file(s). First failing path: {}",
                total_violations,
                failing_files,
                first_failing_path.as_deref().unwrap_or("?")
            )
        };

        let mut response = json!({
            "content": [{ "type": "text", "text": summary_text }],
            "isError": total_violations > 0,
            "data": {
                "incremental": true,
                "files_checked": per_file.len(),
                "failing_files": failing_files,
                "total_violations": total_violations,
                "first_failing_path": first_failing_path,
                "per_file_violations": per_file,
            }
        });
        if let Some(lp) = legacy_proximity_value {
            response["data"]["legacy_proximity"] = lp;
        }
        if let Some(td) = tech_debt_residue_value {
            response["data"]["tech_debt_residue"] = td;
        }
        Some(response)
    }

    /// REQ-AXO-239 — session-close validation surface, symmetric to
    /// `axon_pre_flight_check`. Runs the structured checks an operator would
    /// otherwise reason through by hand before a handoff (GUI-PRO-028) and
    /// returns a pass/warn/fail verdict per check plus an overall roll-up.
    /// Read-only: reuses the existing `soll_validate` / `status` primitives and
    /// `git` porcelain; never mutates.
    pub(crate) fn axon_handoff_check(&self, args: &Value) -> Option<Value> {
        let project_dir = args.get("project_path").and_then(|v| v.as_str());
        let git = |a: &[&str]| -> Option<String> {
            let mut c = std::process::Command::new("git");
            if let Some(d) = project_dir {
                c.current_dir(d);
            }
            c.args(a)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let mut checks: Vec<Value> = Vec::new();
        let mut warns = 0usize;
        let mut fails = 0usize;

        // 1. working tree clean
        let dirty = git(&["status", "--porcelain"]).unwrap_or_default();
        let clean = dirty.is_empty();
        if !clean {
            warns += 1;
        }
        checks.push(json!({
            "check": "git_working_tree",
            "status": if clean { "pass" } else { "warn" },
            "detail": if clean { "working tree clean".to_string() }
                      else { format!("{} uncommitted path(s)", dirty.lines().count()) },
            "remediation": if clean { "" } else { "commit via axon_commit_work or stash before handoff" }
        }));

        // 2. branch pushed (no upstream => undetermined, treated as pass)
        let unpushed = git(&["log", "--oneline", "@{u}.."]).unwrap_or_default();
        let n_unpushed = if unpushed.is_empty() { 0 } else { unpushed.lines().count() };
        if n_unpushed > 0 {
            warns += 1;
        }
        checks.push(json!({
            "check": "branch_pushed",
            "status": if n_unpushed == 0 { "pass" } else { "warn" },
            "detail": format!("{} unpushed commit(s)", n_unpushed),
            "remediation": if n_unpushed > 0 { "push to origin (operator-gated)" } else { "" }
        }));

        // 3. SOLL coherence (reuse soll_validate)
        let soll = self.axon_validate_soll(&json!({ "project_code": args.get("project_code") }));
        let soll_ok = soll
            .as_ref()
            .map(|r| {
                serde_json::to_string(r)
                    .unwrap_or_default()
                    .contains("0 minimal coherence violation")
            })
            .unwrap_or(false);
        if !soll_ok {
            warns += 1;
        }
        checks.push(json!({
            "check": "soll_validate",
            "status": if soll_ok { "pass" } else { "warn" },
            "detail": if soll_ok { "0 SOLL coherence violations" } else { "SOLL violations present or validate unavailable" },
            "remediation": if soll_ok { "" } else { "run soll_validate and resolve / /curate-soll" }
        }));

        // 4. live runtime health (reuse status brief)
        let st = self.axon_status(&json!({ "mode": "brief" }));
        let st_ok = st
            .as_ref()
            .map(|r| {
                !serde_json::to_string(r)
                    .unwrap_or_default()
                    .contains("\"isError\":true")
            })
            .unwrap_or(false);
        if !st_ok {
            fails += 1;
        }
        checks.push(json!({
            "check": "live_runtime",
            "status": if st_ok { "pass" } else { "fail" },
            "detail": if st_ok { "runtime status reachable" } else { "runtime status unreachable / error" },
            "remediation": if st_ok { "" } else { "./scripts/axon-live status" }
        }));

        let overall = if fails > 0 {
            "fail"
        } else if warns > 0 {
            "warn"
        } else {
            "pass"
        };
        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Handoff check: {} — {} check(s), {} warn, {} fail",
                    overall.to_uppercase(), checks.len(), warns, fails
                )
            }],
            "data": {
                "status": "ok",
                "overall": overall,
                "checks": checks,
                "manual_reminders": [
                    "GUI-PRO-028 manual steps not auto-checked: update the rolling session_pointer (CPT-{P}-052), prune boot docs, audit docs/working-notes, run `cargo test --lib` if runtime logic changed"
                ],
                "follow_up_tools": ["axon_commit_work", "soll_validate", "status"]
            }
        }))
    }

    /// REQ-AXO-901754 — scan diff_paths for legacy SOLL proximity.
    fn detect_diff_paths_legacy_proximity(&self, diff_paths: &[Value]) -> Option<Value> {
        use std::collections::HashSet;
        let project_code = self.startup_project_code()?;
        let snapshot = self.soll_cache().snapshot(&project_code).ok()?;

        let mut all_nodes: Vec<super::tools_srs::LegacyNode> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for path_val in diff_paths {
            let Some(path) = path_val.as_str() else {
                continue;
            };
            if let Some(prox) = super::tools_srs::detect_legacy_proximity(path, &snapshot) {
                for node in prox.nodes {
                    if seen.insert(node.id.clone()) {
                        all_nodes.push(node);
                    }
                }
            }
        }

        if all_nodes.is_empty() {
            return None;
        }

        let direction = all_nodes
            .first()
            .map(|n| n.strategy.direction_hint())
            .unwrap_or("review legacy linkage")
            .to_string();
        let confidence = if all_nodes.iter().all(|n| n.successor.is_some()) {
            "high"
        } else {
            "medium"
        };
        Some(json!({
            "nodes": all_nodes.iter().map(|n| json!({
                "id": n.id,
                "strategy": n.strategy,
                "successor": n.successor,
                "superseded_at": n.superseded_at,
            })).collect::<Vec<_>>(),
            "direction": direction,
            "confidence": confidence,
            "warning": "diff touches files linked to superseded SOLL nodes"
        }))
    }

    pub(crate) fn axon_status(&self, args: &Value) -> Option<Value> {
        self.axon_status_impl(args)
    }

    pub(crate) fn axon_project_status(&self, args: &Value) -> Option<Value> {
        self.axon_project_status_impl(args)
    }

    pub(crate) fn axon_why(&self, args: &Value) -> Option<Value> {
        self.axon_why_impl(args)
    }

    pub(crate) fn axon_path(&self, args: &Value) -> Option<Value> {
        self.axon_path_impl(args)
    }

    pub(crate) fn axon_anomalies(&self, args: &Value) -> Option<Value> {
        self.axon_anomalies_impl(args)
    }

    pub(crate) fn axon_snapshot_history(&self, args: &Value) -> Option<Value> {
        Some(self.axon_snapshot_history_impl(args))
    }

    pub(crate) fn axon_snapshot_diff(&self, args: &Value) -> Option<Value> {
        Some(self.axon_snapshot_diff_impl(args))
    }

    pub(crate) fn axon_conception_view(&self, args: &Value) -> Option<Value> {
        let explicit_project_code = args.get("project_code").and_then(|value| value.as_str());
        // REQ-AXO-043 — when project_code is supplied but unregistered,
        // surface the wrong_project_scope contract via the shared helper
        // instead of returning a "Status: ok" view with zero modules
        // (which the LLM caller would misread as "no architecture exists
        // for this project" rather than "this project_code is invalid").
        if let Some(code) = explicit_project_code {
            if self.resolve_project_code(code).is_err() {
                return Some(self.wrong_project_scope_response(code, "conception_view"));
            }
        }
        let project_code = explicit_project_code.unwrap_or("AXO");
        let mode = args
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("brief");
        let conception = self.cached_conception_view(project_code);
        let boundary_violations: Vec<Value> = if mode == "brief" {
            Vec::new()
        } else {
            // Decoupled: We no longer fetch anomalies inline to avoid timeouts.
            // The operator must call 'anomalies' directly if needed.
            Vec::new()
        };
        let evidence = format!(
            "**Project:** `{}`\n\
**Modules / Interfaces / Contracts / Flows:** {} / {} / {} / {}\n\
**Boundary violations:** {}\n",
            project_code,
            conception
                .get("module_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("interface_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("contract_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            conception
                .get("flow_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            boundary_violations.len()
        );
        let report = format!(
            "## 🧱 Conception View\n\n{}",
            format_standard_contract(
                "ok",
                "derived conception view assembled",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, Some(mode)),
                &[
                    "use `why` for rationale",
                    "use `path` to inspect a flow in detail"
                ],
                "medium",
            )
        );
        // REQ-AXO-901755 — SRS slice 5: transitions[] section per
        // superseded node with strategy and IST residual count.
        let transitions: Vec<Value> = {
            let snapshot = self.soll_cache().snapshot(project_code).ok();
            snapshot
                .map(|snap| {
                    super::tools_srs::detect_all_superseded_proximity(&snap)
                        .into_iter()
                        .map(|n| {
                            let residual = super::tools_srs::residual_count_for(&n.id, &snap);
                            json!({
                                "id": n.id,
                                "strategy": n.strategy,
                                "successor": n.successor,
                                "superseded_at": n.superseded_at,
                                "ist_residual_count": residual,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "project_code": project_code,
                "mode": mode,
                "module_count": conception.get("module_count").cloned().unwrap_or_else(|| json!(0)),
                "modules": conception.get("modules").cloned().unwrap_or_else(|| json!([])),
                "interface_count": conception.get("interface_count").cloned().unwrap_or_else(|| json!(0)),
                "interfaces": conception.get("interfaces").cloned().unwrap_or_else(|| json!([])),
                "contract_count": conception.get("contract_count").cloned().unwrap_or_else(|| json!(0)),
                "contracts": conception.get("contracts").cloned().unwrap_or_else(|| json!([])),
                "flow_count": conception.get("flow_count").cloned().unwrap_or_else(|| json!(0)),
                "flows": conception.get("flows").cloned().unwrap_or_else(|| json!([])),
                "boundaries": conception.get("boundaries").cloned().unwrap_or_else(|| json!([])),
                "owners": conception.get("owners").cloned().unwrap_or_else(|| json!([])),
                "suspected_boundary_violation_count": boundary_violations.len(),
                "suspected_boundary_violations": boundary_violations,
                "transitions": transitions,
                "provenance": "derived_read_only_view",
                "confidence": conception.get("confidence").cloned().unwrap_or_else(|| json!("medium")),
                "evidence_sources": ["File", "Symbol", "CALLS", "CONTAINS"],
                "safe_to_act": false,
                "needs_human_confirmation": true,
                "surfaces_used": ["graph_pg", "soll_pg"],
                "total_available": conception.get("module_count").and_then(Value::as_u64).unwrap_or(0)
                    + conception.get("interface_count").and_then(Value::as_u64).unwrap_or(0)
                    + conception.get("contract_count").and_then(Value::as_u64).unwrap_or(0)
                    + conception.get("flow_count").and_then(Value::as_u64).unwrap_or(0),
                "next_call_hint": "why symbol=<flow-or-module-id> to inspect rationale"
            }
        }))
    }

    pub(crate) fn axon_change_safety(&self, args: &Value) -> Option<Value> {
        let explicit_project_code = args.get("project_code").and_then(|value| value.as_str());
        // REQ-AXO-043 — when project_code is supplied but unregistered,
        // surface the wrong_project_scope contract via the shared helper
        // instead of returning Safety=unsafe with confidence=low (the
        // LLM caller would misread that as "this symbol is unsafe to
        // change" rather than "this project_code is invalid").
        if let Some(code) = explicit_project_code {
            if self.resolve_project_code(code).is_err() {
                return Some(self.wrong_project_scope_response(code, "change_safety"));
            }
        }
        let project_code = explicit_project_code.unwrap_or("AXO");
        let target = args.get("target")?.as_str()?.trim();
        if target.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "change_safety requires a non-empty `target`" }],
                "isError": true,
                "data": {
                    "status": "input_invalid",
                    "parameter_repair": {
                        "invalid_field": "target",
                        "follow_up_tools": ["help", "query"],
                        "hint": "supply a non-empty `target` (symbol id or file path); use `query` to discover indexed targets"
                    }
                }
            }));
        }
        let target_type = args
            .get("target_type")
            .and_then(|value| value.as_str())
            .unwrap_or("symbol");
        let resolved_symbol_id = if target_type == "symbol" {
            self.resolve_scoped_symbol_id_canonical(target, Some(project_code))
        } else {
            None
        };
        let validation_signals = match target_type {
            "intent" => self.intent_validation_signals(project_code, target),
            "symbol" => {
                // REQ-AXO-901952 (gap B) — both signals RAM-only. `tested` from
                // IST NodeFlags (the loader carries it since REQ-AXO-91485, so the
                // historical "RAM doesn't carry tested" claim was stale); resolved
                // via the canonical symbol id (conservative `false` when the symbol
                // can't be resolved / the snapshot is cold). No PG `Symbol` count.
                let tested = resolved_symbol_id
                    .as_deref()
                    .filter(|_| self.ensure_ram_snapshot_warm(project_code))
                    .and_then(|id| {
                        crate::ist_snapshot::process_view().node_tested(project_code, id)
                    })
                    .unwrap_or(false);
                // Traceability link count from the SOLL RAM snapshot, matching the
                // legacy `artifact_type='Symbol' AND artifact_ref IN (name,id)`.
                // No PG `soll.Traceability` count.
                let traceability_links = self
                    .soll_cache()
                    .snapshot(project_code)
                    .ok()
                    .map(|snap| {
                        let mut refs: Vec<&str> = vec![target];
                        if let Some(id) = resolved_symbol_id.as_deref() {
                            if id != target {
                                refs.push(id);
                            }
                        }
                        snap.traceability_count_for_artifact("Symbol", &refs) as i64
                    })
                    .unwrap_or(0);
                json!({
                    "tested": tested,
                    "traceability_links": traceability_links,
                    "validation_nodes": 0,
                    "verifies_edges": 0
                })
            }
            _ => self.symbol_validation_signals(project_code, target),
        };
        let coverage_signals = json!({
            "tested": validation_signals.get("tested").cloned().unwrap_or_else(|| json!(false))
        });
        let traceability_signals = json!({
            "traceability_links": validation_signals
                .get("traceability_links")
                .cloned()
                .unwrap_or_else(|| json!(0))
        });
        let (change_safety, reasoning, recommended_guardrails, confidence) =
            summarize_change_safety(
                &coverage_signals,
                &traceability_signals,
                &validation_signals,
            );
        let operator_guidance = change_safety_operator_guidance(
            change_safety,
            &coverage_signals,
            &traceability_signals,
            &validation_signals,
        );
        let (safe_to_act, needs_human_confirmation) =
            (change_safety == "safe", change_safety != "safe");

        let evidence = format!(
            "**Target:** `{}` ({})\n\
**Safety:** `{}`\n\
**Traceability links:** {}\n\
**Tested:** {}\n",
            target,
            target_type,
            change_safety,
            traceability_signals
                .get("traceability_links")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            coverage_signals
                .get("tested")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        );
        let report = format!(
            "## 🛡️ Change Safety\n\n{}",
            format_standard_contract(
                "ok",
                "derived change-safety summary assembled",
                &format!("project:{}", project_code),
                &evidence_by_mode(&evidence, args.get("mode").and_then(|value| value.as_str())),
                &[
                    "run `impact` before mutation",
                    "use `why` to confirm intent remains valid"
                ],
                confidence,
            )
        );
        // REQ-AXO-91514 — tri-modal envelope per GUI-AXO-1003. `change_safety`
        // is an impact-context tool that joins two surfaces : the IST
        // Symbol index (coverage signal `tested`) and the SOLL Traceability
        // table (link count). REQ-AXO-901952 (gap B) moved BOTH onto RAM — the
        // IST snapshot carries `tested` in NodeFlags and the SOLL snapshot
        // carries the Traceability rows — so `surfaces_used` names the logical
        // provenance (`["symbol_index","soll_traceability"]`), now served from
        // the in-memory snapshots, not PG. No results[] array : the response
        // shape is already a single
        // verdict (`change_safety`+`reasoning`) ; adding a parallel
        // results[] would inflate bench precision denominators without
        // helping LLM consumers (same logic as inspect REQ-AXO-91509).
        // REQ-AXO-901753 — SRS slice 3: legacy proximity for change_safety target.
        let legacy_proximity_value = {
            let snapshot = self.soll_cache().snapshot(project_code).ok();
            snapshot.and_then(|snap| {
                super::tools_srs::detect_legacy_proximity(target, &snap).map(|prox| {
                    json!({
                        "nodes": prox.nodes.iter().map(|n| json!({
                            "id": n.id,
                            "strategy": n.strategy,
                            "successor": n.successor,
                            "superseded_at": n.superseded_at,
                        })).collect::<Vec<_>>(),
                        "direction": prox.direction,
                        "confidence": prox.confidence,
                    })
                })
            })
        };

        let mut response = json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "surfaces_used": ["symbol_index", "soll_traceability"],
                "surfaces_degraded": [],
                "total_available": 1,
                "next_call_hint": format!("impact symbol={target}"),
                "pagination": {
                    "offset": 0,
                    "limit": 1,
                    "next_offset": Value::Null,
                },
                "project_code": project_code,
                "target": target,
                "target_type": target_type,
                "coverage_signals": coverage_signals,
                "traceability_signals": traceability_signals,
                "validation_signals": validation_signals,
                "change_safety": change_safety,
                "reasoning": reasoning,
                "recommended_guardrails": recommended_guardrails,
                "operator_guidance": operator_guidance,
                "provenance": "aggregated",
                "confidence": confidence,
                "evidence_sources": ["Symbol", "soll.Traceability", "soll.Node", "soll.Edge"],
                "safe_to_act": safe_to_act,
                "needs_human_confirmation": needs_human_confirmation
            }
        });
        if let Some(lp) = legacy_proximity_value {
            response["data"]["legacy_proximity"] = lp;
        }
        Some(response)
    }
}
