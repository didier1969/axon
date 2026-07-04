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

    // REQ-AXO-901952 — `why` resolves the symbol's script provenance through the
    // BKS IST snapshot; these rows were seeded via raw SQL (bypassing the
    // soll_manager/indexer invalidation). Evict the BKS process snapshots so a
    // stale BKS snapshot left warm by an earlier full-suite test (e.g. the impact
    // SOLL-architecture test) doesn't hide refund_probe. Same pattern as commit
    // 31d415fd (impact SOLL-architecture test).
    crate::ist_snapshot::evict_process_snapshot("BKS");
    server.soll_cache().invalidate("BKS");

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
    // REQ-AXO-901970 — conception flow_count is now RAM-only and derives each
    // symbol's file from the CONTAINS edge (consistent with the other RAM
    // analytics), not Chunk.file_path. In production every symbol has a CONTAINS
    // file; give axo::target one so wrapper→target is correctly same-file (not a
    // cross-file flow → flow_count stays 0).
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/lib.rs', 'axo::target', 'CONTAINS', 'AXO', 0)")
        .unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Build from project vision', 'current', '{\"goal\":\"Vision first\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Runtime truth', 'Keep runtime truthful', 'planned', '{\"priority\":\"P1\"}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Rust authoritative', 'Use Rust as the authoritative runtime', 'current', '{\"context\":\"\",\"rationale\":\"\"}')").unwrap();
    // RAM-only reads (project_status → conception/orphan) were seeded via raw SQL,
    // bypassing cache invalidation: evict so this read warms fresh.
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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
    // REQ-AXO-901926 — project_status now surfaces the REAL (RAM-first,
    // TTL-cached) anomalies summary instead of the old decoupled stub, so the
    // structural counts are no longer forced to 0/0/0.
    assert!(data["anomalies"]["summary"].is_object());
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
fn test_project_status_never_reports_coverage_unknown_when_canonical_validations_exist() {
    // REQ-AXO-901948 — project_status must not assert "unknown" coverage (nor
    // emit the `validation_coverage_unknown` proof gap) when canonical SOLL
    // holds Validation nodes, even if the IST projection hasn't scored them
    // yet. Same canonical-read contract the Vision line honours (901926).
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Axon Vision', 'Vision body', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-901948', 'Validation', 'AXO', 'Proof', 'Evidence node', 'current', '{\"method\":\"manual\"}')").unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": "AXO", "mode": "brief" }
            })),
            id: Some(json!(901948)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let proof_gaps: Vec<String> = data["truth_cockpit"]["proof_gaps"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !proof_gaps.contains(&"validation_coverage_unknown".to_string()),
        "canonical Validation nodes exist; coverage must not be flagged unknown, gaps: {proof_gaps:?}"
    );

    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(
        !text.contains("**Validation coverage:** unknown"),
        "coverage must fall back to the canonical count, not 'unknown': {text}"
    );
}

