use super::*;

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
    assert!(content.contains("Search results"));
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
    assert!(
        content.contains("BookingSystem") && (content.contains("non canonique") || content.contains("canonical")),
        "Error should reject non-canonical project code: {content}"
    );
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
    assert!(result["data"]["created"].is_array());
    assert!(result["data"]["updated"].is_array());
    assert!(result["data"]["linked"].is_array());
    assert!(result["data"]["skipped"].is_array());
    assert!(result["data"]["errors"].is_array());
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
    assert!(result["data"]["result_contract"]["created"].is_array());
    assert!(result["data"]["result_contract"]["updated"].is_array());
    assert!(result["data"]["result_contract"]["linked"].is_array());
    assert!(result["data"]["result_contract"]["skipped"].is_array());
    assert!(result["data"]["result_contract"]["errors"].is_array());
}

#[test]
fn test_axon_soll_apply_plan_accepts_guidelines_stakeholders_validations() {
    // REQ-AXO-092 — build_plan_operations only iterated pillar/requirement/
    // decision/milestone/vision/concept, silently dropping plan.guidelines,
    // plan.stakeholders, plan.validations even though the storage layer
    // already supports all three. Adding them to the iteration list closes
    // the gap and makes soll_apply_plan symmetric with soll_manager.
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
                    "guidelines": [{
                        "logical_key": "gui-tdd-real-io",
                        "title": "TDD with real I/O",
                        "description": "Tests must hit real DBs"
                    }],
                    "stakeholders": [{
                        "logical_key": "stk-platform-eng",
                        "title": "Platform Engineering",
                        "description": "Owns runtime SLOs"
                    }],
                    "validations": [{
                        "logical_key": "val-cold-start",
                        "title": "Cold start validates GPU envelope",
                        "description": "Validation node for the cold-start GPU envelope check",
                        "result": "pending"
                    }]
                }
            }
        })),
        id: Some(json!(10092)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let operations = result["data"]["operations"]
        .as_array()
        .expect("operations array");
    let entities: std::collections::HashSet<&str> = operations
        .iter()
        .filter_map(|op| op.get("entity").and_then(|v| v.as_str()))
        .collect();
    assert!(
        entities.contains("guideline"),
        "plan.guidelines must produce a `guideline` operation: {operations:?}"
    );
    assert!(
        entities.contains("stakeholder"),
        "plan.stakeholders must produce a `stakeholder` operation: {operations:?}"
    );
    assert!(
        entities.contains("validation"),
        "plan.validations must produce a `validation` operation: {operations:?}"
    );
    // Three new entries must each be `create` (none pre-existed)
    let create_ops: Vec<&Value> = operations
        .iter()
        .filter(|op| op.get("kind").and_then(|v| v.as_str()) == Some("create"))
        .collect();
    assert!(
        create_ops.len() >= 3,
        "expected at least 3 create ops, got {}: {operations:?}",
        create_ops.len()
    );
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
fn test_axon_soll_manager_create_without_project_code_auto_resolves_or_errors() {
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
                    "title": "Auto-resolve test",
                    "context": "project_code omitted — should auto-detect from cwd or single project",
                    "rationale": "Zero-config onboarding for single-project or cwd-matched usage",
                    "status": "accepted"
                }
            }
        })),
        id: Some(json!(1002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if is_error {
        // Multi-project without cwd match: should list known codes.
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            content.contains("`project_code`") && content.contains("required"),
            "Error should mention project_code is required: {content}"
        );
    } else {
        // Single project or cwd matched: auto-resolved successfully.
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            !content.is_empty(),
            "Auto-resolved mutation should return non-empty content"
        );
    }
}

#[test]
fn test_infer_soll_mutation_returns_impacted_existing_candidates() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'Perishability ordering', 'Short-life ingredients must be consumed earlier in the week.', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "infer_soll_mutation",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Weekly shopping should allow grouped purchases."
                }
            })),
            id: Some(json!(1)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result["data"]["proposed_operation_kind"].as_str(),
        Some("update_existing_entities")
    );
    assert_eq!(
        result["data"]["candidate_entity_type"].as_str(),
        Some("Requirement")
    );
    assert_eq!(
        result["data"]["target_ids"][0].as_str(),
        Some("REQ-AXO-001")
    );
}

#[test]
fn test_entrench_nuance_requires_confirmation_before_write() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Weekly shopping should allow grouped purchases."
                }
            })),
            id: Some(json!(2)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["confirm_required"].as_bool(), Some(true));

    let rows = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'REQ-AXO-001'")
        .unwrap();
    assert!(!rows.contains("nuances"));
}

#[test]
fn test_entrench_nuance_confirmed_updates_existing_nodes_and_returns_feedback() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true,
                    "target_ids": ["REQ-AXO-001"]
                }
            })),
            id: Some(json!(3)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["confirm_required"].as_bool(), None);
    assert_eq!(
        result["data"]["mutation_feedback"]["changed_entities"][0]["id"].as_str(),
        Some("REQ-AXO-001")
    );

    let rows = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'REQ-AXO-001'")
        .unwrap();
    assert!(rows.contains("Weekly shopping should allow grouped purchases."));
    assert!(rows.contains("nuances"));
}

#[test]
fn test_entrench_nuance_confirmed_rejects_cross_project_target_ids() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("NTO", Some("Nutri"), Some("/tmp/nutri"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-NTO-001', 'Requirement', 'NTO', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true,
                    "target_ids": ["REQ-NTO-001"]
                }
            })),
            id: Some(json!(31)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["invalid_target_ids"][0].as_str(),
        Some("REQ-NTO-001")
    );
}

#[test]
fn test_entrench_nuance_confirmed_requires_explicit_scope_when_inference_is_ambiguous() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'Grouped shopping purchases v2', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true
                }
            })),
            id: Some(json!(32)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert!(result["data"]["ambiguity_warnings"].is_array());
}

#[test]
fn test_soll_manager_create_returns_mutation_feedback() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "requirement",
                    "project_code": "AXO",
                    "data": {
                        "project_code": "AXO",
                        "title": "Roadmap feedback requirement",
                        "description": "A new canonical requirement from roadmap feedback."
                    }
                }
            })),
            id: Some(json!(4)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert!(result["data"]["mutation_feedback"].is_object());
    assert_eq!(
        result["data"]["mutation_feedback"]["topology_delta"]["nodes_created"].as_u64(),
        Some(1)
    );
}

