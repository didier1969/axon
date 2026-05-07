//! MIL-AXO-015 P3 integration test against a real PostgreSQL container.
//!
//! Marked `#[ignore]` so the default `cargo test --lib` stays
//! container-free (and CI without Docker keeps building). Run with:
//!
//!     cargo test --manifest-path src/axon-plugin-postgres/Cargo.toml \
//!                --test integration_pg -- --ignored --nocapture
//!
//! Image: `axon-test/age-pgvector:pg17` (combined AGE+pgvector,
//! built locally from `tests/fixtures/Dockerfile.age-pgvector`). AGE
//! and pgvector are both preinstalled, so the full schema generator
//! NOT and will be exercised separately under MIL-AXO-015 P4 once we
//! settle on a combined AGE+pgvector image (custom Dockerfile or a
//! published derivative).

use std::ffi::{CStr, CString};
use std::thread::sleep;
use std::time::Duration;

use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

use axon_plugin_postgres::{
    pg_close_db, pg_execute, pg_free_string, pg_init_db, pg_query_count, pg_query_json,
};

/// Spin up an `axon-test/age-pgvector:pg17` container and return
/// both the `Container` (kept alive by the test) and a usable
/// `DATABASE_URL`. Build the image once with:
///
///     docker build -t axon-test/age-pgvector:pg17 \
///         -f tests/fixtures/Dockerfile.age-pgvector tests/fixtures
fn start_pg() -> (impl Drop, String) {
    let container = GenericImage::new("axon-test/age-pgvector", "pg17")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "axon_test_pw")
        .with_env_var("POSTGRES_DB", "axon_test_db")
        .with_env_var("POSTGRES_USER", "postgres")
        .start()
        .expect("start postgres+age container");
    let port = container
        .get_host_port_ipv4(5432)
        .expect("ephemeral host port");
    let url = format!(
        "postgres://postgres:axon_test_pw@127.0.0.1:{port}/axon_test_db"
    );
    (container, url)
}

/// Repeat `pg_init_db` for up to ~10 seconds. The container is ready
/// once the wait probe matches the stderr message, but the postgres
/// daemon issues that message slightly before it accepts TCP, so the
/// first connect occasionally races. The retries keep tests stable
/// without inflating CI time on the happy path.
unsafe fn init_db_with_retry(
    url: &str,
    schema: Option<&str>,
) -> *mut axon_plugin_postgres::PgPluginContext {
    let url_c = CString::new(url).expect("url is null-free");
    let schema_c = schema.map(|s| CString::new(s).expect("schema is null-free"));
    let schema_ptr = schema_c
        .as_ref()
        .map(|c| c.as_ptr())
        .unwrap_or(std::ptr::null());
    for attempt in 0..20 {
        let ctx = pg_init_db(url_c.as_ptr(), schema_ptr);
        if !ctx.is_null() {
            return ctx;
        }
        sleep(Duration::from_millis(500));
        if attempt == 19 {
            panic!("pg_init_db kept returning null after 10s");
        }
    }
    unreachable!()
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn end_to_end_basic_sql_round_trip() {
    let (_container, url) = start_pg();

    unsafe {
        let ctx = init_db_with_retry(&url, None);

        let create = CString::new(
            "CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT, score DOUBLE PRECISION)",
        )
        .unwrap();
        assert!(pg_execute(ctx, create.as_ptr()), "CREATE TABLE failed");

        let insert = CString::new(
            "INSERT INTO t VALUES (1, 'alpha', 0.5), (2, 'beta', NULL)",
        )
        .unwrap();
        assert!(pg_execute(ctx, insert.as_ptr()), "INSERT failed");

        let count_sql = CString::new("SELECT count(*)::BIGINT FROM t").unwrap();
        assert_eq!(pg_query_count(ctx, count_sql.as_ptr()), 2);

        let select_sql = CString::new("SELECT id, name, score FROM t ORDER BY id").unwrap();
        let raw = pg_query_json(ctx, select_sql.as_ptr());
        let json = CStr::from_ptr(raw).to_str().unwrap().to_string();
        pg_free_string(raw);
        assert!(json.starts_with("[["), "expected JSON array of arrays: {json}");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rows = parsed.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].as_str().unwrap(), "1");
        assert_eq!(rows[0][1].as_str().unwrap(), "alpha");
        assert_eq!(rows[0][2].as_str().unwrap(), "0.5");
        assert_eq!(rows[1][2].as_str().unwrap(), "null");

        // REQ-AXO-129 — invalid SQL must surface an error envelope, not
        // a silent empty array.
        let bad = CString::new("SELECT * FROM nonexistent_table_xyz").unwrap();
        let raw = pg_query_json(ctx, bad.as_ptr());
        let env_str = CStr::from_ptr(raw).to_str().unwrap().to_string();
        pg_free_string(raw);
        assert!(env_str.starts_with('{'), "envelope expected: {env_str}");
        let env: serde_json::Value = serde_json::from_str(&env_str).unwrap();
        assert_eq!(env["stage"], "query");
        assert!(env["_axon_plugin_error"]
            .as_str()
            .unwrap()
            .contains("nonexistent_table_xyz")
            || env["sql_excerpt"]
                .as_str()
                .unwrap()
                .contains("nonexistent_table_xyz"));

        pg_close_db(ctx);
    }
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn search_path_routes_to_per_project_schema() {
    let (_container, url) = start_pg();

    // Bootstrap a per-project schema namespace (CPT-AXO-039) without
    // going through axon-core::postgres::ddl, since this crate has no
    // path dep on axon-core. The shape mirrors `generate_project_schema`.
    unsafe {
        let bootstrap_ctx = init_db_with_retry(&url, None);
        let bootstrap = CString::new(
            "CREATE SCHEMA IF NOT EXISTS axo;\
             CREATE TABLE IF NOT EXISTS axo.File (path TEXT PRIMARY KEY, project_code TEXT NOT NULL);\
             INSERT INTO axo.File VALUES ('/x/y.rs', 'AXO');",
        )
        .unwrap();
        assert!(pg_execute(bootstrap_ctx, bootstrap.as_ptr()));
        pg_close_db(bootstrap_ctx);

        // Re-open with `schema=axo`. Unqualified `File` should resolve
        // to `axo.File` thanks to the auto-emitted `SET search_path`.
        let ctx = init_db_with_retry(&url, Some("axo"));
        let count_sql = CString::new("SELECT count(*)::BIGINT FROM File").unwrap();
        assert_eq!(pg_query_count(ctx, count_sql.as_ptr()), 1);
        pg_close_db(ctx);
    }
}

