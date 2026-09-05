// RÉUTILISE : McpServer::borner_lignes_sql (tools_system.rs) — la fonction sous test.
//
//! REQ-AXO-902621 (suite) — borner `sql`, et le prouver falsifiable.
//!
//! ## Pourquoi `sql` et pas `batch`
//!
//! Le plan désignait `batch` comme le prochain chantier de bornage. La mesure du
//! 2026-09-05 sur `axon.mcp_call_stat` le RÉFUTE :
//!
//! | outil | appels | octets rendus | pic sur UN appel |
//! |---|---|---|---|
//! | `sql` | 135 507 | **515 Mo** | **13,7 Mo** |
//! | `status` | 8 046 | 17 Mo | 15 Ko |
//! | `batch` | 482 | **38 Ko** | 14 Ko |
//!
//! `batch` n'est pas un problème ; `sql` l'est de deux ordres de grandeur. Une
//! réponse de 13,7 Mo est REFUSÉE par le client avant d'être lue : l'appelant paie
//! le calcul, le transport et l'attente, et n'obtient RIEN. Borner domine
//! strictement.
//!
//! Le dernier cas est le MUTANT : il vérifie que la fixture dépasse réellement le
//! seuil et que le comportement ANTÉRIEUR — rendre la sortie telle quelle — produit
//! bien le volume que la borne supprime. Sans lui, tous les autres cas passeraient
//! aussi sans le correctif (pratique 2169).

use crate::mcp::McpServer;

/// `n` lignes JSON d'environ `poids` caractères chacune.
fn lignes(n: usize, poids: usize) -> String {
    let cellule = "x".repeat(poids);
    let corps: Vec<String> = (0..n)
        .map(|i| format!("[\"{i}\",\"{cellule}\"]"))
        .collect();
    format!("[{}]", corps.join(","))
}

#[test]
fn une_sortie_SOUS_le_seuil_passe_intacte() {
    // Borner ce qui n'a pas besoin de l'être ferait payer une pagination à
    // 135 000 appels dont la quasi-totalité tient largement.
    let petite = lignes(3, 20);
    let (rendu, rendues, tronque) = McpServer::borner_lignes_sql(&petite, 60_000);
    assert_eq!(rendu, petite, "une sortie courte ne doit pas être touchée");
    assert!(!tronque);
    assert_eq!(rendues, 0, "0 = « la borne n'a pas mordu », pas « aucune ligne »");
}

#[test]
fn une_sortie_AU_DESSUS_du_seuil_rend_des_lignes_ENTIERES_et_le_dit() {
    let grosse = lignes(500, 200); // ~100 k caractères
    let (rendu, rendues, tronque) = McpServer::borner_lignes_sql(&grosse, 10_000);

    assert!(tronque);
    assert!(rendues > 0 && rendues < 500, "coupe partielle attendue, obtenu {rendues}");
    // La coupe porte sur des LIGNES : le JSON rendu doit rester parsable, sinon un
    // appelant programmatique reçoit un tableau cassé — pire qu'une réponse longue.
    let json_seul = rendu.split("\n\n").next().unwrap();
    let reparse: Vec<serde_json::Value> =
        serde_json::from_str(json_seul).expect("le JSON rendu doit rester valide");
    assert_eq!(reparse.len(), rendues);
    // Et le rendu doit DIRE ce qu'il a fait, avec le total et la suite.
    assert!(rendu.contains("ok_truncated"), "le statut doit être dans le texte : {rendu:.200}");
    assert!(rendu.contains("sur 500"), "le TOTAL doit être nommé, pas seulement le rendu");
    assert!(rendu.contains("OFFSET"), "la suite doit être exploitable, pas seulement annoncée");
}

#[test]
fn une_ligne_ENORME_est_rendue_plutot_que_zero() {
    // Rendre zéro ligne se lirait comme un résultat vide — exactement la confusion
    // que `ok_empty` existe pour éviter (REQ-AXO-902583).
    let enorme = lignes(2, 50_000);
    let (rendu, rendues, tronque) = McpServer::borner_lignes_sql(&enorme, 1_000);
    assert!(tronque);
    assert_eq!(rendues, 1, "au moins une ligne, même au-delà du seuil");
    let json_seul = rendu.split("\n\n").next().unwrap();
    let reparse: Vec<serde_json::Value> = serde_json::from_str(json_seul).expect("JSON valide");
    assert_eq!(reparse.len(), 1);
}

#[test]
fn une_sortie_NON_delimitable_est_coupee_a_plat_et_annoncee_comme_telle() {
    // Une sortie que `RawValue` ne sait pas découper ne doit pas être rendue comme
    // un tableau : la couper aux caractères produirait du JSON invalide présenté
    // comme valide. On l'annonce plate et incomplète.
    let brut = "x".repeat(5_000);
    let (rendu, rendues, tronque) = McpServer::borner_lignes_sql(&brut, 1_000);
    assert!(tronque);
    assert_eq!(rendues, 0);
    assert!(
        rendu.contains("n'a pas pu être délimitée") && rendu.contains("incomplet"),
        "l'appelant doit savoir que ce qu'il lit n'est pas du JSON valide : {rendu:.300}"
    );
}

// ---------------------------------------------------------------------------------
// LE MUTANT — sans lui, les cas ci-dessus passeraient aussi sans le correctif.
// ---------------------------------------------------------------------------------
#[test]
fn MUTANT_la_fixture_reproduit_bien_le_volume_que_la_borne_supprime() {
    let grosse = lignes(500, 200);
    let entier = grosse.chars().count();
    assert!(
        entier > 60_000,
        "la fixture ne fait que {entier} caractères : elle ne franchit pas le seuil réel, \
         donc les assertions de bornage ne prouvent rien"
    );
    // L'ANCIEN comportement — rendre `result` tel quel — produirait bien ce volume.
    let (borne, _, _) = McpServer::borner_lignes_sql(&grosse, 10_000);
    assert!(
        borne.chars().count() < entier / 5,
        "le gain doit être d'un ordre de grandeur, pas cosmétique : {} contre {entier}",
        borne.chars().count()
    );
}
