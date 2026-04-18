// Copyright (c) Didier Stadelmann. All rights reserved.

use anyhow::Result;

use crate::graph::GraphStore;

impl GraphStore {
    pub fn get_security_audit(&self, project: &str) -> Result<(i64, String)> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let scope = if scoped {
            format!(" AND s1.project_code = '{}' ", escaped)
        } else {
            String::new()
        };
        let query = format!(
            "
            WITH dangerous_paths AS (
                SELECT s1.name, s2.name AS target_name
                FROM CALLS c
                JOIN Symbol s1 ON s1.id = c.source_id
                JOIN Symbol s2 ON s2.id = c.target_id
                WHERE (s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')){scope}
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM CALLS_NIF c
                JOIN Symbol s1 ON s1.id = c.source_id
                JOIN Symbol s2 ON s2.id = c.target_id
                WHERE (s2.is_nif = true OR s2.is_unsafe = true){scope}
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM Symbol s1
                JOIN CALLS c1 ON c1.source_id = s1.id
                JOIN Symbol mid ON mid.id = c1.target_id
                JOIN CALLS c2 ON c2.source_id = mid.id
                JOIN Symbol s2 ON s2.id = c2.target_id
                WHERE (s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')){scope}
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM Symbol s1
                JOIN CALLS_NIF c1 ON c1.source_id = s1.id
                JOIN Symbol mid ON mid.id = c1.target_id
                JOIN CALLS c2 ON c2.source_id = mid.id
                JOIN Symbol s2 ON s2.id = c2.target_id
                WHERE (s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')){scope}
            )
            SELECT name, target_name FROM dangerous_paths
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
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT f.path, s.name
            FROM File f
            JOIN CONTAINS c ON c.source_id = f.path
            JOIN Symbol s ON s.id = c.target_id
            WHERE (lower(s.name) LIKE '%todo%'
               OR lower(s.name) LIKE '%fixme%'
               OR lower(s.name) LIKE '%secret%'
               OR lower(s.name) LIKE '%hardcoded credential%'
               OR EXISTS (
                    SELECT 1 FROM CALLS call
                    JOIN Symbol target ON target.id = call.target_id
                    WHERE call.source_id = s.id
                      AND lower(target.name) IN ('unwrap', 'eval')
               ))
            {}
        ",
            if scoped {
                format!(
                    " AND (f.project_code = '{}' OR s.project_code = '{}')",
                    escaped, escaped
                )
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

    pub fn get_god_objects(
        &self,
        project: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT s.name, count(*) AS fan_in
            FROM Symbol s
            JOIN CALLS c ON c.target_id = s.id
            LEFT JOIN CONTAINS rel ON rel.target_id = s.id
            LEFT JOIN File f ON f.path = rel.source_id
            {}
            AND length(s.name) >= 3
            AND lower(s.name) NOT LIKE '__webpack%'
            AND lower(s.name) NOT LIKE '%minified%'
            AND (
                f.path IS NULL
                OR (
                    lower(f.path) NOT LIKE '%/priv/static/%'
                    AND lower(f.path) NOT LIKE '%/node_modules/%'
                    AND lower(f.path) NOT LIKE '%/dist/%'
                    AND lower(f.path) NOT LIKE '%/_build/%'
                )
            )
            GROUP BY s.name
            HAVING count(*) >= 20
        ",
            if scoped {
                format!("WHERE s.project_code = '{}'", escaped)
            } else {
                "WHERE 1=1".to_string()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut findings = serde_json::Map::new();
        for row in rows {
            if row.len() >= 2 {
                let count = row[1].parse::<i64>().unwrap_or(0);
                findings.insert(row[0].clone(), serde_json::Value::Number(count.into()));
            }
        }
        Ok(findings)
    }

    pub fn get_telemetry_score(&self, project: &str) -> Result<i64> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT count(*) 
            FROM CALLS call
            JOIN Symbol target ON target.id = call.target_id
            WHERE lower(target.name) IN ('println!', 'dbg!', 'console.log', 'io.puts', 'print', 'printf')
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

    pub fn get_dead_code_count(&self, project: &str) -> Result<i64> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT count(*)
            FROM Symbol s
            JOIN CONTAINS c ON c.target_id = s.id
            JOIN File f ON f.path = c.source_id
            WHERE s.kind IN ('function', 'method')
              AND COALESCE(s.is_public, false) = false
              AND s.id NOT IN (SELECT target_id FROM CALLS)
              AND s.id NOT IN (SELECT target_id FROM CALLS_NIF)
              AND f.path NOT LIKE '%/tests/%' AND f.path NOT LIKE '%_test.rs' AND f.path NOT LIKE '%_test.exs'
            {}
            ",
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );
        Ok(self.query_count(&query).unwrap_or(0))
    }

    pub fn get_wrapper_candidates(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            WITH outbound AS (
                SELECT source_id, count(*) AS total_calls
                FROM CALLS
                {}
                GROUP BY 1
            ),
            inbound AS (
                SELECT target_id, count(*) AS total_callers
                FROM CALLS
                {}
                GROUP BY 1
            )
            SELECT s.name, target.name, COALESCE(inbound.total_callers, 0)
            FROM outbound o
            JOIN CALLS c ON c.source_id = o.source_id
            JOIN Symbol s ON s.id = o.source_id
            JOIN Symbol target ON target.id = c.target_id
            LEFT JOIN inbound ON inbound.target_id = target.id
            LEFT JOIN CONTAINS rel ON rel.target_id = s.id
            LEFT JOIN File f ON f.path = rel.source_id
            WHERE o.total_calls = 1
              AND COALESCE(s.is_public, false) = false
              AND s.kind IN ('function', 'method')
              AND (
                f.path IS NULL
                OR (
                    lower(f.path) NOT LIKE '%/tests/%'
                    AND lower(f.path) NOT LIKE '%_test.rs'
                    AND lower(f.path) NOT LIKE '%_test.exs'
                )
              )
              {}
            ORDER BY COALESCE(inbound.total_callers, 0) DESC, s.name ASC
            LIMIT 20
            ",
            if scoped {
                format!("WHERE project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!("WHERE project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 2)
            .map(|row| format!("{} -> {}", row[0], row[1]))
            .collect())
    }

    pub fn get_feature_envy_candidates(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            WITH symbol_files AS (
                SELECT s.id, s.name, f.path
                FROM Symbol s
                JOIN CONTAINS rel ON rel.target_id = s.id
                JOIN File f ON f.path = rel.source_id
                WHERE s.kind IN ('function', 'method')
                  AND (
                    lower(f.path) NOT LIKE '%/tests/%'
                    AND lower(f.path) NOT LIKE '%_test.rs'
                    AND lower(f.path) NOT LIKE '%_test.exs'
                  )
                  {}
            ),
            outbound AS (
                SELECT
                    src.name AS source_name,
                    src.path AS source_path,
                    dst.path AS target_path,
                    count(*) AS call_count
                FROM CALLS c
                JOIN symbol_files src ON src.id = c.source_id
                JOIN symbol_files dst ON dst.id = c.target_id
                {}
                GROUP BY 1, 2, 3
            ),
            scored AS (
                SELECT
                    source_name,
                    source_path,
                    sum(call_count) AS total_calls,
                    sum(CASE WHEN source_path != target_path THEN call_count ELSE 0 END) AS foreign_calls,
                    max_by(target_path, CASE WHEN source_path != target_path THEN call_count ELSE 0 END) AS dominant_foreign_path
                FROM outbound
                GROUP BY 1, 2
            )
            SELECT
                source_name,
                dominant_foreign_path,
                total_calls,
                foreign_calls
            FROM scored
            WHERE total_calls >= 3
              AND foreign_calls >= 2
              AND foreign_calls > (total_calls - foreign_calls)
              AND dominant_foreign_path IS NOT NULL
            ORDER BY foreign_calls DESC, total_calls DESC, source_name ASC
            LIMIT 20
            ",
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!("WHERE c.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 4)
            .map(|row| format!("{} -> {} ({}/{})", row[0], row[1], row[3], row[2]))
            .collect())
    }

    pub fn get_detour_candidates(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            WITH symbol_files AS (
                SELECT s.id, s.name, f.path, COALESCE(s.is_public, false) AS is_public
                FROM Symbol s
                JOIN CONTAINS rel ON rel.target_id = s.id
                JOIN File f ON f.path = rel.source_id
                WHERE s.kind IN ('function', 'method')
                  AND (
                    lower(f.path) NOT LIKE '%/tests/%'
                    AND lower(f.path) NOT LIKE '%_test.rs'
                    AND lower(f.path) NOT LIKE '%_test.exs'
                  )
                  {}
            ),
            inbound AS (
                SELECT target_id, count(*) AS inbound_calls
                FROM CALLS
                {}
                GROUP BY 1
            ),
            outbound AS (
                SELECT source_id, count(*) AS outbound_calls
                FROM CALLS
                {}
                GROUP BY 1
            )
            SELECT
                src.name,
                mid.name,
                dst.name
            FROM CALLS c1
            JOIN CALLS c2 ON c1.target_id = c2.source_id
            JOIN symbol_files src ON src.id = c1.source_id
            JOIN symbol_files mid ON mid.id = c1.target_id
            JOIN symbol_files dst ON dst.id = c2.target_id
            JOIN inbound mid_in ON mid_in.target_id = mid.id
            JOIN outbound mid_out ON mid_out.source_id = mid.id
            WHERE src.path = mid.path
              AND mid.path = dst.path
              AND src.id != dst.id
              AND mid_in.inbound_calls = 1
              AND mid_out.outbound_calls = 1
              AND mid.is_public = false
              {}
            ORDER BY src.name ASC, mid.name ASC, dst.name ASC
            LIMIT 20
            ",
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!("WHERE project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!("WHERE project_code = '{}'", escaped)
            } else {
                String::new()
            },
            if scoped {
                format!(
                    "AND c1.project_code = '{}' AND c2.project_code = '{}'",
                    escaped, escaped
                )
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 3)
            .map(|row| format!("{} -> {} -> {}", row[0], row[1], row[2]))
            .collect())
    }

    pub fn get_abstraction_detour_candidates(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            WITH symbol_files AS (
                SELECT s.id, s.name, lower(s.name) AS lowered_name, s.kind, f.path
                FROM Symbol s
                JOIN CONTAINS rel ON rel.target_id = s.id
                JOIN File f ON f.path = rel.source_id
                WHERE (
                    lower(f.path) NOT LIKE '%/tests/%'
                    AND lower(f.path) NOT LIKE '%_test.rs'
                    AND lower(f.path) NOT LIKE '%_test.exs'
                )
                {}
            )
            SELECT
                iface.name,
                impl.name
            FROM symbol_files iface
            JOIN symbol_files impl ON impl.path = iface.path
            WHERE iface.kind = 'interface'
              AND impl.kind IN ('class', 'struct', 'module')
              AND impl.id != iface.id
              AND (
                    impl.lowered_name = iface.lowered_name || 'impl'
                    OR impl.lowered_name = iface.lowered_name || '_impl'
                    OR impl.lowered_name LIKE iface.lowered_name || '%adapter%'
              )
              AND 1 = (
                    SELECT count(*)
                    FROM symbol_files impl2
                    WHERE impl2.path = iface.path
                      AND impl2.kind IN ('class', 'struct', 'module')
                      AND (
                        impl2.lowered_name = iface.lowered_name || 'impl'
                        OR impl2.lowered_name = iface.lowered_name || '_impl'
                        OR impl2.lowered_name LIKE iface.lowered_name || '%adapter%'
                      )
                )
            ORDER BY iface.name ASC, impl.name ASC
            LIMIT 20
            ",
            if scoped {
                format!(" AND s.project_code = '{}'", escaped)
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter(|row| row.len() >= 2)
            .map(|row| format!("{} -> {}", row[0], row[1]))
            .collect())
    }

    pub fn get_orphan_code_symbols(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT DISTINCT s.name
            FROM Symbol s
            LEFT JOIN CONTAINS rel ON rel.target_id = s.id
            LEFT JOIN File f ON f.path = rel.source_id
            WHERE s.kind IN ('function', 'method')
              AND COALESCE(s.is_public, false) = false
              AND NOT EXISTS (
                    SELECT 1
                    FROM SUBSTANTIATES subst
                    WHERE subst.source_id = s.id OR subst.target_id = s.id
              )
              AND NOT EXISTS (
                    SELECT 1
                    FROM IMPACTS imp
                    WHERE imp.source_id = s.id OR imp.target_id = s.id
              )
              AND NOT EXISTS (
                    SELECT 1
                    FROM soll.Traceability t
                    WHERE t.artifact_type = 'Symbol'
                      AND (t.artifact_ref = s.id OR t.artifact_ref = s.name)
              )
              AND (
                f.path IS NULL
                OR (
                    lower(f.path) NOT LIKE '%/tests/%'
                    AND lower(f.path) NOT LIKE '%_test.rs'
                    AND lower(f.path) NOT LIKE '%_test.exs'
                )
              )
              {}
            ORDER BY s.name ASC
            LIMIT 20
            ",
            if scoped {
                format!(
                    " AND s.project_code = '{}' AND (rel.project_code = '{}' OR rel.project_code IS NULL)",
                    escaped, escaped
                )
            } else {
                String::new()
            }
        );
        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.first().cloned())
            .collect())
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
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let base_calls = if scoped {
            format!(
                "SELECT c.source_id, c.target_id
                 FROM CALLS c
                 JOIN Symbol s ON s.id = c.source_id
                 WHERE s.project_code = '{}'",
                escaped
            )
        } else {
            "SELECT source_id, target_id FROM CALLS".to_string()
        };

        let query = format!(
            "
            WITH RECURSIVE call_paths(source_id, target_id, path_ids, path_names, is_cycle) AS (
                SELECT 
                    c.source_id, 
                    c.target_id, 
                    [c.source_id], 
                    [s.name],
                    false
                FROM ({}) c
                JOIN Symbol s ON s.id = c.source_id
                
                UNION ALL
                
                SELECT 
                    p.source_id, 
                    c.target_id, 
                    list_append(p.path_ids, c.source_id),
                    list_append(p.path_names, s.name),
                    list_contains(p.path_ids, c.target_id)
                FROM call_paths p
                JOIN CALLS c ON p.target_id = c.source_id
                JOIN Symbol s ON s.id = c.source_id
                WHERE NOT p.is_cycle AND len(p.path_ids) < 10
            )
            SELECT array_to_string(list_append(path_names, s_target.name), ' -> ') as cycle_path
            FROM call_paths p
            JOIN Symbol s_target ON s_target.id = p.target_id
            WHERE p.is_cycle = true AND len(p.path_ids) > 1;
            ",
            base_calls
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut findings = Vec::new();
        for row in rows {
            if !row.is_empty() {
                findings.push(row[0].clone());
            }
        }
        Ok(findings)
    }

    pub fn get_circular_dependency_count_fast(&self, project: &str) -> Result<i64> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let query = format!(
            "
            SELECT count(*)
            FROM (
                SELECT
                    least(c1.source_id, c1.target_id) AS left_id,
                    greatest(c1.source_id, c1.target_id) AS right_id
                FROM CALLS c1
                JOIN CALLS c2
                  ON c1.source_id = c2.target_id
                 AND c1.target_id = c2.source_id
                WHERE c1.source_id != c1.target_id
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

    pub fn get_domain_leakage(
        &self,
        project: &str,
        domain_path: &str,
        infra_path: &str,
    ) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped_project = project.replace('\'', "''");
        let escaped_domain = domain_path.replace('\'', "''");
        let escaped_infra = infra_path.replace('\'', "''");

        let query = format!(
            "
            SELECT s_domain.name || ' (' || f_domain.path || ') -> ' || s_infra.name || ' (' || f_infra.path || ')'
            FROM CALLS c
            JOIN Symbol s_domain ON c.source_id = s_domain.id
            JOIN CONTAINS c_domain ON c_domain.target_id = s_domain.id
            JOIN File f_domain ON f_domain.path = c_domain.source_id
            
            JOIN Symbol s_infra ON c.target_id = s_infra.id
            JOIN CONTAINS c_infra ON c_infra.target_id = s_infra.id
            JOIN File f_infra ON f_infra.path = c_infra.source_id
            
            WHERE f_domain.path LIKE '%{}%'
              AND f_infra.path LIKE '%{}%'
            {}
            ",
            escaped_domain,
            escaped_infra,
            if scoped {
                format!(" AND s_domain.project_code = '{}'", escaped_project)
            } else {
                String::new()
            }
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut leaks = Vec::new();
        for row in rows {
            if let Some(leak) = row.first() {
                leaks.push(leak.clone());
            }
        }
        Ok(leaks)
    }

    pub fn get_unsafe_exposure(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let scope = if scoped {
            format!(" AND s_src.project_code = '{}'", escaped)
        } else {
            String::new()
        };

        let query = format!(
            "
            WITH RECURSIVE call_paths(source_id, target_id, depth, path_ids, initial_name) AS (
                SELECT 
                    c.source_id, 
                    c.target_id, 
                    1, 
                    [c.source_id],
                    s_src.name
                FROM CALLS c
                JOIN Symbol s_src ON s_src.id = c.source_id
                WHERE COALESCE(s_src.is_public, false) = true
                {scope}
                
                UNION ALL
                
                SELECT 
                    p.source_id, 
                    c.target_id, 
                    p.depth + 1,
                    list_append(p.path_ids, c.target_id),
                    p.initial_name
                FROM call_paths p
                JOIN CALLS c ON p.target_id = c.source_id
                WHERE NOT list_contains(p.path_ids, c.target_id) AND p.depth < 10
            )
            SELECT DISTINCT p.initial_name || ' -> ... -> ' || s_tgt.name
            FROM call_paths p
            JOIN Symbol s_tgt ON s_tgt.id = p.target_id
            WHERE COALESCE(s_tgt.is_unsafe, false) = true OR lower(s_tgt.name) = 'unwrap';
            ",
            scope = scope
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.first().cloned())
            .collect())
    }

    pub fn get_nif_blocking_risks(&self, project: &str) -> Result<Vec<String>> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let scope = if scoped {
            format!(" AND s_nif.project_code = '{}'", escaped)
        } else {
            String::new()
        };

        let query = format!(
            "
            WITH RECURSIVE call_depths(source_id, target_id, depth, path_ids, initial_target_id, initial_name) AS (
                SELECT 
                    c.source_id, 
                    c.target_id, 
                    1, 
                    [c.source_id],
                    c.target_id,
                    s_nif.name
                FROM CALLS_NIF c
                JOIN Symbol s_nif ON s_nif.id = c.target_id
                WHERE 1=1 {scope}
                
                UNION ALL
                
                SELECT 
                    p.source_id, 
                    c.target_id, 
                    p.depth + 1,
                    list_append(p.path_ids, c.target_id),
                    p.initial_target_id,
                    p.initial_name
                FROM call_depths p
                JOIN CALLS c ON p.target_id = c.source_id
                WHERE NOT list_contains(p.path_ids, c.target_id) AND p.depth < 20
            )
            SELECT initial_name || ' (profondeur: ' || max(depth) || ')'
            FROM call_depths
            GROUP BY initial_target_id, initial_name
            HAVING max(depth) > 5;
            ",
            scope = scope
        );

        let res = self.query_json(&query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.first().cloned())
            .collect())
    }
}
