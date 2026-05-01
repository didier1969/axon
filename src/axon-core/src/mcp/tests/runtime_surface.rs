use super::*;

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
    assert!(tool_names.contains(&"help"));
    assert!(tool_names.contains(&"restore_soll"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"soll_apply_plan"));
    assert!(tool_names.contains(&"soll_work_plan"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"mcp_surface_diagnostics"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"project_registry_lookup"));
    assert!(tool_names.contains(&"soll_relation_schema"));
    assert!(tool_names.contains(&"infer_soll_mutation"));
    assert!(tool_names.contains(&"entrench_nuance"));
    assert!(tool_names.contains(&"soll_generate_docs"));
    assert!(tool_names.contains(&"snapshot_history"));
    assert!(tool_names.contains(&"snapshot_diff"));
    assert!(tool_names.contains(&"conception_view"));
    assert!(tool_names.contains(&"change_safety"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(tool_names.contains(&"axon_pre_flight_check"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(!tool_names.contains(&"soll_apply_plan_v2"));
    assert!(tool_names.contains(&"refine_lattice"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"cypher"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"schema_overview"));
    assert!(tool_names.contains(&"list_labels_tables"));
    assert!(tool_names.contains(&"query_examples"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));
}

#[test]
fn test_help_returns_compact_llm_routing_and_skill_pointer() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "help",
                "arguments": { "topic": "routing", "intent": "prepare_edit" }
            })),
            id: Some(json!(77)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("help data");
    assert_eq!(data["topic"].as_str(), Some("routing"));
    assert_eq!(data["audience"].as_str(), Some("llm_clients_only"));
    assert_eq!(data["protocol"]["intent"].as_str(), Some("prepare_edit"));
    assert_eq!(
        data["skill"]["name"].as_str(),
        Some("axon-engineering-protocol")
    );
    assert_eq!(
        data["skill"]["path"].as_str(),
        Some("docs/skills/axon-engineering-protocol/SKILL.md")
    );
    assert!(data["routing"]
        .as_array()
        .is_some_and(|items| items.len() <= 8));
    assert_eq!(
        data["protocol"]["minimal_sequence"][0].as_str(),
        Some("status")
    );
    assert!(data["protocol"]["minimal_sequence"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == "impact")));
    assert!(data["protocol"]["stop_rule"]
        .as_str()
        .is_some_and(|text| text.contains("blast radius")));
    assert!(data["protocol"]["avoid"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(data["protocol"]["requires_explicit_input_if"]
        .as_array()
        .is_some_and(|items| items
            .iter()
            .any(|item| item == "business intent is missing")));
    assert_eq!(
        data["token_policy"].as_str(),
        Some("brief_first_full_only_when_needed")
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("query -> inspect"), "{text}");
    assert!(text.contains("Protocol: prepare_edit"), "{text}");
    assert!(text.len() < 950, "{text}");
}

#[test]
fn test_help_returns_tool_schema_and_examples_for_soll_apply_plan() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "help",
                "arguments": { "tool": "soll_apply_plan" }
            })),
            id: Some(json!(78)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("help data");
    assert_eq!(data["tool"].as_str(), Some("soll_apply_plan"));
    assert!(data["input_schema"]["required"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == "project_code")));
    assert!(data["input_schema"]["properties"]
        .get("relations")
        .is_some());
    assert!(data["usage_examples"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(
        data["next_action"]["after_success"].as_str(),
        Some("poll `job_status` if the response returns `job_id`; commit only after dry-run matches intent")
    );
}

#[test]
fn test_mcp_tools_list_in_brain_only_exposes_information_surface() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1000)),
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
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_tools_list_in_full_autonomous_exposes_information_surface() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    assert!(tool_names.contains(&"infer_soll_mutation"));
    assert!(tool_names.contains(&"entrench_nuance"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(!tool_names.contains(&"resume_vectorization"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"cypher"));
    assert!(tool_names.contains(&"diagnose_indexing"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_mcp_tools_list_include_internal_adds_resume_vectorization_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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

    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
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
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"cypher"));
    assert!(tool_names.contains(&"schema_overview"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_soll_manager_stays_sync_when_mutation_jobs_are_enabled() {
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
    let content = result["content"][0]["text"].as_str().unwrap_or_default();
    let data = result.get("data").expect("sync response must carry data");
    assert_sync_mutation_contract(data);
    assert!(content.contains("CPT-AXO-"), "{content}");
    let entity_id = content
        .split('`')
        .find(|value| value.starts_with("CPT-AXO-"))
        .expect("entity id in content");
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
    let site_root = tempdir().unwrap();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
        std::env::set_var("AXON_SOLL_SITE_ROOT", site_root.path());
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
    assert_async_job_contract(data, "job_status");
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
    assert_eq!(
        data.get("known_ids")
            .and_then(|value| value.get("preview_id"))
            .and_then(|value| value.as_str()),
        Some(preview_id)
    );

    let final_status = wait_for_job_status(&server, job_id);
    assert_eq!(
        final_status["data"]["status"].as_str().unwrap(),
        "succeeded"
    );
    assert_eq!(final_status["data"]["state"].as_str(), Some("completed"));
    assert!(final_status["data"]["known_ids"].is_object());
    assert!(final_status["data"]["result_contract"].is_object());
    assert!(final_status["data"]["polling_guidance"].is_object());
    assert!(final_status["data"]["recovery_hint"].as_str().is_some());
    assert_eq!(
        final_status["data"]["next_action"]["kind"].as_str(),
        Some("read_terminal_result")
    );
    assert_eq!(
        final_status["data"]["result_data"]["preview_id"].as_str(),
        Some(preview_id)
    );
    let result_preview_id = final_status["data"]["result"]["data"]["preview_id"]
        .as_str()
        .expect("preview id should survive job result");
    assert_eq!(result_preview_id, preview_id);
    assert_eq!(
        final_status["data"]["result"]["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    assert!(site_root.path().join("AXO/index.html").is_file());

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
        std::env::remove_var("AXON_SOLL_SITE_ROOT");
    }
}

#[test]
fn test_axon_init_project_stays_sync_when_mutation_jobs_are_enabled() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_MUTATION_JOBS", "true");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        })),
        id: Some(json!(5003)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let data = result.get("data").expect("sync response must carry data");
    assert_sync_mutation_contract(data);
    assert_eq!(
        data.get("project_code").and_then(|value| value.as_str()),
        Some("BKS")
    );
    assert_eq!(
        data.get("project_name").and_then(|value| value.as_str()),
        Some("BookingSystem")
    );
    assert_eq!(
        data.get("project_path").and_then(|value| value.as_str()),
        Some("/home/dstadel/projects/BookingSystem")
    );
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_project_registry_lookup_finds_project_by_path_name_and_code() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry(
            "BKS",
            Some("BookingSystem"),
            Some("/home/dstadel/projects/BookingSystem"),
        )
        .unwrap();

    for arguments in [
        json!({ "project_code": "BKS" }),
        json!({ "project_name": "BookingSystem" }),
        json!({ "project_path": "/home/dstadel/projects/BookingSystem" }),
    ] {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_registry_lookup",
                "arguments": arguments
            })),
            id: Some(json!(5010)),
        };
        let response = server.handle_request(req).unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["data"]["found"].as_bool(), Some(true));
        assert_eq!(result["data"]["project_code"].as_str(), Some("BKS"));
        assert_eq!(
            result["data"]["project_name"].as_str(),
            Some("BookingSystem")
        );
        assert_eq!(
            result["data"]["project_path"].as_str(),
            Some("/home/dstadel/projects/BookingSystem")
        );
        assert_eq!(
            result["data"]["matches"]
                .as_array()
                .map(|items| items.len()),
            Some(1)
        );
        assert_eq!(
            result["data"]["next_action"]["kind"].as_str(),
            Some("use_canonical_project_code")
        );
        assert_eq!(
            result["data"]["next_action"]["tool"].as_str(),
            Some("project_status")
        );
        assert!(result["data"]["operator_guidance"].is_object());
    }
}

