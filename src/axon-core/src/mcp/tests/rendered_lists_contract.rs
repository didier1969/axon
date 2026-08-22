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
        // Un outil a le droit d'ABRÉGER : `ist_centrality_pagerank` rend
        // `contrat1.rs::fonction_contrat_1` là où `data.results[].id` porte
        // `RLC::src::contrat1.rs::fonction_contrat_1` (REQ-AXO-902201, délibéré
        // et plus lisible). Exiger l'id ENTIER faisait de cet outil un faux
        // positif — et un faux positif pousse à désaffuter la règle.
        //
        // Le repli sur le DERNIER segment `::` est borné à 4 caractères : sans
        // ce plancher, un segment comme `new` matcherait n'importe quelle prose
        // et la garde deviendrait vacueuse, ce qui est exactement le piège
        // documenté dans REQ-AXO-902409 (« nommés OU comptés » l'avait neutralisée).
        const MIN_SEGMENT_LEN: usize = 4;
        let named = identifiers.iter().any(|id| {
            if text.contains(id.as_str()) {
                return true;
            }
            id.rsplit("::")
                .next()
                .filter(|segment| segment.len() >= MIN_SEGMENT_LEN)
                .is_some_and(|segment| text.contains(segment))
        });
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
    // REQ-AXO-902075 / 902409 — évincer le snapshot RAM du projet sémé, sinon
    // les lecteurs IST (`inspect`, `impact`, `wiring`, `orphan_clusters`,
    // `structural_health_*`) tombent sur celui qu'un test frère a laissé chaud,
    // n'y trouvent rien, et sortent du balayage comme « non exercés ».
    crate::ist_snapshot::evict_process_snapshot("RLC");


    let exec = |sql: &str| {
        let _ = server.graph_store.execute(sql);
    };
    // REQ-AXO-902409 — SANS cette ligne, 33 outils sur 113 répondaient
    // `wrong_project_scope` et sortaient du balayage comme « non exercés », ce
    // qui se lisait comme un manque d'arguments. Un projet non ENREGISTRÉ n'est
    // pas un projet : `require_registered_*` refuse avant toute lecture. Le
    // fixture semait des nœuds pour `RLC` sans jamais le déclarer.
    exec("INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) VALUES ('RLC', '/tmp/rlc', 'rlc') ON CONFLICT (project_code) DO NOTHING");
    exec("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('RLC', 'AXON_GLOBAL', 9, 9, 9, 9) ON CONFLICT (project_code) DO NOTHING");
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
// Le CHAUFFER — APRES l'enregistrement du projet : `resolve_project_code_value`
    // refuse un code absent du registre, et le warm sortait donc en
    // `wrong_project_scope` sans que rien ne le dise. Huit outils IST
    // (`wiring`, `orphan_clusters`, `structural_health_*`, `debt_digest`,
    // `ist_*`) restaient alors en `ist_cache_miss`. L'éviction seule ne suffit pas :
    // les lecteurs IST rendent « snapshot cold » (un refus explicite, PIL-AXO-002)
    // au lieu de lire, et sortent du balayage sans que l'invariant les touche.
    let _ = server.handle_request(JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "ist_snapshot_warm",
            "arguments": { "project": "RLC", "project_code": "RLC" }
        })),
        id: Some(json!(902_409)),
    });

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

/// Outils que le balayage ne doit PAS appeler, en plus de ceux que
/// `McpServer::is_mutating_tool` classe déjà comme mutants.
///
/// Un balayage aveugle du catalogue **invoque tout**, y compris ce qui écrit.
/// `embed_provider action=set` bascule le fournisseur d'embed via une variable
/// d'environnement du PROCESSUS — donc partagée avec tous les autres tests ;
/// `idle_drop action=set` écrit une ligne de contrôle DURABLE et
/// cross-processus ; `mcp_inbox_read` en mode `unread` AVANCE le curseur de
/// lecture. Aucun n'est dans `is_mutating_tool`, dont la liste sert la politique
/// async/monitoring et n'a jamais eu vocation à borner un balayage.
///
/// Cette liste locale existe donc parce que la classification canonique est
/// incomplète (REQ-AXO-902412). Risque résiduel assumé et nommé : un mutant
/// absent des DEUX listes serait appelé. C'est la raison pour laquelle le
/// balayage tourne contre un serveur de test à base éphémère — le rayon
/// d'action reste le processus de test, jamais le runtime live.
const RUNTIME_MUTATORS: &[&str] = &[
    "embed_provider",
    "idle_drop",
    "rescan_project",
    "practice_put",
    "practice_retire",
    "practice_tick",
    "mcp_feedback",
    "mcp_inbox_read",
    "mcp_inbox_archive",
    "mcp_outbox_send",
    "mailbox_lease",
    "mailbox_room_create",
    "mailbox_room_join",
    "mailbox_sweep",
    "mailbox_tap",
    "mailbox_topic_subscribe",
    "mailbox_topic_unsubscribe",
    "axon_init_project",
    "axon_commit_work",
    "axon_apply_guidelines",
    "axon_apply_methodology_bundle",
    "document_intent",
    "infer_soll_mutation",
    "contract_evolve",
    "fuse",
    "re_anchor",
    "ist_snapshot_warm",
    "skill_invoke",
];

