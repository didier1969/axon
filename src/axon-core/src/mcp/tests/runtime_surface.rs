use super::*;

/// REQ-AXO-902325 — `run_query_count` must decode the WHOLE numeric family, not just
/// `int8`.
///
/// `count(*)` is `int8`, so counts were always right and the helper read as healthy.
/// Aggregates are not: `round(avg(x)*1000)` over a `REAL` column is `double precision`
/// (measured with `pg_typeof`), and `sum`/`avg` over integers is `numeric`. Both failed
/// `try_get::<_, i64>`, and the `unwrap_or(0)` behind it turned that type error into the
/// plausible value 0 — which is how `practice_card` reported `mean trust 0.00` on a store
/// whose real mean is 0.53.
///
/// No table: the aggregates run over inline VALUES, so this pins the DECODER and nothing
/// else. The `int8` case is asserted alongside deliberately — it is the one that always
/// worked, and it is what made the bug invisible.
#[test]
fn query_count_decodes_float_and_numeric_aggregates_not_just_bigint() {
    let server = create_test_server();

    let n = server
        .graph_store
        .query_count("SELECT count(*) FROM (VALUES (1),(2),(3)) AS t(v)")
        .expect("count(*) is int8");
    assert_eq!(n, 3, "int8 was never the broken case");

    // `double precision` — the shape that actually produced `mean trust 0.00`.
    let f = server
        .graph_store
        .query_count("SELECT round(avg(v)*1000) FROM (VALUES (0.5::real),(0.9::real)) AS t(v)")
        .expect("float8 aggregate must decode");
    assert_eq!(
        f, 700,
        "avg over REAL is double precision; decoding it as i64-only yields a silent 0"
    );

    // `numeric` — the sibling shape (sum/avg over integers), same failure mode.
    let d = server
        .graph_store
        .query_count("SELECT round(avg(v)*1000) FROM (VALUES (1::int),(2::int)) AS t(v)")
        .expect("numeric aggregate must decode");
    assert_eq!(d, 1500, "avg over int is numeric; it must not collapse to 0");

    // Rounded, not truncated: a caller scaling by 1000 to keep three decimals is
    // asking for the nearest integer, and truncation would quietly bias every mean low.
    let r = server
        .graph_store
        .query_count("SELECT 2.6::float8")
        .expect("float8 scalar must decode");
    assert_eq!(r, 3, "the numeric family is rounded, not truncated");
}

/// REQ-AXO-902408 — signalé par KKI (llm_feedback #179). La description de
/// `practice_put` promet un gate qui passe ou REJETTE ; `gate=inconclusive` est
/// un troisième état non documenté, et le caller ne peut pas savoir s'il a
/// quelque chose à faire. Recoupé côté AXO : les trois `practice_put` de la
/// session 121 ont tous rendu `inconclusive` — c'est le cas COURANT.
#[test]
fn practice_put_gate_says_what_it_means_and_whether_to_act() {
    let server = create_test_server();
    let resp = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "practice_put",
                "arguments": {
                    "context": "une garde vient d'etre ecrite",
                    "practice": "la falsifier avant de la committer",
                    "scope": "PGT"
                }
            })),
            id: Some(json!(902_408)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = resp["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("gate="), "le verdict doit être rendu : {text}");
    assert!(
        text.contains("aucune action requise")
            || text.contains("rien à faire")
            || text.contains("stockée"),
        "tout verdict non rejetant doit dire ce qu'il implique et s'il demande \
         quelque chose au caller : {text}"
    );
}

/// REQ-AXO-902325 — the tool advertises "top practices by trust". It computed them,
/// put them in `data`, and never rendered them. Most MCP clients (including the bare
/// HTTP path) surface only `content[0].text`, so the advertised half was invisible to
/// the caller who asked for it.
#[test]
fn practice_card_renders_its_advertised_top_and_a_real_mean() {
    let server = create_test_server();
    for (scope, practice, trust, uses) in [
        ("AXO", "verifier ce que compte un chiffre avant de decider", 0.91_f32, 12),
        ("AXO", "falsifier un gate avant de le committer", 0.72_f32, 5),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO axon.practice (scope, context, practice, trust, use_count, status) \
                 VALUES ('{scope}', 'ctx', '{practice}', {trust}, {uses}, 'active')"
            ))
            .unwrap();
    }

    let resp = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "practice_card", "arguments": { "scope": "AXO" } })),
            id: Some(json!(902_325)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = resp["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("Top by trust"),
        "the advertised top must reach the TEXT channel, not only `data`: {text}"
    );
    assert!(
        text.contains("falsifier un gate") || text.contains("verifier ce que compte"),
        "at least one practice must be named in the rendered top: {text}"
    );
    assert!(
        !text.contains("mean trust 0.00"),
        "a store holding practices at trust 0.72 and 0.91 cannot have a mean of 0.00: {text}"
    );

    let mean = resp["data"]["mean_trust"].as_f64().unwrap_or(0.0);
    assert!(
        mean > 0.5,
        "mean_trust must reflect the stored values, got {mean}"
    );
}

#[test]
fn test_mcp_tools_list() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"help"));
    assert!(tool_names.contains(&"restore_soll"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"soll_apply_plan"));
    assert!(tool_names.contains(&"soll_work_plan"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"mcp_surface_diagnostics"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"project_registry_lookup"));
    assert!(tool_names.contains(&"soll_relation_schema"));
    assert!(tool_names.contains(&"infer_soll_mutation"));
    assert!(tool_names.contains(&"entrench_nuance"));
    assert!(tool_names.contains(&"soll_generate_docs"));
    assert!(tool_names.contains(&"snapshot_history"));
    assert!(tool_names.contains(&"snapshot_diff"));
    assert!(tool_names.contains(&"conception_view"));
    assert!(tool_names.contains(&"change_safety"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(tool_names.contains(&"axon_pre_flight_check"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(!tool_names.contains(&"soll_apply_plan_v2"));
    assert!(!tool_names.contains(&"refine_lattice"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"sql"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"schema_overview"));
    assert!(!tool_names.contains(&"list_labels_tables"));
    assert!(tool_names.contains(&"query_examples"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));
}

/// REQ-AXO-902426 — `help` rabotait le préfixe `axon_` AVANT de chercher, puis
/// échouait sur le nom tronqué que le catalogue ne publie pas.
///
/// Signalé par KKI (`mcp_feedback` #134), reproduit à l'identique côté AXO. Le
/// message d'erreur était sa propre réfutation : « Unknown MCP tool
/// `handoff_check`. Closest: axon_handoff_check » — il suggérait exactement ce
/// que l'appelant avait tapé. Les DEUX outils dont la résolution cassait sont
/// ceux du chemin de livraison.
/// REQ-AXO-902434 — le catalogue PUBLIE six noms préfixés que le routeur reçoit
/// rabotés : deux espaces de noms réconciliés par une convention, pas par une
/// dérivation.
///
/// Mesuré s122 : 108 outils sur 114 portent le même nom des deux côtés, 6 ne se
/// rejoignent qu'après ce retrait — et la convention était recopiée sur **cinq**
/// sites, dont **deux ne l'appliquaient qu'à moitié**. `batch` et la dérivation
/// de schéma ignoraient `mcp_axon_`, que les trois autres traitaient : le même
/// nom marchait par la voie directe et échouait par `batch`.
///
/// C'est la même recopie, dans le mauvais ordre, qui avait cassé `help`
/// (REQ-AXO-902426).
#[test]
fn test_every_published_tool_name_normalises_to_something_the_router_knows() {
    let catalog = crate::mcp::catalog::tools_catalog(true);
    let published: Vec<String> = catalog["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .map(str::to_string)
        .collect();

    // CONTRÔLE POSITIF : sans lui, une liste vide validerait tout.
    assert!(
        published.len() > 100,
        "le catalogue doit publier une surface réelle, sinon ce test ne mesure \
         rien : {} outil(s)",
        published.len()
    );

    // La normalisation est IDEMPOTENTE, et chaque écriture d'un même outil
    // converge vers le même nom — c'est ce que « une seule source » garantit.
    for name in &published {
        let canonical = crate::mcp::canonical_tool_name(name);
        assert_eq!(
            crate::mcp::canonical_tool_name(canonical),
            canonical,
            "la normalisation doit être idempotente : `{name}` -> `{canonical}`"
        );
        for prefix in ["axon_", "mcp_axon_"] {
            let prefixed = format!("{prefix}{canonical}");
            assert_eq!(
                crate::mcp::canonical_tool_name(&prefixed),
                canonical,
                "`{prefixed}` doit désigner le même outil que `{canonical}` — \
                 c'est le préfixe `mcp_axon_` que deux sites sur cinq oubliaient"
            );
        }
    }

    // Et les six noms préfixés du catalogue doivent réellement se raboter.
    let prefixed: Vec<&String> = published.iter().filter(|n| n.starts_with("axon_")).collect();
    assert!(
        !prefixed.is_empty(),
        "contrôle positif : le catalogue publie bien des noms préfixés, sinon la \
         boucle ci-dessus ne teste pas le cas qui a cassé"
    );
    for name in prefixed {
        assert_ne!(
            crate::mcp::canonical_tool_name(name),
            name.as_str(),
            "`{name}` est publié préfixé : le routeur le connaît sous sa forme rabotée"
        );
    }
}

#[test]
fn test_help_resolves_a_tool_whose_real_name_carries_the_axon_prefix() {
    let server = create_test_server();
    let ask = |tool: &str| -> serde_json::Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "help", "arguments": { "tool": tool } })),
                id: Some(json!(902426)),
            })
            .unwrap()
            .result
            .expect("help doit répondre")
    };

    for tool in ["axon_handoff_check", "axon_pre_flight_check", "axon_commit_work"] {
        let res = ask(tool);
        assert_ne!(
            res.get("isError").and_then(serde_json::Value::as_bool),
            Some(true),
            "`help(tool=\"{tool}\")` doit résoudre : c'est le nom que le catalogue \
             PUBLIE.\n{res}"
        );
    }

    // CONTRÔLE POSITIF : la forme sans préfixe continue de marcher (le dispatch
    // l'accepte), sinon le correctif aurait troqué un échec contre un autre.
    let bare = ask("query");
    assert_ne!(
        bare.get("isError").and_then(serde_json::Value::as_bool),
        Some(true),
        "un nom sans préfixe doit continuer de résoudre : {bare}"
    );

    // CONTRÔLE NÉGATIF : un nom réellement inconnu échoue TOUJOURS, et l'erreur
    // nomme ce que l'APPELANT a demandé — reprocher une chaîne qu'il n'a jamais
    // écrite est ce qui rendait le message absurde.
    let ghost = ask("axon_ceci_nexiste_pas");
    assert_eq!(
        ghost.get("isError").and_then(serde_json::Value::as_bool),
        Some(true),
        "un outil inexistant doit toujours échouer : {ghost}"
    );
    let text = ghost["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("axon_ceci_nexiste_pas"),
        "l'erreur doit nommer ce que l'appelant a demandé, pas une variante \
         rabotée qu'il n'a jamais écrite :\n{text}"
    );
}

#[test]
fn test_help_returns_compact_llm_routing_and_skill_pointer() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "help",
                "arguments": { "topic": "routing", "intent": "prepare_edit" }
            })),
            id: Some(json!(77)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("help data");
    assert_eq!(data["topic"].as_str(), Some("routing"));
    assert_eq!(data["audience"].as_str(), Some("llm_clients_only"));
    assert_eq!(data["protocol"]["intent"].as_str(), Some("prepare_edit"));
    assert_eq!(
        data["skill"]["name"].as_str(),
        Some("axon-engineering-protocol")
    );
    assert_eq!(
        data["skill"]["path"].as_str(),
        Some("docs/skills/axon-engineering-protocol/SKILL.md")
    );
    assert!(data["routing"]
        .as_array()
        .is_some_and(|items| items.len() <= 8));
    assert_eq!(
        data["protocol"]["minimal_sequence"][0].as_str(),
        Some("status")
    );
    assert!(data["protocol"]["minimal_sequence"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == "impact")));
    assert!(data["protocol"]["stop_rule"]
        .as_str()
        .is_some_and(|text| text.contains("blast radius")));
    assert!(data["protocol"]["avoid"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(data["protocol"]["requires_explicit_input_if"]
        .as_array()
        .is_some_and(|items| items
            .iter()
            .any(|item| item == "business intent is missing")));
    assert_eq!(
        data["token_policy"].as_str(),
        Some("brief_first_full_only_when_needed")
    );
    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("query -> inspect"), "{text}");
    assert!(text.contains("Protocol: prepare_edit"), "{text}");
    assert!(text.len() < 950, "{text}");
}

/// REQ-AXO-901908 — the authoring path must surface `soll_apply_plan` where it is
/// consumed (help). An LLM bootstrapping a derived SOLL subtree previously fell
/// back to N `soll_manager` round-trips because no init/help routing named the
/// atomic-write tool. `help(intent=author_soll)` and the `soll` topic now carry
/// that pointer.
#[test]
fn test_help_author_soll_intent_surfaces_soll_apply_plan() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "help",
                "arguments": { "topic": "soll", "intent": "author_soll" }
            })),
            id: Some(json!(908)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("help data");
    assert_eq!(data["protocol"]["intent"].as_str(), Some("author_soll"));
    // The atomic-authoring tool is the spine of the protocol.
    assert!(
        data["protocol"]["minimal_sequence"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "soll_apply_plan")),
        "author_soll protocol must name soll_apply_plan: {data}"
    );
    // The soll topic routing also points at it (the path that was missing).
    assert!(
        data["routing"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item.as_str().is_some_and(|s| s.contains("soll_apply_plan")))),
        "soll topic must route to soll_apply_plan: {data}"
    );
}

#[test]
fn test_help_returns_tool_schema_and_examples_for_soll_apply_plan() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "help",
                "arguments": { "tool": "soll_apply_plan" }
            })),
            id: Some(json!(78)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("help data");
    assert_eq!(data["tool"].as_str(), Some("soll_apply_plan"));
    assert!(data["input_schema"]["required"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == "project_code")));
    assert!(data["input_schema"]["properties"]
        .get("relations")
        .is_some());
    assert!(data["usage_examples"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert_eq!(
        data["next_action"]["after_success"].as_str(),
        Some("poll `job_status` if the response returns `job_id`; commit only after dry-run matches intent")
    );
}

#[test]
fn test_mcp_tools_list_in_brain_only_exposes_information_surface() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1000)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_tools_list_in_full_autonomous_exposes_information_surface() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"infer_soll_mutation"));
    assert!(tool_names.contains(&"entrench_nuance"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(!tool_names.contains(&"resume_vectorization"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"sql"));
    assert!(tool_names.contains(&"diagnose_indexing"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_mcp_tools_list_include_internal_adds_resume_vectorization_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: Some(json!({ "include_internal": true })),
        id: Some(json!(1002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"batch"));
    assert!(tool_names.contains(&"job_status"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(tool_names.contains(&"resume_vectorization"));
    assert!(tool_names.contains(&"debug"));
    assert!(tool_names.contains(&"sql"));
    assert!(tool_names.contains(&"schema_overview"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_soll_manager_stays_sync_when_mutation_jobs_are_enabled() {
    let _guard = env_lock();
    // REQ-AXO-099 — panic-safe: the guard restores the prior value on unwind,
    // so a panic in this test cannot leak AXON_MCP_MUTATION_JOBS=true into
    // concurrent/subsequent tests (root cause of the async-job test cluster).
    let _mj = crate::test_support::EnvVarGuard::set("AXON_MCP_MUTATION_JOBS", "true");
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Sync Pillar', '', 'current', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": "AXO",
                    "name": "Async Concept",
                    "explanation": "Created through MCP job",
                    "rationale": "Shared-server mutation path",
                    "attach_to": "PIL-AXO-001", "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(5001)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let content = result["content"][0]["text"].as_str().unwrap_or_default();
    let data = result.get("data").expect("sync response must carry data");
    assert_sync_mutation_contract(data);
    assert!(content.contains("CPT-AXO-"), "{content}");
    let entity_id = content
        .split('`')
        .find(|value| value.starts_with("CPT-AXO-"))
        .expect("entity id in content");
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE id = '{}'",
                entity_id
            ))
            .unwrap(),
        1
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mutating_soll_apply_plan_returns_job_and_reserved_preview_id() {
    let _guard = env_lock();
    let site_root = tempdir().unwrap();
    // REQ-AXO-902025 — panic-safe RAII: a timeout/assert failure here must NOT
    // leak AXON_MCP_MUTATION_JOBS=true into the following sync-expecting tests
    // (the cascade root). The guards restore the prior value on unwind.
    let _mj = crate::test_support::EnvVarGuard::set("AXON_MCP_MUTATION_JOBS", "true");
    let _sr = crate::test_support::EnvVarGuard::set(
        "AXON_SOLL_SITE_ROOT",
        site_root.path().to_str().expect("temp path is utf-8"),
    );
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-job-preview",
                        "title": "Job Preview Requirement",
                        "description": "Dry-run should reserve preview id immediately"
                    }]
                }
            }
        })),
        id: Some(json!(5002)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let data = result.get("data").expect("job response must carry data");
    assert_async_job_contract(data, "job_status");
    let job_id = data
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("job_id");
    let preview_id = data
        .get("reserved_ids")
        .and_then(|value| value.get("preview_id"))
        .and_then(|value| value.as_str())
        .expect("reserved preview_id");
    assert!(preview_id.starts_with("PRV-AXO-"), "{preview_id}");
    assert_eq!(
        data.get("known_ids")
            .and_then(|value| value.get("preview_id"))
            .and_then(|value| value.as_str()),
        Some(preview_id)
    );

    let final_status = wait_for_job_status(&server, job_id);
    assert_eq!(
        final_status["data"]["status"].as_str().unwrap(),
        "succeeded"
    );
    assert_eq!(final_status["data"]["state"].as_str(), Some("completed"));
    assert!(final_status["data"]["known_ids"].is_object());
    assert!(final_status["data"]["result_contract"].is_object());
    assert!(final_status["data"]["polling_guidance"].is_object());
    assert!(final_status["data"]["recovery_hint"].as_str().is_some());
    assert_eq!(
        final_status["data"]["next_action"]["kind"].as_str(),
        Some("read_terminal_result")
    );
    assert_eq!(
        final_status["data"]["result_data"]["preview_id"].as_str(),
        Some(preview_id)
    );
    let result_preview_id = final_status["data"]["result"]["data"]["preview_id"]
        .as_str()
        .expect("preview id should survive job result");
    assert_eq!(result_preview_id, preview_id);
    assert_eq!(
        final_status["data"]["result"]["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    assert!(site_root.path().join("AXO/index.html").is_file());
    // Env restored by the `_mj` / `_sr` guards on drop (panic-safe).
}

#[test]
fn test_axon_init_project_stays_sync_when_mutation_jobs_are_enabled() {
    let _guard = env_lock();
    // REQ-AXO-099 — panic-safe: the guard restores the prior value on unwind,
    // so a panic in this test cannot leak AXON_MCP_MUTATION_JOBS=true into
    // concurrent/subsequent tests (root cause of the async-job test cluster).
    let _mj = crate::test_support::EnvVarGuard::set("AXON_MCP_MUTATION_JOBS", "true");
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        })),
        id: Some(json!(5003)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let data = result.get("data").expect("sync response must carry data");
    assert_sync_mutation_contract(data);
    assert_eq!(
        data.get("project_code").and_then(|value| value.as_str()),
        Some("BKS")
    );
    assert_eq!(
        data.get("project_name").and_then(|value| value.as_str()),
        Some("BookingSystem")
    );
    assert_eq!(
        data.get("project_path").and_then(|value| value.as_str()),
        Some("/home/dstadel/projects/BookingSystem")
    );
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_project_registry_lookup_finds_project_by_path_name_and_code() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry(
            "BKS",
            Some("BookingSystem"),
            Some("/home/dstadel/projects/BookingSystem"),
        )
        .unwrap();

    for arguments in [
        json!({ "project_code": "BKS" }),
        json!({ "project_name": "BookingSystem" }),
        json!({ "project_path": "/home/dstadel/projects/BookingSystem" }),
    ] {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_registry_lookup",
                "arguments": arguments
            })),
            id: Some(json!(5010)),
        };
        let response = server.handle_request(req).unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["data"]["found"].as_bool(), Some(true));
        assert_eq!(result["data"]["project_code"].as_str(), Some("BKS"));
        assert_eq!(
            result["data"]["project_name"].as_str(),
            Some("BookingSystem")
        );
        assert_eq!(
            result["data"]["project_path"].as_str(),
            Some("/home/dstadel/projects/BookingSystem")
        );
        assert_eq!(
            result["data"]["matches"]
                .as_array()
                .map(|items| items.len()),
            Some(1)
        );
        assert_eq!(
            result["data"]["next_action"]["kind"].as_str(),
            Some("use_canonical_project_code")
        );
        assert_eq!(
            result["data"]["next_action"]["tool"].as_str(),
            Some("project_status")
        );
        assert!(result["data"]["operator_guidance"].is_object());
    }
}

