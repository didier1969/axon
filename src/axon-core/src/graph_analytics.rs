// Copyright (c) Didier Stadelmann. All rights reserved.

use anyhow::Result;

use crate::graph::GraphStore;

// REQ-AXO-350 follow-up : pre-MIL-AXO-017 this gate returned false
// in `brain_only` mode because IST edges lived in DuckDB / AGE
// in-process state — the brain alone had no access. Post-REQ-AXO-295
// (`ist.Edge`) the IST is persistent in PG and any role with a
// `GraphStore` (brain or indexer) can query it directly. The gate is
// retained as a single inlined identity so the 16 analytics call
// sites stay token-stable, but always returns true. Once the call
// sites are inlined in a follow-up cleanup the function can be
// deleted entirely.
fn structural_graph_analytics_available() -> bool {
    true
}

impl GraphStore {
    pub fn get_security_audit(&self, project: &str) -> Result<(i64, String)> {
        // REQ-AXO-901970 — RAM-only taint walk (no PG fallback). 1+2-hop reverse
        // CALLS/CALLS_NIF from the dangerous set (unsafe / eval / unwrap) over the
        // process snapshot. "*"/cold → no findings (score 100). Same score formula
        // (saturates at 5 findings) and `[[caller,target],…]` evidence shape.
        if project == "*" {
            return Ok((100, "[]".to_string()));
        }
        let pairs = crate::ist_snapshot::process_view()
            .security_audit_paths(project)
            .unwrap_or_default();
        if pairs.is_empty() {
            return Ok((100, "[]".to_string()));
        }
        let score = (100 - (pairs.len() as i64 * 20)).max(0);
        let rows: Vec<Vec<String>> = pairs.into_iter().map(|(a, b)| vec![a, b]).collect();
        Ok((
            score,
            serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
        ))
    }

    pub fn get_coverage_score(&self, project: &str) -> Result<i64> {
        if !structural_graph_analytics_available() {
            return Ok(0);
        }
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let total = if scoped {
            self.query_count(&format!(
                "SELECT count(*) FROM Symbol WHERE project_code = '{}'",
                escaped
            ))?
        } else {
            self.query_count("SELECT count(*) FROM Symbol")?
        };
        if total <= 0 {
            return Ok(0);
        }
        let tested = if scoped {
            self.query_count(&format!(
                "SELECT count(*) FROM Symbol WHERE project_code = '{}' AND tested = true",
                escaped
            ))?
        } else {
            self.query_count("SELECT count(*) FROM Symbol WHERE tested = true")?
        };
        Ok(((tested * 100) / total).clamp(0, 100))
    }

    pub fn get_technical_debt(
        &self,
        project: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        if !structural_graph_analytics_available() {
            return Ok(serde_json::Map::new());
        }
        // REQ-AXO-901970 — RAM-only (no PG). Project symbols whose name carries a
        // debt fragment OR that CALL unwrap/eval, mapped {file_path: name}.
        // "*"/cold → empty.
        if project == "*" {
            return Ok(serde_json::Map::new());
        }
        let mut findings = serde_json::Map::new();
        for (file, name) in crate::ist_snapshot::process_view()
            .technical_debt(project)
            .unwrap_or_default()
        {
            findings.insert(file, serde_json::Value::String(name));
        }
        Ok(findings)
    }

    pub fn get_telemetry_score(&self, project: &str) -> Result<i64> {
        // REQ-AXO-901970 — RAM-only count of CALLS to raw-logging functions (no
        // PG). "*"/cold → 100 (no penalty; per-project cache can't aggregate "*").
        if project == "*" {
            return Ok(100);
        }
        let bad_logs = crate::ist_snapshot::process_view()
            .telemetry_log_call_count(project)
            .unwrap_or(0) as i64;
        Ok((100 - (bad_logs * 5)).max(0))
    }

