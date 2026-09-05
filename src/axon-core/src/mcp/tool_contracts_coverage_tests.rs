// RÉUTILISE : catalog::tools_catalog (le schéma servi) et
// tool_contracts::{DECLARED_DISPOSITIONS, parameter_dispositions} (les
// dispositions déclarées) — les deux sources que ce fichier confronte.
//
//! REQ-AXO-902583 — le dernier reste : le paramètre schématiquement VALIDE mais
//! INERTE.
//!
//! ## Pourquoi le chokepoint ne peut pas le voir
//!
//! `execute_tool_direct` compare ce que l'appelant a ENVOYÉ à ce que le schéma
//! ACCEPTE. Il ne voit pas ce que le handler a LU. Un paramètre bien orthographié,
//! accepté, et jeté en silence est donc invisible là où les trois autres causes
//! sont attrapées.
//!
//! Le nœud posait l'alternative : « une déclaration par outil, 114 fois, ou un
//! mécanisme qui l'extrait du code ». Ce fichier livre le mécanisme, et il est
//! honnête sur ce qu'il attrape :
//!
//! | garde | ce qu'elle prouve | ce qu'elle NE prouve PAS |
//! |---|---|---|
//! | `chaque_propriete_du_schema_est_LUE_quelque_part` | un paramètre déclaré au schéma et lu NULLE PART dans le crate est inerte par construction | qu'un paramètre lu dans une branche jamais atteinte a un effet |
//! | `une_disposition_declaree_correspond_au_schema_servi` | les déclarations ne dérivent pas du schéma | rien sur les outils non déclarés |
//! | `la_couverture_des_dispositions_ne_REGRESSE_pas` | le plancher de couverture monte, jamais l'inverse | que la couverture soit suffisante — elle ne l'est pas, et le chiffre le dit |
//!
//! La première garde est UNIDIRECTIONNELLE, et c'est dit plutôt que sous-entendu :
//! un nom courant (`id`, `mode`, `limit`) se trouve dans le crate quoi qu'il
//! arrive. Elle ferme la classe « déclaré, jamais lu » — pas la classe « lu, sans
//! effet », qui reste le travail de `DECLARED_DISPOSITIONS`.

use crate::mcp::catalog::tools_catalog;
use crate::mcp::tool_contracts::{
    inert_parameters_for_call, parameter_dispositions, DECLARED_DISPOSITIONS,
};
use serde_json::Value;

/// Plancher de couverture — le nombre d'outils portant des dispositions
/// déclarées. Il MONTE, jamais l'inverse : baisser ce chiffre pour faire passer
/// un test serait retirer un contrat servi à des locataires.
const PLANCHER_OUTILS_INSTRUMENTES: usize = 4;

/// Tous les `.rs` du crate, concaténés. Le scanner est volontairement grossier :
/// il cherche un littéral, pas une analyse de flot. Une analyse fine serait plus
/// juste et ne tiendrait pas dans un test ; celle-ci tient, et son unique
/// direction est sûre.
fn sources_du_crate() -> String {
    let racine = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = String::new();
    let mut pile = vec![racine];
    while let Some(dir) = pile.pop() {
        let Ok(entrees) = std::fs::read_dir(&dir) else { continue };
        for entree in entrees.flatten() {
            let chemin = entree.path();
            if chemin.is_dir() {
                pile.push(chemin);
                continue;
            }
            if chemin.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // `catalog.rs` est EXCLU : c'est la déclaration elle-même. L'y trouver
            // prouverait seulement qu'un paramètre est déclaré, ce qu'on sait déjà.
            if chemin.file_name().and_then(|f| f.to_str()) == Some("catalog.rs") {
                continue;
            }
            if let Ok(texte) = std::fs::read_to_string(&chemin) {
                out.push_str(&texte);
                out.push('\n');
            }
        }
    }
    out
}

