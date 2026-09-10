// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902659 / DEC-AXO-901705 (Feedback #421):
//! Sur HardAwareCriticalBlockLnsFullPrototype.java avec une question explicite sur
//! GREEN-B G6-D2, déterminisme, déduplication et qualification, `why` doit retourner
//! l'exigence active pertinente `REQ-KKI-126` en tête de classement (plutôt que d'être
//! masquée ou reléguée derrière `REQ-KKI-042` par un plafonnement min(2) et un tri par ID ASC).

use crate::mcp::tests::create_test_server;
use serde_json::json;

#[test]
fn c7_why_ranks_active_lexically_matched_requirement_over_older_generic_requirement() {
    let server = create_test_server();
    let project_code = "KKI";
    let file_uri = "src/main/java/HardAwareCriticalBlockLnsFullPrototype.java";

    // Enregistrer le projet et le chunk de fichier indexé
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) VALUES (?, ?, ?)",
            &json!([project_code, "/tmp/kki-project", "KKI Project"]),
        )
        .expect("insert project");

    server
        .graph_store
        .execute_param(
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES (?, ?, ?, ?, ?, ?)",
            &json!(["chk-kki-1", "file", file_uri, project_code, file_uri, "hash-kki-1"]),
        )
        .expect("insert chunk");

    // 1. REQ-KKI-042 : Exigence ancienne générique
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Node (id, type, project_code, status, title, description) VALUES (?, ?, ?, ?, ?, ?)",
            &json!([
                "REQ-KKI-042",
                "Requirement",
                project_code,
                "delivered",
                "Generic Block Prototype Baseline",
                "Original baseline block handling without specific GREEN-B contract"
            ]),
        )
        .expect("insert REQ-KKI-042");

    // 2. REQ-KKI-126 : Exigence active portant le contrat GREEN-B G6-D2
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Node (id, type, project_code, status, title, description) VALUES (?, ?, ?, ?, ?, ?)",
            &json!([
                "REQ-KKI-126",
                "Requirement",
                project_code,
                "current",
                "Contract GREEN-B G6-D2 Determinism Deduplication Qualification",
                "Active specification governing RED3-B/GREEN-B determinism and qualification"
            ]),
        )
        .expect("insert REQ-KKI-126");

    // Attacher le même fichier aux deux exigences dans soll.Traceability
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_id, soll_entity_type, artifact_type, artifact_ref) VALUES (?, ?, ?, ?, ?)",
            &json!(["TRC-KKI-001", "REQ-KKI-042", "requirement", "File", file_uri]),
        )
        .expect("insert trace REQ-KKI-042");

    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_id, soll_entity_type, artifact_type, artifact_ref) VALUES (?, ?, ?, ?, ?)",
            &json!(["TRC-KKI-002", "REQ-KKI-126", "requirement", "File", file_uri]),
        )
        .expect("insert trace REQ-KKI-126");

    // Ajouter une preuve de validation attachée à REQ-KKI-126
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_id, soll_entity_type, artifact_type, artifact_ref) VALUES (?, ?, ?, ?, ?)",
            &json!(["TRC-KKI-003", "REQ-KKI-126", "requirement", "Test", "com.kki.HardAwareCriticalBlockTest"]),
        )
        .expect("insert trace test REQ-KKI-126");

    // Invalider le cache snapshot pour forcer la relecture de la base
    server.soll_cache().invalidate(project_code);

    // Appeler `why` avec la question ciblée
    let req = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "why",
            "arguments": {
                "project": project_code,
                "question": "Sur HardAwareCriticalBlockLnsFullPrototype.java quel est le contrat GREEN-B G6-D2 determinisme qualification?"
            }
        },
        "id": 1
    });

    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .expect("handle_request");
    let result = resp.result.expect("result");
    let data = &result["data"];
    let why = &data["why"];

    let governing_reqs = why["governing_requirements"]
        .as_array()
        .expect("governing_requirements array");

    assert!(
        !governing_reqs.is_empty(),
        "why doit retourner des exigences gouvernantes"
    );

    let first_req = &governing_reqs[0];
    let first_id = first_req["id"].as_str().unwrap_or("");

    assert_eq!(
        first_id,
        "REQ-KKI-126",
        "REQ-KKI-126 doit etre classee en premiere position grace a la correspondance lexicale (GREEN-B G6-D2) et son statut actif"
    );

    let score_126 = first_req["ranking_score"].as_i64().unwrap_or(0);
    assert!(
        score_126 > 95,
        "le score de REQ-KKI-126 ({score_126}) doit etre superieur au score de base File (95) grace aux bonus de correspondance et de statut"
    );
}

#[test]
fn c7_collect_soll_traceability_does_not_artificially_cap_candidates_at_two() {
    let server = create_test_server();
    let project_code = "CAP";
    let file_uri = "src/SharedModule.rs";

    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) VALUES (?, ?, ?)",
            &json!([project_code, "/tmp/cap-project", "CAP Project"]),
        )
        .expect("insert project");

    server
        .graph_store
        .execute_param(
            "INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES (?, ?, ?, ?, ?, ?)",
            &json!(["chk-cap-1", "file", file_uri, project_code, file_uri, "hash-cap-1"]),
        )
        .expect("insert chunk");

    for i in 1..=4 {
        let req_id = format!("REQ-CAP-00{i}");
        server
            .graph_store
            .execute_param(
                "INSERT INTO soll.Node (id, type, project_code, status, title, description) VALUES (?, ?, ?, ?, ?, ?)",
                &json!([req_id, "Requirement", project_code, "current", format!("Requirement {i}"), "Description"]),
            )
            .expect("insert node");

        let trc_id = format!("TRC-CAP-00{i}");
        server
            .graph_store
            .execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_id, soll_entity_type, artifact_type, artifact_ref) VALUES (?, ?, ?, ?, ?)",
                &json!([trc_id, req_id, "requirement", "File", file_uri]),
            )
            .expect("insert trace");
    }

    server.soll_cache().invalidate(project_code);

    let req = json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "why",
            "arguments": {
                "project": project_code,
                "question": "Why does SharedModule.rs exist?",
                "top_k": 4
            }
        },
        "id": 1
    });

    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .expect("handle_request");
    let result = resp.result.expect("result");
    let governing_reqs = result["data"]["why"]["governing_requirements"]
        .as_array()
        .expect("governing_requirements array");

    assert!(
        governing_reqs.len() >= 3,
        "why doit renvoyer au moins 3 exigences lorsque 4 sont liees et top_k=4 (sans etre bride a min(2)), got: {}",
        governing_reqs.len()
    );
}
