// Copyright (c) Didier Stadelmann. All rights reserved.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use super::format::{evidence_by_mode, format_standard_contract, format_table_from_json};
use super::McpServer;
use crate::ist_snapshot::{process_view, RelationType};

#[allow(dead_code)]
type ImpactCache = BTreeMap<String, (i64, Value)>;

#[allow(dead_code)]
static IMPACT_CACHE: OnceLock<Mutex<ImpactCache>> = OnceLock::new();

#[allow(dead_code)]
const IMPACT_CACHE_TTL_MS: i64 = 60_000;

impl McpServer {
    #[cfg(not(test))]
    fn impact_cache() -> &'static Mutex<ImpactCache> {
        IMPACT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    #[cfg(not(test))]
    fn read_impact_cache(key: &str, now_ms: i64) -> Option<Value> {
        let guard = Self::impact_cache().lock().ok()?;
        let (stored_at, value) = guard.get(key)?;
        if now_ms.saturating_sub(*stored_at) > IMPACT_CACHE_TTL_MS {
            return None;
        }
        Some(value.clone())
    }

    #[cfg(test)]
    fn read_impact_cache(_key: &str, _now_ms: i64) -> Option<Value> {
        None
    }

    #[cfg(not(test))]
    fn write_impact_cache(key: String, now_ms: i64, value: &Value) {
        if let Ok(mut guard) = Self::impact_cache().lock() {
            guard.insert(key, (now_ms, value.clone()));
        }
    }

    #[cfg(test)]
    fn write_impact_cache(_key: String, _now_ms: i64, _value: &Value) {}

    fn resolve_scoped_symbol_id(&self, symbol: &str, project: Option<&str>) -> Option<String> {
        self.resolve_scoped_symbol_id_canonical(symbol, project)
    }

    fn suggest_scoped_symbols(&self, symbol: &str, project: Option<&str>, limit: usize) -> String {
        self.suggest_scoped_symbols_canonical(symbol, project, limit)
    }

