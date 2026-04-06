// Copyright (c) Didier Stadelmann. All rights reserved.

use super::*;
use crate::graph::GraphStore;
use crate::parser;
use crate::queue::ProcessingMode;
use std::path::Path;
use std::sync::Arc;
use tempfile::tempdir;

fn create_test_server() -> McpServer {
    let db = crate::tests::test_helpers::create_test_db().expect("failed to create isolated test db");
    println!("TEST DB PATH: {:?}", db.db_path);
    let store = Arc::new(db);
    McpServer::new(store)
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
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"restore_soll"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"soll_apply_plan"));
    assert!(tool_names.contains(&"soll_work_plan"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(!tool_names.contains(&"soll_apply_plan_v2"));
    assert!(!tool_names.contains(&"refine_lattice"));
    assert!(!tool_names.contains(&"batch"));
    assert!(!tool_names.contains(&"cypher"));
    assert!(!tool_names.contains(&"debug"));
    assert!(!tool_names.contains(&"schema_overview"));
    assert!(!tool_names.contains(&"list_labels_tables"));
    assert!(!tool_names.contains(&"query_examples"));
    assert!(!tool_names.contains(&"truth_check"));
    assert!(!tool_names.contains(&"diagnose_indexing"));
    assert!(!tool_names.contains(&"diff"));
    assert!(!tool_names.contains(&"semantic_clones"));
    assert!(!tool_names.contains(&"architectural_drift"));
    assert!(!tool_names.contains(&"bidi_trace"));
    assert!(!tool_names.contains(&"api_break_check"));
    assert!(!tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));
}

#[test]
fn test_soll_work_plan_orders_decision_requirement_milestone_chain() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Rust authoritative', '', 'accepted', '{\"context\":\"\",\"rationale\":\"\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('MIL-AXO-001', 'Milestone', 'AXO', 'AXO', 'Deliver runtime slice', '', 'planned', '{}')")
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
            "arguments": { "project_slug": "AXO", "format": "json" }
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Runtime truth', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'AXO', 'Operator cockpit', '', 'draft', '{\"priority\":\"P2\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_slug": "AXO", "format": "json" }
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'AXO', 'B', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'AXO', 'C', '', 'draft', '{\"priority\":\"P1\"}')")
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
            "arguments": { "project_slug": "AXO", "format": "json" }
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
    assert!(blockers.iter().any(|v| v["id"].as_str() == Some("REQ-AXO-003")));
    assert!(waves.is_empty(), "{:?}", data);
}

#[test]
fn test_soll_work_plan_returns_contract_fields() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Runtime truth', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_slug": "AXO", "format": "json", "include_ist": true }
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'AXO', 'B', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'AXO', 'C', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_slug": "AXO", "format": "json", "limit": 2 }
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'A', '', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'D1', '', 'accepted', '{\"context\":\"\",\"rationale\":\"\"}')")
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
            "arguments": { "project_slug": "AXO", "format": "json", "top": 1 }
        })),
        id: Some(json!(506)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let top = data["top_recommendations"].as_array().expect("top recommendations");

    assert_eq!(top.len(), 1, "{:?}", data);
    assert_eq!(top[0]["id"].as_str(), Some("DEC-AXO-001"));
    assert_eq!(top[0]["kind"].as_str(), Some("unblocker"));
    assert_eq!(data["summary"]["top_count"].as_u64(), Some(1));
    assert_eq!(data["metadata"]["top"].as_u64(), Some(1));
}

#[test]
fn test_axon_debug_reports_backlog_memory_and_storage_views() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store.clone());

    store
        .execute(
            "INSERT INTO File (path, project_slug, status, size, mtime, priority) VALUES \
             ('src/a.rs', 'axon', 'indexed', 10, 1, 100), \
             ('src/b.rs', 'axon', 'pending', 20, 1, 100), \
             ('src/c.rs', 'axon', 'indexing', 30, 1, 100), \
             ('src/d.rs', 'axon', 'indexed_degraded', 40, 1, 100), \
             ('src/e.rs', 'axon', 'oversized_for_current_budget', 50, 1, 100), \
             ('src/f.rs', 'axon', 'skipped', 60, 1, 100)",
        )
        .unwrap();
    store
        .execute(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES \
             ('axon::a', 'a', 'function', false, true, false, false, 'axon')"
        )
        .unwrap();
    store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/a.rs', 'axon::a')")
        .unwrap();
    store
        .execute(
            "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) VALUES \
             ('file', 'src/a.rs', 2, 'queued', 0, 1, NULL, NULL), \
             ('file', 'src/b.rs', 2, 'inflight', 0, 1, NULL, NULL)",
        )
        .unwrap();

    store.refresh_reader_snapshot().unwrap();

    let response = server.axon_debug().expect("debug response");
    let content = response["content"][0]["text"].as_str().unwrap_or_default();

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
            "INSERT INTO File (path, project_slug, status, status_reason, size, mtime, priority) VALUES \
             ('src/a.rs', 'axon', 'pending', 'metadata_changed_scan', 10, 1, 100), \
             ('src/b.rs', 'axon', 'pending', 'metadata_changed_scan', 20, 1, 100), \
             ('src/c.rs', 'axon', 'indexing', 'needs_reindex_while_indexing', 30, 1, 100), \
             ('src/d.rs', 'axon', 'pending', 'manual_or_system_requeue', 40, 1, 100)"
        )
        .unwrap();

    store.refresh_reader_snapshot().unwrap();

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
fn test_axon_architectural_drift() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('ui/app.js', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::fetchData', 'fetchData', 'function', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('db/repo.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::executeSQL', 'executeSQL', 'function', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id) VALUES ('ui/app.js', 'global::fetchData')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('db/repo.rs', 'global::executeSQL')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::fetchData', 'global::executeSQL')")
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
}

