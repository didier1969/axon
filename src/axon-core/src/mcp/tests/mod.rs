// Copyright (c) Didier Stadelmann. All rights reserved.

use super::guidance;
use super::*;
use crate::embedder::{embedding_lane_config_from_env, embedding_provider_diagnostics};
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION,
};
use crate::graph::{ExecFunc, GraphStore, InitDbFunc};
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
use libloading::Symbol as LibSymbol;
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

#[test]
fn embedding_provider_diagnostics_tracks_tensorrt_service_toggle() {
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
    }
    assert!(
        !embedding_provider_diagnostics("cuda_service".to_string()).gpu_service_tensorrt_requested
    );

    unsafe {
        std::env::set_var("AXON_GPU_EMBED_SERVICE_TENSORRT", "1");
    }
    assert!(
        embedding_provider_diagnostics("tensorrt_service".to_string())
            .gpu_service_tensorrt_requested
    );

    unsafe {
        std::env::remove_var("AXON_GPU_EMBED_SERVICE_TENSORRT");
    }
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

fn create_test_server() -> McpServer {
    let temp = tempdir().unwrap();
    let db_root = temp.path().to_str().unwrap().to_string();
    test_db_roots()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(temp);
    let store = Arc::new(GraphStore::new(&db_root).unwrap());
    McpServer::new(store)
}

fn create_test_server_with_distinct_reader(db_root: &Path) -> McpServer {
    let store = Arc::new(GraphStore::new(db_root.to_str().unwrap()).unwrap());
    let server = McpServer::new(store);
    attach_distinct_reader_snapshot(&server.graph_store);
    server
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

fn attach_distinct_reader_snapshot(store: &GraphStore) {
    let db_path = store
        .db_path
        .as_ref()
        .expect("disk-backed test store required for distinct reader");
    let reader_c_path = CString::new(db_path.to_string_lossy().to_string()).unwrap();
    let soll_path = {
        let mut path = PathBuf::from(db_path);
        path.set_file_name("soll.db");
        path
    };
    let attach_q = format!(
        "INSTALL json; LOAD json; SET checkpoint_threshold = '1GB'; ATTACH '{}' AS soll;",
        soll_path.to_string_lossy().replace("'", "''")
    );

    unsafe {
        let init_fn: LibSymbol<InitDbFunc> = store.pool.lib.get(b"duckdb_init_db\0").unwrap();
        let exec_fn: LibSymbol<ExecFunc> = store.pool.lib.get(b"duckdb_execute\0").unwrap();
        let reader_ptr = init_fn(reader_c_path.as_ptr(), true);
        assert!(
            !reader_ptr.is_null(),
            "failed to initialize distinct reader"
        );
        assert!(exec_fn(
            reader_ptr,
            CString::new(attach_q).unwrap().as_ptr()
        ));

        let mut reader_guard = store
            .pool
            .reader_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *reader_guard = reader_ptr;
    }
    store.refresh_reader_snapshot().unwrap();
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