#[test]
fn test_soll_apply_plan_accepts_freshly_initialized_project_code_across_runtime_boundary() {
    // REQ-AXO-902026 — isolate this test on its own ephemeral DB (cloned from
    // `axon_test_template`) instead of two raw `GraphStore::new` calls. The raw
    // path resolves to the SHARED PG and re-runs the GLOBAL schema bootstrap
    // (CREATE EXTENSION/SCHEMA + the pgmq / ist.Chunk migration), which RACED a
    // live runtime writing the same shared instance → intermittent
    // "Writer Error" DDL failures (~1/3 runs). A private clone already carries
    // the canonical schema, so the re-open is a fast `IF NOT EXISTS` no-op and
    // never contends with the live DDL. Both store instances share the SAME
    // clone URL, so the cross-runtime-boundary persistence under test is kept.
    let test_db = crate::test_support::test_db::TestDb::create();
    let db_url = test_db.url();
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph-store");
    let store = Arc::new(
        GraphStore::new_with_database(root.to_string_lossy().as_ref(), &db_url).unwrap(),
    );
    let server = McpServer::new(store);

    let init_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": {
                    "project_path": "/home/dstadel/projects/nutri-opti",
                    "project_name": "nutri-opti"
                }
            })),
            id: Some(json!(5011)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(init_response["data"]["project_code"].as_str(), Some("NTO"));
    drop(server);

    let reopened_store = Arc::new(
        GraphStore::new_with_database(root.to_string_lossy().as_ref(), &db_url).unwrap(),
    );
    let reopened_server = McpServer::new(reopened_store);

    let lookup_response = reopened_server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "project_registry_lookup",
                "arguments": {
                    "project_path": "/home/dstadel/projects/nutri-opti"
                }
            })),
            id: Some(json!(5012)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        lookup_response["data"]["project_code"].as_str(),
        Some("NTO")
    );

    let apply_plan_response = reopened_server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_apply_plan",
                "arguments": {
                    "project_code": "NTO",
                    "author": "test",
                    "dry_run": true,
                    "plan": {
                        "visions": [
                            {
                                "logical_key": "vision-1",
                                "title": "Vision NTO",
                                "description": "Nutri Opti vision"
                            }
                        ],
                        "pillars": [
                            {
                                "logical_key": "pillar-1",
                                "title": "Pillar NTO",
                                "description": "Nutri Opti pillar"
                            }
                        ]
                    }
                }
            })),
            id: Some(json!(5013)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_ne!(
        apply_plan_response
            .get("isError")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    let data = apply_plan_response
        .get("data")
        .expect("apply-plan response must carry data");
    if data.get("job_id").is_some() {
        assert_async_job_contract(data, "job_status");
        let job_id = data
            .get("job_id")
            .and_then(|value| value.as_str())
            .expect("job_id");
        let preview_id = data["reserved_ids"]["preview_id"]
            .as_str()
            .expect("reserved preview id");
        assert!(preview_id.starts_with("PRV-NTO-"), "{preview_id}");

        let final_status = wait_for_job_status(&reopened_server, job_id);
        assert_eq!(
            final_status["data"]["status"].as_str().unwrap(),
            "succeeded"
        );
        assert_eq!(
            final_status["data"]["result_data"]["preview_id"].as_str(),
            Some(preview_id)
        );
    } else {
        assert!(data.get("job_id").is_none());
        assert!(data.get("accepted").is_none());
        assert!(data.get("polling_guidance").is_none());
        assert!(data["preview_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("PRV-NTO-")));
    }
}

#[test]
fn test_soll_manager_requires_project_code_even_when_mutation_jobs_are_enabled() {
    let _guard = env_lock();
    // REQ-AXO-099 — panic-safe: the guard restores the prior value on unwind,
    // so a panic in this test cannot leak AXON_MCP_MUTATION_JOBS=true into
    // concurrent/subsequent tests (root cause of the async-job test cluster).
    let _mj = crate::test_support::EnvVarGuard::set("AXON_MCP_MUTATION_JOBS", "true");
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "name": "Missing project scope",
                    "explanation": "Jobs must reject implicit project identity",
                    "attach_to": "PIL-PRO-001", "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(5003)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let is_error = result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    // project_code is now auto-resolved from canonical project identity,
    // so omitting it no longer triggers an error.
    assert!(
        !is_error,
        "soll_manager should auto-resolve project_code when omitted"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_soll_commit_revision_requires_preview_id_even_when_mutation_jobs_are_enabled() {
    let _guard = env_lock();
    // REQ-AXO-099 — panic-safe: the guard restores the prior value on unwind,
    // so a panic in this test cannot leak AXON_MCP_MUTATION_JOBS=true into
    // concurrent/subsequent tests (root cause of the async-job test cluster).
    let _mj = crate::test_support::EnvVarGuard::set("AXON_MCP_MUTATION_JOBS", "true");
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_commit_revision",
            "arguments": {
                "author": "test"
            }
        })),
        id: Some(json!(5004)),
    };

    let response = server.handle_request(req).unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("Missing required argument: preview_id"),
        "{content}"
    );

    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
}

#[test]
fn test_mcp_tools_list_hides_indexed_runtime_tools_in_graph_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_remains_available_in_graph_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": {
                "query": "booking",
                "project": "BookingSystem"
            }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    assert!(
        !result
            .get("isError")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        "{result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(!content.contains("unavailable in runtime mode 'indexer_graph'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_status_graph_only_reports_semantic_drain_not_applicable() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_graph");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cuda");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_graph_only_reports_semantic_drain_not_applicable",
        );
    }
    let _tempdir = tempdir().unwrap();
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2165)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("status data");
    assert_eq!(data["runtime_mode"].as_str(), Some("indexer_graph"));
    assert!(data["debug_snapshot"].is_null());
    assert!(data["traceability"].is_null());
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_scope"].as_str(),
        Some("indexer_graph")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["semantic_health"].as_str(),
        Some("not_applicable")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["recommendation"].as_str(),
        Some("not_applicable")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_requested"]
            .as_str(),
        Some("cpu")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

// REQ-AXO-106 — the legacy "Advanced indexed surfaces visible: yes/no"
// label gave LLM clients no way to map the bit to a tool decision (the
// signal does not actually gate any tool). Replace with an "IST
// projection freshness: fresh|stale (hint)" line that names the
// concrete semantic and clarifies tools remain usable when stale.
// Surface a parallel `data.availability.ist_projection_fresh` field;
// keep the legacy `advanced_indexed_surfaces_visible` for backward
// compatibility with existing MCP consumers.
#[test]
fn test_status_uses_ist_projection_freshness_label_and_field() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_uses_ist_projection_freshness_label_and_field",
        );
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2166)),
        })
        .unwrap()
        .result
        .unwrap();

    let evidence = response["content"][0]["text"].as_str().unwrap();
    assert!(
        evidence.contains("IST reads:"),
        "human-readable label should name the IST reads usability semantic: {evidence}"
    );
    assert!(
        !evidence.contains("Advanced indexed surfaces"),
        "legacy opaque label must be retired from the text surface: {evidence}"
    );

    let availability = response["data"]["availability"]
        .as_object()
        .expect("data.availability is an object");
    assert!(
        availability.get("ist_projection_fresh").is_some(),
        "new canonical field `ist_projection_fresh` must be present"
    );
    assert!(
        availability
            .get("ist_projection_fresh")
            .and_then(|v| v.as_bool())
            .is_some(),
        "ist_projection_fresh must be a boolean"
    );
    assert!(
        availability
            .get("advanced_indexed_surfaces_visible")
            .and_then(|v| v.as_bool())
            .is_some(),
        "legacy `advanced_indexed_surfaces_visible` alias must remain for backward compatibility"
    );
    assert_eq!(
        availability["ist_projection_fresh"], availability["advanced_indexed_surfaces_visible"],
        "the two fields must always agree until the alias is retired"
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

// REQ-AXO-098 / DEC-AXO-062 / CPT-AXO-023 — `mcp__axon__status` must
// expose subsystem-tagged tristate readiness. `data.readiness` carries
// the rolled-up overall (Failed dominates Degraded; Degraded dominates
// Ready) and `data.subsystems[]` carries the per-subsystem reports
// each with name, state kind, optional reason, last_observed_at_ms.
#[test]
fn test_status_exposes_subsystem_readiness_contract() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    crate::runtime_readiness::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_exposes_subsystem_readiness_contract",
        );
    }
    crate::runtime_readiness::report_subsystem_state(
        crate::runtime_readiness::Subsystem::BrainMcp,
        crate::runtime_readiness::SubsystemState::Ready,
    );
    crate::runtime_readiness::report_subsystem_state(
        crate::runtime_readiness::Subsystem::Embedder,
        crate::runtime_readiness::SubsystemState::Degraded {
            reason: "cpu_fallback".to_string(),
        },
    );

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2167)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("status data");
    let readiness = data
        .get("readiness")
        .expect("readiness field must be present");
    assert_eq!(
        readiness.get("kind").and_then(|v| v.as_str()),
        Some("degraded"),
        "any Degraded subsystem with no Failed → overall Degraded: {readiness:?}"
    );
    let reasons = readiness
        .get("reasons")
        .and_then(|v| v.as_array())
        .expect("Degraded readiness must include reasons array");
    assert!(
        reasons.iter().any(|r| {
            r.as_str()
                .map(|s| s.starts_with("embedder:"))
                .unwrap_or(false)
        }),
        "reasons must be subsystem-prefixed: {reasons:?}"
    );

    let subsystems = data
        .get("subsystems")
        .and_then(|v| v.as_array())
        .expect("subsystems[] must be present");
    assert!(
        subsystems.iter().any(|entry| {
            entry
                .get("subsystem")
                .and_then(|v| v.as_str())
                .map(|s| s == "brain_mcp")
                .unwrap_or(false)
        }),
        "brain_mcp report must be present after explicit Ready report: {subsystems:?}"
    );
    assert!(
        subsystems.iter().any(|entry| {
            entry
                .get("subsystem")
                .and_then(|v| v.as_str())
                .map(|s| s == "embedder")
                .unwrap_or(false)
                && entry
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "degraded")
                    .unwrap_or(false)
        }),
        "embedder must show its Degraded state with kind label: {subsystems:?}"
    );

    crate::runtime_readiness::reset_for_tests();
    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_mcp_tools_list_hides_indexed_runtime_tools_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/list".to_string(),
        params: None,
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let tools = result
        .get("tools")
        .expect("Expected tools array")
        .as_array()
        .expect("tools is array");

    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap())
        .collect();

    assert!(tool_names.contains(&"fs_read"));
    assert!(tool_names.contains(&"soll_validate"));
    assert!(tool_names.contains(&"status"));
    assert!(tool_names.contains(&"project_status"));
    assert!(tool_names.contains(&"why"));
    assert!(tool_names.contains(&"path"));
    assert!(tool_names.contains(&"anomalies"));
    assert!(tool_names.contains(&"retrieve_context"));
    assert!(tool_names.contains(&"query"));
    assert!(tool_names.contains(&"inspect"));
    assert!(tool_names.contains(&"audit"));
    assert!(tool_names.contains(&"impact"));
    assert!(tool_names.contains(&"health"));
    assert!(tool_names.contains(&"truth_check"));
    assert!(tool_names.contains(&"diagnose_indexing"));
    assert!(tool_names.contains(&"diff"));
    assert!(tool_names.contains(&"semantic_clones"));
    assert!(tool_names.contains(&"architectural_drift"));
    assert!(tool_names.contains(&"bidi_trace"));
    assert!(tool_names.contains(&"api_break_check"));
    assert!(tool_names.contains(&"simulate_mutation"));
    assert!(!tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_mcp_query_remains_available_in_full_isolated() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": {
                "query": "booking",
                "project": "BookingSystem"
            }
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    assert!(
        !result
            .get("isError")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        "{result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(!content.contains("unavailable in runtime mode 'indexer_full'"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_brain_only_impact_does_not_return_tool_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "impact",
                "arguments": { "symbol": "missing_symbol", "project": "AXO" }
            })),
            id: Some(json!(2296)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(!content.contains("unavailable"), "{content}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_brain_only_retrieve_context_does_not_return_tool_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "where is missing_symbol defined?", "project": "AXO" }
            })),
            id: Some(json!(2297)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(!content.contains("unavailable"), "{content}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_retrieve_context_auto_resolves_project_code_from_cwd() {
    // REQ-AXO-089 — when `project` arg is omitted, retrieve_context
    // must auto-resolve from AXON_PROJECT_ROOT (or cwd) by matching
    // against ProjectCodeRegistry, like the global CLAUDE.md promises
    // ("project_code is auto-resolved from your working directory").
    // Previously the tool fell through to workspace:* whenever the
    // caller skipped the arg, making answers from inside a project
    // directory look workspace-wide.
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axon-cwd-fixture"))
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axon-cwd-fixture");
    }
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "where is missing_symbol defined" }
            })),
            id: Some(json!(89001)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("project:AXO") || content.contains("Scope:** `project:AXO`"),
        "scope must be project:AXO when AXON_PROJECT_ROOT matches a registered project; got: {content}"
    );
    assert!(
        !content.contains("workspace:*"),
        "scope must NOT fall through to workspace:* once auto-resolution succeeds; got: {content}"
    );
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn retrieve_context_honors_explicit_project_code_through_tools_call() {
    // REQ-AXO-902521 / DGD #309 — the public project_code must win over
    // server-cwd auto-resolution. The old handler read only `project` and
    // silently answered from AXO.
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-scope"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("DGD", Some("dgd"), Some("/home/test/dgd-scope"))
        .unwrap();
    let catalog = crate::mcp::catalog::tools_catalog(true);
    let retrieve_schema = catalog["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["name"] == json!("retrieve_context"))
        })
        .expect("retrieve_context in public catalog");
    assert!(
        retrieve_schema["inputSchema"]["properties"]["project_code"].is_object(),
        "canonical tenant scope must be discoverable by generated MCP clients"
    );
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axo-scope");
    }

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "where is dgd_only_missing_symbol defined?",
                    "project_code": "DGD",
                    "semantic": "lexical",
                    "include_soll": false
                }
            })),
            id: Some(json!(9025211)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("project:DGD"), "{content}");
    assert!(!content.contains("project:AXO"), "{content}");

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn retrieve_context_rejects_conflicting_project_aliases() {
    // REQ-AXO-902521 — ambiguity at the tenant boundary must fail closed.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": {
                    "question": "where is the router?",
                    "project": "AXO",
                    "project_code": "DGD"
                }
            })),
            id: Some(json!(9025212)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response["isError"], json!(true), "{response}");
    assert_eq!(response["data"]["status"], json!("wrong_project_scope"));
}

