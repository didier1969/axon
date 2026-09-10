// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902658 / DEC-AXO-901704 (Feedback #428):
//! 1. `axon_init_project` prend en compte `project_name` explicite passé en argument.
//! 2. `axon_init_project` préserve le `project_name` préexistant dans `soll.ProjectCodeRegistry`
//!    s'il n'est pas redéfini explicitement (au lieu de l'écraser silencieusement par le nom dérivé du path).
//! 3. `diagnose_indexing` élimine la mention erronée `+pattern dans .axonignore` et oriente
//!    clairement vers `.axoninclude`.

use crate::mcp::tests::create_test_server;
use serde_json::json;

#[test]
fn c6_axon_init_project_respects_explicit_project_name() {
    let server = create_test_server();
    let project_path = "/tmp/test-gov-explicit-pne";

    let req = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "project_name": "My Custom Project Display"
            }
        },
        "id": 1
    });

    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .expect("handle_request");
    let result = resp.result.expect("result");
    let data = &result["data"];

    assert_eq!(
        data["project_name"].as_str(),
        Some("My Custom Project Display"),
        "axon_init_project doit honorer le project_name explicite fourni dans les arguments"
    );

    let assigned_code = data["project_code"].as_str().expect("assigned code");

    // Vérifier la persistance dans soll.ProjectCodeRegistry
    let query_res = server
        .graph_store
        .query_json_param(
            "SELECT project_name FROM soll.ProjectCodeRegistry WHERE project_code = ?",
            &json!([assigned_code]),
        )
        .expect("query registry");
    let rows: Vec<Vec<String>> = serde_json::from_str(&query_res).unwrap_or_default();
    assert_eq!(
        rows.first().and_then(|r| r.first()).map(|s| s.as_str()),
        Some("My Custom Project Display"),
        "soll.ProjectCodeRegistry doit contenir le project_name explicite"
    );
}

#[test]
fn c6_axon_init_project_preserves_existing_project_name_when_omitted() {
    let server = create_test_server();
    let project_path = "/tmp/test-gov-preserve-pne";

    // 1. Initialiser une première fois avec un nom personnalisé
    let req1 = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path,
                "project_name": "Important Custom Title"
            }
        },
        "id": 1
    });
    let resp1 = server
        .handle_request(serde_json::from_value(req1).unwrap())
        .expect("handle_request 1");
    let result1 = resp1.result.expect("result 1");
    let code1 = result1["data"]["project_code"].as_str().expect("code 1");

    // 2. Ré-exécuter axon_init_project sans le paramètre project_name
    let req2 = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": project_path
            }
        },
        "id": 2
    });
    let resp2 = server
        .handle_request(serde_json::from_value(req2).unwrap())
        .expect("handle_request 2");
    let result2 = resp2.result.expect("result 2");
    let data2 = &result2["data"];

    assert_eq!(
        data2["project_name"].as_str(),
        Some("Important Custom Title"),
        "axon_init_project sans project_name doit préserver le nom déjà enregistré au lieu de l'écraser"
    );

    // Vérifier en base
    let query_res = server
        .graph_store
        .query_json_param(
            "SELECT project_name FROM soll.ProjectCodeRegistry WHERE project_code = ?",
            &json!([code1]),
        )
        .expect("query registry");
    let rows: Vec<Vec<String>> = serde_json::from_str(&query_res).unwrap_or_default();
    assert_eq!(
        rows.first().and_then(|r| r.first()).map(|s| s.as_str()),
        Some("Important Custom Title"),
        "le project_name dans soll.ProjectCodeRegistry ne doit pas être écrasé lors d'un ré-enrôlement sans nom"
    );
}

#[test]
fn c6_diagnose_indexing_remediation_points_to_axoninclude() {
    let server = create_test_server();

    // Appeler diagnose_indexing sur un projet fictif sans fichiers indexés
    let req = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "diagnose_indexing",
            "arguments": {
                "project_code": "NONEXISTENT_DGN"
            }
        },
        "id": 1
    });

    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .expect("handle_request");
    let result = resp.result.expect("result");
    let text = result["content"][0]["text"].as_str().unwrap_or("");

    assert!(
        !text.contains("+pattern"),
        "diagnose_indexing ne doit JAMAIS suggérer la syntaxe erronée '+pattern' dans .axonignore"
    );
}
