use super::*;
use crate::graph::GraphStore;
use std::sync::Arc;

fn create_test_server() -> McpServer {
    let store =
        Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db").unwrap()));
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

    assert_eq!(tools.len(), 19);

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"axon_refine_lattice"));
    assert!(tool_names.contains(&"axon_fs_read"));
    assert!(tool_names.contains(&"axon_query"));
    assert!(tool_names.contains(&"axon_restore_soll"));
    assert!(tool_names.contains(&"axon_inspect"));
    assert!(tool_names.contains(&"axon_audit"));
    assert!(tool_names.contains(&"axon_impact"));
    assert!(tool_names.contains(&"axon_health"));
    assert!(tool_names.contains(&"axon_diff"));
    assert!(tool_names.contains(&"axon_batch"));
    assert!(tool_names.contains(&"axon_cypher"));
    assert!(tool_names.contains(&"axon_semantic_clones"));
    assert!(tool_names.contains(&"axon_architectural_drift"));
    assert!(tool_names.contains(&"axon_bidi_trace"));
    assert!(tool_names.contains(&"axon_api_break_check"));
    assert!(tool_names.contains(&"axon_simulate_mutation"));
    assert!(tool_names.contains(&"axon_debug"));
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
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('ui/app.js', 'global::fetchData')")
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
            "name": "axon_architectural_drift",
            "arguments": { "source_layer": "ui", "target_layer": "db" }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(
        content.contains("VIOLATION") || content.contains("Détectée") || content.contains("détectée")
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
            "name": "axon_query",
            "arguments": { "query": "auth", "project": "test_proj" }
        })),
        id: Some(json!(3)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("auth_func"));
}

#[test]
fn test_axon_fs_read() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_fs_read",
            "arguments": { "uri": "src/axon-core/src/main.rs", "start_line": 1, "end_line": 5 }
        })),
        id: Some(json!(4)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    assert!(content.contains("L2 Detail") || content.contains("Erreur"));
}

#[test]
fn test_send_notification() {
    let store =
        Arc::new(GraphStore::new(":memory:").unwrap_or_else(|_| GraphStore::new("/tmp/test_db_notif").unwrap()));
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
            "name": "axon_inspect",
            "arguments": {
                "symbol": "core_func",
                "project": "test_proj"
            }
        })),
        id: Some(json!(5)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    assert!(content.contains("Inspection du Symbole"));
    assert!(content.contains("core_func"));
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
        .execute("INSERT INTO CALLS (source_id, target_id) VALUES ('global::run_task', 'global::eval')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(6)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
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
            "name": "axon_audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(10)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

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
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/todo.rs', 'global::todo1')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(11)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

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
            "name": "axon_audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(12)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

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
            "name": "axon_audit",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(13)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

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
        .execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/god.rs', 'global::GodClass')")
        .unwrap();

    for i in 0..10 {
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
            "name": "axon_health",
            "arguments": {
                "project": "*"
            }
        })),
        id: Some(json!(7)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("God Object detected") || content.contains("GodClass"));
}

#[test]
fn test_axon_query_global_default() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_query",
            "arguments": { "query": "auth" }
        })),
        id: Some(json!(8)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    assert!(content.contains("Resultats de recherche"));
    assert!(content.contains("Mode:"));
}

#[test]
fn test_axon_soll_manager_auto_id() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Registry (project_slug, id, last_req, last_cpt, last_dec) VALUES ('AXO', 'AXON_GLOBAL', 0, 10, 0)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_soll_manager",
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
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("CPT-AXO-011"));

    let count = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.Concept WHERE name LIKE 'CPT-AXO-011%'")
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_axon_export_soll() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Vision (id, title, description, goal, metadata) VALUES ('VIS-AXO-001', 'Test Vision', 'Desc', 'Goal', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Concept (name, explanation, rationale, metadata) VALUES ('CPT-AXO-001: My Concept', 'Expl', 'Rat', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_export_soll",
            "arguments": {}
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("# SOLL Extraction"));
    assert!(content.contains("Test Vision"));
    assert!(content.contains("CPT-AXO-001"));
    assert!(content.contains("Exported to"));
}