#[test]
fn test_client_cwd_header_overrides_server_cwd_for_project_resolution() {
    // REQ-AXO-902286 — the shared brain must resolve project_code against the
    // CALLING agent's directory (carried by the tunnel's `X-Axon-Client-Cwd`
    // header, installed as a per-request thread-local via `ClientCwdGuard`), NOT
    // its own cwd. Without the fix, `auto_resolve_project_code_str` reads only
    // AXON_PROJECT_ROOT / server cwd and returns the brain's own project (AXO) for
    // every non-AXO caller — the silent wrong-project class this REQ closes.
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-fixture"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("TE2", Some("trader"), Some("/home/test/te2-fixture"))
        .unwrap();
    // The brain's own directory resolves to AXO.
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axo-fixture");
    }
    // No client cwd installed → falls back to the server directory (AXO).
    assert_eq!(
        server.auto_resolve_project_code_str().as_deref(),
        Some("AXO"),
        "without a client cwd, resolution falls back to the server directory"
    );
    // Client cwd (a non-AXO project) installed for this request → MUST win.
    {
        let _cwd =
            crate::mcp::ClientCwdGuard::install(Some("/home/test/te2-fixture".to_string()));
        assert_eq!(
            server.auto_resolve_project_code_str().as_deref(),
            Some("TE2"),
            "the per-request client cwd must override the server directory"
        );
    }
    // Guard dropped → back to the server project, proving no cross-request leakage.
    assert_eq!(
        server.auto_resolve_project_code_str().as_deref(),
        Some("AXO"),
        "the client cwd must be cleared once the request guard drops"
    );
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_status_cache_is_isolated_by_calling_client_project() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("SCA", Some("status-a"), Some("/home/test/status-a"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("SCB", Some("status-b"), Some("/home/test/status-b"))
        .unwrap();

    let status_for = |path: &str| {
        let _cwd = crate::mcp::ClientCwdGuard::install(Some(path.to_string()));
        server.axon_status(&json!({"mode": "brief"})).unwrap()
    };
    let first = status_for("/home/test/status-a/worktree");
    let second = status_for("/home/test/status-b/worktree");
    let unresolved = status_for("/home/test/status-unregistered");

    let first_text = first["content"][0]["text"].as_str().unwrap();
    let second_text = second["content"][0]["text"].as_str().unwrap();
    assert!(first_text.contains("`SCA`"), "{first_text}");
    assert!(second_text.contains("`SCB`"), "{second_text}");
    assert!(!second_text.contains("`SCA`"), "{second_text}");
    assert_eq!(
        first["data"]["instance_identity"]["auto_detected_project"],
        json!("SCA")
    );
    assert_eq!(
        second["data"]["instance_identity"]["auto_detected_project"],
        json!("SCB")
    );
    assert!(unresolved["data"]["instance_identity"]["auto_detected_project"].is_null());
    let unresolved_text = unresolved["content"][0]["text"].as_str().unwrap();
    assert!(!unresolved_text.contains("`SCA`"), "{unresolved_text}");
    assert!(!unresolved_text.contains("`SCB`"), "{unresolved_text}");
}

#[test]
fn test_cwd_provenance_disclosed_only_when_auto_resolved() {
    // REQ-AXO-902287 (M1) — an auto-resolved project scope gets a one-line
    // provenance note so the LLM knows the project was inferred, not chosen; an
    // explicit project passes through untouched.
    let base = json!({ "content": [{ "type": "text", "text": "body" }] });
    // Auto-resolved (stamped `cwd_auto`) → note appended, names the project.
    let auto_args = json!({ "project_code": "AXO", "project_code_source": "cwd_auto" });
    let out = crate::mcp::McpServer::disclose_cwd_provenance(&auto_args, Some(base.clone()))
        .unwrap();
    let text = out["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("déduit du cwd") && text.contains("AXO"),
        "auto-resolved response must disclose cwd provenance: {text}"
    );
    // Explicit project (no stamp) → untouched.
    let explicit_args = json!({ "project_code": "AXO" });
    let out2 = crate::mcp::McpServer::disclose_cwd_provenance(&explicit_args, Some(base))
        .unwrap();
    assert_eq!(
        out2["content"][0]["text"].as_str().unwrap(),
        "body",
        "explicit project must not get a provenance note"
    );
}

#[test]
fn test_status_brief_omits_public_tools_list_in_text() {
    // REQ-AXO-104 — status mode=brief (the default) must NOT inline the
    // 60-name public_tools list in the human-readable text. The list
    // does not change within a session and is also exposed in
    // `data.public_tools`, so spending ~700 chars per status call on
    // it wastes the LLM context. mode=verbose keeps the list inline.
    let server = create_test_server();
    let brief = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(104001)),
        })
        .unwrap()
        .result
        .unwrap();
    let brief_content = brief["content"][0]["text"].as_str().unwrap();
    // The brief surface must summarize tool count, not enumerate every
    // tool name. The presence of "soll_manager, infer_soll_mutation"
    // (a stable adjacent pair from the catalog) is a good signal that
    // the list was inlined; a brief response should not contain it.
    assert!(
        !brief_content.contains("soll_manager, infer_soll_mutation"),
        "brief mode must not inline the full public_tools list; got: {brief_content}"
    );
    assert!(
        brief_content.contains("Public tools count:")
            || brief_content.contains("public_tools count")
            || brief_content.contains("count:"),
        "brief mode must show a tool count summary or pointer; got: {brief_content}"
    );
    // data.public_tools must remain always-on for machine consumers.
    let data_tools = brief["data"]["public_tools"]
        .as_array()
        .expect("data.public_tools must be present even in brief mode");
    assert!(
        data_tools.len() >= 30,
        "data.public_tools should still enumerate the catalog; got {} entries",
        data_tools.len()
    );

    let verbose = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "verbose" }
            })),
            id: Some(json!(104002)),
        })
        .unwrap()
        .result
        .unwrap();
    let verbose_content = verbose["content"][0]["text"].as_str().unwrap();
    // Verbose must include the inline list.
    assert!(
        verbose_content.contains("**Public tools:**"),
        "verbose mode must inline Public tools header; got: {verbose_content}"
    );
}

#[test]
fn test_status_brief_text_surfaces_trust_boundary_and_next_best_action() {
    // REQ-AXO-042 — `status mode=brief` text rendering must expose
    // `Trust boundary:` and `Next best action:` so an LLM reading the
    // markdown can act without parsing raw `data.truth_cockpit`. Before
    // this, the text only carried low-level signals (drain_state, IST
    // freshness, vector backlog) and the LLM had to derive the next
    // tool itself.
    let server = create_test_server();
    let brief = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(420010)),
        })
        .unwrap()
        .result
        .unwrap();
    let text = brief["content"][0]["text"].as_str().unwrap();
    // REQ-AXO-901871 — the brief surfaces a usability-first IST signal, not a
    // `stale`/`degraded`/`blocker` alarm. It leads with `IST reads:` (usable /
    // usable with lag / live) and affirms which structural tools are valid, so
    // an LLM uses them instead of declining on a process-liveness flag.
    assert!(
        text.contains("**IST reads:**"),
        "brief text must surface the IST reads usability signal; got: {text}"
    );
    assert!(
        text.contains("**Structural tools"),
        "brief text must affirm which structural tools are valid; got: {text}"
    );
}
/// REQ-AXO-902458 — la porte de handoff VOYAIT les violations de règles et ne
/// bloquait pas : `axon_handoff_check` faisait `warns += 1`, jamais
/// `fails += 1`. Avec 484 violations sur 22 projets, elle rendait « WARN, 0 fail ».
///
/// La directive opérateur — « le handoff ne peut pas se faire tant que les règles
/// ne sont pas respectées », répétée trois fois — ne vivait que dans une practice
/// et dans la mémoire du LLM qui la lit. `GUI-PRO-118` : un geste qu'il faut se
/// rappeler d'appliquer n'est pas une porte, c'est une intention.
///
/// Les DEUX natures sont distinguées, et c'est le cœur du test : une violation de
/// RÈGLE est un `fail` (quelqu'un l'a posée exprès) ; une dette préexistante que
/// NULLE règle ne mandate reste un `warn` (personne n'a décidé qu'elle bloque).
#[test]
fn test_handoff_check_fails_on_a_declarative_rule_violation_but_only_warns_on_ungoverned_debt() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();
    let register = |code: &str| {
        exec(&format!(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
             VALUES ('{code}', '/tmp/{code}', '{code}') ON CONFLICT (project_code) DO NOTHING"
        ));
    };
    let status_of = |code: &str| -> String {
        let result = server
            .axon_handoff_check(&json!({ "project_code": code }))
            .expect("handoff_check doit répondre");
        result["data"]["checks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .find(|c| c["check"].as_str() == Some("soll_validate"))
            .and_then(|c| c["status"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("check soll_validate absent de {result}"))
    };

    // ── Projet PROPRE — le contrôle positif, écrit EN PREMIER ──────────────
    // Sans lui, une porte qui rendrait `fail` sur TOUT passerait l'assertion
    // suivante sans rien mesurer.
    register("HNS");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-HNS-001', 'Vision', 'HNS', 'Nord HNS', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-HNS-001', 'Pillar', 'HNS', 'Pilier HNS', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-HNS-001', 'Requirement', 'HNS', 'Exigence saine HNS', 'x', 'planned', '{\"acceptance_criteria\":[\"le test passe\"]}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('PIL-HNS-001', 'VIS-HNS-001', 'EPITOMIZES', 'HNS')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('REQ-HNS-001', 'PIL-HNS-001', 'BELONGS_TO', 'HNS')");
    exec("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref) VALUES ('TRC-HNS-001', 'requirement', 'REQ-HNS-001', 'file', 'src/lib.rs')");
    assert_eq!(
        status_of("HNS"),
        "pass",
        "un graphe qui respecte les règles doit passer — sinon la porte est \
         infranchissable pour une raison que personne n'a décidée"
    );

    // ── Projet en VIOLATION de règle → doit BLOQUER ────────────────────────
    // `MIL-HNR-900` est `superseded` et rien ne pointe vers lui : c'est
    // `GUI-PRO-125`, une règle posée exprès.
    register("HNR");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-HNR-900', 'Milestone', 'HNR', 'Retire sans remplacant', 'x', 'superseded', '{}')");
    assert_eq!(
        status_of("HNR"),
        "fail",
        "une violation de RÈGLE doit bloquer le handoff : la règle a été posée \
         exprès, et la directive opérateur l'exige"
    );

    // ── Dette NON gouvernée par une règle → reste un avertissement ─────────
    // Une exigence `delivered` sans preuve est signalée par le gate dédié
    // `delivered_without_evidence`, mais AUCUNE règle déclarative ne la
    // mandate. La confondre avec une violation de règle rendrait la porte
    // infranchissable sur une dette que personne n'a décidé de bloquer.
    register("HNT");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-HNT-001', 'Vision', 'HNT', 'Nord HNT', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-HNT-001', 'Pillar', 'HNT', 'Pilier HNT', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('PIL-HNT-001', 'VIS-HNT-001', 'EPITOMIZES', 'HNT')");
    // Une paire de relation ILLÉGALE : `relation_policy_for_pair` n'admet pas
    // VAL -> PIL. C'est signalé par `relation_policy_violations`, et AUCUNE
    // règle déclarative ne le mandate — les 36 paires sont encore en dur.
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-HNT-001', 'Validation', 'HNT', 'Validation HNT', 'x', 'passed', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('VAL-HNT-001', 'PIL-HNT-001', 'VERIFIES', 'HNT')");
    assert_eq!(
        status_of("HNT"),
        "warn",
        "une incohérence qu'AUCUNE règle ne gouverne ne doit pas bloquer : seule \
         une règle posée exprès a ce pouvoir"
    );
}


/// REQ-AXO-902250 + REQ-AXO-902358 — GUI-PRO-028's THREE SOLL hard gates now run
/// INSIDE `axon_handoff_check` instead of being raw SQL the LLM must retype at
/// every handoff of every project (session 104 mistyped one: `column e.src does
/// not exist`). Gate 3 (`requirement_without_milestone`, the couverture half) was
/// the last one still hand-typed: without it the tool returned PASS while an open
/// REQ had no milestone parent (Originator VPC, inbox msg 10522).
///
/// The case this pins is the DELIBERATE DIVERGENCE from the prescribed query:
/// that query only excludes delivered/completed/superseded, so it re-flags
/// `rejected` / `deferred` / `archived` milestones forever. Those are terminal
/// states set on purpose — flipping them to green a gate would falsify the record.
#[test]
fn test_handoff_check_runs_soll_gates_and_spares_deliberate_terminal_states() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    // A delivered REQ with NO evidence → must be flagged.
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-HND-001', 'Requirement', 'HND', 'no evidence', 'x', 'delivered', '{}')");
    // A delivered REQ WITH evidence → must NOT be flagged.
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-HND-002', 'Requirement', 'HND', 'has evidence', 'x', 'delivered', '{}')");
    // `id` is not auto-generated on this table — supply it explicitly.
    exec("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref) VALUES ('TRC-HND-001', 'requirement', 'REQ-HND-002', 'file', 'src/lib.rs')");
    // A REJECTED milestone whose only child is terminal → deliberate state, must
    // NOT be flagged (the divergence under test).
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-HND-900', 'Milestone', 'HND', 'rejected mil', 'x', 'rejected', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('MIL-HND-900', 'REQ-HND-002', 'TARGETS', 'HND')");
    // REQ-AXO-902358 — Gate 3 (couverture): an OPEN REQ with NO milestone parent
    // must be flagged; an OPEN REQ WITH one (MIL --TARGETS--> REQ) must not.
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-HND-003', 'Requirement', 'HND', 'orphan open req', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-HND-004', 'Requirement', 'HND', 'covered open req', 'x', 'planned', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-HND-901', 'Milestone', 'HND', 'live mil', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('MIL-HND-901', 'REQ-HND-004', 'TARGETS', 'HND')");

    let result = server
        .axon_handoff_check(&json!({ "project_code": "HND" }))
        .expect("handoff_check must answer");
    let checks = result["data"]["checks"].as_array().cloned().unwrap_or_default();
    let find = |name: &str| {
        checks
            .iter()
            .find(|c| c["check"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("missing check {name} in {checks:?}"))
            .clone()
    };

    let ev = find("delivered_without_evidence");
    let offenders: Vec<String> = ev["offenders"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(
        offenders.contains(&"REQ-HND-001".to_string()),
        "a delivered REQ without evidence must be flagged, got {offenders:?}"
    );
    assert!(
        !offenders.contains(&"REQ-HND-002".to_string()),
        "a delivered REQ WITH evidence must not be flagged, got {offenders:?}"
    );

    let mil = find("milestone_reconciliation");
    let mil_offenders: Vec<String> = mil["offenders"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(
        !mil_offenders.contains(&"MIL-HND-900".to_string()),
        "a REJECTED milestone is a deliberate decision — greening a gate must never \
         push the operator to falsify it; got {mil_offenders:?}"
    );

    // REQ-AXO-902358 — Gate 3: the couverture check that used to be PASS-invisible.
    let orphan = find("requirement_without_milestone");
    let orphan_offenders: Vec<String> = orphan["offenders"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(
        orphan_offenders.contains(&"REQ-HND-003".to_string()),
        "an OPEN REQ with no milestone parent must be flagged, got {orphan_offenders:?}"
    );
    assert!(
        !orphan_offenders.contains(&"REQ-HND-004".to_string()),
        "an OPEN REQ WITH a milestone parent (MIL --TARGETS--> REQ) must not be flagged, \
         got {orphan_offenders:?}"
    );
}

/// REQ-AXO-902239 — an allow-listed tool called WITHOUT its project argument must
/// auto-resolve from the cwd, exactly as `query`/`inspect` always have.
///
/// Ground truth for this REQ: `soll_acyclic_audit` showed 82/82 failed calls in
/// telemetry, not because it is broken (with the argument it answers fine) but
/// because LLMs call it the way they call `query`. The rejection came from the
/// handler, so the fix must land before the handler reads the arguments.
#[test]
fn test_allow_listed_tool_auto_resolves_project_code_from_cwd() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-scope-fixture"))
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axo-scope-fixture");
    }

    // No `project_code` in the arguments at all.
    let result = server
        .execute_tool_direct("soll_acyclic_audit", &json!({}))
        .expect("tool must answer");
    let text = result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !text.contains("requires a project_code"),
        "the project scope must be auto-resolved, got: {text}"
    );

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

/// REQ-AXO-902239 — the EXCLUSION list is the safety-critical half.
///
/// For a whole family of tools an ABSENT project means "every project".
/// `embedding_status` returns a per-project rollup; auto-injecting a cwd-derived
/// scope there would raise no error, it would SILENTLY NARROW the answer — a
/// regression strictly worse than the visible failure this REQ fixes. This test is
/// the guard that keeps the allow-list from quietly growing into a blanket
/// injection.
#[test]
fn test_wildcard_scoped_tools_are_not_narrowed_by_auto_resolution() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-scope-fixture"))
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axo-scope-fixture");
    }

    // The injector must leave the arguments untouched for an excluded tool …
    let wildcard_args = json!({});
    let untouched = server.with_resolved_project_scope("embedding_status", &wildcard_args);
    assert!(
        untouched.get("project").is_none() && untouched.get("project_code").is_none(),
        "embedding_status must keep its wildcard scope, got {untouched}"
    );
    // … and likewise for a SOLL mutation, where writing into a GUESSED project would
    // be irreversible.
    let mutation_args = json!({"action": "create"});
    let mutation = server.with_resolved_project_scope("soll_manager", &mutation_args);
    assert!(
        mutation.get("project_code").is_none(),
        "soll_manager must never receive a guessed project_code, got {mutation}"
    );

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

/// REQ-AXO-902239 — an EXPLICIT argument always wins, and the injection is
/// observable.
#[test]
fn test_explicit_project_code_is_never_overwritten_and_source_is_reported() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-scope-fixture"))
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axo-scope-fixture");
    }

    let explicit_args = json!({"project_code": "BKS"});
    let explicit = server.with_resolved_project_scope("wiring", &explicit_args);
    assert_eq!(
        explicit.get("project_code").and_then(Value::as_str),
        Some("BKS"),
        "an explicit scope must never be overwritten by the cwd"
    );
    assert!(
        explicit.get("project_code_source").is_none(),
        "no injection happened, so nothing to report"
    );

    // Omitted → injected, and TAGGED so telemetry can measure the recovery rate.
    let omitted_args = json!({});
    let injected = server.with_resolved_project_scope("wiring", &omitted_args);
    assert_eq!(
        injected.get("project_code").and_then(Value::as_str),
        Some("AXO")
    );
    assert_eq!(
        injected.get("project_code_source").and_then(Value::as_str),
        Some("cwd_auto"),
        "without this tag the effect of the fix is unmeasurable"
    );

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

