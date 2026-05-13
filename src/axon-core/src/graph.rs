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

/// PostgreSQL is the canonical (and only) storage backend (REQ-AXO-271,
/// operator directive 2026-05-12 — purge of DuckDB). The
/// `PluginBackend` enum is retained as a single-variant marker so the
/// existing FFI plumbing keeps compiling while the deeper refactor
/// lands in a follow-up sweep.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginBackend {
    Postgres,
}

impl PluginBackend {
    /// PostgreSQL is the only backend. `AXON_DB_BACKEND` is ignored
    /// and may be removed entirely once every reference is purged.
    pub(crate) fn current() -> Self {
        PluginBackend::Postgres
    }

    pub(crate) fn plugin_filename(self) -> &'static str {
        "libaxon_plugin_postgres.so"
    }

    pub(crate) fn crate_dir(self) -> &'static str {
        "src/axon-plugin-postgres"
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
    pub(crate) unsafe fn resolve(lib: &Library, _backend: PluginBackend) -> anyhow::Result<Self> {
        Self::resolve_postgres(lib)
    }

    /// Resolve the pg_* C symbols. `init_fn` is the `pg_init_db_compat`
    /// shim defined in axon-plugin-postgres; the first argument is
    /// reinterpreted as a `DATABASE_URL`; the `read_only` flag is
    /// ignored (PG handles concurrency server-side).
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

impl GraphStore {
    /// MIL-AXO-015 P3 slice 3f: is this `GraphStore` backed by the
    /// PostgreSQL plugin? Callers (`axon_init_project`,
    /// `bootstrap_global_pg_schema`, etc.) branch on this rather than
    /// re-reading the env var so the answer stays consistent within a
    /// single store's lifetime.
    pub fn is_postgres_backend(&self) -> bool {
        self.pool.symbols.backend == PluginBackend::Postgres
    }

    /// REQ-AXO-251: when true, the SQL relation tables (CALLS, CALLS_NIF,
    /// CONTAINS, IMPACTS, SUBSTANTIATES) are no longer the canonical edge
    /// store. Readers must short-circuit to AGE Cypher (preferred) or
    /// graceful-empty (diagnostic counts) instead of querying the SQL
    /// tables — those are slated for `DROP TABLE` once Stop A flips.
    ///
    /// Post-A.5 (REQ-AXO-216): once the SQL relation tables are physically
    /// dropped, ANY query against them errors. Callers MUST short-circuit
    /// under PG even when the env knob is unset. Hence: PG ⇒ skip
    /// (regardless of AXON_AGE_ONLY_RELATIONS). DuckDB ⇒ unchanged.
    pub fn skip_legacy_relations(&self) -> bool {
        self.is_postgres_backend()
    }

    /// MIL-AXO-015 post-promote helper: 3-part name `soll.main.X` is
    /// DuckDB-only (catalog.schema.table). PostgreSQL parses it as
    /// cross-database `db.schema.table` and rejects it. Use this helper
    /// to qualify SOLL tables in SQL strings that must work on both
    /// backends.
    pub fn soll_table(&self, name: &str) -> String {
        if self.is_postgres_backend() {
            format!("soll.{name}")
        } else {
            format!("soll.main.{name}")
        }
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
