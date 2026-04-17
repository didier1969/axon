use crate::graph::GraphStore;
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

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
