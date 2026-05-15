use super::*;

#[test]
fn test_retrieve_context_intent_mode_prefers_plan_docs_over_feedback_docs() {
    let plan_weight = McpServer::project_intent_doc_weight(
        "docs/plans/2026-04-19-nutri-opti-concept-foundation.md",
    );
    let feedback_weight = McpServer::project_intent_doc_weight("feedback-axon-soll-2026-04-19.md");
    assert!(plan_weight > feedback_weight);
    assert!(plan_weight > 0.0);
    assert!(
        feedback_weight < 0.0,
        "expected feedback docs to be penalized, got {feedback_weight}"
    );
}

#[test]
fn test_why_wraps_retrieve_context_and_reports_framework_alias() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/payment.rs', 'BKS', 'indexed')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout-why', 'symbol', 'bks::checkout', 'BKS', 'body', 'checkout orchestrates payment capture and settlement', 'hash-why-checkout', 1, 4)").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Operational payment choice', 'current', '{\"rationale\":\"Operational safety\"}')").unwrap();
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
    assert_eq!(
        response["data"]["why"]["rationale_quality"]["automation_contract"].as_str(),
        Some("informational_only")
    );
    assert!(response["data"]["why"]["evidence_states"].is_array());
    assert!(response["data"]["why"]["governing_requirements"].is_array());
    assert!(response["data"]["why"]["governing_decisions"].is_array());
    assert!(response["data"]["why"]["supporting_guidelines"].is_array());
    assert!(response["data"]["why"]["supporting_docs"].is_array());
    assert!(response["data"]["why"]["direct_code_evidence"].is_array());
    assert!(response["data"]["why"]["supporting_code_context"].is_array());
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
    assert!(response["data"]["why"]["governing_decisions"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["id"].as_str() == Some("DEC-BKS-010")
                && row["authority_class"].as_str() == Some("governing")
                && row["evidence_provenance"].as_str() == Some("soll_decision")
                && row["link_mode"].as_str() == Some("direct")
        })));
    assert!(response["data"]["why"]["direct_code_evidence"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["authority_class"].as_str() == Some("supporting")
                && row["evidence_provenance"].as_str() == Some("code_symbol")
                && row["link_mode"].as_str() == Some("direct")
        })));
    assert_eq!(
        response["data"]["why"]["rationale_quality"]["level"].as_str(),
        Some("strong")
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Context Retrieval"), "{text}");
    assert!(!text.contains("missing governing intent"), "{text}");
    assert!(text.contains("Governing decisions"), "{text}");
}

#[test]
fn test_why_surfaces_missing_governing_intent_without_laundering_inference_into_fact() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::refund', 'refund', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/refund.rs', 'BKS', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/refund.rs', 'bks::refund', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-refund-why', 'symbol', 'bks::refund', 'BKS', 'body', 'refund reverses a payment capture and updates settlement state', 'hash-why-refund', 1, 4)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "why",
                "arguments": { "symbol": "refund", "project": "BKS", "mode": "brief" }
            })),
            id: Some(json!(2204)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(response["data"]["why"]["evidence_states"]
        .as_array()
        .is_some_and(|rows| rows
            .iter()
            .any(|row| { row["state"].as_str() == Some("missing_governing_intent") })));
    assert!(response["data"]["why"]["evidence_states"]
        .as_array()
        .is_some_and(|rows| rows
            .iter()
            .any(|row| { row["state"].as_str() == Some("no_direct_traceability") })));
    assert_eq!(
        response["data"]["why"]["rationale_quality"]["level"].as_str(),
        Some("weak")
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("No direct governing intent"), "{text}");
    assert!(!text.contains("governed by"), "{text}");
}

#[test]
fn test_why_uses_concept_links_to_recover_governing_requirement() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::dashboard_surface', 'dashboard', 'module', true, true, false, 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('src/dashboard/lib/axon_dashboard/application.ex', 'AXO', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/dashboard/lib/axon_dashboard/application.ex', 'axon::dashboard_surface', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-dashboard-why', 'symbol', 'axon::dashboard_surface', 'AXO', 'body', 'dashboard renders runtime observation and operator telemetry', 'hash-why-dashboard', 1, 4)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth is queryable through status', 'Runtime truth stays on the Rust runtime authority', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-014', 'Concept', 'AXO', 'Dashboard Observation Surface', 'Dashboard is an observation surface rather than canonical runtime authority', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('CPT-AXO-014', 'REQ-AXO-001', 'EXPLAINS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-DASHBOARD', 'Concept', 'CPT-AXO-014', 'Symbol', 'dashboard', 1.0, 0)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "why",
                "arguments": { "symbol": "dashboard", "project": "AXO", "mode": "brief" }
            })),
            id: Some(json!(22041)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(response["data"]["why"]["governing_requirements"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["id"].as_str() == Some("REQ-AXO-001")
                && row["authority_class"].as_str() == Some("governing")
                && row["evidence_provenance"].as_str() == Some("soll_requirement")
                && row["link_mode"].as_str() == Some("inferred")
        })));
    assert!(response["data"]["why"]["evidence_states"]
        .as_array()
        .is_some_and(|rows| rows
            .iter()
            .all(|row| row["state"].as_str() != Some("missing_governing_intent"))));
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Governing requirement"), "{text}");
    assert!(!text.contains("No direct governing intent"), "{text}");
}