#[test]
fn test_schema_overview_exposes_ist_code_graph_tables() {
    // REQ-AXO-901956 — the IST code-graph tables must be SQL-discoverable so
    // `sql` is a usable fallback when impact/inspect/bidi_trace are hollow.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO ist.Symbol (id, name, kind, project_code) VALUES ('axo::probe', 'probe_fn', 'function', 'AXO')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "schema_overview",
                "arguments": {}
            })),
            id: Some(json!(901956)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("ist + soll"),
        "schema_overview must advertise the ist schema: {text}"
    );
    // the canonical IST code-graph tables must be listed (not just soll.*).
    // PG folds unquoted identifiers to lowercase, so match case-insensitively.
    let lower = text.to_lowercase();
    assert!(
        lower.contains("symbol"),
        "IST symbol table must be discoverable via schema_overview: {text}"
    );
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
    // REQ-AXO-901970 — project_status → conception/orphan now read the RAM
    // snapshot; raw-SQL inserts bypass invalidation, so evict before each capture
    // so the baseline (no orphan) and the second read (with the orphan inserted
    // below) each warm fresh — otherwise the orphan delta is hidden by a stale
    // snapshot warmed at the first call.
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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
    // Evict again so the second capture warms a snapshot that includes axo::orphan
    // (raw insert bypassed invalidation).
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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
    // REQ-AXO-901926 — counts are recoupled (real, not the 0/0/0 stub), so the
    // delta now genuinely tracks structural change: one orphan (axo::orphan)
    // was inserted between the two snapshots.
    assert!(
        delta["orphan_code_count_delta"].as_i64().unwrap_or(0) >= 1,
        "delta must detect the added orphan: {delta:?}"
    );

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
    // REQ-AXO-901721 (Batch D) — per-test IST isolation: snapshot/diff over a
    // unique code instead of the shared real `AXO`, whose metrics drift under
    // concurrent sibling mutations AND whose RAM staleness used to mask the very
    // change this test injects (the old delta=0 asserts only passed when the
    // snapshot didn't observe the inserted orphan — order/timing dependent).
    let code = "TST".to_string();
    let (target, wrapper, lib) = (
        format!("{code}::target"),
        format!("{code}::wrapper"),
        format!("{code}/lib.rs"),
    );
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{code}-lib', 'symbol', '{wrapper}', '{code}', '{lib}', 'hash-{lib}')",
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{target}', 'target_fn', 'function', true, true, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{wrapper}', 'wrapper_fn', 'function', false, false, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{lib}', '{wrapper}', 'CONTAINS', '{code}', 0)",
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{wrapper}', '{target}', 'CALLS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-{code}-001', 'Vision', '{code}', 'Axon Vision', 'Build from project vision', 'current', '{{}}')"))
        .unwrap();
    // REQ-AXO-901970 — project_status reads the RAM snapshot; evict before each
    // capture so the baseline and the post-orphan read each warm fresh from the
    // raw-SQL inserts (which bypass cache invalidation).
    crate::ist_snapshot::evict_process_snapshot(&code);
    server.soll_cache().invalidate(&code);

    let first = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": code, "mode": "brief" }
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
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{code}::orphan', 'orphan_fn', 'function', false, false, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{lib}', '{code}::orphan', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    crate::ist_snapshot::evict_process_snapshot(&code);
    server.soll_cache().invalidate(&code);

    let second = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_status",
                "arguments": { "project_code": code, "mode": "brief" }
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
                "arguments": { "project_code": code, "limit": 10 }
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
                    "project_code": code,
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
    // The orphan inserted between the two snapshots IS a structural orphan
    // (non-public, no callers), so the diff must observe orphan_code +1 — proof
    // the diff machinery detects a real change. The old assert expected 0, which
    // only held when shared-`AXO` RAM staleness hid the insert (the flake root).
    // No wrapper was added between snapshots, so wrapper_count is unchanged.
    assert_eq!(
        diff["data"]["metric_delta"]["orphan_code_count_delta"].as_i64(),
        Some(1)
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

    // REQ-AXO-901952 (gap B) — change_safety now reads `tested` + traceability
    // from the RAM snapshots. These rows were inserted via raw SQL (bypassing
    // soll_manager/indexer invalidation), so evict any stale process snapshot
    // first; change_safety re-warms both caches from the fresh PG rows.
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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

    // REQ-AXO-901952 (gap B) — change_safety warmed the AXO process snapshots
    // (IST + SOLL) from this test's raw-SQL fixtures. Evict them so the warm-but-
    // stale snapshot does not leak into later AXO tests whose RAM fast-paths
    // (project_status deltas, why evidence) would then read this test's symbols
    // instead of their own (the documented stale-cache test-isolation pattern).
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");
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
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(data["rejected_project_code"].as_str(), Some("ZZZ"));
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
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(data["rejected_project_code"].as_str(), Some("ZZZ"));
    assert_eq!(
        data["operator_guidance"]["follow_up_tools"][0].as_str(),
        Some("project_registry_lookup")
    );
}

