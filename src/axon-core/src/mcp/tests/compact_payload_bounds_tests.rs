// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification tests for REQ-AXO-902537:
//! Les modes brief/json compacts doivent borner content ET structuredContent,
//! avec pointeurs de détail.

use super::create_test_server;
use crate::mcp::protocol::JsonRpcRequest;
use serde_json::json;

#[test]
fn test_project_status_brief_bounds_content_and_structured_content() {
    let server = create_test_server();
    let long_desc = "North-star vision for AXO. ".repeat(20);
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Sovereign Truth', '{}', 'current', '{{}}')",
            long_desc
        ))
        .unwrap();

    // Brief mode call
    let req_brief = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "project_status",
            "arguments": { "project_code": "AXO", "mode": "brief" }
        })),
        id: Some(json!(1)),
    };
    let res_brief = server.handle_request(req_brief).unwrap().result.expect("brief result");
    let content_text = res_brief["content"][0]["text"].as_str().unwrap();
    let data_brief = &res_brief["data"];

    // 1. Content size is strictly bounded
    assert!(
        content_text.len() <= 3_000,
        "content text exceeds 3000 chars: {} chars",
        content_text.len()
    );

    // 2. Structured data size is bounded
    let serialized_brief = serde_json::to_vec(data_brief).unwrap();
    assert!(
        serialized_brief.len() <= 10_000,
        "brief structuredContent exceeds 10 KB budget: {} bytes",
        serialized_brief.len()
    );

    // 3. Vision in brief mode is compact with identity and detail pointer
    let vision = &data_brief["vision"];
    assert_eq!(vision["id"], "VIS-AXO-001");
    assert_eq!(vision["title"], "Axon Sovereign Truth");
    assert!(vision.get("summary").is_some(), "vision must have summary");
    assert!(vision["body_chars"].as_u64().unwrap_or(0) > 0);
    assert_eq!(
        vision["expand_with"]["tool"], "soll_get",
        "vision must offer soll_get detail pointer"
    );
    assert_eq!(vision["expand_with"]["arguments"]["id"], "VIS-AXO-001");

    // 4. Detail continuation and omitted subtrees are present
    assert!(data_brief["omitted_in_brief"].as_array().is_some());
    assert_eq!(
        data_brief["detail_continuation"]["tool"], "project_status"
    );
    assert_eq!(
        data_brief["detail_continuation"]["arguments"]["mode"], "verbose"
    );

    // 5. Expand pointers for sub-sections
    assert_eq!(data_brief["expand_anomalies"]["tool"], "anomalies");
    assert_eq!(data_brief["expand_conception"]["tool"], "conception_view");
    assert_eq!(data_brief["expand_soll_context"]["tool"], "soll_query_context");

    // Verbose mode call for size comparison
    let req_verbose = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "project_status",
            "arguments": { "project_code": "AXO", "mode": "verbose" }
        })),
        id: Some(json!(2)),
    };
    let res_verbose = server.handle_request(req_verbose).unwrap().result.expect("verbose result");
    let serialized_verbose = serde_json::to_vec(&res_verbose["data"]).unwrap();

    // Regression check: brief must be strictly more compact than verbose
    assert!(
        serialized_brief.len() < serialized_verbose.len(),
        "brief data ({} bytes) must be strictly smaller than verbose data ({} bytes)",
        serialized_brief.len(),
        serialized_verbose.len()
    );
}

#[test]
fn test_soll_work_plan_compact_omits_long_descriptions_and_provides_pointers() {
    let server = create_test_server();
    let long_desc = "Detailed operational acceptance criteria and specification. ".repeat(15);
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-AXO-101', 'Requirement', 'AXO', 'Sub-second wake', '{}', 'planned', '{{\"priority\":\"P1\"}}')",
            long_desc
        ))
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('DEC-AXO-101', 'Decision', 'AXO', 'Wake signal', '', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-101', 'REQ-AXO-101', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
        })),
        id: Some(json!(3)),
    };
    let res = server.handle_request(req).unwrap().result.expect("soll_work_plan result");
    let data = &res["data"];

    // 1. Items in waves have executable detail pointers and no long descriptions
    let waves = data["ordered_waves"].as_array().expect("ordered_waves");
    assert!(!waves.is_empty());
    for wave in waves {
        for item in wave["items"].as_array().unwrap() {
            assert!(item.get("description").is_none(), "item must not inline raw description");
            assert_eq!(
                item["expand_with"]["tool"], "soll_get",
                "item must have expand_with -> soll_get"
            );
            assert!(item["expand_with"]["arguments"]["id"].is_string());
        }
    }

    // 2. Top recommendations have executable detail pointers
    let top = data["top_recommendations"].as_array().expect("top_recommendations");
    assert!(!top.is_empty());
    for item in top {
        assert_eq!(
            item["expand_with"]["tool"], "soll_get",
            "top recommendation must have expand_with -> soll_get"
        );
        assert!(item["expand_with"]["arguments"]["id"].is_string());
    }

    // 3. Validation gates requirement_verification is compact and has pointers
    let val_req = &data["validation_gates"]["requirement_verification"];
    assert_eq!(val_req["compact"], true);
    assert_eq!(val_req["expand_with"]["tool"], "soll_work_plan");
}

#[test]
fn test_soll_rrf_trimodal_names_top_results_in_text_without_replicating_bodies() {
    let server = create_test_server();
    let body_req = "Very long body text for REQ-AXO-201. ".repeat(25);
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-AXO-201', 'Requirement', 'AXO', 'Storage Lock Engine', '{}', 'current', '{{\"priority\":\"P1\"}}')",
            body_req
        ))
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('DEC-AXO-201', 'Decision', 'AXO', 'Advisory Lock Architecture', 'Brief decision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-201', 'REQ-AXO-201', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": {
                "project_code": "AXO",
                "mode": "rrf_trimodal",
                "seed_node": "DEC-AXO-201",
                "top": 5
            }
        })),
        id: Some(json!(4)),
    };
    let res = server.handle_request(req).unwrap().result.expect("rrf_trimodal result");
    let content_text = res["content"][0]["text"].as_str().unwrap();
    let data = &res["data"];

    // 1. Content names the related node by id and title
    assert!(
        content_text.contains("REQ-AXO-201"),
        "content must name related node ID: {content_text}"
    );
    assert!(
        content_text.contains("Storage Lock Engine"),
        "content must name related node title: {content_text}"
    );
    assert!(
        content_text.contains("soll_get(id="),
        "content must guide reader to soll_get: {content_text}"
    );

    // 2. Content does NOT replicate the huge body
    assert!(
        !content_text.contains("Very long body text for REQ-AXO-201"),
        "content must not replicate node body"
    );

    // 3. Structured results contain enriched metadata and executable pointers
    let results = data["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "RRF should find 1-hop neighbour");
    let first = &results[0];
    assert_eq!(first["id"], "REQ-AXO-201");
    assert_eq!(first["title"], "Storage Lock Engine");
    assert_eq!(first["entity_type"], "Requirement");
    assert_eq!(first["status"], "current");
    assert!(first.get("rrf_score").is_some());
    assert_eq!(first["expand_with"]["tool"], "soll_get");
    assert_eq!(first["expand_with"]["arguments"]["id"], "REQ-AXO-201");
    assert!(first.get("description").is_none(), "structured result must not duplicate body");
}
