//! MIL-AXO-015 P3 smoke test: GraphStore::new under AXON_DB_BACKEND=postgres.
//!
//! Marked `#[ignore]`. Requirements:
//!   1. Docker runtime available (testcontainers spawns apache/age).
//!   2. `libaxon_plugin_postgres.so` already built — the test does not
//!      shell out to cargo. Build with:
//!
//!          cargo build --manifest-path src/axon-plugin-postgres/Cargo.toml --lib
//!
//! The test boots a fresh PG container with AGE preinstalled, points
//! axon-core at it via `AXON_LIVE_DATABASE_URL`, and asserts that
//! `GraphStore::new` runs through `bootstrap_global_pg_schema`
//! cleanly. Subsequent queries against `soll.Node` confirm the global
//! schema bootstrap actually wired the SOLL layer.

use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use axon_core::graph::GraphStore;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

/// Serialise PG-backend tests: each one mutates `AXON_DB_BACKEND` +
/// the URL env vars, and they would race with the duckdb-default
/// tests if cargo runs them in parallel.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn graphstore_boots_under_postgres_backend() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let container = GenericImage::new("apache/age", "release_PG17_1.6.0")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_test_pw")
        .with_env_var("POSTGRES_DB", "axon_test_db")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!("postgres://postgres:axon_test_pw@127.0.0.1:{port}/axon_test_db");

    std::env::set_var("AXON_DB_BACKEND", "postgres");
    std::env::set_var("AXON_LIVE_DATABASE_URL", &url);
    // Defensive: the resolver tries AXON_LIVE first, then DEV, then
    // generic DATABASE_URL. Clear DEV so a host-set value does not
    // leak in.
    std::env::remove_var("AXON_DEV_DATABASE_URL");

    // PG sometimes races between "ready" message and accepting TCP.
    let mut last_err = None;
    let mut store = None;
    for _ in 0..10 {
        match GraphStore::new("/tmp/axon-pg-smoke-unused") {
            Ok(s) => {
                store = Some(s);
                break;
            }
            Err(e) => {
                last_err = Some(e);
                sleep(Duration::from_millis(500));
            }
        }
    }
    let store = store.unwrap_or_else(|| {
        panic!(
            "GraphStore::new under PG failed after retries: {:?}",
            last_err
        )
    });

    // bootstrap_global_pg_schema should have created soll.Node.
    let soll_node_count = store
        .query_count("SELECT count(*)::BIGINT FROM soll.Node")
        .expect("query_count on soll.Node should succeed under PG backend");
    assert_eq!(
        soll_node_count, 0,
        "fresh PG database should have zero SOLL nodes"
    );

    // ProjectCodeRegistry should also exist (extensions + global tables).
    let registry_count = store
        .query_count("SELECT count(*)::BIGINT FROM public.ProjectCodeRegistry")
        .expect("query_count on ProjectCodeRegistry should succeed under PG");
    assert_eq!(registry_count, 0);

    // AGE extension should be loaded — confirm via pg_extension.
    let age_count = store
        .query_count("SELECT count(*)::BIGINT FROM pg_extension WHERE extname = 'age'")
        .expect("query_count on pg_extension should succeed under PG");
    assert_eq!(age_count, 1, "AGE extension should be installed");

    drop(store);

    // Reset env so subsequent test runs in the same `cargo test`
    // invocation do not inherit the postgres backend selection.
    std::env::remove_var("AXON_DB_BACKEND");
    std::env::remove_var("AXON_LIVE_DATABASE_URL");
}