#[test]
fn test_path_returns_bounded_call_path_between_symbols() {
    let server = create_test_server();
    // REQ-AXO-901721 (Batch D) — per-test IST isolation: a process-unique
    // project code + id prefixes so a sibling test reusing a hardcoded code
    // (the old `BKS`) can no longer poison this path's RAM/PG state. Root cause
    // of the order-dependent flakiness that surfaced during REQ-AXO-140.
    let code = "TST".to_string();
    let src = format!("{code}::source");
    let mid = format!("{code}::mid");
    let sink = format!("{code}::sink");
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{src}', 'source_fn', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{mid}', 'mid_fn', 'function', true, false, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sink}', 'sink_fn', 'function', true, true, false, '{code}')")).unwrap();
    // MIL-AXO-017 / REQ-AXO-216 — legacy CALLS table dropped, edges
    // now live in unified ist.Edge with relation_type='CALLS'.
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{src}', '{mid}', 'CALLS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{mid}', '{sink}', 'CALLS', '{code}', 0)"))
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
                    "project": code,
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

    // REQ-AXO-91510 / REQ-AXO-901952 — tri-modal envelope conformance
    // (GUI-AXO-1003). `path` is RAM-only : the lazy warm loads the snapshot
    // from the PG rows inserted above, so surfaces_used = "graph_ram".
    let surfaces: Vec<&str> = data["surfaces_used"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        surfaces.contains(&"graph_pg") || surfaces.contains(&"graph_ram"),
        "surfaces_used must contain graph_pg or graph_ram, got {surfaces:?}"
    );
    assert_eq!(data["total_available"].as_u64(), Some(1));
    assert_eq!(
        data["next_call_hint"].as_str(),
        Some("impact symbol=sink_fn")
    );
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
    assert_eq!(
        data["next_call_hint"].as_str(),
        Some("inspect symbol=isolated_a")
    );
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
            name: "ram_source_fn".into(),
            project_code: "RAM".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
        NodeRecord {
            id: "ram::mid".into(),
            name: "ram_mid_fn".into(),
            project_code: "RAM".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
        NodeRecord {
            id: "ram::sink".into(),
            name: "ram_sink_fn".into(),
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
        provenance.contains("IstGraph::bfs_disjoint_paths"),
        "provenance must reference RAM BFS (REQ-AXO-902019), got: {provenance}"
    );
    // REQ-AXO-902019 — a single linear route reports multiplicity 1, no detours.
    assert_eq!(data["multiplicity"]["route_count"].as_u64(), Some(1));
    assert_eq!(data["multiplicity"]["has_independent_alternates"].as_bool(), Some(false));
    assert_eq!(data["detours"].as_array().map(|a| a.len()), Some(0));
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
    // REQ-AXO-901970 — anomalies warms the RAM snapshot; evict so it warms fresh
    // from this test's raw-SQL inserts (process-global cache is shared).
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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
    // REQ-AXO-901970 — anomalies warms the RAM snapshot; evict so it warms fresh.
    crate::ist_snapshot::evict_process_snapshot("AXO");
    server.soll_cache().invalidate("AXO");

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
    // REQ-AXO-901970 — RAM-only orphan_code correctly excludes the 4 symbols
    // that DO have inbound CALLS (foreign_a/foreign_b/bridge/sink); only `source`
    // and `entry` are orphans (2/7) → alignment (7-2)/7 = 71.4. The prior 0.0
    // reflected the legacy PG path flagging all 7 (it ignored the CALLS edges).
    assert_eq!(data["summary"]["alignment_proxy_score"].as_f64(), Some(71.4));
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
fn test_soll_work_plan_default_limit_is_small_and_marks_truncated() {
    // REQ-AXO-901936 — token-economy: the DEFAULT wave listing (no explicit
    // limit) is small, with drill-down preserved via `truncated` + `limit=N`.
    // AXO carries far more than the default's worth of actionable REQs, so the
    // default must cap the listing and flag truncation.
    let server = create_test_server();
    for i in 1..=15 {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-9360{i:02}', 'Requirement', 'AXO', 'R{i}', '', 'planned', '{{\"priority\":\"P1\"}}') ON CONFLICT (id) DO NOTHING"
            ))
            .unwrap();
    }

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_work_plan",
                "arguments": { "project_code": "AXO", "format": "json" }
            })),
            id: Some(json!(901936)),
        })
        .unwrap()
        .result
        .expect("result");
    let data = response.get("data").expect("data");
    let returned = data["summary"]["returned_items"].as_u64().expect("returned");
    assert!(
        returned <= 12,
        "default wave listing must be small (<=12), got {returned}"
    );
    assert_eq!(
        data["metadata"]["truncated"].as_bool(),
        Some(true),
        "a backlog larger than the default must mark truncated for drill-down"
    );
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
    assert!(
        item_ids.contains(&"DEC-AXO-001"),
        "open accepted decision must be in waves: {:?}",
        item_ids
    );
    assert!(
        item_ids.contains(&"REQ-AXO-001"),
        "open current requirement must be in waves: {:?}",
        item_ids
    );
    // Terminal items excluded.
    assert!(
        !item_ids.contains(&"DEC-AXO-002"),
        "delivered decision must be excluded: {:?}",
        item_ids
    );
    assert!(
        !item_ids.contains(&"REQ-AXO-002"),
        "completed requirement must be excluded: {:?}",
        item_ids
    );
    assert!(
        !item_ids.contains(&"DEC-AXO-003"),
        "superseded decision must be excluded: {:?}",
        item_ids
    );
    assert!(
        !item_ids.contains(&"REQ-AXO-003"),
        "archived requirement must be excluded: {:?}",
        item_ids
    );

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
    assert!(
        unblocks_str.contains("1 descendant"),
        "expected unblocks 1 descendant, got: {}",
        unblocks_str
    );
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
    let mut score_by_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
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
    let old_score = *score_by_id
        .get("DEC-AXO-002")
        .expect("DEC-AXO-002 in waves");
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
    let mut score_by_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
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
    let old_score = *score_by_id
        .get("DEC-AXO-002")
        .expect("DEC-AXO-002 in waves");
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
fn test_status_exposes_traceability_snapshots() {
    // DEC-AXO-901631 — the predictive optimizer + its decision/reward logs were
    // retired. status(mode=full) still exposes the four observable signal
    // snapshots (host / policy / runtime-signals / recent-analytics).
    let _guard = env_lock();
    let server = create_test_server();

    let response = server
        .axon_status(&json!({"mode": "full"}))
        .expect("status response");
    let traceability = &response["data"]["traceability"];

    assert!(traceability["host_snapshot"].is_object());
    assert!(traceability["policy_snapshot"].is_object());
    assert!(traceability["runtime_signals_window"].is_object());
    assert!(traceability["recent_analytics_window"].is_object());
}