#[test]
fn test_wrong_project_scope_response_helper_emits_canonical_contract() {
    // REQ-AXO-043 — direct unit test of the shared helper introduced
    // when consolidating four duplicated contract sites.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("Booking"), Some("/tmp/booking"))
        .unwrap();

    let payload = server.wrong_project_scope_response("BAD_CODE", "test_tool");
    assert_eq!(payload["isError"].as_bool(), Some(true));

    let content = payload["content"][0]["text"]
        .as_str()
        .expect("content text");
    assert!(content.contains("BAD_CODE"));
    assert!(content.contains("test_tool"));

    let data = &payload["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(data["rejected_project_code"].as_str(), Some("BAD_CODE"));
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO") && registered_strs.contains(&"BKS"),
        "must list seeded codes: {registered_strs:?}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert_eq!(
        actions.len(),
        2,
        "base helper emits exactly 2 next_best_actions, got {}",
        actions.len()
    );

    // Variant with extras
    let payload2 = server.wrong_project_scope_response_with_extras(
        "BAD",
        "another_tool",
        &["custom hint A", "custom hint B"],
    );
    let actions2 = payload2["data"]["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert_eq!(
        actions2.len(),
        4,
        "extras variant appends 2 additional actions to the base 2"
    );
    let actions_text: String = actions2
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(actions_text.contains("custom hint A"));
    assert!(actions_text.contains("custom hint B"));
}

#[test]
fn test_axon_soll_verify_requirements_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — soll_verify_requirements adopts the shared helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "MISSING_VR_001" }
            })),
            id: Some(json!(43106)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        result["data"]["rejected_project_code"].as_str(),
        Some("MISSING_VR_001")
    );
}

#[test]
fn test_axon_infer_soll_mutation_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — infer_soll_mutation adopts the shared helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "infer_soll_mutation",
                "arguments": {
                    "project_code": "MISSING_INF_002",
                    "statement": "stub"
                }
            })),
            id: Some(json!(43107)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        result["data"]["rejected_project_code"].as_str(),
        Some("MISSING_INF_002")
    );
}

#[test]
fn test_axon_init_project_warns_when_project_path_does_not_exist_on_disk() {
    // REQ-AXO-118 — a bogus project_path (typo or imaginary directory)
    // previously registered silently. Now the registration succeeds (legit
    // "register a future project" use case) but data.warnings + the
    // LLM-visible content surface the path-doesn-t-exist condition so the
    // typo is catchable at registration time.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let bogus_path = "/path/to/definitely/does/not/exist/xyz_abc_test";
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": { "project_path": bogus_path }
            })),
            id: Some(json!(43108)),
        })
        .unwrap();
    let result = response.result.expect("Expected result");

    // Registration still succeeds (non-blocking warning)
    assert_ne!(result["isError"].as_bool(), Some(true), "should succeed: {result}");
    assert!(
        result["data"]["project_code"].as_str().is_some(),
        "should still assign a code: {result}"
    );

    // But the warning is surfaced
    assert_eq!(
        result["data"]["path_exists_on_disk"].as_bool(),
        Some(false),
        "must report path_exists_on_disk=false: {result}"
    );
    let warnings = result["data"]["warnings"]
        .as_array()
        .expect("warnings array");
    assert_eq!(warnings.len(), 1, "expected exactly one warning: {warnings:?}");
    assert_eq!(
        warnings[0]["kind"].as_str(),
        Some("path_does_not_exist_on_disk")
    );
    assert_eq!(warnings[0]["path"].as_str(), Some(bogus_path));
    assert!(warnings[0]["next_action"].as_str().is_some());

    // Content text mentions the typo / mkdir hint so a one-shot LLM read catches it
    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("does not currently exist on disk"),
        "content must surface the warning: {content}"
    );
    assert!(
        content.contains("mkdir") || content.contains("typo"),
        "content must give a recovery hint: {content}"
    );
}

#[test]
fn test_axon_validate_soll_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — soll_validate now uses the shared
    // wrong_project_scope_response helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_validate",
                "arguments": { "project_code": "NEVER_REGISTERED_VVV" }
            })),
            id: Some(json!(43105)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NEVER_REGISTERED_VVV")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list seeded AXO: {registered_strs:?}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
}

#[test]
fn test_axon_entrench_nuance_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — entrench_nuance previously returned a bare
    // "Entrenchment failed: ..." string when project_code was unregistered.
    // Now mirrors the wrong_project_scope contract for consistency.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "NOT_REGISTERED_RRR",
                    "statement": "irrelevant"
                }
            })),
            id: Some(json!(43104)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NOT_REGISTERED_RRR")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list seeded AXO: {registered_strs:?}"
    );
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
}

#[test]
fn test_axon_soll_work_plan_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — work_plan previously returned `Status: ok` with empty
    // Evidence for a non-registered project_code. Verify the symmetric
    // soll_query_context contract is now applied.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_work_plan",
                "arguments": { "project_code": "NOT_A_REAL_PROJECT_XYZ" }
            })),
            id: Some(json!(43102)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NOT_A_REAL_PROJECT_XYZ")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list registered codes: {registered_strs:?}"
    );
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );

    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("NOT_A_REAL_PROJECT_XYZ"),
        "content must echo rejected: {content}"
    );
    assert!(
        content.contains("AXO"),
        "content must list registered codes: {content}"
    );
}

#[test]
fn test_axon_soll_query_context_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — the previous .ok()? swallowed the resolve_project_code
    // error and the framework rendered a generic "Invalid arguments". The
    // LLM had no way to know which project_codes are registered or how to
    // recover. Surface the structured recovery contract explicitly.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("Booking"), Some("/tmp/booking"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_query_context",
                "arguments": { "project_code": "DEFINITELY_NOT_REGISTERED" }
            })),
            id: Some(json!(40432)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("DEFINITELY_NOT_REGISTERED")
    );

    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO") && registered_strs.contains(&"BKS"),
        "must list registered codes: {registered_strs:?}"
    );

    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
    let follow_up = data["operator_guidance"]["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_up_strs: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_strs.contains(&"project_registry_lookup")
            || follow_up_strs.contains(&"axon_init_project"),
        "follow_up_tools must point to registry/init: {follow_up_strs:?}"
    );

    let content = result["content"][0]["text"]
        .as_str()
        .expect("content text");
    assert!(
        content.contains("DEFINITELY_NOT_REGISTERED"),
        "content must echo the rejected code: {content}"
    );
    assert!(
        content.contains("AXO") || content.contains("BKS"),
        "content must list registered codes: {content}"
    );
}

#[test]
fn test_soll_manager_create_guideline_lands_with_gui_prefix() {
    // REQ-AXO-092 — schema enum advertises `guideline` but the create branch
    // previously rejected it as "Unknown entity", forcing LLMs toward cypher
    // INSERT workarounds. Storage layer already supports the GUI prefix.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "guideline",
                    "project_code": "AXO",
                    "data": {
                        "project_code": "AXO",
                        "title": "TDD with real I/O",
                        "description": "Tests must hit real DBs, not mocks."
                    }
                }
            })),
            id: Some(json!(40921)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_ne!(result["isError"].as_bool(), Some(true), "create guideline should not error: {result}");

    // Response should expose canonical id (GUI-{project}-NNN) and entity_type
    let data = &result["data"];
    let created_id = data["created_id"].as_str().expect("created_id present");
    assert!(
        created_id.starts_with("GUI-AXO-"),
        "id must use GUI-AXO- prefix: {created_id}"
    );
    assert_eq!(data["entity_type"].as_str(), Some("Guideline"));
}

