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
fn test_axon_soll_apply_plan_under_duckdb_uses_explicit_transaction() {
    // REQ-AXO-254: verifies the test harness backend is DuckDB so the
    // BEGIN/COMMIT pairing in `axon_soll_commit_revision` and
    // `axon_soll_rollback_revision` runs through the DuckDB single-
    // connection path. Under PG the FFI deadpool fresh-conn-per-call
    // breaks the pairing, leaving conn A "idle in transaction" with
    // row locks held — the patched code branches on
    // `is_postgres_backend()` to skip the wrapping transaction.
    let server = create_test_server();
    assert!(
        !server.graph_store.is_postgres_backend(),
        "test harness MUST run under DuckDB so the BEGIN/COMMIT branch covers the txn-aware path"
    );

    // Smoke that apply_plan still commits when the explicit txn path
    // is exercised (sanity check post-patch).
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": false,
                "author": "test-req-axo-254",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-axo-254-txn-skip-coverage",
                        "title": "REQ-AXO-254 txn skip coverage",
                        "description": "Smoke that explicit BEGIN/COMMIT works on DuckDB harness",
                        "priority": "P3",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(20254)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("SOLL revision committed"), "{content}");
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
fn test_init_project_missing_path_returns_parameter_repair() {
    // REQ-AXO-147 slice 4 — axon_init_project rejection paths surface
    // canonical data.parameter_repair so a fresh LLM that calls without
    // arguments can fix the input in one round-trip.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "init_project",
                "arguments": {}
            })),
            id: Some(json!(91474)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data");
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("project_path"));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"help"),
        "follow_up_tools must include `help`: {names:?}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("project") && hint.contains("absolute"),
        "hint must guide toward absolute project path: {hint}"
    );
}

#[test]
fn test_soll_manager_unknown_entity_returns_parameter_repair() {
    // REQ-AXO-147 slice 3 — soll_manager rejection paths now surface
    // the canonical data.parameter_repair shape so the LLM can fix
    // input fields in one round-trip.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "wat-not-an-entity",
                    "data": { "project_code": "AXO", "title": "x", "description": "x" }
                }
            })),
            id: Some(json!(91473)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("entity"));
    assert_eq!(
        repair["supplied_value"].as_str(),
        Some("wat-not-an-entity")
    );
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for kind in ["requirement", "decision", "concept", "guideline", "vision"] {
        assert!(
            names.contains(&kind),
            "accepted_values must include `{kind}`: {names:?}"
        );
    }
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("entity"),
        "hint must mention entity: {hint}"
    );
}

#[test]
fn test_soll_manager_create_invalid_status_returns_parameter_repair() {
    // REQ-AXO-325 — server-side status validation. Reject hors-vocabulaire
    // BEFORE the DB CHECK constraint surfaces a cryptic error. Mirror the
    // canonical parameter_repair envelope used elsewhere (entity / project_code
    // / relation_type / target_id). Canonical vocabulary = DEC-PRO-100 (5 values).
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "requirement",
                    "data": {
                        "project_code": "AXO",
                        "title": "REQ-AXO-325 contract test",
                        "description": "status=completed must be rejected with normalization_hint=delivered",
                        "status": "completed"
                    }
                }
            })),
            id: Some(json!(91475)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["category"].as_str(), Some("status"));
    assert_eq!(repair["invalid_field"].as_str(), Some("data.status"));
    assert_eq!(repair["supplied_value"].as_str(), Some("completed"));
    assert_eq!(repair["normalization_hint"].as_str(), Some("delivered"));
    assert_eq!(repair["canonical_source"].as_str(), Some("DEC-PRO-100"));
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for canonical in ["current", "planned", "delivered", "superseded", "rejected"] {
        assert!(
            names.contains(&canonical),
            "accepted_values must include `{canonical}`: {names:?}"
        );
    }
    let example = data["example_valid_call"].clone();
    assert_eq!(example["action"].as_str(), Some("create"));
    assert_eq!(example["entity"].as_str(), Some("requirement"));
    assert_eq!(
        example["data"]["status"].as_str(),
        Some("delivered"),
        "example_valid_call must use the normalization_hint"
    );
}

#[test]
fn test_soll_manager_update_invalid_status_returns_parameter_repair() {
    // REQ-AXO-325 — same vocabulary enforcement on update path.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-91476', 'Requirement', 'AXO', 'REQ-AXO-325 update test', 'fixture for status validation on update path', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "update",
                    "entity": "requirement",
                    "data": {
                        "id": "REQ-AXO-91476",
                        "status": "accepted"
                    }
                }
            })),
            id: Some(json!(91476)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["category"].as_str(), Some("status"));
    assert_eq!(repair["supplied_value"].as_str(), Some("accepted"));
    assert_eq!(repair["normalization_hint"].as_str(), Some("current"));
}

#[test]
fn test_entrench_nuance_cross_project_returns_parameter_repair() {
    // REQ-AXO-147 slice 2 — cross-project target_ids rejection now
    // surfaces structured `data.parameter_repair` (status,
    // expected_project_code, supplied_target_ids, invalid_target_ids,
    // follow_up_tools, hint) so the LLM can filter the bad ids in one
    // round-trip.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("NTO", Some("Nutri"), Some("/tmp/nutri"))
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-NTO-901', 'Requirement', 'NTO', 'Cross-project Req', 'Cross-project entrench rejection contract', 'current', '{}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "AXO",
                    "statement": "Cross-project rejection contract",
                    "confirm": true,
                    "target_ids": ["REQ-NTO-901"]
                }
            })),
            id: Some(json!(91471)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("target_ids"));
    assert_eq!(repair["stage"].as_str(), Some("cross_project_check"));
    assert_eq!(repair["expected_project_code"].as_str(), Some("AXO"));
    let invalid = repair["invalid_target_ids"]
        .as_array()
        .expect("invalid_target_ids array");
    let invalid_names: Vec<&str> = invalid.iter().filter_map(|v| v.as_str()).collect();
    assert!(invalid_names.contains(&"REQ-NTO-901"));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"infer_soll_mutation"),
        "follow_up_tools must include infer_soll_mutation: {names:?}"
    );
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

