// Copyright (c) Didier Stadelmann. All rights reserved.

use super::guidance;
use super::*;
use crate::embedder::embedding_lane_config_from_env;
use crate::embedding_contract::{
    CHUNK_MODEL_ID, DIMENSION, MAX_LENGTH, MODEL_NAME, NATIVE_DIMENSION,
};
use crate::graph::GraphStore;
use crate::parser;
use crate::queue::ProcessingMode;
use crate::service_guard::{self, ServiceKind};
use crate::vector_control::reset_vector_batch_controller_for_tests;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tempfile::tempdir;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

struct RuntimeEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl RuntimeEnvGuard {
    fn full_autonomous() -> Self {
        let lock = env_lock();
        unsafe {
            std::env::set_var("AXON_RUNTIME_MODE", "full");
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

fn create_test_server() -> McpServer {
    let store = Arc::new(
        GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap()),
    );
    McpServer::new(store)
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

#[test]
fn guided_response_omits_guidance_block_when_problem_class_is_none() {
    let response = guidance::build_guided_response(
        json!({ "status": "ok", "summary": "exact symbol resolved" }),
        guidance::GuidanceOutcome::none(),
    );

    assert_eq!(response["status"], "ok");
    assert_eq!(response["summary"], "exact symbol resolved");
    assert!(response.get("problem_class").is_none());
    assert!(response.get("next_best_actions").is_none());
    assert!(response.get("soll").is_none());
}

#[test]
fn guided_response_includes_compact_guidance_fields_only_when_present() {
    let response = guidance::build_guided_response(
        json!({ "status": "warn_input_not_found", "summary": "symbol not found in current scope" }),
        guidance::GuidanceOutcome {
            problem_class: Some("input_not_found".to_string()),
            likely_cause: Some("exact_symbol_mismatch".to_string()),
            next_best_actions: vec![
                "retry with suggested symbol".to_string(),
                "use query to broaden recall".to_string(),
            ],
            confidence: Some("low".to_string()),
            soll: None,
        },
    );

    assert_eq!(response["problem_class"], "input_not_found");
    assert_eq!(response["likely_cause"], "exact_symbol_mismatch");
    assert_eq!(response["confidence"], "low");
    assert_eq!(
        response["next_best_actions"][0],
        "retry with suggested symbol"
    );
    assert!(response.get("soll").is_none());
}

#[test]
fn guided_response_omits_invalid_soll_block_without_authorization_signal() {
    let response = guidance::build_guided_response(
        json!({ "status": "ok", "summary": "code evidence found" }),
        guidance::GuidanceOutcome {
            problem_class: Some("missing_rationale_in_soll".to_string()),
            likely_cause: None,
            next_best_actions: vec!["review current SOLL context".to_string()],
            confidence: Some("medium".to_string()),
            soll: Some(guidance::SollGuidance {
                recommended_action: "recommend_update".to_string(),
                update_kind: "decision_or_requirement".to_string(),
                reason: "intentional rationale is underspecified".to_string(),
                requires_authorization: None,
            }),
        },
    );

    assert_eq!(response["problem_class"], "missing_rationale_in_soll");
    assert!(response.get("soll").is_none());
}

#[test]
fn guided_response_includes_soll_block_when_authorization_signal_is_present() {
    let response = guidance::build_guided_response(
        json!({ "status": "ok", "summary": "code evidence found" }),
        guidance::GuidanceOutcome {
            problem_class: Some("missing_rationale_in_soll".to_string()),
            likely_cause: None,
            next_best_actions: vec!["review current SOLL context".to_string()],
            confidence: Some("medium".to_string()),
            soll: Some(guidance::SollGuidance {
                recommended_action: "recommend_update".to_string(),
                update_kind: "decision_or_requirement".to_string(),
                reason: "intentional rationale is underspecified".to_string(),
                requires_authorization: Some(true),
            }),
        },
    );

    assert_eq!(response["problem_class"], "missing_rationale_in_soll");
    assert_eq!(response["soll"]["recommended_action"], "recommend_update");
    assert_eq!(response["soll"]["update_kind"], "decision_or_requirement");
    assert_eq!(response["soll"]["requires_authorization"], true);
}

#[test]
fn authoritative_phase1_guidance_filters_deferred_soll_gap_classes() {
    let projected = guidance::project_authoritative_phase1_guidance(guidance::GuidanceOutcome {
        problem_class: Some("missing_rationale_in_soll".to_string()),
        likely_cause: Some("code_evidence_without_maintained_rationale".to_string()),
        next_best_actions: vec!["review_current_soll_context".to_string()],
        confidence: Some("medium".to_string()),
        soll: Some(guidance::SollGuidance {
            recommended_action: "recommend_update".to_string(),
            update_kind: "decision_or_requirement".to_string(),
            reason: "missing_rationale_evidence".to_string(),
            requires_authorization: Some(true),
        }),
    });

    assert_eq!(projected, guidance::GuidanceOutcome::none());
}

#[test]
fn authoritative_phase1_guidance_keeps_supported_public_classes() {
    let projected = guidance::project_authoritative_phase1_guidance(guidance::GuidanceOutcome {
        problem_class: Some("degraded".to_string()),
        likely_cause: Some("graph_index_not_fully_ready".to_string()),
        next_best_actions: vec![
            "treat_result_as_partial".to_string(),
            "retry_after_runtime_stabilizes".to_string(),
        ],
        confidence: Some("medium".to_string()),
        soll: None,
    });

    assert_eq!(projected.problem_class.as_deref(), Some("degraded"));
    assert_eq!(
        projected.likely_cause.as_deref(),
        Some("graph_index_not_fully_ready")
    );
}

#[test]
fn authoritative_guidance_attaches_public_fields_without_shadow_wrapper() {
    let response = guidance::attach_guidance_authoritative(
        json!({
            "status": "ok",
            "content": [{ "type": "text", "text": "query fallback" }]
        }),
        guidance::GuidanceOutcome {
            problem_class: Some("degraded".to_string()),
            likely_cause: Some("graph_index_not_fully_ready".to_string()),
            next_best_actions: vec![
                "treat_result_as_partial".to_string(),
                "retry_after_runtime_stabilizes".to_string(),
            ],
            confidence: Some("medium".to_string()),
            soll: None,
        },
    );

    assert_eq!(response["data"]["problem_class"], "degraded");
    assert_eq!(
        response["data"]["likely_cause"],
        "graph_index_not_fully_ready"
    );
    assert_eq!(
        response["data"]["next_best_actions"][0],
        "treat_result_as_partial"
    );
    assert!(response["data"]["_shadow"].is_null());
}

#[test]
fn guidance_shadow_is_additive_and_preserves_existing_payload() {
    let response = guidance::attach_guidance_shadow(
        json!({
            "status": "ok",
            "data": {
                "summary": "exact symbol resolved",
                "symbol": "Axon.Scanner.scan"
            }
        }),
        json!({
            "problem_class": "input_not_found",
            "next_best_actions": ["retry with suggested symbol"]
        }),
    );

    assert_eq!(response["status"], "ok");
    assert_eq!(response["data"]["summary"], "exact symbol resolved");
    assert_eq!(response["data"]["symbol"], "Axon.Scanner.scan");
    assert_eq!(
        response["data"]["_shadow"]["guidance"]["problem_class"],
        "input_not_found"
    );
    assert_eq!(
        response["data"]["_shadow"]["guidance"]["next_best_actions"][0],
        "retry with suggested symbol"
    );
}

#[test]
fn query_guidance_facts_capture_exact_symbol_miss_with_suggestion() {
    let server = create_test_server();
    let candidates = GuidanceCandidates {
        symbols: vec!["Axon.Scanner.scan".to_string()],
        project_codes: vec!["AXO".to_string()],
        canonical_sources: vec!["soll_export".to_string()],
    };

    let facts = server.extract_query_guidance_facts(
        "trigger_scan",
        Some("AXO"),
        &candidates,
        0,
        false,
        true,
        false,
    );

    assert!(facts.contains(&GuidanceFact::problem_signal("input_not_found")));
    assert!(facts.contains(&GuidanceFact::candidate_symbol("Axon.Scanner.scan")));
    assert!(facts.contains(&GuidanceFact::resolved_project_scope("AXO")));
    assert!(facts.contains(&GuidanceFact::canonical_source("soll_export")));
}

#[test]
fn inspect_guidance_facts_capture_symbol_miss_with_canonical_project() {
    let server = create_test_server();
    let candidates = GuidanceCandidates {
        symbols: vec!["axon_retrieve_context".to_string()],
        project_codes: vec!["AXO".to_string()],
        canonical_sources: Vec::new(),
    };

    let facts = server.extract_inspect_guidance_facts(
        "axon_retrieve_contex",
        Some("AXO"),
        &candidates,
        0,
        true,
        false,
    );

    assert!(facts.contains(&GuidanceFact::problem_signal("input_not_found")));
    assert!(facts.contains(&GuidanceFact::candidate_symbol("axon_retrieve_context")));
    assert!(facts.contains(&GuidanceFact::resolved_project_scope("AXO")));
}

#[test]
fn query_guidance_facts_capture_ambiguity_for_duplicate_symbol_names_across_projects() {
    let server = create_test_server();
    let candidates = GuidanceCandidates {
        symbols: vec!["scan".to_string(), "scan".to_string()],
        project_codes: vec!["PJA".to_string(), "PJB".to_string()],
        canonical_sources: Vec::new(),
    };

    let facts =
        server.extract_query_guidance_facts("scan", None, &candidates, 0, false, false, false);

    assert!(facts.contains(&GuidanceFact::problem_signal("input_ambiguous")));
    assert!(facts.contains(&GuidanceFact::candidate_project_code("PJA")));
    assert!(facts.contains(&GuidanceFact::candidate_project_code("PJB")));
}

#[test]
fn query_guidance_facts_capture_wrong_scope_when_candidates_exist_elsewhere() {
    let server = create_test_server();
    let candidates = GuidanceCandidates {
        symbols: vec!["scan".to_string()],
        project_codes: vec!["PJA".to_string()],
        canonical_sources: Vec::new(),
    };

    let facts = server.extract_query_guidance_facts(
        "scan",
        Some("AXO"),
        &candidates,
        0,
        false,
        false,
        false,
    );

    assert!(facts.contains(&GuidanceFact::problem_signal("wrong_project_scope")));
    assert!(facts.contains(&GuidanceFact::candidate_project_code("PJA")));
}

#[test]
fn query_guidance_facts_capture_degraded_index_signal() {
    let server = create_test_server();
    let facts = server.extract_query_guidance_facts(
        "scan",
        Some("AXO"),
        &GuidanceCandidates::default(),
        3,
        false,
        false,
        false,
    );

    assert!(facts.contains(&GuidanceFact::IndexIncomplete));
    assert!(facts.contains(&GuidanceFact::result_degraded("index_partial")));
}

#[test]
fn query_guidance_facts_capture_vectorization_incomplete_signal() {
    let server = create_test_server();
    let facts = server.extract_query_guidance_facts(
        "scan",
        Some("AXO"),
        &GuidanceCandidates::default(),
        0,
        true,
        false,
        false,
    );

    assert!(facts.contains(&GuidanceFact::VectorizationIncomplete));
}

#[test]
fn classify_guidance_marks_wrong_project_scope() {
    let outcome = classify_guidance(&[
        GuidanceFact::requested_target("scan"),
        GuidanceFact::resolved_project_scope("AXO"),
        GuidanceFact::candidate_project_code("PJA"),
        GuidanceFact::problem_signal("wrong_project_scope"),
    ]);

    assert_eq!(
        outcome.problem_class.as_deref(),
        Some("wrong_project_scope")
    );
    assert!(outcome
        .next_best_actions
        .contains(&"use_canonical_project_code".to_string()));
}

#[test]
fn classify_guidance_marks_input_not_found_with_retry_action() {
    let outcome = classify_guidance(&[
        GuidanceFact::requested_target("trigger_scan"),
        GuidanceFact::candidate_symbol("Axon.Scanner.scan"),
        GuidanceFact::problem_signal("input_not_found"),
    ]);

    assert_eq!(outcome.problem_class.as_deref(), Some("input_not_found"));
    assert!(outcome
        .next_best_actions
        .contains(&"retry_with_suggested_symbol".to_string()));
}

#[test]
fn classify_guidance_marks_input_ambiguous() {
    let outcome = classify_guidance(&[
        GuidanceFact::requested_target("scan"),
        GuidanceFact::candidate_project_code("PJA"),
        GuidanceFact::candidate_project_code("PJB"),
        GuidanceFact::problem_signal("input_ambiguous"),
    ]);

    assert_eq!(outcome.problem_class.as_deref(), Some("input_ambiguous"));
    assert!(outcome
        .next_best_actions
        .contains(&"pick_exact_symbol".to_string()));
}

#[test]
fn classify_guidance_marks_degraded_for_index_incomplete() {
    let outcome = classify_guidance(&[
        GuidanceFact::requested_target("scan"),
        GuidanceFact::IndexIncomplete,
    ]);

    assert_eq!(outcome.problem_class.as_deref(), Some("degraded"));
    assert_eq!(
        outcome.likely_cause.as_deref(),
        Some("graph_index_not_fully_ready")
    );
    assert!(outcome
        .next_best_actions
        .contains(&"retry_after_runtime_stabilizes".to_string()));
}

#[test]
fn classify_guidance_marks_missing_rationale_in_soll_with_authorized_recommendation() {
    let outcome = classify_guidance(&[
        GuidanceFact::requested_target("retrieve_context"),
        GuidanceFact::problem_signal("missing_rationale_in_soll"),
    ]);

    assert_eq!(
        outcome.problem_class.as_deref(),
        Some("missing_rationale_in_soll")
    );
    assert_eq!(
        outcome
            .soll
            .as_ref()
            .and_then(|soll| soll.requires_authorization),
        Some(true)
    );
}

#[test]
fn classify_guidance_marks_tool_unavailable() {
    let outcome = classify_guidance(&[GuidanceFact::problem_signal("tool_unavailable")]);
    assert_eq!(outcome.problem_class.as_deref(), Some("tool_unavailable"));
}

#[test]
fn classify_guidance_marks_degraded_for_backend_pressure() {
    let outcome = classify_guidance(&[GuidanceFact::problem_signal("backend_pressure")]);
    assert_eq!(outcome.problem_class.as_deref(), Some("degraded"));
    assert_eq!(
        outcome.likely_cause.as_deref(),
        Some("runtime_pressure_reduces_reliability")
    );
}

#[test]
fn classify_guidance_marks_degraded_for_vectorization_incomplete() {
    let outcome = classify_guidance(&[GuidanceFact::VectorizationIncomplete]);
    assert_eq!(outcome.problem_class.as_deref(), Some("degraded"));
    assert_eq!(
        outcome.likely_cause.as_deref(),
        Some("semantic_layer_not_fully_ready")
    );
}

#[test]
fn classify_guidance_marks_intent_missing_in_soll() {
    let outcome = classify_guidance(&[GuidanceFact::problem_signal("intent_missing_in_soll")]);
    assert_eq!(
        outcome.problem_class.as_deref(),
        Some("intent_missing_in_soll")
    );
    assert_eq!(
        outcome.soll.as_ref().map(|soll| soll.reason.as_str()),
        Some("missing_intent_evidence")
    );
}

#[test]
fn query_guidance_facts_capture_backend_pressure_signal() {
    let server = create_test_server();
    let facts = server.extract_query_guidance_facts(
        "scan",
        Some("AXO"),
        &GuidanceCandidates::default(),
        0,
        false,
        false,
        true,
    );

    assert!(facts.contains(&GuidanceFact::problem_signal("backend_pressure")));
}

#[test]
fn inspect_shadow_guidance_emits_debug_payload_when_enabled() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_MCP_GUIDANCE_SHADOW", "1");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::axon_retrieve_context', 'axon_retrieve_context', 'method', true, true, false, 'AXO')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "inspect",
                "arguments": { "symbol": "axon_retrieve_contex", "project": "AXO" }
            })),
            id: Some(json!(6210)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(
        result["data"]["_shadow"]["guidance"]["problem_class"],
        "input_not_found"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }
}

#[test]
fn inspect_authoritative_guidance_emits_public_phase1_fields_when_enabled() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::axon_retrieve_context', 'axon_retrieve_context', 'method', true, true, false, 'AXO')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "inspect",
                "arguments": { "symbol": "axon_retrieve_contex", "project": "AXO" }
            })),
            id: Some(json!(6211)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["problem_class"], "input_not_found");
    assert_eq!(result["data"]["likely_cause"], "exact_symbol_mismatch");
    assert!(result["data"]["_shadow"].is_null());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
    }
}

#[test]
fn unavailable_tool_authoritative_guidance_emits_public_phase1_fields() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": { "query_text": "scan", "project": "AXO" }
            })),
            id: Some(json!(9001)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["problem_class"], "tool_unavailable");
    assert_eq!(
        result["data"]["likely_cause"],
        "runtime_profile_does_not_allow_tool"
    );
    assert!(result["data"]["_shadow"].is_null());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
    }
}

#[test]
fn query_shadow_guidance_marks_tool_unavailable_when_runtime_profile_blocks_tool() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var("AXON_MCP_GUIDANCE_SHADOW", "1");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": { "query": "scan", "project": "AXO" }
            })),
            id: Some(json!(6211)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(
        result["data"]["_shadow"]["guidance"]["problem_class"],
        "tool_unavailable"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }
}