#[test]
fn test_soll_manager_create_unknown_entity_returns_recovery_contract() {
    // REQ-AXO-043 — unknown-entity error must surface accepted_entities and
    // next_action so the LLM client can recover without re-reading source.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "rumour",  // not in schema
                    "project_code": "AXO",
                    "data": { "project_code": "AXO", "title": "x", "description": "y" }
                }
            })),
            id: Some(json!(40431)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(content.contains("Unknown entity"), "content must surface failure: {content}");
    assert!(
        content.contains("guideline") && content.contains("requirement"),
        "content must list accepted entity types: {content}"
    );

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    assert_eq!(data["rejected_entity"].as_str(), Some("rumour"));
    let accepted = data["accepted_entities"].as_array().expect("accepted_entities array");
    assert!(accepted.iter().any(|v| v.as_str() == Some("guideline")));
    assert!(accepted.iter().any(|v| v.as_str() == Some("requirement")));
    assert!(data["next_action"].as_str().is_some(), "next_action must be set");
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
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
        content.contains("Non-canonical project_code"),
        "{content}"
    );
    assert!(content.contains("BookingSystem"), "{content}");
    assert!(content.contains("3-char uppercase canonical codes"), "{content}");
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
        content.contains("Non-canonical project_code"),
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
        content.contains("Non-canonical project_code"),
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
        update_content.contains("Update succeeded"),
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
fn test_soll_manager_update_unknown_id_returns_normalized_contract() {
    // REQ-AXO-125 — when soll_manager update fails (e.g. the target id
    // does not exist), the response must NOT echo raw SQL or DuckDB
    // internals to the LLM-visible content. The normalized contract
    // puts kind + category + recovery in `content.text` and keeps the
    // truncated raw error under `data.diagnostic_excerpt` for opt-in
    // inspection.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/tmp/fake"))
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "update",
                "entity": "requirement",
                "data": {
                    "id": "REQ-AXO-9999",
                    "status": "completed"
                }
            }
        })),
        id: Some(json!(125001)),
    };
    let response = server
        .handle_request(req)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "update on missing id must surface isError"
    );
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        !content.contains("INSERT INTO") && !content.contains("UPDATE soll"),
        "LLM-visible content must NOT contain raw SQL: {content}"
    );
    assert!(
        content.contains("update failed"),
        "content should describe the kind: {content}"
    );
    let data = response.get("data").expect("normalized error must include data");
    assert_eq!(data["kind"].as_str(), Some("update_failed"));
    assert!(
        data["category"].is_string(),
        "data.category must classify the error"
    );
    assert!(
        data["next_action"].is_string(),
        "data.next_action must give a recovery hint"
    );
    assert!(
        data["diagnostic_excerpt"].is_string(),
        "data.diagnostic_excerpt must hold the truncated raw error for opt-in inspection"
    );
}

// REQ-AXO-126 — soll_export disabled-branch is verified via the
// soll_export_enabled() function logic and production smoke-test;
// integration coverage is intentionally not added here because the
// AXON_SOLL_EXPORT_DISABLED env var would race with the other
// export tests under cargo's parallel runner. The cfg(test) branch
// keeps the existing tests unchanged: tests are enabled by default,
// production is disabled by default and opts in via
// AXON_SOLL_EXPORT_ENABLED.

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
    assert!(export_body.contains("## Entities: Vision") || export_body.contains("## Entities: Vision"));

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

## Entities: Vision
### VIS-AXO-001 - Test Vision
**Description:** Desc
**Status:** draft
**Meta:** `{"goal": "Goal", "source":"test"}`

## Entities: Pillar
### PIL-AXO-001 - Platform Core
**Description:** Keep the conceptual core stable
**Status:** accepted
**Meta:** `{}`

## Entities: Concept
### CPT-AXO-001 - Graph Truth
**Description:** Use a structural graph as source of truth
**Status:** accepted
**Meta:** `{"rationale": "Because the project needs stable intent"}`

## Entities: Milestone
### MIL-AXO-001 - First Usable State
**Description:** 
**Status:** in_progress
**Meta:** `{}`

## Entities: Requirement
### REQ-AXO-001 - Reliable Restore
**Description:** SOLL must be restorable from exports
**Status:** draft
**Meta:** `{"priority":"high"}`

## Entities: Decision
### DEC-AXO-001 - Merge Restore
**Description:** 
**Status:** accepted
**Meta:** `{"rationale": "Restoration should be merge-oriented and non-destructive"}`

## Entities: Validation
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
        content.contains("SOLL restore complete"),
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

    assert!(content.contains("violation"));
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

    assert!(content.contains("Duplicate titles"), "{content}");
    assert!(content.contains("Duplicate req"), "{content}");
    assert!(content.contains("Duplicate dec"), "{content}");
    assert!(
        content.contains("Requirements without criteria/evidence"),
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

    // REQ-AXO-001 has no acceptance_criteria in metadata, so validation
    // now flags it as uncovered even though it has a VERIFIES link.
    assert!(content.contains("1 minimal coherence violation(s)"), "{content}");
    assert!(content.contains("Requirements without criteria/evidence"), "{content}");
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
    // Updated 2026-05-01 (commit 0f1ec17): soll_validate now uses the
    // shared wrong_project_scope_response helper. The content text format
    // changed from "Canonical project error: ..." to
    // "Project `FSC` not found in registry for soll_validate. ...".
    // Assertions updated to the structured wrong_project_scope contract.
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
    assert!(content.contains("FSC"), "must echo rejected code: {content}");
    assert!(
        content.contains("not found in registry"),
        "must surface the registry-miss reason: {content}"
    );
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(result["data"]["rejected_project_code"].as_str(), Some("FSC"));
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

    assert!(content.contains("Invalid relations"), "{content}");
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
    assert!(content.contains("Result type"));
    assert!(content.contains("operational source"), "{content}");
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
    assert!(content.contains("Result type"), "{content}");
    assert!(content.contains("documentary"), "{content}");
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
        content.contains("degraded structural without file anchor"),
        "{content}"
    );
    assert!(content.contains("trigger_scan"), "{content}");
}

#[test]
fn test_axon_query_empty_fallback_returns_structured_recovery_without_empty_result_phrase() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "booking", "project": "AXO" }
        })),
        id: Some(json!(212)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("degraded structural without file anchor"),
        "{content}"
    );
    assert!(!content.contains("Aucun résultat trouvé."), "{content}");
    let data = result.get("data").unwrap();
    assert_eq!(data["result_count"].as_u64(), Some(0));
    assert_eq!(data["query_state"].as_str(), Some("structure_only_empty"));
    assert!(data["operator_guidance"].as_object().is_some());
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
    assert!(impact_text.contains("Derived Local Projection"));

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

    assert!(impact_text.contains("call graph is not yet available"));
    assert!(impact_text.contains("parse_batch"));
    let data = impact_result.get("data").unwrap();
    assert_eq!(data["impact_available"].as_bool(), Some(false));
    assert_eq!(
        data["next_action"]["kind"].as_str(),
        Some("wait_for_call_graph_truth")
    );
    assert_eq!(data["next_action"]["tool"].as_str(), Some("inspect"));
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

    assert!(content.contains("partial truth"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);
}