/// Apache AGE round-trip: enable the extension, create a graph, run a
/// Cypher CREATE then a MATCH, and verify both the row count and the
/// returned property surface as a JSON string. Validates that the
/// plugin's auto `LOAD 'age'` + search_path setup makes `cypher()`
/// resolve unqualified across reused connections.
///
/// agtype values are not natively decodable via tokio-postgres's text
/// protocol path, so the assertion uses a `::text` cast on the cypher
/// projection. P3 slice 3 will introduce native agtype handling so
/// callers don't need the cast.
#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn age_cypher_round_trip_via_query_json() {
    let (_container, url) = start_pg();

    unsafe {
        // Bootstrap: enable AGE in the database BEFORE the plugin
        // probes for it. axon-core's graph_bootstrap will own this in
        // production via `generate_global_schema`.
        let bootstrap_ctx = init_db_with_retry(&url, None);
        let setup = CString::new(
            "CREATE EXTENSION IF NOT EXISTS age;\
             LOAD 'age';\
             SELECT * FROM ag_catalog.create_graph('test_graph');",
        )
        .unwrap();
        assert!(pg_execute(bootstrap_ctx, setup.as_ptr()), "AGE bootstrap");
        pg_close_db(bootstrap_ctx);

        // Re-open: the init-time probe now sees the AGE extension
        // installed and flips age_enabled, so every subsequent
        // connection acquire runs LOAD 'age' + SET search_path.
        let ctx = init_db_with_retry(&url, None);

        // CREATE a node via Cypher and return a SCALAR property
        // (agtype string). `::text` is only valid on scalar agtype
        // values; vertex/edge/map/list need agtype_to_jsonb() instead
        // (handled in P3 slice 3 via native agtype decoding).
        let create = CString::new(
            "SELECT name::text \
             FROM cypher('test_graph', \
                  $$ CREATE (a:Person {name: 'Alice'}) RETURN a.name $$) \
             AS (name agtype)",
        )
        .unwrap();
        let raw = pg_query_json(ctx, create.as_ptr());
        let json = CStr::from_ptr(raw).to_str().unwrap().to_string();
        pg_free_string(raw);
        assert!(json.starts_with("[["), "expected row array, got: {json}");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        let name_text = parsed[0][0].as_str().unwrap();
        assert!(name_text.contains("Alice"), "name text was: {name_text}");

        // MATCH the node back. Validates that the auto LOAD 'age' kicks
        // in for connections re-acquired from the pool.
        let match_sql = CString::new(
            "SELECT name::text \
             FROM cypher('test_graph', $$ MATCH (n:Person) RETURN n.name $$) AS (name agtype)",
        )
        .unwrap();
        let raw = pg_query_json(ctx, match_sql.as_ptr());
        let json = CStr::from_ptr(raw).to_str().unwrap().to_string();
        pg_free_string(raw);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rows = parsed.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        let v = rows[0][0].as_str().unwrap();
        assert!(v.contains("Alice"), "expected Alice, got: {v}");

        pg_close_db(ctx);
    }
}