#[test]
fn test_why_marks_script_artifacts_as_correlated_weak_support() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::refund_probe', 'refund_probe', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, status) VALUES ('scripts/refund_probe.rs', 'BKS', 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('scripts/refund_probe.rs', 'bks::refund_probe', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-refund-probe', 'symbol', 'bks::refund_probe', 'BKS', 'body', 'refund_probe scans refund timing and prints local diagnostics', 'hash-why-refund-probe', 1, 4)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "why",
                "arguments": { "symbol": "refund_probe", "project": "BKS", "mode": "brief" }
            })),
            id: Some(json!(2205)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(response["data"]["why"]["direct_code_evidence"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| {
            row["evidence_provenance"].as_str() == Some("script")
                && row["authority_class"].as_str() == Some("correlated")
                && row["link_mode"].as_str() == Some("direct")
        })));
    assert!(response["data"]["why"]["evidence_states"]
        .as_array()
        .is_some_and(|rows| rows
            .iter()
            .any(|row| { row["state"].as_str() == Some("correlation_only") })));
    assert_eq!(
        response["data"]["why"]["rationale_quality"]["level"].as_str(),
        Some("weak")
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("correlation_only"), "{text}");
    assert!(!text.contains("Direct governing decision"), "{text}");
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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'current', '{\"goal\":\"Vision first\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'planned', '{\"priority\":\"P1\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', 'Use Rust as the authoritative runtime', 'current', '{\"context\":\"\",\"rationale\":\"\"}')").unwrap();

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
    assert_eq!(data["runtime"]["mode"].as_str(), Some("brief_compact"));
    assert!(data["runtime"]["debug_snapshot"].is_null());
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
    assert!(data["operator_guidance"].as_object().is_some());
    assert!(data["operator_guidance"]["recommended_next_step"]
        .as_str()
        .is_some());
    assert!(data["operator_guidance"]["actionable_now"].is_boolean());
    assert!(data["operator_guidance"]["blocking_factors"].is_array());
    assert!(data["operator_guidance"]["remediation_actions"].is_array());
    assert!(data["truth_cockpit"].as_object().is_some());
    assert!(data["truth_cockpit"]["next_best_action"]["tool"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["freshness"]["state"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["proof_gaps"].is_array());
    assert!(data["next_action"]["kind"].as_str().is_some());
    assert!(data["next_action"]["tool"].as_str().is_some());
    assert_eq!(
        data["next_action"],
        data["truth_cockpit"]["next_best_action"]
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Project Status"), "{text}");
    assert!(text.contains("Axon Vision"), "{text}");
}

#[test]
fn test_project_status_reports_delta_vs_previous_snapshot() {
    let _guard = env_lock();
    let history_dir = tempdir().unwrap();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'current', '{}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'current', '{}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-010', 'Requirement', 'AXO', 'Card charging', 'Charge cards safely', 'current', '{}')")
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
    assert!(data["operator_guidance"].as_object().is_some());
    assert_eq!(
        data["operator_guidance"]["mutation_class_recommendation"].as_str(),
        Some("safe_for_direct_mutation")
    );
    assert_eq!(
        data["operator_guidance"]["actionable_now"].as_bool(),
        Some(true)
    );
    assert!(data["operator_guidance"]["blocking_factors"]
        .as_array()
        .is_some());
}

// REQ-AXO-043 — conception_view and change_safety must adopt the
// shared wrong_project_scope_response helper so an unregistered
// project_code surfaces a structured recovery contract instead of a
// "Status: ok" view with zero modules / Safety=unsafe with low
// confidence (which the LLM caller would misread as a real signal
// rather than an invalid input).

#[test]
fn test_conception_view_rejects_unregistered_project_code() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "conception_view",
                "arguments": { "project_code": "ZZZ" }
            })),
            id: Some(json!(23010)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "unregistered project_code must surface isError; response={response:?}"
    );
    let data = response.get("data").expect("data");
    assert_eq!(
        data["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("ZZZ")
    );
    assert!(
        data["registered_project_codes"].is_array(),
        "registered_project_codes must be an array"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
}

#[test]
fn test_change_safety_rejects_unregistered_project_code() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "change_safety",
                "arguments": {
                    "project_code": "ZZZ",
                    "target": "anything",
                    "target_type": "symbol"
                }
            })),
            id: Some(json!(23011)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "unregistered project_code must surface isError; response={response:?}"
    );
    let data = response.get("data").expect("data");
    assert_eq!(
        data["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("ZZZ")
    );
    assert_eq!(
        data["operator_guidance"]["follow_up_tools"][0].as_str(),
        Some("project_registry_lookup")
    );
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
    assert_eq!(
        data["next_action"]["kind"].as_str(),
        Some("expand_blast_radius_from_path")
    );
    assert_eq!(data["next_action"]["tool"].as_str(), Some("impact"));
    assert!(data["operator_guidance"]["follow_up_tools"].is_array());
    let path = data["path"].as_array().unwrap();
    let rendered = path
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(rendered, vec!["source_fn", "mid_fn", "sink_fn"]);
}

#[test]
fn test_path_missing_anchor_still_exposes_canonical_sources_and_guidance() {
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "path",
                "arguments": {
                    "source": "missing_symbol",
                    "sink": "missing_symbol",
                    "project": "AXO",
                    "depth": 2
                }
            })),
            id: Some(json!(2205)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(response["isError"].as_bool(), Some(true));
    assert!(data["operator_guidance"].as_object().is_some());
    assert!(data["canonical_sources"].as_object().is_some());
    assert_eq!(data["next_action"]["tool"].as_str(), Some("impact"));
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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-099', 'Requirement', 'AXO', 'Unimplemented requirement', 'No traceability yet', 'planned', '{\"priority\":\"P2\"}')").unwrap();

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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'planned', '{\"priority\":\"P1\"}')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', '', 'current', '{\"context\":\"\",\"rationale\":\"\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'Operator cockpit', '', 'planned', '{\"priority\":\"P2\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'B', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'C', '', 'planned', '{\"priority\":\"P1\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', '', 'planned', '{\"priority\":\"P1\"}')")
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
    assert_eq!(
        data["metadata"]["include_validation_details"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["validation_gates"]["requirement_verification"]["compact"].as_bool(),
        Some(true)
    );
    assert!(data["validation_gates"]["requirement_verification"]
        .get("details")
        .is_none());
}