#[test]
fn test_axon_query_includes_compact_guidance_for_wrong_project_scope() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
    }
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
    assert!(result["data"]["operator_guidance"].as_object().is_some());
    assert!(result["data"]["operator_guidance"]["follow_up_tools"]
        .as_array()
        .is_some());
    assert!(result["data"]["next_action"]["tool"].as_str().is_some());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
    }
}

#[test]
fn test_axon_query_includes_compact_guidance_when_runtime_profile_blocks_tool() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var("AXON_MCP_GUIDANCE_AUTHORITATIVE", "1");
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
    // requires_indexed_runtime() now returns false for all tools,
    // so query is always available — verify a normal (non-error) response.
    assert!(
        !result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false),
        "query should not be blocked in this runtime profile"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_MCP_GUIDANCE_AUTHORITATIVE");
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

    assert!(content.contains("completeness"), "{}", content);
    assert!(content.contains("1/2"), "{}", content);
    assert!(content.contains("backlog"), "{}", content);
    assert!(content.contains("metadata_changed_scan"), "{}", content);
}

#[test]
fn test_axon_inspect_warns_when_symbol_is_degraded() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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

    assert!(content.contains("Symbol Inspection"), "{}", content);
    assert!(content.contains("partial truth"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);
    let data = result.get("data").unwrap();
    assert_eq!(data["symbol_found"].as_bool(), Some(true));
    assert!(data["operator_guidance"]["blocking_factors"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(
        data["operator_guidance"]["actionable_now"].as_bool(),
        Some(false)
    );

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

    assert!(content.contains("partial truth"), "{}", content);
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
    assert!(content.contains("partial truth"), "{}", content);
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
        assert!(content.contains("SOLL entity created"));
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
        restore_text.contains("SOLL restore complete"),
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
    server
        .graph_store
        .execute("INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES ('REV-AXO-001', 'tester', 'mcp', 'Context rebuild', 'committed', 10, 11)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at) VALUES ('REV-AXO-001', 'Node', 'REQ-AXO-001', 'update', '{}', '{}', 11)")
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
    let digest = data.get("operational_digest").expect("operational digest");
    let entity_counts = digest["entity_counts"].as_array().expect("entity counts");
    assert!(entity_counts.iter().any(|value| {
        value["entity_type"].as_str() == Some("Vision") && value["count"].as_u64() == Some(1)
    }));
    assert_eq!(
        digest["requirement_coverage_summary"]["total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        digest["topology_summary"]["orphan_requirement_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        digest["last_meaningful_revision"]["revision_id"].as_str(),
        Some("REV-AXO-001")
    );
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
    assert!(content.contains("not found"), "{content}");
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

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='SOLVES' AND source_id = 'DEC-AXO-001' AND target_id = 'REQ-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_create_can_attach_requirement_to_pillar() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Pillar', 'Protect structure', '', '{}')")
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
                    "title": "Attachable requirement",
                    "description": "Should auto-link to pillar",
                    "priority": "P1",
                    "attach_to": "PIL-AXO-001"
                }
            }
        })),
        id: Some(json!(41015)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("expected create data");
    let created_id = data["created_id"].as_str().expect("created_id");
    assert!(created_id.starts_with("REQ-AXO-"), "{created_id}");
    assert_eq!(data["attached"].as_bool(), Some(true));
    assert_eq!(data["attached_to"].as_str(), Some("PIL-AXO-001"));
    assert_eq!(data["applied_relation"].as_str(), Some("BELONGS_TO"));
    assert_eq!(data["attach_status"].as_str(), Some("attached"));
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id='{}' AND target_id='PIL-AXO-001' AND relation_type='BELONGS_TO'",
                created_id
            ))
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_create_attached_decision_requires_relation_hint_when_ambiguous() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Existing decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "AXO",
                    "title": "New linked decision",
                    "description": "Should need explicit relation",
                    "context": "Context",
                    "rationale": "Because",
                    "status": "accepted",
                    "attach_to": "DEC-AXO-001"
                }
            }
        })),
        id: Some(json!(41016)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("expected create data");
    let created_id = data["created_id"].as_str().expect("created_id");
    assert!(created_id.starts_with("DEC-AXO-"), "{created_id}");
    assert_eq!(data["attach_attempted"].as_bool(), Some(true));
    assert_eq!(data["attached"].as_bool(), Some(false));
    assert_eq!(data["attach_status"].as_str(), Some("needs_relation_hint"));
    let guidance = data["attach_guidance"]
        .as_object()
        .expect("attach guidance");
    let allowed_relations = guidance["allowed_relations"]
        .as_array()
        .expect("allowed relations")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowed_relations.contains(&"SUPERSEDES"));
    assert!(allowed_relations.contains(&"REFINES"));
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id='{}' AND target_id='DEC-AXO-001'",
                created_id
            ))
            .unwrap(),
        0
    );
}

#[test]
fn test_axon_soll_manager_create_attached_validation_rejects_invalid_target_kind_with_guidance() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Vision', 'North star', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "validation",
                "data": {
                    "project_code": "AXO",
                    "title": "Proof",
                    "method": "manual",
                    "result": "pending",
                    "attach_to": "VIS-AXO-001"
                }
            }
        })),
        id: Some(json!(41017)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("expected create data");
    assert_eq!(data["attached"].as_bool(), Some(false));
    assert!(
        matches!(data["attach_status"].as_str(), Some("invalid_target_kind") | Some("forbidden_relation")),
        "attach_status should indicate rejection: {:?}", data["attach_status"]
    );
    let guidance = data["attach_guidance"]
        .as_object()
        .expect("attach guidance");
    assert_eq!(guidance["pair_allowed"].as_bool(), Some(false));
    assert!(guidance["suggested_next_actions"].as_array().is_some());
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
    assert!(content.contains("Allowed"), "{content}");
    assert!(content.contains("SOLVES"), "{content}");
    assert!(content.contains("REFINES"), "{content}");
    let data = result
        .get("data")
        .expect("expected structured relation guidance");
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("REQ"));
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert_eq!(data["default_relation"].as_str(), Some("SOLVES"));
    let allowed_relations = data["allowed_relations"]
        .as_array()
        .expect("allowed_relations should be present")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowed_relations.contains(&"SOLVES"));
    assert!(allowed_relations.contains(&"REFINES"));
    assert!(data["suggested_next_actions"].as_array().is_some());
    assert!(data["canonical_examples"].as_array().is_some());
    assert!(data["recommended_incoming_links_to_target_kind"]
        .as_array()
        .is_some());
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

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='REFINES' AND source_id = 'DEC-AXO-001' AND target_id = 'REQ-AXO-001'")
            .unwrap(),
        1
    );
}