#[test]
fn test_axon_architectural_drift() {
    // REQ-AXO-91516 — architectural_drift reads the RAM IstGraphView via
    // ist_snapshot::algorithms::layer_violations (prefix-match on NODE ids),
    // NOT SQL/file-paths. To exercise it we must (a) warm a RAM snapshot whose
    // node ids start with the layer prefixes, (b) connect them with one upward
    // CALLS edge (src_layer < tgt_layer), and (c) pass the `project` arg so
    // project_for_graph is non-empty and the warm-RAM path is taken.
    use crate::ist_snapshot::snapshot::{
        EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
    };
    use std::sync::Arc;

    let _guard = env_lock();
    let server = create_test_server();

    let nodes = vec![
        NodeRecord {
            id: "ui/app.js".into(),
            name: "ui/app.js".into(),
            project_code: "PRJ".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
        NodeRecord {
            id: "db/repo.rs".into(),
            name: "db/repo.rs".into(),
            project_code: "PRJ".into(),
            kind: NodeKind::Function,
            flags: NodeFlags::default(),
        },
    ];
    let edges = vec![EdgeTriple {
        source: "ui/app.js".into(),
        target: "db/repo.rs".into(),
        rel: RelationType::Calls,
    }];
    crate::ist_snapshot::publish_process_snapshot(
        "PRJ".into(),
        Arc::new(IstGraph::build(nodes, edges)),
    );
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "architectural_drift",
            "arguments": { "source_layer": "ui", "target_layer": "db", "project": "PRJ" }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    crate::ist_snapshot::evict_process_snapshot("PRJ");

    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    println!("AUDIT_ALPHA_CONTENT={content}");

    assert!(content.contains("Architectural drift"), "{content}");
    let data = result.get("data").unwrap();
    assert_eq!(data["total_available"].as_u64(), Some(1));
    assert_eq!(data["surfaces_used"][0].as_str(), Some("graph_ram"));
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

/// REQ-AXO-901949 inv.5 — `query` graph r=1 expansion is a detail surface:
/// omitted under brief (default), included under verbose/full. Proves `mode` is
/// a real knob for normal-sized results, not a no-op until the text cap.
#[test]
fn test_axon_query_mode_gates_graph_r1_expansion() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    // Two symbols with file_path (via Chunk) + a CALLS edge caller -> callee.
    for (sid, name, file) in [
        ("prj::caller_fn", "caller_fn", "prj/caller.rs"),
        ("prj::callee_fn", "callee_fn", "prj/callee.rs"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{sid}', 'symbol', '{sid}', 'PRJ', '{file}', 'hash-{sid}')"
            ))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sid}', '{name}', 'function', false, true, false, 'PRJ')"
            ))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file}', '{sid}', 'CONTAINS', 'PRJ', 0)"
            ))
            .unwrap();
    }
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::caller_fn', 'prj::callee_fn', 'CALLS', 'PRJ', 0)")
        .unwrap();
    // REQ-AXO-901970 — query_graph_r1_neighbors now expands via the RAM IST
    // snapshot. These rows were seeded with raw SQL (bypassing cache
    // invalidation), so evict any stale PRJ process snapshot; the verbose query
    // re-warms it fresh and the CALLS neighbour callee_fn resolves in RAM.
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let run = |mode: Option<&str>| {
        let mut arguments = json!({ "query": "caller_fn", "project": "PRJ" });
        if let Some(m) = mode {
            arguments["mode"] = json!(m);
        }
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "query", "arguments": arguments })),
            id: Some(json!(7)),
        };
        server.handle_request(req).unwrap().result.expect("result")
    };

    // Brief (default): the expansion is empty — neighbour not computed.
    let brief = run(None);
    let brief_neighbours = brief["data"]["context"]["related_symbols_via_graph"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        brief_neighbours.is_empty(),
        "brief must omit graph r=1 expansion, got {brief_neighbours:?}"
    );

    // Verbose: the CALLS neighbour `callee_fn` surfaces in the expansion.
    let verbose = run(Some("verbose"));
    let verbose_neighbours: Vec<String> = verbose["data"]["context"]
        ["related_symbols_via_graph"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        verbose_neighbours.iter().any(|n| n == "callee_fn"),
        "verbose must include the CALLS neighbour callee_fn, got {verbose_neighbours:?}"
    );

    // The `full` alias behaves like verbose (LLM-natural opt-in token).
    let full = run(Some("full"));
    let full_neighbours: Vec<String> = full["data"]["context"]["related_symbols_via_graph"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        full_neighbours.iter().any(|n| n == "callee_fn"),
        "`full` alias must behave like verbose, got {full_neighbours:?}"
    );
}

