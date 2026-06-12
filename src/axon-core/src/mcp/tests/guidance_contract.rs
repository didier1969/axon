use super::*;

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
    let tempdir = tempdir().unwrap();
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "resume_vectorization",
                "arguments": {}
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
fn test_invalid_arguments_returns_parameter_repair_with_input_schema_reference() {
    // REQ-AXO-139 slice — universal parameter_repair contract for the
    // dispatcher's Invalid-arguments fallback path. When a tool handler
    // returns None because a required arg is missing or malformed, the
    // dispatcher must surface `data.parameter_repair` with the missing
    // required fields, the supplied arguments, the canonical input_schema,
    // and a `help` follow-up tool — same shape as cypher binder /
    // soll_attach_evidence / inspect slices.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": {}
            })),
            id: Some(json!(91391)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["status"], "input_invalid");
    assert_eq!(result["data"]["problem_class"], "invalid_arguments");

    let repair = result["data"]
        .get("parameter_repair")
        .expect("parameter_repair payload required for invalid_arguments");
    assert_eq!(repair["tool"].as_str(), Some("query"));
    let missing = repair["missing_required_fields"]
        .as_array()
        .expect("missing_required_fields array");
    let missing_names: Vec<&str> = missing.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        missing_names.contains(&"query"),
        "missing_required_fields must include `query`: {missing_names:?}"
    );
    assert_eq!(
        repair["invalid_field"].as_str(),
        Some("query"),
        "invalid_field must point at the first missing required field"
    );

    assert!(
        repair["input_schema"].is_object(),
        "parameter_repair must reference the canonical input_schema: {repair}"
    );
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_names.contains(&"help"),
        "follow_up_tools must include `help`: {follow_names:?}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("corrected_call") && hint.contains("query"),
        "hint must steer toward emitting `corrected_call` for `query` (repair-as-data, REQ-AXO-901949): {hint}"
    );
    // Repair-as-data (REQ-AXO-901949): the corrected call is handed back ready
    // to emit, with each missing required field stubbed.
    assert!(
        repair["corrected_call"]["arguments"]["query"]
            .as_str()
            .is_some_and(|v| v.contains("<FILL")),
        "corrected_call must stub the missing required `query`: {repair}"
    );
}

#[test]
fn test_invalid_arguments_surfaces_per_action_conditional_field() {
    // REQ-AXO-901949 inv.3 — top-level `required` (action/entity/data) is
    // satisfied, but `soll_manager update` without `data.id` still hits the
    // None-path. The dispatcher must read the schema's allOf conditionals so the
    // real missing field (`data.id`) is reported AND stubbed nested, instead of
    // an empty/unhelpful repair envelope.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "update",
                    "entity": "requirement",
                    "data": { "description": "no id supplied" }
                }
            })),
            id: Some(json!(91392)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["problem_class"], "invalid_arguments");
    let repair = result["data"]
        .get("parameter_repair")
        .expect("parameter_repair payload required");

    let missing_names: Vec<&str> = repair["missing_required_fields"]
        .as_array()
        .expect("missing_required_fields array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        missing_names.contains(&"data.id"),
        "per-action conditional must surface `data.id`: {missing_names:?}"
    );
    // The corrected call stubs the missing field NESTED under data, ready to fill.
    assert!(
        repair["corrected_call"]["arguments"]["data"]["id"]
            .as_str()
            .is_some_and(|v| v.contains("<FILL")),
        "corrected_call must stub the nested `data.id`: {repair}"
    );
    // The originally-supplied data is preserved alongside the stub.
    assert_eq!(
        repair["corrected_call"]["arguments"]["data"]["description"].as_str(),
        Some("no id supplied"),
        "corrected_call must preserve supplied fields: {repair}"
    );
}

