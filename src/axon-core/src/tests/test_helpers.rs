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

pub fn create_test_db() -> Result<GraphStore> {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = format!("/tmp/axon_test_db_{}_{}_{}", pid, now, count);
    let store = GraphStore::new(&db_path)?;
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
