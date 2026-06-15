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
/// observe shared PG state (e.g. `axon.VectorWorkerFault`,
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

/// Process-lifetime parking lot for per-test databases created via
/// `create_test_db`. The `TestDb` `Drop` never runs from a `static`, so the
/// canonical reclamation is the `libc::atexit` hook armed inside `TestDb::create`
/// (force-drops every db this process created). Parking here keeps the database
/// alive for the test's duration; the returned `GraphStore` (and its native
/// pool) is owned by the caller and dropped per-test, releasing connections.
fn parked_test_dbs() -> &'static Mutex<Vec<crate::test_support::test_db::TestDb>> {
    static PARKED: OnceLock<Mutex<Vec<crate::test_support::test_db::TestDb>>> = OnceLock::new();
    PARKED.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn create_test_db() -> Result<GraphStore> {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    // REQ-AXO-901877 — per-test PostgreSQL isolation. Each call gets a fresh
    // `createdb -T axon_test_template` clone (same canonical DDL + global seed +
    // auto-seed triggers) instead of the process-SHARED database, so non-mcp
    // tests (graph_bootstrap, pipeline_v2 stage_a3/orchestrator, …) no longer
    // pollute one another through shared IST/SOLL state. The store's native pool
    // (REQ-AXO-901959) writes the bulk graph COPY into THIS database, so the
    // pipeline tests read back what they wrote. The TestDb is parked
    // process-lifetime and force-dropped at exit (its Drop never fires from a
    // static; the atexit hook reclaims it). Mirrors `create_test_server`.
    let test_db = crate::test_support::test_db::TestDb::create();
    let url = test_db.url();
    parked_test_dbs()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(test_db);
    let db_path = format!("/tmp/axon_test_db_unused_{}_{}", pid, count);
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