/// REQ-AXO-902467 — doleance APS #197, severite BLOCKING, reproduite deux fois.
///
/// Quand le projet n'est PAS resolvable, 31 sites de la surface MCP retombent
/// sur `"AXO"` en dur. Le brain live etant un singleton dont le cwd propre est
/// le depot Axon, tout appel non resolu atterrit silencieusement sur AXO — et
/// l'appelant recoit un paquet BIEN FORME sur le MAUVAIS projet, sans rien qui
/// le lui dise.
///
/// Un refus coute un tour ; une mauvaise ancre coute une session, et contamine
/// tout ce qui est ecrit ensuite. C'est aussi une atteinte a PIL-AXO-001 : une
/// seule verite runtime, observable identiquement par tous les consommateurs.
///
/// `re_anchor` est le pire cas et celui que la doleance cite en premier : il ne
/// resout MEME PAS, il code `"AXO"` directement (tools_skill.rs:704).
#[test]
fn an_unresolvable_project_is_refused_not_silently_answered_as_axo() {
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axo-fixture"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("BookingSystem"), Some("/home/test/bks-fixture"))
        .unwrap();

    // Un cwd qui n'appartient a AUCUN projet enregistre : la resolution ne peut
    // pas trancher, et c'est le cas ou la doleance mord.
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/tmp/appartient-a-aucun-projet");
    }
    assert!(
        server.auto_resolve_project_code_str().is_none(),
        "prealable du test : ce cwd doit etre non resolvable"
    );

    // `re_anchor` sans project_code explicite. Il doit REFUSER en nommant les
    // candidats, jamais repondre sur AXO.
    let response = server
        .execute_tool_direct("re_anchor", &json!({ "reason": "post_compact" }))
        .expect("re_anchor repond");

    let is_error = response
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let rendered = serde_json::to_string(&response).unwrap_or_default();

    assert!(
        is_error,
        "re_anchor a REPONDU sur un projet qu'il n'a pas resolu — c'est la doleance \
         APS #197 : un paquet bien forme sur le mauvais projet.\n---\n{rendered}"
    );

    let status = response["data"]["status"].as_str().unwrap_or_default();
    assert!(
        status == "missing_project_code" || status == "wrong_project_scope",
        "le refus doit porter un `status` canonique deja cable dans guidance.rs \
         (missing_project_code / wrong_project_scope), pas un statut ad hoc : {status:?}"
    );

    // Demande n. 2 du tenant : NOMMER les candidats. Un refus qui ne dit pas ou
    // aller est une impasse (PIL-AXO-002).
    assert!(
        rendered.contains("BKS") || rendered.contains("project_registry_lookup"),
        "le refus doit nommer les projets candidats ou l'outil qui les liste — \
         sinon l'appelant est dans une impasse.\n---\n{rendered}"
    );

    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_auto_resolve_project_code_str_helper() {
    // REQ-AXO-089 (helper coverage) — auto_resolve_project_code_str is
    // the canonical helper used by retrieve_context, query, and
    // inspect to map AXON_PROJECT_ROOT (or cwd) onto a single
    // registered project_code. Direct unit coverage so the contract
    // does not depend on the indexed-surface fixtures any individual
    // tool needs to exercise its full code path.
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axon-cwd-fixture"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("BookingSystem"), Some("/home/test/bks-other"))
        .unwrap();
    // Exact match returns the code.
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/home/test/axon-cwd-fixture");
    }
    assert_eq!(
        server.auto_resolve_project_code_str().as_deref(),
        Some("AXO")
    );
    // Subdirectory of a registered path also resolves.
    unsafe {
        std::env::set_var(
            "AXON_PROJECT_ROOT",
            "/home/test/axon-cwd-fixture/src/axon-core",
        );
    }
    assert_eq!(
        server.auto_resolve_project_code_str().as_deref(),
        Some("AXO")
    );
    // Unrelated path returns None (workspace fallback at the call site).
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/tmp/unrelated");
    }
    assert!(server.auto_resolve_project_code_str().is_none());
    // Empty env returns None.
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "");
    }
    // Empty AXON_PROJECT_ROOT falls through to current_dir; we cannot
    // assert deterministically what that is, so just confirm the helper
    // does not panic when fed back-to-back changes.
    let _ = server.auto_resolve_project_code_str();
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_retrieve_context_falls_back_to_workspace_when_cwd_unmatched() {
    // REQ-AXO-089 — when AXON_PROJECT_ROOT doesn't match any
    // registered project, retrieve_context must fall back to
    // workspace:* rather than fail or invent a code. This preserves
    // the historic behaviour for callers running from outside any
    // registered project (e.g., a fresh worktree or a temp dir).
    let _guard = env_lock();
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/test/axon-cwd-fixture"))
        .unwrap();
    unsafe {
        std::env::set_var("AXON_PROJECT_ROOT", "/tmp/unrelated-path");
    }
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "anything goes here" }
            })),
            id: Some(json!(89002)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("workspace:*"),
        "scope must fall back to workspace:* when cwd does not match any registered project; got: {content}"
    );
    unsafe {
        std::env::remove_var("AXON_PROJECT_ROOT");
    }
}

#[test]
fn test_retrieve_context_empty_question_returns_recovery_contract() {
    // REQ-AXO-043 — empty `question` previously returned a bare error
    // string with no operator_guidance, no next_action, and no example.
    // Verify the structured contract.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "retrieve_context",
                "arguments": { "question": "   " }
            })),
            id: Some(json!(43101)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response["isError"].as_bool(), Some(true));
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("non-empty") && content.contains("question"),
        "content must explain the missing field: {content}"
    );
    assert!(
        content.contains("example") || content.contains("Pass"),
        "content must include guidance toward a valid call: {content}"
    );

    let data = &response["data"];
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    assert_eq!(data["missing_field"].as_str(), Some("question"));
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert!(!actions.is_empty(), "next_best_actions must be non-empty");
    let follow_up = data["operator_guidance"]["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools");
    let follow_up_strs: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_strs.contains(&"inspect") || follow_up_strs.contains(&"query"),
        "follow_up_tools must point to inspect/query: {follow_up_strs:?}"
    );
}

#[test]
fn test_brain_only_resume_vectorization_stays_unavailable() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "resume_vectorization",
                "arguments": {}
            })),
            id: Some(json!(2298)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(content.contains("resume_vectorization"), "{content}");
    assert!(content.contains("unavailable"), "{content}");
    assert!(content.contains("public brain authority"), "{content}");
    assert!(content.contains("active indexer authority"), "{content}");

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
    }
}

#[test]
fn test_pre_flight_check_alias_uses_dry_run_commit_work() {
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_pre_flight_check",
                "arguments": {
                    "diff_paths": ["docs/skills/axon-engineering-protocol/SKILL.md"]
                }
            })),
            id: Some(json!(2201)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = response["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Dry Run"), "{text}");
}

