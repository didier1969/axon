// RÉUTILISE : McpServer::borner_paquet_au_budget (tools_context.rs) — la fonction
// sous test. La matrice de coupe est pure : ni base, ni plongement, ni runtime.
//
//! REQ-AXO-902596 — `token_budget` comme BORNE DURE, pas comme estimation imprimée
//! à côté.
//!
//! Voix client KKI : le budget ne pilotait que la sélection des chunks
//! (`consumed_tokens + estimated > token_budget / 2`) ; le paquet assemblé pouvait
//! ensuite le dépasser, et l'enveloppe se contentait de le CONSTATER dans
//! `token_budget_estimate`. Un plafond qu'on mesure après coup n'est pas un plafond.
//!
//! Ce que ces tests NE couvrent PAS, dit plutôt que sous-entendu : le critère 2 du
//! REQ — « un identifiant exact présent dans la question garde priorité sur un
//! voisin sémantique » — est une question de CLASSEMENT, pas de bornage. Elle
//! appartient à la même famille que l'ancre déterministe de `REQ-AXO-902602` et
//! n'est pas livrée ici.

use crate::mcp::McpServer;
use serde_json::{json, Value};

/// Paquet synthétique : chaque bande porte `n` éléments d'environ `poids` caractères.
fn paquet(n: usize, poids: usize) -> Value {
    let item = |i: usize| json!({ "id": i, "corps": "x".repeat(poids) });
    let bande = |n: usize| (0..n).map(item).collect::<Vec<_>>();
    json!({
        "answer_sketch": "la réponse",
        "direct_evidence": bande(n),
        "supporting_chunks": bande(n),
        "structural_neighbors": bande(n),
        "supporting_docs": bande(n),
        "supporting_guidelines": bande(n),
        "supporting_code_context": bande(n),
        "explicit_soll_anchors": { "requested": ["REQ-AXO-902596"] },
        "token_budget_estimate": { "requested_budget": 0 }
    })
}

fn jetons(v: &Value) -> usize {
    serde_json::to_string(v).unwrap_or_default().chars().count() / 4 + 1
}

#[test]
fn un_paquet_SOUS_le_budget_n_est_pas_touche() {
    let mut p = paquet(2, 10);
    let avant = p.clone();
    let omises = McpServer::borner_paquet_au_budget(&mut p, 100_000);
    assert!(omises.is_empty(), "aucune coupe attendue : {omises:?}");
    assert_eq!(p, avant, "borner ce qui tient perdrait du contexte sans rien gagner");
}

#[test]
fn au_dessus_du_budget_les_bandes_PERIPHERIQUES_partent_les_premieres() {
    let mut p = paquet(20, 400);
    let budget = jetons_cible(&p);
    let omises = McpServer::borner_paquet_au_budget(&mut p, budget);

    assert!(!omises.is_empty(), "le paquet dépasse : une coupe est attendue");
    let ordre: Vec<&str> = omises
        .iter()
        .filter_map(|b| b.get("band").and_then(Value::as_str))
        .collect();
    assert_eq!(
        ordre.first(),
        Some(&"structural_neighbors"),
        "la bande la plus périphérique doit partir la PREMIÈRE ; obtenu {ordre:?}"
    );
    // Chaque bande retirée est NOMMÉE avec son compte — jamais un effacement muet.
    for b in &omises {
        assert!(
            b.get("items_omitted").and_then(Value::as_u64).unwrap_or(0) > 0,
            "une bande retirée sans compte ne dit rien : {b}"
        );
    }
}

/// Un budget volontairement placé sous le poids du paquet, mais au-dessus du noyau
/// intouchable — pour que la coupe morde sans être obligée de tout retirer.
fn jetons_cible(p: &Value) -> usize {
    jetons(p) / 2
}

#[test]
fn le_NOYAU_n_est_JAMAIS_coupe_meme_sous_un_budget_absurde() {
    // LE cas qui compte : couper la réponse ou les ancres nommées pour tenir un
    // budget rendrait une enveloppe conforme et INUTILE.
    let mut p = paquet(20, 400);
    McpServer::borner_paquet_au_budget(&mut p, 1);
    assert_eq!(p["answer_sketch"], json!("la réponse"), "la réponse survit");
    assert_eq!(
        p["direct_evidence"].as_array().map(|a| a.len()),
        Some(20),
        "ce qui fonde la réponse survit"
    );
    assert_eq!(
        p["explicit_soll_anchors"]["requested"],
        json!(["REQ-AXO-902596"]),
        "ce que l'appelant a NOMMÉ survit"
    );
}

#[test]
fn une_bande_coupee_reste_PRESENTE_et_vide() {
    // « retiré faute de place » et « rien trouvé » ne doivent pas se confondre :
    // supprimer la clé ferait lire la seconde à la place de la première.
    let mut p = paquet(20, 400);
    McpServer::borner_paquet_au_budget(&mut p, 1);
    assert_eq!(
        p["structural_neighbors"], json!([]),
        "la bande coupée doit rester présente et VIDE, pas disparaître"
    );
    assert!(
        p.get("structural_neighbors").is_some(),
        "la clé doit exister pour que l'absence soit lisible"
    );
}

#[test]
fn un_budget_ZERO_ne_coupe_RIEN() {
    // `token_budget=0` signifie « pas de budget demandé », pas « ne rends rien ».
    // Le traiter comme un plafond nul viderait toute réponse sans budget explicite.
    let mut p = paquet(20, 400);
    let avant = p.clone();
    let omises = McpServer::borner_paquet_au_budget(&mut p, 0);
    assert!(omises.is_empty());
    assert_eq!(p, avant);
}

// ---------------------------------------------------------------------------------
// LE MUTANT — sans lui, les cas ci-dessus passeraient sans le correctif.
// ---------------------------------------------------------------------------------
#[test]
fn MUTANT_le_paquet_de_fixture_depasse_REELLEMENT_le_budget_teste() {
    let p = paquet(20, 400);
    let poids = jetons(&p);
    let budget = jetons_cible(&p);
    assert!(
        poids > budget,
        "la fixture pèse {poids} jetons pour un budget de {budget} : elle ne franchit pas \
         le plafond, donc les assertions de coupe ne prouvent rien"
    );
    // Et la coupe doit ramener SOUS le budget, sinon la borne n'est pas dure.
    let mut coupe = p.clone();
    McpServer::borner_paquet_au_budget(&mut coupe, budget);
    assert!(
        jetons(&coupe) <= budget,
        "après coupe le paquet pèse encore {} jetons pour un budget de {budget} : \
         le plafond reste une estimation, pas une borne",
        jetons(&coupe)
    );
}
