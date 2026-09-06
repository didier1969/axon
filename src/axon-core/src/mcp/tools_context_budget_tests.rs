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

/// Aucune bande porteuse — le cas des routes `exact_lookup` / `hybrid` /
/// `soll_hybrid`, dont la réponse vit dans `direct_evidence` et
/// `relevant_soll_entities`, tous deux déjà hors de portée de la coupe.
const AUCUNE_BANDE_PORTEUSE: Option<&str> = None;

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

/// La grandeur que la borne borne — voir `McpServer::poids_du_contenu`.
///
/// Peser ici le paquet ENTIER mesurerait autre chose que ce qui est testé : le
/// mutant serait alors faux d'exactement le poids de la machinerie de diagnostic,
/// et il déclarerait « le plafond reste une estimation » sur une borne correcte.
fn jetons(v: &Value) -> usize {
    McpServer::poids_du_contenu(v)
}

/// Le poids de l'enveloppe ENTIÈRE — la grandeur que la borne pesait À TORT
/// jusqu'au 2026-09-06. Ne sert qu'à prouver qu'une fixture distingue réellement
/// les deux pesées.
fn jetons_enveloppe(v: &Value) -> usize {
    serde_json::to_string(v).unwrap_or_default().chars().count() / 4 + 1
}

#[test]
fn un_paquet_SOUS_le_budget_n_est_pas_touche() {
    let mut p = paquet(2, 10);
    let avant = p.clone();
    let omises = McpServer::borner_paquet_au_budget(&mut p, 100_000, AUCUNE_BANDE_PORTEUSE);
    assert!(omises.is_empty(), "aucune coupe attendue : {omises:?}");
    assert_eq!(p, avant, "borner ce qui tient perdrait du contexte sans rien gagner");
}

