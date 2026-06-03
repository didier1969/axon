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
    server.graph_store.execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/payment.rs', 'symbol', 'sym-src/payment.rs', 'BKS', 'src/payment.rs', 'hash-src/payment.rs')").unwrap();
    server.graph_store.execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/payment.rs', 'bks::checkout', 'CONTAINS', 'BKS', 0)").unwrap();
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/refund.rs', 'symbol', 'sym-src/refund.rs', 'BKS', 'src/refund.rs', 'hash-src/refund.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/refund.rs', 'bks::refund', 'CONTAINS', 'BKS', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/dashboard/lib/axon_dashboard/application.ex', 'symbol', 'sym-src/dashboard/lib/axon_dashboard/application.ex', 'AXO', 'src/dashboard/lib/axon_dashboard/application.ex', 'hash-src/dashboard/lib/axon_dashboard/application.ex')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/dashboard/lib/axon_dashboard/application.ex', 'axon::dashboard_surface', 'CONTAINS', 'AXO', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-scripts/refund_probe.rs', 'symbol', 'bks::refund_probe', 'BKS', 'scripts/refund_probe.rs', 'hash-scripts/refund_probe.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('scripts/refund_probe.rs', 'bks::refund_probe', 'CONTAINS', 'BKS', 0)")
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
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/lib.rs', 'symbol', 'sym-src/lib.rs', 'AXO', 'src/lib.rs', 'hash-src/lib.rs')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::wrapper', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::wrapper', 'axo::target', 'CALLS', 'AXO', 0)")
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
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/lib.rs', 'symbol', 'sym-src/lib.rs', 'AXO', 'src/lib.rs', 'hash-src/lib.rs')",
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
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::wrapper', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::wrapper', 'axo::target', 'CALLS', 'AXO', 0)")
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
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::orphan', 'CONTAINS', 'AXO', 0)")
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
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/lib.rs', 'symbol', 'sym-src/lib.rs', 'AXO', 'src/lib.rs', 'hash-src/lib.rs')",
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
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::wrapper', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::wrapper', 'axo::target', 'CALLS', 'AXO', 0)")
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
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::orphan', 'CONTAINS', 'AXO', 0)")
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
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/api.rs', 'symbol', 'sym-src/api.rs', 'AXO', 'src/api.rs', 'hash-src/api.rs')",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/impl.rs', 'symbol', 'sym-src/impl.rs', 'AXO', 'src/impl.rs', 'hash-src/impl.rs')")
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
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/api.rs', 'axo::iface', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/impl.rs', 'axo::svc', 'CONTAINS', 'AXO', 0)")
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
fn test_change_safety_exposes_trimodal_envelope_on_happy_path() {
    // REQ-AXO-91514 — happy-path change_safety call returns the
    // GUI-AXO-1003 envelope (surfaces_used, surfaces_degraded,
    // total_available, next_call_hint, pagination) alongside the
    // existing verdict shape. Bug : a fixture row may not exist for
    // the target ; that's fine — the envelope must populate
    // regardless of whether the verdict is `safe`, `unsafe`, or
    // `unknown`.
    let server = create_test_server();
    super::delete_fixture_symbols(&server, &["axo::cs_target"]);
    server
        .graph_store
        .execute(
            "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES \
             ('axo::cs_target', 'cs_target_fn', 'function', true, true, false, 'AXO')",
        )
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "change_safety",
                "arguments": {
                    "project_code": "AXO",
                    "target": "cs_target_fn",
                    "target_type": "symbol"
                }
            })),
            id: Some(json!(23012)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("data");
    let surfaces: Vec<&str> = data["surfaces_used"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(surfaces.contains(&"symbol_index"));
    assert!(surfaces.contains(&"soll_traceability"));
    assert!(data["surfaces_degraded"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(false));
    assert_eq!(data["total_available"].as_u64(), Some(1));
    assert_eq!(
        data["next_call_hint"].as_str(),
        Some("impact symbol=cs_target_fn")
    );
    assert_eq!(data["pagination"]["offset"].as_u64(), Some(0));
    assert_eq!(data["pagination"]["limit"].as_u64(), Some(1));
    assert!(data["pagination"]["next_offset"].is_null());
    // Verdict fields still present (additive contract).
    assert!(data["change_safety"].as_str().is_some());
    assert!(data["coverage_signals"].is_object());
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
    // REQ-AXO-91562 workaround — tests share live PG, wipe fixture ids
    // before INSERT to avoid PK collisions from prior runs.
    super::delete_fixture_symbols(&server, &["bks::source", "bks::mid", "bks::sink"]);
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::source', 'source_fn', 'function', true, true, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::mid', 'mid_fn', 'function', true, false, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::sink', 'sink_fn', 'function', true, true, false, 'BKS')").unwrap();
    // MIL-AXO-017 / REQ-AXO-216 — legacy CALLS table dropped, edges
    // now live in unified ist.Edge with relation_type='CALLS'.
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('bks::source', 'bks::mid', 'CALLS', 'BKS', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('bks::mid', 'bks::sink', 'CALLS', 'BKS', 0)")
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

    // REQ-AXO-91510 — tri-modal envelope conformance (GUI-AXO-1003).
    // Cache is cold in this test → PG fallback surface = "graph_pg" +
    // "graph_ram_unavailable" in degraded. RAM-warm case is covered by
    // test_path_uses_ram_snapshot_when_warm below.
    let surfaces: Vec<&str> = data["surfaces_used"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        surfaces.contains(&"graph_pg") || surfaces.contains(&"graph_ram"),
        "surfaces_used must contain graph_pg or graph_ram, got {surfaces:?}"
    );
    assert_eq!(data["total_available"].as_u64(), Some(1));
    assert_eq!(data["next_call_hint"].as_str(), Some("impact symbol=sink_fn"));
    assert_eq!(data["pagination"]["offset"].as_u64(), Some(0));
    assert_eq!(data["pagination"]["limit"].as_u64(), Some(3));
    assert!(data["pagination"]["next_offset"].is_null());
}

#[test]
fn test_path_not_found_branch_exposes_trimodal_envelope() {
    // REQ-AXO-91510 — envelope must populate on the no-path branch too.
    let server = create_test_server();
    super::delete_fixture_symbols(&server, &["bks::isolated_a", "bks::isolated_b"]);
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::isolated_a', 'isolated_a', 'function', true, true, false, 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('bks::isolated_b', 'isolated_b', 'function', true, true, false, 'BKS')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "path",
                "arguments": {
                    "source": "isolated_a",
                    "sink": "isolated_b",
                    "project": "BKS",
                    "depth": 3
                }
            })),
            id: Some(json!(2206)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(data["path_found"].as_bool(), Some(false));
    let surfaces: Vec<&str> = data["surfaces_used"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        surfaces.contains(&"graph_pg") || surfaces.contains(&"graph_ram"),
        "surfaces_used must contain graph_pg or graph_ram, got {surfaces:?}"
    );
    assert_eq!(data["total_available"].as_u64(), Some(0));
    assert_eq!(data["next_call_hint"].as_str(), Some("inspect symbol=isolated_a"));
    assert_eq!(data["pagination"]["offset"].as_u64(), Some(0));
    assert!(data["pagination"]["next_offset"].is_null());
}

#[test]
fn test_path_uses_ram_snapshot_when_warm() {
    // REQ-AXO-91510 — when IstGraphView is warm for the project, the
    // BFS runs in RAM (PIL-AXO-9002, feedback_trimodal_use_ram_graph_
    // not_pg) and `surfaces_used` reports `graph_ram` with empty
    // `surfaces_degraded`.
    use crate::ist_snapshot::snapshot::{
        EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
    };
    use std::sync::Arc;

    let server = create_test_server();
    super::delete_fixture_symbols(&server, &["ram::source", "ram::mid", "ram::sink"]);
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('ram::source', 'ram_source_fn', 'function', true, true, false, 'RAM')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('ram::mid', 'ram_mid_fn', 'function', true, false, false, 'RAM')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('ram::sink', 'ram_sink_fn', 'function', true, true, false, 'RAM')").unwrap();

    // Warm the process-level IstGraphView cache directly.
    let nodes = vec![
        NodeRecord {
            id: "ram::source".into(),
            project_code: "RAM".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
        NodeRecord {
            id: "ram::mid".into(),
            project_code: "RAM".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
        NodeRecord {
            id: "ram::sink".into(),
            project_code: "RAM".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
    ];
    let edges = vec![
        EdgeTriple {
            source: "ram::source".into(),
            target: "ram::mid".into(),
            rel: RelationType::Calls,
        },
        EdgeTriple {
            source: "ram::mid".into(),
            target: "ram::sink".into(),
            rel: RelationType::Calls,
        },
    ];
    let graph = Arc::new(IstGraph::build(nodes, edges));
    crate::ist_snapshot::publish_process_snapshot("RAM".into(), graph);
    std::env::set_var("AXON_IST_RAM_ENABLED", "1");

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "path",
                "arguments": {
                    "source": "ram_source_fn",
                    "sink": "ram_sink_fn",
                    "project": "RAM",
                    "depth": 4
                }
            })),
            id: Some(json!(2207)),
        })
        .unwrap()
        .result
        .unwrap();

    std::env::remove_var("AXON_IST_RAM_ENABLED");
    crate::ist_snapshot::evict_process_snapshot("RAM");

    let data = response.get("data").unwrap();
    assert_eq!(data["path_found"].as_bool(), Some(true));
    let surfaces: Vec<&str> = data["surfaces_used"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(
        surfaces,
        vec!["graph_ram"],
        "warm RAM cache must serve via graph_ram surface, not PG fallback"
    );
    assert!(
        data["surfaces_degraded"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "RAM-served path must report empty surfaces_degraded"
    );
    let path: Vec<&str> = data["path"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(path, vec!["ram_source_fn", "ram_mid_fn", "ram_sink_fn"]);
    let provenance = data["provenance"].as_str().unwrap_or_default();
    assert!(
        provenance.contains("IstGraph::bfs_shortest_path"),
        "provenance must reference RAM BFS, got: {provenance}"
    );
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
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/lib.rs', 'symbol', 'sym-src/lib.rs', 'AXO', 'src/lib.rs', 'hash-src/lib.rs')",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::wrapper', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::orphan', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::wrapper', 'axo::target', 'CALLS', 'AXO', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/source.rs', 'symbol', 'sym-src/source.rs', 'AXO', 'src/source.rs', 'hash-src/source.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/foreign.rs', 'symbol', 'sym-src/foreign.rs', 'AXO', 'src/foreign.rs', 'hash-src/foreign.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/interface.rs', 'symbol', 'sym-src/interface.rs', 'AXO', 'src/interface.rs', 'hash-src/interface.rs')")
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
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/source.rs', 'axo::source', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/source.rs', 'axo::local_helper', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/source.rs', 'axo::entry', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/source.rs', 'axo::bridge', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/source.rs', 'axo::sink', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/foreign.rs', 'axo::foreign_a', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/foreign.rs', 'axo::foreign_b', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/interface.rs', 'axo::iface', 'CONTAINS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/interface.rs', 'axo::iface_impl', 'CONTAINS', 'AXO', 0)")
        .unwrap();

    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::source', 'axo::local_helper', 'CALLS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::source', 'axo::foreign_a', 'CALLS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::source', 'axo::foreign_b', 'CALLS', 'AXO', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::entry', 'axo::bridge', 'CALLS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axo::bridge', 'axo::sink', 'CALLS', 'AXO', 0)")
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

/// REQ-AXO-901617 — `actionable` flag defaults to true so wave-1 surfaces
/// REQ leaves rather than parent Decisions. Without the flip, accepted
/// Decisions with no evidence dominated wave-1 over the actual work items.
#[test]
fn test_soll_work_plan_actionable_defaults_true_emits_req_leaves() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'work item', '', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'intent', '', 'current', '{}')")
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
            "arguments": { "project_code": "AXO", "format": "json" }
        })),
        id: Some(json!(901617)),
    };
    let response = server.handle_request(req);
    let data = response
        .unwrap()
        .result
        .expect("result")
        .get("data")
        .cloned()
        .expect("data");
    assert_eq!(
        data["metadata"]["actionable"].as_bool(),
        Some(true),
        "default must be actionable=true: {data:?}"
    );
    let waves = data["ordered_waves"].as_array().expect("waves");
    let ids: Vec<&str> = waves
        .iter()
        .flat_map(|w| w["items"].as_array().unwrap().iter())
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"REQ-AXO-001"),
        "REQ leaf must surface with default actionable=true: {ids:?}"
    );
    assert!(
        !ids.contains(&"DEC-AXO-001"),
        "Decision must NOT surface with default actionable=true: {ids:?}"
    );
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
            // REQ-AXO-901617: actionable now defaults true. This test asserts
            // legacy parent-Decision/Milestone ordering, so opt back into the
            // pre-flip behavior via actionable=false.
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "include_ist": true, "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "limit": 2, "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "top": 1, "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "include_decay": false, "actionable": false }
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
            "arguments": { "project_code": "AXO", "format": "json", "top": 1, "actionable": false }
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