    fn build_local_projection_section(
        &self,
        _symbol: &str,
        anchor: &str,
        depth: u64,
        project: Option<&str>,
    ) -> Option<String> {
        let radius = depth.clamp(1, 2);
        let columns = [
            "Target Type",
            "Target ID",
            "Link Type",
            "Distance",
            "Label",
            "URI",
        ];

        // REQ-AXO-901884 / feedback_trimodal_use_ram_graph_not_pg — RAM-first
        // (PIL-AXO-9002): when the per-project CSR is warm, derive the local
        // neighborhood (forward ∪ reverse reach) from the in-memory graph. The
        // PG `query_graph_projection` (ist.impact + ist.callers_of SQL) is the
        // degraded cold/unscoped fallback ONLY. RAM rows carry target_id as
        // label + empty uri — name/file enrichment is a PG-only join — matching
        // the structural_neighbors RAM contract (tools_context.rs, edge_kind
        // "ram_csr").
        // REQ-AXO-901952 — RAM-only neighborhood projection. Derive the
        // project from the anchor's metadata when unscoped ; a cold cache
        // yields no section (this enrichment is best-effort, never a PG
        // fallback).
        let effective = match project {
            Some(p) => Some(p.to_string()),
            None => self.symbol_project_code(anchor),
        };
        let p = effective.as_deref()?;
        if !self.ensure_ram_snapshot_warm(p) {
            return None;
        }
        let view = process_view();
        let cap = 200usize;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut ids: Vec<String> = Vec::new();
        for reach in [
            view.forward_at_radius(p, anchor, radius as u32, cap, &[]),
            view.reverse_at_radius(p, anchor, radius as u32, cap, &[]),
        ]
        .into_iter()
        .flatten()
        {
            for id in reach {
                if id != anchor && seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
        }
        if ids.len() <= 1 {
            return None;
        }
        let json_rows: Vec<Vec<Value>> = ids
            .into_iter()
            .map(|id| {
                vec![
                    Value::String("symbol".to_string()),
                    Value::String(id.clone()),
                    Value::String("ram_csr".to_string()),
                    Value::Number(0.into()),
                    Value::String(id),
                    Value::String(String::new()),
                ]
            })
            .collect();
        let projection_res = serde_json::to_string(&json_rows).ok()?;
        Some(format!(
            "\n\n### Derived Local Projection\n\n**Status:** derived neighborhood view (RAM CSR), useful for local context; does not replace the canonical `CALLS` truth.\n\n{}",
            format_table_from_json(&projection_res, &columns)
        ))
    }

    pub(crate) fn axon_impact(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let cache_key = format!(
            "{}|{}|{}|{}",
            project.unwrap_or("*"),
            symbol,
            depth,
            mode.unwrap_or("brief")
        );
        if let Some(cached) = Self::read_impact_cache(&cache_key, now_ms) {
            return Some(cached);
        }
        let Some(target_id) = self.resolve_scoped_symbol_id(symbol, project) else {
            return self.axon_impact_without_calls(symbol, project, depth);
        };

        // REQ-AXO-91512 — RAM-first via IstGraphView (PIL-AXO-9002,
        // feedback_trimodal_use_ram_graph_not_pg). When the cache is
        // warm, the reverse-traversal runs entirely in RAM ; the PG
        // path (`impact_callers_via_ist_edge` → REQ-AXO-296 SQL fn)
        // is the degraded fallback for cold cache or unscoped queries.
        // Inferred `bridge_name` edges are a PG-text-matching artifact
        // not represented in the IST snapshot ; when RAM serves the
        // query, `inferred_bridge_edges` is reported as 0 and a
        // `surfaces_degraded` hint flags the gap.
        // REQ-AXO-901952 — RAM is the SINGLE source for the caller traversal
        // (PIL-AXO-9002). Cold cache or an unscoped (project=None) query →
        // loud degraded error, never a PG fallback. The text-matching
        // `bridge_name` inference (PG-only, false-positive-prone) is dropped
        // by design : the structural RAM graph reports only real edges.
        // The RAM snapshot is per-project ; when the caller omits `project`,
        // derive it from the resolved symbol's metadata so an unscoped
        // (workspace-wide) impact still serves from RAM. The graph traversal
        // stays in RAM ; only this metadata lookup touches PG.
        let effective_project: Option<String> = match project {
            Some(p) => Some(p.to_string()),
            None => self.symbol_project_code(&target_id),
        };
        let ram_attempted = effective_project
            .as_deref()
            .map(|p| self.ensure_ram_snapshot_warm(p))
            .unwrap_or(false);
        if !ram_attempted {
            let why = if effective_project.is_none() {
                "impact could not resolve the symbol's project for the RAM IST snapshot ; pass an explicit `project` (REQ-AXO-901952, no PG fallback)"
            } else {
                "IST RAM snapshot is cold for this project and could not be warmed ; call `ist_snapshot_warm` then retry (REQ-AXO-901952, no PG fallback)"
            };
            return Some(Self::impact_ram_unavailable_error(symbol, project, depth, why));
        }
        let view = process_view();
        let surfaces_used: Vec<&'static str> = vec!["graph_ram"];
        let surfaces_degraded: Vec<&'static str> = Vec::new();
        let proj_key = effective_project.as_deref().unwrap_or("");
        let query_outcome: Result<String, anyhow::Error> =
            Ok(self.build_impact_rows_from_ram(&view, proj_key, &target_id, depth));

        match query_outcome {
            Ok(res) => {
                let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
                let mut impact_rows = BTreeMap::<String, (String, String, String)>::new();
                let mut impacted_symbol_ids = BTreeSet::<String>::new();
                let mut impacted_symbol_names = BTreeSet::<String>::new();
                let mut direct_edges = 0_i64;
                let mut nif_edges = 0_i64;

                for row in &rows {
                    let caller_id = row
                        .first()
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let edge_type = row.get(1).and_then(|v| v.as_str()).unwrap_or("unknown");
                    let origin = row
                        .get(2)
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let name = row
                        .get(3)
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string();
                    let kind = row
                        .get(4)
                        .and_then(|v| v.as_str())
                        .unwrap_or("-")
                        .to_string();

                    if !caller_id.is_empty() {
                        impacted_symbol_ids.insert(caller_id.clone());
                        impact_rows
                            .entry(caller_id)
                            .or_insert_with(|| (origin, name.clone(), kind));
                    }
                    if !name.is_empty() {
                        impacted_symbol_names.insert(name);
                    }
                    match edge_type {
                        "calls" => direct_edges += 1,
                        "calls_nif" => nif_edges += 1,
                        _ => {}
                    }
                }

                let impact_radius = impacted_symbol_ids.len() as i64;
                if rows.is_empty() && impact_radius == 0 {
                    return self.axon_impact_without_calls(symbol, project, depth);
                }

                let display_rows = impact_rows
                    .values()
                    .map(|(origin, name, kind)| json!([origin, name, kind]))
                    .collect::<Vec<_>>();
                let display_raw =
                    serde_json::to_string(&display_rows).unwrap_or_else(|_| "[]".to_string());
                let mut table = if display_rows.len() > 15 {
                    format!(
                        "_Report aggregated because {} symbols are impacted. Only major architectural impacts are detailed below._\n\n",
                        display_rows.len()
                    )
                } else {
                    format_table_from_json(
                        &display_raw,
                        &["File / Project", "Impacted Symbol", "Type"],
                    )
                };

                impacted_symbol_ids.insert(target_id.clone());
                impacted_symbol_names.insert(symbol.to_string());
                // REQ-AXO-901952 (gap C) — SOLL impact via the in-memory SOLL
                // snapshot instead of WITH RECURSIVE over soll.Traceability +
                // soll.Edge. Find the SOLL entry points whose Symbol
                // traceability matches an impacted symbol (id or name), then
                // BFS forward over the SOLL graph (depth < 10) collecting the
                // reachable intent nodes. Per-project scope (the RAM snapshot
                // is per-project) ; the impacted symbols all live in proj_key.
                // REQ-AXO-902043 — the symbol→governing-intent traversal is shared
                // with `fuse` (single source).
                let soll_rows = self.fused_governing_intent_ram(
                    proj_key,
                    &impacted_symbol_ids,
                    &impacted_symbol_names,
                );

                if !soll_rows.is_empty() {
                    table.push_str("\n### 🏛️ SOLL Impact (Architecture Compromise)\n\n| Entity | Type | Title |\n| --- | --- | --- |\n");
                    for row in soll_rows {
                        let id = row.first().and_then(|v| v.as_str()).unwrap_or("-");
                        let t = row.get(1).and_then(|v| v.as_str()).unwrap_or("-");
                        let title = row.get(2).and_then(|v| v.as_str()).unwrap_or("-");
                        table.push_str(&format!("| `{}` | `{}` | {} |\n", id, t, title));
                    }
                    table.push('\n');
                }

                let confidence_label = if direct_edges + nif_edges > 0 {
                    "high"
                } else {
                    "low"
                };

                let mut evidence = String::new();
                if let Some(note) = self.project_scope_truth_note(project) {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                if let Some(note) =
                    self.degraded_truth_note(self.degraded_symbol_count(symbol, project))
                {
                    evidence.push_str(&note);
                    evidence.push('\n');
                }
                evidence.push_str(&format!(
                    "**Impact Radius (depth {}):** {} components affected across the Lattice.\n\n",
                    depth, impact_radius
                ));
                evidence.push_str(&format!(
                    "**Coverage:** confidence={} (direct_calls={}, calls_nif={})\n\n",
                    confidence_label, direct_edges, nif_edges
                ));
                evidence.push_str(&table);
                if let Some(section) =
                    self.build_local_projection_section(symbol, &target_id, depth, project)
                {
                    evidence.push_str(&section);
                }
                let scope = project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string());
                let report = format!(
                    "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}",
                    symbol,
                    format_standard_contract(
                        "ok",
                        "impact analysis computed",
                        &scope,
                        &evidence_by_mode(&evidence, mode),
                        &[
                            "review top impacted symbols",
                            "run simulate_mutation before editing"
                        ],
                        confidence_label,
                    )
                );

                // REQ-AXO-901952 — the inferred `bridge_name` concept is
                // removed ; the RAM graph reports only real edges, so there is
                // no "partially inferred" blocking factor to raise.
                let blocking_factors = Vec::<Value>::new();
                let remediation_actions = blocking_factors
                    .iter()
                    .filter_map(|factor| {
                        factor
                            .get("recommended_action")
                            .and_then(|value| value.as_str())
                            .map(|value| Value::from(value.to_string()))
                    })
                    .collect::<Vec<_>>();
                let next_action = json!({
                    "kind": "simulate_mutation_before_editing",
                    "tool": "simulate_mutation",
                    "when": "now"
                });
                // REQ-AXO-901753 — SRS slice 3: legacy proximity from
                // target + impacted symbols.
                let legacy_proximity_value =
                    self.detect_impact_legacy_proximity(project, &target_id, &impacted_symbol_ids);

                // REQ-AXO-91512 — tri-modal envelope (GUI-AXO-1003).
                let total_available = impact_radius as u64;
                let mut response = json!({
                    "content": [{ "type": "text", "text": report }],
                    "data": {
                        "surfaces_used": surfaces_used,
                        "surfaces_degraded": surfaces_degraded,
                        "total_available": total_available,
                        "next_call_hint": format!("simulate_mutation symbol={symbol}"),
                        "pagination": {
                            "offset": 0,
                            "limit": total_available,
                            "next_offset": Value::Null,
                        },
                        "symbol": symbol,
                        "project": project,
                        "depth": depth,
                        "impact_radius": impact_radius,
                        "summary": {
                            "confidence": confidence_label,
                            "direct_edges": direct_edges,
                            "calls_nif_edges": nif_edges
                        },
                        "operator_guidance": {
                            "actionable_now": blocking_factors.is_empty(),
                            "blocking_factors": blocking_factors,
                            "remediation_actions": remediation_actions,
                            "follow_up_tools": ["simulate_mutation", "path", "why"],
                            "next_action": next_action
                        },
                        "next_action": next_action
                    }
                });
                if let Some(lp) = legacy_proximity_value {
                    response["data"]["legacy_proximity"] = lp;
                }
                Self::write_impact_cache(cache_key, now_ms, &response);
                Some(response)
            }
            Err(e) => Some(json!({
                "content": [{ "type": "text", "text": format!("Impact Analysis Error: {}", e) }],
                "isError": true,
                "data": {
                    "status": "internal_error",
                    "parameter_repair": {
                        "invalid_field": "symbol",
                        "follow_up_tools": ["inspect", "query", "status"],
                        "hint": "impact computation failed; verify the symbol resolves via `inspect` and the runtime is healthy via `status`"
                    },
                    "diagnostic_excerpt": e.to_string().chars().take(240).collect::<String>()
                }
            })),
        }
    }

    /// REQ-AXO-901753 — SRS slice 3: detect legacy proximity for
    /// the target symbol and its impacted dependents.
    fn detect_impact_legacy_proximity(
        &self,
        project: Option<&str>,
        target_id: &str,
        impacted_ids: &BTreeSet<String>,
    ) -> Option<Value> {
        use std::collections::HashSet;
        let project_code = project?;
        let snapshot = self.soll_cache().snapshot(project_code).ok()?;

        let mut all_nodes: Vec<super::tools_srs::LegacyNode> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        let mut check = |artifact: &str| {
            if let Some(prox) = super::tools_srs::detect_legacy_proximity(artifact, &snapshot) {
                for node in prox.nodes {
                    if seen.insert(node.id.clone()) {
                        all_nodes.push(node);
                    }
                }
            }
        };

        check(target_id);
        for id in impacted_ids {
            check(id);
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
        }))
    }

    /// REQ-AXO-901952 (gap C) / REQ-AXO-902043 — governing SOLL intent reachable
    /// from a set of impacted symbols, traversed over the RAM SOLL snapshot. Find
    /// the SOLL entry points whose Symbol traceability matches an impacted symbol
    /// (id or name), then BFS forward over the SOLL graph (depth < 10) collecting
    /// the reachable intent nodes. Per-project (the RAM snapshot is per-project).
    /// Returns `[id, type, title]` rows ordered by (type DESC, id ASC). Shared by
    /// `impact` (architecture-compromise section) and `fuse` (WHY-primary lead) so
    /// the symbol→intent traversal lives in exactly one place.
    pub(crate) fn fused_governing_intent_ram(
        &self,
        proj_key: &str,
        impacted_symbol_ids: &BTreeSet<String>,
        impacted_symbol_names: &BTreeSet<String>,
    ) -> Vec<Vec<Value>> {
        let mut soll_rows: Vec<Vec<Value>> = Vec::new();
        if let Ok(snap) = self.soll_cache().snapshot(proj_key) {
            use std::collections::{HashMap as StdHashMap, HashSet, VecDeque};
            let mut seen: HashSet<String> = HashSet::new();
            let mut depth_of: StdHashMap<String, u32> = StdHashMap::new();
            let mut queue: VecDeque<String> = VecDeque::new();
            for t in &snap.traceability {
                if t.artifact_type == "Symbol"
                    && (impacted_symbol_ids.contains(&t.artifact_ref)
                        || impacted_symbol_names.contains(&t.artifact_ref))
                    && seen.insert(t.soll_entity_id.clone())
                {
                    depth_of.insert(t.soll_entity_id.clone(), 1);
                    queue.push_back(t.soll_entity_id.clone());
                }
            }
            while let Some(id) = queue.pop_front() {
                let d = depth_of.get(&id).copied().unwrap_or(1);
                if d >= 10 {
                    continue;
                }
                let targets: Vec<String> = snap
                    .outgoing_edges(&id)
                    .map(|(tgt, _rel)| tgt.to_string())
                    .collect();
                for tgt in targets {
                    if seen.insert(tgt.clone()) {
                        depth_of.insert(tgt.clone(), d + 1);
                        queue.push_back(tgt);
                    }
                }
            }
            let mut collected: Vec<(String, String, String)> = seen
                .iter()
                .filter_map(|id| {
                    snap.nodes
                        .get(id)
                        .map(|n| (n.id.clone(), n.entity_type.clone(), n.title.clone()))
                })
                .collect();
            // ORDER BY n.type DESC, n.id
            collected.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            soll_rows = collected
                .into_iter()
                .map(|(id, t, title)| vec![Value::from(id), Value::from(t), Value::from(title)])
                .collect();
        }
        soll_rows
    }

    /// REQ-AXO-902043 — `fuse`: the WHY⊕HOW primitive. For one code symbol, return
    /// its governing SOLL intent (REQ/DEC/PIL) AND its IST impact radius in a single
    /// coherent RAM traversal. `why` answers SOLL→code, `impact` answers IST blast
    /// radius; `fuse` fuses both, WHY-primary (VIS-AXO-001: the why governs, the how
    /// is inferred). RAM-only (PIL-AXO-9002): cold cache / unscoped → loud degraded
    /// error, never a PG fallback.
    pub(crate) fn axon_fuse(&self, args: &Value) -> Option<Value> {
        let symbol = args.get("symbol")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args
            .get("depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 5);

        let Some(target_id) = self.resolve_scoped_symbol_id(symbol, project) else {
            return Some(Self::fuse_unresolved_error(symbol, project));
        };
        let effective_project: Option<String> = match project {
            Some(p) => Some(p.to_string()),
            None => self.symbol_project_code(&target_id),
        };
        let ram_attempted = effective_project
            .as_deref()
            .map(|p| self.ensure_ram_snapshot_warm(p))
            .unwrap_or(false);
        if !ram_attempted {
            let why = if effective_project.is_none() {
                "fuse could not resolve the symbol's project for the RAM snapshot; pass an explicit `project` (PIL-AXO-9002, no PG fallback)"
            } else {
                "IST RAM snapshot is cold for this project; call `ist_snapshot_warm` then retry (PIL-AXO-9002, no PG fallback)"
            };
            return Some(Self::fuse_ram_unavailable_error(symbol, project, why));
        }
        let proj_key = effective_project.as_deref().unwrap_or("");
        let view = process_view();

        // IST leg — reverse reach (blast radius).
        let res = self.build_impact_rows_from_ram(&view, proj_key, &target_id, depth);
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let mut impacted_symbol_ids = BTreeSet::<String>::new();
        let mut impacted_symbol_names = BTreeSet::<String>::new();
        let mut impact_display: Vec<Value> = Vec::new();
        for row in &rows {
            let caller_id = row.first().and_then(|v| v.as_str()).unwrap_or("");
            let origin = row.get(2).and_then(|v| v.as_str()).unwrap_or("Unknown");
            let name = row.get(3).and_then(|v| v.as_str()).unwrap_or("-");
            let kind = row.get(4).and_then(|v| v.as_str()).unwrap_or("-");
            if !caller_id.is_empty() {
                impacted_symbol_ids.insert(caller_id.to_string());
                impact_display.push(json!({"symbol": name, "kind": kind, "origin": origin}));
            }
            if !name.is_empty() {
                impacted_symbol_names.insert(name.to_string());
            }
        }
        impacted_symbol_ids.insert(target_id.clone());
        impacted_symbol_names.insert(symbol.to_string());
        // Radius excludes the target symbol itself.
        let impact_radius = impacted_symbol_ids.len().saturating_sub(1) as i64;

        // SOLL leg — governing intent (WHY), fused on the same impacted set.
        let soll_rows =
            self.fused_governing_intent_ram(proj_key, &impacted_symbol_ids, &impacted_symbol_names);
        crate::soll_snapshot::record_fusion_read(true);
        let governing_intent: Vec<Value> = soll_rows
            .iter()
            .map(|r| {
                json!({
                    "id": r.first().and_then(|v| v.as_str()).unwrap_or("-"),
                    "type": r.get(1).and_then(|v| v.as_str()).unwrap_or("-"),
                    "title": r.get(2).and_then(|v| v.as_str()).unwrap_or("-"),
                })
            })
            .collect();

        // WHY-primary envelope (GUI-AXO-1026 terse; verbose adds the impacted list).
        let intent_count = governing_intent.len();
        let mut text = format!("## 🔗 Fuse: {symbol}\n\n");
        if soll_rows.is_empty() {
            text.push_str(
                "**Governing intent (WHY):** none traced — no SOLL node references this symbol or its impact set.\n\n",
            );
        } else {
            text.push_str(&format!(
                "**Governing intent (WHY) — {intent_count}:**\n\n| Entity | Type | Title |\n| --- | --- | --- |\n"
            ));
            for r in &soll_rows {
                let id = r.first().and_then(|v| v.as_str()).unwrap_or("-");
                let t = r.get(1).and_then(|v| v.as_str()).unwrap_or("-");
                let title = r.get(2).and_then(|v| v.as_str()).unwrap_or("-");
                text.push_str(&format!("| `{id}` | `{t}` | {title} |\n"));
            }
            text.push('\n');
        }
        text.push_str(&format!(
            "**Impact (HOW):** radius {impact_radius} at depth {depth}.\n"
        ));

        Some(json!({
            "content": [{ "type": "text", "text": text }],
            "data": {
                "status": "ok",
                "symbol": symbol,
                "project": effective_project,
                "depth": depth,
                "governing_intent": governing_intent,
                "impact": {
                    "radius": impact_radius,
                    "symbols": if mode == Some("verbose") { Value::Array(impact_display) } else { Value::Null },
                },
                "surfaces_used": ["soll_ram", "graph_ram"],
                "fusion_provenance": { "soll": "soll_ram", "ist": "graph_ram" },
                "next_call_hint": format!("`why symbol={symbol}` for full rationale, `impact symbol={symbol}` for full blast radius"),
                "next_action": { "tool": "why", "arguments": { "symbol": symbol } }
            }
        }))
    }

    fn fuse_unresolved_error(symbol: &str, project: Option<&str>) -> Value {
        json!({
            "content": [{ "type": "text", "text": format!(
                "fuse: symbol `{symbol}` not found in the IST. Widen via `query`, or pass an explicit `project`."
            )}],
            "isError": true,
            "data": {
                "status": "input_not_found",
                "symbol": symbol,
                "project": project,
                "next_action": { "tool": "query", "arguments": { "query": symbol } },
                "operator_guidance": { "follow_up_tools": ["query", "inspect"], "confidence": "high" }
            }
        })
    }

    fn fuse_ram_unavailable_error(symbol: &str, project: Option<&str>, why: &str) -> Value {
        json!({
            "content": [{ "type": "text", "text": format!("fuse degraded: {why}") }],
            "isError": true,
            "data": {
                "status": "degraded",
                "symbol": symbol,
                "project": project,
                "reason": why,
                "next_action": { "tool": "ist_snapshot_warm", "arguments": { "project": project } },
                "operator_guidance": { "follow_up_tools": ["ist_snapshot_warm"], "confidence": "high" }
            }
        })
    }

    fn axon_impact_without_calls(
        &self,
        symbol: &str,
        project: Option<&str>,
        depth: u64,
    ) -> Option<Value> {
        let (query, params) = if let Some(project) = project {
            (
                "SELECT name, kind, COALESCE(project_code, 'unknown') \
                 FROM Symbol \
                 WHERE (name = $sym OR id = $sym) AND project_code = $project \
                 LIMIT 5",
                json!({ "sym": symbol, "project": project }),
            )
        } else {
            (
                "SELECT name, kind, COALESCE(project_code, 'unknown') \
                 FROM Symbol \
                 WHERE name = $sym OR id = $sym \
                 LIMIT 5",
                json!({ "sym": symbol }),
            )
        };
        let symbol_res = self
            .graph_store
            .query_json_param(query, &params)
            .unwrap_or_else(|_| "[]".to_string());
        let symbol_rows: Vec<Vec<Value>> = serde_json::from_str(&symbol_res).unwrap_or_default();
        if symbol_rows.is_empty() {
            let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
            let suggestions_table =
                format_table_from_json(&suggestions, &["Suggested Symbol", "Type", "Project"]);
            let suggestions_rows: Vec<Vec<Value>> =
                serde_json::from_str(&suggestions).unwrap_or_default();
            // REQ-AXO-043 — same dead-end as inspect/path: "retry with one
            // suggested symbol" is unactionable when there are no suggestions.
            let has_suggestions = !suggestions_rows.is_empty();
            let next_actions: &[&str] = if has_suggestions {
                &[
                    "retry with one suggested symbol",
                    "use query/inspect to validate exact name",
                ]
            } else {
                &[
                    "broaden the search via `query` with a less specific term",
                    "verify spelling and project scope",
                    "or pass the exact canonical symbol id",
                ]
            };
            let suggestions = suggestions_rows
                .iter()
                .filter_map(|row| row.first().and_then(Value::as_str))
                .map(|value| Value::from(value.to_string()))
                .collect::<Vec<_>>();
            let recommended_action = if has_suggestions {
                "retry with one suggested symbol or validate the target with query/inspect first"
            } else {
                "broaden the search via `query` with a less specific term, or verify spelling and project scope"
            };
            let next_action_kind = if has_suggestions {
                "select_valid_symbol_then_retry_impact"
            } else {
                "broaden_search"
            };
            let next_action_tool = if has_suggestions { "inspect" } else { "query" };
            let next_action_when = if has_suggestions {
                "after_symbol_selection"
            } else {
                "after_widening_or_correcting_the_search"
            };
            let follow_up_tools: Vec<&str> = if has_suggestions {
                vec!["inspect"]
            } else {
                vec!["query", "inspect"]
            };
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}",
                        symbol,
                        format_standard_contract(
                            "warn_input_not_found",
                            "symbol not found in current scope",
                            &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                            &format!("No exact matching symbol found in current scope.\n\n### Suggestions\n\n{}", suggestions_table),
                            next_actions,
                            "medium",
                        )
                    )
                }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "impact_available": false,
                    "suggestions": suggestions,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "symbol_not_found_in_scope",
                            "severity": "high",
                            "recommended_action": recommended_action
                        }],
                        "remediation_actions": next_actions.iter().map(|s| Value::from(*s)).collect::<Vec<_>>(),
                        "follow_up_tools": follow_up_tools,
                        "next_action": {
                            "kind": next_action_kind,
                            "tool": next_action_tool,
                            "when": next_action_when
                        }
                    },
                    "next_action": {
                        "kind": next_action_kind,
                        "tool": next_action_tool,
                        "when": next_action_when
                    }
                }
            }));
        }
        let degraded_note = self.degraded_truth_note(self.degraded_symbol_count(symbol, project));
        let project_note = self.project_scope_truth_note(project);

        // REQ-AXO-901970 — RAM-only "does the call graph have data" sentinel.
        // Derive the project from the param or the resolved symbol's project_code,
        // warm its snapshot, count CALLS+CALLS_NIF edges in RAM. No PG ist.Edge.
        let sentinel_project = project.map(str::to_string).or_else(|| {
            symbol_rows
                .first()
                .and_then(|row| row.get(2))
                .and_then(Value::as_str)
                .filter(|code| *code != "unknown")
                .map(str::to_string)
        });
        let calls_count = sentinel_project
            .filter(|code| self.ensure_ram_snapshot_warm(code))
            .and_then(|code| {
                process_view()
                    .count_edges_with_relation(&code, &[RelationType::Calls, RelationType::CallsNif])
            })
            .unwrap_or(0);
        if calls_count > 0 {
            return Some(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}{}No impact computed at depth {}.",
                        symbol,
                        project_note.clone().unwrap_or_default(),
                        degraded_note.clone().unwrap_or_default(),
                        depth
                    )
                }],
                "data": {
                    "symbol": symbol,
                    "project": project,
                    "depth": depth,
                    "impact_available": false,
                    "operator_guidance": {
                        "actionable_now": false,
                        "blocking_factors": [{
                            "factor": "no_impact_resolved_at_requested_depth",
                            "severity": "medium",
                            "recommended_action": "increase depth or inspect local dependency flow before assuming there is no impact"
                        }],
                        "remediation_actions": [
                            "increase depth or inspect local dependency flow before assuming there is no impact"
                        ],
                        "follow_up_tools": ["path", "inspect"],
                        "next_action": {
                            "kind": "inspect_local_dependency_flow",
                            "tool": "path",
                            "when": "now"
                        }
                    },
                    "next_action": {
                        "kind": "inspect_local_dependency_flow",
                        "tool": "path",
                        "when": "now"
                    }
                }
            }));
        }

        Some(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "## 💥 Cross-Cutting Impact Analysis: {}\n\n{}{}Symbol exists, but the call graph is not yet available in this live database.\n\n{}\n\n**Status:** CALLS is empty; impact radius cannot yet be reliably computed.",
                    symbol,
                    project_note.unwrap_or_default(),
                    degraded_note.unwrap_or_default(),
                    format_table_from_json(&symbol_res, &["Name", "Type", "Project"])
                )
            }],
            "data": {
                "symbol": symbol,
                "project": project,
                "depth": depth,
                "impact_available": false,
                "operator_guidance": {
                    "actionable_now": false,
                    "blocking_factors": [{
                        "factor": "call_graph_not_available",
                        "severity": "high",
                        "recommended_action": "wait for live call-graph truth before relying on impact for risky mutation"
                    }],
                    "remediation_actions": [
                        "wait for live call-graph truth before relying on impact for risky mutation"
                    ],
                    "follow_up_tools": ["inspect", "query"],
                    "next_action": {
                        "kind": "wait_for_call_graph_truth",
                        "tool": "inspect",
                        "when": "after_indexing_progress"
                    }
                },
                "next_action": {
                    "kind": "wait_for_call_graph_truth",
                    "tool": "inspect",
                    "when": "after_indexing_progress"
                }
            }
        }))
    }

    pub(crate) fn axon_diff(&self, args: &Value) -> Option<Value> {
        let diff = args.get("diff_content")?.as_str()?;
        let mode = args.get("mode").and_then(|v| v.as_str());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v.clamp(10, 500) as usize)
            .unwrap_or(120);
        let mut files = std::collections::HashSet::new();
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.insert(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/") {
                if path != "/dev/null" {
                    files.insert(path.to_string());
                }
            }
        }

        let mut all_results = Vec::new();
        for file in files {
            let query = format!(
                "SELECT s.name, s.kind FROM Symbol s LEFT JOIN Chunk ch ON ch.source_id = s.id AND ch.source_type = 'symbol' WHERE ch.file_path LIKE '%{}%' LIMIT {}",
                file.replace("'", "''"),
                limit
            );
            if let Ok(res) = self.graph_store.query_json(&query) {
                all_results.push(format!("File: {}\nSymbols:\n{}", file, res));
            }
        }
        let mut joined = all_results.join("\n\n");
        let truncated = if joined.len() > 60_000 {
            joined.truncate(60_000);
            true
        } else {
            false
        };
        let evidence = if truncated {
            format!("{}\n\n[truncated=true, max_chars=60000]", joined)
        } else {
            joined
        };
        let report = format!(
            "## 🧬 Diff Impact\n\n{}",
            format_standard_contract(
                "ok",
                "diff symbol extraction completed",
                "workspace:*",
                &evidence_by_mode(&evidence, mode),
                &[
                    "increase `limit` if needed",
                    "run impact on selected symbols for blast radius"
                ],
                if truncated { "medium" } else { "high" },
            )
        );
        // REQ-AXO-91520 — GUI-AXO-1003 tri-modal envelope. `axon_diff`
        // parses a git diff and resolves the touched files to Symbol
        // rows via batched `Symbol JOIN CONTAINS JOIN File` queries —
        // a workspace-wide PG scan. RAM migration would require an
        // `IstGraph::symbols_in_file(path)` reverse index (file →
        // symbols) which the current snapshot does not maintain ;
        // additive surface for a follow-up slice. Envelope flags the
        // PG surface honestly.
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "surfaces_used": ["graph_pg"],
                "total_available": all_results.len() as u64,
                "truncated": truncated,
                "next_call_hint": "impact symbol=<diff-touched-symbol> for blast radius"
            }
        }))
    }

    pub(crate) fn axon_simulate_mutation(&self, args: &Value) -> Option<Value> {
        let symbol = match args.get("symbol").and_then(|v| v.as_str()) {
            Some(v) if !v.trim().is_empty() => v,
            _ => {
                return Some(json!({
                    "content": [{ "type": "text", "text": "Missing required argument: symbol" }],
                    "isError": true,
                    "data": {
                        "status": "input_invalid",
                        "parameter_repair": {
                            "invalid_field": "symbol",
                            "follow_up_tools": ["help", "query"],
                            "hint": "supply a non-empty `symbol`; use `query` to discover symbol names"
                        }
                    }
                }));
            }
        };
        let mode = args.get("mode").and_then(|v| v.as_str());
        let project = args.get("project").and_then(|v| v.as_str());
        let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(2);
        let target_id = match self.resolve_scoped_symbol_id(symbol, project) {
            Some(id) => id,
            None => {
                // REQ-AXO-043 — same dead-end as inspect/path/impact: when
                // suggestion table is empty, "retry with one suggested
                // symbol" is unactionable. Tailor recovery to actual state.
                let suggestions = self.suggest_scoped_symbols(symbol, project, 8);
                let suggestions_table =
                    format_table_from_json(&suggestions, &["Suggested Symbol", "Type", "Project"]);
                let suggestion_rows: Vec<Vec<Value>> =
                    serde_json::from_str(&suggestions).unwrap_or_default();
                let has_suggestions = !suggestion_rows.is_empty();
                let next_actions: &[&str] = if has_suggestions {
                    &[
                        "retry with one suggested symbol",
                        "run inspect to validate symbol name",
                    ]
                } else {
                    &[
                        "broaden the search via `query` with a less specific term",
                        "verify spelling and project scope",
                        "or pass the exact canonical symbol id",
                    ]
                };
                let suggestion_strs: Vec<Value> = suggestion_rows
                    .iter()
                    .filter_map(|row| row.first().and_then(Value::as_str))
                    .map(|value| Value::from(value.to_string()))
                    .collect();
                let next_action_kind = if has_suggestions {
                    "select_valid_symbol_then_retry_simulate"
                } else {
                    "broaden_search"
                };
                let next_action_tool = if has_suggestions { "inspect" } else { "query" };
                return Some(json!({
                    "content": [{
                        "type": "text",
                        "text": format!(
                            "## 🔮 Dry-Run Mutation: {}\n\n{}",
                            symbol,
                            format_standard_contract(
                                "warn_input_not_found",
                                "symbol not found in current scope",
                                &project.map(|p| format!("project:{}", p)).unwrap_or_else(|| "workspace:*".to_string()),
                                &evidence_by_mode(
                                    &format!("No exact symbol found in current scope.\n\n### Suggestions\n\n{}", suggestions_table),
                                    mode,
                                ),
                                next_actions,
                                "medium",
                            )
                        )
                    }],
                    "data": {
                        "symbol": symbol,
                        "project": project,
                        "symbol_found": false,
                        "suggestions": suggestion_strs,
                        "next_action": {
                            "kind": next_action_kind,
                            "tool": next_action_tool,
                        }
                    }
                }));
            }
        };
        // REQ-AXO-901952 — RAM-only blast-radius probe (reverse BFS of CALLS
        // edges) via IstGraphView. Derive the project from the resolved symbol
        // when unscoped ; a cold cache yields a 0 radius with the degraded
        // surface flagged (this is a what-if estimate, not an authoritative
        // count) — no PG `ist.callers_of` fallback.
        let view = crate::ist_snapshot::process_view();
        let effective_project: Option<String> = match project {
            Some(p) => Some(p.to_string()),
            None => self.symbol_project_code(&target_id),
        };
        let ram_warm = effective_project
            .as_deref()
            .map(|p| self.ensure_ram_snapshot_warm(p))
            .unwrap_or(false);
        let mut surfaces_used: Vec<&'static str> = Vec::new();
        let mut surfaces_degraded: Vec<&'static str> = Vec::new();

        let count: i64 = if ram_warm {
            surfaces_used.push("graph_ram");
            let project_key = effective_project.as_deref().unwrap_or("");
            let depth_u32 = depth.clamp(1, 10) as u32;
            let callers = view
                .reverse_at_radius(project_key, &target_id, depth_u32, 10_000, &[])
                .unwrap_or_default();
            callers.len() as i64
        } else {
            surfaces_degraded.push("graph_ram_unavailable");
            0
        };

        let report = format!(
            "## 🔮 Dry-Run Mutation: {}\n\n{}",
            symbol,
            format_standard_contract(
                "ok",
                "mutation blast-radius estimated",
                &project
                    .map(|p| format!("project:{}", p))
                    .unwrap_or_else(|| "workspace:*".to_string()),
                &evidence_by_mode(
                    &format!(
                        "Modifying '{}' will cascade-impact ~{} components in the architecture.",
                        symbol, count
                    ),
                    mode,
                ),
                &["review impact output for precise affected components"],
                "high",
            )
        );
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "symbol": symbol,
                "project": project,
                "impact_radius": count,
                "depth": depth,
                "surfaces_used": surfaces_used,
                "surfaces_degraded": surfaces_degraded,
                "total_available": count,
                "next_call_hint": "impact symbol=<symbol> for precise affected components",
            }
        }))
    }

    /// MIL-AXO-015 B.3: AGE Cypher implementation of axon_impact's
    /// caller traversal. Returns the same 5-column JSON shape as the
    /// SQL WITH RECURSIVE form: rows of `[caller_id, edge_type,
    /// origin_path, name, kind]`.
    ///
    /// Three sub-patterns combined via UNION:
    /// - CALLS variable-length traversal up to `depth`
    /// - CALLS_NIF variable-length traversal up to `depth`
    /// - bridge_name (same Symbol.name across project, NIF flag)
    ///
    /// Returns `None` on identifier validation failure, AGE query
    /// error, or empty result so the caller falls back to SQL —
    /// covers AGE empty (dual-write opt-in not enabled), schema gaps,
    /// or AGE quirks we haven't covered yet.
    /// MIL-AXO-017 slice 5 (REQ-AXO-299) — query `ist.callers_of`
    /// REQ-AXO-91512 — RAM-first counterpart of
    /// `impact_callers_via_ist_edge`. Performs the reverse traversal
    /// in the in-memory IST snapshot (PIL-AXO-9002), classifies each
    /// caller's direct edge to the target via
    /// `IstGraphView::direct_edge_relation`, and materialises the
    /// 5-column row shape downstream parsers expect (`[caller_id,
    /// edge_type, origin, name, kind]`). The origin column is set to
    /// the project_code (Symbol-level granularity ; per-chunk file
    /// path materialisation stays a PG-only feature for v1).
    fn build_impact_rows_from_ram(
        &self,
        view: &crate::ist_snapshot::IstGraphView,
        project: &str,
        target_id: &str,
        depth: u64,
    ) -> String {
        let depth_u32 = depth.clamp(1, 10) as u32;
        let callers = view
            .reverse_at_radius(project, target_id, depth_u32, 10_000, &[])
            .unwrap_or_default();
        if callers.is_empty() {
            return "[]".to_string();
        }
        // Per-caller direct-edge classification (CALLS / CALLS_NIF).
        // Indirect callers (distance > 1) have no direct edge to the
        // target ; we mark them `calls` to preserve the legacy
        // confidence-label arithmetic (`direct_edges + nif_edges > 0
        // ⇒ high confidence`), since the RAM snapshot guarantees a
        // real graph path (no text-matching heuristic).
        let mut edge_type_by_caller: std::collections::HashMap<String, &'static str> =
            std::collections::HashMap::new();
        for caller in &callers {
            // REQ-AXO-91505 — surface the new IMPLEMENTS / IMPORTS / USES
            // edge kinds emitted by the parsers (Rust traits, Elixir
            // protocols, every-language import/use lines). Falls back to
            // "calls" so the legacy confidence-label arithmetic (direct +
            // nif > 0 ⇒ high confidence) still works for unknown edges.
            let label = match view.direct_edge_relation(project, caller, target_id) {
                Some(RelationType::CallsNif) => "calls_nif",
                Some(RelationType::Calls) => "calls",
                Some(RelationType::Implements) => "implements",
                Some(RelationType::Imports) => "imports",
                Some(RelationType::Uses) => "uses",
                Some(_) | None => "calls",
            };
            edge_type_by_caller.insert(caller.clone(), label);
        }
        // Single batch SQL to materialise name + kind + project.
        let escaped: Vec<String> = callers
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect();
        let sql = format!(
            "SELECT id, name, kind, COALESCE(project_code, 'unknown') \
             FROM ist.Symbol WHERE id IN ({})",
            escaped.join(", ")
        );
        let raw = self
            .graph_store
            .query_json(&sql)
            .unwrap_or_else(|_| "[]".to_string());
        let lookup_rows: Vec<Vec<Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut meta_by_id: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::new();
        for row in &lookup_rows {
            let id = row
                .first()
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let name = row
                .get(1)
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            let kind = row
                .get(2)
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string();
            let proj = row
                .get(3)
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            if !id.is_empty() {
                meta_by_id.insert(id, (name, kind, proj));
            }
        }
        // Build the 5-column row format the downstream parser expects.
        let rows: Vec<Value> = callers
            .iter()
            .map(|caller_id| {
                let edge_type = edge_type_by_caller
                    .get(caller_id)
                    .copied()
                    .unwrap_or("calls");
                let (name, kind, origin) = meta_by_id
                    .get(caller_id)
                    .cloned()
                    .unwrap_or_else(|| ("-".to_string(), "-".to_string(), project.to_string()));
                json!([caller_id, edge_type, origin, name, kind])
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
    }

    /// REQ-AXO-901952 — resolve a symbol's project_code (the RAM snapshot
    /// cache key) from PG metadata when the caller didn't scope the query.
    /// Metadata lookup only — the graph traversal stays in RAM. `pub(super)`
    /// so sibling tool modules (api_break_check) share the same derive logic.
    pub(super) fn symbol_project_code(&self, symbol_id: &str) -> Option<String> {
        let sql = format!(
            "SELECT project_code FROM ist.Symbol WHERE id = '{}' LIMIT 1",
            symbol_id.replace('\'', "''")
        );
        let raw = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
        rows.first()
            .and_then(|r| r.first())
            .and_then(Value::as_str)
            .map(|s| s.to_string())
    }

    /// REQ-AXO-901952 — loud degraded error when the RAM IST snapshot cannot
    /// serve `impact` (cold cache or unscoped query). No PG fallback, never a
    /// silent empty impact radius. Replaces the removed PG fallback
    /// `impact_callers_via_ist_edge` (ist.callers_of WITH RECURSIVE).
    fn impact_ram_unavailable_error(
        symbol: &str,
        project: Option<&str>,
        depth: u64,
        why: &str,
    ) -> Value {
        json!({
            "content": [{ "type": "text", "text": format!("impact unavailable : {why}") }],
            "isError": true,
            "data": {
                "status": "degraded",
                "surfaces_used": [],
                "surfaces_degraded": ["graph_ram_unavailable"],
                "total_available": Value::Null,
                "next_call_hint": "ist_snapshot_warm project_code=<project>",
                "symbol": symbol,
                "project": project,
                "depth": depth,
                "impact_radius": Value::Null,
                "operator_guidance": {
                    "actionable_now": false,
                    "blocking_factors": [{
                        "factor": "ist_ram_snapshot_unavailable",
                        "severity": "high",
                        "recommended_action": why
                    }],
                    "follow_up_tools": ["ist_snapshot_warm", "status"],
                    "next_action": { "kind": "warm_ram_snapshot", "tool": "ist_snapshot_warm", "when": "now" }
                },
                "next_action": { "kind": "warm_ram_snapshot", "tool": "ist_snapshot_warm", "when": "now" }
            }
        })
    }

    /// REQ-AXO-162/160 — resolve a user symbol (canonical id or bare name) to its
    /// canonical IST id: exact id first, else the shortest `::name` suffix match.
    fn resolve_test_target_id(&self, project: &str, symbol: &str) -> Option<String> {
        let esc = |s: &str| s.replace('\'', "''");
        let sql = format!(
            "SELECT id FROM ist.Symbol WHERE project_code = '{p}' AND (id = '{s}' OR id LIKE '%::{s}') ORDER BY length(id) ASC LIMIT 1",
            p = esc(project),
            s = esc(symbol)
        );
        let raw = self.graph_store.query_json(&sql).ok()?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&raw).ok()?;
        rows.into_iter()
            .next()?
            .into_iter()
            .next()?
            .as_str()
            .map(String::from)
    }

    /// REQ-AXO-162 — tests that exercise `symbol_id`: reverse-CALLS callers
    /// (radius ≤ N) carrying the `tested` flag (test fns + folded pytest
    /// fixtures, REQ-AXO-901958). Pure RAM IST traversal (PIL-AXO-9002).
    fn tests_exercising(&self, project: &str, symbol_id: &str, radius: u32) -> Vec<String> {
        let view = process_view();
        let mut tests: Vec<String> = view
            .reverse_at_radius(
                project,
                symbol_id,
                radius,
                10_000,
                &[RelationType::Calls, RelationType::CallsNif],
            )
            .unwrap_or_default()
            .into_iter()
            .filter(|c| view.node_tested(project, c) == Some(true))
            .collect();
        tests.sort();
        tests.dedup();
        tests
    }

    /// REQ-AXO-162 — `tests_for(symbol)`: which tests exercise a symbol. Atomic
    /// primitive that composes into `test_impact`. Activates fully once the
    /// macro-call edges (CPT-AXO-90050) are live via a reindex.
    pub(crate) fn axon_tests_for(&self, args: &Value) -> Option<Value> {
        let Some(symbol) = args.get("symbol").and_then(|v| v.as_str()) else {
            return Some(json!({
                "content": [{ "type": "text", "text": "`tests_for` requires `symbol`." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "symbol" } }
            }));
        };
        let project = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.symbol_project_code(symbol));
        let Some(project) = project else {
            return Some(json!({
                "content": [{ "type": "text", "text": "`tests_for`: could not resolve project; pass `project_code`." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "project_code" } }
            }));
        };
        let radius = args.get("radius").and_then(|v| v.as_u64()).unwrap_or(4).clamp(1, 8) as u32;
        let Some(symbol_id) = self.resolve_test_target_id(&project, symbol) else {
            return Some(json!({
                "content": [{ "type": "text", "text": format!("`tests_for`: symbol '{symbol}' not found in {project}.") }],
                "data": { "status": "input_not_found", "symbol": symbol, "next_action": { "tool": "query" } }
            }));
        };
        if !self.ensure_ram_snapshot_warm(&project) {
            return Some(json!({
                "content": [{ "type": "text", "text": "`tests_for`: RAM IST snapshot cold." }],
                "data": { "status": "degraded", "surfaces_degraded": ["graph_ram_unavailable"], "next_action": { "tool": "ist_snapshot_warm" } }
            }));
        }
        let tests = self.tests_exercising(&project, &symbol_id, radius);
        Some(json!({
            "content": [{ "type": "text", "text": format!("{} test(s) exercise {symbol_id}", tests.len()) }],
            "data": {
                "status": "ok",
                "symbol": symbol_id,
                "tests": tests,
                "surfaces_used": ["graph_ram"],
                "follow_up_tools": ["test_impact", "inspect"]
            }
        }))
    }

    /// REQ-AXO-160 — `test_impact(symbols)`: the minimal regression-test set for a
    /// set of changed symbols (union of `tests_for` over each). Commercial
    /// flagship. `symbols` = changed symbol names/ids (diff→symbol resolution is a
    /// thin convenience layer to add once `query --changed` lands).
    pub(crate) fn axon_test_impact(&self, args: &Value) -> Option<Value> {
        let symbols: Vec<String> = args
            .get("symbols")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if symbols.is_empty() {
            return Some(json!({
                "content": [{ "type": "text", "text": "`test_impact` requires `symbols` (changed symbol names/ids)." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "symbols" } }
            }));
        }
        let project = args
            .get("project_code")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| self.symbol_project_code(&symbols[0]));
        let Some(project) = project else {
            return Some(json!({
                "content": [{ "type": "text", "text": "`test_impact`: pass `project_code`." }],
                "isError": true,
                "data": { "status": "input_invalid", "parameter_repair": { "invalid_field": "project_code" } }
            }));
        };
        let radius = args.get("radius").and_then(|v| v.as_u64()).unwrap_or(4).clamp(1, 8) as u32;
        if !self.ensure_ram_snapshot_warm(&project) {
            return Some(json!({
                "content": [{ "type": "text", "text": "`test_impact`: RAM IST snapshot cold." }],
                "data": { "status": "degraded", "surfaces_degraded": ["graph_ram_unavailable"] }
            }));
        }
        let mut all_tests: BTreeSet<String> = BTreeSet::new();
        let mut per_symbol = serde_json::Map::new();
        let mut unresolved: Vec<String> = Vec::new();
        for sym in &symbols {
            match self.resolve_test_target_id(&project, sym) {
                Some(id) => {
                    let t = self.tests_exercising(&project, &id, radius);
                    for x in &t {
                        all_tests.insert(x.clone());
                    }
                    per_symbol.insert(sym.clone(), json!(t));
                }
                None => unresolved.push(sym.clone()),
            }
        }
        let minimal: Vec<String> = all_tests.into_iter().collect();
        Some(json!({
            "content": [{ "type": "text", "text": format!("Minimal test set: {} test(s) across {} changed symbol(s)", minimal.len(), symbols.len()) }],
            "data": {
                "status": "ok",
                "project": project,
                "minimal_test_set": minimal,
                "per_symbol": per_symbol,
                "unresolved": unresolved,
                "surfaces_used": ["graph_ram"],
                "follow_up_tools": ["tests_for"]
            }
        }))
    }
}
