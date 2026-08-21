// Copyright (c) Didier Stadelmann. All rights reserved.

//! REQ-AXO-902409 — « N éléments soumis ⇒ N éléments RENDUS ».
//!
//! Une hypothèse architecturale fausse, écrite cinq fois : les outils sont
//! rédigés comme si `data.*` était visible du LLM. Le client Claude Code
//! n'expose que `content[0].text`. Chaque nouvel outil qui rend une liste
//! rejoue le défaut, et chaque correction coûte un aller-retour
//! tenant → doléance → REQ → promote :
//!
//! | outil | REQ | ce qui n'atteignait pas le texte |
//! |---|---|---|
//! | `axon_init_project` | 902355 | corps Vision + Pillars |
//! | `practice_card` | 902325 | le « top by trust » annoncé |
//! | `mcp_feedback_report` | 902398 | les doléances elles-mêmes |
//! | `soll_apply_plan` (commit) | 902403 | les ids canoniques attribués |
//! | `soll_apply_plan` (dry-run) | 902411 | les relations prévisualisées |
//!
//! Les tests existants ne l'attrapaient pas parce qu'ils assertent sur `data.*`
//! — exactement le canal invisible. `test_mcp_feedback_report_lists_filters_
//! and_resolves` vérifiait `data["feedback"]` et passait au vert pendant que la
//! sortie réelle était trois lignes de compteurs. **Un test qui lit le canal que
//! le client n'expose pas mesure autre chose que ce qu'il prétend.**
//!
//! Ce module ferme la CLASSE : il énumère les outils depuis le catalogue, les
//! appelle, et vérifie l'invariant sur ce qui revient. Aucune liste tenue à la
//! main — une énumération manuelle dérive dès le prochain outil.

use super::*;

/// Clés qui font d'un objet un ÉLÉMENT identifiable — donc quelque chose que le
/// lecteur doit pouvoir nommer, et sur quoi une action de suivi se branche
/// (`mark_resolved={id}`, `inspect symbol=`, `soll_get(id=)`).
const IDENTIFYING_KEYS: &[&str] = &[
    "id",
    "name",
    "symbol",
    "logical_key",
    "tool",
    "path",
    "file_path",
    "canonical_id",
    "source_id",
];

/// Tableaux de `data` qui décrivent le CALCUL, pas son résultat. Les rendre
/// n'apporterait rien au lecteur ; les exiger ferait du bruit à chaque appel.
///
/// Cette liste est délibérément courte et justifiée entrée par entrée : c'est le
/// seul endroit du module où un jugement humain s'exerce, donc le seul endroit
/// où la garde peut être affaiblie sans qu'on le voie.
const DIAGNOSTIC_ARRAYS: &[&str] = &[
    // Provenance de la réponse, déjà résumée en une ligne dans le texte.
    "surfaces_used",
    "surfaces_degraded",
    "canonical_sources",
    // Facteurs de blocage : rendus sous forme de phrase, pas de liste d'items.
    "blocking_factors",
    "remediation_actions",
    // Le catalogue lui-même : `status` en donne le COMPTE et renvoie à
    // `mode=verbose`, ce qui est le bon arbitrage de coût.
    "public_tools",
];

/// L'invariant : **un outil doit livrer ce que sa description promet.**
///
/// Deux versions ont été écrites et réfutées avant celle-ci ; les garder en
/// mémoire vaut mieux que garder seulement la bonne.
///
/// 1. *« chaque tableau d'items doit être NOMMÉ »* — a signalé
///    `runtime_filesystem_health` à tort. Cet outil rend délibérément les seuls
///    artefacts EN DÉFAUT plus le dénominateur (« 3 artefact(s) inspecté(s),
///    aucun problème ») : c'est déjà le correctif de REQ-AXO-902378, et lister
///    trois chemins sains n'apprend rien.
/// 2. *« nommés OU comptés »* — assouplissement qui a **neutralisé la garde** :
///    falsifiée en désarmant la table de `mcp_feedback_report`, elle est restée
///    VERTE. Le défaut historique est précisément « trois lignes de compteurs
///    sans les items » : accepter le compte, c'est accepter le défaut.
///
/// La règle qui tient les deux bouts est celle de PIL-AXO-002 — « 'Use the
/// guidance below' MUST deliver guidance below ». Elle se lit dans le catalogue,
/// pas dans le code : **si la description promet d'énumérer, les éléments
/// doivent être nommés ; sinon le compte suffit.** `mcp_feedback_report`
/// annonce « Lists voluntary LLM doléances … newest-first » ;
/// `runtime_filesystem_health` annonce une santé, pas un inventaire.
///
/// Limite assumée : la promesse est détectée par mots-clés dans la prose du
/// catalogue. Un outil qui promet d'énumérer sans employer ces mots échappe à la
/// garde — c'est un faux négatif connu, préférable au faux positif qui pousse à
/// affaiblir la règle jusqu'à ce qu'elle ne morde plus.
fn unrendered_item_arrays(response: &Value, promises_enumeration: bool) -> Vec<String> {
    let text = response["content"][0]["text"].as_str().unwrap_or_default();
    let Some(data) = response.get("data").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut violations = Vec::new();
    for (key, value) in data {
        if DIAGNOSTIC_ARRAYS.contains(&key.as_str()) {
            continue;
        }
        let Some(items) = value.as_array().filter(|a| !a.is_empty()) else {
            continue;
        };
        // Les identifiants portés par les objets du tableau.
        let identifiers: Vec<String> = items
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|item| {
                IDENTIFYING_KEYS
                    .iter()
                    .find_map(|k| item.get(*k).and_then(Value::as_str))
                    .map(str::to_string)
            })
            .filter(|id| !id.is_empty())
            .collect();
        if identifiers.is_empty() {
            continue; // tableau de scalaires ou d'objets anonymes : hors périmètre
        }
        let named = identifiers.iter().any(|id| text.contains(id.as_str()));
        let counted = text.contains(&items.len().to_string());
        let satisfied = if promises_enumeration { named } else { named || counted };
        if !satisfied {
            violations.push(format!(
                "data.{key} porte {} élément(s) identifiable(s) — dont `{}` — et le \
                 texte n'en nomme AUCUN{}",
                identifiers.len(),
                identifiers[0],
                if promises_enumeration {
                    " alors que la description de l'outil promet de les ÉNUMÉRER"
                } else {
                    " ni n'en donne le compte"
                }
            ));
        }
    }
    violations
}