#[test]
fn test_mcp_tools_list() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"restore_soll"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"soll_apply_plan"));
    assert!(tool_names.contains(&"soll_work_plan"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"snapshot_history"));
    assert!(tool_names.contains(&"snapshot_diff"));
    assert!(tool_names.contains(&"conception_view"));
    assert!(tool_names.contains(&"change_safety"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(tool_names.contains(&"axon_pre_flight_check"));
    assert!(tool_names.contains(&"job_status"));
    assert!(!tool_names.contains(&"retrieve_context"));
    assert!(!tool_names.contains(&"query"));
    assert!(!tool_names.contains(&"inspect"));
    assert!(!tool_names.contains(&"audit"));
    assert!(!tool_names.contains(&"impact"));
    assert!(!tool_names.contains(&"health"));
    assert!(!tool_names.contains(&"soll_apply_plan_v2"));
    assert!(tool_names.contains(&"refine_lattice"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"cypher"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"schema_overview"));
    assert!(tool_names.contains(&"list_labels_tables"));
    assert!(tool_names.contains(&"query_examples"));
    assert!(!tool_names.contains(&"truth_check"));
    assert!(!tool_names.contains(&"diagnose_indexing"));
    assert!(!tool_names.contains(&"diff"));
    assert!(!tool_names.contains(&"semantic_clones"));
    assert!(!tool_names.contains(&"architectural_drift"));
    assert!(!tool_names.contains(&"bidi_trace"));
    assert!(!tool_names.contains(&"api_break_check"));
    assert!(!tool_names.contains(&"simulate_mutation"));
    assert!(tool_names.contains(&"resume_vectorization"));
}

#[test]
fn test_mcp_tools_list_in_full_autonomous_exposes_core_and_hides_relegated_tools() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"architectural_drift"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_mcp_tools_list_include_internal_exposes_expert_tools_in_full_autonomous() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: Some(json!({ "include_internal": true })),
        id: Some(json!(1002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_mutating_soll_manager_returns_job_and_reserved_entity_id() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": "AXO",
                    "name": "Async Concept",
                    "explanation": "Created through MCP job",
                    "rationale": "Shared-server mutation path"
                }
            }
        })),
        id: Some(json!(5001)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let data = result.get("data").expect("job response must carry data");
    let job_id = data
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("job_id");
    let entity_id = data
        .get("reserved_ids")
        .and_then(|value| value.get("entity_id"))
        .and_then(|value| value.as_str())
        .expect("reserved entity_id");
    assert!(data
        .get("accepted")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(entity_id.starts_with("CPT-AXO-"), "{entity_id}");

    let final_status = wait_for_job_status(&server, job_id);
    assert_eq!(
        final_status["data"]["status"].as_str().unwrap(),
        "succeeded"
    );
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE id = '{}'",
                entity_id
            ))
            .unwrap(),
        1
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mutating_soll_apply_plan_returns_job_and_reserved_preview_id() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-job-preview",
                        "title": "Job Preview Requirement",
                        "description": "Dry-run should reserve preview id immediately"
                    }]
                }
            }
        })),
        id: Some(json!(5002)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let data = result.get("data").expect("job response must carry data");
    let job_id = data
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("job_id");
    let preview_id = data
        .get("reserved_ids")
        .and_then(|value| value.get("preview_id"))
        .and_then(|value| value.as_str())
        .expect("reserved preview_id");
    assert!(preview_id.starts_with("PRV-AXO-"), "{preview_id}");

    let final_status = wait_for_job_status(&server, job_id);
    assert_eq!(
        final_status["data"]["status"].as_str().unwrap(),
        "succeeded"
    );
    let result_preview_id = final_status["data"]["result"]["data"]["preview_id"]
        .as_str()
        .expect("preview id should survive job result");
    assert_eq!(result_preview_id, preview_id);

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mutating_soll_manager_requires_project_code_for_job_reservation() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "name": "Missing project scope",
                    "explanation": "Jobs must reject implicit project identity"
                }
            }
        })),
        id: Some(json!(5003)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Mutation job reservation failed"),
        "{content}"
    );
    assert!(
        content.contains("`project_code` est obligatoire"),
        "{content}"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mutating_soll_commit_revision_requires_preview_id_for_job_reservation() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_commit_revision",
            "arguments": {
                "author": "test"
            }
        })),
        id: Some(json!(5004)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Mutation job reservation failed"),
        "{content}"
    );
    assert!(
        content.contains("`preview_id` est obligatoire"),
        "{content}"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mcp_tools_list_hides_indexed_runtime_tools_in_graph_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "graph_only");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(!tool_names.contains(&"retrieve_context"));
    assert!(!tool_names.contains(&"query"));
    assert!(!tool_names.contains(&"inspect"));
    assert!(!tool_names.contains(&"audit"));
    assert!(!tool_names.contains(&"impact"));
    assert!(!tool_names.contains(&"health"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_returns_mode_error_in_graph_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "graph_only");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": {
                "query": "booking",
                "project": "BookingSystem"
            }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("unavailable in runtime mode 'graph_only'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_tools_list_hides_indexed_runtime_tools_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(!tool_names.contains(&"retrieve_context"));
    assert!(!tool_names.contains(&"query"));
    assert!(!tool_names.contains(&"inspect"));
    assert!(!tool_names.contains(&"audit"));
    assert!(!tool_names.contains(&"impact"));
    assert!(!tool_names.contains(&"health"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_returns_profile_error_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": {
                "query": "booking",
                "project": "BookingSystem"
            }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("profile 'full_isolated'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_pre_flight_check_alias_uses_dry_run_commit_work() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_pre_flight_check",
                "arguments": {
                    "diff_paths": ["docs/skills/axon-engineering-protocol/SKILL.md"]
                }
            })),
            id: Some(json!(2201)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Dry Run"), "{text}");
}

#[test]
fn test_status_reports_public_surface_and_runtime_truth() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2202)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let public_tools = data["public_tools"].as_array().unwrap();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(public_tool_names.contains(&"status"));
    assert!(public_tool_names.contains(&"project_status"));
    assert!(public_tool_names.contains(&"why"));
    assert!(public_tool_names.contains(&"path"));
    assert!(public_tool_names.contains(&"anomalies"));
    assert!(public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"cypher"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));
    assert!(public_tool_names.contains(&"batch"));
    assert!(public_tool_names.contains(&"job_status"));
    assert!(public_tool_names.contains(&"resume_vectorization"));
    assert!(!public_tool_names.contains(&"health"));
    assert!(!public_tool_names.contains(&"audit"));
    assert!(!public_tool_names.contains(&"truth_check"));
    assert!(data
        .get("runtime_mode")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data
        .get("runtime_profile")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data
        .get("truth_status")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data["availability"]["degraded_notes"].as_array().is_some());
    assert_eq!(
        data["canonical_sources"]["soll_export"]["reimportable"].as_bool(),
        Some(true)
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_status_reports_retrieve_context_in_public_surface_when_full_autonomous() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(22021)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let public_tools = data["public_tools"].as_array().unwrap();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"cypher"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"batch"));
    assert!(public_tool_names.contains(&"job_status"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(public_tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_status_exposes_runtime_version_identity() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RELEASE_VERSION", "0.7.0");
        std::env::set_var("AXON_BUILD_ID", "v0.7.0-rc1-12-gabcdef");
        std::env::set_var("AXON_PACKAGE_VERSION", "2.0.0");
        std::env::set_var("AXON_INSTALL_GENERATION", "live-2026-04-18");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["runtime_version"]["release_version"].as_str(),
        Some("0.7.0")
    );
    assert_eq!(
        data["runtime_version"]["build_id"].as_str(),
        Some("v0.7.0-rc1-12-gabcdef")
    );
    assert_eq!(
        data["runtime_version"]["package_version"].as_str(),
        Some("2.0.0")
    );
    assert_eq!(
        data["runtime_version"]["install_generation"].as_str(),
        Some("live-2026-04-18")
    );

    unsafe {
        std::env::remove_var("AXON_RELEASE_VERSION");
        std::env::remove_var("AXON_BUILD_ID");
        std::env::remove_var("AXON_PACKAGE_VERSION");
        std::env::remove_var("AXON_INSTALL_GENERATION");
    }
}

#[test]
fn test_status_exposes_resource_policy_identity() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RESOURCE_PRIORITY", "critical");
        std::env::set_var("AXON_BACKGROUND_BUDGET_CLASS", "balanced");
        std::env::set_var("AXON_GPU_ACCESS_POLICY", "preferred");
        std::env::set_var("AXON_WATCHER_POLICY", "full");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        std::env::set_var("MAX_AXON_WORKERS", "8");
        std::env::set_var("AXON_QUEUE_MEMORY_BUDGET_BYTES", "3221225472");
        std::env::set_var("AXON_WATCHER_SUBTREE_HINT_BUDGET", "128");
        std::env::set_var("AXON_VECTOR_WORKERS", "2");
        std::env::set_var("AXON_GRAPH_WORKERS", "3");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["resource_policy"]["resource_priority"].as_str(),
        Some("critical")
    );
    assert_eq!(
        data["resource_policy"]["background_budget_class"].as_str(),
        Some("balanced")
    );
    assert_eq!(
        data["resource_policy"]["gpu_access_policy"].as_str(),
        Some("preferred")
    );
    assert_eq!(
        data["resource_policy"]["watcher_policy"].as_str(),
        Some("full")
    );
    assert_eq!(
        data["resource_policy"]["embedding_provider"].as_str(),
        Some("cpu")
    );
    assert_eq!(
        data["resource_policy"]["max_axon_workers"].as_str(),
        Some("8")
    );
    assert_eq!(
        data["resource_policy"]["queue_memory_budget_bytes"].as_str(),
        Some("3221225472")
    );
    assert_eq!(
        data["resource_policy"]["watcher_subtree_hint_budget"].as_str(),
        Some("128")
    );
    assert_eq!(
        data["resource_policy"]["vector_workers"].as_str(),
        Some("2")
    );
    assert_eq!(data["resource_policy"]["graph_workers"].as_str(), Some("3"));

    unsafe {
        std::env::remove_var("AXON_RESOURCE_PRIORITY");
        std::env::remove_var("AXON_BACKGROUND_BUDGET_CLASS");
        std::env::remove_var("AXON_GPU_ACCESS_POLICY");
        std::env::remove_var("AXON_WATCHER_POLICY");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("MAX_AXON_WORKERS");
        std::env::remove_var("AXON_QUEUE_MEMORY_BUDGET_BYTES");
        std::env::remove_var("AXON_WATCHER_SUBTREE_HINT_BUDGET");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_GRAPH_WORKERS");
    }
}

#[test]
fn test_why_wraps_retrieve_context_and_reports_framework_alias() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/payment.rs', 'BKS', 'indexed')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout-why', 'symbol', 'bks::checkout', 'BKS', 'body', 'checkout orchestrates payment capture and settlement', 'hash-why-checkout', 1, 4)").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Operational payment choice', 'accepted', '{\"rationale\":\"Operational safety\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-BKS-WHY', 'Decision', 'DEC-BKS-010', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "why",
                "arguments": { "symbol": "checkout", "project": "BKS", "mode": "brief" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(response["data"]["framework_alias"].as_str(), Some("why"));
    assert_eq!(
        response["data"]["why"]["target"]["symbol"].as_str(),
        Some("checkout")
    );
    assert_eq!(
        response["data"]["why"]["target"]["project"].as_str(),
        Some("BKS")
    );
    assert_eq!(
        response["data"]["why"]["route"].as_str(),
        Some("soll_hybrid")
    );
    assert!(response["data"]["why"]["linked_intentions"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(
        response["data"]["why"]["supporting_artifacts"]["supporting_chunks"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(response["data"]["why"]["missing_evidence"]
        .as_array()
        .is_some());
    assert!(response["data"]["why"]["confidence"].as_object().is_some());
    assert_eq!(
        response["data"]["why"]["canonical_sources"]["soll_export"]["reimportable"].as_bool(),
        Some(true)
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Context Retrieval"), "{text}");
}

#[test]
fn test_project_status_assembles_live_project_situation_from_read_surfaces() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::wrapper', 'wrapper_fn', 'function', false, false, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::target', 'target_fn', 'function', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/lib.rs', 'AXO', 'indexed')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::wrapper', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::wrapper', 'axo::target', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'accepted', '{\"goal\":\"Vision first\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'draft', '{\"priority\":\"P1\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', 'Use Rust as the authoritative runtime', 'accepted', '{\"context\":\"\",\"rationale\":\"\"}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(22031)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(data["project_code"].as_str(), Some("AXO"));
    assert!(data["snapshot_id"].as_str().is_some());
    assert!(data["generated_at"].as_u64().is_some());
    assert!(data["delta_vs_previous"].as_object().is_some());
    assert_eq!(data["vision"]["id"].as_str(), Some("VIS-AXO-001"));
    assert_eq!(data["vision"]["source"].as_str(), Some("SOLL"));
    assert!(data["runtime"]["runtime_mode"].as_str().is_some());
    assert_eq!(data["conception"]["module_count"].as_u64(), Some(1));
    assert_eq!(data["conception"]["interface_count"].as_u64(), Some(0));
    assert_eq!(data["conception"]["contract_count"].as_u64(), Some(1));
    assert_eq!(data["conception"]["flow_count"].as_u64(), Some(0));
    assert!(data["conception"]["modules"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(data["conception"]["interfaces"].as_array().is_some());
    assert!(data["conception"]["contracts"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(data["conception"]["flows"].as_array().is_some());
    assert_eq!(
        data["anomalies"]["summary"]["note"].as_str(),
        Some("Anomalies calculation decoupled to prevent timeout. Use 'anomalies' tool directly.")
    );
    assert!(data["soll_context"]["visions"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Project Status"), "{text}");
    assert!(text.contains("Axon Vision"), "{text}");
}

#[test]
fn test_project_status_reports_delta_vs_previous_snapshot() {
    let _guard = env_lock();
    let history_dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_STRUCTURAL_HISTORY_DIR",
            history_dir.path().to_string_lossy().to_string(),
        );
    }
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/lib.rs', 'AXO', 'indexed')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::target', 'target_fn', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::wrapper', 'wrapper_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::wrapper', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::wrapper', 'axo::target', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'accepted', '{}')")
        .unwrap();

    let first = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(22032)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        first["data"]["delta_vs_previous"]["available"].as_bool(),
        Some(false)
    );

    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::orphan', 'orphan_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::orphan', 'AXO')")
        .unwrap();

    let second = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(22033)),
        })
        .unwrap()
        .result
        .unwrap();
    let delta = &second["data"]["delta_vs_previous"];
    assert_eq!(delta["available"].as_bool(), Some(true));
    assert_eq!(delta["wrapper_count_delta"].as_i64(), Some(0));
    assert_eq!(delta["orphan_code_count_delta"].as_i64(), Some(0));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_STRUCTURAL_HISTORY_DIR");
    }
}

#[test]
fn test_snapshot_history_and_diff_persist_outside_soll() {
    let _env = env_lock();
    let history_dir = tempdir().unwrap();
    std::env::set_var(
        "AXON_STRUCTURAL_HISTORY_DIR",
        history_dir.path().to_string_lossy().to_string(),
    );
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/lib.rs', 'AXO', 'indexed')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::target', 'target_fn', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::wrapper', 'wrapper_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::wrapper', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::wrapper', 'axo::target', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'accepted', '{}')")
        .unwrap();

    let first = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(23001)),
        })
        .unwrap()
        .result
        .unwrap();
    let first_snapshot = first["data"]["snapshot_id"]
        .as_str()
        .expect("first snapshot id")
        .to_string();

    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::orphan', 'orphan_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::orphan', 'AXO')")
        .unwrap();

    let second = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(23002)),
        })
        .unwrap()
        .result
        .unwrap();
    let second_snapshot = second["data"]["snapshot_id"]
        .as_str()
        .expect("second snapshot id")
        .to_string();

    let history = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "snapshot_history",
                "arguments": { "project_code": "AXO", "limit": 10 }
            })),
            id: Some(json!(23003)),
        })
        .unwrap()
        .result
        .unwrap();
    let history_items = history["data"]["snapshots"].as_array().expect("snapshots");
    assert_eq!(history_items.len(), 2);
    assert_eq!(
        history_items[0]["snapshot_id"].as_str(),
        Some(first_snapshot.as_str())
    );
    assert_eq!(
        history_items[1]["snapshot_id"].as_str(),
        Some(second_snapshot.as_str())
    );
    assert_eq!(
        history["data"]["storage"]["scope"].as_str(),
        Some("derived_non_canonical")
    );

    let diff = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "snapshot_diff",
                "arguments": {
                    "project_code": "AXO",
                    "from_snapshot_id": first_snapshot,
                    "to_snapshot_id": second_snapshot
                }
            })),
            id: Some(json!(23004)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        diff["data"]["from_snapshot_id"].as_str(),
        Some(first_snapshot.as_str())
    );
    assert_eq!(
        diff["data"]["to_snapshot_id"].as_str(),
        Some(second_snapshot.as_str())
    );
    assert_eq!(
        diff["data"]["metric_delta"]["orphan_code_count_delta"].as_i64(),
        Some(0)
    );
    assert_eq!(
        diff["data"]["metric_delta"]["wrapper_count_delta"].as_i64(),
        Some(0)
    );
    std::env::remove_var("AXON_STRUCTURAL_HISTORY_DIR");
}

#[test]
fn test_conception_view_and_change_safety_are_exposed_as_read_only_derivations() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/api.rs', 'AXO', 'indexed')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/impl.rs', 'AXO', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::iface', 'PaymentPort', 'interface', false, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::svc', 'charge_card', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/api.rs', 'axo::iface', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/impl.rs', 'axo::svc', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-010', 'Requirement', 'AXO', 'Card charging', 'Charge cards safely', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-1', 'Requirement', 'REQ-AXO-010', 'Symbol', 'charge_card', 1.0, 0)")
        .unwrap();

    let conception = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "conception_view",
                "arguments": { "project_code": "AXO", "mode": "full" }
            })),
            id: Some(json!(23005)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(conception["data"]["project_code"].as_str(), Some("AXO"));
    assert_eq!(
        conception["data"]["provenance"].as_str(),
        Some("derived_read_only_view")
    );
    assert!(conception["data"]["modules"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(conception["data"]["interfaces"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(conception["data"]["contracts"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(conception["data"]["flows"].as_array().is_some());

    let change_safety = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "change_safety",
                "arguments": {
                    "project_code": "AXO",
                    "target": "charge_card",
                    "target_type": "symbol"
                }
            })),
            id: Some(json!(23006)),
        })
        .unwrap()
        .result
        .unwrap();
    let data = change_safety.get("data").expect("data");
    assert_eq!(data["target"].as_str(), Some("charge_card"));
    assert_eq!(data["target_type"].as_str(), Some("symbol"));
    assert_eq!(data["change_safety"].as_str(), Some("safe"));
    assert_eq!(data["provenance"].as_str(), Some("aggregated"));
    assert!(data["coverage_signals"].as_object().is_some());
    assert!(data["traceability_signals"].as_object().is_some());
    assert!(data["validation_signals"].as_object().is_some());
    assert!(data["reasoning"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(data["recommended_guardrails"].as_array().is_some());
}

#[test]
fn test_path_returns_bounded_call_path_between_symbols() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::source', 'source_fn', 'function', true, true, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::mid', 'mid_fn', 'function', true, false, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::sink', 'sink_fn', 'function', true, true, false, 'BKS')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('bks::source', 'bks::mid', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('bks::mid', 'bks::sink', 'BKS')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "path",
                "arguments": {
                    "source": "source_fn",
                    "sink": "sink_fn",
                    "project": "BKS",
                    "depth": 4
                }
            })),
            id: Some(json!(2204)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(data["path_found"].as_bool(), Some(true));
    assert_eq!(data["path_type"].as_str(), Some("bounded_call_path"));
    assert_eq!(data["bounded_depth_used"].as_u64(), Some(4));
    assert!(data["detours"].as_array().is_some());
    assert_eq!(
        data["canonical_sources"]["soll_export"]["reimportable"].as_bool(),
        Some(true)
    );
    let path = data["path"].as_array().unwrap();
    let rendered = path
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(rendered, vec!["source_fn", "mid_fn", "sink_fn"]);
}