// REQ-AXO-126 — soll_export is snapshot-per-release: the automatic
// hook on `axon_commit_work` was removed and the MCP tool stays
// available on demand (called once per live promotion by
// scripts/release/promote_live_safe.sh, plus ad-hoc operator calls).
// No env-var gate; the per-call rate is now bounded by promotion
// frequency. This test exercises the on-demand path; commit-work
// integration tests below assert that no auto-export occurs.

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
fn test_axon_validate_soll_exempts_archived_requirements_from_uncovered_list() {
    // REQ-AXO-245: archived Requirements are explicitly closed and must not
    // appear in the "Requirements without criteria/evidence" list, otherwise
    // operators are forced to backfill criteria on already-closed work and the
    // violation count cannot reach zero by curation alone.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-900', 'Requirement', 'AXO', 'Active uncovered', 'No criteria', 'draft', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-901', 'Requirement', 'AXO', 'Closed and archived', 'No criteria, but archived', 'archived', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "AXO" }
        })),
        id: Some(json!(3245)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("Requirements without criteria/evidence"),
        "{content}"
    );
    assert!(content.contains("REQ-AXO-900"), "{content}");
    assert!(
        !content.contains("REQ-AXO-901"),
        "archived requirement leaked into uncovered list: {content}"
    );
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

// REQ-AXO-088 — `reserve_budget` did not match `reserve_memory_budget`
// because `_` was missing from the wildcard separator set: the query
// stayed as a literal token instead of becoming the LIKE pattern
// `reserve%budget`. Adding `_` to the wildcard replacement set turns
// underscore-separated query fragments back into fuzzy matches that hit
// the corresponding underscore-separated symbol names.
#[test]
fn test_axon_query_underscore_fragment_matches_underscore_symbol() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code) VALUES ('src/axon-core/src/queue.rs', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::reserve_memory_budget', 'reserve_memory_budget', 'function', false, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/axon-core/src/queue.rs', 'axon::reserve_memory_budget', 'AXO')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "reserve_budget", "project": "AXO" }
        })),
        id: Some(json!(881)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("reserve_memory_budget"),
        "fuzzy underscore-aware match must surface the existing symbol: {content}"
    );
    assert!(
        !content.contains("No exact structural match resolved"),
        "must not give up with the empty-result phrase: {content}"
    );
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

// REQ-AXO-043 / REQ-AXO-125 — the link path sanitizes raw DuckDB writer
// errors out of the LLM-visible `content.text` while preserving non-SQL
// errors verbatim and keeping the existing flat `data.relation_guidance`
// shape that callers depend on. The DEC→DEC pair is the cleanest way to
// trigger a cardinality conflict (allow_multiple_types=false with
// `allowed=["SUPERSEDES","REFINES"]`); that conflict is NOT a writer
// error so its readable text must pass through, and `data` must keep
// `pair_allowed`/`source_kind`/`canonical_examples`.
#[test]
fn test_axon_soll_manager_link_cardinality_conflict_preserves_text_and_data_shape() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'D1', '', 'accepted', '{\"context\":\"c\",\"rationale\":\"r\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'D2', '', 'accepted', '{\"context\":\"c\",\"rationale\":\"r\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES ('DEC-AXO-001', 'DEC-AXO-002', 'SUPERSEDES', '{}')")
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
                    "target_id": "DEC-AXO-002",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(43001)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();

    // Non-SQL error text passes through with the readable cardinality message.
    assert!(content.contains("Cardinality conflict"), "{content}");
    // No raw SQL must leak even on the readable-error path.
    assert!(
        !content.contains("INSERT INTO") && !content.contains("Writer Error"),
        "LLM-visible content must NOT contain raw SQL: {content}"
    );
    // Existing relation_guidance shape preserved (flat fields under data).
    let data = response.get("data").expect("relation_guidance must be attached");
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("DEC"));
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert!(data["allowed_relations"].as_array().is_some());
    assert!(data["canonical_examples"].as_array().is_some());
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
fn test_axon_soll_manager_link_decision_refines_concept() {
    // REQ-AXO-188 #1+#2: DEC -> CPT must accept REFINES (and SUPERSEDES) so
    // architecture-state Concepts can record which Decision governs or
    // retires them. Without this canonical edge, the linkage stays text-only
    // inside the description body and is not queryable via the graph.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Architecture decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'Architecture-state CPT', 'Concept desc', '', '{\"tags\":\"architecture-state\"}')")
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
                    "target_id": "CPT-AXO-001",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(4188)),
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
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='REFINES' AND source_id='DEC-AXO-001' AND target_id='CPT-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_link_decision_supersedes_concept() {
    // REQ-AXO-188 #1+#2: DEC -> CPT also accepts SUPERSEDES for the case
    // where a decision retires or wholly replaces an architecture concept.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'Replacement decision', '', 'accepted', '{\"context\":\"ctx\",\"rationale\":\"why\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-002', 'Concept', 'AXO', 'Retired concept', 'desc', '', '{}')")
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
                    "source_id": "DEC-AXO-002",
                    "target_id": "CPT-AXO-002",
                    "relation_type": "SUPERSEDES"
                }
            }
        })),
        id: Some(json!(4189)),
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
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='SUPERSEDES' AND source_id='DEC-AXO-002' AND target_id='CPT-AXO-002'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_link_same_type_supersedes_allowed() {
    // REQ-AXO-326 — PIL/GUI/REQ/CPT same-type SUPERSEDES now accepted so the
    // graph carries canonical replacement edges (previously blocked by policy
    // gap, forcing metadata.superseded_by workaround which is not graph-native).
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-101', 'Pillar', 'AXO', 'Old Pillar', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-102', 'Pillar', 'AXO', 'New Pillar', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-AXO-101', 'Guideline', 'AXO', 'Old Guideline', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-AXO-102', 'Guideline', 'AXO', 'New Guideline', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-101', 'Requirement', 'AXO', 'Old Req', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-102', 'Requirement', 'AXO', 'New Req', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-101', 'Concept', 'AXO', 'Old CPT', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-102', 'Concept', 'AXO', 'New CPT', '', 'current', '{}')").unwrap();

    for (entity, source, target) in [
        ("pillar", "PIL-AXO-101", "PIL-AXO-102"),
        ("guideline", "GUI-AXO-101", "GUI-AXO-102"),
        ("requirement", "REQ-AXO-101", "REQ-AXO-102"),
        ("concept", "CPT-AXO-101", "CPT-AXO-102"),
    ] {
        let response = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": {
                        "action": "link",
                        "entity": entity,
                        "data": {
                            "source_id": source,
                            "target_id": target,
                            "relation_type": "SUPERSEDES"
                        }
                    }
                })),
                id: Some(json!(91577)),
            })
            .unwrap();
        let result = response.result.expect("expected SOLL link result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            content.contains("Link created"),
            "{entity} {source}->{target}: {content}"
        );
        assert_eq!(
            server
                .graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM soll.Edge WHERE relation_type='SUPERSEDES' AND source_id='{source}' AND target_id='{target}'"
                ))
                .unwrap(),
            1
        );
    }
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
fn test_soll_attach_evidence_parameter_repair_per_kind_hint_for_missing_artifact_ref() {
    // REQ-AXO-139 slice — when an artifact is rejected because `artifact_ref`
    // (and its aliases) are absent, surface a structured `parameter_repair`
    // payload with a per-kind `required_field_hint` so the LLM can fix the
    // input in one round-trip.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-213a', 'Requirement', 'AXO', 'Per-kind hint contract', 'Missing artifact_ref must surface per-kind hint', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-213a",
                    "artifacts": [
                        { "artifact_type": "symbol" }
                    ]
                }
            })),
            id: Some(json!(41139)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifact_ref"));
    assert_eq!(repair["rejected_artifact_kind"].as_str(), Some("Symbol"));
    assert_eq!(
        repair["primary_reason"].as_str(),
        Some("missing_artifact_ref")
    );
    let aliases = repair["accepted_aliases"]
        .as_array()
        .expect("accepted_aliases array");
    let alias_names: Vec<&str> = aliases.iter().filter_map(|v| v.as_str()).collect();
    assert!(alias_names.contains(&"artifact_ref"));
    assert!(alias_names.contains(&"path"));
    assert!(alias_names.contains(&"file_path"));
    assert!(alias_names.contains(&"uri"));
    let hint = repair["required_field_hint"]
        .as_str()
        .expect("required_field_hint string");
    assert!(
        hint.contains("symbol id"),
        "Symbol-kind hint must reference symbol id: {hint}"
    );
    let top_hint = repair["hint"].as_str().expect("hint string");
    assert!(
        top_hint.contains("(Symbol)"),
        "top-level hint must mention rejected kind: {top_hint}"
    );
}