/// Sème de quoi rendre les listes NON VIDES : un tableau vide ne falsifie rien,
/// et c'est précisément le piège que ce module existe pour éviter.
fn seed_listable_content(server: &McpServer) {
    use crate::test_support::ist_fixtures::{CallFixture, IstSeed, SymbolFixture};

    // IST : sans symboles, tous les outils de code répondent à vide et le
    // balayage se réduit aux surfaces SOLL/système.
    let mut seed = IstSeed::new();
    for n in 1..=4 {
        seed = seed.symbol(
            SymbolFixture::new(
                format!("RLC::src/contrat{n}.rs::fonction_contrat_{n}"),
                format!("fonction_contrat_{n}"),
                "function",
                "RLC",
            )
            .tested(n % 2 == 0)
            .is_public(true),
        );
    }
    for n in 2..=4 {
        seed = seed.call(CallFixture::canonical(
            format!("RLC::src/contrat{n}.rs::fonction_contrat_{n}"),
            "RLC::src/contrat1.rs::fonction_contrat_1",
            "RLC",
        ));
    }
    let _ = crate::test_support::ist_fixtures::seed_ist(&server.graph_store, &seed);

    let exec = |sql: &str| {
        let _ = server.graph_store.execute(sql);
    };
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-RLC-001', 'Pillar', 'RLC', 'Pilier de contrat', 'corps', 'current', '{}') ON CONFLICT (id) DO NOTHING");
    for n in 1..=3 {
        exec(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-RLC-00{n}', 'Requirement', 'RLC', 'Exigence de contrat {n}', 'corps', 'planned', '{{\"priority\":\"P1\"}}') ON CONFLICT (id) DO NOTHING"));
        exec(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-RLC-00{n}', 'PIL-RLC-001', 'BELONGS_TO') ON CONFLICT DO NOTHING"));
    }
    for n in 1..=3 {
        exec(&format!(
            "INSERT INTO axon.practice (scope, context, practice, trust, use_count, status) \
             VALUES ('RLC', 'contexte {n}', 'pratique de contrat {n}', 0.8, {n}, 'active')"
        ));
    }
    for n in 1..=3 {
        let _ = server.handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback",
                "arguments": {
                    "problem": format!("RLC_PROBE doléance de contrat {n}"),
                    "severity": "minor",
                    "category": "bug",
                    "tool": "help",
                    "project_code": "RLC"
                }
            })),
            id: Some(json!(n)),
        });
    }
}

/// Mots par lesquels une description PROMET d'énumérer. Le contrat que la garde
/// fait respecter est celui que l'outil s'est donné lui-même.
const ENUMERATION_PROMISES: &[&str] = &[
    "lists ",
    "list of",
    "newest-first",
    "ranked",
    "top-n",
    "top n",
    "worklist",
    "inventory",
    "enumerate",
    "returns the contradicting",
];

/// Valeur plausible pour un paramètre REQUIS, choisie par NOM de paramètre —
/// jamais par nom d'outil.
///
/// La distinction est ce qui empêche la dérive : la liste des OUTILS vient du
/// catalogue, seule la traduction « ce paramètre s'appelle `symbol` donc voici
/// un symbole » est écrite ici. Un paramètre inconnu laisse l'outil en erreur,
/// donc **non exercé** — jamais « conforme ». Oublier une entrée fait baisser le
/// dénominateur, visiblement ; ça ne peut pas fabriquer un vert.
fn value_for_required_parameter(parameter: &str) -> Option<Value> {
    Some(match parameter {
        "symbol" | "target" => Value::from("fonction_contrat_1"),
        "id" | "entity_id" | "node_id" => Value::from("REQ-RLC-001"),
        "query" | "question" | "search" | "candidate" | "intent_text" => {
            Value::from("fonction_contrat_1")
        }
        "path" | "file_path" => Value::from("src/contrat1.rs"),
        "entity_type" => Value::from("requirement"),
        "source" | "source_id" => Value::from("REQ-RLC-001"),
        "target_id" => Value::from("PIL-RLC-001"),
        "from" | "to" => Value::from("fonction_contrat_1"),
        _ => return None,
    })
}

