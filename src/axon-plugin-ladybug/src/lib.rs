use lbug::{Connection, Database, SystemConfig};
use std::ffi::{c_char, CStr, CString};
use std::path::Path;

pub struct PluginContext {
    db: Database,
}

#[no_mangle]
pub unsafe extern "C" fn ladybug_init_db(path: *const c_char) -> *mut PluginContext {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    
    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };
    
    if !Path::new(path_str).exists() {
        if let Err(_) = std::fs::create_dir_all(path_str) {
            return std::ptr::null_mut();
        }
    }
    
    let config = SystemConfig::default();
    match Database::new(path_str, config) {
        Ok(db) => {
            let ctx = Box::new(PluginContext { db });
            Box::into_raw(ctx)
        }
        Err(_) => std::ptr::null_mut()
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
    
    conn.query(query_str).is_ok()
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
    
    let mut result = match conn.query(query_str) {
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
        Err(_) => return CString::new("[]").unwrap().into_raw(),
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