#[test]
fn test_soll_attach_evidence_parameter_repair_no_artifacts() {
    // REQ-AXO-139 slice — empty `artifacts` array surfaces a generic
    // parameter_repair pointing at the `artifacts` field.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-213b', 'Requirement', 'AXO', 'Empty artifacts contract', 'Empty array must surface parameter_repair', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-213b",
                    "artifacts": []
                }
            })),
            id: Some(json!(41140)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("no_artifacts"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifacts"));
    assert!(repair["accepted_aliases"].is_array());
    assert!(repair["accepted_artifact_schema"].is_array());
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("artifact_ref"),
        "no_artifacts hint must mention artifact_ref alias: {hint}"
    );
}

#[test]
fn test_soll_attach_evidence_parameter_repair_artifact_type_not_allowed() {
    // REQ-AXO-139 slice — when artifact_type isn't in the entity's
    // accepted_artifact_schema, parameter_repair surfaces invalid_field
    // = `artifact_type` plus the supplied + accepted lists for one-shot fix.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-913', 'Concept', 'AXO', 'Schema-not-allowed contract', 'Concept does not accept Test artifacts', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Concept",
                    "entity_id": "CPT-AXO-913",
                    "artifacts": [
                        // Concept's accepted_artifact_schema = [document, file, symbol, rationale];
                        // `test` is not allowed.
                        { "artifact_type": "test", "artifact_ref": "module::tests::dummy" }
                    ]
                }
            })),
            id: Some(json!(41141)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifact_type"));
    assert_eq!(repair["supplied_artifact_type"].as_str(), Some("test"));
    let accepted = repair["accepted_artifact_schema"]
        .as_array()
        .expect("accepted_artifact_schema array");
    let accepted_names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    assert!(accepted_names.contains(&"document"));
    assert!(accepted_names.contains(&"rationale"));
    assert!(!accepted_names.contains(&"test"));
}

#[test]
fn test_soll_verify_requirements_terminal_status_counts_as_done() {
    // REQ-AXO-136: status=`completed` and status=`delivered` are terminal —
    // done by definition. The verifier must not flag missing dimensions and
    // must increment the `done` count when an LLM closes a REQ via
    // `soll_manager update status=completed`.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-501', 'Requirement', 'AXO', 'Closed work no metadata', '', 'completed', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-502', 'Requirement', 'AXO', 'Closed work delivered alias', '', 'delivered', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(45136)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["summary"]["done"].as_u64(), Some(2),
        "both terminal-status REQs must count as done: {:?}", result["data"]);
    assert_eq!(result["data"]["summary"]["partial"].as_u64(), Some(0),
        "terminal REQs must not be partial: {:?}", result["data"]);
    assert_eq!(result["data"]["summary"]["missing"].as_u64(), Some(0),
        "terminal REQs must not be missing: {:?}", result["data"]);

    let details = result["data"]["details"].as_array().expect("details");
    let entry_501 = details.iter()
        .find(|v| v["id"].as_str() == Some("REQ-AXO-501"))
        .expect("REQ-AXO-501 entry");
    assert_eq!(entry_501["state"].as_str(), Some("done"),
        "completed REQ must be `done`: {:?}", entry_501);
    let missing_501 = entry_501["missing_dimensions"].as_array()
        .expect("missing dimensions array");
    assert!(!missing_501.iter().any(|v| v.as_str() == Some("status")),
        "completed status must not be flagged as missing: {:?}", missing_501);
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

// REQ-AXO-145 — `axon_pre_flight_check` accepts `incremental: true` to
// validate each diff_path individually and return per-file violations.
// Default (omitted/false) preserves the batch-validation contract.
//
// Tests use a unique trigger path (`src/req145_fixture/`) so the new
// guideline isolates from any pre-seeded GUI-PRO-* rules.
fn insert_req145_fixture_guideline(server: &McpServer) {
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-REQ145-001', 'Guideline', 'AXO', 'REQ-145 fixture rule', \
             'Diffs touching src/req145_fixture/ must include req145_marker.rs', 'active', \
             '{\"trigger_path\":\"src/req145_fixture/\",\"required_path\":\"req145_marker.rs\",\"enforcement\":\"strict\"}')",
        )
        .unwrap();
}