#[test]
fn test_axon_restore_soll() {
    let server = create_test_server();
    let export_path = "/tmp/axon_restore_soll_test.md";
    let markdown = r#"# SOLL Extraction

*Généré le : 2026-03-30 02:00:00*

## 1. Vision & Objectifs Stratégiques
### Test Vision
**Description:** Desc
**Goal:** Goal
**Meta:** `{"source":"test"}`

## 2. Piliers d'Architecture
* **PIL-AXO-001** : Platform Core (Keep the conceptual core stable)

## 2b. Concepts
* **CPT-AXO-001: Graph Truth** : Use a structural graph as source of truth (Because the project needs stable intent)

## 3. Jalons & Roadmap (Milestones)
### MLS-AXO-001 : First Usable State
*Statut :* `in_progress`

## 4. Exigences & Rayon d'Impact (Requirements)
### REQ-AXO-001 - Reliable Restore
*Priorité :* `high`
*Description :* SOLL must be restorable from exports

## 5. Registre des Décisions (ADR)
### DEC-AXO-001
**Titre :** Merge Restore
**Statut :** `accepted`
**Rationnel :** Restoration should be merge-oriented and non-destructive

## 6. Preuves de Validation & Witness
* `VAL-AXO-001`: **passed** via `manual-test` (timestamp: 1234567890)
"#;
    std::fs::write(export_path, markdown).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(3)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("Restauration SOLL terminee"));
    assert!(content.contains("Vision: 1"));
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Vision").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Pillar").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Concept").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Milestone").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Requirement").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Decision").unwrap(), 1);
    assert_eq!(server.graph_store.query_count("SELECT count(*) FROM soll.Validation").unwrap(), 1);

    let _ = std::fs::remove_file(export_path);
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
            "name": "axon_query",
            "arguments": { "query": "trigger scan", "project": "axon" }
        })),
        id: Some(json!(21)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("trigger_scan"));
    assert!(content.contains("trigger_global_scan"));
    assert!(content.contains("server.ex") || content.contains("pool_facade.ex"));
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
            "name": "axon_query",
            "arguments": { "query": "trigger scan", "project": "axon" }
        })),
        id: Some(json!(211)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(content.contains("degrade structurel sans ancrage fichier"));
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
            "name": "axon_impact",
            "arguments": { "symbol": "parse_batch", "depth": 2 }
        })),
        id: Some(json!(22)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(impact_text.contains("parse_batch"));
    assert!(impact_text.contains("consumer_a"));
    assert!(impact_text.contains("consumer_b"));

    let api_break_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_api_break_check",
            "arguments": { "symbol": "parse_batch" }
        })),
        id: Some(json!(23)),
    };

    let api_break_response = server.handle_request(api_break_req);
    let api_break_result = api_break_response.unwrap().result.expect("Expected result");
    let api_break_text = api_break_result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(api_break_text.contains("RISQUE DE RUPTURE D'API"));
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
            "name": "axon_impact",
            "arguments": { "symbol": "parse_batch", "depth": 2 }
        })),
        id: Some(json!(221)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(impact_text.contains("le graphe d'appel n'est pas encore disponible"));
    assert!(impact_text.contains("parse_batch"));
}

#[test]
fn test_vcr4_soll_continuity_create_export_restore_verify() {
    let source_server = create_test_server();
    source_server
        .graph_store
        .execute("INSERT INTO soll.Vision (id, title, description, goal, metadata) VALUES ('VIS-AXO-900', 'Axon Vision', 'Stable conceptual continuity', 'Protect SOLL while evolving IST', '{\"scenario\":\"vcr4\"}')")
        .unwrap();

    let create_calls = vec![
        json!({
            "name": "axon_soll_manager",
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
            "name": "axon_soll_manager",
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
            "name": "axon_soll_manager",
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
            "name": "axon_soll_manager",
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
            "name": "axon_soll_manager",
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
            "name": "axon_soll_manager",
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
        let result = response.unwrap().result.expect("Expected SOLL creation result");
        let content = result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
        assert!(content.contains("Entité SOLL créée"));
    }

    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_export_soll",
            "arguments": {}
        })),
        id: Some(json!(200)),
    };

    let export_response = source_server.handle_request(export_req);
    let export_result = export_response.unwrap().result.expect("Expected SOLL export result");
    let export_text = export_result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();
    assert!(export_text.contains("Exported to docs/vision/SOLL_EXPORT_"));

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
            "name": "axon_restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(201)),
    };

    let restore_response = restore_server.handle_request(restore_req);
    let restore_result = restore_response.unwrap().result.expect("Expected SOLL restore result");
    let restore_text = restore_result.get("content").unwrap()[0].get("text").unwrap().as_str().unwrap();

    assert!(restore_text.contains("Restauration SOLL terminee"));
    assert!(restore_text.contains("Vision: 1"));
    assert!(restore_text.contains("Pillars: 1"));
    assert!(restore_text.contains("Concepts: 1"));
    assert!(restore_text.contains("Milestones: 1"));
    assert!(restore_text.contains("Requirements: 1"));
    assert!(restore_text.contains("Decisions: 1"));
    assert!(restore_text.contains("Validations: 1"));

    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Vision").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Pillar").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Concept").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Milestone").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Requirement").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Decision").unwrap(), 1);
    assert_eq!(restore_server.graph_store.query_count("SELECT count(*) FROM soll.Validation").unwrap(), 1);

    let _ = std::fs::remove_file(&export_path);
}
