use crate::graph::GraphStore;
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

pub fn embedder_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// REQ-AXO-901669 — per-invocation unique scope tag for tests that
/// observe shared PG state (e.g. `axon_runtime.VectorWorkerFault`,
/// `VectorLaneState`).
///
/// Cargo runs `--lib` tests in parallel against the same dev PG instance
/// (resolved from `AXON_DEV_DATABASE_URL`). Hardcoded scope labels such
/// as `"vector"` collide both with sibling tests and with persisted
/// telemetry left by prior live/indexer runs. Tests that assert
/// emptiness must therefore use a fresh scope label per invocation.
///
/// The returned string is stable inside one test (one helper call) and
/// distinct across calls thanks to the process pid + monotonic counter
/// + nanosecond timestamp. Use as e.g. `unique_test_scope("worker-fault")`
/// → `"worker-fault-37-12345-1779216801234567890"`.
pub fn unique_test_scope(label: &str) -> String {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{label}-{count}-{pid}-{now}")
}

/// REQ-AXO-91560 — per-invocation unique canonical project_code for tests
/// that mutate `soll.Registry` / `soll.Node` (shared PG instance under
/// MIL-AXO-017). `soll.Registry.project_code` is PRIMARY KEY and the
/// canonical shape is `^[A-Z][A-Z0-9]{2}$` (3 chars, see
/// `db/ddl/01_soll_schema.sql`). Hardcoded `"AXO"` literals collide
/// across the ~158 parallel tests in `mcp::tests::soll_and_guidelines`
/// — each test must therefore allocate its own code and pass it to
/// `soll_manager` / `Registry` inserts, then build asserted IDs via
/// `format!("CPT-{code}-NNN", code = code)` rather than `"CPT-AXO-NNN"`.
///
/// Encoding : `T` prefix + 2 base-36 digits derived from a monotonic
/// counter. 1296 codes per process, recycled-on-overflow (acceptable :
/// PG state is wiped between full test runs and any single `cargo test`
/// invocation produces far fewer than 1296 distinct test-side projects).
/// The prefix `T` (= "test") avoids collision with the canonical
/// production codes (`AXO`, `BKS`, `PRJ`, …) seeded by `create_test_db`.
pub fn unique_test_project_code() -> String {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let slot = count % (36 * 36);
    let first = slot / 36;
    let second = slot % 36;
    fn base36(d: usize) -> char {
        let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        alphabet[d % 36] as char
    }
    format!("T{}{}", base36(first), base36(second))
}

pub fn create_test_db() -> Result<GraphStore> {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = format!("/tmp/axon_test_db_{}_{}_{}", pid, now, count);
    // REQ-AXO-901882 — harness guard: never resolve to the production
    // axon_live/axon_dev SOLL. `GraphStore::new` would fall through to
    // `resolve_database_url(None)` (defaults `AXON_INSTANCE=live`); route via
    // an explicit URL to a process-shared disposable clone of
    // `axon_test_template` instead. Per-test isolation of these sites is the
    // follow-up REQ-AXO-901877.
    let url = crate::test_support::test_db::shared_test_db_url();
    let store = GraphStore::new_with_database(&db_path, &url)?;
    let _ = store.sync_project_registry_entry(
        "BKS",
        Some("BookingSystem"),
        Some("/home/dstadel/projects/BookingSystem"),
    );
    let _ =
        store.sync_project_registry_entry("AXO", Some("Axon"), Some("/home/dstadel/projects/axon"));
    let _ = store.sync_project_registry_entry("PRJ", Some("ProjectFixture"), Some("/tmp/prj"));
    let _ = store.sync_project_registry_entry("PJA", Some("ProjectFixtureA"), Some("/tmp/pja"));
    let _ = store.sync_project_registry_entry("PJB", Some("ProjectFixtureB"), Some("/tmp/pjb"));
    let _ = store.sync_project_registry_entry("OTH", Some("OtherFixture"), Some("/tmp/other"));
    Ok(store)
}
