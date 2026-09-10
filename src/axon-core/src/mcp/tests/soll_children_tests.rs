// Copyright (c) Didier Stadelmann. All rights reserved.

use super::*;
use serde_json::Value;

fn extract_node_ids(res: &Value) -> Vec<String> {
    res["data"]["nodes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// REQ-AXO-902642 — TARGETS is parent→child (source_id = MIL, target_id = REQ).
/// Semantic traversal `children` on MIL MUST return the targeted REQ.
/// Semantic traversal `parents` on REQ MUST return the targeting MIL.
#[test]
fn test_targets_semantic_traversal_in_both_directions() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('MIL-OPV-084', 'Milestone', 'OPV', 'Sprint Alpha', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-OPV-994', 'Requirement', 'OPV', 'Core Feature', 'desc', 'planned', '{}')",
    );
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('MIL-OPV-084', 'REQ-OPV-994', 'TARGETS')",
    );

    // 1. Semantic children on MIL (default direction) -> returns REQ-OPV-994
    let mil_children = server
        .axon_soll_children(&json!({ "id": "MIL-OPV-084" }))
        .expect("must answer");
    assert_eq!(mil_children["data"]["status"], "ok");
    assert_eq!(mil_children["data"]["count"], 1);
    let ids = extract_node_ids(&mil_children);
    assert_eq!(ids, vec!["REQ-OPV-994".to_string()]);

    // 2. Semantic children on MIL with explicit relation_type="TARGETS"
    let mil_children_rel = server
        .axon_soll_children(&json!({ "id": "MIL-OPV-084", "relation_type": "TARGETS" }))
        .expect("must answer");
    assert_eq!(mil_children_rel["data"]["count"], 1);
    assert_eq!(
        extract_node_ids(&mil_children_rel),
        vec!["REQ-OPV-994".to_string()]
    );

    // 3. Semantic parents on MIL -> 0, with hint towards `children`
    let mil_parents = server
        .axon_soll_children(&json!({ "id": "MIL-OPV-084", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(mil_parents["data"]["count"], 0);
    let mil_parents_text = mil_parents["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        mil_parents_text.contains("direction=\"children\"")
            || mil_parents_text.contains("direction=\\\"children\\\""),
        "mil parents 0 must point to children, got: {mil_parents_text}"
    );

    // 4. Semantic parents on REQ -> returns MIL-OPV-084
    let req_parents = server
        .axon_soll_children(&json!({ "id": "REQ-OPV-994", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(req_parents["data"]["count"], 1);
    assert_eq!(
        extract_node_ids(&req_parents),
        vec!["MIL-OPV-084".to_string()]
    );

    // 5. Semantic parents on REQ with relation_type="TARGETS"
    let req_parents_rel = server
        .axon_soll_children(
            &json!({ "id": "REQ-OPV-994", "direction": "parents", "relation_type": "TARGETS" }),
        )
        .expect("must answer");
    assert_eq!(req_parents_rel["data"]["count"], 1);
    assert_eq!(
        extract_node_ids(&req_parents_rel),
        vec!["MIL-OPV-084".to_string()]
    );

    // 6. Semantic children on REQ -> 0, with hint towards `parents`
    let req_children = server
        .axon_soll_children(&json!({ "id": "REQ-OPV-994", "direction": "children" }))
        .expect("must answer");
    assert_eq!(req_children["data"]["count"], 0);
    let req_children_text = req_children["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        req_children_text.contains("direction=\"parents\"")
            || req_children_text.contains("direction=\\\"parents\\\""),
        "req children 0 must point to parents, got: {req_children_text}"
    );
}

/// REQ-AXO-902642 — Physical directions: `incoming` (target_id = id) and `outgoing` (source_id = id).
#[test]
fn test_physical_directions_incoming_and_outgoing() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('MIL-PHY-001', 'Milestone', 'PHY', 'M1', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-PHY-001', 'Requirement', 'PHY', 'R1', 'desc', 'planned', '{}')",
    );
    // Edge is physical outgoing for MIL-PHY-001, physical incoming for REQ-PHY-001
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('MIL-PHY-001', 'REQ-PHY-001', 'TARGETS')",
    );

    // MIL outgoing -> REQ
    let mil_out = server
        .axon_soll_children(&json!({ "id": "MIL-PHY-001", "direction": "outgoing" }))
        .expect("must answer");
    assert_eq!(extract_node_ids(&mil_out), vec!["REQ-PHY-001".to_string()]);

    // MIL incoming -> 0
    let mil_in = server
        .axon_soll_children(&json!({ "id": "MIL-PHY-001", "direction": "incoming" }))
        .expect("must answer");
    assert_eq!(mil_in["data"]["count"], 0);

    // REQ incoming -> MIL
    let req_in = server
        .axon_soll_children(&json!({ "id": "REQ-PHY-001", "direction": "incoming" }))
        .expect("must answer");
    assert_eq!(extract_node_ids(&req_in), vec!["MIL-PHY-001".to_string()]);

    // REQ outgoing -> 0
    let req_out = server
        .axon_soll_children(&json!({ "id": "REQ-PHY-001", "direction": "outgoing" }))
        .expect("must answer");
    assert_eq!(req_out["data"]["count"], 0);
}

/// REQ-AXO-902642 — SOLVES is parent→child (source_id = DEC, target_id = REQ).
#[test]
fn test_solves_semantic_traversal() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('DEC-SLV-001', 'Decision', 'SLV', 'D1', 'desc', 'accepted', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-SLV-001', 'Requirement', 'SLV', 'R1', 'desc', 'planned', '{}')",
    );
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('DEC-SLV-001', 'REQ-SLV-001', 'SOLVES')",
    );

    // DEC children -> REQ
    let dec_children = server
        .axon_soll_children(&json!({ "id": "DEC-SLV-001", "direction": "children" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&dec_children),
        vec!["REQ-SLV-001".to_string()]
    );

    // REQ parents -> DEC
    let req_parents = server
        .axon_soll_children(&json!({ "id": "REQ-SLV-001", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&req_parents),
        vec!["DEC-SLV-001".to_string()]
    );
}

/// REQ-AXO-902642 — BELONGS_TO, REFINES, EPITOMIZES are child→parent (source_id = child, target_id = parent).
#[test]
fn test_child_to_parent_relations_traversal() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('VIS-TRV-001', 'Vision', 'TRV', 'Vision 1', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('PIL-TRV-001', 'Pillar', 'TRV', 'Pillar 1', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-TRV-001', 'Requirement', 'TRV', 'Req Root', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-TRV-002', 'Requirement', 'TRV', 'Req Child', 'desc', 'current', '{}')",
    );

    // PIL -[EPITOMIZES]-> VIS
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('PIL-TRV-001', 'VIS-TRV-001', 'EPITOMIZES')",
    );
    // REQ -[BELONGS_TO]-> PIL
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-TRV-001', 'PIL-TRV-001', 'BELONGS_TO')",
    );
    // REQ-2 -[REFINES]-> REQ-1
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-TRV-002', 'REQ-TRV-001', 'REFINES')",
    );

    // VIS children -> PIL
    let vis_children = server
        .axon_soll_children(&json!({ "id": "VIS-TRV-001" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&vis_children),
        vec!["PIL-TRV-001".to_string()]
    );

    // PIL parents -> VIS
    let pil_parents = server
        .axon_soll_children(&json!({ "id": "PIL-TRV-001", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&pil_parents),
        vec!["VIS-TRV-001".to_string()]
    );

    // PIL children -> REQ-TRV-001
    let pil_children = server
        .axon_soll_children(&json!({ "id": "PIL-TRV-001" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&pil_children),
        vec!["REQ-TRV-001".to_string()]
    );

    // REQ-001 children -> REQ-TRV-002
    let req1_children = server
        .axon_soll_children(&json!({ "id": "REQ-TRV-001" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&req1_children),
        vec!["REQ-TRV-002".to_string()]
    );

    // REQ-002 parents -> REQ-TRV-001
    let req2_parents = server
        .axon_soll_children(&json!({ "id": "REQ-TRV-002", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(
        extract_node_ids(&req2_parents),
        vec!["REQ-TRV-001".to_string()]
    );
}

/// REQ-AXO-902642 — A mixed graph: filiation (REFINES, TARGETS) MUST NOT include dependencies (BLOCKED_BY)
/// or retirements (SUPERSEDES). Non-hierarchical relation query must orient caller to incoming/outgoing.
#[test]
fn test_mixed_graph_strictly_excludes_non_hierarchical_relations_from_filiation() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-MIX-100', 'Requirement', 'MIX', 'Center Req', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-MIX-101', 'Requirement', 'MIX', 'Child Refines', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-MIX-102', 'Requirement', 'MIX', 'Child Targets', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-MIX-103', 'Requirement', 'MIX', 'Blocker', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-MIX-104', 'Requirement', 'MIX', 'Superseding', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('DEC-MIX-001', 'Decision', 'MIX', 'Decision Blocker', 'desc', 'current', '{}')",
    );

    // Child via incoming ChildToParent REFINES:
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-MIX-101', 'REQ-MIX-100', 'REFINES')",
    );
    // Child via outgoing ParentToChild TARGETS:
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-MIX-100', 'REQ-MIX-102', 'TARGETS')",
    );
    // Dependency via incoming BLOCKED_BY:
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-MIX-103', 'REQ-MIX-100', 'BLOCKED_BY')",
    );
    // Dependency via outgoing BLOCKED_BY:
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-MIX-100', 'DEC-MIX-001', 'BLOCKED_BY')",
    );
    // Retirement via incoming SUPERSEDES:
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('REQ-MIX-104', 'REQ-MIX-100', 'SUPERSEDES')",
    );

    // 1. Semantic `children` on REQ-MIX-100: must return ONLY REQ-MIX-101 and REQ-MIX-102!
    let children = server
        .axon_soll_children(&json!({ "id": "REQ-MIX-100", "direction": "children" }))
        .expect("must answer");
    let got_ids = extract_node_ids(&children);
    assert_eq!(children["data"]["count"], 2);
    assert!(got_ids.contains(&"REQ-MIX-101".to_string()));
    assert!(got_ids.contains(&"REQ-MIX-102".to_string()));
    assert!(
        !got_ids.contains(&"REQ-MIX-103".to_string()),
        "BLOCKED_BY is a dependency, not a child"
    );
    assert!(
        !got_ids.contains(&"DEC-MIX-001".to_string()),
        "BLOCKED_BY is a dependency, not a child"
    );
    assert!(
        !got_ids.contains(&"REQ-MIX-104".to_string()),
        "SUPERSEDES is a retirement, not a child"
    );

    // 2. Physical `incoming` on REQ-MIX-100: returns all incoming edges
    let inc = server
        .axon_soll_children(&json!({ "id": "REQ-MIX-100", "direction": "incoming" }))
        .expect("must answer");
    let inc_ids = extract_node_ids(&inc);
    assert_eq!(inc["data"]["count"], 3);
    assert!(inc_ids.contains(&"REQ-MIX-101".to_string()));
    assert!(inc_ids.contains(&"REQ-MIX-103".to_string()));
    assert!(inc_ids.contains(&"REQ-MIX-104".to_string()));

    // 3. Physical `outgoing` on REQ-MIX-100: returns all outgoing edges
    let out = server
        .axon_soll_children(&json!({ "id": "REQ-MIX-100", "direction": "outgoing" }))
        .expect("must answer");
    let out_ids = extract_node_ids(&out);
    assert_eq!(out["data"]["count"], 2);
    assert!(out_ids.contains(&"REQ-MIX-102".to_string()));
    assert!(out_ids.contains(&"DEC-MIX-001".to_string()));

    // 4. Asking for a non-hierarchical relation with direction="children"
    let non_hier = server
        .axon_soll_children(
            &json!({ "id": "REQ-MIX-100", "direction": "children", "relation_type": "BLOCKED_BY" }),
        )
        .expect("must answer");
    assert_eq!(non_hier["data"]["count"], 0);
    let text = non_hier["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("non-hierarchical") || text.contains("dependency"),
        "must explain that BLOCKED_BY is non-hierarchical: {text}"
    );
    assert!(
        text.contains("incoming") && text.contains("outgoing"),
        "must orient user towards incoming/outgoing: {text}"
    );
}

