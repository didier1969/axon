// Copyright (c) Didier Stadelmann. All rights reserved.

use super::guidance;
use super::*;
use crate::embedder::{embedding_lane_config_from_env, embedding_provider_diagnostics};
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION,
};
use crate::graph::GraphStore;
use crate::ingress_buffer::{
    record_ingress_flush, reset_ingress_metrics_for_tests, IngressBuffer, IngressCause,
    IngressFileEvent, IngressSource,
};
use crate::parser;
use crate::queue::ProcessingMode;
use crate::runtime_boot::RuntimeBootProfile;
use crate::runtime_topology::AxonProcessRole;
use crate::service_guard::{self, ServiceKind};
use crate::vector_control::{
    current_utility_first_scheduler_diagnostics, reset_utility_first_scheduler_for_tests,
    reset_vector_batch_controller_for_tests,
};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::tempdir;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn test_db_roots() -> &'static Mutex<Vec<tempfile::TempDir>> {
    static ROOTS: OnceLock<Mutex<Vec<tempfile::TempDir>>> = OnceLock::new();
    ROOTS.get_or_init(|| Mutex::new(Vec::new()))
}

#[test]
fn split_boot_roles_enable_only_owned_services() {
    let brain = RuntimeBootProfile::brain();
    assert!(brain.start_mcp_http);
    assert!(!brain.start_ingestion_workers);
    assert!(brain.promotable);

    let indexer = RuntimeBootProfile::indexer();
    assert!(!indexer.start_mcp_http);
    assert!(indexer.start_ingestion_workers);
    assert!(indexer.promotable);
}


struct RuntimeEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl RuntimeEnvGuard {
    fn full_autonomous() -> Self {
        let lock = env_lock();
        unsafe {
            std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
            std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        }
        Self { _lock: lock }
    }
}

impl Drop for RuntimeEnvGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("AXON_RUNTIME_MODE");
            std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        }
    }
}

struct SollSiteRootGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl SollSiteRootGuard {
    fn new(path: &Path) -> Self {
        let lock = env_lock();
        unsafe {
            std::env::set_var("AXON_SOLL_SITE_ROOT", path);
        }
        Self { _lock: lock }
    }
}

impl Drop for SollSiteRootGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("AXON_SOLL_SITE_ROOT");
        }
    }
}

/// REQ-AXO-91562 Slice 2 — per-test database isolation via PG template.
///
/// Each test gets a fresh database cloned from `axon_test_template`.
///
/// Lifecycle / reclamation (REQ-AXO-901848): the `TestDb` guard's `Drop`
/// (below) issues a best-effort `dropdb`, but every guard created through
/// `create_test_server` is parked for the test's duration in the
/// process-lifetime `static TEST_DBS` Vec — and Rust never runs `Drop` on the
/// contents of a `static` at process exit. Combined with `panic = "abort"` and
/// the possibility of a SIGKILL, the `Drop` path is effectively dead for parked
/// guards, so each run permanently leaked one database per test (727 leaked /
/// 96 GB observed before this fix). The canonical reclamation is therefore the
/// idempotent, connection-safe pre-run sweep [`sweep_stale_test_databases`],
/// invoked once per process the first time a `TestDb` is created. It reclaims
/// databases leaked by *previous* runs and is independent of how this process
/// terminates. The `Drop` is retained only as an opportunistic fast path for
/// the rare guard that is dropped normally.
struct TestDb {
    db_name: String,
    pg_port: String,
}

/// REQ-AXO-901848 — reclaim `axon_test_*` databases leaked by previous test
/// runs. Runs exactly once per test process (guarded by [`sweep_once`]) before
/// the first database is created.
///
/// Concurrency safety: only databases with **zero** active backends in
/// `pg_stat_activity` are dropped, so a database currently in use by a
/// parallel test binary is never touched. Fresh databases created by *this*
/// run carry unique nanosecond+thread-id names that cannot collide with the
/// leaked names being swept, so there is no create/sweep race. The template
/// (`axon_test_template`) and any non-test database are excluded by the
/// `LIKE 'axon\_test\_%'` filter plus an explicit guard.
fn sweep_stale_test_databases(pg_port: &str) {
    // `DROP DATABASE` cannot run inside a transaction block, so a DO/loop is
    // not an option; `\gexec` executes each generated statement as its own
    // top-level command. ON_ERROR_STOP=0 keeps one failed drop (e.g. a
    // database that acquired a connection between SELECT and DROP) from
    // aborting the rest.
    let script = "\\set ON_ERROR_STOP 0\n\
        SELECT format('DROP DATABASE IF EXISTS %I', d.datname)\n\
        FROM pg_database d\n\
        WHERE d.datname LIKE 'axon\\_test\\_%'\n\
          AND d.datname <> 'axon_test_template'\n\
          AND NOT EXISTS (\n\
            SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname\n\
          )\n\
        \\gexec\n";

    let mut child = match std::process::Command::new("psql")
        .args([
            "-h", "127.0.0.1",
            "-p", pg_port,
            "-U", "axon",
            "-d", "postgres",
            "-X", // ignore ~/.psqlrc for deterministic behaviour
            "-q",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        // psql unavailable (e.g. unit-only environment without PG): the sweep
        // is best-effort, so a missing binary must not fail the test run.
        Err(_) => return,
    };

    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        let _ = stdin.write_all(script.as_bytes());
    }
    let _ = child.wait();
}