#[test]
fn au_dessus_du_budget_les_bandes_PERIPHERIQUES_partent_les_premieres() {
    let mut p = paquet(20, 400);
    let budget = jetons_cible(&p);
    let omises = McpServer::borner_paquet_au_budget(&mut p, budget, AUCUNE_BANDE_PORTEUSE);

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
    McpServer::borner_paquet_au_budget(&mut p, 1, AUCUNE_BANDE_PORTEUSE);
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
    McpServer::borner_paquet_au_budget(&mut p, 1, AUCUNE_BANDE_PORTEUSE);
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
fn la_MACHINERIE_de_diagnostic_n_est_PAS_facturee_au_budget() {
    // LE défaut que la borne portait pendant vingt-deux commits, en un test.
    //
    // Un paquet dont le CONTENU tient largement, mais dont les diagnostics pèsent à
    // eux seuls plus que le budget. Peser l'enveloppe entière vidait alors TOUTES
    // les bandes de contenu pour tenir un plafond que le contenu n'avait jamais
    // franchi — et rendait une enveloppe conforme et inutile, le mode d'échec exact
    // que ce REQ prétendait fermer.
    //
    // Mesuré en vrai le 2026-09-06 : contenu 399 jetons, enveloppe 1 242, budget
    // 900 — deux bandes de contenu supprimées pour ~400 jetons de chronos et de
    // compteurs que l'appelant n'a pas demandés et ne peut pas refuser.
    let mut p = paquet(1, 40);
    p.as_object_mut().unwrap().insert(
        "retrieval_diagnostics".to_string(),
        json!({ "trace": "d".repeat(20_000) }),
    );
    let budget = jetons(&p) * 2; // le contenu tient au large
    assert!(
        jetons_enveloppe(&p) > budget,
        "la fixture doit déborder par sa MACHINERIE ({} jetons d'enveloppe pour un \
         budget de {budget}), sinon elle ne distingue pas les deux pesées",
        jetons_enveloppe(&p)
    );

    let omises = McpServer::borner_paquet_au_budget(&mut p, budget, AUCUNE_BANDE_PORTEUSE);

    assert!(
        omises.is_empty(),
        "le contenu tient dans le budget : aucune bande ne doit partir. C'est la \
         machinerie qui déborde, et l'appelant ne l'a pas demandée — {omises:?}"
    );
    assert_eq!(
        p["supporting_chunks"].as_array().map(|a| a.len()),
        Some(1),
        "la bande de contenu doit survivre INTACTE"
    );
    assert_eq!(
        p["structural_neighbors"].as_array().map(|a| a.len()),
        Some(1),
        "la bande la plus périphérique aussi : elle n'a pas causé le dépassement"
    );
}

#[test]
fn la_bande_qui_PORTE_la_reponse_de_la_route_n_est_pas_coupee() {
    // Question `impact` — « qu'est-ce qui casse si X change ? ». La réponse EST la
    // liste des voisins. La couper en gardant leur contexte rend le contexte de la
    // réponse, sans la réponse : conforme au budget, et inutile.
    //
    // Mesuré le 2026-09-06 : les 5 voisins (258 jetons) partaient les PREMIERS, puis
    // `supporting_code_context` (889) devait partir aussi — alors que ce second
    // retrait suffisait seul (1 747 − 889 = 858 sous un budget de 1 200). Les cinq
    // voisins étaient sacrifiés pour rien.
    //
    // L'ordre « du périphérique au central » reste le bon principe ; c'est sa table
    // STATIQUE qui était fausse, la centralité dépendant de la question posée.
    let mut p = paquet(6, 300);
    let budget = jetons(&p) * 3 / 4;

    let omises =
        McpServer::borner_paquet_au_budget(&mut p, budget, Some("structural_neighbors"));

    assert!(!omises.is_empty(), "le paquet dépasse : une coupe est attendue");
    assert_eq!(
        p["structural_neighbors"].as_array().map(|a| a.len()),
        Some(6),
        "la bande porteuse de la route doit survivre INTACTE ; retirées : {omises:?}"
    );
    assert!(
        !omises
            .iter()
            .any(|b| b.get("band").and_then(Value::as_str) == Some("structural_neighbors")),
        "la bande porteuse ne doit même pas figurer parmi les retirées : {omises:?}"
    );
    // Et la borne reste DURE : l'exemption déplace la coupe, elle ne l'annule pas.
    assert!(
        jetons(&p) <= budget,
        "exempter une bande ne dispense pas de tenir le budget : {} jetons pour {budget}",
        jetons(&p)
    );
}

#[test]
fn MUTANT_sans_exemption_la_bande_porteuse_serait_bien_coupee() {
    // Sans ce contrôle, le test ci-dessus passerait aussi sur une fixture qui ne
    // franchit jamais le plafond sur cette bande — il ne prouverait rien.
    let mut p = paquet(6, 300);
    let budget = jetons(&p) * 3 / 4;
    let omises = McpServer::borner_paquet_au_budget(&mut p, budget, AUCUNE_BANDE_PORTEUSE);
    assert!(
        omises
            .iter()
            .any(|b| b.get("band").and_then(Value::as_str) == Some("structural_neighbors")),
        "sans exemption la bande DOIT être coupée, sinon l'exemption ne protège rien : \
         {omises:?}"
    );
}

#[test]
fn un_budget_ZERO_ne_coupe_RIEN() {
    // `token_budget=0` signifie « pas de budget demandé », pas « ne rends rien ».
    // Le traiter comme un plafond nul viderait toute réponse sans budget explicite.
    let mut p = paquet(20, 400);
    let avant = p.clone();
    let omises = McpServer::borner_paquet_au_budget(&mut p, 0, AUCUNE_BANDE_PORTEUSE);
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
    McpServer::borner_paquet_au_budget(&mut coupe, budget, AUCUNE_BANDE_PORTEUSE);
    assert!(
        jetons(&coupe) <= budget,
        "après coupe le paquet pèse encore {} jetons pour un budget de {budget} : \
         le plafond reste une estimation, pas une borne",
        jetons(&coupe)
    );
}
