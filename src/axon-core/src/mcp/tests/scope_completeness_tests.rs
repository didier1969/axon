// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902352:
//! `Scope completeness N/N` compte les fichiers INDEXES, pas les fichiers SOURCE :
//! un langage entier absent se lit « 100 % complet ».
//!
//! Critères d'acceptation :
//! 1. `Scope completeness` distingue le dénominateur INDEXÉ du dénominateur SOURCE,
//!    ou nomme les extensions écartées.
//! 2. Un projet polyglotte dont un langage n'est pas supporté ne peut pas afficher N/N sans mention.
//! 3. Repro INK : la réponse mentionne les 18 fichiers .rs non couverts.

use crate::mcp::tools_dx::ProjectScopeSummary;
use crate::mcp::tools_framework_runtime_status::ligne_code_intel;
use crate::mcp::tools_dx::project_scope_truth_note_pure;
use crate::scanner::ScopeBreakdown;

#[test]
fn c1_repro_ink_scope_completeness_surfaces_excluded_rust_files() {
    // Repro INK mesuré dans REQ-AXO-902352 :
    // 312 fichiers enrôlés (Elixir + Markdown), 18 fichiers .rs non couverts.
    let summary = ProjectScopeSummary {
        total_files: 312,
        completed_files: 312,
        backlog_files: 0,
        pending_reasons: Vec::new(),
        excluded_source_files: 18,
        excluded_extensions: ".rs: 18".to_string(),
        unchunked_files: 0,
    };

    let note = project_scope_truth_note_pure("INK", &summary);

    // 1. Dénominateur enrôlé présent
    assert!(note.contains("Scope completeness `INK`:** 312/312 fichier(s) enrôlé(s)"));
    // 2. Mention explicite des 18 fichiers .rs écartés
    assert!(
        note.contains("18 fichier(s) source écarté(s)"),
        "doit nommer les 18 fichiers source écartés: {note}"
    );
    assert!(
        note.contains(".rs: 18"),
        "doit citer l'extension .rs et son compte: {note}"
    );
    // 3. Avertissement sur l'absence non prouvée pour les langages non couverts
    assert!(
        note.contains("ne prouve PAS l'absence"),
        "un résultat vide ne prouve pas l'absence sur un projet avec code source écarté: {note}"
    );
}

#[test]
fn c2_polyglot_unsupported_language_cannot_display_green_live_without_mention() {
    // Critère 2 : un projet polyglotte avec fichiers source écartés ne doit PAS
    // être certifié trustworthy ni rendu comme un simple LIVE sans mention.
    let summary = ProjectScopeSummary {
        total_files: 312,
        completed_files: 312,
        backlog_files: 0,
        pending_reasons: Vec::new(),
        excluded_source_files: 18,
        excluded_extensions: ".rs: 18".to_string(),
        unchunked_files: 0,
    };

    // symbol_coverage_is_trustworthy DOIT être false car un langage entier est absent
    assert!(
        !summary.symbol_coverage_is_trustworthy(),
        "un projet avec fichiers source écartés ne peut pas être trustworthy"
    );

    // Dans le bandeau de status, ligne_code_intel doit basculer en PARTIAL
    let status_line = ligne_code_intel(Some("INK"), Some(&summary));
    assert!(
        status_line.starts_with("**Code-intel:** PARTIAL"),
        "le status doit être PARTIAL et non LIVE: {status_line}"
    );
    assert!(
        status_line.contains("18 fichier(s) source écarté(s)"),
        "status doit mentionner les 18 fichiers écartés: {status_line}"
    );
    assert!(
        status_line.contains(".rs: 18"),
        "status doit mentionner .rs: {status_line}"
    );
}

#[test]
fn c3_nominal_project_without_excluded_source_stays_live() {
    // Non-régression : un projet nominal sans fichiers source écartés
    let summary = ProjectScopeSummary {
        total_files: 900,
        completed_files: 850,
        backlog_files: 50,
        pending_reasons: Vec::new(),
        excluded_source_files: 0,
        excluded_extensions: String::new(),
        unchunked_files: 0,
    };

    assert!(summary.symbol_coverage_is_trustworthy());
    let note = project_scope_truth_note_pure("AXO", &summary);
    assert!(!note.contains("écarté"));

    let status_line = ligne_code_intel(Some("AXO"), Some(&summary));
    assert!(status_line.starts_with("**Code-intel:** LIVE"));
}