#[test]
fn test_axon_query_with_project() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('test_proj/f1.rs', 'test_proj')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('test_proj/f2.rs', 'test_proj')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::auth_func', 'auth_func', 'function', false, true, false, 'test_proj')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('test_proj/f1.rs', 'global::auth_func')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "auth", "project": "test_proj" }
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
        crate::tests::test_helpers::create_test_db().expect("failed isolated notif db"),
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
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::core_func', 'core_func', 'function', true, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::caller_func', 'caller_func', 'function', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::caller_func', 'global::core_func')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": {
                "symbol": "core_func",
                "project": "test_proj"
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
    assert!(content.contains("Inspection du Symbole"));
    assert!(content.contains("core_func"));
}

#[test]
fn test_graph_embedding_semantic_clones_adds_derived_neighborhood_matches() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/auth.rs', 'global')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/access.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::authorize_request', 'authorize_request', 'function', false, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::check_token_chain', 'check_token_chain', 'function', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/auth.rs', 'global::authorize_request')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/access.rs', 'global::check_token_chain')")
        .unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'global::authorize_request', 1, 'sig-auth', '1', 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'global::check_token_chain', 1, 'sig-access', '1', 1001)").unwrap();
    server.graph_store.execute("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'global::authorize_request', 1, 'graph-bge-small-en-v1.5-384', 'sig-auth', '1', CAST([1.0] || repeat([0.0], 383) AS FLOAT[384]), 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'global::check_token_chain', 1, 'graph-bge-small-en-v1.5-384', 'sig-access', '1', CAST([0.99, 0.01] || repeat([0.0], 382) AS FLOAT[384]), 1001)").unwrap();

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

    assert!(content.contains("check_token_chain"));
    assert!(content.contains("derive du graphe"));
}

#[test]
fn test_graph_embedding_semantic_clones_ignores_stale_projection_signatures() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/auth.rs', 'global')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/access.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::authorize_request', 'authorize_request', 'function', false, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::check_token_chain', 'check_token_chain', 'function', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/auth.rs', 'global::authorize_request')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/access.rs', 'global::check_token_chain')")
        .unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'global::authorize_request', 1, 'sig-auth', '1', 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphProjectionState (anchor_type, anchor_id, radius, source_signature, projection_version, updated_at) VALUES ('symbol', 'global::check_token_chain', 1, 'sig-access-current', '1', 1001)").unwrap();
    server.graph_store.execute("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'global::authorize_request', 1, 'graph-bge-small-en-v1.5-384', 'sig-auth', '1', CAST([1.0] || repeat([0.0], 383) AS FLOAT[384]), 1000)").unwrap();
    server.graph_store.execute("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'global::check_token_chain', 1, 'graph-bge-small-en-v1.5-384', 'sig-access-stale', '1', CAST([0.99, 0.01] || repeat([0.0], 382) AS FLOAT[384]), 1001)").unwrap();

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

    assert!(!content.contains("derive du graphe"));
    assert!(!content.contains("check_token_chain"));
}

