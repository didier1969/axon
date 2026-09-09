// Copyright (c) Didier Stadelmann. All rights reserved.

use super::*;
use serde_json::json;

fn register_test_project(server: &McpServer, code: &str) {
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
             VALUES ('{code}', '/tmp/{code}', '{code}') ON CONFLICT (project_code) DO NOTHING"
        ))
        .unwrap();
    server.soll_cache().invalidate(code);
}

/// REQ-AXO-902595 — `soll_verify_requirements` teste que les critères d'acceptation
/// sont SATISFAITS, jamais seulement DÉCLARÉS.
///
/// Signalé par CSC : `REQ-CSC-046` est passé de `partial` à `done` du seul fait
/// d'avoir renseigné des critères textuels non vérifiés, alors que des critères
/// restaient ouverts et que le site n'était pas déployé.
///
/// Critères d'acceptation REQ-AXO-902595 :
/// 1. Un Requirement dont les critères sont écrits mais non vérifiés n'est JAMAIS compté `done`.
/// 2. L'état intermédiaire est NOMMÉ dans la réponse (`criteria_declared`), pas seulement absent du compteur `done`.
/// 3. Contrôle positif : ajouter des `acceptance_criteria` à un REQ non livré ne déplace plus son compteur vers `done`.
#[test]
fn test_requirement_with_unverified_criteria_does_not_count_as_done() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let project_code = "CSC";
    register_test_project(&server, project_code);

    // Requirement avec statut 'current' (non-terminal), des critères textuels non vérifiés (forme historique)
    // et une preuve rattachée (evidence_count > 0, artifact_type='symbol' pour éviter le filtre broken file).
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-{project_code}-046', 'Requirement', '{project_code}', \
             'Corpus non curé et site non déployé', '', 'current', \
             '{{\"acceptance_criteria\": [\"corpus curé\", \"tag posé\", \"walkthrough fait\"]}}')"
        ))
        .unwrap();

    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) \
             VALUES ('TRC-{project_code}-046', 'requirement', 'REQ-{project_code}-046', 'symbol', 'corpus_cure_fn', 1.0, 0)"
        ))
        .unwrap();

    server.soll_cache().invalidate(project_code);

    let result = server
        .axon_soll_verify_requirements(&json!({
            "project_code": project_code,
            "mode": "verbose"
        }))
        .expect("soll_verify_requirements response");

    assert!(
        result.get("isError").and_then(Value::as_bool) != Some(true),
        "call failed: {:?}",
        result
    );

    let data = &result["data"];
    let summary = &data["summary"];

    // Invariant REQ-AXO-902595 : `done` doit être 0 ! Les critères sont écrits mais non vérifiés.
    assert_eq!(
        summary["done"].as_u64(),
        Some(0),
        "un REQ non livré avec des critères non vérifiés ne doit JAMAIS être compté done: {:?}",
        summary
    );

    // L'état intermédiaire `criteria_declared` doit être exposé et valoir 1
    assert_eq!(
        summary["criteria_declared"].as_u64(),
        Some(1),
        "l'état intermédiaire `criteria_declared` doit être compté dans summary: {:?}",
        summary
    );

    // Détail du REQ
    let details = data["details"].as_array().expect("details array");
    let entry = details
        .iter()
        .find(|v| v["id"].as_str() == Some(&format!("REQ-{project_code}-046")))
        .expect("entry for REQ-CSC-046");

    assert_eq!(
        entry["state"].as_str(),
        Some("criteria_declared"),
        "entry state must be `criteria_declared`: {:?}",
        entry
    );
    assert_eq!(
        entry["completion_state"].as_str(),
        Some("criteria_declared"),
        "completion_state must be `criteria_declared`: {:?}",
        entry
    );
}

#[test]
fn test_adding_criteria_to_undelivered_requirement_does_not_increment_done() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let project_code = "CSD";
    register_test_project(&server, project_code);

    // 1. Initialement : REQ 'current' avec preuve mais sans critères.
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-{project_code}-001', 'Requirement', '{project_code}', \
             'Exigence initiale', '', 'current', '{{}}')"
        ))
        .unwrap();

    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) \
             VALUES ('TRC-{project_code}-001', 'requirement', 'REQ-{project_code}-001', 'symbol', 'symbol_csd', 1.0, 0)"
        ))
        .unwrap();

    server.soll_cache().invalidate(project_code);

    let res_before = server
        .axon_soll_verify_requirements(&json!({
            "project_code": project_code,
            "mode": "verbose"
        }))
        .expect("response before");

    assert_eq!(
        res_before["data"]["summary"]["done"].as_u64(),
        Some(0),
        "initial done must be 0"
    );
    assert_eq!(
        res_before["data"]["summary"]["partial"].as_u64(),
        Some(1),
        "initial partial must be 1"
    );

    // 2. Ajout de critères d'acceptation textuels non vérifiés.
    // Dans l'ancien code, cela faisait basculer partial -> done !
    server
        .graph_store
        .execute(&format!(
            "UPDATE soll.Node SET metadata = '{{\"acceptance_criteria\": [\"critère A non vérifié\"]}}' \
             WHERE id = 'REQ-{project_code}-001'"
        ))
        .unwrap();

    server.soll_cache().invalidate(project_code);

    let res_after = server
        .axon_soll_verify_requirements(&json!({
            "project_code": project_code,
            "mode": "verbose"
        }))
        .expect("response after");

    // Contrôle positif : l'ajout de critères ne doit PAS faire monter le compteur `done`.
    assert_eq!(
        res_after["data"]["summary"]["done"].as_u64(),
        Some(0),
        "done must remain 0 after adding unverified criteria (CSC falsification test)"
    );
    assert_eq!(
        res_after["data"]["summary"]["criteria_declared"].as_u64(),
        Some(1),
        "criteria_declared must become 1"
    );
}

#[test]
fn test_requirement_with_all_criteria_met_or_waived_counts_as_done() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let project_code = "CSE";
    register_test_project(&server, project_code);

    // Requirement avec statut 'current', preuve attachée et tous les critères formellement
    // vérifiés ('met') ou légitimement écartés avec raison ('waived').
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-{project_code}-001', 'Requirement', '{project_code}', \
             'Tous critères vérifiés', '', 'current', \
             '{{\"acceptance_criteria\": [ \
                 {{\"criterion\": \"test unitaire vert\", \"state\": \"met\"}}, \
                 {{\"criterion\": \"test charge sous VM\", \"state\": \"waived\", \"reason\": \"VM inaccessible sur runner CI\"}} \
             ]}}')"
        ))
        .unwrap();

    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) \
             VALUES ('TRC-{project_code}-001', 'requirement', 'REQ-{project_code}-001', 'symbol', 'symbol_cse', 1.0, 0)"
        ))
        .unwrap();

    server.soll_cache().invalidate(project_code);

    let res = server
        .axon_soll_verify_requirements(&json!({
            "project_code": project_code,
            "mode": "verbose"
        }))
        .expect("response");

    assert_eq!(
        res["data"]["summary"]["done"].as_u64(),
        Some(1),
        "requirement with all criteria met or waived must count as done"
    );
    assert_eq!(
        res["data"]["summary"]["criteria_declared"].as_u64(),
        Some(0),
        "criteria_declared must be 0 when all criteria are satisfied"
    );
}