#[test]
fn test_soll_work_plan_respects_limit_and_marks_truncated() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'B', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'C', '', 'planned', '{\"priority\":\"P1\"}')")
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'A', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'D1', '', 'current', '{\"context\":\"\",\"rationale\":\"\"}')")
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
fn test_soll_work_plan_excludes_terminal_state_nodes_from_wave_1() {
    // REQ-AXO-135: terminal-state Decisions (delivered/superseded) and
    // Requirements (completed/superseded/archived) must be excluded from
    // wave 1 AND from descendant counting so 'unblocks N descendants'
    // reflects OPEN descendants only.
    let server = create_test_server();
    // Open work: DEC-AXO-001 (accepted) -> REQ-AXO-001 (current)
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Open work', '', 'current', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Active decision', '', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();
    // Terminal: DEC-AXO-002 (delivered) -> REQ-AXO-002 (completed)
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-002', 'Requirement', 'AXO', 'Closed work', '', 'completed', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'Delivered decision', '', 'delivered', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-002', 'REQ-AXO-002', 'SOLVES')")
        .unwrap();
    // Superseded edge case
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-003', 'Decision', 'AXO', 'Superseded decision', '', 'superseded', '{}')")
        .unwrap();
    // Archived edge case
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-003', 'Requirement', 'AXO', 'Archived work', '', 'archived', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(550)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let waves = data["ordered_waves"].as_array().expect("waves");
    let item_ids: Vec<&str> = waves
        .iter()
        .flat_map(|wave| wave["items"].as_array().unwrap().iter())
        .filter_map(|item| item["id"].as_str())
        .collect();

    // Open items present.
    assert!(item_ids.contains(&"DEC-AXO-001"), "open accepted decision must be in waves: {:?}", item_ids);
    assert!(item_ids.contains(&"REQ-AXO-001"), "open current requirement must be in waves: {:?}", item_ids);
    // Terminal items excluded.
    assert!(!item_ids.contains(&"DEC-AXO-002"), "delivered decision must be excluded: {:?}", item_ids);
    assert!(!item_ids.contains(&"REQ-AXO-002"), "completed requirement must be excluded: {:?}", item_ids);
    assert!(!item_ids.contains(&"DEC-AXO-003"), "superseded decision must be excluded: {:?}", item_ids);
    assert!(!item_ids.contains(&"REQ-AXO-003"), "archived requirement must be excluded: {:?}", item_ids);

    // Descendant counter weighted by OPEN descendants only — DEC-AXO-001
    // should report 'unblocks 1 descendant(s)' (REQ-AXO-001), not 0 or 2.
    let dec1 = waves
        .iter()
        .flat_map(|wave| wave["items"].as_array().unwrap().iter())
        .find(|item| item["id"].as_str() == Some("DEC-AXO-001"))
        .expect("DEC-AXO-001 must be in waves");
    let reasons = dec1["reasons"]
        .as_array()
        .expect("reasons array")
        .iter()
        .filter_map(|r| r.as_str())
        .collect::<Vec<_>>();
    let unblocks_str = reasons
        .iter()
        .find(|r| r.starts_with("unblocks "))
        .expect("unblocks reason must be present");
    assert!(unblocks_str.contains("1 descendant"), "expected unblocks 1 descendant, got: {}", unblocks_str);
}

#[test]
fn test_soll_work_plan_temporal_decay_lowers_old_node_score() {
    // REQ-AXO-144 — temporal score decay. Two structurally identical
    // Decisions (each unblocking one open REQ) ranked by activity:
    // DEC-AXO-001 was updated 1 day ago, DEC-AXO-002 was updated
    // 100 days ago. With decay enabled (default), the 100-day old
    // Decision must score strictly lower. Disabling decay yields
    // identical scores again (back-compat / benchmarking knob).
    let server = create_test_server();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let day_ms: i64 = 24 * 60 * 60 * 1000;
    let recent_ms = now_ms - day_ms;
    let old_ms = now_ms - 100 * day_ms;

    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Recent target', '', 'current', '{{\"priority\":\"P1\"}}'),\
                    ('REQ-AXO-002', 'Requirement', 'AXO', 'Old target', '', 'current', '{{\"priority\":\"P1\"}}'),\
                    ('DEC-AXO-001', 'Decision', 'AXO', 'Recent decision', '', 'current', '{{\"updated_at\":{}}}'),\
                    ('DEC-AXO-002', 'Decision', 'AXO', 'Old decision', '', 'current', '{{\"updated_at\":{}}}')",
            recent_ms, old_ms
        ))
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES \
             ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES'),\
             ('DEC-AXO-002', 'REQ-AXO-002', 'SOLVES')",
        )
        .unwrap();

    // 1) Decay enabled (default).
    let req_decay = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(701)),
    };
    let response = server.handle_request(req_decay);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let waves = data["ordered_waves"].as_array().expect("waves");
    let mut score_by_id: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for wave in waves {
        for item in wave["items"].as_array().expect("items") {
            let id = item["id"].as_str().unwrap_or("").to_string();
            let score = item["score"].as_i64().unwrap_or(0);
            score_by_id.insert(id, score);
        }
    }
    let recent_score = *score_by_id
        .get("DEC-AXO-001")
        .expect("DEC-AXO-001 in waves");
    let old_score = *score_by_id.get("DEC-AXO-002").expect("DEC-AXO-002 in waves");
    assert!(
        old_score < recent_score,
        "100-day-old decision must score lower than 1-day-old decision when decay is enabled (recent={}, old={})",
        recent_score,
        old_score
    );

    // The old decision must surface a `decayed by age` reason because
    // its decay factor is well below 0.5 (~exp(-100/30) ≈ 0.036).
    let old_item = waves
        .iter()
        .flat_map(|wave| wave["items"].as_array().unwrap().iter())
        .find(|item| item["id"].as_str() == Some("DEC-AXO-002"))
        .expect("DEC-AXO-002 must be in waves");
    let reasons: Vec<&str> = old_item["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str())
        .collect();
    assert!(
        reasons.iter().any(|r| r.starts_with("decayed by age")),
        "old decision must surface 'decayed by age' reason: {:?}",
        reasons
    );

    // 2) Decay disabled — both decisions score identically again.
    let req_no_decay = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "include_decay": false }
        })),
        id: Some(json!(702)),
    };
    let response = server.handle_request(req_no_decay);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let waves = data["ordered_waves"].as_array().expect("waves");
    let mut score_by_id: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for wave in waves {
        for item in wave["items"].as_array().expect("items") {
            let id = item["id"].as_str().unwrap_or("").to_string();
            let score = item["score"].as_i64().unwrap_or(0);
            score_by_id.insert(id, score);
        }
    }
    let recent_score = *score_by_id
        .get("DEC-AXO-001")
        .expect("DEC-AXO-001 in waves");
    let old_score = *score_by_id.get("DEC-AXO-002").expect("DEC-AXO-002 in waves");
    assert_eq!(
        recent_score, old_score,
        "include_decay=false must yield identical scores for structurally identical nodes"
    );
}

