// Copyright (c) Didier Stadelmann. All rights reserved.

//! Qualification TDD de REQ-AXO-902491:
//! `rescan_project` rend status=ok alors que le wipe de cache a ÉCHOUÉ.
//!
//! Critères d'acceptation :
//! 1. `status` doit refléter l'opération entière. Un wipe échoué en mode `full` ou `targeted`
//!    ⇒ `status="partial"`, JAMAIS `ok`.
//! 2. Le texte doit dire la CONSÉQUENCE, pas seulement l'erreur PG :
//!    « le cache n'a pas été effacé : l'indexeur considérera ces fichiers comme inchangés et ne les refera pas. Réessayez. »
//! 3. Recommencer côté serveur avant de rendre la main (3 tentatives pour résorber les courses transitoires).
//! 4. En régime nominal où le wipe réussit, le statut reste "ok".

use crate::mcp::tools_system::rescan_compute_overall_status;
use crate::mcp::tools_system::rescan_format_cache_wipe_failure;

#[test]
fn c1_failed_full_wipe_never_returns_status_ok() {
    // Repro mesuré le 2026-08-26 sur ELE :
    // cache_invalidation = "wipe_failed: Writer Error: 55000: attempted to delete invisible tuple..."
    let cache_invalidation =
        "wipe_failed: Writer Error: 55000: attempted to delete invisible tuple";
    let notify_outcome = "enrolled:867";

    let status = rescan_compute_overall_status(cache_invalidation, notify_outcome);
    assert_ne!(
        status, "ok",
        "un wipe échoué ne doit JAMAIS rendre status=ok"
    );
    assert_eq!(
        status, "partial",
        "un wipe échoué doit rendre status=partial"
    );
}

#[test]
fn c2_failed_targeted_wipe_returns_partial() {
    let cache_invalidation = "targeted_wipe_failed: database lock timeout";
    let notify_outcome = "enrolled:3";

    let status = rescan_compute_overall_status(cache_invalidation, notify_outcome);
    assert_eq!(status, "partial");
}

#[test]
fn c3_failed_delta_reconcile_returns_partial() {
    let cache_invalidation = "delta_reconcile_failed: deadlock detected";
    let notify_outcome = "enrolled:12";

    let status = rescan_compute_overall_status(cache_invalidation, notify_outcome);
    assert_eq!(status, "partial");
}

#[test]
fn c4_nominal_wipe_returns_ok() {
    let cache_invalidation = "wiped by project_code (full mode) + indexer dedup-cache invalidated";
    let notify_outcome = "enrolled:867";

    let status = rescan_compute_overall_status(cache_invalidation, notify_outcome);
    assert_eq!(status, "ok");
}

#[test]
fn c5_refused_notify_returns_refused() {
    let cache_invalidation = "wiped by project_code (full mode)";
    let notify_outcome = "refused:path is excluded from the watch root";

    let status = rescan_compute_overall_status(cache_invalidation, notify_outcome);
    assert_eq!(status, "refused");
}

#[test]
fn c6_wipe_failure_message_explains_consequences_to_caller() {
    let raw_pg_error = "55000: attempted to delete invisible tuple";
    let formatted = rescan_format_cache_wipe_failure("wipe_failed", raw_pg_error);

    // Doit contenir l'erreur d'origine
    assert!(formatted.contains(raw_pg_error));
    // Doit expliquer la conséquence : cache non effacé
    assert!(
        formatted.contains("cache n'a pas été effacé")
            || formatted.contains("cache n'a pas ete efface"),
        "doit mentionner que le cache n'a pas été effacé: {formatted}"
    );
    // Doit mentionner que l'indexeur considérera les fichiers comme inchangés
    assert!(
        formatted.contains("inchangés") || formatted.contains("inchanges"),
        "doit mentionner que les fichiers apparaîtront inchangés: {formatted}"
    );
    // Doit conseiller de réessayer
    assert!(
        formatted.contains("éessayez") || formatted.contains("eessayez"),
        "doit conseiller de réessayer: {formatted}"
    );
}