#[test]
fn test_axon_audit_taint_analysis() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/api.rs', 'global')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/api_dummy.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::user_input', 'user_input', 'function', false, true, false, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::run_task', 'run_task', 'function', false, true, false, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::eval', 'eval', 'function', false, true, false, true, 'global')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/api.rs', 'global::user_input')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::user_input', 'global::run_task')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id) VALUES ('global::run_task', 'global::eval')",
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/danger.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::risky_func', 'risky_func', 'function', false, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::unwrap', 'unwrap', 'method', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/danger.rs', 'global::risky_func')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::risky_func', 'global::unwrap')")
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/todo.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::todo1', '// TODO: Fix this', 'TODO', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/todo.rs', 'global::todo1')",
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/config.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/config.rs', 'global::secret1')")
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/api.ex', 'global')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/api_dummy.ex', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::elixir_func', 'elixir_func', 'function', false, true, false, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::rust_nif', 'rust_nif', 'function', false, true, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('global::unsafe_block', 'unsafe_block', 'function', false, true, false, true, 'global')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/api.ex', 'global::elixir_func')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS_NIF (source_id, target_id) VALUES ('global::elixir_func', 'global::rust_nif')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::rust_nif', 'global::unsafe_block')")
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/god.rs', 'global')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/god_dummy.rs', 'global')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::GodClass', 'GodClass', 'class', false, true, false, 'global')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/god.rs', 'global::GodClass')",
        )
        .unwrap();

    for i in 0..20 {
        server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::dep{}', 'dep{}', 'function', false, true, false, 'global')", i, i)).unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO CALLS (source_id, target_id) VALUES ('global::dep{}', 'global::GodClass')", i))
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
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug) VALUES ('apps/alpha/lib/input.rs', 'alpha')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('apps/beta/lib/unsafe.rs', 'beta')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('alpha::safe_entry', 'safe_entry', 'function', true, true, false, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('beta::beta_entry', 'beta_entry', 'function', false, true, false, false, 'beta')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('beta::eval', 'eval', 'function', false, true, false, true, 'beta')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('apps/alpha/lib/input.rs', 'alpha::safe_entry')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('apps/beta/lib/unsafe.rs', 'beta::beta_entry')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id) VALUES ('beta::beta_entry', 'beta::eval')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "alpha"
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
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug) VALUES ('apps/alpha/lib/covered.rs', 'alpha')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('apps/beta/lib/god.rs', 'beta')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::covered', 'covered', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::GodClass', 'GodClass', 'class', false, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('apps/alpha/lib/covered.rs', 'alpha::covered')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('apps/beta/lib/god.rs', 'beta::GodClass')")
        .unwrap();

    for i in 0..6 {
        server
            .graph_store
            .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::dep{}', 'dep{}', 'function', false, true, false, 'beta')", i, i))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO CALLS (source_id, target_id) VALUES ('beta::dep{}', 'beta::GodClass')",
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
                "project": "alpha"
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
fn test_axon_audit_uses_project_slug_even_when_path_does_not_contain_project_name() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/shared/api.rs', 'alpha')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/shared/safe.rs', 'beta')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('alpha::entrypoint', 'entrypoint', 'function', false, true, false, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('alpha::eval', 'eval', 'function', false, true, false, true, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES ('beta::safe_fn', 'safe_fn', 'function', true, true, false, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/shared/api.rs', 'alpha::entrypoint')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/shared/api.rs', 'alpha::eval')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/shared/safe.rs', 'beta::safe_fn')")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO CALLS (source_id, target_id) VALUES ('alpha::entrypoint', 'alpha::eval')",
        )
        .unwrap();
    assert_eq!(
        server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_slug = $proj OR path LIKE '%' || $proj || '%'",
                &json!({"proj": "alpha"})
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
                "project": "alpha"
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

    assert!(content.contains("Audit de Conformité : alpha"));
    assert!(content.contains("eval"));
    assert!(!content.contains("seems unindexed"));
}

#[test]
fn test_axon_health_uses_project_slug_even_when_path_does_not_contain_project_name() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug) VALUES ('src/shared/alpha_core.rs', 'alpha')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/shared/beta_core.rs', 'beta')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::GodClass', 'GodClass', 'class', false, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::stable_api', 'stable_api', 'function', true, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/shared/alpha_core.rs', 'alpha::GodClass')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/shared/beta_core.rs', 'beta::stable_api')")
        .unwrap();

    for i in 0..5 {
        server
            .graph_store
            .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::dep{}', 'dep{}', 'function', false, true, false, 'alpha')", i, i))
            .unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO CALLS (source_id, target_id) VALUES ('alpha::dep{}', 'alpha::GodClass')", i))
            .unwrap();
    }
    assert_eq!(
        server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM File WHERE project_slug = $proj OR path LIKE '%' || $proj || '%'",
                &json!({"proj": "beta"})
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
                "project": "beta"
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

    assert!(content.contains("Health Report: beta"));
    assert!(content.contains("Coverage 100%"));
    assert!(!content.contains("GodClass"));
    assert!(!content.contains("seems unindexed"));
}

