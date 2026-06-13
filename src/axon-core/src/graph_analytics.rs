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
        if !structural_graph_analytics_available() {
            return Ok((100, "[]".to_string()));
        }
        // REQ-AXO-350 : ist.Edge replaces legacy CALLS / CALLS_NIF (MIL-AXO-017).
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let scope = if scoped {
            format!(" AND s1.project_code = '{}' ", escaped)
        } else {
            String::new()
        };
        // REQ-AXO-901923 — target-first walk. The previous query expanded
        // CALLS forward over the whole edge set (Edge⋈Symbol⋈Edge⋈Symbol on
        // 390k edges) with the danger filter applied AFTER the 2-hop join.
        // `unwrap` (ubiquitous in Rust, huge fan-in) made that O(E²) and blew
        // the MCP gateway timeout. We instead start from the SMALL `dangerous`
        // set and walk BACKWARD via the index on Edge.target_id, bounded by
        // `LIMIT` (the score saturates at 5 findings anyway, so a sample is
        // sufficient and the result stays a cheap streamed index scan).
        let query = format!(
            "
            WITH dangerous AS (
                SELECT id FROM Symbol
                WHERE is_unsafe = true OR lower(name) IN ('eval', 'unwrap')
            ),
            direct AS (
                SELECT s1.name AS name, s2.name AS target_name
                FROM ist.Edge c
                JOIN dangerous d ON d.id = c.target_id
                JOIN Symbol s1 ON s1.id = c.source_id
                JOIN Symbol s2 ON s2.id = c.target_id
                WHERE (c.relation_type = 'CALLS' OR c.relation_type = 'CALLS_NIF'){scope}
            ),
            indirect AS (
                -- REQ-AXO-901721 — cross-language taint: the indirect (2-hop)
                -- walk must follow CALLS_NIF, not just CALLS, or an
                -- elixir -CALLS_NIF-> rust_nif -CALLS-> unsafe chain (the
                -- canonical cross-language taint) is silently undetected. The
                -- `direct` CTE already accepts both relation kinds; indirect now
                -- matches, so a NIF boundary on either hop is traversed.
                SELECT s1.name AS name, s2.name AS target_name
                FROM ist.Edge c2
                JOIN dangerous d ON d.id = c2.target_id AND c2.relation_type IN ('CALLS', 'CALLS_NIF')
                JOIN ist.Edge c1 ON c1.target_id = c2.source_id AND c1.relation_type IN ('CALLS', 'CALLS_NIF')
                JOIN Symbol s1 ON s1.id = c1.source_id
                JOIN Symbol s2 ON s2.id = c2.target_id
                WHERE true{scope}
            )
            SELECT name, target_name FROM (
                SELECT name, target_name FROM direct
                UNION ALL
                SELECT name, target_name FROM indirect
            ) dangerous_paths
            LIMIT 100
        ",
            scope = scope
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        if rows.is_empty() {
            return Ok((100, "[]".to_string()));
        }

        let score = (100 - (rows.len() as i64 * 20)).max(0);
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
        // REQ-AXO-350 : ist.Edge replaces legacy CONTAINS / CALLS (MIL-AXO-017).
        // REQ-AXO-901653 slice-5c : public.File retired ; ist.IndexedFile is
        // the canonical per-file pivot. IndexedFile has no `project_code`
        // column ; the scoping happens via Symbol.project_code on the
        // CONTAINS edge target.
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT f.path, s.name
            FROM ist.IndexedFile f
            JOIN ist.Edge c ON c.source_id = f.path AND c.relation_type = 'CONTAINS'
            JOIN ist.Symbol s ON s.id = c.target_id
            WHERE (lower(s.name) LIKE '%todo%'
               OR lower(s.name) LIKE '%fixme%'
               OR lower(s.name) LIKE '%secret%'
               OR lower(s.name) LIKE '%hardcoded credential%'
               OR EXISTS (
                    SELECT 1 FROM ist.Edge call
                    JOIN ist.Symbol target ON target.id = call.target_id
                    WHERE call.relation_type = 'CALLS'
                      AND call.source_id = s.id
                      AND lower(target.name) IN ('unwrap', 'eval')
               ))
            {}
        ",
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut findings = serde_json::Map::new();
        for row in rows {
            if row.len() >= 2 {
                findings.insert(row[0].clone(), serde_json::Value::String(row[1].clone()));
            }
        }
        Ok(findings)
    }

    pub fn get_telemetry_score(&self, project: &str) -> Result<i64> {
        if !structural_graph_analytics_available() {
            return Ok(100);
        }
        // REQ-AXO-350 : ist.Edge replaces legacy CALLS table (MIL-AXO-017).
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT count(*)
            FROM ist.Edge call
            JOIN Symbol target ON target.id = call.target_id
            WHERE call.relation_type = 'CALLS'
              AND lower(target.name) IN ('println!', 'dbg!', 'console.log', 'io.puts', 'print', 'printf')
            {}
            ",
            if scoped {
                format!(" AND call.source_id IN (SELECT id FROM Symbol WHERE project_code = '{}')", escaped)
            } else {
                String::new()
            }
        );
        let bad_logs = self.query_count(&query).unwrap_or(0);
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
        // REQ-AXO-91486 slice 2 — RAM fast-path. When the cache holds a
        // snapshot for the project (RAM unconditional, REQ-AXO-901952), count
        // reciprocal CALLS cycles in-memory (linear scan over CSR) instead of
        // running the SQL self-join. Cache miss → fallback to the canonical PG
        // path below. `project="*"` (workspace-wide) skips the fast-path
        // because the cache is per-project.
        if project != "*" {
            if let Some(count) =
                crate::ist_snapshot::process_view().reciprocal_calls_cycle_count(project)
            {
                return Ok(count as i64);
            }
        }
        // REQ-AXO-350 : ist.Edge self-join replaces legacy CALLS (MIL-AXO-017).
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT count(*)
            FROM (
                SELECT
                    least(c1.source_id, c1.target_id) AS left_id,
                    greatest(c1.source_id, c1.target_id) AS right_id
                FROM ist.Edge c1
                JOIN ist.Edge c2
                  ON c1.source_id = c2.target_id
                 AND c1.target_id = c2.source_id
                 AND c2.relation_type = 'CALLS'
                WHERE c1.relation_type = 'CALLS'
                  AND c1.source_id != c1.target_id
                  {}
                GROUP BY 1, 2
            ) reciprocal_cycles
            ",
            if scoped {
                format!(
                    "AND c1.project_code = '{}' AND c2.project_code = '{}'",
                    escaped, escaped
                )
            } else {
                String::new()
            }
        );
        Ok(self.query_count(&query).unwrap_or(0))
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

    #[test]
    fn batch_a_get_security_audit_uses_public_edge() {
        let body = extract_fn_body(SOURCE, "pub fn get_security_audit");
        assert!(!body.contains("skip_legacy_relations"));
        assert!(body.contains("FROM ist.Edge"));
        assert!(body.contains("relation_type = 'CALLS'"));
        assert!(body.contains("relation_type = 'CALLS_NIF'"));
    }

    #[test]
    fn batch_a_get_telemetry_score_uses_public_edge() {
        let body = extract_fn_body(SOURCE, "pub fn get_telemetry_score");
        assert!(!body.contains("skip_legacy_relations"));
        assert!(body.contains("FROM ist.Edge"));
        assert!(body.contains("relation_type = 'CALLS'"));
    }

    #[test]
    fn batch_a_get_circular_dependency_count_fast_uses_public_edge() {
        let body = extract_fn_body(SOURCE, "pub fn get_circular_dependency_count_fast");
        assert!(!body.contains("skip_legacy_relations"));
        assert!(body.contains("FROM ist.Edge"));
        assert!(body.contains("c1.relation_type = 'CALLS'"));
        assert!(body.contains("c2.relation_type = 'CALLS'"));
    }

    // REQ-AXO-350 batch (b) — CALLS+CONTAINS mixed gates.

    #[test]
    fn batch_b_get_technical_debt_uses_public_edge() {
        let body = extract_fn_body(SOURCE, "pub fn get_technical_debt");
        assert!(!body.contains("skip_legacy_relations"));
        assert!(body.contains("c.relation_type = 'CONTAINS'"));
        assert!(body.contains("call.relation_type = 'CALLS'"));
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