// REQ-AXO-901970 — direct unit coverage of the RAM parity decisions in
// query_graph_r1_neighbors: reverse CALLS callers surface, file endpoints
// (CONTAINS sources, no `::`) are excluded, and the anchor name itself is
// filtered out. Hand-built snapshot, no PG.
#[test]
fn query_graph_r1_neighbors_ram_excludes_files_and_anchor() {
    use crate::ist_snapshot::snapshot::{
        EdgeTriple, IstGraph, NodeFlags, NodeKind, NodeRecord, RelationType,
    };
    use crate::ist_snapshot::{evict_process_snapshot, publish_process_snapshot};
    use std::collections::HashSet;

    let code = "TR1";
    let target = "TR1::f.rs::target".to_string();
    let caller = "TR1::f.rs::caller".to_string();
    let file = "f.rs".to_string();

    let mk = |id: &str| NodeRecord {
        id: id.to_string(),
        name: id.rsplit("::").next().unwrap_or(id).to_string(),
        project_code: code.to_string(),
        kind: NodeKind::Function,
        flags: NodeFlags::default(),
    };
    let nodes = vec![mk(&target), mk(&caller)];
    let edges = vec![
        // caller CALLS target → reverse from target yields caller.
        EdgeTriple {
            source: caller.clone(),
            target: target.clone(),
            rel: RelationType::Calls,
        },
        // file CONTAINS target → reverse from target yields the file (must be dropped).
        EdgeTriple {
            source: file.clone(),
            target: target.clone(),
            rel: RelationType::Contains,
        },
    ];
    evict_process_snapshot(code);
    publish_process_snapshot(code.to_string(), std::sync::Arc::new(IstGraph::build(nodes, edges)));

    let server = create_test_server();
    let direct: HashSet<String> = ["target".to_string()].into_iter().collect();
    let result = server.query_graph_r1_neighbors(&direct, code, 10);

    let names: Vec<&str> = result
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert!(names.contains(&"caller"), "reverse CALLS caller must surface: {names:?}");
    assert!(!names.contains(&"target"), "anchor name must be excluded: {names:?}");
    assert!(
        !names.iter().any(|n| n.contains("f.rs")),
        "file endpoint must be excluded: {names:?}"
    );
    assert_eq!(
        result[0]["kind"].as_str(),
        Some("function"),
        "kind resolves from RAM NodeKind"
    );

    evict_process_snapshot(code);
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

    // REQ-AXO-901952 — structural neighbours are RAM-only now. Evict any
    // cached AXO snapshot so the warm path reloads the rows inserted above
    // (in production the indexer refreshes the snapshot on change ; a direct
    // PG insert in a test bypasses that, leaving a stale cache otherwise).
    crate::ist_snapshot::evict_process_snapshot("AXO");

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
                SymbolFixture::new("prj::core_func", "core_func", "function", "PRJ").tested(true),
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
    assert!(
        suggestions.is_empty(),
        "preconditions: no suggestions: {suggestions:?}"
    );
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
    assert!(
        suggestions.is_empty(),
        "preconditions: no suggestions: {suggestions:?}"
    );
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
    assert!(
        suggestions.is_empty(),
        "preconditions: no suggestions for nonsense symbol"
    );

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
        remediation_text.contains("query")
            || remediation_text.contains("broaden")
            || remediation_text.contains("spelling"),
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
    // REQ-AXO-901860 — audit/health readiness gate (file_count_for_project ->
    // ist.project_telemetry.files_total) counts ENROLLED files via
    // axon.Project x ist.IndexedFile. Seed the parent Project + IndexedFile
    // rows (path = CONTAINS-edge source_id) so the project is indexed.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PRJ') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('src/api.rs', 'PRJ', 'hash-src/api.rs', 0), ('src/api_dummy.rs', 'PRJ', 'hash-src/api_dummy.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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

    // REQ-AXO-901970 — taint/security audit reads the RAM snapshot; evict so it
    // warms fresh from this test's raw-SQL inserts, and scope to the fixture's
    // project (security_audit is RAM-per-project, no cross-project "*").
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PRJ"
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
    // REQ-AXO-901860 — enroll the project so audit isn't gated as unindexed;
    // IndexedFile.path must match the CONTAINS-edge source_id for
    // get_technical_debt's IndexedFile JOIN.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PRJ') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('src/danger.rs', 'PRJ', 'hash-src/danger.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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

    // REQ-AXO-901970 — technical_debt audit reads the RAM snapshot; evict so it
    // warms fresh from this test's raw-SQL inserts, and scope to the fixture's
    // project (technical_debt is RAM-per-project, no cross-project "*").
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PRJ"
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
    // REQ-AXO-901860 — enroll project + IndexedFile (path = CONTAINS source)
    // so audit isn't gated and the tech-debt IndexedFile JOIN matches.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PRJ') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('src/todo.rs', 'PRJ', 'hash-src/todo.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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

    // REQ-AXO-901970 — technical_debt audit reads the RAM snapshot; evict so it
    // warms fresh from this test's raw-SQL inserts, and scope to the fixture's
    // project (technical_debt is RAM-per-project, no cross-project "*").
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PRJ"
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
    // REQ-AXO-901860 — enroll project + IndexedFile (path = CONTAINS source)
    // so audit isn't gated and the secrets tech-debt JOIN matches.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PRJ') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('src/config.rs', 'PRJ', 'hash-src/config.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/config.rs', 'symbol', 'sym-src/config.rs', 'PRJ', 'src/config.rs', 'hash-src/config.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'PRJ')").unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/config.rs', 'prj::secret1', 'CONTAINS', 'PRJ', 0)")
        .unwrap();

    // REQ-AXO-901970 — secrets audit reads the RAM snapshot; evict so it warms
    // fresh from this test's raw-SQL inserts, and scope to the fixture's project
    // (technical_debt/secrets is RAM-per-project, no cross-project "*").
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": "PRJ"
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
    // REQ-AXO-901721 (Batch D) — per-test IST isolation: unique code + id/path
    // prefixes, and scope the audit to that code (was `project:"*"`, which
    // scanned the whole shared live PG and diluted/masked the taint chain so the
    // assert was non-deterministic). REQ-AXO-901860 — enroll project +
    // IndexedFile so audit isn't gated as unindexed.
    let code = "TST".to_string();
    let (ef, rn, ub) = (
        format!("{code}::elixir_func"),
        format!("{code}::rust_nif"),
        format!("{code}::unsafe_block"),
    );
    let (api, dummy) = (format!("{code}/api.ex"), format!("{code}/api_dummy.ex"));
    server
        .graph_store
        .execute(&format!("INSERT INTO axon.Project (code) VALUES ('{code}') ON CONFLICT (code) DO NOTHING"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('{api}', '{code}', 'hash-{api}', 0), ('{dummy}', '{code}', 'hash-{dummy}', 0) ON CONFLICT (path) DO NOTHING")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{code}-api', 'symbol', '{ef}', '{code}', '{api}', 'hash-{api}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{code}-dummy', 'symbol', '{rn}', '{code}', '{dummy}', 'hash-{dummy}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('{ef}', 'elixir_func', 'function', false, true, false, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('{rn}', 'rust_nif', 'function', false, true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code) VALUES ('{ub}', 'unsafe_block', 'function', false, true, false, true, '{code}')")).unwrap();

    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{api}', '{ef}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{ef}', '{rn}', 'CALLS_NIF', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{rn}', '{ub}', 'CALLS', '{code}', 0)"))
        .unwrap();

    // REQ-AXO-901970 — cross-language taint audit reads the RAM snapshot; evict
    // so it warms fresh from this test's raw-SQL inserts (scoped unique code).
    crate::ist_snapshot::evict_process_snapshot(&code);
    server.soll_cache().invalidate(&code);

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "audit",
            "arguments": {
                "project": code
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
    // REQ-AXO-901860 — enroll project + IndexedFile so health isn't gated as
    // unindexed; god-objects then surface via get_god_objects (project=*).
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PRJ') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('src/god.rs', 'PRJ', 'hash-src/god.rs', 0), ('src/god_dummy.rs', 'PRJ', 'hash-src/god_dummy.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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

    // REQ-AXO-901924 — a god object has high fan-OUT (it orchestrates many
    // collaborators). GodClass CALLS 20 distinct deps. (Fan-IN — being called
    // by many — is a popular hub, NOT a god object: now flagged as such would
    // be a false positive, see god_objects_does_not_flag_high_fan_in_utility.)
    for i in 0..20 {
        server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('prj::dep{}', 'dep{}', 'function', false, true, false, 'PRJ')", i, i)).unwrap();
        server
            .graph_store
            .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('prj::GodClass', 'prj::dep{}', 'CALLS', 'PRJ', 0)", i))
            .unwrap();
    }
    // REQ-AXO-901970 — health god_objects reads the RAM snapshot; evict so it
    // warms fresh from this test's raw-SQL inserts (fan-OUT god object).
    crate::ist_snapshot::evict_process_snapshot("PRJ");
    server.soll_cache().invalidate("PRJ");

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "health",
            // REQ-AXO-901970 — scope to the fixture's project: god_objects is
            // RAM-per-project (no cross-project "*" aggregation; that would need
            // every project's snapshot warm). The PRJ GodClass (CALLS fan-out 20)
            // is flagged once the PRJ snapshot is warm.
            "arguments": {
                "project": "PRJ"
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
fn test_axon_batch_routes_all_tools_not_just_three() {
    // REQ-AXO-901925 — batch must route EVERY tool through the canonical
    // dispatcher and return one result per call. Previously only
    // query/inspect/impact were handled; status/embedding_status fell to the
    // `_ => None` arm and the whole batch silently returned `[]`.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "batch",
            "arguments": {
                "calls": [
                    {"tool": "status", "args": {"mode": "brief"}},
                    {"tool": "embedding_status", "args": {"project": "AXO"}}
                ]
            }
        })),
        id: Some(json!(42)),
    };
    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let text = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let parsed: Vec<Value> =
        serde_json::from_str(text).expect("batch must return a JSON array of results");
    assert_eq!(
        parsed.len(),
        2,
        "one result per call — was [] before REQ-AXO-901925"
    );
    assert_eq!(
        parsed[0].get("name").and_then(|v| v.as_str()),
        Some("status")
    );
    assert_eq!(
        parsed[1].get("name").and_then(|v| v.as_str()),
        Some("embedding_status")
    );
    assert!(
        parsed[0].get("result").is_some(),
        "non-query tool result must be present, not dropped"
    );
}