#[test]
fn test_anomalies_reports_wrappers_and_orphans_with_actions() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::wrapper', 'wrapper_fn', 'function', false, false, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::target', 'target_fn', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::orphan', 'orphan_fn', 'function', false, false, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/lib.rs', 'AXO', 'indexed')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::wrapper', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/lib.rs', 'axo::orphan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::wrapper', 'axo::target', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-099', 'Requirement', 'AXO', 'Unimplemented requirement', 'No traceability yet', 'draft', '{\"priority\":\"P2\"}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "anomalies",
                "arguments": { "project": "AXO", "mode": "brief" }
            })),
            id: Some(json!(2205)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let findings = data["findings"].as_array().unwrap();
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("wrapper")));
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("orphan_code")));
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("orphan_intent")));
    assert!(findings.iter().all(|finding| finding
        .get("recommended_action")
        .and_then(|value| value.as_str())
        .is_some()));
    assert!(findings.iter().all(|finding| finding
        .get("estimated_effort")
        .and_then(|value| value.as_str())
        .is_some()));
    assert!(findings.iter().all(|finding| finding
        .get("estimated_risk")
        .and_then(|value| value.as_str())
        .is_some()));
    assert!(findings.iter().all(|finding| finding
        .get("validation_signals")
        .and_then(|value| value.as_object())
        .is_some()));
    assert_eq!(
        data["summary"]["validation_coverage_score"].as_i64(),
        Some(33)
    );
    assert_eq!(data["summary"]["total_symbols"].as_i64(), Some(3));
    assert_eq!(data["summary"]["total_intent_nodes"].as_i64(), Some(1));
    assert_eq!(
        data["summary"]["alignment_proxy_score"].as_f64(),
        Some(33.3)
    );
    assert_eq!(
        data["summary"]["rectitude_proxy_score"].as_f64(),
        Some(66.7)
    );
    assert_eq!(data["summary"]["cycle_health_score"].as_f64(), Some(100.0));
    assert_eq!(data["summary"]["orphan_code_rate"].as_f64(), Some(66.7));
    assert_eq!(data["summary"]["orphan_intent_rate"].as_f64(), Some(100.0));

    let recommendations = data["recommendations"].as_array().expect("recommendations");
    assert!(!recommendations.is_empty());
    assert!(recommendations.iter().all(|item| item
        .get("sequencing_dependencies")
        .and_then(|value| value.as_array())
        .is_some()));
    assert!(recommendations.iter().all(|item| item
        .get("validation_signals")
        .and_then(|value| value.as_object())
        .is_some()));

    let wrapper = findings
        .iter()
        .find(|finding| finding["type"].as_str() == Some("wrapper"))
        .expect("wrapper finding");
    assert_eq!(
        wrapper["validation_signals"]["tested"].as_bool(),
        Some(false)
    );
    assert_eq!(
        wrapper["validation_signals"]["traceability_links"].as_u64(),
        Some(0)
    );

    let orphan_intent = findings
        .iter()
        .find(|finding| finding["type"].as_str() == Some("orphan_intent"))
        .expect("orphan_intent finding");
    assert_eq!(
        orphan_intent["validation_signals"]["validation_nodes"].as_u64(),
        Some(0)
    );

    let first_recommendation = recommendations.first().expect("top recommendation");
    assert_eq!(first_recommendation["severity"].as_str(), Some("high"));
}

#[test]
fn test_anomalies_report_feature_envy_detours_and_abstraction_detours() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/source.rs', 'AXO', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/foreign.rs', 'AXO', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/interface.rs', 'AXO', 'indexed')")
        .unwrap();

    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::source', 'source_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::local_helper', 'local_helper', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::foreign_a', 'foreign_a', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::foreign_b', 'foreign_b', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::entry', 'entry_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::bridge', 'bridge_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::sink', 'sink_fn', 'function', false, false, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::iface', 'CheckoutPort', 'interface', false, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::iface_impl', 'CheckoutPortImpl', 'class', false, true, false, 'AXO')")
        .unwrap();

    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/source.rs', 'axo::source', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/source.rs', 'axo::local_helper', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/source.rs', 'axo::entry', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/source.rs', 'axo::bridge', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/source.rs', 'axo::sink', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/foreign.rs', 'axo::foreign_a', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/foreign.rs', 'axo::foreign_b', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/interface.rs', 'axo::iface', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/interface.rs', 'axo::iface_impl', 'AXO')")
        .unwrap();

    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::source', 'axo::local_helper', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::source', 'axo::foreign_a', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::source', 'axo::foreign_b', 'AXO')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::entry', 'axo::bridge', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axo::bridge', 'axo::sink', 'AXO')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "anomalies",
                "arguments": { "project": "AXO", "mode": "brief" }
            })),
            id: Some(json!(2206)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let findings = data["findings"].as_array().unwrap();
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("feature_envy")));
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("detour")));
    assert!(findings
        .iter()
        .any(|finding| finding["type"].as_str() == Some("abstraction_detour")));
    assert_eq!(data["summary"]["feature_envy_count"].as_u64(), Some(1));
    assert_eq!(data["summary"]["detour_count"].as_u64(), Some(1));
    assert_eq!(
        data["summary"]["abstraction_detour_count"].as_u64(),
        Some(1)
    );
    assert_eq!(data["summary"]["alignment_proxy_score"].as_f64(), Some(0.0));
    assert_eq!(
        data["summary"]["rectitude_proxy_score"].as_f64(),
        Some(57.1)
    );
    assert_eq!(data["summary"]["cycle_health_score"].as_f64(), Some(100.0));
}

#[test]
fn test_soll_work_plan_orders_decision_requirement_milestone_chain() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'draft', '{\"priority\":\"P1\"}')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', '', 'accepted', '{\"context\":\"\",\"rationale\":\"\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-AXO-001', 'Milestone', 'AXO', 'Deliver runtime slice', '', 'planned', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'MIL-AXO-001', 'BELONGS_TO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(501)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let _waves = data
        .get("ordered_waves")
        .and_then(|v| v.as_array())
        .expect("waves array");

    assert_eq!(data["summary"]["cycle_count"].as_u64(), Some(0));
}

#[test]
fn test_soll_work_plan_groups_parallel_ready_nodes_in_same_wave() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'Operator cockpit', '', 'draft', '{\"priority\":\"P2\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(502)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let waves = data["ordered_waves"].as_array().expect("waves");
    let first_wave_items = waves[0]["items"].as_array().expect("items");

    assert_eq!(waves.len(), 1, "{:?}", data);
    assert_eq!(first_wave_items.len(), 2, "{:?}", data);
    assert_eq!(first_wave_items[0]["id"].as_str(), Some("REQ-AXO-001"));
    assert_eq!(first_wave_items[1]["id"].as_str(), Some("REQ-AXO-002"));
}

#[test]
fn test_soll_work_plan_reports_cycles_and_blocks_dependents() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'B', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'C', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'REQ-AXO-002', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-002', 'REQ-AXO-001', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'REQ-AXO-003', 'BELONGS_TO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(503)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let cycles = data["cycles"].as_array().expect("cycles");
    let blockers = data["blockers"].as_array().expect("blockers");
    let waves = data["ordered_waves"].as_array().expect("waves");

    assert_eq!(cycles.len(), 1, "{:?}", data);
    assert!(cycles[0]["node_ids"].to_string().contains("REQ-AXO-001"));
    assert!(cycles[0]["node_ids"].to_string().contains("REQ-AXO-002"));
    assert!(blockers
        .iter()
        .any(|v| v["id"].as_str() == Some("REQ-AXO-003")));
    assert!(waves.is_empty(), "{:?}", data);
}

#[test]
fn test_soll_work_plan_returns_contract_fields() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "include_ist": true }
        })),
        id: Some(json!(504)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");

    assert!(data.get("summary").is_some(), "{:?}", data);
    assert!(data.get("blockers").is_some(), "{:?}", data);
    assert!(data.get("cycles").is_some(), "{:?}", data);
    assert!(data.get("ordered_waves").is_some(), "{:?}", data);
    assert!(data.get("top_recommendations").is_some(), "{:?}", data);
    assert!(data.get("validation_gates").is_some(), "{:?}", data);
    assert!(data.get("metadata").is_some(), "{:?}", data);
    assert_eq!(data["metadata"]["algorithm_version"].as_str(), Some("v1"));
}

#[test]
fn test_soll_work_plan_respects_limit_and_marks_truncated() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'B', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'C', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "limit": 2 }
        })),
        id: Some(json!(505)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let waves = data["ordered_waves"].as_array().expect("waves");
    let items = waves[0]["items"].as_array().expect("items");

    assert_eq!(items.len(), 2, "{:?}", data);
    assert_eq!(data["summary"]["returned_items"].as_u64(), Some(2));
    assert_eq!(data["metadata"]["truncated"].as_bool(), Some(true));
}

#[test]
fn test_soll_work_plan_returns_top_recommendations() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'D1', '', 'accepted', '{\"context\":\"\",\"rationale\":\"\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "top": 1 }
        })),
        id: Some(json!(506)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let top = data["top_recommendations"]
        .as_array()
        .expect("top recommendations");

    assert_eq!(top.len(), 1, "{:?}", data);
    assert_eq!(top[0]["id"].as_str(), Some("DEC-AXO-001"));
    assert_eq!(top[0]["kind"].as_str(), Some("unblocker"));
    assert_eq!(data["summary"]["top_count"].as_u64(), Some(1));
    assert_eq!(data["metadata"]["top"].as_u64(), Some(1));
}