#[test]
fn test_soll_work_plan_counts_decision_evidence() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', '', 'current', '{\"acceptance_criteria\":\"Runtime truth is visible\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', '', 'current', '{\"context\":\"\",\"rationale\":\"\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES ('TRC-DEC-AXO-001', 'decision', 'DEC-AXO-001', 'File', 'src/main.rs', 0.9, '{}', 1)")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_work_plan",
            "arguments": { "project_code": "AXO", "format": "json", "top": 1 }
        })),
        id: Some(json!(507)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").expect("data payload");
    let top = data["top_recommendations"]
        .as_array()
        .expect("top recommendations");
    let gates = top[0]["validation_gates"]
        .as_array()
        .expect("validation_gates");

    assert_eq!(top[0]["id"].as_str(), Some("DEC-AXO-001"));
    assert_ne!(
        top[0]["reason"].as_str(),
        Some("aucune evidence rattachee"),
        "{:?}",
        top[0]
    );
    assert!(
        gates
            .iter()
            .all(|gate| gate.as_str() != Some("attach evidence")),
        "{:?}",
        top[0]
    );
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
    crate::runtime_tuning::reset_runtime_tuning_snapshot(
        crate::embedder::bootstrap_runtime_tuning_state(),
    );
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

    assert!(content.contains("Known files: 6"), "{content}");
    assert!(content.contains("Remaining backlog: 2"), "{content}");
    assert!(content.contains("Pending: 1"), "{content}");
    assert!(content.contains("Indexing: 1"), "{content}");
    assert!(content.contains("Indexed degraded: 1"), "{content}");
    assert!(content.contains("Oversized: 1"), "{content}");
    assert!(content.contains("Skipped: 1"), "{content}");
    assert!(content.contains("DuckDB Storage"), "{content}");
    assert!(content.contains("RSS Anon"), "{content}");
    assert!(content.contains("DuckDB Memory"), "{content}");
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

    assert!(content.contains("Top backlog causes"), "{content}");
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

    let response = server.axon_status(&json!({"mode": "full"})).expect("status response");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Rust SDK selected for payment integration', 'current', '{\"rationale\":\"Operational safety\"}')").unwrap();
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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

    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Use Rust Stripe SDK', 'Operational payment choice', 'current', '{\"rationale\":\"Operational safety\"}')").unwrap();
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-FILE', 'Decision', 'BKS', 'Payment file rationale', 'File-level rationale', 'current', '{}')").unwrap();
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
fn test_retrieve_context_rationale_prefers_canonical_project_docs_over_workspace_noise() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }

    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('src/payment.rs', 'BKS', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('docs/plans/2026-04-20-bks-stripe-implementation-plan.md', 'BKS', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO File (path, project_code, graph_ready, status) VALUES ('.axon/cache/noise.md', 'BKS', TRUE, 'indexed')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::checkout', 'checkout', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::stripe_plan_doc', 'stripe_plan_doc', 'module', false, false, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::workspace_noise_doc', 'workspace_noise_doc', 'module', false, false, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'bks::checkout', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('docs/plans/2026-04-20-bks-stripe-implementation-plan.md', 'bks::stripe_plan_doc', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('.axon/cache/noise.md', 'bks::workspace_noise_doc', 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-bks-checkout-rationale', 'symbol', 'bks::checkout', 'BKS', 'body', 'checkout uses the Stripe SDK for payment capture and settlement', 'hash-bks-checkout-rationale', 1, 8)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-bks-plan-doc', 'symbol', 'bks::stripe_plan_doc', 'BKS', 'body', 'Implementation plan: use the Stripe SDK for operational safety and payment reliability.', 'hash-bks-plan-doc', 1, 8)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-bks-noise-doc', 'symbol', 'bks::workspace_noise_doc', 'BKS', 'body', 'Cached workspace note mentioning Stripe SDK without canonical project intent.', 'hash-bks-noise-doc', 1, 8)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-BKS-T6', 'Decision', 'BKS', 'Use Stripe SDK', 'Canonical rationale', 'current', '{\"rationale\":\"Operational safety\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-BKS-T6', 'Decision', 'DEC-BKS-T6', 'Symbol', 'checkout', 1.0, 0)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "Why does checkout use the Stripe SDK?",
                    "project": "BKS",
                    "token_budget": 1200,
                    "top_k": 4
                }
            })),
            id: Some(json!(6216)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let packet = &result["data"]["packet"];
    assert_eq!(
        packet["retrieval_policy"]["linked_evidence_first"].as_bool(),
        Some(true),
        "{packet:?}"
    );
    let chunks = packet["supporting_chunks"]
        .as_array()
        .expect("expected supporting chunks");
    let plan_pos = chunks
        .iter()
        .position(|value| value["chunk_id"].as_str() == Some("chunk-bks-plan-doc"));
    let noise_pos = chunks
        .iter()
        .position(|value| value["chunk_id"].as_str() == Some("chunk-bks-noise-doc"));
    assert!(plan_pos.is_some(), "{packet:?}");
    if let Some(noise_pos) = noise_pos {
        assert!(plan_pos.expect("plan pos") < noise_pos, "{packet:?}");
    }

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_retrieve_context_under_critical_pressure_avoids_unanchored_fallback_chunks() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-CRIT', 'Decision', 'AXO', 'Batch rationale', 'Why parse_batch exists', 'current', '{}')")
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
fn test_retrieve_context_under_recovering_pressure_keeps_soll_join_for_rationale() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    service_guard::reset_for_tests();
    service_guard::record_latency(ServiceKind::Mcp, 1_700);
    service_guard::record_latency(ServiceKind::Mcp, 140);

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
        .execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-anchor-rationale-recovering', 'symbol', 'axon::parse_batch', 'AXO', 'body', 'parse_batch commits the writer batch and updates lifecycle state', 'hash-anchor-rationale-recovering', 1, 12)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-RECOVERING', 'Decision', 'AXO', 'Keep parse_batch explicit', 'Rationale for parse_batch', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-RECOVERING', 'Decision', 'DEC-AXO-RECOVERING', 'Symbol', 'parse_batch', 1.0, 0)")
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
            id: Some(json!(6211)),
        })
        .unwrap();

    let result = response.result.expect("Expected result");
    let planner = &result["data"]["planner"];
    let packet = &result["data"]["packet"];
    assert_eq!(
        planner["route"].as_str(),
        Some("soll_hybrid"),
        "{planner:?}"
    );
    assert!(
        packet["relevant_soll_entities"]
            .as_array()
            .is_some_and(|rows| rows
                .iter()
                .any(|row| { row["id"].as_str() == Some("DEC-AXO-RECOVERING") })),
        "{packet:?}"
    );
    assert!(
        !packet["excluded_because"]
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
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
    let data = result.get("data").unwrap();
    assert_eq!(data["symbol"].as_str(), Some("axon_retrieve_context"));
    assert_eq!(data["impact_radius"].as_i64(), Some(1));
    assert_eq!(
        data["next_action"]["kind"].as_str(),
        Some("simulate_mutation_before_editing")
    );
    assert_eq!(
        data["next_action"]["tool"].as_str(),
        Some("simulate_mutation")
    );
    assert!(data["operator_guidance"]["follow_up_tools"].is_array());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_axon_inspect_accepts_canonical_project_code_for_repo_code_symbols() {
    // REQ-AXO-142 — rewritten to use `test_support::ist_fixtures` so the
    // production `Symbol` shape (including `is_unsafe`) is enforced by the
    // builder, not duplicated in inline SQL.
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let harness = crate::test_support::ist_fixtures::create_test_server_with_ist_seed(
        crate::test_support::ist_fixtures::IstSeed::new().symbol(
            crate::test_support::ist_fixtures::SymbolFixture::new(
                "axon::axon_retrieve_context",
                "axon_retrieve_context",
                "method",
                "AXO",
            )
            .tested(true)
            .is_public(true)
            .is_unsafe(false),
        ),
    )
    .unwrap();

    let response = harness
        .server
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
    assert!(content.contains("L2 Detail") || content.contains("Error"));
}

#[test]
fn test_send_notification() {
    let server = create_test_server();
    let notif = server.send_notification("notifications/tools/list_changed", None);
    assert_eq!(notif.method, "notifications/tools/list_changed");
    assert!(notif.params.is_none());

    let serialized = serde_json::to_string(&notif).unwrap();
    assert!(serialized.contains("notifications/tools/list_changed"));
}

#[test]
fn test_axon_inspect() {
    // REQ-AXO-142 — rewritten to use `test_support::ist_fixtures`. The
    // canonical-target_id branch of the inspect_callers_query (REQ-AXO-134)
    // is exercised here; the synthetic-target_id branch is in
    // `mcp::tools_dx::inspect_callers_query_tests`.
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, CallFixture, IstSeed, SymbolFixture,
    };
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let harness = create_test_server_with_ist_seed(
        IstSeed::new()
            .symbol(
                SymbolFixture::new("prj::core_func", "core_func", "function", "PRJ")
                    .tested(true),
            )
            .symbol(
                SymbolFixture::new("prj::caller_func", "caller_func", "function", "PRJ")
                    .tested(false),
            )
            .call(CallFixture::canonical(
                "prj::caller_func",
                "prj::core_func",
                "PRJ",
            )),
    )
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

    let response = harness.server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Symbol Inspection"), "{content}");
    assert!(content.contains("core_func"), "{content}");
    let data = result.get("data").unwrap();
    assert_eq!(data["symbol_found"].as_bool(), Some(true));
    assert_eq!(data["summary"]["kind"].as_str(), Some("function"));
    assert!(data["summary"]["tested"].is_boolean());
    assert_eq!(
        data["next_action"]["kind"].as_str(),
        Some("expand_dependency_blast_radius")
    );
    assert_eq!(data["next_action"]["tool"].as_str(), Some("impact"));
    assert!(data["operator_guidance"]["follow_up_tools"].is_array());
}