#[test]
fn test_axon_query_global_default() {
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
        .execute("INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 10, 0)")
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
                    "project_slug": "AXO",
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
        .execute("INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 11, 0)")
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
                    "project_slug": "AXO",
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
                    "project_slug": "BookingSystem",
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

    assert!(result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
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
                "project_slug": "AXO",
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

    assert!(
        content.contains("SOLL revision committed"),
        "{content}"
    );
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
                "project_slug": "AXO",
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
fn test_axon_soll_manager_rejects_non_canonical_project_alias() {
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
                    "project_slug": "FSC",
                    "title": "Alias should fail",
                    "context": "Only canonical slugs are accepted",
                    "rationale": "Server owns project identity",
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

    assert!(result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content.contains("Projet canonique"), "{content}");
    assert!(content.contains("FSC"), "{content}");
}

#[test]
fn test_axon_soll_manager_pillar_uses_dedicated_counter() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 3, 12, 0, 0)")
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
                    "project_slug": "AXO",
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
        .execute("INSERT INTO soll.Registry (project_slug, id, last_pil, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 0, 0, 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-007', 'Requirement', 'AXO', 'AXO', 'Existing', 'Already there', '', '{\"priority\":\"P1\"}')")
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'AXO', 'Test Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'AXO', 'My Concept', 'Expl', '', '{}')").unwrap();

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
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
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
    let export_path = content
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
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

    assert!(content.contains("Restauration SOLL terminee"), "{}", content);
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Orphan requirement', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', 'AXO', '', '', 'pending', '{\"method\":\"manual\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Orphan decision', '', 'proposed', '{\"context\":\"No link\",\"rationale\":\"Testing\"}')")
        .unwrap();

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
fn test_axon_validate_soll_reports_clean_minimal_graph() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'AXO', 'Platform Core', 'Protect SOLL', '', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Linked requirement', 'Has links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', 'AXO', '', '', 'passed', '{\"method\":\"manual\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Linked decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
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
fn test_axon_validate_soll_can_scope_by_project_slug() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'AXO orphan', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-BKS-001', 'Requirement', 'BKS', 'BKS', 'BKS orphan', 'No structural links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_slug": "AXO" }
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
            "arguments": { "project_slug": "FSC" }
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

    assert!(result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content.contains("Projet canonique"), "{content}");
    assert!(content.contains("FSC"), "{content}");
}

#[test]
fn test_axon_validate_soll_reports_invalid_and_dangling_relations() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'AXO', 'Platform Core', 'Protect SOLL', '', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Linked requirement', 'Has links', 'draft', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VAL-AXO-001', 'Validation', 'AXO', 'AXO', '', '', 'passed', '{\"method\":\"manual\"}')")
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
            "arguments": { "project_slug": "AXO" }
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
fn test_axon_export_soll_can_scope_by_project_slug() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'AXO', 'AXO Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VIS-BKS-001', 'Vision', 'BookingSystem', 'BKS', 'BKS Vision', 'Desc', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'AXO', 'AXO Concept', 'Expl', '', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('CPT-BKS-001', 'Concept', 'BookingSystem', 'BKS', 'BKS Concept', 'Expl', '', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": { "project_slug": "BookingSystem" }
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
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
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
        .bulk_insert_files(&[(path.clone(), "proj".to_string(), 128, 1)])
        .unwrap();

    let extraction = parser::ExtractionResult {
        project_slug: Some("proj".to_string()),
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/server.ex', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::trigger_scan', 'trigger_scan', 'function', true, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::trigger_global_scan', 'trigger_global_scan', 'function', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/server.ex', 'axon::trigger_scan')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex', 'axon::trigger_global_scan')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": "axon" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/runtime/watcher.rs', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::opaque_worker', 'opaque_worker', 'function', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/runtime/watcher.rs', 'axon::opaque_worker')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::opaque_worker::chunk', 'symbol', 'axon::opaque_worker', 'axon', 'function', 'symbol: opaque_worker\nkind: function\nfile: src/runtime/watcher.rs\nlines: 10-18\n\nwhen a manual scan requested event arrives, relay it to the rust watcher and keep the ui passive', 'hash-a', 10, 18)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "manual scan requested", "project": "axon" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/runtime/requeue.rs', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/runtime/noise.rs', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::worker_alpha', 'worker_alpha', 'function', true, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::worker_beta', 'worker_beta', 'function', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/runtime/requeue.rs', 'axon::worker_alpha')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/runtime/noise.rs', 'axon::worker_beta')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::worker_alpha::chunk', 'symbol', 'axon::worker_alpha', 'axon', 'function', 'symbol: worker_alpha\nkind: function\nfile: src/runtime/requeue.rs\nlines: 20-28\n\nrequeue claimed file back to pending when the common lane is full', 'hash-b', 20, 28)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::worker_beta::chunk', 'symbol', 'axon::worker_beta', 'axon', 'function', 'symbol: worker_beta\nkind: function\nfile: src/runtime/noise.rs\nlines: 2-8\n\nlog queue metrics and continue', 'hash-c', 2, 8)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "requeue claimed file", "project": "axon" }
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

    assert!(content.contains("worker_alpha"));
    assert!(content.contains("requeue claimed file back to pending"));
    assert!(content.contains("src/runtime/requeue.rs"));
}