/// Run [`sweep_stale_test_databases`] at most once per test process.
fn sweep_once(pg_port: &str) {
    static SWEEP: OnceLock<()> = OnceLock::new();
    SWEEP.get_or_init(|| {
        sweep_stale_test_databases(pg_port);
    });
}

/// REQ-AXO-91560 — guarantee `axon_test_template` carries the canonical
/// schema **and** the global SOLL seed before any test clones it.
///
/// The ephemeral-DB isolation (`createdb -T template`) hands each test a
/// pristine database, but a bare/empty template strips the ambient global
/// seed (the `PRO` sentinel rows + `GUI-PRO-*` guidelines) that the shared
/// devenv PG used to provide for free. Tests asserting bootstrap/init
/// guideline injection, or attaching nodes to seeded global pillars, then
/// fail. Applying the idempotent `db/ddl/*.sql` + `db/seed/*.sql` to the
/// template once per process bakes the seed INTO it, so every clone
/// inherits the canonical baseline for free. Reproducible on a fresh
/// machine — no manual template setup required (the previous template was
/// hand-created and partial, which violated build reproducibility).
///
/// Runs at most once per process via `OnceLock`; `get_or_init` blocks
/// concurrent callers until the template is fully built, so no clone ever
/// sees a half-seeded template. Every psql command is synchronous and its
/// connection is closed before the first `createdb -T`, so the
/// "template in use" hazard cannot arise.
fn ensure_template_once(pg_port: &str) {
    static TEMPLATE: OnceLock<()> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let template = std::env::var("AXON_TEST_TEMPLATE")
            .unwrap_or_else(|_| "axon_test_template".to_string());

        // Create the template database if absent. A pre-existing (possibly
        // empty) template is fine — the idempotent DDL+seed below brings it
        // to canonical state. A failure here (already exists) is ignored.
        let _ = std::process::Command::new("createdb")
            .args(["-h", "127.0.0.1", "-p", pg_port, "-U", "axon", &template])
            .output();

        // Apply canonical DDL then seed in lexical order, mirroring
        // scripts/lib/ensure-runtime.sh {apply_canonical_ddl,
        // apply_canonical_seed}. `generate_global_schema()` compiles the
        // same db/ddl files (DEC-AXO-082), so there is no schema divergence.
        let db_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("db");
        apply_sql_dir(pg_port, &template, &db_dir.join("ddl"));
        apply_sql_dir(pg_port, &template, &db_dir.join("seed"));
    });
}

/// Apply every `NN_*.sql` file in `dir` (lexical order) to `dbname` via
/// psql. Best-effort: a missing directory or psql binary is a silent no-op
/// (unit-only environments without PG), matching the sweep's tolerance.
fn apply_sql_dir(pg_port: &str, dbname: &str, dir: &Path) {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "sql")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.bytes().next())
                        .is_some_and(|b| b.is_ascii_digit())
            })
            .collect(),
        Err(_) => return,
    };
    files.sort();
    for f in files {
        let Some(path) = f.to_str() else { continue };
        let _ = std::process::Command::new("psql")
            .args([
                "-h", "127.0.0.1", "-p", pg_port, "-U", "axon", "-d", dbname,
                "-X", "-q", "-v", "ON_ERROR_STOP=1", "-f", path,
            ])
            .output();
    }
}

