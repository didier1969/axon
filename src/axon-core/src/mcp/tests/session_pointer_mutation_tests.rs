// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902510:
//! `axon_init_project` efface le session_pointer en silence — une mutation destructive dans un paramètre d'appel.
//!
//! Critères d'acceptation :
//! 1. Rejet de contradiction : refuser d'effacer (null) ou de passer à kind=none le session_pointer si un nœud SOLL session_pointer actif (status=current) existe.
//! 2. Traçabilité de l'ancien pointeur : la valeur précédente est retournée dans `data.previous_session_pointer` et `data.session_pointer_mutation`.
//! 3. Annonce explicite : la réponse texte annonce la mutation au lieu de rester silencieuse.
//! 4. Effacement autorisé quand aucun nœud SOLL actif n'existe.

use crate::mcp::tests::create_test_server;
use serde_json::json;

#[test]
fn c1_rejects_clearing_session_pointer_when_active_soll_node_exists() {
    let server = create_test_server();
    let project_code = "SP1";
    let project_path = "/tmp/test-sp1";

    // Enregistrer le projet
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name, session_pointer_json) VALUES (?, ?, ?, ?)",
            &json!([project_code, project_path, "sp1", "{\"kind\":\"soll_node\",\"value\":\"CPT-SP1-001\"}"]),
        )
        .expect("insert project registry");

    // Créer un nœud SOLL Concept actif de type session_pointer
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Node (id, type, project_code, status, title, description, metadata) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &json!([
                "CPT-SP1-001",
                "concept",
                project_code,
                "current",
                "Session Pointer SP1",
                "Description",
                "{\"kind\":\"session_pointer\"}"
            ]),
        )
        .expect("insert soll node");

    // Appel axon_init_project avec null pour tenter d'effacer le session_pointer
    let req_null = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "session_pointer": null
            }
        },
        "id": 1
    });

    let resp_null = server
        .handle_request(serde_json::from_value(req_null).unwrap())
        .unwrap();
    let res_null = resp_null.result.unwrap();
    assert_eq!(
        res_null.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );
    let text_null = res_null["content"][0]["text"].as_str().unwrap();
    assert!(text_null.contains("Contradiction detected") || text_null.contains("CPT-SP1-001"));

    // Appel axon_init_project avec kind=none pour tenter de désactiver
    let req_none = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "session_pointer": { "kind": "none" }
            }
        },
        "id": 2
    });

    let resp_none = server
        .handle_request(serde_json::from_value(req_none).unwrap())
        .unwrap();
    let res_none = resp_none.result.unwrap();
    assert_eq!(
        res_none.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );
    let text_none = res_none["content"][0]["text"].as_str().unwrap();
    assert!(text_none.contains("Contradiction detected") || text_none.contains("CPT-SP1-001"));
}

#[test]
fn c2_reports_previous_session_pointer_on_update_and_clear() {
    let server = create_test_server();
    let project_code = "SP2";
    let project_path = "/tmp/test-sp2";

    // Initialiser le projet avec un session_pointer initial
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name, session_pointer_json) VALUES (?, ?, ?, ?)",
            &json!([project_code, project_path, "sp2", "{\"kind\":\"file\",\"value\":\"docs/handoff-1.md\",\"label\":\"initial handoff\"}"]),
        )
        .expect("insert project registry");

    // Mettre à jour vers un nouveau pointeur
    let req_update = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "session_pointer": {
                    "kind": "file",
                    "value": "docs/handoff-2.md",
                    "label": "second handoff"
                }
            }
        },
        "id": 3
    });

    let resp_update = server
        .handle_request(serde_json::from_value(req_update).unwrap())
        .unwrap();
    let res_update = resp_update.result.unwrap();
    assert_ne!(
        res_update.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );

    // data.previous_session_pointer doit contenir l'ancien pointeur
    let prev = res_update["data"]["previous_session_pointer"].clone();
    assert_eq!(prev["kind"].as_str(), Some("file"));
    assert_eq!(prev["value"].as_str(), Some("docs/handoff-1.md"));

    let text_update = res_update["content"][0]["text"].as_str().unwrap();
    assert!(
        text_update.contains("MUTATION: session_pointer was UPDATED")
            || text_update.contains("docs/handoff-1.md")
    );

    // Maintenant effacer (null) puisqu'aucun nœud SOLL n'existe
    let req_clear = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "session_pointer": null
            }
        },
        "id": 4
    });

    let resp_clear = server
        .handle_request(serde_json::from_value(req_clear).unwrap())
        .unwrap();
    let res_clear = resp_clear.result.unwrap();
    assert_ne!(
        res_clear.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );

    let prev_clear = res_clear["data"]["previous_session_pointer"].clone();
    assert_eq!(prev_clear["value"].as_str(), Some("docs/handoff-2.md"));

    let text_clear = res_clear["content"][0]["text"].as_str().unwrap();
    assert!(
        text_clear.contains("MUTATION: session_pointer was CLEARED")
            || text_clear.contains("docs/handoff-2.md")
    );
}
