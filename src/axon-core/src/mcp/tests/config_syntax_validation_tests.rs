// Copyright (c) Didier Stadelmann. All rights reserved.

use std::fs;
use tempfile::tempdir;

/// REQ-AXO-902657 (Feedback #431) — Validation syntaxique statique des fichiers de configuration
/// critiques (.json, .toml, .nix, .yaml) au pré-vol et au commit.

#[test]
fn c5_validate_config_syntax_rejects_invalid_json() {
    let tmp = tempdir().expect("tempdir");
    let bad_json = tmp.path().join("config.json");
    fs::write(&bad_json, "{ \"key\": , }").expect("write bad json");

    let err = crate::mcp::tools_soll::workflow::workflow_project::validate_config_file_syntax(
        tmp.path(),
        "config.json",
    );
    assert!(
        err.is_some(),
        "un fichier JSON malformé doit être rejeté par validate_config_file_syntax"
    );
    let violation = err.unwrap();
    let diagnostic = violation["diagnostic"].as_str().unwrap_or("");
    assert!(
        diagnostic.contains("JSON syntaxiquement invalide"),
        "le diagnostic doit expliciter l'erreur de syntaxe JSON: {diagnostic}"
    );
}

#[test]
fn c5_validate_config_syntax_rejects_invalid_toml() {
    let tmp = tempdir().expect("tempdir");
    let bad_toml = tmp.path().join("Cargo.toml");
    fs::write(&bad_toml, "package = [this is broken").expect("write bad toml");

    let err = crate::mcp::tools_soll::workflow::workflow_project::validate_config_file_syntax(
        tmp.path(),
        "Cargo.toml",
    );
    assert!(
        err.is_some(),
        "un fichier TOML malformé doit être rejeté par validate_config_file_syntax"
    );
    let violation = err.unwrap();
    let diagnostic = violation["diagnostic"].as_str().unwrap_or("");
    assert!(
        diagnostic.contains("TOML syntaxiquement invalide"),
        "le diagnostic doit expliciter l'erreur de syntaxe TOML: {diagnostic}"
    );
}

#[test]
fn c5_validate_config_syntax_accepts_valid_files() {
    let tmp = tempdir().expect("tempdir");
    let good_json = tmp.path().join("settings.json");
    fs::write(&good_json, "{\"valid\": true, \"count\": 42}").expect("write good json");
    let good_toml = tmp.path().join("Config.toml");
    fs::write(&good_toml, "[section]\nname = \"axon\"\nactive = true\n").expect("write good toml");

    assert!(
        crate::mcp::tools_soll::workflow::workflow_project::validate_config_file_syntax(
            tmp.path(),
            "settings.json"
        )
        .is_none(),
        "un fichier JSON valide ne doit produire aucune violation"
    );
    assert!(
        crate::mcp::tools_soll::workflow::workflow_project::validate_config_file_syntax(
            tmp.path(),
            "Config.toml"
        )
        .is_none(),
        "un fichier TOML valide ne doit produire aucune violation"
    );
}

#[test]
fn c5_commit_work_dry_run_rejects_broken_config_in_diff_paths() {
    let tmp = tempdir().expect("tempdir");
    let bad_json = tmp.path().join("bad_schema.json");
    fs::write(&bad_json, "{\ninvalid json here\n}").expect("write bad json");

    let violations =
        crate::mcp::tools_soll::workflow::workflow_project::validate_diff_paths_config_syntax(
            tmp.path(),
            &["bad_schema.json".to_string()],
        );
    assert_eq!(
        violations.len(),
        1,
        "diff_paths contenant un JSON invalide doit lever exactement 1 violation"
    );
    assert!(violations[0]["diagnostic"]
        .as_str()
        .unwrap()
        .contains("bad_schema.json"));
}

#[test]
fn c5_axon_commit_work_rejects_corrupted_config_file() {
    let server = super::create_test_server();
    let tmp = tempdir().expect("tempdir");
    let bad_config = tmp.path().join("broken_settings.json");
    fs::write(&bad_config, "{ \"key\": , }").expect("write bad json");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "project_path": tmp.path().to_str().unwrap(),
                "diff_paths": ["broken_settings.json"],
                "message": "fix(config): broken settings",
                "dry_run": true
            }
        },
        "id": 1
    });

    let res = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "axon_commit_work must reject corrupted config file"
    );
    let content = res.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        content.contains("JSON syntaxiquement invalide"),
        "error message must mention JSON syntax invalidity: {content}"
    );
}

#[test]
fn c5_axon_pre_flight_check_rejects_corrupted_config_file() {
    let server = super::create_test_server();
    let tmp = tempdir().expect("tempdir");
    let bad_config = tmp.path().join("broken_cargo.toml");
    fs::write(&bad_config, "package = [invalid toml").expect("write bad toml");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "project_path": tmp.path().to_str().unwrap(),
                "diff_paths": ["broken_cargo.toml"],
                "message": "fix(toml): broken toml"
            }
        },
        "id": 2
    });

    let res = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "axon_pre_flight_check must reject corrupted config file"
    );
    let content = res.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        content.contains("TOML syntaxiquement invalide"),
        "error message must mention TOML syntax invalidity: {content}"
    );
}
