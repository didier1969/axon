use duckdb::Connection;
use std::ffi::{c_char, CStr, CString};

pub struct PluginContext {
    pub conn: Connection,
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_init_db(path: *const c_char) -> *mut PluginContext {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    if path_str != ":memory:" {
        if !std::path::Path::new(path_str).exists() {
            if let Some(parent) = std::path::Path::new(path_str).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }

    let conn_res = if path_str == ":memory:" {
        Connection::open_in_memory()
    } else {
        Connection::open(path_str)
    };

    match conn_res {
        Ok(conn) => {
            // Install and load duckpgq and vss
            let _ = conn.execute_batch("INSTALL duckpgq; LOAD duckpgq;");
            let _ = conn.execute_batch("INSTALL vss; LOAD vss;");

            let ctx = Box::new(PluginContext { conn });
            Box::into_raw(ctx)
        }
        Err(e) => {
            eprintln!("DuckDB C-FFI Init Error: {:?}", e);
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
pub unsafe extern "C" fn duckdb_query_count(ctx: *mut PluginContext, query: *const c_char) -> i64 {
    if ctx.is_null() || query.is_null() { return -1; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return -1 };
    let ctx_ref = &*ctx;
    
    match ctx_ref.conn.query_row(query_str, [], |row| row.get::<_, i64>(0)) {
        Ok(v) => v,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_query_json(ctx: *mut PluginContext, query: *const c_char) -> *mut c_char {
    if ctx.is_null() || query.is_null() { return CString::new("[]").unwrap().into_raw(); }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return CString::new("[]").unwrap().into_raw() };
    let ctx_ref = &*ctx;
    
    match ctx_ref.conn.prepare(query_str) {
        Ok(mut stmt) => {
            let col_count = stmt.column_count();
            let mut rows_out = Vec::new();
            
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    let mut row_vals = Vec::new();
                    for i in 0..col_count {
                        let val: duckdb::types::Value = row.get(i).unwrap_or(duckdb::types::Value::Null);
                        row_vals.push(format!("{:?}", val));
                    }
                    rows_out.push(row_vals);
                }
            }
            let json_str = serde_json::to_string(&rows_out).unwrap_or_else(|_| "[]".to_string());
            CString::new(json_str).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw()
        },
        Err(e) => CString::new(format!("Error: {}", e)).unwrap().into_raw(),
    }
}

fn json_to_duckdb_value(v: &serde_json::Value) -> duckdb::types::Value {
    match v {
        serde_json::Value::Null => duckdb::types::Value::Null,
        serde_json::Value::Bool(b) => duckdb::types::Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() { duckdb::types::Value::BigInt(i) } 
            else if let Some(f) = n.as_f64() { duckdb::types::Value::Double(f) } 
            else { duckdb::types::Value::Null }
        },
        serde_json::Value::String(s) => duckdb::types::Value::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => duckdb::types::Value::Text(v.to_string())
    }
}

fn parse_params(params_str: &str) -> Vec<duckdb::types::Value> {
    let mut params_vec = Vec::new();
    if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(params_str) {
        for v in arr { params_vec.push(json_to_duckdb_value(&v)); }
    } else if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(params_str) {
        // Just extract values for now if it's an object, assuming positional alignment
        for (_, v) in map { params_vec.push(json_to_duckdb_value(&v)); }
    }
    params_vec
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_query_count_param(ctx: *mut PluginContext, query: *const c_char, params_json: *const c_char) -> i64 {
    if ctx.is_null() || query.is_null() || params_json.is_null() { return -1; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return -1 };
    let params_str = match CStr::from_ptr(params_json).to_str() { Ok(s) => s, Err(_) => "{}" };
    let ctx_ref = &*ctx;
    
    let params_vec = parse_params(params_str);
    let params_refs: Vec<&dyn duckdb::ToSql> = params_vec.iter().map(|v| v as &dyn duckdb::ToSql).collect();

    let mut stmt = match ctx_ref.conn.prepare(query_str) { Ok(s) => s, Err(_) => return -1 };

    match stmt.query_row(params_refs.as_slice(), |row: &duckdb::Row| row.get::<_, i64>(0)) {
        Ok(v) => v,
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_query_json_param(ctx: *mut PluginContext, query: *const c_char, params_json: *const c_char) -> *mut c_char {
    if ctx.is_null() || query.is_null() || params_json.is_null() { return CString::new("[]").unwrap().into_raw(); }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return CString::new("[]").unwrap().into_raw() };
    let params_str = match CStr::from_ptr(params_json).to_str() { Ok(s) => s, Err(_) => "{}" };
    let ctx_ref = &*ctx;
    
    let params_vec = parse_params(params_str);
    let params_refs: Vec<&dyn duckdb::ToSql> = params_vec.iter().map(|v| v as &dyn duckdb::ToSql).collect();

    let mut stmt = match ctx_ref.conn.prepare(query_str) { Ok(s) => s, Err(e) => return CString::new(format!("Error: {}", e)).unwrap().into_raw() };
    let col_count = stmt.column_count();
    
    match stmt.query(params_refs.as_slice()) {
        Ok(mut rows) => {
            let mut rows_out = Vec::new();
            while let Ok(Some(row)) = rows.next() {
                let mut rv = Vec::new();
                for i in 0..col_count {
                    let val: duckdb::types::Value = row.get(i).unwrap_or(duckdb::types::Value::Null);
                    rv.push(format!("{:?}", val));
                }
                rows_out.push(rv);
            }
            let js = serde_json::to_string(&rows_out).unwrap_or_else(|_| "[]".to_string());
            CString::new(js).unwrap().into_raw()
        },
        Err(e) => CString::new(format!("Error: {}", e)).unwrap().into_raw(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_execute_param(ctx: *mut PluginContext, query: *const c_char, params_json: *const c_char) -> bool {
    if ctx.is_null() || query.is_null() || params_json.is_null() { return false; }
    let query_str = match CStr::from_ptr(query).to_str() { Ok(s) => s, Err(_) => return false };
    let params_str = match CStr::from_ptr(params_json).to_str() { Ok(s) => s, Err(_) => "{}" };
    let ctx_ref = &*ctx;
    
    let params_vec = parse_params(params_str);
    let params_refs: Vec<&dyn duckdb::ToSql> = params_vec.iter().map(|v| v as &dyn duckdb::ToSql).collect();

    let mut stmt = match ctx_ref.conn.prepare(query_str) { Ok(s) => s, Err(_) => return false };

    match stmt.execute(params_refs.as_slice()) {
        Ok(_) => true,
        Err(_) => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_execute_batch(ctx: *mut PluginContext, queries_json: *const c_char) -> bool {
    if ctx.is_null() || queries_json.is_null() { return false; }
    let json_str = match CStr::from_ptr(queries_json).to_str() { Ok(s) => s, Err(_) => return false };
    let queries: Vec<String> = match serde_json::from_str(json_str) { Ok(q) => q, Err(_) => return false };
    let ctx_ref = &*ctx;
    
    if let Err(_) = ctx_ref.conn.execute_batch("BEGIN TRANSACTION") { return false; }
    for q in queries {
        if let Err(_) = ctx_ref.conn.execute_batch(&q) { 
            let _ = ctx_ref.conn.execute_batch("ROLLBACK"); 
            return false; 
        }
    }
    match ctx_ref.conn.execute_batch("COMMIT") { Ok(_) => true, Err(_) => false }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_free_string(s: *mut c_char) {
    if !s.is_null() { let _ = unsafe { CString::from_raw(s) }; }
}

#[no_mangle]
pub unsafe extern "C" fn duckdb_close_db(ctx: *mut PluginContext) {
    if !ctx.is_null() { let _ = unsafe { Box::from_raw(ctx) }; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_duckdb_init() {
        let path = CString::new(":memory:").unwrap();
        unsafe {
            let ctx = duckdb_init_db(path.as_ptr());
            assert!(!ctx.is_null());
            
            // Check that we can execute a basic query
            let query = CString::new("SELECT 42;").unwrap();
            let res = duckdb_execute(ctx, query.as_ptr());
            assert!(res);
            
            duckdb_close_db(ctx);
        }
    }
}