#[test]
fn test_axon_simulate_mutation_unknown_symbol_with_no_suggestions_recommends_widening() {
    // REQ-AXO-043 — fourth symmetric fix (inspect, path, impact, simulate).
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "simulate_mutation",
            "arguments": { "symbol": "completely_made_up_symbol_uvw_zzz_456" }
        })),
        id: Some(json!(50435)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");

    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        !content.contains("- retry with one suggested symbol"),
        "report must not list 'retry with one suggested symbol' when none exist: {content}"
    );
    assert!(
        content.contains("broaden") || content.contains("query") || content.contains("spelling"),
        "report must steer toward widening: {content}"
    );

    let data = result.get("data").expect("data block present");
    assert_eq!(data["symbol_found"].as_bool(), Some(false));
    let suggestions = data["suggestions"].as_array().expect("suggestions array");
    assert!(suggestions.is_empty(), "preconditions: no suggestions: {suggestions:?}");
    assert_eq!(data["next_action"]["kind"].as_str(), Some("broaden_search"));
    assert_eq!(data["next_action"]["tool"].as_str(), Some("query"));
}

#[test]
fn test_axon_anomalies_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — anomalies returned `Status: ok` with all-zero counts
    // for an unregistered project_code. Mirror the wrong_project_scope
    // contract used by soll_query_context / soll_work_plan.
    let _runtime = RuntimeEnvGuard::full_autonomous();
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
                "name": "anomalies",
                "arguments": { "project": "NOT_A_PROJECT_QQQ" }
            })),
            id: Some(json!(43103)),
        })
        .unwrap();
    let result = response.result.expect("Expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NOT_A_PROJECT_QQQ")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "registered codes must include seeded AXO: {registered_strs:?}"
    );
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    let action_text: String = actions
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        action_text.contains("workspace") || action_text.contains("omit"),
        "next_best_actions should mention the workspace fallback: {action_text}"
    );
}

#[test]
fn test_axon_why_empty_symbol_returns_recovery_contract() {
    // REQ-AXO-043 — symbol="" previously produced a malformed
    // "Why does  exist?" question (double space) that retrieve_context
    // happily processed, returning Status: ok. Trim and reject empty
    // input.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "why",
            "arguments": { "symbol": "  " }
        })),
        id: Some(json!(50434)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));
    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("symbol") && content.contains("question"),
        "content must mention both fields: {content}"
    );

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    assert_eq!(data["missing_field"].as_str(), Some("symbol_or_question"));
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    let follow_up = data["operator_guidance"]["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools");
    let follow_up_strs: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_strs.contains(&"inspect") || follow_up_strs.contains(&"retrieve_context"),
        "follow_up_tools should point to inspect/retrieve_context: {follow_up_strs:?}"
    );
}