#[test]
fn test_status_reports_public_surface_and_runtime_truth() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE", "false");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_public_surface_and_runtime_truth",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);
    let _tempdir = tempdir().unwrap();
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2202)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let public_tools = data["public_tools"].as_array().unwrap();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(public_tool_names.contains(&"status"));
    assert!(public_tool_names.contains(&"mcp_surface_diagnostics"));
    assert!(public_tool_names.contains(&"project_status"));
    assert!(public_tool_names.contains(&"project_registry_lookup"));
    assert!(public_tool_names.contains(&"soll_relation_schema"));
    assert!(public_tool_names.contains(&"why"));
    assert!(public_tool_names.contains(&"path"));
    assert!(public_tool_names.contains(&"anomalies"));
    assert!(public_tool_names.contains(&"batch"));
    assert!(public_tool_names.contains(&"job_status"));
    assert!(public_tool_names.contains(&"query"));
    assert!(public_tool_names.contains(&"inspect"));
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"impact"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(!public_tool_names.contains(&"resume_vectorization"));
    assert!(!public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"sql"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(!public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));
    assert!(data
        .get("runtime_mode")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data
        .get("runtime_profile")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data
        .get("truth_status")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(data["truth_cockpit"].as_object().is_some());
    assert!(data["truth_cockpit"]["next_best_action"]["tool"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["freshness"]["state"]
        .as_str()
        .is_some());
    assert!(data["truth_cockpit"]["proof_gaps"].is_array());
    assert_eq!(
        data["next_action"],
        data["truth_cockpit"]["next_best_action"]
    );
    assert_runtime_authority_roles(
        &data["runtime_authority"]["runtime_state"],
        AxonProcessRole::Indexer,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert!(
        data["runtime_authority"]["runtime_state"]["system_converged"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["runtime_state"]["indexer_feed"]["state"]
            .as_str()
            .is_some()
    );
    // REQ-AXO-901859 — with no PG heartbeat seeded, indexer liveness is
    // fail-loud absent: last_good_payload_at_ms is canonically null. The
    // contract is that the KEY is always present (u64 when a heartbeat
    // exists, null when absent), never missing.
    {
        let v =
            &data["runtime_authority"]["runtime_state"]["indexer_feed"]["last_good_payload_at_ms"];
        assert!(
            v.is_u64() || v.is_null(),
            "last_good_payload_at_ms must be u64 or null, got {v:?}"
        );
    }
    assert!(
        data["runtime_authority"]["runtime_state"]["ist_snapshot"]["state"]
            .as_str()
            .is_some()
    );
    assert!(data["availability"]["degraded_notes"].as_array().is_some());
    assert_eq!(
        data["async_contract"]["canonical_follow_up_tool"].as_str(),
        Some("job_status")
    );
    assert_eq!(data["async_policy"]["mode"].as_str(), Some("allowlist"));
    assert_eq!(
        data["async_policy"]["sync_by_default"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["async_policy"]["latency_target_p95_ms"].as_i64(),
        Some(200)
    );
    let allowlisted_tools = data["async_policy"]["allowlisted_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowlisted_tools.contains(&"restore_soll"));
    assert!(allowlisted_tools.contains(&"soll_apply_plan"));
    assert!(!allowlisted_tools.contains(&"resume_vectorization"));
    let monitored_sync_tools = data["async_policy"]["monitored_sync_mutation_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(monitored_sync_tools.contains(&"soll_commit_revision"));
    assert_eq!(
        data["utility_first_scheduler"]["state"].as_str(),
        Some("balanced_drain")
    );
    assert!(data["utility_first_scheduler"]["reason"].as_str().is_some());
    assert!(data["utility_first_scheduler"]["ready_reserve_target"]
        .as_u64()
        .is_some());
    assert_eq!(
        data["async_contract"]["stale_client_binding_possible"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["canonical_sources"]["soll_export"]["reimportable"].as_bool(),
        Some(true)
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["target"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["effective"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["clamp_visible"]
            .as_bool()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_workers"]["authority_state"].as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["effective"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["graph_workers"]["authority_state"].as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["chunk_batch_size"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["file_vectorization_batch_size"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["seed"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["target"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["effective"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_ready_queue_depth")
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_ready_queue_depth"]["authority_state"]
            .as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_persist_queue_bound"]["seed"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_persist_queue_bound"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_persist_queue_depth")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["vector_max_inflight_persists"]["seed"]
            .as_u64()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["vector_max_inflight_persists"]
            ["effective_source"]
            .as_str(),
        Some("service_guard.current_persist_claims")
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["queue_persist_effective_semantics"]
            ["vector_ready_queue_depth"]
            .as_str(),
        Some("observed_current_queue_depth_not_capacity")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["seed"]["sleep_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["seed"]["profile"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["target"]["idle_sleep_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["effective"]["pause"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["controller_state"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["lane_parameters"]["semantic_cadence"]["authority_state"]
            .as_str(),
        Some("partially_unified")
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["gpu_vector_lease"]["exclusive_required"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["lane_parameters"]["gpu_vector_lease"]["path"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["runtime_authority"]["limiting_factors"]["available_in_mode"].as_str(),
        Some("full")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["authority_state"].as_str(),
        Some("transitional")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["wake_contract_state"].as_str(),
        Some("fragmented")
    );
    assert_eq!(
        data["runtime_authority"]["quiescent_state"]["wake_observability_state"].as_str(),
        Some("partial")
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["graph_backlog_depth"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["graph_projection_queue_depth"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["operator_focus"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["focus_recommendation"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["confidence"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["wake_noise_level"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["dominant_wake_share_pct"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["measurement_readiness"]
            .as_str()
            .is_some()
    );
    assert!(data["runtime_authority"]["quiescent_state"]["diagnosis"]
        ["recommended_next_measurement"]
        .as_str()
        .is_some());
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["qualification_verdict"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["qualification_reason"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["actionable_now"]
            .as_bool()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["diagnosis"]["blocking_factors"]
            .as_array()
            .is_some()
    );
    // REQ-AXO-901870 — the `reader_refresh` loop-interval was removed with
    // the reader-replica refresher; assert the surviving optimizer_loop key.
    assert!(
        data["runtime_authority"]["quiescent_state"]["loop_intervals_ms"]["optimizer_loop"]
            .is_u64()
            || data["runtime_authority"]["quiescent_state"]["loop_intervals_ms"]["optimizer_loop"]
                .is_null()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["wakeups_last_60s"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_wakeup_at_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["resume_latency_p95_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["useful_resume_latency_p95_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_quiescent_exit_reason"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["last_wake_source"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]["dominant_wake_source"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["last_background_wake_detail"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["dominant_background_wake_detail"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["background_wake_ingress_promoter_total"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["wake_activity"]
            ["background_wake_autonomous_ingestor_total"]
            .as_u64()
            .is_some()
    );
    assert!(data["runtime_authority"]["quiescent_state"]["diagnosis"]
        ["dominant_background_wake_detail"]
        .as_str()
        .is_some());
    assert!(
        data["runtime_authority"]["quiescent_state"]["lane_liveness"]
            ["vector_worker_heartbeat_age_ms"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["lane_liveness"]["vector_lane_state"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["observed_residual_work"]
            ["ready_queue_depth_current"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["measurement_window_sec"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]["state"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["recommendation"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["files_vector_ready_last_minute"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["burn_rate"]
            ["chunks_embedded_last_minute"]
            .as_u64()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_requested"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["provider_effective"]
            .as_str()
            .is_some()
    );
    assert!(
        data["runtime_authority"]["quiescent_state"]["backlog_drain"]["gpu_access_policy"]
            .as_str()
            .is_some()
    );

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
    }
    service_guard::set_runtime_truth_feed_for_tests(
        Some(1_000),
        Some(900),
        50,
        Some("indexer_feed_heartbeat_stale"),
    );
    let degraded = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();
    let degraded_data = degraded.get("data").unwrap();
    assert_eq!(
        degraded_data["runtime_authority"]["runtime_state"]["indexer_feed"]["stale"].as_bool(),
        Some(true)
    );
    assert_eq!(
        degraded_data["runtime_authority"]["runtime_state"]["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(degraded_data["truth_status"].as_str(), Some("degraded"));
    assert!(degraded_data["availability"]["degraded_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str().is_some()));

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
    }
    let now_ms = now_ms_for_tests();
    service_guard::set_runtime_truth_feed_for_tests(
        Some(now_ms),
        Some(now_ms.saturating_sub(100)),
        60_000,
        Some("indexer_feed_partial_runtime_truth"),
    );
    let degraded_but_fresh = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2204)),
        })
        .unwrap()
        .result
        .unwrap();
    let degraded_but_fresh_data = degraded_but_fresh.get("data").unwrap();
    // REQ-AXO-901859 — indexer_feed derives SOLELY from the PG
    // EmbedderLifecycleHeartbeat (single source of truth, no bridge/file
    // fallback). With no heartbeat row present, the canonical liveness
    // verdict is heartbeat-absent: state == "stale", stale == true ("not
    // provably alive — say so loudly"). The runtime-truth-bridge feed set
    // above drives truth_status / degraded_notes, asserted below.
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["indexer_feed"]["state"]
            .as_str(),
        Some("stale")
    );
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["indexer_feed"]["stale"]
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        degraded_but_fresh_data["runtime_authority"]["runtime_state"]["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        degraded_but_fresh_data["truth_status"].as_str(),
        Some("degraded")
    );
    // REQ-AXO-901859 — with the indexer_feed deriving SOLELY from the PG
    // EmbedderLifecycleHeartbeat and no heartbeat row present, the canonical
    // degraded note for the feed is `indexer_heartbeat_absent` (the reason
    // carried through tools_framework_runtime_status::degraded_notes). The
    // superseded bridge-fed note `indexer_feed_partial_runtime_truth` no
    // longer exists in the product (single-source-of-truth re-canonicalization).
    assert!(degraded_but_fresh_data["availability"]["degraded_notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some("indexer_heartbeat_absent")));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
        std::env::remove_var("AXON_GPU_VECTOR_EXCLUSIVE_LEASE");
    }
}

#[test]
fn test_initialize_reports_brain_server_identity_when_shadow_role_is_brain() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_initialize_reports_brain_server_identity_when_shadow_role_is_brain",
        );
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "initialize".to_string(),
            params: Some(json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "codex-test", "version": "0.0.0" }
            })),
            id: Some(json!(2201)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(response["protocolVersion"].as_str(), Some("2025-11-25"));
    assert_eq!(response["serverInfo"]["name"].as_str(), Some("axon-brain"));
    assert_eq!(response["serverInfo"]["version"].as_str(), Some("2.2.0"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_reports_brain_and_indexer_authorities() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
    let _tempdir = tempdir().unwrap();
    let server = create_test_server();

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "brain");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "1");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_brain_and_indexer_authorities_brain",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let brain_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2207)),
        })
        .unwrap()
        .result
        .unwrap();

    let brain_runtime_state = &brain_response["data"]["runtime_authority"]["runtime_state"];
    assert_runtime_authority_roles(
        brain_runtime_state,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert_eq!(brain_runtime_state["brain_ready"].as_bool(), Some(true));
    assert_eq!(brain_runtime_state["indexer_ready"].as_bool(), Some(false));
    assert_eq!(
        brain_runtime_state["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        brain_response["data"]["truth_status"].as_str(),
        Some("degraded")
    );

    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "indexer");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_brain_and_indexer_authorities_indexer",
        );
    }
    service_guard::record_runtime_truth_bridge_dispatch(None);

    let indexer_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2208)),
        })
        .unwrap()
        .result
        .unwrap();

    let indexer_runtime_state = &indexer_response["data"]["runtime_authority"]["runtime_state"];
    assert_runtime_authority_roles(
        indexer_runtime_state,
        AxonProcessRole::Indexer,
        AxonProcessRole::Brain,
        AxonProcessRole::Brain,
        AxonProcessRole::Indexer,
    );
    assert_eq!(indexer_runtime_state["brain_ready"].as_bool(), Some(false));
    assert_eq!(indexer_runtime_state["indexer_ready"].as_bool(), Some(true));
    assert_eq!(
        indexer_runtime_state["system_converged"].as_bool(),
        Some(false)
    );
    assert_eq!(
        indexer_response["data"]["truth_status"].as_str(),
        Some("degraded")
    );

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_status_exposes_tensorrt_ready_vector_pipeline_telemetry() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_exposes_tensorrt_ready_vector_pipeline_telemetry",
        );
        std::env::set_var("AXON_TENSORRT_CACHE_DIR", "/tmp/axon-tensorrt-cache");
    }
    service_guard::record_vector_prepare_reply_wait_ms(3);
    service_guard::record_vector_prepare_send_wait_ms(5);
    service_guard::record_vector_prepare_queue_wait_ms(7);
    service_guard::record_vector_gpu_idle_wait_ms(11);
    service_guard::record_vector_embed_breakdown(13, 17);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::DbWrite, 19);
    service_guard::record_vector_persist_send_wait_ms(23);
    service_guard::record_vector_persist_queue_wait_ms(29);
    service_guard::record_vector_stage_ms(service_guard::VectorStageKind::MarkDone, 31);
    service_guard::record_vector_finalize_send_wait_ms(37);
    service_guard::record_vector_finalize_queue_wait_ms(41);

    let _tempdir = tempdir().unwrap();
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(2203)),
        })
        .unwrap()
        .result
        .unwrap();

    let telemetry = &response["data"]["runtime_authority"]["vector_pipeline_telemetry"];
    assert_eq!(
        telemetry["contract"].as_str(),
        Some("tensorrt_ready_vector_pipeline_v1")
    );
    assert_eq!(telemetry["production_lanes"][0].as_str(), Some("graph"));
    assert_eq!(telemetry["production_lanes"][1].as_str(), Some("vector"));
    assert_eq!(telemetry["stage_totals"]["prepare_ms"].as_u64(), Some(15));
    assert_eq!(
        telemetry["stage_totals"]["ready_wait_ms"].as_u64(),
        Some(11)
    );
    assert_eq!(telemetry["stage_totals"]["inference_ms"].as_u64(), Some(13));
    assert_eq!(
        telemetry["stage_totals"]["output_extract_ms"].as_u64(),
        Some(17)
    );
    assert_eq!(telemetry["stage_totals"]["persist_ms"].as_u64(), Some(71));
    assert_eq!(
        telemetry["provider"]["tensorrt_cache_dir"].as_str(),
        Some("/tmp/axon-tensorrt-cache")
    );
    assert!(telemetry["provider"]["effective_strategy"].is_string());
    assert!(telemetry["provider"]["fallback_count"].is_u64());

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
        std::env::remove_var("AXON_TENSORRT_CACHE_DIR");
    }
}

#[test]
fn test_status_indexer_omits_soll_mcp_job_counts() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_RUNTIME_SHADOW_ROLE", "indexer");
        std::env::set_var("AXON_SPLIT_SHADOW_ONLY", "0");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_indexer_omits_soll_mcp_job_counts",
        );
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["runtime_authority"]["runtime_state"]["process_role"].as_str(),
        Some(AxonProcessRole::Indexer.as_str())
    );
    assert_eq!(data["job_counts"].as_array().map(Vec::len), Some(0));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_SHADOW_ROLE");
        std::env::remove_var("AXON_SPLIT_SHADOW_ONLY");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

// REQ-AXO-901893 (LEGACY FEED PURGE) — test_status_reports_admission_exclusion_diagnostics
// was removed: it asserted the admission_controller's ingress promotion counters
// (admission_last_durably_persisted_count / admission_completion_diagnostics),
// which were ripped with the ingress_buffer. Admission now gates on
// persisted_file_pending + WIP only (Watchman + DBQ-A feed pipeline A directly).

#[test]
fn test_graph_backlog_blocks_vector_priority_until_graph_ready_advances() {
    let _guard = env_lock();
    // REQ-AXO-902274 — ce test lit/écrit l'état PROCESS-GLOBAL (service_guard, UtilityFirstScheduler).
    // Sans ce verrou, un test concurrent le réinitialise en plein milieu : `semantic_policy`
    // rendait `gpu_cadence_refill` au lieu de `balanced_drain`, vert en isolation, rouge en
    // parallèle. Ordre env → service_guard, identique partout (embedder/tests.rs) : c'est
    // l'uniformité de l'ordre qui écarte le deadlock.
    let _sg_guard = crate::service_guard::lock_for_tests();
    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();

    service_guard::record_vector_ready_queue_depth(0);
    service_guard::record_vector_prepare_inflight_depth(0);
    service_guard::record_vector_persist_queue_depth(0);
    // REQ-AXO-901674 — record_graph_vector_priority_context removed (was no-op
    // since FVQ/GPQ tables dropped slice-5d). Scheduler diagnostics derive
    // graph-vs-vector backlog from current_utility_first_scheduler_diagnostics
    // inputs directly.

    let first =
        current_utility_first_scheduler_diagnostics(1, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(first.state.as_str(), "balanced_drain");
    assert_eq!(first.reason, "semantic_underfed");
    assert!(first.semantic_underfeed, "{first:?}");
    assert_eq!(
        service_guard::vector_runtime_metrics().ready_queue_depth_current,
        0
    );

    let held =
        current_utility_first_scheduler_diagnostics(0, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(held.state.as_str(), "balanced_drain");

    let released =
        current_utility_first_scheduler_diagnostics(0, 16, service_guard::ServicePressure::Healthy);
    assert_eq!(released.state.as_str(), "balanced_drain");
    assert!(released.semantic_underfeed, "{released:?}");

    service_guard::reset_for_tests();
    reset_utility_first_scheduler_for_tests();
}

// REQ-AXO-901653 slice-5c — `test_vectorization_admits_only_graph_ready_files` deleted ;
// exercised dropped enqueue_file_vectorization_refresh + public.File/FileVectorizationQueue.

#[test]
fn test_status_reports_retrieve_context_in_public_surface_when_full_autonomous() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "indexer_full");
        std::env::set_var("AXON_ENABLE_AUTONOMOUS_INGESTOR", "true");
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(22021)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let public_tools = data["public_tools"].as_array().unwrap();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"batch"));
    assert!(public_tool_names.contains(&"job_status"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(!public_tool_names.contains(&"resume_vectorization"));
    assert!(!public_tool_names.contains(&"refine_lattice"));
    assert!(public_tool_names.contains(&"sql"));
    assert!(public_tool_names.contains(&"debug"));
    assert!(public_tool_names.contains(&"schema_overview"));
    assert!(!public_tool_names.contains(&"list_labels_tables"));
    assert!(public_tool_names.contains(&"query_examples"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_ENABLE_AUTONOMOUS_INGESTOR");
    }
}

#[test]
fn test_status_reports_information_surface_in_brain_only() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RUNTIME_MODE", "brain_only");
        std::env::set_var(
            "AXON_RUNTIME_IDENTITY",
            "test_status_reports_information_surface_in_brain_only",
        );
    }
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "status",
                "arguments": { "mode": "brief" }
            })),
            id: Some(json!(22022)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    let public_tools = data["public_tools"].as_array().unwrap();
    let public_tool_names = public_tools
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(public_tool_names.contains(&"query"));
    assert!(public_tool_names.contains(&"inspect"));
    assert!(public_tool_names.contains(&"retrieve_context"));
    assert!(public_tool_names.contains(&"impact"));
    assert!(public_tool_names.contains(&"health"));
    assert!(public_tool_names.contains(&"audit"));
    assert!(public_tool_names.contains(&"truth_check"));
    assert!(public_tool_names.contains(&"diagnose_indexing"));
    assert!(public_tool_names.contains(&"diff"));
    assert!(public_tool_names.contains(&"semantic_clones"));
    assert!(public_tool_names.contains(&"architectural_drift"));
    assert!(public_tool_names.contains(&"bidi_trace"));
    assert!(public_tool_names.contains(&"api_break_check"));
    assert!(public_tool_names.contains(&"simulate_mutation"));
    assert!(!public_tool_names.contains(&"resume_vectorization"));

    unsafe {
        std::env::remove_var("AXON_RUNTIME_MODE");
        std::env::remove_var("AXON_RUNTIME_IDENTITY");
    }
}

#[test]
fn test_mcp_surface_diagnostics_exposes_server_truth_and_binding_caveat() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_PUBLIC_HOST", "192.168.1.50");
        std::env::set_var("AXON_PUBLIC_HOST_SOURCE", "explicit");
        std::env::set_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE", "1");
        std::env::set_var("AXON_MCP_PUBLIC_URL", "http://192.168.1.50:44129/mcp");
        std::env::set_var("AXON_SQL_PUBLIC_URL", "http://192.168.1.50:44129/sql");
        std::env::set_var("AXON_DASHBOARD_PUBLIC_URL", "http://192.168.1.50:44127/");
    }

    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_surface_diagnostics",
                "arguments": { "mode": "json" }
            })),
            id: Some(json!(22022)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").unwrap();
    assert_eq!(
        data["async_contract"]["canonical_follow_up_tool"].as_str(),
        Some("job_status")
    );
    assert_eq!(data["async_policy"]["mode"].as_str(), Some("allowlist"));
    let allowlisted_tools = data["async_policy"]["allowlisted_tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowlisted_tools.contains(&"restore_soll"));
    assert!(allowlisted_tools.contains(&"soll_apply_plan"));
    assert!(!allowlisted_tools.contains(&"resume_vectorization"));
    assert_eq!(
        data["client_binding_notes"]["stale_client_binding_possible"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["client_binding_notes"]["session_freshness_status"].as_str(),
        Some("unknown_outside_server")
    );
    assert!(
        data["client_binding_notes"]["canonical_refresh_instruction"]
            .as_str()
            .unwrap_or_default()
            .contains("Refresh or reconnect")
    );
    assert_eq!(
        data["advertised_endpoints"]["available"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["advertised_endpoints"]["mcp_url"].as_str(),
        Some("http://192.168.1.50:44129/mcp")
    );
    assert_eq!(
        data["client_binding_notes"]["external_endpoint_rule"].as_str(),
        Some("Do not use instance_identity.*_url as an external endpoint. Isolated clients must prefer advertised_endpoints.* when available.")
    );
    let critical_tools = data["server_truth"]["critical_tools"].as_array().unwrap();
    assert!(critical_tools
        .iter()
        .any(|value| value.as_str() == Some("project_registry_lookup")));
    assert!(critical_tools
        .iter()
        .any(|value| value.as_str() == Some("axon_init_project")));

    unsafe {
        std::env::remove_var("AXON_PUBLIC_HOST");
        std::env::remove_var("AXON_PUBLIC_HOST_SOURCE");
        std::env::remove_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE");
        std::env::remove_var("AXON_MCP_PUBLIC_URL");
        std::env::remove_var("AXON_SQL_PUBLIC_URL");
        std::env::remove_var("AXON_DASHBOARD_PUBLIC_URL");
    }
}

#[test]
fn test_status_exposes_runtime_version_identity() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RELEASE_VERSION", "0.7.0");
        std::env::set_var("AXON_BUILD_ID", "v0.7.0-rc1-12-gabcdef");
        std::env::set_var("AXON_PACKAGE_VERSION", "0.7.0");
        std::env::set_var("AXON_INSTALL_GENERATION", "live-2026-04-18");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["runtime_version"]["release_version"].as_str(),
        Some("0.7.0")
    );
    assert_eq!(
        data["runtime_version"]["build_id"].as_str(),
        Some("v0.7.0-rc1-12-gabcdef")
    );
    assert_eq!(
        data["runtime_version"]["package_version"].as_str(),
        Some("0.7.0")
    );
    assert_eq!(
        data["runtime_version"]["install_generation"].as_str(),
        Some("live-2026-04-18")
    );

    unsafe {
        std::env::remove_var("AXON_RELEASE_VERSION");
        std::env::remove_var("AXON_BUILD_ID");
        std::env::remove_var("AXON_PACKAGE_VERSION");
        std::env::remove_var("AXON_INSTALL_GENERATION");
    }
}

#[test]
fn test_status_exposes_resource_policy_identity() {
    // RUNTIME-TUNING-SNAPSHOT-OK: `resource_policy.vector_workers` retombe sur
    // `local_vector_workers`, qui est un `std::env::var("AXON_VECTOR_WORKERS")`
    // direct (tools_framework_runtime_status.rs) — pas l'instantane memoise.
    // Y glisser un rafraichissement changerait silencieusement ce que ce test
    // mesure. (REQ-AXO-902414)
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_RESOURCE_PRIORITY", "critical");
        std::env::set_var("AXON_BACKGROUND_BUDGET_CLASS", "balanced");
        std::env::set_var("AXON_GPU_ACCESS_POLICY", "preferred");
        std::env::set_var("AXON_WATCHER_POLICY", "full");
        std::env::set_var("AXON_EMBEDDING_PROVIDER", "cpu");
        std::env::set_var("AXON_VECTOR_WORKERS", "2");
        std::env::set_var("AXON_GRAPH_WORKERS", "3");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["resource_policy"]["resource_priority"].as_str(),
        Some("critical")
    );
    assert_eq!(
        data["resource_policy"]["background_budget_class"].as_str(),
        Some("balanced")
    );
    assert_eq!(
        data["resource_policy"]["gpu_access_policy"].as_str(),
        Some("preferred")
    );
    assert_eq!(
        data["resource_policy"]["watcher_policy"].as_str(),
        Some("full")
    );
    assert_eq!(
        data["resource_policy"]["embedding_provider"].as_str(),
        Some("cpu")
    );
    assert_eq!(
        data["resource_policy"]["vector_workers"].as_str(),
        Some("2")
    );
    assert_eq!(data["resource_policy"]["graph_workers"].as_str(), Some("3"));

    unsafe {
        std::env::remove_var("AXON_RESOURCE_PRIORITY");
        std::env::remove_var("AXON_BACKGROUND_BUDGET_CLASS");
        std::env::remove_var("AXON_GPU_ACCESS_POLICY");
        std::env::remove_var("AXON_WATCHER_POLICY");
        std::env::remove_var("AXON_EMBEDDING_PROVIDER");
        std::env::remove_var("AXON_VECTOR_WORKERS");
        std::env::remove_var("AXON_GRAPH_WORKERS");
    }
}

