// Copyright (c) Didier Stadelmann. All rights reserved.

use super::*;
use serde_json::json;

/// REQ-AXO-902429 — Retirement governance:
/// soll_manager action=update to 'superseded' or 'rejected' must require
/// either data.superseded_by=<ID> or an explicit retirement rationale.

#[test]
fn test_superseded_update_without_replacement_or_rationale_is_rejected() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-001', 'Requirement', 'RET', 'Old requirement', 'Legacy', 'planned', '{}')");

    let res = server
        .axon_soll_manager(&json!({
            "action": "update",
            "entity": "requirement",
            "data": {
                "id": "REQ-RET-001",
                "status": "superseded"
            }
        }))
        .expect("must answer");

    assert_eq!(res["isError"], true, "Must be rejected when superseded_by and rationale are missing");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("superseded_by") && text.contains("retirement_reason"),
        "Error message must name both ways (superseded_by or retirement_reason), got: {text}"
    );
}

#[test]
fn test_rejected_update_without_rationale_is_rejected() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-002', 'Requirement', 'RET', 'Candidate rejected', 'Desc', 'planned', '{}')");

    let res = server
        .axon_soll_manager(&json!({
            "action": "update",
            "entity": "requirement",
            "data": {
                "id": "REQ-RET-002",
                "status": "rejected"
            }
        }))
        .expect("must answer");

    assert_eq!(res["isError"], true, "Must be rejected when retirement reason is missing");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("retirement_reason") || text.contains("rationale"),
        "Error message must demand retirement rationale, got: {text}"
    );
}

#[test]
fn test_superseded_update_with_superseded_by_establishes_edge_and_succeeds() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-003', 'Requirement', 'RET', 'Old feature', 'Legacy body', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-004', 'Requirement', 'RET', 'Living replacement', 'Modern body', 'planned', '{}')");

    let res = server
        .axon_soll_manager(&json!({
            "action": "update",
            "entity": "requirement",
            "data": {
                "id": "REQ-RET-003",
                "status": "superseded",
                "superseded_by": "REQ-RET-004"
            }
        }))
        .expect("must answer");

    assert!(res["isError"].is_null() || res["isError"] == false, "Must succeed: {res:?}");

    // Verify node status updated to superseded
    let raw = server
        .graph_store
        .query_json_writer("SELECT status FROM soll.Node WHERE id = 'REQ-RET-003'")
        .unwrap();
    assert!(raw.contains("superseded"), "Expected status superseded, got: {raw}");

    // Verify SUPERSEDES edge automatically created: REQ-RET-004 --SUPERSEDES--> REQ-RET-003
    let edge_count = server
        .graph_store
        .query_count(
            "SELECT count(*) FROM soll.Edge WHERE source_id = 'REQ-RET-004' AND target_id = 'REQ-RET-003' AND relation_type = 'SUPERSEDES'"
        )
        .unwrap();
    assert_eq!(edge_count, 1, "SUPERSEDES edge must be created from replacement to retired node");
}

#[test]
fn test_superseded_update_with_existing_incoming_edge_succeeds() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-005', 'Requirement', 'RET', 'To retire', 'Desc', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-006', 'Requirement', 'RET', 'Living successor', 'Desc', 'planned', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-RET-006', 'REQ-RET-005', 'SUPERSEDES')");

    // Now update REQ-RET-005 to superseded without supplying superseded_by (since edge already exists)
    let res = server
        .axon_soll_manager(&json!({
            "action": "update",
            "entity": "requirement",
            "data": {
                "id": "REQ-RET-005",
                "status": "superseded"
            }
        }))
        .expect("must answer");

    assert!(res["isError"].is_null() || res["isError"] == false, "Must succeed with existing incoming SUPERSEDES edge: {res:?}");
}

#[test]
fn test_soll_children_surfaces_superseded_replacement() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-007', 'Requirement', 'RET', 'Retired REQ', 'Desc', 'superseded', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-RET-008', 'Requirement', 'RET', 'Active Successor', 'Desc', 'current', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-RET-008', 'REQ-RET-007', 'SUPERSEDES')");

    let res = server
        .axon_soll_children(&json!({ "id": "REQ-RET-007" }))
        .expect("must answer");

    assert_eq!(res["data"]["status"], "ok");
    assert_eq!(res["data"]["superseded_by"], "REQ-RET-008");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("REQ-RET-008"),
        "soll_children output must explicitly name the replacement REQ-RET-008, got: {text}"
    );
}

#[test]
fn test_why_surfaces_superseded_replacement() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('DEC-RET-009', 'Decision', 'RET', 'Retired Architecture', 'Desc', 'superseded', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('DEC-RET-010', 'Decision', 'RET', 'Modern Architecture', 'Desc', 'delivered', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('DEC-RET-010', 'DEC-RET-009', 'SUPERSEDES')");

    let res = server
        .axon_why(&json!({ "symbol": "DEC-RET-009" }))
        .expect("must answer");

    assert_eq!(res["data"]["superseded_by"], "DEC-RET-010");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("DEC-RET-010"),
        "why output must explicitly name the replacement DEC-RET-010, got: {text}"
    );
}