// REQ-AXO-115 — Concept→Pillar BELONGS_TO is the canonical edge for a
// Concept that formalizes a Pillar-level operational protocol
// (e.g. CPT-AXO-019 → PIL-AXO-003). Before this, the pair was forbidden
// and the dependency had to be expressed indirectly via REQ traversal.
#[test]
fn test_axon_soll_manager_link_concept_belongs_to_pillar() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'Operational protocol', 'Concept desc', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "concept",
                "data": {
                    "source_id": "CPT-AXO-001",
                    "target_id": "PIL-AXO-001"
                }
            }
        })),
        id: Some(json!(4106)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id='CPT-AXO-001' AND target_id='PIL-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_soll_relation_schema_resolves_pair_by_ids() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001"
                }
            })),
            id: Some(json!(4105)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("expected relation schema data");
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("REQ"));
    assert_eq!(data["default_relation"].as_str(), Some("SOLVES"));
    assert_eq!(data["projection"]["role"].as_str(), Some("primary"));
    assert_eq!(data["direction"].as_str(), Some("source_to_target"));
    assert_eq!(
        data["projection"]["parent_preference_rank"].as_u64(),
        Some(10)
    );
    assert!(data["allowed_target_kinds_from_source"]
        .as_array()
        .is_some());
    assert!(data["allowed_targets"].as_array().is_some());
    assert!(data["forbidden_targets"].as_array().is_some());
    assert_eq!(
        data["source_graph_role"].as_str(),
        Some("decision that solves, refines, or impacts implementation")
    );
    assert!(data["canonical_examples"].as_array().is_some());
}

#[test]
fn test_soll_relation_schema_unresolved_ids_return_guided_discovery_payload() {
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_id": "DEC-AXO-999",
                    "target_id": "REQ-AXO-001"
                }
            })),
            id: Some(json!(4106)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_ne!(
        response.get("isError").and_then(|value| value.as_bool()),
        Some(true)
    );
    let data = response
        .get("data")
        .expect("expected guided discovery payload");
    assert_eq!(data["resolved"].as_bool(), Some(false));
    assert_eq!(data["lookup_stage"].as_str(), Some("source_id"));
    assert!(data["suggested_next_actions"].as_array().is_some());
}

#[test]
fn test_soll_relation_schema_source_only_is_constructive_for_vision_and_pillar() {
    let server = create_test_server();

    let vision_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "VIS"
                }
            })),
            id: Some(json!(4107)),
        })
        .unwrap()
        .result
        .unwrap();
    let vision_data = vision_response.get("data").expect("vision guidance");
    assert_eq!(vision_data["source_kind"].as_str(), Some("VIS"));
    assert_eq!(
        vision_data["graph_role"].as_str(),
        Some("project north star")
    );
    assert_eq!(
        vision_data["kind_projection"]["root_eligible"].as_bool(),
        Some(true)
    );
    assert!(vision_data["incoming_from_source_kinds"]
        .as_array()
        .expect("incoming guidance")
        .iter()
        .any(|item| item["source_kind"].as_str() == Some("PIL")));

    let pillar_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "PIL"
                }
            })),
            id: Some(json!(4108)),
        })
        .unwrap()
        .result
        .unwrap();
    let pillar_data = pillar_response.get("data").expect("pillar guidance");
    assert_eq!(pillar_data["source_kind"].as_str(), Some("PIL"));
    assert_eq!(
        pillar_data["kind_projection"]["tree_order_rank"].as_u64(),
        Some(20)
    );
    assert!(pillar_data["allowed_targets"]
        .as_array()
        .expect("outgoing guidance")
        .iter()
        .any(|item| item["target_kind"].as_str() == Some("VIS")));
    assert!(pillar_data["incoming_from_source_kinds"]
        .as_array()
        .expect("incoming guidance")
        .iter()
        .any(|item| item["source_kind"].as_str() == Some("REQ")));
    assert!(pillar_data["allowed_targets"]
        .as_array()
        .expect("outgoing guidance")
        .iter()
        .any(|item| item["projection"]["role"].as_str() == Some("primary")));
    assert!(pillar_data["forbidden_targets"].as_array().is_some());
}

#[test]
fn test_soll_relation_schema_pair_suggests_reverse_direction_when_pair_is_forbidden() {
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "VIS",
                    "target_type": "PIL"
                }
            })),
            id: Some(json!(41081)),
        })
        .unwrap()
        .result
        .unwrap();
    let data = response.get("data").expect("forbidden pair guidance");
    assert_eq!(data["pair_allowed"].as_bool(), Some(false));
    assert_eq!(data["did_you_mean"]["source_kind"].as_str(), Some("PIL"));
    assert_eq!(data["did_you_mean"]["target_kind"].as_str(), Some("VIS"));
    assert_eq!(
        data["did_you_mean"]["relation_type"].as_str(),
        Some("EPITOMIZES")
    );
}

#[test]
fn test_axon_validate_soll_returns_structured_repair_guidance_and_completeness() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-200', 'Requirement', 'AXO', 'Lonely requirement', 'No links', 'draft', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-200', 'Validation', 'AXO', '', '', 'pending', '{\"method\":\"manual\"}')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_validate",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(4109)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("structured validation data");
    assert_eq!(data["status"].as_str(), Some("warn_soll_invariants"));
    assert_eq!(data["completeness"]["populated"].as_bool(), Some(true));
    assert_eq!(
        data["completeness"]["structurally_connected"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["completeness"]["evidence_ready"].as_bool(),
        Some(false)
    );
    let repair_guidance = data["repair_guidance"]
        .as_array()
        .expect("repair guidance array");
    assert!(repair_guidance
        .iter()
        .any(|entry| entry["category"].as_str() == Some("orphan_requirements")));
    assert!(repair_guidance
        .iter()
        .any(|entry| entry["category"].as_str() == Some("validations_without_verifies")));
}

#[test]
fn test_soll_attach_evidence_normalizes_entity_type_for_requirement_verification() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-210', 'Requirement', 'AXO', 'Normalized evidence', 'Uppercase entity type should still count', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-210",
                    "artifacts": [{
                        "artifact_type": "Symbol",
                        "artifact_ref": "normalized_requirement",
                        "confidence": 1.0
                    }]
                }
            })),
            id: Some(json!(4111)),
        })
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(4112)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["done"].as_u64(), Some(1));
    assert_eq!(data["partial"].as_u64(), Some(0));
    assert_eq!(data["missing"].as_u64(), Some(0));
}