#[test]
fn test_status_exposes_advertised_endpoints_separately_from_runtime_local_urls() {
    let _guard = env_lock();
    unsafe {
        std::env::set_var("AXON_MCP_URL", "http://127.0.0.1:44129/mcp");
        std::env::set_var("AXON_SQL_URL", "http://127.0.0.1:44129/sql");
        std::env::set_var("AXON_DASHBOARD_URL", "http://127.0.0.1:44127/");
        std::env::set_var("AXON_PUBLIC_HOST", "192.168.1.50");
        std::env::set_var("AXON_PUBLIC_HOST_SOURCE", "derived");
        std::env::set_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE", "1");
        std::env::set_var("AXON_MCP_PUBLIC_URL", "http://192.168.1.50:44129/mcp");
        std::env::set_var("AXON_SQL_PUBLIC_URL", "http://192.168.1.50:44129/sql");
        std::env::set_var("AXON_DASHBOARD_PUBLIC_URL", "http://192.168.1.50:44127/");
    }

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();

    assert_eq!(
        data["instance_identity"]["mcp_url"].as_str(),
        Some("http://127.0.0.1:44129/mcp")
    );
    assert_eq!(
        data["advertised_endpoints"]["available"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["advertised_endpoints"]["public_host_source"].as_str(),
        Some("derived")
    );
    assert_eq!(
        data["advertised_endpoints"]["mcp_url"].as_str(),
        Some("http://192.168.1.50:44129/mcp")
    );
    assert_eq!(
        data["client_reachability_notes"]["instance_identity_is_runtime_local_only"].as_bool(),
        Some(true)
    );

    unsafe {
        std::env::remove_var("AXON_MCP_URL");
        std::env::remove_var("AXON_SQL_URL");
        std::env::remove_var("AXON_DASHBOARD_URL");
        std::env::remove_var("AXON_PUBLIC_HOST");
        std::env::remove_var("AXON_PUBLIC_HOST_SOURCE");
        std::env::remove_var("AXON_PUBLIC_ENDPOINTS_AVAILABLE");
        std::env::remove_var("AXON_MCP_PUBLIC_URL");
        std::env::remove_var("AXON_SQL_PUBLIC_URL");
        std::env::remove_var("AXON_DASHBOARD_PUBLIC_URL");
    }
}

/// REQ-AXO-108 — `data.instance_identity.data_root_absolute` exposes
/// the canonicalized absolute path of `AXON_DB_ROOT` so an LLM and an
/// operator running `ls`/`du` against the same path can confirm they
/// are looking at the same on-disk IST. The companion `data_root`
/// (compact form) stays for human display.
#[test]
fn test_status_exposes_data_root_absolute_for_unambiguous_cross_reference() {
    let _lock = crate::test_support::env_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let tmp = tempdir().unwrap();
    let abs_path = tmp.path().to_path_buf();
    let _g_db =
        crate::test_support::EnvVarGuard::set("AXON_DB_ROOT", &abs_path.display().to_string());

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();
    let identity = data["instance_identity"].as_object().unwrap();

    // Compact form for human display — present and not "unknown".
    let compact = identity["data_root"].as_str().unwrap();
    assert!(
        compact != "unknown",
        "data_root must be non-unknown when AXON_DB_ROOT is set, got: {compact}"
    );

    // Absolute form for cross-reference — REQ-AXO-108 contract.
    let absolute = identity["data_root_absolute"].as_str().unwrap();
    assert!(
        absolute.starts_with('/'),
        "data_root_absolute must be an absolute path starting with '/', got: {absolute}"
    );
    // canonicalize() resolves symlinks so the returned path may not
    // string-equal abs_path; assert the file_name matches instead.
    let abs_filename = std::path::PathBuf::from(absolute)
        .file_name()
        .map(|n| n.to_string_lossy().to_string());
    let expected_filename = abs_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string());
    assert_eq!(
        abs_filename, expected_filename,
        "data_root_absolute must point at the same final dir as AXON_DB_ROOT"
    );
}

/// REQ-AXO-108 — when AXON_DB_ROOT is not set, data_root_absolute
/// returns the literal "unknown" rather than panicking, mirroring the
/// existing `data_root` field's behaviour. This keeps the contract
/// safe in test fixtures or partial-boot scenarios.
#[test]
fn test_status_data_root_absolute_returns_unknown_when_env_missing() {
    let _lock = crate::test_support::env_test_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _g_db = crate::test_support::EnvVarGuard::unset("AXON_DB_ROOT");

    let server = create_test_server();
    let response = server.axon_status(&json!({ "mode": "json" })).unwrap();
    let data = response.get("data").unwrap();
    let identity = data["instance_identity"].as_object().unwrap();

    assert_eq!(
        identity["data_root_absolute"].as_str(),
        Some("unknown"),
        "data_root_absolute must be the sentinel 'unknown' when AXON_DB_ROOT is unset"
    );
}

// REQ-AXO-146 — `job_status(wait: true)` blocks the call until the job
// reaches a terminal state OR `timeout_ms` elapses, eliminating the
// polling round-trips that the LLM would otherwise pay 2s+/iteration.
#[test]
fn test_job_status_wait_returns_immediately_when_already_terminal() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.McpJob (job_id, tool_name, status, submitted_at, finished_at, request_json, reserved_ids_json, result_json, error_text) \
             VALUES ('JOB-REQ146-OK', 'soll_apply_plan', 'succeeded', 1, 2, '{}', '{}', '{\"data\":{\"applied\":1}}', '')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "job_status",
            "arguments": {
                "job_id": "JOB-REQ146-OK",
                "wait": true,
                "timeout_ms": 5_000,
                "poll_interval_ms": 50
            }
        })),
        id: Some(json!(801)),
    };
    let started = std::time::Instant::now();
    let response = server.handle_request(req).unwrap().result.unwrap();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(
        elapsed_ms < 1_000,
        "wait must short-circuit when the job is already terminal (took {}ms)",
        elapsed_ms
    );
    let data = response.get("data").expect("data payload");
    assert_eq!(data["state"].as_str(), Some("completed"));
    let wait_meta = data
        .get("wait_metadata")
        .expect("wait_metadata present when wait=true");
    assert_eq!(wait_meta["wait"].as_bool(), Some(true));
    assert_eq!(wait_meta["timed_out"].as_bool(), Some(false));
    assert_eq!(wait_meta["reached_terminal"].as_bool(), Some(true));
    assert!(wait_meta["polls"].as_u64().unwrap_or(0) >= 1);
}

#[test]
fn test_job_status_wait_returns_partial_snapshot_on_timeout() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.McpJob (job_id, tool_name, status, submitted_at, request_json, reserved_ids_json) \
             VALUES ('JOB-REQ146-WAIT', 'soll_apply_plan', 'queued', 1, '{}', '{}')",
        )
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "job_status",
            "arguments": {
                "job_id": "JOB-REQ146-WAIT",
                "wait": true,
                "timeout_ms": 120,
                "poll_interval_ms": 30
            }
        })),
        id: Some(json!(802)),
    };
    let started = std::time::Instant::now();
    let response = server.handle_request(req).unwrap().result.unwrap();
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(
        elapsed_ms >= 100,
        "wait must honour timeout_ms (took {}ms, expected ≥100)",
        elapsed_ms
    );
    assert!(
        elapsed_ms < 2_000,
        "wait must not block longer than timeout_ms + small slack (took {}ms)",
        elapsed_ms
    );
    let data = response.get("data").expect("data payload");
    assert_eq!(
        data["state"].as_str(),
        Some("queued"),
        "non-terminal job stays in queued state across the wait"
    );
    let wait_meta = data
        .get("wait_metadata")
        .expect("wait_metadata present when wait=true");
    assert_eq!(wait_meta["timed_out"].as_bool(), Some(true));
    assert_eq!(wait_meta["reached_terminal"].as_bool(), Some(false));
    assert!(
        wait_meta["polls"].as_u64().unwrap_or(0) >= 2,
        "wait should issue ≥2 snapshot reads inside a 120ms window with 30ms interval"
    );
    // Continue-polling guidance still surfaces so an LLM resuming the
    // call after the wait returns sees the canonical recovery path.
    assert_eq!(
        data["next_action"]["when"].as_str(),
        Some("continue_polling_until_terminal_state")
    );
}

// REQ-AXO-902289 — `entity` omitted with a canonical `data.id` is inferred,
// not rejected.
//
// The friction signature (soll_manager / invalid_arguments / entity, 43 occ)
// looked like a vocabulary problem, but `field_in_error` carries the first
// MISSING REQUIRED field, not a rejected value: those calls omitted `entity`
// entirely. The id format (DEC-AXO-085) makes it a function of the prefix, so
// the call is under-specified in a recoverable way — same principle as
// REQ-AXO-902288 for `relation_type`.
fn soll_manager_call(server: &McpServer, arguments: Value, id: i64) -> Value {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({ "name": "soll_manager", "arguments": arguments })),
        id: Some(json!(id)),
    };
    server.handle_request(req).unwrap().result.unwrap()
}

fn rejected_field(result: &Value) -> Option<String> {
    result
        .pointer("/data/parameter_repair/invalid_field")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[test]
fn test_soll_manager_infers_entity_from_canonical_id_prefix() {
    let server = create_test_server();

    // A canonical id with no `entity`: the call must get PAST argument
    // validation. It still fails — the node does not exist — and that is the
    // point: the failure is about the target, not about a field the caller
    // could not have known to repeat.
    let inferred = soll_manager_call(
        &server,
        json!({
            "action": "update",
            "data": { "id": "REQ-AXO-999999", "status": "delivered" }
        }),
        7101,
    );
    assert_ne!(
        rejected_field(&inferred).as_deref(),
        Some("entity"),
        "entity is derivable from the `REQ-` prefix — it must not be demanded back"
    );

    // VN — no id to infer from: the missing field is still reported.
    let no_id = soll_manager_call(
        &server,
        json!({ "action": "update", "data": { "status": "delivered" } }),
        7102,
    );
    assert_eq!(
        rejected_field(&no_id).as_deref(),
        Some("entity"),
        "without an id there is nothing to infer from — reject as before"
    );

    // VN — unknown prefix: inference must not guess past a prefix it does not
    // recognise, or a typo would silently mutate the wrong kind of node.
    let unknown_prefix = soll_manager_call(
        &server,
        json!({
            "action": "update",
            "data": { "id": "XXX-AXO-1", "status": "delivered" }
        }),
        7103,
    );
    assert_eq!(
        rejected_field(&unknown_prefix).as_deref(),
        Some("entity"),
        "an unrecognised prefix falls back to the ordinary rejection"
    );
}

// REQ-AXO-902291 — the project scope of a READ-ONLY single-project tool must be
// resolved from the client cwd, never left to the handler's literal "AXO".
//
// Measured before the fix: a tunnel launched from an AgriOptim cwd got
// `SOLL context for AXO` — Axon's own SOLL served to another project's client,
// silently. The transport (REQ-AXO-902286) and the chokepoint (REQ-AXO-902239)
// both worked; these handlers simply were not on the list the chokepoint reads.
//
// The determination "absent means THIS project" vs "absent means EVERY project"
// is the substance of this REQ, and the code answers it on its own: a tool that
// defaults to one hardcoded project is single-project BY CONSTRUCTION, while a
// rollup tool has no default to write. Both directions are pinned below — the
// second matters most, because injecting a scope into a rollup tool would
// SILENTLY NARROW its answer, strictly worse than today's visible wrong one.
#[test]
fn single_project_readonly_tools_resolve_scope_from_cwd() {
    let listed: std::collections::HashMap<&str, &str> =
        McpServer::PROJECT_AUTORESOLVE_TOOLS.iter().copied().collect();

    // Each of these carried `args.get("project_code").unwrap_or("AXO")` in its
    // handler — it answers about exactly one project.
    for tool in [
        "soll_query_context",
        "soll_verify_requirements",
        "snapshot_history",
        "snapshot_diff",
        "project_status",
        "tech_debt_inventory",
        "data_catalog",
        "conception_view",
        "change_safety",
        "detect_remnants",
    ] {
        assert_eq!(
            listed.get(tool),
            Some(&"project_code"),
            "`{tool}` answers about ONE project and must take its scope from the \
             client cwd — otherwise a client working in another project silently \
             reads AXO's data"
        );
    }
}

// REQ-AXO-902301 — the parameter carried under a neighbouring name.
//
// Reproduced live before the fix: `query(symbol=…)` and `sql(query=…)` were
// rejected with the canonical field reported missing (42 + 86 occurrences). The
// vocabulary works against itself at the junction of two tools — `inspect` takes
// `symbol`, `query` takes `query`, and the documented sequence is "query →
// inspect"; for a SQL statement every instinct writes `query=`.
#[test]
fn a_parameter_passed_under_a_neighbouring_name_is_accepted() {
    let as_symbol = json!({ "symbol": "classify_diff_paths" });
    let (patched, used) = McpServer::with_aliased_parameter("query", &as_symbol);
    assert_eq!(
        patched.get("query").and_then(Value::as_str),
        Some("classify_diff_paths"),
        "`symbol` is the natural carry-over from `inspect` — read it as `query`"
    );
    assert_eq!(used, Some(("symbol".to_string(), "query".to_string())));

    let as_query = json!({ "query": "SELECT 1" });
    let (patched, used) = McpServer::with_aliased_parameter("sql", &as_query);
    assert_eq!(patched.get("sql").and_then(Value::as_str), Some("SELECT 1"));
    assert_eq!(used, Some(("query".to_string(), "sql".to_string())));
}

#[test]
fn an_explicit_canonical_parameter_always_wins_over_an_alias() {
    // The caller who filled the canonical field has already decided; an alias
    // must never overwrite it.
    let both = json!({ "query": "chosen", "symbol": "ignored" });
    let (patched, used) = McpServer::with_aliased_parameter("query", &both);
    assert_eq!(patched.get("query").and_then(Value::as_str), Some("chosen"));
    assert_eq!(used, None, "no alias was honoured, so nothing to disclose");
}

#[test]
fn a_tool_outside_the_alias_table_is_untouched() {
    let args = json!({ "symbol": "whatever" });
    let (patched, used) = McpServer::with_aliased_parameter("inspect", &args);
    assert_eq!(patched.as_ref(), &args, "zero-copy passthrough");
    assert_eq!(used, None);
}

/// REQ-AXO-902583 — un alias HONORÉ ne doit pas être accusé d'être inconnu.
///
/// Mesuré sur le runtime promu `v0.8.0-1691-ga416e13b` : `sql(query="SELECT 1")`
/// rendait DEUX phrases contradictoires dans la même réponse —
///   « paramètre reçu sous `query` et lu comme `sql` (REQ-AXO-902301) »
///   « `query` … INCONNU de cet outil, donc sans effet (REQ-AXO-902583) »
/// — pendant que la requête tournait pour de bon.
///
/// Cause : la réparation COPIE la valeur sous le nom canonique sans retirer
/// l'alias, alors que `parameters_outside_the_schema` suppose qu'elle l'a
/// DÉPLACÉ (son filtre est « encore présent après réparation »). On rétablit
/// l'invariant à la source plutôt que d'ajouter une seconde liste d'exclusion :
/// le handler cesse aussi de recevoir la clé en double.
#[test]
fn un_alias_honore_disparait_des_arguments_et_n_est_pas_accuse() {
    let as_query = json!({ "query": "SELECT 1" });
    let (patched, used) = McpServer::with_aliased_parameter("sql", &as_query);
    assert_eq!(patched.get("sql").and_then(Value::as_str), Some("SELECT 1"));
    assert_eq!(used, Some(("query".to_string(), "sql".to_string())));
    assert!(
        patched.get("query").is_none(),
        "l'alias honoré doit être RETIRÉ, pas dupliqué : {patched}"
    );

    // Et le contrôle qui accusait ne trouve plus rien à dire.
    let ignores = McpServer::parameters_outside_the_schema("sql", &as_query, &patched);
    assert!(
        ignores.is_empty(),
        "un alias honoré n'est pas un paramètre ignoré : {ignores:?}"
    );
}

/// Même défaut sur l'autre réparation qui copie : la liste donnée sous un nom
/// voisin (`files=` pour `diff_paths`).
#[test]
fn une_liste_normalisee_depuis_un_alias_ne_laisse_pas_la_cle_source() {
    let flat = json!({ "files": "src/lib.rs", "message": "fix: x" });
    let (patched, note) = McpServer::with_normalised_list_parameter("commit_work", &flat);
    assert!(note.is_some(), "la normalisation doit avoir eu lieu : {patched}");
    assert_eq!(
        patched.get("diff_paths"),
        Some(&json!(["src/lib.rs"])),
        "la valeur atteint le nom canonique : {patched}"
    );
    assert!(
        patched.get("files").is_none(),
        "la clé source honorée doit être RETIRÉE : {patched}"
    );
}

/// Une valeur unique donnée sous le nom CANONIQUE n'est pas un alias : la clé
/// doit rester, sinon on retirerait le paramètre que l'appelant a bien nommé.
#[test]
fn une_valeur_unique_sous_le_nom_canonique_reste_en_place() {
    let scalaire = json!({ "diff_paths": "src/lib.rs", "message": "fix: x" });
    let (patched, note) = McpServer::with_normalised_list_parameter("commit_work", &scalaire);
    assert!(note.is_some());
    assert_eq!(patched.get("diff_paths"), Some(&json!(["src/lib.rs"])));
}

/// REQ-AXO-902583 — `detail` et `guidance` sont lus par
/// `attach_default_tool_guidance` pour TOUT outil et ne figurent dans AUCUN
/// schéma. Les accuser d'être « sans effet » est faux : `detail="full"` attache
/// réellement l'enveloppe complète. C'est la forme `REQ-AXO-902584` — une
/// affirmation positive fausse, pire qu'un silence.
#[test]
fn un_champ_du_protocole_de_guidance_n_est_pas_accuse_d_etre_inconnu() {
    let args = json!({ "id": "GUI-AXO-1038", "detail": "full", "guidance": "full" });
    let ignores = McpServer::parameters_outside_the_schema("soll_get", &args, &args);
    assert!(
        ignores.is_empty(),
        "`detail` et `guidance` sont honorés par le protocole, pas ignorés : {ignores:?}"
    );

    // Et le contrôle garde son mordant sur un vrai intrus.
    let avec_intrus = json!({ "id": "GUI-AXO-1038", "detail": "full", "sectionz": "Règle" });
    let ignores = McpServer::parameters_outside_the_schema("soll_get", &avec_intrus, &avec_intrus);
    assert_eq!(
        ignores,
        vec!["sectionz".to_string()],
        "l'intrus reste nommé, seul : {ignores:?}"
    );
}

// REQ-AXO-902303 — the `data` fields written at the top level.
#[test]
fn stray_top_level_fields_are_moved_into_data() {
    let flat = json!({
        "action": "update", "entity": "requirement",
        "id": "REQ-AXO-999999", "status": "delivered",
    });
    let (patched, note) = McpServer::with_hoisted_soll_data("soll_manager", &flat);

    assert_eq!(patched["data"]["id"].as_str(), Some("REQ-AXO-999999"));
    assert_eq!(patched["data"]["status"].as_str(), Some("delivered"));
    assert!(
        patched.get("id").is_none(),
        "the field must MOVE, not be duplicated at both levels: {patched}"
    );
    assert!(note.is_some(), "the move must be disclosed");
}

#[test]
fn an_existing_data_field_is_never_overwritten_from_the_top_level() {
    // A caller who built the envelope has decided. Overwriting one of its fields
    // from the top level would be a mutation they did not ask for.
    let both = json!({
        "action": "update", "entity": "requirement",
        "id": "REQ-AXO-000000",
        "data": { "id": "REQ-AXO-999999", "status": "delivered" },
    });
    let (patched, _) = McpServer::with_hoisted_soll_data("soll_manager", &both);
    assert_eq!(
        patched["data"]["id"].as_str(),
        Some("REQ-AXO-999999"),
        "`data` wins over a stray top-level field: {patched}"
    );
}

#[test]
fn a_well_formed_soll_manager_call_is_untouched() {
    let clean = json!({
        "action": "update", "entity": "requirement",
        "data": { "id": "REQ-AXO-999999" },
    });
    let (patched, note) = McpServer::with_hoisted_soll_data("soll_manager", &clean);
    assert_eq!(patched.as_ref(), &clean, "zero-copy passthrough");
    assert_eq!(note, None);
}

// REQ-AXO-902302 — a single value where the tool expects a list.
#[test]
fn a_scalar_is_read_as_a_one_element_list() {
    let scalar = json!({ "diff_paths": "src/foo.rs" });
    let (patched, note) = McpServer::with_normalised_list_parameter("pre_flight_check", &scalar);
    assert_eq!(
        patched["diff_paths"],
        json!(["src/foo.rs"]),
        "checking ONE file is the common case; nothing in the name imposes the array"
    );
    assert!(note.is_some(), "the normalisation must be disclosed");
}

#[test]
fn a_list_parameter_is_accepted_under_files_or_paths() {
    let as_files = json!({ "files": ["src/a.rs", "src/b.rs"] });
    let (patched, note) = McpServer::with_normalised_list_parameter("commit_work", &as_files);
    assert_eq!(patched["diff_paths"], json!(["src/a.rs", "src/b.rs"]));
    assert!(note.is_some());
}

#[test]
fn a_usable_canonical_list_is_left_untouched() {
    let ok = json!({ "diff_paths": ["src/a.rs"], "files": ["ignored.rs"] });
    let (patched, note) = McpServer::with_normalised_list_parameter("pre_flight_check", &ok);
    assert_eq!(patched.as_ref(), &ok, "zero-copy when nothing needs fixing");
    assert_eq!(note, None);
    assert_eq!(
        patched["diff_paths"], json!(["src/a.rs"]),
        "an alias must never overwrite a usable canonical list"
    );
}

/// REQ-AXO-902418 — l'enum publié pour `artifact_type` doit être l'UNION de ce
/// que le gestionnaire accepte, pas une copie tenue à la main d'une partie.
///
/// Le littéral qui occupait cette place se déclarait lui-même « mirror of
/// shared.rs::accepted_evidence_artifact_schema, the single source of truth » et
/// avait divergé : `commit`, `sollref` et `url`, acceptés pour TOUTE entité, en
/// étaient absents. Un LLM qui lit l'enum n'avait aucun moyen d'apprendre qu'un
/// SHA de commit est attachable — coût mesuré chez TE2 (`mcp_feedback` #185) :
/// cinq rejets, puis un second appel en `commit` qui passe du premier coup.
#[test]
fn the_published_evidence_type_enum_is_the_union_of_what_the_handler_accepts() {
    let catalog = crate::mcp::catalog::tools_catalog(true);
    let tools = catalog["tools"].as_array().expect("tools array");
    let published: Vec<String> = tools
        .iter()
        .find(|t| t["name"].as_str() == Some("soll_attach_evidence"))
        .expect("`soll_attach_evidence` doit exister au catalogue")["inputSchema"]["properties"]
        ["artifacts"]["items"]["properties"]["artifact_type"]["enum"]
        .as_array()
        .expect("`artifact_type` doit publier un enum")
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::to_string)
        .collect();

    // CONTRÔLE POSITIF : un enum vide, ou un chemin JSON qui a bougé, rendrait
    // les deux boucles ci-dessous vertes en ne comparant rien.
    assert!(
        published.contains(&"file".to_string()),
        "l'enum publié doit au minimum porter la valeur de tous les jours ; s'il \
         ne la porte pas, le chemin de lecture est faux et ce test ne mesure \
         rien : {published:?}"
    );

    let accepted = crate::mcp::tools_soll::all_accepted_evidence_artifact_types();
    for kind in &accepted {
        assert!(
            published.iter().any(|p| p == kind),
            "`{kind}` est accepté par le gestionnaire et ABSENT de l'enum publié : \
             un appelant ne peut pas le deviner avant d'avoir provoqué l'erreur.\n  \
             publié : {published:?}\n  accepté : {accepted:?}"
        );
    }
    for kind in &published {
        assert!(
            accepted.contains(&kind.as_str()),
            "`{kind}` est publié comme légal alors qu'AUCUNE entité ne l'accepte : \
             l'enum promet une valeur que le gestionnaire refusera toujours.\n  \
             publié : {published:?}\n  accepté : {accepted:?}"
        );
    }
}