#[test]
fn test_axon_impact_unknown_symbol_with_no_suggestions_recommends_widening() {
    // REQ-AXO-043 — `impact` had the same dead-end as inspect/path: report
    // and operator_guidance said "retry with one suggested symbol" even
    // when no suggestions could be produced. Verify the symmetric fix.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": {
                "symbol": "completely_made_up_symbol_qqq_zzz_999",
            }
        })),
        id: Some(json!(50433)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");

    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        !content.contains("- retry with one suggested symbol"),
        "report must not list 'retry with one suggested' when none exist: {content}"
    );

    let data = result.get("data").expect("data block present");
    assert_eq!(data["impact_available"].as_bool(), Some(false));
    let suggestions = data["suggestions"].as_array().expect("suggestions array");
    assert!(suggestions.is_empty(), "preconditions: no suggestions: {suggestions:?}");
    assert_eq!(data["next_action"]["kind"].as_str(), Some("broaden_search"));
    assert_eq!(data["next_action"]["tool"].as_str(), Some("query"));

    // remediation_actions must not advise picking a nonexistent suggestion
    let remediation = data["operator_guidance"]["remediation_actions"]
        .as_array()
        .expect("remediation_actions array");
    let remediation_text: String = remediation
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        !remediation_text.contains("retry with one suggested"),
        "remediation must not advise picking a nonexistent suggestion: {remediation_text}"
    );
}

#[test]
fn test_axon_path_unknown_symbol_with_no_suggestions_recommends_widening() {
    // REQ-AXO-043 — `path` (axon_bidi_trace) had the same dead-end as
    // inspect: report said "pick one suggested symbol" even when the
    // suggestion table was empty. Verify the symmetric fix.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "path",
            "arguments": {
                "source": "completely_made_up_symbol_zzz_abc_123",
            }
        })),
        id: Some(json!(50432)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");

    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        !content.contains("- pick one suggested symbol"),
        "report must not list 'pick one suggested symbol' when none exist: {content}"
    );
    assert!(
        content.contains("broaden") || content.contains("query") || content.contains("spelling"),
        "report must steer toward widening: {content}"
    );

    let data = result.get("data").expect("data block present");
    assert_eq!(data["symbol_found"].as_bool(), Some(false));
    let suggestions = data["suggestions"].as_array().expect("suggestions array");
    assert!(
        suggestions.is_empty(),
        "preconditions: no suggestions for nonsense symbol: {suggestions:?}"
    );
    assert_eq!(data["next_action"]["kind"].as_str(), Some("broaden_search"));
    assert_eq!(data["next_action"]["tool"].as_str(), Some("query"));
}

#[test]
fn test_axon_inspect_unknown_symbol_with_no_suggestions_recommends_widening() {
    // REQ-AXO-043 — when no suggestions can be produced, the next_action /
    // remediation_actions must NOT say "pick one suggested symbol" because
    // there is nothing to pick from. They must steer the LLM toward
    // widening the search instead.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": {
                "symbol": "completely_made_up_symbol_xyz_abc_123",
            }
        })),
        id: Some(json!(50431)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").unwrap();
    assert_eq!(data["symbol_found"].as_bool(), Some(false));

    let suggestions = data["suggestions"].as_array().expect("suggestions array");
    assert!(suggestions.is_empty(), "preconditions: no suggestions for nonsense symbol");

    let next_action_kind = data["next_action"]["kind"].as_str().unwrap_or("");
    assert_eq!(
        next_action_kind, "broaden_search",
        "empty-suggestions case must route to broaden_search, not pick_canonical_symbol"
    );
    assert_eq!(data["next_action"]["tool"].as_str(), Some("query"));

    let remediation = data["operator_guidance"]["remediation_actions"]
        .as_array()
        .expect("remediation_actions array");
    let remediation_text: String = remediation
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        !remediation_text.contains("pick one suggested"),
        "must not advise picking a suggestion when none exist: {remediation_text}"
    );
    assert!(
        remediation_text.contains("query") || remediation_text.contains("broaden") || remediation_text.contains("spelling"),
        "must steer toward widening/spelling: {remediation_text}"
    );

    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        !content.contains("- pick one suggested symbol"),
        "report must not list 'pick one suggested symbol' when none exist: {content}"
    );
}

#[test]
fn test_axon_inspect_unknown_symbol_returns_parameter_repair_with_widening_actions() {
    // REQ-AXO-139 slice — universal parameter_repair contract for inspect
    // symbol-not-found. Mirrors cypher-binder + soll_attach_evidence slices.
    // When no suggestions exist the LLM should be steered toward widening
    // tools (`query`, `list_labels_tables`) instead of guessing.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": {
                "symbol": "totally_unrelated_symbol_zz_qq_42",
            }
        })),
        id: Some(json!(50443)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let data = result.get("data").unwrap();
    assert_eq!(data["symbol_found"].as_bool(), Some(false));

    let repair = data
        .get("parameter_repair")
        .expect("parameter_repair payload required for inspect not-found");
    assert_eq!(repair["invalid_field"].as_str(), Some("symbol"));
    assert_eq!(
        repair["supplied_value"].as_str(),
        Some("totally_unrelated_symbol_zz_qq_42")
    );
    assert!(
        repair["scope"].as_str().is_some(),
        "parameter_repair must surface scope: {repair}"
    );

    let widening = repair["widening_actions"]
        .as_array()
        .expect("widening_actions array");
    let widening_text: String = widening
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        widening_text.contains("query") || widening_text.contains("less specific"),
        "widening_actions must steer toward broader query: {widening_text}"
    );

    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_up_names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_names.contains(&"query"),
        "no-suggestions follow_up_tools must include `query`: {follow_up_names:?}"
    );
    assert!(
        follow_up_names.contains(&"list_labels_tables"),
        "no-suggestions follow_up_tools must include `list_labels_tables`: {follow_up_names:?}"
    );

    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("totally_unrelated_symbol_zz_qq_42"),
        "hint must reference the supplied symbol: {hint}"
    );
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
    assert!(content.contains("graph-derived context"), "{content}");
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

    assert!(!content.contains("graph-derived context"));
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
    assert!(content.contains("temporarily disabled"), "{content}");
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

    assert!(content.contains("Technical Debt"));
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

    assert!(content.contains("Technical Debt"));
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

    assert!(content.contains("Technical Debt"));
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

    assert!(content.contains("Security: 100/100"), "{}", content);
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

    assert!(content.contains("Compliance Audit: PJA"));
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

