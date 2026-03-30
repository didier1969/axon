use std::ffi::CString;

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::graph::{ExecFunc, FreeStrFunc, GraphStore, QueryCountFunc, QueryJsonFunc};

impl GraphStore {
    pub fn execute(&self, query: &str) -> Result<()> {
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
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
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(query, *guard)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(&expanded, *guard)
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
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
                return Ok(val.as_i64().unwrap_or(0));
            }
        }
        Ok(0)
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        if queries.is_empty() {
            return Ok(());
        }

        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());

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
                _ => return Err(anyhow!("Unsupported parameter type for ${}: {}", key, value)),
            };
            expanded = expanded.replace(&format!("${}", key), &replacement);
        }

        Ok(expanded)
    }
}
