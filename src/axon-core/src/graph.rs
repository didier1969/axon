use libloading::{Library, Symbol as LibSymbol};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingFile {
    pub path: String,
    pub trace_id: String,
    pub priority: i64,
    pub size_bytes: u64,
    pub defer_count: u32,
    pub last_deferred_at_ms: Option<i64>,
}

// FFI Types
pub(crate) type InitDbFunc =
    unsafe extern "C" fn(path: *const std::os::raw::c_char, read_only: bool) -> *mut c_void;
pub(crate) type ExecFunc =
    unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> bool;
pub(crate) type QueryJsonFunc = unsafe extern "C" fn(
    ctx: *mut c_void,
    query: *const std::os::raw::c_char,
) -> *mut std::os::raw::c_char;
pub(crate) type QueryCountFunc =
    unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> i64;
pub(crate) type FreeStrFunc = unsafe extern "C" fn(ptr: *mut std::os::raw::c_char);
pub(crate) type CloseDbFunc = unsafe extern "C" fn(ctx: *mut c_void);

pub(crate) struct LatticePool {
    pub(crate) lib: Arc<Library>,
    pub(crate) writer_ctx: Mutex<*mut c_void>,
    pub(crate) reader_ctx: Mutex<*mut c_void>,
}

unsafe impl Send for LatticePool {}
unsafe impl Sync for LatticePool {}

pub struct GraphStore {
    pub(crate) pool: Arc<LatticePool>,
    pub(crate) db_path: Option<PathBuf>,
}

impl Drop for LatticePool {
    fn drop(&mut self) {
        unsafe {
            let close_fn: LibSymbol<CloseDbFunc> = self.lib.get(b"duckdb_close_db\0").unwrap();
            let writer_ctx = *self.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
            let reader_ctx = *self.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
            if !writer_ctx.is_null() {
                close_fn(writer_ctx);
            }
            if !reader_ctx.is_null() && reader_ctx != writer_ctx {
                close_fn(reader_ctx);
            }
        }
    }
}