#[test]
fn test_axon_debug_reports_backlog_memory_and_storage_views() {
    let _guard = env_lock();
    std::env::remove_var("AXON_ENABLE_GRAPH_VECTORIZATION");
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
        std::env::set_var("AXON_EMBEDDING_GPU_PRESENT", "false");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::set_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION", "true");
        std::env::set_var("AXON_QUERY_EMBED_WORKERS", "2");
        std::env::set_var("AXON_VECTOR_WORKERS", "5");
        std::env::set_var("AXON_GRAPH_WORKERS", "3");
        std::env::set_var("AXON_CHUNK_BATCH_SIZE", "64");
        std::env::set_var("AXON_FILE_VECTORIZATION_BATCH_SIZE", "24");
    }
    reset_vector_batch_controller_for_tests(&embedding_lane_config_from_env());
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::Fetch, 11);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::Embed, 22);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::DbWrite, 33);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::CompletionCheck, 44);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::MarkDone, 55);
    service_guard::record_vector_embed_call(7, 3);
    service_guard::record_vector_files_completed(2);
    service_guard::record_vector_claimed_work_items(5);
    service_guard::record_vector_partial_file_cycles(4);
    service_guard::record_vector_mark_done_call();
    service_guard::record_vector_prepare_dispatch();
    service_guard::record_vector_prepare_prefetch();
    service_guard::record_vector_prepare_fallback_inline();
    service_guard::record_vector_prepare_outcome(9, 2, 1);
    service_guard::record_vector_prepare_reply_wait_ms(66);
    service_guard::record_vector_prepare_send_wait_ms(77);
    service_guard::record_vector_prepare_queue_wait_ms(78);
    service_guard::record_vector_prepare_queue_depth(2);
    service_guard::record_vector_embed_inputs(7, 700, 9);
    service_guard::record_vector_embed_breakdown(10, 12);
    service_guard::record_vector_finalize_enqueued();
    service_guard::record_vector_finalize_fallback_inline();
    service_guard::record_vector_finalize_send_wait_ms(88);
    service_guard::record_vector_finalize_queue_wait_ms(89);
    service_guard::record_vector_finalize_queue_depth(3);
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store.clone());

    store
        .execute(
            "INSERT INTO File (path, project_code, status, size, mtime, priority) VALUES \
             ('src/a.rs', 'AXO', 'indexed', 10, 1, 100), \
             ('src/b.rs', 'AXO', 'pending', 20, 1, 100), \
             ('src/c.rs', 'AXO', 'indexing', 30, 1, 100), \
             ('src/d.rs', 'AXO', 'indexed_degraded', 40, 1, 100), \
             ('src/e.rs', 'AXO', 'oversized_for_current_budget', 50, 1, 100), \
             ('src/f.rs', 'AXO', 'skipped', 60, 1, 100)",
        )
        .unwrap();
    store
        .execute(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES \
             ('axon::a', 'a', 'function', false, true, false, false, 'AXO')"
        )
        .unwrap();
    store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/a.rs', 'axon::a', 'AXO')")
        .unwrap();
    store
        .execute(
            "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) VALUES \
             ('file', 'src/a.rs', 2, 'queued', 0, 1, NULL, NULL), \
             ('file', 'src/b.rs', 2, 'inflight', 0, 1, NULL, NULL)",
        )
        .unwrap();
    store
        .execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
             ('src/a.rs', 'queued', 1), \
             ('src/b.rs', 'inflight', 1)",
        )
        .unwrap();

    let response = server.axon_debug().expect("debug response");
    let content = response["content"][0]["text"].as_str().unwrap_or_default();
    let contract = &response["data"]["embedding_contract"];

    assert!(content.contains("Fichiers connus : 6"), "{content}");
    assert!(content.contains("Backlog restant : 2"), "{content}");
    assert!(content.contains("Pending : 1"), "{content}");
    assert!(content.contains("Indexing : 1"), "{content}");
    assert!(content.contains("Indexed degraded : 1"), "{content}");
    assert!(content.contains("Oversized : 1"), "{content}");
    assert!(content.contains("Skipped : 1"), "{content}");
    assert!(content.contains("Stockage DuckDB"), "{content}");
    assert!(content.contains("RSS Anon"), "{content}");
    assert!(content.contains("Mémoire DuckDB"), "{content}");
    assert!(content.contains("Ingress Buffer"), "{content}");
    assert!(
        content.contains("Graph Projection Queue Queued : 1"),
        "{content}"
    );
    assert!(
        content.contains("Graph Projection Queue Inflight : 1"),
        "{content}"
    );
    assert!(
        content.contains("Graph Projection Queue Pending : 2"),
        "{content}"
    );
    assert!(content.contains("GPU Present Detected : no"), "{content}");
    assert!(
        content.contains("Embedding Provider Requested : cuda"),
        "{content}"
    );
    assert!(
        content.contains("Embedding Provider Effective"),
        "{content}"
    );
    assert!(
        content.contains("Embedding Acceleration State : gpu_requested_but_unavailable"),
        "{content}"
    );
    assert!(content.contains("Query Workers : 2"), "{content}");
    assert!(content.contains("Vector Workers : 5"), "{content}");
    assert!(content.contains("Graph Workers : 3"), "{content}");
    assert!(content.contains("Vector Runtime Breakdown"), "{content}");
    assert!(content.contains("Fetch ms total : 11"), "{content}");
    assert!(content.contains("Embed ms total : 22"), "{content}");
    assert!(content.contains("DB write ms total : 33"), "{content}");
    assert!(content.contains("Prepare dispatch total : 1"), "{content}");
    assert!(content.contains("Prepare prefetch total : 1"), "{content}");
    assert!(
        content.contains("Prepare fallback inline total : 1"),
        "{content}"
    );
    assert!(
        content.contains("Prepare reply wait ms total : 66"),
        "{content}"
    );
    assert!(
        content.contains("Prepare send wait ms total : 77"),
        "{content}"
    );
    assert!(
        content.contains("Prepare queue wait ms total : 78"),
        "{content}"
    );
    assert!(
        content.contains("Prepare queue depth current/max : 2/2"),
        "{content}"
    );
    assert!(content.contains("Embed input texts total : 7"), "{content}");
    assert!(
        content.contains("Embed input text bytes total : 700"),
        "{content}"
    );
    assert!(content.contains("Embed clone ms total : 9"), "{content}");
    assert!(content.contains("Finalize enqueued total : 1"), "{content}");
    assert!(
        content.contains("Finalize fallback inline total : 1"),
        "{content}"
    );
    assert!(
        content.contains("Finalize send wait ms total : 88"),
        "{content}"
    );
    assert!(
        content.contains("Finalize queue wait ms total : 89"),
        "{content}"
    );
    assert!(
        content.contains("Finalize queue depth current/max : 3/3"),
        "{content}"
    );
    assert!(content.contains("Embed calls total : 1"), "{content}");
    assert!(
        content.contains("Claimed work items total : 5"),
        "{content}"
    );
    assert!(
        content.contains("Partial file cycles total : 4"),
        "{content}"
    );
    assert!(content.contains("Mark done calls total : 1"), "{content}");
    assert!(content.contains("Files touched total : 3"), "{content}");
    assert!(
        content.contains("Avg chunks per embed call : 7.00"),
        "{content}"
    );
    assert!(
        content.contains("Avg files per embed call : 3.00"),
        "{content}"
    );
    assert!(
        content.contains("Avg embed input texts per call : 7.00"),
        "{content}"
    );
    assert!(
        content.contains("Avg embed input bytes per call : 700.00"),
        "{content}"
    );
    assert!(
        content.contains("Avg embed input bytes per chunk : 100.00"),
        "{content}"
    );
    assert!(
        content.contains("Embed clone ms per call : 9.00"),
        "{content}"
    );
    assert!(
        content.contains("File vectorization queue statuses"),
        "{content}"
    );
    assert!(content.contains("`inflight` : 1"), "{content}");
    assert!(content.contains("`queued` : 1"), "{content}");
    assert!(
        content.contains("Vector Stage Latencies (recent window)"),
        "{content}"
    );
    assert!(
        content.contains("Fetch p50/p95/max ms : 11/11/11 (samples: 1)"),
        "{content}"
    );
    assert!(
        content.contains("Embed p50/p95/max ms : 22/22/22 (samples: 1)"),
        "{content}"
    );
    assert!(
        content.contains("Drain State : gpu_scaling_blocked"),
        "{content}"
    );
    assert!(
        content.contains("GPU Background Worker Cap : 0"),
        "{content}"
    );
    assert_eq!(contract["model_name"].as_str(), Some(MODEL_NAME));
    assert_eq!(contract["dimension"].as_u64(), Some(DIMENSION as u64));
    assert_eq!(
        contract["native_dimension"].as_u64(),
        Some(NATIVE_DIMENSION as u64)
    );
    assert_eq!(contract["max_length"].as_u64(), Some(MAX_LENGTH as u64));
    assert_eq!(contract["storage_type"].as_str(), Some("float16"));
    assert_eq!(contract["gpu_present_detected"].as_bool(), Some(false));
    assert_eq!(contract["provider_requested"].as_str(), Some("cuda"));
    assert!(contract["provider_effective"].as_str().is_some());
    assert_eq!(contract["provider_gpu_mismatch"].as_bool(), Some(true));
    assert_eq!(
        contract["acceleration_state"].as_str(),
        Some("gpu_requested_but_unavailable")
    );
    assert_eq!(
        contract["drain_state"].as_str(),
        Some("gpu_scaling_blocked")
    );
    assert_eq!(contract["gpu_background_worker_cap"].as_u64(), Some(0));
    assert_eq!(
        contract["vector_runtime"]["prepare_dispatch_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_prefetch_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_fallback_inline_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["prepared_work_items_total"].as_u64(),
        Some(9)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_empty_batches_total"].as_u64(),
        Some(0)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_immediate_completed_total"].as_u64(),
        Some(2)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_failed_fetches_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_reply_wait_ms_total"].as_u64(),
        Some(66)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_send_wait_ms_total"].as_u64(),
        Some(77)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_queue_wait_ms_total"].as_u64(),
        Some(78)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_queue_depth_current"].as_u64(),
        Some(2)
    );
    assert_eq!(
        contract["vector_runtime"]["prepare_queue_depth_max"].as_u64(),
        Some(2)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_input_texts_total"].as_u64(),
        Some(7)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_input_text_bytes_total"].as_u64(),
        Some(700)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_clone_ms_total"].as_u64(),
        Some(9)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_transform_ms_total"].as_u64(),
        Some(10)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_export_ms_total"].as_u64(),
        Some(12)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_enqueued_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_fallback_inline_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_send_wait_ms_total"].as_u64(),
        Some(88)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_queue_wait_ms_total"].as_u64(),
        Some(89)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_queue_depth_current"].as_u64(),
        Some(3)
    );
    assert_eq!(
        contract["vector_runtime"]["finalize_queue_depth_max"].as_u64(),
        Some(3)
    );
    assert_eq!(contract["query_workers"].as_u64(), Some(2));
    assert_eq!(contract["vector_workers"].as_u64(), Some(5));
    assert_eq!(contract["graph_workers"].as_u64(), Some(3));
    assert_eq!(contract["max_chunks_per_file"].as_u64(), Some(64));
    assert_eq!(
        contract["max_embed_batch_bytes"].as_u64(),
        Some(4 * 1024 * 1024)
    );
    assert_eq!(
        contract["vector_runtime"]["fetch_ms_total"].as_u64(),
        Some(11)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_ms_total"].as_u64(),
        Some(22)
    );
    assert_eq!(
        contract["vector_runtime"]["db_write_ms_total"].as_u64(),
        Some(33)
    );
    assert_eq!(
        contract["vector_runtime"]["completion_check_ms_total"].as_u64(),
        Some(44)
    );
    assert_eq!(
        contract["vector_runtime"]["mark_done_ms_total"].as_u64(),
        Some(55)
    );
    assert_eq!(
        contract["vector_runtime"]["batches_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["chunks_embedded_total"].as_u64(),
        Some(7)
    );
    assert_eq!(
        contract["vector_runtime"]["files_completed_total"].as_u64(),
        Some(2)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_calls_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["claimed_work_items_total"].as_u64(),
        Some(5)
    );
    assert_eq!(
        contract["vector_runtime"]["partial_file_cycles_total"].as_u64(),
        Some(4)
    );
    assert_eq!(
        contract["vector_runtime"]["mark_done_calls_total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["files_touched_total"].as_u64(),
        Some(3)
    );
    assert_eq!(
        contract["vector_runtime"]["avg_chunks_per_embed_call"].as_f64(),
        Some(7.0)
    );
    assert_eq!(
        contract["vector_runtime"]["avg_files_per_embed_call"].as_f64(),
        Some(3.0)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["db_write"]["p95_ms"].as_u64(),
        Some(33)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["mark_done"]["p95_ms"].as_u64(),
        Some(55)
    );
    assert_eq!(
        contract["vector_batch_controller"]["state"].as_str(),
        Some("holding")
    );
    assert_eq!(
        contract["vector_runtime"]["avg_embed_input_texts_per_call"].as_f64(),
        Some(7.0)
    );
    assert_eq!(
        contract["vector_runtime"]["avg_embed_input_bytes_per_call"].as_f64(),
        Some(700.0)
    );
    assert_eq!(
        contract["vector_runtime"]["avg_embed_input_bytes_per_chunk"].as_f64(),
        Some(100.0)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_transform_ms_per_call"].as_f64(),
        Some(10.0)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_export_ms_per_call"].as_f64(),
        Some(12.0)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_clone_ms_per_call"].as_f64(),
        Some(9.0)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_transform_ms_per_chunk"].as_f64(),
        Some(10.0 / 7.0)
    );
    assert_eq!(
        contract["vector_runtime"]["embed_export_ms_per_chunk"].as_f64(),
        Some(12.0 / 7.0)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["fetch"]["samples"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["fetch"]["p50_ms"].as_u64(),
        Some(11)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["fetch"]["p95_ms"].as_u64(),
        Some(11)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["fetch"]["max_ms"].as_u64(),
        Some(11)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["embed"]["samples"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["embed"]["p50_ms"].as_u64(),
        Some(22)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["db_write"]["p50_ms"].as_u64(),
        Some(33)
    );
    assert_eq!(
        contract["vector_runtime"]["latency_recent"]["mark_done"]["p50_ms"].as_u64(),
        Some(55)
    );
    assert_eq!(
        contract["file_vectorization_queue_statuses"]
            .as_array()
            .map(|v| v.len()),
        Some(2)
    );
    assert_eq!(
        contract["file_vectorization_queue_statuses"][0]["status"].as_str(),
        Some("inflight")
    );
    assert_eq!(
        contract["file_vectorization_queue_statuses"][0]["count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        contract["file_vectorization_queue_statuses"][1]["status"].as_str(),
        Some("queued")
    );
    assert_eq!(
        contract["file_vectorization_queue_statuses"][1]["count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        contract["vector_batch_controller"]["state"].as_str(),
        Some("holding")
    );
    assert_eq!(
        contract["vector_batch_controller"]["target_embed_batch_chunks"].as_u64(),
        Some(64)
    );
    assert_eq!(
        contract["vector_batch_controller"]["target_files_per_cycle"].as_u64(),
        Some(24)
    );
    assert_eq!(contract["chunk_model_id"].as_str(), Some(CHUNK_MODEL_ID));

    unsafe {
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
        std::env::remove_var("AXON_EMBEDDING_GPU_PRESENT");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_ALLOW_GPU_EMBED_OVERSUBSCRIPTION");
        std::env::remove_var("AXON_QUERY_EMBED_WORKERS");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_GRAPH_WORKERS");
        std::env::remove_var("AXON_CHUNK_BATCH_SIZE");
        std::env::remove_var("AXON_FILE_VECTORIZATION_BATCH_SIZE");
    }
    service_guard::reset_for_tests();
}

#[test]
fn test_axon_list_labels_tables_reclassifies_graph_tables_as_derived_optional() {
    let _guard = env_lock();
    std::env::remove_var("AXON_ENABLE_GRAPH_VECTORIZATION");
    let server = create_test_server();

    let response = server
        .axon_list_labels_tables(&json!({}))
        .expect("labels/tables response");
    let content = response["content"][0]["text"].as_str().unwrap_or_default();

    assert!(content.contains("Core tables"), "{content}");
    assert!(content.contains("Derived optional tables"), "{content}");
    assert!(content.contains("GraphEmbedding"), "{content}");
    assert!(content.contains("GraphProjectionQueue"), "{content}");
}

#[test]
fn test_axon_debug_reports_top_pending_reasons() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store.clone());

    store
        .execute(
            "INSERT INTO File (path, project_code, status, status_reason, size, mtime, priority) VALUES \
             ('src/a.rs', 'AXO', 'pending', 'metadata_changed_scan', 10, 1, 100), \
             ('src/b.rs', 'AXO', 'pending', 'metadata_changed_scan', 20, 1, 100), \
             ('src/c.rs', 'AXO', 'indexing', 'needs_reindex_while_indexing', 30, 1, 100), \
             ('src/d.rs', 'AXO', 'pending', 'manual_or_system_requeue', 40, 1, 100)"
        )
        .unwrap();

    let response = server.axon_debug().expect("debug response");
    let content = response["content"][0]["text"].as_str().unwrap_or_default();

    assert!(content.contains("Causes backlog dominantes"), "{content}");
    assert!(
        content.contains("`metadata_changed_scan`")
            || content.contains("`manual_or_system_requeue`")
            || content.contains("`needs_reindex_while_indexing`")
            || content.contains("`unknown`"),
        "{content}"
    );
}

#[test]
fn test_status_reports_canonical_file_vectorization_queue_counts() {
    let _guard = env_lock();
    let server = create_test_server();

    server
        .graph_store
        .execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
             ('/tmp/q.rs', 'queued', 1), \
             ('/tmp/i.rs', 'inflight', 2)",
        )
        .unwrap();

    let response = server.axon_status(&json!({})).expect("status response");
    let content = response["content"][0]["text"].as_str().unwrap_or_default();

    assert!(
        content.contains("**Vector backlog:** queued=1 inflight=1"),
        "{content}"
    );
}

#[test]
fn test_status_exposes_traceability_optimizer_snapshots_and_latest_logs() {
    let _guard = env_lock();
    let server = create_test_server();

    server
        .graph_store
        .log_optimizer_decision(
            "opt-1",
            1000,
            "shadow",
            "{\"host\":true}",
            "{\"policy\":true}",
            "{\"signals\":true}",
            "{\"analytics\":true}",
            "hold",
            "{\"decision\":true}",
            "[\"cpu\"]",
            false,
            false,
            1000,
            2000,
        )
        .unwrap();
    server
        .graph_store
        .log_reward_observation(
            "opt-1",
            2000,
            1000,
            2000,
            "{\"reward\":1}",
            123.0,
            4.0,
            "{\"cpu\":0}",
            "{\"pressure\":\"low\"}",
        )
        .unwrap();

    let response = server.axon_status(&json!({})).expect("status response");
    let traceability = &response["data"]["traceability"];

    assert!(traceability["host_snapshot"].is_object());
    assert!(traceability["policy_snapshot"].is_object());
    assert!(traceability["runtime_signals_window"].is_object());
    assert!(traceability["recent_analytics_window"].is_object());
    assert_eq!(
        traceability["latest_optimizer_decision"]["decision_id"].as_str(),
        Some("opt-1")
    );
    assert_eq!(
        traceability["latest_reward_observation"]["decision_id"].as_str(),
        Some("opt-1")
    );
}

#[test]
fn test_axon_architectural_drift() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('ui/app.js', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::fetchData', 'fetchData', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('db/repo.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::executeSQL', 'executeSQL', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('ui/app.js', 'prj::fetchData', 'PRJ')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('db/repo.rs', 'prj::executeSQL', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::fetchData', 'prj::executeSQL', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "architectural_drift",
            "arguments": { "source_layer": "ui", "target_layer": "db" }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    println!("AUDIT_ALPHA_CONTENT={content}");

    assert!(
        content.contains("VIOLATION")
            || content.contains("Détectée")
            || content.contains("détectée")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_query_with_project() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('prj/f1.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('prj/f2.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::auth_func', 'auth_func', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('prj/f1.rs', 'prj::auth_func', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "auth", "project": "PRJ" }
        })),
        id: Some(json!(3)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    println!("HEALTH_BETA_CONTENT={content}");

    assert!(content.contains("auth_func"));
    assert!(result.get("problem_class").is_none(), "{result}");
}

#[test]
fn test_retrieve_context_routes_breakage_question_to_impact_and_packages_neighbors() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/api.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/consumer_a.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/consumer_b.rs', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_a', 'consumer_a', 'function', false, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_b', 'consumer_b', 'function', false, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/consumer_a.rs', 'axon::consumer_a', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/consumer_b.rs', 'axon::consumer_b', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::consumer_a', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::consumer_b', 'axon::parse_batch', 'AXO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "retrieve_context",
            "arguments": {
                "question": "What breaks if parse_batch changes?",
                "project": "AXO",
                "token_budget": 1200
            }
        })),
        id: Some(json!(6001)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.expect("Expected result");
    let data = result.get("data").expect("expected data payload");
    let route = data
        .get("planner")
        .and_then(|planner| planner.get("route"))
        .and_then(|value| value.as_str())
        .expect("expected planner route");
    assert_eq!(route, "impact");

    let packet = data.get("packet").expect("expected packet");
    let structural_neighbors = packet
        .get("structural_neighbors")
        .and_then(|value| value.as_array())
        .expect("expected structural neighbors");
    assert!(
        structural_neighbors
            .iter()
            .any(|row| row.to_string().contains("consumer_a")),
        "{:?}",
        structural_neighbors
    );
    assert!(
        packet
            .get("answer_sketch")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .contains("parse_batch"),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_joins_soll_when_question_is_about_rationale() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();

    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/payment.rs', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('api::checkout', 'checkout', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'api::checkout', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout', 'symbol', 'api::checkout', 'BKS', 'body', 'checkout calls stripe charge creation', 'hash-checkout', 1, 8)")
        .unwrap();

    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-BKS-005', 'Requirement', 'BKS', 'Stripe integration', 'Need Stripe support', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Rust SDK selected for payment integration', 'accepted', '{\"rationale\":\"Operational safety\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-BKS-010', 'REQ-BKS-005', 'SOLVES')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-001', 'Decision', 'DEC-BKS-010', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "retrieve_context",
            "arguments": {
                "question": "Why does checkout use the Stripe SDK?",
                "project": "BKS",
                "token_budget": 1200
            }
        })),
        id: Some(json!(6002)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.expect("Expected result");
    let data = result.get("data").expect("expected data payload");
    let route = data
        .get("planner")
        .and_then(|planner| planner.get("route"))
        .and_then(|value| value.as_str())
        .expect("expected planner route");
    assert_eq!(route, "soll_hybrid");

    let packet = data.get("packet").expect("expected packet");
    let soll_entities = packet
        .get("relevant_soll_entities")
        .and_then(|value| value.as_array())
        .expect("expected soll entities");
    assert!(
        soll_entities
            .iter()
            .any(|row| row.to_string().contains("DEC-BKS-010")),
        "{:?}",
        soll_entities
    );
    assert!(
        packet
            .get("why_these_items")
            .and_then(|value| value.as_array())
            .map(|items| !items.is_empty())
            .unwrap_or(false),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_returns_evidence_packet_and_budget_diagnostics_for_wiring_question() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/router.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::trigger_scan', 'trigger_scan', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::worker_loop', 'worker_loop', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/router.rs', 'axon::trigger_scan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/router.rs', 'axon::worker_loop', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::worker_loop', 'axon::trigger_scan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-trigger', 'symbol', 'axon::trigger_scan', 'AXO', 'body', 'trigger_scan queues a new scan and notifies the worker loop', 'hash-trigger', 1, 10)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "retrieve_context",
            "arguments": {
                "question": "Where is trigger_scan wired?",
                "project": "AXO",
                "token_budget": 900
            }
        })),
        id: Some(json!(6003)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.expect("Expected result");
    let data = result.get("data").expect("expected data payload");
    let route = data
        .get("planner")
        .and_then(|planner| planner.get("route"))
        .and_then(|value| value.as_str())
        .expect("expected planner route");
    assert_eq!(route, "wiring");

    let packet = data.get("packet").expect("expected packet");
    assert!(packet.get("answer_sketch").is_some(), "{packet:?}");
    assert!(packet.get("direct_evidence").is_some(), "{packet:?}");
    assert!(packet.get("supporting_chunks").is_some(), "{packet:?}");
    assert!(packet.get("structural_neighbors").is_some(), "{packet:?}");
    assert!(packet.get("confidence").is_some(), "{packet:?}");
    assert!(packet.get("excluded_because").is_some(), "{packet:?}");
    let timings = packet
        .get("retrieval_timings_ms")
        .and_then(|value| value.as_object())
        .expect("expected retrieval timings");
    for key in [
        "planner",
        "entry_lookup",
        "runtime_guard",
        "chunk_lookup",
        "chunk_selection",
        "graph_expansion",
        "soll_join",
        "packet_assembly",
        "total",
    ] {
        assert!(
            timings.get(key).and_then(|value| value.as_u64()).is_some(),
            "missing timing {key}: {packet:?}"
        );
    }
    assert!(
        packet
            .get("token_budget_estimate")
            .and_then(|value| value.get("estimated_tokens"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0)
            > 0,
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_uses_repo_literal_fallback_when_index_has_no_anchor() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Where is trigger_scan wired?",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(60031)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    assert!(
        packet["direct_evidence"]
            .to_string()
            .contains("trigger_scan"),
        "{packet:?}"
    );
    assert!(
        packet["supporting_chunks"]
            .to_string()
            .contains("repo_literal"),
        "{packet:?}"
    );
    assert!(
        packet["supporting_chunks"]
            .to_string()
            .contains("src/axon-core/src/parser/elixir.rs"),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    service_guard::reset_for_tests();
}

#[test]
fn test_retrieve_context_accepts_canonical_project_code_for_repo_code_symbols() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, graph_ready) VALUES ('/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs', 'AXO', 'indexed', TRUE)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::axon_retrieve_context', 'axon_retrieve_context', 'method', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs', 'axon::axon_retrieve_context', 'AXO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "retrieve_context",
            "arguments": {
                "question": "axon_retrieve_context",
                "project": "AXO",
                "token_budget": 900
            }
        })),
        id: Some(json!(6004)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.expect("Expected result");
    let packet = result
        .get("data")
        .and_then(|data| data.get("packet"))
        .expect("expected packet");
    let direct_evidence = packet
        .get("direct_evidence")
        .and_then(|value| value.as_array())
        .expect("expected direct evidence array");

    assert!(
        direct_evidence
            .iter()
            .any(|row| row.to_string().contains("axon_retrieve_context")),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_eval_harness_hits_route_and_grounded_evidence_thresholds() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, vector_ready, status) VALUES ('src/core/api.rs', 'AXO', TRUE, FALSE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, vector_ready, status) VALUES ('src/core/consumer.rs', 'AXO', TRUE, FALSE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, vector_ready, status) VALUES ('src/payment.rs', 'BKS', TRUE, FALSE, 'indexed')")
        .unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer', 'consumer', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/consumer.rs', 'axon::consumer', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::consumer', 'axon::parse_batch', 'AXO')")
        .unwrap();

    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-parse', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the writer batch and updates file lifecycle state', 'hash-parse', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout', 'symbol', 'bks::checkout', 'BKS', 'body', 'checkout creates a Stripe charge through the Rust SDK', 'hash-checkout', 1, 12)")
        .unwrap();

    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Operational payment choice', 'accepted', '{\"rationale\":\"Operational safety\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-EVAL-1', 'Decision', 'DEC-BKS-010', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    let cases = vec![
        (
            "What breaks if parse_batch changes?",
            "AXO",
            "impact",
            "consumer",
            false,
        ),
        (
            "Where is parse_batch wired?",
            "AXO",
            "wiring",
            "parse_batch",
            false,
        ),
        (
            "Why does checkout use the Stripe SDK?",
            "BKS",
            "soll_hybrid",
            "checkout",
            true,
        ),
    ];

    let mut route_hits = 0usize;
    let mut grounded_hits = 0usize;
    let mut soll_hits = 0usize;

    for (question, project, expected_route, expected_symbol, expects_soll) in cases {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": question,
                    "project": project,
                    "token_budget": 1200
                }
            })),
            id: Some(json!(6100)),
        };

        let response = server.handle_request(req).unwrap();
        let result = response.result.expect("Expected result");
        let data = result.get("data").expect("expected data payload");
        let route = data["planner"]["route"].as_str().unwrap_or_default();
        let packet = &data["packet"];

        if route == expected_route {
            route_hits += 1;
        }
        if packet["answer_sketch"]
            .as_str()
            .unwrap_or_default()
            .contains(expected_symbol)
            || packet["direct_evidence"]
                .to_string()
                .contains(expected_symbol)
            || packet["supporting_chunks"]
                .to_string()
                .contains(expected_symbol)
        {
            grounded_hits += 1;
        }
        if expects_soll
            && packet["relevant_soll_entities"]
                .to_string()
                .contains("DEC-BKS-010")
        {
            soll_hits += 1;
        }
    }

    let route_hit_rate = route_hits as f64 / 3.0;
    let grounded_hit_rate = grounded_hits as f64 / 3.0;
    assert!(
        route_hit_rate >= 1.0,
        "route_hit_rate={route_hit_rate}, route_hits={route_hits}"
    );
    assert!(
        grounded_hit_rate >= 1.0,
        "grounded_hit_rate={grounded_hit_rate}, grounded_hits={grounded_hits}"
    );
    assert_eq!(soll_hits, 1, "expected exactly one SOLL-rationale hit");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_uses_file_anchor_for_path_like_question() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/axon-core/src/mcp/tools_context.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "src/axon-core/src/mcp/tools_context.rs",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6201)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let data = result.get("data").expect("expected data payload");
    assert_eq!(
        data["planner"]["route"].as_str().unwrap_or_default(),
        "exact_lookup"
    );
    let packet = data.get("packet").expect("expected packet");
    let direct_evidence = packet["direct_evidence"]
        .as_array()
        .expect("expected direct evidence");
    assert!(
        direct_evidence.iter().any(|row| {
            row.get("evidence_class").and_then(|value| value.as_str()) == Some("canonical_file")
                && row.to_string().contains("tools_context.rs")
        }),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_prefers_anchored_chunks_over_generic_semantic_noise() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/core/api.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/docs/noise.md', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::noise_symbol', 'noise_symbol', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/docs/noise.md', 'axon::noise_symbol', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-anchor', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the writer batch and updates file lifecycle state', 'hash-anchor', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-noise', 'symbol', 'axon::noise_symbol', 'AXO', 'body', 'parse_batch appears in a broad semantic discussion without direct implementation detail', 'hash-noise', 1, 12)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "parse_batch",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6202)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = result["data"]["packet"].clone();
    let chunks = packet["supporting_chunks"]
        .as_array()
        .expect("expected supporting chunks");
    assert!(!chunks.is_empty(), "{packet:?}");
    let first = &chunks[0];
    assert_eq!(
        first["chunk_id"].as_str().unwrap_or_default(),
        "chunk-anchor",
        "{packet:?}"
    );
    assert_eq!(
        first["anchored_to_entry"].as_bool(),
        Some(true),
        "{packet:?}"
    );
    assert!(
        result["data"]["packet"]["retrieval_diagnostics"]["anchored_chunks_selected"]
            .as_u64()
            .unwrap_or(0)
            >= 1,
        "{packet:?}"
    );
    assert!(
        result["data"]["packet"]["retrieval_diagnostics"]["chunk_candidates_considered"]
            .as_u64()
            .unwrap_or(u64::MAX)
            <= 4,
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_skips_semantic_search_under_critical_pressure_and_reports_partial_truth() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    service_guard::reset_for_tests();
    service_guard::record_latency(ServiceKind::Mcp, 1_700);

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/core/api.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-anchor', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the writer batch and updates file lifecycle state', 'hash-anchor-critical', 1, 12)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "parse_batch",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6203)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let planner = &result["data"]["planner"];
    let packet = &result["data"]["packet"];
    assert_eq!(
        planner["semantic_search_used"].as_bool(),
        Some(false),
        "{planner:?}"
    );
    assert!(
        planner["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("pressure_critical"),
        "{planner:?}"
    );
    assert!(
        packet["missing_evidence"]
            .to_string()
            .contains("Semantic chunk search was skipped or unavailable"),
        "{packet:?}"
    );
    assert!(
        packet["direct_evidence"]
            .to_string()
            .contains("parse_batch"),
        "{packet:?}"
    );

    service_guard::reset_for_tests();
    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_prefers_same_file_impl_chunk_over_docs_chunk() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/core/router.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('docs/router.md', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::trigger_scan', 'trigger_scan', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::router_docs', 'router_docs', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/router.rs', 'axon::trigger_scan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('docs/router.md', 'axon::router_docs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-router-impl', 'symbol', 'axon::trigger_scan', 'AXO', 'body', 'trigger_scan queues the router worker and commits the scan request', 'hash-router-impl', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-router-doc', 'symbol', 'axon::router_docs', 'AXO', 'body', 'trigger_scan is mentioned in the router overview documentation', 'hash-router-doc', 1, 12)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Where is trigger_scan wired?",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6204)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    let chunks = packet["supporting_chunks"]
        .as_array()
        .expect("expected supporting chunks");
    assert!(!chunks.is_empty(), "{packet:?}");
    assert_eq!(
        chunks[0]["chunk_id"].as_str().unwrap_or_default(),
        "chunk-router-impl",
        "{packet:?}"
    );
    assert!(
        packet["excluded_because"]
            .to_string()
            .contains("docs_file_penalty")
            || packet["excluded_because"]
                .to_string()
                .contains("same_file_preferred")
            || packet["retrieval_diagnostics"]["chunk_candidates_considered"]
                .as_u64()
                .unwrap_or_default()
                == 1,
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_caps_broad_semantic_fallbacks_to_one() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/core/api.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-anchor-main', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the batch writer', 'hash-anchor-main', 1, 12)")
        .unwrap();
    for idx in 0..3 {
        server.graph_store.execute(&format!(
            "INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/noise/semantic_{idx}.rs', 'AXO', TRUE, 'indexed')"
        )).unwrap();
        server.graph_store.execute(&format!(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::semantic_noise_{idx}', 'semantic_noise_{idx}', 'function', true, true, false, 'AXO')"
        )).unwrap();
        server.graph_store.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/noise/semantic_{idx}.rs', 'axon::semantic_noise_{idx}', 'AXO')"
        )).unwrap();
        server.graph_store.execute(&format!(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-noise-{idx}', 'symbol', 'axon::semantic_noise_{idx}', 'AXO', 'body', 'parse_batch appears in broad semantic background number {idx}', 'hash-noise-{idx}', 1, 12)"
        )).unwrap();
    }

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "parse_batch",
                    "project": "AXO",
                    "token_budget": 1100
                }
            })),
            id: Some(json!(6205)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    let chunks = packet["supporting_chunks"]
        .as_array()
        .expect("expected supporting chunks");
    let broad_count = chunks
        .iter()
        .filter(|chunk| {
            chunk["anchored_to_entry"].as_bool() == Some(false)
                && chunk["same_file_as_entry"].as_bool() == Some(false)
        })
        .count();
    assert!(broad_count <= 1, "{packet:?}");
    if broad_count == 1 {
        assert!(
            packet["excluded_because"]
                .to_string()
                .contains("broader_semantic_dropped_due_to_anchor"),
            "{packet:?}"
        );
    }

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_prefers_direct_file_traceability_for_rationale() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/payment.rs', 'BKS', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')")
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-FILE', 'Decision', 'BKS', 'Payment file rationale', 'File-level rationale', 'accepted', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-BKS-FILE', 'Requirement', 'BKS', 'File-level requirement', 'Requirement tied to file', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-BKS-FILE', 'REQ-BKS-FILE', 'SOLVES')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-FILE-001', 'Decision', 'DEC-BKS-FILE', 'File', 'src/payment.rs', 1.0, 0)").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Why is src/payment.rs designed this way?",
                    "project": "BKS",
                    "token_budget": 1000
                }
            })),
            id: Some(json!(6206)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    let soll_entities = packet["relevant_soll_entities"]
        .as_array()
        .expect("expected soll entities");
    assert!(
        soll_entities
            .iter()
            .any(|row| row.to_string().contains("DEC-BKS-FILE")),
        "{packet:?}"
    );
    assert!(
        soll_entities[0]
            .to_string()
            .contains("direct_file_traceability"),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_under_critical_pressure_avoids_unanchored_fallback_chunks() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    service_guard::reset_for_tests();
    service_guard::record_latency(ServiceKind::Mcp, 1_700);

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('scripts/noise.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::semantic_noise_parse', 'semantic_noise_parse', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('scripts/noise.rs', 'axon::semantic_noise_parse', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-no-anchor-impact', 'symbol', 'axon::semantic_noise_parse', 'AXO', 'body', 'parse_batch appears in generic benchmark noise and fallback prose', 'hash-no-anchor-impact', 1, 12)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "What breaks if parse_batch changes?",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6210)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let planner = &result["data"]["planner"];
    let packet = &result["data"]["packet"];
    let chunks = packet["supporting_chunks"]
        .as_array()
        .expect("expected supporting chunks");
    assert_eq!(planner["route"].as_str(), Some("impact"), "{planner:?}");
    assert!(
        planner["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("pressure_critical"),
        "{planner:?}"
    );
    assert!(
        chunks.is_empty(),
        "critical pressure should prefer no support over unanchored fallback noise: {packet:?}"
    );
    assert!(
        packet["missing_evidence"].to_string().contains(
            "An anchor was found but no anchored chunk-level grounding evidence was retained"
        ),
        "{packet:?}"
    );
    assert!(
        packet["direct_evidence"]
            .to_string()
            .contains("\"project_code\":\"AXO\""),
        "{packet:?}"
    );

    service_guard::reset_for_tests();
    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_under_critical_pressure_skips_graph_and_soll_even_with_anchor() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    service_guard::reset_for_tests();
    service_guard::record_latency(ServiceKind::Mcp, 1_700);

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/core/api.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-anchor-rationale', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the writer batch and updates lifecycle state', 'hash-anchor-rationale', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::parse_batch', 'axon::write_revision', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::write_revision', 'write_revision', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-CRIT', 'Decision', 'AXO', 'Batch rationale', 'Why parse_batch exists', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-CRIT', 'Decision', 'DEC-AXO-CRIT', 'Symbol', 'axon::parse_batch', 1.0, 0)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Why does parse_batch exist?",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6209)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    assert_eq!(
        packet["structural_neighbors"]
            .as_array()
            .map(|rows| rows.len()),
        Some(0),
        "{packet:?}"
    );
    assert_eq!(
        packet["relevant_soll_entities"]
            .as_array()
            .map(|rows| rows.len()),
        Some(0),
        "{packet:?}"
    );
    assert!(
        packet["excluded_because"]
            .to_string()
            .contains("graph_expansion_skipped_due_to_pressure_guarded"),
        "{packet:?}"
    );
    assert!(
        packet["excluded_because"]
            .to_string()
            .contains("soll_join_skipped_due_to_pressure_guarded"),
        "{packet:?}"
    );

    service_guard::reset_for_tests();
    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_falls_back_to_repo_local_global_symbol_when_project_code_misses() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('/home/dstadel/projects/axon/src/runtime/router.rs', 'AXO', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::Axon.Watcher.Server.trigger_scan', 'Axon.Watcher.Server.trigger_scan', 'function', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/home/dstadel/projects/axon/src/runtime/router.rs', 'axon::Axon.Watcher.Server.trigger_scan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-axon-trigger', 'symbol', 'axon::Axon.Watcher.Server.trigger_scan', 'AXO', 'body', 'trigger_scan queues a new scan and notifies the worker loop', 'hash-global-trigger', 1, 12)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Where is trigger_scan wired?",
                    "project": "AXO",
                    "token_budget": 900
                }
            })),
            id: Some(json!(6210)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    assert!(
        packet["direct_evidence"]
            .to_string()
            .contains("trigger_scan"),
        "{packet:?}"
    );
    assert!(
        packet["supporting_chunks"]
            .to_string()
            .contains("chunk-axon-trigger"),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    service_guard::reset_for_tests();
}

#[test]
fn test_retrieve_context_reports_precise_missing_rationale_evidence() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/payment.rs', 'BKS', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Why does checkout exist?",
                    "project": "BKS",
                    "token_budget": 1000
                }
            })),
            id: Some(json!(6207)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    assert!(
        packet["missing_evidence"]
            .to_string()
            .contains("anchor_found_but_no_traceability"),
        "{packet:?}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_impact_accepts_canonical_project_code_for_repo_code_symbols() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO File (path, project_code, status, graph_ready) VALUES ('/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs', 'AXO', 'indexed', TRUE)").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::axon_retrieve_context', 'axon_retrieve_context', 'method', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::caller', 'caller', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_context.rs', 'axon::axon_retrieve_context', 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::caller', 'axon::axon_retrieve_context', 'AXO')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "impact",
                "arguments": { "symbol": "axon_retrieve_context", "project": "AXO", "depth": 2 }
            })),
            id: Some(json!(6208)),
        })
        .unwrap();
    let result = response.result.expect("Expected result");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("caller"), "{text}");
    assert!(
        !text.contains("symbol not found in current scope"),
        "{text}"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_inspect_accepts_canonical_project_code_for_repo_code_symbols() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::axon_retrieve_context', 'axon_retrieve_context', 'method', true, true, false, 'AXO')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "inspect",
                "arguments": { "symbol": "axon_retrieve_context", "project": "AXO" }
            })),
            id: Some(json!(6209)),
        })
        .unwrap();
    let result = response.result.expect("Expected result");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("axon_retrieve_context"), "{text}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_fs_read() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "fs_read",
            "arguments": { "uri": "src/axon-core/src/main.rs", "start_line": 1, "end_line": 5 }
        })),
        id: Some(json!(4)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("L2 Detail") || content.contains("Erreur"));
}