// list_labels_tables tool removed (post-MIL-AXO-017 legacy cleanup).



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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-ui/app.js', 'symbol', 'sym-ui/app.js', 'PRJ', 'ui/app.js', 'hash-ui/app.js')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::fetchData', 'fetchData', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-db/repo.rs', 'symbol', 'sym-db/repo.rs', 'PRJ', 'db/repo.rs', 'hash-db/repo.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::executeSQL', 'executeSQL', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('ui/app.js', 'prj::fetchData', 'CONTAINS', 'PRJ', 0)",
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('db/repo.rs', 'prj::executeSQL', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::fetchData', 'prj::executeSQL', 'CALLS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-prj/f1.rs', 'symbol', 'sym-prj/f1.rs', 'PRJ', 'prj/f1.rs', 'hash-prj/f1.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-prj/f2.rs', 'symbol', 'sym-prj/f2.rs', 'PRJ', 'prj/f2.rs', 'hash-prj/f2.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::auth_func', 'auth_func', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj/f1.rs', 'prj::auth_func', 'CONTAINS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/core/api.rs', 'symbol', 'sym-src/core/api.rs', 'AXO', 'src/core/api.rs', 'hash-src/core/api.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/core/consumer_a.rs', 'symbol', 'sym-src/core/consumer_a.rs', 'AXO', 'src/core/consumer_a.rs', 'hash-src/core/consumer_a.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/core/consumer_b.rs', 'symbol', 'sym-src/core/consumer_b.rs', 'AXO', 'src/core/consumer_b.rs', 'hash-src/core/consumer_b.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::parse_batch', 'parse_batch', 'function', true, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_a', 'consumer_a', 'function', false, true, false, 'AXO')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('axon::consumer_b', 'consumer_b', 'function', false, true, false, 'AXO')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/core/api.rs', 'axon::parse_batch', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/core/consumer_a.rs', 'axon::consumer_a', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/core/consumer_b.rs', 'axon::consumer_b', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axon::consumer_a', 'axon::parse_batch', 'CALLS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axon::consumer_b', 'axon::parse_batch', 'CALLS', 'AXO', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/payment.rs', 'symbol', 'sym-src/payment.rs', 'BKS', 'src/payment.rs', 'hash-src/payment.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('api::checkout', 'checkout', 'function', true, true, false, 'BKS')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/payment.rs', 'api::checkout', 'CONTAINS', 'BKS', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/runtime/router.rs', 'symbol', 'sym-src/runtime/router.rs', 'AXO', 'src/runtime/router.rs', 'hash-src/runtime/router.rs')")
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
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/runtime/router.rs', 'axon::trigger_scan', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/runtime/router.rs', 'axon::worker_loop', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('axon::worker_loop', 'axon::trigger_scan', 'CALLS', 'AXO', 0)")
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
    // tools (`query`, `schema_overview`) instead of guessing.
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
        follow_up_names.contains(&"schema_overview"),
        "no-suggestions follow_up_tools must include `schema_overview`: {follow_up_names:?}"
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/auth.rs', 'symbol', 'sym-src/auth.rs', 'PRJ', 'src/auth.rs', 'hash-src/auth.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/access.rs', 'symbol', 'sym-src/access.rs', 'PRJ', 'src/access.rs', 'hash-src/access.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/auth.rs', 'prj::authorize_request', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/access.rs', 'prj::check_token_chain', 'CONTAINS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/auth.rs', 'symbol', 'sym-src/auth.rs', 'PRJ', 'src/auth.rs', 'hash-src/auth.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/access.rs', 'symbol', 'sym-src/access.rs', 'PRJ', 'src/access.rs', 'hash-src/access.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/auth.rs', 'prj::authorize_request', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/access.rs', 'prj::check_token_chain', 'CONTAINS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/auth.rs', 'symbol', 'sym-src/auth.rs', 'PRJ', 'src/auth.rs', 'hash-src/auth.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/access.rs', 'symbol', 'sym-src/access.rs', 'PRJ', 'src/access.rs', 'hash-src/access.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::authorize_request', 'authorize_request', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::check_token_chain', 'check_token_chain', 'function', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/auth.rs', 'prj::authorize_request', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/access.rs', 'prj::check_token_chain', 'CONTAINS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/api.rs', 'symbol', 'sym-src/api.rs', 'PRJ', 'src/api.rs', 'hash-src/api.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/api_dummy.rs', 'symbol', 'sym-src/api_dummy.rs', 'PRJ', 'src/api_dummy.rs', 'hash-src/api_dummy.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::user_input', 'user_input', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::run_task', 'run_task', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::eval', 'eval', 'function', false, true, false, true, 'PRJ')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/api.rs', 'prj::user_input', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::user_input', 'prj::run_task', 'CALLS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::run_task', 'prj::eval', 'CALLS', 'PRJ', 0)",
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/danger.rs', 'symbol', 'sym-src/danger.rs', 'PRJ', 'src/danger.rs', 'hash-src/danger.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::risky_func', 'risky_func', 'function', false, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::unwrap', 'unwrap', 'method', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/danger.rs', 'prj::risky_func', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::risky_func', 'prj::unwrap', 'CALLS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/todo.rs', 'symbol', 'sym-src/todo.rs', 'PRJ', 'src/todo.rs', 'hash-src/todo.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::todo1', '// TODO: Fix this', 'TODO', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/todo.rs', 'prj::todo1', 'CONTAINS', 'PRJ', 0)",
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/config.rs', 'symbol', 'sym-src/config.rs', 'PRJ', 'src/config.rs', 'hash-src/config.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/config.rs', 'prj::secret1', 'CONTAINS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/api.ex', 'symbol', 'sym-src/api.ex', 'PRJ', 'src/api.ex', 'hash-src/api.ex')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/api_dummy.ex', 'symbol', 'sym-src/api_dummy.ex', 'PRJ', 'src/api_dummy.ex', 'hash-src/api_dummy.ex')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::elixir_func', 'elixir_func', 'function', false, true, false, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::rust_nif', 'rust_nif', 'function', false, true, true, false, 'PRJ')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('prj::unsafe_block', 'unsafe_block', 'function', false, true, false, true, 'PRJ')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/api.ex', 'prj::elixir_func', 'CONTAINS', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::elixir_func', 'prj::rust_nif', 'CALLS_NIF', 'PRJ', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::rust_nif', 'prj::unsafe_block', 'CALLS', 'PRJ', 0)")
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/god.rs', 'symbol', 'sym-src/god.rs', 'PRJ', 'src/god.rs', 'hash-src/god.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/god_dummy.rs', 'symbol', 'sym-src/god_dummy.rs', 'PRJ', 'src/god_dummy.rs', 'hash-src/god_dummy.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::GodClass', 'GodClass', 'class', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/god.rs', 'prj::GodClass', 'CONTAINS', 'PRJ', 0)",
        )
        .unwrap();

    for i in 0..20 {
        server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::dep{}', 'dep{}', 'function', false, true, false, 'PRJ')", i, i)).unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::dep{}', 'prj::GodClass', 'CALLS', 'PRJ', 0)", i))
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-apps/pja/lib/input.rs', 'symbol', 'sym-apps/pja/lib/input.rs', 'PJA', 'apps/pja/lib/input.rs', 'hash-apps/pja/lib/input.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-apps/pjb/lib/unsafe.rs', 'symbol', 'sym-apps/pjb/lib/unsafe.rs', 'PJB', 'apps/pjb/lib/unsafe.rs', 'hash-apps/pjb/lib/unsafe.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJA::safe_entry', 'safe_entry', 'function', true, true, false, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJB::beta_entry', 'beta_entry', 'function', false, true, false, false, 'PJB')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('PJB::eval', 'eval', 'function', false, true, false, true, 'PJB')").unwrap();

    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('apps/pja/lib/input.rs', 'PJA::safe_entry', 'CONTAINS', 'PJA', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('apps/pjb/lib/unsafe.rs', 'PJB::beta_entry', 'CONTAINS', 'PJB', 0)")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('PJB::beta_entry', 'PJB::eval', 'CALLS', 'PJB', 0)",
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
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-apps/pja/lib/covered.rs', 'symbol', 'sym-apps/pja/lib/covered.rs', 'PJA', 'apps/pja/lib/covered.rs', 'hash-apps/pja/lib/covered.rs')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-apps/pjb/lib/god.rs', 'symbol', 'sym-apps/pjb/lib/god.rs', 'PJB', 'apps/pjb/lib/god.rs', 'hash-apps/pjb/lib/god.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJA::covered', 'covered', 'function', true, true, false, 'PJA')").unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::GodClass', 'GodClass', 'class', false, true, false, 'PJB')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('apps/pja/lib/covered.rs', 'PJA::covered', 'CONTAINS', 'PJA', 0)")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('apps/pjb/lib/god.rs', 'PJB::GodClass', 'CONTAINS', 'PJB', 0)")
        .unwrap();

    for i in 0..6 {
        server
            .graph_store
            .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('PJB::dep{}', 'dep{}', 'function', false, true, false, 'PJB')", i, i))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('PJB::dep{}', 'PJB::GodClass', 'CALLS', 'PJB', 0)",
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
    server.graph_store.execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/payment.rs', 'symbol', 'sym-src/payment.rs', 'AXO', 'src/payment.rs', 'hash-src/payment.rs')").unwrap();
    server.graph_store.execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/payment.rs', 'axo::checkout', 'CONTAINS', 'AXO', 0)").unwrap();
    server.graph_store.execute("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('chunk-checkout-layered', 'symbol', 'axo::checkout', 'AXO', 'body', 'checkout orchestrates payment capture', 'hash-checkout-layered', 1, 4)").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-2640', 'Requirement', 'AXO', 'Layered envelope', 'Phase A multi-resolution retrieval test fixture', 'current', '{\"priority\":\"P1\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-LAYERED', 'Requirement', 'REQ-AXO-2640', 'Symbol', 'checkout', 1.0, 0)").unwrap();

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
        intent_reqs.iter().any(|row| row["id"].as_str() == Some("REQ-AXO-2640")),
        "intent_band.requirements should contain REQ-AXO-2640, got {:?}",
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
    server.graph_store.execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/cl.rs', 'symbol', 'sym-src/cl.rs', 'AXO', 'src/cl.rs', 'hash-src/cl.rs')").unwrap();
    server.graph_store.execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/cl.rs', 'axo::cl', 'CONTAINS', 'AXO', 0)").unwrap();

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
    server.graph_store.execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/small.rs', 'symbol', 'sym-src/small.rs', 'AXO', 'src/small.rs', 'hash-src/small.rs')").unwrap();
    server.graph_store.execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/small.rs', 'axo::small', 'CONTAINS', 'AXO', 0)").unwrap();

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
    server.graph_store.execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/big.rs', 'symbol', 'sym-src/big.rs', 'AXO', 'src/big.rs', 'hash-src/big.rs')").unwrap();
    for i in 0..30 {
        server.graph_store.execute(&format!(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/big.rs', 'axo::big_{i}', 'CONTAINS', 'AXO', 0)"
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