#[test]
fn test_axon_pre_flight_check_incremental_returns_per_file_violations() {
    let server = create_test_server();
    insert_req145_fixture_guideline(&server);

    // Mixed batch: bad file (triggers fixture rule, no marker) +
    // good file (carries the marker).
    let req_incremental = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": [
                    "src/req145_fixture/feature.rs",
                    "src/req145_fixture/req145_marker.rs"
                ],
                "incremental": true
            }
        },
        "id": 1
    });

    let res = server
        .handle_request(serde_json::from_value(req_incremental).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "incremental dry-run with one failing file must surface isError=true"
    );

    let data = res.get("data").expect("data field present");
    assert_eq!(
        data.get("incremental").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(data.get("files_checked").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(data.get("failing_files").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        data.get("first_failing_path").and_then(|v| v.as_str()),
        Some("src/req145_fixture/feature.rs")
    );

    let per_file = data
        .get("per_file_violations")
        .and_then(|v| v.as_object())
        .expect("per_file_violations is an object");
    let bad_entry = per_file
        .get("src/req145_fixture/feature.rs")
        .expect("bad path entry present");
    assert_eq!(bad_entry.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert!(
        bad_entry
            .get("violations")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "bad path must carry at least one violation"
    );
    let good_entry = per_file
        .get("src/req145_fixture/req145_marker.rs")
        .expect("good path entry present");
    assert_eq!(good_entry.get("ok").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn test_axon_pre_flight_check_default_mode_remains_batch() {
    let server = create_test_server();
    insert_req145_fixture_guideline(&server);

    // Same mixed batch but WITHOUT incremental. The aggregate batch view
    // satisfies the rule because the marker file is in the same set,
    // so it must pass.
    let req_default = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": [
                    "src/req145_fixture/feature.rs",
                    "src/req145_fixture/req145_marker.rs"
                ]
            }
        },
        "id": 2
    });

    let res = server
        .handle_request(serde_json::from_value(req_default).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        !res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "default (batch) mode passes when marker is in the same diff_paths set"
    );
    let text = res
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert!(
        text.contains("Validation passed"),
        "batch mode must surface the batch validation message"
    );
    // Default mode never sets the incremental marker.
    let incremental_marker = res
        .pointer("/data/incremental")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!incremental_marker);
}

// REQ-AXO-121 — `path_satisfies_required_path` must recognize inline
// `#[cfg(test)]` blocks inside a modified `.rs` file as satisfying the
// `tests.rs` requirement. This unblocks (a) Rust binary crates whose
// canonical idiom is `#[cfg(test)] mod tests {}` inline, and (b)
// trivial library hygiene fixes (one-line attribute changes in files
// that already carry inline tests). The sibling `_tests.rs` patterns
// remain valid; this is a pure addition to the matcher.
#[test]
fn test_axon_commit_work_recognizes_inline_cfg_test_in_modified_rs_file() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'TDD', 'tests required', 'active', \
             '{\"trigger_path\":\"src/inline_tests/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
        )
        .unwrap();

    // Write a temp file that emulates a Rust source with inline tests.
    let tmp = tempdir().unwrap();
    let inline_test_path = tmp.path().join("src/inline_tests/foo.rs");
    std::fs::create_dir_all(inline_test_path.parent().unwrap()).unwrap();
    std::fs::write(
        &inline_test_path,
        "fn foo() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn smoke() {}\n}\n",
    )
    .unwrap();
    let inline_path_str = inline_test_path.to_string_lossy().to_string();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": [inline_path_str],
                "message": "test: inline cfg(test) recognized",
                "dry_run": true
            }
        },
        "id": 1
    });

    let result = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "inline #[cfg(test)] must satisfy the TDD gate without a sibling _tests.rs file: {content}"
    );
    assert!(content.contains("Validation passed"), "{content}");
}

#[test]
fn test_axon_commit_work_still_rejects_modified_rs_file_without_any_test_marker() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'TDD', 'tests required', 'active', \
             '{\"trigger_path\":\"src/no_tests_here/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
        )
        .unwrap();

    let tmp = tempdir().unwrap();
    let bare_path = tmp.path().join("src/no_tests_here/bar.rs");
    std::fs::create_dir_all(bare_path.parent().unwrap()).unwrap();
    std::fs::write(&bare_path, "fn bar() {}\n").unwrap();
    let bare_path_str = bare_path.to_string_lossy().to_string();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": [bare_path_str],
                "message": "test: no inline tests, no sibling",
                "dry_run": true
            }
        },
        "id": 2
    });

    let result = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "a .rs file with neither inline tests nor a sibling test path must still be rejected: {content}"
    );
    assert!(content.contains("Remediation"), "{content}");
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
    // REQ-AXO-278: Bootstrap-vs-Continuation phase detection (GUI-PRO-026)
    assert!(
        bundle.contains_key("bootstrap_required"),
        "kickoff_bundle must include bootstrap_required boolean per REQ-AXO-278"
    );
    assert!(
        bundle["bootstrap_required"].is_boolean(),
        "bootstrap_required must be boolean, got {:?}",
        bundle["bootstrap_required"]
    );
    assert!(
        bundle.contains_key("input_documents"),
        "kickoff_bundle must include input_documents[] array per REQ-AXO-278"
    );
    assert!(
        bundle["input_documents"].is_array(),
        "input_documents must be an array, got {:?}",
        bundle["input_documents"]
    );
    // Fresh project (no VIS-{code}-001) => bootstrap_required=true
    let bootstrap_required = bundle["bootstrap_required"].as_bool().unwrap();
    let input_documents = bundle["input_documents"].as_array().unwrap();
    if bootstrap_required {
        // input_documents[] may be empty if path doesn't exist on disk, but
        // shape must hold (array of objects with path/size_bytes/mtime_unix_secs)
        for doc in input_documents {
            let obj = doc.as_object().expect("input_documents entries must be objects");
            assert!(obj.contains_key("path"));
            assert!(obj.contains_key("size_bytes"));
            assert!(obj.contains_key("mtime_unix_secs"));
        }
    } else {
        assert!(
            input_documents.is_empty(),
            "input_documents must be empty when bootstrap_required=false (Continuation phase)"
        );
    }
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
    assert!(kinds.contains("sql"), "entry_points must include `sql` steps: {kinds:?}");

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