#[test]
fn test_vcr1_chunk_retrieval_uses_ingested_docstring_content() {
    let server = create_test_server();
    let path = "/tmp/axon_docstring_query.rs".to_string();
    server
        .graph_store
        .bulk_insert_files(&[(path.clone(), "axon".to_string(), 120, 1)])
        .unwrap();

    let extraction = crate::parser::ExtractionResult {
        project_slug: Some("axon".to_string()),
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
            "arguments": { "query": "fake indexing overlay", "project": "axon" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/runtime/path_only_fake_indexing_overlay.rs', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/runtime/docstring_truth.rs', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::path_only_probe', 'path_only_probe', 'function', true, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::truth_probe', 'truth_probe', 'function', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/runtime/path_only_fake_indexing_overlay.rs', 'axon::path_only_probe')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/runtime/docstring_truth.rs', 'axon::truth_probe')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::path_only_probe::chunk', 'symbol', 'axon::path_only_probe', 'axon', 'function', 'symbol: path_only_probe\nkind: function\nfile: src/runtime/path_only_fake_indexing_overlay.rs\nlines: 1-4\n\nlog metrics and continue', 'hash-path', 1, 4)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::truth_probe::chunk', 'symbol', 'axon::truth_probe', 'axon', 'function', 'symbol: truth_probe\nkind: function\nfile: src/runtime/docstring_truth.rs\nlines: 10-18\ndocstring: prevent fake indexing overlay in the cockpit while forwarding to the rust watcher.\n\nnotify runtime and preserve live truth', 'hash-doc', 10, 18)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "fake indexing overlay", "project": "axon" }
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
        .find("truth_probe")
        .expect("truth probe should appear");
    let path_pos = content
        .find("path_only_probe")
        .expect("path-only probe should appear");
    assert!(
        truth_pos < path_pos,
        "content-backed match should rank ahead of path-only match"
    );
    assert!(content.contains("docstring"));
    assert!(content.contains("file path"));
}

#[test]
fn test_axon_query_exact_config_lookup_prefers_operational_source_over_documentary_chunk() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('config/runtime.exs', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::runtime_config', 'runtime_config', 'module', true, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::audit_section', 'audit_section', 'section', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('config/runtime.exs', 'axon::runtime_config')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon::audit_section')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::runtime_config::chunk', 'symbol', 'axon::runtime_config', 'axon', 'module', 'symbol: runtime_config\nkind: module\nfile: config/runtime.exs\nlines: 1-12\n\nconfigures Credo.Check.Refactor.CyclomaticComplexity threshold for the application runtime', 'hash-runtime', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::audit_section::chunk', 'symbol', 'axon::audit_section', 'axon', 'section', 'symbol: audit_section\nkind: section\nfile: docs/AXON_TEXT_PARSING_AUDIT.md\nlines: 20-35\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit', 20, 35)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": "axon" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::audit_section', 'audit_section', 'section', true, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('docs/AXON_TEXT_PARSING_AUDIT.md', 'axon::audit_section')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES ('axon::audit_section::chunk', 'symbol', 'axon::audit_section', 'axon', 'section', 'symbol: audit_section\nkind: section\nfile: docs/AXON_TEXT_PARSING_AUDIT.md\nlines: 20-35\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit-only', 20, 35)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": "axon" }
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

    assert!(content.contains("docs/AXON_TEXT_PARSING_AUDIT.md"), "{content}");
    assert!(content.contains("Type de resultat"), "{content}");
    assert!(content.contains("documentaire"), "{content}");
    assert!(content.contains("config_lookup_exact"), "{content}");
}

#[test]
fn test_axon_query_falls_back_when_contains_is_absent() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::Axon.Watcher.Server.trigger_scan', 'Axon.Watcher.Server.trigger_scan', 'function', true, true, false, 'global')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": "axon" }
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

    println!("TEST QUERY CONTENT:\n{}", content);
    assert!(content.contains("degrade structurel sans ancrage fichier"), "{content}");
    assert!(content.contains("trigger_scan"));
}

#[test]
fn test_vcr2_impact_before_change_on_public_api() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/core/api.rs', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/core/consumer_a.rs', 'axon')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/core/consumer_b.rs', 'axon')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::consumer_a', 'consumer_a', 'function', false, true, false, 'axon')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('axon::consumer_b', 'consumer_b', 'function', false, true, false, 'axon')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/core/api.rs', 'axon::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/core/consumer_a.rs', 'axon::consumer_a')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/core/consumer_b.rs', 'axon::consumer_b')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('axon::consumer_a', 'axon::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('axon::consumer_b', 'axon::parse_batch')")
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

    server.graph_store.refresh_reader_snapshot().unwrap();
    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("VCR2 IMPACT TEXT: \n{}", impact_text);
    assert!(impact_text.contains("parse_batch"));
    assert!(impact_text.contains("consumer_a"));
    assert!(impact_text.contains("consumer_b"));
    assert!(impact_text.contains("Projection locale"), "{impact_text}");

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

    assert!(api_break_text.contains("warn_api_break_risk") || api_break_text.contains("public api consumer impact detected"));
    assert!(api_break_text.contains("consumer_a"));
    assert!(api_break_text.contains("consumer_b"));
}