#[test]
fn test_soll_apply_plan_accepts_freshly_initialized_project_code_across_runtime_boundary() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph-store");
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store);

    let init_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": {
                    "project_path": "/home/dstadel/projects/nutri-opti",
                    "project_name": "nutri-opti"
                }
            })),
            id: Some(json!(5011)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(init_response["data"]["project_code"].as_str(), Some("NTO"));
    drop(server);

    let reopened_store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let reopened_server = McpServer::new(reopened_store);

    let lookup_response = reopened_server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_registry_lookup",
                "arguments": {
                    "project_path": "/home/dstadel/projects/nutri-opti"
                }
            })),
            id: Some(json!(5012)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        lookup_response["data"]["project_code"].as_str(),
        Some("NTO")
    );

    let apply_plan_response = reopened_server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_apply_plan",
                "arguments": {
                    "project_code": "NTO",
                    "author": "test",
                    "dry_run": true,
                    "plan": {
                        "visions": [
                            {
                                "logical_key": "vision-1",
                                "title": "Vision NTO",
                                "description": "Nutri Opti vision"
                            }
                        ],
                        "pillars": [
                            {
                                "logical_key": "pillar-1",
                                "title": "Pillar NTO",
                                "description": "Nutri Opti pillar"
                            }
                        ]
                    }
                }
            })),
            id: Some(json!(5013)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_ne!(
        apply_plan_response
            .get("isError")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    let data = apply_plan_response
        .get("data")
        .expect("apply-plan response must carry data");
    if data.get("job_id").is_some() {
        assert_async_job_contract(data, "job_status");
        let job_id = data
            .get("job_id")
            .and_then(|value| value.as_str())
            .expect("job_id");
        let preview_id = data["reserved_ids"]["preview_id"]
            .as_str()
            .expect("reserved preview id");
        assert!(preview_id.starts_with("PRV-NTO-"), "{preview_id}");

        let final_status = wait_for_job_status(&reopened_server, job_id);
        assert_eq!(
            final_status["data"]["status"].as_str().unwrap(),
            "succeeded"
        );
        assert_eq!(
            final_status["data"]["result_data"]["preview_id"].as_str(),
            Some(preview_id)
        );
    } else {
        assert!(data.get("job_id").is_none());
        assert!(data.get("accepted").is_none());
        assert!(data.get("polling_guidance").is_none());
        assert!(data["preview_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("PRV-NTO-")));
    }
}

#[test]
fn test_soll_manager_requires_project_code_even_when_mutation_jobs_are_enabled() {
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
    let is_error = result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    // project_code is now auto-resolved from canonical project identity,
    // so omitting it no longer triggers an error.
    assert!(
        !is_error,
        "soll_manager should auto-resolve project_code when omitted"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_soll_commit_revision_requires_preview_id_even_when_mutation_jobs_are_enabled() {
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
        content.contains("Missing required argument: preview_id"),
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
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
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_remains_available_in_graph_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
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
    assert!(
        !result
            .get("isError")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        "{result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(!content.contains("unavailable in runtime mode 'indexer_graph'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_status_graph_only_reports_semantic_drain_not_applicable() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_graph_only_reports_semantic_drain_not_applicable",
        );
    }
    let tempdir = tempdir().unwrap();
    let server = create_test_server_with_distinct_reader(tempdir.path());
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2165)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("status data");
    assert_eq!(data["runtime_mode"].as_str(), Some("indexer_graph"));
    assert!(data["debug_snapshot"].is_null());
    assert!(data["traceability"].is_null());
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_scope"].as_str(),
        Some("indexer_graph")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["semantic_health"].as_str(),
        Some("not_applicable")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["recommendation"].as_str(),
        Some("not_applicable")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_requested"]
            .as_str(),
        Some("cpu")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_mcp_tools_list_hides_indexed_runtime_tools_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_remains_available_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    assert!(
        !result
            .get("isError")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        "{result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(!content.contains("unavailable in runtime mode 'indexer_full'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_brain_only_impact_does_not_return_tool_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "impact",
                "arguments": { "symbol": "missing_symbol", "project": "AXO" }
            })),
            id: Some(json!(2296)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(!content.contains("unavailable"), "{content}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_brain_only_retrieve_context_does_not_return_tool_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "where is missing_symbol defined?", "project": "AXO" }
            })),
            id: Some(json!(2297)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(!content.contains("unavailable"), "{content}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_retrieve_context_auto_resolves_project_code_from_cwd() {
    // REQ-AXO-089 — when `project` arg is omitted, retrieve_context
    // must auto-resolve from AXON_PROJECT_ROOT (or cwd) by matching
    // against ProjectCodeRegistry, like the global CLAUDE.md promises
    // ("project_code is auto-resolved from your working directory").
    // Previously the tool fell through to workspace:* whenever the
    // caller skipped the arg, making answers from inside a project
    // directory look workspace-wide.
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry(
            "AXO",
            Some("axon"),
            Some("/home/test/axon-cwd-fixture"),
        )
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axon-cwd-fixture");
    }
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "where is missing_symbol defined" }
            })),
            id: Some(json!(89001)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("project:AXO") || content.contains("Scope:** `project:AXO`"),
        "scope must be project:AXO when AXON_PROJECT_ROOT matches a registered project; got: {content}"
    );
    assert!(
        !content.contains("workspace:*"),
        "scope must NOT fall through to workspace:* once auto-resolution succeeds; got: {content}"
    );
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_retrieve_context_falls_back_to_workspace_when_cwd_unmatched() {
    // REQ-AXO-089 — when AXON_PROJECT_ROOT doesn't match any
    // registered project, retrieve_context must fall back to
    // workspace:* rather than fail or invent a code. This preserves
    // the historic behaviour for callers running from outside any
    // registered project (e.g., a fresh worktree or a temp dir).
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry(
            "AXO",
            Some("axon"),
            Some("/home/test/axon-cwd-fixture"),
        )
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/tmp/unrelated-path");
    }
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "anything goes here" }
            })),
            id: Some(json!(89002)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("workspace:*"),
        "scope must fall back to workspace:* when cwd does not match any registered project; got: {content}"
    );
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_retrieve_context_empty_question_returns_recovery_contract() {
    // REQ-AXO-043 — empty `question` previously returned a bare error
    // string with no operator_guidance, no next_action, and no example.
    // Verify the structured contract.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "   " }
            })),
            id: Some(json!(43101)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response["isError"].as_bool(), Some(true));
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("non-empty") && content.contains("question"),
        "content must explain the missing field: {content}"
    );
    assert!(
        content.contains("example") || content.contains("Pass"),
        "content must include guidance toward a valid call: {content}"
    );

    let data = &response["data"];
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    assert_eq!(data["missing_field"].as_str(), Some("question"));
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert!(!actions.is_empty(), "next_best_actions must be non-empty");
    let follow_up = data["operator_guidance"]["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools");
    let follow_up_strs: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_strs.contains(&"inspect") || follow_up_strs.contains(&"query"),
        "follow_up_tools must point to inspect/query: {follow_up_strs:?}"
    );
}

