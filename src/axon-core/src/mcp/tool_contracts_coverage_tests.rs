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
const PLANCHER_OUTILS_INSTRUMENTES: usize = 5;

/// Second plancher — le nombre de paramètres RÉELLEMENT examinés.
///
/// Le plancher d'outils seul devient gamable dès que `unexamined` existe : un outil
/// déclaré entièrement non examiné ajoute 1 au compte d'outils et 0 à l'honnêteté.
/// Les deux planchers doivent donc vivre ensemble, et celui-ci est le seul qui
/// mesure du travail de lecture.
/// s146 : 13 → 29. Les 16 ajoutés sont les paramètres de `soll_manager`
/// (`action`, `entity`, et les 14 champs de `data`), tous LUS branche par branche
/// dans `tools_soll/manager.rs` — aucun n'est passé en `unexamined` pour faire
/// monter le chiffre.
const PLANCHER_PARAMETRES_EXAMINES: usize = 29;

/// Les paramètres dont un handler a été lu, tous outils confondus.
fn parametres_examines() -> usize {
    DECLARED_DISPOSITIONS
        .iter()
        .map(|(_, d)| d.declared.len())
        .sum()
}

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
        // REQ-AXO-902583 (s146) — CHEMINS (`data.section`), pas clés plates : la
        // table déclare désormais des paramètres imbriqués, et les confronter à une
        // liste de premier niveau les ferait tous passer pour des fantômes.
        let props = outil
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .map(crate::mcp::tool_contracts::chemins_de_proprietes_du_schema)
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
            // REQ-AXO-902583 (s146) — le scanner cherche le NOM du champ, pas son
            // chemin : le code lit `data.get("section")`, jamais le littéral
            // `"data.section"`. Prendre le chemin entier ferait rougir la garde sur
            // chaque paramètre imbriqué, pour une raison qui n'est pas la sienne.
            let feuille = prop.rsplit('.').next().unwrap_or(prop.as_str());
            if !sources.contains(&format!("\"{feuille}\"")) {
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
    for (outil, dispositions) in DECLARED_DISPOSITIONS {
        let Some((_, props)) = catalogue.iter().find(|(nom, _)| nom == outil) else {
            panic!(
                "`{outil}` porte des dispositions déclarées mais n'existe pas au catalogue — \
                 une déclaration orpheline ne protège rien et se lit comme une couverture"
            );
        };
        for declaration in dispositions.declared {
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
    let examines = parametres_examines();
    assert!(
        examines >= PLANCHER_PARAMETRES_EXAMINES,
        "les paramètres examinés sont tombés à {examines}, sous le plancher de \
         {PLANCHER_PARAMETRES_EXAMINES} : une lecture de handler a été remplacée par un \
         `unexamined`, ce qui fait baisser la connaissance sans faire baisser le compte d'outils"
    );
    // Les chiffres sont PUBLIÉS, pas seulement gardés. « 2 sur 107 » est une dette qu'on
    // peut discuter ; une couverture tue se lit comme une couverture complète. DEUX
    // chiffres, parce qu'un outil déclaré n'est pas un outil lu.
    let total = proprietes_par_outil().len();
    let non_examines: usize = DECLARED_DISPOSITIONS
        .iter()
        .map(|(_, d)| d.unexamined.len())
        .sum();
    eprintln!(
        "REQ-AXO-902583 — dispositions déclarées : {instrumentes} outil(s) sur {total} · \
         {examines} paramètre(s) examiné(s), {non_examines} encore non lu(s). Les outils \
         absents rendent une liste vide, ce qui signifie « je ne sais pas », jamais « rien \
         à signaler ». REQ-AXO-902583 (s146) — les objets imbriqués SONT désormais descendus \
         (`data.section`) : l'invariant compare des chemins de feuilles, pas les seules clés \
         de premier niveau. Les listes (`items`) ne le sont pas."
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

// ---------------------------------------------------------------------------------
// LES MUTANTS du contrôle de couverture — REQ-AXO-902583.
//
// Ils éprouvent `ecart_de_couverture`, LA fonction que le gardien du dépôt
// (`runtime_surface::toute_disposition_declaree_couvre_exactement…`) appelle. Un
// mutant qui exercerait une copie ne dirait rien du code qui garde réellement.
// ---------------------------------------------------------------------------------

/// Sans lui, un vérificateur qui rendrait toujours `None` laisserait passer les
/// quatre tables réelles, et tous les tests ci-dessus resteraient verts.
#[test]
fn MUTANT_le_controle_sait_REFUSER_une_table_incomplete() {
    use crate::mcp::tool_contracts::{ecart_de_couverture, ParameterDeclaration,
                                     ParameterDisposition, ToolDispositions};

    const LUE: &[ParameterDeclaration] = &[ParameterDeclaration {
        name: "question",
        disposition: ParameterDisposition::Honoured,
    }];
    let schema = vec!["question".to_string(), "top_k".to_string()];

    // La table RATE `top_k` : c'est exactement la dérive que 5c7218cd avait laissée
    // passer sur `retrieve_context` (3 déclarés sur 10 servis).
    let incomplete = ToolDispositions { declared: LUE, unexamined: &[] };
    let ecart = ecart_de_couverture(&schema, &incomplete)
        .expect("une table qui rate une propriété DOIT être refusée");
    assert!(ecart.contains("top_k"), "l'écart doit NOMMER ce qui manque : {ecart}");

    // La même table, complétée par un `unexamined` honnête, passe.
    let complete = ToolDispositions { declared: LUE, unexamined: &["top_k"] };
    assert_eq!(ecart_de_couverture(&schema, &complete), None);

    // Et un paramètre déclaré LU et non lu à la fois est refusé : sans ce cas, on
    // pourrait satisfaire l'invariant en listant tout des deux côtés.
    let contradictoire = ToolDispositions { declared: LUE, unexamined: &["question", "top_k"] };
    assert!(ecart_de_couverture(&schema, &contradictoire).is_some());
}

/// Ferme le vecteur de triche que `unexamined` ouvre : gonfler le compte d'outils
/// instrumentés sans lire une seule ligne de handler.
#[test]
fn MUTANT_un_outil_entierement_unexamined_ne_compte_pas_comme_examine() {
    use crate::mcp::tool_contracts::ToolDispositions;

    let vitrine = ToolDispositions { declared: &[], unexamined: &["a", "b", "c"] };
    assert_eq!(
        vitrine.declared.len(),
        0,
        "un outil sans aucune lecture de handler doit compter ZÉRO paramètre examiné, \
         quel que soit le nombre de propriétés qu'il énumère"
    );
    // Le contrôle de couverture, lui, l'accepte — c'est voulu : déclarer qu'on n'a
    // pas lu est honnête. C'est le PLANCHER des paramètres examinés qui empêche que
    // cette honnêteté serve à faire monter un chiffre.
    assert_eq!(
        crate::mcp::tool_contracts::ecart_de_couverture(
            &["a".to_string(), "b".to_string(), "c".to_string()],
            &vitrine
        ),
        None
    );
}

// ---------------------------------------------------------------------------------
// REQ-AXO-902583 (s146) — LA DESCENTE dans les objets imbriqués.
//
// L'invariant ne comparait que les clés de premier niveau, et il le disait. Sur
// `soll_manager` — 20 399 appels, premier gisement de la surface — ce premier
// niveau ne porte que `action`, `entity` et `data`, tous honorés : le contrôle
// validait donc une table structurellement incapable de signaler quoi que ce soit.
//
// Une récursion qui ne sait pas dire NON est le même défaut un étage plus bas :
// les deux mutants ci-dessous la font rougir sur ses DEUX moitiés, `manquants`
// et `fantômes`.
// ---------------------------------------------------------------------------------

/// Le schéma de démonstration : un scalaire au premier niveau, un objet qui porte
/// deux feuilles. C'est la forme exacte de `soll_manager`.
fn schema_imbrique_de_demonstration() -> Value {
    serde_json::json!({
        "action": { "type": "string" },
        "data": {
            "type": "object",
            "properties": {
                "section": { "type": "string" },
                "id":      { "type": "string" }
            }
        }
    })
}

#[test]
fn la_descente_rend_les_FEUILLES_et_jamais_le_conteneur() {
    use crate::mcp::tool_contracts::chemins_de_proprietes_du_schema;

    let chemins = chemins_de_proprietes_du_schema(&schema_imbrique_de_demonstration());
    assert_eq!(
        chemins,
        vec!["action".to_string(), "data.id".to_string(), "data.section".to_string()],
        "un objet porteur de sous-propriétés est REMPLACÉ par ses feuilles, pas doublé \
         par elles : compter `data` en plus de `data.section` compterait deux fois le \
         même fait, et `Honoured` sur un conteneur n'affirme rien d'éprouvable"
    );
}

#[test]
fn MUTANT_la_descente_sait_dire_NON_sur_ses_DEUX_moities() {
    use crate::mcp::tool_contracts::{chemins_de_proprietes_du_schema, ecart_de_couverture,
                                     ParameterDeclaration, ParameterDisposition,
                                     ToolDispositions};

    let schema = chemins_de_proprietes_du_schema(&schema_imbrique_de_demonstration());

    // MOITIÉ 1 — un champ imbriqué SERVI mais absent de la table. Sans la descente,
    // ce cas passait au vert : `data` était couvert, donc tout `data.*` l'était.
    const SANS_LA_FEUILLE: &[ParameterDeclaration] = &[
        ParameterDeclaration { name: "action", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.id", disposition: ParameterDisposition::Honoured },
    ];
    let trouee = ToolDispositions { declared: SANS_LA_FEUILLE, unexamined: &[] };
    let ecart = ecart_de_couverture(&schema, &trouee)
        .expect("une feuille servie et non déclarée DOIT être refusée");
    assert!(
        ecart.contains("data.section"),
        "l'écart doit NOMMER la feuille manquante, pas son conteneur : {ecart}"
    );

    // MOITIÉ 2 — un champ imbriqué DÉCLARÉ qui n'est plus servi. C'est la dérive
    // inverse, et elle est tout aussi silencieuse : la table décrit un paramètre
    // que plus personne ne peut poser.
    const AVEC_UN_FANTOME: &[ParameterDeclaration] = &[
        ParameterDeclaration { name: "action", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.id", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.section", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.disparu", disposition: ParameterDisposition::Honoured },
    ];
    let fantome = ToolDispositions { declared: AVEC_UN_FANTOME, unexamined: &[] };
    let ecart = ecart_de_couverture(&schema, &fantome)
        .expect("une feuille déclarée et plus servie DOIT être refusée");
    assert!(
        ecart.contains("data.disparu"),
        "l'écart doit NOMMER la feuille fantôme : {ecart}"
    );

    // Et la table exacte passe — sinon le contrôle crie toujours et ne dit rien.
    const EXACTE: &[ParameterDeclaration] = &[
        ParameterDeclaration { name: "action", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.id", disposition: ParameterDisposition::Honoured },
        ParameterDeclaration { name: "data.section", disposition: ParameterDisposition::Honoured },
    ];
    assert_eq!(
        ecart_de_couverture(&schema, &ToolDispositions { declared: EXACTE, unexamined: &[] }),
        None
    );
}

// ---------------------------------------------------------------------------------
// REQ-AXO-902583 (s146) — le COMPORTEMENT de `soll_manager`, les deux moitiés.
//
// Un contrôle qui crie toujours ne dit rien : chaque cas négatif est apparié à son
// positif sur le MÊME champ.
// ---------------------------------------------------------------------------------

#[test]
fn soll_manager_signale_un_champ_imbrique_sans_effet_sous_cette_action() {
    let inertes = inert_parameters_for_call(
        "soll_manager",
        &serde_json::json!({
            "action": "update",
            "entity": "requirement",
            "data": { "id": "REQ-AXO-1", "section": "un ajout", "section_title": "Titre" }
        }),
    );
    let noms: Vec<&str> = inertes.iter().map(|i| i.name.as_str()).collect();
    assert!(
        noms.contains(&"data.section") && noms.contains(&"data.section_title"),
        "`section` et `section_title` ne sont lus que par `append_section` : {noms:?}"
    );
    assert!(
        !noms.contains(&"data.id"),
        "`data.id` EST lu par `update` — le signaler enverrait corriger ce qui marche : {noms:?}"
    );
    let section = inertes.iter().find(|i| i.name == "data.section").expect("présent");
    assert!(
        section.reason.contains("update"),
        "la raison doit nommer la valeur REÇUE, pas décrire une généralité : {}",
        section.reason
    );
    assert!(
        !section.remedy.to_lowercase().contains("orthograph"),
        "le remède de l'inertie ne doit JAMAIS renvoyer à l'orthographe — c'est la \
         remédiation de l'AUTRE cause : {}",
        section.remedy
    );
}

#[test]
fn soll_manager_ne_signale_RIEN_quand_l_action_lit_le_champ() {
    let inertes = inert_parameters_for_call(
        "soll_manager",
        &serde_json::json!({
            "action": "append_section",
            "entity": "requirement",
            "data": { "id": "REQ-AXO-1", "section": "un ajout", "section_title": "Titre" }
        }),
    );
    assert!(
        inertes.is_empty(),
        "sous `append_section`, les trois champs sont lus : {inertes:?}"
    );

    // La moitié POSITIVE de `FieldOneOf` sur ses DEUX valeurs — sinon rien ne
    // distingue la variante multi-valeurs d'un `FieldEquals` sur la première.
    for action in ["link", "unlink"] {
        let inertes = inert_parameters_for_call(
            "soll_manager",
            &serde_json::json!({
                "action": action,
                "entity": "requirement",
                "data": { "source_id": "A", "target_id": "B", "relation_type": "BELONGS_TO" }
            }),
        );
        let noms: Vec<&str> = inertes.iter().map(|i| i.name.as_str()).collect();
        assert!(
            !noms.contains(&"data.source_id") && !noms.contains(&"data.target_id"),
            "`{action}` lit les deux extrémités : {noms:?}"
        );
        // …et `entity`, lui, n'est lu par AUCUNE des deux branches : c'est la
        // mesure qui l'établit, la lecture des deux corps entiers.
        assert!(
            noms.contains(&"entity"),
            "`entity` est requis par le schéma et n'apparaît nulle part dans `{action}` : \
             le taire laisserait chercher la panne du mauvais côté : {noms:?}"
        );
    }
}

/// Un verdict `inert` FAUX est pire que le silence : il envoie réparer un appel
/// qui a marché.
///
/// `append_section` n'est pas une branche autonome — elle re-dispatche vers
/// `update` avec le `data` de l'appelant, moins `section`/`section_title`. Tout ce
/// que `update` lit traverse donc et s'applique. Sans ce test, la table lisait la
/// branche visible et manquait la délégation.
#[test]
fn append_section_DELEGUE_a_update_et_ce_qu_update_lit_reste_effectif() {
    let inertes = inert_parameters_for_call(
        "soll_manager",
        &serde_json::json!({
            "action": "append_section",
            "entity": "requirement",
            "data": {
                "id": "REQ-AXO-1",
                "section": "un ajout",
                "status": "current",
                "title": "un titre",
                "priority": "P1",
                "tags": ["a"],
                "acceptance_criteria": ["c"]
            }
        }),
    );
    assert!(
        inertes.is_empty(),
        "`append_section` re-dispatche vers `update` : tout ce que `update` lit \
         s'applique. Les déclarer inertes enverrait réparer un appel qui a marché : \
         {inertes:?}"
    );

    // MOITIÉ NÉGATIVE — la délégation ne rend PAS tout effectif. `attach_to` et
    // `project_code` ne sont lus que par `create`, que le re-dispatch n'emprunte
    // jamais. Sans ce contrôle, « tout passe sous append_section » serait la
    // sur-correction symétrique.
    let inertes = inert_parameters_for_call(
        "soll_manager",
        &serde_json::json!({
            "action": "append_section",
            "entity": "requirement",
            "data": { "id": "REQ-AXO-1", "section": "x", "attach_to": "PIL-AXO-001",
                      "project_code": "AXO" }
        }),
    );
    let noms: Vec<&str> = inertes.iter().map(|i| i.name.as_str()).collect();
    assert!(
        noms.contains(&"data.attach_to") && noms.contains(&"data.project_code"),
        "le re-dispatch va vers `update`, jamais vers `create` : {noms:?}"
    );
}

#[test]
fn MUTANT_FieldOneOf_ne_se_laisse_PAS_ecrire_comme_une_negation() {
    use crate::mcp::tool_contracts::ParameterCondition;

    let positive = ParameterCondition::FieldOneOf {
        field: "action",
        values: &["link", "unlink"],
    };
    let negative = ParameterCondition::FieldNotOneOf {
        field: "action",
        values: &["create", "update", "append_section"],
    };

    // Sur les actions D'AUJOURD'HUI, les deux formes sont indiscernables.
    for action in ["create", "update", "append_section", "link", "unlink"] {
        let args = serde_json::json!({ "action": action });
        assert_eq!(
            positive.holds(&args),
            negative.holds(&args),
            "les deux formes coïncident sur les actions existantes (`{action}`)"
        );
    }

    // Sur une action FUTURE, elles divergent — et c'est tout l'enjeu : la forme
    // négative la déclarerait « effectif » sans qu'aucun test ne rougisse.
    let demain = serde_json::json!({ "action": "une_action_ajoutee_demain" });
    assert!(!positive.holds(&demain), "la forme positive reste muette sur l'inconnu");
    assert!(
        negative.holds(&demain),
        "la forme négative accueille l'inconnu comme effectif — c'est le défaut \
         qu'on refuse d'écrire, et ce test est ce qui empêche de l'écrire par inadvertance"
    );
}
