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

/// REQ-AXO-902579 — SUPERSEDES multi-successors fan-out support:
/// 1. Deux sources distinctes peuvent superseder le meme noeud retire.
/// 2. Une meme source ne peut pas superseder deux fois le meme noeud.
/// 3. La chaine de revisions reste gardee : le message d'erreur actuel subsiste pour ce cas.
/// 4. Un test epingle les trois.
#[test]
fn test_req_902579_supersedes_multi_sources_fanout_and_same_source_rejection() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('PIL-FAN-001', 'Pillar', 'FAN', 'Original Pillar', 'Pillar to be split', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('PIL-FAN-002', 'Pillar', 'FAN', 'Split Part A', 'Part A', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('PIL-FAN-003', 'Pillar', 'FAN', 'Split Part B', 'Part B', 'current', '{}')");

    // 1. Première source supersède PIL-FAN-001
    let link_res1 = server
        .axon_soll_manager(&json!({
            "action": "link",
            "entity": "pillar",
            "data": {
                "source_id": "PIL-FAN-002",
                "target_id": "PIL-FAN-001",
                "relation_type": "SUPERSEDES"
            }
        }))
        .expect("must answer");
    assert!(link_res1["isError"].is_null() || link_res1["isError"] == false, "First SUPERSEDES must succeed: {link_res1:?}");

    // Vérifier que PIL-FAN-001 est désormais superseded
    let status_target: String = server
        .graph_store
        .query_json("SELECT status FROM soll.Node WHERE id = 'PIL-FAN-001'")
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&raw).ok())
        .and_then(|rows| rows.first().and_then(|r| r.first()).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default();
    assert_eq!(status_target, "superseded");

    // Critère 1: Deuxième source distincte PIL-FAN-003 supersède AUSSI PIL-FAN-001 (éclatement multi-successeurs)
    let link_res2 = server
        .axon_soll_manager(&json!({
            "action": "link",
            "entity": "pillar",
            "data": {
                "source_id": "PIL-FAN-003",
                "target_id": "PIL-FAN-001",
                "relation_type": "SUPERSEDES"
            }
        }))
        .expect("must answer");
    assert!(
        link_res2["isError"].is_null() || link_res2["isError"] == false,
        "Critère 1: Distinct source PIL-FAN-003 must be allowed to supersede the same retired node PIL-FAN-001: {link_res2:?}"
    );

    // Vérifier que 2 arêtes SUPERSEDES pointent vers PIL-FAN-001
    let incoming_count = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.Edge WHERE target_id = 'PIL-FAN-001' AND relation_type = 'SUPERSEDES'")
        .unwrap();
    assert_eq!(incoming_count, 2, "Both SUPERSEDES edges must exist in soll.Edge");

    // Critère 2 & 3: La MÊME source PIL-FAN-002 tente à nouveau de superséder PIL-FAN-001 -> DOIT ÊTRE REJETÉE
    let link_dup = server
        .axon_soll_manager(&json!({
            "action": "link",
            "entity": "pillar",
            "data": {
                "source_id": "PIL-FAN-002",
                "target_id": "PIL-FAN-001",
                "relation_type": "SUPERSEDES"
            }
        }))
        .expect("must answer");
    assert_eq!(link_dup["isError"], true, "Critère 2: Same source PIL-FAN-002 cannot supersede PIL-FAN-001 twice");
    let err_msg = link_dup["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        err_msg.contains("SUPERSEDES target `PIL-FAN-001` is already retired (status=superseded). It was replaced by `PIL-FAN-002`"),
        "Critère 3: Error message must name that it was replaced by PIL-FAN-002: {err_msg}"
    );

    // Lecture du graphe (soll_children) : rend les N remplaçants
    let children_res = server
        .axon_soll_children(&json!({ "id": "PIL-FAN-001" }))
        .expect("must answer");
    assert_eq!(children_res["data"]["status"], "ok");
    assert!(children_res["data"]["superseded_by_all"].is_array(), "superseded_by_all must be an array");
    let all_reps = children_res["data"]["superseded_by_all"].as_array().unwrap();
    assert_eq!(all_reps.len(), 2);
    let children_text = children_res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(children_text.contains("PIL-FAN-002") && children_text.contains("PIL-FAN-003"), "soll_children must mention both replacements: {children_text}");

    // Lecture du graphe (why) : rend les N remplaçants
    let why_res = server
        .axon_why(&json!({ "symbol": "PIL-FAN-001" }))
        .expect("must answer");
    let why_text = why_res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(why_text.contains("PIL-FAN-002") && why_text.contains("PIL-FAN-003"), "why must mention both replacements: {why_text}");
}