// REQ-AXO-264 Phase A — layered envelope contract.
//
// Backward-compat first: the existing `retrieve_context` tool remains
// unchanged. The new `retrieve_context_layered` tool wraps it and
// returns the three bands (intent / code / recent) in a single
// machine-actionable response. v0 stub: code_band reuses the existing
// packet; recent_band is empty + tagged `not_yet_implemented`.
#[test]
fn test_retrieve_context_layered_returns_three_bands_in_one_call() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::checkout', 'checkout', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/payment.rs', 'AXO', 'indexed')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/payment.rs', 'axo::checkout', 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout-layered', 'symbol', 'axo::checkout', 'AXO', 'body', 'checkout orchestrates payment capture', 'hash-checkout-layered', 1, 4)").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-264-T', 'Requirement', 'AXO', 'Layered envelope', 'Phase A multi-resolution retrieval test fixture', 'current', '{\"priority\":\"P1\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-LAYERED', 'Requirement', 'REQ-AXO-264-T', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context_layered",
                "arguments": {
                    "question": "how does checkout orchestrate payment capture?",
                    "project": "AXO",
                    "mode": "brief",
                }
            })),
            id: Some(json!(2640)),
        })
        .unwrap()
        .result
        .unwrap();

    // Bands present in data
    assert!(
        response["data"]["intent_band"].is_object(),
        "intent_band missing: {response}"
    );
    assert!(
        response["data"]["code_band"].is_object(),
        "code_band missing: {response}"
    );
    assert!(
        response["data"]["recent_band"].is_object(),
        "recent_band missing: {response}"
    );
    assert!(
        response["data"]["metadata"].is_object(),
        "metadata missing: {response}"
    );

    // Each band reports its token usage
    assert!(response["data"]["intent_band"]["tokens_used"].is_number());
    assert!(response["data"]["code_band"]["tokens_used"].is_number());
    assert!(response["data"]["recent_band"]["tokens_used"].is_number());

    // Metadata exposes retrieval path + total tokens + elapsed
    assert!(response["data"]["metadata"]["retrieval_path"].is_string());
    assert!(response["data"]["metadata"]["total_tokens"].is_number());
    assert!(response["data"]["metadata"]["elapsed_ms"].is_number());

    // v1 contract: recent_band reports a `status` so LLM clients know
    // whether the git lookup succeeded or fell back. Acceptable values:
    //   - "ok": git log ran (entries may still be empty if no recent commits)
    //   - "no_project_root": no AXON_PROJECT_ROOT/cwd resolvable
    //   - "git_error": git invocation failed (e.g. not a repo, git missing)
    let recent_status = response["data"]["recent_band"]["status"].as_str().unwrap_or("");
    assert!(
        matches!(recent_status, "ok" | "no_project_root" | "git_error"),
        "recent_band.status must be one of {{ok, no_project_root, git_error}}, got {recent_status:?}",
    );

    // intent_band should surface the SOLL Requirement we inserted
    let intent_reqs = response["data"]["intent_band"]["requirements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        intent_reqs.iter().any(|row| row["id"].as_str() == Some("REQ-AXO-264-T")),
        "intent_band.requirements should contain REQ-AXO-264-T, got {:?}",
        intent_reqs
    );

    // code_band carries chunks (reuse of existing packet evidence)
    let code_chunks = response["data"]["code_band"]["chunks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !code_chunks.is_empty(),
        "code_band.chunks should not be empty for a question with code evidence (got {:?})",
        code_chunks
    );
}

// REQ-AXO-264 backward compat — the original `retrieve_context` tool
// must remain unchanged in shape (no `intent_band`, no `code_band`,
// no `recent_band`, no `metadata.retrieval_path` at the top of `data`).
#[test]
fn test_retrieve_context_legacy_shape_unchanged_after_layered_addition() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::cl', 'cl', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/cl.rs', 'AXO', 'indexed')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/cl.rs', 'axo::cl', 'AXO')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "where is cl defined?",
                    "project": "AXO",
                    "mode": "brief",
                }
            })),
            id: Some(json!(2641)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(response["data"]["packet"].is_object(),
        "legacy retrieve_context must still expose `data.packet`");
    assert!(response["data"]["intent_band"].is_null(),
        "legacy retrieve_context must NOT expose intent_band");
    assert!(response["data"]["code_band"].is_null(),
        "legacy retrieve_context must NOT expose code_band");
    assert!(response["data"]["recent_band"].is_null(),
        "legacy retrieve_context must NOT expose recent_band");
}