#[test]
fn invalid_arguments_authoritative_guidance_includes_micro_instruction_and_contract() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "fs_read",
                "arguments": {}
            })),
            id: Some(json!(9002)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["problem_class"], "invalid_arguments");
    assert_eq!(
        result["data"]["likely_cause"],
        "request_shape_does_not_match_tool_contract"
    );
    assert_eq!(result["data"]["next_action"]["tool"], "help");
    assert_eq!(
        result["data"]["next_action"]["arguments"]["tool"],
        "fs_read"
    );
    assert_eq!(
        result["data"]["repair_instruction"].as_str().unwrap(),
        "Emit `parameter_repair.corrected_call` — it is your arguments with missing required fields stubbed. Fill the stubs and re-call. No schema diffing needed."
    );

    unsafe {
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
    }
}

#[test]
fn unknown_tool_name_authoritative_guidance_includes_surface_recovery() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "not_a_real_tool",
                "arguments": {}
            })),
            id: Some(json!(9003)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(result["data"]["problem_class"], "unknown_tool_name");
    assert_eq!(
        result["data"]["next_best_actions"][0],
        "retry_with_public_tool_name"
    );
    assert_eq!(
        result["data"]["next_action"]["tool"],
        "mcp_surface_diagnostics"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
    }
}

#[test]
fn status_response_is_terse_by_default() {
    // REQ-AXO-901947 invariant 4 — a clean success carries only the answer +
    // `next_action`, NOT the full operator_guidance envelope (push→pull). The
    // full envelope is opt-in (`detail:"full"`) — see the sibling test.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(9014)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    // Terse: the heavy guidance envelope is absent on a clean success...
    assert!(
        result["data"]["operator_guidance"]["llm_usage_instruction"].is_null(),
        "terse-default success must NOT push the full guidance envelope: {}",
        result["data"]
    );
    // ...but the minimal continuation (`next_action`) is always present.
    assert!(
        result["data"]["next_action"]["tool"].as_str().is_some(),
        "terse-default success must still carry a minimal next_action: {}",
        result["data"]
    );
}

#[test]
fn status_response_with_detail_full_pulls_rich_operator_guidance() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief", "detail": "full" }
            })),
            id: Some(json!(9004)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_eq!(
        result["data"]["operator_guidance"]["workflow_stage"].as_str(),
        Some("runtime_truth")
    );
    assert!(result["data"]["operator_guidance"]["primary_goal"]
        .as_str()
        .is_some_and(|text| text.contains("runtime truth")));
    assert!(result["data"]["operator_guidance"]["token_efficiency_hint"]
        .as_str()
        .is_some_and(|text| text.contains("runtime truth")));
    assert_ne!(
        result["data"]["next_action"]["tool"].as_str(),
        Some("status")
    );
    assert!(
        result["data"]["operator_guidance"]["alternative_strategies"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(result["data"]["operator_guidance"]["llm_usage_instruction"]
        .as_str()
        .is_some_and(|text| text.contains("Use `next_action` first")));
    assert!(result["data"]["operator_guidance"]["fallback_strategy"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["if"] == "invalid_arguments")));
    assert!(result["data"]["operator_guidance"]["explicit_input_rule"]
        .as_str()
        .is_some_and(|text| text.contains("Do not ask the client to choose MCP tools")));
    assert_eq!(
        result["data"]["operator_guidance"]["llm_contract"]["first"].as_str(),
        Some("next_action")
    );
    assert_eq!(
        result["data"]["operator_guidance"]["llm_contract"]["token_rule"].as_str(),
        Some("prefer brief mode; escalate only after a named missing dimension")
    );
}

#[test]
fn query_shadow_guidance_keeps_public_read_surface_available() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
                "arguments": { "query": "scan", "project": "AXO", "detail": "full" }
            })),
            id: Some(json!(6211)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    assert_ne!(
        result["data"]["_shadow"]["guidance"]["problem_class"],
        "tool_unavailable"
    );
    assert!(result["data"]["operator_guidance"].as_object().is_some());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_MCP_GUIDANCE_SHADOW");
    }
}