    pub fn get_orphan_intent_nodes(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT n.id, n.type, COALESCE(n.title, '')
            FROM soll.Node n
            LEFT JOIN soll.Traceability t
              ON lower(t.soll_entity_type) = lower(n.type)
             AND t.soll_entity_id = n.id
            WHERE n.type IN ('Requirement', 'Decision', 'Concept', 'Validation')
              AND t.id IS NULL
              {}
            ORDER BY n.id ASC
            LIMIT 20
            ",
            if scoped {
                format!(" AND n.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 2)
            .map(|row| {
                let title = row.get(2).cloned().unwrap_or_default();
                if title.is_empty() {
                    format!("{} ({})", row[0], row[1])
                } else {
                    format!("{} ({}) - {}", row[0], row[1], title)
                }
            })
            .collect())
    }

    pub fn get_circular_dependencies(&self, project: &str) -> Result<Vec<String>> {
        if !structural_graph_analytics_available() {
            return Ok(Vec::new());
        }
        // REQ-AXO-901952 — RAM-only cycle LISTING. Each non-trivial Tarjan SCC
        // (size > 1) in the per-project snapshot is a circular-dependency
        // cluster; render its members (short names = canonical id tail) joined
        // by " -> " and closed back to the first, mirroring the legacy
        // `WITH RECURSIVE` path-string output. Workspace-wide ("*") and a cold
        // cache surface empty — the per-project reciprocal-cycle count
        // (`get_circular_dependency_count_fast`) stays the RAM heartbeat. No PG
        // `WITH RECURSIVE` fallback (the whole point of REQ-AXO-901952).
        if project == "*" {
            return Ok(Vec::new());
        }
        let Some(sccs) = crate::ist_snapshot::process_view().structural_sccs(project) else {
            return Ok(Vec::new());
        };
        let findings = sccs
            .into_iter()
            .map(|members| {
                let mut names: Vec<&str> = members
                    .iter()
                    .map(|id| id.rsplit("::").next().unwrap_or(id.as_str()))
                    .collect();
                // Close the loop so the rendering reads as a cycle (a -> b -> a).
                if let Some(first) = names.first().copied() {
                    names.push(first);
                }
                names.join(" -> ")
            })
            .collect();
        Ok(findings)
    }

    pub fn get_circular_dependency_count_fast(&self, project: &str) -> Result<i64> {
        if !structural_graph_analytics_available() {
            return Ok(0);
        }
        // REQ-AXO-901970 — RAM-only reciprocal CALLS cycle count (linear scan
        // over the CSR snapshot). Replaces the last PG `ist.Edge` self-join in
        // graph_analytics. Workspace-wide ("*") and a cold cache surface 0 — the
        // per-project cache can't aggregate the workspace, and there is no PG
        // fallback (the whole point of REQ-AXO-901952/901970).
        if project == "*" {
            return Ok(0);
        }
        Ok(crate::ist_snapshot::process_view()
            .reciprocal_calls_cycle_count(project)
            .unwrap_or(0) as i64)
    }

    // MIL-AXO-017 slice 6B Phase C: AGE cycle detection helpers
    // (circular_dependency_count_via_age, circular_dependencies_via_age) removed ;
    // SQL WITH RECURSIVE in callers is canonical.

    pub fn get_unsafe_exposure(&self, project: &str) -> Result<Vec<String>> {
        if !structural_graph_analytics_available() {
            return Ok(Vec::new());
        }
        // REQ-AXO-901970 — RAM-only. From each public callable, BFS forward CALLS
        // (depth ≤ 10) to any unsafe target (NodeFlags.unsafe_ OR name='unwrap').
        // Replaces the PG `WITH RECURSIVE`. Workspace-wide ("*") and a cold cache
        // surface empty (no PG fallback — the whole point of REQ-AXO-901952/901970).
        if project == "*" {
            return Ok(Vec::new());
        }
        Ok(crate::ist_snapshot::process_view()
            .unsafe_exposure(project)
            .unwrap_or_default())
    }

    pub fn get_injection_risk_paths(&self, project: &str) -> Result<Vec<String>> {
        if !structural_graph_analytics_available() {
            return Ok(Vec::new());
        }
        // REQ-AXO-902210 — RAM-only, same shape as get_unsafe_exposure: from
        // each public callable, BFS forward CALLS (depth ≤ 10) to a known
        // injection sink (slice 1: the raw-SQL execution gateway). Workspace-
        // wide ("*") and a cold cache surface empty (no PG fallback).
        if project == "*" {
            return Ok(Vec::new());
        }
        Ok(crate::ist_snapshot::process_view()
            .injection_risk_paths(project)
            .unwrap_or_default())
    }

    pub fn get_nif_blocking_risks(&self, project: &str) -> Result<Vec<String>> {
        if !structural_graph_analytics_available() {
            return Ok(Vec::new());
        }
        // REQ-AXO-901970 — RAM-only. From each CALLS_NIF target, BFS forward CALLS
        // (depth ≤ 20) tracking the deepest chain; report NIFs whose max depth
        // exceeds 5. Replaces the PG `WITH RECURSIVE`. Workspace-wide ("*") and a
        // cold cache surface empty (no PG fallback).
        if project == "*" {
            return Ok(Vec::new());
        }
        Ok(crate::ist_snapshot::process_view()
            .nif_blocking_risks(project)
            .unwrap_or_default())
    }

}

#[cfg(test)]
mod migration_guard_tests {
    // REQ-AXO-350 batch (a) — source-level regression guard. Asserts the three
    // migrated functions no longer carry a `skip_legacy_relations()` early
    // return and that their SQL bodies reference `ist.Edge`. Fast unit
    // test with no PG fixture (the live PG-backed end-to-end is exercised
    // by tests::maillon_tests::test_graph_analytics_detects_*).
    const SOURCE: &str = include_str!("graph_analytics.rs");