#[test]
fn test_soll_attach_evidence_accepts_file_path_aliases_and_reports_rejections() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-211', 'Requirement', 'AXO', 'File evidence alias', 'File path aliases should attach and explain failures', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let valid_path = repo_root.join("README.md");

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-211",
                    "artifacts": [
                        {
                            "artifact_type": "document",
                            "path": valid_path.to_string_lossy().to_string(),
                            "confidence": 1.0
                        },
                        {
                            "artifact_type": "document",
                            "path": "docs/plans/does-not-exist.md"
                        }
                    ]
                }
            })),
            id: Some(json!(41121)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["attached"].as_u64(), Some(1));
    let accepted_schema = data["accepted_artifact_schema"].as_array().expect("schema");
    assert!(accepted_schema
        .iter()
        .any(|value| value.as_str() == Some("document")));
    let diagnostics = data["artifact_diagnostics"]
        .as_array()
        .expect("artifact diagnostics");
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["status"].as_str(), Some("attached"));
    assert_eq!(
        diagnostics[0]["normalized_artifact_type"].as_str(),
        Some("File")
    );
    assert_eq!(diagnostics[1]["status"].as_str(), Some("rejected"));
    let rejected_reasons = diagnostics[1]["reasons"]
        .as_array()
        .expect("rejected reasons");
    assert!(
        rejected_reasons
            .iter()
            .any(|reason| reason.as_str() == Some("path_not_resolvable")),
        "{result}"
    );
    // REQ-AXO-043 — partial result must surface a top-level status + next_action
    assert_eq!(data["status"].as_str(), Some("partial"));
    assert_eq!(data["total"].as_u64(), Some(2));
    assert!(data["next_action"].as_str().is_some());
    let problem_class = data["operator_guidance"]["problem_class"]
        .as_str()
        .expect("operator_guidance.problem_class");
    assert_eq!(problem_class, "partial_input_invalid");
}

#[test]
fn test_soll_attach_evidence_rejected_all_returns_recovery_contract() {
    // REQ-AXO-043 — when all artifacts are rejected, the LLM-visible content
    // must surface the failure mode AND data must include status, next_action,
    // and operator_guidance.problem_class so the client can recover without
    // re-reading per-artifact diagnostics.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-212a', 'Requirement', 'AXO', 'Reject-all contract', 'All-rejected attach must surface recovery', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-212a",
                    "artifacts": [
                        { "artifact_type": "document", "path": "docs/plans/does-not-exist-1.md" },
                        { "artifact_type": "document", "path": "docs/plans/does-not-exist-2.md" }
                    ]
                }
            })),
            id: Some(json!(41123)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    assert_eq!(data["attached"].as_u64(), Some(0));
    assert_eq!(data["total"].as_u64(), Some(2));
    assert!(
        data["next_action"].as_str().is_some(),
        "next_action must be set when all rejected: {result}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions array");
    assert!(!actions.is_empty(), "next_best_actions must be non-empty when rejected_all");

    // The LLM-visible content text must surface the failure (not just "Attached 0")
    let content_text = result["content"][0]["text"]
        .as_str()
        .expect("content text");
    assert!(
        content_text.contains("0 of 2") && content_text.contains("rejected"),
        "content must surface the rejection: {content_text}"
    );
}

#[test]
fn test_soll_verify_requirements_returns_missing_dimensions_and_actions() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-212', 'Requirement', 'AXO', 'Actionable verification', 'Verification should explain why this requirement is partial', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(41122)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["summary"]["total"].as_u64(), Some(1));
    let required_dimensions = result["data"]["completion_model"]["required_dimensions"]
        .as_array()
        .expect("required dimensions");
    assert!(required_dimensions.iter().any(|value| {
        value["canonical_key"].as_str() == Some("structured_acceptance_criteria")
    }));

    let details = result["data"]["details"].as_array().expect("details");
    let entry = details
        .iter()
        .find(|value| value["id"].as_str() == Some("REQ-AXO-212"))
        .expect("requirement entry");
    assert_eq!(entry["state"].as_str(), Some("partial"));
    assert_eq!(entry["completion_state"].as_str(), Some("partial"));
    assert!(entry["coverage_reason"]
        .as_str()
        .unwrap_or_default()
        .contains("supporting_evidence"));
    let missing_dimensions = entry["missing_dimensions"]
        .as_array()
        .expect("missing dimensions");
    assert!(missing_dimensions
        .iter()
        .any(|value| value.as_str() == Some("evidence")));
    assert!(missing_dimensions
        .iter()
        .any(|value| value.as_str() == Some("validation")));
    let next_actions = entry["suggested_next_actions"]
        .as_array()
        .expect("next actions");
    assert!(next_actions.iter().any(|value| value
        .as_str()
        .unwrap_or_default()
        .contains("soll_attach_evidence")));
    let missing_dimensions_detailed = entry["missing_dimensions_detailed"]
        .as_array()
        .expect("missing dimensions detailed");
    assert!(missing_dimensions_detailed
        .iter()
        .any(|value| { value["canonical_key"].as_str() == Some("supporting_evidence") }));
    let next_actions_detailed = entry["next_actions_detailed"]
        .as_array()
        .expect("next actions detailed");
    assert!(next_actions_detailed.iter().any(|value| {
        value["dimension"].as_str() == Some("qualifying_validation_edge")
            && value["mutation_class"].as_str() == Some("link_validation")
    }));
    let requirements = result["data"]["requirements"]
        .as_array()
        .expect("requirements alias");
    assert_eq!(requirements.len(), details.len());
}

#[test]
fn test_anomalies_downgrades_noncanonical_intent_gaps_when_soll_baseline_is_complete() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Core pillar', '', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Healthy requirement', '', 'current', '{\"acceptance_criteria\":\"done\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Healthy decision', '', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', 'Healthy validation', '', 'passed', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'PIL-AXO-001', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('VAL-AXO-001', 'REQ-AXO-001', 'VERIFIES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-001', 'requirement', 'REQ-AXO-001', 'Symbol', 'healthy_requirement', 1.0, 0)")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "anomalies",
                "arguments": { "project": "AXO", "mode": "brief" }
            })),
            id: Some(json!(4113)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(
        data["summary"]["concept_completeness"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["summary"]["implementation_completeness"].as_bool(),
        Some(true)
    );
    assert_eq!(data["summary"]["orphan_intent_count"].as_u64(), Some(0));
    assert!(
        data["summary"]["heuristic_intent_gap_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
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
        assert!(content.contains("SOLL entity created"));
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
        assert!(content.contains("Link created"));
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
        restore_text.contains("SOLL restore complete"),
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
    assert!(content_bad.contains("Remediation"));

    // 2. Simulate a good commit (modifies src/mcp/ AND legacy tests.rs)
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
    assert!(content_good.contains("Validation passed"));

    // 3. Modular MCP tests must also satisfy the legacy `tests.rs` rule.
    let req_modular_test = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": [
                    "src/axon-core/src/mcp.rs",
                    "src/axon-core/src/mcp/tests/guidance_contract.rs"
                ],
                "message": "fix: update mcp guidance tests",
                "dry_run": true
            }
        },
        "id": 3
    });

    let res_modular_test = server
        .handle_request(serde_json::from_value(req_modular_test).unwrap())
        .unwrap()
        .result
        .unwrap();
    assert!(!res_modular_test
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
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
    assert!(content.contains("Available global rules"));
    assert!(content.contains("Server-assigned project code: `BKS`"));
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

// REQ-AXO-119 — axon_init_project must return a stable kickoff bundle
// (kickoff_prompt, methodology_summary, entry_points, active_handoff)
// on every call so an LLM with only Axon MCP access can onboard
// itself in one round-trip without having to re-discover the
// bootstrap protocol or the project's reading order.

#[test]
fn test_axon_init_project_returns_kickoff_bundle_for_first_init() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/home/dstadel/projects/BookingSystem" }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = result["data"]["kickoff_bundle"].as_object().expect(
        "first init must return a kickoff_bundle in data",
    );
    assert!(bundle.contains_key("kickoff_prompt"));
    assert!(bundle.contains_key("methodology_summary"));
    assert!(bundle.contains_key("entry_points"));
    assert!(bundle.contains_key("active_handoff"));
    let entry_points = bundle["entry_points"]
        .as_array()
        .expect("entry_points must be an array");
    assert!(
        entry_points.len() >= 8,
        "entry_points must list the cold-start reading order; got {} steps",
        entry_points.len()
    );
    // Verify the four canonical kinds are represented.
    let kinds: std::collections::HashSet<&str> = entry_points
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()))
        .collect();
    assert!(kinds.contains("file"), "entry_points must include `file` steps: {kinds:?}");
    assert!(kinds.contains("mcp"), "entry_points must include `mcp` steps: {kinds:?}");
    assert!(kinds.contains("cypher"), "entry_points must include `cypher` steps: {kinds:?}");

    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("Kickoff bundle"),
        "content must point to the bundle: {content}"
    );
}