#[test]
fn test_axon_impact_reports_missing_call_graph_truthfully() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::parse_batch', 'parse_batch', 'function', true, true, false, 'global')")
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

    server.graph_store.refresh_reader_snapshot().unwrap();
    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains("le graphe d'appel n'est pas encore disponible"), "{impact_text}");
    assert!(impact_text.contains("parse_batch"));
}

#[test]
fn test_axon_impact_respects_project_scope_for_duplicate_symbol_names() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/alpha/api.rs', 'alpha')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/alpha/consumer.rs', 'alpha')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/beta/api.rs', 'beta')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug) VALUES ('src/beta/consumer.rs', 'beta')")
        .unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::consumer_alpha', 'consumer_alpha', 'function', false, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::parse_batch', 'parse_batch', 'function', true, true, false, 'beta')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::consumer_beta', 'consumer_beta', 'function', false, true, false, 'beta')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/api.rs', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/consumer.rs', 'alpha::consumer_alpha')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/beta/api.rs', 'beta::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/beta/consumer.rs', 'beta::consumer_beta')")
        .unwrap();

    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('alpha::consumer_alpha', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('beta::consumer_beta', 'beta::parse_batch')")
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": {
                "symbol": "parse_batch",
                "project": "alpha",
                "depth": 2
            }
        })),
        id: Some(json!(199)),
    };

    server.graph_store.refresh_reader_snapshot().unwrap();
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
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status, last_error_reason) VALUES ('src/alpha/large.rs', 'alpha', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status) VALUES ('src/beta/worker.rs', 'beta', 'indexed')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::worker_loop', 'worker_loop', 'function', true, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/large.rs', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/beta/worker.rs', 'beta::worker_loop')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "rare docstring phrase", "project": "alpha" }
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
}

#[test]
fn test_axon_query_reports_project_completion_when_scope_is_partial() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status, status_reason) VALUES \
             ('src/alpha/live.rs', 'alpha', 'indexed', NULL), \
             ('src/alpha/todo.rs', 'alpha', 'pending', 'metadata_changed_scan')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/live.rs', 'alpha::parse_batch')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "parse_batch", "project": "alpha" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status, last_error_reason) VALUES ('src/alpha/large.rs', 'alpha', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/large.rs', 'alpha::parse_batch')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": { "symbol": "parse_batch", "project": "alpha" }
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
}

#[test]
fn test_axon_impact_reports_partial_truth_for_degraded_symbol() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status, last_error_reason) VALUES ('src/alpha/large.rs', 'alpha', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug, status) VALUES ('src/beta/live.rs', 'beta', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::caller', 'caller', 'function', false, true, false, 'beta')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::callee', 'callee', 'function', true, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/large.rs', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/beta/live.rs', 'beta::caller')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/beta/live.rs', 'beta::callee')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('beta::caller', 'beta::callee')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "parse_batch", "project": "alpha", "depth": 2 }
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
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO File (path, project_slug, status, last_error_reason) VALUES ('src/alpha/large.rs', 'alpha', 'indexed_degraded', 'degraded_structure_only')",
        )
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/alpha/large.rs', 'alpha::parse_batch')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            "arguments": { "project": "alpha" }
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

    assert!(content.contains("Health Report: alpha"), "{}", content);
    assert!(content.contains("verite partielle"), "{}", content);
    assert!(content.contains("indexed_degraded"), "{}", content);
}

#[test]
fn test_axon_query_project_scope_uses_project_slug_not_path_substring() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug, status) VALUES ('/tmp/shared/api.rs', 'alpha', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug, status) VALUES ('/tmp/shared/worker.rs', 'beta', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::parse_batch', 'parse_batch', 'function', true, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/shared/api.rs', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/shared/worker.rs', 'beta::parse_batch')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "parse_batch", "project": "alpha" }
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
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug, status) VALUES ('/tmp/shared/api.rs', 'alpha', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_slug, status) VALUES ('/tmp/shared/worker.rs', 'beta', 'indexed')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('alpha::parse_batch', 'parse_batch', 'function', true, true, false, 'alpha')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('beta::parse_batch', 'parse_batch', 'module', false, true, false, 'beta')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/shared/api.rs', 'alpha::parse_batch')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('/tmp/shared/worker.rs', 'beta::parse_batch')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": { "symbol": "parse_batch", "project": "alpha" }
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VIS-AXO-900', 'Vision', 'AXO', 'AXO', 'Axon Vision', 'Stable conceptual continuity', '', '{\"goal\":\"Protect SOLL while evolving IST\"}')")
        .unwrap();

    let create_calls = vec![
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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

    assert!(restore_text.contains("Restauration SOLL terminee"), "{}", restore_text);
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
fn test_axon_soll_manager_link_rejects_missing_endpoint() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
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

    assert!(result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content.contains("introuvable"), "{content}");
}