fn is_write_capable(tool: &str) -> bool {
    McpServer::is_mutating_tool(tool) || RUNTIME_MUTATORS.contains(&tool)
}

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
        // REQ-AXO-902409 tranche 2 — chaque nom ajouté ici sort un outil de la
        // colonne « sans arguments ». Mesuré à l'ajout : 6 exercés → 9 rien
        // qu'en enregistrant le projet, puis au-delà avec ces valeurs.
        "project_code" | "project" | "scope" => Value::from("RLC"),
        "project_path" => Value::from("/tmp/rlc"),
        "source_type" => Value::from("requirement"),
        "target_type" => Value::from("pillar"),
        "statement" => Value::from("les fonctions de contrat sont testées"),
        "tool" => Value::from("help"),
        "symbols" => json!(["fonction_contrat_1"]),
        "sql" => Value::from("SELECT 1"),
        "uri" => Value::from("src/contrat1.rs"),
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
                    // REQ-AXO-902409 — un balayage ne doit rien ÉCRIRE.
                    if is_write_capable(&name) {
                        return None;
                    }
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
    // Ordre env → service_guard, identique partout dans la caisse : c'est
    // l'uniformité de l'ordre qui écarte le deadlock. Le verrou est exigé par
    // `no_test_touches_global_service_state_without_the_lock` (REQ-AXO-902274),
    // qui m'a attrapé sur le `reset` ci-dessous — la garde a fait exactement son
    // travail, et elle a dit quoi faire.
    let _sg_guard = crate::service_guard::lock_for_tests();
    crate::service_guard::reset_for_tests();
    // Le balayage appelle ~90 outils, dont les surfaces runtime : elles TOUCHENT
    // l'ordonnanceur utility-first, un état de PROCESSUS partagé par tous les
    // tests. On le remet à neuf des DEUX côtés : avant, pour ne pas mesurer
    // l'héritage d'un autre test ; après, pour ne pas le léguer.
    //
    // Note d'enquête, pour qui relira : l'arrivée de ce module a fait tomber
    // `test_single_gpu_worker_cruise_mode_grows_more_aggressively_when_ready_
    // queue_starves` à 80 au lieu de 104. Ce module en était le DÉCLENCHEUR — il
    // déplace l'ordre d'exécution — et non la cause : ce test lisait un
    // instantané de réglage mémoïsé qu'il n'établissait jamais (REQ-AXO-902414).
    // Deux hypothèses accusant ce balayage-ci ont été réfutées avant celle-là.
    crate::vector_control::reset_utility_first_scheduler_for_tests();
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

    // Un balayage en LECTURE ne doit pas réécrire l'environnement du processus.
    //
    // « Lecture seule pour les données du tenant » n'implique PAS « sans effet de
    // bord sur le processus » : `runtime_boot.rs:62` et `embedder.rs:1346` posent
    // `AXON_VECTOR_WORKERS` / `AXON_EMBEDDING_PROVIDER` en CODE DE PRODUCTION. Un
    // outil qui ne fait que répondre pourrait donc les écraser, et le verrou
    // d'environnement des tests n'y pourrait rien — l'écriture n'ayant pas lieu
    // dans un test.
    //
    // Mesuré : aucune dérive aujourd'hui. La garde reste parce que le chemin
    // existe et qu'une dérive corromprait silencieusement tous les tests suivants,
    // en rendant par-dessus le marché ce rapport-ci sans valeur.
    // TOUTES les `AXON_*`, pas une liste choisie à la main.
    //
    // La première version de cette sonde en surveillait QUATRE, retenues à vue de
    // nez, et concluait « aucune dérive ». C'était un verdict sans dénominateur
    // sur mon propre appareil de mesure : `vector_control.rs` lit à lui seul
    // `AXON_VECTOR_TARGET_READY_CHUNKS`, `AXON_GPU_READY_LOW_WATERMARK_CHUNKS`,
    // `AXON_GPU_READY_HIGH_WATERMARK*`, `AXON_GPU_PRESSURE_EMBED_BATCH_CHUNKS`…
    // — aucune n'était dans les quatre. Un instrument qui ne couvre pas l'espace
    // qu'il prétend surveiller rend un « rien trouvé » qui ne veut rien dire.
    let snapshot_axon_env = || -> std::collections::BTreeMap<String, String> {
        std::env::vars()
            .filter(|(k, _)| k.starts_with("AXON_"))
            .collect()
    };
    let env_before = snapshot_axon_env();

    let mut exercised: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();
    // REQ-AXO-902409 — « 6 outils exercés sur 113 » n'est pas une liste de
    // travail : il faut savoir POURQUOI les 107 autres ne le sont pas. Deux
    // causes, deux gestes opposés — enrichir les arguments, ou semer des
    // données. Les confondre, c'est rendre un chiffre sans dénominateur, le
    // défaut même que ce module combat.
    let mut unexercised_no_args: Vec<String> = Vec::new();
    let mut unexercised_no_data: Vec<String> = Vec::new();

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
            unexercised_no_args.push(tool.clone());
            continue; // pas de réponse exploitable : non exercé
        };
        if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
            // La RAISON, pas seulement le fait : « non exercé » sans cause
            // n'indique pas quoi enrichir — c'est le verdict sans dénominateur
            // que ce module combat, appliqué à son propre appareil de mesure.
            let why = result
                .get("data")
                .and_then(|d| d.get("status"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    result["content"][0]["text"]
                        .as_str()
                        .unwrap_or("?")
                        .chars()
                        .take(48)
                        .collect()
                });
            unexercised_no_args.push(format!("{tool} [{why}]"));
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
        } else {
            // L'outil a RÉPONDU sans rendre d'items : ses données manquent au
            // fixture. C'est l'autre moitié de la liste de travail.
            unexercised_no_data.push(tool.clone());
        }
        for violation in found {
            violations.push(format!("{tool} : {violation}"));
        }
    }

    // Rendre l'état de processus tel qu'on l'a trouvé — les DEUX registres.
    //
    // ⚠️ Ces deux réinitialisations sont de l'HYGIÈNE, pas le correctif d'un bug.
    // Elles ont été ajoutées en poursuivant une hypothèse — « le balayage nourrit
    // le `service_guard`, que le contrôleur consulte » — qui a ensuite été
    // RÉFUTÉE : la suite complète a rougi avec les deux réinitialisations en
    // place. La vraie cause était ailleurs (un instantané de réglage mémoïsé que
    // le test victime n'établissait jamais, REQ-AXO-902414), et elle est corrigée
    // chez le test victime, pas ici.
    //
    // Elles restent parce qu'un balayage de ~90 outils doit rendre les registres
    // de processus tels qu'il les a trouvés, quelle que soit la cause du jour.
    crate::service_guard::reset_for_tests();
    crate::vector_control::reset_utility_first_scheduler_for_tests();

    // L'invariant d'innocuité, AVANT les verdicts de rendu : un balayage qui
    // laisse l'environnement modifié corrompt tous les tests qui suivent, et le
    // rapport de rendu qu'il produit ne vaut alors plus rien.
    let env_after = snapshot_axon_env();
    let mut env_drift: Vec<String> = Vec::new();
    for (key, before) in &env_before {
        match env_after.get(key) {
            Some(after) if after == before => {}
            Some(after) => env_drift.push(format!("{key} : `{before}` → `{after}`")),
            None => env_drift.push(format!("{key} : `{before}` → SUPPRIMÉE")),
        }
    }
    for (key, after) in &env_after {
        if !env_before.contains_key(key) {
            env_drift.push(format!("{key} : absente → `{after}`"));
        }
    }
    assert!(
        env_drift.is_empty(),
        "le balayage a MODIFIÉ l'environnement `AXON_*` du processus alors qu'il \
         n'appelle que des outils en lecture — du code de production écrit ces \
         variables (runtime_boot.rs:62, embedder.rs:1346) et le verrou de test ne \
         peut pas l'en empêcher. Les tests qui asservissent leur résultat à ces \
         variables (contrôleur de lot vectoriel) tomberont LOIN d'ici :\n  - {}",
        env_drift.join("\n  - ")
    );

    // Le dénominateur d'abord : sans lui, « 0 violation » ne veut rien dire.
    // Plancher tenu SOUS la mesure courante (12) pour ne pas rougir sur une
    // variation d'environnement, mais assez haut pour qu'un balayage qui
    // n'exerce plus rien se voie. Le chiffre exact est rendu, pas caché.
    //
    // Tranche 2 (REQ-AXO-902409) : 6 → 12 exercés, par deux corrections du
    // FIXTURE et aucune du contrat. (a) `RLC` n'était pas dans
    // `soll.ProjectCodeRegistry`, donc 33 outils répondaient
    // `wrong_project_scope` ; (b) le snapshot IST était chauffé AVANT cet
    // enregistrement, donc la chauffe échouait en silence et huit outils IST
    // restaient en `ist_cache_miss`. Les deux se lisaient comme « manque
    // d'arguments » — d'où la classification des non-exercés et l'impression de
    // leur RAISON : sans elle il n'y a pas de liste de travail, juste un ratio.
    // Le bilan est IMPRIMÉ, pas seulement porté par un message d'échec : c'est
    // lui qui dit par où élargir, et il n'a de valeur que quand le test est VERT.
    // `cargo test -- --nocapture rendered_lists` le rend lisible.
    println!(
        "[REQ-AXO-902409] balayage : {} exercé(s) · {} sans arguments · {} sans données\n\
         exercés          : {exercised:?}\n\
         sans arguments   : {unexercised_no_args:?}\n\
         sans données     : {unexercised_no_data:?}",
        exercised.len(),
        unexercised_no_args.len(),
        unexercised_no_data.len(),
    );

    assert!(
        exercised.len() >= 10,
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