#[test]
fn test_axon_init_project_returns_identical_bundle_on_re_init() {
    let server = create_test_server();
    let args = serde_json::json!({ "project_path": "/home/dstadel/projects/BookingSystem" });
    let make_req = |id: u64| serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": "axon_init_project", "arguments": args },
        "id": id
    });
    let first = server
        .handle_request(serde_json::from_value(make_req(1)).unwrap())
        .unwrap()
        .result
        .unwrap();
    let second = server
        .handle_request(serde_json::from_value(make_req(2)).unwrap())
        .unwrap()
        .result
        .unwrap();
    // Both calls must return the same project_code.
    assert_eq!(first["data"]["project_code"], second["data"]["project_code"]);
    // The kickoff bundle must be present and equivalent on both calls.
    let b1 = &first["data"]["kickoff_bundle"];
    let b2 = &second["data"]["kickoff_bundle"];
    assert!(b1.is_object() && b2.is_object());
    assert_eq!(b1["kickoff_prompt"], b2["kickoff_prompt"]);
    assert_eq!(
        b1["methodology_summary"],
        b2["methodology_summary"]
    );
    assert_eq!(b1["entry_points"], b2["entry_points"]);
    assert_eq!(b1["active_handoff"], b2["active_handoff"]);
}

#[test]
fn test_axon_init_project_bundle_active_handoff_null_when_no_working_notes() {
    let server = create_test_server();
    // /tmp/non-existent-axon-project has no docs/working-notes directory.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/tmp/non-existent-axon-project-for-bundle-test" }
        },
        "id": 119
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = &result["data"]["kickoff_bundle"];
    assert!(
        bundle["active_handoff"].is_null(),
        "active_handoff must be null when docs/working-notes is absent: {bundle}"
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
    assert!(content.contains("is server-assigned"), "{content}");
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
    assert!(content.contains("Inheritance applied"));

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

// REQ-AXO-043 — axon_apply_guidelines must surface a recovery contract
// when the call cannot produce useful output (empty input or all-unknown
// global rule IDs). The previous behaviour silently returned
// "Inheritance applied. New local rules created: []", misleading the LLM
// into thinking work happened.

#[test]
fn test_axon_apply_guidelines_rejects_empty_accepted_list() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": []
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "empty accepted_global_rule_ids must surface isError=true; result={result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        content.contains("at least one canonical Guideline ID"),
        "{content}"
    );
    let data = result.get("data").unwrap();
    assert_eq!(data.get("empty_input").and_then(|v| v.as_bool()), Some(true));
    assert!(data.get("recovery_hint").is_some());
    assert_eq!(data.get("applied").unwrap().as_array().unwrap().len(), 0);
    assert_eq!(
        data.get("unknown_global_rule_ids")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn test_axon_apply_guidelines_rejects_all_unknown_rule_ids() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-NONEXISTENT", "GUI-NOPE-999"]
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "all-unknown IDs must surface isError=true; result={result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("No rules applied"), "{content}");
    assert!(content.contains("GUI-PRO-NONEXISTENT"), "{content}");
    let data = result.get("data").unwrap();
    let unknowns = data
        .get("unknown_global_rule_ids")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(unknowns.len(), 2);
    assert!(unknowns
        .iter()
        .any(|v| v.as_str() == Some("GUI-PRO-NONEXISTENT")));
    assert!(unknowns.iter().any(|v| v.as_str() == Some("GUI-NOPE-999")));
}

#[test]
fn test_axon_apply_guidelines_partial_success_surfaces_unknown() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-001", "GUI-PRO-NONEXISTENT"]
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    // Partial success is NOT an error — the call produced useful output
    // for the known IDs and reported unknowns alongside.
    assert!(
        result.get("isError").is_none()
            || result.get("isError").and_then(|v| v.as_bool()) == Some(false),
        "partial success should not flag isError; result={result:?}"
    );
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("applied").unwrap().as_array().unwrap().len(),
        1,
        "exactly one applied"
    );
    let unknowns = data
        .get("unknown_global_rule_ids")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(unknowns.len(), 1);
    assert_eq!(unknowns[0].as_str(), Some("GUI-PRO-NONEXISTENT"));
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
        content.contains("Commit succeeded") || content.contains("Commit failed"),
        "{}",
        content
    );
    assert!(content.contains("Exported to"), "{}", content);
    if let Some(export_path) = content
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .map(str::trim)
    {
        let _ = std::fs::remove_file(export_path);
    }
}