/// Les outils, pris du catalogue — nom, promesse d'énumération, et arguments
/// dérivés de leur propre `inputSchema.required`. Pas de liste tenue à la main :
/// une énumération manuelle dérive dès le prochain outil.
fn catalog_tools(base_arguments: &Value) -> Vec<(String, bool, Value)> {
    crate::mcp::catalog::tools_catalog(false)["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    let name = t.get("name").and_then(Value::as_str)?.to_string();
                    let description = t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase();
                    let promises = ENUMERATION_PROMISES
                        .iter()
                        .any(|needle| description.contains(needle));

                    let mut arguments = base_arguments.clone();
                    if let (Some(required), Some(map)) = (
                        t["inputSchema"]["required"].as_array(),
                        arguments.as_object_mut(),
                    ) {
                        for parameter in required.iter().filter_map(Value::as_str) {
                            if let Some(value) = value_for_required_parameter(parameter) {
                                map.insert(parameter.to_string(), value);
                            }
                        }
                    }
                    Some((name, promises, arguments))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// REQ-AXO-902409 — la garde de classe.
///
/// Elle balaie le catalogue avec un sac d'arguments génériques. Un outil qui
/// exige des arguments qu'on ne lui donne pas répond en erreur ou avec des
/// tableaux vides : il est compté comme NON EXERCÉ, jamais comme conforme.
/// Le test rend son propre dénominateur — un balayage qui n'exerce rien
/// passerait au vert en ne mesurant rien, ce qui est exactement la classe de
/// défaut visée (REQ-AXO-902384).
#[test]
fn every_tool_that_returns_items_names_them_in_the_text() {
    let _guard = env_lock();
    let server = create_test_server();
    seed_listable_content(&server);

    // Sac générique : les outils ignorent ce qu'ils ne connaissent pas. Il n'y a
    // pas d'argument spécifique à un outil ici, sinon la liste dériverait.
    let arguments = json!({
        "project": "RLC",
        "project_code": "RLC",
        "scope": "RLC",
        "limit": 5,
        "window_hours": 168,
        "mode": "brief"
    });

    let mut exercised: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();

    for (tool, promises_enumeration, arguments) in catalog_tools(&arguments) {
        let Some(result) = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": tool, "arguments": arguments })),
                id: Some(json!(902_409)),
            })
            .and_then(|response| response.result)
        else {
            continue; // pas de réponse exploitable : non exercé
        };
        if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            continue; // arguments insuffisants : non exercé, pas conforme
        }
        let found = unrendered_item_arrays(&result, promises_enumeration);
        // « Exercé » = l'appel a produit au moins un tableau d'items, donc
        // l'invariant avait quelque chose à mordre.
        let has_items = result
            .get("data")
            .and_then(Value::as_object)
            .map(|data| {
                data.iter().any(|(k, v)| {
                    !DIAGNOSTIC_ARRAYS.contains(&k.as_str())
                        && v.as_array().is_some_and(|a| {
                            a.iter().filter_map(Value::as_object).any(|item| {
                                IDENTIFYING_KEYS.iter().any(|key| {
                                    item.get(*key).and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                                })
                            })
                        })
                })
            })
            .unwrap_or(false);
        if has_items {
            exercised.push(tool.clone());
        }
        for violation in found {
            violations.push(format!("{tool} : {violation}"));
        }
    }

    // Le dénominateur d'abord : sans lui, « 0 violation » ne veut rien dire.
    // Plancher tenu SOUS la mesure courante (6 : query, promote_status,
    // mcp_friction_report, mcp_telemetry_report, mcp_feedback_report,
    // runtime_filesystem_health) pour ne pas rougir sur une variation
    // d'environnement, mais assez haut pour qu'un balayage qui n'exerce plus
    // rien se voie. Le chiffre exact est rendu dans le message, pas caché.
    assert!(
        exercised.len() >= 5,
        "balayage vacuous — seulement {} outil(s) ont rendu des items exploitables \
         ({exercised:?}). Un test qui n'exerce rien passe au vert en ne mesurant \
         rien : c'est la classe de défaut que ce module combat (REQ-AXO-902384). \
         Enrichir `seed_listable_content`.",
        exercised.len()
    );

    assert!(
        violations.is_empty(),
        "{} outil(s) placent leurs éléments dans `data.*` sans les nommer dans le \
         TEXTE — le client Claude Code n'expose que `content[0].text` \
         (REQ-AXO-902409, cause fermée par REQ-AXO-902355) :\n  - {}\n\n\
         ({} outil(s) exercés sur ce balayage : {exercised:?})",
        violations.len(),
        violations.join("\n  - "),
        exercised.len()
    );
}