#[test]
fn test_send_notification() {
    let store = Arc::new(
        GraphStore::new(":memory:")
            .unwrap_or_else(|_| GraphStore::new("/tmp/test_db_notif").unwrap()),
    );
    let server = McpServer::new(store);
    let notif = server.send_notification("notifications/tools/list_changed", None);
    assert_eq!(notif.method, "notifications/tools/list_changed");
    assert!(notif.params.is_none());

    let serialized = serde_json::to_string(&notif).unwrap();
    assert!(serialized.contains("notifications/tools/list_changed"));
}

#[test]
fn test_axon_inspect() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::core_func', 'core_func', 'function', true, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::caller_func', 'caller_func', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::caller_func', 'prj::core_func', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": {
                "symbol": "core_func",
                "project": "PRJ"
            }
        })),
        id: Some(json!(5)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Inspection du Symbole"), "{content}");
    assert!(content.contains("core_func"), "{content}");
}

#[test]
fn test_graph_embedding_semantic_clones_adds_derived_neighborhood_matches() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
    }
    let server = create_test_server();
    let graph_model_id = current_graph_model_id();
    let anchor_embedding = graph_embedding_sql(&[1.0]);
    let peer_embedding = graph_embedding_sql(&[0.99, 0.01]);
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/auth.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/access.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/auth.rs', 'prj::authorize_request', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/access.rs', 'prj::check_token_chain', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, 'sig-auth', '1', 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, 'sig-access', '1', 1001)").unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, '{}', 'sig-auth', '1', {}, 1000)", graph_model_id, anchor_embedding)).unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, '{}', 'sig-access', '1', {}, 1001)", graph_model_id, peer_embedding)).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "semantic_clones",
            "arguments": { "symbol": "authorize_request" }
        })),
        id: Some(json!(77)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    unsafe {
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    assert!(content.contains("check_token_chain"));
    assert!(content.contains("contexte derive du graphe"), "{content}");
}

#[test]
fn test_graph_embedding_semantic_clones_ignores_stale_projection_signatures() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "true");
    }
    let server = create_test_server();
    let graph_model_id = current_graph_model_id();
    let anchor_embedding = graph_embedding_sql(&[1.0]);
    let stale_embedding = graph_embedding_sql(&[0.99, 0.01]);
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/auth.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/access.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/auth.rs', 'prj::authorize_request', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/access.rs', 'prj::check_token_chain', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, 'sig-auth', '1', 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, 'sig-access-current', '1', 1001)").unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, '{}', 'sig-auth', '1', {}, 1000)", graph_model_id, anchor_embedding)).unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, '{}', 'sig-access-stale', '1', {}, 1001)", graph_model_id, stale_embedding)).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "semantic_clones",
            "arguments": { "symbol": "authorize_request" }
        })),
        id: Some(json!(78)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    unsafe {
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    assert!(!content.contains("contexte derive du graphe"));
    assert!(!content.contains("check_token_chain"));
}

#[test]
fn test_graph_embedding_semantic_clones_reports_explicit_fallback_when_disabled() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_GRAPH_EMBEDDINGS_ENABLED", "false");
    }
    let server = create_test_server();
    let graph_model_id = current_graph_model_id();
    let anchor_embedding = graph_embedding_sql(&[1.0]);
    let peer_embedding = graph_embedding_sql(&[0.99, 0.01]);
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/auth.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/access.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/auth.rs', 'prj::authorize_request', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/access.rs', 'prj::check_token_chain', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, 'sig-auth', '1', 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, 'sig-access', '1', 1001)").unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::authorize_request', 1, '{}', 'sig-auth', '1', {}, 1000)", graph_model_id, anchor_embedding)).unwrap();
    server.graph_store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'prj::check_token_chain', 1, '{}', 'sig-access', '1', {}, 1001)", graph_model_id, peer_embedding)).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "semantic_clones",
            "arguments": { "symbol": "authorize_request" }
        })),
        id: Some(json!(79)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    unsafe {
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_GRAPH_EMBEDDINGS_ENABLED");
    }

    assert!(!content.contains("derive optionnel du graphe"));
    assert!(content.contains("temporairement desactive"), "{content}");
}