#[test]
fn test_brain_only_resume_vectorization_stays_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
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
            id: Some(json!(2298)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("resume_vectorization"), "{content}");
    assert!(content.contains("unavailable"), "{content}");
    assert!(content.contains("public brain authority"), "{content}");
    assert!(content.contains("active indexer authority"), "{content}");

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
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE", "false");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_public_surface_and_runtime_truth",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);
    let tempdir = tempdir().unwrap();
    let server = create_test_server_with_distinct_reader(tempdir.path());
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
    assert!(public_tool_names.contains(&"mcp_surface_diagnostics"));
    assert!(public_tool_names.contains(&"project_status"));
    assert!(public_tool_names.contains(&"project_registry_lookup"));
    assert!(public_tool_names.contains(&"soll_relation_schema"));
    assert!(public_tool_names.contains(&"why"));
    assert!(public_tool_names.contains(&"path"));
    assert!(public_tool_names.contains(&"anomalies"));
    assert!(public_tool_names.contains(&"batch"));
    assert!(public_tool_names.contains(&"job_status"));
    assert!(public_tool_names.contains(&"query"));
    assert!(public_tool_names.contains(&"inspect"));
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"impact"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(!public_tool_names.contains(&"resume_vectorization"));
    assert!(public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"cypher"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));
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
    assert!(data["truth_cockpit"].as_object().is_some());
    assert!(data["truth_cockpit"]["next_best_action"]["tool"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["freshness"]["state"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["proof_gaps"].is_array());
    assert_eq!(
        data["next_action"],
        data["truth_cockpit"]["next_best_action"]
    );
    assert_runtime_authority_roles(
        &data["runtime_authority"]["runtime_state"],
        AxonProcessRole::Indexer,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert!(
        data["runtime_authority"]["runtime_state"]["system_converged"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["runtime_state"]["indexer_feed"]["state"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["runtime_state"]["indexer_feed"]["last_good_payload_at_ms"]
            .as_u64()
            .is_some(),
        true
    );
    assert!(
        data["runtime_authority"]["runtime_state"]["ist_snapshot"]["state"]
            .as_str()
            .is_some()
    );
    assert!(data["availability"]["degraded_notes"].as_array().is_some());
    assert_eq!(
        data["async_contract"]["canonical_follow_up_tool"].as_str(),
        Some("job_status")
    );
    assert_eq!(data["async_policy"]["mode"].as_str(), Some("allowlist"));
    assert_eq!(
        data["async_policy"]["sync_by_default"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["async_policy"]["latency_target_p95_ms"].as_i64(),
        Some(200)
    );
    let allowlisted_tools = data["async_policy"]["allowlisted_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowlisted_tools.contains(&"restore_soll"));
    assert!(allowlisted_tools.contains(&"soll_apply_plan"));
    assert!(!allowlisted_tools.contains(&"resume_vectorization"));
    let monitored_sync_tools = data["async_policy"]["monitored_sync_mutation_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(monitored_sync_tools.contains(&"soll_commit_revision"));
    assert_eq!(
        data["utility_first_scheduler"]["state"].as_str(),
        Some("balanced_drain")
    );
    assert!(data["utility_first_scheduler"]["reason"].as_str().is_some());
    assert!(data["utility_first_scheduler"]["ready_reserve_target"]
        .as_u64()
        .is_some());
    assert_eq!(
        data["async_contract"]["stale_client_binding_possible"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["canonical_sources"]["soll_export"]["reimportable"].as_bool(),
        Some(true)
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["target"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["effective"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["clamp_visible"]
            .as_bool()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["authority_state"].as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["effective"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["authority_state"].as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["chunk_batch_size"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["file_vectorization_batch_size"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["target"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["effective"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_ready_queue_depth")
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["authority_state"]
            .as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_persist_queue_bound"]["seed"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_persist_queue_bound"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_persist_queue_depth")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_max_inflight_persists"]["seed"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_max_inflight_persists"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_persist_claims")
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["queue_persist_effective_semantics"]
            ["vector_ready_queue_depth"]
            .as_str(),
        Some("observed_current_queue_depth_not_capacity")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["seed"]["sleep_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["seed"]["profile"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["target"]["idle_sleep_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["effective"]["pause"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["controller_state"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["authority_state"]
            .as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["gpu_vector_lease"]["exclusive_required"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["gpu_vector_lease"]["path"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available_in_mode"].as_str(),
        Some("full")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["authority_state"].as_str(),
        Some("transitional")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["wake_contract_state"].as_str(),
        Some("fragmented")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["wake_observability_state"].as_str(),
        Some("partial")
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["graph_backlog_depth"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["graph_projection_queue_depth"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["operator_focus"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["focus_recommendation"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["confidence"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["wake_noise_level"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["dominant_wake_share_pct"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["measurement_readiness"]
            .as_str()
            .is_some()
    );
    assert!(data["runtime_authority"]["quiescent_state"]["diagnosis"]
        ["recommended_next_measurement"]
        .as_str()
        .is_some());
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["qualification_verdict"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["qualification_reason"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["actionable_now"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["blocking_factors"]
            .as_array()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["loop_intervals_ms"]["reader_refresh"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["wakeups_last_60s"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_wakeup_at_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["resume_latency_p95_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["useful_resume_latency_p95_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_quiescent_exit_reason"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_wake_source"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["dominant_wake_source"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["last_background_wake_detail"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["dominant_background_wake_detail"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["background_wake_ingress_promoter_total"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["background_wake_autonomous_ingestor_total"]
            .as_u64()
            .is_some()
    );
    assert!(data["runtime_authority"]["quiescent_state"]["diagnosis"]
        ["dominant_background_wake_detail"]
        .as_str()
        .is_some());
    assert!(
        data["runtime_authority"]["quiescent_state"]["lane_liveness"]
            ["vector_worker_heartbeat_age_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["lane_liveness"]["vector_lane_state"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["observed_residual_work"]
            ["ready_queue_depth_current"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["measurement_window_sec"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]["state"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["recommendation"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["files_vector_ready_last_minute"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["chunks_embedded_last_minute"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_requested"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_effective"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["gpu_access_policy"]
            .as_str()
            .is_some()
    );

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    service_guard::set_runtime_truth_feed_for_tests(
        Some(1_000),
        Some(900),
        50,
        Some("indexer_feed_heartbeat_stale"),
    );
    let degraded = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();
    let degraded_data = degraded.get("data").unwrap();
    assert_eq!(
        degraded_data["runtime_authority"]["runtime_state"]["indexer_feed"]["stale"].as_bool(),
        Some(true)
    );
    assert_eq!(
        degraded_data["runtime_authority"]["runtime_state"]["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(degraded_data["truth_status"].as_str(), Some("degraded"));
    assert!(degraded_data["availability"]["degraded_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str().is_some()));

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
    }
    let now_ms = now_ms_for_tests();
    service_guard::set_runtime_truth_feed_for_tests(
        Some(now_ms),
        Some(now_ms.saturating_sub(100)),
        60_000,
        Some("indexer_feed_partial_runtime_truth"),
    );
    let degraded_but_fresh = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2204)),
        })
        .unwrap()
        .result
        .unwrap();
    let degraded_but_fresh_data = degraded_but_fresh.get("data").unwrap();
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["indexer_feed"]["state"]
            .as_str(),
        Some("degraded")
    );
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["indexer_feed"]["stale"]
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        degraded_but_fresh_data["truth_status"].as_str(),
        Some("degraded")
    );
    assert!(degraded_but_fresh_data["availability"]["degraded_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("indexer_feed_partial_runtime_truth")));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
        std::env::remove_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE");
    }
}

#[test]
fn test_initialize_reports_brain_server_identity_when_shadow_role_is_brain() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "brain");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "1");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_initialize_reports_brain_server_identity_when_shadow_role_is_brain",
        );
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "codex-test", "version": "0.0.0" }
            })),
            id: Some(json!(2201)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(response["protocolVersion"].as_str(), Some("2025-11-25"));
    assert_eq!(response["serverInfo"]["name"].as_str(), Some("axon-brain"));
    assert_eq!(response["serverInfo"]["version"].as_str(), Some("2.2.0"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_reports_brain_and_indexer_authorities() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    let tempdir = tempdir().unwrap();
    let server = create_test_server_with_distinct_reader(tempdir.path());

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "brain");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "1");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_brain_and_indexer_authorities_brain",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let brain_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2207)),
        })
        .unwrap()
        .result
        .unwrap();

    let brain_runtime_state = &brain_response["data"]["runtime_authority"]["runtime_state"];
    assert_runtime_authority_roles(
        brain_runtime_state,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert_eq!(brain_runtime_state["brain_ready"].as_bool(), Some(true));
    assert_eq!(brain_runtime_state["indexer_ready"].as_bool(), Some(false));
    assert_eq!(
        brain_runtime_state["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        brain_response["data"]["truth_status"].as_str(),
        Some("degraded")
    );

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "indexer");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_brain_and_indexer_authorities_indexer",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let indexer_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2208)),
        })
        .unwrap()
        .result
        .unwrap();

    let indexer_runtime_state = &indexer_response["data"]["runtime_authority"]["runtime_state"];
    assert_runtime_authority_roles(
        indexer_runtime_state,
        AxonProcessRole::Indexer,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert_eq!(indexer_runtime_state["brain_ready"].as_bool(), Some(false));
    assert_eq!(indexer_runtime_state["indexer_ready"].as_bool(), Some(true));
    assert_eq!(
        indexer_runtime_state["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        indexer_response["data"]["truth_status"].as_str(),
        Some("degraded")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_exposes_tensorrt_ready_vector_pipeline_telemetry() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_exposes_tensorrt_ready_vector_pipeline_telemetry",
        );
        std::env::set_var("AXON_TENSORRT_CACHE_DIR", "/tmp/axon-tensorrt-cache");
    }
    service_guard::record_vector_prepare_reply_wait_ms(3);
    service_guard::record_vector_prepare_send_wait_ms(5);
    service_guard::record_vector_prepare_queue_wait_ms(7);
    service_guard::record_vector_gpu_idle_wait_ms(11);
    service_guard::record_vector_embed_breakdown(13, 17);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::DbWrite, 19);
    service_guard::record_vector_persist_send_wait_ms(23);
    service_guard::record_vector_persist_queue_wait_ms(29);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::MarkDone, 31);
    service_guard::record_vector_finalize_send_wait_ms(37);
    service_guard::record_vector_finalize_queue_wait_ms(41);

    let tempdir = tempdir().unwrap();
    let server = create_test_server_with_distinct_reader(tempdir.path());
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();

    let telemetry = &response["data"]["runtime_authority"]["vector_pipeline_telemetry"];
    assert_eq!(
        telemetry["contract"].as_str(),
        Some("tensorrt_ready_vector_pipeline_v1")
    );
    assert_eq!(telemetry["production_lanes"][0].as_str(), Some("graph"));
    assert_eq!(telemetry["production_lanes"][1].as_str(), Some("vector"));
    assert_eq!(telemetry["stage_totals"]["prepare_ms"].as_u64(), Some(15));
    assert_eq!(
        telemetry["stage_totals"]["ready_wait_ms"].as_u64(),
        Some(11)
    );
    assert_eq!(telemetry["stage_totals"]["inference_ms"].as_u64(), Some(13));
    assert_eq!(
        telemetry["stage_totals"]["output_extract_ms"].as_u64(),
        Some(17)
    );
    assert_eq!(telemetry["stage_totals"]["persist_ms"].as_u64(), Some(71));
    assert_eq!(
        telemetry["provider"]["tensorrt_cache_dir"].as_str(),
        Some("/tmp/axon-tensorrt-cache")
    );
    assert!(telemetry["provider"]["effective_strategy"].is_string());
    assert!(telemetry["provider"]["fallback_count"].is_u64());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
        std::env::remove_var("AXON_TENSORRT_CACHE_DIR");
    }
}

#[test]
fn test_status_brain_exposes_indexer_runtime_telemetry_from_heartbeat() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    let tempdir = tempdir().unwrap();
    let server = create_test_server_with_distinct_reader(tempdir.path());
    let run_root = tempdir.path().join(".axon-dev").join("run-indexer");
    std::fs::create_dir_all(&run_root).unwrap();
    let heartbeat_path = run_root.join("runtime-heartbeat.json");
    std::fs::write(
        &heartbeat_path,
        serde_json::to_vec_pretty(&json!({
            "runtime_mode": "indexer_full",
            "release_version": "0.7.0",
            "build_id": "v0.7.0-test",
            "install_generation": "workspace",
            "last_heartbeat_at_ms": 1234,
            "last_good_payload_at_ms": 1234,
            "stale_after_ms": 5000,
            "stale": false,
            "degraded_reason": null,
            "runtime_truth_feed": {
                "stale": false,
                "observed_age_ms": 0,
                "stale_after_ms": 5000,
                "last_heartbeat_at_ms": 1234,
                "last_good_payload_at_ms": 1234,
                "degraded_reason": null
            },
            "runtime_telemetry": {
                "ingress_enabled": true,
                "ingress_buffered_entries": 144,
                "ingress_hot_entries": 12,
                "ingress_scan_entries": 132,
                "ingress_subtree_hints": 3,
                "ingress_subtree_hint_in_flight": 1,
                "ingress_subtree_hint_accepted_total": 9,
                "ingress_subtree_hint_blocked_total": 2,
                "ingress_subtree_hint_suppressed_total": 4,
                "ingress_flush_count": 7,
                "ingress_last_flush_duration_ms": 18,
                "ingress_last_promoted_count": 96,
                "graph_projection_queue": {
                    "queued": 55,
                    "inflight": 8,
                    "total": 63
                },
                "file_vectorization_queue": {
                    "queued": 4,
                    "inflight": 2,
                    "total": 6
                },
                "claim_mode": "fast",
                "service_pressure": "healthy",
                "utility_first_scheduler_state": "balanced_drain",
                "utility_first_scheduler_reason": "semantic_underfed",
                "semantic_underfeed": true
            }
        }))
        .unwrap(),
    )
    .unwrap();

    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", tempdir.path());
        std::env::set_var("AXON_INSTANCE_KIND", "dev");
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "brain");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "0");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_brain_exposes_indexer_runtime_telemetry_from_heartbeat",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2210)),
        })
        .unwrap()
        .result
        .unwrap();

    let indexer_runtime =
        &response["data"]["runtime_authority"]["runtime_state"]["indexer_runtime"];
    assert_eq!(indexer_runtime["available"].as_bool(), Some(true));
    assert_eq!(
        indexer_runtime["telemetry_source"].as_str(),
        Some("runtime_heartbeat")
    );
    assert_eq!(
        indexer_runtime["telemetry"]["ingress_buffered_entries"].as_u64(),
        Some(144)
    );
    assert_eq!(
        indexer_runtime["telemetry"]["ingress_scan_entries"].as_u64(),
        Some(132)
    );
    assert_eq!(
        indexer_runtime["telemetry"]["ingress_hot_entries"].as_u64(),
        Some(12)
    );
    assert_eq!(
        indexer_runtime["telemetry"]["ingress_last_promoted_count"].as_u64(),
        Some(96)
    );
    assert_eq!(
        indexer_runtime["telemetry"]["graph_projection_queue"]["total"].as_u64(),
        Some(63)
    );

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
        std::env::remove_var("AXON_INSTANCE_KIND");
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_indexer_omits_soll_mcp_job_counts() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "indexer");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "0");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_indexer_omits_soll_mcp_job_counts",
        );
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["runtime_authority"]["runtime_state"]["process_role"].as_str(),
        Some(AxonProcessRole::Indexer.as_str())
    );
    assert_eq!(data["job_counts"].as_array().map(Vec::len), Some(0));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_reports_ist_alias_writer_path_is_explicitly_degraded() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_ist_alias_writer_path_is_explicitly_degraded",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2206)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let runtime_state = &data["runtime_authority"]["runtime_state"];
    assert_eq!(
        runtime_state["ist_snapshot"]["state"].as_str(),
        Some("degraded")
    );
    assert_eq!(
        runtime_state["ist_snapshot"]["trust_boundary"].as_str(),
        Some("graph_store.writer_alias_direct_read")
    );
    assert_eq!(
        runtime_state["ist_snapshot"]["read_path"].as_str(),
        Some("writer_alias_direct")
    );
    assert_eq!(
        runtime_state["ist_snapshot"]["unsafe_read"].as_bool(),
        Some(true)
    );
    assert_eq!(runtime_state["system_converged"].as_bool(), Some(false));
    assert_eq!(
        runtime_state["ist_snapshot"]["degraded_reason"].as_str(),
        Some("ist_reader_aliases_writer_direct_path")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_reports_ist_snapshot_degraded_when_reader_state_is_unstable() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_ist_snapshot_degraded_when_reader_state_is_unstable",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let server = create_test_server();
    let now_ms = now_ms_for_tests();
    {
        let mut reader_guard = server
            .graph_store
            .pool
            .reader_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *reader_guard = std::ptr::null_mut();
    }
    server
        .graph_store
        .reader_state
        .commit_epoch
        .store(14, std::sync::atomic::Ordering::Relaxed);
    server
        .graph_store
        .reader_state
        .reader_epoch
        .store(5, std::sync::atomic::Ordering::Relaxed);
    server
        .graph_store
        .reader_state
        .refresh_requested_epoch
        .store(14, std::sync::atomic::Ordering::Relaxed);
    server
        .graph_store
        .reader_state
        .refresh_inflight
        .store(true, std::sync::atomic::Ordering::Relaxed);
    server
        .graph_store
        .reader_state
        .last_refresh_started_ms
        .store(now_ms, std::sync::atomic::Ordering::Relaxed);
    server
        .graph_store
        .reader_state
        .last_refresh_completed_ms
        .store(now_ms, std::sync::atomic::Ordering::Relaxed);

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2205)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let runtime_state = &data["runtime_authority"]["runtime_state"];
    assert_eq!(
        runtime_state["ist_snapshot"]["state"].as_str(),
        Some("degraded")
    );
    assert_eq!(
        runtime_state["ist_snapshot"]["trust_boundary"].as_str(),
        Some("graph_store.reader_snapshot_diagnostics")
    );
    assert_eq!(
        runtime_state["ist_snapshot"]["unsafe_read"].as_bool(),
        Some(true)
    );
    assert!(runtime_state["ist_snapshot"]["degraded_reason"]
        .as_str()
        .is_some());
    assert_eq!(runtime_state["system_converged"].as_bool(), Some(false));
    assert_eq!(data["truth_status"].as_str(), Some("degraded"));
    assert!(data["availability"]["degraded_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("runtime_authority_not_converged")));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_reports_canonical_ingestion_stage_model() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    service_guard::reset_for_tests();
    reset_ingress_metrics_for_tests();

    let mut ingress = IngressBuffer::default();
    ingress.record_file(IngressFileEvent::new(
        "/tmp/watcher-buffered.rs",
        "AXO",
        11,
        111,
        50,
        IngressSource::Watcher,
        IngressCause::Modified,
    ));
    ingress.record_file(IngressFileEvent::new(
        "/tmp/scan-buffered.rs",
        "AXO",
        12,
        112,
        25,
        IngressSource::Scan,
        IngressCause::Discovered,
    ));

    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
             ('/tmp/persisted-only.rs', 'AXO', 'pending', 'promoted', FALSE, FALSE, 10, 10, 10), \
             ('/tmp/graph-actionable.rs', 'AXO', 'pending', 'promoted', FALSE, FALSE, 20, 20, 20), \
             ('/tmp/graph-ready-owned.rs', 'AXO', 'indexed', 'graph_indexed', TRUE, FALSE, 30, 30, 30), \
             ('/tmp/vector-ready.rs', 'AXO', 'indexed', 'graph_indexed', TRUE, TRUE, 40, 40, 40)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at) VALUES \
             ('file', '/tmp/graph-actionable.rs', 2, 'queued', 0, 1), \
             ('file', '/tmp/graph-ready-owned.rs', 2, 'inflight', 1, 2)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
             ('/tmp/graph-ready-owned.rs', 'queued', 1, NULL, NULL, NULL, 'vector-lane', 0)",
        )
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "full" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();

    let model = &response["data"]["runtime_authority"]["canonical_ingestion_stage_model"];
    assert_eq!(model["authority_state"].as_str(), Some("canonical"));
    assert_eq!(
        model["freshness"]["recommended_mode_for_current_counts"].as_str(),
        Some("full")
    );
    assert!(
        response["data"]["runtime_authority"]["limiting_factors"]["primary"]
            .as_str()
            .is_some()
    );
    assert!(
        response["data"]["runtime_authority"]["limiting_factors"]["signals"]["graph_backlog_depth"]
            .as_u64()
            .is_some()
    );
    assert_eq!(model["ingress_buffered"]["current_count"].as_u64(), Some(2));
    assert_eq!(
        model["watcher_buffered"]["ownership_surface"].as_str(),
        Some("ingress_buffer")
    );
    assert!(
        model["watcher_buffered"]["current_count"].as_u64() == Some(1),
        "watcher buffered entries should be reflected exactly"
    );
    assert_eq!(model["scan_buffered"]["current_count"].as_u64(), Some(1));
    assert_eq!(
        model["ingress_promotion"]["ownership_surface"].as_str(),
        Some("ingress_buffer")
    );
    assert!(model["ingress_promotion"]["flush_count"].as_u64().is_some());
    assert!(model["ingress_promotion"]["last_flush_duration_ms"]
        .as_u64()
        .is_some());
    assert!(model["ingress_promotion"]["last_promoted_count"]
        .as_u64()
        .is_some());
    assert!(model["ingress_promotion"]["promoted_total"]
        .as_u64()
        .is_some());
    assert!(model["ingress_promotion"]["last_durably_persisted_count"]
        .as_u64()
        .is_some());
    assert!(model["ingress_promotion"]["durably_persisted_total"]
        .as_u64()
        .is_some());
    assert!(
        model["ingress_promotion"]["last_excluded_from_pending_count"]
            .as_u64()
            .is_some()
    );
    assert!(model["ingress_promotion"]["excluded_from_pending_total"]
        .as_u64()
        .is_some());
    assert_eq!(
        model["persisted_file"]["ownership_surface"].as_str(),
        Some("File")
    );
    assert_eq!(model["persisted_file"]["current_count"].as_u64(), Some(4));
    assert_eq!(
        model["persisted_file_pending"]["ownership_surface"].as_str(),
        Some("File")
    );
    assert_eq!(
        model["persisted_file_pending"]["current_count"].as_u64(),
        Some(2)
    );
    assert_eq!(model["graph_wip"]["current_count"].as_u64(), Some(0));
    assert_eq!(
        model["structural_graph_backlog"]["ownership_surface"].as_str(),
        Some("File")
    );
    assert_eq!(
        model["structural_graph_backlog"]["current_count"].as_u64(),
        Some(2)
    );
    assert_eq!(
        model["structural_graph_backlog"]["queue_breakdown"]["queued"].as_u64(),
        Some(2)
    );
    assert_eq!(
        model["structural_graph_backlog"]["queue_breakdown"]["inflight"].as_u64(),
        Some(0)
    );
    assert_eq!(
        model["graph_projection_queue_owned"]["ownership_surface"].as_str(),
        Some("GraphProjectionQueue")
    );
    assert_eq!(
        model["graph_projection_queue_owned"]["current_count"].as_u64(),
        Some(2)
    );
    assert_eq!(
        model["graph_projection_queue_owned"]["queue_breakdown"]["queued"].as_u64(),
        Some(1)
    );
    assert_eq!(
        model["graph_projection_queue_owned"]["queue_breakdown"]["inflight"].as_u64(),
        Some(1)
    );
    assert_eq!(model["graph_ready"]["current_count"].as_u64(), Some(2));
    assert_eq!(
        model["file_vectorization_queue_owned"]["ownership_surface"].as_str(),
        Some("FileVectorizationQueue")
    );
    assert_eq!(
        model["file_vectorization_queue_owned"]["current_count"].as_u64(),
        Some(1)
    );
    assert_eq!(model["vector_ready"]["status"].as_str(), Some("tracked"));
    assert_eq!(model["vector_ready"]["current_count"].as_u64(), Some(1));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_status_reports_compact_machine_status_surface() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE", "false");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    service_guard::reset_for_tests();
    reset_ingress_metrics_for_tests();

    let mut ingress = IngressBuffer::default();
    ingress.record_file(IngressFileEvent::new(
        "/tmp/machine-status-buffered.rs",
        "AXO",
        15,
        115,
        75,
        IngressSource::Scan,
        IngressCause::Discovered,
    ));

    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
             ('/tmp/machine-status-pending.rs', 'AXO', 'pending', 'promoted', FALSE, FALSE, 10, 10, 10), \
             ('/tmp/machine-status-graph-ready.rs', 'AXO', 'indexed', 'graph_indexed', TRUE, FALSE, 20, 20, 20), \
             ('/tmp/machine-status-vector-ready.rs', 'AXO', 'indexed', 'graph_indexed', TRUE, TRUE, 30, 30, 30)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at) VALUES \
             ('file', '/tmp/machine-status-pending.rs', 2, 'queued', 0, 1)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
             ('/tmp/machine-status-graph-ready.rs', 'queued', 1)",
        )
        .unwrap();

    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();
    let machine = &data["machine_status"];

    assert_eq!(machine["source"].as_str(), Some("status_json"));
    assert_eq!(
        machine["truth_status"].as_str(),
        data["truth_status"].as_str()
    );
    assert_eq!(machine["pipeline"]["known"].as_u64(), Some(3));
    assert_eq!(machine["pipeline"]["pending"].as_u64(), Some(1));
    assert_eq!(machine["pipeline"]["graph_ready"].as_u64(), Some(2));
    assert_eq!(machine["pipeline"]["vector_ready"].as_u64(), Some(1));
    assert_eq!(machine["ingress"]["buffered_entries"].as_u64(), Some(1));
    assert_eq!(
        machine["queues"]["graph_projection"]["total"].as_u64(),
        Some(1)
    );
    assert_eq!(
        machine["queues"]["vectorization"]["queued"].as_u64(),
        Some(1)
    );
    assert_eq!(
        machine["vector"]["chunk_embeddings_rate_window_ms"].as_u64(),
        Some(5_000)
    );
    assert_eq!(
        machine["vector"]["ready_queue_chunks_current"].as_u64(),
        Some(0)
    );
    assert_eq!(
        machine["vector"]["prepare_inflight_chunks_current"].as_u64(),
        Some(0)
    );
    assert_eq!(
        machine["vector"]["ready_replenishment_deficit_current"].as_u64(),
        Some(0)
    );
    assert_eq!(
        machine["vector"]["graph_workers_active_current"].as_u64(),
        Some(0)
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available_in_mode"].as_str(),
        Some("full")
    );
    assert_eq!(
        machine["blocking"]["dominant"].as_str(),
        Some("vector_backlog_present")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE");
    }
}

#[test]
fn test_status_reports_priority_contract_for_watcher_first_pipeline() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE", "false");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    service_guard::reset_for_tests();
    reset_ingress_metrics_for_tests();

    let mut ingress = IngressBuffer::default();
    ingress.record_file(IngressFileEvent::new(
        "/tmp/watcher-hot.rs",
        "AXO",
        9,
        99,
        100,
        IngressSource::Watcher,
        IngressCause::Modified,
    ));

    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at) VALUES \
             ('file', '/tmp/graph-priority.rs', 2, 'queued', 0, 1)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) VALUES \
             ('/tmp/graph-priority.rs', 'AXO', 'pending', 'promoted', FALSE, FALSE, 10, 10, 10), \
             ('/tmp/vector-backlog.rs', 'AXO', 'indexed', 'graph_indexed', TRUE, FALSE, 30, 30, 30)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
             ('/tmp/vector-backlog.rs', 'queued', 1)",
        )
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "full" }
            })),
            id: Some(json!(2204)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        response["data"]["runtime_authority"]["proposed_control_model"].as_str(),
        Some("admission_first_stock_control")
    );
    let loop_semantics = &response["data"]["runtime_authority"]["loop_semantics"];
    assert_eq!(
        loop_semantics["upstream_push_loop"]["mode"].as_str(),
        Some("push")
    );
    assert_eq!(
        loop_semantics["upstream_push_loop"]["summary_scope"].as_str(),
        Some("high_level_loop_summary")
    );
    assert_eq!(
        loop_semantics["upstream_push_loop"]["boundary"].as_str(),
        Some("buffered_discovery_to_graph_ready")
    );
    assert_eq!(
        loop_semantics["upstream_push_loop"]["critical_throughput_stock"].as_str(),
        Some("persisted_file_pending")
    );
    assert_eq!(
        loop_semantics["gpu_paced_downstream_loop"]["mode"].as_str(),
        Some("pull")
    );
    assert_eq!(
        loop_semantics["gpu_paced_downstream_loop"]["boundary"].as_str(),
        Some("graph_ready_to_vector_ready")
    );
    assert_eq!(
        loop_semantics["gpu_paced_downstream_loop"]["idle_when_source_stock_empty"].as_bool(),
        Some(true)
    );
    assert_eq!(loop_semantics["finalize"]["mode"].as_str(), Some("async"));
    assert_eq!(
        loop_semantics["finalize"]["hot_path"].as_bool(),
        Some(false)
    );
    let admission = &response["data"]["runtime_authority"]["admission_controller"];
    let edges = &response["data"]["runtime_authority"]["canonical_edges"];
    assert_eq!(admission["owner"].as_str(), Some("admission_controller"));
    assert_eq!(admission["control_model_state"].as_str(), Some("proposed"));
    assert_eq!(
        admission["admission_completion_surface"].as_str(),
        Some("File(status='pending', graph_ready=FALSE, eligible_for_graph=TRUE)")
    );
    assert_eq!(admission["buffered_discovery_current"].as_u64(), Some(1));
    assert_eq!(
        admission["persisted_file_pending_current"].as_u64(),
        Some(1)
    );
    assert_eq!(admission["graph_wip_current"].as_u64(), Some(0));
    assert_eq!(admission["admission_wip_current"].as_u64(), Some(0));
    assert_eq!(admission["blocking_authority"].as_str(), Some("none"));
    assert_eq!(admission["allowed_by_contract"].as_bool(), Some(true));
    assert_eq!(
        admission["allowed_under_current_runtime"].as_bool(),
        Some(true)
    );
    assert!(admission["target_band"].as_u64().is_some());
    assert!(admission["reorder_point"].as_u64().is_some());
    assert!(admission["max_wip"].as_u64().is_some());
    assert!(admission["hold_window_ms"].as_u64().is_some());
    assert!(admission["forced_bulk_fill_threshold"].as_u64().is_some());
    assert!(admission["admission_flush_count"].as_u64().is_some());
    assert!(admission["admission_last_flush_duration_ms"]
        .as_u64()
        .is_some());
    assert!(admission["admission_last_promoted_count"]
        .as_u64()
        .is_some());
    assert!(admission["admission_promoted_total"].as_u64().is_some());
    assert!(admission["admission_last_durably_persisted_count"]
        .as_u64()
        .is_some());
    assert!(admission["admission_durably_persisted_total"]
        .as_u64()
        .is_some());
    assert!(admission["admission_last_excluded_from_pending_count"]
        .as_u64()
        .is_some());
    assert!(admission["admission_excluded_from_pending_total"]
        .as_u64()
        .is_some());
    assert!(
        admission["admission_completion_diagnostics"]["flush_happened"]
            .as_bool()
            .is_some()
    );
    assert!(
        admission["admission_completion_diagnostics"]["durable_file_persistence_completed"]
            .as_bool()
            .is_some()
    );
    assert!(
        admission["admission_completion_diagnostics"]["persisted_but_excluded_from_pending"]
            .as_bool()
            .is_some()
    );
    assert_eq!(
        edges["admission_edge"]["owner"].as_str(),
        Some("admission_controller")
    );
    assert_eq!(
        edges["admission_edge"]["blocking_authority"].as_str(),
        Some("none")
    );
    assert_eq!(
        edges["admission_edge"]["allowed_by_contract"].as_bool(),
        Some(true)
    );
    assert_eq!(
        edges["admission_edge"]["allowed_under_current_runtime"].as_bool(),
        Some(true)
    );
    assert_eq!(
        edges["graph_production_edge"]["owner"].as_str(),
        Some("graph_production_controller")
    );
    assert_eq!(
        edges["graph_production_edge"]["blocking_authority"].as_str(),
        Some("none")
    );
    assert_eq!(
        edges["graph_production_edge"]["allowed_under_current_runtime"].as_bool(),
        Some(true)
    );
    assert_eq!(
        edges["vector_downstream_edge"]["owner"].as_str(),
        Some("vector_downstream_controller")
    );
    assert_eq!(
        edges["vector_downstream_edge"]["blocking_authority"].as_str(),
        Some("none")
    );
    assert_eq!(
        edges["vector_downstream_edge"]["allowed_by_contract"].as_bool(),
        Some(true)
    );
    assert_eq!(
        edges["vector_downstream_edge"]["allowed_under_current_runtime"].as_bool(),
        Some(true)
    );

    let contract = &response["data"]["runtime_authority"]["priority_contract"];
    let ingestion = &response["data"]["runtime_authority"]["canonical_ingestion_stage_model"];
    assert_eq!(
        contract["contract_version"].as_str(),
        Some("watcher_graph_vector_v1")
    );
    assert_eq!(
        contract["authority_state"].as_str(),
        Some("declared_runtime_truth")
    );
    assert_eq!(
        ingestion["ingress_buffered"]["current_count"].as_u64(),
        Some(1)
    );

    let ordered = contract["pipeline_order"].as_array().unwrap();
    assert_eq!(ordered[0]["lane"].as_str(), Some("watcher_identification"));
    assert_eq!(ordered[0]["priority"].as_str(), Some("highest"));
    assert_eq!(
        ordered[0]["admission_requires"].as_array().unwrap().len(),
        0
    );
    assert_eq!(ordered[1]["lane"].as_str(), Some("graphing_after_enqueue"));
    assert_eq!(ordered[1]["priority"].as_str(), Some("second"));
    assert_eq!(
        ordered[1]["admission_requires"][0].as_str(),
        Some("persisted_file")
    );
    assert_eq!(
        ordered[2]["lane"].as_str(),
        Some("vectorization_after_graph_ready")
    );
    assert_eq!(ordered[2]["priority"].as_str(), Some("third"));
    assert_eq!(
        ordered[2]["admission_requires"][0].as_str(),
        Some("graph_ready")
    );

    assert_eq!(
        contract["backlog_scope"]["structural_graph_backlog_depth"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["backlog_scope"]["graph_projection_queue_depth"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["lane_gates"]["watcher_identification"]["backlog_gated"].as_bool(),
        Some(false)
    );
    assert_eq!(
        contract["lane_gates"]["graphing_after_enqueue"]["backlog_gated"].as_bool(),
        Some(true)
    );
    assert_eq!(
        contract["lane_gates"]["graphing_after_enqueue"]["gate_kind"].as_str(),
        Some("upstream_ingress_priority")
    );
    assert_eq!(
        contract["lane_gates"]["vectorization_after_graph_ready"]["backlog_gated"].as_bool(),
        Some(true)
    );
    assert_eq!(
        contract["lane_gates"]["vectorization_after_graph_ready"]["gate_kind"].as_str(),
        Some("soft_priority_gate")
    );
    assert_eq!(
        contract["vectorization_can_advance_ahead_of_graph_backlog"]["allowed_by_contract"]
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        contract["vectorization_can_advance_ahead_of_graph_backlog"]
            ["allowed_under_current_runtime"]
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        contract["vectorization_can_advance_ahead_of_graph_backlog"]["enforcement_state"].as_str(),
        Some("hard_blocked_until_graph_backlog_clears")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::remove_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE");
    }
}

#[test]
fn test_status_reports_admission_exclusion_diagnostics() {
    reset_ingress_metrics_for_tests();
    record_ingress_flush(12, 0, 1, 1);

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "full" }
            })),
            id: Some(json!(2205)),
        })
        .unwrap()
        .result
        .unwrap();

    let admission = &response["data"]["runtime_authority"]["admission_controller"];
    assert_eq!(
        admission["admission_last_durably_persisted_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        admission["admission_last_excluded_from_pending_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        admission["admission_completion_diagnostics"]["durable_file_persistence_completed"]
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        admission["admission_completion_diagnostics"]["persisted_but_excluded_from_pending"]
            .as_bool(),
        Some(true)
    );
}

#[test]
fn test_newly_identified_file_is_enqueued_immediately_for_graph_pipeline() {
    let _guard = env_lock();
    let server = create_test_server();
    let mut ingress = IngressBuffer::default();
    ingress.record_file(IngressFileEvent::new(
        "/tmp/new-graph-hot-path.rs",
        "AXO",
        128,
        42,
        900,
        IngressSource::Watcher,
        IngressCause::Discovered,
    ));

    let batch = ingress.drain_batch(100);
    let promoted = server.graph_store.promote_ingress_batch(&batch).unwrap();

    assert_eq!(promoted.promoted_files, 1);
    assert_eq!(promoted.promoted_tombstones, 0);

    let file_row = server
        .graph_store
        .query_json(
            "SELECT status, status_reason, file_stage, graph_ready FROM File WHERE path = '/tmp/new-graph-hot-path.rs'",
        )
        .unwrap();
    assert!(file_row.contains("pending"), "{file_row}");
    assert!(file_row.contains("watcher_hot_identified"), "{file_row}");
    assert!(file_row.contains("promoted"), "{file_row}");

    let queue_row = server
        .graph_store
        .query_json(
            "SELECT anchor_type, anchor_id, status FROM GraphProjectionQueue WHERE anchor_type = 'file' AND anchor_id = '/tmp/new-graph-hot-path.rs'",
        )
        .unwrap();
    assert!(queue_row.contains("queued"), "{queue_row}");
    assert!(
        queue_row.contains("/tmp/new-graph-hot-path.rs"),
        "{queue_row}"
    );
}

#[test]
fn test_graph_backlog_blocks_vector_priority_until_graph_ready_advances() {
    let _guard = env_lock();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();

    service_guard::record_vector_ready_queue_depth(0);
    service_guard::record_vector_prepare_inflight_depth(0);
    service_guard::record_vector_persist_queue_depth(0);
    service_guard::record_graph_vector_priority_context(1, 16);

    let first =
        current_utility_first_scheduler_diagnostics(1, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(first.state.as_str(), "balanced_drain");
    assert_eq!(first.reason, "semantic_underfed");
    assert!(first.semantic_underfeed, "{first:?}");
    assert_eq!(
        service_guard::vector_runtime_metrics().ready_queue_depth_current,
        0
    );

    service_guard::record_graph_vector_priority_context(0, 16);
    let held =
        current_utility_first_scheduler_diagnostics(0, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(held.state.as_str(), "balanced_drain");

    service_guard::record_graph_vector_priority_context(0, 16);
    let released =
        current_utility_first_scheduler_diagnostics(0, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(released.state.as_str(), "balanced_drain");
    assert!(released.semantic_underfeed, "{released:?}");

    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
}

#[test]
fn test_vectorization_admits_only_graph_ready_files() {
    let server = create_test_server();

    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) VALUES \
             ('/tmp/not-graph-ready.rs', 'PRJ', 'pending', 1, 1, 100, 'promoted', FALSE, FALSE), \
             ('/tmp/oversized.rs', 'PRJ', 'oversized_for_current_budget', 1, 1, 100, 'oversized', TRUE, FALSE), \
             ('/tmp/graph-ready-orphan.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-graph-ready-orphan', 'symbol', 'sym-graph-ready-orphan', 'PRJ', '/tmp/graph-ready-orphan.rs', 'function', 'body', 'hash-graph-ready-orphan', 1, 1)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES \
             ('/tmp/graph-ready-orphan.rs', 'sym-graph-ready-orphan', 'PRJ')",
        )
        .unwrap();

    server
        .graph_store
        .enqueue_file_vectorization_refresh("/tmp/not-graph-ready.rs")
        .unwrap();
    server
        .graph_store
        .enqueue_file_vectorization_refresh("/tmp/oversized.rs")
        .unwrap();

    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/not-graph-ready.rs'"
            )
            .unwrap(),
        0
    );
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/oversized.rs'"
            )
            .unwrap(),
        0
    );

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "resume_vectorization",
            "arguments": {}
        })),
        id: Some(json!(2205)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(
        response["data"]["queued_files"].as_u64(),
        Some(1),
        "{response:?}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/graph-ready-orphan.rs'"
            )
            .unwrap(),
        1
    );
}