/// REQ-AXO-902642 — Returned node objects MUST carry physical `source_id` and `target_id`
/// to make `soll_manager action=unlink` immediate and unambiguous.
#[test]
fn test_node_payload_carries_physical_source_and_target_ids() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('MIL-PLD-001', 'Milestone', 'PLD', 'Milestone Payload', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-PLD-001', 'Requirement', 'PLD', 'Req Payload', 'desc', 'planned', '{}')",
    );
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('MIL-PLD-001', 'REQ-PLD-001', 'TARGETS')",
    );

    let res = server
        .axon_soll_children(&json!({ "id": "MIL-PLD-001" }))
        .expect("must answer");
    let nodes = res["data"]["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
    let n = &nodes[0];
    assert_eq!(n["id"], "REQ-PLD-001");
    assert_eq!(n["type"], "Requirement");
    assert_eq!(n["status"], "planned");
    assert_eq!(n["title"], "Req Payload");
    assert_eq!(n["relation_type"], "TARGETS");
    assert_eq!(
        n["source_id"], "MIL-PLD-001",
        "physical source_id must match Edge.source_id"
    );
    assert_eq!(
        n["target_id"], "REQ-PLD-001",
        "physical target_id must match Edge.target_id"
    );
}

/// REQ-AXO-902642 — Opposite direction check respects relation_type when provided.
#[test]
fn test_opposite_direction_check_respects_relation_filter() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('MIL-FLT-001', 'Milestone', 'FLT', 'Milestone Filter', 'desc', 'current', '{}')",
    );
    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-FLT-001', 'Requirement', 'FLT', 'Req Filter', 'desc', 'planned', '{}')",
    );
    exec(
        "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
          VALUES ('MIL-FLT-001', 'REQ-FLT-001', 'TARGETS')",
    );

    // Direction parents with relation_type="TARGETS" -> 0 found, other way has 1 TARGETS edge
    let res_targets = server
        .axon_soll_children(
            &json!({ "id": "MIL-FLT-001", "direction": "parents", "relation_type": "TARGETS" }),
        )
        .expect("must answer");
    assert_eq!(res_targets["data"]["count"], 0);
    let text_targets = res_targets["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text_targets.contains("1 edge(s) exist the other way"),
        "must notice the 1 TARGETS edge: {text_targets}"
    );

    // Direction parents with relation_type="BELONGS_TO" -> 0 found, other way has 0 BELONGS_TO edges
    let res_belongs = server
        .axon_soll_children(
            &json!({ "id": "MIL-FLT-001", "direction": "parents", "relation_type": "BELONGS_TO" }),
        )
        .expect("must answer");
    assert_eq!(res_belongs["data"]["count"], 0);
    let text_belongs = res_belongs["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !text_belongs.contains("1 edge(s) exist the other way"),
        "must NOT claim an edge exists for BELONGS_TO: {text_belongs}"
    );
}

