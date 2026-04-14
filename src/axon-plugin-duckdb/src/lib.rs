use duckdb::{Connection, AccessMode, Config};
use std::ffi::{c_char, CStr, CString};

pub struct PluginContext {
    pub conn: Connection,
}

fn plugin_trace_enabled() -> bool {
    std::env::var("AXON_DUCKDB_PLUGIN_TRACE")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn duckpgq_load_enabled() -> bool {
    std::env::var("AXON_DUCKPGQ_LOAD")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_init_db(path: *const c_char, read_only: bool) -> *mut PluginContext {
    if plugin_trace_enabled() {
        eprintln!("[duckdb_init_db] enter read_only={read_only} path_ptr={path:p}");
    }
    if path.is_null() { return std::ptr::null_mut(); }
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_init_db] path={path_str}");
    }

    if path_str != ":memory:" {
        let p = std::path::Path::new(path_str);
        if !read_only && !p.exists() {
            if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
        }
    }
    if plugin_trace_enabled() {
        eprintln!("[duckdb_init_db] path_ready");
    }

    let config = if read_only {
        Config::default()
            .access_mode(AccessMode::ReadOnly)
            .unwrap_or_else(|_| Config::default())
            .allow_unsigned_extensions()
            .unwrap_or_else(|_| Config::default())
    } else {
        Config::default()
            .allow_unsigned_extensions()
            .unwrap_or_else(|_| Config::default())
    };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_init_db] config_ready");
    }

    let conn_res = if path_str == ":memory:" {
        Connection::open_in_memory_with_flags(config)
    } else {
        Connection::open_with_flags(path_str, config)
    };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_init_db] connection_attempted");
    }

    match conn_res {
        Ok(conn) => {
            if plugin_trace_enabled() {
                eprintln!("[duckdb_init_db] connection_open");
            }
            if duckpgq_load_enabled() {
                if let Err(e) = conn.execute("LOAD '/home/dstadel/projects/duckdb-graph/build/release/extension/duckpgq/duckpgq.duckdb_extension'", []) {
                    eprintln!("Failed to load duckpgq extension: {:?}", e);
                } else if plugin_trace_enabled() {
                    eprintln!("[duckdb_init_db] duckpgq_loaded");
                }
            } else if plugin_trace_enabled() {
                eprintln!("[duckdb_init_db] duckpgq_skipped");
            }
            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
            if plugin_trace_enabled() {
                eprintln!("[duckdb_init_db] pragmas_done");
            }
            Box::into_raw(Box::new(PluginContext { conn }))
        }
        Err(e) => {
            eprintln!("DuckDB Init Error (RO={}): {:?}", read_only, e);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_execute(ctx: *mut PluginContext, query: *const c_char) -> bool {
    if ctx.is_null() || query.is_null() { return false; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return false };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_execute] {}", query_str);
    }
    let ctx_ref = &*ctx;
    match ctx_ref.conn.execute_batch(query_str) {
        Ok(_) => true,
        Err(e) => { eprintln!("Error executing query: {} | {}", e, query_str); false }
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_execute_param(ctx: *mut PluginContext, query: *const c_char, params_json: *const c_char) -> bool {
    if ctx.is_null() || query.is_null() || params_json.is_null() { return false; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return false };
    let params_str = match CStr::from_ptr(params_json).to_str() { Ok(s) => s, Err(_) => return false };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_execute_param] {} | {}", query_str, params_str);
    }
    let ctx_ref = &*ctx;

    let params: serde_json::Value = serde_json::from_str(params_str).unwrap_or(serde_json::Value::Null);
    let res = if let serde_json::Value::Array(arr) = params {
        let duck_params: Vec<String> = arr.iter().map(|v| {
            if let Some(s) = v.as_str() { s.to_string() } else { v.to_string().replace("\"", "") }
        }).collect();
        let param_refs: Vec<&dyn duckdb::ToSql> = duck_params.iter().map(|s| s as &dyn duckdb::ToSql).collect();
        ctx_ref.conn.execute(query_str, param_refs.as_slice())
    } else {
        ctx_ref.conn.execute(query_str, [])
    };

    match res {
        Ok(_) => true,
        Err(e) => { eprintln!("Param Execute Error: {} | {}", e, query_str); false }
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_query_count(ctx: *mut PluginContext, query: *const c_char) -> i64 {
    if ctx.is_null() || query.is_null() { return -1; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return -1 };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_query_count] {}", query_str);
    }
    let ctx_ref = &*ctx;
    match ctx_ref.conn.query_row(query_str, [], |row| row.get::<_, i64>(0)) {
        Ok(v) => v,
        Err(e) => { eprintln!("Count Error: {} | Query: {}", e, query_str); -1 },
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_query_json(ctx: *mut PluginContext, query: *const c_char) -> *mut c_char {
    if ctx.is_null() || query.is_null() { return CString::new("[]").unwrap().into_raw(); }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return CString::new("[]").unwrap().into_raw() };
    if plugin_trace_enabled() {
        eprintln!("[duckdb_query_json] {}", query_str);
    }
    let ctx_ref = &*ctx;
    
    let is_select = query_str.trim().to_lowercase().starts_with("select") ||
                    query_str.trim().to_lowercase().starts_with("with") ||
                    query_str.trim().to_lowercase().starts_with("show") ||
                    query_str.trim().to_lowercase().starts_with("-from") ||
                    query_str.trim().to_lowercase().starts_with("-match") ||
                    query_str.to_lowercase().contains("returning");    
    if !is_select {
        match ctx_ref.conn.execute(query_str, []) {
            Ok(_) => return CString::new("[]").unwrap().into_raw(),
            Err(e) => {
                eprintln!("Query Execution Error: {} | {}", e, query_str);
                return CString::new("[]").unwrap().into_raw();
            }
        }
    }

    let mut stmt = match ctx_ref.conn.prepare(query_str) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Prepare Error: {} | Query: {}", e, query_str);
            return CString::new("[]").unwrap().into_raw();
        }
    };

    let rows = stmt.query_map([], |row| {
        let count = row.as_ref().column_count();
        let mut row_data = Vec::new();
        for i in 0..count {
            let val: duckdb::types::Value = row.get(i).unwrap_or(duckdb::types::Value::Null);
            row_data.push(format_duckdb_value(&val));
        }
        Ok(row_data)
    });

    match rows {
        Ok(mapped_rows) => {
            let results: Vec<Vec<String>> = mapped_rows.filter_map(|r| r.ok()).collect();
            let json = serde_json::to_string(&results).unwrap_or("[]".to_string());
            CString::new(json).unwrap().into_raw()
        },
        Err(_) => CString::new("[]").unwrap().into_raw()
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_free_string(ptr: *mut c_char) {
    if !ptr.is_null() { let _ = CString::from_raw(ptr); }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_close_db(ctx: *mut PluginContext) {
    if !ctx.is_null() { let _ = Box::from_raw(ctx); }
}

fn format_duckdb_value(v: &duckdb::types::Value) -> String {
    match v {
        duckdb::types::Value::Null => "null".to_string(),
        duckdb::types::Value::Boolean(b) => b.to_string(),
        duckdb::types::Value::TinyInt(i) => i.to_string(),
        duckdb::types::Value::SmallInt(i) => i.to_string(),
        duckdb::types::Value::Int(i) => i.to_string(),
        duckdb::types::Value::BigInt(i) => i.to_string(),
        duckdb::types::Value::Float(f) => f.to_string(),
        duckdb::types::Value::Double(f) => f.to_string(),
        duckdb::types::Value::Text(s) => s.clone(),
        duckdb::types::Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
        duckdb::types::Value::HugeInt(i) => i.to_string(),
        duckdb::types::Value::UTinyInt(i) => i.to_string(),
        duckdb::types::Value::USmallInt(i) => i.to_string(),
        duckdb::types::Value::UInt(i) => i.to_string(),
        duckdb::types::Value::UBigInt(i) => i.to_string(),
        duckdb::types::Value::Decimal(d) => d.to_string(),
        duckdb::types::Value::Date32(d) => d.to_string(),
        duckdb::types::Value::Time64(_, t) => t.to_string(),
        duckdb::types::Value::Timestamp(_, ts) => ts.to_string(),
        duckdb::types::Value::Interval { months, days, nanos } => {
            format!("{{\"months\":{months},\"days\":{days},\"nanos\":{nanos}}}")
        }
        duckdb::types::Value::Enum(s) => s.clone(),
        duckdb::types::Value::List(items) | duckdb::types::Value::Array(items) => {
            let rendered = items
                .iter()
                .map(format_duckdb_value)
                .collect::<Vec<_>>();
            serde_json::to_string(&rendered).unwrap_or_else(|_| "[]".to_string())
        }
        duckdb::types::Value::Struct(entries) => {
            let mut rendered = serde_json::Map::new();
            for (key, value) in entries.iter() {
                rendered.insert(key.clone(), serde_json::Value::String(format_duckdb_value(value)));
            }
            serde_json::Value::Object(rendered).to_string()
        }
        duckdb::types::Value::Map(entries) => {
            let rendered = entries
                .iter()
                .map(|(key, value)| {
                    (
                        format_duckdb_value(key),
                        serde_json::Value::String(format_duckdb_value(value)),
                    )
                })
                .collect::<serde_json::Map<String, serde_json::Value>>();
            serde_json::Value::Object(rendered).to_string()
        }
        duckdb::types::Value::Union(value) => format_duckdb_value(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(name: &str) -> CString {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        CString::new(format!("/tmp/{}_{}.duckdb", name, nanos)).unwrap()
    }

    #[test]
    fn query_count_survives_table_with_1024_float_array() {
        unsafe {
            let path = temp_db_path("axon_plugin_count_1024");
            let ctx = duckdb_init_db(path.as_ptr(), false);
            assert!(!ctx.is_null());

            let create = CString::new(
                "CREATE TABLE IF NOT EXISTS embeddings (id VARCHAR, embedding FLOAT[1024]);\
                 INSERT INTO embeddings VALUES ('a', CAST([1.0] || repeat([0.0], 1023) AS FLOAT[1024]));",
            )
            .unwrap();
            assert!(duckdb_execute(ctx, create.as_ptr()));

            let count = CString::new("SELECT count(*) FROM embeddings").unwrap();
            assert_eq!(duckdb_query_count(ctx, count.as_ptr()), 1);

            duckdb_close_db(ctx);
        }
    }

    #[test]
    fn query_json_formats_1024_float_array_without_debug_fallback() {
        unsafe {
            let path = temp_db_path("axon_plugin_json_1024");
            let ctx = duckdb_init_db(path.as_ptr(), false);
            assert!(!ctx.is_null());

            let create = CString::new(
                "CREATE TABLE IF NOT EXISTS embeddings (id VARCHAR, embedding FLOAT[1024]);\
                 INSERT INTO embeddings VALUES ('a', CAST([1.0] || repeat([0.0], 1023) AS FLOAT[1024]));",
            )
            .unwrap();
            assert!(duckdb_execute(ctx, create.as_ptr()));

            let query = CString::new("SELECT embedding FROM embeddings").unwrap();
            let raw = duckdb_query_json(ctx, query.as_ptr());
            assert!(!raw.is_null());
            let json = CStr::from_ptr(raw).to_str().unwrap().to_string();
            duckdb_free_string(raw);

            assert!(json.starts_with("[["));
            assert!(json.contains("1"));

            duckdb_close_db(ctx);
        }
    }
}