#[test]
fn no_alias_shadows_a_real_parameter_of_its_own_tool() {
    // The guard that keeps this safe as the table grows: an alias must NOT be a
    // field the tool genuinely accepts, or honouring it would silently overwrite
    // a legitimate argument.
    let catalog = crate::mcp::catalog::tools_catalog(true);
    let tools = catalog["tools"].as_array().expect("tools array");
    // REQ-AXO-902302 — BOTH tables, or the guard covers half the surface and goes
    // green while the other half shadows a real parameter.
    let all = McpServer::PARAMETER_ALIASES
        .iter()
        .chain(McpServer::SCALAR_TO_ARRAY_PARAMS.iter());
    for (tool, canonical, aliases) in all {
        // Two namespaces, deliberately: the catalog advertises `axon_commit_work`
        // / `axon_pre_flight_check`, while dispatch strips the `axon_` prefix — and
        // the alias tables are keyed on the DISPATCH name (which is also what the
        // friction log records). Look the tool up under both, or the guard panics
        // on a name that is perfectly correct where it is used.
        let prefixed = format!("axon_{tool}");
        let schema = tools
            .iter()
            .find(|t| {
                let name = t["name"].as_str();
                name == Some(tool) || name == Some(prefixed.as_str())
            })
            .map(|t| &t["inputSchema"]["properties"])
            .unwrap_or_else(|| panic!("`{tool}` must exist in the catalog (bare or axon_-prefixed)"));
        for alias in *aliases {
            assert!(
                schema.get(alias).is_none(),
                "`{tool}` genuinely accepts `{alias}`, so treating it as an alias for \
                 `{canonical}` would shadow a real parameter"
            );
        }
    }
}

#[test]
fn rollup_tools_are_never_scope_injected() {
    let listed: std::collections::HashMap<&str, &str> =
        McpServer::PROJECT_AUTORESOLVE_TOOLS.iter().copied().collect();

    // For these, an absent project means EVERY project: they return a per-project
    // rollup. Injecting a scope would narrow the answer with no error raised —
    // the regression this allow-list exists to prevent.
    for tool in [
        "embedding_status",
        "audit",
        "health",
        "anomalies",
        "semantic_clones",
        "diagnose_indexing",
        "status",
    ] {
        assert!(
            !listed.contains_key(tool),
            "`{tool}` reports across ALL projects — injecting a scope would silently \
             shrink its answer instead of failing visibly"
        );
    }
}

#[test]
fn test_mcp_feedback_report_renders_a_named_item_in_full() {
    // REQ-AXO-902439 — the list lane clips `problem` at 160 chars and shows
    // `proposed_solution` only for open blocking items, so triaging a long
    // doléance forced `sql SELECT problem FROM axon.llm_feedback` — the raw-SQL
    // fallback the canon forbids. Paid twice on 2026-08-21 by the author of
    // this fix. The `ids` lane must return the item whole.
    let server = create_test_server();
    // Longer than the 160-char scan clip, so "full" is measurable rather than
    // asserted.
    let long_problem = format!("PROBLEM_HEAD {} PROBLEM_TAIL", "x".repeat(400));
    let long_solution = format!("SOLUTION_HEAD {} SOLUTION_TAIL", "y".repeat(400));
    // `id` is GENERATED ALWAYS AS IDENTITY — let PG assign it and read it back,
    // rather than pinning a literal the column refuses.
    server
        .graph_store
        .execute_param(
            "INSERT INTO axon.llm_feedback (created_at, llm_identity, category, severity, \
             tool, project_code, problem, proposed_solution, satisfaction, triage_status) \
             VALUES (now(), 'test-llm', 'incomplete', 'minor', 'mcp_feedback_report', \
             'AXO', ?, ?, 3, 'open')",
            &json!([long_problem, long_solution]),
        )
        .expect("insert feedback fixture");
    let raw = server
        .graph_store
        .query_json(
            "SELECT id FROM axon.llm_feedback WHERE llm_identity = 'test-llm' \
             ORDER BY id DESC LIMIT 1",
        )
        .expect("read back fixture id");
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).expect("rows");
    let fixture_id: i64 = rows[0][0]
        .as_i64()
        .or_else(|| rows[0][0].as_str().and_then(|s| s.parse().ok()))
        .expect("fixture id");

    // POSITIVE CONTROL — the LIST lane must clip, otherwise this test would
    // pass against a surface that never clipped and measures nothing.
    let listed = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback_report",
                "arguments": { "project_code": "AXO", "limit": 50 }
            })),
            id: Some(json!(9024391)),
        })
        .unwrap()
        .result
        .unwrap();
    let listed_text = listed["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        !listed_text.contains("PROBLEM_TAIL"),
        "positive control: the list lane is supposed to clip the problem"
    );

    let detail = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback_report",
                "arguments": { "ids": [fixture_id] }
            })),
            id: Some(json!(9024392)),
        })
        .unwrap()
        .result
        .unwrap();
    let text = detail["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("PROBLEM_TAIL"),
        "the ids lane must render the problem WHOLE, got {} chars",
        text.len()
    );
    assert!(
        text.contains("SOLUTION_TAIL"),
        "the ids lane must render the proposed solution too, even for a \
         non-blocking item"
    );

    // An unknown id is NAMED back, never silently absent.
    let unknown = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback_report",
                "arguments": { "ids": [fixture_id, 777902439] }
            })),
            id: Some(json!(9024393)),
        })
        .unwrap()
        .result
        .unwrap();
    let unknown_ids = unknown["data"]["unknown_ids"]
        .as_array()
        .expect("unknown_ids array");
    assert_eq!(
        unknown_ids.len(),
        1,
        "the id that does not exist must be reported, not dropped"
    );
}

#[test]
fn test_sql_repair_names_the_identifier_folding_trap() {
    // REQ-AXO-902444 — VPC filed a bug against GUI-PRO-028 ("the prescribed SQL
    // does not run"), then retracted it (llm_feedback #221): the guideline was
    // correct, the quotes were theirs. PG renders `soll."Node"` failing as
    // `relation "soll.Node" does not exist` — with quotes IT added — which
    // reads as "the unquoted form failed". Two turns and one wrong product
    // report were spent on a message that states a true fact in a misleading
    // shape.
    use crate::mcp::tool_contracts::render_pg_repair_text_for_tests;

    let mixed_case = json!({
        "problem_class": "undefined_table",
        "referenced_relations": [{ "relation": "soll.Node", "real_columns": [], "exists": false }]
    });
    let text = render_pg_repair_text_for_tests(&mixed_case);
    assert!(
        text.contains("folds UNQUOTED identifiers to lower case"),
        "the folding trap must be named when the identifier carries an \
         uppercase letter: {text}"
    );
    assert!(
        text.contains("added by PG"),
        "and it must say the quotes in the message are PG's own: {text}"
    );

    // POSITIVE CONTROL — an all-lowercase name cannot be hitting this trap, so
    // the note must NOT fire. Without this the note would be unconditional
    // noise on every missing table.
    let lower = json!({
        "problem_class": "undefined_table",
        "referenced_relations": [{ "relation": "soll.foo", "real_columns": [], "exists": false }]
    });
    let text = render_pg_repair_text_for_tests(&lower);
    assert!(
        !text.contains("folds UNQUOTED identifiers"),
        "no folding note for an all-lowercase identifier: {text}"
    );
}

/// REQ-AXO-902380 — le CONTRAT PUBLIÉ doit décrire ce que le serveur FAIT.
///
/// Rapporté par OPV : « le contrat des outils annonce `project_code` auto-résolu
/// depuis la cwd », alors qu'ils lisaient `Default: AXO` sur les outils qu'ils
/// utilisent. Conclusion rationnelle de leur part : passer `project=OPV` partout.
///
/// La mesure du 2026-08-26 donne le mécanisme exact, et ce n'est pas celui que la
/// doléance suppose. Le transport (REQ-AXO-902286) et le chokepoint
/// (REQ-AXO-902239) sont corrects : la résolution suit BIEN le cwd du client. Ce
/// qui ment, c'est la DESCRIPTION — **8 des 24 outils auto-résolus** annonçaient
/// `default: AXO`. Ce sont exactement les dix de la seconde vague
/// (REQ-AXO-902291) : on les a ajoutés à l'allow-list sans toucher à leur schéma.
///
/// ⚠️ C'est la sixième fois cette semaine qu'une règle vit à DEUX endroits et
/// qu'un seul est corrigé. Cette garde est le second endroit rendu dépendant du
/// premier : l'allow-list reste la source, et une divergence ne peut plus passer.
///
/// Elle lit le CODE (`PROJECT_AUTORESOLVE_TOOLS`) et le CATALOGUE, jamais la doc.
#[test]
fn le_contrat_publie_ne_peut_pas_contredire_l_auto_resolution() {
    let catalogue = crate::mcp::catalog::tools_catalog(true);
    let outils = catalogue
        .get("tools")
        .and_then(|t| t.as_array())
        .expect("le catalogue expose `tools`");

    let mut menteurs: Vec<String> = Vec::new();
    let mut muets: Vec<String> = Vec::new();

    for (nom, cle) in McpServer::PROJECT_AUTORESOLVE_TOOLS {
        let Some(outil) = outils
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(*nom))
        else {
            // Un outil de l'allow-list absent du catalogue est un défaut à part
            // entière : on auto-résout pour un outil que personne ne voit.
            menteurs.push(format!("{nom} (ABSENT du catalogue)"));
            continue;
        };
        let desc = outil
            .get("inputSchema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.get(*cle))
            .and_then(|f| f.get("description"))
            .and_then(|d| d.as_str());
        let Some(desc) = desc else {
            muets.push(format!("{nom}.{cle}"));
            continue;
        };
        let annonce_axo = desc.contains("default: AXO")
            || desc.contains("Default: AXO")
            || desc.contains("(default AXO)");
        let annonce_resolution = desc.contains("uto-resolved") || desc.contains("cwd");
        if annonce_axo && !annonce_resolution {
            menteurs.push(format!("{nom}.{cle} — « {desc} »"));
        } else if !annonce_resolution {
            muets.push(format!("{nom}.{cle}"));
        }
    }

    assert!(
        menteurs.is_empty(),
        "{} outil(s) auto-résolus annoncent AXO comme défaut — un tenant qui lit ça \
         passe `project=` partout, ce qui est exactement la friction rapportée :\n  {}",
        menteurs.len(),
        menteurs.join("\n  ")
    );
    assert!(
        muets.is_empty(),
        "{} outil(s) auto-résolus ne disent RIEN de la résolution — le silence laisse \
         le lecteur supposer, et il suppose le pire :\n  {}",
        muets.len(),
        muets.join("\n  ")
    );
}