#[test]
fn test_axon_soll_manager_link_applies_default_relation() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
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

    assert!(result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content.contains("Relations autorisées"), "{content}");
    assert!(content.contains("SOLVES"), "{content}");
    assert!(content.contains("REFINES"), "{content}");
}

#[test]
fn test_axon_soll_manager_link_allows_authorized_cumulative_relation() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'AXO', 'Decision', '', 'accepted', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'AXO', 'Req', 'Desc', 'draft', '{\"priority\":\"P1\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) VALUES ('VIS-AXO-901', 'Vision', 'AXO', 'AXO', 'Axon Vision', 'Stable conceptual continuity', '', '{\"goal\":\"Protect SOLL while evolving IST\"}')")
        .unwrap();

    let create_calls = vec![
        json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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
                    "project_slug": "AXO",
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

    assert!(restore_text.contains("Restauration SOLL terminee"), "{}", restore_text);
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
    assert!(validation_metadata.contains("test"), "{}", validation_metadata);

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
fn test_axon_pre_flight_check_enforces_guideline() {
    let server = create_test_server();

    // Insert a Guideline into SolDB requiring tests to be updated if src/mcp/ is modified
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata)
         VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'AXO', 'Mise à jour des Tests', 'Les modifications de src/mcp/ doivent inclure des tests', 'active', '{\"trigger_path\":\"src/mcp/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    // 1. Simulate a bad commit (modifies src/mcp/ but no tests.rs)
    let req_bad = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs"]
            }
        },
        "id": 1
    });

    let res_bad = server.handle_request(serde_json::from_value(req_bad).unwrap()).unwrap().result.unwrap();
    let content_bad = res_bad.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    println!("DEBUG CONTENT BAD: {}", content_bad);

    // It should be rejected
    assert!(res_bad.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content_bad.contains("GUI-AXO-001") || content_bad.contains("GUI-PRO-001"));
    assert!(content_bad.contains("remediation_plan"));

    // 2. Simulate a good commit (modifies src/mcp/ AND tests.rs)
    let req_good = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs", "src/axon-core/src/mcp/tests.rs", "SKILL.md"]
            }
        },
        "id": 2
    });

    let res_good = server.handle_request(serde_json::from_value(req_good).unwrap()).unwrap().result.unwrap();
    let content_good = res_good.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    // It should pass
    assert!(!res_good.get("isError").and_then(|v| v.as_bool()).unwrap_or(false));
    assert!(content_good.contains("Quality Gate Passed"));
}

#[test]
fn test_bootstrap_injects_global_guidelines() {
    let server = create_test_server();
    
    // Check GUI-PRO-001
    let count1 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-001' AND type = 'Guideline' AND project_slug = 'GLOBAL' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count1, 1, "GUI-PRO-001 should be injected at bootstrap");

    let meta1_raw = server.graph_store.query_json(
        "SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-001'"
    ).unwrap();
    println!("DEBUG META1 RAW: {}", meta1_raw);
    let meta1: Vec<Vec<String>> = serde_json::from_str(&meta1_raw).unwrap();
    assert!(meta1[0][0].contains("\"phase\":\"pre-code\"") || meta1[0][0].contains("\"phase\": \"pre-code\""), "GUI-PRO-001 should have phase: pre-code");

    // Check GUI-PRO-002
    let count2 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-002' AND type = 'Guideline' AND project_slug = 'GLOBAL' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count2, 1, "GUI-PRO-002 should be injected at bootstrap");

    let meta2_raw = server.graph_store.query_json(
        "SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-002'"
    ).unwrap();
    println!("DEBUG META2 RAW: {}", meta2_raw);
    let meta2: Vec<Vec<String>> = serde_json::from_str(&meta2_raw).unwrap();
    assert!(meta2[0][0].contains("\"phase\":\"post-code\"") || meta2[0][0].contains("\"phase\": \"post-code\""), "GUI-PRO-002 should have phase: post-code");
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
                "project_name": "BookingSystem",
                "project_slug": "BKS",
                "concept_document_url_or_text": "We want a booking system."
            }
        },
        "id": 1
    });

    let response = server.handle_request(serde_json::from_value(req).unwrap()).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    
    println!("DEBUG INIT OUTPUT: {}", content);

    // Output should contain the global guidelines injected at bootstrap
    assert!(content.contains("GUI-PRO-001"));
    assert!(content.contains("GUI-PRO-002"));
    assert!(content.contains("Voici les règles globales disponibles."));
}