/// `(nom d'outil, noms de propriétés)` tels que le catalogue les SERT.
///
/// Lu du catalogue construit, jamais d'une regex sur le source : une extraction
/// textuelle confond `"name": "query"` cité dans la description d'un outil avec
/// l'entrée de l'outil `query` — vérifié, elle le fait.
fn proprietes_par_outil() -> Vec<(String, Vec<String>)> {
    let catalogue = tools_catalog(true);
    let mut out = Vec::new();
    let Some(outils) = catalogue.get("tools").and_then(Value::as_array) else {
        return out;
    };
    for outil in outils {
        let Some(nom) = outil.get("name").and_then(Value::as_str) else { continue };
        let props = outil
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        out.push((nom.to_string(), props));
    }
    out
}

#[test]
fn chaque_propriete_du_schema_est_LUE_quelque_part() {
    let sources = sources_du_crate();
    let mut jamais_lues: Vec<String> = Vec::new();
    for (outil, props) in proprietes_par_outil() {
        for prop in props {
            if !sources.contains(&format!("\"{prop}\"")) {
                jamais_lues.push(format!("{outil}.{prop}"));
            }
        }
    }
    assert!(
        jamais_lues.is_empty(),
        "ces paramètres sont DÉCLARÉS au schéma et lus NULLE PART dans le crate — \
         un appelant les fournit, paie la réponse, et n'obtient aucun effet ni aucun \
         signal (REQ-AXO-902583) : {jamais_lues:?}"
    );
}

#[test]
fn MUTANT_le_scanner_sait_dire_NON() {
    // Sans ce contrôle, un scanner qui rendrait « trouvé » pour tout ferait passer
    // la garde ci-dessus quel que soit l'état du code. C'est le contrôle négatif :
    // un nom que personne n'écrit doit être introuvable.
    let sources = sources_du_crate();
    assert!(
        !sources.contains("\"zzz_parametre_qui_n_existe_nulle_part\""),
        "le scanner trouve un littéral inexistant : il ne peut rien réfuter"
    );
    // Et un nom qu'on sait présent doit l'être — sinon le scanner ne lit rien.
    assert!(
        sources.contains("\"sections\""),
        "le scanner ne trouve pas un littéral connu : il ne lit pas les sources"
    );
}

#[test]
fn une_disposition_declaree_correspond_au_schema_servi() {
    let catalogue = proprietes_par_outil();
    for (outil, declarations) in DECLARED_DISPOSITIONS {
        let Some((_, props)) = catalogue.iter().find(|(nom, _)| nom == outil) else {
            panic!(
                "`{outil}` porte des dispositions déclarées mais n'existe pas au catalogue — \
                 une déclaration orpheline ne protège rien et se lit comme une couverture"
            );
        };
        for declaration in *declarations {
            assert!(
                props.iter().any(|p| p == declaration.name),
                "`{outil}` déclare une disposition pour `{}`, absent de son schéma : la \
                 déclaration a dérivé, et le paramètre qu'elle décrit n'existe plus",
                declaration.name
            );
        }
    }
}

#[test]
fn la_couverture_des_dispositions_ne_REGRESSE_pas() {
    let instrumentes = DECLARED_DISPOSITIONS.len();
    assert!(
        instrumentes >= PLANCHER_OUTILS_INSTRUMENTES,
        "la couverture est tombée à {instrumentes} outil(s) instrumenté(s), sous le \
         plancher de {PLANCHER_OUTILS_INSTRUMENTES} : un contrat servi a été retiré"
    );
    // Le chiffre est PUBLIÉ, pas seulement gardé. « 2 sur 107 » est une dette qu'on
    // peut discuter ; une couverture tue se lit comme une couverture complète.
    let total = proprietes_par_outil().len();
    eprintln!(
        "REQ-AXO-902583 — dispositions déclarées : {instrumentes} outil(s) sur {total}. \
         Les autres rendent une liste vide, ce qui signifie « je ne sais pas », jamais \
         « rien à signaler »."
    );
    // Et l'invariant qui rend ce chiffre lisible : aucun outil n'est déclaré deux fois.
    let mut noms: Vec<&str> = DECLARED_DISPOSITIONS.iter().map(|(n, _)| *n).collect();
    noms.sort_unstable();
    let avant = noms.len();
    noms.dedup();
    assert_eq!(avant, noms.len(), "un outil est déclaré deux fois : {noms:?}");
}