// ===========================================================================
// REQ-AXO-902583 (P4) — le paramètre VALIDE mais INERTE
// ===========================================================================

/// TIER 1 — anti-dérive UNIVERSELLE : la déclaration doit couvrir EXACTEMENT le
/// schéma.
///
/// C'est la dérive la plus probable, et la plus silencieuse : quelqu'un ajoute
/// un paramètre à `soll_get`, la table ne bouge pas, et la surface se met à
/// affirmer d'un appel qu'il n'a rien d'inerte alors qu'elle n'a plus regardé
/// que trois champs sur quatre. Le contrôle est GÉNÉRÉ depuis le catalogue —
/// aucune liste recopiée qui pourrait dériver à son tour.
#[test]
fn toute_disposition_declaree_couvre_exactement_le_schema_de_son_outil() {
    let catalogue = crate::mcp::catalog::tools_catalog(true);
    let outils = catalogue["tools"].as_array().expect("catalogue non vide");

    for (nom, declarations) in crate::mcp::tool_contracts::DECLARED_DISPOSITIONS {
        let entree = outils
            .iter()
            .find(|tool| {
                tool.get("name").and_then(Value::as_str).is_some_and(|name| {
                    crate::mcp::catalog::tool_names_denote_the_same_tool(name, nom)
                })
            })
            .unwrap_or_else(|| panic!("`{nom}` est déclaré mais absent du catalogue"));

        let mut du_schema: Vec<String> = entree["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("`{nom}` n'expose pas de propriétés"))
            .keys()
            .cloned()
            .collect();
        du_schema.sort();

        // Lu par la RÉSOLUTION, pas depuis la table directement : le contrôle doit
        // éprouver le chemin que le chokepoint emprunte, sinon il valide une table
        // que personne n'utilise.
        let resolues = crate::mcp::tool_contracts::parameter_dispositions(nom)
            .unwrap_or_else(|| panic!("`{nom}` est dans la table mais la résolution le rate"));
        // Comparaison par VALEUR, pas par pointeur : un `const` Rust est substitué à
        // chaque site d'usage, donc deux lectures de la même constante n'ont aucune
        // raison de partager une adresse. `std::ptr::eq` rougissait ici sans qu'aucune
        // table ait dérivé — un contrôle qui ne mesure pas ce qu'il croit mesurer.
        assert_eq!(
            resolues, *declarations,
            "`{nom}` — la résolution ne rend pas la table déclarée"
        );
        let mut declares: Vec<String> = resolues.iter().map(|d| d.name.to_string()).collect();
        declares.sort();

        assert_eq!(
            declares, du_schema,
            "`{nom}` — la table des dispositions a dérivé de son schéma. Un champ non \
             déclaré ne sera JAMAIS signalé comme inerte, et la surface se taira en \
             laissant croire qu'elle a regardé."
        );
    }
}

/// TIER 3, moitié POSITIVE — la condition tient, donc RIEN n'est signalé.
///
/// Un contrôle qui crie toujours ne dit rien. Cette moitié est ce qui prouve que
/// le verdict sait rendre « effectif ».
#[test]
fn un_parametre_conditionnel_dont_la_condition_tient_n_est_pas_signale() {
    use crate::mcp::tool_contracts::inert_parameters_for_call;

    // `section` sans `sections` → la condition `FieldUnset` tient.
    assert!(
        inert_parameters_for_call("soll_get", &json!({ "id": "X", "section": "Règle" })).is_empty()
    );
    // `sections: false` compte comme non posé — c'est la même intention.
    assert!(inert_parameters_for_call(
        "soll_get",
        &json!({ "id": "X", "section": "Règle", "sections": false })
    )
    .is_empty());
    // `around` AVEC `mode=source` → la condition `FieldEquals` tient.
    assert!(inert_parameters_for_call(
        "inspect",
        &json!({ "symbol": "f", "mode": "source", "around": "foo", "offset": 40 })
    )
    .is_empty());
    // Un outil NON instrumenté ne rend jamais de verdict — silence, pas « rien à
    // signaler » : les deux se lisent différemment et confondre les deux est le
    // défaut que ce REQ ferme.
    assert!(inert_parameters_for_call(
        "query",
        &json!({ "query": "f", "around": "foo" })
    )
    .is_empty());
}

/// TIER 3, moitié NÉGATIVE — la condition ne tient pas, donc le paramètre est
/// signalé, avec la RAISON tirée de l'appel courant.
#[test]
fn un_parametre_conditionnel_inerte_est_nomme_avec_sa_cause_et_son_remede() {
    use crate::mcp::tool_contracts::inert_parameters_for_call;

    let inertes = inert_parameters_for_call(
        "soll_get",
        &json!({ "id": "GUI-AXO-1034", "sections": true, "section": "Porte" }),
    );
    assert_eq!(inertes.len(), 1, "seul `section` est inerte ici : {inertes:?}");
    assert_eq!(inertes[0].name, "section");
    assert!(
        inertes[0].reason.contains("`sections`") && inertes[0].reason.contains("true"),
        "la raison doit nommer le champ ET la valeur REÇUE, sinon elle se lit comme \
         de la documentation : {}",
        inertes[0].reason
    );
    assert!(
        inertes[0].remedy.contains("sections"),
        "le remède doit dire quoi changer : {}",
        inertes[0].remedy
    );

    // `inspect` sans `mode=source` : les DEUX paramètres de fenêtrage sont inertes.
    let inertes = inert_parameters_for_call(
        "inspect",
        &json!({ "symbol": "f", "around": "foo", "offset": 40 }),
    );
    let noms: Vec<_> = inertes.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(noms, vec!["around", "offset"]);
    assert!(
        inertes[0].reason.contains("n'est pas fourni"),
        "un `mode` absent doit se dire ABSENT, pas « vaut null » : {}",
        inertes[0].reason
    );

    // `mode` fourni mais AUTRE que `source` : la raison doit citer la valeur reçue.
    let inertes = inert_parameters_for_call(
        "inspect",
        &json!({ "symbol": "f", "mode": "verbose", "around": "foo" }),
    );
    assert_eq!(inertes.len(), 1);
    assert!(
        inertes[0].reason.contains("verbose"),
        "la valeur réellement reçue doit apparaître : {}",
        inertes[0].reason
    );
}

/// Un paramètre conditionnel ABSENT de l'appel n'a rien à se voir reprocher.
///
/// Sans ce contrôle, `inspect(symbol=f)` — l'appel le plus courant de tout le
/// parc — signalerait `around` et `offset` à chaque fois. Un avertissement
/// permanent n'est plus un avertissement.
#[test]
fn un_parametre_conditionnel_non_fourni_n_est_jamais_signale() {
    use crate::mcp::tool_contracts::inert_parameters_for_call;

    assert!(inert_parameters_for_call("inspect", &json!({ "symbol": "f" })).is_empty());
    assert!(inert_parameters_for_call("soll_get", &json!({ "id": "X" })).is_empty());
    // `null` explicite = absent, pas « fourni avec une valeur vide ».
    assert!(inert_parameters_for_call(
        "inspect",
        &json!({ "symbol": "f", "around": Value::Null })
    )
    .is_empty());
}

/// TIER 3 COMPORTEMENTAL — ce qui rend la déclaration VRAIE plutôt que supposée.
///
/// Les trois tests ci-dessus éprouvent la TABLE. Celui-ci éprouve le FAIT
/// qu'elle décrit : que `sections=true` rende réellement `section` sans effet.
/// Sans lui, une évolution de `soll_get` qui inverserait la préséance laisserait
/// la table mentir sans qu'aucun test ne rougisse — un garde qui ne sait pas
/// rendre faux.
#[test]
fn la_disposition_declaree_de_soll_get_decrit_le_comportement_reel() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-PDS-001', 'Guideline', 'PDS', 'Probe', \
             '## Alpha\nCORPS ALPHA\n\n## Beta\nCORPS BETA', 'current', '{}')",
        )
        .unwrap();

    let texte = |args: Value| -> String {
        server
            .axon_soll_get(&args)
            .expect("soll_get répond")
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    // Moitié NÉGATIVE : avec `sections=true`, ajouter `section` ne change RIEN.
    let titres_seuls = texte(json!({ "id": "GUI-PDS-001", "sections": true }));
    let titres_plus_section =
        texte(json!({ "id": "GUI-PDS-001", "sections": true, "section": "Alpha" }));
    assert_eq!(
        titres_seuls, titres_plus_section,
        "`sections=true` rend `section` INERTE — c'est ce que la table déclare, et \
         c'est ce qui doit être vrai"
    );

    // Moitié POSITIVE : sans `sections`, `section` change bien la réponse.
    let corps_entier = texte(json!({ "id": "GUI-PDS-001" }));
    let une_section = texte(json!({ "id": "GUI-PDS-001", "section": "Alpha" }));
    assert_ne!(
        corps_entier, une_section,
        "sans `sections`, `section` DOIT mordre — sinon la disposition `Conditional` \
         est fausse dans les deux sens"
    );
    assert!(
        une_section.contains("CORPS ALPHA") && !une_section.contains("CORPS BETA"),
        "`section=Alpha` doit rendre Alpha et RIEN d'autre : {une_section}"
    );
}

/// La restitution au chokepoint : deux causes, deux phrases, deux clés — et les
/// remédiations sont OPPOSÉES.
///
/// « corrigez l'orthographe » pour un nom inconnu ; « surtout ne la corrigez
/// pas » pour un inerte. Les fondre en un seul message enverrait la moitié des
/// appelants au mauvais endroit.
#[test]
fn les_deux_causes_se_divulguent_separement_et_ne_s_annulent_pas() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-PDS-002', 'Guideline', 'PDS', 'Probe', \
             '## Alpha\nCORPS ALPHA', 'current', '{}')",
        )
        .unwrap();

    // (a) INERTE seul — la branche que le retour anticipé sur `ignored_parameters`
    //     tuait avant ce lot.
    let inerte = server
        .execute_tool_direct(
            "soll_get",
            &json!({ "id": "GUI-PDS-002", "sections": true, "section": "Alpha" }),
        )
        .expect("soll_get répond");
    let texte = inerte
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        texte.contains("SANS EFFET") && texte.contains("NE CORRIGEZ PAS"),
        "un inerte seul doit se divulguer, avec le conseil INVERSE de celui d'un \
         inconnu : {texte}"
    );
    assert_eq!(
        inerte
            .pointer("/data/parameter_dispositions/0/parameter")
            .and_then(Value::as_str),
        Some("section"),
        "et il doit être lisible en DONNÉE pour un client qui ne lit pas le texte"
    );
    assert!(
        inerte.pointer("/data/ignored_parameters").is_none(),
        "aucun nom inconnu ici — ne pas inventer la seconde liste"
    );

    // (b) les DEUX à la fois — chacune sa phrase, aucune n'écrase l'autre.
    let deux = server
        .execute_tool_direct(
            "soll_get",
            &json!({
                "id": "GUI-PDS-002", "sections": true, "section": "Alpha", "zorglub": 1
            }),
        )
        .expect("soll_get répond");
    let texte = deux
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        texte.contains("INCONNU(S)") && texte.contains("SANS EFFET"),
        "les deux phrases doivent coexister : {texte}"
    );
    assert_eq!(
        deux.pointer("/data/ignored_parameters/0").and_then(Value::as_str),
        Some("zorglub")
    );
    assert_eq!(
        deux.pointer("/data/parameter_dispositions/0/parameter")
            .and_then(Value::as_str),
        Some("section")
    );
}

// ===========================================================================
// Lot 902588 / 902583 — trois surfaces qui affirmaient plus qu'elles ne savaient
// ===========================================================================

/// REQ-AXO-902583 (DOC, friction c) — `practice_put` disait `inserted: false` sur
/// une pratique bel et bien stockée.
///
/// DOC : « deux champs qui se contredisent dans une seule réponse coûtent un
/// aller-retour à chaque appel ». Ils ont dû faire un `practice_recall` de contrôle.
/// Le champ n'était pas faux — `inserted` est un fait de LIGNE (INSERT neuf contre
/// UPDATE, la fonction étant idempotente sur (scope, practice)) — il répondait à une
/// autre question que celle que l'appelant se pose.
#[test]
fn practice_put_repond_a_la_question_posee_est_ce_ecrit() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let premier = server
        .axon_practice_put(&json!({
            "context": "situation de contrôle",
            "practice": "la même pratique, écrite deux fois",
            "scope": "PDS"
        }))
        .expect("practice_put répond");
    assert_eq!(premier.pointer("/data/persisted").and_then(Value::as_bool), Some(true));
    assert_eq!(premier.pointer("/data/write").and_then(Value::as_str), Some("stored"));

    // Rejouée à l'identique : c'est un UPDATE, donc `inserted` passe à false — et
    // c'est CE cas qui trompait DOC. `persisted` doit rester vrai.
    let second = server
        .axon_practice_put(&json!({
            "context": "situation de contrôle",
            "practice": "la même pratique, écrite deux fois",
            "scope": "PDS"
        }))
        .expect("practice_put répond");
    assert_eq!(
        second.pointer("/data/persisted").and_then(Value::as_bool),
        Some(true),
        "une pratique ré-écrite est TOUJOURS en base — c'est la question de l'appelant"
    );
    assert_eq!(
        second.pointer("/data/write").and_then(Value::as_str),
        Some("updated"),
        "et la nuance reste disponible, en toutes lettres"
    );
    // Le champ historique survit : des appelants le lisent.
    assert!(second.pointer("/data/inserted").is_some());
}

/// REQ-AXO-902598 (APS) — 333k tokens for four counters: compact by default.
///
/// « La réponse a été écrêtée sur disque par notre client. Le verdict utile tenait
/// en `{done, missing, partial, total}`. » Perdre l'information utile par EXCÈS
/// d'information est le pire échec possible pour une surface : le client a bien
/// reçu la réponse, et n'a pas pu la lire.
#[test]
fn soll_verify_requirements_est_compact_par_defaut_et_verbose_est_explicitement_opt_in() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    // Projet `TST` et NON `AXO` : les instantanés SOLL passent par un cache GLOBAL au
    // processus, et les tests tournent en parallèle. Une sonde écrite sous `AXO` a fait
    // rougir `test_project_status_reports_delta_vs_previous_snapshot`, qui exige
    // `delta_vs_previous.available == false` sur un serveur neuf : mon instantané lui
    // servait de « précédent ». Une base par test ne suffit pas quand un cache est
    // partagé — l'isolation doit aussi porter sur le NOM du projet.
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-TST-990001', 'Requirement', 'TST', 'Sonde', 'corps', 'current', '{}')",
        )
        .unwrap();

    let compact_par_defaut = server
        .axon_soll_verify_requirements(&json!({ "project_code": "TST" }))
        .expect("réponse");
    let verbose = server
        .axon_soll_verify_requirements(&json!({ "project_code": "TST", "mode": "verbose" }))
        .expect("réponse");

    // Vérifier le SUCCÈS avant de comparer des clés. Sans ce contrôle, un code projet
    // non résoluble rend une enveloppe d'ERREUR — qui n'a ni `details` ni `summary` —
    // et le test échoue sur une clé absente en laissant croire à un défaut du mode
    // `brief`. C'est exactement ce qui s'est produit au premier passage.
    for (nom, r) in [("compact_par_defaut", &compact_par_defaut), ("verbose", &verbose)] {
        assert!(
            r.get("isError").and_then(Value::as_bool) != Some(true),
            "l'appel `{nom}` a échoué, le reste du test ne mesure rien : {}",
            r.pointer("/content/0/text").and_then(Value::as_str).unwrap_or("?")
        );
    }

    // Les quatre chiffres — le verdict utile — sont là dans les deux formes, et ÉGAUX.
    for cle in ["done", "partial", "missing"] {
        assert_eq!(
            compact_par_defaut.pointer(&format!("/data/summary/{cle}")),
            verbose.pointer(&format!("/data/summary/{cle}")),
            "`brief` doit abréger, pas changer le verdict : `{cle}` diverge"
        );
    }

    // Il abrège réellement — sinon le mode ne sert à rien.
    assert!(compact_par_defaut.pointer("/data/details").is_none());
    assert!(verbose.pointer("/data/details").is_some());
    assert!(compact_par_defaut.pointer("/data/top_gaps").is_some());

    // Et il DIT ce qu'il omet : une réponse abrégée qui tait son abrègement se lit
    // comme une réponse complète, et un appelant conclurait « aucune exigence
    // partielle » sur une liste absente.
    let omis: Vec<String> = compact_par_defaut
        .pointer("/data/omitted_in_brief")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(
        omis.contains(&"details".to_string()),
        "l'abrègement doit être déclaré, got {omis:?}"
    );

    // L'opt-in verbose est explicite et ne se fait pas passer pour une réponse abrégée.
    assert!(verbose.pointer("/data/omitted_in_brief").is_none());
}
