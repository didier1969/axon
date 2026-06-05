use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingFile {
    pub path: String,
    pub trace_id: String,
    pub priority: i64,
    pub size_bytes: u64,
    pub defer_count: u32,
    pub last_deferred_at_ms: Option<i64>,
}

/// REQ-AXO-901881 W2 / REQ-AXO-901880 — the store's PG connection layer.
/// Formerly an FFI cdylib (`axon-plugin-postgres`) loaded via `libloading`
/// and called through a `*mut c_void` writer ctx + a single mutex; now a
/// native in-process `deadpool_postgres` pool ([`NativePgCtx`]) — one fewer
/// connection stack, no C-ABI marshalling, no writer-mutex serialization,
/// no per-store tokio runtime. `NativePgCtx` is auto `Send + Sync`
/// (`Pool` + `Option<String>`), so no `unsafe impl` is needed.
pub(crate) struct LatticePool {
    pub(crate) native: crate::postgres::native::NativePgCtx,
}

impl GraphStore {
    /// REQ-AXO-271 slice 2d : under PG canonical (post-MIL-AXO-017 / AGE
    /// retirement), the legacy SQL relation tables (CALLS, CALLS_NIF,
    /// CONTAINS, IMPACTS, SUBSTANTIATES) are dropped or empty. Callers
    /// uniformly short-circuit reads to `ist.Edge` + the SQL graph
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
    // Construction-time SOLL access mode recorded from `SollAccessMode`.
    // Read only by tests now that the hand-rolled `ensure_additive_soll_schema`
    // (the former runtime reader of these flags) is retired — the canonical
    // `01_soll_schema.sql` bootstrap no longer branches on access mode. Kept as
    // the store's recorded state contract (and `soll_attached` is test-asserted).
    #[allow(dead_code)]
    pub(crate) soll_attached: bool,
    #[allow(dead_code)]
    pub(crate) soll_read_only_mode: bool,
}
// REQ-AXO-901881 W2 — no manual Drop: the native deadpool pool releases its
// connections on drop, and the runtime is the process-global native_runtime
// (never dropped here), eliminating the FFI close_fn + the per-store
// runtime-drop hazard.