// REQ-AXO-176 — kickoff bundle enrichment: aggregate recent project
// activity inline so a fresh LLM session reaches productive state from
// a single MCP call, without adding a 10th SOLL entity type.
#[test]
fn test_axon_init_project_bundle_includes_recent_activity_fields() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/home/dstadel/projects/BookingSystem" }
        },
        "id": 176
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = result["data"]["kickoff_bundle"]
        .as_object()
        .expect("kickoff_bundle must be present");

    // Each new field must be an array; content may be empty for a sparse
    // project. The contract is shape-stable, not row-count-stable.
    for field in [
        "in_progress_requirements",
        "wave_1_unblockers",
        "recent_req_commits",
        "recent_soll_writes",
    ] {
        assert!(
            bundle.contains_key(field),
            "bundle must contain `{field}` (REQ-AXO-176)"
        );
        assert!(
            bundle[field].is_array(),
            "`{field}` must be an array, got {}: {}",
            bundle[field],
            bundle.get(field).map(|v| v.to_string()).unwrap_or_default()
        );
    }

    // in_progress_requirements rows must carry the documented schema.
    if let Some(arr) = bundle["in_progress_requirements"].as_array() {
        for row in arr {
            assert!(
                row.get("id").and_then(|v| v.as_str()).is_some(),
                "in_progress_requirements row must have id: {row}"
            );
            assert!(
                row.get("title").and_then(|v| v.as_str()).is_some(),
                "in_progress_requirements row must have title: {row}"
            );
            assert!(
                row.get("priority").is_some(),
                "in_progress_requirements row must have priority key (may be null): {row}"
            );
        }
    }

    // recent_soll_writes rows must carry id+type+title+updated_at keys.
    if let Some(arr) = bundle["recent_soll_writes"].as_array() {
        for row in arr {
            for key in ["id", "type", "title", "updated_at"] {
                assert!(
                    row.get(key).is_some(),
                    "recent_soll_writes row must have `{key}` key: {row}"
                );
            }
        }
    }

    // Human-readable text must reference the new fields so an LLM
    // scanning content alone can discover them.
    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("in_progress_requirements"),
        "response text must advertise the new bundle fields: {content}"
    );
}

// REQ-AXO-143 — `session_pointer` is the canonical workflow-agnostic
// onboarding pointer. Persisted on axon_init_project, surfaced on the
// kickoff bundle AND on `status.data.instance_identity.session_pointer`.
#[test]
fn test_axon_init_project_persists_session_pointer_url_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-url-fixture",
                "session_pointer": {
                    "kind": "url",
                    "value": "https://linear.app/team/issue/AXO-143",
                    "label": "active ticket"
                }
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = &result["data"]["kickoff_bundle"];
    let pointer = bundle.get("session_pointer").expect("session_pointer present");
    assert_eq!(pointer["kind"].as_str(), Some("url"));
    assert_eq!(
        pointer["value"].as_str(),
        Some("https://linear.app/team/issue/AXO-143")
    );
    assert_eq!(pointer["label"].as_str(), Some("active ticket"));
    // active_handoff alias only mirrors kind=file.
    assert!(
        bundle["active_handoff"].is_null(),
        "active_handoff alias must stay null when kind=url: {bundle}"
    );
}

#[test]
fn test_axon_init_project_session_pointer_kind_none_clears_value() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-none-fixture",
                "session_pointer": { "kind": "none" }
            }
        },
        "id": 2
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let pointer = result["data"]["kickoff_bundle"]["session_pointer"].clone();
    assert_eq!(pointer["kind"].as_str(), Some("none"));
    assert!(pointer["value"].is_null());
}

#[test]
fn test_axon_init_project_rejects_invalid_session_pointer_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-invalid-fixture",
                "session_pointer": { "kind": "wiki", "value": "ignored" }
            }
        },
        "id": 3
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "invalid kind must be rejected: {result}"
    );
    let parameter_repair = result["data"]["parameter_repair"].clone();
    assert_eq!(
        parameter_repair["invalid_field"].as_str(),
        Some("session_pointer")
    );
}

#[test]
fn test_axon_init_project_rejects_session_pointer_missing_value_for_url_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-missing-value-fixture",
                "session_pointer": { "kind": "soll_node" }
            }
        },
        "id": 4
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
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
// REQ-AXO-126 — `axon_commit_work` no longer auto-fires `soll_export`.
// The release-promotion pipeline owns the snapshot moment now (option D
// in the retention design). The response must contain only the git
// commit status and must NOT contain any "Exported to" / "Export
// Report" markers.
//
// REQ-AXO-246 — must run in an isolated tempdir + ephemeral git repo,
// never against AXON_REPO. Pass project_path explicitly so the tool
// routes git commands via Command::current_dir to the sandbox.
fn test_axon_commit_work_executes_git_without_auto_export_when_dry_run_false() {
    let server = create_test_server();
    let sandbox = init_commit_work_sandbox();

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
                "project_path": sandbox.path().to_str().unwrap(),
                "message": "test: REQ-AXO-246 isolated commit (sandbox, never reaches AXON_REPO)",
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

    // Git commit must have succeeded inside the sandbox.
    assert!(
        content.contains("Commit succeeded"),
        "expected sandbox commit to succeed: {content}"
    );
    // REQ-AXO-126 — no auto-export markers must appear on the
    // commit-work response surface.
    assert!(
        !content.contains("Exported to"),
        "auto-export hook must be gone from commit_work: {content}"
    );
    assert!(
        !content.contains("Export Report"),
        "Export Report block must not be emitted from commit_work: {content}"
    );

    // REQ-AXO-246 regression assertion: the new commit landed in the
    // sandbox repo, not anywhere else. HEAD should reference our message.
    let head_subject = std::process::Command::new("git")
        .current_dir(sandbox.path())
        .args(["log", "-1", "--pretty=%s"])
        .output()
        .expect("git log");
    let subject = String::from_utf8_lossy(&head_subject.stdout);
    assert!(
        subject.contains("REQ-AXO-246 isolated commit"),
        "sandbox HEAD must hold the test commit; got: {subject}"
    );
}

// REQ-AXO-246 — set up an ephemeral git repo for axon_commit_work tests.
// Returns a TempDir whose drop cleans the sandbox at end of test.
fn init_commit_work_sandbox() -> tempfile::TempDir {
    let dir = tempdir().expect("tempdir");
    let path = dir.path();
    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .expect("git invocation");
        assert!(status.success(), "git {:?} failed in sandbox", args);
    };
    run_git(&["init", "--initial-branch=main"]);
    run_git(&["config", "user.email", "axon-test@example.invalid"]);
    run_git(&["config", "user.name", "axon-test"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"sandbox\"\nversion = \"0.0.1\"\n",
    )
    .expect("seed Cargo.toml");
    run_git(&["add", "Cargo.toml"]);
    run_git(&["commit", "-m", "initial sandbox commit"]);
    // Stage a real change so axon_commit_work has something to commit.
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"sandbox\"\nversion = \"0.0.2\"\n",
    )
    .expect("modify Cargo.toml");
    dir
}