/// REQ-AXO-902642 — Capping at 200 items sets `data.capped: true` and notes it in text.
#[test]
fn test_capping_at_200_items_sets_capped_flag() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
          VALUES ('REQ-CAP-000', 'Requirement', 'CAP', 'Parent', 'desc', 'current', '{}')",
    );

    for i in 1..=205 {
        let child_id = format!("REQ-CAP-{i:03}");
        exec(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('{child_id}', 'Requirement', 'CAP', 'Child {i}', 'desc', 'current', '{{}}')"
        ));
        exec(&format!(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type) \
             VALUES ('{child_id}', 'REQ-CAP-000', 'REFINES')"
        ));
    }

    let res = server
        .axon_soll_children(&json!({ "id": "REQ-CAP-000" }))
        .expect("must answer");
    assert_eq!(res["data"]["count"], 200);
    assert_eq!(
        res["data"]["capped"], true,
        "capped flag must be true when reaching 200"
    );
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("capped at 200"),
        "text must announce capping: {text}"
    );
}

/// REQ-AXO-902642 — A database error MUST propagate `isError: true` and status "backend_error"
/// instead of silently converting the error into an empty `nodes: [], status: ok` list.
#[test]
fn test_database_error_propagates_is_error_rather_than_swallowing_as_empty_ok() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    // Drop the edge table in this isolated test database to force a query error
    server.graph_store.execute("DROP TABLE soll.Edge").unwrap();

    let res = server
        .axon_soll_children(&json!({ "id": "REQ-ERR-001" }))
        .expect("must answer");
    assert_eq!(res["isError"], true, "query error must set isError: true");
    assert_eq!(res["data"]["status"], "backend_error");
}

/// REQ-AXO-902642 — Invalid direction rejected with `isError: true` and `input_invalid`.
#[test]
fn test_invalid_direction_rejected_with_input_invalid() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let res = server
        .axon_soll_children(&json!({ "id": "REQ-INV-001", "direction": "diagonal" }))
        .expect("must answer");
    assert_eq!(res["isError"], true);
    assert_eq!(res["data"]["status"], "input_invalid");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("Invalid direction"));
}