    fn extract_fn_body<'a>(src: &'a str, fn_signature: &str) -> &'a str {
        let start = src
            .find(fn_signature)
            .unwrap_or_else(|| panic!("{fn_signature} not found in graph_analytics.rs"));
        let tail = &src[start..];
        // Terminate at the next sibling `pub fn` OR at the closing `}` of
        // the `impl GraphStore` block (for the last function in the impl).
        let next_fn = tail.find("\n    pub fn ");
        let end_of_impl = tail.find("\n}\n");
        let body_end_relative = match (next_fn, end_of_impl) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => tail.len(),
        };
        &tail[..body_end_relative]
    }

    // REQ-AXO-901970 — security_audit / telemetry are RAM-only (taint walk +
    // log-call count over the process snapshot); no PG ist.Edge SQL.
    #[test]
    fn batch_a_get_security_audit_is_ram_only() {
        let code = code_only(SOURCE, "pub fn get_security_audit");
        assert!(!code.contains("FROM ist.Edge"), "no IST graph SQL");
        assert!(!code.contains("query_json"), "no SQL round-trip");
        assert!(code.contains("security_audit_paths"), "routed through RAM");
    }

    #[test]
    fn batch_a_get_telemetry_score_is_ram_only() {
        let code = code_only(SOURCE, "pub fn get_telemetry_score");
        assert!(!code.contains("FROM ist.Edge"), "no IST graph SQL");
        assert!(!code.contains("query_count"), "no SQL round-trip");
        assert!(code.contains("telemetry_log_call_count"), "routed through RAM");
    }

    // REQ-AXO-901970 — the reciprocal-cycle count is now RAM-only (linear scan
    // over the CSR snapshot); the last PG `ist.Edge` self-join in this file is
    // gone. Guard the no-PG-fallback invariant on CODE.
    #[test]
    fn batch_a_get_circular_dependency_count_fast_is_ram_only() {
        let code = code_only(SOURCE, "pub fn get_circular_dependency_count_fast");
        assert!(!code.contains("skip_legacy_relations"));
        assert!(!code.contains("FROM ist.Edge"), "no IST graph SQL");
        assert!(!code.contains("query_count"), "no SQL round-trip");
        assert!(
            code.contains("process_view()") && code.contains("reciprocal_calls_cycle_count"),
            "routed through the RAM reciprocal-cycle count"
        );
    }

    // REQ-AXO-350 batch (b) — CALLS+CONTAINS mixed gates.

    // REQ-AXO-901970 — technical_debt is RAM-only (name fragments + CALLS to
    // unwrap/eval over the process snapshot); no PG ist.Edge SQL.
    #[test]
    fn batch_b_get_technical_debt_is_ram_only() {
        let code = code_only(SOURCE, "pub fn get_technical_debt");
        assert!(!code.contains("FROM ist.Edge"), "no IST graph SQL");
        assert!(!code.contains("query_json"), "no SQL round-trip");
        assert!(code.contains("technical_debt"), "routed through RAM");
    }


    // REQ-AXO-901952 — the cycle LISTING is now RAM-only (Tarjan SCC over the
    // process snapshot); the legacy PG `WITH RECURSIVE` path enumeration is
    // gone. Body-introspection guards the no-PG-fallback invariant: no
    // `WITH RECURSIVE`, no `ist.Edge` SQL, routed through `process_view()`.
    #[test]
    fn batch_c_get_circular_dependencies_is_ram_only_no_pg_recursion() {
        // Strip `//` comment tails before introspecting: the explanatory
        // comments in the migrated body legitimately mention "WITH RECURSIVE"
        // (describing what was removed), so the guard must inspect CODE only.
        let raw = extract_fn_body(SOURCE, "pub fn get_circular_dependencies");
        let code: String = raw
            .lines()
            .map(|line| match line.find("//") {
                Some(i) => &line[..i],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("WITH RECURSIVE"),
            "no PG recursive cycle enumeration"
        );
        assert!(!code.contains("FROM ist.Edge"), "no IST graph SQL");
        assert!(!code.contains("query_json"), "no SQL round-trip at all");
        assert!(
            code.contains("process_view()") && code.contains("structural_sccs"),
            "routed through the RAM Tarjan SCC path"
        );
        // DuckDB residue must stay gone.
        assert!(!code.contains("list_append"));
        assert!(!code.contains("list_contains"));
    }

    // REQ-AXO-901970 — get_unsafe_exposure / get_nif_blocking_risks are now
    // RAM-only (BFS over the process snapshot); the legacy PG `WITH RECURSIVE`
    // call-path enumeration is gone. Guard the no-PG-fallback invariant on CODE
    // (comment tails stripped — comments legitimately mention WITH RECURSIVE).
    fn code_only(src: &str, sig: &str) -> String {
        extract_fn_body(src, sig)
            .lines()
            .map(|l| match l.find("//") {
                Some(i) => &l[..i],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn batch_c_get_unsafe_exposure_is_ram_only_no_pg_recursion() {
        let code = code_only(SOURCE, "pub fn get_unsafe_exposure");
        assert!(!code.contains("WITH RECURSIVE"));
        assert!(!code.contains("FROM ist.Edge"));
        assert!(!code.contains("query_json"));
        assert!(code.contains("process_view()") && code.contains("unsafe_exposure"));
    }

    #[test]
    fn batch_c_get_nif_blocking_risks_is_ram_only_no_pg_recursion() {
        let code = code_only(SOURCE, "pub fn get_nif_blocking_risks");
        assert!(!code.contains("WITH RECURSIVE"));
        assert!(!code.contains("FROM ist.Edge"));
        assert!(!code.contains("query_json"));
        assert!(code.contains("process_view()") && code.contains("nif_blocking_risks"));
    }
}
