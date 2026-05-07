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

/// MIL-AXO-015 P3 slice 3b: backend selector consumed by
/// `PluginSymbols::resolve` and `find_plugin_path`. Defaults to
/// `Duckdb` so existing deployments keep their current behaviour;
/// callers opt into PostgreSQL via `AXON_DB_BACKEND=postgres`
/// (DEC-AXO-075).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginBackend {
    Duckdb,
    Postgres,
}

impl PluginBackend {
    /// Read `AXON_DB_BACKEND` once. Recognised values: `postgres`,
    /// `postgresql`, `pg` → `Postgres`. Anything else (including unset
    /// or empty) → `Duckdb`. Case-insensitive.
    pub(crate) fn current() -> Self {
        match std::env::var("AXON_DB_BACKEND")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("postgres") | Some("postgresql") | Some("pg") => PluginBackend::Postgres,
            _ => PluginBackend::Duckdb,
        }
    }

    /// Filename of the plugin shared object as built by the matching
    /// crate's `cargo build`. `find_plugin_path` searches the per-crate
    /// `target/{release,debug}` paths for this file.
    pub(crate) fn plugin_filename(self) -> &'static str {
        match self {
            PluginBackend::Duckdb => "libaxon_plugin_duckdb.so",
            PluginBackend::Postgres => "libaxon_plugin_postgres.so",
        }
    }

    /// Per-backend `src/<crate>/` path segment used by
    /// `find_plugin_path` to locate the .so under the repo tree.
    pub(crate) fn crate_dir(self) -> &'static str {
        match self {
            PluginBackend::Duckdb => "src/axon-plugin-duckdb",
            PluginBackend::Postgres => "src/axon-plugin-postgres",
        }
    }
}

/// MIL-AXO-015 P3 slice 3a: cached FFI fn pointers for the loaded
/// plugin. Resolved once at `LatticePool` construction; all subsequent
/// hot-path call sites read directly from these fields rather than
/// hitting `Library::get` again per call. Slice 3b adds the `backend`
/// discriminator + `resolve_postgres` so the same surface can be
/// fronted by either axon-plugin-duckdb or axon-plugin-postgres.
#[derive(Copy, Clone)]
pub(crate) struct PluginSymbols {
    pub(crate) backend: PluginBackend,
    pub(crate) init_fn: InitDbFunc,
    pub(crate) close_fn: CloseDbFunc,
    pub(crate) exec_fn: ExecFunc,
    pub(crate) query_count_fn: QueryCountFunc,
    pub(crate) query_json_fn: QueryJsonFunc,
    pub(crate) free_str_fn: FreeStrFunc,
}

impl PluginSymbols {
    /// Resolve every plugin symbol up-front against the loaded
    /// dynamic library, dispatching by backend.
    ///
    /// # Safety
    ///
    /// `lib` must remain alive for the lifetime of the returned
    /// `PluginSymbols` because the cached fn pointers reference
    /// addresses inside the loaded image. `LatticePool` enforces this
    /// by owning the `Arc<Library>` alongside the symbols.
    pub(crate) unsafe fn resolve(lib: &Library, backend: PluginBackend) -> anyhow::Result<Self> {
        match backend {
            PluginBackend::Duckdb => Self::resolve_duckdb(lib),
            PluginBackend::Postgres => Self::resolve_postgres(lib),
        }
    }

    /// Resolve the duckdb_* C symbols.
    ///
    /// # Safety
    ///
    /// See `PluginSymbols::resolve`.
    pub(crate) unsafe fn resolve_duckdb(lib: &Library) -> anyhow::Result<Self> {
        Ok(Self {
            backend: PluginBackend::Duckdb,
            init_fn: *lib.get::<InitDbFunc>(b"duckdb_init_db\0")?,
            close_fn: *lib.get::<CloseDbFunc>(b"duckdb_close_db\0")?,
            exec_fn: *lib.get::<ExecFunc>(b"duckdb_execute\0")?,
            query_count_fn: *lib.get::<QueryCountFunc>(b"duckdb_query_count\0")?,
            query_json_fn: *lib.get::<QueryJsonFunc>(b"duckdb_query_json\0")?,
            free_str_fn: *lib.get::<FreeStrFunc>(b"duckdb_free_string\0")?,
        })
    }

    /// Resolve the pg_* C symbols. `init_fn` resolves to the
    /// `pg_init_db_compat` shim defined in axon-plugin-postgres so the
    /// signature stays identical to the duckdb plugin's `duckdb_init_db`.
    /// The first argument is reinterpreted as a `DATABASE_URL`; the
    /// `read_only` flag is ignored (PG handles concurrency server-side).
    ///
    /// # Safety
    ///
    /// See `PluginSymbols::resolve`.
    pub(crate) unsafe fn resolve_postgres(lib: &Library) -> anyhow::Result<Self> {
        Ok(Self {
            backend: PluginBackend::Postgres,
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
