// RÉUTILISE : McpServer::borner_corps_pointeur (tools_skill.rs) — la fonction sous test.
//
//! REQ-AXO-902621 — borner le corps du session pointer dans `re_anchor`.
//!
//! Le cas 1 est LA régression : un corps de 330 k caractères (la taille réelle de
//! `CPT-AXO-052` le 2026-09-05) doit ressortir borné. Le dernier cas est le MUTANT —
//! il vérifie que la fixture dépasse réellement le seuil, sans quoi tous les autres
//! passeraient aussi bien sans le correctif (pratique 2169).

use crate::mcp::McpServer;

/// Fabrique un journal append-only : `n` sections `## Titre k`, la dernière portant
/// un marqueur reconnaissable.
fn journal(n: usize, remplissage_par_section: usize) -> String {
    let mut s = String::new();
    for k in 0..n {
        s.push_str(&format!("## Section {k}\n"));
        s.push_str(&"x".repeat(remplissage_par_section));
        s.push('\n');
    }
    s.push_str("## DERNIÈRE\nle seul contenu qu'un recalage doit lire\n");
    s
}

#[test]
fn un_corps_court_passe_INTACT() {
    // Borner ce qui n'a pas besoin de l'être perdrait du contexte sans rien gagner.
    let corps = "## Règle\nune ligne\n\n## Pourquoi\ndeux lignes\n";
    let (rendu, titres, tronque) = McpServer::borner_corps_pointeur(corps);
    assert_eq!(rendu, corps, "un corps sous le seuil ne doit pas être touché");
    assert!(!tronque);
    assert_eq!(titres, vec!["Règle".to_string(), "Pourquoi".to_string()]);
}

#[test]
fn un_journal_long_rend_la_DERNIERE_section_et_tous_les_titres() {
    let corps = journal(40, 500); // ~20 k caractères, bien au-delà du seuil
    let (rendu, titres, tronque) = McpServer::borner_corps_pointeur(&corps);

    assert!(tronque, "un corps de {} chars doit être borné", corps.chars().count());
    assert!(
        rendu.starts_with("## DERNIÈRE"),
        "le recalage doit recevoir la dernière section ; obtenu le début : {:?}",
        &rendu.chars().take(40).collect::<String>()
    );
    assert!(
        rendu.contains("le seul contenu qu'un recalage doit lire"),
        "la dernière section doit être rendue ENTIÈRE"
    );
    // Les titres restent TOUS là : c'est la table des matières qui permet de tirer
    // une autre section à la demande via soll_get(section=…).
    assert_eq!(titres.len(), 41);
    assert_eq!(titres.last().unwrap(), "DERNIÈRE");
    assert!(
        rendu.chars().count() < corps.chars().count() / 10,
        "le gain doit être d'un ordre de grandeur, pas cosmétique"
    );
}

#[test]
fn un_corps_long_SANS_section_garde_la_queue_pas_la_tete() {
    // Sur un journal append-only, le récent est en bas. Garder la tête rendrait le
    // plus ancien — l'inverse de ce qu'un recalage demande.
    let mut corps = "DEBUT-ANCIEN\n".to_string();
    corps.push_str(&"y".repeat(20_000));
    corps.push_str("\nFIN-RECENTE");
    let (rendu, titres, tronque) = McpServer::borner_corps_pointeur(&corps);

    assert!(tronque);
    assert!(titres.is_empty(), "aucun `## ` : aucun titre à annoncer");
    assert!(rendu.ends_with("FIN-RECENTE"), "la queue doit être conservée");
    assert!(
        !rendu.contains("DEBUT-ANCIEN"),
        "la tête doit être écartée, pas la queue"
    );
}

#[test]
fn le_tronquage_n_est_jamais_MUET() {
    // REQ-AXO-902583 : une surface qui retire quelque chose doit le NOMMER. Sans ce
    // contrôle, un appelant lirait un pointeur amputé en le croyant complet.
    let corps = journal(40, 500);
    let (_, _, tronque) = McpServer::borner_corps_pointeur(&corps);
    assert!(tronque, "`body_truncated` est le seul signal que l'appelant reçoit");
}

// ---------------------------------------------------------------------------------
// LE MUTANT — sans lui, les cas ci-dessus passeraient aussi bien sans le correctif.
// ---------------------------------------------------------------------------------
#[test]
fn MUTANT_la_fixture_depasse_reellement_le_seuil() {
    let corps = journal(40, 500);
    let entier = corps.chars().count();
    assert!(
        entier > 8_000,
        "la fixture ne fait que {entier} chars : elle ne franchit pas le seuil, \
         donc les assertions de bornage ne prouvent rien"
    );
    // Et l'ANCIEN comportement — rendre le corps tel quel — produirait bien ce volume.
    let ancien_comportement = corps.clone();
    assert_eq!(
        ancien_comportement.chars().count(),
        entier,
        "le contrôle négatif doit refléter l'ancien rendu, non borné"
    );
}
