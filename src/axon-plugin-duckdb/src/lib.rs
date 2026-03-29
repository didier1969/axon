use duckdb::{Connection, AccessMode, Config};
use std::ffi::{c_char, CStr, CString};

pub struct PluginContext {
    pub conn: Connection,
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_init_db(path: *const c_char, read_only: bool) -> *mut PluginContext {
    if path.is_null() { return std::ptr::null_mut(); }
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    if path_str != ":memory:" {
        let p = std::path::Path::new(path_str);
        if !read_only && !p.exists() {
            if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
        }
    }

    let config = if read_only {
        match Config::default().access_mode(AccessMode::ReadOnly) {
            Ok(c) => c,
            Err(_) => Config::default()
        }
    } else {
        Config::default()
    };

    let conn_res = if path_str == ":memory:" {
        Connection::open_in_memory_with_flags(config)
    } else {
        Connection::open_with_flags(path_str, config)
    };

    match conn_res {
        Ok(conn) => {
            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
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
    let ctx_ref = &*ctx;
    
    let is_select = query_str.trim().to_lowercase().starts_with("select") || 
                    query_str.trim().to_lowercase().starts_with("with") || 
                    query_str.trim().to_lowercase().starts_with("show") || 
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
        _ => format!("{:?}", v),
    }
}