#[test]
fn test_axon_audit_respects_project_scope() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    // REQ-AXO-901860 — enroll both projects + their IndexedFile rows so the
    // scoped audit (project=PJA) isn't gated as unindexed.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PJA') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PJB') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('apps/pja/lib/input.rs', 'PJA', 'hash-apps/pja/lib/input.rs', 0), ('apps/pjb/lib/unsafe.rs', 'PJB', 'hash-apps/pjb/lib/unsafe.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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
    // REQ-AXO-901860 — enroll both projects + their IndexedFile rows so the
    // scoped health (project=PJA) isn't gated as unindexed.
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PJA') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO axon.Project (code) VALUES ('PJB') ON CONFLICT (code) DO NOTHING")
        .unwrap();
    server.graph_store.execute("INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) VALUES ('apps/pja/lib/covered.rs', 'PJA', 'hash-apps/pja/lib/covered.rs', 0), ('apps/pjb/lib/god.rs', 'PJB', 'hash-apps/pjb/lib/god.rs', 0) ON CONFLICT (path) DO NOTHING").unwrap();
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
    let recent_status = response["data"]["recent_band"]["status"]
        .as_str()
        .unwrap_or("");
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
        intent_reqs
            .iter()
            .any(|row| row["id"].as_str() == Some("REQ-AXO-2640")),
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

    assert!(
        response["data"]["packet"].is_object(),
        "legacy retrieve_context must still expose `data.packet`"
    );
    assert!(
        response["data"]["intent_band"].is_null(),
        "legacy retrieve_context must NOT expose intent_band"
    );
    assert!(
        response["data"]["code_band"].is_null(),
        "legacy retrieve_context must NOT expose code_band"
    );
    assert!(
        response["data"]["recent_band"].is_null(),
        "legacy retrieve_context must NOT expose recent_band"
    );
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
        let status = cmd
            .status()
            .unwrap_or_else(|err| panic!("command failed to start: {err}"));
        assert!(status.success(), "command failed: {:?}", cmd);
    };
    must_run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("init")
            .arg("-q"),
    );
    must_run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("config")
            .arg("user.email")
            .arg("test@axon.local"),
    );
    must_run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("config")
            .arg("user.name")
            .arg("Axon Test"),
    );
    std::fs::write(repo.join("alpha.rs"), b"// alpha\n").unwrap();
    std::fs::write(repo.join("beta.rs"), b"// beta\n").unwrap();
    must_run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("add")
            .arg("alpha.rs")
            .arg("beta.rs"),
    );
    must_run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("commit")
            .arg("-q")
            .arg("-m")
            .arg("layered-test seed commit"),
    );

    let band = McpServer::collect_recent_band(Some(repo.to_string_lossy().as_ref()));
    assert_eq!(band["status"].as_str(), Some("ok"), "band: {band}");
    let edits = band["git_recent_edits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let files: Vec<&str> = edits.iter().filter_map(|e| e["file"].as_str()).collect();
    assert!(
        files.contains(&"alpha.rs"),
        "alpha.rs missing from {files:?}"
    );
    assert!(files.contains(&"beta.rs"), "beta.rs missing from {files:?}");
    let first = &edits[0];
    assert!(first["last_commit_subject"]
        .as_str()
        .unwrap_or("")
        .contains("layered-test seed commit"));
    assert!(band["tokens_used"].as_u64().unwrap_or(0) > 0);
}