#[test]
fn test_soll_apply_plan_resolves_logical_keys_in_relations() {
    // REQ-AXO-137: soll_apply_plan must resolve logical_key references in
    // relations[].{source_id,target_id} to the canonical IDs produced by
    // sibling create operations in the same plan, so a transactional batch
    // truly creates BOTH the nodes AND the edges in one call.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Anchor pillar', '', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test_runner",
                "dry_run": false,
                "plan": {
                    "concepts": [{
                        "logical_key": "CPT-anchor-protocol",
                        "title": "Anchor protocol concept",
                        "description": "Concept created via plan to test logical_key resolution",
                        "status": "accepted",
                        "metadata": {}
                    }]
                },
                "relations": [{
                    "source_id": "CPT-anchor-protocol",
                    "target_id": "PIL-AXO-001",
                    "relation_type": "BELONGS_TO"
                }]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_ne!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "apply_plan must succeed: {:?}",
        result
    );

    // Lookup the canonical id of the freshly-created concept.
    let cpt_id = server
        .graph_store
        .query_json(
            "SELECT id FROM soll.Node WHERE type='Concept' AND title='Anchor protocol concept' AND project_code='AXO' LIMIT 1",
        )
        .unwrap();
    let cpt_rows: Vec<Vec<String>> = serde_json::from_str(&cpt_id).unwrap_or_default();
    assert!(
        !cpt_rows.is_empty(),
        "concept must have been created: {}",
        cpt_id
    );
    let canonical_concept_id = cpt_rows[0][0].clone();
    assert!(
        canonical_concept_id.starts_with("CPT-AXO-"),
        "canonical id must follow CPT-AXO-NNN format, got {}",
        canonical_concept_id
    );

    // Assert an Edge was created with the resolved canonical id.
    let edge_count = server
        .graph_store
        .query_json(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = 'PIL-AXO-001' AND relation_type = 'BELONGS_TO'",
            canonical_concept_id
        ))
        .unwrap();
    let edge_rows: Vec<Vec<String>> = serde_json::from_str(&edge_count).unwrap_or_default();
    let count: i64 = edge_rows.first()
        .and_then(|r| r.first())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    assert_eq!(
        count, 1,
        "Edge must be materialized via logical_key resolution; got {} edges (canonical id={}). Raw: {}",
        count, canonical_concept_id, edge_count
    );

    // REQ-AXO-137 response-surface contract: data.linked[] must expose the
    // RESOLVED canonical ids, not the original logical_keys, so the LLM can
    // query Edge directly without re-resolving. raw_source_id/raw_target_id
    // preserve the original input for audit.
    let data = result.get("data").expect("response data");
    let linked = data["linked"].as_array().expect("linked array");
    assert_eq!(linked.len(), 1, "exactly one link expected: {:?}", data);
    assert_eq!(
        linked[0]["source_id"].as_str(),
        Some(canonical_concept_id.as_str()),
        "data.linked[].source_id must be canonical id, not logical_key: {:?}",
        linked[0]
    );
    assert_eq!(
        linked[0]["target_id"].as_str(),
        Some("PIL-AXO-001"),
        "target was canonical at input, must stay canonical: {:?}",
        linked[0]
    );
    assert_eq!(
        linked[0]["raw_source_id"].as_str(),
        Some("CPT-anchor-protocol"),
        "raw_source_id must preserve the original logical_key for audit: {:?}",
        linked[0]
    );
}

#[test]
fn test_restore_soll_invalid_path_returns_parameter_repair() {
    // REQ-AXO-147 slice 1 — operations.rs failure paths now surface
    // data.parameter_repair so the LLM can recover in one round-trip.
    // Restoring from a path that does not exist must point at the `path`
    // field with a hint to use docs/vision/SOLL_EXPORT_*.md.
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "restore_soll",
            "arguments": {
                "path": "/tmp/this/path/definitely/does/not/exist-axo-147.md"
            }
        },
        "id": 9001
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "non-existent path must surface isError=true: {result:?}"
    );
    let data = result.get("data").expect("data payload required");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("path"));
    assert!(
        repair["supplied_value"]
            .as_str()
            .unwrap_or("")
            .contains("does/not/exist"),
        "parameter_repair must echo the supplied path: {repair}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("SOLL_EXPORT") || hint.contains("docs/vision"),
        "hint must point at the canonical export location: {hint}"
    );
}

#[test]
fn test_soll_export_unregistered_project_code_returns_wrong_project_scope() {
    // REQ-AXO-147 slice 1 — soll_export now uses the shared
    // wrong_project_scope_response helper for unregistered codes
    // (consistent with soll_validate / soll_query_context / soll_work_plan).
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_export",
            "arguments": {
                "project_code": "ZZZ"
            }
        },
        "id": 9002
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data payload required");
    let status = data["status"].as_str().unwrap_or("");
    assert!(
        status == "wrong_project_scope" || status == "input_invalid",
        "unregistered project_code must surface a structured status (got `{status}`): {data:?}"
    );
}

#[test]
fn test_document_intent_classifies_and_creates_canonical_soll_node() {
    // REQ-AXO-141 — document_intent is the discoverable MCP entry point for
    // "documente" / "document this" workflows. With suggest_type omitted,
    // the server-side classifier picks one of {requirement, decision,
    // concept, guideline} based on body keywords. Returns the canonical
    // SOLL id assigned by soll_manager.
    let server = create_test_server();

    // Body contains both "framework" (concept-keyword) and "fix needed"
    // (requirement-keyword); requirement must win because the LLM
    // contract treats problem-class signals as more actionable.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "document_intent",
            "arguments": {
                "intent": "Indexer fails on empty file",
                "body": "the framework is broken when the file is 0 bytes — fix needed before next release",
                "project_code": "AXO",
                "tags": ["llm-friction", "indexer"]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_ne!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "document_intent must succeed: {result:?}"
    );

    let data = result.get("data").expect("response data");
    assert_eq!(data["status"].as_str(), Some("ok"));
    assert_eq!(
        data["entity_type"].as_str(),
        Some("requirement"),
        "classifier must pick `requirement` when problem-class keyword fires: {data:?}"
    );
    assert_eq!(
        data["classifier_reason"].as_str(),
        Some("matched_requirement_keyword")
    );
    let canonical_id = data["canonical_id"].as_str().expect("canonical_id string");
    assert!(
        canonical_id.starts_with("REQ-AXO-"),
        "auto-classified requirement must get a REQ-AXO-NNN id, got {canonical_id}"
    );

    // The actual SOLL Node row must exist with the expected fields.
    let row = server
        .graph_store
        .query_json(&format!(
            "SELECT type, title, description, status, metadata FROM soll.Node WHERE id = '{}' LIMIT 1",
            canonical_id
        ))
        .unwrap();
    let parsed: Vec<Vec<String>> = serde_json::from_str(&row).unwrap_or_default();
    let node = parsed.first().expect("created Node row");
    assert_eq!(node[0], "Requirement");
    assert_eq!(node[1], "Indexer fails on empty file");
    assert!(
        node[4].contains("classifier_reason"),
        "metadata must persist classifier_reason: {}",
        node[4]
    );
    assert!(
        node[4].contains("llm-friction"),
        "metadata.tags must be persisted: {}",
        node[4]
    );
}

