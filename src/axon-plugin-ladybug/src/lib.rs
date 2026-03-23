use lbug::{Connection, Database, SystemConfig};
use std::ffi::{c_char, CStr, CString};
use std::path::Path;

pub struct PluginContext {
    pub db: Database,
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_init_db(path: *const c_char) -> *mut PluginContext {
    // ... (keep existing path logic)
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    if !std::path::Path::new(path_str).exists() {
        if let Some(parent) = std::path::Path::new(path_str).parent() {
            if let Err(_) = std::fs::create_dir_all(parent) {
                return std::ptr::null_mut();
            }
        }
    }

    let config = SystemConfig::default().buffer_pool_size(1024 * 1024 * 1024); // Limit to 1GB RAM (Ghost Mode)
    match Database::new(path_str, config) {
        Ok(db) => {
            let ctx = Box::new(PluginContext { db });
            Box::into_raw(ctx)
        }
        Err(e) => {
            eprintln!("Ladybug C-FFI DB Init Error: {:?}", e);
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_execute(ctx: *mut PluginContext, query: *const c_char) -> bool {
    if ctx.is_null() || query.is_null() {
        return false;
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    
    let ctx_ref = &*ctx;
    
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return false,
    };
    
    match conn.query(query_str) {
        Ok(mut result) => {
            while let Some(_) = result.next() {}
            true
        },
        Err(e) => {
            eprintln!("Error executing query: {} | {}", e, query_str);
            false
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_query_count(ctx: *mut PluginContext, query: *const c_char) -> i64 {
    if ctx.is_null() || query.is_null() {
        return -1;
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    
    let ctx_ref = &*ctx;
    
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return -1,
    };
    
    let mut result = match conn.execute(&mut stmt, kuzu_params) {
        Ok(r) => r,
        Err(_) => return -1,
    };
    
    if let Some(row) = result.next() {
        match row[0] {
            lbug::Value::Int64(v) => v,
            _ => 0,
        }
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_query_json(ctx: *mut PluginContext, query: *const c_char) -> *mut c_char {
    if ctx.is_null() || query.is_null() {
        return CString::new("[]").unwrap().into_raw();
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return CString::new("[]").unwrap().into_raw(),
    };
    
    let ctx_ref = &*ctx;
    
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return -1,
    };
    
    let mut result = match conn.query(query_str) {
        Ok(r) => r,
        Err(e) => return CString::new(format!("Error: {}", e)).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw(),
    };
    
    let mut rows = Vec::new();
    while let Some(row) = result.next() {
        let mut row_vals = Vec::new();
        for val in row {
            row_vals.push(format!("{:?}", val));
        }
        rows.push(row_vals);
    }
    
    let json_str = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
    CString::new(json_str).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_query_count_param(
    ctx: *mut PluginContext,
    query: *const c_char,
    params_json: *const c_char,
) -> i64 {
    if ctx.is_null() || query.is_null() || params_json.is_null() {
        return -1;
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    
    let params_str = match CStr::from_ptr(params_json).to_str() {
        Ok(s) => s,
        Err(_) => "{}",
    };
    
    let ctx_ref = &*ctx;
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return CString::new("[]").unwrap().into_raw(),
    };

    let mut stmt = match conn.prepare(query_str) {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let mut owned_params = Vec::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(params_str) {
        for (k, v) in map {
            if let Some(lbug_v) = json_to_lbug_value(&v) {
                owned_params.push((k, lbug_v));
            }
        }
    }
    
    let kuzu_params: Vec<(&str, lbug::Value)> = owned_params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();

    let mut result = match conn.execute(&mut stmt, kuzu_params) {
        Ok(r) => r,
        Err(_) => return -1,
    };
    
    if let Some(row) = result.next() {
        match row[0] {
            lbug::Value::Int64(v) => v,
            _ => 0,
        }
    } else {
        0
    }
}

fn json_to_lbug_value(v: &serde_json::Value) -> Option<lbug::Value> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(lbug::Value::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(lbug::Value::Int64(i))
            } else if let Some(f) = n.as_f64() {
                Some(lbug::Value::Double(f))
            } else {
                None
            }
        },
        serde_json::Value::String(s) => Some(lbug::Value::String(s.clone())),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Some(lbug::Value::String(v.to_string()))
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_query_json_param(
    ctx: *mut PluginContext,
    query: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    if ctx.is_null() || query.is_null() || params_json.is_null() {
        return CString::new("[]").unwrap().into_raw();
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return CString::new("[]").unwrap().into_raw(),
    };
    
    let params_str = match CStr::from_ptr(params_json).to_str() {
        Ok(s) => s,
        Err(_) => "{}",
    };
    
    let ctx_ref = &*ctx;
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return CString::new("[]").unwrap().into_raw(),
    };

    let mut stmt = match conn.prepare(query_str) {
        Ok(s) => s,
        Err(e) => return CString::new(format!("Error preparing: {}", e)).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw(),
    };

    let mut owned_params = Vec::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(params_str) {
        for (k, v) in map {
            if let Some(lbug_v) = json_to_lbug_value(&v) {
                owned_params.push((k, lbug_v));
            }
        }
    }
    
    let kuzu_params: Vec<(&str, lbug::Value)> = owned_params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();

    let mut result = match conn.execute(&mut stmt, kuzu_params) {
        Ok(r) => r,
        Err(e) => return CString::new(format!("Error executing: {}", e)).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw(),
    };
    
    let mut rows = Vec::new();
    while let Some(row) = result.next() {
        let mut row_vals = Vec::new();
        for val in row {
            row_vals.push(format!("{:?}", val));
        }
        rows.push(row_vals);
    }
    
    let json_res = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
    CString::new(json_res).unwrap_or_else(|_| CString::new("[]").unwrap()).into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_execute_param(
    ctx: *mut PluginContext,
    query: *const c_char,
    params_json: *const c_char,
) -> bool {
    if ctx.is_null() || query.is_null() || params_json.is_null() {
        return false;
    }
    
    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    
    let params_str = match CStr::from_ptr(params_json).to_str() {
        Ok(s) => s,
        Err(_) => "{}",
    };
    
    let ctx_ref = &*ctx;
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mut stmt = match conn.prepare(query_str) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error preparing batch: {} | Query: {}", e, query_str);
            return false;
        }
    };

    let mut owned_params = Vec::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(params_str) {
        for (k, v) in map {
            if let Some(lbug_v) = json_to_lbug_value(&v) {
                owned_params.push((k, lbug_v));
            }
        }
    }
    
    let kuzu_params: Vec<(&str, lbug::Value)> = owned_params.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();

    match conn.execute(&mut stmt, kuzu_params) {
        Ok(mut result) => {
            while let Some(_) = result.next() {}
            true
        },
        Err(e) => {
            eprintln!("Error executing param query: {} | Params: {}", e, params_str);
            false
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_close_db(ctx: *mut PluginContext) {
    if !ctx.is_null() {
        let _ = Box::from_raw(ctx);
    }
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_execute_batch(ctx: *mut PluginContext, queries_json: *const c_char) -> bool {
    if ctx.is_null() || queries_json.is_null() {
        return false;
    }
    
    let json_str = match CStr::from_ptr(queries_json).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    
    let queries: Vec<String> = match serde_json::from_str(json_str) {
        Ok(q) => q,
        Err(_) => return false,
    };
    
    let ctx_ref = &*ctx;
    let conn = match Connection::new(&ctx_ref.db) {
        Ok(c) => c,
        Err(_) => return false,
    };
    
    let _ = conn.query("BEGIN TRANSACTION");
    for query in queries {
        match conn.query(&query) {
            Ok(mut result) => {
                while let Some(_) = result.next() {}
            },
            Err(e) => {
                eprintln!("Batch query failed: {} - Error: {:?}", query, e);
                let _ = conn.query("ROLLBACK");
                return false;
            }
        }
    }
    match conn.query("COMMIT") {
        Ok(mut result) => {
            while let Some(_) = result.next() {}
            true
        },
        Err(_) => false,
    }
}
