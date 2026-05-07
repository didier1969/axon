//! MIL-AXO-015 P3 slice 3b validation: load this crate's cdylib via
//! libloading and exercise the FFI symbols exactly the way axon-core's
//! `LatticePool` will once `AXON_DB_BACKEND=postgres` ships. This is
//! the only test that proves the C ABI contract end-to-end (the
//! integration tests in `integration_pg.rs` use the rlib variant and
//! therefore bypass the shared object's actual exports).
//!
//! Marked `#[ignore]`: requires Docker plus a built `cdylib` artefact
//! at `target/<profile>/libaxon_plugin_postgres.{so,dylib}`. cargo test
//! builds this automatically because it links the crate's `[lib]`
//! definition. Run with:
//!
//!     cargo test --manifest-path src/axon-plugin-postgres/Cargo.toml \
//!                --test ffi_contract -- --ignored --nocapture

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use libloading::{Library, Symbol};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, ImageExt};

type InitDbCompatFn = unsafe extern "C" fn(*const c_char, bool) -> *mut c_void;
type CloseDbFn = unsafe extern "C" fn(*mut c_void);
type ExecFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;
type QueryCountFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> i64;
type QueryJsonFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
type FreeStrFn = unsafe extern "C" fn(*mut c_char);

/// Locate the freshly-built cdylib next to the test binary. Cargo runs
/// tests with `CARGO_TARGET_TMPDIR` set; the cdylib lives one or two
/// levels up depending on the profile.
fn cdylib_path() -> PathBuf {
    // Same target dir cargo uses for this crate.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let extension = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    let prefix = if cfg!(target_os = "windows") { "" } else { "lib" };
    let filename = format!("{prefix}axon_plugin_postgres.{extension}");
    for profile in &["debug", "release"] {
        let p = manifest_dir
            .join("target")
            .join(profile)
            .join(&filename);
        if p.exists() {
            return p;
        }
    }
    panic!(
        "could not find {filename} under {}/target/{{debug,release}}; build with `cargo build --lib` first",
        manifest_dir.display()
    );
}

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
        .expect("start container");
    let port = container.get_host_port_ipv4(5432).expect("port");
    let url = format!("postgres://postgres:axon_test_pw@127.0.0.1:{port}/axon_test_db");
    (container, url)
}

#[test]
#[ignore = "requires docker; opt-in via `cargo test -- --ignored`"]
fn ffi_contract_matches_duckdb_init_signature() {
    let path = cdylib_path();
    let (_container, url) = start_pg();

    unsafe {
        let lib = Library::new(&path).expect("load cdylib");
        let init: Symbol<InitDbCompatFn> =
            lib.get(b"pg_init_db_compat\0").expect("pg_init_db_compat");
        let close: Symbol<CloseDbFn> = lib.get(b"pg_close_db\0").expect("pg_close_db");
        let exec: Symbol<ExecFn> = lib.get(b"pg_execute\0").expect("pg_execute");
        let count: Symbol<QueryCountFn> =
            lib.get(b"pg_query_count\0").expect("pg_query_count");
        let qjson: Symbol<QueryJsonFn> =
            lib.get(b"pg_query_json\0").expect("pg_query_json");
        let free_str: Symbol<FreeStrFn> = lib.get(b"pg_free_string\0").expect("pg_free_string");

        // Mimic axon-core: same signature it uses for duckdb_init_db.
        // The first arg is a DATABASE_URL when backend=Postgres; the
        // bool is ignored.
        let url_c = CString::new(url).unwrap();
        let mut ctx: *mut c_void = std::ptr::null_mut();
        for attempt in 0..20 {
            ctx = init(url_c.as_ptr(), false);
            if !ctx.is_null() {
                break;
            }
            sleep(Duration::from_millis(500));
            if attempt == 19 {
                panic!("pg_init_db_compat returned null after 10s of retries");
            }
        }

        let create =
            CString::new("CREATE TABLE t(id BIGINT PRIMARY KEY, name TEXT)").unwrap();
        assert!(exec(ctx, create.as_ptr()), "CREATE TABLE failed");

        let insert =
            CString::new("INSERT INTO t VALUES (1, 'alpha'), (2, 'beta')").unwrap();
        assert!(exec(ctx, insert.as_ptr()), "INSERT failed");

        let count_sql = CString::new("SELECT count(*)::BIGINT FROM t").unwrap();
        assert_eq!(count(ctx, count_sql.as_ptr()), 2);

        let select = CString::new("SELECT id, name FROM t ORDER BY id").unwrap();
        let raw = qjson(ctx, select.as_ptr());
        let json = CStr::from_ptr(raw).to_str().unwrap().to_string();
        free_str(raw);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rows = parsed.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].as_str().unwrap(), "1");
        assert_eq!(rows[0][1].as_str().unwrap(), "alpha");

        close(ctx);
        // Library drops here, unloading the cdylib cleanly.
    }
}
