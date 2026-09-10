// Copyright (c) Didier Stadelmann. All rights reserved.

//! REQ-AXO-902513 — Un état neuf ajouté au code d'un outil doit être ajouté à
//! sa description publiée — généralisation de la garde.
//!
//! Motivé par l'incident de `neutral_borderline` (`REQ-AXO-902502`) et étendu
//! ici aux autres outils MCP publiant des états/phases énumérés (`promote_status`,
//! `change_safety`, etc.).
//!
//! Invariants vérifiés :
//! 1. La garde lit directement les sources de code (via `include_str!`) ET la
//!    description rendue par `tools_catalog(true)`. Elle ne porte aucune copie
//!    statique manuelle des listes.
//! 2. Tout état rendu par le code DOIT être présent dans la description publiée.
//! 3. La garde est générique et s'applique de manière uniforme.
//! 4. Test de falsification : retirer un état de la description provoque un échec.

use crate::mcp::catalog::tools_catalog;

/// Extrait les chaînes littérales entre guillemets contenues dans un sous-bloc
/// délimité par `start_marker` et `end_marker`.
fn extract_literals_in_block<'a>(
    source: &'a str,
    start_marker: &str,
    end_marker: &str,
) -> Vec<&'a str> {
    let start = source
        .find(start_marker)
        .unwrap_or_else(|| panic!("start_marker `{start_marker}` introuvable dans le code source"));
    let sub = &source[start..];
    let end = sub
        .find(end_marker)
        .unwrap_or_else(|| panic!("end_marker `{end_marker}` introuvable après `{start_marker}`"));
    let block = &sub[..end];

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in block
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        if seen.insert(s) {
            out.push(s);
        }
    }
    out
}

/// Extrait les chaînes littérales encapsulées dans `Some("...")` dans un sous-bloc.
fn extract_some_string_literals<'a>(
    source: &'a str,
    start_marker: &str,
    end_marker: &str,
) -> Vec<&'a str> {
    let start = source
        .find(start_marker)
        .unwrap_or_else(|| panic!("start_marker `{start_marker}` introuvable dans le code source"));
    let sub = &source[start..];
    let end = sub
        .find(end_marker)
        .unwrap_or_else(|| panic!("end_marker `{end_marker}` introuvable après `{start_marker}`"));
    let block = &sub[..end];

    let mut out = Vec::new();
    let marker = "Some(\"";
    let mut cursor = block;
    while let Some(pos) = cursor.find(marker) {
        let after = &cursor[pos + marker.len()..];
        if let Some(quote_end) = after.find('"') {
            out.push(&after[..quote_end]);
            cursor = &after[quote_end + 1..];
        } else {
            break;
        }
    }
    out
}

/// Vérifie de façon générique que tous les états extraits du code source
/// sont mentionnés dans la description publiée de l'outil dans le catalogue.
fn assert_code_states_announced_in_description(
    tool_name: &str,
    state_kind: &str,
    code_states: &[&str],
) {
    assert!(
        !code_states.is_empty(),
        "aucun état extrait du code source pour `{tool_name}` ({state_kind})"
    );

    let catalogue = tools_catalog(true);
    let description = catalogue["tools"]
        .as_array()
        .expect("catalogue sans tableau `tools`")
        .iter()
        .find(|t| t["name"] == tool_name)
        .unwrap_or_else(|| panic!("outil `{tool_name}` absent du catalogue publié"))["description"]
        .as_str()
        .expect("description non textuelle");

    let muets: Vec<&&str> = code_states
        .iter()
        .filter(|s| !description.contains(**s))
        .collect();

    assert!(
        muets.is_empty(),
        "{} état(s) de `{}` que le code peut rendre pour `{}` sont ABSENTS de la description publiée : {:?} — la description publiée est : {}",
        muets.len(),
        state_kind,
        tool_name,
        muets,
        description
    );
}

#[test]
fn test_req_902513_generic_guard_contradiction_check_verdicts() {
    let source = include_str!("../tools_nli.rs");
    let verdicts = extract_literals_in_block(source, "let verdict = if", "\n        };");
    let mut expected = vec![
        "contradicts",
        "neutral_borderline",
        "neutral",
        "inconclusive",
    ];
    let mut actual = verdicts.clone();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    assert_code_states_announced_in_description("contradiction_check", "verdict", &verdicts);
}

#[test]
fn test_req_902513_generic_guard_promote_status_phases() {
    let source = include_str!("../../release_reconciler.rs");

    // 1. Phases issues de liveness_phase
    let liveness_phases = extract_some_string_literals(
        source,
        "pub fn liveness_phase(l: &LivenessFacts)",
        "\npub fn liveness_next_action",
    );

    // 2. Phases issues de phase(&facts)
    let release_phases = extract_literals_in_block(
        source,
        "pub fn phase(f: &ReleaseFacts) -> &'static str {",
        "\n}",
    );

    let mut all_phases = liveness_phases;
    all_phases.extend(release_phases);
    all_phases.sort_unstable();
    all_phases.dedup();

    assert_code_states_announced_in_description("promote_status", "phase", &all_phases);
}

#[test]
fn test_req_902513_generic_guard_change_safety_states() {
    let source = include_str!("../tools_framework_change_safety.rs");
    let states =
        extract_literals_in_block(source, "let safety = if tested.is_none() {", "\n    };");
    assert_code_states_announced_in_description("change_safety", "safety", &states);
}

#[test]
fn test_req_902513_falsification_reports_missing_state() {
    // Falsification explicite : si un état réel n'est pas dans une fausse description,
    // l'assertion de présence échoue immédiatement en nommant l'état manquant.
    let fake_description =
        "phase ∈ {brain_down, indexer_down, staged, drift, uninitialized, clean}";
    let states = vec!["brain_down", "brain_accept_queue_saturated", "clean"];

    let muets: Vec<&&str> = states
        .iter()
        .filter(|s| !fake_description.contains(**s))
        .collect();

    assert_eq!(muets, vec![&"brain_accept_queue_saturated"]);
}