#[test]
fn test_axon_audit_taint_analysis() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/api.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/api_dummy.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::user_input', 'user_input', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::run_task', 'run_task', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::eval', 'eval', 'function', false, true, false, true, 'PRJ')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/api.rs', 'prj::user_input', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::user_input', 'prj::run_task', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::run_task', 'prj::eval', 'PRJ')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(6)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("user_input"));
    assert!(content.contains("user_input"));
    assert!(content.contains("eval"));
}

#[test]
fn test_axon_audit_technical_debt() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/danger.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::risky_func', 'risky_func', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::unwrap', 'unwrap', 'method', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/danger.rs', 'prj::risky_func', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::risky_func', 'prj::unwrap', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(10)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Dette Technique"));
    assert!(content.contains("unwrap"));
    assert!(content.contains("src/danger.rs"));
}

#[test]
fn test_axon_audit_technical_debt_comments() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/todo.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::todo1', '// TODO: Fix this', 'TODO', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/todo.rs', 'prj::todo1', 'PRJ')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(11)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Dette Technique"));
    assert!(content.contains("TODO"));
    assert!(content.contains("Fix this"));
}

#[test]
fn test_axon_audit_secrets_detection() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/config.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/config.rs', 'prj::secret1', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(12)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Dette Technique"));
    assert!(content.contains("SECRET_API_KEY"));
    assert!(content.contains("hardcoded credential"));
}

#[test]
fn test_axon_audit_cross_language_taint() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/api.ex', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/api_dummy.ex', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::elixir_func', 'elixir_func', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::rust_nif', 'rust_nif', 'function', false, true, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::unsafe_block', 'unsafe_block', 'function', false, true, false, true, 'PRJ')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/api.ex', 'prj::elixir_func', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS_NIF (source_id, target_id, project_code) VALUES ('prj::elixir_func', 'prj::rust_nif', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::rust_nif', 'prj::unsafe_block', 'PRJ')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(13)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(!content.contains("Score 100/100"));
    assert!(content.contains("elixir_func"));
    assert!(content.contains("unsafe_block"));
}

#[test]
fn test_axon_health_god_objects() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/god.rs', 'PRJ')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/god_dummy.rs', 'PRJ')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::GodClass', 'GodClass', 'class', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/god.rs', 'prj::GodClass', 'PRJ')",
        )
        .unwrap();

    for i in 0..20 {
        server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::dep{}', 'dep{}', 'function', false, true, false, 'PRJ')", i, i)).unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('prj::dep{}', 'prj::GodClass', 'PRJ')", i))
            .unwrap();
    }

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(7)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("God Objects detected") || content.contains("GodClass"));
}

#[test]
fn test_axon_audit_respects_project_scope() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('apps/pja/lib/input.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('apps/pjb/lib/unsafe.rs', 'PJB')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJA::safe_entry', 'safe_entry', 'function', true, true, false, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJB::beta_entry', 'beta_entry', 'function', false, true, false, false, 'PJB')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJB::eval', 'eval', 'function', false, true, false, true, 'PJB')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('apps/pja/lib/input.rs', 'PJA::safe_entry', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('apps/pjb/lib/unsafe.rs', 'PJB::beta_entry', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJB::beta_entry', 'PJB::eval', 'PJB')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PJA"
            }
        })),
        id: Some(json!(14)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Sécurité : 100/100"), "{}", content);
    assert!(!content.contains("beta_entry"), "{}", content);
    assert!(!content.contains("eval"), "{}", content);
}

#[test]
fn test_axon_health_respects_project_scope() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('apps/pja/lib/covered.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('apps/pjb/lib/god.rs', 'PJB')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::covered', 'covered', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::GodClass', 'GodClass', 'class', false, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('apps/pja/lib/covered.rs', 'PJA::covered', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('apps/pjb/lib/god.rs', 'PJB::GodClass', 'PJB')")
        .unwrap();

    for i in 0..6 {
        server
            .graph_store
            .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::dep{}', 'dep{}', 'function', false, true, false, 'PJB')", i, i))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJB::dep{}', 'PJB::GodClass', 'PJB')",
                i
            ))
            .unwrap();
    }

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            "arguments": {
                "project": "PJA"
            }
        })),
        id: Some(json!(15)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Coverage 100%"), "{}", content);
    assert!(!content.contains("God Object"), "{}", content);
    assert!(!content.contains("GodClass"), "{}", content);
}

#[test]
fn test_axon_audit_uses_project_code_even_when_path_does_not_contain_project_name() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/shared/api.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/shared/safe.rs', 'PJB')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJA::entrypoint', 'entrypoint', 'function', false, true, false, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJA::eval', 'eval', 'function', false, true, false, true, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJB::safe_fn', 'safe_fn', 'function', true, true, false, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/shared/api.rs', 'PJA::entrypoint', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/shared/api.rs', 'PJA::eval', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/shared/safe.rs', 'PJB::safe_fn', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJA::entrypoint', 'PJA::eval', 'PJA')",
        )
        .unwrap();
    assert_eq!(
        server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_code = $proj OR path LIKE '%' || $proj || '%'",
                &json!({"proj": "PJA"})
            )
            .unwrap(),
        1
    );

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PJA"
            }
        })),
        id: Some(json!(94)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Audit de Conformité : PJA"));
    assert!(content.contains("eval"));
    assert!(!content.contains("seems unindexed"));
}

#[test]
fn test_axon_health_uses_project_code_even_when_path_does_not_contain_project_name() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/shared/pja_core.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/shared/pjb_core.rs', 'PJB')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::GodClass', 'GodClass', 'class', false, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::stable_api', 'stable_api', 'function', true, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/shared/pja_core.rs', 'PJA::GodClass', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/shared/pjb_core.rs', 'PJB::stable_api', 'PJB')")
        .unwrap();

    for i in 0..5 {
        server
            .graph_store
            .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::dep{}', 'dep{}', 'function', false, true, false, 'PJA')", i, i))
            .unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJA::dep{}', 'PJA::GodClass', 'PJA')", i))
            .unwrap();
    }
    assert_eq!(
        server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_code = $proj OR path LIKE '%' || $proj || '%'",
                &json!({"proj": "PJB"})
            )
            .unwrap(),
        1
    );

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            "arguments": {
                "project": "PJB"
            }
        })),
        id: Some(json!(95)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Health Report: PJB"));
    assert!(content.contains("Coverage 100%"));
    assert!(!content.contains("GodClass"));
    assert!(!content.contains("seems unindexed"));
}

#[test]
fn test_axon_query_global_default() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "auth" }
        })),
        id: Some(json!(8)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Resultats de recherche"));
    assert!(content.contains("Mode:"));
}

#[test]
fn test_axon_soll_manager_auto_id() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 10, 0)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": "AXO",
                    "name": "Test Concept",
                    "explanation": "To test auto id",
                    "rationale": "Because testing is good"
                }
            }
        })),
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("CPT-AXO-011"));

    let count = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.Node WHERE type='Concept' AND id = 'CPT-AXO-011'")
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_axon_soll_manager_accepts_mcp_axon_prefixed_name() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 11, 0)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "mcp_axon_soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": "AXO",
                    "name": "Prefixed concept",
                    "explanation": "Should work through legacy prefixed tool names",
                    "rationale": "Client compatibility"
                }
            }
        })),
        id: Some(json!(10001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("CPT-AXO-012"), "{content}");
}

#[test]
fn test_axon_soll_manager_rejects_legacy_project_without_canonical_meta() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "BookingSystem",
                    "title": "Canonical Booking Decision",
                    "context": "Project code must be server-managed",
                    "rationale": "Slug longs are not canonical",
                    "status": "accepted"
                }
            }
        })),
        id: Some(json!(1001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("meta.json"), "{content}");
    assert!(content.contains("BookingSystem"), "{content}");
}

#[test]
fn test_axon_soll_apply_plan_commit_finds_persisted_preview() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": false,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-preview-commit",
                        "title": "Preview Commit Requirement",
                        "description": "Commit should read back the persisted preview",
                        "priority": "P1",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(10002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("SOLL revision committed"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Requirement' AND title = 'Preview Commit Requirement'")
            .unwrap(),
        1
    );
    let revision_rows = server
        .graph_store
        .query_json("SELECT revision_id FROM soll.Revision ORDER BY created_at DESC LIMIT 1")
        .unwrap();
    assert!(revision_rows.contains("REV-AXO-001"), "{revision_rows}");
}

#[test]
fn test_axon_soll_apply_plan_dry_run_uses_canonical_preview_id() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-preview-id",
                        "title": "Preview Id Requirement",
                        "description": "Preview ids should be canonical",
                        "priority": "P1",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(10003)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let preview_id = result["data"]["preview_id"].as_str().unwrap();
    assert_eq!(preview_id, "PRV-AXO-001");
}

#[test]
fn test_axon_soll_apply_plan_scopes_duplicates_to_same_project() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-BKS-001', 'Requirement', 'BKS', 'Shared title', 'Other project duplicate', 'draft', '{\"logical_key\":\"shared-key\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "shared-key",
                        "title": "Shared title",
                        "description": "Should still create in AXO scope"
                    }]
                }
            }
        })),
        id: Some(json!(100031)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let operations = result["data"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["kind"].as_str(), Some("create"));
}

#[test]
fn test_axon_soll_manager_create_requires_explicit_canonical_project_code() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "title": "Missing project code",
                    "context": "Mutations must declare an explicit project scope",
                    "rationale": "The server must not guess the target project",
                    "status": "accepted"
                }
            }
        })),
        id: Some(json!(1002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("`project_code` est obligatoire"),
        "{content}"
    );
    assert!(content.contains("AXO"), "{content}");
}

#[test]
fn test_axon_soll_apply_plan_rejects_non_canonical_project_identifier() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "BookingSystem",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-non-canonical-project",
                        "title": "Bad project identity",
                        "description": "Mutations must reject non canonical project identifiers"
                    }]
                }
            }
        })),
        id: Some(json!(10004)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Identifiant projet non canonique"),
        "{content}"
    );
    assert!(content.contains("BookingSystem"), "{content}");
    assert!(content.contains("3 caractères"), "{content}");
}

#[test]
fn test_axon_init_project_rejects_non_canonical_project_code() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_name": "BookingSystem",
                "project_code": "booking-system",
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        },
        "id": 10005
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Identifiant projet non canonique"),
        "{content}"
    );
    assert!(content.contains("booking-system"), "{content}");
}

#[test]
fn test_axon_apply_guidelines_rejects_non_canonical_project_code() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "axon",
                "accepted_global_rule_ids": ["GUI-PRO-001"]
            }
        },
        "id": 10006
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Identifiant projet non canonique"),
        "{content}"
    );
    assert!(content.contains("axon"), "{content}");
}

#[test]
fn test_axon_soll_manager_pillar_uses_dedicated_counter() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 3, 12, 0, 0)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_code": "AXO",
                    "title": "Dedicated Pillar Counter",
                    "description": "Pillars must not consume requirement ids"
                }
            }
        })),
        id: Some(json!(102)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("PIL-AXO-004"), "{content}");
}

#[test]
fn test_axon_soll_manager_recovers_when_registry_lags_existing_entities() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-007', 'Requirement', 'AXO', 'Existing', 'Already there', '', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "Recovered Counter",
                    "description": "Should continue after observed max",
                    "priority": "P1"
                }
            }
        })),
        id: Some(json!(103)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("REQ-AXO-008"), "{content}");
}

#[test]
fn test_axon_soll_manager_can_create_and_update_vision() {
    let server = create_test_server();

    let create_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "vision",
                "data": {
                    "project_code": "AXO",
                    "title": "Axon Vision",
                    "description": "Deterministic ingestion",
                    "goal": "Structural truth first",
                    "metadata": {"owner": "platform"}
                }
            }
        })),
        id: Some(json!(104)),
    };

    let create_response = server.handle_request(create_req);
    let create_result = create_response.unwrap().result.unwrap();
    let create_content = create_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(create_content.contains("VIS-AXO-001"), "{create_content}");

    let update_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "update",
                "entity": "vision",
                "data": {
                    "id": "VIS-AXO-001",
                    "goal": "Graph before vectors",
                    "metadata": {"owner": "runtime"}
                }
            }
        })),
        id: Some(json!(105)),
    };

    let update_response = server.handle_request(update_req);
    let update_result = update_response.unwrap().result.unwrap();
    let update_content = update_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        update_content.contains("Mise à jour réussie"),
        "{update_content}"
    );

    let vision_json = server
        .graph_store
        .query_json(
            "SELECT title, description, metadata FROM soll.Node WHERE type='Vision' AND id = 'VIS-AXO-001'",
        )
        .unwrap();
    assert!(vision_json.contains("Axon Vision"), "{vision_json}");
    assert!(
        vision_json.contains("Graph before vectors"),
        "{vision_json}"
    );
    assert!(vision_json.contains("runtime"), "{vision_json}");
}

#[test]
fn test_axon_soll_manager_creates_stakeholder_on_file_backed_store() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store.clone());

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "stakeholder",
                "data": {
                    "project_code": "AXO",
                    "name": "Runtime Rust",
                    "role": "Owns ingestion and canonical persistence"
                }
            }
        })),
        id: Some(json!(101)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("STK-AXO-001"), "{content}");

    std::thread::sleep(std::time::Duration::from_millis(75));

    let count = store
        .query_count("SELECT count(*) FROM soll.Node WHERE type='Stakeholder' AND id = 'STK-AXO-001' AND title = 'Runtime Rust'")
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_axon_export_soll() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Test Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'My Concept', 'Expl', '', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG EXPORT CONTENT: {}", content);

    assert!(content.contains("docs/vision/SOLL_EXPORT_"));

    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let export_content = std::fs::read_to_string(&export_path).unwrap();
    assert!(export_content.contains("# SOLL Extraction"));
    assert!(export_content.contains("Test Vision"));
    assert!(export_content.contains("CPT-AXO-001"));

    let export_body = std::fs::read_to_string(&export_path).expect("export file should exist");
    assert!(export_body.contains("## Entités : Vision"));

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_export_soll_resolves_repo_root_docs_vision() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(401)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Exported to"), "{content}");
    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let expected_dir =
        super::soll::canonical_soll_export_dir().expect("expected canonical export dir");
    let export_parent = Path::new(&export_path)
        .parent()
        .expect("expected export parent");

    assert_eq!(export_parent, expected_dir.as_path());
    assert!(!export_path.contains("src/axon-core/docs/vision/SOLL_EXPORT_"));

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_restore_soll() {
    let server = create_test_server();
    let export_path = "/tmp/axon_restore_soll_test.md";
    let markdown = r#"# SOLL Extraction

## Entités : Vision
### VIS-AXO-001 - Test Vision
**Description:** Desc
**Status:** draft
**Meta:** `{"goal": "Goal", "source":"test"}`

## Entités : Pillar
### PIL-AXO-001 - Platform Core
**Description:** Keep the conceptual core stable
**Status:** accepted
**Meta:** `{}`

## Entités : Concept
### CPT-AXO-001 - Graph Truth
**Description:** Use a structural graph as source of truth
**Status:** accepted
**Meta:** `{"rationale": "Because the project needs stable intent"}`

## Entités : Milestone
### MIL-AXO-001 - First Usable State
**Description:** 
**Status:** in_progress
**Meta:** `{}`

## Entités : Requirement
### REQ-AXO-001 - Reliable Restore
**Description:** SOLL must be restorable from exports
**Status:** draft
**Meta:** `{"priority":"high"}`

## Entités : Decision
### DEC-AXO-001 - Merge Restore
**Description:** 
**Status:** accepted
**Meta:** `{"rationale": "Restoration should be merge-oriented and non-destructive"}`

## Entités : Validation
### VAL-AXO-001 - manual-test
**Description:** 
**Status:** passed
**Meta:** `{"method": "manual-test", "timestamp": 1234567890}`
"#;
    std::fs::write(export_path, markdown).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(3)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("Restauration SOLL terminee"),
        "{}",
        content
    );
    assert!(content.contains("Vision: 1"));
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Vision'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Pillar'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Concept'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Milestone'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Requirement'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Decision'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Validation'")
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_validate_soll_reports_orphan_invariants() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Orphan requirement', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', '', '', 'pending', '{\"method\":\"manual\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Unlinked decision', 'No SOLVES or IMPACTS edges', 'accepted', '{}')")
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-BKS-001', 'Concept', 'BKS', 'BKS Concept', 'Expl', '', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": {}
        })),
        id: Some(json!(31)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("3 violation"));
    assert!(content.contains("REQ-AXO-001"));
    assert!(content.contains("VAL-AXO-001"));
    assert!(content.contains("DEC-AXO-001"));
}

#[test]
fn test_axon_validate_soll_reports_duplicate_titles_and_uncovered_requirements() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-010', 'Requirement', 'AXO', 'Duplicate req', 'No criteria', 'draft', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-011', 'Requirement', 'AXO', 'Duplicate req', 'Still no criteria', 'draft', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-010', 'Decision', 'AXO', 'Duplicate dec', 'No links', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-011', 'Decision', 'AXO', 'Duplicate dec', 'No links', 'accepted', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "AXO" }
        })),
        id: Some(json!(3204)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Titres dupliqués"), "{content}");
    assert!(content.contains("Duplicate req"), "{content}");
    assert!(content.contains("Duplicate dec"), "{content}");
    assert!(
        content.contains("Requirements sans critères/preuves"),
        "{content}"
    );
    assert!(content.contains("REQ-AXO-010"), "{content}");
    assert!(content.contains("REQ-AXO-011"), "{content}");
}

