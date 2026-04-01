// Copyright (c) Didier Stadelmann. All rights reserved.

use anyhow::Result;

use crate::graph::GraphStore;

impl GraphStore {
    pub fn get_security_audit(&self, project: &str) -> Result<(i64, String)> {
        let scoped = project != "*";
        let escaped = project.replace('\'', "''");
        let scope = if scoped {
            format!(" AND s1.project_slug = '{}' ", escaped)
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
                "SELECT count(*) FROM Symbol WHERE project_slug = '{}'",
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
                "SELECT count(*) FROM Symbol WHERE project_slug = '{}' AND tested = true",
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
            WHERE lower(s.name) LIKE '%todo%'
               OR lower(s.name) LIKE '%fixme%'
               OR lower(s.name) LIKE '%secret%'
               OR lower(s.name) LIKE '%hardcoded credential%'
               OR EXISTS (
                    SELECT 1 FROM CALLS call
                    JOIN Symbol target ON target.id = call.target_id
                    WHERE call.source_id = s.id
                      AND lower(target.name) IN ('unwrap', 'eval')
               )
            {}
        ",
            if scoped {
                format!(
                    " AND (f.project_slug = '{}' OR s.project_slug = '{}')",
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
            {}
            GROUP BY s.name
            HAVING count(*) >= 5
        ",
            if scoped {
                format!("WHERE s.project_slug = '{}'", escaped)
            } else {
                String::new()
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
}