// REQ-AXO-264 A6 v1 — recent_band must surface git log entries from
// the last 24h on the resolved project root, deduped per file with the
// most recent commit winning.
#[test]
fn test_recent_band_collects_git_log_within_24h_window() {
    use std::process::Command;
    let tempdir = tempdir().unwrap();
    let repo = tempdir.path();

    // Init a fresh git repo and produce one commit touching two files.
    let must_run = |cmd: &mut Command| {
        let status = cmd.status().unwrap_or_else(|err| panic!("command failed to start: {err}"));
        assert!(status.success(), "command failed: {:?}", cmd);
    };
    must_run(Command::new("git").arg("-C").arg(repo).arg("init").arg("-q"));
    must_run(Command::new("git").arg("-C").arg(repo).arg("config").arg("user.email").arg("test@axon.local"));
    must_run(Command::new("git").arg("-C").arg(repo).arg("config").arg("user.name").arg("Axon Test"));
    std::fs::write(repo.join("alpha.rs"), b"// alpha\n").unwrap();
    std::fs::write(repo.join("beta.rs"), b"// beta\n").unwrap();
    must_run(Command::new("git").arg("-C").arg(repo).arg("add").arg("alpha.rs").arg("beta.rs"));
    must_run(Command::new("git").arg("-C").arg(repo).arg("commit").arg("-q").arg("-m").arg("layered-test seed commit"));

    let band = McpServer::collect_recent_band(Some(repo.to_string_lossy().as_ref()));
    assert_eq!(band["status"].as_str(), Some("ok"), "band: {band}");
    let edits = band["git_recent_edits"].as_array().cloned().unwrap_or_default();
    let files: Vec<&str> = edits.iter()
        .filter_map(|e| e["file"].as_str())
        .collect();
    assert!(files.contains(&"alpha.rs"), "alpha.rs missing from {files:?}");
    assert!(files.contains(&"beta.rs"), "beta.rs missing from {files:?}");
    let first = &edits[0];
    assert!(first["last_commit_subject"].as_str().unwrap_or("").contains("layered-test seed commit"));
    assert!(band["tokens_used"].as_u64().unwrap_or(0) > 0);
}

// REQ-AXO-264 A6 v1 — recent_band returns a stable structured response
// when the project root is missing or invalid, instead of crashing or
// returning a bare error.
#[test]
fn test_recent_band_returns_stable_contract_when_no_project_root() {
    let none_band = McpServer::collect_recent_band(None);
    assert_eq!(none_band["status"].as_str(), Some("no_project_root"));
    assert!(none_band["git_recent_edits"].as_array().map_or(false, |a| a.is_empty()));
    assert_eq!(none_band["tokens_used"].as_u64(), Some(0));

    let bogus_band = McpServer::collect_recent_band(Some("/nonexistent/axon/test/path/does-not-exist"));
    assert_eq!(bogus_band["status"].as_str(), Some("no_project_root"));
    assert!(bogus_band["git_recent_edits"].as_array().map_or(false, |a| a.is_empty()));
}

// REQ-AXO-264 A6 v1 — non-git directory must return git_error contract,
// not panic, not ok-with-empty.
#[test]
fn test_recent_band_returns_git_error_when_not_a_repo() {
    let tempdir = tempdir().unwrap();
    let band = McpServer::collect_recent_band(Some(tempdir.path().to_string_lossy().as_ref()));
    assert_eq!(band["status"].as_str(), Some("git_error"), "band: {band}");
    assert!(band["git_recent_edits"].as_array().map_or(false, |a| a.is_empty()));
}

// REQ-AXO-264 A3 v2 — default budgets surfaced when caller omits `bands`.
#[test]
fn test_retrieve_context_layered_surfaces_default_budgets() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axo::small', 'small', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/small.rs', 'AXO', 'indexed')").unwrap();
    server.graph_store.execute("INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/small.rs', 'axo::small', 'AXO')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context_layered",
                "arguments": {
                    "question": "where is small defined?",
                    "project": "AXO",
                    "mode": "brief",
                }
            })),
            id: Some(json!(2642)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(response["data"]["intent_band"]["tokens_budget"].as_u64(), Some(2000));
    assert_eq!(response["data"]["code_band"]["tokens_budget"].as_u64(), Some(6000));
    // recent_band budget enforcement is internal — not surfaced as a field
    // on recent_band itself but counted in tokens_overflowed.
    assert!(response["data"]["intent_band"]["tokens_overflowed"].is_number());
    assert!(response["data"]["code_band"]["tokens_overflowed"].is_number());
    assert!(response["data"]["metadata"]["tokens_pre_truncation"].is_object());
    assert!(response["data"]["metadata"]["total_tokens_overflowed"].is_number());
    assert_eq!(
        response["data"]["metadata"]["phase_a_version"].as_str(),
        Some("v2"),
    );
}

// REQ-AXO-264 A3 v2 — caller-supplied `bands.code.max_tokens` truncates
// the chunks band; tokens_overflowed counts dropped rows.
#[test]
fn test_retrieve_context_layered_truncates_code_band_under_budget() {
    let server = create_test_server();
    // Many chunks so the unrestricted code_band exceeds a small budget.
    for i in 0..30 {
        let sym_id = format!("axo::big_{i}");
        let chunk_id = format!("chunk-big-{i}");
        let chunk_content = format!("function big_{i} repeats long text for token budgeting tests, repeated to grow tokens above the budget; lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua, ut enim ad minim veniam quis nostrud exercitation");
        server.graph_store.execute(&format!(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_id}', 'big_{i}', 'function', true, true, false, 'AXO')"
        )).unwrap();
        server.graph_store.execute(&format!(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{chunk_id}', 'symbol', '{sym_id}', 'AXO', 'body', '{chunk_content}', 'hash-big-{i}', 1, 30)"
        )).unwrap();
    }
    server.graph_store.execute("INSERT INTO File (path, project_code, status) VALUES ('src/big.rs', 'AXO', 'indexed')").unwrap();
    for i in 0..30 {
        server.graph_store.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('src/big.rs', 'axo::big_{i}', 'AXO')"
        )).unwrap();
    }

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context_layered",
                "arguments": {
                    "question": "where is big defined?",
                    "project": "AXO",
                    "mode": "brief",
                    "top_k": 20,
                    "bands": {
                        "code": {"max_tokens": 200},
                    },
                }
            })),
            id: Some(json!(2643)),
        })
        .unwrap()
        .result
        .unwrap();

    let kept = response["data"]["code_band"]["chunks"].as_array().cloned().unwrap_or_default();
    let used = response["data"]["code_band"]["tokens_used"].as_u64().unwrap_or(0);
    let budget = response["data"]["code_band"]["tokens_budget"].as_u64().unwrap_or(0);
    let overflowed = response["data"]["code_band"]["tokens_overflowed"].as_u64().unwrap_or(0);

    assert_eq!(budget, 200, "explicit budget should be surfaced");
    assert!(used <= budget, "tokens_used ({used}) must be <= budget ({budget})");
    assert!(
        overflowed > 0 || kept.is_empty(),
        "with a 200-token budget on 30 fat chunks we expect either truncation overflow > 0 or empty kept set; got kept={} overflowed={}",
        kept.len(), overflowed
    );
}