// REQ-AXO-264 A6 v1 — recent_band returns a stable structured response
// when the project root is missing or invalid, instead of crashing or
// returning a bare error.
#[test]
fn test_recent_band_returns_stable_contract_when_no_project_root() {
    let none_band = McpServer::collect_recent_band(None);
    assert_eq!(none_band["status"].as_str(), Some("no_project_root"));
    assert!(none_band["git_recent_edits"]
        .as_array()
        .map_or(false, |a| a.is_empty()));
    assert_eq!(none_band["tokens_used"].as_u64(), Some(0));

    let bogus_band =
        McpServer::collect_recent_band(Some("/nonexistent/axon/test/path/does-not-exist"));
    assert_eq!(bogus_band["status"].as_str(), Some("no_project_root"));
    assert!(bogus_band["git_recent_edits"]
        .as_array()
        .map_or(false, |a| a.is_empty()));
}

// REQ-AXO-264 A6 v1 — non-git directory must return git_error contract,
// not panic, not ok-with-empty.
#[test]
fn test_recent_band_returns_git_error_when_not_a_repo() {
    let tempdir = tempdir().unwrap();
    let band = McpServer::collect_recent_band(Some(tempdir.path().to_string_lossy().as_ref()));
    assert_eq!(band["status"].as_str(), Some("git_error"), "band: {band}");
    assert!(band["git_recent_edits"]
        .as_array()
        .map_or(false, |a| a.is_empty()));
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

    assert_eq!(
        response["data"]["intent_band"]["tokens_budget"].as_u64(),
        Some(2000)
    );
    assert_eq!(
        response["data"]["code_band"]["tokens_budget"].as_u64(),
        Some(6000)
    );
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

    let kept = response["data"]["code_band"]["chunks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let used = response["data"]["code_band"]["tokens_used"]
        .as_u64()
        .unwrap_or(0);
    let budget = response["data"]["code_band"]["tokens_budget"]
        .as_u64()
        .unwrap_or(0);
    let overflowed = response["data"]["code_band"]["tokens_overflowed"]
        .as_u64()
        .unwrap_or(0);

    assert_eq!(budget, 200, "explicit budget should be surfaced");
    assert!(
        used <= budget,
        "tokens_used ({used}) must be <= budget ({budget})"
    );
    assert!(
        overflowed > 0 || kept.is_empty(),
        "with a 200-token budget on 30 fat chunks we expect either truncation overflow > 0 or empty kept set; got kept={} overflowed={}",
        kept.len(), overflowed
    );
}

// REQ-AXO-902187 — closed-loop wiring test: structural_health_index persists a Δ
// snapshot per call and re-surfaces below-target axes that did not improve. First
// call on a fresh project must report no delta (nothing to compare against yet);
// the second call over UNCHANGED data must report a zero aggregate delta and flag
// every still-below-target axis as re_surfaced (the anti-Goodhart verdict: an axis
// that failed to improve resurfaces instead of silently dropping off the radar).
#[test]
fn test_structural_health_index_persists_delta_and_re_surfaces_stagnant_axes() {
    let _guard = env_lock();
    let history_dir = tempdir().unwrap();
    std::env::set_var(
        "AXON_STRUCTURAL_HISTORY_DIR",
        history_dir.path().to_string_lossy().to_string(),
    );
    let server = create_test_server();
    let code = "TST".to_string();
    let (target, wrapper, lib) = (
        format!("{code}::target"),
        format!("{code}::wrapper"),
        format!("{code}/lib.rs"),
    );
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{code}-lib', 'symbol', '{wrapper}', '{code}', '{lib}', 'hash-{lib}')",
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{target}', 'target_fn', 'function', false, true, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{wrapper}', 'wrapper_fn', 'function', false, false, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{lib}', '{wrapper}', 'CONTAINS', '{code}', 0)",
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{wrapper}', '{target}', 'CALLS', '{code}', 0)"))
        .unwrap();
    assert!(server.ensure_ram_snapshot_warm(&code));

    let call = |id: i64| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "structural_health_index",
                    "arguments": { "project_code": code }
                })),
                id: Some(json!(id)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let first = call(90218701);
    assert_eq!(first["data"]["delta_vs_previous"], Value::Null, "no prior snapshot yet: {first:?}");

    let second = call(90218702);
    let delta = &second["data"]["delta_vs_previous"];
    assert!(!delta.is_null(), "second call must diff against the first snapshot: {second:?}");
    assert_eq!(
        delta["aggregate_delta"].as_f64().unwrap_or(f64::NAN),
        0.0,
        "unchanged snapshot data must yield a zero aggregate delta: {delta:?}"
    );
    let below = second["data"]["below_target"].as_array().cloned().unwrap_or_default();
    assert!(!below.is_empty(), "weighted_coverage (0 covered nodes) must be below its 0.80 target: {second:?}");
    assert!(
        below.iter().all(|b| b["re_surfaced"].as_bool() == Some(true)),
        "every below-target axis stagnated (delta=0) between the two calls, all must re-surface: {below:?}"
    );

    std::env::remove_var("AXON_STRUCTURAL_HISTORY_DIR");
}