#[test]
fn test_axon_validate_soll_reports_clean_minimal_graph() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Core', 'Protect SOLL', '', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Linked requirement', 'Has links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', '', '', 'passed', '{\"method\":\"manual\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Linked decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'PIL-AXO-001', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('VAL-AXO-001', 'REQ-AXO-001', 'VERIFIES')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": {}
        })),
        id: Some(json!(32)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("0 violation"));
    assert!(content.contains("cohérence minimale"));
}

#[test]
fn test_axon_validate_soll_can_scope_by_project_code() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO orphan', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-BKS-001', 'Requirement', 'BKS', 'BKS orphan', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "AXO" }
        })),
        id: Some(json!(3201)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("project:AXO"), "{content}");
    assert!(content.contains("REQ-AXO-001"), "{content}");
    assert!(!content.contains("REQ-BKS-001"), "{content}");
}

#[test]
fn test_axon_validate_soll_rejects_non_canonical_project_alias() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "FSC" }
        })),
        id: Some(json!(3203)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("Projet canonique"), "{content}");
    assert!(content.contains("FSC"), "{content}");
}

#[test]
fn test_axon_validate_soll_reports_invalid_and_dangling_relations() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Core', 'Protect SOLL', '', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Linked requirement', 'Has links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', '', '', 'passed', '{\"method\":\"manual\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('VAL-AXO-001', 'PIL-AXO-001', 'VERIFIES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-404', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "AXO" }
        })),
        id: Some(json!(3204)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Relations invalides"), "{content}");
    assert!(content.contains("VERIFIES"), "{content}");
    assert!(content.contains("DEC-AXO-404"), "{content}");
}

#[test]
fn test_axon_export_soll_can_scope_by_project_code() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry(
            "BKS",
            Some("BookingSystem"),
            Some("/home/dstadel/projects/BookingSystem"),
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'AXO Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-BKS-001', 'Vision', 'BKS', 'BKS Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'AXO Concept', 'Expl', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-BKS-001', 'Concept', 'BKS', 'BKS Concept', 'Expl', '', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": { "project_code": "BKS" }
        })),
        id: Some(json!(3202)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let export_body = std::fs::read_to_string(&export_path).expect("export file should exist");
    assert!(export_body.contains("VIS-BKS-001"), "{export_body}");
    assert!(export_body.contains("CPT-BKS-001"), "{export_body}");
    assert!(!export_body.contains("VIS-AXO-001"), "{export_body}");
    assert!(!export_body.contains("CPT-AXO-001"), "{export_body}");

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_resume_vectorization_backfills_missing_queue_entries() {
    let server = create_test_server();
    let path = "/tmp/resume_vectorization.rs".to_string();
    server
        .graph_store
        .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 128, 1)])
        .unwrap();

    let extraction = parser::ExtractionResult {
        project_code: Some("PRJ".to_string()),
        symbols: vec![parser::Symbol {
            name: "resume_vectorization".to_string(),
            kind: "func".to_string(),
            start_line: 1,
            end_line: 1,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: std::collections::HashMap::new(),
            embedding: None,
        }],
        relations: vec![],
    };

    server
        .graph_store
        .insert_file_data_batch_with_vectorization_policy(
            &[crate::worker::DbWriteTask::FileExtraction {
                reservation_id: "resume-vectorization".to_string(),
                path: path.clone(),
                content: Some("fn resume_vectorization() {}".to_string()),
                extraction,
                processing_mode: ProcessingMode::Full,
                trace_id: "trace".to_string(),
                observed_cost_bytes: 0,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }],
            false,
        )
        .unwrap();

    let before = server
        .graph_store
        .query_count("SELECT count(*) FROM FileVectorizationQueue")
        .unwrap();
    assert_eq!(before, 0);

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "resume_vectorization",
            "arguments": {}
        })),
        id: Some(json!(904)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result["content"][0]["text"].as_str().unwrap_or_default();
    let queued = result["data"]["queued_files"].as_u64();

    assert!(content.contains("Resume Vectorization"), "{content}");
    assert_eq!(queued, Some(1), "{result:?}");
    let after = server
        .graph_store
        .query_count("SELECT count(*) FROM FileVectorizationQueue")
        .unwrap();
    assert_eq!(after, 1);
}

#[test]
fn test_vcr1_symbol_discovery_for_scan_trigger_flow() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/server.ex', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::trigger_scan', 'trigger_scan', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::trigger_global_scan', 'trigger_global_scan', 'function', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/server.ex', 'axon::trigger_scan', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex', 'axon::trigger_global_scan', 'AXO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": "AXO" }
        })),
        id: Some(json!(21)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("trigger_scan"));
    assert!(content.contains("trigger_global_scan"));
    assert!(content.contains("server.ex") || content.contains("pool_facade.ex"));
}

#[test]
fn test_vcr1_chunk_content_fallback_finds_symbol_from_natural_behavior_phrase() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/watcher.rs', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::opaque_worker', 'opaque_worker', 'function', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/watcher.rs', 'axon::opaque_worker', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::opaque_worker::chunk', 'symbol', 'axon::opaque_worker', 'AXO', 'function', 'symbol: opaque_worker\nkind: function\n\nwhen a manual scan requested event arrives, relay it to the rust watcher and keep the ui passive', 'hash-a', 10, 18)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "manual scan requested", "project": "AXO" }
        })),
        id: Some(json!(24)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("opaque_worker"));
    assert!(content.contains("chunk body") || content.contains("chunk metadata"));
    assert!(content.contains("rust watcher"));
}

#[test]
fn test_vcr1_chunk_content_result_includes_snippet_for_disambiguation() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/requeue.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/noise.rs', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::worker_alpha', 'worker_alpha', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::worker_beta', 'worker_beta', 'function', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/requeue.rs', 'axon::worker_alpha', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/noise.rs', 'axon::worker_beta', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::worker_alpha::chunk', 'symbol', 'axon::worker_alpha', 'AXO', 'function', 'symbol: worker_alpha\nkind: function\n\nrequeue claimed file back to pending when the common lane is full', 'hash-b', 20, 28)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::worker_beta::chunk', 'symbol', 'axon::worker_beta', 'AXO', 'function', 'symbol: worker_beta\nkind: function\n\nlog queue metrics and continue', 'hash-c', 2, 8)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "requeue claimed file", "project": "AXO" }
        })),
        id: Some(json!(25)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("requeue claimed file back to pending"),
        "{content}"
    );
    assert!(content.contains("src/runtime/requeue.rs"), "{content}");
}

#[test]
fn test_vcr1_chunk_retrieval_uses_ingested_docstring_content() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let path = "/tmp/axon_docstring_query.rs".to_string();
    server
        .graph_store
        .bulk_insert_files(&[(path.clone(), "AXO".to_string(), 120, 1)])
        .unwrap();

    let extraction = crate::parser::ExtractionResult {
        project_code: Some("AXO".to_string()),
        symbols: vec![crate::parser::Symbol {
            name: "opaque_gate".to_string(),
            kind: "function".to_string(),
            start_line: 1,
            end_line: 3,
            docstring: Some("Relays manual scan requests to the rust watcher without forcing a fake indexing overlay.".to_string()),
            is_entry_point: false,
            is_public: true,
            tested: true,
            is_nif: false,
            is_unsafe: false,
            properties: std::collections::HashMap::new(),
            embedding: None,
        }],
        relations: vec![],
    };

    server
        .graph_store
        .insert_file_data_batch(&[crate::worker::DbWriteTask::FileExtraction {
            reservation_id: "res-docstring-trace".to_string(),
            path: path.clone(),
            content: Some("fn opaque_gate() {\n    notify_runtime();\n}\n".to_string()),
            extraction,
            processing_mode: ProcessingMode::Full,
            trace_id: "docstring-trace".to_string(),
            observed_cost_bytes: 0,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        }])
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "fake indexing overlay", "project": "AXO" }
        })),
        id: Some(json!(26)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("opaque_gate"));
    assert!(content.contains("fake indexing overlay"));
    assert!(content.contains("docstring"));
}

#[test]
fn test_vcr1_chunk_fallback_prefers_docstring_or_body_over_path_only_match() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/path_only_fake_indexing_overlay.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/runtime/docstring_truth.rs', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::path_only_probe', 'path_only_probe', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::truth_probe', 'truth_probe', 'function', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/path_only_fake_indexing_overlay.rs', 'axon::path_only_probe', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/runtime/docstring_truth.rs', 'axon::truth_probe', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::path_only_probe::chunk', 'symbol', 'axon::path_only_probe', 'AXO', 'function', 'symbol: path_only_probe\nkind: function\n\nlog metrics and continue', 'hash-path', 1, 4)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::truth_probe::chunk', 'symbol', 'axon::truth_probe', 'AXO', 'function', 'symbol: truth_probe\nkind: function\ndocstring: prevent fake indexing overlay in the cockpit while forwarding to the rust watcher.\n\nnotify runtime and preserve live truth', 'hash-doc', 10, 18)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "fake indexing overlay", "project": "AXO" }
        })),
        id: Some(json!(27)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let truth_pos = content
        .find("src/runtime/docstring_truth.rs")
        .expect("docstring-backed file should appear");
    let path_pos = content
        .find("src/runtime/path_only_fake_indexing_overlay.rs")
        .expect("path-only file should appear");
    assert!(
        truth_pos < path_pos,
        "content-backed match should rank ahead of path-only match"
    );
    assert!(content.contains("docstring"), "{content}");
}

#[test]
fn test_axon_query_exact_config_lookup_prefers_operational_source_over_documentary_chunk() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('config/runtime.exs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::runtime_config', 'runtime_config', 'module', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::audit_section', 'audit_section', 'section', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('config/runtime.exs', 'axon::runtime_config', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon::audit_section', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::runtime_config::chunk', 'symbol', 'axon::runtime_config', 'AXO', 'module', 'symbol: runtime_config\nkind: module\n\nconfigures Credo.Check.Refactor.CyclomaticComplexity threshold for the application runtime', 'hash-runtime', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::audit_section::chunk', 'symbol', 'axon::audit_section', 'AXO', 'section', 'symbol: audit_section\nkind: section\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit', 20, 35)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": "AXO" }
        })),
        id: Some(json!(281)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let config_pos = content
        .find("config/runtime.exs")
        .expect("operational config result should appear");
    let doc_pos = content
        .find("docs/AXON_TEXT_PARSING_AUDIT.md")
        .expect("documentary result should appear");
    assert!(
        config_pos < doc_pos,
        "operational config source should rank ahead of documentary prose: {content}"
    );
    assert!(content.contains("Type de resultat"));
    assert!(content.contains("source operatoire"), "{content}");
    assert!(content.contains("config_lookup_exact"), "{content}");
}

#[test]
fn test_axon_query_exact_config_lookup_marks_documentary_result_when_only_docs_match() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::audit_section', 'audit_section', 'section', true, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon::audit_section', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('axon::audit_section::chunk', 'symbol', 'axon::audit_section', 'AXO', 'section', 'symbol: audit_section\nkind: section\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit-only', 20, 35)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": "AXO" }
        })),
        id: Some(json!(282)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("docs/AXON_TEXT_PARSING_AUDIT.md"),
        "{content}"
    );
    assert!(content.contains("Type de resultat"), "{content}");
    assert!(content.contains("documentaire"), "{content}");
    assert!(content.contains("config_lookup_exact"), "{content}");
}

#[test]
fn test_axon_query_falls_back_when_contains_is_absent() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::Axon.Watcher.Server.trigger_scan', 'Axon.Watcher.Server.trigger_scan', 'function', true, true, false, 'AXO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": "AXO" }
        })),
        id: Some(json!(211)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("degrade structurel sans ancrage fichier"),
        "{content}"
    );
    assert!(content.contains("trigger_scan"), "{content}");
}

#[test]
fn test_vcr2_impact_before_change_on_public_api() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/api.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/consumer_a.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/core/consumer_b.rs', 'AXO')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_a', 'consumer_a', 'function', false, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_b', 'consumer_b', 'function', false, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/api.rs', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/consumer_a.rs', 'axon::consumer_a', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/core/consumer_b.rs', 'axon::consumer_b', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::consumer_a', 'axon::parse_batch', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('axon::consumer_b', 'axon::parse_batch', 'AXO')")
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "parse_batch", "depth": 2 }
        })),
        id: Some(json!(22)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains("parse_batch"));
    assert!(impact_text.contains("consumer_a"));
    assert!(impact_text.contains("consumer_b"));
    assert!(impact_text.contains("Projection locale"));

    let api_break_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "api_break_check",
            "arguments": { "symbol": "parse_batch" }
        })),
        id: Some(json!(23)),
    };

    let api_break_response = server.handle_request(api_break_req);
    let api_break_result = api_break_response.unwrap().result.expect("Expected result");
    let api_break_text = api_break_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        api_break_text.contains("warn_api_break_risk")
            || api_break_text.contains("public api consumer impact detected")
    );
    assert!(api_break_text.contains("consumer_a"));
    assert!(api_break_text.contains("consumer_b"));
}

#[test]
fn test_axon_impact_reports_missing_call_graph_truthfully() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::parse_batch', 'parse_batch', 'function', true, true, false, 'PRJ')")
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "parse_batch", "depth": 2 }
        })),
        id: Some(json!(221)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains("le graphe d'appel n'est pas encore disponible"));
    assert!(impact_text.contains("parse_batch"));
}

#[test]
fn test_axon_impact_respects_project_scope_for_duplicate_symbol_names() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/pja/api.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/pja/consumer.rs', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/pjb/api.rs', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/pjb/consumer.rs', 'PJB')")
        .unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::consumer_alpha', 'consumer_alpha', 'function', false, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::parse_batch', 'parse_batch', 'function', true, true, false, 'PJB')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::consumer_beta', 'consumer_beta', 'function', false, true, false, 'PJB')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/api.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/consumer.rs', 'PJA::consumer_alpha', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pjb/api.rs', 'PJB::parse_batch', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pjb/consumer.rs', 'PJB::consumer_beta', 'PJB')")
        .unwrap();

    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJA::consumer_alpha', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJB::consumer_beta', 'PJB::parse_batch', 'PJB')")
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": {
                "symbol": "parse_batch",
                "project": "PJA",
                "depth": 2
            }
        })),
        id: Some(json!(199)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains("consumer_alpha"), "{}", impact_text);
    assert!(!impact_text.contains("consumer_beta"), "{}", impact_text);
}

#[test]
fn test_axon_query_reports_partial_truth_when_project_is_degraded() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, last_error_reason) VALUES ('src/pja/large.rs', 'PJA', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status) VALUES ('src/pjb/worker.rs', 'PJB', 'indexed')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::worker_loop', 'worker_loop', 'function', true, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/large.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pjb/worker.rs', 'PJB::worker_loop', 'PJB')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "rare docstring phrase", "project": "PJA" }
        })),
        id: Some(json!(301)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("verite partielle"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);
    assert_eq!(result["problem_class"], "index_incomplete");
    assert_eq!(result["next_best_actions"][0], "treat_result_as_partial");
}

#[test]
fn test_axon_query_includes_compact_guidance_for_wrong_project_scope() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/pja/config.ex', 'PJA', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::Config.Module.scan', 'Config.Module.scan', 'function', true, true, false, 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/config.ex', 'PJA::Config.Module.scan', 'PJA')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": { "query": "Config.Module.scan", "project": "AXO" }
            })),
            id: Some(json!(6212)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["problem_class"], "wrong_project_scope");
    assert_eq!(
        result["likely_cause"],
        "non_canonical_or_incorrect_project_code"
    );
    assert_eq!(result["next_best_actions"][0], "use_canonical_project_code");
}

#[test]
fn test_axon_query_includes_compact_guidance_when_runtime_profile_blocks_tool() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": { "query": "scan", "project": "AXO" }
            })),
            id: Some(json!(6213)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["problem_class"], "tool_unavailable");
    assert_eq!(
        result["next_best_actions"][0],
        "switch_to_supported_runtime_profile"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_axon_query_reports_project_completion_when_scope_is_partial() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, status_reason) VALUES \
             ('src/pja/live.rs', 'PJA', 'indexed', NULL), \
             ('src/pja/todo.rs', 'PJA', 'pending', 'metadata_changed_scan')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/live.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "parse_batch", "project": "PJA" }
        })),
        id: Some(json!(3011)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Completu"), "{}", content);
    assert!(content.contains("1/2"), "{}", content);
    assert!(content.contains("backlog"), "{}", content);
    assert!(content.contains("metadata_changed_scan"), "{}", content);
}

#[test]
fn test_axon_inspect_warns_when_symbol_is_degraded() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, last_error_reason) VALUES ('src/pja/large.rs', 'PJA', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/large.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": { "symbol": "parse_batch", "project": "PJA" }
        })),
        id: Some(json!(302)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Inspection du Symbole"), "{}", content);
    assert!(content.contains("verite partielle"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);

    unsafe {
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_impact_reports_partial_truth_for_degraded_symbol() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, last_error_reason) VALUES ('src/pja/large.rs', 'PJA', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/pjb/live.rs', 'PJB', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::caller', 'caller', 'function', false, true, false, 'PJB')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::callee', 'callee', 'function', true, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/large.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pjb/live.rs', 'PJB::caller', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pjb/live.rs', 'PJB::callee', 'PJB')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('PJB::caller', 'PJB::callee', 'PJB')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "parse_batch", "project": "PJA", "depth": 2 }
        })),
        id: Some(json!(303)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("verite partielle"), "{}", content);
    assert!(content.contains("structure_only"), "{}", content);
}

#[test]
fn test_axon_health_warns_when_project_contains_degraded_files() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, last_error_reason) VALUES ('src/pja/large.rs', 'PJA', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/pja/large.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            "arguments": { "project": "PJA" }
        })),
        id: Some(json!(304)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Health Report: PJA"), "{}", content);
    assert!(content.contains("verite partielle"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);
}

#[test]
fn test_axon_query_project_scope_uses_project_code_not_path_substring() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('/tmp/shared/api.rs', 'PJA', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('/tmp/shared/worker.rs', 'PJB', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::parse_batch', 'parse_batch', 'function', true, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/tmp/shared/api.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/tmp/shared/worker.rs', 'PJB::parse_batch', 'PJB')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "parse_batch", "project": "PJA" }
        })),
        id: Some(json!(305)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("/tmp/shared/api.rs"), "{}", content);
    assert!(!content.contains("/tmp/shared/worker.rs"), "{}", content);
}