#[test]
fn test_document_intent_rejects_invalid_suggest_type_with_parameter_repair() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "document_intent",
            "arguments": {
                "intent": "x",
                "body": "x",
                "suggest_type": "wat"
            }
        },
        "id": 2
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("suggest_type"));
    assert_eq!(repair["supplied_value"].as_str(), Some("wat"));
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for kind in ["requirement", "decision", "concept", "guideline"] {
        assert!(
            names.contains(&kind),
            "accepted_values must include `{kind}`: {names:?}"
        );
    }
}

#[test]
fn test_soll_apply_plan_surfaces_unresolved_logical_keys_in_errors_and_parameter_repair() {
    // REQ-AXO-139 slice — when a relation references a logical_key that
    // is neither a canonical TYPE-CODE-NNN id nor created in the same plan
    // batch, the response must surface the unresolved keys in `errors[]`
    // and a top-level `parameter_repair` so the LLM can fix the inputs in
    // one round-trip.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-091', 'Pillar', 'AXO', 'Anchor pillar 91', '', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test_runner",
                "dry_run": false,
                "plan": {
                    "concepts": [{
                        "logical_key": "CPT-resolved-cpt-91",
                        "title": "Resolved concept slice 5",
                        "description": "Concept created via plan; its logical_key resolves",
                        "status": "accepted",
                        "metadata": {}
                    }]
                },
                "relations": [
                    {
                        // Resolved (sibling create) — no error expected for this row.
                        "source_id": "CPT-resolved-cpt-91",
                        "target_id": "PIL-AXO-091",
                        "relation_type": "BELONGS_TO"
                    },
                    {
                        // Unresolved logical_key on source — must show up in errors[].
                        "source_id": "CPT-typo-not-created",
                        "target_id": "PIL-AXO-091",
                        "relation_type": "BELONGS_TO"
                    }
                ]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("response data");

    let errors = data["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("errors array required in: {data:?}"));
    let unresolved_entries: Vec<&Value> = errors
        .iter()
        .filter(|e| e.get("kind").and_then(|v| v.as_str()) == Some("unresolved_logical_key"))
        .collect();
    assert_eq!(
        unresolved_entries.len(),
        1,
        "exactly one unresolved_logical_key error expected; got: {errors:?}"
    );
    let err = unresolved_entries[0];
    assert_eq!(err["operation"].as_str(), Some("link"));
    let unresolved_keys = err["unresolved_keys"]
        .as_array()
        .expect("unresolved_keys array");
    let unresolved_names: Vec<&str> = unresolved_keys.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        unresolved_names.contains(&"CPT-typo-not-created"),
        "unresolved_keys must list the missing logical_key: {unresolved_names:?}"
    );
    assert!(
        err["available_logical_keys"]
            .as_array()
            .map(|arr| arr.iter().any(|v| v.as_str() == Some("CPT-resolved-cpt-91")))
            .unwrap_or(false),
        "available_logical_keys must list the keys that DID resolve: {err:?}"
    );

    let repair = data["parameter_repair"].clone();
    assert!(
        !repair.is_null(),
        "parameter_repair must be set when unresolved logical_keys exist: {data:?}"
    );
    assert_eq!(
        repair["invalid_field"].as_str(),
        Some("operations[].payload.source_id|target_id")
    );
    let repair_unresolved = repair["unresolved_keys"]
        .as_array()
        .expect("repair unresolved_keys array");
    let repair_unresolved_names: Vec<&str> =
        repair_unresolved.iter().filter_map(|v| v.as_str()).collect();
    assert!(repair_unresolved_names.contains(&"CPT-typo-not-created"));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_names.contains(&"soll_manager"),
        "follow_up_tools must include `soll_manager`: {follow_names:?}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("logical_key") || hint.contains("canonical"),
        "hint must explain logical_key vs canonical id: {hint}"
    );
}

#[test]
fn test_axon_commit_work_refuses_partial_diff_when_git_add_fails() {
    // REQ-AXO-138 — when `git add <diff_paths>` exits non-zero (e.g., a path
    // doesn't exist), axon_commit_work must NOT proceed to `git commit`.
    // Previously the code only checked Command::output() Err (process-spawn
    // failure) and let exit-code failures pass through silently, resulting in
    // commits that captured only whatever was pre-staged. Now the exit status
    // is checked and a structured parameter_repair response is returned.
    let server = create_test_server();
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
                "diff_paths": ["this/path/definitely/does/not/exist.rs"],
                "message": "test: REQ-AXO-138 partial-diff refusal",
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

    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "non-existent diff_path must surface isError=true: {}",
        content
    );
    assert!(
        content.contains("Git add failed") || content.contains("Refusing to commit"),
        "error text must explain partial-diff refusal: {}",
        content
    );
    let data = result.get("data").expect("data payload required for repair");
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("input_invalid")
    );
    assert!(
        data.get("git_add_exit_code")
            .and_then(|v| v.as_i64())
            .is_some(),
        "git_add_exit_code must be exposed for diagnostics"
    );
    assert_eq!(
        data.get("parameter_repair")
            .and_then(|pr| pr.get("invalid_field"))
            .and_then(|v| v.as_str()),
        Some("diff_paths")
    );
    assert!(
        !content.contains("Commit succeeded"),
        "commit must NOT have happened: {}",
        content
    );
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