// REQ-AXO-902186 slice 2 — the worklist must surface candidates from ALL FOUR categories
// (coverage/coupling/resilience/acyclicity) when the graph has an instance of each, and
// rank them by TRUE ROI (expected ΔSHI ÷ blast-radius), not raw severity or centrality.
#[test]
fn test_structural_health_worklist_ranks_all_categories_by_roi() {
    let _guard = env_lock();
    let history_dir = tempdir().unwrap();
    std::env::set_var(
        "AXON_STRUCTURAL_HISTORY_DIR",
        history_dir.path().to_string_lossy().to_string(),
    );
    let server = create_test_server();
    let code = "TST".to_string();
    // REQ-AXO-902193 — `is_testable_symbol` requires a REAL file component (a `::`-segment
    // ending `.rs`) in the id, else the symbol is treated as an external call-target and
    // excluded from the coverage/pagerank candidate scan. Every symbol id below embeds
    // `src/lib.rs` for exactly this reason (a bare `{code}::name` id, as used by OTHER
    // tests that only exercise anomaly counts, would silently vanish from this worklist).
    let module = format!("{code}::src/lib.rs");

    // Coverage: an untested hub with a caller.
    let (target, wrapper) = (format!("{module}::target"), format!("{module}::wrapper"));
    server.graph_store.execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-{code}-lib', 'symbol', '{wrapper}', '{code}', 'src/lib.rs', 'hash-{code}-lib')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{target}', 'target_fn', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{wrapper}', 'wrapper_fn', 'function', false, false, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{module}', '{wrapper}', 'CONTAINS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{wrapper}', '{target}', 'CALLS', '{code}', 0)")).unwrap();

    // Acyclicity: a mutual-call cycle cyc_a <-> cyc_b.
    let (cyc_a, cyc_b) = (format!("{module}::cyc_a"), format!("{module}::cyc_b"));
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{cyc_a}', 'cyc_a', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{cyc_b}', 'cyc_b', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{cyc_a}', '{cyc_b}', 'CALLS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{cyc_b}', '{cyc_a}', 'CALLS', '{code}', 0)")).unwrap();

    // Resilience: a path art_a - art_b - art_c (undirected articulation at art_b).
    let (art_a, art_b, art_c) = (
        format!("{module}::art_a"),
        format!("{module}::art_b"),
        format!("{module}::art_c"),
    );
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{art_a}', 'art_a', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{art_b}', 'art_b', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{art_c}', 'art_c', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{art_a}', '{art_b}', 'CALLS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{art_b}', '{art_c}', 'CALLS', '{code}', 0)")).unwrap();

    assert!(server.ensure_ram_snapshot_warm(&code));

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "structural_health_worklist",
                "arguments": { "project_code": code, "top": 50 }
            })),
            id: Some(json!(90218703)),
        })
        .unwrap()
        .result
        .unwrap();

    let worklist = response["data"]["worklist"].as_array().cloned().unwrap_or_default();
    assert!(!worklist.is_empty(), "worklist must not be empty: {response:?}");

    let categories: std::collections::HashSet<&str> =
        worklist.iter().filter_map(|c| c["category"].as_str()).collect();
    assert!(categories.contains("coverage"), "expected a coverage candidate: {worklist:?}");
    assert!(categories.contains("acyclicity"), "expected an acyclicity candidate (cyc_a<->cyc_b): {worklist:?}");
    assert!(categories.contains("resilience"), "expected a resilience candidate (art_b is an articulation point): {worklist:?}");

    for c in &worklist {
        assert!(c["blast_radius"].as_u64().unwrap_or(0) >= 1, "blast_radius must be >= 1: {c:?}");
        let roi = c["roi"].as_f64().unwrap();
        let delta = c["expected_delta_shi"].as_f64().unwrap();
        let blast = c["blast_radius"].as_u64().unwrap() as f64;
        assert!((roi - delta / blast).abs() < 1e-9, "roi must equal expected_delta_shi/blast_radius: {c:?}");
    }
    // Ranked strictly non-increasing by ROI.
    for pair in worklist.windows(2) {
        let a = pair[0]["roi"].as_f64().unwrap();
        let b = pair[1]["roi"].as_f64().unwrap();
        assert!(a >= b, "worklist must be ranked by descending ROI: {a} then {b} in {worklist:?}");
    }

    std::env::remove_var("AXON_STRUCTURAL_HISTORY_DIR");
}