#[test]
fn un_outil_NON_instrumente_repond_je_ne_sais_pas_et_non_rien_a_signaler() {
    // L'invariant que la surface repose sur : `None` (non instrumenté) et
    // `Some(&[])` (instrumenté, rien d'inerte) sont deux réponses différentes.
    // Les confondre est le défaut que ce REQ existe pour fermer.
    assert!(
        parameter_dispositions("soll_get").is_some(),
        "prealable : `soll_get` est instrumenté, sinon l'assertion suivante ne prouve rien"
    );
    assert!(
        parameter_dispositions("un_outil_qui_n_existe_pas").is_none(),
        "un outil inconnu doit rendre `None` — « je ne sais pas » — et non une liste vide"
    );
}

// ---------------------------------------------------------------------------------
// REQ-AXO-902583 — la variante `FieldNotOneOf`, et les deux outils qu'elle ouvre.
//
// Les deux variantes antérieures ne savaient pas dire « effectif SAUF si le champ
// prend telle valeur » — la forme d'un drapeau dont le DÉFAUT est actif. C'est
// pourtant le cas des deux paramètres les plus coûteux mesurés :
// `wait_for_semantic` fait DORMIR l'appel pour rien quand `semantic=off`, et
// `half_life_days` est lu puis jeté quand `include_decay=false`.
// ---------------------------------------------------------------------------------

#[test]
fn wait_for_semantic_est_INERTE_quand_le_plongement_est_desactive() {
    let inertes = inert_parameters_for_call(
        "retrieve_context",
        &serde_json::json!({
            "question": "pourquoi ?",
            "semantic": "off",
            "wait_for_semantic": 500
        }),
    );
    assert_eq!(
        inertes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["wait_for_semantic"],
        "sans plongement, l'attente est payée en latence et ne rachète rien : {inertes:?}"
    );
    let inerte = &inertes[0];
    assert!(
        inerte.reason.contains("off"),
        "la raison doit NOMMER la valeur reçue, sinon elle se lit comme de la documentation : {}",
        inerte.reason
    );
    assert!(
        !inerte.remedy.is_empty(),
        "un inerte sans remède laisse l'appelant corriger l'orthographe d'un mot juste"
    );
}

#[test]
fn le_MEME_appel_sans_semantic_off_ne_signale_RIEN() {
    // MUTANT — sans ce cas, la garde ci-dessus passerait aussi si tout
    // `wait_for_semantic` était déclaré inerte, ce qui serait le défaut symétrique :
    // une alarme permanente sur un paramètre qui marche.
    let inertes = inert_parameters_for_call(
        "retrieve_context",
        &serde_json::json!({ "question": "pourquoi ?", "wait_for_semantic": 500 }),
    );
    assert!(
        inertes.is_empty(),
        "`semantic` absent = plongement actif : l'attente sert, rien à signaler : {inertes:?}"
    );
}

#[test]
fn half_life_days_est_INERTE_seulement_sous_include_decay_false() {
    // `include_decay` vaut TRUE par défaut : c'est un `false` EXPLICITE qui
    // neutralise. `FieldUnset` aurait la polarité inverse et aurait signalé
    // l'inverse exact — d'où la nouvelle variante.
    let neutralise = inert_parameters_for_call(
        "soll_work_plan",
        &serde_json::json!({ "include_decay": false, "half_life_days": 14 }),
    );
    assert_eq!(
        neutralise.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["half_life_days"],
        "`decay_factor_for_node` rend 1.0 dès la première ligne : la demi-vie est jetée"
    );

    let absent = inert_parameters_for_call(
        "soll_work_plan",
        &serde_json::json!({ "half_life_days": 14 }),
    );
    assert!(
        absent.is_empty(),
        "`include_decay` absent vaut TRUE : la demi-vie compte, rien à signaler : {absent:?}"
    );

    let explicite = inert_parameters_for_call(
        "soll_work_plan",
        &serde_json::json!({ "include_decay": true, "half_life_days": 14 }),
    );
    assert!(explicite.is_empty(), "`include_decay=true` : idem : {explicite:?}");
}