#[test]
fn test_status_reports_retrieve_context_in_public_surface_when_full_autonomous() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    assert!(!public_tool_names.contains(&"resume_vectorization"));
    assert!(public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"cypher"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_status_reports_information_surface_in_brain_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_information_surface_in_brain_only",
        );
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
            id: Some(json!(22022)),
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
    assert!(public_tool_names.contains(&"query"));
    assert!(public_tool_names.contains(&"inspect"));
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"impact"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(!public_tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_mcp_surface_diagnostics_exposes_server_truth_and_binding_caveat() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_PUBLIC_HOST", "192.168.1.50");
        std::env::set_var("AXON_PUBLIC_HOST_SOURCE", "explicit");
        std::env::set_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE", "1");
        std::env::set_var("AXON_MCP_PUBLIC_URL", "http://192.168.1.50:44129/mcp");
        std::env::set_var("AXON_SQL_PUBLIC_URL", "http://192.168.1.50:44129/sql");
        std::env::set_var("AXON_DASHBOARD_PUBLIC_URL", "http://192.168.1.50:44127/");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_surface_diagnostics",
                "arguments": { "mode": "json" }
            })),
            id: Some(json!(22022)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(
        data["async_contract"]["canonical_follow_up_tool"].as_str(),
        Some("job_status")
    );
    assert_eq!(data["async_policy"]["mode"].as_str(), Some("allowlist"));
    let allowlisted_tools = data["async_policy"]["allowlisted_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowlisted_tools.contains(&"restore_soll"));
    assert!(allowlisted_tools.contains(&"soll_apply_plan"));
    assert!(!allowlisted_tools.contains(&"resume_vectorization"));
    assert_eq!(
        data["client_binding_notes"]["stale_client_binding_possible"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["client_binding_notes"]["session_freshness_status"].as_str(),
        Some("unknown_outside_server")
    );
    assert!(
        data["client_binding_notes"]["canonical_refresh_instruction"]
            .as_str()
            .unwrap_or_default()
            .contains("Refresh or reconnect")
    );
    assert_eq!(
        data["advertised_endpoints"]["available"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["advertised_endpoints"]["mcp_url"].as_str(),
        Some("http://192.168.1.50:44129/mcp")
    );
    assert_eq!(
        data["client_binding_notes"]["external_endpoint_rule"].as_str(),
        Some("Do not use instance_identity.*_url as an external endpoint. Isolated clients must prefer advertised_endpoints.* when available.")
    );
    let critical_tools = data["server_truth"]["critical_tools"].as_array().unwrap();
    assert!(critical_tools
        .iter()
        .any(|value| value.as_str() == Some("project_registry_lookup")));
    assert!(critical_tools
        .iter()
        .any(|value| value.as_str() == Some("axon_init_project")));

    unsafe {
        std::env::remove_var("AXON_PUBLIC_HOST");
        std::env::remove_var("AXON_PUBLIC_HOST_SOURCE");
        std::env::remove_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE");
        std::env::remove_var("AXON_MCP_PUBLIC_URL");
        std::env::remove_var("AXON_SQL_PUBLIC_URL");
        std::env::remove_var("AXON_DASHBOARD_PUBLIC_URL");
    }
}

#[test]
fn test_status_exposes_runtime_version_identity() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RELEASE_VERSION", "0.7.0");
        std::env::set_var("AXON_BUILD_ID", "v0.7.0-rc1-12-gabcdef");
        std::env::set_var("AXON_PACKAGE_VERSION", "0.7.0");
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
        Some("0.7.0")
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
fn test_status_exposes_advertised_endpoints_separately_from_runtime_local_urls() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_URL", "http://127.0.0.1:44129/mcp");
        std::env::set_var("AXON_SQL_URL", "http://127.0.0.1:44129/sql");
        std::env::set_var("AXON_DASHBOARD_URL", "http://127.0.0.1:44127/");
        std::env::set_var("AXON_PUBLIC_HOST", "192.168.1.50");
        std::env::set_var("AXON_PUBLIC_HOST_SOURCE", "derived");
        std::env::set_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE", "1");
        std::env::set_var("AXON_MCP_PUBLIC_URL", "http://192.168.1.50:44129/mcp");
        std::env::set_var("AXON_SQL_PUBLIC_URL", "http://192.168.1.50:44129/sql");
        std::env::set_var("AXON_DASHBOARD_PUBLIC_URL", "http://192.168.1.50:44127/");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["instance_identity"]["mcp_url"].as_str(),
        Some("http://127.0.0.1:44129/mcp")
    );
    assert_eq!(
        data["advertised_endpoints"]["available"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["advertised_endpoints"]["public_host_source"].as_str(),
        Some("derived")
    );
    assert_eq!(
        data["advertised_endpoints"]["mcp_url"].as_str(),
        Some("http://192.168.1.50:44129/mcp")
    );
    assert_eq!(
        data["client_reachability_notes"]["instance_identity_is_runtime_local_only"].as_bool(),
        Some(true)
    );

    unsafe {
        std::env::remove_var("AXON_MCP_URL");
        std::env::remove_var("AXON_SQL_URL");
        std::env::remove_var("AXON_DASHBOARD_URL");
        std::env::remove_var("AXON_PUBLIC_HOST");
        std::env::remove_var("AXON_PUBLIC_HOST_SOURCE");
        std::env::remove_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE");
        std::env::remove_var("AXON_MCP_PUBLIC_URL");
        std::env::remove_var("AXON_SQL_PUBLIC_URL");
        std::env::remove_var("AXON_DASHBOARD_PUBLIC_URL");
    }
}