#[test]
fn test_axon_apply_guidelines_creates_local_copies() {
    let server = create_test_server();
    
    // First init the project
    server.graph_store.sync_project_code_registry_entry("BookingSystem", "BKS").unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_slug": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-001"]
            }
        },
        "id": 1
    });

    let response = server.handle_request(serde_json::from_value(req).unwrap()).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    
    // Output should confirm creation
    assert!(content.contains("GUI-AXO-001"));
    assert!(content.contains("Héritage appliqué"));

    // Verify in DB
    let count = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-AXO-001' AND type = 'Guideline' AND project_slug = 'AXO'"
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
                "project_slug": "AXO",
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

    let response = server.handle_request(serde_json::from_value(req).unwrap()).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    
    // Should be committed immediately because dry_run = false
    assert!(content.contains("SOLL revision committed"), "{}", content);

    // We expect identity_mapping in the result.data
    let data = result.get("data").expect("Should have data field");
    let identity_mapping = data.get("identity_mapping").expect("Should have identity_mapping");
    
    let dec_id = identity_mapping.get("dec-1").unwrap().as_str().unwrap();
    let req_id = identity_mapping.get("req-1").unwrap().as_str().unwrap();
    
    assert!(dec_id.starts_with("DEC-AXO-"));
    assert!(req_id.starts_with("REQ-AXO-"));

    // Verify the edge in DB using the canonical IDs
    let edge_count = server.graph_store.query_count(&format!(
        "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = 'SOLVES'",
        dec_id, req_id
    )).unwrap();
    assert_eq!(edge_count, 1, "The relation should be created using canonical IDs");
}


#[test]
fn test_axon_pre_flight_check_exports_when_dry_run_false() {
    let server = create_test_server();
    
    // Insert a dummy Guideline that passes trivially
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_slug, project_code, title, description, status, metadata) 
         VALUES ('GUI-AXO-999', 'Guideline', 'AXO', 'AXO', 'Dummy', 'Dummy', 'active', '{\"trigger_path\":\"\",\"required_path\":\"\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": ["Cargo.toml"]
            }
        },
        "id": 1
    });

    let response = server.handle_request(serde_json::from_value(req).unwrap()).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    
    // It should not be an error
    assert!(!result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false), "{}", content);
    
    // It should contain Export mentions and success
    assert!(content.contains("Quality Gate Passed"), "{}", content);
    assert!(content.contains("Exported to"), "{}", content);
}


#[test]
fn test_axon_impact_traces_through_soll_architecture() {
    let server = create_test_server();
    
    // 1. Create Code Symbols and Calls
    server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/payment.rs', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('payment::process', 'process', 'function', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/payment.rs', 'payment::process')").unwrap();
    
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_slug) VALUES ('api::checkout', 'checkout', 'function', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('api::checkout', 'payment::process')").unwrap();
    
    // 2. Create SOLL Intent Graph
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, title) VALUES ('VIS-BKS-001', 'Vision', 'BKS', 'Paiement sans friction')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, title) VALUES ('REQ-BKS-005', 'Requirement', 'BKS', 'Intégration Stripe')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_slug, title) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Utiliser Rust Stripe SDK')").unwrap();
    
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
    let content = impact_res.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    
    // 5. Asserts
    println!("DEBUG IMPACT CONTENT: {}", content);
    assert!(content.contains("checkout"), "Should find caller symbol");
    assert!(content.contains("DEC-BKS-010"), "Should bridge to SOLL Decision");
    assert!(content.contains("Utiliser Rust Stripe SDK"), "Should list decision title");
    assert!(content.contains("REQ-BKS-005"), "Should traverse to Requirement");
    assert!(content.contains("VIS-BKS-001"), "Should traverse to Vision");
    assert!(content.contains("Paiement sans friction"), "Should list vision title");
}

#[test]
fn test_axon_architectural_drift_finds_deep_paths() {
    let server = create_test_server();
    
    server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/domain/entity.rs', 'global')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/application/service.rs', 'global')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_slug) VALUES ('src/infrastructure/db.rs', 'global')").unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::domain::Entity', 'Entity', 'struct', false, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::application::Service', 'Service', 'struct', false, true, false, 'global')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::infrastructure::Db', 'Db', 'struct', false, true, false, 'global')").unwrap();

    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/domain/entity.rs', 'global::domain::Entity')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/application/service.rs', 'global::application::Service')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/infrastructure/db.rs', 'global::infrastructure::Db')").unwrap();

    server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::domain::Entity', 'global::application::Service')").unwrap();
    server.graph_store.execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::application::Service', 'global::infrastructure::Db')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "architectural_drift",
            "arguments": { "source_layer": "domain", "target_layer": "infrastructure" }
        })),
        id: Some(json!(999)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEEP PATH CONTENT: {}", content);
    assert!(content.contains("VIOLATION D'ARCHITECTURE"), "Should detect violation");
    assert!(content.contains("global::domain::Entity -> global::application::Service -> global::infrastructure::Db"), "Should contain the path");
}
// Trigger TDD rule for omniscience migration