#[test]
fn test_axon_inspect_respects_project_scope_for_duplicate_symbol_names() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('/tmp/shared/api.rs', 'PJA', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('/tmp/shared/worker.rs', 'PJB', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::parse_batch', 'parse_batch', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::parse_batch', 'parse_batch', 'module', false, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/tmp/shared/api.rs', 'PJA::parse_batch', 'PJA')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('/tmp/shared/worker.rs', 'PJB::parse_batch', 'PJB')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": { "symbol": "parse_batch", "project": "PJA" }
        })),
        id: Some(json!(306)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("| parse_batch | function | true |"),
        "{}",
        content
    );
    assert!(
        !content.contains("| parse_batch | module | false |"),
        "{}",
        content
    );
}

#[test]
fn test_vcr4_soll_continuity_create_export_restore_verify() {
    let source_server = create_test_server();
    source_server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-900', 'Vision', 'AXO', 'Axon Vision', 'Stable conceptual continuity', '', '{\"goal\":\"Protect SOLL while evolving IST\"}')")
        .unwrap();

    let create_calls = vec![
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_code": "AXO",
                    "title": "Concept Preservation",
                    "description": "SOLL must survive runtime churn"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "Reliable Restore",
                    "description": "Restore from official export without destructive reset",
                    "priority": "P1"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": "AXO",
                    "name": "Merge Restore",
                    "explanation": "Reconstruct conceptual entities from export",
                    "rationale": "Avoid losing intent across iterations"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "AXO",
                    "title": "Protect SOLL",
                    "context": "Agents previously removed conceptual state",
                    "rationale": "Exports must preserve the conceptual thread",
                    "status": "accepted"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "milestone",
                "data": {
                    "project_code": "AXO",
                    "title": "Usable Internal Continuity",
                    "status": "in_progress"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "validation",
                "data": {
                    "project_code": "AXO",
                    "method": "vcr4-e2e",
                    "result": "passed"
                }
            }
        }),
    ];

    for (idx, call) in create_calls.into_iter().enumerate() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(call),
            id: Some(json!(100 + idx)),
        };
        let response = source_server.handle_request(req);
        let result = response
            .unwrap()
            .result
            .expect("Expected SOLL creation result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.contains("Entité SOLL créée"));
    }

    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(200)),
    };

    let export_response = source_server.handle_request(export_req);
    let export_result = export_response
        .unwrap()
        .result
        .expect("Expected SOLL export result");
    let export_text = export_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(export_text.contains("docs/vision/SOLL_EXPORT_"));

    let export_path = export_text
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
        .trim()
        .to_string();

    let restore_server = create_test_server();
    let restore_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(201)),
    };

    let restore_response = restore_server.handle_request(restore_req);
    let restore_result = restore_response
        .unwrap()
        .result
        .expect("Expected SOLL restore result");
    let restore_text = restore_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        restore_text.contains("Restauration SOLL terminee"),
        "{}",
        restore_text
    );
    assert!(restore_text.contains("Vision: 1"));
    assert!(restore_text.contains("Pillars: 1"));
    assert!(restore_text.contains("Concepts: 1"));
    assert!(restore_text.contains("Milestones: 1"));
    assert!(restore_text.contains("Requirements: 1"));
    assert!(restore_text.contains("Decisions: 1"));
    assert!(restore_text.contains("Validations: 1"));

    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Vision'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Pillar'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Concept'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Milestone'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Requirement'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Decision'")
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Validation'")
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(&export_path);
}

#[test]
fn test_soll_query_context_returns_project_visions_from_source() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'accepted', '{\"goal\":\"Vision first\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_query_context",
            "arguments": { "project_code": "AXO", "limit": 5 }
        })),
        id: Some(json!(7801)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("data payload");
    let visions = data
        .get("visions")
        .and_then(|value| value.as_array())
        .expect("visions array");
    assert!(
        !visions.is_empty(),
        "visions should be returned from SOLL source"
    );
    let first = visions
        .first()
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(first.contains("VIS-AXO-001"), "{first}");
    assert!(first.contains("Axon Vision"), "{first}");
    assert!(first.contains("accepted"), "{first}");
    assert!(first.contains("Build from project vision"), "{first}");
}

#[test]
fn test_axon_soll_manager_link_rejects_missing_endpoint() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "requirement",
                "data": {
                    "source_id": "REQ-AXO-001",
                    "target_id": "PIL-AXO-404"
                }
            }
        })),
        id: Some(json!(4101)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("introuvable"), "{content}");
}

#[test]
fn test_axon_soll_manager_link_applies_default_relation() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001"
                }
            }
        })),
        id: Some(json!(4102)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Liaison établie"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='SOLVES' AND source_id = 'DEC-AXO-001' AND target_id = 'REQ-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_link_rejects_relation_outside_policy() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001",
                    "relation_type": "VERIFIES"
                }
            }
        })),
        id: Some(json!(4103)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("Relations autorisées"), "{content}");
    assert!(content.contains("SOLVES"), "{content}");
    assert!(content.contains("REFINES"), "{content}");
}

#[test]
fn test_axon_soll_manager_link_allows_authorized_cumulative_relation() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(4104)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Liaison établie"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='REFINES' AND source_id = 'DEC-AXO-001' AND target_id = 'REQ-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_vcr4_soll_restore_recovers_links_and_metadata_when_present() {
    let source_server = create_test_server();
    source_server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-900', 'Vision', 'AXO', 'Axon Vision', 'Stable conceptual continuity', '', '{\"goal\":\"Protect SOLL while evolving IST\"}')")
        .unwrap();

    let create_calls = vec![
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_code": "AXO",
                    "title": "Concept Preservation",
                    "description": "SOLL must survive runtime churn",
                    "metadata": { "owner": "platform" }
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "Reliable Restore",
                    "description": "Restore from official export without destructive reset",
                    "priority": "P1",
                    "metadata": { "risk": "high" }
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "AXO",
                    "title": "Protect SOLL",
                    "context": "Agents previously removed conceptual state",
                    "rationale": "Exports must preserve the conceptual thread",
                    "status": "accepted",
                    "metadata": { "scope": "restore" }
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "validation",
                "data": {
                    "project_code": "AXO",
                    "method": "vcr4-links",
                    "result": "passed",
                    "metadata": { "evidence": "test" }
                }
            }
        }),
    ];

    let mut created_ids = Vec::new();
    for (idx, call) in create_calls.into_iter().enumerate() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(call),
            id: Some(json!(300 + idx)),
        };
        let response = source_server.handle_request(req);
        let result = response
            .unwrap()
            .result
            .expect("Expected SOLL creation result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.contains("Entité SOLL créée"));
        created_ids.push(
            content
                .split('`')
                .nth(1)
                .expect("Expected generated SOLL id")
                .to_string(),
        );
    }

    let pillar_id = created_ids[0].clone();
    let requirement_id = created_ids[1].clone();
    let decision_id = created_ids[2].clone();
    let validation_id = created_ids[3].clone();

    let link_calls = vec![
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "requirement",
                "data": {
                    "source_id": requirement_id.clone(),
                    "target_id": pillar_id.clone(),
                    "relation_type": "BELONGS_TO"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": decision_id.clone(),
                    "target_id": requirement_id.clone(),
                    "relation_type": "SOLVES"
                }
            }
        }),
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "validation",
                "data": {
                    "source_id": validation_id.clone(),
                    "target_id": requirement_id.clone(),
                    "relation_type": "VERIFIES"
                }
            }
        }),
    ];

    for (idx, call) in link_calls.into_iter().enumerate() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(call),
            id: Some(json!(400 + idx)),
        };
        let response = source_server.handle_request(req);
        let result = response.unwrap().result.expect("Expected SOLL link result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.contains("Liaison établie"));
    }

    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(500)),
    };

    let export_response = source_server.handle_request(export_req);
    let export_result = export_response
        .unwrap()
        .result
        .expect("Expected SOLL export result");
    let export_text = export_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let export_path = export_text
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
        .trim()
        .to_string();
    let export_markdown = std::fs::read_to_string(&export_path).unwrap();
    println!("DEBUG EXPORT:\n{}", export_markdown);
    assert!(export_markdown.contains("BELONGS_TO"));
    assert!(export_markdown.contains("SOLVES"));
    assert!(export_markdown.contains("VERIFIES"));
    assert!(export_markdown.contains("platform"));
    assert!(export_markdown.contains("high"));
    assert!(export_markdown.contains("scope"));

    let restore_server = create_test_server();
    let restore_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(501)),
    };

    let restore_response = restore_server.handle_request(restore_req);
    let restore_result = restore_response
        .unwrap()
        .result
        .expect("Expected SOLL restore result");
    let restore_text = restore_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        restore_text.contains("Restauration SOLL terminee"),
        "{}",
        restore_text
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id = '{}' AND target_id = '{}'",
                requirement_id, pillar_id
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='SOLVES' AND source_id = '{}' AND target_id = '{}'",
                decision_id, requirement_id
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='VERIFIES' AND source_id = '{}' AND target_id = '{}'",
                validation_id, requirement_id
            ))
            .unwrap(),
        1
    );

    let pillar_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Pillar' AND id = '{}'",
            pillar_id
        ))
        .unwrap();
    let requirement_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Requirement' AND id = '{}'",
            requirement_id
        ))
        .unwrap();
    let decision_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Decision' AND id = '{}'",
            decision_id
        ))
        .unwrap();
    let all_validations = restore_server
        .graph_store
        .query_json("SELECT * FROM soll.Node WHERE type='Validation'")
        .unwrap();
    println!("ALL VALIDATIONS: {}", all_validations);

    let validation_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Validation' AND id = '{}'",
            validation_id
        ))
        .unwrap();

    assert!(pillar_metadata.contains("platform"));
    assert!(
        requirement_metadata.contains("high"),
        "{}",
        requirement_metadata
    );
    assert!(decision_metadata.contains("restore"));
    assert!(
        validation_metadata.contains("test"),
        "{}",
        validation_metadata
    );

    let second_restore_response = restore_server.handle_request(JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(502)),
    });
    second_restore_response
        .unwrap()
        .result
        .expect("Expected second restore result");

    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id = '{}' AND target_id = '{}'",
                requirement_id, pillar_id
            ))
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(&export_path);
}

#[test]
fn test_axon_commit_work_enforces_guideline() {
    let server = create_test_server();

    // Insert a Guideline into SolDB requiring tests to be updated if src/mcp/ is modified
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) 
         VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'Mise à jour des Tests', 'Les modifications de src/mcp/ doivent inclure des tests', 'active', '{\"trigger_path\":\"src/mcp/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    // 1. Simulate a bad commit (modifies src/mcp/ but no tests.rs)
    let req_bad = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs"],
                "message": "fix: update tools",
                "dry_run": true
            }
        },
        "id": 1
    });

    let res_bad = server
        .handle_request(serde_json::from_value(req_bad).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content_bad = res_bad.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG CONTENT BAD: {}", content_bad);

    // It should be rejected
    assert!(res_bad
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content_bad.contains("GUI-AXO-001") || content_bad.contains("GUI-PRO-001"));
    assert!(content_bad.contains("remediation_plan"));

    // 2. Simulate a good commit (modifies src/mcp/ AND tests.rs)
    let req_good = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs", "src/axon-core/src/mcp/tests.rs", "SKILL.md"],
                "message": "fix: update tools and tests",
                "dry_run": true
            }
        },
        "id": 2
    });

    let res_good = server
        .handle_request(serde_json::from_value(req_good).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content_good = res_good.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // It should pass
    assert!(!res_good
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content_good.contains("Validation réussie"));
}

#[test]
fn test_bootstrap_injects_global_guidelines() {
    let server = create_test_server();

    // Check GUI-PRO-001
    let count1 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-001' AND type = 'Guideline' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count1, 1, "GUI-PRO-001 should be injected at bootstrap");

    let meta1_raw = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-001'")
        .unwrap();
    println!("DEBUG META1 RAW: {}", meta1_raw);
    let meta1: Vec<Vec<String>> = serde_json::from_str(&meta1_raw).unwrap();
    assert!(
        meta1[0][0].contains("\"phase\":\"pre-code\"")
            || meta1[0][0].contains("\"phase\": \"pre-code\""),
        "GUI-PRO-001 should have phase: pre-code"
    );

    // Check GUI-PRO-002
    let count2 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-002' AND type = 'Guideline' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count2, 1, "GUI-PRO-002 should be injected at bootstrap");

    let meta2_raw = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-002'")
        .unwrap();
    println!("DEBUG META2 RAW: {}", meta2_raw);
    let meta2: Vec<Vec<String>> = serde_json::from_str(&meta2_raw).unwrap();
    assert!(
        meta2[0][0].contains("\"phase\":\"post-code\"")
            || meta2[0][0].contains("\"phase\": \"post-code\""),
        "GUI-PRO-002 should have phase: post-code"
    );
}

#[test]
fn test_axon_init_project_returns_global_guidelines() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/home/dstadel/projects/BookingSystem",
                "concept_document_url_or_text": "We want a booking system."
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG INIT OUTPUT: {}", content);

    // Output should contain the global guidelines injected at bootstrap
    assert!(content.contains("GUI-PRO-001"));
    assert!(content.contains("GUI-PRO-002"));
    assert!(content.contains("Voici les règles globales disponibles."));
    assert!(content.contains("Code projet attribué par le serveur: `BKS`"));
    assert_eq!(result["data"]["project_code"].as_str(), Some("BKS"));
    assert_eq!(
        result["data"]["project_name"].as_str(),
        Some("BookingSystem")
    );
    assert_eq!(
        result["data"]["project_path"].as_str(),
        Some("/home/dstadel/projects/BookingSystem")
    );
}

#[test]
fn test_axon_init_project_rejects_client_project_code_when_it_differs_from_server_assignment() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_code": "AXO",
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        },
        "id": 10007
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(content.contains("attribué par le serveur"), "{content}");
    assert!(content.contains("BKS"), "{content}");
}

#[test]
fn test_axon_apply_guidelines_creates_local_copies() {
    let server = create_test_server();

    // First init the project
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("BookingSystem"), None)
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-001"]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // Output should confirm creation
    assert!(content.contains("GUI-AXO-001"));
    assert!(content.contains("Héritage appliqué"));

    // Verify in DB
    let count = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-AXO-001' AND type = 'Guideline' AND project_code = 'AXO'"
    ).unwrap();
    assert_eq!(count, 1, "Local guideline should be created");

    // Verify edge
    let edge_count = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Edge WHERE relation_type = 'INHERITS_FROM' AND source_id = 'GUI-AXO-001' AND target_id = 'GUI-PRO-001'"
    ).unwrap();
    assert_eq!(edge_count, 1, "Inheritance edge should be created");
}

#[test]
fn test_soll_commit_revision_returns_identity_mapping_and_resolves_relations() {
    let server = create_test_server();

    // Create a plan with logical keys and a relation using those keys
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test",
                "dry_run": false,
                "plan": {
                    "requirements": [
                        { "logical_key": "req-1", "title": "Req A", "description": "Desc A" }
                    ],
                    "decisions": [
                        { "logical_key": "dec-1", "title": "Dec B", "description": "Desc B" }
                    ]
                },
                "relations": [
                    {
                        "source_id": "dec-1",
                        "target_id": "req-1",
                        "relation_type": "SOLVES"
                    }
                ]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // Should be committed immediately because dry_run = false
    assert!(content.contains("SOLL revision committed"), "{}", content);

    // We expect identity_mapping in the result.data
    let data = result.get("data").expect("Should have data field");
    let identity_mapping = data
        .get("identity_mapping")
        .expect("Should have identity_mapping");

    let dec_id = identity_mapping.get("dec-1").unwrap().as_str().unwrap();
    let req_id = identity_mapping.get("req-1").unwrap().as_str().unwrap();

    assert!(dec_id.starts_with("DEC-AXO-"));
    assert!(req_id.starts_with("REQ-AXO-"));

    // Verify the edge in DB using the canonical IDs
    let edge_count = server.graph_store.query_count(&format!(
        "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = 'SOLVES'",
        dec_id, req_id
    )).unwrap();
    assert_eq!(
        edge_count, 1,
        "The relation should be created using canonical IDs"
    );
}

#[test]
fn test_axon_commit_work_executes_git_and_export_when_dry_run_false() {
    let server = create_test_server();

    // Insert a dummy Guideline that passes trivially
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) 
         VALUES ('GUI-AXO-999', 'Guideline', 'AXO', 'Dummy', 'Dummy', 'active', '{\"trigger_path\":\"\",\"required_path\":\"\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["Cargo.toml"],
                "message": "test: dummy commit from mcp tests",
                "dry_run": false
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // It should not be an error
    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "{}",
        content
    );

    // It should contain Git and Export mentions
    assert!(
        content.contains("Commit effectué") || content.contains("Commit échoué"),
        "{}",
        content
    );
    assert!(content.contains("Exported to"), "{}", content);
}

#[test]
fn test_axon_impact_traces_through_soll_architecture() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    // 1. Create Code Symbols and Calls
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/payment.rs', 'BKS')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_code) VALUES ('payment::process', 'process', 'function', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'payment::process', 'BKS')").unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_code) VALUES ('api::checkout', 'checkout', 'function', 'BKS')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id, project_code) VALUES ('api::checkout', 'payment::process', 'BKS')",
        )
        .unwrap();

    // 2. Create SOLL Intent Graph
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('VIS-BKS-001', 'Vision', 'BKS', 'Paiement sans friction')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('REQ-BKS-005', 'Requirement', 'BKS', 'Intégration Stripe')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Utiliser Rust Stripe SDK')").unwrap();

    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-BKS-005', 'VIS-BKS-001', 'BELONGS_TO')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-BKS-010', 'REQ-BKS-005', 'SOLVES')").unwrap();

    // 3. Create Traceability Bridge (Code -> Intent)
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-001', 'Decision', 'DEC-BKS-010', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    // 4. Query Impact on the deep code function
    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "process", "depth": 2 }
        })),
        id: Some(json!(1)),
    };

    let impact_res = server.handle_request(impact_req).unwrap().result.unwrap();
    let content = impact_res.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // 5. Asserts
    println!("DEBUG IMPACT CONTENT: {}", content);
    assert!(content.contains("checkout"), "Should find caller symbol");
    assert!(
        content.contains("DEC-BKS-010"),
        "Should bridge to SOLL Decision"
    );
    assert!(
        content.contains("Utiliser Rust Stripe SDK"),
        "Should list decision title"
    );
    assert!(
        content.contains("REQ-BKS-005"),
        "Should traverse to Requirement"
    );
    assert!(content.contains("VIS-BKS-001"), "Should traverse to Vision");
    assert!(
        content.contains("Paiement sans friction"),
        "Should list vision title"
    );
}