#[test]
fn test_soll_generate_docs_creates_navigable_site_and_manifest() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Human-readable docs', 'Readable docs for humans', 'current', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Generate derived site', 'Decision desc', 'accepted', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('PIL-AXO-001', 'VIS-AXO-001', 'EPITOMIZES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'PIL-AXO-001', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9910)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["pages_total"].as_u64(), Some(7));
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Generated navigable SOLL docs"));

    let index_path = out.path().join("index.html");
    let node_path = out.path().join("nodes/REQ-AXO-001.html");
    let subtree_path = out.path().join("subtrees/VIS-AXO-001.html");
    let manifest_path = out.path().join("_manifest.json");

    assert!(index_path.is_file());
    assert!(node_path.is_file());
    assert!(subtree_path.is_file());
    assert!(manifest_path.is_file());

    let index_html = std::fs::read_to_string(index_path).unwrap();
    assert!(index_html.contains("mermaid.initialize"));
    assert!(index_html.contains("PIL-AXO-001"));
    assert!(index_html.contains("toggle-left"));
    assert!(index_html.contains("toggle-right"));
    assert!(index_html.contains("Project Tree"));
    assert!(index_html.contains("Vision Children"));
    assert!(index_html.contains("derived / non-canonical"));
    assert!(index_html.contains("All Node Pages"));
    assert!(index_html.contains("nodes/REQ-AXO-001.html"));
    assert!(index_html.contains("flowchart LR"));

    let node_html = std::fs::read_to_string(node_path).unwrap();
    assert!(node_html.contains("Readable docs for humans"));
    assert!(node_html.contains("Incoming Neighbors"));
    assert!(node_html.contains("Relations"));
    assert!(node_html.contains("Primary Hierarchy Parents"));
    assert!(node_html.contains("Primary Hierarchy Children"));
    assert!(node_html.contains("Containing Subtrees"));
    assert!(node_html.contains("Primary Parent Node Pages"));
    assert!(node_html.contains("Operator Relation Diagnostics"));
    assert!(node_html.contains("boundary: canonical"));
    assert!(node_html.contains("toggle-left"));
    assert!(node_html.contains("toggle-right"));
    assert!(node_html
        .contains("Generated node page combining hierarchy, local context, and relation diagnostics"));

    let subtree_html = std::fs::read_to_string(subtree_path).unwrap();
    assert!(subtree_html.contains("All Nodes In This Subtree"));
    assert!(subtree_html.contains("../nodes/REQ-AXO-001.html"));
    assert!(subtree_html.contains("derived / non-canonical"));
    assert!(subtree_html.contains("Subtree Inclusion Reasons"));
    assert!(subtree_html.contains("Included because this node is the subtree root"));
    assert!(subtree_html.contains("Included by reverse reachability toward root"));

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["project_code"].as_str(), Some("AXO"));
    assert_eq!(manifest["pages_total"].as_u64(), Some(7));
}

#[test]
fn test_soll_generate_docs_keeps_unattached_nodes_out_of_primary_project_roots() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-999', 'Decision', 'AXO', 'Detached decision', 'No hierarchy parent', 'draft', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9918)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(result["data"]["pages_total"].as_u64().unwrap_or(0) >= 3);

    let index_html = std::fs::read_to_string(out.path().join("index.html")).unwrap();
    assert!(index_html.contains("Unattached Node Pages"));
    assert!(index_html.contains("nodes/DEC-AXO-999.html"));
    assert!(!index_html.contains("mermaid-id-DEC-AXO-999"));
}

#[test]
fn test_soll_generate_docs_is_incremental_when_content_is_unchanged() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('PIL-AXO-001', 'VIS-AXO-001', 'EPITOMIZES')")
        .unwrap();

    let call = |server: &McpServer| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_generate_docs",
                    "arguments": {
                        "project_code": "AXO",
                        "output_dir": out.path().to_string_lossy().to_string()
                    }
                })),
                id: Some(json!(9911)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let first = call(&server);
    assert!(first["data"]["pages_written"].as_u64().unwrap_or(0) > 0);

    let second = call(&server);
    assert_eq!(second["data"]["pages_written"].as_u64(), Some(0));
    assert!(second["data"]["pages_unchanged"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn test_soll_generate_docs_with_site_root_builds_project_and_global_root() {
    let server = create_test_server();
    let site_root = tempdir().unwrap();

    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/dstadel/projects/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry(
            "NTO",
            Some("nutri-opti"),
            Some("/home/dstadel/projects/nutri-opti"),
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "site_root_dir": site_root.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9912)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["refresh_mode"].as_str(), Some("full"));
    assert!(site_root.path().join("index.html").is_file());
    assert!(site_root.path().join("_root_manifest.json").is_file());
    assert!(site_root.path().join("AXO/index.html").is_file());

    let root_html = std::fs::read_to_string(site_root.path().join("index.html")).unwrap();
    assert!(root_html.contains("SOLL Derived Projects"));
    assert!(root_html.contains("AXO/index.html"));
    assert!(root_html.contains("NTO"));
    assert!(root_html.contains("GLO"));
}

#[test]
fn test_sync_mutation_auto_refreshes_derived_docs_and_root() {
    let site_root = tempdir().unwrap();
    let _site_root = SollSiteRootGuard::new(site_root.path());
    let server = create_test_server();

    let init_result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": {
                    "project_path": "/tmp/nutri-opti",
                    "project_name": "nutri-opti",
                    "project_code": "NTO"
                }
            })),
            id: Some(json!(9913)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        init_result["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    assert!(site_root.path().join("NTO/index.html").is_file());
    assert!(site_root.path().join("index.html").is_file());

    let create_result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "vision",
                    "data": {
                        "project_code": "NTO",
                        "title": "Preventive nutrition platform",
                        "description": "Greenfield vision"
                    }
                }
            })),
            id: Some(json!(9914)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        create_result["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    let project_html = std::fs::read_to_string(site_root.path().join("NTO/index.html")).unwrap();
    assert!(project_html.contains("Preventive nutrition platform"));
}

#[test]
fn test_soll_generate_docs_deletes_obsolete_project_pages_from_manifest() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Human-readable docs', 'Readable docs for humans', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'VIS-AXO-001', 'BELONGS_TO')")
        .unwrap();

    let call = |server: &McpServer| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_generate_docs",
                    "arguments": {
                        "project_code": "AXO",
                        "output_dir": out.path().to_string_lossy().to_string()
                    }
                })),
                id: Some(json!(9915)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let first = call(&server);
    assert!(first["data"]["pages_total"].as_u64().unwrap_or(0) >= 3);
    assert!(out.path().join("nodes/REQ-AXO-001.html").is_file());

    server
        .graph_store
        .execute(
            "DELETE FROM soll.Edge WHERE source_id = 'REQ-AXO-001' AND target_id = 'VIS-AXO-001'",
        )
        .unwrap();
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id = 'REQ-AXO-001'")
        .unwrap();

    let second = call(&server);
    assert_eq!(second["data"]["refresh_mode"].as_str(), Some("incremental"));
    assert_eq!(second["data"]["pages_deleted"].as_u64(), Some(1));
    assert!(!out.path().join("nodes/REQ-AXO-001.html").exists());
}

#[test]
fn test_soll_generate_docs_for_project_only_returns_null_root_fields() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9916)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(result["data"]["site_root"].is_null());
    assert!(result["data"]["root_manifest_path"].is_null());
    assert!(result["data"]["root_index_path"].is_null());
}

#[test]
fn test_soll_generate_docs_forces_full_rebuild_when_manifest_is_incompatible() {
    let server = create_test_server();
    let out = tempdir().unwrap();
    std::fs::create_dir_all(out.path().join("nodes")).unwrap();
    std::fs::write(out.path().join("nodes/STALE-AXO-001.html"), "stale").unwrap();
    std::fs::write(
        out.path().join("_manifest.json"),
        r#"{"generator_version":"legacy","pages":[{"path":"nodes/STALE-AXO-001.html"}]}"#,
    )
    .unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9917)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["refresh_mode"].as_str(), Some("full"));
    assert!(!out.path().join("nodes/STALE-AXO-001.html").exists());
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