#[test]
fn test_soll_remove_evidence_drops_only_broken_file_refs_by_default() {
    // REQ-AXO-254 — close MIL-AXO-015 wave G followup. Verify the new
    // soll_remove_evidence tool only removes Traceability rows whose
    // artifact_ref does NOT exist on disk by default (broken_only=true).
    let server = create_test_server();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-254-test', 'Requirement', 'AXO', 'soll_remove_evidence smoke', 'broken_only mode', 'current', '{\"acceptance_criteria\":\"a\"}')")
        .unwrap();

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let valid = repo_root.join("README.md");
    let valid_path = valid.to_string_lossy().to_string();

    // Seed: 1 valid + 2 broken artifact refs.
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-VALID-1", "Requirement", "REQ-AXO-254-test", "file", valid_path, 1.0, "{}", 1u64]),
        )
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-BROKEN-1", "Requirement", "REQ-AXO-254-test", "file", "/tmp/does-not-exist-axo-254-1.rs", 1.0, "{}", 2u64]),
        )
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-BROKEN-2", "Requirement", "REQ-AXO-254-test", "document", "/tmp/does-not-exist-axo-254-2.md", 1.0, "{}", 3u64]),
        )
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-254-test"}
            })),
            id: Some(json!(254001)),
        })
        .unwrap()
        .result
        .unwrap();
    let data = response["data"].clone();
    assert_eq!(data["mode"].as_str(), Some("broken_only"));
    assert_eq!(data["removed_count"].as_u64(), Some(2));
    let removed_refs: Vec<&str> = data["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("artifact_ref").and_then(|v| v.as_str()))
        .collect();
    assert!(removed_refs.contains(&"/tmp/does-not-exist-axo-254-1.rs"));
    assert!(removed_refs.contains(&"/tmp/does-not-exist-axo-254-2.md"));
    let kept = data["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept[0].get("artifact_ref").and_then(|v| v.as_str()),
        Some(valid_path.as_str())
    );

    // Idempotent: second call returns 0 removed, 1 kept.
    let response2 = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-254-test"}
            })),
            id: Some(json!(254002)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response2["data"]["removed_count"].as_u64(), Some(0));
    assert_eq!(
        response2["data"]["kept"].as_array().unwrap().len(),
        1
    );
}

// REQ-AXO-274 phase 2 — canonical relation policy extensions
#[test]
fn test_relation_policy_accepts_cpt_to_cpt_inherits_from() {
    let server = create_test_server();
    // CPT-PRO sibling (universal)
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-PRO-099', 'Concept', 'PRO', 'Universal concept', 'cross-project mental model', 'active', '{}')")
        .unwrap();
    // CPT-AXO project-specific specialization
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-099', 'Concept', 'AXO', 'Axon-specific concept', 'Axon-specific specialization', 'active', '{}')")
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
                    "source_id": "CPT-AXO-099",
                    "target_id": "CPT-PRO-099",
                    "relation_type": "INHERITS_FROM"
                }
            }
        })),
        id: Some(json!(27401)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "CPT->CPT INHERITS_FROM must be canonical post REQ-AXO-274 phase 2: {content}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE source_id='CPT-AXO-099' AND target_id='CPT-PRO-099' AND relation_type='INHERITS_FROM'")
            .unwrap(),
        1
    );
}

#[test]
fn test_relation_policy_accepts_gui_to_pil_belongs_to() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-PRO-099', 'Pillar', 'PRO', 'Test methodology pillar', 'theming axis', 'active', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-PRO-099', 'Guideline', 'PRO', 'Test guideline', 'rule', 'active', '{}')")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "guideline",
                "data": {
                    "source_id": "GUI-PRO-099",
                    "target_id": "PIL-PRO-099",
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(27402)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "GUI->PIL BELONGS_TO must be canonical post REQ-AXO-274 phase 2: {content}"
    );
}

#[test]
fn test_relation_policy_accepts_cpt_to_dec_inherits_from() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-PRO-099', 'Decision', 'PRO', 'Cross-project canonical decision', 'body', 'accepted', '{\"rationale\":\"R\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-098', 'Concept', 'AXO', 'Axon mirror concept', 'specialization of DEC-PRO-099', 'active', '{}')")
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
                    "source_id": "CPT-AXO-098",
                    "target_id": "DEC-PRO-099",
                    "relation_type": "INHERITS_FROM"
                }
            }
        })),
        id: Some(json!(27403)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "CPT->DEC INHERITS_FROM must be canonical post REQ-AXO-274 phase 2: {content}"
    );
}

// REQ-AXO-276 — axon_apply_methodology_bundle MCP tool
#[test]
fn test_axon_apply_methodology_bundle_rejects_missing_bundle_path() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": "axon_apply_methodology_bundle", "arguments": {} },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let data = &result["data"];
    assert_eq!(
        data["status"].as_str().unwrap(),
        "input_invalid",
        "missing bundle_path must return input_invalid"
    );
    assert_eq!(
        data["parameter_repair"]["invalid_field"].as_str().unwrap(),
        "bundle_path"
    );
}

#[test]
fn test_axon_apply_methodology_bundle_rejects_unsupported_schema() {
    let server = create_test_server();
    let tmp_dir = std::env::temp_dir().join(format!(
        "axon_methodology_bundle_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let bundle_path = tmp_dir.join("bad-schema.json");
    std::fs::write(&bundle_path, r#"{"schema":"wrong-schema-v0","version":"0.1","project_code":"AXO"}"#)
        .unwrap();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_methodology_bundle",
            "arguments": { "bundle_path": bundle_path.to_string_lossy() }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["status"].as_str().unwrap(), "input_invalid");
    assert_eq!(
        result["data"]["parameter_repair"]["invalid_field"]
            .as_str()
            .unwrap(),
        "schema"
    );
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_axon_apply_methodology_bundle_dry_run_returns_summary() {
    let server = create_test_server();
    let tmp_dir = std::env::temp_dir().join(format!(
        "axon_methodology_bundle_dryrun_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let bundle_path = tmp_dir.join("minimal-bundle.json");
    let body = serde_json::json!({
        "schema": "axon-methodology-bundle-v1",
        "version": "1.0.0-test",
        "axon_min_version": "0.8.0",
        "project_code": "AXO",
        "pillars": [],
        "concepts": [],
        "guidelines": [
            {
                "logical_key": "gui_test_new",
                "title": "Test methodology guideline",
                "description": "Test body",
                "status": "active"
            },
            {
                "logical_key": "gui_test_regularization",
                "canonical_id_hint": "GUI-PRO-001",
                "title": "TDD Obligatoire",
                "regularization": true
            }
        ],
        "decisions": [],
        "requirements": [],
        "relations": []
    });
    std::fs::write(&bundle_path, serde_json::to_string(&body).unwrap()).unwrap();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_methodology_bundle",
            "arguments": {
                "bundle_path": bundle_path.to_string_lossy(),
                "dry_run": true
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let data = &result["data"];
    assert_eq!(data["status"].as_str().unwrap(), "ok");
    assert_eq!(data["dry_run"].as_bool().unwrap(), true);
    assert_eq!(data["bundle_version"].as_str().unwrap(), "1.0.0-test");
    assert_eq!(data["project_code"].as_str().unwrap(), "AXO");
    assert_eq!(
        data["guidelines_applied"].as_u64().unwrap(),
        1,
        "1 non-regularization guideline counted under dry_run"
    );
    assert_eq!(
        data["guidelines_skipped_regularization"].as_u64().unwrap(),
        1,
        "1 regularization stanza skipped"
    );
    std::fs::remove_dir_all(&tmp_dir).ok();
}
