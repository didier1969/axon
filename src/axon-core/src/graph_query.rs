use std::ffi::CString;
use std::hash::{Hash, Hasher};

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;
use serde_json::Value;

use crate::graph::{ExecFunc, FreeStrFunc, GraphStore, QueryCountFunc, QueryJsonFunc};

impl GraphStore {
    fn graph_projection_version() -> &'static str {
        "1"
    }

    fn projection_signature(entries: &[String]) -> String {
        let mut normalized = entries.to_vec();
        normalized.sort();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn graph_projection_state_matches(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
        signature: &str,
        version: &str,
    ) -> Result<bool> {
        let res = self.query_json_param(
            "SELECT source_signature, projection_version \
             FROM GraphProjectionState \
             WHERE anchor_type = $anchor_type \
               AND anchor_id = $anchor_id \
               AND radius = $radius \
             LIMIT 1",
            &serde_json::json!({
                "anchor_type": anchor_type,
                "anchor_id": anchor_id,
                "radius": radius,
            }),
        )?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(false);
        };
        let Some(existing_signature) = row.first().and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        let Some(existing_version) = row.get(1).and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        Ok(existing_signature == signature && existing_version == version)
    }

    fn resolve_symbol_anchor_id(&self, symbol: &str) -> Result<Option<String>> {
        let res = self.query_json_param(
            "SELECT id FROM Symbol WHERE id = $sym OR name = $sym LIMIT 1",
            &serde_json::json!({ "sym": symbol }),
        )?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()))
    }

    pub fn refresh_symbol_projection(&self, symbol: &str, radius: u64) -> Result<Option<String>> {
        let Some(anchor_id) = self.resolve_symbol_anchor_id(symbol)? else {
            return Ok(None);
        };

        let radius = radius.max(1) as i64;
        let params = serde_json::json!({
            "anchor": anchor_id,
            "radius": radius,
        });
        let query = "WITH RECURSIVE \
                call_edges(source_id, target_id) AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL \
                    SELECT source_id, target_id FROM CALLS_NIF \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS_NIF \
                ), \
                traverse(node_id, distance) AS ( \
                    SELECT $anchor AS node_id, 0 AS distance \
                    UNION ALL \
                    SELECT e.target_id, t.distance + 1 \
                    FROM call_edges e JOIN traverse t ON e.source_id = t.node_id \
                    WHERE t.distance < $radius \
                ) \
            SELECT node_id, MIN(distance) \
            FROM traverse \
            GROUP BY node_id";
        let res = self.query_json_param(query, &params)?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let created_at = chrono::Utc::now().timestamp_millis();
        let anchor_escaped = anchor_id.replace('\'', "''");
        let version = Self::graph_projection_version();
        let mut signature_entries = vec![format!(
            "symbol|{}|symbol|{}|anchor|0",
            anchor_id, anchor_id
        )];

        for row in &rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(0);
            if node_id == anchor_id {
                continue;
            }
            signature_entries.push(format!(
                "symbol|{}|symbol|{}|call-neighborhood|{}",
                anchor_id, node_id, distance
            ));
        }
        let signature = Self::projection_signature(&signature_entries);

        if self.graph_projection_state_matches("symbol", &anchor_id, radius, &signature, version)? {
            return Ok(Some(anchor_id));
        }

        let mut queries = vec![format!(
            "DELETE FROM GraphProjection WHERE anchor_type = 'symbol' AND anchor_id = '{}' AND radius = {};",
            anchor_escaped, radius
        )];
        queries.push(format!(
            "DELETE FROM GraphProjectionState WHERE anchor_type = 'symbol' AND anchor_id = '{}' AND radius = {};",
            anchor_escaped, radius
        ));

        queries.push(format!(
            "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('symbol', '{}', 'symbol', '{}', 'anchor', 0, {}, '{}', {});",
            anchor_escaped, anchor_escaped, radius, version, created_at
        ));

        for row in rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(0);
            if node_id == anchor_id {
                continue;
            }
            queries.push(format!(
                "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('symbol', '{}', 'symbol', '{}', 'call-neighborhood', {}, {}, '{}', {});",
                anchor_escaped,
                node_id.replace('\'', "''"),
                distance,
                radius,
                version,
                created_at
            ));
        }
        queries.push(format!(
            "INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', '{}', {}, '{}', '{}', {});",
            anchor_escaped, radius, signature, version, created_at
        ));

        self.execute_batch(&queries)?;
        Ok(Some(anchor_id))
    }

    pub fn refresh_file_projection(&self, file_path: &str, radius: u64) -> Result<()> {
        let radius = radius.max(1) as i64;
        let params = serde_json::json!({
            "file": file_path,
            "radius": radius,
        });
        let query = "WITH RECURSIVE \
                call_edges(source_id, target_id) AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL \
                    SELECT target_id, source_id FROM CALLS \
                ), \
                seed(node_id, distance) AS ( \
                    SELECT target_id, 1 AS distance FROM CONTAINS WHERE source_id = $file \
                    UNION ALL \
                    SELECT e.target_id, s.distance + 1 \
                    FROM call_edges e JOIN seed s ON e.source_id = s.node_id \
                    WHERE s.distance < $radius \
                ) \
            SELECT node_id, MIN(distance) \
            FROM seed \
            GROUP BY node_id";
        let res = self.query_json_param(query, &params)?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&res).unwrap_or_default();
        let created_at = chrono::Utc::now().timestamp_millis();
        let file_escaped = file_path.replace('\'', "''");
        let version = Self::graph_projection_version();
        let mut signature_entries = vec![format!("file|{}|file|{}|file|0", file_path, file_path)];

        for row in &rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(1);
            let edge_kind = if distance == 1 {
                "contains"
            } else {
                "call-neighborhood"
            };
            signature_entries.push(format!(
                "file|{}|symbol|{}|{}|{}",
                file_path, node_id, edge_kind, distance
            ));
        }
        let signature = Self::projection_signature(&signature_entries);

        if self.graph_projection_state_matches("file", file_path, radius, &signature, version)? {
            return Ok(());
        }

        let mut queries = vec![format!(
            "DELETE FROM GraphProjection WHERE anchor_type = 'file' AND anchor_id = '{}' AND radius = {};",
            file_escaped, radius
        )];
        queries.push(format!(
            "DELETE FROM GraphProjectionState WHERE anchor_type = 'file' AND anchor_id = '{}' AND radius = {};",
            file_escaped, radius
        ));

        queries.push(format!(
            "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('file', '{}', 'file', '{}', 'file', 0, {}, '{}', {});",
            file_escaped, file_escaped, radius, version, created_at
        ));

        for row in rows {
            let Some(node_id) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let distance = row.get(1).and_then(|value| value.as_i64()).unwrap_or(1);
            let edge_kind = if distance == 1 {
                "contains"
            } else {
                "call-neighborhood"
            };
            queries.push(format!(
                "INSERT INTO GraphProjection (anchor_type, anchor_id, target_type, target_id, edge_kind, distance, radius, projection_version, created_at) VALUES ('file', '{}', 'symbol', '{}', '{}', {}, {}, '{}', {});",
                file_escaped,
                node_id.replace('\'', "''"),
                edge_kind,
                distance,
                radius,
                version,
                created_at
            ));
        }
        queries.push(format!(
            "INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('file', '{}', {}, '{}', '{}', {});",
            file_escaped, radius, signature, version, created_at
        ));

        self.execute_batch(&queries)
    }

    pub fn query_graph_projection(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: u64,
    ) -> Result<String> {
        let query = "SELECT gp.target_type, gp.target_id, gp.edge_kind, gp.distance, \
                            COALESCE(s.name, gp.target_id) AS label, \
                            COALESCE(f.path, contain.source_id, '') AS uri \
                     FROM GraphProjection gp \
                     LEFT JOIN Symbol s ON gp.target_type = 'symbol' AND s.id = gp.target_id \
                     LEFT JOIN CONTAINS contain ON gp.target_type = 'symbol' AND contain.target_id = gp.target_id \
                     LEFT JOIN File f ON gp.target_type = 'file' AND f.path = gp.target_id \
                     WHERE gp.anchor_type = $anchor_type AND gp.anchor_id = $anchor_id AND gp.radius = $radius \
                     ORDER BY gp.distance ASC, gp.edge_kind ASC, label ASC";
        self.query_json_param(
            query,
            &serde_json::json!({
                "anchor_type": anchor_type,
                "anchor_id": anchor_id,
                "radius": radius as i64,
            }),
        )
    }

    pub fn execute(&self, query: &str) -> Result<()> {
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new(query)?.as_ptr()) {
                return Err(anyhow!("Writer Error: {}", query));
            }
        }
        Ok(())
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        let expanded = Self::expand_named_params(query, params)?;
        self.execute(&expanded)
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(query, *guard)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(&expanded, *guard)
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        unsafe {
            let count_fn: LibSymbol<QueryCountFunc> = self.pool.lib.get(b"duckdb_query_count\0")?;
            Ok(count_fn(*guard, CString::new(query)?.as_ptr()))
        }
    }

    pub fn query_count_param(&self, query: &str, params: &serde_json::Value) -> Result<i64> {
        let res = self.query_json_param(query, params)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res).unwrap_or_default();
        if let Some(row) = rows.get(0) {
            if let Some(val) = row.get(0) {
                if let Some(number) = val.as_i64() {
                    return Ok(number);
                }
                if let Some(text) = val.as_str() {
                    return Ok(text.parse::<i64>().unwrap_or(0));
                }
            }
        }
        Ok(0)
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        if queries.is_empty() {
            return Ok(());
        }

        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;

            if !exec_fn(*guard, CString::new("BEGIN TRANSACTION;")?.as_ptr()) {
                return Err(anyhow!("Batch Writer Error: BEGIN TRANSACTION failed"));
            }

            for q in queries {
                if !exec_fn(*guard, CString::new(q.as_str())?.as_ptr()) {
                    let _ = exec_fn(*guard, CString::new("ROLLBACK;")?.as_ptr());
                    return Err(anyhow!("Batch Writer Error on query: {}", q));
                }
            }

            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Batch Writer Error: COMMIT failed"));
            }
        }
        Ok(())
    }

    pub(crate) fn query_on_ctx(&self, query: &str, ctx: *mut std::ffi::c_void) -> Result<String> {
        unsafe {
            let query_fn: LibSymbol<QueryJsonFunc> = self.pool.lib.get(b"duckdb_query_json\0")?;
            let free_fn: LibSymbol<FreeStrFunc> = self.pool.lib.get(b"duckdb_free_string\0")?;
            let ptr = query_fn(ctx, CString::new(query)?.as_ptr());
            if ptr.is_null() {
                return Ok("[]".to_string());
            }
            let res = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
            free_fn(ptr);
            Ok(res)
        }
    }

    fn expand_named_params(query: &str, params: &serde_json::Value) -> Result<String> {
        if let Some(arr) = params.as_array() {
            let mut expanded = query.to_string();
            for value in arr {
                let replacement = match value {
                    serde_json::Value::Null => "NULL".to_string(),
                    serde_json::Value::Bool(v) => v.to_string(),
                    serde_json::Value::Number(v) => v.to_string(),
                    serde_json::Value::String(v) => format!("'{}'", v.replace('\'', "''")),
                    _ => return Err(anyhow!("Unsupported positional parameter type: {}", value)),
                };

                if let Some(pos) = expanded.find('?') {
                    expanded.replace_range(pos..=pos, &replacement);
                } else {
                    return Err(anyhow!("Too many positional parameters supplied"));
                }
            }
            return Ok(expanded);
        }

        let mut expanded = query.to_string();
        let obj = match params.as_object() {
            Some(obj) => obj,
            None => return Ok(expanded),
        };

        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::Null => "NULL".to_string(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::String(v) => format!("'{}'", v.replace('\'', "''")),
                _ => {
                    return Err(anyhow!(
                        "Unsupported parameter type for ${}: {}",
                        key,
                        value
                    ))
                }
            };
            expanded = expanded.replace(&format!("${}", key), &replacement);
        }

        Ok(expanded)
    }
}
