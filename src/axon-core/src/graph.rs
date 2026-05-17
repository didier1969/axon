use libloading::Library;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex};

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

/// MIL-AXO-015 P3 slice 3a: cached FFI fn pointers for the loaded
/// `axon-plugin-postgres` cdylib. Resolved once at `LatticePool`
/// construction; all subsequent hot-path call sites read directly
/// from these fields rather than hitting `Library::get` again per call.
#[derive(Copy, Clone)]
pub(crate) struct PluginSymbols {
    pub(crate) init_fn: InitDbFunc,
    pub(crate) close_fn: CloseDbFunc,
    pub(crate) exec_fn: ExecFunc,
    pub(crate) query_count_fn: QueryCountFunc,
    pub(crate) query_json_fn: QueryJsonFunc,
    pub(crate) free_str_fn: FreeStrFunc,
}

impl PluginSymbols {
    /// Resolve the pg_* C symbols. `init_fn` is the `pg_init_db_compat`
    /// shim defined in axon-plugin-postgres; the first argument is
    /// reinterpreted as a `DATABASE_URL`; the `read_only` flag is
    /// ignored (PG handles concurrency server-side).
    ///
    /// # Safety
    ///
    /// `lib` must remain alive for the lifetime of the returned
    /// `PluginSymbols` because the cached fn pointers reference
    /// addresses inside the loaded image. `LatticePool` enforces this
    /// by owning the `Arc<Library>` alongside the symbols.
    pub(crate) unsafe fn resolve(lib: &Library) -> anyhow::Result<Self> {
        Ok(Self {
            init_fn: *lib.get::<InitDbFunc>(b"pg_init_db_compat\0")?,
            close_fn: *lib.get::<CloseDbFunc>(b"pg_close_db\0")?,
            exec_fn: *lib.get::<ExecFunc>(b"pg_execute\0")?,
            query_count_fn: *lib.get::<QueryCountFunc>(b"pg_query_count\0")?,
            query_json_fn: *lib.get::<QueryJsonFunc>(b"pg_query_json\0")?,
            free_str_fn: *lib.get::<FreeStrFunc>(b"pg_free_string\0")?,
        })
    }
}

pub(crate) struct LatticePool {
    pub(crate) lib: Arc<Library>,
    pub(crate) symbols: PluginSymbols,
    pub(crate) writer_ctx: Mutex<*mut c_void>,
    pub(crate) reader_ctx: Mutex<*mut c_void>,
}

unsafe impl Send for LatticePool {}
unsafe impl Sync for LatticePool {}

#[derive(Debug)]
pub(crate) struct ReaderSnapshotState {
    pub(crate) commit_epoch: AtomicU64,
    pub(crate) reader_epoch: AtomicU64,
    pub(crate) refresh_inflight: AtomicBool,
    pub(crate) refresh_requested_epoch: AtomicU64,
    pub(crate) last_refresh_started_ms: AtomicU64,
    pub(crate) last_refresh_completed_ms: AtomicU64,
    pub(crate) refresh_coalesced_total: AtomicU64,
    pub(crate) reads_on_reader_total: AtomicU64,
    pub(crate) reads_on_writer_total: AtomicU64,
    pub(crate) fresh_required_fallback_writer_total: AtomicU64,
}

impl ReaderSnapshotState {
    pub(crate) fn new(now_ms: u64) -> Self {
        Self {
            commit_epoch: AtomicU64::new(1),
            reader_epoch: AtomicU64::new(1),
            refresh_inflight: AtomicBool::new(false),
            refresh_requested_epoch: AtomicU64::new(1),
            last_refresh_started_ms: AtomicU64::new(now_ms),
            last_refresh_completed_ms: AtomicU64::new(now_ms),
            refresh_coalesced_total: AtomicU64::new(0),
            reads_on_reader_total: AtomicU64::new(0),
            reads_on_writer_total: AtomicU64::new(0),
            fresh_required_fallback_writer_total: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReaderSnapshotDiagnostics {
    pub(crate) commit_epoch: u64,
    pub(crate) reader_epoch: u64,
    pub(crate) reader_epoch_lag: u64,
    pub(crate) refresh_inflight: bool,
    pub(crate) refresh_requested_epoch: u64,
    pub(crate) last_refresh_started_ms: u64,
    pub(crate) last_refresh_completed_ms: u64,
    pub(crate) refresh_coalesced_total: u64,
    pub(crate) reads_on_reader_total: u64,
    pub(crate) reads_on_writer_total: u64,
    pub(crate) fresh_required_fallback_writer_total: u64,
    pub(crate) reader_refresh_failures_total: u64,
}

impl GraphStore {
    /// REQ-AXO-271 slice 2d : under PG canonical (post-MIL-AXO-017 / AGE
    /// retirement), the legacy SQL relation tables (CALLS, CALLS_NIF,
    /// CONTAINS, IMPACTS, SUBSTANTIATES) are dropped or empty. Callers
    /// uniformly short-circuit reads to `public.Edge` + the SQL graph
    /// function library (`callers_of`, `path`). The function is kept as
    /// a stable read-path predicate ; the value is now invariantly `true`.
    pub fn skip_legacy_relations(&self) -> bool {
        true
    }

    /// REQ-AXO-271 slice 2d : SOLL lives in a single PG schema (`soll`) ;
    /// the historical DuckDB ATTACH'd `soll.main.X` 3-part form is gone.
    pub fn soll_table(&self, name: &str) -> String {
        format!("soll.{name}")
    }
}

pub struct GraphStore {
    pub(crate) pool: Arc<LatticePool>,
    pub(crate) db_path: Option<PathBuf>,
    pub(crate) reader_only_ist_mode: bool,
    pub(crate) soll_attached: bool,
    pub(crate) soll_read_only_mode: bool,
    pub(crate) recent_write_epoch_ms: AtomicU64,
    pub(crate) last_reader_refresh_epoch_ms: AtomicU64,
    pub(crate) reader_refresh_failures_total: AtomicU64,
    pub(crate) reader_state: ReaderSnapshotState,
    pub(crate) reader_refresh_wait: Mutex<u64>,
    pub(crate) reader_refresh_notify: Condvar,
}

impl Drop for LatticePool {
    fn drop(&mut self) {
        unsafe {
            let close_fn = self.symbols.close_fn;
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
