// Copyright (c) Didier Stadelmann. All rights reserved.

use super::guidance;
use super::*;
use crate::embedder::{embedding_lane_config_from_env, embedding_provider_diagnostics};
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION,
};
use crate::graph::GraphStore;
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

use crate::test_support::test_db::{sweep_stale_test_databases, TestDb};

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

// TestDb / sweep / template infra relocated to `crate::test_support::test_db`
// (REQ-AXO-91560 — single shared home for raw-SQL and IST-fixture isolation).

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
    // pgvector literal (ist.GraphEmbedding.embedding is `vector(1024)`),
    // not the DuckDB-era `CAST([...] AS FLOAT[N])` form.
    format!("'[{literal}]'::vector")
}

mod context_and_analysis;
mod guidance_contract;
mod runtime_surface;
mod soll_and_guidelines;