impl TestDb {
    fn create() -> Self {
        // REQ-AXO-901848 — reclaim databases leaked by previous runs before
        // creating this run's database. Idempotent and connection-safe.
        let pg_port_for_sweep =
            std::env::var("PGPORT").unwrap_or_else(|_| "44144".to_string());
        sweep_once(&pg_port_for_sweep);
        // REQ-AXO-91560 — bring the clone template to canonical schema+seed
        // before the first `createdb -T` below.
        ensure_template_once(&pg_port_for_sweep);

        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tid = std::thread::current().id();
        let db_name = format!("axon_test_{:x}_{:?}", id, tid)
            .replace("ThreadId(", "t")
            .replace(')', "");
        let pg_port = std::env::var("PGPORT").unwrap_or_else(|_| "44144".to_string());
        let template = std::env::var("AXON_TEST_TEMPLATE")
            .unwrap_or_else(|_| "axon_test_template".to_string());

        let output = std::process::Command::new("createdb")
            .args([
                "-h", "127.0.0.1",
                "-p", &pg_port,
                "-U", "axon",
                "-T", &template,
                &db_name,
            ])
            .output()
            .expect("createdb command failed to execute");

        if !output.status.success() {
            panic!(
                "TestDb create failed for {}: {}",
                db_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        TestDb { db_name, pg_port }
    }

    fn url(&self) -> String {
        format!(
            "postgres://axon@127.0.0.1:{}/{}",
            self.pg_port, self.db_name
        )
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let _ = std::process::Command::new("dropdb")
            .args([
                "-h", "127.0.0.1",
                "-p", &self.pg_port,
                "-U", "axon",
                "--if-exists",
                &self.db_name,
            ])
            .output();
    }
}

#[allow(dead_code)]
pub(crate) fn delete_fixture_symbols(server: &McpServer, ids: &[&str]) {
    if ids.is_empty() {
        return;
    }
    let quoted: Vec<String> = ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    let list = quoted.join(", ");
    let _ = server.graph_store.execute(&format!(
        "DELETE FROM ist.ChunkEmbedding WHERE chunk_id IN \
         (SELECT id FROM ist.Chunk WHERE source_id IN ({list}))"
    ));
    let _ = server.graph_store.execute(&format!(
        "DELETE FROM ist.Chunk WHERE source_id IN ({list})"
    ));
    let _ = server.graph_store.execute(&format!(
        "DELETE FROM ist.Edge WHERE source_id IN ({list}) OR target_id IN ({list})"
    ));
    let _ = server.graph_store.execute(&format!(
        "DELETE FROM ist.Symbol WHERE id IN ({list})"
    ));
}

fn create_test_server() -> McpServer {
    let temp = tempdir().unwrap();
    let db_root = temp.path().to_str().unwrap().to_string();
    test_db_roots()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(temp);

    let test_db = TestDb::create();
    let db_url = test_db.url();
    TEST_DBS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(test_db);

    let store = Arc::new(GraphStore::new_with_database(&db_root, &db_url).unwrap());
    McpServer::new(store)
}

static TEST_DBS: Mutex<Vec<TestDb>> = Mutex::new(Vec::new());

/// REQ-AXO-901848 — helper: does database `name` exist on the test cluster?
fn test_database_exists(pg_port: &str, name: &str) -> bool {
    let out = std::process::Command::new("psql")
        .args([
            "-h", "127.0.0.1",
            "-p", pg_port,
            "-U", "axon",
            "-d", "postgres",
            "-X", "-tAc",
            &format!("SELECT 1 FROM pg_database WHERE datname = '{name}'"),
        ])
        .output()
        .expect("psql existence check failed to execute");
    String::from_utf8_lossy(&out.stdout).trim() == "1"
}

/// REQ-AXO-901848 — regression guard for the leaked-test-database fix.
///
/// Creates a database that mimics a leak from a previous run (no active
/// connection), runs the reclamation sweep, and asserts the leak is dropped
/// while the shared `axon_test_template` is preserved. This locks in the
/// canonical reclamation mechanism that replaces the dead `Drop` path
/// (see [`TestDb`]).
#[test]
fn sweep_reclaims_leaked_test_databases_but_preserves_template() {
    let pg_port = std::env::var("PGPORT").unwrap_or_else(|_| "44144".to_string());

    // Simulate a database leaked by a previous run: a real clone of the
    // template, with no connection held against it.
    let leaked = format!(
        "axon_test_sweepguard_{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let created = std::process::Command::new("createdb")
        .args([
            "-h", "127.0.0.1",
            "-p", &pg_port,
            "-U", "axon",
            "-T", "axon_test_template",
            &leaked,
        ])
        .output()
        .expect("createdb failed to execute");
    assert!(
        created.status.success(),
        "setup createdb failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(
        test_database_exists(&pg_port, &leaked),
        "leaked test database should exist before the sweep"
    );

    // Reclamation: call the sweep directly (not `sweep_once`, whose
    // process-wide guard may already be consumed by other tests).
    sweep_stale_test_databases(&pg_port);

    assert!(
        !test_database_exists(&pg_port, &leaked),
        "sweep must reclaim a connection-free leaked axon_test_* database"
    );
    assert!(
        test_database_exists(&pg_port, "axon_test_template"),
        "sweep must never drop the shared template database"
    );
}

fn assert_runtime_authority_roles(
    authority: &serde_json::Value,
    expected_process_role: AxonProcessRole,
    expected_public_mcp_authority: AxonProcessRole,
    expected_soll_writer_authority: AxonProcessRole,
    expected_ist_writer_authority: AxonProcessRole,
) {
    assert_eq!(
        authority["process_role"].as_str(),
        Some(expected_process_role.as_str())
    );
    assert_eq!(
        authority["public_mcp_authority"].as_str(),
        Some(expected_public_mcp_authority.as_str())
    );
    assert_eq!(
        authority["soll_writer_authority"].as_str(),
        Some(expected_soll_writer_authority.as_str())
    );
    assert_eq!(
        authority["ist_writer_authority"].as_str(),
        Some(expected_ist_writer_authority.as_str())
    );
    assert!(
        authority.get("topology").is_none(),
        "public runtime authority must not expose a topology selector"
    );
}

fn now_ms_for_tests() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn wait_for_job_status(server: &McpServer, job_id: &str) -> Value {
    for _ in 0..50 {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "job_status",
                "arguments": { "job_id": job_id }
            })),
            id: Some(json!(9001)),
        };
        let response = server.handle_request(req).unwrap();
        let result = response.result.unwrap();
        let status = result
            .get("data")
            .and_then(|data| data.get("status"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        if matches!(status, "succeeded" | "failed") {
            return result;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("job {} did not finish in time", job_id);
}

fn assert_async_job_contract(data: &Value, expected_follow_up_tool: &str) {
    assert_eq!(
        data.get("accepted").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert!(data
        .get("job_id")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty()));
    assert!(data
        .get("tool_name")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty()));
    assert!(data
        .get("status")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty()));
    assert!(data.get("reserved_ids").is_some());
    assert!(data.get("known_ids").is_some());
    assert_eq!(
        data.get("next_action")
            .and_then(|value| value.get("tool"))
            .and_then(|value| value.as_str()),
        Some(expected_follow_up_tool)
    );
    assert!(data
        .get("next_action")
        .and_then(|value| value.get("arguments"))
        .and_then(|value| value.get("job_id"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty()));
    assert!(data.get("result_contract").is_some());
    assert!(data.get("polling_guidance").is_some());
    assert_eq!(
        data.get("polling_guidance")
            .and_then(|value| value.get("poll_interval_seconds"))
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    let until_states = data
        .get("polling_guidance")
        .and_then(|value| value.get("until_states"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(until_states
        .iter()
        .any(|value| value.as_str() == Some("completed")));
    assert!(until_states
        .iter()
        .any(|value| value.as_str() == Some("failed")));
    assert!(data
        .get("recovery_hint")
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.is_empty()));
}

fn assert_sync_mutation_contract(data: &Value) {
    assert!(data.get("job_id").is_none());
    assert!(data.get("accepted").is_none());
    assert!(data.get("result_contract").is_none());
    assert!(data.get("polling_guidance").is_none());
    assert!(data.get("recovery_hint").is_none());
}

fn current_graph_model_id() -> String {
    crate::embedding_contract::GRAPH_MODEL_ID.to_string()
}

fn graph_embedding_sql(seed: &[f32]) -> String {
    let dimension = DIMENSION;
    assert!(seed.len() <= dimension);
    let mut values = vec![0.0_f32; dimension];
    for (idx, value) in seed.iter().enumerate() {
        values[idx] = *value;
    }
    let literal = values
        .iter()
        .map(|value| {
            let mut rendered = format!("{value}");
            if !rendered.contains('.') {
                rendered.push_str(".0");
            }
            rendered
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("CAST([{literal}] AS FLOAT[{dimension}])")
}

mod context_and_analysis;
mod guidance_contract;
mod runtime_surface;
mod soll_and_guidelines;