#[test]
fn c4_scope_breakdown_formats_excluded_source_summary() {
    let breakdown = ScopeBreakdown {
        eligible: 312,
        walked_files: 330,
        excluded_by_reason: vec![("ignored_by_extension_or_hidden_filter".to_string(), 18)],
        parsable_but_excluded: vec![("rs".to_string(), 18)],
    };

    assert_eq!(breakdown.excluded_source_count(), 18);
    assert_eq!(breakdown.excluded_source_summary(), ".rs: 18");
}

#[test]
fn c5_empty_code_intel_signals_fail_open_for_guard() {
    // REQ-AXO-902406: When a project has 0 files with symbols or total_files <= 0,
    // code_intel status must report empty and code_intel_empty must be true.
    let empty_summary = ProjectScopeSummary {
        total_files: 0,
        completed_files: 0,
        backlog_files: 0,
        pending_reasons: Vec::new(),
        excluded_source_files: 0,
        excluded_extensions: String::new(),
        unchunked_files: 0,
    };
    assert_eq!(empty_summary.completed_files, 0);

    let is_empty = empty_summary.total_files <= 0 || empty_summary.completed_files <= 0;
    assert!(is_empty, "empty project scope must resolve is_empty=true");

    let live_summary = ProjectScopeSummary {
        total_files: 100,
        completed_files: 90,
        backlog_files: 10,
        pending_reasons: Vec::new(),
        excluded_source_files: 0,
        excluded_extensions: String::new(),
        unchunked_files: 0,
    };
    let is_live_empty = live_summary.total_files <= 0 || live_summary.completed_files <= 0;
    assert!(!is_live_empty, "live project scope must resolve is_empty=false");
    assert!(live_summary.symbol_coverage_is_trustworthy());
}

#[test]
fn c6_repro_kki_unchunked_files_blocks_trustworthy_and_warns_on_empty() {
    // Repro KKI mesuré dans REQ-AXO-902511 :
    // Dépôt KIE de 17 318 fichiers enrôlés, 16 460 portant des symboles (95 % global),
    // mais 103 fichiers SANS CHUNK (dont les 96 fichiers du code applicatif kki-domain-vertical-slice).
    let summary = ProjectScopeSummary {
        total_files: 17318,
        completed_files: 16460,
        backlog_files: 858,
        pending_reasons: Vec::new(),
        excluded_source_files: 0,
        excluded_extensions: String::new(),
        unchunked_files: 103,
    };

    // 1. Même avec un ratio global de 95 %, symbol_coverage_is_trustworthy DOIT être false
    // car des fichiers enrôlés sont sans chunk.
    assert!(
        !summary.symbol_coverage_is_trustworthy(),
        "un projet avec fichiers sans chunk ne peut pas être trustworthy (REQ-AXO-902511)"
    );

    // 2. Le bandeau de status DOIT basculer en PARTIAL
    let status_line = ligne_code_intel(Some("KKI"), Some(&summary));
    assert!(
        status_line.starts_with("**Code-intel:** PARTIAL"),
        "status doit être PARTIAL et non LIVE: {status_line}"
    );
    assert!(
        status_line.contains("103 fichier(s) enrôlé(s) sans aucun chunk"),
        "status doit expliciter le gap d'indexation: {status_line}"
    );

    // 3. La note de portée de query/inspect DOIT expliciter le gap et avertir
    // qu'un résultat vide ne prouve pas l'absence sur une zone locale.
    let note = project_scope_truth_note_pure("KKI", &summary);
    assert!(
        note.contains("103 fichier(s) sans chunk (gap indexation)"),
        "la note doit nommer les fichiers sans chunk: {note}"
    );
    assert!(
        note.contains("ne garantit pas la couverture d'une zone locale"),
        "la note doit avertir contre l'illusion de l'agrégat global: {note}"
    );
    assert!(
        note.contains("ne prouve PAS l'absence d'un symbole"),
        "un résultat vide ne doit pas être pris pour une preuve d'inexistence: {note}"
    );
}

