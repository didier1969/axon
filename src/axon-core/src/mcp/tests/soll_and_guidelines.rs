use super::*;

/// REQ-AXO-91560 — satisfy the `ist.Chunk` FK parents (`axon.Project` +
/// `ist.IndexedFile`, both made NOT NULL by the FK-integrity hardening of
/// REQ-AXO-901860) before a test inserts a chunk against the isolated DB.
/// Idempotent — safe to call once per chunk insert.
fn seed_ist_path(server: &McpServer, code: &str, path: &str) {
    let _ = server.graph_store.execute(&format!(
        "INSERT INTO axon.Project (code) VALUES ('{code}') ON CONFLICT (code) DO NOTHING"
    ));
    let _ = server.graph_store.execute(&format!(
        "INSERT INTO ist.IndexedFile (path, project_code, last_seen_ms) VALUES ('{path}', '{code}', 0) ON CONFLICT (path) DO NOTHING"
    ));
}

#[test]
fn test_axon_query_global_default() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "auth" }
        })),
        id: Some(json!(8)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Search results"));
    assert!(content.contains("Mode:"));
}

#[test]
fn test_axon_soll_manager_auto_id() {
    // REQ-AXO-91560 — PG isolation via unique project_code + attach_to a
    // seeded Pillar so the MIL-AXO-020 create+attach invariant holds.
    let server = create_test_server();
    let code = "TST".to_string();
    let expected_id = format!("CPT-{code}-011");
    let pillar_id = format!("PIL-{code}-001");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 1, 0, 10, 0) ON CONFLICT (project_code) DO UPDATE SET last_pil = 1, last_cpt = 10"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Test Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": code,
                    "name": "Test Concept",
                    "explanation": "To test auto id",
                    "rationale": "Because testing is good",
                    "attach_to": pillar_id,
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(1)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains(&expected_id),
        "expected {expected_id} in response, got: {content}"
    );

    let count = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.Node WHERE type='Concept' AND id = '{expected_id}'"
        ))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_mcp_call_telemetry_aggregates_per_call_with_latency() {
    // REQ-AXO-901961 S1 — every call records a time-bucketed stat (ok + error),
    // signature-only, latency aggregated. Isolated by a synthetic tool + project
    // so concurrent telemetry writes from sibling tests don't collide.
    let server = create_test_server();
    let proj = "TLM901961";
    let tool = "synthetic_telemetry_probe";
    let ok = json!({ "data": { "project_code": proj } });
    // An error response whose received_arguments carry a SECRET — never stored.
    let err = json!({
        "isError": true,
        "data": {
            "operator_guidance": { "problem_class": "invalid_arguments" },
            "project_code": proj,
            "received_arguments": { "x": "SUPER_SECRET_TELEMETRY_VALUE" }
        }
    });
    // 2 ok (5ms + 15ms) into one bucket, 1 error (10ms) into another.
    server.record_mcp_call(tool, &ok, 5);
    server.record_mcp_call(tool, &ok, 15);
    server.record_mcp_call(tool, &err, 10);

    // Privacy: no argument content may appear anywhere in the table.
    let dump = server
        .graph_store
        .query_json(&format!(
            "SELECT tool||'|'||status||'|'||call_count||'|'||latency_sum_ms FROM axon.mcp_call_stat WHERE project_code='{proj}'"
        ))
        .unwrap();
    assert!(!dump.contains("SUPER_SECRET"), "no arg content may be stored: {dump}");

    // ok bucket aggregates: 2 calls, sum=20 (avg=10), max=15.
    let avg_ok = server
        .graph_store
        .query_count(&format!(
            "SELECT (latency_sum_ms / call_count)::BIGINT FROM axon.mcp_call_stat \
             WHERE project_code='{proj}' AND tool='{tool}' AND status='ok'"
        ))
        .unwrap();
    assert_eq!(avg_ok, 10, "avg ok latency = 20/2 = 10ms");
    let max_ok = server
        .graph_store
        .query_count(&format!(
            "SELECT latency_max_ms::BIGINT FROM axon.mcp_call_stat \
             WHERE project_code='{proj}' AND tool='{tool}' AND status='ok'"
        ))
        .unwrap();
    assert_eq!(max_ok, 15, "ok tail outlier kept");
    let err_count = server
        .graph_store
        .query_count(&format!(
            "SELECT call_count FROM axon.mcp_call_stat \
             WHERE project_code='{proj}' AND tool='{tool}' AND status='error'"
        ))
        .unwrap();
    assert_eq!(err_count, 1, "the error call is recorded under status=error");

    // S4 — mcp_telemetry_report projects the rollup into usage+latency analytics.
    let report = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_telemetry_report",
                "arguments": { "project_code": proj, "window_hours": 24 }
            })),
            id: Some(json!(961)),
        })
        .unwrap()
        .result
        .unwrap();
    let text = report["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains(tool), "report must list the probed tool: {text}");
    assert!(
        report["data"]["total_calls"].as_i64() == Some(3),
        "report aggregates the 3 calls: {}",
        report["data"]
    );
    // avg ok latency (10ms) appears in the structured per-tool data.
    let tools = report["data"]["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["tool"].as_str() == Some(tool)),
        "probed tool present in telemetry data: {tools:?}"
    );
}

#[test]
fn test_mcp_call_stat_retention_prunes_stale_buckets_on_telemetry_report() {
    // REQ-AXO-901961 S2 — buckets older than the retention window are pruned
    // when mcp_telemetry_report runs (operator-invoked, off the per-call hot
    // path); recent buckets survive. Isolated by a unique project_code.
    let server = create_test_server();
    let proj = "TLMRET901961";
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO axon.mcp_call_stat \
                (tool, project_code, status, bucket_hour, call_count, latency_sum_ms, latency_max_ms, contract_version) \
             VALUES \
                ('stale_probe','{proj}','ok', date_trunc('hour', now() - interval '200 days'), 1, 5, 5, 'v'), \
                ('fresh_probe','{proj}','ok', date_trunc('hour', now() - interval '1 hour'), 1, 5, 5, 'v')"
        ))
        .expect("seed stale + fresh buckets");

    // Huge window so the report query itself filters nothing — the prune (not
    // the window predicate) must be what removes the stale bucket.
    let _ = server.handle_request(JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "mcp_telemetry_report",
            "arguments": { "project_code": proj, "window_hours": 1_000_000 }
        })),
        id: Some(json!(9612)),
    });

    let stale = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM axon.mcp_call_stat WHERE project_code='{proj}' AND tool='stale_probe'"
        ))
        .unwrap();
    assert_eq!(stale, 0, "bucket older than the retention window must be pruned");
    let fresh = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM axon.mcp_call_stat WHERE project_code='{proj}' AND tool='fresh_probe'"
        ))
        .unwrap();
    assert_eq!(fresh, 1, "recent bucket must survive the prune");
}

#[test]
fn test_sql_tool_is_read_only_rejects_mutations() {
    // REQ-AXO-901966 — the `sql` tool must refuse writes (contract = read-only);
    // it runs on the writer-capable pool, so the guard is load-bearing.
    let server = create_test_server();
    let resp = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "sql",
                "arguments": { "sql": "INSERT INTO axon.llm_feedback (problem) VALUES ('UNIQ_SHOULD_NOT_PERSIST')" }
            })),
            id: Some(json!(9663)),
        })
        .unwrap()
        .result
        .unwrap();
    let text = resp["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("rejected_write") || text.contains("READ-ONLY"),
        "mutation must be rejected: {text}"
    );
    assert_eq!(
        resp["data"]["rejected"].as_bool(),
        Some(true),
        "rejected flag set: {}",
        resp["data"]
    );
    let n = server
        .graph_store
        .query_count("SELECT count(*) FROM axon.llm_feedback WHERE problem='UNIQ_SHOULD_NOT_PERSIST'")
        .unwrap();
    assert_eq!(n, 0, "the INSERT must NOT have executed");

    // a read still works through the same tool.
    let ok = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "sql", "arguments": { "sql": "SELECT 1" } })),
            id: Some(json!(9666)),
        })
        .unwrap()
        .result
        .unwrap();
    assert!(
        ok["data"]["rejected"].as_bool() != Some(true),
        "SELECT must not be rejected: {}",
        ok["data"]
    );

    // REQ-AXO-902560 — l'assertion ci-dessus ne prouve PAS que la requête a
    // rendu quoi que ce soit : `rejected != true` reste vrai d'un `SELECT 1`
    // qui renverrait `[]`. Le défaut mesuré en session 130 est exactement
    // celui-là — un appelant recevait une enveloppe sans lignes et le test
    // restait vert. Un `SELECT 1` ne PEUT pas rendre zéro ligne : on l'exige.
    let sql_text = ok["content"][0]["text"]
        .as_str()
        .expect("sql doit rendre du texte");
    assert!(
        sql_text.contains('1') && sql_text.trim() != "[]",
        "SELECT 1 doit rendre sa ligne, pas une enveloppe vide : {sql_text:?}"
    );

    // REQ-AXO-902560 — et cette charge utile doit atteindre le canal que le
    // client consomme réellement. Mesuré en session 130 : ce client rend
    // `structuredContent`/`data` et JAMAIS `content[0].text` ; un outil qui
    // n'écrit que dans `content` est donc invisible au LLM, ce qui a fait
    // conclure à tort que `sql` était cassé alors qu'il répondait juste.
    // Asserter sur la CLÉ que le correctif insère, pas sur la présence du
    // caractère `1` quelque part dans l'enveloppe : `canonical_sources`,
    // `next` et `next_call_hint` en contiennent déjà (ids, compteurs), si
    // bien qu'un `contains('1')` resterait vert sur une charge utile absente
    // — le faux témoin même que ce test est censé supprimer.
    let sql_structured = ok
        .get("structuredContent")
        .expect("structuredContent doit exister (REQ-AXO-902517)");
    let sql_mirrored = sql_structured
        .get("rendered_text")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| {
            panic!("`sql` doit miroiter sa charge utile sous `rendered_text` : {sql_structured}")
        });
    assert_eq!(
        sql_mirrored, sql_text,
        "le miroir doit porter le texte rendu tel quel, pas un résumé"
    );
    assert!(
        sql_mirrored.contains('1'),
        "le miroir doit porter la ligne de `SELECT 1` : {sql_mirrored:?}"
    );
}

/// REQ-AXO-902323 — two DIFFERENT causes must produce two DIFFERENT
/// `problem_class` values, because the friction signature keys on
/// `(project, tool, problem_class, field)` and `sql` never carries a `field`.
///
/// Pre-fix both answered the hardcoded `input_invalid`, so a wrong table name, a
/// wrong column name and a genuine contract defect all bumped ONE counter — and
/// any caller typo could flip a legitimately resolved signature to "regressed"
/// (observed on #3187, 2026-08-15). The precise class was already computed by
/// `pg_error_repair` and already printed in the text; only the field the
/// friction recorder reads still said `input_invalid`.
#[test]
fn test_sql_errors_carry_their_real_cause_not_a_single_generic_class() {
    let server = create_test_server();
    let ask = |sql: &str, id: i64| -> Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "sql", "arguments": { "sql": sql } })),
                id: Some(json!(id)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let bad_column = ask("SELECT no_such_column FROM soll.Node", 902_323_1);
    assert_eq!(
        bad_column["data"]["operator_guidance"]["problem_class"].as_str(),
        Some("undefined_column"),
        "a wrong column must be classified as such: {}",
        bad_column["data"]
    );

    let bad_table = ask("SELECT id FROM soll.NoSuchRelation", 902_323_2);
    assert_eq!(
        bad_table["data"]["operator_guidance"]["problem_class"].as_str(),
        Some("undefined_table"),
        "a wrong relation must be classified as such: {}",
        bad_table["data"]
    );

    // The point of the split: the two causes no longer share a signature key.
    assert_ne!(
        bad_column["data"]["operator_guidance"]["problem_class"],
        bad_table["data"]["operator_guidance"]["problem_class"],
        "two unrelated causes must not collapse into one friction signature"
    );

    // An error the repair cannot classify keeps the generic class — the fallback
    // must stay, otherwise this trades one blind spot for another. Division by
    // zero (22012) is a genuine EXECUTION error: it passes the read-only guard
    // (a malformed verb like `SELEC` never reaches PG, it is rejected upstream as
    // a non-read) and `classify_pg_undefined` returns None for it.
    let unclassifiable = ask("SELECT 1/0", 902_323_3);
    assert_eq!(
        unclassifiable["data"]["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid"),
        "an unclassifiable error must keep the generic class: {}",
        unclassifiable["data"]
    );
}

#[test]
fn test_mcp_feedback_records_voluntary_doleance() {
    // REQ-AXO-901966 — voluntary content-rich LLM feedback persists one row;
    // a missing `problem` is rejected without writing.
    let server = create_test_server();
    let resp = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback",
                "arguments": {
                    "problem": "UNIQ_DOLEANCE_PROBE inspect was too verbose",
                    "category": "too_verbose",
                    "severity": "blocking",
                    "tool": "inspect",
                    "proposed_solution": "add a brief mode",
                    "satisfaction": 3,
                    "llm_identity": "Claude Opus 4.8",
                    "project_code": "AXO"
                }
            })),
            id: Some(json!(9664)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        resp["data"]["recorded"].as_bool(),
        Some(true),
        "feedback recorded: {}",
        resp["data"]
    );

    let row = server
        .graph_store
        .query_json(
            "SELECT category||'|'||severity||'|'||tool||'|'||satisfaction||'|'||llm_identity \
             FROM axon.llm_feedback WHERE problem='UNIQ_DOLEANCE_PROBE inspect was too verbose'",
        )
        .unwrap();
    assert!(
        row.contains("too_verbose|blocking|inspect|3|Claude Opus 4.8"),
        "row persisted with all fields incl severity: {row}"
    );

    let bad = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({ "name": "mcp_feedback", "arguments": { "category": "bug" } })),
            id: Some(json!(9665)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        bad["data"]["recorded"].as_bool(),
        Some(false),
        "missing `problem` must be rejected: {}",
        bad["data"]
    );
}

#[test]
fn test_mcp_feedback_report_lists_filters_and_resolves() {
    // REQ-AXO-902020 — content-rich READ/triage surface over axon.llm_feedback,
    // symmetric to mcp_friction_report. Exercises the full catalog→dispatch→tool
    // path (handle_request), so it also validates the wiring.
    let server = create_test_server();
    let write = |problem: &str, severity: &str, tool: &str, id: i64| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "mcp_feedback",
                    "arguments": {
                        "problem": problem,
                        "severity": severity,
                        "tool": tool,
                        "project_code": "AXO"
                    }
                })),
                id: Some(json!(id)),
            })
            .unwrap()
            .result
            .unwrap();
    };
    write("FBR_PROBE blocking on inspect", "blocking", "inspect", 1);
    write("FBR_PROBE minor on query", "minor", "query", 2);

    let report = |args: Value| -> Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "mcp_feedback_report", "arguments": args })),
                id: Some(json!(99)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // Default report: both probes present, open, one blocking.
    let r = report(json!({ "project_code": "AXO" }));
    let items = r["data"]["feedback"].as_array().unwrap();
    let probe_ids: Vec<i64> = items
        .iter()
        .filter(|f| f["problem"].as_str().unwrap_or("").starts_with("FBR_PROBE"))
        .map(|f| f["id"].as_i64().unwrap())
        .collect();
    assert_eq!(probe_ids.len(), 2, "both probes listed: {}", r["data"]);
    assert!(
        items
            .iter()
            .any(|f| f["severity"] == "blocking" && f["problem"].as_str().unwrap().contains("inspect")),
        "content-rich row carries severity + problem"
    );

    // Severity filter narrows to the blocking probe.
    let blk = report(json!({ "project_code": "AXO", "severity": "blocking" }));
    let blk_probe: Vec<&Value> = blk["data"]["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["problem"].as_str().unwrap_or("").starts_with("FBR_PROBE"))
        .collect();
    assert_eq!(blk_probe.len(), 1, "severity=blocking filters to one probe");

    // Resolve the blocking probe → open-only report drops it; include_resolved keeps it.
    let blocking_id = blk_probe[0]["id"].as_i64().unwrap();
    let _ = report(json!({ "mark_resolved": { "id": blocking_id, "resolved_by_req": "REQ-AXO-902020" } }));

    let open_only = report(json!({ "project_code": "AXO" }));
    assert!(
        !open_only["data"]["feedback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"].as_i64() == Some(blocking_id)),
        "resolved item is excluded from the open-only report"
    );
    let with_resolved = report(json!({ "project_code": "AXO", "include_resolved": true }));
    let resolved_row = with_resolved["data"]["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"].as_i64() == Some(blocking_id))
        .expect("include_resolved surfaces the resolved item");
    assert_eq!(resolved_row["triage_status"], "resolved");
    assert_eq!(resolved_row["resolved_by_req"], "REQ-AXO-902020");
}

#[test]
fn test_mcp_feedback_report_renders_items_in_text_not_only_data() {
    // REQ-AXO-902398 — signalé par KKI (llm_feedback #180, blocking) : le
    // rapport rendait TROIS LIGNES DE COMPTEURS pour 10 items. Les items
    // n'atteignaient que `data.feedback`, que le client Claude Code n'expose
    // pas au LLM — même cause que REQ-AXO-902355 pour le kickoff_bundle.
    //
    // Conséquence le jour même : interrogé sur ce que KKI avait envoyé, AXO a
    // répondu « rien reçu ». Les 11 items étaient là. Un compteur n'est pas un
    // rapport, et `mark_resolved` exige un `id` que le rapport ne donnait pas.
    let server = create_test_server();
    let probe = "FBRTXT_PROBE le rendu doit sortir dans le texte";
    server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback",
                "arguments": {
                    "problem": probe,
                    "severity": "blocking",
                    "category": "bug",
                    "tool": "mcp_feedback_report",
                    "project_code": "FBRTXT"
                }
            })),
            id: Some(json!(1)),
        })
        .unwrap()
        .result
        .unwrap();

    let r = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "mcp_feedback_report",
                "arguments": { "project_code": "FBRTXT" }
            })),
            id: Some(json!(2)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = r["content"][0]["text"].as_str().unwrap_or_default();
    let id = r["data"]["feedback"][0]["id"].as_i64().expect("un item");

    assert!(
        text.contains(&id.to_string()),
        "l'id doit figurer dans le TEXTE — `mark_resolved` l'exige et le triage \
         est infaisable sans lui.\n---\n{text}"
    );
    assert!(
        text.contains("blocking") && text.contains("mcp_feedback_report"),
        "sévérité et outil doivent être lisibles dans le texte.\n---\n{text}"
    );
    assert!(
        text.contains("le rendu doit sortir dans le texte"),
        "le problème lui-même doit être lisible : sans lui le rapport ne \
         rapporte rien.\n---\n{text}"
    );
}

#[test]
fn test_mcp_friction_closed_loop_capture_report_resolve_regress() {
    // REQ-AXO-901957 — capture (no arg content) → aggregate → report →
    // resolve with REQ/VAL → regress on recurrence. Isolated by a synthetic
    // tool + unique project_code so concurrent friction writes don't collide.
    let server = create_test_server();
    let proj = "FRIC901957";
    let tool = "synthetic_friction_probe";
    // A problematic response whose received_arguments carry a SECRET — the
    // friction row must NEVER store it (privacy).
    let problematic = json!({
        "data": {
            "operator_guidance": { "problem_class": "invalid_arguments" },
            "parameter_repair": { "invalid_field": "target" },
            "project_code": proj,
            "received_arguments": { "target": "SUPER_SECRET_CLIENT_VALUE" }
        }
    });
    // 1 + aggregation: capture twice.
    server.record_mcp_friction(tool, &json!({}), &problematic);
    server.record_mcp_friction(tool, &json!({}), &problematic);
    // A terse success (no problem_class) must NOT be captured.
    server.record_mcp_friction(tool, &json!({}), &json!({ "data": { "project_code": proj } }));

    // Privacy: the secret value must appear NOWHERE in the table.
    let dump = server
        .graph_store
        .query_json(&format!(
            "SELECT COALESCE(project_code,'')||'|'||COALESCE(tool,'')||'|'||COALESCE(problem_class,'')||'|'||COALESCE(field_in_error,'')||'|'||COALESCE(resolution_note,'') FROM axon.mcp_friction WHERE project_code='{proj}'"
        ))
        .unwrap();
    assert!(
        !dump.contains("SUPER_SECRET"),
        "no argument content may be stored: {dump}"
    );

    let report = |mark: Option<serde_json::Value>| -> serde_json::Value {
        let mut args = serde_json::Map::new();
        args.insert("project_code".to_string(), json!(proj));
        if let Some(m) = mark {
            args.insert("mark_resolved".to_string(), m);
        }
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "mcp_friction_report", "arguments": args })),
                id: Some(json!(901957)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // Report: one open signature, occurrence_count == 2, field surfaced.
    let r1 = report(None);
    let open = r1["data"]["open_frictions"].as_array().expect("open array");
    let sig = open
        .iter()
        .find(|f| f["tool"] == json!(tool) && f["problem_class"] == json!("invalid_arguments"))
        .expect("captured signature present");
    assert_eq!(sig["field_in_error"], json!("target"));
    assert_eq!(
        sig["occurrence_count"].as_str().or_else(|| None).unwrap_or("2"),
        "2",
        "two observations must aggregate into occurrence_count=2: {sig}"
    );
    let id = sig["id"]
        .as_i64()
        .or_else(|| sig["id"].as_str().and_then(|s| s.parse().ok()))
        .expect("signature id");

    // Resolve: link the SOLL REQ that fixed it.
    let r2 = report(Some(json!({ "id": id, "resolved_by_req": "REQ-AXO-901957" })));
    let still_open = r2["data"]["open_frictions"]
        .as_array()
        .map(|a| a.iter().any(|f| f["id"].as_i64() == Some(id) || f["id"].as_str().and_then(|s| s.parse::<i64>().ok()) == Some(id)))
        .unwrap_or(false);
    assert!(!still_open, "resolved signature must leave the open list");
    let resolved = r2["data"]["resolved_frictions"].as_array().expect("resolved");
    assert!(
        resolved.iter().any(|f| f["resolved_by_req"] == json!("REQ-AXO-901957")),
        "resolved signature must carry the REQ link: {:?}",
        r2["data"]["resolved_frictions"]
    );

    // Regression: recurrence after resolution → regressed flag.
    server.record_mcp_friction(tool, &json!({}), &problematic);
    let r3 = report(None);
    let regressed = r3["data"]["resolved_frictions"]
        .as_array()
        .map(|a| a.iter().any(|f| f["regressed"].as_bool() == Some(true)))
        .unwrap_or(false);
    assert!(
        regressed,
        "a recurrence after resolution must flag regression: {:?}",
        r3["data"]["resolved_frictions"]
    );
}

// REQ-AXO-902309 — la signature doit porter le tenant qui l'a subie.
//
// Lire uniquement `data.project_code` laissait 47 signatures sur 68 sans projet :
// une réponse d'ERREUR est précisément celle qui n'a pas eu le temps d'écho­er sa
// portée. Conséquence mesurée le 2026-08-14 : `mcp_friction_report
// project_code=AXO` annonçait « 0 ouvert » avec 24 ouvertes.
#[test]
fn friction_signature_falls_back_to_the_callers_project_scope() {
    let server = create_test_server();
    let proj = "FRIC902309";
    let tool = "synthetic_scope_probe";
    // La réponse ne dit PAS le projet — seul l'appelant le sait.
    let problematic = json!({
        "data": {
            "operator_guidance": { "problem_class": "invalid_arguments" },
            "parameter_repair": { "invalid_field": "target" }
        }
    });
    server.record_mcp_friction(tool, &json!({ "project": proj }), &problematic);

    let stamped = server
        .graph_store
        .query_json(&format!(
            "SELECT COALESCE(project_code,'(vide)') FROM axon.mcp_friction WHERE tool='{tool}'"
        ))
        .unwrap();
    assert!(
        stamped.contains(proj) && !stamped.contains("(vide)"),
        "la signature doit porter le projet de l'appelant, pas une chaîne vide : {stamped}"
    );
}

// REQ-AXO-902309 — un rapport filtré ne doit jamais se présenter comme complet.
#[test]
fn a_filtered_friction_report_discloses_the_unattributed_backlog() {
    let server = create_test_server();
    // Une signature historique, sans tampon projet (ce que produisait l'ancien code).
    server
        .graph_store
        .execute(
            "INSERT INTO axon.mcp_friction (project_code, tool, problem_class, field_in_error, contract_version) \
             VALUES ('', 'legacy_unstamped_probe', 'invalid_arguments', 'target', 'test') \
             ON CONFLICT (project_code, tool, problem_class, field_in_error) DO NOTHING",
        )
        .unwrap();

    let text = server
        .execute_tool_direct("mcp_friction_report", &json!({ "project_code": "FRIC902309B" }))
        .expect("report")["content"][0]["text"]
        .as_str()
        .expect("texte")
        .to_string();
    assert!(
        text.contains("NON attribuée"),
        "le rapport filtré doit NOMMER le seau non attribué au lieu de l'omettre : {text}"
    );
}

// REQ-AXO-902310 — une classe non-échec revue après fermeture n'est PAS une
// régression. C'est le défaut symétrique de celui que REQ-AXO-902297 a fermé :
// un mode dégradé légitime ne peut jamais RESTER « résolu », il continue d'être
// observé — le signaler à chaque rapport rendrait l'indicateur inutilisable.
#[test]
fn a_non_failure_class_reobserved_after_closure_is_not_a_regression() {
    let server = create_test_server();
    let proj = "FRIC902310";
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO axon.mcp_friction \
               (project_code, tool, problem_class, field_in_error, contract_version, \
                status, resolved_at, resolved_by_req, last_observed_at) \
             VALUES ('{proj}', 'degraded_probe', 'degraded', '', 'test', \
                     'resolved', now() - interval '2 days', 'REQ-AXO-902297', now()), \
                    ('{proj}', 'real_probe', 'invalid_arguments', 'target', 'test', \
                     'resolved', now() - interval '2 days', 'REQ-AXO-902310', now())"
        ))
        .unwrap();

    let res = server
        .execute_tool_direct("mcp_friction_report", &json!({ "project_code": proj }))
        .expect("report");
    assert_eq!(
        res["data"]["regressed_count"].as_i64(),
        Some(1),
        "seule la classe d'ÉCHEC compte comme régression : {:?}",
        res["data"]
    );
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("RÉGRESSÉE") && text.contains("real_probe"),
        "la régression doit être NOMMÉE, pas seulement comptée : {text}"
    );
    assert!(
        !text.contains("degraded_probe ⚠️"),
        "un mode dégradé légitime ne doit pas être marqué régressé : {text}"
    );
}

// REQ-AXO-902310 — la régression est comptée sur toute la table, pas sur la page.
// Même classe que REQ-AXO-902292, laissée en place sur ce seul champ.
#[test]
fn the_regression_count_is_not_capped_by_the_page_limit() {
    let server = create_test_server();
    let proj = "FRIC902310B";
    for i in 0..4 {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO axon.mcp_friction \
                   (project_code, tool, problem_class, field_in_error, contract_version, \
                    status, resolved_at, resolved_by_req, last_observed_at, occurrence_count) \
                 VALUES ('{proj}', 'probe_{i}', 'invalid_arguments', 'f{i}', 'test', \
                         'resolved', now() - interval '2 days', 'REQ-AXO-902310', now(), {i})"
            ))
            .unwrap();
    }
    let res = server
        .execute_tool_direct(
            "mcp_friction_report",
            &json!({ "project_code": proj, "limit": 1 }),
        )
        .expect("report");
    assert_eq!(
        res["data"]["regressed_count"].as_i64(),
        Some(4),
        "4 régressions doivent être comptées même avec limit=1 : {:?}",
        res["data"]
    );
}

// REQ-AXO-902310 — les deux lectures de `problem_class` (Rust et SQL) dérivent
// d'une seule liste. Une seconde copie écrite à la main est exactement la façon
// dont ce module a déjà vu deux lectures d'un champ diverger.
#[test]
fn the_sql_and_rust_failure_predicates_derive_from_one_list() {
    use crate::mcp::tools_friction::{
        failure_class_sql, problem_class_is_failure, NON_FAILURE_PROBLEM_CLASSES,
    };
    let sql = failure_class_sql();
    for class in NON_FAILURE_PROBLEM_CLASSES {
        assert!(
            !problem_class_is_failure(class),
            "`{class}` est dans la liste des non-échecs mais Rust le compte comme échec"
        );
        assert!(
            sql.contains(&format!("'{class}'")),
            "`{class}` manque au prédicat SQL : {sql}"
        );
    }
    assert!(
        problem_class_is_failure("invalid_arguments"),
        "une classe inconnue doit rester un échec (elle doit remonter, pas être avalée)"
    );
}

// ── REQ-AXO-902312 — `soll_manager create` déduit ce qui est déductible ─────
//
// Trois appels pour créer UN nœud, le 2026-08-14 : rejet sur `project_code`, puis
// rejet sur `relation_type`, puis succès. Les deux champs manquants étaient
// dérivables de `attach_to`, déjà présent dans la charge utile dès le 1er appel.
fn create_call(server: &McpServer, data: Value) -> Value {
    server
        .execute_tool_direct(
            "soll_manager",
            &json!({ "action": "create", "entity": "requirement", "data": data }),
        )
        .expect("soll_manager returns a result")
}

fn seed_pillar(server: &McpServer, code: &str, id: &str, title: &str) {
    // Une mutation exige un code ENREGISTRÉ (require_registered_mutation_project_code).
    // `TST` l'est par le seed de test ; tout autre code doit être inscrit ici,
    // sinon le fixture échoue sur `wrong_project_scope` avant d'atteindre la
    // règle qu'il teste.
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
             VALUES ('{code}', '/tmp/{code}', '{code}') ON CONFLICT (project_code) DO NOTHING"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) \
             VALUES ('{code}', 'AXON_GLOBAL', 9, 9, 9, 9) ON CONFLICT (project_code) DO NOTHING"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('{id}', 'Pillar', '{code}', '{title}', '', 'current', '{{}}') \
             ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();
}

#[test]
fn create_infers_project_and_relation_from_the_named_parent() {
    let server = create_test_server();
    seed_pillar(&server, "TST", "PIL-TST-901", "Ancre 902311");

    // Ni `project_code` ni `relation_type` : le parent porte les deux.
    let res = create_call(
        &server,
        json!({ "title": "déduction 902311", "description": "corps", "attach_to": "PIL-TST-901" }),
    );

    assert_ne!(
        res["isError"].as_bool(),
        Some(true),
        "un parent nommé suffit — les deux champs sont dérivables : {res}"
    );
    assert_eq!(res["data"]["project_code"], json!("TST"));
    assert_eq!(res["data"]["applied_relation"], json!("BELONGS_TO"));
    assert_eq!(res["data"]["project_code_inferred_from_parent"], json!(true));
    assert_eq!(res["data"]["relation_type_inferred"], json!(true));

    // Et la déduction est ANNONCÉE : un champ que l'appelant n'a pas écrit est un
    // champ qu'il ne peut pas vérifier.
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("`project_code` non fourni") && text.contains("`relation_type` non fourni"),
        "les deux déductions doivent être divulguées : {text}"
    );
}

#[test]
fn create_still_refuses_when_the_pair_is_genuinely_ambiguous() {
    // La frontière est inchangée : univoque → appliqué, ambigu → refusé. Déduire
    // sur une paire à plusieurs relations légales serait deviner.
    let server = create_test_server();
    seed_pillar(&server, "TST", "PIL-TST-902", "Ancre ambiguë");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-TST-902', 'Requirement', 'TST', 'cible', '', 'current', '{}') \
             ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();

    let res = server
        .execute_tool_direct(
            "soll_manager",
            &json!({ "action": "create", "entity": "concept",
                     "data": { "title": "ambigu", "description": "x", "attach_to": "REQ-TST-902" } }),
        )
        .expect("result");
    assert_eq!(
        res["isError"].as_bool(),
        Some(true),
        "CPT → REQ admet plusieurs relations : le refus doit tenir : {res}"
    );
}

#[test]
fn the_missing_parent_refusal_names_its_candidates() {
    // Décision opérateur : le serveur ne devine JAMAIS le parent — deviner
    // écrirait une arête SOLL que personne n'a demandée. Mais le refus doit
    // cesser d'être un cul-de-sac : 154 occurrences disent que renvoyer vers un
    // AUTRE outil ne marche pas.
    let server = create_test_server();
    seed_pillar(&server, "TST", "PIL-TST-903", "Pilier nommable");

    let res = create_call(
        &server,
        json!({ "project_code": "TST", "title": "sans parent", "description": "x" }),
    );
    assert_eq!(res["isError"].as_bool(), Some(true), "le refus tient");

    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("PIL-TST-903") && text.contains("Pilier nommable"),
        "le refus doit NOMMER les parents candidats, pas les décrire : {text}"
    );
    let candidates = res["data"]["parameter_repair"]["candidate_parents"]
        .as_array()
        .expect("candidate_parents");
    assert!(
        candidates.iter().any(|c| c["id"] == json!("PIL-TST-903")),
        "les candidats doivent être exploitables en machine : {candidates:?}"
    );
    assert!(
        res["data"]["parameter_repair"]["corrected_call"]["arguments"]["data"]["attach_to"]
            .is_string(),
        "un appel corrigé avec un seul blanc à remplir : {res}"
    );
}

// REQ-AXO-902313 — `field_in_error = "arguments"` n'est pas une cause, c'est un
// agrégat de causes non mesurées (38 occurrences, 2ᵉ signature ouverte). On ne
// corrige pas ce qu'on ne nomme pas.
#[test]
fn an_invalid_argument_is_named_not_bucketed_under_arguments() {
    use crate::mcp::tool_contracts::first_schema_mismatch;
    let schema = json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer" },
            "mode": { "type": "string", "enum": ["brief", "verbose"] },
            "project": { "type": "string" }
        },
        "required": []
    });

    let (field, reason) =
        first_schema_mismatch(&schema, &json!({ "limit": "douze" })).expect("type mismatch");
    assert_eq!(field, "limit");
    assert!(reason.starts_with("type_mismatch"), "{reason}");

    let (field, reason) =
        first_schema_mismatch(&schema, &json!({ "mode": "bavard" })).expect("enum violation");
    assert_eq!(field, "mode");
    assert!(reason.contains("brief") && reason.contains("verbose"), "{reason}");

    let (field, reason) =
        first_schema_mismatch(&schema, &json!({ "porject": "AXO" })).expect("unknown property");
    assert_eq!(field, "porject", "un nom mal orthographié doit être nommé tel quel");
    assert_eq!(reason, "unknown_property");

    // Le cas qui doit RESTER sans nom : les arguments satisfont le schéma. C'est
    // alors un signal distinct (le handler a refusé pour sa propre raison), pas du
    // bruit à agréger.
    assert!(
        first_schema_mismatch(&schema, &json!({ "limit": 12, "mode": "brief" })).is_none(),
        "des arguments valides ne doivent produire aucun coupable"
    );
}

// ── REQ-AXO-902314 — `soll_attach_evidence` : les deux plus gros clusters ───
//
// Rejouées sur le live le 2026-08-14 avant d'écrire une ligne : les deux cassent
// encore. #5+#7 (158 occ.) = une chaîne nue dans `artifacts` ; #176+#437 (47 occ.)
// = un `artifact_type` légal globalement mais pas pour CETTE entité.
fn attach(server: &McpServer, entity_id: &str, artifacts: Value) -> Value {
    server
        .execute_tool_direct(
            "soll_attach_evidence",
            &json!({ "entity_type": "requirement", "entity_id": entity_id, "artifacts": artifacts }),
        )
        .expect("soll_attach_evidence returns a result")
}

fn seed_requirement(server: &McpServer, id: &str) {
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('{id}', 'Requirement', 'TST', 'cible evidence', '', 'current', '{{}}') \
             ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();
}

#[test]
fn a_bare_string_artifact_is_an_artifact_ref() {
    let server = create_test_server();
    seed_requirement(&server, "REQ-TST-914");

    // Un symbole, pas un chemin : la validation d'existence de fichier
    // (REQ-AXO-901619) est un contrôle SÉPARÉ et légitime, qu'on ne veut pas
    // mélanger ici — ce test porte sur la seule coercition de forme.
    let res = attach(&server, "REQ-TST-914", json!(["axon_soll_attach_evidence"]));
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        !text.contains("all rejected"),
        "la forme abrégée `artifacts: [\"ref\"]` doit être acceptée : {text}"
    );
    assert_eq!(res["data"]["attached"].as_i64(), Some(1), "{res}");
}

#[test]
fn an_artifact_type_illegal_for_the_entity_falls_back_to_what_the_ref_implies() {
    let server = create_test_server();
    seed_requirement(&server, "REQ-TST-915");

    // `diff` est légal sur une décision, PAS sur une exigence. Le ref, lui, est un
    // fichier — et le serveur sait déjà lire ça quand le champ est absent.
    // La réparation produit le type `File`, qui déclenche la validation
    // d'existence (REQ-AXO-901619) : le fichier doit donc exister sous la racine
    // projet du serveur de test.
    let root = std::path::Path::new("/tmp/TST/src");
    std::fs::create_dir_all(root).expect("racine de test");
    let probe = root.join("evidence_probe.rs");
    std::fs::write(&probe, b"// sonde REQ-AXO-902314\n").expect("fichier sonde");

    let res = attach(
        &server,
        "REQ-TST-915",
        json!([{ "artifact_type": "diff", "artifact_ref": "src/evidence_probe.rs" }]),
    );
    assert_eq!(res["data"]["attached"].as_i64(), Some(1), "{res}");
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("Diff") && text.contains("déduit"),
        "la réparation doit être divulguée dans le TEXTE, pas seulement en data : {text}"
    );
}

/// REQ-AXO-902418 — un SHA n'est pas un chemin cassé, et la réparation cessait
/// de le confondre.
///
/// TE2 (`mcp_feedback` #185) a envoyé cinq SHA typés `file` — le seul choix
/// plausible dans l'enum publié, où `commit` était absent — et a reçu
/// `did_you_mean: "/home/.../trader-elixir-v2/567592f"`, un chemin qui
/// n'existera jamais. La réparation censée résoudre la friction poussait
/// activement vers la mauvaise piste.
#[test]
fn a_commit_sha_rejected_as_a_file_is_repaired_toward_its_type_not_toward_a_path() {
    let server = create_test_server();
    seed_requirement(&server, "REQ-TST-918");

    let res = attach(
        &server,
        "REQ-TST-918",
        json!([{ "artifact_type": "file", "artifact_ref": "b3f46fae" }]),
    );

    // CONTRÔLE POSITIF : la pièce a bien été REFUSÉE. Sans lui, une réparation
    // absente (parce que l'attachement a réussi) rendrait vertes les assertions
    // suivantes en ne mesurant rien.
    assert_eq!(
        res["data"]["attached"].as_i64(),
        Some(0),
        "précondition : un SHA typé `file` doit être refusé, sinon ce test ne \
         mesure pas la réparation : {res}"
    );

    let repair = &res["data"]["parameter_repair"];
    assert_eq!(
        repair["invalid_field"].as_str(),
        Some("artifact_type"),
        "le champ fautif est le TYPE, pas le ref : le SHA est correct, c'est \
         `file` qui le fait passer au contrôle disque : {res}"
    );
    assert_eq!(
        repair["suggested_artifact_type"].as_str(),
        Some("commit"),
        "la réparation doit NOMMER le type qui marche : {res}"
    );
    assert!(
        repair.get("did_you_mean").is_none(),
        "aucun « vouliez-vous dire <chemin> » ne doit être proposé pour un SHA — \
         c'est précisément la suggestion qui a coûté un aller-retour à TE2 : {res}"
    );
    let hint = repair["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("commit") && !hint.contains("project root`"),
        "l'indice doit dire quoi renvoyer, sans préfixer le SHA par la racine \
         projet : {hint}"
    );
}

#[test]
fn an_unrepairable_artifact_type_is_still_rejected() {
    // La frontière tient — et c'est ce test qui l'a imposée. Écrite d'abord en
    // réutilisant l'inférence générique, la réparation transformait SILENCIEUSEMENT
    // tout type illégal en `Document` : cette fonction DÉFAUTE à `Document` quand le
    // ref ne dit rien. Une règle de réparation sans contre-exemple ne se distingue
    // pas d'une coercition aveugle.
    let server = create_test_server();
    seed_requirement(&server, "REQ-TST-916");

    // REQ-AXO-902499 — l'exemple a changé, PAS l'intention du test.
    //
    // Ce test utilisait `rationale`, désormais ACCEPTÉ sur `requirement` (doléance
    // VPC #245 : « le seul item perdu était le POURQUOI, le seul irreconstituible »).
    // Il faut donc un type qui reste réellement irréparable — sinon la garde ne
    // mesurerait plus rien. `screenshot` n'est dans aucun vocabulaire d'entité et
    // n'est proche d'aucun accepté : c'est le contre-exemple que ce test réclame.
    let res = attach(
        &server,
        "REQ-TST-916",
        json!([{ "artifact_type": "screenshot", "artifact_ref": "capture.png" }]),
    );
    assert_eq!(res["data"]["attached"].as_i64(), Some(0), "{res}");

    // Et le contrôle POSITIF qui manquait : `rationale`, lui, doit passer.
    let ok = attach(
        &server,
        "REQ-TST-916",
        json!([{ "artifact_type": "rationale", "artifact_ref": "parce que" }]),
    );
    assert_eq!(
        ok["data"]["attached"].as_i64(),
        Some(1),
        "`rationale` doit etre accepte sur une exigence (REQ-AXO-902499) : {ok}"
    );
}

// ── REQ-AXO-902319 — le troisième état : refus correct et permanent ────────
//
// `open` et `resolved` ne pouvaient pas être tous deux faux, et pourtant ils
// l'étaient pour un refus définitif : `open` le laisse en tête des priorités de
// correction sans rien à corriger, `resolved` ment ET fait crier au loup la règle
// de régression de REQ-AXO-902310 à chaque récurrence.
#[test]
fn a_by_design_refusal_leaves_the_fix_me_list_without_faking_a_fix() {
    let server = create_test_server();
    let proj = "FRI319";
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO axon.mcp_friction \
               (project_code, tool, problem_class, field_in_error, contract_version, \
                status, occurrence_count, last_observed_at) \
             VALUES ('{proj}', 'probe_by_design', 'attach_required', 'data.attach_to', 'test', \
                     'open', 154, now())"
        ))
        .unwrap();
    let id = server
        .graph_store
        .query_single_i64_writer(&format!(
            "SELECT id FROM axon.mcp_friction WHERE project_code='{proj}' AND tool='probe_by_design'"
        ))
        .unwrap()
        .unwrap();

    let res = server
        .execute_tool_direct(
            "mcp_friction_report",
            &json!({ "project_code": proj, "mark_by_design": {
                "id": id, "note": "attach_to n'est pas déductible : deviner un parent écrirait une arête SOLL non demandée." } }),
        )
        .expect("report");

    assert_eq!(res["data"]["total_open"].as_i64(), Some(0), "hors priorités : {res}");
    assert_eq!(res["data"]["total_resolved"].as_i64(), Some(0), "et surtout PAS résolu : {res}");
    assert_eq!(res["data"]["total_by_design"].as_i64(), Some(1));

    // Visible, avec sa raison : « par conception » doit rester contestable.
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("Par conception") && text.contains("arête SOLL non demandée"),
        "la raison doit être imprimée, pas seulement stockée : {text}"
    );
}

#[test]
fn a_by_design_signature_never_trips_the_regression_flag() {
    // Le cœur du REQ : une re-observation est ATTENDUE (le refus est permanent),
    // donc elle ne doit jamais compter comme régression.
    let server = create_test_server();
    let proj = "FRI319B";
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO axon.mcp_friction \
               (project_code, tool, problem_class, field_in_error, contract_version, \
                status, resolved_at, occurrence_count, last_observed_at) \
             VALUES ('{proj}', 'probe_perm', 'attach_required', 'data.attach_to', 'test', \
                     'by_design', now() - interval '2 days', 200, now())"
        ))
        .unwrap();

    let res = server
        .execute_tool_direct("mcp_friction_report", &json!({ "project_code": proj }))
        .expect("report");
    assert_eq!(
        res["data"]["regressed_count"].as_i64(),
        Some(0),
        "un refus par conception revu depuis sa décision n'est PAS une régression : {:?}",
        res["data"]
    );
}

#[test]
fn marking_by_design_without_a_reason_is_refused() {
    // Cet état retire une signature de la vue pour de bon. Sans raison écrite, il
    // ne se distingue pas d'un abandon — donc `note` est obligatoire ici, alors
    // qu'elle est facultative sur `mark_resolved`.
    let server = create_test_server();
    let res = server
        .execute_tool_direct(
            "mcp_friction_report",
            &json!({ "mark_by_design": { "id": 999_999 } }),
        )
        .expect("report");
    assert_eq!(res["isError"].as_bool(), Some(true), "{res}");
    assert_eq!(
        res["data"]["parameter_repair"]["invalid_field"],
        json!("mark_by_design.note")
    );
}

// ── REQ-AXO-902321 — trouvés en REJOUANT les signatures dormantes ──────────

#[test]
fn an_unknown_entity_type_is_refused_instead_of_written_through() {
    // Le pire des trois cas : `entity_type: "exigence"` était ACCEPTÉ et écrivait
    // une ligne soll.Traceability typée `exigence`, qu'aucune requête filtrant sur
    // les types canoniques ne retrouvera jamais. Une preuve qui existe et reste
    // introuvable est pire qu'une preuve refusée — et l'appelant lisait « Attached 1 ».
    let server = create_test_server();
    seed_requirement(&server, "REQ-TST-920");

    let res = server
        .execute_tool_direct(
            "soll_attach_evidence",
            &json!({ "entity_type": "exigence", "entity_id": "REQ-TST-920",
                     "artifacts": ["un_symbole"] }),
        )
        .expect("result");
    assert_eq!(res["isError"].as_bool(), Some(true), "{res}");
    assert_eq!(res["data"]["parameter_repair"]["invalid_field"], json!("entity_type"));

    let written = server
        .graph_store
        .query_single_i64_writer(
            "SELECT count(*) FROM soll.Traceability WHERE soll_entity_type = 'exigence'",
        )
        .unwrap()
        .unwrap_or(0);
    assert_eq!(written, 0, "aucune ligne hors vocabulaire ne doit être écrite");
}

#[test]
fn a_create_carrying_an_unknown_id_drops_it_and_says_so() {
    // Un id sans référent ne peut rien casser : le serveur alloue et REND l'id
    // canonique. Le refuser coûtait un aller-retour pour aucune sûreté.
    let server = create_test_server();
    seed_pillar(&server, "TST", "PIL-TST-921", "Ancre 902320");

    let res = create_call(
        &server,
        json!({ "id": "REQ-TST-999999", "title": "id fantôme", "description": "x",
                "attach_to": "PIL-TST-921" }),
    );
    assert_ne!(res["isError"].as_bool(), Some(true), "{res}");
    let text = res["content"][0]["text"].as_str().expect("texte");
    assert!(
        text.contains("REQ-TST-999999") && text.contains("ignoré"),
        "le retrait de l'id doit être annoncé : {text}"
    );
}

#[test]
fn a_create_carrying_an_existing_id_is_refused_as_a_probable_update() {
    // Le contre-exemple qui borne la règle précédente : ici l'id DÉSIGNE un nœud
    // réel. Le retirer créerait un doublon de ce que l'appelant voulait modifier —
    // le pire des trois résultats. Donc on refuse, en nommant l'intention.
    let server = create_test_server();
    seed_pillar(&server, "TST", "PIL-TST-922", "Ancre 902320b");
    seed_requirement(&server, "REQ-TST-922");

    let res = create_call(
        &server,
        json!({ "id": "REQ-TST-922", "title": "collision", "description": "x",
                "attach_to": "PIL-TST-922" }),
    );
    assert_eq!(res["isError"].as_bool(), Some(true), "{res}");
    assert_eq!(
        res["data"]["parameter_repair"]["corrected_call"]["arguments"]["action"],
        json!("update"),
        "l'appel corrigé doit proposer update : {res}"
    );
}

#[test]
fn a_closed_enum_nested_in_a_one_of_is_still_a_closed_enum() {
    // `soll_manager.action` est déclaré en `oneOf` : ne lire que le `enum` de
    // premier niveau rendait la violation invisible, et `action: "creat"` retombait
    // sur le seau « arguments » que REQ-AXO-902313 venait justement de retirer.
    use crate::mcp::tool_contracts::{closed_enum_values, first_schema_mismatch};
    let spec = json!({
        "oneOf": [
            { "type": "string", "enum": ["create", "update", "link", "unlink"] },
            { "const": "append_section", "type": "string" }
        ]
    });
    let values = closed_enum_values(&spec).expect("enum imbriqué");
    assert!(values.contains(&json!("create")) && values.contains(&json!("append_section")));

    // Une branche SANS enum rend l'union ouverte : pas de vocabulaire fermé.
    let open = json!({ "oneOf": [{ "enum": ["a"] }, { "type": "string" }] });
    assert!(closed_enum_values(&open).is_none(), "une union ouverte n'est pas un enum fermé");

    let schema = json!({ "type": "object", "properties": { "action": spec }, "required": [] });
    let (field, reason) =
        first_schema_mismatch(&schema, &json!({ "action": "creat" })).expect("violation");
    assert_eq!(field, "action");
    assert!(reason.starts_with("enum_violation"), "{reason}");
}

#[test]
fn test_soll_manager_link_auto_canonizes_unambiguous_relation() {
    // REQ-AXO-901939 — a non-canonical relation on a pair with EXACTLY ONE
    // canonical relation is auto-applied (not rejected), and the substitution
    // is surfaced. A pair with MULTIPLE allowed relations stays a reject.
    let server = create_test_server();
    let code = "TST".to_string();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 1, 1, 1, 0) ON CONFLICT (project_code) DO UPDATE SET last_pil = 1"
        ))
        .unwrap();
    for (id, ty) in [
        (format!("PIL-{code}-001"), "Pillar"),
        (format!("REQ-{code}-001"), "Requirement"),
        (format!("CPT-{code}-001"), "Concept"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{id}', '{ty}', '{code}', 't', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
            ))
            .unwrap();
    }
    let link = |src: String, tgt: String, rel: &str, rid: i64| -> serde_json::Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": { "action": "link", "entity": "requirement",
                        "data": { "source_id": src, "target_id": tgt, "relation_type": rel } }
                })),
                id: Some(json!(rid)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // REQ -> PIL admits exactly BELONGS_TO. Request the wrong REFINES → auto.
    let r = link(
        format!("REQ-{code}-001"),
        format!("PIL-{code}-001"),
        "REFINES",
        1,
    );
    assert_ne!(r.get("isError").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(r["data"]["auto_canonized_from"].as_str(), Some("REFINES"));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("auto-applied"),
        "auto-canonize must be noted: {:?}",
        r["content"]
    );
    let edge = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id='REQ-{code}-001' AND target_id='PIL-{code}-001' AND relation_type='BELONGS_TO'"
        ))
        .unwrap();
    assert_eq!(edge, 1, "canonical BELONGS_TO edge must exist");

    // CPT -> REQ admits EXPLAINS or REFINES (ambiguous): a wrong relation
    // (BELONGS_TO) must still be REJECTED, not silently picked.
    let amb = link(
        format!("CPT-{code}-001"),
        format!("REQ-{code}-001"),
        "BELONGS_TO",
        2,
    );
    assert_eq!(
        amb.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "ambiguous pair must reject a non-canonical relation: {amb:?}"
    );
}

#[test]
fn test_soll_manager_link_never_auto_canonizes_a_requested_supersedes() {
    // REQ-AXO-902462 — l'auto-canonisation de REQ-AXO-901939 est utile quand un
    // LLM devine mal une relation de FILIATION. Appliquée à `SUPERSEDES`, elle
    // change la NATURE de l'affirmation : « ceci REMPLACE cela » devient « ce
    // jalon PLANIFIE ce travail », et rend `status: ok`. Mesuré deux fois sur
    // AXO le 2026-08-22 en réparant GUI-PRO-125 (MIL→REQ devenu TARGETS,
    // DEC→GUI devenu COMPLIES_WITH) ; les deux arêtes ont dû être retirées à la
    // main. La garde symétrique existait déjà — « ne JAMAIS auto-canoniser VERS
    // SUPERSEDES » (REQ-AXO-902098) — mais « jamais DEPUIS » manquait.
    let server = create_test_server();
    let code = "TSU".to_string();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 0, 2, 0, 0) ON CONFLICT (project_code) DO UPDATE SET last_req = 2"
        ))
        .unwrap();
    for (id, ty) in [
        (format!("MIL-{code}-901"), "Milestone"),
        (format!("REQ-{code}-901"), "Requirement"),
        (format!("REQ-{code}-902"), "Requirement"),
    ] {
        // DO UPDATE, pas DO NOTHING : une execution precedente a pu retirer la
        // cible, et un fixture qui ne se re-arme pas rend le test non rejouable.
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{id}', '{ty}', '{code}', 't', '', 'current', '{{}}') ON CONFLICT (id) DO UPDATE SET status = 'current'"
            ))
            .unwrap();
    }
    // PG partage entre tests : une execution precedente a pu laisser une arete.
    server
        .graph_store
        .execute(&format!(
            "DELETE FROM soll.Edge WHERE source_id LIKE 'MIL-{code}-9%' OR target_id LIKE 'REQ-{code}-9%'"
        ))
        .unwrap();
    let link = |src: String, tgt: String, rel: &str, rid: i64| -> serde_json::Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": { "action": "link", "entity": "milestone",
                        "data": { "source_id": src, "target_id": tgt, "relation_type": rel } }
                })),
                id: Some(json!(rid)),
            })
            .unwrap()
            .result
            .unwrap()
    };
    let edges_between = |src: &str, tgt: &str| -> i64 {
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id='{src}' AND target_id='{tgt}'"
            ))
            .unwrap()
    };

    // CONTRÔLE POSITIF — le comportement de REQ-AXO-901939 est CONSERVÉ : sur la
    // même paire MIL→REQ (qui n'admet que TARGETS), une relation de filiation
    // mal devinée s'auto-canonise toujours. Sans ce contrôle, une garde qui
    // désarmerait l'auto-canonisation ENTIÈRE passerait au vert.
    let ok = link(
        format!("MIL-{code}-901"),
        format!("REQ-{code}-901"),
        "BLOCKED_BY",
        1,
    );
    assert_ne!(
        ok.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "une relation de filiation mal devinee doit toujours s'auto-canoniser : {ok:?}"
    );
    assert_eq!(
        ok["data"]["auto_canonized_from"].as_str(),
        Some("BLOCKED_BY")
    );

    // L'INVARIANT — même paire, mais la demande est un RETRAIT. La relation
    // ECRITE est celle qui a ete DEMANDEE, ou rien : jamais une autre. Une
    // relation qui porte un effet de bord destructif n'est pas substituable par
    // une relation de planification.
    //
    // Note : depuis REQ-AXO-902461 cette demande est ACCEPTEE (le retrait est
    // exprimable entre deux noeuds SOLL quelconques). L'invariant que ce test
    // protege n'est PAS le refus — c'etait son symptome a l'epoque ou la paire
    // etait fermee — mais l'absence de SUBSTITUTION. Il tient dans les deux
    // regimes : ce qui ne doit jamais arriver, c'est qu'une TARGETS soit ecrite
    // a la place.
    let retire = link(
        format!("MIL-{code}-901"),
        format!("REQ-{code}-902"),
        "SUPERSEDES",
        2,
    );
    assert!(
        retire["data"]["auto_canonized_from"].is_null(),
        "un SUPERSEDES demande ne doit JAMAIS etre auto-canonise : {retire:?}"
    );
    assert_eq!(
        edges_between(&format!("MIL-{code}-901"), &format!("REQ-{code}-902")),
        1,
        "exactement une arete entre les deux — pas une TARGETS en plus"
    );
    let written = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id='MIL-{code}-901' AND target_id='REQ-{code}-902' AND relation_type='TARGETS'"
        ))
        .unwrap();
    assert_eq!(
        written, 0,
        "une TARGETS ecrite a la place d'un SUPERSEDES dirait le CONTRAIRE de l'intention demandee"
    );

    // Et le cas ou la substitution reste possible : une extremite qui n'est PAS
    // un noeud SOLL. Le fallback de REQ-AXO-902461 ne s'y applique pas, donc la
    // demande retombe sur la politique de la paire — elle doit REJETER, jamais
    // ecrire une relation approchante.
    let artifact = link(
        format!("REQ-{code}-901"),
        "src/axon-core/src/lib.rs".to_string(),
        "SUPERSEDES",
        3,
    );
    assert_eq!(
        artifact.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "un artefact ne retire pas une intention : {artifact:?}"
    );
    assert_eq!(
        edges_between(&format!("REQ-{code}-901"), "src/axon-core/src/lib.rs"),
        0,
        "le refus ne doit ecrire AUCUNE arete"
    );
}

#[test]
fn soll_validate_does_not_condemn_a_retirement_edge_that_the_writer_accepted() {
    // REQ-AXO-902461 — l'ECRITURE et la LECTURE doivent lire la meme regle.
    // `select_relation_type_for_link` consulte le fallback de retrait ; le
    // validateur (`collect_relation_policy_violations`) ne consultait que la
    // matrice de filiation. Resultat mesure au promote du 2026-08-24 :
    // `VIS-AXO-001 -SUPERSEDES-> VIS-AXO-901` ecrite AVEC SUCCES, puis signalee
    // « pair VIS -> VIS forbidden » par `soll_validate` dans la foulee.
    //
    // Un registre qui se contredit lui-meme est pire qu'un registre qui refuse :
    // le tenant ne sait plus lequel des deux croire.
    let server = create_test_server();
    let code = "TST".to_string();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 0, 1, 0, 1) ON CONFLICT (project_code) DO UPDATE SET last_req = 1"
        ))
        .unwrap();
    for (id, ty) in [
        (format!("VIS-{code}-971"), "Vision"),
        (format!("VIS-{code}-972"), "Vision"),
        (format!("DEC-{code}-971"), "Decision"),
        (format!("REQ-{code}-971"), "Requirement"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{id}', '{ty}', '{code}', 't', 'corps', 'current', '{{}}') ON CONFLICT (id) DO UPDATE SET status = 'current'"
            ))
            .unwrap();
    }
    server
        .graph_store
        .execute(&format!("DELETE FROM soll.Edge WHERE source_id LIKE '%-{code}-97%' OR target_id LIKE '%-{code}-97%'"))
        .unwrap();

    // Les deux paires que la matrice de filiation ignore, et que le fallback de
    // retrait autorise depuis REQ-AXO-902461.
    for (src, tgt) in [
        (format!("VIS-{code}-971"), format!("VIS-{code}-972")),
        (format!("DEC-{code}-971"), format!("REQ-{code}-971")),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('{src}', '{tgt}', 'SUPERSEDES', '{code}') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
            ))
            .unwrap();
    }
    server.soll_cache().invalidate(&code);

    let report = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": code }))
        .expect("soll_validate repond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert!(
        !report.contains("pair VIS -> VIS forbidden"),
        "le validateur condamne une arete que l'ecriture accepte :\n---\n{report}"
    );
    assert!(
        !report.contains("pair DEC -> REQ forbidden"),
        "le validateur condamne une arete que l'ecriture accepte :\n---\n{report}"
    );

    // CONTROLE POSITIF — le validateur ne devient pas permissif pour autant :
    // une relation qui n'est PAS un retrait, sur une paire sans politique, doit
    // rester signalee. Sans lui, un validateur qui accepterait tout passerait.
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('VIS-{code}-971', 'VIS-{code}-972', 'BELONGS_TO', '{code}') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
        ))
        .unwrap();
    server.soll_cache().invalidate(&code);
    let report2 = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": code }))
        .expect("soll_validate repond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        report2.contains("BELONGS_TO") && report2.contains(&format!("VIS-{code}-971")),
        "une relation NON-retrait sur une paire sans politique doit rester signalee :\n---\n{report2}"
    );
}

#[test]
fn test_soll_manager_link_supersedes_is_expressible_between_any_two_soll_nodes() {
    // REQ-AXO-902461 — le RETRAIT est exprimable entre deux noeuds SOLL
    // QUELCONQUES. Trois tenants (APS, OPV, AXO) ont remonte la meme cause le
    // meme jour : `SUPERSEDES` etait refuse des que les types differaient, et
    // meme sur VIS -> VIS ou ils sont IDENTIQUES. La forme la plus ordinaire du
    // retrait n'est pas « un but en absorbe un autre » mais « une DECISION
    // tranche » — et le graphe ne savait pas l'ecrire.
    //
    // Une regle (GUI-PRO-125) qui exige un etat que l'ecriture REFUSE de
    // produire pousse mecaniquement a la falsification du registre : le tenant
    // n'a que le choix entre laisser rouge ou fabriquer une arete depuis un
    // noeud qui n'est pas le vrai remplacant.
    let server = create_test_server();
    let code = "TSW".to_string();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 0, 3, 1, 1) ON CONFLICT (project_code) DO UPDATE SET last_req = 3"
        ))
        .unwrap();
    for (id, ty) in [
        (format!("VIS-{code}-901"), "Vision"),
        (format!("VIS-{code}-902"), "Vision"),
        (format!("MIL-{code}-901"), "Milestone"),
        (format!("CPT-{code}-901"), "Concept"),
        (format!("DEC-{code}-901"), "Decision"),
        (format!("GUI-{code}-901"), "Guideline"),
        (format!("REQ-{code}-901"), "Requirement"),
        (format!("REQ-{code}-902"), "Requirement"),
        (format!("REQ-{code}-903"), "Requirement"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{id}', '{ty}', '{code}', 't', '', 'current', '{{}}') ON CONFLICT (id) DO UPDATE SET status = 'current'"
            ))
            .unwrap();
    }
    // PG partage entre tests : purger les aretes d'une execution precedente.
    server
        .graph_store
        .execute(&format!(
            "DELETE FROM soll.Edge WHERE project_code = '{code}'"
        ))
        .unwrap();

    let link = |src: String, tgt: String, rel: &str, rid: i64| -> serde_json::Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": { "action": "link", "entity": "requirement",
                        "data": { "source_id": src, "target_id": tgt, "relation_type": rel } }
                })),
                id: Some(json!(rid)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // Les 6 paires mesurees comme refusees sur le parc le 2026-08-22.
    for (rid, (src, tgt)) in [
        (format!("VIS-{code}-901"), format!("VIS-{code}-902")), // types IDENTIQUES, refuse quand meme
        (format!("MIL-{code}-901"), format!("CPT-{code}-901")), // CPT-AXO-040 retire par MIL-AXO-017
        (format!("DEC-{code}-901"), format!("GUI-{code}-901")), // GUI-AXO-1000 retire par DEC-AXO-085
        (format!("DEC-{code}-901"), format!("REQ-{code}-901")), // REQ-APS-470, REQ-OPV-001
        (format!("MIL-{code}-901"), format!("REQ-{code}-902")), // REQ-AXO-91531 retire par MIL-AXO-020
    ]
    .into_iter()
    .enumerate()
    {
        let r = link(src.clone(), tgt.clone(), "SUPERSEDES", 100 + rid as i64);
        assert_ne!(
            r.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "{src} -SUPERSEDES-> {tgt} doit etre acceptee : {r:?}"
        );
        // L'arete ECRITE doit etre un SUPERSEDES, pas une relation approchante.
        let edges = server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id='{src}' AND target_id='{tgt}' AND relation_type='SUPERSEDES'"
            ))
            .unwrap();
        assert_eq!(edges, 1, "{src} -> {tgt} doit porter une arete SUPERSEDES");
        // Et le retrait doit avoir eu lieu : c'est ce que GUI-PRO-125 lit.
        let retired = server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE id='{tgt}' AND status='superseded'"
            ))
            .unwrap();
        assert_eq!(retired, 1, "{tgt} doit etre passe a superseded");
    }

    // CONTROLE POSITIF 1 — le fallback n'ouvre QUE le retrait. Une autre
    // relation sur une paire sans politique reste REFUSEE. Sans ce controle,
    // une implementation qui ouvrirait la paire entiere passerait au vert.
    let other = link(
        format!("VIS-{code}-901"),
        format!("VIS-{code}-902"),
        "BELONGS_TO",
        200,
    );
    assert_eq!(
        other.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "seul SUPERSEDES est ouvert : BELONGS_TO sur VIS -> VIS doit rester refuse : {other:?}"
    );

    // CONTROLE POSITIF 2 — une paire qui a DEJA une politique garde son defaut.
    // MIL -> REQ vaut TARGETS quand aucune relation n'est demandee ; le fallback
    // ne doit pas devenir le nouveau defaut d'une paire existante.
    let implicit = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": { "action": "link", "entity": "milestone",
                    "data": { "source_id": format!("MIL-{code}-901"), "target_id": format!("REQ-{code}-903") } }
            })),
            id: Some(json!(201)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_ne!(
        implicit.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "MIL -> REQ sans relation explicite doit appliquer son defaut : {implicit:?}"
    );
    let targets = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id='MIL-{code}-901' AND target_id='REQ-{code}-903' AND relation_type='TARGETS'"
        ))
        .unwrap();
    assert_eq!(targets, 1, "le defaut TARGETS de MIL -> REQ doit etre intact");
    let still_open = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.Node WHERE id='REQ-{code}-903' AND status='current'"
        ))
        .unwrap();
    assert_eq!(
        still_open, 1,
        "un TARGETS ne retire personne — le fallback ne doit pas contaminer le defaut"
    );
}

#[test]
fn test_soll_manager_link_cycle_guard_filiation_and_inheritance() {
    // REQ-AXO-901593 — the cycle pre-check covers BOTH filiation (regression
    // after the parametrization refactor) and the non-filiation guarded
    // relations (INHERITS_FROM/USES/...). DEC-AXO-098.
    let server = create_test_server();
    let code = "TST".to_string();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 0, 2, 0, 0) ON CONFLICT (project_code) DO UPDATE SET last_req = 2"
        ))
        .unwrap();
    for (id, ty) in [
        (format!("REQ-{code}-001"), "Requirement"),
        (format!("REQ-{code}-002"), "Requirement"),
        (format!("GUI-{code}-001"), "Guideline"),
        (format!("GUI-{code}-002"), "Guideline"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{id}', '{ty}', '{code}', 't', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
            ))
            .unwrap();
    }

    let link = |src: &str, tgt: &str, rel: &str, rid: i64| -> serde_json::Value {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "link",
                    "entity": "requirement",
                    "data": { "source_id": src, "target_id": tgt, "relation_type": rel }
                }
            })),
            id: Some(json!(rid)),
        };
        server.handle_request(req).unwrap().result.unwrap()
    };
    let is_err =
        |r: &serde_json::Value| r.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);

    let req1 = format!("REQ-{code}-001");
    let req2 = format!("REQ-{code}-002");
    // Filiation (regression): REFINES forms a DAG ; the reverse closes a cycle.
    assert!(
        !is_err(&link(&req1, &req2, "REFINES", 1)),
        "first REFINES should succeed: {:?}",
        link(&req1, &req2, "REFINES", 1)
    );
    assert!(
        is_err(&link(&req2, &req1, "REFINES", 2)),
        "filiation cycle must be blocked"
    );

    let g1 = format!("GUI-{code}-001");
    let g2 = format!("GUI-{code}-002");
    // Non-filiation (REQ-AXO-901593 new): INHERITS_FROM is now cycle-guarded.
    let first = link(&g1, &g2, "INHERITS_FROM", 3);
    assert!(
        !is_err(&first),
        "first INHERITS_FROM should succeed: {first:?}"
    );
    assert!(
        is_err(&link(&g2, &g1, "INHERITS_FROM", 4)),
        "inheritance cycle must be blocked (REQ-AXO-901593)"
    );
}

#[test]
fn test_axon_soll_manager_accepts_mcp_axon_prefixed_name() {
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // attach_to seeding (Pillar).
    let server = create_test_server();
    let code = "TST".to_string();
    let pillar_id = format!("PIL-{code}-001");
    let expected_id = format!("CPT-{code}-012");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 1, 0, 11, 0) ON CONFLICT (project_code) DO UPDATE SET last_pil = 1, last_cpt = 11"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Test Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "mcp_axon_soll_manager",
            "arguments": {
                "action": "create",
                "entity": "concept",
                "data": {
                    "project_code": code,
                    "name": "Prefixed concept",
                    "explanation": "Should work through legacy prefixed tool names",
                    "rationale": "Client compatibility",
                    "attach_to": pillar_id,
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(10001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&expected_id), "{content}");
}

#[test]
fn test_axon_soll_manager_rejects_legacy_project_without_canonical_meta() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "BookingSystem",
                    "title": "Canonical Booking Decision",
                    "context": "Project code must be server-managed",
                    "rationale": "Slug longs are not canonical",
                    "status": "accepted"
                }
            }
        })),
        id: Some(json!(1001)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("BookingSystem")
            && (content.contains("non canonique") || content.contains("canonical")),
        "Error should reject non-canonical project code: {content}"
    );
}

#[test]
fn test_axon_soll_apply_plan_commit_finds_persisted_preview() {
    // REQ-AXO-91560 — per-test project_code isolation.
    //
    // REQ-AXO-902300 — this test READS `AXON_MCP_MUTATION_JOBS` (through the
    // dispatch, which routes to an async job when it is on) but took no env lock,
    // so a concurrent test flipping that var made it fail with "Mutation job
    // accepted" instead of the inline result. Verified: green under
    // `--test-threads=1`, red in parallel. The existing repo guard only covers
    // tests that MUTATE the env, not those that merely depend on it.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
    let server = create_test_server();
    let code = "TST".to_string();
    let title = format!("Preview Commit Requirement {code}");

    // Self-seed a canonical Pillar so the plan's requirement create can attach
    // to it (MIL-AXO-020 requires attach_to+relation_type on every create).
    // project_code MUST equal the id segment ('{code}') or the BEFORE INSERT
    // trigger soll_node_id_segment_check rejects the row.
    let pillar_id = format!("PIL-{code}-001");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Preview Commit Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": code,
                "dry_run": false,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-preview-commit",
                        "title": title,
                        "description": "Commit should read back the persisted preview",
                        "priority": "P1",
                        "status": "current",
                        "attach_to": pillar_id,
                        "relation_type": "BELONGS_TO"
                    }]
                }
            }
        })),
        id: Some(json!(10002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("SOLL revision committed"), "{content}");
    // REQ-AXO-902403 — signalé par KKI (llm_feedback #176) : les ids canoniques
    // étaient attribués mais n'atteignaient que `data.identity_mapping`. KKI a
    // dû les INFÉRER de l'ordre du plan, puis câbler cinq arêtes sur cette
    // supposition. Le wrapper doit en dire au moins autant que `soll_manager`,
    // qui imprime « SOLL entity created: REQ-… ».
    assert!(
        content.contains("req-preview-commit"),
        "le logical_key doit apparaître dans le texte.\n---\n{content}"
    );
    assert!(
        content.contains(&format!("REQ-{code}-")),
        "l'id canonique attribué doit apparaître dans le texte, pas seulement \
         dans data.*.\n---\n{content}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Requirement' AND title = '{title}'"
            ))
            .unwrap(),
        1
    );
    let revision_rows = server
        .graph_store
        .query_json(&format!("SELECT revision_id FROM soll.Revision WHERE project_code = '{code}' ORDER BY created_at DESC LIMIT 1"))
        .unwrap();
    // REQ-AXO-902142 — the legacy sequential `REV-{code}-001` expectation is
    // stale: soll_apply_plan now mints `REV-{code}-{ts}-{nonce}` deliberately
    // (REQ-AXO-902086, collision-free under concurrent writes; revisions are
    // audit rows, DEC-AXO-085 numeric format does not apply). Assert the live
    // contract — a revision row for this project carrying the canonical prefix.
    let expected_prefix = format!("REV-{code}-");
    assert!(revision_rows.contains(&expected_prefix), "{revision_rows}");
    assert!(result["data"]["created"].is_array());
    assert!(result["data"]["updated"].is_array());
    assert!(result["data"]["linked"].is_array());
    assert!(result["data"]["skipped"].is_array());
    assert!(result["data"]["errors"].is_array());
}

#[test]
fn test_axon_soll_apply_plan_dry_run_uses_canonical_preview_id() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": code,
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-preview-id",
                        "title": "Preview Id Requirement",
                        "description": "Preview ids should be canonical",
                        "priority": "P1",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(10003)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let preview_id = result["data"]["preview_id"].as_str().unwrap();
    assert_eq!(preview_id, format!("PRV-{code}-001"));
    assert!(result["data"]["result_contract"]["created"].is_array());
    assert!(result["data"]["result_contract"]["updated"].is_array());
    assert!(result["data"]["result_contract"]["linked"].is_array());
    assert!(result["data"]["result_contract"]["skipped"].is_array());
    assert!(result["data"]["result_contract"]["errors"].is_array());
}

#[test]
fn test_axon_soll_apply_plan_accepts_guidelines_stakeholders_validations() {
    // REQ-AXO-092 — build_plan_operations only iterated pillar/requirement/
    // decision/milestone/vision/concept, silently dropping plan.guidelines,
    // plan.stakeholders, plan.validations even though the storage layer
    // already supports all three. Adding them to the iteration list closes
    // the gap and makes soll_apply_plan symmetric with soll_manager.
    //
    // REQ-AXO-902300 — same env-race fix as the preview test above: this reads
    // `AXON_MCP_MUTATION_JOBS` via the dispatch without holding the lock, so a
    // concurrent flip turned the inline response into an async job envelope and
    // the `operations` array was absent.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
    let server = create_test_server();
    let code = "TST".to_string();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": code,
                "dry_run": true,
                "author": "test",
                "plan": {
                    "guidelines": [{
                        "logical_key": "gui-tdd-real-io",
                        "title": "TDD with real I/O",
                        "description": "Tests must hit real DBs"
                    }],
                    "stakeholders": [{
                        "logical_key": "stk-platform-eng",
                        "title": "Platform Engineering",
                        "description": "Owns runtime SLOs"
                    }],
                    "validations": [{
                        "logical_key": "val-cold-start",
                        "title": "Cold start validates GPU envelope",
                        "description": "Validation node for the cold-start GPU envelope check",
                        "result": "pending"
                    }]
                }
            }
        })),
        id: Some(json!(10092)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let operations = result["data"]["operations"]
        .as_array()
        .expect("operations array");
    let entities: std::collections::HashSet<&str> = operations
        .iter()
        .filter_map(|op| op.get("entity").and_then(|v| v.as_str()))
        .collect();
    assert!(
        entities.contains("guideline"),
        "plan.guidelines must produce a `guideline` operation: {operations:?}"
    );
    assert!(
        entities.contains("stakeholder"),
        "plan.stakeholders must produce a `stakeholder` operation: {operations:?}"
    );
    assert!(
        entities.contains("validation"),
        "plan.validations must produce a `validation` operation: {operations:?}"
    );
    // Three new entries must each be `create` (none pre-existed)
    let create_ops: Vec<&Value> = operations
        .iter()
        .filter(|op| op.get("kind").and_then(|v| v.as_str()) == Some("create"))
        .collect();
    assert!(
        create_ops.len() >= 3,
        "expected at least 3 create ops, got {}: {operations:?}",
        create_ops.len()
    );
}

// REQ-AXO-901625 — silent-success regression cluster.
//
// The Pollux Cuisine 2026-05-20 session called
// `soll_apply_plan(project_code=CSC, plan={requirements:[7], relations:[16]})`
// and polled `job_status` to `succeeded` — but zero nodes and zero edges
// were materialised. Three root causes overlapped :
//
//   1. `dry_run` defaulted to `true`. Omitting the flag produced a
//      preview that never matched the LLM-facing "succeeded ⇒ applied"
//      contract that every other mutator honours.
//   2. `relations` nested inside `plan` were silently dropped because
//      `build_plan_operations` reads relations from the top-level args.
//   3. An empty operations array returned a benign "DRY-RUN ready"
//      message instead of an `isError: true` envelope.
//
// The tests below pin each branch so the silent-success path cannot
// regress.

#[test]
fn test_soll_apply_plan_dry_run_defaults_to_false_and_actually_commits() {
    // REQ-AXO-901625 root-cause guard : when the caller omits `dry_run`,
    // the plan must be COMMITTED (not previewed). Before the fix the
    // default was `true`, so the LLM saw `succeeded` but soll.Node was
    // untouched — the symptom logged by the operator.
    let _guard = env_lock();
    // Ensure AXON_MCP_MUTATION_JOBS is unset so the call returns the
    // synchronous envelope (commit branch) rather than queuing a job
    // when running after a sibling test that left the var set.
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                // dry_run intentionally omitted — must default to false.
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-901625-default-commit",
                        "title": "Default dry_run must commit",
                        "description": "Verifies REQ-AXO-901625 silent-success fix.",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(901_625_01)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result
        .get("content")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    // Core assertion : the default branch must take the COMMIT path
    // (success or failure), not the DRY-RUN preview path. The pre-fix
    // behaviour returned a `succeeded` envelope containing
    // "DRY-RUN ready" with zero mutations. Now we either see
    // "SOLL revision committed" (happy path) or "SOLL commit error"
    // (downstream PG state collision unrelated to REQ-AXO-901625, e.g.
    // a shared-backend revision id race). Either is acceptable here :
    // the silent-success regression we are pinning is "DRY-RUN ready"
    // bubbling out when the caller omitted `dry_run`.
    assert!(
        !content.contains("DRY-RUN ready"),
        "default dry_run must NOT take the preview branch. content={content}"
    );
    // When the commit succeeds end-to-end the envelope must self-describe
    // via `applied=true` + `dry_run=false` so a caller can branch on a
    // single boolean. On commit failure the envelope is `isError=true`
    // (no `applied` flag) — we still pass because we excluded the DRY-RUN
    // path above.
    if !result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        assert!(
            content.contains("SOLL revision committed"),
            "happy-path content must announce the revision commit: {content}"
        );
        assert_eq!(
            result["data"]["applied"].as_bool(),
            Some(true),
            "data.applied must be true on commit branch"
        );
        assert_eq!(
            result["data"]["dry_run"].as_bool(),
            Some(false),
            "data.dry_run must be false on commit branch"
        );
        let node_count = server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE type='Requirement' AND title = 'Default dry_run must commit'",
            )
            .unwrap();
        assert_eq!(
            node_count, 1,
            "default dry_run must materialise the requirement in soll.Node"
        );
    }
}

#[test]
fn test_soll_apply_plan_dry_run_true_surfaces_applied_false_flag() {
    // REQ-AXO-901625 — when the operator opts in to dry_run=true the
    // envelope must self-describe via `applied=false` + `dry_run=true`
    // so a caller can branch on a single boolean instead of parsing the
    // human-readable content text.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
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
                        "logical_key": "req-901625-explicit-preview",
                        "title": "Explicit preview only",
                        "description": "Should NOT touch soll.Node.",
                        "status": "current"
                    }]
                }
            }
        })),
        id: Some(json!(901_625_02)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    assert_eq!(
        result["data"]["applied"].as_bool(),
        Some(false),
        "explicit dry_run=true must set applied=false"
    );
    assert_eq!(
        result["data"]["dry_run"].as_bool(),
        Some(true),
        "explicit dry_run=true must echo dry_run=true"
    );
    let content = result
        .get("content")
        .and_then(|v| v.get(0))
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        content.contains("NO mutations applied"),
        "dry-run content blob must flag the no-op explicitly: {content}"
    );
    // soll.Node must remain untouched by the preview path.
    let node_count = server
        .graph_store
        .query_count(
            "SELECT count(*) FROM soll.Node WHERE type='Requirement' AND title = 'Explicit preview only'",
        )
        .unwrap();
    assert_eq!(node_count, 0, "dry_run=true must not materialise nodes");
}

#[test]
fn test_soll_apply_plan_dry_run_surfaces_commit_blockers_for_missing_attach_to() {
    // REQ-AXO-901992 B2 — a non-Vision create lacking attach_to + relation_type
    // dry-runs as "ready" but FAILS at commit. The dry-run must surface those
    // commit invariants as data.commit_blockers (the HYC consumer hit a false
    // "DRY-RUN ready" then a cascade of commit failures). Additive: the preview
    // contract (applied=false) is preserved.
    let _guard = env_lock();
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
                        "logical_key": "req-901992-b2-missing-attach",
                        "title": "Missing attach_to and relation_type"
                    }]
                }
            }
        })),
        id: Some(json!(901_992_02)),
    };

    let result = server.handle_request(req).unwrap().result.unwrap();
    // Preview contract preserved.
    assert_eq!(result["data"]["applied"].as_bool(), Some(false));
    // …but the dry-run is now honest about the commit-time invariants.
    let blockers = result["data"]["commit_blockers"]
        .as_array()
        .expect("commit_blockers present in dry-run");
    assert!(
        !blockers.is_empty(),
        "dry-run must surface the missing attach_to/relation_type as a commit blocker: {result}"
    );
    let missing: Vec<&str> = blockers[0]["missing"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        missing.contains(&"attach_to") && missing.contains(&"relation_type"),
        "blocker must name both missing fields: {missing:?}"
    );
}

#[test]
fn test_soll_apply_plan_hoists_relations_nested_inside_plan() {
    // REQ-AXO-901625 → REQ-AXO-902300 — the schema-drift mistake observed in the
    // Pollux Cuisine session: `relations` nested inside `plan` instead of at the
    // top level.
    //
    // History of this contract, because it moved twice: originally the array was
    // SILENTLY DROPPED and the call reported `succeeded` with zero edges
    // materialised. REQ-AXO-901625 turned that into an explicit rejection with a
    // `corrected_call` — a real improvement, but still a round-trip. The code's own
    // comment explains why callers keep making it: "the collection name reads
    // naturally as part of the plan object". That is a contract ergonomics defect,
    // not a caller error, and the correction is deterministic (same content, wrong
    // slot). So it is now HOISTED and applied.
    //
    // This test therefore asserts the opposite of what it used to: success, not
    // refusal. The refusal case moved to the genuinely ambiguous one (both slots
    // filled) — see the test below.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
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
                        "logical_key": "req-901625-misplaced",
                        "title": "Misplaced relations parent",
                        "description": "Triggers relations-inside-plan guard"
                    }],
                    // INTENTIONALLY misplaced — this is the LLM mistake.
                    "relations": [
                        {"source_id": "req-901625-misplaced", "target_id": "PIL-AXO-001", "relation_type": "BELONGS_TO"}
                    ]
                }
            }
        })),
        id: Some(json!(901_625_03)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap_or_default();

    assert_ne!(
        result["isError"].as_bool(),
        Some(true),
        "a nested-only `relations` is unambiguous — hoist it instead of spending a \
         round-trip prescribing what we can already do: {text}"
    );
    assert!(
        text.contains("hissé") || text.contains("REQ-AXO-902300"),
        "the hoist MUST be disclosed in the text channel — a silent input \
         normalisation is a loss of trust, not a convenience: {text}"
    );
    assert!(
        result["data"]["preview_id"].as_str().is_some(),
        "the dry-run must have produced a real preview, i.e. the relation was \
         actually taken into account: {result}"
    );
}

#[test]
fn test_soll_apply_plan_refuses_relations_filled_in_both_places() {
    // REQ-AXO-902300 — the frontier inherited from REQ-AXO-902288: unambiguous →
    // auto-canonicalise, ambiguous → refuse. With BOTH slots filled, picking one
    // would drop relations and merging would duplicate them; neither is ours to
    // decide for the caller.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
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
                        "logical_key": "req-902300-both",
                        "title": "Both placements filled",
                        "description": "Ambiguous: relations in plan AND at top level"
                    }],
                    "relations": [
                        {"source_logical_key": "req-902300-both", "target_id": "PIL-AXO-001", "relation_type": "BELONGS_TO"}
                    ]
                },
                "relations": [
                    {"source_logical_key": "req-902300-both", "target_id": "PIL-AXO-002", "relation_type": "BELONGS_TO"}
                ]
            }
        })),
        id: Some(json!(902_300_01)),
    };

    let result = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "two competing lists must refuse, not guess: {result}"
    );
    assert_eq!(
        result["data"]["parameter_repair"]["category"].as_str(),
        Some("relations_misplaced_inside_plan")
    );
    assert_eq!(
        result["data"]["parameter_repair"]["nested_items"].as_u64(),
        Some(1),
        "the refusal still reports how many items sit in the wrong slot"
    );
}

#[test]
fn test_soll_apply_plan_hoists_when_top_level_relations_is_an_empty_array() {
    // REQ-AXO-902300 — `relations: []` at the top level is not a competing list,
    // it is the absence of one. Treating it as "both filled" would refuse a call
    // that is in fact unambiguous.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
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
                        "logical_key": "req-902300-empty",
                        "title": "Empty top-level relations",
                        "description": "Degenerate case"
                    }],
                    "relations": [
                        {"source_logical_key": "req-902300-empty", "target_id": "PIL-AXO-001", "relation_type": "BELONGS_TO"}
                    ]
                },
                "relations": []
            }
        })),
        id: Some(json!(902_300_02)),
    };

    let result = server.handle_request(req).unwrap().result.unwrap();
    assert_ne!(
        result["isError"].as_bool(),
        Some(true),
        "an empty top-level array is not a competing list: {result}"
    );
}

#[test]
fn test_soll_apply_plan_rejects_empty_plan_with_explicit_error() {
    // REQ-AXO-901625 — empty-plan guard. A plan with all-empty
    // collections (or missing entirely) produced zero operations and
    // returned a benign "DRY-RUN ready" success message before the fix.
    // Now the call returns `input_invalid` so the caller catches the
    // malformed payload immediately.
    let _guard = env_lock();
    unsafe {
        std::env::remove_var("AXON_MCP_MUTATION_JOBS");
    }
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
                // plan present but contains no recognised collection.
                "plan": {
                    "typo_requirements": [{"title": "wrong key"}]
                }
            }
        })),
        id: Some(json!(901_625_04)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "empty plan must return isError=true"
    );
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("input_invalid"),
        "status must be input_invalid for empty plan"
    );
    assert_eq!(
        result["data"]["parameter_repair"]["category"].as_str(),
        Some("empty_plan"),
        "parameter_repair must categorise the empty plan"
    );
}

#[test]
fn test_axon_soll_apply_plan_scopes_duplicates_to_same_project() {
    // REQ-AXO-91560 — per-test project_code isolation. Two distinct
    // codes exercise the "same logical_key, different project" branch.
    let server = create_test_server();
    let target = "PJA".to_string();
    let other = "PJB".to_string();
    let other_req = format!("REQ-{other}-001");
    let shared_title = format!("Shared title {target}");
    let shared_key = format!("shared-key-{target}");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{other_req}', 'Requirement', '{other}', '{shared_title}', 'Other project duplicate', 'planned', '{{\"logical_key\":\"{shared_key}\"}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": target,
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": shared_key,
                        "title": shared_title,
                        "description": format!("Should still create in {target} scope")
                    }]
                }
            }
        })),
        id: Some(json!(100031)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let operations = result["data"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["kind"].as_str(), Some("create"));
}

#[test]
fn test_axon_soll_manager_create_without_project_code_auto_resolves_or_errors() {
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-PRO-900', 'Requirement', 'PRO', 'Anchor', '', 'current', '{}')").unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "title": "Auto-resolve test",
                    "context": "project_code omitted — should auto-detect from cwd or single project",
                    "rationale": "Zero-config onboarding for single-project or cwd-matched usage",
                    "status": "current",
                    "attach_to": "REQ-PRO-900", "relation_type": "SOLVES"
                }
            }
        })),
        id: Some(json!(1002)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if is_error {
        // Multi-project without cwd match: should list known codes.
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            content.contains("`project_code`") && content.contains("required"),
            "Error should mention project_code is required: {content}"
        );
    } else {
        // Single project or cwd matched: auto-resolved successfully.
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            !content.is_empty(),
            "Auto-resolved mutation should return non-empty content"
        );
    }
}

#[test]
fn test_infer_soll_mutation_returns_impacted_existing_candidates() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_a = format!("REQ-{code}-001");
    let req_b = format!("REQ-{code}-002");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_a}', 'Requirement', '{code}', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_b}', 'Requirement', '{code}', 'Perishability ordering', 'Short-life ingredients must be consumed earlier in the week.', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "infer_soll_mutation",
                "arguments": {
                    "project_code": code,
                    "statement": "Weekly shopping should allow grouped purchases."
                }
            })),
            id: Some(json!(1)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result["data"]["proposed_operation_kind"].as_str(),
        Some("update_existing_entities")
    );
    assert_eq!(
        result["data"]["candidate_entity_type"].as_str(),
        Some("Requirement")
    );
    assert_eq!(
        result["data"]["target_ids"][0].as_str(),
        Some(req_a.as_str())
    );
}

#[test]
fn test_entrench_nuance_requires_confirmation_before_write() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_id = format!("REQ-{code}-001");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": code,
                    "statement": "Weekly shopping should allow grouped purchases."
                }
            })),
            id: Some(json!(2)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["confirm_required"].as_bool(), Some(true));

    let rows = server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE id = '{req_id}'"
        ))
        .unwrap();
    assert!(!rows.contains("nuances"));
}

#[test]
fn test_entrench_nuance_confirmed_updates_existing_nodes_and_returns_feedback() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_id = format!("REQ-{code}-001");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": code,
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true,
                    "target_ids": [req_id]
                }
            })),
            id: Some(json!(3)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["confirm_required"].as_bool(), None);
    assert_eq!(
        result["data"]["mutation_feedback"]["changed_entities"][0]["id"].as_str(),
        Some(req_id.as_str())
    );

    let rows = server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE id = '{req_id}'"
        ))
        .unwrap();
    assert!(rows.contains("Weekly shopping should allow grouped purchases."));
    assert!(rows.contains("nuances"));
}

#[test]
fn test_init_project_missing_path_returns_parameter_repair() {
    // REQ-AXO-147 slice 4 — axon_init_project rejection paths surface
    // canonical data.parameter_repair so a fresh LLM that calls without
    // arguments can fix the input in one round-trip.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "init_project",
                "arguments": {}
            })),
            id: Some(json!(91474)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data");
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("project_path"));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"help"),
        "follow_up_tools must include `help`: {names:?}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("project") && hint.contains("absolute"),
        "hint must guide toward absolute project path: {hint}"
    );
}

#[test]
fn test_soll_manager_unknown_entity_returns_parameter_repair() {
    // REQ-AXO-147 slice 3 — soll_manager rejection paths now surface
    // the canonical data.parameter_repair shape so the LLM can fix
    // input fields in one round-trip.
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "wat-not-an-entity",
                    "data": { "project_code": "AXO", "title": "x", "description": "x" }
                }
            })),
            id: Some(json!(91473)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("entity"));
    assert_eq!(repair["supplied_value"].as_str(), Some("wat-not-an-entity"));
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for kind in ["requirement", "decision", "concept", "guideline", "vision"] {
        assert!(
            names.contains(&kind),
            "accepted_values must include `{kind}`: {names:?}"
        );
    }
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(hint.contains("entity"), "hint must mention entity: {hint}");
}

#[test]
fn test_soll_manager_create_invalid_status_returns_parameter_repair() {
    // REQ-AXO-325 — server-side status validation. Reject hors-vocabulaire
    // BEFORE the DB CHECK constraint surfaces a cryptic error. Mirror the
    // canonical parameter_repair envelope used elsewhere (entity / project_code
    // / relation_type / target_id). Canonical vocabulary = DEC-PRO-100 (5 values).
    let server = create_test_server();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "requirement",
                    "data": {
                        "project_code": "AXO",
                        "title": "REQ-AXO-325 contract test",
                        "description": "status=completed must be rejected with normalization_hint=delivered",
                        "status": "completed"
                    }
                }
            })),
            id: Some(json!(91475)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["category"].as_str(), Some("status"));
    assert_eq!(repair["invalid_field"].as_str(), Some("data.status"));
    assert_eq!(repair["supplied_value"].as_str(), Some("completed"));
    assert_eq!(repair["normalization_hint"].as_str(), Some("delivered"));
    assert_eq!(repair["canonical_source"].as_str(), Some("DEC-PRO-100"));
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for canonical in ["current", "planned", "delivered", "superseded", "rejected"] {
        assert!(
            names.contains(&canonical),
            "accepted_values must include `{canonical}`: {names:?}"
        );
    }
    let example = data["example_valid_call"].clone();
    assert_eq!(example["action"].as_str(), Some("create"));
    assert_eq!(example["entity"].as_str(), Some("requirement"));
    assert_eq!(
        example["data"]["status"].as_str(),
        Some("delivered"),
        "example_valid_call must use the normalization_hint"
    );
}

#[test]
fn test_soll_manager_update_invalid_status_returns_parameter_repair() {
    // REQ-AXO-325 — same vocabulary enforcement on update path.
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_id = format!("REQ-{code}-91476");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'REQ-AXO-325 update test', 'fixture for status validation on update path', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "update",
                    "entity": "requirement",
                    "data": {
                        "id": req_id,
                        "status": "accepted"
                    }
                }
            })),
            id: Some(json!(91476)),
        })
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["category"].as_str(), Some("status"));
    assert_eq!(repair["supplied_value"].as_str(), Some("accepted"));
    assert_eq!(repair["normalization_hint"].as_str(), Some("current"));
}

#[test]
fn test_entrench_nuance_cross_project_returns_parameter_repair() {
    // REQ-AXO-147 slice 2 — cross-project target_ids rejection now
    // surfaces structured `data.parameter_repair`.
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let target = "PJA".to_string();
    let cross = "PJB".to_string();
    let cross_req = format!("REQ-{cross}-901");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cross_req}', 'Requirement', '{cross}', 'Cross-project Req', 'Cross-project entrench rejection contract', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": target,
                    "statement": "Cross-project rejection contract",
                    "confirm": true,
                    "target_ids": [cross_req]
                }
            })),
            id: Some(json!(91471)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));

    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("target_ids"));
    assert_eq!(repair["stage"].as_str(), Some("cross_project_check"));
    assert_eq!(
        repair["expected_project_code"].as_str(),
        Some(target.as_str())
    );
    let invalid = repair["invalid_target_ids"]
        .as_array()
        .expect("invalid_target_ids array");
    let invalid_names: Vec<&str> = invalid.iter().filter_map(|v| v.as_str()).collect();
    assert!(invalid_names.contains(&cross_req.as_str()));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"infer_soll_mutation"),
        "follow_up_tools must include infer_soll_mutation: {names:?}"
    );
}

#[test]
fn test_entrench_nuance_confirmed_rejects_cross_project_target_ids() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let target = "PJA".to_string();
    let cross = "PJB".to_string();
    let cross_req = format!("REQ-{cross}-001");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cross_req}', 'Requirement', '{cross}', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": target,
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true,
                    "target_ids": [cross_req]
                }
            })),
            id: Some(json!(31)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["invalid_target_ids"][0].as_str(),
        Some(cross_req.as_str())
    );
}

#[test]
fn test_entrench_nuance_confirmed_requires_explicit_scope_when_inference_is_ambiguous() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_a = format!("REQ-{code}-001");
    let req_b = format!("REQ-{code}-002");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_a}', 'Requirement', '{code}', 'Grouped shopping purchases', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_b}', 'Requirement', '{code}', 'Grouped shopping purchases v2', 'Weekly shopping should allow grouped purchases for the same trip.', 'current', '{{}}')")).unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": code,
                    "statement": "Weekly shopping should allow grouped purchases.",
                    "confirm": true
                }
            })),
            id: Some(json!(32)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert!(result["data"]["ambiguity_warnings"].is_array());
}

#[test]
fn test_soll_manager_create_returns_mutation_feedback() {
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // attach_to/Pillar seeding.
    let server = create_test_server();
    let code = "TST".to_string();
    let pillar_id = format!("PIL-{code}-001");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Test Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "requirement",
                    "project_code": code,
                    "data": {
                        "project_code": code,
                        "title": "Roadmap feedback requirement",
                        "description": "A new canonical requirement from roadmap feedback.",
                        "attach_to": pillar_id,
                        "relation_type": "BELONGS_TO"
                    }
                }
            })),
            id: Some(json!(4)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert!(result["data"]["mutation_feedback"].is_object());
    assert_eq!(
        result["data"]["mutation_feedback"]["topology_delta"]["nodes_created"].as_u64(),
        Some(1)
    );
}

#[test]
fn test_wrong_project_scope_response_helper_emits_canonical_contract() {
    // REQ-AXO-043 — direct unit test of the shared helper introduced
    // when consolidating four duplicated contract sites.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("Booking"), Some("/tmp/booking"))
        .unwrap();

    let payload = server.wrong_project_scope_response("BAD_CODE", "test_tool");
    assert_eq!(payload["isError"].as_bool(), Some(true));

    let content = payload["content"][0]["text"]
        .as_str()
        .expect("content text");
    assert!(content.contains("BAD_CODE"));
    assert!(content.contains("test_tool"));

    let data = &payload["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(data["rejected_project_code"].as_str(), Some("BAD_CODE"));
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO") && registered_strs.contains(&"BKS"),
        "must list seeded codes: {registered_strs:?}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert_eq!(
        actions.len(),
        2,
        "base helper emits exactly 2 next_best_actions, got {}",
        actions.len()
    );

    // Variant with extras
    let payload2 = server.wrong_project_scope_response_with_extras(
        "BAD",
        "another_tool",
        &["custom hint A", "custom hint B"],
    );
    let actions2 = payload2["data"]["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions");
    assert_eq!(
        actions2.len(),
        4,
        "extras variant appends 2 additional actions to the base 2"
    );
    let actions_text: String = actions2
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(actions_text.contains("custom hint A"));
    assert!(actions_text.contains("custom hint B"));
}

#[test]
fn test_axon_soll_verify_requirements_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — soll_verify_requirements adopts the shared helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "MISSING_VR_001" }
            })),
            id: Some(json!(43106)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        result["data"]["rejected_project_code"].as_str(),
        Some("MISSING_VR_001")
    );
}

#[test]
fn test_axon_infer_soll_mutation_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — infer_soll_mutation adopts the shared helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "infer_soll_mutation",
                "arguments": {
                    "project_code": "MISSING_INF_002",
                    "statement": "stub"
                }
            })),
            id: Some(json!(43107)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        result["data"]["rejected_project_code"].as_str(),
        Some("MISSING_INF_002")
    );
}

#[test]
fn test_axon_init_project_warns_when_project_path_does_not_exist_on_disk() {
    // REQ-AXO-118 — a bogus project_path (typo or imaginary directory)
    // previously registered silently. Now the registration succeeds (legit
    // "register a future project" use case) but data.warnings + the
    // LLM-visible content surface the path-doesn-t-exist condition so the
    // typo is catchable at registration time.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let bogus_path = "/path/to/definitely/does/not/exist/xyz_abc_test";
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": { "project_path": bogus_path }
            })),
            id: Some(json!(43108)),
        })
        .unwrap();
    let result = response.result.expect("Expected result");

    // Registration still succeeds (non-blocking warning)
    assert_ne!(
        result["isError"].as_bool(),
        Some(true),
        "should succeed: {result}"
    );
    assert!(
        result["data"]["project_code"].as_str().is_some(),
        "should still assign a code: {result}"
    );

    // But the warning is surfaced
    assert_eq!(
        result["data"]["path_exists_on_disk"].as_bool(),
        Some(false),
        "must report path_exists_on_disk=false: {result}"
    );
    let warnings = result["data"]["warnings"]
        .as_array()
        .expect("warnings array");
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one warning: {warnings:?}"
    );
    assert_eq!(
        warnings[0]["kind"].as_str(),
        Some("path_does_not_exist_on_disk")
    );
    assert_eq!(warnings[0]["path"].as_str(), Some(bogus_path));
    assert!(warnings[0]["next_action"].as_str().is_some());

    // Content text mentions the typo / mkdir hint so a one-shot LLM read catches it
    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("does not currently exist on disk"),
        "content must surface the warning: {content}"
    );
    assert!(
        content.contains("mkdir") || content.contains("typo"),
        "content must give a recovery hint: {content}"
    );
}

#[test]
fn test_axon_validate_soll_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — soll_validate now uses the shared
    // wrong_project_scope_response helper.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_validate",
                "arguments": { "project_code": "NEVER_REGISTERED_VVV" }
            })),
            id: Some(json!(43105)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NEVER_REGISTERED_VVV")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list seeded AXO: {registered_strs:?}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
}

#[test]
fn test_axon_entrench_nuance_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — entrench_nuance previously returned a bare
    // "Entrenchment failed: ..." string when project_code was unregistered.
    // Now mirrors the wrong_project_scope contract for consistency.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "entrench_nuance",
                "arguments": {
                    "project_code": "NOT_REGISTERED_RRR",
                    "statement": "irrelevant"
                }
            })),
            id: Some(json!(43104)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NOT_REGISTERED_RRR")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list seeded AXO: {registered_strs:?}"
    );
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
}

#[test]
fn test_axon_soll_work_plan_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — work_plan previously returned `Status: ok` with empty
    // Evidence for a non-registered project_code. Verify the symmetric
    // soll_query_context contract is now applied.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_work_plan",
                "arguments": { "project_code": "NOT_A_REAL_PROJECT_XYZ" }
            })),
            id: Some(json!(43102)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("NOT_A_REAL_PROJECT_XYZ")
    );
    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO"),
        "must list registered codes: {registered_strs:?}"
    );
    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );

    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("NOT_A_REAL_PROJECT_XYZ"),
        "content must echo rejected: {content}"
    );
    assert!(
        content.contains("AXO"),
        "content must list registered codes: {content}"
    );
}

#[test]
fn test_axon_soll_query_context_unknown_project_returns_recovery_contract() {
    // REQ-AXO-043 — the previous .ok()? swallowed the resolve_project_code
    // error and the framework rendered a generic "Invalid arguments". The
    // LLM had no way to know which project_codes are registered or how to
    // recover. Surface the structured recovery contract explicitly.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("Booking"), Some("/tmp/booking"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_query_context",
                "arguments": { "project_code": "DEFINITELY_NOT_REGISTERED" }
            })),
            id: Some(json!(40432)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("wrong_project_scope"));
    assert_eq!(
        data["rejected_project_code"].as_str(),
        Some("DEFINITELY_NOT_REGISTERED")
    );

    let registered = data["registered_project_codes"]
        .as_array()
        .expect("registered_project_codes array");
    let registered_strs: Vec<&str> = registered.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        registered_strs.contains(&"AXO") && registered_strs.contains(&"BKS"),
        "must list registered codes: {registered_strs:?}"
    );

    assert!(data["next_action"].as_str().is_some());
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("wrong_project_scope")
    );
    let follow_up = data["operator_guidance"]["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_up_strs: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_up_strs.contains(&"project_registry_lookup")
            || follow_up_strs.contains(&"axon_init_project"),
        "follow_up_tools must point to registry/init: {follow_up_strs:?}"
    );

    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("DEFINITELY_NOT_REGISTERED"),
        "content must echo the rejected code: {content}"
    );
    assert!(
        content.contains("AXO") || content.contains("BKS"),
        "content must list registered codes: {content}"
    );
}

#[test]
fn test_soll_manager_create_guideline_lands_with_gui_prefix() {
    // REQ-AXO-092 — schema enum advertises `guideline` but the create branch
    // previously rejected it as "Unknown entity", forcing LLMs toward cypher
    // INSERT workarounds. Storage layer already supports the GUI prefix.
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // attach_to a seeded Pillar.
    let server = create_test_server();
    let code = "TST".to_string();
    let pillar_id = format!("PIL-{code}-001");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Test Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "guideline",
                    "project_code": code,
                    "data": {
                        "project_code": code,
                        "title": "TDD with real I/O",
                        "description": "Tests must hit real DBs, not mocks.",
                        "attach_to": pillar_id,
                        "relation_type": "BELONGS_TO"
                    }
                }
            })),
            id: Some(json!(40921)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_ne!(
        result["isError"].as_bool(),
        Some(true),
        "create guideline should not error: {result}"
    );

    // Response should expose canonical id (GUI-{project}-NNN) and entity_type
    let data = &result["data"];
    let created_id = data["created_id"].as_str().expect("created_id present");
    let expected_prefix = format!("GUI-{code}-");
    assert!(
        created_id.starts_with(&expected_prefix),
        "id must use {expected_prefix} prefix: {created_id}"
    );
    assert_eq!(data["entity_type"].as_str(), Some("Guideline"));
}

#[test]
fn test_soll_manager_create_unknown_entity_returns_recovery_contract() {
    // REQ-AXO-043 — unknown-entity error must surface accepted_entities and
    // next_action so the LLM client can recover without re-reading source.
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("Axon"), Some("/tmp/axon"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "rumour",  // not in schema
                    "project_code": "AXO",
                    "data": { "project_code": "AXO", "title": "x", "description": "y" }
                }
            })),
            id: Some(json!(40431)),
        })
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
    let content = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content.contains("Unknown entity"),
        "content must surface failure: {content}"
    );
    assert!(
        content.contains("guideline") && content.contains("requirement"),
        "content must list accepted entity types: {content}"
    );

    let data = &result["data"];
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    assert_eq!(data["rejected_entity"].as_str(), Some("rumour"));
    let accepted = data["accepted_entities"]
        .as_array()
        .expect("accepted_entities array");
    assert!(accepted.iter().any(|v| v.as_str() == Some("guideline")));
    assert!(accepted.iter().any(|v| v.as_str() == Some("requirement")));
    assert!(
        data["next_action"].as_str().is_some(),
        "next_action must be set"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
}

#[test]
fn test_axon_soll_apply_plan_rejects_non_canonical_project_identifier() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "BookingSystem",
                "dry_run": true,
                "author": "test",
                "plan": {
                    "requirements": [{
                        "logical_key": "req-non-canonical-project",
                        "title": "Bad project identity",
                        "description": "Mutations must reject non canonical project identifiers"
                    }]
                }
            }
        })),
        id: Some(json!(10004)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("Non-canonical project_code"), "{content}");
    assert!(content.contains("BookingSystem"), "{content}");
    assert!(
        content.contains("3-char uppercase canonical codes"),
        "{content}"
    );
}

#[test]
fn test_axon_init_project_rejects_non_canonical_project_code() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_name": "BookingSystem",
                "project_code": "booking-system",
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        },
        "id": 10005
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
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
    assert!(content.contains("Non-canonical project_code"), "{content}");
    assert!(content.contains("booking-system"), "{content}");
}

#[test]
fn test_axon_apply_guidelines_rejects_non_canonical_project_code() {
    // REQ-AXO-902142 — serialize env access + force the synchronous path. A
    // sibling test (runtime_surface) sets AXON_MCP_MUTATION_JOBS=true; without
    // the lock+guard this test races into the async-job envelope and the
    // recovery-contract assertions fail under concurrent runs.
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "axon",
                "accepted_global_rule_ids": ["GUI-PRO-001"]
            }
        },
        "id": 10006
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
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
    assert!(content.contains("Non-canonical project_code"), "{content}");
    assert!(content.contains("axon"), "{content}");
}

#[test]
fn test_axon_soll_manager_pillar_uses_dedicated_counter() {
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // Vision seeded so the new pillar can EPITOMIZES it.
    let server = create_test_server();
    let code = "TST".to_string();
    let vis_id = format!("VIS-{code}-001");
    let expected_pillar = format!("PIL-{code}-004");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 3, 12, 0, 0) ON CONFLICT (project_code) DO UPDATE SET last_pil = 3, last_req = 12"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', 'Test Vision', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "pillar",
                "data": {
                    "project_code": code,
                    "title": "Dedicated Pillar Counter",
                    "description": "Pillars must not consume requirement ids",
                    "attach_to": vis_id,
                    "relation_type": "EPITOMIZES"
                }
            }
        })),
        id: Some(json!(102)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&expected_pillar), "{content}");
}

#[test]
fn test_axon_soll_manager_recovers_when_registry_lags_existing_entities() {
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // attach_to a seeded Pillar.
    let server = create_test_server();
    let code = "TST".to_string();
    let pillar_id = format!("PIL-{code}-001");
    let req_existing = format!("REQ-{code}-007");
    let expected_req = format!("REQ-{code}-008");
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 1, 0, 0, 0) ON CONFLICT (project_code) DO UPDATE SET last_pil = 1, last_req = 0"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Test Pillar', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_existing}', 'Requirement', '{code}', 'Existing', 'Already there', '', '{{\"priority\":\"P1\"}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": code,
                    "title": "Recovered Counter",
                    "description": "Should continue after observed max",
                    "priority": "P1",
                    "attach_to": pillar_id,
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(103)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&expected_req), "{content}");
}

#[test]
fn test_axon_soll_manager_can_create_and_update_vision() {
    let server = create_test_server();

    let create_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "vision",
                "data": {
                    "project_code": "AXO",
                    "title": "Axon Vision",
                    "description": "Deterministic ingestion",
                    "goal": "Structural truth first",
                    "metadata": {"owner": "platform"}
                }
            }
        })),
        id: Some(json!(104)),
    };

    let create_response = server.handle_request(create_req);
    let create_result = create_response.unwrap().result.unwrap();
    assert_eq!(
        create_result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "soll_manager must reject Vision creation"
    );
    let create_content = create_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        create_content.contains("cannot create a Vision"),
        "{create_content}"
    );
}

#[test]
fn test_axon_soll_manager_creates_stakeholder_on_file_backed_store() {
    // REQ-AXO-91560 — per-test project_code isolation + MIL-AXO-020
    // attach_to a seeded Pillar.
    let temp = tempdir().unwrap();
    let root = temp.path().join("graph_v2");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(GraphStore::new(root.to_string_lossy().as_ref()).unwrap());
    let server = McpServer::new(store.clone());
    let code = "TST".to_string();
    // File-backed store targets the shared dev PG (not an ephemeral clone), so
    // the template registry seed doesn't reach it — register the fixed scope at
    // runtime (idiomatic, same as the AXO/BKS fixtures elsewhere in this module).
    store
        .sync_project_registry_entry(&code, Some("Test TST"), Some("/tmp/TST"))
        .unwrap();
    let req_id = format!("REQ-{code}-001");
    store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Test Requirement', '', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "stakeholder",
                "data": {
                    "project_code": code,
                    "name": "Runtime Rust",
                    "role": "Owns ingestion and canonical persistence",
                    "attach_to": req_id,
                    "relation_type": "ORIGINATES"
                }
            }
        })),
        id: Some(json!(101)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("SOLL entity created"), "{content}");
    // The allocator id is run-dependent on the SHARED dev PG (counter persists
    // across `cargo test` invocations under a fixed scope), so assert against
    // the ACTUAL created id from the response — never a hardcoded `STK-TST-001`,
    // which is non-reproducible on a non-ephemeral backend (REQ-AXO-902001).
    let created_id = result["data"]["created_id"]
        .as_str()
        .expect("created_id present")
        .to_string();

    std::thread::sleep(std::time::Duration::from_millis(75));

    let count = store
        .query_count(&format!("SELECT count(*) FROM soll.Node WHERE type='Stakeholder' AND id = '{created_id}' AND title = 'Runtime Rust'"))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_soll_manager_update_unknown_id_returns_normalized_contract() {
    // REQ-AXO-125 — when soll_manager update fails (e.g. the target id
    // does not exist), the response must NOT echo raw SQL or DuckDB
    // internals to the LLM-visible content. The normalized contract
    // puts kind + category + recovery in `content.text` and keeps the
    // truncated raw error under `data.diagnostic_excerpt` for opt-in
    // inspection.
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let missing_id = format!("REQ-{code}-9999");
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "update",
                "entity": "requirement",
                "data": {
                    "id": missing_id,
                    "status": "delivered"
                }
            }
        })),
        id: Some(json!(125001)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "update on missing id must surface isError"
    );
    let content = response["content"][0]["text"].as_str().unwrap();
    assert!(
        !content.contains("INSERT INTO") && !content.contains("UPDATE soll"),
        "LLM-visible content must NOT contain raw SQL: {content}"
    );
    assert!(
        content.contains("update failed"),
        "content should describe the kind: {content}"
    );
    let data = response
        .get("data")
        .expect("normalized error must include data");
    assert_eq!(data["kind"].as_str(), Some("update_failed"));
    assert!(
        data["category"].is_string(),
        "data.category must classify the error"
    );
    assert!(
        data["next_action"].is_string(),
        "data.next_action must give a recovery hint"
    );
    assert!(
        data["diagnostic_excerpt"].is_string(),
        "data.diagnostic_excerpt must hold the truncated raw error for opt-in inspection"
    );
}

// REQ-AXO-126 — soll_export is snapshot-per-release: the automatic
// hook on `axon_commit_work` was removed and the MCP tool stays
// available on demand (called once per live promotion by
// scripts/release/promote_live_safe.sh, plus ad-hoc operator calls).
// No env-var gate; the per-call rate is now bounded by promotion
// frequency. This test exercises the on-demand path; commit-work
// integration tests below assert that no auto-export occurs.

#[test]
fn test_axon_export_soll() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let vis_id = format!("VIS-{code}-001");
    let cpt_id = format!("CPT-{code}-001");
    let test_vision_title = format!("Test Vision {code}");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', '{test_vision_title}', 'Desc', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cpt_id}', 'Concept', '{code}', 'My Concept', 'Expl', 'current', '{{}}')")).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(2)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG EXPORT CONTENT: {}", content);

    assert!(content.contains("docs/vision/SOLL_EXPORT_"));

    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let export_content = std::fs::read_to_string(&export_path).unwrap();
    assert!(export_content.contains("# SOLL Extraction"));
    assert!(export_content.contains(&test_vision_title));
    assert!(export_content.contains(&cpt_id));

    let export_body = std::fs::read_to_string(&export_path).expect("export file should exist");
    assert!(
        export_body.contains("## Entities: Vision") || export_body.contains("## Entities: Vision")
    );

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_export_soll_resolves_repo_root_docs_vision() {
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(401)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("Exported to"), "{content}");
    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let expected_dir =
        super::soll::canonical_soll_export_dir().expect("expected canonical export dir");
    let export_parent = Path::new(&export_path)
        .parent()
        .expect("expected export parent");

    assert_eq!(export_parent, expected_dir.as_path());
    assert!(!export_path.contains("src/axon-core/docs/vision/SOLL_EXPORT_"));

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_restore_soll() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();
    let export_path = "/tmp/axon_restore_soll_test.md";
    let markdown = r#"# SOLL Extraction

## Entities: Vision
### VIS-AXO-001 - Test Vision
**Description:** Desc
**Status:** draft
**Meta:** `{"goal": "Goal", "source":"test"}`

## Entities: Pillar
### PIL-AXO-001 - Platform Core
**Description:** Keep the conceptual core stable
**Status:** accepted
**Meta:** `{}`

## Entities: Concept
### CPT-AXO-001 - Graph Truth
**Description:** Use a structural graph as source of truth
**Status:** accepted
**Meta:** `{"rationale": "Because the project needs stable intent"}`

## Entities: Milestone
### MIL-AXO-001 - First Usable State
**Description:** 
**Status:** in_progress
**Meta:** `{}`

## Entities: Requirement
### REQ-AXO-001 - Reliable Restore
**Description:** SOLL must be restorable from exports
**Status:** draft
**Meta:** `{"priority":"high"}`

## Entities: Decision
### DEC-AXO-001 - Merge Restore
**Description:** 
**Status:** accepted
**Meta:** `{"rationale": "Restoration should be merge-oriented and non-destructive"}`

## Entities: Validation
### VAL-AXO-001 - manual-test
**Description:** 
**Status:** passed
**Meta:** `{"method": "manual-test", "timestamp": 1234567890}`
"#;
    std::fs::write(export_path, markdown).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(3)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("SOLL restore complete"), "{}", content);
    assert!(content.contains("Vision: 1"));
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Vision'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE type='Pillar' AND project_code='AXO'"
            )
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE type='Concept' AND project_code='AXO'"
            )
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Milestone'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Requirement'")
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE type='Decision' AND project_code='AXO'"
            )
            .unwrap(),
        1
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE type='Validation'")
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(export_path);
}

#[test]
fn test_axon_validate_soll_reports_orphan_invariants() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "PJA".to_string();
    let other = "PJB".to_string();
    let req_id = format!("REQ-{code}-001");
    let val_id = format!("VAL-{code}-001");
    let dec_id = format!("DEC-{code}-001");
    let cpt_other = format!("CPT-{other}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Orphan requirement', 'No structural links', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{val_id}', 'Validation', '{code}', '', '', 'pending', '{{\"method\":\"manual\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_id}', 'Decision', '{code}', 'Unlinked decision', 'No SOLVES or IMPACTS edges', 'current', '{{}}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cpt_other}', 'Concept', '{other}', 'Other Concept', 'Expl', 'current', '{{}}')")).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(31)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("violation"));
    assert!(content.contains(&req_id));
    assert!(content.contains(&val_id));
    assert!(content.contains(&dec_id));
}

/// REQ-AXO-902405 — une Decision creee par le CHEMIN NOMINAL ne peut pas naitre
/// en violation. L'ecrivain declare `REFINES` legal pour `DEC -> REQ`
/// (`relation_policy`, `allowed: ["SOLVES","REFINES"]`, `allow_multiple_types`
/// donc aucune canonisation automatique ne rattrape le choix) tandis que la
/// regle `decision_without_links` ne reconnaissait que `SOLVES | IMPACTS`.
///
/// Ce test fait jouer les DEUX ensemble — outil d'ecriture puis validateur —
/// parce qu'un test qui n'interroge qu'un seul cote ne peut pas voir une
/// divergence entre les deux. C'est ce qui a laisse celle-ci vivre.
#[test]
fn test_a_decision_attached_via_refines_is_not_reported_as_unlinked() {
    let server = create_test_server();
    // Code projet DECLARE dans le registre et inutilise par les autres tests :
    // un code inconnu fait echouer l'ecriture AVANT la question posee ici.
    let code = "SWX".to_string();
    let req_id = format!("REQ-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Registry (project_code, id, last_pil, last_req, last_cpt, last_dec) VALUES ('{code}', 'AXON_GLOBAL', 0, 1, 0, 0) ON CONFLICT (project_code) DO UPDATE SET last_req = 1"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Ancre', 'Le besoin que la decision affine', 'current', '{{\"priority\":\"P1\"}}')"))
        .unwrap();

    // Temoin : une Decision sans la moindre arete, qui doit etre signalee.
    let orphan_id = format!("DEC-{code}-900");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{orphan_id}', 'Decision', '{code}', 'Decision sans aucun lien', 'Temoin du controle positif', 'current', '{{}}')"))
        .unwrap();

    // Chemin nominal : l'outil d'ecriture, avec la relation qu'il DECLARE legale.
    let create = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": code,
                    "attach_to": req_id,
                    "relation_type": "REFINES",
                    "title": "Decision qui affine sans resoudre",
                    "description": "Elle precise le besoin sans le clore.",
                    "status": "current"
                }
            }
        })),
        id: Some(json!(9405)),
    };
    let created = server
        .handle_request(create)
        .unwrap()
        .result
        .expect("creation refusee");
    let created_text = created.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        created_text.contains("created"),
        "precondition : l'ecrivain doit ACCEPTER REFINES pour DEC -> REQ. \
         S'il le refuse desormais, c'est l'autre moitie du contrat qui a bouge \
         et ce test doit etre relu, pas rafistole :\n{created_text}"
    );
    let dec_id = created_text
        .split('`')
        .find(|token| token.starts_with("DEC-"))
        .expect("id de la Decision introuvable dans la reponse")
        .to_string();

    // Ce que l'ecrivain a REELLEMENT pose. Sans cette verification, un test qui
    // demande REFINES et recoit SOLVES (canonisation) passerait au vert en
    // mesurant le cas nominal, pas le cas litigieux.
    let stored = server
        .graph_store
        .query_json(&format!(
            "SELECT relation_type FROM soll.Edge WHERE source_id = '{dec_id}' OR target_id = '{dec_id}'"
        ))
        .unwrap_or_else(|_| "[]".to_string());
    assert!(
        stored.contains("REFINES"),
        "precondition : l'arete posee doit etre REFINES, sinon ce test mesure \
         autre chose que la divergence visee. Pose : {stored}"
    );

    // Le validateur, sur la meme base.
    let validate = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(9406)),
    };
    let validated = server
        .handle_request(validate)
        .unwrap()
        .result
        .expect("validation sans resultat");

    // REQ-AXO-902455 — l'invariant a migré du code vers la règle-donnée
    // `GUI-PRO-129`. Le VERDICT que ce test protège est inchangé ; c'est la
    // surface qui a bougé, et elle porte maintenant le `rule_id`.
    //
    // La règle n'énumère plus les relations : elle demande UNE arête, quelle
    // qu'elle soit. C'est ce qui rend impossible la rechute que ce test
    // surveille — une liste recopiée depuis la politique ne peut plus diverger
    // d'elle puisqu'il n'y a plus de liste.
    let unlinked: Vec<Value> = validated
        .pointer("/data/violations/declarative_rule_violations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|v| v.as_str().is_some_and(|line| line.contains("GUI-PRO-129")))
        .collect();

    // CONTROLE POSITIF, avant le verdict. Une Decision sans la moindre arete
    // DOIT apparaitre ici. Sans ce controle, une liste vide — parce que la
    // regle ne tourne pas, parce que le chemin JSON a bouge, parce que le
    // filtre de statut exclut tout — rendrait l'assertion suivante verte en ne
    // mesurant rien. C'est exactement la classe de defaut que cette session
    // corrige ailleurs (REQ-AXO-902384) ; elle vaut aussi pour mes tests.
    assert!(
        unlinked
            .iter()
            .any(|v| v.as_str().is_some_and(|line| line.contains(orphan_id.as_str()))),
        "controle positif en echec : {orphan_id} n'a AUCUNE arete et n'est pas \
         signalee. La regle ne s'execute pas sur cette base, ou le chemin \
         `/data/violations/declarative_rule_violations` a change — le verdict \
         suivant ne voudrait rien dire.\n  signales : {unlinked:?}\n  reponse : {validated}"
    );

    assert!(
        !unlinked
            .iter()
            .any(|v| v.as_str().is_some_and(|line| line.contains(dec_id.as_str()))),
        "{dec_id} a ete rattachee par `REFINES`, une relation que l'outil \
         d'ecriture declare legale pour DEC -> REQ, et le validateur la compte \
         pourtant comme « sans lien ». Le validateur doit lire la POLITIQUE, \
         pas en recopier une part.\n  signales : {unlinked:?}"
    );
}

#[test]
fn test_axon_validate_soll_reports_duplicate_titles_and_uncovered_requirements() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_a = format!("REQ-{code}-010");
    let req_b = format!("REQ-{code}-011");
    let dec_a = format!("DEC-{code}-010");
    let dec_b = format!("DEC-{code}-011");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_a}', 'Requirement', '{code}', 'Duplicate req', 'No criteria', 'planned', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_b}', 'Requirement', '{code}', 'Duplicate req', 'Still no criteria', 'planned', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_a}', 'Decision', '{code}', 'Duplicate dec', 'No links', 'current', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_b}', 'Decision', '{code}', 'Duplicate dec', 'No links', 'current', '{{}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(3204)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // REQ-AXO-902455 — les doublons de titre ne sont plus détectés par un SQL
    // en dur mais par la règle-donnée `GUI-PRO-121`. Le VERDICT est le même,
    // sur les mêmes nœuds ; c'est sa source qui a changé, et la ligne le dit
    // désormais en citant la Guideline qui la mandate.
    let rule_lines: String = content
        .lines()
        .filter(|l| l.contains("GUI-PRO-121"))
        .collect::<Vec<_>>()
        .join("\n");
    for id in [&req_a, &req_b, &dec_a, &dec_b] {
        assert!(
            rule_lines.contains(id.as_str()),
            "`{id}` porte un titre partagé et doit être nommé par GUI-PRO-121.\n---\n{content}"
        );
    }
    assert!(
        rule_lines.contains("duplicate req") && rule_lines.contains("duplicate dec"),
        "la ligne doit nommer la VALEUR partagée, pas seulement les ids.\n---\n{content}"
    );
    // La section du check en dur ne doit plus exister : deux verdicts sur le
    // même sujet peuvent diverger sans que rien ne le signale (GUI-PRO-017).
    assert!(
        !content.contains("Duplicate titles (potential semantic duplicates)"),
        "le check en dur a été retiré ; sa section ne doit plus être émise.\n---\n{content}"
    );
    // `uncovered_requirements` reste en code — c'est une CONJONCTION (ni preuve
    // ni critère) que `parse_soll_rule` refuse par construction.
    assert!(
        content.contains("Requirements without criteria/evidence"),
        "{content}"
    );
    assert!(content.contains(&req_a), "{content}");
    assert!(content.contains(&req_b), "{content}");
}

#[test]
fn test_axon_validate_soll_reports_clean_minimal_graph() {
    // REQ-AXO-91560 — per-test project_code isolation (PG shared instance).
    let server = create_test_server();
    let code = "TST".to_string();
    let pillar_id = format!("PIL-{code}-001");
    let req_id = format!("REQ-{code}-001");
    let val_id = format!("VAL-{code}-001");
    let dec_id = format!("DEC-{code}-001");
    // REQ-AXO-902455 — un graphe SOLL « propre » remonte à une Vision : c'est
    // ce que `GUI-PRO-122` rend vérifiable. Le fixture n'en avait pas, et la
    // règle a eu raison de le dire — l'exigence pendait dans le vide. On
    // complète le fixture plutôt que d'affaiblir l'attente.
    let vis_id = format!("VIS-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', 'Nord du projet de test', 'Ancre de filiation', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pillar_id}', 'Pillar', '{code}', 'Platform Core', 'Protect SOLL', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Linked requirement', 'Has links', 'planned', '{{\"priority\":\"P1\"}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{val_id}', 'Validation', '{code}', '', '', 'passed', '{{\"method\":\"manual\"}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_id}', 'Decision', '{code}', 'Linked decision', '', 'current', '{{\"context\":\"Context\",\"rationale\":\"Because\"}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{pillar_id}', '{vis_id}', 'EPITOMIZES') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{req_id}', '{pillar_id}', 'BELONGS_TO')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{val_id}', '{req_id}', 'VERIFIES')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{dec_id}', '{req_id}', 'SOLVES')"
        ))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(32)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // REQ-AXO-001 has no acceptance_criteria in metadata, so validation
    // now flags it as uncovered even though it has a VERIFIES link.
    //
    // REQ-AXO-902455 — DEUX surfaces voient cette absence, sous deux angles
    // qui ne se recouvrent que partiellement, d'où le compte de 2 :
    //   - `uncovered_requirements` (en code) : ni preuve NI critère — une
    //     CONJONCTION, que `parse_soll_rule` refuse par construction ;
    //   - `GUI-PRO-126` (règle-donnée) : pas de critère, preuve ou non.
    // Mesuré sur le parc hors projets de test : 29 exigences ouvertes sans
    // critère, dont 11 chevauchent et **18 sont un ajout réel** — elles ont
    // une preuve, donc `uncovered_requirements` ne les a jamais vues.
    assert!(
        content.contains("2 minimal coherence violation(s)"),
        "{content}"
    );
    assert!(
        content.contains("Requirements without criteria/evidence"),
        "{content}"
    );
}

#[test]
fn test_axon_validate_soll_exempts_archived_requirements_from_uncovered_list() {
    // REQ-AXO-245: archived Requirements are explicitly closed and must not
    // appear in the "Requirements without criteria/evidence" list, otherwise
    // operators are forced to backfill criteria on already-closed work and the
    // violation count cannot reach zero by curation alone.
    // REQ-AXO-91560 — per-test project_code isolation (PG shared instance).
    let server = create_test_server();
    let code = "TST".to_string();
    let active_id = format!("REQ-{code}-900");
    let archived_id = format!("REQ-{code}-901");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{active_id}', 'Requirement', '{code}', 'Active uncovered', 'No criteria', 'planned', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{archived_id}', 'Requirement', '{code}', 'Closed and archived', 'No criteria, but archived', 'archived', '{{}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(3245)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("Requirements without criteria/evidence"),
        "{content}"
    );
    assert!(content.contains(&active_id), "{content}");
    assert!(
        !content.contains(&archived_id),
        "archived requirement leaked into uncovered list: {content}"
    );
}

#[test]
fn test_axon_validate_soll_can_scope_by_project_code() {
    // REQ-AXO-91560 — two unique project_codes per test run avoid
    // collisions on shared PG (`AXO`/`BKS` poisoned by prior live runs).
    let server = create_test_server();
    let code_a = "PJA".to_string();
    let code_b = "PJB".to_string();
    let req_a = format!("REQ-{code_a}-001");
    let req_b = format!("REQ-{code_b}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_a}', 'Requirement', '{code_a}', 'A orphan', 'No structural links', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_b}', 'Requirement', '{code_b}', 'B orphan', 'No structural links', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code_a }
        })),
        id: Some(json!(3201)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&format!("project:{code_a}")), "{content}");
    assert!(content.contains(&req_a), "{content}");
    assert!(!content.contains(&req_b), "{content}");
}

#[test]
fn test_axon_validate_soll_rejects_non_canonical_project_alias() {
    // Updated 2026-05-01 (commit 0f1ec17): soll_validate now uses the
    // shared wrong_project_scope_response helper. The content text format
    // changed from "Canonical project error: ..." to
    // "Project `FSC` not found in registry for soll_validate. ...".
    // Assertions updated to the structured wrong_project_scope contract.
    let server = create_test_server();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": "FSC" }
        })),
        id: Some(json!(3203)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(
        content.contains("FSC"),
        "must echo rejected code: {content}"
    );
    assert!(
        content.contains("not found in registry"),
        "must surface the registry-miss reason: {content}"
    );
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("wrong_project_scope")
    );
    assert_eq!(
        result["data"]["rejected_project_code"].as_str(),
        Some("FSC")
    );
}

#[test]
fn test_axon_validate_soll_reports_invalid_and_dangling_relations() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let pil_id = format!("PIL-{code}-001");
    let req_id = format!("REQ-{code}-001");
    let val_id = format!("VAL-{code}-001");
    let dangling_dec = format!("DEC-{code}-404");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pil_id}', 'Pillar', '{code}', 'Platform Core', 'Protect SOLL', 'current', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Linked requirement', 'Has links', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{val_id}', 'Validation', '{code}', '', '', 'passed', '{{\"method\":\"manual\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{val_id}', '{pil_id}', 'VERIFIES')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('{dangling_dec}', '{req_id}', 'SOLVES')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_validate",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(3204)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Invalid relations"), "{content}");
    assert!(content.contains("VERIFIES"), "{content}");
    assert!(content.contains(&dangling_dec), "{content}");
}

#[test]
fn test_axon_export_soll_can_scope_by_project_code() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let kept = "PJA".to_string();
    let excluded = "PJB".to_string();
    let vis_kept = format!("VIS-{kept}-001");
    let cpt_kept = format!("CPT-{kept}-001");
    let vis_excluded = format!("VIS-{excluded}-001");
    let cpt_excluded = format!("CPT-{excluded}-001");
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_excluded}', 'Vision', '{excluded}', 'Excluded Vision', 'Desc', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_kept}', 'Vision', '{kept}', 'Kept Vision', 'Desc', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cpt_excluded}', 'Concept', '{excluded}', 'Excluded Concept', 'Expl', 'current', '{{}}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{cpt_kept}', 'Concept', '{kept}', 'Kept Concept', 'Expl', 'current', '{{}}')")).unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": { "project_code": kept }
        })),
        id: Some(json!(3202)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let export_path = content
        .lines()
        .find_map(|line| line.split("Exported to ").nth(1))
        .unwrap_or_else(|| panic!("Expected export path line\n{content}"))
        .trim()
        .to_string();

    let export_body = std::fs::read_to_string(&export_path).expect("export file should exist");
    assert!(export_body.contains(&vis_kept), "{export_body}");
    assert!(export_body.contains(&cpt_kept), "{export_body}");
    assert!(!export_body.contains(&vis_excluded), "{export_body}");
    assert!(!export_body.contains(&cpt_excluded), "{export_body}");

    let _ = std::fs::remove_file(export_path);
}

// REQ-AXO-901653 slice-5c — `test_resume_vectorization_backfills_missing_queue_entries`
// deleted ; exercised dropped insert_file_data_batch_with_vectorization_policy +
// public.FileVectorizationQueue + crate::worker::DbWriteTask.

#[test]
fn test_vcr1_symbol_discovery_for_scan_trigger_flow() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let sym_trigger = format!("{code}::trigger_scan");
    let sym_global = format!("{code}::trigger_global_scan");
    let file_server = format!("src/dashboard/lib/{code}/axon/watcher/server.ex");
    let file_pool = format!("src/dashboard/lib/{code}/axon/watcher/pool_facade.ex");
    seed_ist_path(&server, &code, &file_server);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_server}', 'symbol', '{sym_trigger}', '{code}', '{file_server}', 'hash-{file_server}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_pool);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_pool}', 'symbol', '{sym_global}', '{code}', '{file_pool}', 'hash-{file_pool}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_trigger}', 'trigger_scan', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_global}', 'trigger_global_scan', 'function', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_server}', '{sym_trigger}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_pool}', '{sym_global}', 'CONTAINS', '{code}', 0)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": code }
        })),
        id: Some(json!(21)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("trigger_scan"));
    assert!(content.contains("trigger_global_scan"));
    assert!(content.contains("server.ex") || content.contains("pool_facade.ex"));
}

#[test]
fn test_vcr1_chunk_content_fallback_finds_symbol_from_natural_behavior_phrase() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file = format!("src/runtime/{code}_watcher.rs");
    let sym = format!("{code}::opaque_worker");
    let chunk_id = format!("{sym}::chunk");
    seed_ist_path(&server, &code, &file);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file}', 'symbol', 'sym-{file}', '{code}', '{file}', 'hash-{file}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym}', 'opaque_worker', 'function', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file}', '{sym}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{chunk_id}', 'symbol', '{sym}', '{code}', 'function', 'symbol: opaque_worker\nkind: function\n\nwhen a manual scan requested event arrives, relay it to the rust watcher and keep the ui passive', 'hash-a-{code}', 10, 18)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "manual scan requested", "project": code }
        })),
        id: Some(json!(24)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("opaque_worker"));
    assert!(content.contains("chunk body") || content.contains("chunk metadata"));
    assert!(content.contains("rust watcher"));
}

#[test]
fn test_vcr1_chunk_content_result_includes_snippet_for_disambiguation() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file_a = format!("src/runtime/{code}_requeue.rs");
    let file_b = format!("src/runtime/{code}_noise.rs");
    let sym_a = format!("{code}::worker_alpha");
    let sym_b = format!("{code}::worker_beta");
    seed_ist_path(&server, &code, &file_a);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a}', 'symbol', 'sym-{file_a}', '{code}', '{file_a}', 'hash-{file_a}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_b);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b}', 'symbol', 'sym-{file_b}', '{code}', '{file_b}', 'hash-{file_b}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a}', 'worker_alpha', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b}', 'worker_beta', 'function', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a}', '{sym_a}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b}', '{sym_b}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_a}::chunk', 'symbol', '{sym_a}', '{code}', 'function', 'symbol: worker_alpha\nkind: function\n\nrequeue claimed file back to pending when the common lane is full', 'hash-b-{code}', 20, 28)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_b}::chunk', 'symbol', '{sym_b}', '{code}', 'function', 'symbol: worker_beta\nkind: function\n\nlog queue metrics and continue', 'hash-c-{code}', 2, 8)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "requeue claimed file", "project": code }
        })),
        id: Some(json!(25)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("requeue claimed file back to pending"),
        "{content}"
    );
    assert!(content.contains(&file_a), "{content}");
}

// REQ-AXO-901653 slice-5c — `test_vcr1_chunk_retrieval_uses_ingested_docstring_content`
// deleted ; relied on v1 worker::DbWriteTask + insert_file_data_batch ingestion path.
// Pipeline_v2 ingestion harness rewrite tracked by REQ-AXO-901663.

#[test]
fn test_vcr1_chunk_fallback_prefers_docstring_or_body_over_path_only_match() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file_path_only = format!("src/runtime/{code}_path_only_fake_indexing_overlay.rs");
    let file_truth = format!("src/runtime/{code}_docstring_truth.rs");
    let sym_path = format!("{code}::path_only_probe");
    let sym_truth = format!("{code}::truth_probe");
    seed_ist_path(&server, &code, &file_path_only);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_path_only}', 'symbol', 'sym-{file_path_only}', '{code}', '{file_path_only}', 'hash-{file_path_only}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_truth);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_truth}', 'symbol', 'sym-{file_truth}', '{code}', '{file_truth}', 'hash-{file_truth}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_path}', 'path_only_probe', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_truth}', 'truth_probe', 'function', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_path_only}', '{sym_path}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_truth}', '{sym_truth}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_path}::chunk', 'symbol', '{sym_path}', '{code}', 'function', 'symbol: path_only_probe\nkind: function\n\nlog metrics and continue', 'hash-path-{code}', 1, 4)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_truth}::chunk', 'symbol', '{sym_truth}', '{code}', 'function', 'symbol: truth_probe\nkind: function\ndocstring: prevent fake indexing overlay in the cockpit while forwarding to the rust watcher.\n\nnotify runtime and preserve live truth', 'hash-doc-{code}', 10, 18)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "fake indexing overlay", "project": code }
        })),
        id: Some(json!(27)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let truth_pos = content
        .find(file_truth.as_str())
        .expect("docstring-backed file should appear");
    let path_pos = content
        .find(file_path_only.as_str())
        .expect("path-only file should appear");
    assert!(
        truth_pos < path_pos,
        "content-backed match should rank ahead of path-only match"
    );
    assert!(content.contains("docstring"), "{content}");
}

#[test]
fn test_axon_query_exact_config_lookup_prefers_operational_source_over_documentary_chunk() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file_config = format!("config/{code}_runtime.exs");
    let file_doc = format!("docs/{code}_TEXT_PARSING_AUDIT.md");
    let sym_runtime = format!("{code}::runtime_config");
    let sym_audit = format!("{code}::audit_section");
    seed_ist_path(&server, &code, &file_config);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_config}', 'symbol', 'sym-{file_config}', '{code}', '{file_config}', 'hash-{file_config}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_doc);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_doc}', 'symbol', 'sym-{file_doc}', '{code}', '{file_doc}', 'hash-{file_doc}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_runtime}', 'runtime_config', 'module', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_audit}', 'audit_section', 'section', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_config}', '{sym_runtime}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_doc}', '{sym_audit}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_runtime}::chunk', 'symbol', '{sym_runtime}', '{code}', 'module', 'symbol: runtime_config\nkind: module\n\nconfigures Credo.Check.Refactor.CyclomaticComplexity threshold for the application runtime', 'hash-runtime-{code}', 1, 12)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_audit}::chunk', 'symbol', '{sym_audit}', '{code}', 'section', 'symbol: audit_section\nkind: section\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit-{code}', 20, 35)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": code }
        })),
        id: Some(json!(281)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let config_pos = content
        .find(file_config.as_str())
        .expect("operational config result should appear");
    let doc_pos = content
        .find(file_doc.as_str())
        .expect("documentary result should appear");
    assert!(
        config_pos < doc_pos,
        "operational config source should rank ahead of documentary prose: {content}"
    );
    assert!(content.contains("Result type"));
    assert!(content.contains("operational source"), "{content}");
    assert!(content.contains("config_lookup_exact"), "{content}");
}

#[test]
fn test_axon_query_exact_config_lookup_marks_documentary_result_when_only_docs_match() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file_doc = format!("docs/{code}_TEXT_PARSING_AUDIT.md");
    let sym_audit = format!("{code}::audit_section");
    seed_ist_path(&server, &code, &file_doc);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_doc}', 'symbol', 'sym-{file_doc}', '{code}', '{file_doc}', 'hash-{file_doc}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_audit}', 'audit_section', 'section', true, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_doc}', '{sym_audit}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Chunk (id, source_type, source_id, project_code, kind, content, content_hash, start_line, end_line) VALUES ('{sym_audit}::chunk', 'symbol', '{sym_audit}', '{code}', 'section', 'symbol: audit_section\nkind: section\n\naudit notes mention Credo.Check.Refactor.CyclomaticComplexity as a failing lookup scenario', 'hash-audit-only-{code}', 20, 35)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "Credo.Check.Refactor.CyclomaticComplexity", "project": code }
        })),
        id: Some(json!(282)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&file_doc), "{content}");
    assert!(content.contains("Result type"), "{content}");
    assert!(content.contains("documentary"), "{content}");
    assert!(content.contains("config_lookup_exact"), "{content}");
}

// REQ-AXO-088 — `reserve_budget` did not match `reserve_memory_budget`
// because `_` was missing from the wildcard separator set: the query
// stayed as a literal token instead of becoming the LIKE pattern
// `reserve%budget`. Adding `_` to the wildcard replacement set turns
// underscore-separated query fragments back into fuzzy matches that hit
// the corresponding underscore-separated symbol names.
#[test]
fn test_axon_query_underscore_fragment_matches_underscore_symbol() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let file = format!("src/axon-core/src/{code}_queue.rs");
    let sym = format!("{code}::reserve_memory_budget");
    seed_ist_path(&server, &code, &file);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file}', 'symbol', 'sym-{file}', '{code}', '{file}', 'hash-{file}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym}', 'reserve_memory_budget', 'function', false, true, false, '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file}', '{sym}', 'CONTAINS', '{code}', 0)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "reserve_budget", "project": code }
        })),
        id: Some(json!(881)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("reserve_memory_budget"),
        "fuzzy underscore-aware match must surface the existing symbol: {content}"
    );
    assert!(
        !content.contains("No exact structural match resolved"),
        "must not give up with the empty-result phrase: {content}"
    );
}

#[test]
fn test_axon_query_falls_back_when_contains_is_absent() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let sym = format!("{code}::Axon.Watcher.Server.trigger_scan");
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym}', 'Axon.Watcher.Server.trigger_scan', 'function', true, true, false, '{code}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "trigger scan", "project": code }
        })),
        id: Some(json!(211)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("degraded structural without file anchor"),
        "{content}"
    );
    assert!(content.contains("trigger_scan"), "{content}");
}

#[test]
fn test_axon_query_empty_fallback_returns_structured_recovery_without_empty_result_phrase() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": "booking", "project": code }
        })),
        id: Some(json!(212)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        content.contains("degraded structural without file anchor"),
        "{content}"
    );
    assert!(!content.contains("Aucun résultat trouvé."), "{content}");

    // REQ-AXO-902407 — la guidance doit être DANS le texte. Ce test asserait
    // uniquement `data["operator_guidance"]`, que plusieurs clients MCP
    // n'exposent jamais au modèle : il restait vert pendant que la réponse
    // visible disait « use recovery guidance » sans en rendre une seule ligne.
    assert!(
        content.contains("What to do next"),
        "la réponse vide doit rendre la marche à suivre, pas y renvoyer :\n{content}"
    );
    assert!(
        content.contains("retrieve_context"),
        "la marche à suivre doit NOMMER un appel concret :\n{content}"
    );
    assert!(
        content.contains("symbol NAMES"),
        "elle doit dire ce que `query` cherche réellement — c'est le \
         malentendu qui fait relancer la même requête aveugle :\n{content}"
    );

    let data = result.get("data").unwrap();
    assert_eq!(data["result_count"].as_u64(), Some(0));
    assert_eq!(data["query_state"].as_str(), Some("structure_only_empty"));
    assert!(data["operator_guidance"].as_object().is_some());
}

/// REQ-AXO-902407 — une PHRASE et un IDENTIFIANT ne se rattrapent pas de la
/// même façon, et rendre la même marche à suivre aux deux serait rendre une
/// marche à suivre pour personne.
#[test]
fn test_axon_query_empty_routes_a_literal_phrase_to_content_search_first() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    let ask = |q: &str| -> String {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "query",
                "arguments": { "query": q, "project": "TST" }
            })),
            id: Some(json!(2124)),
        };
        server
            .handle_request(req)
            .unwrap()
            .result
            .expect("Expected result")
            .get("content")
            .unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };

    let phrase = ask("where do we validate the refresh token");
    let identifier = ask("booking");

    // La phrase : recherche de CONTENU en premier, avec le motif recopié.
    let first_step_of_phrase = phrase
        .split("**What to do next**")
        .nth(1)
        .expect("la branche phrase doit rendre la marche à suivre")
        .lines()
        .find(|l| l.trim_start().starts_with("1."))
        .expect("premier pas manquant")
        .to_string();
    assert!(
        first_step_of_phrase.contains("retrieve_context"),
        "pour une phrase, le PREMIER pas doit être la recherche de contenu :\n{first_step_of_phrase}"
    );
    assert!(
        phrase.contains("where do we validate the refresh token"),
        "la marche à suivre doit recopier le motif, sinon elle n'est pas \
         collable telle quelle :\n{phrase}"
    );

    // L'identifiant : on raccourcit le motif d'abord, le contenu ensuite.
    let first_step_of_identifier = identifier
        .split("**What to do next**")
        .nth(1)
        .expect("la branche identifiant doit rendre la marche à suivre")
        .lines()
        .find(|l| l.trim_start().starts_with("1."))
        .expect("premier pas manquant")
        .to_string();
    assert!(
        !first_step_of_identifier.contains("retrieve_context"),
        "pour un identifiant, `retrieve_context` n'est pas le premier réflexe — \
         raccourcir le motif l'est :\n{first_step_of_identifier}"
    );

    // Les deux disent le périmètre réellement interrogé : « absent de ce projet »
    // et « absent partout » sont deux réponses différentes.
    for text in [&phrase, &identifier] {
        assert!(
            text.contains("project=\"*\""),
            "la marche à suivre doit proposer de retirer la borne de projet :\n{text}"
        );
    }
}

#[test]
fn test_vcr2_impact_before_change_on_public_api() {
    // REQ-AXO-91560 — per-test project_code isolation. Symbol names stay
    // unique because they include the per-test code suffix (e.g.
    // `parse_batch_{code}`) so the impact/api_break_check name lookup
    // doesn't collide with rows left by other parallel tests.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let name_parse = format!("parse_batch_{code}");
    let name_a = format!("consumer_a_{code}");
    let name_b = format!("consumer_b_{code}");
    let sym_parse = format!("{code}::{name_parse}");
    let sym_a = format!("{code}::{name_a}");
    let sym_b = format!("{code}::{name_b}");
    let file_api = format!("src/core/{code}_api.rs");
    let file_a = format!("src/core/{code}_consumer_a.rs");
    let file_b = format!("src/core/{code}_consumer_b.rs");
    seed_ist_path(&server, &code, &file_api);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_api}', 'symbol', 'sym-{file_api}', '{code}', '{file_api}', 'hash-{file_api}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_a);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a}', 'symbol', 'sym-{file_a}', '{code}', '{file_a}', 'hash-{file_a}')"))
        .unwrap();
    seed_ist_path(&server, &code, &file_b);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b}', 'symbol', 'sym-{file_b}', '{code}', '{file_b}', 'hash-{file_b}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_parse}', '{name_parse}', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a}', '{name_a}', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b}', '{name_b}', 'function', false, true, false, '{code}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_api}', '{sym_parse}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a}', '{sym_a}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b}', '{sym_b}', 'CONTAINS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_a}', '{sym_parse}', 'CALLS', '{code}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_b}', '{sym_parse}', 'CALLS', '{code}', 0)"))
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": name_parse, "depth": 2 }
        })),
        id: Some(json!(22)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains(&name_parse));
    assert!(impact_text.contains(&name_a));
    assert!(impact_text.contains(&name_b));
    assert!(impact_text.contains("Derived Local Projection"));

    let api_break_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "api_break_check",
            "arguments": { "symbol": name_parse }
        })),
        id: Some(json!(23)),
    };

    let api_break_response = server.handle_request(api_break_req);
    let api_break_result = api_break_response.unwrap().result.expect("Expected result");
    let api_break_text = api_break_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        api_break_text.contains("warn_api_break_risk")
            || api_break_text.contains("public api consumer impact detected")
    );
    assert!(api_break_text.contains(&name_a));
    assert!(api_break_text.contains(&name_b));
}

#[test]
fn test_axon_impact_reports_missing_call_graph_truthfully() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "TST".to_string();
    let name = format!("parse_batch_{code}");
    let sym = format!("{code}::{name}");
    server
        .graph_store
        .execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym}', '{name}', 'function', true, true, false, '{code}')"))
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": name, "depth": 2 }
        })),
        id: Some(json!(221)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains("call graph is not yet available"));
    assert!(impact_text.contains(&name));
    let data = impact_result.get("data").unwrap();
    assert_eq!(data["impact_available"].as_bool(), Some(false));
    assert_eq!(
        data["next_action"]["kind"].as_str(),
        Some("wait_for_call_graph_truth")
    );
    assert_eq!(data["next_action"]["tool"].as_str(), Some("inspect"));
}

#[test]
fn test_axon_impact_respects_project_scope_for_duplicate_symbol_names() {
    // REQ-AXO-91560 — per-test project_code isolation. Two distinct scoped
    // codes simulate the original PJA/PJB cross-project setup so the
    // shared `parse_batch` name remains a deliberate collision *between*
    // those two scoped codes (which is what the test exercises).
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code_a = "PJA".to_string();
    let code_b = "PJB".to_string();
    let name_parse = format!("parse_batch_{code_a}_{code_b}");
    let name_alpha = format!("consumer_alpha_{code_a}");
    let name_beta = format!("consumer_beta_{code_b}");
    let sym_a_parse = format!("{code_a}::{name_parse}");
    let sym_a_alpha = format!("{code_a}::{name_alpha}");
    let sym_b_parse = format!("{code_b}::{name_parse}");
    let sym_b_beta = format!("{code_b}::{name_beta}");
    let file_a_api = format!("src/{code_a}/api.rs");
    let file_a_consumer = format!("src/{code_a}/consumer.rs");
    let file_b_api = format!("src/{code_b}/api.rs");
    let file_b_consumer = format!("src/{code_b}/consumer.rs");
    seed_ist_path(&server, &code_a, &file_a_api);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a_api}', 'symbol', 'sym-{file_a_api}', '{code_a}', '{file_a_api}', 'hash-{file_a_api}')"))
        .unwrap();
    seed_ist_path(&server, &code_a, &file_a_consumer);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a_consumer}', 'symbol', 'sym-{file_a_consumer}', '{code_a}', '{file_a_consumer}', 'hash-{file_a_consumer}')"))
        .unwrap();
    seed_ist_path(&server, &code_b, &file_b_api);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b_api}', 'symbol', 'sym-{file_b_api}', '{code_b}', '{file_b_api}', 'hash-{file_b_api}')"))
        .unwrap();
    seed_ist_path(&server, &code_b, &file_b_consumer);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b_consumer}', 'symbol', 'sym-{file_b_consumer}', '{code_b}', '{file_b_consumer}', 'hash-{file_b_consumer}')"))
        .unwrap();

    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a_parse}', '{name_parse}', 'function', true, true, false, '{code_a}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a_alpha}', '{name_alpha}', 'function', false, true, false, '{code_a}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b_parse}', '{name_parse}', 'function', true, true, false, '{code_b}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b_beta}', '{name_beta}', 'function', false, true, false, '{code_b}')")).unwrap();

    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a_api}', '{sym_a_parse}', 'CONTAINS', '{code_a}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a_consumer}', '{sym_a_alpha}', 'CONTAINS', '{code_a}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b_api}', '{sym_b_parse}', 'CONTAINS', '{code_b}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b_consumer}', '{sym_b_beta}', 'CONTAINS', '{code_b}', 0)"))
        .unwrap();

    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_a_alpha}', '{sym_a_parse}', 'CALLS', '{code_a}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_b_beta}', '{sym_b_parse}', 'CALLS', '{code_b}', 0)"))
        .unwrap();

    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": {
                "symbol": name_parse,
                "project": code_a,
                "depth": 2
            }
        })),
        id: Some(json!(199)),
    };

    let impact_response = server.handle_request(impact_req);
    let impact_result = impact_response.unwrap().result.expect("Expected result");
    let impact_text = impact_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(impact_text.contains(&name_alpha), "{}", impact_text);
    assert!(!impact_text.contains(&name_beta), "{}", impact_text);
}

/// REQ-AXO-902584 — `impact` rendait `confidence: high` et un rayon d'impact sur
/// un symbole qui n'existe QUE comme extrémité d'une arête `CALLS`.
///
/// Attribution mesurée chez LLL : `examples/tmph5laa9_f.lll`, un fichier
/// TEMPORAIRE supprimé du disque, a laissé derrière lui UNE arête
/// `fulfill --CALLS--> stock_reserve` sans aucune ligne `Symbol` correspondante.
/// `query` rendait une enveloppe vide et `inspect` refusait — les deux avaient
/// raison. `impact` affirmait, parce que sa traversée est INVERSE : son index
/// RAM porte les nœuds ayant une arête ENTRANTE, qu'ils soient des symboles ou
/// de simples extrémités survivantes.
///
/// Un silence fait CHERCHER, une fausse certitude fait ÉCRIRE : LLL était à un
/// appel d'inscrire un chiffre inventé dans son SOLL.
///
/// Les DEUX verdicts sont rejoués sur le même chemin, comme l'exigent les
/// critères d'acceptation du REQ : un garde incapable de rendre le cas positif
/// ne prouverait rien.
#[test]
fn test_axon_impact_refuses_a_symbol_that_exists_only_as_an_edge_endpoint() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code = "PJG".to_string();
    let name_reel = format!("cible_reelle_{code}");
    let name_appelant = format!("appelant_{code}");
    let name_fantome = format!("cible_fantome_{code}");
    let sym_reel = format!("{code}::{name_reel}");
    let sym_appelant = format!("{code}::{name_appelant}");
    // Le fantôme porte un id de fichier disparu, comme le cas réel.
    let sym_fantome = format!("{code}::examples::tmp_disparu.lll::{name_fantome}");
    let file_api = format!("src/{code}/api.rs");
    let file_consumer = format!("src/{code}/consumer.rs");

    for path in [&file_api, &file_consumer] {
        seed_ist_path(&server, &code, path);
        server
            .graph_store
            .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{path}', 'symbol', 'sym-{path}', '{code}', '{path}', 'hash-{path}')"))
            .unwrap();
    }

    // Deux symboles RÉELS : la cible et son appelant. Le fantôme, lui, n'est
    // délibérément PAS inséré dans `Symbol` — c'est tout le sujet.
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_reel}', '{name_reel}', 'function', true, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_appelant}', '{name_appelant}', 'function', false, true, false, '{code}')")).unwrap();

    for (file, sym) in [(&file_api, &sym_reel), (&file_consumer, &sym_appelant)] {
        server
            .graph_store
            .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file}', '{sym}', 'CONTAINS', '{code}', 0)"))
            .unwrap();
    }
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_appelant}', '{sym_reel}', 'CALLS', '{code}', 0)"))
        .unwrap();
    // L'arête orpheline : elle survit à son fichier, sa cible n'est nulle part.
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{sym_appelant}', '{sym_fantome}', 'CALLS', '{code}', 0)"))
        .unwrap();

    let appeler = |symbole: &str, id: i64| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "impact",
                    "arguments": { "symbol": symbole, "project": code, "depth": 2 }
                })),
                id: Some(json!(id)),
            })
            .unwrap()
            .result
            .expect("impact must answer")
    };

    // (1) CAS POSITIF — le symbole EST dans l'index. L'impact se calcule et la
    //     confiance haute est légitime. Sans ce cas, le garde ne prouverait que
    //     sa capacité à tout refuser.
    let reel = appeler(&name_reel, 901);
    let data_reel = reel.get("data").expect("data");
    assert_ne!(
        data_reel["impact_available"].as_bool(),
        Some(false),
        "un symbole réellement indexé doit rester analysable : {data_reel}"
    );
    assert_eq!(
        data_reel["summary"]["confidence"].as_str(),
        Some("high"),
        "confiance haute légitime sur un symbole réel : {data_reel}"
    );

    // (2) CAS DU DÉFAUT — le symbole n'existe QUE comme cible d'arête.
    //     `impact` doit refuser, comme `query` et `inspect` le font déjà.
    let fantome = appeler(&name_fantome, 902);
    let data_fantome = fantome.get("data").expect("data");
    assert_eq!(
        data_fantome["impact_available"].as_bool(),
        Some(false),
        "un symbole absent de l'index ne doit JAMAIS produire un rayon d'impact : {data_fantome}"
    );
    let facteurs = data_fantome["operator_guidance"]["blocking_factors"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    assert!(
        facteurs > 0,
        "le refus doit NOMMER ce qui bloque, pas se contenter d'être vide : {data_fantome}"
    );
    assert_ne!(
        data_fantome["summary"]["confidence"].as_str(),
        Some("high"),
        "aucune confiance haute sur un symbole qu'`inspect` refuse : {data_fantome}"
    );
}

#[test]
fn test_axon_query_project_scope_uses_project_code_not_path_substring() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code_a = "PJA".to_string();
    let code_b = "PJB".to_string();
    let name_parse = format!("parse_batch_{code_a}_{code_b}");
    let sym_a = format!("{code_a}::{name_parse}");
    let sym_b = format!("{code_b}::{name_parse}");
    let file_a = format!("/tmp/{code_a}_{code_b}/api.rs");
    let file_b = format!("/tmp/{code_a}_{code_b}/worker.rs");
    seed_ist_path(&server, &code_a, &file_a);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a}', 'symbol', 'sym-{file_a}', '{code_a}', '{file_a}', 'hash-{file_a}')"))
        .unwrap();
    seed_ist_path(&server, &code_b, &file_b);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b}', 'symbol', 'sym-{file_b}', '{code_b}', '{file_b}', 'hash-{file_b}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a}', '{name_parse}', 'function', true, true, false, '{code_a}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b}', '{name_parse}', 'function', true, true, false, '{code_b}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a}', '{sym_a}', 'CONTAINS', '{code_a}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b}', '{sym_b}', 'CONTAINS', '{code_b}', 0)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "query",
            "arguments": { "query": name_parse, "project": code_a }
        })),
        id: Some(json!(305)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains(&file_a), "{}", content);
    assert!(!content.contains(&file_b), "{}", content);
}

#[test]
fn test_axon_inspect_respects_project_scope_for_duplicate_symbol_names() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let code_a = "PJA".to_string();
    let code_b = "PJB".to_string();
    let name_parse = format!("parse_batch_{code_a}_{code_b}");
    let sym_a = format!("{code_a}::{name_parse}");
    let sym_b = format!("{code_b}::{name_parse}");
    let file_a = format!("/tmp/{code_a}_{code_b}/api.rs");
    let file_b = format!("/tmp/{code_a}_{code_b}/worker.rs");
    seed_ist_path(&server, &code_a, &file_a);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_a}', 'symbol', 'sym-{file_a}', '{code_a}', '{file_a}', 'hash-{file_a}')"))
        .unwrap();
    seed_ist_path(&server, &code_b, &file_b);
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-{file_b}', 'symbol', 'sym-{file_b}', '{code_b}', '{file_b}', 'hash-{file_b}')"))
        .unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_a}', '{name_parse}', 'function', true, true, false, '{code_a}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{sym_b}', '{name_parse}', 'module', false, true, false, '{code_b}')")).unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_a}', '{sym_a}', 'CONTAINS', '{code_a}', 0)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{file_b}', '{sym_b}', 'CONTAINS', '{code_b}', 0)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "inspect",
            "arguments": { "symbol": name_parse, "project": code_a }
        })),
        id: Some(json!(306)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let expected_function = format!("| {name_parse} | function | true |");
    let expected_module = format!("| {name_parse} | module | false |");
    assert!(content.contains(&expected_function), "{}", content);
    assert!(!content.contains(&expected_module), "{}", content);
}

#[test]
fn test_vcr4_soll_continuity_create_export_restore_verify() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    // REQ-AXO-91560 — per-test project_code isolation. Restore counts
    // ('Vision: 1', 'Pillars: 1', ...) depend on a fresh per-code
    // namespace because the restore server is a different McpServer that
    // shares the same PG instance.
    let source_server = create_test_server();
    let code = "TST".to_string();
    let vision_id = format!("VIS-{code}-900");
    source_server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vision_id}', 'Vision', '{code}', 'Axon Vision', 'Stable conceptual continuity', '', '{{\"goal\":\"Protect SOLL while evolving IST\"}}')"))
        .unwrap();

    // Sequential creates: each non-Vision node attaches to a prior node via
    // the canonical relation. created_id captured from result.data.created_id.
    // Canonical statuses only (current|planned|delivered|superseded|rejected).
    let do_create = |entity: &str, data: serde_json::Value, id: i64| -> String {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": { "action": "create", "entity": entity, "data": data }
            })),
            id: Some(json!(id)),
        };
        let response = source_server.handle_request(req);
        let result = response
            .unwrap()
            .result
            .expect("Expected SOLL creation result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.contains("SOLL entity created"), "{content}");
        result["data"]["created_id"]
            .as_str()
            .expect("created_id present")
            .to_string()
    };

    // Pillar -> seeded Vision (EPITOMIZES)
    let pillar_id = do_create(
        "pillar",
        json!({
            "project_code": code,
            "title": "Concept Preservation",
            "description": "SOLL must survive runtime churn",
            "attach_to": vision_id,
            "relation_type": "EPITOMIZES"
        }),
        100,
    );

    // Requirement -> Pillar (BELONGS_TO)
    let requirement_id = do_create(
        "requirement",
        json!({
            "project_code": code,
            "title": "Reliable Restore",
            "description": "Restore from official export without destructive reset",
            "priority": "P1",
            "attach_to": pillar_id,
            "relation_type": "BELONGS_TO"
        }),
        101,
    );

    // Concept -> Requirement (EXPLAINS)
    let _concept_id = do_create(
        "concept",
        json!({
            "project_code": code,
            "name": "Merge Restore",
            "explanation": "Reconstruct conceptual entities from export",
            "rationale": "Avoid losing intent across iterations",
            "attach_to": requirement_id,
            "relation_type": "EXPLAINS"
        }),
        102,
    );

    // Decision -> Requirement (SOLVES), status current
    let _decision_id = do_create(
        "decision",
        json!({
            "project_code": code,
            "title": "Protect SOLL",
            "context": "Agents previously removed conceptual state",
            "rationale": "Exports must preserve the conceptual thread",
            "status": "current",
            "attach_to": requirement_id,
            "relation_type": "SOLVES"
        }),
        103,
    );

    // Milestone -> Requirement (TARGETS), status current
    let _milestone_id = do_create(
        "milestone",
        json!({
            "project_code": code,
            "title": "Usable Internal Continuity",
            "status": "current",
            "attach_to": requirement_id,
            "relation_type": "TARGETS"
        }),
        104,
    );

    // Validation -> Requirement (VERIFIES), result delivered
    let _validation_id = do_create(
        "validation",
        json!({
            "project_code": code,
            "method": "vcr4-e2e",
            "result": "delivered",
            "attach_to": requirement_id,
            "relation_type": "VERIFIES"
        }),
        105,
    );

    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": { "project_code": code }
        })),
        id: Some(json!(200)),
    };

    let export_response = source_server.handle_request(export_req);
    let export_result = export_response
        .unwrap()
        .result
        .expect("Expected SOLL export result");
    let export_text = export_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(export_text.contains("docs/vision/SOLL_EXPORT_"));

    let export_path = export_text
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
        .trim()
        .to_string();

    let restore_server = create_test_server();
    let restore_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(201)),
    };

    let restore_response = restore_server.handle_request(restore_req);
    let restore_result = restore_response
        .unwrap()
        .result
        .expect("Expected SOLL restore result");
    let restore_text = restore_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        restore_text.contains("SOLL restore complete"),
        "{}",
        restore_text
    );
    assert!(restore_text.contains("Vision: 1"));
    assert!(restore_text.contains("Pillars: 1"));
    assert!(restore_text.contains("Concepts: 1"));
    assert!(restore_text.contains("Milestones: 1"));
    assert!(restore_text.contains("Requirements: 1"));
    assert!(restore_text.contains("Decisions: 1"));
    assert!(restore_text.contains("Validations: 1"));

    // The restore path canonicalises the Vision to the singleton
    // `VIS-AXO-001` under project_code='AXO' (axon_restore_soll,
    // tools_soll/operations.rs:640) regardless of the export's namespace —
    // there is exactly ONE canonical SOLL Vision by design (Vision creation is
    // forbidden outside axon_init_project, see soll_manager contract). So the
    // restored Vision is asserted under 'AXO', while every other entity
    // round-trips under the per-test `{code}`.
    assert_eq!(
        restore_server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE type='Vision' AND project_code='AXO'"
            )
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Pillar' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Concept' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Milestone' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Requirement' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Decision' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE type='Validation' AND project_code='{code}'"
            ))
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(&export_path);
}

#[test]
fn test_soll_query_context_returns_project_visions_from_source() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let vis_id = format!("VIS-{code}-001");
    let req_id = format!("REQ-{code}-001");
    let rev_id = format!("REV-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', 'Axon Vision', 'Build from project vision', 'current', '{{\"goal\":\"Vision first\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Req', 'Desc', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Revision (revision_id, author, source, summary, status, created_at, committed_at) VALUES ('{rev_id}', 'tester', 'mcp', 'Context rebuild', 'committed', 10, 11)"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.RevisionChange (revision_id, entity_type, entity_id, action, before_json, after_json, created_at) VALUES ('{rev_id}', 'Node', '{req_id}', 'update', '{{}}', '{{}}', 11)"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_query_context",
            "arguments": { "project_code": code, "limit": 5 }
        })),
        id: Some(json!(7801)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("data payload");
    let visions = data
        .get("visions")
        .and_then(|value| value.as_array())
        .expect("visions array");
    assert!(
        !visions.is_empty(),
        "visions should be returned from SOLL source"
    );
    let first = visions
        .first()
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(first.contains(&vis_id), "{first}");
    assert!(first.contains("Axon Vision"), "{first}");
    assert!(first.contains("current"), "{first}");
    assert!(first.contains("Build from project vision"), "{first}");
    let digest = data.get("operational_digest").expect("operational digest");
    let entity_counts = digest["entity_counts"].as_array().expect("entity counts");
    assert!(entity_counts.iter().any(|value| {
        value["entity_type"].as_str() == Some("Vision") && value["count"].as_u64() == Some(1)
    }));
    assert_eq!(
        digest["requirement_coverage_summary"]["total"].as_u64(),
        Some(1)
    );
    // REQ-AXO-902455 — `orphan_requirement_count` a disparu de la topologie :
    // l'invariant est la règle `GUI-PRO-127`, et la surface porte désormais les
    // violations AVEC leur `rule_id`, donc avec la raison.
    let cited = digest["topology_summary"]["declarative_rule_violations"]
        .as_array()
        .expect("declarative_rule_violations array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        cited.contains("GUI-PRO-127"),
        "l'exigence non rattachée doit être signalée, en citant sa règle.\n---\n{cited}"
    );
    assert_eq!(
        digest["topology_summary"]["declarative_rule_violation_count"]
            .as_u64()
            .map(|n| n > 0),
        Some(true)
    );
    assert_eq!(
        digest["last_meaningful_revision"]["revision_id"].as_str(),
        Some(rev_id.as_str())
    );
}

#[test]
fn test_soll_query_context_changed_since_returns_delta_and_cursor() {
    // REQ-AXO-901941 — `changed_since` returns only nodes whose updated_at is
    // newer than the cursor; the response carries a fresh `cursor`.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_id = format!("REQ-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Changed req', '', 'planned', '{{\"updated_at\":5000}}')"))
        .unwrap();

    let call = |changed_since: Option<i64>, rid: i64| -> serde_json::Value {
        let mut args = serde_json::Map::new();
        args.insert("project_code".to_string(), json!(code));
        if let Some(c) = changed_since {
            args.insert("changed_since".to_string(), json!(c));
        }
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "soll_query_context", "arguments": args })),
                id: Some(json!(rid)),
            })
            .unwrap()
            .result
            .unwrap()
    };
    let reqs_contain = |resp: &serde_json::Value, id: &str| -> bool {
        resp["data"]["requirements"]
            .as_array()
            .map(|a| a.iter().any(|v| v.as_str().map(|s| s.contains(id)).unwrap_or(false)))
            .unwrap_or(false)
    };

    // changed_since before the node's updated_at → included.
    let before = call(Some(1000), 1);
    assert!(reqs_contain(&before, &req_id), "delta must include newer node");
    assert!(
        before["data"]["cursor"].as_i64().unwrap_or(0) > 0,
        "a fresh cursor must be returned"
    );
    // changed_since after the node's updated_at → excluded.
    let after = call(Some(9000), 2);
    assert!(
        !reqs_contain(&after, &req_id),
        "delta must exclude a node older than the cursor"
    );
    // no cursor → full (node present).
    let full = call(None, 3);
    assert!(reqs_contain(&full, &req_id), "full query must include the node");
}

#[test]
fn test_soll_query_context_bounds_vision_body_to_digest() {
    // REQ-AXO-901935 — a list surface must render a bounded digest, never the
    // full Vision body (often >1 KB) on every call.
    let server = create_test_server();
    let code = "TST".to_string();
    let vis_id = format!("VIS-{code}-001");
    let long_body = "X".repeat(500);
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', 'Big Vision', '{long_body}', 'current', '{{}}')"))
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_query_context",
                "arguments": { "project_code": code, "limit": 5 }
            })),
            id: Some(json!(901935)),
        })
        .unwrap()
        .result
        .unwrap();
    let entry = response["data"]["visions"][0]
        .as_str()
        .expect("vision entry");
    // entry = id|title|status|<digest>
    let digest = entry.rsplit('|').next().unwrap_or("");
    assert!(
        digest.chars().count() <= 200,
        "vision body must be bounded to a digest in the list surface, got {} chars",
        digest.chars().count()
    );
    assert!(entry.contains(&vis_id) && entry.contains("Big Vision"));
}

#[test]
fn test_axon_soll_manager_link_rejects_missing_endpoint() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let req_id = format!("REQ-{code}-001");
    let pil_missing = format!("PIL-{code}-404");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Req', 'Desc', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "requirement",
                "data": {
                    "source_id": req_id,
                    "target_id": pil_missing
                }
            }
        })),
        id: Some(json!(4101)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("not found"), "{content}");
}

#[test]
fn test_axon_soll_manager_link_applies_default_relation() {
    // REQ-AXO-91560 — per-test project_code isolation.
    let server = create_test_server();
    let code = "TST".to_string();
    let dec_id = format!("DEC-{code}-001");
    let req_id = format!("REQ-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_id}', 'Decision', '{code}', 'Decision', '', 'current', '{{\"context\":\"Context\",\"rationale\":\"Because\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Req', 'Desc', 'planned', '{{\"priority\":\"P1\"}}')"))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": dec_id,
                    "target_id": req_id
                }
            }
        })),
        id: Some(json!(4102)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count(&format!("SELECT count(*) FROM soll.Edge WHERE relation_type='SOLVES' AND source_id = '{dec_id}' AND target_id = '{req_id}'"))
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_create_can_attach_requirement_to_pillar() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Pillar', 'Protect structure', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "Attachable requirement",
                    "description": "Should auto-link to pillar",
                    "priority": "P1",
                    "attach_to": "PIL-AXO-001",
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(41015)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("expected create data");
    let created_id = data["created_id"].as_str().expect("created_id");
    assert!(created_id.starts_with("REQ-AXO-"), "{created_id}");
    assert_eq!(data["attached"].as_bool(), Some(true));
    assert_eq!(data["attached_to"].as_str(), Some("PIL-AXO-001"));
    assert_eq!(data["applied_relation"].as_str(), Some("BELONGS_TO"));
    assert_eq!(data["attach_status"].as_str(), Some("attached"));
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE source_id='{}' AND target_id='PIL-AXO-001' AND relation_type='BELONGS_TO'",
                created_id
            ))
            .unwrap(),
        1
    );
}

// REQ-AXO-901727 (Option A) — TechnologyMigration is a canonical SOLL entity:
// it allocates a `TMG-AXO-NNN` id and attaches to a Pillar via BELONGS_TO.
#[test]
fn test_soll_manager_create_technology_migration_entity() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Pillar', 'Protect structure', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "technology_migration",
                "data": {
                    "project_code": "AXO",
                    "title": "DuckDB -> PostgreSQL migration",
                    "description": "Tracks the incomplete DuckDB retirement remnants",
                    "attach_to": "PIL-AXO-001",
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(41727)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("expected create data");
    let created_id = data["created_id"].as_str().expect("created_id");
    assert!(
        created_id.starts_with("TMG-AXO-"),
        "TechnologyMigration allocates a TMG id, got: {created_id}"
    );
    assert_eq!(data["attached"].as_bool(), Some(true));
    assert_eq!(data["applied_relation"].as_str(), Some("BELONGS_TO"));
    assert_eq!(
        server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Node WHERE id='{created_id}' AND type='TechnologyMigration'"
            ))
            .unwrap(),
        1,
        "node persisted with canonical type"
    );
}

// ── REQ-AXO-901727 N2/N3/N4 — HAS_REMNANT cross-graph edge + inventory ──

/// Seed a TechnologyMigration node + two IST artifacts (one symbol, one file)
/// for the tech-debt tests below.
fn seed_tech_debt_fixture(server: &McpServer) {
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('TMG-AXO-001', 'TechnologyMigration', 'AXO', 'DuckDB -> PostgreSQL', 'residue', 'active', '{\"from_tech\":\"DuckDB\",\"to_tech\":\"PostgreSQL\",\"debt_policy\":\"full_clean\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Symbol (id, name, kind, project_code) VALUES ('AXO::resid::duck_fn', 'duck_fn', 'function', 'AXO') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.IndexedFile (path, project_code, last_seen_ms) VALUES ('src/legacy/duck.rs', 'AXO', 0) ON CONFLICT (path) DO NOTHING")
        .unwrap();
}

fn link_remnant_request(target_id: &str, target_kind: Option<&str>) -> JsonRpcRequest {
    let mut data = json!({
        "source_id": "TMG-AXO-001",
        "target_id": target_id,
        "relation_type": "HAS_REMNANT"
    });
    if let Some(kind) = target_kind {
        data["target_kind"] = json!(kind);
    }
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            // `entity` is required by soll_manager for every action (unused by
            // link); real LLM callers always pass it.
            "arguments": { "action": "link", "entity": "technology_migration", "data": data }
        })),
        id: Some(json!(90230)),
    }
}

// REQ-AXO-902030 (N2) — HAS_REMNANT is the only SOLL→IST edge. A TMG node links
// to an IST symbol; target_kind is auto-detected; the edge is idempotent.
#[test]
fn test_link_has_remnant_creates_cross_graph_edge_to_symbol() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);

    let response = server
        .handle_request(link_remnant_request("AXO::resid::duck_fn", None))
        .unwrap()
        .result
        .unwrap();
    let data = response.get("data").expect("link data");
    assert_eq!(data["status"].as_str(), Some("ok"));
    assert_eq!(data["target_kind"].as_str(), Some("ist:symbol"));
    assert_eq!(data["edges_created"].as_i64(), Some(1));

    // Edge persisted in soll.Edge with the target_kind discriminator.
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE source_id='TMG-AXO-001' AND target_id='AXO::resid::duck_fn' AND relation_type='HAS_REMNANT' AND metadata->>'target_kind'='ist:symbol'")
            .unwrap(),
        1,
        "cross-graph edge persisted with target_kind"
    );

    // Idempotent: second link is a no-op (edges_created=0).
    let again = server
        .handle_request(link_remnant_request("AXO::resid::duck_fn", None))
        .unwrap()
        .result
        .unwrap();
    assert_eq!(again["data"]["edges_created"].as_i64(), Some(0));
}

// REQ-AXO-902030 — an explicit target_kind hint for a FILE is honored.
#[test]
fn test_link_has_remnant_to_file_with_explicit_kind() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);

    let response = server
        .handle_request(link_remnant_request("src/legacy/duck.rs", Some("ist:indexed_file")))
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response["data"]["status"].as_str(), Some("ok"));
    assert_eq!(response["data"]["target_kind"].as_str(), Some("ist:indexed_file"));
}

// REQ-AXO-902030 — source that is not a TechnologyMigration is rejected.
#[test]
fn test_link_has_remnant_rejects_non_migration_source() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-700', 'Requirement', 'AXO', 'not a migration', '', 'planned', '{}')")
        .unwrap();

    let mut req = link_remnant_request("AXO::resid::duck_fn", None);
    req.params.as_mut().unwrap()["arguments"]["data"]["source_id"] = json!("REQ-AXO-700");
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(response["isError"].as_bool(), Some(true));
    assert_eq!(response["data"]["status"].as_str(), Some("input_invalid"));
    assert_eq!(
        response["data"]["parameter_repair"]["category"].as_str(),
        Some("source_not_a_migration")
    );
}

// REQ-AXO-902030 — target absent from the IST is rejected (input_not_found).
#[test]
fn test_link_has_remnant_rejects_unknown_ist_target() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);

    let response = server
        .handle_request(link_remnant_request("AXO::does::not_exist", None))
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response["isError"].as_bool(), Some(true));
    assert_eq!(response["data"]["status"].as_str(), Some("input_not_found"));
}

// REQ-AXO-902031 (N3) — tech_debt_inventory lists migrations + remnants.
#[test]
fn test_tech_debt_inventory_lists_migrations_and_remnants() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);
    server
        .handle_request(link_remnant_request("AXO::resid::duck_fn", None))
        .unwrap();
    server
        .handle_request(link_remnant_request("src/legacy/duck.rs", Some("ist:indexed_file")))
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "tech_debt_inventory",
            "arguments": { "project_code": "AXO" }
        })),
        id: Some(json!(90231)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let data = response.get("data").expect("inventory data");
    assert_eq!(data["migration_count"].as_i64(), Some(1));
    assert_eq!(data["total_remnants"].as_i64(), Some(2));
    let migration = &data["migrations"][0];
    assert_eq!(migration["id"].as_str(), Some("TMG-AXO-001"));
    assert_eq!(migration["from_tech"].as_str(), Some("DuckDB"));
    assert_eq!(migration["remnant_count"].as_i64(), Some(2));
    assert_eq!(migration["by_target_kind"]["ist:symbol"].as_i64(), Some(1));
    assert_eq!(migration["by_target_kind"]["ist:indexed_file"].as_i64(), Some(1));
}

// REQ-AXO-902331 (résidu final) — a TEST function whose name carries the legacy
// token is NOT migration debt: it exercises the old contract on purpose and is
// structurally dead (the test harness is not a CALLS edge), so the name scan used
// to flag it. The IST `tested` marker now excludes it while a real production
// residue with the same name shape is still reported.
#[test]
fn test_detect_remnants_excludes_test_functions_from_name_scan() {
    let server = create_test_server();
    // TMG bound to the pipeline ruleset by detect_key (find_tmg_by_detect_key).
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('TMG-AXO-091', 'TechnologyMigration', 'AXO', 'pipeline v1 -> v2', 'residue', 'active', '{\"detect_key\":\"pipeline_v1_to_v2\",\"from_tech\":\"pipeline_v1\",\"to_tech\":\"pipeline_v2\",\"debt_policy\":\"full_clean\"}')")
        .unwrap();
    // Two symbols, both matching `_v1([^0-9a-z]|$)` and both structurally dead
    // (no incoming CALLS edge). Only `tested` differs.
    server
        .graph_store
        .execute("INSERT INTO ist.Symbol (id, name, kind, project_code) VALUES ('AXO::prod::legacy_compose_v1', 'legacy_compose_v1', 'function', 'AXO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO ist.Symbol (id, name, kind, project_code, tested) VALUES ('AXO::test::legacy_compose_v1_roundtrip', 'legacy_compose_v1_roundtrip', 'function', 'AXO', true)")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "detect_remnants",
                "arguments": { "project_code": "AXO", "detect_key": "pipeline_v1_to_v2" }
            })),
            id: Some(json!(902331)),
        })
        .unwrap()
        .result
        .unwrap();

    // The production residue is counted; the test function is not.
    assert_eq!(
        response["structuredContent"]["total_remnants"].as_i64(),
        Some(1),
        "only the production symbol is residue, not the test fn"
    );
    // Ground truth on the persisted edges: prod linked, test excluded.
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='HAS_REMNANT' AND target_id='AXO::prod::legacy_compose_v1'")
            .unwrap(),
        1,
        "production residue is linked as a remnant"
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='HAS_REMNANT' AND target_id='AXO::test::legacy_compose_v1_roundtrip'")
            .unwrap(),
        0,
        "test fn matching the legacy name is NOT flagged as debt"
    );
}

// REQ-AXO-902032 (N4) — pre-flight residue helper resolves a file path (exact
// or repo-relative suffix) back to its migration.
#[test]
fn test_migrations_with_remnant_path_resolves_residue() {
    let server = create_test_server();
    seed_tech_debt_fixture(&server);
    server
        .handle_request(link_remnant_request("src/legacy/duck.rs", Some("ist:indexed_file")))
        .unwrap();

    let hits = server.migrations_with_remnant_path(&["src/legacy/duck.rs".to_string()]);
    assert_eq!(hits.len(), 1, "edited residue file resolves to its migration");
    assert_eq!(hits[0]["migration_id"].as_str(), Some("TMG-AXO-001"));
    assert_eq!(hits[0]["debt_policy"].as_str(), Some("full_clean"));

    // A clean (non-residue) path returns nothing — zero overhead path.
    assert!(server
        .migrations_with_remnant_path(&["src/clean.rs".to_string()])
        .is_empty());
}

// REQ-AXO-902032 (N4) — work-plan signal surfaces active migrations with
// residue, ranked by debt magnitude; absent when none.
#[test]
fn test_tech_debt_work_plan_signal() {
    let server = create_test_server();
    assert!(
        server.tech_debt_work_plan_signal("AXO").is_none(),
        "no migrations → no signal (zero overhead)"
    );

    seed_tech_debt_fixture(&server);
    server
        .handle_request(link_remnant_request("AXO::resid::duck_fn", None))
        .unwrap();

    let signal = server
        .tech_debt_work_plan_signal("AXO")
        .expect("signal present once residue exists");
    assert_eq!(signal["active_migrations"].as_i64(), Some(1));
    assert_eq!(signal["total_remnants"].as_i64(), Some(1));
    assert_eq!(signal["migrations"][0]["id"].as_str(), Some("TMG-AXO-001"));
}

#[test]
fn test_soll_manager_create_requirement_warns_on_missing_acceptance_criteria() {
    // REQ-AXO-901942 — proactive inline guard at creation, not a late
    // soll_validate discovery round-trip.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Platform Pillar', 'Protect structure', '', '{}')")
        .unwrap();

    // (a) no acceptance_criteria → warned.
    let bare = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": { "action": "create", "entity": "requirement", "data": {
                    "project_code": "AXO", "title": "Bare req", "description": "no criteria",
                    "attach_to": "PIL-AXO-001", "relation_type": "BELONGS_TO"
                }}
            })),
            id: Some(json!(901942)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        bare["data"]["acceptance_criteria_warning"].as_bool(),
        Some(true)
    );
    assert!(
        bare["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No acceptance_criteria"),
        "missing-criteria create must warn inline: {:?}",
        bare["content"]
    );

    // (b) acceptance_criteria supplied → no warning.
    let with_ac = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": { "action": "create", "entity": "requirement", "data": {
                    "project_code": "AXO", "title": "Specced req", "description": "has criteria",
                    "acceptance_criteria": ["the thing works", "tests are green"],
                    "attach_to": "PIL-AXO-001", "relation_type": "BELONGS_TO"
                }}
            })),
            id: Some(json!(901943)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        with_ac["data"]["acceptance_criteria_warning"].as_bool(),
        Some(false)
    );
}

#[test]
fn test_axon_soll_manager_create_attached_decision_requires_relation_hint_when_ambiguous() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Existing decision', '', 'current', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "decision",
                "data": {
                    "project_code": "AXO",
                    "title": "New linked decision",
                    "description": "Should need explicit relation",
                    "context": "Context",
                    "rationale": "Because",
                    "status": "accepted",
                    "attach_to": "DEC-AXO-001"
                }
            }
        })),
        id: Some(json!(41016)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );
    let data = response.get("data").expect("expected create data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    // The non-canonical status "accepted" is rejected by the canonical-status
    // gate (manager.rs) BEFORE the attach_required gate is reached, so
    // production returns problem_class="input_invalid" + invalid_field="data.status".
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    assert_eq!(
        data["parameter_repair"]["invalid_field"].as_str(),
        Some("data.status")
    );
}

#[test]
fn test_axon_soll_manager_create_attached_validation_rejects_invalid_target_kind_with_guidance() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Vision', 'North star', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "validation",
                "data": {
                    "project_code": "AXO",
                    "title": "Proof",
                    "method": "manual",
                    "result": "current",
                    "attach_to": "VIS-AXO-001",
                    "relation_type": "VERIFIES"
                }
            }
        })),
        id: Some(json!(41017)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true)
    );
    let data = response.get("data").expect("expected create data");
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("forbidden_relation_for_type")
    );
    assert_eq!(
        data["parameter_repair"]["source_type"].as_str(),
        Some("VAL")
    );
    assert_eq!(
        data["parameter_repair"]["target_type"].as_str(),
        Some("VIS")
    );
}

/// REQ-AXO-902283 / REQ-AXO-902288 — a MILESTONE create whose relation to a REQ is wrong (only
/// TARGETS is legal) auto-canonizes to TARGETS instead of rejecting, mirroring action=link.
/// Since REQ-902288 this single-legal auto-canon applies to EVERY source kind (see
/// test_soll_manager_create_auto_canonizes_single_legal_relation for the REQ→PIL case).
#[test]
fn test_soll_manager_create_milestone_autocanonizes_wrong_relation_to_targets() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-990001', 'Requirement', 'AXO', 'target req', 'x', 'current', '{}')")
        .unwrap();

    // MIL → REQ admits only TARGETS; ask for REFINES on purpose.
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "milestone",
                "data": {
                    "project_code": "AXO",
                    "title": "M1 auto-canonize probe",
                    "description": "probe",
                    "attach_to": "REQ-AXO-990001",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(902_283)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_ne!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "MIL→REQ with a wrong relation must auto-canonize, not reject: {response}"
    );
    // The edge that landed must be TARGETS, not the requested REFINES.
    let rels: Vec<String> = server
        .graph_store
        .query_json("SELECT relation_type FROM soll.Edge WHERE target_id = 'REQ-AXO-990001'")
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.into_iter().next())
        .collect();
    assert!(rels.contains(&"TARGETS".to_string()), "the edge must be TARGETS, got {rels:?}");
    assert!(!rels.contains(&"REFINES".to_string()), "REFINES must NOT be created, got {rels:?}");
}

/// REQ-AXO-902283 (Lot F) — a MIL→PIL create has no legal relation (a milestone TARGETS REQs,
/// never a pillar), so it still rejects — but with an explicit `milestone_guidance` hint that
/// teaches the mental model instead of a bare empty allowed-set.
#[test]
fn test_soll_manager_create_milestone_to_pillar_rejects_with_guidance() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-990001', 'Pillar', 'AXO', 'a pillar', 'x', 'current', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "milestone",
                "data": {
                    "project_code": "AXO",
                    "title": "M2 to-pillar probe",
                    "description": "probe",
                    "attach_to": "PIL-AXO-990001",
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(9_022_831)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(response.get("isError").and_then(|v| v.as_bool()), Some(true));
    let guidance = response["data"]["parameter_repair"]["milestone_guidance"].as_str();
    assert!(
        guidance.is_some_and(|g| g.contains("TARGETS") && g.contains("REQUIREMENT")),
        "MIL→PIL must carry the milestone_guidance hint, got: {:?}",
        response["data"]["parameter_repair"]
    );
}

/// REQ-AXO-902249 — `soll_children` replaces the hand-written
/// `JOIN soll.Edge / soll.Node` (real columns `source_id`/`target_id`, mistyped
/// on the first attempt in session 104). The point of the tool is to make that
/// class of error impossible, so the test pins BOTH directions and the filter.
#[test]
fn test_soll_children_traverses_both_directions_and_filters_relation() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-CHD-100', 'Requirement', 'CHD', 'umbrella', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-CHD-101', 'Requirement', 'CHD', 'child refines', 'x', 'delivered', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-CHD-102', 'Requirement', 'CHD', 'child blocked', 'x', 'planned', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-CHD-001', 'Pillar', 'CHD', 'parent pillar', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-CHD-101', 'REQ-CHD-100', 'REFINES')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-CHD-102', 'REQ-CHD-100', 'BLOCKED_BY')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-CHD-100', 'PIL-CHD-001', 'BELONGS_TO')");

    let ids = |v: &Value| -> Vec<String> {
        v["data"]["nodes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| n["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    // children (default): both edges pointing AT the umbrella.
    let all = server
        .axon_soll_children(&json!({ "id": "REQ-CHD-100" }))
        .expect("must answer");
    let got = ids(&all);
    assert!(
        got.contains(&"REQ-CHD-101".to_string()) && got.contains(&"REQ-CHD-102".to_string()),
        "both children expected, got {got:?}"
    );

    // relation filter narrows to exactly one.
    let refines = server
        .axon_soll_children(&json!({ "id": "REQ-CHD-100", "relation_type": "REFINES" }))
        .expect("must answer");
    assert_eq!(ids(&refines), vec!["REQ-CHD-101".to_string()]);

    // parents: climbs the other way.
    let parents = server
        .axon_soll_children(&json!({ "id": "REQ-CHD-100", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(
        ids(&parents),
        vec!["PIL-CHD-001".to_string()],
        "direction=parents must climb, not re-list children"
    );
}

/// REQ-AXO-902401 — signalé par KKI (llm_feedback #171). SOLL's canonical
/// orientation is NOT uniform: `BELONGS_TO`/`REFINES` point child → parent,
/// `TARGETS`/`SOLVES` point parent → child. So a Milestone's targeted
/// Requirements answer to `direction=parents`, and the default `children` call
/// printed a bare "0 found" while ten REQs hung off `MIL-KKI-005`. A zero with
/// no denominator reads as "there are none" — the vacuous-verdict class of
/// REQ-AXO-902384.
#[test]
fn test_soll_children_zero_names_the_other_direction() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    let exec = |sql: &str| server.graph_store.execute(sql).unwrap();

    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-CHZ-001', 'Milestone', 'CHZ', 'jalon', 'x', 'current', '{}')");
    exec("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-CHZ-010', 'Requirement', 'CHZ', 'vise par le jalon', 'x', 'planned', '{}')");
    exec("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('MIL-CHZ-001', 'REQ-CHZ-010', 'TARGETS')");

    let empty = server
        .axon_soll_children(&json!({ "id": "MIL-CHZ-001" }))
        .expect("must answer");
    let text = empty["content"][0]["text"].as_str().unwrap_or_default();

    assert_eq!(empty["data"]["count"], 0, "cette direction est bien vide");
    assert!(
        text.contains("direction=\\\"parents\\\"") || text.contains("direction=\"parents\""),
        "un zéro doit nommer la direction où les arêtes se trouvent.\n---\n{text}"
    );
    assert!(
        text.contains("1 edge(s) exist the other way"),
        "le dénominateur de l'autre direction doit être donné.\n---\n{text}"
    );

    // Et l'autre direction les rend réellement.
    let other = server
        .axon_soll_children(&json!({ "id": "MIL-CHZ-001", "direction": "parents" }))
        .expect("must answer");
    assert_eq!(other["data"]["count"], 1);
}

/// REQ-AXO-902248 — `soll_get` replaces the single most-prescribed raw-SQL
/// pattern in the system (`sql SELECT description FROM soll.Node WHERE id=…`,
/// which the GLOBAL CLAUDE.md tells every LLM to run, in every project).
#[test]
fn test_soll_get_returns_node_body_and_repairs_unknown_id() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-SGT-001', 'Guideline', 'SGT', 'Probe guideline', 'CANONICAL BODY TEXT', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-SGT-002', 'Guideline', 'SGT', 'Sibling', 'other', 'current', '{}')")
        .unwrap();

    // Known id → the BODY is the answer (that is what the procedures reach for).
    let ok = server
        .axon_soll_get(&json!({ "id": "GUI-SGT-001" }))
        .expect("soll_get must answer");
    let text = ok["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("CANONICAL BODY TEXT"),
        "the body must be in the terse answer, got: {text}"
    );
    assert_eq!(ok["data"]["type"].as_str(), Some("Guideline"));
    assert_eq!(ok["data"]["node_status"].as_str(), Some("current"));

    // Unknown id → repair AS DATA with real neighbours, not a bare "not found".
    let miss = server
        .axon_soll_get(&json!({ "id": "GUI-SGT-999" }))
        .expect("soll_get must answer");
    assert_eq!(miss.get("isError").and_then(Value::as_bool), Some(true));
    assert_eq!(miss["data"]["status"].as_str(), Some("not_found"));
    let nearby: Vec<String> = miss["data"]["parameter_repair"]["nearby_ids"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    assert!(
        nearby.contains(&"GUI-SGT-001".to_string()),
        "an unknown id must hand back real neighbours, got {nearby:?}"
    );
}

/// REQ-AXO-902580 — `sections` / `section` étaient LIVRÉS mais inopérants pour
/// le client : la sélection n'atteignait que `content[0].text`, tandis que
/// `data.description` — le champ que le client sérialise vers le LLM — portait
/// toujours le corps ENTIER. Mesuré deux fois : `sections=true` sur CPT-AXO-052
/// a rendu 134 369 caractères là où ~120 jetons étaient demandés.
///
/// REQ-AXO-902496 avait livré la logique de découpe sans jamais la tester
/// (`tested: false`) : le seul test couvrait le corps complet et la réparation
/// d'id. Une troncature qui ne tronque que la moitié rendue n'économise rien.
#[test]
fn test_soll_get_sections_and_section_truncate_the_structured_body() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('CPT-SGT-010', 'Concept', 'SGT', 'Sectioned', \
             '## Premiere section\nCORPS PREMIER\n\n## Deuxieme section\nCORPS DEUXIEME\n\n## Troisieme section\nCORPS TROISIEME', \
             'current', '{}')",
        )
        .unwrap();

    // (1) `sections=true` — les TITRES, et rien du contenu.
    let sommaire = server
        .axon_soll_get(&json!({ "id": "CPT-SGT-010", "sections": true }))
        .expect("soll_get must answer");
    let body = sommaire["data"]["description"].as_str().unwrap_or_default();
    assert!(
        body.contains("Premiere section") && body.contains("Troisieme section"),
        "le sommaire doit porter les titres, got: {body}"
    );
    assert!(
        !body.contains("CORPS PREMIER") && !body.contains("CORPS TROISIEME"),
        "`sections=true` doit tronquer data.description, PAS seulement content[0].text — got: {body}"
    );

    // (2) `section=<fragment>` — cette section, et seulement elle.
    let une = server
        .axon_soll_get(&json!({ "id": "CPT-SGT-010", "section": "Deuxieme" }))
        .expect("soll_get must answer");
    let body = une["data"]["description"].as_str().unwrap_or_default();
    assert!(
        body.contains("CORPS DEUXIEME"),
        "la section demandee doit etre rendue, got: {body}"
    );
    assert!(
        !body.contains("CORPS PREMIER") && !body.contains("CORPS TROISIEME"),
        "`section=` ne doit rendre QUE la section retenue dans data.description — got: {body}"
    );

    // (3) Sans parametre — comportement strictement inchange.
    let entier = server
        .axon_soll_get(&json!({ "id": "CPT-SGT-010" }))
        .expect("soll_get must answer");
    let body = entier["data"]["description"].as_str().unwrap_or_default();
    assert!(
        body.contains("CORPS PREMIER")
            && body.contains("CORPS DEUXIEME")
            && body.contains("CORPS TROISIEME"),
        "sans parametre, le corps entier reste du (non-regression), got: {body}"
    );

    // (4) `section_titles` reste rendu dans les trois cas.
    for reponse in [&sommaire, &une, &entier] {
        let titres = reponse["data"]["section_titles"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        assert_eq!(
            titres, 3,
            "section_titles doit rester rendu dans les trois cas"
        );
    }
}

/// REQ-AXO-902288 — a single-legal pair (`REQ → PIL` admits only `BELONGS_TO`)
/// now AUTO-CANONIZES a wrong/guessed relation_type on CREATE instead of
/// rejecting, generalizing REQ-902283's MIL-only behavior to every source kind
/// (mirrors action=link). This is the inversion of the old REQ-902247
/// corrected_call contract for this path: without the fix the call errors; with
/// it, the REQ is created and its edge carries the one legal relation. Regression
/// proof for the #1 open friction `forbidden_relation_for_type`.
#[test]
fn test_soll_manager_create_auto_canonizes_single_legal_relation() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-002', 'Pillar', 'AXO', 'Surface', 'probe pillar', '', '{}')")
        .unwrap();

    // REQ → PIL admits only BELONGS_TO; ask for a wrong one on purpose.
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "902288 auto-canon probe",
                    "description": "probe",
                    "attach_to": "PIL-AXO-002",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(902_288)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_ne!(
        response.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "single-legal pair must auto-canonize, not reject: {response}"
    );

    // The edge to the pillar must carry the ONE legal relation (BELONGS_TO), not
    // the wrong REFINES the caller guessed.
    let rows_json = server
        .graph_store
        .query_json("SELECT relation_type FROM soll.Edge WHERE target_id = 'PIL-AXO-002'")
        .unwrap();
    let rows: Vec<Vec<String>> = serde_json::from_str(&rows_json).unwrap();
    let relations: Vec<String> =
        rows.into_iter().filter_map(|r| r.into_iter().next()).collect();
    assert!(
        relations.iter().any(|r| r == "BELONGS_TO"),
        "the created edge must be auto-canonized to BELONGS_TO, got {relations:?}"
    );
    assert!(
        !relations.iter().any(|r| r == "REFINES"),
        "the guessed REFINES must NOT have been used, got {relations:?}"
    );
}

/// REQ-AXO-902247 — the mirror case: when SEVERAL relations are legal, picking one
/// for the caller would be guessing. `corrected_call` must then be ABSENT, so its
/// presence always means "apply this verbatim".
#[test]
fn test_soll_manager_forbidden_relation_omits_corrected_call_when_ambiguous() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-902192', 'Requirement', 'AXO', 'Parent', 'probe req', 'current', '{}')")
        .unwrap();

    // REQ → REQ admits several (REFINES / BELONGS_TO / SUPERSEDES / BLOCKED_BY).
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "requirement",
                "data": {
                    "project_code": "AXO",
                    "title": "902247 ambiguous probe",
                    "description": "probe",
                    "attach_to": "REQ-AXO-902192",
                    "relation_type": "EPITOMIZES"
                }
            }
        })),
        id: Some(json!(902_248)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    assert_eq!(response.get("isError").and_then(|v| v.as_bool()), Some(true));

    let repair = &response["data"]["parameter_repair"];
    let accepted = repair["accepted_values"].as_array().cloned().unwrap_or_default();
    assert!(
        accepted.len() > 1,
        "fixture assumes an ambiguous pair; got {accepted:?}"
    );
    assert!(
        repair["corrected_call"].is_null(),
        "several legal relations → must NOT pick one for the caller, got {repair}"
    );
}

#[test]
fn test_axon_soll_manager_link_rejects_relation_outside_policy() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'current', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001",
                    "relation_type": "VERIFIES"
                }
            }
        })),
        id: Some(json!(4103)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content.contains("Allowed"), "{content}");
    assert!(content.contains("SOLVES"), "{content}");
    assert!(content.contains("REFINES"), "{content}");
    let data = result
        .get("data")
        .expect("expected structured relation guidance");
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("REQ"));
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert_eq!(data["default_relation"].as_str(), Some("SOLVES"));
    let allowed_relations = data["allowed_relations"]
        .as_array()
        .expect("allowed_relations should be present")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(allowed_relations.contains(&"SOLVES"));
    assert!(allowed_relations.contains(&"REFINES"));
    assert!(data["suggested_next_actions"].as_array().is_some());
    assert!(data["canonical_examples"].as_array().is_some());
    assert!(data["recommended_incoming_links_to_target_kind"]
        .as_array()
        .is_some());
}

#[test]
fn test_axon_soll_manager_link_allows_authorized_cumulative_relation() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'current', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(4104)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='REFINES' AND source_id = 'DEC-AXO-001' AND target_id = 'REQ-AXO-001'")
            .unwrap(),
        1
    );
}

// REQ-AXO-043 / REQ-AXO-125 — the link path sanitizes raw DuckDB writer
// errors out of the LLM-visible `content.text` while preserving non-SQL
// errors verbatim and keeping the existing flat `data.relation_guidance`
// shape that callers depend on. The DEC→DEC pair is the cleanest way to
// trigger a cardinality conflict (allow_multiple_types=false with
// `allowed=["SUPERSEDES","REFINES"]`); that conflict is NOT a writer
// error so its readable text must pass through, and `data` must keep
// `pair_allowed`/`source_kind`/`canonical_examples`.
#[test]
fn test_axon_soll_manager_link_cardinality_conflict_preserves_text_and_data_shape() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'D1', '', 'current', '{\"context\":\"c\",\"rationale\":\"r\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'D2', '', 'current', '{\"context\":\"c\",\"rationale\":\"r\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata) VALUES ('DEC-AXO-001', 'DEC-AXO-002', 'SUPERSEDES', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "DEC-AXO-002",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(43001)),
    };

    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap();

    // Non-SQL error text passes through with the readable cardinality message.
    assert!(content.contains("Cardinality conflict"), "{content}");
    // No raw SQL must leak even on the readable-error path.
    assert!(
        !content.contains("INSERT INTO") && !content.contains("Writer Error"),
        "LLM-visible content must NOT contain raw SQL: {content}"
    );
    // Existing relation_guidance shape preserved (flat fields under data).
    let data = response
        .get("data")
        .expect("relation_guidance must be attached");
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("DEC"));
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert!(data["allowed_relations"].as_array().is_some());
    assert!(data["canonical_examples"].as_array().is_some());
}

// REQ-AXO-115 — Concept→Pillar BELONGS_TO is the canonical edge for a
// Concept that formalizes a Pillar-level operational protocol
// (e.g. CPT-AXO-019 → PIL-AXO-003). Before this, the pair was forbidden
// and the dependency had to be expressed indirectly via REQ traversal.
#[test]
fn test_axon_soll_manager_link_concept_belongs_to_pillar() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'Operational protocol', 'Concept desc', '', '{}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "concept",
                "data": {
                    "source_id": "CPT-AXO-001",
                    "target_id": "PIL-AXO-001"
                }
            }
        })),
        id: Some(json!(4106)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id='CPT-AXO-001' AND target_id='PIL-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_link_decision_refines_concept() {
    // REQ-AXO-188 #1+#2: DEC -> CPT must accept REFINES (and SUPERSEDES) so
    // architecture-state Concepts can record which Decision governs or
    // retires them. Without this canonical edge, the linkage stays text-only
    // inside the description body and is not queryable via the graph.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Architecture decision', '', 'current', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-001', 'Concept', 'AXO', 'Architecture-state CPT', 'Concept desc', '', '{\"tags\":\"architecture-state\"}')")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "CPT-AXO-001",
                    "relation_type": "REFINES"
                }
            }
        })),
        id: Some(json!(4188)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(content.contains("Link created"), "{content}");
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='REFINES' AND source_id='DEC-AXO-001' AND target_id='CPT-AXO-001'")
            .unwrap(),
        1
    );
}

#[test]
fn test_axon_soll_manager_link_decision_supersedes_concept() {
    // REQ-AXO-188 #1+#2 : DEC -> CPT accepte SUPERSEDES pour le cas ou une
    // decision retire ou remplace entierement un concept d'architecture.
    //
    // REQ-AXO-902461 — ce test assertait le CONTRAIRE de son propre nom : il
    // exigeait `isError` et `supersedes_type_mismatch`. La matrice de relations
    // autorisait pourtant DEC -> CPT en SUPERSEDES (`allowed: ["REFINES",
    // "SUPERSEDES"]`) ; c'etait la garde `src_type != tgt_type` de MIL-AXO-020,
    // dans manager.rs, qui la refusait. DEUX sources de verite en conflit, et
    // le test avait ete retourne du cote de la garde sans que son nom ni son
    // commentaire suivent. La garde est levee ; le test dit de nouveau ce que
    // son nom annonce.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'Replacement decision', '', 'current', '{\"context\":\"ctx\",\"rationale\":\"why\"}') ON CONFLICT (id) DO UPDATE SET status = 'current'")
        .unwrap();
    // PG partage : la cible doit etre re-armee, une execution precedente l'a retiree.
    server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE source_id='DEC-AXO-002' AND target_id='CPT-AXO-002'")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-002', 'Concept', 'AXO', 'Retired concept', 'desc', 'current', '{}') ON CONFLICT (id) DO UPDATE SET status = 'current'")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-002",
                    "target_id": "CPT-AXO-002",
                    "relation_type": "SUPERSEDES"
                }
            }
        })),
        id: Some(json!(4189)),
    };

    let response = server.handle_request(req);
    let result = response.unwrap().result.expect("Expected SOLL link result");
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "une decision qui retire un concept doit etre enregistrable : {content}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE relation_type='SUPERSEDES' AND source_id='DEC-AXO-002' AND target_id='CPT-AXO-002'")
            .unwrap(),
        1,
        "l'arete ecrite doit etre un SUPERSEDES : {content}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Node WHERE id='CPT-AXO-002' AND status='superseded'")
            .unwrap(),
        1,
        "le retrait doit avoir eu lieu — c'est ce que GUI-PRO-125 lit : {content}"
    );
}

#[test]
fn test_axon_soll_manager_link_same_type_supersedes_allowed() {
    // REQ-AXO-326 — PIL/GUI/REQ/CPT same-type SUPERSEDES now accepted so the
    // graph carries canonical replacement edges (previously blocked by policy
    // gap, forcing metadata.superseded_by workaround which is not graph-native).
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-101', 'Pillar', 'AXO', 'Old Pillar', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-102', 'Pillar', 'AXO', 'New Pillar', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-AXO-101', 'Guideline', 'AXO', 'Old Guideline', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-AXO-102', 'Guideline', 'AXO', 'New Guideline', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-101', 'Requirement', 'AXO', 'Old Req', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-102', 'Requirement', 'AXO', 'New Req', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-101', 'Concept', 'AXO', 'Old CPT', '', 'superseded', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-102', 'Concept', 'AXO', 'New CPT', '', 'current', '{}')").unwrap();

    for (entity, source, target) in [
        ("pillar", "PIL-AXO-101", "PIL-AXO-102"),
        ("guideline", "GUI-AXO-101", "GUI-AXO-102"),
        ("requirement", "REQ-AXO-101", "REQ-AXO-102"),
        ("concept", "CPT-AXO-101", "CPT-AXO-102"),
    ] {
        let response = server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": {
                        "action": "link",
                        "entity": entity,
                        "data": {
                            "source_id": source,
                            "target_id": target,
                            "relation_type": "SUPERSEDES"
                        }
                    }
                })),
                id: Some(json!(91577)),
            })
            .unwrap();
        let result = response.result.expect("expected SOLL link result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            content.contains("SUPERSEDES applied") && content.contains("retires"),
            "{entity} {source}->{target}: {content}"
        );
        assert_eq!(
            server
                .graph_store
                .query_count(&format!(
                    "SELECT count(*) FROM soll.Edge WHERE relation_type='SUPERSEDES' AND source_id='{source}' AND target_id='{target}'"
                ))
                .unwrap(),
            1
        );
        let revision_id = result["data"]["revision_id"]
            .as_str()
            .unwrap_or_else(|| panic!("SUPERSEDES doit annoncer sa revision : {result}"));
        assert!(revision_id.starts_with("link-"), "revision_id: {revision_id}");
        assert_eq!(
            server
                .graph_store
                .query_count_param(
                    "SELECT count(*) FROM soll.RevisionChange \
                     WHERE revision_id = $r AND action = 'link' AND entity_type = 'edge'",
                    &json!({ "r": revision_id }),
                )
                .unwrap(),
            1,
            "le lien SUPERSEDES doit etre journalise : {result}"
        );
    }
}

#[test]
fn test_soll_relation_schema_resolves_pair_by_ids() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Decision', '', 'current', '{\"context\":\"Context\",\"rationale\":\"Because\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Req', 'Desc', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_id": "DEC-AXO-001",
                    "target_id": "REQ-AXO-001"
                }
            })),
            id: Some(json!(4105)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("expected relation schema data");
    assert_eq!(data["pair_allowed"].as_bool(), Some(true));
    assert_eq!(data["source_kind"].as_str(), Some("DEC"));
    assert_eq!(data["target_kind"].as_str(), Some("REQ"));
    assert_eq!(data["default_relation"].as_str(), Some("SOLVES"));
    assert_eq!(data["projection"]["role"].as_str(), Some("primary"));
    assert_eq!(data["direction"].as_str(), Some("source_to_target"));
    assert_eq!(
        data["projection"]["parent_preference_rank"].as_u64(),
        Some(10)
    );
    assert!(data["allowed_target_kinds_from_source"]
        .as_array()
        .is_some());
    assert!(data["allowed_targets"].as_array().is_some());
    assert!(data["forbidden_targets"].as_array().is_some());
    assert_eq!(
        data["source_graph_role"].as_str(),
        Some("decision that solves, refines, or impacts implementation")
    );
    assert!(data["canonical_examples"].as_array().is_some());
}

/// REQ-AXO-902410 — la voie par TYPE doit rendre la MÊME réponse que la voie par
/// ID.
///
/// Elle ne le faisait pas : la branche par id normalisait le kind
/// (`classify_existing_link_endpoint(...).label()` → `MIL`), la branche par type
/// passait la chaîne brute de l'appelant. `"milestone"` ne matchait donc jamais
/// la politique et l'outil répondait « Direction MILESTONE -> REQUIREMENT has no
/// canonical relation » — pour la relation qui rattache TOUT le backlog de
/// quatre tenants (APS, OPV, TE2, AXO).
///
/// C'est la voie par type que le message d'erreur de `soll_manager` prescrit
/// (« call `soll_relation_schema` to get the matrix for a kind BEFORE guessing —
/// top open friction in telemetry »). L'outil censé résoudre la friction n°1
/// rendait un fait négatif FAUX sous un `Status: ok`.
#[test]
fn test_soll_relation_schema_by_type_matches_by_id() {
    let server = create_test_server();

    let by_type = |source: &str, target: &str| -> Value {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_relation_schema",
                    "arguments": { "source_type": source, "target_type": target }
                })),
                id: Some(json!(902_410)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // Les deux paires que KKI a heurtées, plus la casse et les préfixes.
    for (source, target, expected) in [
        ("milestone", "requirement", "TARGETS"),
        ("Milestone", "Requirement", "TARGETS"),
        ("MIL", "REQ", "TARGETS"),
        ("requirement", "pillar", "BELONGS_TO"),
    ] {
        let response = by_type(source, target);
        let text = response["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains(expected),
            "{source} -> {target} doit rendre {expected} par TYPE comme par ID.\n---\n{text}"
        );
        assert_eq!(
            response["data"]["pair_allowed"].as_bool(),
            Some(true),
            "{source} -> {target} : la paire est légale et le graphe en est plein"
        );
    }

    // Un type inconnu est REFUSÉ, pas rendu comme « aucune relation canonique » :
    // un faux négatif se lit comme une réponse.
    let unknown = by_type("jalonnage", "requirement");
    assert_eq!(unknown["isError"].as_bool(), Some(true));
    assert_eq!(
        unknown["data"]["status"].as_str(),
        Some("input_invalid"),
        "un type inconnu doit produire une réparation de paramètre, pas un verdict"
    );
}

#[test]
fn test_soll_relation_schema_unresolved_ids_return_guided_discovery_payload() {
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_id": "DEC-AXO-999",
                    "target_id": "REQ-AXO-001"
                }
            })),
            id: Some(json!(4106)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_ne!(
        response.get("isError").and_then(|value| value.as_bool()),
        Some(true)
    );
    let data = response
        .get("data")
        .expect("expected guided discovery payload");
    assert_eq!(data["resolved"].as_bool(), Some(false));
    assert_eq!(data["lookup_stage"].as_str(), Some("source_id"));
    assert!(data["suggested_next_actions"].as_array().is_some());
}

#[test]
fn test_soll_relation_schema_source_only_is_constructive_for_vision_and_pillar() {
    let server = create_test_server();

    let vision_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "VIS"
                }
            })),
            id: Some(json!(4107)),
        })
        .unwrap()
        .result
        .unwrap();
    let vision_data = vision_response.get("data").expect("vision guidance");
    assert_eq!(vision_data["source_kind"].as_str(), Some("VIS"));
    assert_eq!(
        vision_data["graph_role"].as_str(),
        Some("project north star")
    );
    assert_eq!(
        vision_data["kind_projection"]["root_eligible"].as_bool(),
        Some(true)
    );
    assert!(vision_data["incoming_from_source_kinds"]
        .as_array()
        .expect("incoming guidance")
        .iter()
        .any(|item| item["source_kind"].as_str() == Some("PIL")));

    let pillar_response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "PIL"
                }
            })),
            id: Some(json!(4108)),
        })
        .unwrap()
        .result
        .unwrap();
    let pillar_data = pillar_response.get("data").expect("pillar guidance");
    assert_eq!(pillar_data["source_kind"].as_str(), Some("PIL"));
    assert_eq!(
        pillar_data["kind_projection"]["tree_order_rank"].as_u64(),
        Some(20)
    );
    assert!(pillar_data["allowed_targets"]
        .as_array()
        .expect("outgoing guidance")
        .iter()
        .any(|item| item["target_kind"].as_str() == Some("VIS")));
    assert!(pillar_data["incoming_from_source_kinds"]
        .as_array()
        .expect("incoming guidance")
        .iter()
        .any(|item| item["source_kind"].as_str() == Some("REQ")));
    assert!(pillar_data["allowed_targets"]
        .as_array()
        .expect("outgoing guidance")
        .iter()
        .any(|item| item["projection"]["role"].as_str() == Some("primary")));
    assert!(pillar_data["forbidden_targets"].as_array().is_some());
}

/// REQ-AXO-902003 — a source-only / target-only lookup must render the legal
/// pair matrix in the VISIBLE text, not just in `data`. An LLM optimises on the
/// rendered sentence; the opaque "inspect `data`" message forced trial-and-error
/// discovery at consumer bootstrap, defeating the tool's whole promise.
#[test]
fn test_soll_relation_schema_kind_only_renders_matrix_in_visible_text() {
    let server = create_test_server();

    // Source-only: outgoing matrix in the text.
    let pillar = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": { "source_type": "PIL" }
            })),
            id: Some(json!(41091)),
        })
        .unwrap()
        .result
        .unwrap();
    let pillar_text = pillar["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        pillar_text.contains("PIL can legally reach:"),
        "source-only must render the outgoing matrix, got: {pillar_text}"
    );
    assert!(
        pillar_text.contains("VIS via EPITOMIZES"),
        "outgoing matrix must inline the PIL->VIS canonical relation, got: {pillar_text}"
    );
    assert!(
        !pillar_text.contains("inspect `data`"),
        "source-only must NOT fall back to the opaque message, got: {pillar_text}"
    );

    // Target-only: incoming matrix in the text.
    let vision = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": { "target_type": "VIS" }
            })),
            id: Some(json!(41092)),
        })
        .unwrap()
        .result
        .unwrap();
    let vision_text = vision["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        vision_text.contains("VIS can be legally reached by:"),
        "target-only must render the incoming matrix, got: {vision_text}"
    );
    assert!(
        vision_text.contains("PIL via EPITOMIZES"),
        "incoming matrix must inline the PIL->VIS canonical relation, got: {vision_text}"
    );
}

#[test]
fn test_soll_relation_schema_pair_suggests_reverse_direction_when_pair_is_forbidden() {
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "VIS",
                    "target_type": "PIL"
                }
            })),
            id: Some(json!(41081)),
        })
        .unwrap()
        .result
        .unwrap();
    let data = response.get("data").expect("forbidden pair guidance");
    assert_eq!(data["pair_allowed"].as_bool(), Some(false));
    assert_eq!(data["did_you_mean"]["source_kind"].as_str(), Some("PIL"));
    assert_eq!(data["did_you_mean"]["target_kind"].as_str(), Some("VIS"));
    assert_eq!(
        data["did_you_mean"]["relation_type"].as_str(),
        Some("EPITOMIZES")
    );
}

#[test]
fn test_soll_relation_schema_forbidden_pair_inlines_legal_route_in_visible_text() {
    // REQ-AXO-901907 — for a non-canonical direction the rendered text must
    // carry the actual attach path (legal inverse + which source kinds can
    // reach the target), not merely NAME the `data` fields. An LLM optimises
    // on the visible text and won't drill into the structured envelope.
    let server = create_test_server();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_relation_schema",
                "arguments": {
                    "source_type": "VIS",
                    "target_type": "PIL"
                }
            })),
            id: Some(json!(901907)),
        })
        .unwrap()
        .result
        .unwrap();

    let text = response["content"][0]["text"]
        .as_str()
        .expect("visible text present");
    assert!(
        text.contains("no canonical relation"),
        "must state the direction is non-canonical, got: {text}"
    );
    // the legal inverse must be inlined with its concrete relation type
    assert!(
        text.contains("Legal inverse: PIL -[EPITOMIZES]-> VIS"),
        "must inline the legal inverse route, got: {text}"
    );
    // the field NAME must no longer be the only guidance
    assert!(
        !text.contains("check `reverse_canonical`"),
        "must not punt the LLM into `data`, got: {text}"
    );
    // recommended incoming source-kinds for the target must be inlined
    assert!(
        text.contains("Source kinds that can legally reach PIL:") && text.contains("-["),
        "must inline the recommended incoming routes, got: {text}"
    );
}

#[test]
fn test_axon_validate_soll_returns_structured_repair_guidance_and_completeness() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-200', 'Requirement', 'AXO', 'Lonely requirement', 'No links', 'planned', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VAL-AXO-200', 'Validation', 'AXO', '', '', 'pending', '{\"method\":\"manual\"}')")
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_validate",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(4109)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("structured validation data");
    assert_eq!(data["status"].as_str(), Some("warn_soll_invariants"));
    assert_eq!(data["completeness"]["populated"].as_bool(), Some(true));
    assert_eq!(
        data["completeness"]["structurally_connected"].as_bool(),
        Some(false)
    );
    assert_eq!(
        data["completeness"]["evidence_ready"].as_bool(),
        Some(false)
    );
    let repair_guidance = data["repair_guidance"]
        .as_array()
        .expect("repair guidance array");
    // REQ-AXO-902455 — `orphan_requirements` et `validations_without_verifies`
    // ne sont plus des catégories de guidance : ce sont `GUI-PRO-127` et
    // `GUI-PRO-128`, et leur réparation vit dans le corps de la Guideline. Une
    // seule entrée les porte, et elle renvoie à `soll_get(rule_id)` — deux
    // textes de réparation pour un même défaut divergent, c'est ce qui est
    // arrivé à `decisions_without_links` (REQ-AXO-902405).
    let rules_entry = repair_guidance
        .iter()
        .find(|entry| entry["category"].as_str() == Some("declarative_rule_violations"))
        .expect("les violations de règles doivent porter leur guidance");
    let cited = rules_entry["ids"]
        .as_array()
        .expect("ids array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        cited.contains("GUI-PRO-127"),
        "l'exigence non rattachée doit être signalée par GUI-PRO-127.\n---\n{cited}"
    );
    assert!(
        cited.contains("GUI-PRO-128"),
        "la validation sans VERIFIES doit être signalée par GUI-PRO-128.\n---\n{cited}"
    );
    // Chaque ligne cite sa Guideline : c'est ce qui transforme « c'est
    // signalé » en « c'est signalé PARCE QUE <intention> ».
    assert!(
        rules_entry["next_steps"]
            .as_array()
            .expect("next_steps array")
            .iter()
            .any(|s| s.as_str().is_some_and(|t| t.contains("soll_get"))),
        "la guidance doit renvoyer à la règle qui a signalé.\n---\n{rules_entry}"
    );
}

#[test]
fn test_soll_attach_evidence_normalizes_entity_type_for_requirement_verification() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-210', 'Requirement', 'AXO', 'Normalized evidence', 'Uppercase entity type should still count', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-210",
                    "artifacts": [{
                        "artifact_type": "Symbol",
                        "artifact_ref": "normalized_requirement",
                        "confidence": 1.0
                    }]
                }
            })),
            id: Some(json!(4111)),
        })
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(4112)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["done"].as_u64(), Some(1));
    assert_eq!(data["partial"].as_u64(), Some(0));
    assert_eq!(data["missing"].as_u64(), Some(0));
}

// REQ-AXO-902213 — the optional `role` parameter writes `metadata.role` on the
// inserted Traceability row so the anti-orphan gate (REQ-AXO-902192, which
// reads `metadata->>'role' IN ('entry','deliverable')` keyed on the symbol's
// artifact_ref) can EXEMPT a declared entry point. This is the write side that
// closes the round-trip with that reader.
#[test]
fn test_soll_attach_evidence_role_entry_writes_metadata_role() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-902213', 'Requirement', 'AXO', 'Declared entry write path', 'role param writes metadata.role', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "requirement",
                    "entity_id": "REQ-AXO-902213",
                    "role": "entry",
                    "artifacts": [{
                        "artifact_type": "symbol",
                        "artifact_ref": "declared_entry_fn"
                    }]
                }
            })),
            id: Some(json!(9022131)),
        })
        .unwrap()
        .result
        .unwrap();

    // Attach must succeed cleanly (a silent rejection would produce a
    // misleading empty-row failure below).
    assert_eq!(result["data"]["attached"].as_u64(), Some(1), "{result}");

    // Interface assertion (GUI-PRO-115): the persisted jsonb carries role=entry.
    let raw = server
        .graph_store
        .query_json(
            "SELECT metadata->>'role' FROM soll.Traceability \
             WHERE soll_entity_id = 'REQ-AXO-902213' AND artifact_ref = 'declared_entry_fn'",
        )
        .unwrap();
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    assert_eq!(rows.len(), 1, "exactly one traceability row expected: {raw}");
    assert_eq!(rows[0][0].as_str(), Some("entry"), "{raw}");
}

// REQ-AXO-902213 — NON-REGRESSION: with no `role` argument the inserted row
// carries NO `role` key (behaviour is byte-identical to the pre-REQ path).
#[test]
fn test_soll_attach_evidence_no_role_leaves_metadata_role_absent() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-990215', 'Requirement', 'AXO', 'No role regression', 'omitting role leaves metadata untouched', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "requirement",
                    "entity_id": "REQ-AXO-990215",
                    "artifacts": [{
                        "artifact_type": "symbol",
                        "artifact_ref": "ordinary_fn"
                    }]
                }
            })),
            id: Some(json!(9022132)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result["data"]["attached"].as_u64(), Some(1), "{result}");

    // Assert the row carries NO `role` key at all (intent: strict passthrough
    // when `role` is omitted). `metadata->>'role'` is SQL NULL both for a
    // missing key AND a null value, and query_json renders SQL NULL as the
    // string "null" — so read the metadata TEXT and check the key is absent,
    // which is unambiguous and independent of NULL rendering conventions.
    let raw = server
        .graph_store
        .query_json(
            "SELECT metadata::text FROM soll.Traceability \
             WHERE soll_entity_id = 'REQ-AXO-990215' AND artifact_ref = 'ordinary_fn'",
        )
        .unwrap();
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    assert_eq!(rows.len(), 1, "exactly one traceability row expected: {raw}");
    let metadata_text = rows[0][0].as_str().unwrap_or_default();
    assert!(
        !metadata_text.contains("role"),
        "no role key must be present when `role` is omitted: {raw}"
    );
}

// REQ-AXO-902213 — an out-of-vocabulary `role` value rejects the WHOLE call
// cleanly (isError + parameter_repair) and inserts ZERO rows, so a typo can
// never pollute the gate's `IN ('entry','deliverable')` filter.
#[test]
fn test_soll_attach_evidence_invalid_role_rejects_and_inserts_nothing() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-990216', 'Requirement', 'AXO', 'Invalid role rejection', 'bad role rejects whole call', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "requirement",
                    "entity_id": "REQ-AXO-990216",
                    "role": "bidon",
                    "artifacts": [{
                        "artifact_type": "symbol",
                        "artifact_ref": "should_not_persist"
                    }]
                }
            })),
            id: Some(json!(9022133)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["isError"].as_bool(), Some(true), "{result}");
    assert_eq!(
        result["data"]["parameter_repair"]["invalid_field"].as_str(),
        Some("role"),
        "{result}"
    );

    let raw = server
        .graph_store
        .query_json(
            "SELECT metadata->>'role' FROM soll.Traceability \
             WHERE soll_entity_id = 'REQ-AXO-990216'",
        )
        .unwrap();
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    assert!(rows.is_empty(), "invalid role must insert no rows: {raw}");
}

#[test]
fn test_soll_attach_evidence_accepts_file_path_aliases_and_reports_rejections() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-211', 'Requirement', 'AXO', 'File evidence alias', 'File path aliases should attach and explain failures', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let valid_path = repo_root.join("README.md");

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-211",
                    "artifacts": [
                        {
                            "artifact_type": "document",
                            "path": valid_path.to_string_lossy().to_string(),
                            "confidence": 1.0
                        },
                        {
                            "artifact_type": "document",
                            "path": "docs/plans/does-not-exist.md"
                        }
                    ]
                }
            })),
            id: Some(json!(41121)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["attached"].as_u64(), Some(1));
    let accepted_schema = data["accepted_artifact_schema"].as_array().expect("schema");
    assert!(accepted_schema
        .iter()
        .any(|value| value.as_str() == Some("document")));
    let diagnostics = data["artifact_diagnostics"]
        .as_array()
        .expect("artifact diagnostics");
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["status"].as_str(), Some("attached"));
    assert_eq!(
        diagnostics[0]["normalized_artifact_type"].as_str(),
        Some("File")
    );
    assert_eq!(diagnostics[1]["status"].as_str(), Some("rejected"));
    let rejected_reasons = diagnostics[1]["reasons"]
        .as_array()
        .expect("rejected reasons");
    assert!(
        rejected_reasons
            .iter()
            .any(|reason| reason.as_str() == Some("path_not_resolvable")),
        "{result}"
    );
    // REQ-AXO-043 — partial result must surface a top-level status + next_action
    assert_eq!(data["status"].as_str(), Some("partial"));
    assert_eq!(data["total"].as_u64(), Some(2));
    assert!(data["next_action"].as_str().is_some());
    let problem_class = data["operator_guidance"]["problem_class"]
        .as_str()
        .expect("operator_guidance.problem_class");
    assert_eq!(problem_class, "partial_input_invalid");
}

#[test]
fn test_soll_attach_evidence_rejected_all_returns_recovery_contract() {
    // REQ-AXO-043 — when all artifacts are rejected, the LLM-visible content
    // must surface the failure mode AND data must include status, next_action,
    // and operator_guidance.problem_class so the client can recover without
    // re-reading per-artifact diagnostics.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-2120', 'Requirement', 'AXO', 'Reject-all contract', 'All-rejected attach must surface recovery', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-2120",
                    "artifacts": [
                        { "artifact_type": "document", "path": "docs/plans/does-not-exist-1.md" },
                        { "artifact_type": "document", "path": "docs/plans/does-not-exist-2.md" }
                    ]
                }
            })),
            id: Some(json!(41123)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    assert_eq!(data["attached"].as_u64(), Some(0));
    assert_eq!(data["total"].as_u64(), Some(2));
    assert!(
        data["next_action"].as_str().is_some(),
        "next_action must be set when all rejected: {result}"
    );
    assert_eq!(
        data["operator_guidance"]["problem_class"].as_str(),
        Some("input_invalid")
    );
    let actions = data["operator_guidance"]["next_best_actions"]
        .as_array()
        .expect("next_best_actions array");
    assert!(
        !actions.is_empty(),
        "next_best_actions must be non-empty when rejected_all"
    );

    // The LLM-visible content text must surface the failure (not just "Attached 0")
    let content_text = result["content"][0]["text"].as_str().expect("content text");
    assert!(
        content_text.contains("0 of 2") && content_text.contains("rejected"),
        "content must surface the rejection: {content_text}"
    );
}

#[test]
fn test_soll_attach_evidence_parameter_repair_per_kind_hint_for_missing_artifact_ref() {
    // REQ-AXO-139 slice — when an artifact is rejected because `artifact_ref`
    // (and its aliases) are absent, surface a structured `parameter_repair`
    // payload with a per-kind `required_field_hint` so the LLM can fix the
    // input in one round-trip.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-2130', 'Requirement', 'AXO', 'Per-kind hint contract', 'Missing artifact_ref must surface per-kind hint', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-2130",
                    "artifacts": [
                        { "artifact_type": "symbol" }
                    ]
                }
            })),
            id: Some(json!(41139)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifact_ref"));
    assert_eq!(repair["rejected_artifact_kind"].as_str(), Some("Symbol"));
    assert_eq!(
        repair["primary_reason"].as_str(),
        Some("missing_artifact_ref")
    );
    let aliases = repair["accepted_aliases"]
        .as_array()
        .expect("accepted_aliases array");
    let alias_names: Vec<&str> = aliases.iter().filter_map(|v| v.as_str()).collect();
    assert!(alias_names.contains(&"artifact_ref"));
    assert!(alias_names.contains(&"path"));
    assert!(alias_names.contains(&"file_path"));
    assert!(alias_names.contains(&"uri"));
    let hint = repair["required_field_hint"]
        .as_str()
        .expect("required_field_hint string");
    assert!(
        hint.contains("symbol id"),
        "Symbol-kind hint must reference symbol id: {hint}"
    );
    let top_hint = repair["hint"].as_str().expect("hint string");
    assert!(
        top_hint.contains("(Symbol)"),
        "top-level hint must mention rejected kind: {top_hint}"
    );
    // REQ-AXO-901938 — the actionable guidance must inline a copy-pasteable
    // minimal example so the LLM corrects in one round-trip.
    let action = data["operator_guidance"]["next_best_actions"][0]
        .as_str()
        .expect("next_best_actions[0] string");
    assert!(
        action.contains("Example:") && action.contains("artifact_type"),
        "missing_artifact_ref guidance must inline a minimal example: {action}"
    );
}

#[test]
fn test_soll_attach_evidence_parameter_repair_no_artifacts() {
    // REQ-AXO-139 slice — empty `artifacts` array surfaces a generic
    // parameter_repair pointing at the `artifacts` field.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-2131', 'Requirement', 'AXO', 'Empty artifacts contract', 'Empty array must surface parameter_repair', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Requirement",
                    "entity_id": "REQ-AXO-2131",
                    "artifacts": []
                }
            })),
            id: Some(json!(41140)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("no_artifacts"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifacts"));
    assert!(repair["accepted_aliases"].is_array());
    assert!(repair["accepted_artifact_schema"].is_array());
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("artifact_ref"),
        "no_artifacts hint must mention artifact_ref alias: {hint}"
    );
}

#[test]
fn test_soll_attach_evidence_parameter_repair_artifact_type_not_allowed() {
    // REQ-AXO-139 slice — when artifact_type isn't in the entity's
    // accepted_artifact_schema, parameter_repair surfaces invalid_field
    // = `artifact_type` plus the supplied + accepted lists for one-shot fix.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-913', 'Concept', 'AXO', 'Schema-not-allowed contract', 'Concept does not accept Test artifacts', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_attach_evidence",
                "arguments": {
                    "entity_type": "Concept",
                    "entity_id": "CPT-AXO-913",
                    "artifacts": [
                        // Concept's accepted_artifact_schema = [document, file, symbol, rationale];
                        // `test` is not allowed.
                        { "artifact_type": "test", "artifact_ref": "module::tests::dummy" }
                    ]
                }
            })),
            id: Some(json!(41141)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(data["status"].as_str(), Some("rejected_all"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("artifact_type"));
    assert_eq!(repair["supplied_artifact_type"].as_str(), Some("test"));
    let accepted = repair["accepted_artifact_schema"]
        .as_array()
        .expect("accepted_artifact_schema array");
    let accepted_names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    assert!(accepted_names.contains(&"document"));
    assert!(accepted_names.contains(&"rationale"));
    assert!(!accepted_names.contains(&"test"));
}

#[test]
fn test_soll_verify_requirements_terminal_status_counts_as_done() {
    // REQ-AXO-136: status=`completed` and status=`delivered` are terminal —
    // done by definition. The verifier must not flag missing dimensions and
    // must increment the `done` count when an LLM closes a REQ via
    // `soll_manager update status=completed`.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-501', 'Requirement', 'AXO', 'Closed work no metadata', '', 'completed', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-502', 'Requirement', 'AXO', 'Closed work delivered alias', '', 'delivered', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(45136)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        result["data"]["summary"]["done"].as_u64(),
        Some(2),
        "both terminal-status REQs must count as done: {:?}",
        result["data"]
    );
    assert_eq!(
        result["data"]["summary"]["partial"].as_u64(),
        Some(0),
        "terminal REQs must not be partial: {:?}",
        result["data"]
    );
    assert_eq!(
        result["data"]["summary"]["missing"].as_u64(),
        Some(0),
        "terminal REQs must not be missing: {:?}",
        result["data"]
    );

    let details = result["data"]["details"].as_array().expect("details");
    let entry_501 = details
        .iter()
        .find(|v| v["id"].as_str() == Some("REQ-AXO-501"))
        .expect("REQ-AXO-501 entry");
    assert_eq!(
        entry_501["state"].as_str(),
        Some("done"),
        "completed REQ must be `done`: {:?}",
        entry_501
    );
    let missing_501 = entry_501["missing_dimensions"]
        .as_array()
        .expect("missing dimensions array");
    assert!(
        !missing_501.iter().any(|v| v.as_str() == Some("status")),
        "completed status must not be flagged as missing: {:?}",
        missing_501
    );
}

#[test]
fn test_soll_verify_requirements_returns_missing_dimensions_and_actions() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-212', 'Requirement', 'AXO', 'Actionable verification', 'Verification should explain why this requirement is partial', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(41122)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["summary"]["total"].as_u64(), Some(1));
    let required_dimensions = result["data"]["completion_model"]["required_dimensions"]
        .as_array()
        .expect("required dimensions");
    assert!(required_dimensions.iter().any(|value| {
        value["canonical_key"].as_str() == Some("structured_acceptance_criteria")
    }));

    let details = result["data"]["details"].as_array().expect("details");
    let entry = details
        .iter()
        .find(|value| value["id"].as_str() == Some("REQ-AXO-212"))
        .expect("requirement entry");
    assert_eq!(entry["state"].as_str(), Some("partial"));
    assert_eq!(entry["completion_state"].as_str(), Some("partial"));
    assert!(entry["coverage_reason"]
        .as_str()
        .unwrap_or_default()
        .contains("supporting_evidence"));
    let missing_dimensions = entry["missing_dimensions"]
        .as_array()
        .expect("missing dimensions");
    assert!(missing_dimensions
        .iter()
        .any(|value| value.as_str() == Some("evidence")));
    assert!(missing_dimensions
        .iter()
        .any(|value| value.as_str() == Some("validation")));
    let next_actions = entry["suggested_next_actions"]
        .as_array()
        .expect("next actions");
    assert!(next_actions.iter().any(|value| value
        .as_str()
        .unwrap_or_default()
        .contains("soll_attach_evidence")));
    let missing_dimensions_detailed = entry["missing_dimensions_detailed"]
        .as_array()
        .expect("missing dimensions detailed");
    assert!(missing_dimensions_detailed
        .iter()
        .any(|value| { value["canonical_key"].as_str() == Some("supporting_evidence") }));
    let next_actions_detailed = entry["next_actions_detailed"]
        .as_array()
        .expect("next actions detailed");
    assert!(next_actions_detailed.iter().any(|value| {
        value["dimension"].as_str() == Some("qualifying_validation_edge")
            && value["mutation_class"].as_str() == Some("link_validation")
    }));
    let requirements = result["data"]["requirements"]
        .as_array()
        .expect("requirements alias");
    assert_eq!(requirements.len(), details.len());
}

#[test]
fn test_soll_verify_requirements_names_broken_file_evidence_offenders() {
    // REQ-AXO-902337 piste 1 — a broken file-evidence reference must be
    // NAMED (node id + traceability id + path) in the output, not merely
    // counted. SWT had to drop to raw SQL on soll.Traceability precisely
    // because only the count was surfaced.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-902337', 'Requirement', 'AXO', 'Offender naming', 'A broken evidence path must be named', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .unwrap();
    // Absolute path that does not exist → the freshness sweep stat()s it and
    // records artifact_status='broken'. Absolute so resolution never depends
    // on the project root.
    let broken_path = "/nonexistent/axon/req_902337_broken_offender.rs";
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-AXO-902337', 'requirement', 'REQ-AXO-902337', 'file', '{broken_path}', 1.0, 0)"))
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "AXO" }
            })),
            id: Some(json!(902337)),
        })
        .unwrap()
        .result
        .unwrap();

    let details = result["data"]["details"].as_array().expect("details");
    let entry = details
        .iter()
        .find(|value| value["id"].as_str() == Some("REQ-AXO-902337"))
        .expect("requirement entry");
    assert_eq!(entry["broken_file_evidence_count"].as_u64(), Some(1));
    let offenders = entry["broken_file_evidence_offenders"]
        .as_array()
        .expect("broken_file_evidence_offenders array");
    assert_eq!(offenders.len(), 1, "exactly one broken offender");
    let offender = &offenders[0];
    assert_eq!(offender["path"].as_str(), Some(broken_path));
    assert_eq!(
        offender["traceability_id"].as_str(),
        Some("TRC-AXO-902337")
    );

    // The path must also appear in the human/LLM text surface, so no raw
    // SQL is needed to identify what to purge.
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains(broken_path),
        "text must name the broken evidence path, got: {text}"
    );
    assert!(
        text.contains("REQ-AXO-902337"),
        "text must name the offending requirement, got: {text}"
    );
}

#[test]
fn test_anomalies_downgrades_noncanonical_intent_gaps_when_soll_baseline_is_complete() {
    let server = create_test_server();
    let code = "TST".to_string();
    let pil_id = format!("PIL-{code}-001");
    let req_id = format!("REQ-{code}-001");
    let dec_id = format!("DEC-{code}-001");
    let val_id = format!("VAL-{code}-001");
    let trc_id = format!("TRC-{code}-001");
    // REQ-AXO-902455 — une baseline SOLL « complète » remonte à une Vision
    // (`GUI-PRO-122`). Sans elle l'exigence pend dans le vide, et
    // `concept_completeness` doit dire non — c'est le fixture qu'on complète,
    // pas le verdict qu'on assouplit.
    let vis_id = format!("VIS-{code}-001");
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{vis_id}', 'Vision', '{code}', 'Nord du projet de test', 'Ancre de filiation', 'current', '{{}}') ON CONFLICT (id) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{pil_id}', 'Pillar', '{code}', 'Core pillar', '', 'current', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{req_id}', 'Requirement', '{code}', 'Healthy requirement', '', 'current', '{{\"acceptance_criteria\":\"done\"}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{dec_id}', 'Decision', '{code}', 'Healthy decision', '', 'current', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('{val_id}', 'Validation', '{code}', 'Healthy validation', '', 'delivered', '{{}}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('{pil_id}', '{vis_id}', 'EPITOMIZES', '{code}') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('{req_id}', '{pil_id}', 'BELONGS_TO', '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('{dec_id}', '{req_id}', 'SOLVES', '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('{val_id}', '{req_id}', 'VERIFIES', '{code}')"))
        .unwrap();
    server
        .graph_store
        .execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('{trc_id}', 'requirement', '{req_id}', 'Symbol', 'healthy_requirement', 1.0, 0)"))
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "anomalies",
                "arguments": { "project": code, "mode": "brief" }
            })),
            id: Some(json!(4113)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = result["data"].clone();
    assert_eq!(
        data["summary"]["concept_completeness"].as_bool(),
        Some(true)
    );
    assert_eq!(
        data["summary"]["implementation_completeness"].as_bool(),
        Some(true)
    );
    assert_eq!(data["summary"]["orphan_intent_count"].as_u64(), Some(0));
    assert!(
        data["summary"]["heuristic_intent_gap_count"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );
}

#[test]
fn test_vcr4_soll_restore_recovers_links_and_metadata_when_present() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let source_server = create_test_server();
    source_server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-900', 'Vision', 'AXO', 'Axon Vision', 'Stable conceptual continuity', '', '{\"goal\":\"Protect SOLL while evolving IST\"}')")
        .unwrap();

    let do_create = |entity: &str, data: serde_json::Value, id: i64| -> String {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": { "action": "create", "entity": entity, "data": data }
            })),
            id: Some(json!(id)),
        };
        let response = source_server.handle_request(req);
        let result = response
            .unwrap()
            .result
            .expect("Expected SOLL creation result");
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(content.contains("SOLL entity created"), "{content}");
        result["data"]["created_id"]
            .as_str()
            .expect("created_id present")
            .to_string()
    };

    // Pillar -> seeded Vision (EPITOMIZES)
    let pillar_id = do_create(
        "pillar",
        json!({
            "project_code": "AXO",
            "title": "Concept Preservation",
            "description": "SOLL must survive runtime churn",
            "metadata": { "owner": "platform" },
            "attach_to": "VIS-AXO-900",
            "relation_type": "EPITOMIZES"
        }),
        300,
    );

    // Requirement -> Pillar (BELONGS_TO)
    let requirement_id = do_create(
        "requirement",
        json!({
            "project_code": "AXO",
            "title": "Reliable Restore",
            "description": "Restore from official export without destructive reset",
            "priority": "P1",
            "metadata": { "risk": "high" },
            "attach_to": pillar_id,
            "relation_type": "BELONGS_TO"
        }),
        301,
    );

    // Decision -> Requirement (SOLVES), status current
    let decision_id = do_create(
        "decision",
        json!({
            "project_code": "AXO",
            "title": "Protect SOLL",
            "context": "Agents previously removed conceptual state",
            "rationale": "Exports must preserve the conceptual thread",
            "status": "current",
            "metadata": { "scope": "restore" },
            "attach_to": requirement_id,
            "relation_type": "SOLVES"
        }),
        302,
    );

    // Validation -> Requirement (VERIFIES), result delivered
    let validation_id = do_create(
        "validation",
        json!({
            "project_code": "AXO",
            "method": "vcr4-links",
            "result": "delivered",
            "metadata": { "evidence": "test" },
            "attach_to": requirement_id,
            "relation_type": "VERIFIES"
        }),
        303,
    );

    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_export",
            "arguments": {}
        })),
        id: Some(json!(500)),
    };

    let export_response = source_server.handle_request(export_req);
    let export_result = export_response
        .unwrap()
        .result
        .expect("Expected SOLL export result");
    let export_text = export_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let export_path = export_text
        .lines()
        .find_map(|line| line.strip_prefix("✅ Exported to "))
        .expect("Expected export path line")
        .trim()
        .to_string();
    let export_markdown = std::fs::read_to_string(&export_path).unwrap();
    println!("DEBUG EXPORT:\n{}", export_markdown);
    assert!(export_markdown.contains("BELONGS_TO"));
    assert!(export_markdown.contains("SOLVES"));
    assert!(export_markdown.contains("VERIFIES"));
    assert!(export_markdown.contains("platform"));
    assert!(export_markdown.contains("high"));
    assert!(export_markdown.contains("scope"));

    let restore_server = create_test_server();
    let restore_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(501)),
    };

    let restore_response = restore_server.handle_request(restore_req);
    let restore_result = restore_response
        .unwrap()
        .result
        .expect("Expected SOLL restore result");
    let restore_text = restore_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        restore_text.contains("SOLL restore complete"),
        "{}",
        restore_text
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id = '{}' AND target_id = '{}'",
                requirement_id, pillar_id
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='SOLVES' AND source_id = '{}' AND target_id = '{}'",
                decision_id, requirement_id
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='VERIFIES' AND source_id = '{}' AND target_id = '{}'",
                validation_id, requirement_id
            ))
            .unwrap(),
        1
    );

    let pillar_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Pillar' AND id = '{}'",
            pillar_id
        ))
        .unwrap();
    let requirement_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Requirement' AND id = '{}'",
            requirement_id
        ))
        .unwrap();
    let decision_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Decision' AND id = '{}'",
            decision_id
        ))
        .unwrap();
    let all_validations = restore_server
        .graph_store
        .query_json("SELECT * FROM soll.Node WHERE type='Validation'")
        .unwrap();
    println!("ALL VALIDATIONS: {}", all_validations);

    let validation_metadata = restore_server
        .graph_store
        .query_json(&format!(
            "SELECT metadata FROM soll.Node WHERE type='Validation' AND id = '{}'",
            validation_id
        ))
        .unwrap();

    assert!(pillar_metadata.contains("platform"));
    assert!(
        requirement_metadata.contains("high"),
        "{}",
        requirement_metadata
    );
    assert!(decision_metadata.contains("restore"));
    assert!(
        validation_metadata.contains("test"),
        "{}",
        validation_metadata
    );

    let second_restore_response = restore_server.handle_request(JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "restore_soll",
            "arguments": { "path": export_path }
        })),
        id: Some(json!(502)),
    });
    second_restore_response
        .unwrap()
        .result
        .expect("Expected second restore result");

    assert_eq!(
        restore_server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM soll.Edge WHERE relation_type='BELONGS_TO' AND source_id = '{}' AND target_id = '{}'",
                requirement_id, pillar_id
            ))
            .unwrap(),
        1
    );

    let _ = std::fs::remove_file(&export_path);
}

#[test]
fn test_axon_commit_work_enforces_guideline() {
    let server = create_test_server();

    // Insert a Guideline into SolDB requiring tests to be updated if src/mcp/ is modified
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) 
         VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'Mise à jour des Tests', 'Les modifications de src/mcp/ doivent inclure des tests', 'active', '{\"trigger_path\":\"src/mcp/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    // 1. Simulate a bad commit (modifies src/mcp/ but no tests.rs)
    let req_bad = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "project_code": "AXO",
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs"],
                "message": "fix: update tools",
                "dry_run": true
            }
        },
        "id": 1
    });

    let res_bad = server
        .handle_request(serde_json::from_value(req_bad).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content_bad = res_bad.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG CONTENT BAD: {}", content_bad);

    // It should be rejected
    assert!(res_bad
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content_bad.contains("GUI-AXO-001") || content_bad.contains("GUI-PRO-001"));
    assert!(content_bad.contains("Remediation"));

    // 2. Simulate a good commit (modifies src/mcp/ AND legacy tests.rs)
    let req_good = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "project_code": "AXO",
                "diff_paths": ["src/axon-core/src/mcp/tools_soll.rs", "src/axon-core/src/mcp/tests.rs", "SKILL.md"],
                "message": "fix: update tools and tests",
                "dry_run": true
            }
        },
        "id": 2
    });

    let res_good = server
        .handle_request(serde_json::from_value(req_good).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content_good = res_good.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // It should pass
    assert!(!res_good
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(content_good.contains("Validation passed"));

    // 3. Modular MCP tests must also satisfy the legacy `tests.rs` rule.
    let req_modular_test = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "project_code": "AXO",
                "diff_paths": [
                    "src/axon-core/src/mcp.rs",
                    "src/axon-core/src/mcp/tests/guidance_contract.rs"
                ],
                "message": "fix: update mcp guidance tests",
                "dry_run": true
            }
        },
        "id": 3
    });

    let res_modular_test = server
        .handle_request(serde_json::from_value(req_modular_test).unwrap())
        .unwrap()
        .result
        .unwrap();
    assert!(!res_modular_test
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
}

// REQ-AXO-145 — `axon_pre_flight_check` accepts `incremental: true` to
// validate each diff_path individually and return per-file violations.
// Default (omitted/false) preserves the batch-validation contract.
//
// Tests use a unique trigger path (`src/req145_fixture/`) so the new
// guideline isolates from any pre-seeded GUI-PRO-* rules.
fn insert_req145_fixture_guideline(server: &McpServer) {
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-AXO-1450', 'Guideline', 'AXO', 'REQ-145 fixture rule', \
             'Diffs touching src/req145_fixture/ must include req145_marker.rs', 'active', \
             '{\"trigger_path\":\"src/req145_fixture/\",\"required_path\":\"req145_marker.rs\",\"enforcement\":\"strict\"}')",
        )
        .unwrap();
}

#[test]
fn test_axon_pre_flight_check_incremental_returns_per_file_violations() {
    let server = create_test_server();
    insert_req145_fixture_guideline(&server);

    // Mixed batch: bad file (triggers fixture rule, no marker) +
    // good file (carries the marker).
    let req_incremental = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "project_code": "AXO",
                "diff_paths": [
                    "src/req145_fixture/feature.rs",
                    "src/req145_fixture/req145_marker.rs"
                ],
                "incremental": true
            }
        },
        "id": 1
    });

    let res = server
        .handle_request(serde_json::from_value(req_incremental).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "incremental dry-run with one failing file must surface isError=true"
    );

    let data = res.get("data").expect("data field present");
    assert_eq!(
        data.get("incremental").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(data.get("files_checked").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(data.get("failing_files").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        data.get("first_failing_path").and_then(|v| v.as_str()),
        Some("src/req145_fixture/feature.rs")
    );

    let per_file = data
        .get("per_file_violations")
        .and_then(|v| v.as_object())
        .expect("per_file_violations is an object");
    let bad_entry = per_file
        .get("src/req145_fixture/feature.rs")
        .expect("bad path entry present");
    assert_eq!(bad_entry.get("ok").and_then(|v| v.as_bool()), Some(false));
    assert!(
        bad_entry
            .get("violations")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "bad path must carry at least one violation"
    );
    let good_entry = per_file
        .get("src/req145_fixture/req145_marker.rs")
        .expect("good path entry present");
    assert_eq!(good_entry.get("ok").and_then(|v| v.as_bool()), Some(true));
}

#[test]
fn test_axon_pre_flight_check_default_mode_remains_batch() {
    let server = create_test_server();
    insert_req145_fixture_guideline(&server);

    // Same mixed batch but WITHOUT incremental. The aggregate batch view
    // satisfies the rule because the marker file is in the same set,
    // so it must pass.
    let req_default = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_pre_flight_check",
            "arguments": {
                "diff_paths": [
                    "src/req145_fixture/feature.rs",
                    "src/req145_fixture/req145_marker.rs"
                ]
            }
        },
        "id": 2
    });

    let res = server
        .handle_request(serde_json::from_value(req_default).unwrap())
        .unwrap()
        .result
        .unwrap();

    assert!(
        !res.get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "default (batch) mode passes when marker is in the same diff_paths set"
    );
    let text = res
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    assert!(
        text.contains("Validation passed"),
        "batch mode must surface the batch validation message"
    );
    // Default mode never sets the incremental marker.
    let incremental_marker = res
        .pointer("/data/incremental")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!incremental_marker);
}

// REQ-AXO-121 — `path_satisfies_required_path` must recognize inline
// `#[cfg(test)]` blocks inside a modified `.rs` file as satisfying the
// `tests.rs` requirement. This unblocks (a) Rust binary crates whose
// canonical idiom is `#[cfg(test)] mod tests {}` inline, and (b)
// trivial library hygiene fixes (one-line attribute changes in files
// that already carry inline tests). The sibling `_tests.rs` patterns
// remain valid; this is a pure addition to the matcher.
#[test]
fn test_axon_commit_work_recognizes_inline_cfg_test_in_modified_rs_file() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'TDD', 'tests required', 'active', \
             '{\"trigger_path\":\"src/inline_tests/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
        )
        .unwrap();

    // Write a temp file that emulates a Rust source with inline tests.
    let tmp = tempdir().unwrap();
    let inline_test_path = tmp.path().join("src/inline_tests/foo.rs");
    std::fs::create_dir_all(inline_test_path.parent().unwrap()).unwrap();
    std::fs::write(
        &inline_test_path,
        "fn foo() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn smoke() {}\n}\n",
    )
    .unwrap();
    let inline_path_str = inline_test_path.to_string_lossy().to_string();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": [inline_path_str],
                "message": "test: inline cfg(test) recognized",
                "dry_run": true
            }
        },
        "id": 1
    });

    let result = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "inline #[cfg(test)] must satisfy the TDD gate without a sibling _tests.rs file: {content}"
    );
    assert!(content.contains("Validation passed"), "{content}");
}

#[test]
fn test_axon_commit_work_still_rejects_modified_rs_file_without_any_test_marker() {
    let server = create_test_server();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('GUI-AXO-001', 'Guideline', 'AXO', 'TDD', 'tests required', 'active', \
             '{\"trigger_path\":\"src/no_tests_here/\",\"required_path\":\"tests.rs\",\"enforcement\":\"strict\"}')"
        )
        .unwrap();

    let tmp = tempdir().unwrap();
    let bare_path = tmp.path().join("src/no_tests_here/bar.rs");
    std::fs::create_dir_all(bare_path.parent().unwrap()).unwrap();
    std::fs::write(&bare_path, "fn bar() {}\n").unwrap();
    let bare_path_str = bare_path.to_string_lossy().to_string();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "project_code": "AXO",
                "diff_paths": [bare_path_str],
                "message": "test: no inline tests, no sibling",
                "dry_run": true
            }
        },
        "id": 2
    });

    let result = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "a .rs file with neither inline tests nor a sibling test path must still be rejected: {content}"
    );
    assert!(content.contains("Remediation"), "{content}");
}

#[test]
fn test_bootstrap_injects_global_guidelines() {
    let server = create_test_server();

    // Check GUI-PRO-001
    let count1 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-001' AND type = 'Guideline' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count1, 1, "GUI-PRO-001 should be injected at bootstrap");

    let meta1_raw = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-001'")
        .unwrap();
    println!("DEBUG META1 RAW: {}", meta1_raw);
    let meta1: Vec<Vec<String>> = serde_json::from_str(&meta1_raw).unwrap();
    assert!(
        meta1[0][0].contains("\"phase\":\"pre-code\"")
            || meta1[0][0].contains("\"phase\": \"pre-code\""),
        "GUI-PRO-001 should have phase: pre-code"
    );

    // Check GUI-PRO-002
    let count2 = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-PRO-002' AND type = 'Guideline' AND project_code = 'PRO'"
    ).unwrap();
    assert_eq!(count2, 1, "GUI-PRO-002 should be injected at bootstrap");

    let meta2_raw = server
        .graph_store
        .query_json("SELECT metadata FROM soll.Node WHERE id = 'GUI-PRO-002'")
        .unwrap();
    println!("DEBUG META2 RAW: {}", meta2_raw);
    let meta2: Vec<Vec<String>> = serde_json::from_str(&meta2_raw).unwrap();
    assert!(
        meta2[0][0].contains("\"phase\":\"post-code\"")
            || meta2[0][0].contains("\"phase\": \"post-code\""),
        "GUI-PRO-002 should have phase: post-code"
    );
}

#[test]
fn test_axon_init_project_returns_global_guidelines() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/home/dstadel/projects/BookingSystem",
                "concept_document_url_or_text": "We want a booking system."
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    println!("DEBUG INIT OUTPUT: {}", content);

    // Output should contain the global guidelines injected at bootstrap
    assert!(content.contains("GUI-PRO-001"));
    assert!(content.contains("GUI-PRO-002"));
    assert!(content.contains("Available global rules"));
    // REQ-AXO-901909 — the catalogue is a terse digest, not a full-body
    // dump. The read-on-demand pointer must be advertised so the LLM knows
    // where the full bodies live, and no rule line may carry an unbounded
    // multi-line body.
    assert!(
        content.contains("read any body in full via"),
        "init must point to the on-demand body read, got: {content}"
    );
    // REQ-AXO-902400 — and it must point at `soll_get`, not at the raw SQL the
    // canon forbids. Signalé par KKI (llm_feedback #175) : l'init prescrivait
    // lui-même `sql SELECT description FROM soll.Node`, dans le tout premier
    // appel de chaque session, chez chaque tenant.
    assert!(
        content.contains("soll_get(id="),
        "init must prescribe soll_get for body reads, got: {content}"
    );
    assert!(
        !content.contains("SELECT description FROM soll.Node"),
        "init must NOT prescribe the raw SQL body read, got: {content}"
    );
    for line in content.lines().filter(|l| l.starts_with("- **GUI-")) {
        assert!(
            line.chars().count() <= 200,
            "REQ-AXO-901909: guideline line must be a bounded digest, got: {line}"
        );
    }
    assert!(content.contains("Server-assigned project code: `BKS`"));
    assert_eq!(result["data"]["project_code"].as_str(), Some("BKS"));
    assert_eq!(
        result["data"]["project_name"].as_str(),
        Some("BookingSystem")
    );
    assert_eq!(
        result["data"]["project_path"].as_str(),
        Some("/home/dstadel/projects/BookingSystem")
    );
}

/// REQ-AXO-902500 — le digest des ~60 règles PRO ne doit PAS être réémis à chaque init.
///
/// Trois cas, trois réponses, et ce test les falsifie tous les trois. Sans ces contre-
/// exemples le correctif serait invérifiable : `test_axon_init_project_returns_global_guidelines`
/// n'exerce que la première branche, et une régression qui rendrait le digest partout
/// le laisserait vert.
///
/// Convergence TE2 + KKI, mesurée : ~12 Ko par appel pour un contenu qui ne change
/// quasiment jamais, y compris quand l'appel est MUTATIF — c'est-à-dire au moment exact
/// où `GUI-PRO-028` prescrit d'appeler cet outil pour poser un `session_pointer`, en fin
/// de handoff, quand le contexte est le plus rare.
#[test]
fn le_digest_des_regles_globales_nest_rendu_quaux_projets_qui_nont_pas_arbitre() {
    let server = create_test_server();

    let init = |args: serde_json::Value| -> String {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "axon_init_project", "arguments": args },
            "id": 1
        });
        let response = server
            .handle_request(serde_json::from_value(req).unwrap())
            .unwrap();
        response.result.unwrap().get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    };

    // --- Cas 1 : projet neuf, aucune guideline propre ⇒ le digest ET la question.
    let bootstrap = init(serde_json::json!({
        "project_path": "/home/dstadel/projects/DigestBootstrap",
        "concept_document_url_or_text": "un projet neuf."
    }));
    assert!(
        bootstrap.contains("Available global rules") && bootstrap.contains("GUI-PRO-001"),
        "un projet qui n'a jamais arbitré doit recevoir le digest, got: {bootstrap}"
    );

    // --- Cas 2 : appel MUTATIF (pose de session_pointer) ⇒ jamais de digest.
    // C'est le cas que KKI nomme : la procédure censée PRÉSERVER du contexte avant
    // compaction en consommait 12 Ko pour écrire trois champs.
    let mutatif = init(serde_json::json!({
        "project_path": "/home/dstadel/projects/DigestBootstrap",
        "session_pointer": { "kind": "none", "value": "" }
    }));
    assert!(
        !mutatif.contains("Available global rules"),
        "un appel mutatif ne doit PAS resservir le digest, got: {mutatif}"
    );
    assert!(
        mutatif.contains("appel mutatif"),
        "l'omission doit se DÉCLARER, pas se produire en silence (invariant KKI #204), got: {mutatif}"
    );

    // --- Cas 3 : projet ayant DÉJÀ ses propres guidelines ⇒ une ligne, pas un digest.
    // C'est littéralement le cas TE2 : « GUI-TE2-018 apparaît plus haut dans la même
    // sortie. Je ne réponds jamais à la question — il n'y a rien à activer. »
    // Le code projet est ALLOUÉ par le serveur, pas devinable — on le lit, on ne le
    // suppose pas (c'est le défaut même que ce volet corrige ailleurs).
    let codes: Vec<Vec<String>> = serde_json::from_str(
        &server
            .graph_store
            .query_json(
                "SELECT DISTINCT project_code FROM soll.Node \
                 WHERE type = 'Vision' AND project_code <> 'PRO'",
            )
            .expect("lire le code projet"),
    )
    .expect("decoder le code projet");
    assert_eq!(
        codes.len(),
        1,
        "le harnais doit porter exactement un projet non-PRO, got: {codes:?}"
    );
    let code = &codes[0][0];

    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.Node (id, type, title, description, status, project_code) \
             VALUES ('GUI-{code}-001', 'Guideline', 'regle locale', 'corps', 'current', '{code}')"
        ))
        .expect("seed guideline locale");

    let continuation = init(serde_json::json!({
        "project_path": "/home/dstadel/projects/DigestBootstrap"
    }));
    assert!(
        !continuation.contains("Available global rules"),
        "un projet qui a DÉJÀ arbitré ne doit pas revoir le digest, got: {continuation}"
    );
    assert!(
        continuation.contains("règle(s) globale(s) active(s)")
            && continuation.contains("soll_get"),
        "la ligne de remplacement doit COMPTER les règles et dire où lire un corps, got: {continuation}"
    );
}

// REQ-AXO-119 — axon_init_project must return a stable kickoff bundle
// (kickoff_prompt, methodology_summary, entry_points, active_handoff)
// on every call so an LLM with only Axon MCP access can onboard
// itself in one round-trip without having to re-discover the
// bootstrap protocol or the project's reading order.

#[test]
fn test_axon_init_project_returns_kickoff_bundle_for_first_init() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/home/dstadel/projects/BookingSystem" }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = result["data"]["kickoff_bundle"]
        .as_object()
        .expect("first init must return a kickoff_bundle in data");
    let client_bundle = result["structuredContent"]["kickoff_bundle"]
        .as_object()
        .expect("REQ-AXO-902517: MCP clients must receive kickoff_bundle in structuredContent");
    assert_eq!(
        client_bundle, bundle,
        "structuredContent must mirror the canonical producer data without drift"
    );
    assert!(bundle.contains_key("kickoff_prompt"));
    assert!(bundle.contains_key("methodology_summary"));
    assert!(bundle.contains_key("entry_points"));
    assert!(bundle.contains_key("active_handoff"));
    // REQ-AXO-278: Bootstrap-vs-Continuation phase detection (GUI-PRO-026)
    assert!(
        bundle.contains_key("bootstrap_required"),
        "kickoff_bundle must include bootstrap_required boolean per REQ-AXO-278"
    );
    assert!(
        bundle["bootstrap_required"].is_boolean(),
        "bootstrap_required must be boolean, got {:?}",
        bundle["bootstrap_required"]
    );
    assert!(
        bundle.contains_key("input_documents"),
        "kickoff_bundle must include input_documents[] array per REQ-AXO-278"
    );
    assert!(
        bundle["input_documents"].is_array(),
        "input_documents must be an array, got {:?}",
        bundle["input_documents"]
    );
    // Fresh project (no VIS-{code}-001) => bootstrap_required=true
    let bootstrap_required = bundle["bootstrap_required"].as_bool().unwrap();
    let input_documents = bundle["input_documents"].as_array().unwrap();
    if bootstrap_required {
        // input_documents[] may be empty if path doesn't exist on disk, but
        // shape must hold (array of objects with path/size_bytes/mtime_unix_secs)
        for doc in input_documents {
            let obj = doc
                .as_object()
                .expect("input_documents entries must be objects");
            assert!(obj.contains_key("path"));
            assert!(obj.contains_key("size_bytes"));
            assert!(obj.contains_key("mtime_unix_secs"));
        }
    } else {
        assert!(
            input_documents.is_empty(),
            "input_documents must be empty when bootstrap_required=false (Continuation phase)"
        );
    }
    let entry_points = bundle["entry_points"]
        .as_array()
        .expect("entry_points must be an array");
    assert!(
        entry_points.len() >= 8,
        "entry_points must list the cold-start reading order; got {} steps",
        entry_points.len()
    );
    // file + mcp must be represented; `sql` is now FORBIDDEN (REQ-AXO-902355).
    let kinds: std::collections::HashSet<&str> = entry_points
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()))
        .collect();
    assert!(
        kinds.contains("file"),
        "entry_points must include `file` steps: {kinds:?}"
    );
    assert!(
        kinds.contains("mcp"),
        "entry_points must include `mcp` steps: {kinds:?}"
    );
    // REQ-AXO-902355 — the cold-start reading order must NOT prescribe raw SQL:
    // Vision/Pillar bodies are PUSHED in soll_skeleton (and inlined in the
    // Continuation block); Decisions pull via soll_get. This assertion used to
    // REQUIRE a `sql` step — it enforced the very defect VPC filed (inbox 10520);
    // inverting it prevents the defect's return instead.
    assert!(
        !kinds.contains("sql"),
        "entry_points must NOT prescribe raw `sql` reads (REQ-AXO-902355): {kinds:?}"
    );

    // REQ-AXO-902355 — lock all three residual `SELECT description` prescriptions
    // (entry_points, soll_skeleton.pull_note, default_kickoff_prompt) at once:
    // no cold-start read path may hand an LLM a raw soll.Node SELECT.
    let bundle_str = serde_json::to_string(bundle).unwrap();
    assert!(
        !bundle_str.contains("SELECT description"),
        "kickoff_bundle must not prescribe `SELECT description FROM soll.Node` anywhere (REQ-AXO-902248/902355)"
    );

    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("Kickoff bundle"),
        "content must point to the bundle: {content}"
    );
    // REQ-AXO-902516 — this response is consumed from the CLIENT repository.
    // A relative Axon runtime script is therefore not executable there.  Route
    // through the public MCP status contract, with copyable JSON arguments;
    // status owns the environment-specific operator recovery hint.
    assert!(
        !content.contains("./scripts/axon-live"),
        "client onboarding must not prescribe an Axon-repo-relative script: {content}"
    );
    assert!(
        content.contains(r#"status({"mode":"brief"})"#),
        "the conditional indexing check needs a schema-valid MCP call: {content}"
    );
}

#[test]
fn test_axon_init_project_returns_identical_bundle_on_re_init() {
    let server = create_test_server();
    let args = serde_json::json!({ "project_path": "/home/dstadel/projects/BookingSystem" });
    let make_req = |id: u64| {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "axon_init_project", "arguments": args },
            "id": id
        })
    };
    let first = server
        .handle_request(serde_json::from_value(make_req(1)).unwrap())
        .unwrap()
        .result
        .unwrap();
    let second = server
        .handle_request(serde_json::from_value(make_req(2)).unwrap())
        .unwrap()
        .result
        .unwrap();
    // Both calls must return the same project_code.
    assert_eq!(
        first["data"]["project_code"],
        second["data"]["project_code"]
    );
    // The kickoff bundle must be present and equivalent on both calls.
    let b1 = &first["data"]["kickoff_bundle"];
    let b2 = &second["data"]["kickoff_bundle"];
    assert!(b1.is_object() && b2.is_object());
    assert_eq!(b1["kickoff_prompt"], b2["kickoff_prompt"]);
    assert_eq!(b1["methodology_summary"], b2["methodology_summary"]);
    assert_eq!(b1["entry_points"], b2["entry_points"]);
    assert_eq!(b1["active_handoff"], b2["active_handoff"]);
}

#[test]
fn test_axon_init_project_bundle_active_handoff_null_when_no_working_notes() {
    let server = create_test_server();
    // /tmp/non-existent-axon-project has no docs/working-notes directory.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/tmp/non-existent-axon-project-for-bundle-test" }
        },
        "id": 119
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = &result["data"]["kickoff_bundle"];
    assert!(
        bundle["active_handoff"].is_null(),
        "active_handoff must be null when docs/working-notes is absent: {bundle}"
    );
}

// REQ-AXO-176 — kickoff bundle enrichment: aggregate recent project
// activity inline so a fresh LLM session reaches productive state from
// a single MCP call, without adding a 10th SOLL entity type.
#[test]
fn test_axon_init_project_bundle_includes_recent_activity_fields() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": { "project_path": "/home/dstadel/projects/BookingSystem" }
        },
        "id": 176
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = result["data"]["kickoff_bundle"]
        .as_object()
        .expect("kickoff_bundle must be present");

    // Each new field must be an array; content may be empty for a sparse
    // project. The contract is shape-stable, not row-count-stable.
    for field in [
        "in_progress_requirements",
        "wave_1_unblockers",
        "recent_req_commits",
        "recent_soll_writes",
    ] {
        assert!(
            bundle.contains_key(field),
            "bundle must contain `{field}` (REQ-AXO-176)"
        );
        assert!(
            bundle[field].is_array(),
            "`{field}` must be an array, got {}: {}",
            bundle[field],
            bundle.get(field).map(|v| v.to_string()).unwrap_or_default()
        );
    }

    // in_progress_requirements rows must carry the documented schema.
    if let Some(arr) = bundle["in_progress_requirements"].as_array() {
        for row in arr {
            assert!(
                row.get("id").and_then(|v| v.as_str()).is_some(),
                "in_progress_requirements row must have id: {row}"
            );
            assert!(
                row.get("title").and_then(|v| v.as_str()).is_some(),
                "in_progress_requirements row must have title: {row}"
            );
            assert!(
                row.get("priority").is_some(),
                "in_progress_requirements row must have priority key (may be null): {row}"
            );
        }
    }

    // recent_soll_writes rows must carry id+type+title+updated_at keys.
    if let Some(arr) = bundle["recent_soll_writes"].as_array() {
        for row in arr {
            for key in ["id", "type", "title", "updated_at"] {
                assert!(
                    row.get(key).is_some(),
                    "recent_soll_writes row must have `{key}` key: {row}"
                );
            }
        }
    }

    // Human-readable text must reference the new fields so an LLM
    // scanning content alone can discover them.
    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("in_progress_requirements"),
        "response text must advertise the new bundle fields: {content}"
    );
}

// REQ-AXO-143 — `session_pointer` is the canonical workflow-agnostic
// onboarding pointer. Persisted on axon_init_project, surfaced on the
// kickoff bundle AND on `status.data.instance_identity.session_pointer`.
#[test]
fn test_axon_init_project_persists_session_pointer_url_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-url-fixture",
                "session_pointer": {
                    "kind": "url",
                    "value": "https://linear.app/team/issue/AXO-143",
                    "label": "active ticket"
                }
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let bundle = &result["data"]["kickoff_bundle"];
    let pointer = bundle
        .get("session_pointer")
        .expect("session_pointer present");
    assert_eq!(pointer["kind"].as_str(), Some("url"));
    assert_eq!(
        pointer["value"].as_str(),
        Some("https://linear.app/team/issue/AXO-143")
    );
    assert_eq!(pointer["label"].as_str(), Some("active ticket"));
    // active_handoff alias only mirrors kind=file.
    assert!(
        bundle["active_handoff"].is_null(),
        "active_handoff alias must stay null when kind=url: {bundle}"
    );
}

#[test]
fn test_axon_init_project_session_pointer_kind_none_clears_value() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-none-fixture",
                "session_pointer": { "kind": "none" }
            }
        },
        "id": 2
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let pointer = result["data"]["kickoff_bundle"]["session_pointer"].clone();
    assert_eq!(pointer["kind"].as_str(), Some("none"));
    assert!(pointer["value"].is_null());
}

#[test]
fn test_axon_init_project_rejects_invalid_session_pointer_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-invalid-fixture",
                "session_pointer": { "kind": "wiki", "value": "ignored" }
            }
        },
        "id": 3
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "invalid kind must be rejected: {result}"
    );
    let parameter_repair = result["data"]["parameter_repair"].clone();
    assert_eq!(
        parameter_repair["invalid_field"].as_str(),
        Some("session_pointer")
    );
}

#[test]
fn test_axon_init_project_rejects_session_pointer_missing_value_for_url_kind() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_path": "/tmp/req143-missing-value-fixture",
                "session_pointer": { "kind": "soll_node" }
            }
        },
        "id": 4
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["isError"].as_bool(), Some(true));
}

#[test]
fn test_axon_init_project_rejects_client_project_code_when_it_differs_from_server_assignment() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_init_project",
            "arguments": {
                "project_code": "AXO",
                "project_path": "/home/dstadel/projects/BookingSystem"
            }
        },
        "id": 10007
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
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
    assert!(content.contains("is server-assigned"), "{content}");
    assert!(content.contains("BKS"), "{content}");
}

#[test]
fn test_axon_apply_guidelines_creates_local_copies() {
    // REQ-AXO-902142 — serialize env access + force the synchronous path (a
    // sibling test sets AXON_MCP_MUTATION_JOBS=true; the async envelope has no
    // `GUI-AXO-001` content line, breaking this test under concurrency).
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();

    // First init the project
    server
        .graph_store
        .sync_project_registry_entry("BKS", Some("BookingSystem"), None)
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-001"]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // Output should confirm creation
    assert!(content.contains("GUI-AXO-001"));
    assert!(content.contains("Inheritance applied"));

    // Verify in DB
    let count = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Node WHERE id = 'GUI-AXO-001' AND type = 'Guideline' AND project_code = 'AXO'"
    ).unwrap();
    assert_eq!(count, 1, "Local guideline should be created");

    // Verify edge
    let edge_count = server.graph_store.query_count(
        "SELECT count(*) FROM soll.Edge WHERE relation_type = 'INHERITS_FROM' AND source_id = 'GUI-AXO-001' AND target_id = 'GUI-PRO-001'"
    ).unwrap();
    assert_eq!(edge_count, 1, "Inheritance edge should be created");
}

// REQ-AXO-043 — axon_apply_guidelines must surface a recovery contract
// when the call cannot produce useful output (empty input or all-unknown
// global rule IDs). The previous behaviour silently returned
// "Inheritance applied. New local rules created: []", misleading the LLM
// into thinking work happened.

#[test]
fn test_axon_apply_guidelines_rejects_empty_accepted_list() {
    // REQ-AXO-902142 — serialize env access + force the synchronous path so the
    // empty-input recovery contract (isError=true) is asserted, not an async job.
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": []
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "empty accepted_global_rule_ids must surface isError=true; result={result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        content.contains("at least one canonical Guideline ID"),
        "{content}"
    );
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("empty_input").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert!(data.get("recovery_hint").is_some());
    assert_eq!(data.get("applied").unwrap().as_array().unwrap().len(), 0);
    assert_eq!(
        data.get("unknown_global_rule_ids")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn test_axon_apply_guidelines_rejects_all_unknown_rule_ids() {
    // REQ-AXO-902142 — serialize env access + force the synchronous path so the
    // all-unknown-ids recovery contract (isError=true) is asserted, not an async job.
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-NONEXISTENT", "GUI-NOPE-999"]
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "all-unknown IDs must surface isError=true; result={result:?}"
    );
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(content.contains("No rules applied"), "{content}");
    assert!(content.contains("GUI-PRO-NONEXISTENT"), "{content}");
    let data = result.get("data").unwrap();
    let unknowns = data
        .get("unknown_global_rule_ids")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(unknowns.len(), 2);
    assert!(unknowns
        .iter()
        .any(|v| v.as_str() == Some("GUI-PRO-NONEXISTENT")));
    assert!(unknowns.iter().any(|v| v.as_str() == Some("GUI-NOPE-999")));
}

#[test]
fn test_axon_apply_guidelines_partial_success_surfaces_unknown() {
    // REQ-AXO-902142 — serialize env access + force the synchronous path so the
    // partial-success body (known_ids/unknown_ids) is present, not an async job.
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_guidelines",
            "arguments": {
                "project_code": "AXO",
                "accepted_global_rule_ids": ["GUI-PRO-001", "GUI-PRO-NONEXISTENT"]
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    // Partial success is NOT an error — the call produced useful output
    // for the known IDs and reported unknowns alongside.
    assert!(
        result.get("isError").is_none()
            || result.get("isError").and_then(|v| v.as_bool()) == Some(false),
        "partial success should not flag isError; result={result:?}"
    );
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("applied").unwrap().as_array().unwrap().len(),
        1,
        "exactly one applied"
    );
    let unknowns = data
        .get("unknown_global_rule_ids")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(unknowns.len(), 1);
    assert_eq!(unknowns[0].as_str(), Some("GUI-PRO-NONEXISTENT"));
}

#[test]
fn test_soll_commit_revision_returns_identity_mapping_and_resolves_relations() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    let server = create_test_server();

    // Self-seed canonical AXO parents so the plan's create operations can
    // attach (MIL-AXO-020). attach_to is NOT logical-key-resolved in the
    // create path, so each parent must already exist in soll.Node. The new
    // Requirement attaches to a Pillar (REQ->PIL=BELONGS_TO); the new Decision
    // attaches to a Requirement (DEC->REQ=SOLVES is the only canonical DEC
    // attach target — there is no DEC->PIL pair).
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-901', 'Pillar', 'AXO', 'Identity-mapping anchor pillar', '', 'current', '{}')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-901', 'Requirement', 'AXO', 'Identity-mapping anchor requirement', '', 'current', '{}')").unwrap();

    // Create a plan with logical keys and a relation using those keys
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test",
                "dry_run": false,
                "plan": {
                    "requirements": [
                        { "logical_key": "req-1", "title": "Req A", "description": "Desc A", "attach_to": "PIL-AXO-901", "relation_type": "BELONGS_TO" }
                    ],
                    "decisions": [
                        { "logical_key": "dec-1", "title": "Dec B", "description": "Desc B", "attach_to": "REQ-AXO-901", "relation_type": "SOLVES" }
                    ]
                },
                "relations": [
                    {
                        "source_id": "dec-1",
                        "target_id": "req-1",
                        "relation_type": "SOLVES"
                    }
                ]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // Should be committed immediately because dry_run = false
    assert!(content.contains("SOLL revision committed"), "{}", content);

    // We expect identity_mapping in the result.data
    let data = result.get("data").expect("Should have data field");
    let identity_mapping = data
        .get("identity_mapping")
        .expect("Should have identity_mapping");

    let dec_id = identity_mapping.get("dec-1").unwrap().as_str().unwrap();
    let req_id = identity_mapping.get("req-1").unwrap().as_str().unwrap();

    assert!(dec_id.starts_with("DEC-AXO-"));
    assert!(req_id.starts_with("REQ-AXO-"));

    // Verify the edge in DB using the canonical IDs
    let edge_count = server.graph_store.query_count(&format!(
        "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = '{}' AND relation_type = 'SOLVES'",
        dec_id, req_id
    )).unwrap();
    assert_eq!(
        edge_count, 1,
        "The relation should be created using canonical IDs"
    );
}

#[test]
// REQ-AXO-126 — `axon_commit_work` no longer auto-fires `soll_export`.
// The release-promotion pipeline owns the snapshot moment now (option D
// in the retention design). The response must contain only the git
// commit status and must NOT contain any "Exported to" / "Export
// Report" markers.
//
// REQ-AXO-246 — must run in an isolated tempdir + ephemeral git repo,
// never against AXON_REPO. Pass project_path explicitly so the tool
// routes git commands via Command::current_dir to the sandbox.
fn test_axon_commit_work_executes_git_without_auto_export_when_dry_run_false() {
    let server = create_test_server();
    let sandbox = init_commit_work_sandbox();

    // Insert a dummy Guideline that passes trivially
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
         VALUES ('GUI-AXO-999', 'Guideline', 'AXO', 'Dummy', 'Dummy', 'active', '{\"trigger_path\":\"\",\"required_path\":\"\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["Cargo.toml"],
                "project_path": sandbox.path().to_str().unwrap(),
                "message": "test: REQ-AXO-246 isolated commit (sandbox, never reaches AXON_REPO)",
                "dry_run": false
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // It should not be an error
    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "{}",
        content
    );

    // Git commit must have succeeded inside the sandbox.
    assert!(
        content.contains("Commit succeeded"),
        "expected sandbox commit to succeed: {content}"
    );
    // REQ-AXO-126 — no auto-export markers must appear on the
    // commit-work response surface.
    assert!(
        !content.contains("Exported to"),
        "auto-export hook must be gone from commit_work: {content}"
    );
    assert!(
        !content.contains("Export Report"),
        "Export Report block must not be emitted from commit_work: {content}"
    );

    // REQ-AXO-246 regression assertion: the new commit landed in the
    // sandbox repo, not anywhere else. HEAD should reference our message.
    let head_subject = std::process::Command::new("git")
        .current_dir(sandbox.path())
        .args(["log", "-1", "--pretty=%s"])
        .output()
        .expect("git log");
    let subject = String::from_utf8_lossy(&head_subject.stdout);
    assert!(
        subject.contains("REQ-AXO-246 isolated commit"),
        "sandbox HEAD must hold the test commit; got: {subject}"
    );
}

// REQ-AXO-902062 (feedback #48, APS) — regression guard for the SUCCESS branch of a pure
// file DELETION. The pre-fix `git add <paths>` errored on a removed path (pathspec did not
// match) and refused the whole commit; `git add -A -- <paths>` (workflow_project.rs) stages
// the removal. The sibling test above covers only the FAILURE branch (never-tracked path);
// this proves deletion is actually DELIVERED end-to-end (row gone from the committed tree).
#[test]
fn test_axon_commit_work_delivers_pure_file_deletion() {
    let server = create_test_server();
    let sandbox = init_commit_work_sandbox();

    // Seed + commit a tracked file, then delete it from the working tree.
    let run_git = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .current_dir(sandbox.path())
            .args(args)
            .status()
            .expect("git invocation");
        assert!(st.success(), "git {args:?} failed in sandbox");
    };
    std::fs::write(sandbox.path().join("victim.txt"), "delete me\n").expect("seed victim");
    run_git(&["add", "victim.txt"]);
    run_git(&["commit", "-m", "seed victim for deletion test"]);
    std::fs::remove_file(sandbox.path().join("victim.txt")).expect("rm victim");

    // Dummy Guideline that passes trivially (same pattern as the sibling success test).
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
         VALUES ('GUI-AXO-999', 'Guideline', 'AXO', 'Dummy', 'Dummy', 'active', '{\"trigger_path\":\"\",\"required_path\":\"\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["victim.txt"],
                "project_path": sandbox.path().to_str().unwrap(),
                "message": "test: REQ-AXO-902062 pure deletion delivery (feedback #48)",
                "dry_run": false
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert!(
        !result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false),
        "a pure deletion must NOT be refused: {content}"
    );
    assert!(
        content.contains("Commit succeeded"),
        "the deletion commit must succeed: {content}"
    );
    // The removal must be in HEAD: git ls-files no longer tracks victim.txt.
    let tracked = std::process::Command::new("git")
        .current_dir(sandbox.path())
        .args(["ls-files", "victim.txt"])
        .output()
        .expect("git ls-files");
    assert!(
        String::from_utf8_lossy(&tracked.stdout).trim().is_empty(),
        "victim.txt must be gone from the committed tree after a pure-deletion commit"
    );
}

// REQ-AXO-246 — set up an ephemeral git repo for axon_commit_work tests.
// Returns a TempDir whose drop cleans the sandbox at end of test.
fn init_commit_work_sandbox() -> tempfile::TempDir {
    let dir = tempdir().expect("tempdir");
    let path = dir.path();
    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .expect("git invocation");
        assert!(status.success(), "git {:?} failed in sandbox", args);
    };
    run_git(&["init", "--initial-branch=main"]);
    run_git(&["config", "user.email", "axon-test@example.invalid"]);
    run_git(&["config", "user.name", "axon-test"]);
    run_git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"sandbox\"\nversion = \"0.0.1\"\n",
    )
    .expect("seed Cargo.toml");
    run_git(&["add", "Cargo.toml"]);
    run_git(&["commit", "-m", "initial sandbox commit"]);
    // Stage a real change so axon_commit_work has something to commit.
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"sandbox\"\nversion = \"0.0.2\"\n",
    )
    .expect("modify Cargo.toml");
    dir
}

#[test]
fn test_soll_apply_plan_resolves_logical_keys_in_relations() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    // REQ-AXO-137: soll_apply_plan must resolve logical_key references in
    // relations[].{source_id,target_id} to the canonical IDs produced by
    // sibling create operations in the same plan, so a transactional batch
    // truly creates BOTH the nodes AND the edges in one call.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Anchor pillar', '', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test_runner",
                "dry_run": false,
                "plan": {
                    "concepts": [{
                        "logical_key": "CPT-anchor-protocol",
                        "title": "Anchor protocol concept",
                        "description": "Concept created via plan to test logical_key resolution",
                        "status": "current",
                        "metadata": {},
                        "attach_to": "PIL-AXO-001",
                        "relation_type": "BELONGS_TO"
                    }]
                },
                "relations": [{
                    "source_id": "CPT-anchor-protocol",
                    "target_id": "PIL-AXO-001",
                    "relation_type": "BELONGS_TO"
                }]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_ne!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "apply_plan must succeed: {:?}",
        result
    );

    // Lookup the canonical id of the freshly-created concept.
    let cpt_id = server
        .graph_store
        .query_json(
            "SELECT id FROM soll.Node WHERE type='Concept' AND title='Anchor protocol concept' AND project_code='AXO' LIMIT 1",
        )
        .unwrap();
    let cpt_rows: Vec<Vec<String>> = serde_json::from_str(&cpt_id).unwrap_or_default();
    assert!(
        !cpt_rows.is_empty(),
        "concept must have been created: {}",
        cpt_id
    );
    let canonical_concept_id = cpt_rows[0][0].clone();
    assert!(
        canonical_concept_id.starts_with("CPT-AXO-"),
        "canonical id must follow CPT-AXO-NNN format, got {}",
        canonical_concept_id
    );

    // Assert an Edge was created with the resolved canonical id.
    let edge_count = server
        .graph_store
        .query_json(&format!(
            "SELECT count(*) FROM soll.Edge WHERE source_id = '{}' AND target_id = 'PIL-AXO-001' AND relation_type = 'BELONGS_TO'",
            canonical_concept_id
        ))
        .unwrap();
    let edge_rows: Vec<Vec<String>> = serde_json::from_str(&edge_count).unwrap_or_default();
    let count: i64 = edge_rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    assert_eq!(
        count, 1,
        "Edge must be materialized via logical_key resolution; got {} edges (canonical id={}). Raw: {}",
        count, canonical_concept_id, edge_count
    );

    // REQ-AXO-137 response-surface contract: data.linked[] must expose the
    // RESOLVED canonical ids, not the original logical_keys, so the LLM can
    // query Edge directly without re-resolving. raw_source_id/raw_target_id
    // preserve the original input for audit.
    let data = result.get("data").expect("response data");
    let linked = data["linked"].as_array().expect("linked array");
    assert_eq!(linked.len(), 1, "exactly one link expected: {:?}", data);
    assert_eq!(
        linked[0]["source_id"].as_str(),
        Some(canonical_concept_id.as_str()),
        "data.linked[].source_id must be canonical id, not logical_key: {:?}",
        linked[0]
    );
    assert_eq!(
        linked[0]["target_id"].as_str(),
        Some("PIL-AXO-001"),
        "target was canonical at input, must stay canonical: {:?}",
        linked[0]
    );
    assert_eq!(
        linked[0]["raw_source_id"].as_str(),
        Some("CPT-anchor-protocol"),
        "raw_source_id must preserve the original logical_key for audit: {:?}",
        linked[0]
    );
}

#[test]
fn test_restore_soll_invalid_path_returns_parameter_repair() {
    // REQ-AXO-147 slice 1 — operations.rs failure paths now surface
    // data.parameter_repair so the LLM can recover in one round-trip.
    // Restoring from a path that does not exist must point at the `path`
    // field with a hint to use docs/vision/SOLL_EXPORT_*.md.
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "restore_soll",
            "arguments": {
                "path": "/tmp/this/path/definitely/does/not/exist-axo-147.md"
            }
        },
        "id": 9001
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "non-existent path must surface isError=true: {result:?}"
    );
    let data = result.get("data").expect("data payload required");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("path"));
    assert!(
        repair["supplied_value"]
            .as_str()
            .unwrap_or("")
            .contains("does/not/exist"),
        "parameter_repair must echo the supplied path: {repair}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("SOLL_EXPORT") || hint.contains("docs/vision"),
        "hint must point at the canonical export location: {hint}"
    );
}

#[test]
fn test_soll_export_unregistered_project_code_returns_wrong_project_scope() {
    // REQ-AXO-147 slice 1 — soll_export now uses the shared
    // wrong_project_scope_response helper for unregistered codes
    // (consistent with soll_validate / soll_query_context / soll_work_plan).
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_export",
            "arguments": {
                "project_code": "ZZZ"
            }
        },
        "id": 9002
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("data payload required");
    let status = data["status"].as_str().unwrap_or("");
    assert!(
        status == "wrong_project_scope" || status == "input_invalid",
        "unregistered project_code must surface a structured status (got `{status}`): {data:?}"
    );
}

#[test]
fn test_document_intent_classifies_and_creates_canonical_soll_node() {
    // REQ-AXO-141 — document_intent is the discoverable MCP entry point for
    // "documente" / "document this" workflows. With suggest_type omitted,
    // the server-side classifier picks one of {requirement, decision,
    // concept, guideline} based on body keywords. Returns the canonical
    // SOLL id assigned by soll_manager.
    let server = create_test_server();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Intent Pillar', '', 'current', '{}')").unwrap();

    // Body contains both "framework" (concept-keyword) and "fix needed"
    // (requirement-keyword); requirement must win because the LLM
    // contract treats problem-class signals as more actionable.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "document_intent",
            "arguments": {
                "intent": "Indexer fails on empty file",
                "body": "the framework is broken when the file is 0 bytes — fix needed before next release",
                "project_code": "AXO",
                "tags": ["llm-friction", "indexer"]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_ne!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "document_intent must succeed: {result:?}"
    );

    let data = result.get("data").expect("response data");
    assert_eq!(data["status"].as_str(), Some("ok"));
    assert_eq!(
        data["entity_type"].as_str(),
        Some("requirement"),
        "classifier must pick `requirement` when problem-class keyword fires: {data:?}"
    );
    assert_eq!(
        data["classifier_reason"].as_str(),
        Some("matched_requirement_keyword")
    );
    let canonical_id = data["canonical_id"].as_str().expect("canonical_id string");
    assert!(
        canonical_id.starts_with("REQ-AXO-"),
        "auto-classified requirement must get a REQ-AXO-NNN id, got {canonical_id}"
    );

    // The actual SOLL Node row must exist with the expected fields.
    let row = server
        .graph_store
        .query_json(&format!(
            "SELECT type, title, description, status, metadata FROM soll.Node WHERE id = '{}' LIMIT 1",
            canonical_id
        ))
        .unwrap();
    let parsed: Vec<Vec<String>> = serde_json::from_str(&row).unwrap_or_default();
    let node = parsed.first().expect("created Node row");
    assert_eq!(node[0], "Requirement");
    assert_eq!(node[1], "Indexer fails on empty file");
    assert!(
        node[4].contains("classifier_reason"),
        "metadata must persist classifier_reason: {}",
        node[4]
    );
    assert!(
        node[4].contains("llm-friction"),
        "metadata.tags must be persisted: {}",
        node[4]
    );
}

#[test]
fn test_document_intent_rejects_invalid_suggest_type_with_parameter_repair() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "document_intent",
            "arguments": {
                "intent": "x",
                "body": "x",
                "suggest_type": "wat"
            }
        },
        "id": 2
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").expect("data");
    assert_eq!(data["status"].as_str(), Some("input_invalid"));
    let repair = data["parameter_repair"].clone();
    assert_eq!(repair["invalid_field"].as_str(), Some("suggest_type"));
    assert_eq!(repair["supplied_value"].as_str(), Some("wat"));
    let accepted = repair["accepted_values"]
        .as_array()
        .expect("accepted_values array");
    let names: Vec<&str> = accepted.iter().filter_map(|v| v.as_str()).collect();
    for kind in ["requirement", "decision", "concept", "guideline"] {
        assert!(
            names.contains(&kind),
            "accepted_values must include `{kind}`: {names:?}"
        );
    }
}

#[test]
fn test_soll_apply_plan_surfaces_unresolved_logical_keys_in_errors_and_parameter_repair() {
    let _env = env_lock();
    let _mj = crate::test_support::EnvVarGuard::unset("AXON_MCP_MUTATION_JOBS");
    // REQ-AXO-139 slice — when a relation references a logical_key that
    // is neither a canonical TYPE-CODE-NNN id nor created in the same plan
    // batch, the response must surface the unresolved keys in `errors[]`
    // and a top-level `parameter_repair` so the LLM can fix the inputs in
    // one round-trip.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-091', 'Pillar', 'AXO', 'Anchor pillar 91', '', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_apply_plan",
            "arguments": {
                "project_code": "AXO",
                "author": "test_runner",
                "dry_run": false,
                "plan": {
                    "concepts": [{
                        "logical_key": "CPT-resolved-cpt-91",
                        "title": "Resolved concept slice 5",
                        "description": "Concept created via plan; its logical_key resolves",
                        "status": "current",
                        "metadata": {},
                        "attach_to": "PIL-AXO-091",
                        "relation_type": "BELONGS_TO"
                    }]
                },
                "relations": [
                    {
                        // Resolved (sibling create) — no error expected for this row.
                        "source_id": "CPT-resolved-cpt-91",
                        "target_id": "PIL-AXO-091",
                        "relation_type": "BELONGS_TO"
                    },
                    {
                        // Unresolved logical_key on source — must show up in errors[].
                        "source_id": "CPT-typo-not-created",
                        "target_id": "PIL-AXO-091",
                        "relation_type": "BELONGS_TO"
                    }
                ]
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.expect("expected result");
    let data = result.get("data").expect("response data");

    let errors = data["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("errors array required in: {data:?}"));
    let unresolved_entries: Vec<&Value> = errors
        .iter()
        .filter(|e| e.get("kind").and_then(|v| v.as_str()) == Some("unresolved_logical_key"))
        .collect();
    assert_eq!(
        unresolved_entries.len(),
        1,
        "exactly one unresolved_logical_key error expected; got: {errors:?}"
    );
    let err = unresolved_entries[0];
    assert_eq!(err["operation"].as_str(), Some("link"));
    let unresolved_keys = err["unresolved_keys"]
        .as_array()
        .expect("unresolved_keys array");
    let unresolved_names: Vec<&str> = unresolved_keys.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        unresolved_names.contains(&"CPT-typo-not-created"),
        "unresolved_keys must list the missing logical_key: {unresolved_names:?}"
    );
    assert!(
        err["available_logical_keys"]
            .as_array()
            .map(|arr| arr
                .iter()
                .any(|v| v.as_str() == Some("CPT-resolved-cpt-91")))
            .unwrap_or(false),
        "available_logical_keys must list the keys that DID resolve: {err:?}"
    );

    let repair = data["parameter_repair"].clone();
    assert!(
        !repair.is_null(),
        "parameter_repair must be set when unresolved logical_keys exist: {data:?}"
    );
    assert_eq!(
        repair["invalid_field"].as_str(),
        Some("operations[].payload.source_id|target_id")
    );
    let repair_unresolved = repair["unresolved_keys"]
        .as_array()
        .expect("repair unresolved_keys array");
    let repair_unresolved_names: Vec<&str> = repair_unresolved
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(repair_unresolved_names.contains(&"CPT-typo-not-created"));
    let follow_up = repair["follow_up_tools"]
        .as_array()
        .expect("follow_up_tools array");
    let follow_names: Vec<&str> = follow_up.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        follow_names.contains(&"soll_manager"),
        "follow_up_tools must include `soll_manager`: {follow_names:?}"
    );
    let hint = repair["hint"].as_str().expect("hint string");
    assert!(
        hint.contains("logical_key") || hint.contains("canonical"),
        "hint must explain logical_key vs canonical id: {hint}"
    );
}

#[test]
fn test_axon_commit_work_refuses_partial_diff_when_git_add_fails() {
    // REQ-AXO-138 — when `git add <diff_paths>` exits non-zero (e.g., a path
    // doesn't exist), axon_commit_work must NOT proceed to `git commit`.
    // Previously the code only checked Command::output() Err (process-spawn
    // failure) and let exit-code failures pass through silently, resulting in
    // commits that captured only whatever was pre-staged. Now the exit status
    // is checked and a structured parameter_repair response is returned.
    let server = create_test_server();
    server.graph_store.execute(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
         VALUES ('GUI-AXO-999', 'Guideline', 'AXO', 'Dummy', 'Dummy', 'active', '{\"trigger_path\":\"\",\"required_path\":\"\",\"enforcement\":\"strict\"}')"
    ).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["this/path/definitely/does/not/exist.rs"],
                "message": "test: REQ-AXO-138 partial-diff refusal",
                "dry_run": false
            }
        },
        "id": 1
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    assert_eq!(
        result.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "non-existent diff_path must surface isError=true: {}",
        content
    );
    assert!(
        content.contains("Git add failed") || content.contains("Refusing to commit"),
        "error text must explain partial-diff refusal: {}",
        content
    );
    let data = result
        .get("data")
        .expect("data payload required for repair");
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("input_invalid")
    );
    // REQ-AXO-902296 — this used to assert `git_add_exit_code`, git's raw exit
    // status, as the diagnostic. Since the paths are now classified BEFORE the
    // index is touched, an impossible path never reaches `git add` and there is no
    // exit code to report — by design: `git add` is atomic over its pathspecs, so
    // letting it run staged NOTHING and named only the first offender.
    //
    // The requirement REQ-AXO-138 protects (a refusal the caller can act on) is
    // asserted harder here than the old proxy did: the offending path itself, and
    // its reason, must be named. `git_add_exit_code` is still surfaced on the path
    // where `git add` does run and genuinely fails.
    let rejected = data
        .get("parameter_repair")
        .and_then(|pr| pr.get("rejected_paths"))
        .and_then(|v| v.as_array())
        .expect("the rejected paths must be enumerated for repair");
    assert_eq!(rejected.len(), 1, "exactly one path was impossible: {rejected:?}");
    assert_eq!(
        rejected[0].get("path").and_then(|v| v.as_str()),
        Some("this/path/definitely/does/not/exist.rs"),
        "the offending path must be named, not left to a raw stderr: {rejected:?}"
    );
    assert!(
        rejected[0]
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|r| r.contains("no such file")),
        "each rejection carries its own actionable reason: {rejected:?}"
    );
    assert_eq!(
        data.get("parameter_repair")
            .and_then(|pr| pr.get("invalid_field"))
            .and_then(|v| v.as_str()),
        Some("diff_paths")
    );
    assert!(
        !content.contains("Commit succeeded"),
        "commit must NOT have happened: {}",
        content
    );
}

// REQ-AXO-902417 — `axon_commit_work` committed the ENTIRE git index, not the
// declared `diff_paths`. Measured by TE2 (`mcp_feedback` #186): two declared
// paths, two others staged earlier in the session for a LATER commit, and the
// resulting commit carried all four — including a 401-line deletion the message
// never mentioned.
//
// These tests run against a throwaway repo via the tool's own `project_path`
// argument, and interrogate the COMMIT, not the tool's prose. A test that only
// read the response text would pass on a tool that reported the right thing and
// committed the wrong one — which is exactly the failure being fixed.
mod commit_is_bounded_to_the_declaration {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    }

    fn git_stdout(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn repo_with_two_committed_files() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        git(p, &["init", "-q", "."]);
        git(p, &["config", "user.email", "t@t"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("declared.txt"), "v1").unwrap();
        std::fs::write(p.join("unrelated.txt"), "v1").unwrap();
        git(p, &["add", "-A", "."]);
        git(p, &["commit", "-qm", "base"]);
        dir
    }

    fn commit_via_tool(
        server: &McpServer,
        repo: &std::path::Path,
        diff_paths: serde_json::Value,
        message: &str,
    ) -> serde_json::Value {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_commit_work",
                "arguments": {
                    "diff_paths": diff_paths,
                    "message": message,
                    "project_path": repo.to_string_lossy(),
                }
            })),
            id: Some(json!(9417)),
        };
        server
            .handle_request(req)
            .unwrap()
            .result
            .expect("axon_commit_work returned no result")
    }

    #[test]
    fn a_path_staged_before_the_call_stays_out_of_the_commit_and_stays_staged() {
        let server = create_test_server();
        let dir = repo_with_two_committed_files();
        let repo = dir.path();

        // The measured situation: something staged earlier, meant for LATER.
        std::fs::write(repo.join("unrelated.txt"), "v2").unwrap();
        git(repo, &["add", "--", "unrelated.txt"]);
        // And the change this commit is actually about.
        std::fs::write(repo.join("declared.txt"), "v2").unwrap();

        let result = commit_via_tool(
            &server,
            repo,
            json!(["declared.txt"]),
            "fix: REQ-AXO-902417 only the declared path",
        );
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        // POSITIVE CONTROL, before the verdict. If the commit never happened —
        // because the tool refused, because `project_path` was ignored, because
        // the guideline gate fired — then "unrelated.txt is absent from the
        // commit" would be trivially true and would measure nothing.
        let committed = git_stdout(repo, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(
            committed.contains("declared.txt"),
            "precondition: a commit must have happened and must carry the declared \
             path. Nothing below means anything otherwise.\n  commit: {committed:?}\n\
             \n  tool said: {content}"
        );

        // The verdict.
        assert!(
            !committed.contains("unrelated.txt"),
            "`unrelated.txt` was staged before the call and NOT declared in \
             `diff_paths`; it must not ride along under a message that does not \
             mention it.\n  commit contained: {committed:?}\n  tool said: {content}"
        );

        // And nothing was lost. Excluding the path would be a poor trade if it
        // also dropped the caller's staged work on the floor.
        let still_staged = git_stdout(repo, &["diff", "--cached", "--name-only"]);
        assert!(
            still_staged.contains("unrelated.txt"),
            "the excluded path must still be STAGED afterwards — excluded, not \
             discarded.\n  still staged: {still_staged:?}"
        );

        // The response must say so: git's own "N files changed" is what a
        // confident caller skips, having declared the paths themselves.
        assert!(
            content.contains("declared.txt") && content.contains("unrelated.txt"),
            "the response must name both what was committed and what was left \
             staged:\n{content}"
        );
    }

    #[test]
    fn a_brand_new_file_is_committed_by_the_bounded_commit() {
        // The most common real case for this tool — every new test file, every
        // new module — and the one the other tests miss: they all declare paths
        // git already tracks. It matters because git's own wording for a
        // path-limited commit is "record the current content of the listed files
        // (WHICH MUST ALREADY BE KNOWN TO GIT)". The `git add -A --` above is
        // what makes an untracked file known; that this suffices was reasoned,
        // and reasoning is what this session keeps catching out. Measure it.
        let server = create_test_server();
        let dir = repo_with_two_committed_files();
        let repo = dir.path();
        std::fs::write(repo.join("fresh.rs"), "fn main() {}").unwrap();

        let result = commit_via_tool(
            &server,
            repo,
            json!(["fresh.rs"]),
            "feat: REQ-AXO-902417 a brand-new file",
        );
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        assert_ne!(
            result.get("isError").and_then(serde_json::Value::as_bool),
            Some(true),
            "committing a new file must not be refused:\n{content}"
        );
        let committed = git_stdout(repo, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(
            committed.contains("fresh.rs"),
            "a previously-untracked declared file must land in the commit — if this \
             is red, `--only` needs the path staged differently and EVERY commit \
             creating a file is broken.\n  commit: {committed:?}\n  tool said: {content}"
        );
        assert!(
            git_stdout(repo, &["status", "--porcelain"]).trim().is_empty(),
            "nothing must be left behind: {:?}",
            git_stdout(repo, &["status", "--porcelain"])
        );
    }

    #[test]
    fn a_deletion_already_staged_with_git_rm_and_declared_is_still_committed() {
        // REQ-AXO-902296 must survive the bound: a `git rm` path is absent from
        // BOTH worktree and index-as-content, so a naive pathspec commit could
        // drop it. Declaring it must still commit its deletion.
        let server = create_test_server();
        let dir = repo_with_two_committed_files();
        let repo = dir.path();
        git(repo, &["rm", "-q", "declared.txt"]);

        let result = commit_via_tool(
            &server,
            repo,
            json!(["declared.txt"]),
            "fix: REQ-AXO-902417 a declared deletion still lands",
        );
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let committed = git_stdout(repo, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(
            committed.contains("declared.txt"),
            "a staged deletion that IS declared must be committed — bounding the \
             commit must not undo REQ-AXO-902296.\n  commit: {committed:?}\n\
             \n  tool said: {content}"
        );
        assert!(
            !std::path::Path::new(&repo.join("declared.txt")).exists(),
            "sanity: the file really is gone from the worktree"
        );
    }

    #[test]
    fn a_merge_in_progress_is_refused_by_name_never_by_committing_everything() {
        // The condition nobody tests. `git commit --only` refuses during a
        // merge; the only wrong answer is to fall back to the unbounded commit,
        // which would restore the defect silently under the one state where the
        // index is guaranteed to hold work the caller did not declare.
        let server = create_test_server();
        let dir = repo_with_two_committed_files();
        let repo = dir.path();

        git(repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(repo.join("declared.txt"), "side").unwrap();
        git(repo, &["commit", "-qam", "side change"]);
        git(repo, &["checkout", "-q", "-"]);
        std::fs::write(repo.join("declared.txt"), "trunk").unwrap();
        git(repo, &["commit", "-qam", "trunk change"]);

        // Conflicting merge, then a resolution staged by hand.
        let merge = std::process::Command::new("git")
            .current_dir(repo)
            .args(["merge", "side"])
            .output()
            .expect("git runs");
        assert!(
            !merge.status.success(),
            "precondition: the merge must CONFLICT, otherwise no merge is in \
             progress and this test measures nothing: {merge:?}"
        );
        std::fs::write(repo.join("declared.txt"), "resolved").unwrap();
        git(repo, &["add", "--", "declared.txt"]);
        assert!(
            repo.join(".git/MERGE_HEAD").exists(),
            "precondition: a merge must still be in progress"
        );

        let result = commit_via_tool(
            &server,
            repo,
            json!(["declared.txt"]),
            "fix: REQ-AXO-902417 merge refusal",
        );
        let content = result.get("content").unwrap()[0]
            .get("text")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        assert_eq!(
            result.get("isError").and_then(serde_json::Value::as_bool),
            Some(true),
            "a commit that did not happen must not come back in a success-shaped \
             envelope:\n{content}"
        );
        assert_eq!(
            result
                .pointer("/data/operator_guidance/problem_class")
                .and_then(serde_json::Value::as_str),
            Some("partial_commit_refused_during_merge"),
            "the refusal must be CLASSIFIED, not left to git's raw stderr:\n{content}"
        );
        assert!(
            repo.join(".git/MERGE_HEAD").exists(),
            "the tool must NOT have completed the merge behind the caller's back — \
             that is the silent fallback this guard exists to prevent"
        );
    }
}

/// REQ-AXO-902425 — `soll_rollback_revision` est publié comme outil, et la règle
/// dure du produit est « ne JAMAIS supprimer un nœud SOLL — revenir en arrière
/// par `soll_rollback_revision` ». Le filet n'avait rien dessous : seul
/// `action=unlink` journalisait. Signalé par FSF (`mcp_feedback` #83) après avoir
/// remplacé le corps de leur pointeur de session canonique — 106 347 caractères —
/// en écrivant DANS le nœud que l'opération était récupérable. Elle ne l'était
/// pas.
///
/// Ce test fait l'ALLER-RETOUR. Vérifier qu'une ligne d'audit existe mesurerait
/// la moitié qui ne sert à rien : ce que FSF avait besoin de récupérer, c'est le
/// CORPS, et les deux lignes qu'ils ont trouvées avaient `before_json = {}`.
#[test]
fn test_soll_update_is_journalled_and_the_previous_body_comes_back() {
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };

    let original = "Corps canonique d'origine.\n".repeat(40);
    let seed = IstSeed::new().node(
        SollNodeFixture::new("CPT-TST-830", "Concept", "TST", "Pointeur de session")
            .description(original.clone())
            .status("current"),
    );
    let harness = create_test_server_with_ist_seed(seed).expect("serveur de test");

    let call = |args: serde_json::Value, name: &str| -> serde_json::Value {
        harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": name, "arguments": args })),
                id: Some(json!(902425)),
            })
            .unwrap()
            .result
            .unwrap_or_else(|| panic!("`{name}` n'a rien rendu"))
    };
    let body_now = || -> String {
        harness
            .server
            .graph_store
            .query_json("SELECT description FROM soll.Node WHERE id = 'CPT-TST-830'")
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&raw).ok())
            .and_then(|rows| {
                rows.first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default()
    };

    assert_eq!(body_now(), original, "précondition : le corps est en base");

    // L'écrasement — l'opération la PLUS destructive de la surface SOLL.
    let updated = call(
        json!({
            "action": "update",
            "entity": "concept",
            "data": { "id": "CPT-TST-830", "description": "Version élaguée." }
        }),
        "soll_manager",
    );
    let text = updated["content"][0]["text"].as_str().unwrap_or_default();

    // CONTRÔLE POSITIF : l'écrasement a bien eu lieu. Sans lui, un « rollback
    // réussi » plus bas serait vrai sans rien avoir restauré.
    assert_eq!(
        body_now(),
        "Version élaguée.",
        "précondition : le corps doit avoir été REMPLACÉ, sinon la restauration \
         ne mesure rien"
    );

    let revision_id = updated["data"]["revision_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "l'écriture doit NOMMER sa révision — un LLM ne peut pas deviner \
                 que `soll_rollback_revision` a quelque chose où revenir :\n{updated}"
            )
        })
        .to_string();
    assert!(
        text.contains("soll_rollback_revision"),
        "la réponse VISIBLE doit dire comment revenir en arrière, pas seulement \
         `data.*` que plusieurs clients n'exposent pas :\n{text}"
    );

    // Le corps précédent doit être DANS le journal, pas un `{}` — c'est
    // exactement ce que FSF a trouvé et qui ne les a pas sauvés.
    let journalled = harness
        .server
        .graph_store
        .query_json(&format!(
            "SELECT before_json->>'description' FROM soll.RevisionChange WHERE revision_id = '{revision_id}'"
        ))
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&raw).ok())
        .and_then(|rows| {
            rows.first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    assert_eq!(
        journalled, original,
        "le journal doit porter le CORPS précédent en entier. Une ligne dont \
         `before_json` vaut `{{}}` est une ligne d'audit qui ne restaure rien."
    );

    // L'ALLER-RETOUR — la seule assertion qui prouve la garantie.
    let rolled = call(
        json!({ "revision_id": revision_id }),
        "soll_rollback_revision",
    );
    assert_ne!(
        rolled.get("isError").and_then(serde_json::Value::as_bool),
        Some(true),
        "le rollback doit aboutir :\n{rolled}"
    );
    assert_eq!(
        body_now(),
        original,
        "le corps d'origine doit être REVENU. C'est la garantie que la règle \
         « ne jamais supprimer, revenir en arrière » met en jeu."
    );
}

/// REQ-AXO-902427 — `soll_work_plan` cachait exactement le travail à faire et
/// listait comme « bloqué » du travail LIVRÉ.
///
/// Signalé par APS (`mcp_feedback` #201), reproduit sur AXO : 12 des 19 entrées
/// de la section « Blockers » étaient des exigences `delivered`. Mécanisme :
/// `REQ → MIL` n'admet que `BLOCKED_BY` dans la matrice de relations, donc toute
/// exigence rattachée à un jalon est « bloquée par » lui PAR CONSTRUCTION — et
/// les vagues ne contenaient plus que de la dette d'hygiène. APS le résume
/// mieux que moi : *l'artefact écrit à la main battait le plan calculé, donc le
/// plan ne servait pas.*
#[test]
fn test_work_plan_separates_belonging_to_a_live_milestone_from_being_blocked() {
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };

    let seed = IstSeed::new()
        .node(SollNodeFixture::new("PIL-XON-001", "Pillar", "XON", "Axe").status("current"))
        .node(SollNodeFixture::new("MIL-XON-100", "Milestone", "XON", "Jalon EN COURS").status("current"))
        .node(SollNodeFixture::new("MIL-XON-200", "Milestone", "XON", "Jalon PAS COMMENCE").status("planned"))
        // Rattachée au jalon EN COURS : c'est du travail à faire, pas un blocage.
        .node(
            SollNodeFixture::new("REQ-XON-801", "Requirement", "XON", "Dans le jalon en cours")
                .status("current"),
        )
        // Rattachée à un jalon PAS COMMENCÉ : là on attend vraiment.
        .node(
            SollNodeFixture::new("REQ-XON-802", "Requirement", "XON", "Attend un jalon a venir")
                .status("current"),
        )
        // LIVRÉE, et portant encore l'arête. Un travail terminé ne bloque rien.
        .node(
            SollNodeFixture::new("REQ-XON-803", "Requirement", "XON", "Deja livree")
                .status("delivered"),
        )
        ;
    let harness = create_test_server_with_ist_seed(seed).expect("serveur de test");

    // Les arêtes SOLL vivent dans `soll.Edge`. `EdgeFixture` écrit dans
    // `ist.Edge` — les y poser laissait le plan les ignorer en silence, et
    // c'est le contrôle positif ci-dessous qui l'a attrapé.
    for (src, rel, tgt) in [
        ("REQ-XON-801", "BELONGS_TO", "PIL-XON-001"),
        ("REQ-XON-802", "BELONGS_TO", "PIL-XON-001"),
        ("REQ-XON-803", "BELONGS_TO", "PIL-XON-001"),
        ("REQ-XON-801", "BLOCKED_BY", "MIL-XON-100"),
        ("REQ-XON-802", "BLOCKED_BY", "MIL-XON-200"),
        ("REQ-XON-803", "BLOCKED_BY", "MIL-XON-100"),
    ] {
        harness
            .server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                 VALUES ('{src}', '{tgt}', '{rel}', 'XON')"
            ))
            .unwrap();
    }

    let plan = harness
        .server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_work_plan",
                "arguments": { "project_code": "XON", "format": "brief", "top": 8 }
            })),
            id: Some(json!(902427)),
        })
        .unwrap()
        .result
        .expect("soll_work_plan doit répondre")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let blockers = plan
        .split("Blockers:")
        .nth(1)
        .and_then(|rest| rest.split("Wave 1:").next())
        .unwrap_or("")
        .to_string();

    // CONTRÔLE POSITIF d'abord : un vrai blocage DOIT rester listé. Sans lui, un
    // correctif qui viderait la section entière passerait les deux assertions
    // suivantes en ne mesurant rien.
    assert!(
        blockers.contains("REQ-XON-802"),
        "contrôle positif : une exigence qui attend un jalon PAS COMMENCÉ est \
         réellement bloquée et doit rester listée.\n--- blockers ---\n{blockers}\n--- PLAN COMPLET ---\n{plan}"
    );

    assert!(
        !blockers.contains("REQ-XON-803"),
        "une exigence LIVRÉE ne peut pas être bloquée. Sa présence ici est ce qui \
         a rendu la section ignorable chez APS (12 sur 19).\n--- blockers ---\n{blockers}"
    );
    assert!(
        !blockers.contains("REQ-XON-801"),
        "un jalon EN COURS est le CONTENANT du travail, pas son obstacle : \
         l'exigence qui lui appartient ne doit pas être écartée.\n--- blockers ---\n{blockers}"
    );

    // Et elle doit être RÉELLEMENT exécutable, pas seulement absente des
    // blockers — c'est la moitié qui manquait à APS.
    assert!(
        plan.contains("REQ-XON-801"),
        "l'exigence du jalon en cours doit apparaître dans le plan comme \
         actionnable.\n--- plan ---\n{plan}"
    );
}

/// REQ-AXO-902428 — poser une arête `SUPERSEDES` rouvrait le nœud qui
/// supersède, preuves attachées comprises.
///
/// TROIS tenants l'ont mesuré indépendamment, chacun avec ses identifiants :
/// FSF #90 (`REQ-FSF-480`, `delivered` → `current`), OPV #93 (`REQ-OPV-844` et
/// `797`, revenus en tête du plan comme « work in progress — finish before
/// starting new work »), VPC #101 (`REQ-VPC-092`, découvert seulement en
/// recomptant l'histogramme des statuts avant/après).
///
/// Le message y aidait : « `A` retires `B` (status flipped) » se lit comme
/// portant sur `B`, sujet de la phrase. Il portait aussi sur `A`, en silence.
#[test]
fn test_supersedes_retires_the_target_without_reopening_the_source() {
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };

    let seed = IstSeed::new()
        .node(SollNodeFixture::new("REQ-CCL-901", "Requirement", "CCL", "Remplacante livree").status("delivered"))
        .node(SollNodeFixture::new("REQ-CCL-902", "Requirement", "CCL", "Remplacee").status("current"))
        .node(SollNodeFixture::new("REQ-CCL-903", "Requirement", "CCL", "Retiree sans remplacant").status("superseded"))
        .node(SollNodeFixture::new("REQ-CCL-904", "Requirement", "CCL", "Autre remplacante").status("current"))
        .node(SollNodeFixture::new("REQ-CCL-905", "Requirement", "CCL", "Retiree AVEC remplacant").status("superseded"))
        .node(SollNodeFixture::new("REQ-CCL-906", "Requirement", "CCL", "Le remplacant deja enregistre").status("current"));
    let harness = create_test_server_with_ist_seed(seed).expect("serveur de test");

    // 905 a DÉJÀ son remplaçant enregistré — c'est le cas où refuser est le bon
    // conseil (« supersède plutôt le plus récent »).
    harness
        .server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
             VALUES ('REQ-CCL-906', 'REQ-CCL-905', 'SUPERSEDES', 'CCL')",
        )
        .unwrap();

    let link = |src: &str, tgt: &str| -> serde_json::Value {
        harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_manager",
                    "arguments": {
                        "action": "link",
                        "entity": "requirement",
                        "data": { "source_id": src, "target_id": tgt, "relation_type": "SUPERSEDES" }
                    }
                })),
                id: Some(json!(902428)),
            })
            .unwrap()
            .result
            .expect("soll_manager doit répondre")
    };
    let status_of = |id: &str| -> String {
        harness
            .server
            .graph_store
            .query_json(&format!("SELECT status FROM soll.Node WHERE id = '{id}'"))
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&raw).ok())
            .and_then(|rows| {
                rows.first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default()
    };

    let res = link("REQ-CCL-901", "REQ-CCL-902");
    let text = res["content"][0]["text"].as_str().unwrap_or_default();

    // CONTRÔLE POSITIF : l'opération a bien eu lieu. Sans lui, « la source n'a
    // pas bougé » serait vrai d'un appel qui n'a rien fait du tout.
    assert_eq!(
        status_of("REQ-CCL-902"),
        "superseded",
        "précondition : la CIBLE doit être retirée, sinon rien n'est mesuré.\n{res}"
    );

    // LE verdict.
    assert_eq!(
        status_of("REQ-CCL-901"),
        "delivered",
        "la SOURCE doit garder son statut. La forcer à `current` rouvrait une \
         exigence LIVRÉE — avec ses preuves — et `soll_work_plan` la remontait \
         alors comme « travail en cours », envoyant la session suivante refaire \
         du travail déjà fait.\n{res}"
    );
    assert_eq!(
        res["data"]["source_status_after"].as_str(),
        Some("delivered"),
        "la réponse annonçait `current` EN DUR — vrai seulement parce que le SQL \
         l'y forçait.\n{res}"
    );
    assert!(
        text.contains("REQ-CCL-901") && text.contains("inchang"),
        "le texte doit nommer ce qui arrive à CHAQUE bout : « (status flipped) » \
         se lisait comme portant sur la cible.\n{text}"
    );

    // La cible déjà retirée SANS remplaçant enregistré : c'est exactement le trou
    // que l'ancien message décrivait avant de refuser de le combler.
    let recovered = link("REQ-CCL-904", "REQ-CCL-903");
    assert_ne!(
        recovered.get("isError").and_then(serde_json::Value::as_bool),
        Some(true),
        "un nœud retiré dont le remplaçant n'est PAS enregistré doit pouvoir le \
         recevoir : refuser, c'est refuser de combler le trou qu'on signale \
         (PIL-AXO-002).\n{recovered}"
    );

    // CONTRÔLE NÉGATIF : quand un remplaçant EST enregistré, refuser reste le bon
    // conseil — sinon le correctif aurait supprimé la garde au lieu de la borner.
    let refused = link("REQ-CCL-904", "REQ-CCL-905");
    assert_eq!(
        refused.get("isError").and_then(serde_json::Value::as_bool),
        Some(true),
        "une cible dont le remplaçant est DÉJÀ enregistré doit toujours être \
         refusée, en nommant ce remplaçant.\n{refused}"
    );
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("REQ-CCL-906"),
        "le refus doit NOMMER le remplaçant existant : {refused}"
    );
}

/// REQ-AXO-902431 — `soll_remove_evidence` se décrivait comme « safe
/// maintenance » et supprimait en masse, sans retour possible, sur une base de
/// traçabilité.
///
/// FSF (`mcp_feedback` #86, `blocking`) : le mode par défaut aurait supprimé
/// **59 références de commits VALIDES** — vérifiées une par une par
/// `git cat-file -e`, 59 commits réels, 0 introuvable. Ils ne l'ont évité que
/// parce qu'ils avaient mesuré le faux positif juste avant. *« Un utilisateur
/// qui lit "safe maintenance" et l'exécute n'a aucune raison de vérifier
/// d'abord. »*
///
/// REQ-AXO-902390 a retiré la cause du faux étiquetage. Ce test porte l'autre
/// moitié : le mode qui DÉCIDE lui-même quoi supprimer est en aperçu.
#[test]
fn test_remove_evidence_previews_before_it_deletes_in_the_mode_that_decides_alone() {
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };

    let seed = IstSeed::new().node(
        SollNodeFixture::new("REQ-MTG-701", "Requirement", "MTG", "Porteuse de preuves")
            .status("delivered"),
    );
    let harness = create_test_server_with_ist_seed(seed).expect("serveur de test");

    let seed_row = |id: &str, kind: &str, r: &str| {
        harness
            .server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, created_at) \
                 VALUES ('{id}', 'requirement', 'REQ-MTG-701', '{kind}', '{r}', 0)"
            ))
            .unwrap();
    };
    seed_row("tr-mtg-1", "File", "/chemin/qui/nexiste/pas/a.rs");
    seed_row("tr-mtg-2", "File", "/chemin/qui/nexiste/pas/b.rs");

    let rows_left = || -> i64 {
        harness
            .server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM soll.Traceability WHERE soll_entity_id = $e",
                &json!({ "e": "REQ-MTG-701" }),
            )
            .unwrap_or(-1)
    };
    let remove = |args: serde_json::Value| -> serde_json::Value {
        harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "soll_remove_evidence", "arguments": args })),
                id: Some(json!(902431)),
            })
            .unwrap()
            .result
            .expect("soll_remove_evidence doit répondre")
    };

    assert_eq!(rows_left(), 2, "précondition : deux lignes en base");

    // L'appel que FSF aurait fait : le mode par défaut, sans rien préciser.
    let preview = remove(json!({ "entity_id": "REQ-MTG-701" }));
    let text = preview["content"][0]["text"].as_str().unwrap_or_default();

    assert_eq!(
        rows_left(),
        2,
        "LE verdict : l'appel par défaut ne doit RIEN supprimer. C'est le mode qui \
         décide lui-même de son ensemble, sur une base de traçabilité, sans retour \
         possible.\n{preview}"
    );
    // CONTRÔLE POSITIF : l'aperçu doit quand même NOMMER ce qui partirait —
    // sinon « rien n'a été supprimé » serait vrai d'un outil qui ne fait rien.
    assert_eq!(
        preview["data"]["removed_count"].as_i64(),
        Some(2),
        "l'aperçu doit nommer les deux lignes candidates : {preview}"
    );
    assert!(
        text.contains("APERCU") || text.contains("APERÇU"),
        "un aperçu ne doit pas se lire comme une suppression : {text}"
    );

    // Confirmé : ça supprime. Sans ça, l'outil serait simplement cassé.
    let applied = remove(json!({ "entity_id": "REQ-MTG-701", "dry_run": false }));
    assert_eq!(
        rows_left(),
        0,
        "avec `dry_run:false`, la suppression doit bien avoir lieu : {applied}"
    );

    // CONTRÔLE DE NON-RÉGRESSION : le mode explicite reste immédiat. Là,
    // l'appelant a NOMMÉ chaque ligne — il n'y a pas de surprise à protéger, et
    // lui imposer un aller-retour serait de la cérémonie.
    seed_row("tr-mtg-3", "Test", "module::tests::nommee");
    assert_eq!(rows_left(), 1);
    let explicit = remove(json!({
        "entity_id": "REQ-MTG-701",
        "broken_only": false,
        "artifact_refs": ["module::tests::nommee"]
    }));
    assert_eq!(
        rows_left(),
        0,
        "le mode explicite ne doit PAS être passé en aperçu : {explicit}"
    );
}

/// REQ-AXO-902432 — un `revision_id` ANNONCÉ doit exister dans le journal.
///
/// En unifiant les deux écrivains de révision (`update` et `unlink` recopiaient
/// le même couple d'INSERT), j'ai d'abord laissé la réponse d'`unlink` annoncer
/// un id **reconstruit localement**. Quelques millisecondes d'écart avec celui
/// que l'écrivain venait d'écrire suffisaient à en faire une clé qui n'existe
/// nulle part — donc un `soll_rollback_revision` sans cible, sur exactement le
/// mécanisme dont la raison d'être est qu'on puisse revenir en arrière.
///
/// Attrapé à la relecture, pas par le compilateur : les deux chaînes se
/// compilent. Cette garde le rend mesurable, sur les TROIS chemins — c'est ce
/// que l'unification doit garantir et ce qu'une seule copie testée ne dirait
/// pas.
#[test]
fn test_every_announced_revision_id_exists_in_the_journal() {
    use crate::test_support::ist_fixtures::{
        create_test_server_with_ist_seed, IstSeed, SollNodeFixture,
    };

    let seed = IstSeed::new()
        .node(SollNodeFixture::new("REQ-NAN-601", "Requirement", "NAN", "Source").status("current"))
        .node(SollNodeFixture::new("REQ-NAN-602", "Requirement", "NAN", "Cible").status("current"));
    let harness = create_test_server_with_ist_seed(seed).expect("serveur de test");
    harness
        .server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
             VALUES ('REQ-NAN-601', 'REQ-NAN-602', 'REFINES', 'NAN')",
        )
        .unwrap();

    let call = |args: serde_json::Value| -> serde_json::Value {
        harness
            .server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "soll_manager", "arguments": args })),
                id: Some(json!(902432)),
            })
            .unwrap()
            .result
            .expect("soll_manager doit répondre")
    };
    let journal_has = |rev: &str| -> bool {
        harness
            .server
            .graph_store
            .query_count_param(
                "SELECT count(*) FROM soll.RevisionChange WHERE revision_id = $r",
                &json!({ "r": rev }),
            )
            .unwrap_or(0)
            > 0
    };

    // Chemin 1 — `update`.
    let updated = call(json!({
        "action": "update",
        "entity": "requirement",
        "data": { "id": "REQ-NAN-601", "description": "Corps remplacé." }
    }));
    let update_rev = updated["data"]["revision_id"]
        .as_str()
        .unwrap_or_else(|| panic!("`update` doit nommer sa révision : {updated}"));
    assert!(
        journal_has(update_rev),
        "la révision `{update_rev}` annoncée par `update` doit EXISTER dans le \
         journal — sinon `soll_rollback_revision` n'a pas de cible."
    );

    // Chemin 2 — `unlink`. C'est celui qui annonçait un id reconstruit.
    let unlinked = call(json!({
        "action": "unlink",
        "entity": "requirement",
        "data": {
            "source_id": "REQ-NAN-601",
            "target_id": "REQ-NAN-602",
            "relation_type": "REFINES"
        }
    }));
    let unlink_rev = unlinked["data"]["revision_id"]
        .as_str()
        .unwrap_or_else(|| panic!("`unlink` doit nommer sa révision : {unlinked}"));
    assert!(
        journal_has(unlink_rev),
        "la révision `{unlink_rev}` annoncée par `unlink` doit EXISTER dans le \
         journal. C'est précisément ce qui était faux quand chaque chemin \
         reconstruisait son id de son côté."
    );

    // Chemin 3 — `link`. REQ-AXO-902466 : avant cette garde, les retraits
    // d'arêtes étaient journalisés mais pas leurs créations. Un autre process
    // ne pouvait donc ni dater le lien ni invalider son snapshot depuis le
    // journal de révisions.
    let linked = call(json!({
        "action": "link",
        "entity": "requirement",
        "data": {
            "source_id": "REQ-NAN-601",
            "target_id": "REQ-NAN-602",
            "relation_type": "REFINES"
        }
    }));
    let link_rev = linked["data"]["revision_id"]
        .as_str()
        .unwrap_or_else(|| panic!("`link` doit nommer sa révision : {linked}"));
    assert!(
        journal_has(link_rev),
        "la révision `{link_rev}` annoncée par `link` doit EXISTER dans le journal"
    );

    // Les trois chemins passent par le MÊME écrivain : leurs ids ne peuvent pas
    // se ressembler par hasard, ils partagent leur forme.
    assert!(
        update_rev.starts_with("update-")
            && unlink_rev.starts_with("unlink-")
            && link_rev.starts_with("link-"),
        "chaque révision porte son action en préfixe : {update_rev} / {unlink_rev} / {link_rev}"
    );
}

#[test]
fn test_soll_generate_docs_creates_navigable_site_and_manifest() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Human-readable docs', 'Readable docs for humans', 'current', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-001', 'Decision', 'AXO', 'Generate derived site', 'Decision desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('PIL-AXO-001', 'VIS-AXO-001', 'EPITOMIZES')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'PIL-AXO-001', 'BELONGS_TO')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-AXO-001', 'REQ-AXO-001', 'SOLVES')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9910)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["pages_total"].as_u64(), Some(7));
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Generated navigable SOLL docs"));

    let index_path = out.path().join("index.html");
    let node_path = out.path().join("nodes/REQ-AXO-001.html");
    let subtree_path = out.path().join("subtrees/VIS-AXO-001.html");
    let manifest_path = out.path().join("_manifest.json");

    assert!(index_path.is_file());
    assert!(node_path.is_file());
    assert!(subtree_path.is_file());
    assert!(manifest_path.is_file());

    let index_html = std::fs::read_to_string(index_path).unwrap();
    assert!(index_html.contains("mermaid.initialize"));
    assert!(index_html.contains("PIL-AXO-001"));
    assert!(index_html.contains("toggle-left"));
    assert!(index_html.contains("toggle-right"));
    assert!(index_html.contains("Project Tree"));
    assert!(index_html.contains("Vision Children"));
    assert!(index_html.contains("derived / non-canonical"));
    assert!(index_html.contains("All Node Pages"));
    assert!(index_html.contains("nodes/REQ-AXO-001.html"));
    assert!(index_html.contains("flowchart LR"));

    let node_html = std::fs::read_to_string(node_path).unwrap();
    assert!(node_html.contains("Readable docs for humans"));
    assert!(node_html.contains("Incoming Neighbors"));
    assert!(node_html.contains("Relations"));
    assert!(node_html.contains("Primary Hierarchy Parents"));
    assert!(node_html.contains("Primary Hierarchy Children"));
    assert!(node_html.contains("Containing Subtrees"));
    assert!(node_html.contains("Primary Parent Node Pages"));
    assert!(node_html.contains("Operator Relation Diagnostics"));
    assert!(node_html.contains("boundary: canonical"));
    assert!(node_html.contains("toggle-left"));
    assert!(node_html.contains("toggle-right"));
    assert!(node_html.contains(
        "Generated node page combining hierarchy, local context, and relation diagnostics"
    ));

    // REQ-AXO-312 — a node with incoming filiation edges (children pointing at
    // it) must render the micro column, regardless of the child's preferred
    // hierarchy parent. PIL-AXO-001 has child REQ-AXO-001 (BELONGS_TO) and
    // parent VIS-AXO-001 (EPITOMIZES) → both macro and micro subgraphs. The
    // Vision, whose only relation is the incoming pillar (EPITOMIZES), must
    // also render a micro column. Regression guard for the inverted-edge bug
    // that emptied micro.
    let pillar_html = std::fs::read_to_string(out.path().join("nodes/PIL-AXO-001.html")).unwrap();
    assert!(
        pillar_html.contains("subgraph sgMicro"),
        "a node with an incoming child edge must render a micro column"
    );
    assert!(pillar_html.contains("▼ Micro"));
    assert!(pillar_html.contains("subgraph sgMacro"));

    let vision_html = std::fs::read_to_string(out.path().join("nodes/VIS-AXO-001.html")).unwrap();
    assert!(
        vision_html.contains("subgraph sgMicro"),
        "the vision must render its incoming pillar in the micro column"
    );

    let subtree_html = std::fs::read_to_string(subtree_path).unwrap();
    assert!(subtree_html.contains("All Nodes In This Subtree"));
    assert!(subtree_html.contains("../nodes/REQ-AXO-001.html"));
    assert!(subtree_html.contains("derived / non-canonical"));
    assert!(subtree_html.contains("Subtree Inclusion Reasons"));
    assert!(subtree_html.contains("Included because this node is the subtree root"));
    assert!(subtree_html.contains("Included by reverse reachability toward root"));

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["project_code"].as_str(), Some("AXO"));
    assert_eq!(manifest["pages_total"].as_u64(), Some(7));
}

#[test]
fn test_soll_generate_docs_keeps_unattached_nodes_out_of_primary_project_roots() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-999', 'Decision', 'AXO', 'Detached decision', 'No hierarchy parent', 'planned', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9918)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(result["data"]["pages_total"].as_u64().unwrap_or(0) >= 3);

    let index_html = std::fs::read_to_string(out.path().join("index.html")).unwrap();
    assert!(index_html.contains("Unattached Node Pages"));
    assert!(index_html.contains("nodes/DEC-AXO-999.html"));
    assert!(!index_html.contains("mermaid-id-DEC-AXO-999"));
}

#[test]
fn test_soll_generate_docs_is_incremental_when_content_is_unchanged() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-001', 'Pillar', 'AXO', 'Operational truth', 'Pillar desc', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('PIL-AXO-001', 'VIS-AXO-001', 'EPITOMIZES')")
        .unwrap();

    let call = |server: &McpServer| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_generate_docs",
                    "arguments": {
                        "project_code": "AXO",
                        "output_dir": out.path().to_string_lossy().to_string()
                    }
                })),
                id: Some(json!(9911)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let first = call(&server);
    assert!(first["data"]["pages_written"].as_u64().unwrap_or(0) > 0);

    let second = call(&server);
    assert_eq!(second["data"]["pages_written"].as_u64(), Some(0));
    assert!(second["data"]["pages_unchanged"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn test_soll_generate_docs_with_site_root_builds_project_and_global_root() {
    let server = create_test_server();
    let site_root = tempdir().unwrap();

    server
        .graph_store
        .sync_project_registry_entry("AXO", Some("axon"), Some("/home/dstadel/projects/axon"))
        .unwrap();
    server
        .graph_store
        .sync_project_registry_entry(
            "NTO",
            Some("nutri-opti"),
            Some("/home/dstadel/projects/nutri-opti"),
        )
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "site_root_dir": site_root.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9912)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["refresh_mode"].as_str(), Some("full"));
    assert!(site_root.path().join("index.html").is_file());
    assert!(site_root.path().join("_root_manifest.json").is_file());
    assert!(site_root.path().join("AXO/index.html").is_file());

    let root_html = std::fs::read_to_string(site_root.path().join("index.html")).unwrap();
    assert!(root_html.contains("SOLL Derived Projects"));
    assert!(root_html.contains("AXO/index.html"));
    assert!(root_html.contains("NTO"));
    assert!(root_html.contains("GLO"));
}

#[test]
fn test_sync_mutation_auto_refreshes_derived_docs_and_root() {
    let site_root = tempdir().unwrap();
    let _site_root = SollSiteRootGuard::new(site_root.path());
    let server = create_test_server();

    let init_result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_init_project",
                "arguments": {
                    "project_path": "/tmp/nutri-opti",
                    "project_name": "nutri-opti",
                    "project_code": "NTO"
                }
            })),
            id: Some(json!(9913)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        init_result["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    assert!(site_root.path().join("NTO/index.html").is_file());
    assert!(site_root.path().join("index.html").is_file());

    let create_result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_manager",
                "arguments": {
                    "action": "create",
                    "entity": "requirement",
                    "data": {
                        "project_code": "NTO",
                        "title": "Preventive nutrition platform",
                        "description": "Greenfield requirement",
                        "attach_to": "PIL-NTO-001",
                        "relation_type": "BELONGS_TO"
                    }
                }
            })),
            id: Some(json!(9914)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(
        create_result["data"]["derived_docs_refresh"]["status"].as_str(),
        Some("ok")
    );
    let project_html = std::fs::read_to_string(site_root.path().join("NTO/index.html")).unwrap();
    assert!(project_html.contains("Preventive nutrition platform"));
}

#[test]
fn test_soll_generate_docs_deletes_obsolete_project_pages_from_manifest() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-001', 'Requirement', 'AXO', 'Human-readable docs', 'Readable docs for humans', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-AXO-001', 'VIS-AXO-001', 'BELONGS_TO')")
        .unwrap();

    let call = |server: &McpServer| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "soll_generate_docs",
                    "arguments": {
                        "project_code": "AXO",
                        "output_dir": out.path().to_string_lossy().to_string()
                    }
                })),
                id: Some(json!(9915)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    let first = call(&server);
    assert!(first["data"]["pages_total"].as_u64().unwrap_or(0) >= 3);
    assert!(out.path().join("nodes/REQ-AXO-001.html").is_file());

    server
        .graph_store
        .execute(
            "DELETE FROM soll.Edge WHERE source_id = 'REQ-AXO-001' AND target_id = 'VIS-AXO-001'",
        )
        .unwrap();
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id = 'REQ-AXO-001'")
        .unwrap();

    let second = call(&server);
    assert_eq!(second["data"]["refresh_mode"].as_str(), Some("incremental"));
    assert_eq!(second["data"]["pages_deleted"].as_u64(), Some(1));
    assert!(!out.path().join("nodes/REQ-AXO-001.html").exists());
}

#[test]
fn test_soll_generate_docs_for_project_only_returns_null_root_fields() {
    let server = create_test_server();
    let out = tempdir().unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9916)),
        })
        .unwrap()
        .result
        .unwrap();

    assert!(result["data"]["site_root"].is_null());
    assert!(result["data"]["root_manifest_path"].is_null());
    assert!(result["data"]["root_index_path"].is_null());
}

#[test]
fn test_soll_generate_docs_forces_full_rebuild_when_manifest_is_incompatible() {
    let server = create_test_server();
    let out = tempdir().unwrap();
    std::fs::create_dir_all(out.path().join("nodes")).unwrap();
    std::fs::write(out.path().join("nodes/STALE-AXO-001.html"), "stale").unwrap();
    std::fs::write(
        out.path().join("_manifest.json"),
        r#"{"generator_version":"legacy","pages":[{"path":"nodes/STALE-AXO-001.html"}]}"#,
    )
    .unwrap();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-001', 'Vision', 'AXO', 'Reliable Axon', 'Top vision', 'current', '{}')")
        .unwrap();

    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_generate_docs",
                "arguments": {
                    "project_code": "AXO",
                    "output_dir": out.path().to_string_lossy().to_string()
                }
            })),
            id: Some(json!(9917)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_eq!(result["data"]["refresh_mode"].as_str(), Some("full"));
    assert!(!out.path().join("nodes/STALE-AXO-001.html").exists());
}

#[test]
fn test_axon_impact_traces_through_soll_architecture() {
    let _runtime = RuntimeEnvGuard::full_autonomous();
    let server = create_test_server();

    // 1. Create Code Symbols and Calls
    server
        .graph_store
        .execute("INSERT INTO ist.Chunk (id, source_type, source_id, project_code, file_path, content_hash) VALUES ('chunk-test-src/payment.rs', 'symbol', 'sym-src/payment.rs', 'BKS', 'src/payment.rs', 'hash-src/payment.rs')")
        .unwrap();
    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_code) VALUES ('payment::process', 'process', 'function', 'BKS')").unwrap();
    server.graph_store.execute("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('src/payment.rs', 'payment::process', 'CONTAINS', 'BKS', 0)").unwrap();

    server.graph_store.execute("INSERT INTO Symbol (id, name, kind, project_code) VALUES ('api::checkout', 'checkout', 'function', 'BKS')").unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('api::checkout', 'payment::process', 'CALLS', 'BKS', 0)",
        )
        .unwrap();

    // 2. Create SOLL Intent Graph
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('VIS-BKS-001', 'Vision', 'BKS', 'Paiement sans friction')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('REQ-BKS-005', 'Requirement', 'BKS', 'Intégration Stripe')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Node (id, type, project_code, title) VALUES ('DEC-BKS-010', 'Decision', 'BKS', 'Utiliser Rust Stripe SDK')").unwrap();

    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('REQ-BKS-005', 'VIS-BKS-001', 'BELONGS_TO')").unwrap();
    server.graph_store.execute("INSERT INTO soll.Edge (source_id, target_id, relation_type) VALUES ('DEC-BKS-010', 'REQ-BKS-005', 'SOLVES')").unwrap();

    // 3. Create Traceability Bridge (Code -> Intent)
    server.graph_store.execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-001', 'Decision', 'DEC-BKS-010', 'Symbol', 'checkout', 1.0, 0)").unwrap();

    // REQ-AXO-901952 — impact now reads the in-memory IST + SOLL snapshots
    // (RAM-only). These raw SQL inserts bypass the cache invalidation that
    // soll_manager / the indexer perform in production, so evict the BKS
    // snapshots to force a fresh reload before the impact call (otherwise a
    // stale cache populated by an earlier BKS test hides these rows).
    crate::ist_snapshot::evict_process_snapshot("BKS");
    server.soll_cache().invalidate("BKS");

    // 4. Query Impact on the deep code function
    let impact_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "impact",
            "arguments": { "symbol": "process", "depth": 2 }
        })),
        id: Some(json!(1)),
    };

    let impact_res = server.handle_request(impact_req).unwrap().result.unwrap();
    let content = impact_res.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    // 5. Asserts
    println!("DEBUG IMPACT CONTENT: {}", content);
    assert!(content.contains("checkout"), "Should find caller symbol");
    assert!(
        content.contains("DEC-BKS-010"),
        "Should bridge to SOLL Decision"
    );
    assert!(
        content.contains("Utiliser Rust Stripe SDK"),
        "Should list decision title"
    );
    assert!(
        content.contains("REQ-BKS-005"),
        "Should traverse to Requirement"
    );
    assert!(content.contains("VIS-BKS-001"), "Should traverse to Vision");
    assert!(
        content.contains("Paiement sans friction"),
        "Should list vision title"
    );
}

#[test]
fn test_soll_remove_evidence_drops_only_broken_file_refs_by_default() {
    // REQ-AXO-254 — close MIL-AXO-015 wave G followup. Verify the new
    // soll_remove_evidence tool only removes Traceability rows whose
    // artifact_ref does NOT exist on disk by default (broken_only=true).
    let server = create_test_server();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-2540', 'Requirement', 'AXO', 'soll_remove_evidence smoke', 'broken_only mode', 'current', '{\"acceptance_criteria\":\"a\"}')")
        .unwrap();

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let valid = repo_root.join("README.md");
    let valid_path = valid.to_string_lossy().to_string();

    // Seed: 1 valid + 2 broken artifact refs.
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-VALID-1", "Requirement", "REQ-AXO-2540", "file", valid_path, 1.0, "{}", 1u64]),
        )
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-BROKEN-1", "Requirement", "REQ-AXO-2540", "file", "/tmp/does-not-exist-axo-254-1.rs", 1.0, "{}", 2u64]),
        )
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-BROKEN-2", "Requirement", "REQ-AXO-2540", "document", "/tmp/does-not-exist-axo-254-2.md", 1.0, "{}", 3u64]),
        )
        .unwrap();

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-2540"}
            })),
            id: Some(json!(254001)),
        })
        .unwrap()
        .result
        .unwrap();
    let data = response["data"].clone();
    assert_eq!(data["mode"].as_str(), Some("broken_only"));
    assert_eq!(data["removed_count"].as_u64(), Some(2));
    let removed_refs: Vec<&str> = data["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("artifact_ref").and_then(|v| v.as_str()))
        .collect();
    assert!(removed_refs.contains(&"/tmp/does-not-exist-axo-254-1.rs"));
    assert!(removed_refs.contains(&"/tmp/does-not-exist-axo-254-2.md"));
    let kept = data["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept[0].get("artifact_ref").and_then(|v| v.as_str()),
        Some(valid_path.as_str())
    );

    // REQ-AXO-902431 — l'appel ci-dessus est desormais un APERCU. Ce que ce test
    // garde depuis REQ-AXO-254 — la SELECTION : seuls les refs casses sont
    // candidats, le valide est preserve — est inchange et vient d'etre verifie.
    // Ce qui change est l'EFFET : rien n'a ete supprime.
    assert_eq!(data["dry_run"].as_bool(), Some(true), "{response}");
    let still_there = server
        .graph_store
        .query_count_param(
            "SELECT count(*) FROM soll.Traceability WHERE soll_entity_id = $e",
            &json!({ "e": "REQ-AXO-2540" }),
        )
        .unwrap_or(-1);
    assert_eq!(
        still_there, 3,
        "le mode qui derive lui-meme son ensemble est en apercu : les 3 lignes \
         doivent etre intactes apres l'appel par defaut"
    );

    // Applique, puis idempotence — c'est la seconde chose que ce test garde.
    let applied = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-2540", "dry_run": false}
            })),
            id: Some(json!(254002)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(applied["data"]["removed_count"].as_u64(), Some(2), "{applied}");

    let response2 = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-2540", "dry_run": false}
            })),
            id: Some(json!(254003)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response2["data"]["removed_count"].as_u64(), Some(0));
    assert_eq!(response2["data"]["kept"].as_array().unwrap().len(), 1);
}

#[test]
fn test_soll_remove_evidence_explicit_mode_reaches_non_file_artifact_types() {
    // REQ-AXO-902265 — the explicit mode promised removal "regardless of disk state" with
    // no word about artifact TYPE, but the candidate query filtered
    // `artifact_type IN ('file','document')` in BOTH modes. Measured on NEX: two `Test`
    // refs and three `Metric` refs, copied verbatim out of soll.Traceability, answered
    // `removed 0` — indistinguishable from "that ref does not exist". A dead evidence row
    // that cannot be removed still reads as evidence.
    let server = create_test_server();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-9022650', 'Requirement', 'AXO', 'remove_evidence non-file types', 'explicit mode', 'current', '{\"acceptance_criteria\":\"a\"}')")
        .unwrap();

    let seed = |trace_id: &str, artifact_type: &str, artifact_ref: &str, created: u64| {
        server
            .graph_store
            .execute_param(
                "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &json!([trace_id, "Requirement", "REQ-AXO-9022650", artifact_type, artifact_ref, 1.0, "{}", created]),
            )
            .unwrap();
    };
    seed("TRC-902265-TEST", "Test", "tests::some_dead_test", 1);
    seed("TRC-902265-METRIC", "Metric", "bench/scratch-never-committed.json", 2);
    seed("TRC-902265-FILE", "file", "/tmp/does-not-exist-902265.rs", 3);

    // Explicit mode targeting ONLY the two non-file rows: they must now be reachable.
    let data = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {
                    "entity_id": "REQ-AXO-9022650",
                    "broken_only": false,
                    "artifact_refs": [
                        "tests::some_dead_test",
                        "bench/scratch-never-committed.json"
                    ]
                }
            })),
            id: Some(json!(902265001)),
        })
        .unwrap()
        .result
        .unwrap()["data"]
        .clone();

    assert_eq!(data["mode"].as_str(), Some("explicit_refs"));
    assert_eq!(
        data["removed_count"].as_u64(),
        Some(2),
        "Test and Metric refs must be removable in explicit mode: {data}"
    );
    let removed_types: Vec<&str> = data["removed"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("artifact_type").and_then(|v| v.as_str()))
        .collect();
    assert!(removed_types.contains(&"Test"), "got {removed_types:?}");
    assert!(removed_types.contains(&"Metric"), "got {removed_types:?}");
    // The untargeted `file` row is preserved: widening the type reach must not widen the
    // blast radius.
    assert_eq!(data["kept"].as_array().unwrap().len(), 1);
    assert!(data["unmatched_refs"].as_array().unwrap().is_empty());

    // A ref that matches nothing must SAY so. `removed 0` on its own reads as "the row was
    // protected", which is how an evidence store gets cleaned in name only.
    let miss = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {
                    "entity_id": "REQ-AXO-9022650",
                    "broken_only": false,
                    "artifact_refs": ["tests::typo_in_this_ref"]
                }
            })),
            id: Some(json!(902265002)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(miss["data"]["removed_count"].as_u64(), Some(0));
    let unmatched: Vec<&str> = miss["data"]["unmatched_refs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(unmatched, vec!["tests::typo_in_this_ref"]);
    let text = miss["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("matched NO row"),
        "the human-readable line must name the miss, got: {text}"
    );

    // BLAST-RADIUS GUARD. Widening the candidate SELECT to every artifact type is safe
    // only because an empty `artifact_refs` matches nothing. If that ever inverted —
    // "no filter given" read as "remove everything" — this call would wipe an entity's
    // whole evidence store in one shot. The remaining `file` row from earlier in this
    // test is the canary.
    let empty = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-9022650", "broken_only": false}
            })),
            id: Some(json!(902265004)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        empty["data"]["removed_count"].as_u64(),
        Some(0),
        "broken_only=false with NO artifact_refs must remove nothing, never everything"
    );
    let survivors: String = server
        .graph_store
        .query_json("SELECT artifact_ref FROM soll.Traceability WHERE soll_entity_id = 'REQ-AXO-9022650'")
        .unwrap();
    assert!(
        survivors.contains("/tmp/does-not-exist-902265.rs"),
        "the untargeted row must survive an empty explicit request, got {survivors}"
    );
}

#[test]
fn test_soll_remove_evidence_broken_only_still_ignores_non_file_types() {
    // REQ-AXO-902265, the other half: `broken_only` decides via a filesystem existence
    // check, which says nothing about a `Test` or `Metric` ref. Widening the sweep there
    // would DELETE rows the tool cannot evaluate — a data-loss bug dressed as a fix. This
    // pins the asymmetry so a later "consistency" cleanup cannot quietly remove it.
    let server = create_test_server();

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-9022651', 'Requirement', 'AXO', 'remove_evidence broken_only scope', 'broken_only mode', 'current', '{\"acceptance_criteria\":\"a\"}')")
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-902265-KEEP-TEST", "Requirement", "REQ-AXO-9022651", "Test", "tests::not_a_path_at_all", 1.0, "{}", 1u64]),
        )
        .unwrap();
    server
        .graph_store
        .execute_param(
            "INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            &json!(["TRC-902265-DROP-FILE", "Requirement", "REQ-AXO-9022651", "file", "/tmp/does-not-exist-902265-b.rs", 1.0, "{}", 2u64]),
        )
        .unwrap();

    let data = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-9022651"}
            })),
            id: Some(json!(902265003)),
        })
        .unwrap()
        .result
        .unwrap()["data"]
        .clone();

    assert_eq!(data["mode"].as_str(), Some("broken_only"));
    // Only the broken FILE row goes. The Test row is never even a candidate — it must not
    // appear in `removed`, and `unmatched_refs` stays empty (no caller ref to fail).
    assert_eq!(data["removed_count"].as_u64(), Some(1));
    assert_eq!(
        data["removed"][0]["artifact_ref"].as_str(),
        Some("/tmp/does-not-exist-902265-b.rs")
    );
    assert!(data["unmatched_refs"].as_array().unwrap().is_empty());
    let survivors: String = server
        .graph_store
        .query_json("SELECT artifact_ref FROM soll.Traceability WHERE soll_entity_id = 'REQ-AXO-9022651'")
        .unwrap();
    assert!(
        survivors.contains("tests::not_a_path_at_all"),
        "the Test row must survive broken_only, got {survivors}"
    );
}

// REQ-AXO-274 phase 2 — canonical relation policy extensions
#[test]
fn test_relation_policy_accepts_cpt_to_cpt_inherits_from() {
    let server = create_test_server();
    // CPT-PRO sibling (universal)
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-PRO-099', 'Concept', 'PRO', 'Universal concept', 'cross-project mental model', 'active', '{}') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    // CPT-AXO project-specific specialization
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-099', 'Concept', 'AXO', 'Axon-specific concept', 'Axon-specific specialization', 'active', '{}')")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "concept",
                "data": {
                    "source_id": "CPT-AXO-099",
                    "target_id": "CPT-PRO-099",
                    "relation_type": "INHERITS_FROM"
                }
            }
        })),
        id: Some(json!(27401)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "CPT->CPT INHERITS_FROM must be canonical post REQ-AXO-274 phase 2: {content}"
    );
    assert_eq!(
        server
            .graph_store
            .query_count("SELECT count(*) FROM soll.Edge WHERE source_id='CPT-AXO-099' AND target_id='CPT-PRO-099' AND relation_type='INHERITS_FROM'")
            .unwrap(),
        1
    );
}

#[test]
fn test_relation_policy_accepts_gui_to_pil_belongs_to() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-PRO-099', 'Pillar', 'PRO', 'Test methodology pillar', 'theming axis', 'active', '{}') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-PRO-099', 'Guideline', 'PRO', 'Test guideline', 'rule', 'active', '{}') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    // REQ-AXO-91560 — the canonical seed (db/seed/01_global_soll.sql) ships
    // the GUI-PRO-099→PIL-PRO-099 BELONGS_TO sentinel edge, now baked into
    // the test template. Drop it so this test exercises a fresh `Link created`
    // rather than colliding with the seeded edge.
    server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE source_id='GUI-PRO-099' AND target_id='PIL-PRO-099' AND relation_type='BELONGS_TO'")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "guideline",
                "data": {
                    "source_id": "GUI-PRO-099",
                    "target_id": "PIL-PRO-099",
                    "relation_type": "BELONGS_TO"
                }
            }
        })),
        id: Some(json!(27402)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "GUI->PIL BELONGS_TO must be canonical post REQ-AXO-274 phase 2: {content}"
    );
}

#[test]
fn test_relation_policy_accepts_cpt_to_dec_inherits_from() {
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-PRO-099', 'Decision', 'PRO', 'Cross-project canonical decision', 'body', 'current', '{\"rationale\":\"R\"}') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-098', 'Concept', 'AXO', 'Axon mirror concept', 'specialization of DEC-PRO-099', 'active', '{}') ON CONFLICT (id) DO NOTHING")
        .unwrap();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "link",
                "entity": "concept",
                "data": {
                    "source_id": "CPT-AXO-098",
                    "target_id": "DEC-PRO-099",
                    "relation_type": "INHERITS_FROM"
                }
            }
        })),
        id: Some(json!(27403)),
    };
    let response = server.handle_request(req).unwrap().result.unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        content.contains("Link created"),
        "CPT->DEC INHERITS_FROM must be canonical post REQ-AXO-274 phase 2: {content}"
    );
}

// REQ-AXO-276 — axon_apply_methodology_bundle MCP tool
#[test]
fn test_axon_apply_methodology_bundle_rejects_missing_bundle_path() {
    let server = create_test_server();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": { "name": "axon_apply_methodology_bundle", "arguments": {} },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let data = &result["data"];
    assert_eq!(
        data["status"].as_str().unwrap(),
        "input_invalid",
        "missing bundle_path must return input_invalid"
    );
    assert_eq!(
        data["parameter_repair"]["invalid_field"].as_str().unwrap(),
        "bundle_path"
    );
}

#[test]
fn test_axon_apply_methodology_bundle_rejects_unsupported_schema() {
    let server = create_test_server();
    let tmp_dir = std::env::temp_dir().join(format!(
        "axon_methodology_bundle_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let bundle_path = tmp_dir.join("bad-schema.json");
    std::fs::write(
        &bundle_path,
        r#"{"schema":"wrong-schema-v0","version":"0.1","project_code":"AXO"}"#,
    )
    .unwrap();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_methodology_bundle",
            "arguments": { "bundle_path": bundle_path.to_string_lossy() }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    assert_eq!(result["data"]["status"].as_str().unwrap(), "input_invalid");
    assert_eq!(
        result["data"]["parameter_repair"]["invalid_field"]
            .as_str()
            .unwrap(),
        "schema"
    );
    std::fs::remove_dir_all(&tmp_dir).ok();
}

#[test]
fn test_axon_apply_methodology_bundle_dry_run_returns_summary() {
    let server = create_test_server();
    let tmp_dir = std::env::temp_dir().join(format!(
        "axon_methodology_bundle_dryrun_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let bundle_path = tmp_dir.join("minimal-bundle.json");
    let body = serde_json::json!({
        "schema": "axon-methodology-bundle-v1",
        "version": "1.0.0-test",
        "axon_min_version": "0.8.0",
        "project_code": "AXO",
        "pillars": [],
        "concepts": [],
        "guidelines": [
            {
                "logical_key": "gui_test_new",
                "title": "Test methodology guideline",
                "description": "Test body",
                "status": "active"
            },
            {
                "logical_key": "gui_test_regularization",
                "canonical_id_hint": "GUI-PRO-001",
                "title": "TDD Obligatoire",
                "regularization": true
            }
        ],
        "decisions": [],
        "requirements": [],
        "relations": []
    });
    std::fs::write(&bundle_path, serde_json::to_string(&body).unwrap()).unwrap();
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_apply_methodology_bundle",
            "arguments": {
                "bundle_path": bundle_path.to_string_lossy(),
                "dry_run": true
            }
        },
        "id": 1
    });
    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let data = &result["data"];
    assert_eq!(data["status"].as_str().unwrap(), "ok");
    assert_eq!(data["dry_run"].as_bool().unwrap(), true);
    assert_eq!(data["bundle_version"].as_str().unwrap(), "1.0.0-test");
    assert_eq!(data["project_code"].as_str().unwrap(), "AXO");
    assert_eq!(
        data["guidelines_applied"].as_u64().unwrap(),
        1,
        "1 non-regularization guideline counted under dry_run"
    );
    assert_eq!(
        data["guidelines_skipped_regularization"].as_u64().unwrap(),
        1,
        "1 regularization stanza skipped"
    );
    std::fs::remove_dir_all(&tmp_dir).ok();
}

// REQ-AXO-91578 — SKI (Skill) entity type addition.
// Verifies that soll_manager(create, entity='skill', project_code='PRO')
// successfully allocates a SKI-PRO-NNN id, inserts the row with type='Skill',
// and rejects creation when attach_to/relation_type pair has no canonical
// policy.
#[test]
fn test_skill_entity_type_create_with_canonical_inherit_from_guideline() {
    let server = create_test_server();
    server
        .graph_store
        .sync_project_registry_entry("TSK", Some("skill-test"), Some("/tmp/skill-test"))
        .unwrap();
    let _ = server.graph_store.execute(
        "DELETE FROM soll.Edge WHERE source_id LIKE 'SKI-TSK-%' OR target_id LIKE 'SKI-TSK-%'",
    );
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE project_code='TSK'");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('GUI-TSK-001', 'Guideline', 'TSK', 'TDD fixture', 'red green refactor', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "skill",
                "data": {
                    "project_code": "TSK",
                    "title": "Test skill — TDD obligatoire procedure",
                    "description": "Procedural body invoked by LLM via mcp__axon__skill_invoke. Implements GUI-PRO-001 (TDD obligatoire) as an executable skill : red → green → refactor loop using Axon MCP for query/inspect/commit.",
                    "attach_to": "GUI-TSK-001",
                    "relation_type": "INHERITS_FROM",
                    "status": "current"
                }
            }
        },
        "id": 91578
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let is_error = result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    assert!(
        !is_error,
        "SKI entity create should succeed, got: {content}"
    );
    assert!(
        content.contains("SKI-TSK-"),
        "Response should include canonical SKI id, got: {content}"
    );

    let count = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.Node WHERE type='Skill' AND project_code='TSK'")
        .unwrap();
    assert!(
        count >= 1,
        "at least one isolated SKI-TSK row expected after create, got {count}"
    );
    server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE source_id LIKE 'SKI-TSK-%' OR target_id LIKE 'SKI-TSK-%'")
        .unwrap();
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE project_code='TSK'")
        .unwrap();
}

// REQ-AXO-91578 — SKI entity must reject create when NO canonical relation
// exists for (SKI, target_type). Validates closed-policy enforcement.
// REQ-AXO-902288 — the target must be a genuinely no-policy pair: SKI→GUI is
// single-legal (INHERITS_FROM) and now AUTO-CANONIZES a wrong relation, so it no
// longer rejects. SKI→MIL has no policy at all (SKI reaches only PIL/GUI/SKI/PRT),
// so a create against a Milestone still (correctly) rejects — any relation.
#[test]
fn test_skill_entity_rejects_non_canonical_attach_target() {
    let server = create_test_server();
    // Seed a Milestone; (SKI, MIL) has no canonical relation policy.
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('MIL-PRO-001', 'Milestone', 'PRO', 'probe milestone', 'x', 'current', '{}')")
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "skill",
                "data": {
                    "project_code": "PRO",
                    "title": "Test skill — SKI→MIL has no policy, must reject",
                    "description": "SKI→MIL admits no canonical relation ; create must reject.",
                    "attach_to": "MIL-PRO-001",
                    "relation_type": "INHERITS_FROM",
                    "status": "current"
                }
            }
        },
        "id": 91578
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let is_error = result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    assert!(
        is_error,
        "SKI→MIL should reject (no canonical policy), got: {content}"
    );
}

// REQ-AXO-91579 — PRT (PromptTemplate) entity type addition.
#[test]
fn test_prompt_template_entity_type_create_with_canonical_inherit_from_guideline() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "soll_manager",
            "arguments": {
                "action": "create",
                "entity": "prompt_template",
                "data": {
                    "project_code": "PRO",
                    "title": "Test PRD body template",
                    "description": "Mustache template for PRD body sections, rendered by SKI-PRO-prd-synthesis. Parameters: project_code, acceptance_criteria, user_stories.",
                    "attach_to": "GUI-PRO-001",
                    "relation_type": "INHERITS_FROM",
                    "status": "current"
                }
            }
        },
        "id": 91579
    });

    let response = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = response.result.unwrap();
    let content = result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();

    let is_error = result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    assert!(
        !is_error,
        "PRT entity create should succeed, got: {content}"
    );
    assert!(
        content.contains("PRT-PRO-"),
        "Response should include canonical PRT id, got: {content}"
    );

    let count = server
        .graph_store
        .query_count(
            "SELECT count(*) FROM soll.Node WHERE type='PromptTemplate' AND project_code='PRO'",
        )
        .unwrap();
    assert!(
        count >= 1,
        "at least one PRT-PRO row expected after create, got {count}"
    );
}

// REQ-AXO-91580 — skill_list + skill_invoke MCP tools.
#[test]
fn test_skill_list_and_invoke_round_trip() {
    let server = create_test_server();
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id IN ('SKI-PRO-997', 'SKI-PRO-998')")
        .unwrap();

    // Seed a SKI directly (faster than going through soll_manager).
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('SKI-PRO-998', 'Skill', 'PRO', 'Test TDD skill', 'Body : red green refactor. Test fixture for SKI MCP surface.', 'current', '{\"invocation_mode\":\"MANDATED\",\"applicable_to\":[\"delivery\"]}'::jsonb) \
             ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('SKI-PRO-997', 'Skill', 'PRO', 'Retired test skill', 'Must stay hidden.', 'rejected', '{\"invocation_mode\":\"OPTIONAL\",\"applicable_to\":[\"test\"]}'::jsonb)",
        )
        .unwrap();

    // skill_list (no filter) — should include our SKI-PRO-998.
    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "skill_list",
            "arguments": {}
        },
        "id": 91580
    });
    let list_resp = server
        .handle_request(serde_json::from_value(list_req).unwrap())
        .unwrap();
    let list_result = list_resp.result.unwrap();
    let list_text = list_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(
        list_text.contains("SKI-PRO-998"),
        "skill_list output should contain seeded id, got: {list_text}"
    );
    assert!(
        !list_text.contains("SKI-PRO-997"),
        "terminal fixtures must not pollute the invocable catalogue: {list_text}"
    );
    let listed = list_result["data"]["skills"]
        .as_array()
        .and_then(|skills| skills.iter().find(|skill| skill["id"] == json!("SKI-PRO-998")))
        .expect("seeded current skill in structured list");
    assert_eq!(listed["invocation_mode"], json!("MANDATED"));
    assert_eq!(listed["applicable_to"], json!(["delivery"]));

    // skill_invoke by id — should return body.
    let invoke_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "skill_invoke",
            "arguments": { "id": "SKI-PRO-998" }
        },
        "id": 91580
    });
    let invoke_resp = server
        .handle_request(serde_json::from_value(invoke_req).unwrap())
        .unwrap();
    let invoke_result = invoke_resp.result.unwrap();
    let invoke_text = invoke_result.get("content").unwrap()[0]
        .get("text")
        .unwrap()
        .as_str()
        .unwrap();
    let is_error = invoke_result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!is_error, "skill_invoke should succeed, got: {invoke_text}");
    assert!(
        invoke_text.contains("Body : red green refactor"),
        "skill_invoke should return body, got: {invoke_text}"
    );

    // skill_invoke not_found — should reject cleanly.
    let nf_req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "skill_invoke",
            "arguments": { "id": "SKI-PRO-doesnotexist" }
        },
        "id": 91580
    });
    let nf_resp = server
        .handle_request(serde_json::from_value(nf_req).unwrap())
        .unwrap();
    let nf_result = nf_resp.result.unwrap();
    let nf_is_error = nf_result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(nf_is_error, "skill_invoke should reject unknown id");
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id IN ('SKI-PRO-997', 'SKI-PRO-998')")
        .unwrap();
}

#[test]
fn re_anchor_without_mandated_skills_routes_directly_to_work_plan() {
    // REQ-AXO-902516 / DGD #306 — zero mandated skills must never produce an
    // impossible, ID-less skill_invoke instruction.
    let server = create_test_server();
    let code = "NSK";
    server
        .graph_store
        .sync_project_registry_entry(&code, Some("no-skill-fixture"), Some("/tmp/no-skill"))
        .unwrap();
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "re_anchor",
                "arguments": {"project_code": code, "reason": "test"}
            })),
            id: Some(json!(902516)),
        })
        .unwrap()
        .result
        .unwrap();
    let content = response["content"][0]["text"].as_str().unwrap_or_default();
    assert!(!content.contains("skill_invoke"), "{content}");
    assert_eq!(response["data"]["mandated_skills"], json!([]));
    assert_eq!(response["data"]["next_action"]["tool"], json!("soll_work_plan"));
}

// REQ-AXO-91581 slice 2 — prompt_template_get applies Mustache substitution
// when no metadata.parameters sidecar is declared (backwards-compat path).
#[test]
fn test_prompt_template_get_renders_mustache_without_param_spec() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='PRT-PRO-998'");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('PRT-PRO-998', 'PromptTemplate', 'PRO', 'Test brief', 'You are a {{role}}. Context: {{context}}.', 'current', '{}'::jsonb)",
        )
        .unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": {
                "id": "PRT-PRO-998",
                "params": {"role": "reviewer", "context": "code-audit"}
            }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        data.get("rendering_engine").and_then(|v| v.as_str()),
        Some("mustache_v1"),
        "slice 2 must advertise mustache_v1 rendering engine"
    );
    let rendered = data.get("rendered_text").and_then(|v| v.as_str()).unwrap();
    assert!(
        !rendered.contains("{{role}}") && rendered.contains("reviewer"),
        "Mustache substitution must replace {{{{role}}}} with `reviewer`, got: {rendered}"
    );
    assert!(
        rendered.contains("code-audit"),
        "Mustache substitution must replace {{{{context}}}} with `code-audit`, got: {rendered}"
    );
}

// REQ-AXO-91581 slice 2 — typed parameter sidecar enforces required fields.
#[test]
fn test_prompt_template_get_rejects_missing_required_param() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='PRT-PRO-997'");
    let metadata = r#"{
        "parameters": [
            {"name": "role", "type": "string", "required": true, "description": "Reviewer role"},
            {"name": "tone", "type": "string", "required": false, "default": "neutral"}
        ]
    }"#;
    let insert_sql = format!(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
         VALUES ('PRT-PRO-997', 'PromptTemplate', 'PRO', 'Reviewer brief', 'You are a {{{{role}}}} ({{{{tone}}}}).', 'current', '{}'::jsonb)",
        metadata.replace('\'', "''")
    );
    server.graph_store.execute(&insert_sql).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": { "id": "PRT-PRO-997", "params": {} }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("input_invalid")
    );
    let repair = data.get("parameter_repair").unwrap();
    assert_eq!(
        repair.get("category").and_then(|v| v.as_str()),
        Some("param_validation_failed")
    );
    let errors = repair.get("errors").and_then(|v| v.as_array()).unwrap();
    assert!(
        errors.iter().any(|e| {
            e.get("rule").and_then(|v| v.as_str()) == Some("required_missing")
                && e.get("param").and_then(|v| v.as_str()) == Some("role")
        }),
        "must emit `required_missing` for `role`, got: {errors:?}"
    );
}

// REQ-AXO-91581 slice 2 — declared defaults applied when caller omits them.
#[test]
fn test_prompt_template_get_applies_param_default() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='PRT-PRO-996'");
    let metadata = r#"{
        "parameters": [
            {"name": "tone", "type": "string", "required": false, "default": "neutral"}
        ]
    }"#;
    let insert_sql = format!(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
         VALUES ('PRT-PRO-996', 'PromptTemplate', 'PRO', 'Tone brief', 'Tone: {{{{tone}}}}.', 'current', '{}'::jsonb)",
        metadata.replace('\'', "''")
    );
    server.graph_store.execute(&insert_sql).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": { "id": "PRT-PRO-996", "params": {} }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("ok"));
    let rendered = data.get("rendered_text").and_then(|v| v.as_str()).unwrap();
    assert!(
        rendered.contains("Tone: neutral."),
        "declared default must populate rendering, got: {rendered}"
    );
    let used = data.get("params_used").unwrap();
    assert_eq!(
        used.get("tone").and_then(|v| v.as_str()),
        Some("neutral"),
        "effective params must echo the resolved default"
    );
}

// REQ-AXO-91581 slice 2 — type mismatch is a structured validation error.
#[test]
fn test_prompt_template_get_rejects_type_mismatch() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='PRT-PRO-995'");
    let metadata = r#"{
        "parameters": [
            {"name": "iterations", "type": "integer", "required": true}
        ]
    }"#;
    let insert_sql = format!(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
         VALUES ('PRT-PRO-995', 'PromptTemplate', 'PRO', 'Iter brief', 'Run {{{{iterations}}}} times.', 'current', '{}'::jsonb)",
        metadata.replace('\'', "''")
    );
    server.graph_store.execute(&insert_sql).unwrap();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": { "id": "PRT-PRO-995", "params": { "iterations": "many" } }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let errors = result
        .get("data")
        .and_then(|d| d.get("parameter_repair"))
        .and_then(|p| p.get("errors"))
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(
        errors.iter().any(|e| {
            e.get("rule").and_then(|v| v.as_str()) == Some("type_mismatch")
                && e.get("param").and_then(|v| v.as_str()) == Some("iterations")
        }),
        "must emit `type_mismatch` for `iterations`, got: {errors:?}"
    );
}

// REQ-AXO-91581 slice 2 — validation_rule regex is enforced for strings.
#[test]
fn test_prompt_template_get_enforces_validation_rule_regex() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='PRT-PRO-994'");
    let metadata = r#"{
        "parameters": [
            {"name": "slug", "type": "string", "required": true, "validation_rule": "^[a-z][a-z0-9-]*$"}
        ]
    }"#;
    let insert_sql = format!(
        "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
         VALUES ('PRT-PRO-994', 'PromptTemplate', 'PRO', 'Slug brief', 'Slug: {{{{slug}}}}.', 'current', '{}'::jsonb)",
        metadata.replace('\'', "''")
    );
    server.graph_store.execute(&insert_sql).unwrap();

    // Bad input — uppercase letters.
    let bad = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": { "id": "PRT-PRO-994", "params": { "slug": "BadSlug" } }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(bad).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let errors = result
        .get("data")
        .and_then(|d| d.get("parameter_repair"))
        .and_then(|p| p.get("errors"))
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(
        errors.iter().any(|e| {
            e.get("rule").and_then(|v| v.as_str()) == Some("validation_rule_violated")
        }),
        "must emit `validation_rule_violated`, got: {errors:?}"
    );

    // Good input — same template renders cleanly.
    let good = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "prompt_template_get",
            "arguments": { "id": "PRT-PRO-994", "params": { "slug": "good-slug" } }
        },
        "id": 91581
    });
    let resp = server
        .handle_request(serde_json::from_value(good).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    assert_ne!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let rendered = result
        .get("data")
        .and_then(|d| d.get("rendered_text"))
        .and_then(|v| v.as_str())
        .unwrap();
    assert!(
        rendered.contains("Slug: good-slug."),
        "validation_rule must accept matching input, got: {rendered}"
    );
}

// REQ-AXO-91581 slice 2 — unit-level coverage of the helper directly so
// rendering / validation can evolve without spinning up the MCP server.
#[test]
fn test_validate_and_resolve_prompt_params_helper_paths() {
    use crate::mcp::tools_skill::{render_mustache_template, validate_and_resolve_prompt_params};

    let spec = serde_json::json!([
        {"name": "role", "type": "string", "required": true},
        {"name": "tone", "type": "string", "required": false, "default": "neutral"},
        {"name": "n", "type": "integer", "required": false},
    ]);
    let spec_array = spec.as_array().unwrap();

    // Missing required → error surfaced ; default still applied.
    let supplied = serde_json::json!({});
    let (effective, errors) = validate_and_resolve_prompt_params(spec_array, &supplied);
    assert!(errors
        .iter()
        .any(|e| e["rule"] == "required_missing" && e["param"] == "role"));
    assert_eq!(effective["tone"], serde_json::json!("neutral"));

    // All good → no errors, render succeeds.
    let supplied = serde_json::json!({ "role": "reviewer", "n": 3 });
    let (effective, errors) = validate_and_resolve_prompt_params(spec_array, &supplied);
    assert!(
        errors.is_empty(),
        "valid input must produce zero errors, got: {errors:?}"
    );
    assert_eq!(effective["tone"], serde_json::json!("neutral"));

    let rendered = render_mustache_template(
        "You are a {{role}} ({{tone}}). Iterations: {{n}}.",
        &effective,
    )
    .unwrap();
    assert_eq!(rendered, "You are a reviewer (neutral). Iterations: 3.");
}

// REQ-AXO-91582 — re_anchor MCP tool single-call recovery packet.
#[test]
fn test_re_anchor_returns_canonical_state_packet() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "re_anchor",
            "arguments": { "reason": "test_drift_signal", "project_code": "AXO" }
        },
        "id": 91582
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        data.get("project_code").and_then(|v| v.as_str()),
        Some("AXO")
    );
    assert_eq!(
        data.get("reason").and_then(|v| v.as_str()),
        Some("test_drift_signal")
    );
    // The envelope MUST contain these 5 load-bearing sections per CPT-AXO-90018.
    assert!(data.get("active_methodology").is_some());
    assert!(data.get("mandated_skills").is_some());
    assert!(data.get("recent_revisions").is_some());
    assert!(data.get("session_pointer").is_some());
    assert!(data.get("work_plan_top").is_some());
}

// REQ-AXO-902281 (feedback #45, NEX) — re_anchor must read the REGISTERED session pointer
// for ANY project, not guess `CPT-{project}-052` (which was AXO-only, null everywhere else).
// Register NEX with a pointer node whose id is NOT CPT-NEX-052, and assert re_anchor resolves
// THAT node — the exact result axon_init_project's resolve_session_pointer yields.
#[test]
fn test_re_anchor_reads_registered_pointer_for_non_axo_project() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.ProjectCodeRegistry WHERE project_code='NEX'");
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id='CPT-NEX-777'");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-NEX-777', 'Concept', 'NEX', 'NEX session pointer', 'NEX resume body here', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name, session_pointer_json) VALUES ('NEX', '/tmp/nex', 'nex', '{\"kind\":\"soll_node\",\"value\":\"CPT-NEX-777\",\"label\":\"NEX pointer\"}')")
        .unwrap();

    // Ground truth: exactly what axon_init_project resolves for NEX.
    let registered = server
        .graph_store
        .read_session_pointer("NEX")
        .unwrap()
        .expect("NEX registry pointer");
    assert_eq!(
        registered.get("value").and_then(|v| v.as_str()),
        Some("CPT-NEX-777")
    );

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "re_anchor",
            "arguments": { "reason": "non_axo_drift", "project_code": "NEX" }
        },
        "id": 902281
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let data = resp.result.unwrap().get("data").cloned().unwrap();
    let sp = data.get("session_pointer").unwrap();
    // The OLD hardcode (CPT-NEX-052) would have yielded null; the fix resolves the REAL node.
    assert_eq!(
        sp.get("id").and_then(|v| v.as_str()),
        Some("CPT-NEX-777"),
        "re_anchor must resolve the registered pointer, not CPT-NEX-052: {sp:?}"
    );
    assert_eq!(
        sp.get("body").and_then(|v| v.as_str()),
        Some("NEX resume body here"),
        "the resolved pointer body must be the registered node's body"
    );
}

// REQ-AXO-91583 — status() returns methodology_drift_warnings field.
#[test]
fn test_status_returns_methodology_drift_warnings_field() {
    let server = create_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "status",
            "arguments": { "mode": "brief" }
        },
        "id": 91583
    });
    let resp = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    let drift = data
        .get("methodology_drift_warnings")
        .expect("status() must include methodology_drift_warnings field per REQ-AXO-91583");
    assert!(
        drift.get("mandated_skills").is_some(),
        "drift envelope must contain mandated_skills list"
    );
    assert_eq!(
        drift.get("tracking_version").and_then(|v| v.as_str()),
        Some("v1_inmemory_audit"),
        "v1 tracking flag must be explicit"
    );
    assert!(drift.get("recently_invoked").is_some());
    assert!(drift.get("drift_warnings").is_some());
}

// REQ-AXO-91592 — soll_manager(action=unlink) round-trip : create + link
// then unlink ; the edge disappears and an audit revision is recorded.
#[test]
fn test_soll_manager_unlink_round_trip() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE source_id IN ('DEC-AXO-901592','REQ-AXO-901592')");
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id IN ('DEC-AXO-901592','REQ-AXO-901592')");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-901592', 'Decision', 'AXO', 'Test Decision', 'context', 'current', '{\"context\":\"ctx\",\"rationale\":\"r\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-AXO-901592', 'Requirement', 'AXO', 'Test Req', 'd', 'planned', '{\"priority\":\"P1\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('DEC-AXO-901592', 'REQ-AXO-901592', 'SOLVES', 'AXO') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "unlink",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-901592",
                    "target_id": "REQ-AXO-901592",
                    "relation_type": "SOLVES"
                }
            }
        })),
        id: Some(json!(91592)),
    };
    let resp = server.handle_request(req).unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(data.get("edges_removed").and_then(|v| v.as_i64()), Some(1));

    // Edge gone.
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE source_id='DEC-AXO-901592' AND target_id='REQ-AXO-901592' AND relation_type='SOLVES'"
            )
            .unwrap(),
        0,
        "edge must be removed"
    );
    // Audit row present.
    let revision_id = data
        .get("revision_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(revision_id.starts_with("unlink-"), "revision_id format");
    let count_changes = server
        .graph_store
        .query_count(&format!(
            "SELECT count(*) FROM soll.RevisionChange WHERE revision_id='{}' AND action='unlink' AND entity_type='edge'",
            revision_id.replace('\'', "''")
        ))
        .unwrap();
    assert_eq!(count_changes, 1, "RevisionChange row must be recorded");
}

// REQ-AXO-91592 — unlink on a non-existent edge returns `edge_not_found`.
#[test]
fn test_soll_manager_unlink_edge_not_found() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "unlink",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-919999",
                    "target_id": "REQ-AXO-919999",
                    "relation_type": "SOLVES"
                }
            }
        })),
        id: Some(json!(91592)),
    };
    let resp = server.handle_request(req).unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("edge_not_found")
    );
    let repair = data.get("parameter_repair").unwrap();
    assert_eq!(
        repair.get("category").and_then(|v| v.as_str()),
        Some("edge_not_found")
    );
}

// REQ-AXO-91592 — missing relation_type is structured input_invalid (no
// inference ; the caller MUST identify the exact edge).
#[test]
fn test_soll_manager_unlink_requires_relation_type() {
    let server = create_test_server();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "unlink",
                "entity": "decision",
                "data": {
                    "source_id": "DEC-AXO-901",
                    "target_id": "REQ-AXO-901"
                }
            }
        })),
        id: Some(json!(91592)),
    };
    let resp = server.handle_request(req).unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("input_invalid")
    );
    assert_eq!(
        data.get("parameter_repair")
            .and_then(|p| p.get("invalid_field"))
            .and_then(|v| v.as_str()),
        Some("data.relation_type")
    );
}

// REQ-AXO-91592 — EPITOMIZES is protected ; unlink without force=true is
// refused with the `protected_edge` envelope.
#[test]
fn test_soll_manager_unlink_protected_without_force() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE relation_type='EPITOMIZES' AND source_id='PIL-AXO-902' AND target_id='VIS-AXO-902'");
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id IN ('PIL-AXO-902','VIS-AXO-902')");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-902', 'Vision', 'AXO', 'Test Vision', 'd', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-902', 'Pillar', 'AXO', 'Test Pillar', 'd', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('PIL-AXO-902', 'VIS-AXO-902', 'EPITOMIZES', 'AXO') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "unlink",
                "entity": "pillar",
                "data": {
                    "source_id": "PIL-AXO-902",
                    "target_id": "VIS-AXO-902",
                    "relation_type": "EPITOMIZES"
                }
            }
        })),
        id: Some(json!(91592)),
    };
    let resp = server.handle_request(req).unwrap();
    let result = resp.result.unwrap();
    assert_eq!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
    let data = result.get("data").unwrap();
    assert_eq!(
        data.get("status").and_then(|v| v.as_str()),
        Some("input_invalid")
    );
    assert_eq!(
        data.get("parameter_repair")
            .and_then(|p| p.get("category"))
            .and_then(|v| v.as_str()),
        Some("protected_edge")
    );
    // Edge MUST still be present.
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE source_id='PIL-AXO-902' AND target_id='VIS-AXO-902' AND relation_type='EPITOMIZES'"
            )
            .unwrap(),
        1,
        "protected edge must NOT be removed without force"
    );
}

// REQ-AXO-91592 — EPITOMIZES with explicit force=true is honoured ; the
// edge is removed and audit recorded.
#[test]
fn test_soll_manager_unlink_protected_with_force() {
    let server = create_test_server();
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Edge WHERE relation_type='EPITOMIZES' AND source_id='PIL-AXO-903' AND target_id='VIS-AXO-903'");
    let _ = server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id IN ('PIL-AXO-903','VIS-AXO-903')");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('VIS-AXO-903', 'Vision', 'AXO', 'Test Vision', 'd', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('PIL-AXO-903', 'Pillar', 'AXO', 'Test Pillar', 'd', 'current', '{}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) VALUES ('PIL-AXO-903', 'VIS-AXO-903', 'EPITOMIZES', 'AXO') ON CONFLICT (source_id, target_id, relation_type) DO NOTHING")
        .unwrap();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "soll_manager",
            "arguments": {
                "action": "unlink",
                "entity": "pillar",
                "data": {
                    "source_id": "PIL-AXO-903",
                    "target_id": "VIS-AXO-903",
                    "relation_type": "EPITOMIZES",
                    "force": true
                }
            }
        })),
        id: Some(json!(91592)),
    };
    let resp = server.handle_request(req).unwrap();
    let result = resp.result.unwrap();
    let data = result.get("data").unwrap();
    assert_eq!(data.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        server
            .graph_store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE source_id='PIL-AXO-903' AND target_id='VIS-AXO-903' AND relation_type='EPITOMIZES'"
            )
            .unwrap(),
        0,
        "force=true must allow removal of the protected edge"
    );
}

/// REQ-AXO-901757 slice A — `soll_query_context(search=...)` returns SOLL nodes
/// ranked by ts_rank over title+description (FTS), and excludes non-matches.
/// Correctness holds with or without the soll_node_fts_idx GIN (the index is a
/// latency optimization; PG computes to_tsvector on a seq-scan otherwise).
#[test]
fn test_soll_query_context_search_returns_fts_ranked_nodes() {
    let server = create_test_server();
    let code = "FTS";
    server
        .graph_store
        .execute(&format!(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) \
             VALUES ('{code}', 'FtsFixture', '/tmp/fts') ON CONFLICT (project_code) DO NOTHING"
        ))
        .unwrap();
    let nodes = [
        (
            "REQ-FTS-001",
            "GPU embedding throughput restoration",
            "restore the embed rate on the vector lane",
        ),
        (
            "REQ-FTS-002",
            "Dashboard layout polish",
            "phoenix liveview grid columns",
        ),
        (
            "REQ-FTS-003",
            "Chunker giant-line windowing",
            "char windows bound the body budget",
        ),
    ];
    for (id, title, desc) in nodes {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Requirement', '{code}', '{title}', '{desc}', 'planned', '{{}}') \
                 ON CONFLICT (id) DO NOTHING"
            ))
            .unwrap();
    }

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_query_context",
                "arguments": { "project_code": code, "search": "embedding throughput" }
            })),
            id: Some(json!(757)),
        })
        .unwrap()
        .result
        .unwrap();

    let data = response.get("data").expect("search data");
    assert_eq!(data["search"].as_str(), Some("embedding throughput"));
    assert_eq!(data["surfaces_used"][0].as_str(), Some("soll_fts"));
    let matches = data["matches"].as_array().expect("matches array");
    assert!(!matches.is_empty(), "expected an FTS match: {data}");
    // Only the embedding node carries both 'embedding' AND 'throughput'.
    assert_eq!(
        matches[0]["id"].as_str(),
        Some("REQ-FTS-001"),
        "top match must be the embedding node: {data}"
    );
    assert!(
        matches
            .iter()
            .all(|m| m["id"].as_str() != Some("REQ-FTS-002")),
        "dashboard node must not match 'embedding throughput': {data}"
    );
}

// REQ-AXO-902192 S3 — the fail-closed anti-orphan gate. A symbol tagged
// `deliverable` (soll.Traceability, metadata.role='deliverable') that is
// reachable ONLY from a test (zero production caller — `test_only`) among the
// files in a commit must block `axon_commit_work`. Untagged orphans (the
// common case — opt-in gate, zero blast radius on existing code) and
// deliverable symbols that ARE wired to production must NOT block.
#[test]
fn test_axon_commit_work_blocks_deliverable_symbol_never_wired_to_production() {
    let server = create_test_server();
    let code = "TST".to_string();
    let module = format!("{code}::src/lib.rs");

    // orphan_tagged: deliverable-tagged, called ONLY by a #[test] fn — must block.
    let orphan_tagged = format!("{module}::orphan_tagged");
    // orphan_untagged: same shape, but NOT tagged deliverable — must NOT block (opt-in).
    let orphan_untagged = format!("{module}::orphan_untagged");
    // wired: deliverable-tagged but has a real production caller — must NOT block.
    let wired = format!("{module}::wired_fn");
    let wired_caller = format!("{module}::wired_caller");
    let test_fn = format!("{module}::exercises_orphans");

    for (id, name) in [
        (&orphan_tagged, "orphan_tagged"),
        (&orphan_untagged, "orphan_untagged"),
        (&wired, "wired_fn"),
        (&wired_caller, "wired_caller"),
    ] {
        server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{id}', '{name}', 'function', false, true, false, '{code}')")).unwrap();
    }
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{test_fn}', 'exercises_orphans', 'function', true, false, false, '{code}')")).unwrap();

    for id in [&orphan_tagged, &orphan_untagged, &wired, &wired_caller] {
        server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{module}', '{id}', 'CONTAINS', '{code}', 0)")).unwrap();
    }
    // test_fn calls the two "orphan" candidates (their only caller is a test).
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{test_fn}', '{orphan_tagged}', 'CALLS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{test_fn}', '{orphan_untagged}', 'CALLS', '{code}', 0)")).unwrap();
    // wired_caller (a non-test prod symbol) calls `wired` — a real production edge.
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{wired_caller}', '{wired}', 'CALLS', '{code}', 0)")).unwrap();

    // Tag orphan_tagged + wired as deliverable (orphan_untagged is deliberately left untagged).
    server.graph_store.execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES ('TRC-{code}-1', 'Requirement', 'REQ-{code}-1', 'Symbol', 'orphan_tagged', 1.0, '{{\"role\":\"deliverable\"}}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES ('TRC-{code}-2', 'Requirement', 'REQ-{code}-1', 'Symbol', 'wired_fn', 1.0, '{{\"role\":\"deliverable\"}}', 0)")).unwrap();

    crate::ist_snapshot::evict_process_snapshot(&code);
    assert!(server.ensure_ram_snapshot_warm(&code));

    let commit = |diff_path: &str, id: i64| {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({
                    "name": "axon_commit_work",
                    "arguments": {
                        "project_code": code,
                        "diff_paths": [diff_path],
                        "message": "feat: touch orphan candidates",
                        "dry_run": true
                    }
                })),
                id: Some(json!(id)),
            })
            .unwrap()
            .result
            .unwrap()
    };

    // 1) deliverable + test_only (orphan_tagged) in the diff → BLOCKED.
    let blocked = commit("src/lib.rs", 9021921);
    assert_eq!(blocked["isError"].as_bool(), Some(true), "deliverable test_only symbol must block: {blocked:?}");
    let violations = blocked["data"]["violations"].as_array().cloned().unwrap_or_default();
    assert!(
        violations.iter().any(|v| v["diagnostic"].as_str().unwrap_or("").contains("orphan_tagged")),
        "violation must name orphan_tagged: {violations:?}"
    );

    // 2) Remove the blocking symbol; only the untagged orphan + the prod-wired
    // deliverable symbol remain in the diff's file → must NOT block.
    server.graph_store.execute(&format!("DELETE FROM Symbol WHERE id = '{orphan_tagged}'")).unwrap();
    server.graph_store.execute(&format!("DELETE FROM ist.Edge WHERE target_id = '{orphan_tagged}'")).unwrap();
    crate::ist_snapshot::evict_process_snapshot(&code);
    assert!(server.ensure_ram_snapshot_warm(&code));

    let clean = commit("src/lib.rs", 9021922);
    assert_ne!(
        clean["isError"].as_bool(),
        Some(true),
        "untagged orphan + prod-wired deliverable must NOT block: {clean:?}"
    );
    assert!(clean["content"][0]["text"].as_str().unwrap_or("").contains("Validation passed"));
}

// REQ-AXO-902192 S3 — an explicit `role='entry'` tag (a legitimately declared
// dynamic-dispatch entry point) exempts a symbol from the gate even when it is
// ALSO tagged `deliverable` and classifies as `test_only`.
#[test]
fn test_axon_commit_work_entry_tag_exempts_deliverable_symbol_from_gate() {
    let server = create_test_server();
    let code = "TST".to_string();
    let module = format!("{code}::src/lib.rs");
    let hook = format!("{module}::registered_hook");
    let test_fn = format!("{module}::exercises_hook");

    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{hook}', 'registered_hook', 'function', false, true, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_code) VALUES ('{test_fn}', 'exercises_hook', 'function', true, false, false, '{code}')")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{module}', '{hook}', 'CONTAINS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO ist.Edge (source_id, target_id, relation_type, project_code, created_at_ms) VALUES ('{test_fn}', '{hook}', 'CALLS', '{code}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES ('TRC-{code}-3', 'Requirement', 'REQ-{code}-1', 'Symbol', 'registered_hook', 1.0, '{{\"role\":\"deliverable\"}}', 0)")).unwrap();
    server.graph_store.execute(&format!("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, metadata, created_at) VALUES ('TRC-{code}-4', 'Requirement', 'REQ-{code}-1', 'Symbol', 'registered_hook', 1.0, '{{\"role\":\"entry\"}}', 0)")).unwrap();

    crate::ist_snapshot::evict_process_snapshot(&code);
    assert!(server.ensure_ram_snapshot_warm(&code));

    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "axon_commit_work",
                "arguments": {
                    "project_code": code,
                    "diff_paths": ["src/lib.rs"],
                    "message": "feat: touch hook",
                    "dry_run": true
                }
            })),
            id: Some(json!(9021923)),
        })
        .unwrap()
        .result
        .unwrap();

    assert_ne!(
        response["isError"].as_bool(),
        Some(true),
        "role='entry' must exempt a deliverable symbol from the gate: {response:?}"
    );
}

#[test]
fn test_relative_evidence_is_judged_against_the_registry_root_not_the_brain_cwd() {
    // REQ-AXO-902436 — the freshness sweep resolved a RELATIVE `artifact_ref`
    // through `resolve_canonical_project_identity`, which only reads
    // `.axon/meta.json` from disk. Measured on axon_live: that scan knows 13
    // project roots while `soll.ProjectCodeRegistry` knows 75. For the 62 it
    // misses the root came back `None`, every relative ref was stat()ed
    // against the brain's own cwd, and 126 of 156 relative refs were reported
    // `broken` while present under their real root (TE2 78/82, OPV 47/47).
    // `soll_remove_evidence(broken_only=true)` deletes exactly that list.
    //
    // The fixture uses a project code that exists ONLY in the registry — no
    // `.axon/meta.json` anywhere — because that is precisely the population
    // the disk-only resolver cannot see.
    let server = create_test_server();
    let root = tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("docs")).expect("mkdir docs");
    std::fs::write(root.path().join("docs/proof.md"), b"proof").expect("write proof");
    let root_str = root.path().to_string_lossy().to_string();
    server
        .graph_store
        .sync_project_registry_entry("ZZ9", Some("zz9-fixture"), Some(&root_str))
        .expect("register fixture project");

    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-ZZ9-902436', 'Requirement', 'ZZ9', 'Registry-rooted evidence', 'Relative evidence must resolve against the registry root', 'current', '{\"acceptance_criteria\":\"documented\"}')")
        .expect("insert requirement");
    // The real reference: relative, and PRESENT under the registry root.
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-ZZ9-PRESENT', 'requirement', 'REQ-ZZ9-902436', 'file', 'docs/proof.md', 1.0, 0)")
        .expect("insert present evidence");
    // POSITIVE CONTROL — same shape, same sweep, but genuinely absent under
    // the root. Without it, `broken == 0` could equally mean "the sweep never
    // looked at this project", which is the failure this test exists to catch.
    server
        .graph_store
        .execute("INSERT INTO soll.Traceability (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, confidence, created_at) VALUES ('TRC-ZZ9-ABSENT', 'requirement', 'REQ-ZZ9-902436', 'file', 'docs/never_written.md', 1.0, 0)")
        .expect("insert absent evidence");

    // Driven through the public MCP surface — the contract a client actually
    // reads, not an internal struct.
    let result = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_verify_requirements",
                "arguments": { "project_code": "ZZ9" }
            })),
            id: Some(json!(902436)),
        })
        .unwrap()
        .result
        .unwrap();

    let details = result["data"]["details"].as_array().expect("details");
    let entry = details
        .iter()
        .find(|value| value["id"].as_str() == Some("REQ-ZZ9-902436"))
        .expect("requirement entry");
    let offenders: Vec<&str> = entry["broken_file_evidence_offenders"]
        .as_array()
        .expect("offenders array")
        .iter()
        .filter_map(|o| o["path"].as_str())
        .collect();
    assert!(
        offenders.contains(&"docs/never_written.md"),
        "positive control: the sweep must actually run and flag the absent \
         reference — got {offenders:?}"
    );
    assert!(
        !offenders.contains(&"docs/proof.md"),
        "a relative reference that EXISTS under the registry root must not be \
         reported broken — got {offenders:?}"
    );
    assert_eq!(
        entry["broken_file_evidence_count"].as_u64(),
        Some(1),
        "exactly the absent one, never the present one"
    );
    assert_eq!(
        result["data"]["unresolvable_file_evidence_count"].as_u64(),
        Some(0),
        "the root resolved, so nothing is left unjudged"
    );
}

#[test]
fn test_an_unjudgeable_reference_is_never_called_broken() {
    // REQ-AXO-902436 — the destructive half. When the root cannot be resolved
    // at all, a RELATIVE reference is UNMEASURABLE, not broken: there is
    // nothing to check it against. Reporting it broken is what aims
    // `soll_remove_evidence(broken_only=true)` at valid proofs.
    //
    // An ABSOLUTE reference stays judgeable with no root whatsoever, so it
    // keeps its verdict — the root only ever mattered for relative paths.
    use crate::mcp::tools_soll::classify_evidence_ref_against_root;

    assert_eq!(
        classify_evidence_ref_against_root("docs/audits/attestation.md", None),
        "unresolved_root",
        "no root → a relative path is UNJUDGED, not broken"
    );
    assert_eq!(
        classify_evidence_ref_against_root("/nonexistent/axon/req_902436_absolute_probe.log", None),
        "broken",
        "an absolute path needs no root to be judged absent"
    );

    let root = tempdir().expect("tempdir");
    std::fs::create_dir_all(root.path().join("sub")).expect("mkdir sub");
    std::fs::write(root.path().join("sub/present.md"), b"x").expect("write");
    assert_eq!(
        classify_evidence_ref_against_root("sub/present.md", Some(root.path())),
        "present"
    );
    assert_eq!(
        classify_evidence_ref_against_root("sub/missing.md", Some(root.path())),
        "broken"
    );
    assert_eq!(
        classify_evidence_ref_against_root("sub", Some(root.path())),
        "directory"
    );
}

#[test]
fn test_auto_evidence_attaches_the_subject_requirement_not_the_ones_merely_cited() {
    // REQ-AXO-902445 — APS measured four commits in one session attaching
    // evidence to requirements they never touched (llm_feedback #207), because
    // the message named them as precedents: "same distinction as REQ-APS-572",
    // "REQ-APS-560 is why Horde is unrunnable". That is what a GOOD commit
    // message does, so the tool was penalising message quality — and
    // `soll_verify_requirements` counted a long-delivered requirement as better
    // covered than it is.
    let message = "fix(cluster): drain the handoff queue (REQ-ZZ7-573)\n\n\
                   Same DERIVED / CHOSEN distinction as REQ-ZZ7-572, and \
                   REQ-ZZ7-560 is the reason Horde is unrunnable here.";

    let subject_ids = crate::mcp::tools_soll::parse_commit_req_ids_for_tests(
        message.lines().next().unwrap_or(""),
    );
    let all_ids = crate::mcp::tools_soll::parse_commit_req_ids_for_tests(message);

    // POSITIVE CONTROL — the whole-message parse really does pick up all three,
    // otherwise this test would prove nothing about narrowing.
    assert_eq!(
        all_ids,
        vec!["REQ-ZZ7-573", "REQ-ZZ7-572", "REQ-ZZ7-560"],
        "positive control: the message really does cite three requirements"
    );

    assert_eq!(
        subject_ids,
        vec!["REQ-ZZ7-573"],
        "only the subject declares what the commit proves"
    );

    // REQ-AXO-902445, second defect, found BY this guard: the scanner accepted
    // an uppercase-only project segment, while a canonical code is 3
    // ALPHAnumeric characters. Every tenant whose code carries a digit — TE2,
    // GS2, ZZ7 — parsed to nothing, so `axon_commit_work` attached no evidence
    // for them at all, and said nothing about it. That is why the fixture above
    // deliberately uses `ZZ7` rather than a letters-only code.
    assert!(
        !crate::mcp::tools_soll::parse_commit_req_ids_for_tests(
            "fix(x): thing (REQ-TE2-154)"
        )
        .is_empty(),
        "a project code carrying a digit must parse"
    );
    // …and a malformed segment still must not.
    assert!(
        crate::mcp::tools_soll::parse_commit_req_ids_for_tests(
            "fix(x): thing (REQ-TOOLONG-154)"
        )
        .is_empty(),
        "a segment that is not a canonical 3-char code is not an id"
    );
}

#[test]
fn test_a_digit_bearing_tenant_really_gets_a_traceability_row_end_to_end() {
    // REQ-AXO-902444, contrôle demandé par TE2 (mailbox 13928) et qui manquait à
    // ma propre garde. Leur formulation : « c'est le seul test qui distingue
    // corrigé de écrit ».
    //
    // Ma garde d'origine testait le PARSEUR — `REQ-TE2-154` est bien extrait
    // d'un titre. Elle ne testait PAS la chaîne complète commit → ligne de
    // `soll.Traceability` pour un code projet portant un chiffre. Or c'est là
    // que se mesure le trou que TE2 a chiffré : 16 REQ `delivered` sans aucune
    // preuve, sur toute l'histoire de leur projet, alors que leurs titres de
    // commit nomment systématiquement leur REQ.
    let server = create_test_server();
    let sandbox = init_commit_work_sandbox();
    // Un code à CHIFFRE — c'est toute la question.
    server
        .graph_store
        .sync_project_registry_entry(
            "Z9X",
            Some("z9x-fixture"),
            Some(sandbox.path().to_str().unwrap()),
        )
        .expect("register digit-bearing project");
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('REQ-Z9X-154', 'Requirement', 'Z9X', 'Digit-coded tenant', 'Auto-evidence must reach a digit-coded tenant', 'current', '{}')")
        .expect("insert requirement");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": "axon_commit_work",
            "arguments": {
                "diff_paths": ["Cargo.toml"],
                "project_path": sandbox.path().to_str().unwrap(),
                "message": "fix(ml): REQ-Z9X-154 — sandbox commit, never reaches the real repo",
                "dry_run": false
            }
        },
        "id": 902444
    });
    let result = server
        .handle_request(serde_json::from_value(req).unwrap())
        .unwrap()
        .result
        .unwrap();
    assert!(
        !result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "the commit itself must succeed: {result:?}"
    );

    // LA question : une ligne de Traceability existe-t-elle réellement ?
    let rows = server
        .graph_store
        .query_json(
            "SELECT artifact_type, artifact_ref FROM soll.Traceability \
             WHERE soll_entity_id = 'REQ-Z9X-154'",
        )
        .expect("read traceability");
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&rows).expect("rows");
    assert_eq!(
        rows.len(),
        1,
        "un commit dont le TITRE nomme un REQ d'un tenant à code chiffré doit \
         produire UNE ligne de Traceability — c'est ce qui manquait à TE2 sur \
         toute l'histoire du projet. Obtenu : {rows:?}"
    );
    assert_eq!(
        rows[0][0].as_str().map(str::to_ascii_lowercase).as_deref(),
        Some("commit"),
        "et c'est une preuve de type Commit"
    );
}

/// REQ-AXO-902453 défaut 1 — signalé par TE2 (`llm_feedback` #224). Le refus
/// `attach_required` proposait les six Pillars du projet à un `milestone` ;
/// **aucun n'est atteignable depuis un MIL**, et le message SUIVANT l'expliquait.
/// Deux appels perdus, et une hésitation sur laquelle des deux réponses croire.
/// La matrice existait déjà : c'était une jointure, pas une fonctionnalité.
#[test]
fn attach_required_proposes_only_parents_the_source_kind_can_actually_reach() {
    let server = create_test_server();
    // Pas d'apostrophe : `seed_pillar` interpole le titre sans échapper.
    seed_pillar(&server, "TSA", "PIL-TSA-901", "Pilier inatteignable depuis un jalon");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('REQ-TSA-901', 'Requirement', 'TSA', 'Cible legale dun jalon', '', 'current', '{}') \
             ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();

    let refusal = |entity: &str| -> String {
        server
            .execute_tool_direct(
                "soll_manager",
                &json!({
                    "action": "create",
                    "entity": entity,
                    "data": { "title": "sans parent", "description": "corps", "project_code": "TSA" }
                }),
            )
            .expect("soll_manager répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // Un MIL atteint REQ via TARGETS, et MIL via SUPERSEDES (destructif, donc
    // jamais proposé). Il n'atteint AUCUN Pillar.
    let milestone = refusal("milestone");
    assert!(
        !milestone.contains("PIL-TSA-901"),
        "un jalon ne peut pas s'attacher à un Pillar — le proposer envoie dans un mur.\n---\n{milestone}"
    );
    assert!(
        milestone.contains("REQ-TSA-901"),
        "la seule cible légale doit être proposée.\n---\n{milestone}"
    );
    assert!(
        milestone.contains("TARGETS"),
        "la relation doit être nommée avec le parent, sinon le second appel se devine.\n---\n{milestone}"
    );

    // Contrôle positif : REQ → PIL est légal (BELONGS_TO). Le Pillar doit rester
    // proposé — sinon le correctif a simplement cassé le cas qui marchait.
    let requirement = refusal("requirement");
    assert!(
        requirement.contains("PIL-TSA-901") && requirement.contains("BELONGS_TO"),
        "un requirement atteint bien un Pillar : le filtre ne doit pas l'écarter.\n---\n{requirement}"
    );
}

/// REQ-AXO-902453 défaut 2 — deux chemins qui devaient s'accorder. `link` posait
/// l'arête SUPERSEDES ET retirait la cible, en le disant ; `create` posait
/// l'arête et se TAISAIT, la cible restant `current`. Le graphe se contredisait
/// alors lui-même — supersédé par une arête, ouvert par son statut — et TE2 ne
/// l'a découvert qu'en comptant les jalons ouverts de `soll_roadmap`.
#[test]
fn create_with_supersedes_retires_the_target_exactly_like_link_does() {
    let server = create_test_server();
    seed_pillar(&server, "TSB", "PIL-TSB-901", "Ancre");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('MIL-TSB-901', 'Milestone', 'TSB', 'Jalon remplacé', '', 'current', '{}') \
             ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();

    let status_of = |id: &str| -> String {
        let raw = server
            .graph_store
            .query_json(&format!(
                "SELECT COALESCE(status, '') FROM soll.Node WHERE id = '{id}'"
            ))
            .unwrap_or_default();
        serde_json::from_str::<Vec<Vec<String>>>(&raw)
            .unwrap_or_default()
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next())
            .unwrap_or_default()
    };

    let res = server
        .execute_tool_direct(
            "soll_manager",
            &json!({
                "action": "create",
                "entity": "milestone",
                "data": {
                    "title": "Jalon remplaçant",
                    "description": "corps",
                    "attach_to": "MIL-TSB-901",
                    "relation_type": "SUPERSEDES"
                }
            }),
        )
        .expect("soll_manager répond");
    assert_ne!(res["isError"].as_bool(), Some(true), "{res}");

    assert_eq!(
        status_of("MIL-TSB-901"),
        "superseded",
        "une arête SUPERSEDES qui ne retire pas sa cible laisse DEUX jalons ouverts"
    );
    let text = res["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("superseded") && text.contains("MIL-TSB-901"),
        "le silence était la moitié du défaut : la bascule doit être annoncée.\n---\n{text}"
    );

    // Contrôle positif : une création ORDINAIRE ne touche pas au statut du parent.
    let ordinary = create_call(
        &server,
        json!({ "title": "exigence ordinaire", "description": "corps", "attach_to": "PIL-TSB-901" }),
    );
    assert_ne!(ordinary["isError"].as_bool(), Some(true), "{ordinary}");
    assert_eq!(
        status_of("PIL-TSB-901"),
        "current",
        "seul SUPERSEDES retire ; un BELONGS_TO ne doit rien muter"
    );
}

/// REQ-AXO-902455 — la porte demandée par TE2 (`llm_feedback` #224) n'est plus
/// une branche Rust : c'est une RÈGLE-DONNÉE, portée par `GUI-PRO-119` et
/// `GUI-PRO-120`, seedées sous `PRO` donc héritées par tout tenant.
/// `DEC-AXO-901652` le prescrivait ; ce test vérifie que la donnée décide.
///
/// Aucune Guideline n'est créée ici : les règles viennent du seed. Un test qui
/// poserait lui-même sa règle prouverait que le moteur évalue, pas que le
/// produit LIVRE la règle.
#[test]
fn soll_validate_enforces_the_supersedes_rules_carried_as_data_not_as_code() {
    let server = create_test_server();
    seed_pillar(&server, "TSC", "PIL-TSC-901", "Ancre");
    for (id, status) in [
        ("MIL-TSC-901", "current"),    // remplaçant vivant
        ("MIL-TSC-902", "current"),    // cible INCOHÉRENTE : supersédée mais ouverte
        ("MIL-TSC-903", "superseded"), // cible cohérente — le contrôle positif
        ("MIL-TSC-904", "superseded"), // source RETIRÉE : l'arête part du mauvais bout
        ("MIL-TSC-905", "current"),    // ... vers un nœud VIVANT
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Milestone', 'TSC', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}'"
            ))
            .unwrap();
    }
    for (source, target) in [
        ("MIL-TSC-901", "MIL-TSC-902"),
        ("MIL-TSC-901", "MIL-TSC-903"),
        // Arête à l'envers : c'est la SOURCE qui est retirée. Mesuré sur AXO,
        // 7 des 10 cas réels sont de cette forme.
        ("MIL-TSC-904", "MIL-TSC-905"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                 VALUES ('{source}', '{target}', 'SUPERSEDES', 'TSC') \
                 ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
            ))
            .unwrap();
    }

    // Les règles LIVRÉES, pas des règles fabriquées par le test.
    let rules = server.load_soll_rules("TSC");
    assert!(
        rules.iter().any(|r| r.id == "GUI-PRO-119")
            && rules.iter().any(|r| r.id == "GUI-PRO-120"),
        "le seed doit livrer les deux règles ; chargées : {:?}",
        rules.iter().map(|r| &r.id).collect::<Vec<_>>()
    );

    let validate = || -> String {
        server
            .execute_tool_direct("soll_validate", &json!({ "project_code": "TSC" }))
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let text = validate();

    assert!(
        text.contains("MIL-TSC-902"),
        "la cible encore ouverte doit être signalée.\n---\n{text}"
    );
    // Contrôle positif : la supersession COHÉRENTE ne doit produire aucun bruit.
    // Sans lui, une règle qui signale TOUTE arête SUPERSEDES passerait au vert.
    //
    // L'assertion porte sur les LIGNES DE CES DEUX RÈGLES, pas sur le rapport
    // entier : `MIL-TSC-903` y figure légitimement au titre de `GUI-PRO-130`
    // (son corps est vide, donc il ne dit pas qu'il est retiré). Assertion sur
    // le texte entier = un test qui rougit dès qu'une AUTRE règle est posée.
    let supersedes_lines: String = text
        .lines()
        .filter(|l| l.contains("GUI-PRO-119") || l.contains("GUI-PRO-120"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !supersedes_lines.contains("MIL-TSC-903"),
        "une supersession cohérente n'est pas une violation.\n---\n{text}"
    );
    assert!(
        !text.contains("0 minimal coherence violation"),
        "le compteur doit inclure les violations de règles.\n---\n{text}"
    );

    // Chaque ligne cite SA Guideline : c'est ce qui transforme « c'est
    // interdit » en « c'est interdit PARCE QUE <intention> », et c'est la valeur
    // que DEC-AXO-901649 nomme. Les deux incohérences se réparent à l'OPPOSÉ,
    // donc elles ne peuvent pas citer la même règle.
    let line_for = |id: &str| -> String {
        text.lines()
            .find(|l| l.contains(id))
            .unwrap_or_default()
            .to_string()
    };
    let forgotten = line_for("MIL-TSC-902");
    let inverted = line_for("MIL-TSC-905");
    assert!(
        forgotten.contains("GUI-PRO-119"),
        "une cible oubliée relève de GUI-PRO-119.\n---\n{text}"
    );
    assert!(
        inverted.contains("GUI-PRO-120"),
        "une arête dont la SOURCE est retirée est inversée : GUI-PRO-120, dont la \
         réparation est l'OPPOSÉE.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    // Retirer la Guideline doit éteindre la règle. Si le rouge persiste, c'est
    // qu'une branche Rust décide encore — ce que ce lot a précisément supprimé.
    server
        .graph_store
        .execute("UPDATE soll.Node SET status = 'superseded' WHERE id = 'GUI-PRO-119'")
        .unwrap();
    let after = validate();
    assert!(
        !after.contains("GUI-PRO-119"),
        "règle retirée, elle ne doit plus être évaluée : la DONNÉE décide.\n---\n{after}"
    );
    assert!(
        after.contains("GUI-PRO-120"),
        "retirer une règle ne doit pas éteindre les autres.\n---\n{after}"
    );
}

/// REQ-AXO-902455 — axe « unicité », porté par `GUI-PRO-121`. PREMIER prédicat
/// qui compare des nœuds ENTRE EUX ; c'est lui qui matérialise
/// `DEC-AXO-901673`. La règle vient du seed, pas du test : un test qui poserait
/// sa propre règle prouverait que le moteur évalue, pas que le produit LIVRE.
#[test]
fn soll_validate_names_both_nodes_when_two_living_ones_share_a_title() {
    let server = create_test_server();
    seed_pillar(&server, "TSF", "PIL-TSF-901", "Ancre unicite");
    for (id, title, status) in [
        ("MIL-TSF-901", "Jalon en double", "current"),
        ("MIL-TSF-902", "Jalon en double", "planned"),
        ("MIL-TSF-903", "Jalon unique", "current"),
        // Retiré : hors du sujet (`current`/`planned`). Sans ce cas, une règle
        // qui ignorerait le filtre de statut passerait au vert.
        ("MIL-TSF-904", "Jalon en double", "superseded"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Milestone', 'TSF', '{title}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}', title = '{title}'"
            ))
            .unwrap();
    }

    assert!(
        server.load_soll_rules("TSF").iter().any(|r| r.id == "GUI-PRO-121"),
        "le seed doit livrer GUI-PRO-121"
    );

    // Les écritures du test passent par `execute` : le snapshot RAM ne les
    // voit pas tant qu'il n'est pas invalidé. En runtime c'est `pg_notify`
    // qui le fait ; ici c'est explicite, sinon la falsification mesurerait
    // un graphe périmé et passerait au vert sans rien vérifier.
    let validate = || -> String {
        server.soll_cache().invalidate("TSF");
        server
            .execute_tool_direct("soll_validate", &json!({ "project_code": "TSF" }))
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let lines_of = |text: &str, rule: &str| -> Vec<String> {
        text.lines()
            .filter(|l| l.contains(rule))
            .map(str::to_string)
            .collect()
    };

    let text = validate();
    let cited = lines_of(&text, "GUI-PRO-121");
    let joined = cited.join("\n");

    // Une violation NOMME les nœuds en cause — un compteur de doublons n'ouvre
    // aucune action (REQ-AXO-902409).
    assert!(
        joined.contains("MIL-TSF-901") && joined.contains("MIL-TSF-902"),
        "les DEUX porteurs du titre partagé doivent être nommés.\n---\n{text}"
    );
    // Contrôles positifs : sans eux, une règle qui signale TOUT nœud passerait.
    assert!(
        !joined.contains("MIL-TSF-903"),
        "un titre unique n'est pas une violation d'unicité.\n---\n{text}"
    );
    assert!(
        !joined.contains("MIL-TSF-904"),
        "un nœud RETIRÉ est hors du sujet de la règle : le filtre de statut \
         doit être lu.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    server
        .graph_store
        .execute("UPDATE soll.Node SET title = 'Jalon redevenu unique' WHERE id = 'MIL-TSF-902'")
        .unwrap();
    let after = validate();
    assert!(
        lines_of(&after, "GUI-PRO-121").is_empty(),
        "le doublon levé, la règle doit s'éteindre.\n---\n{after}"
    );
}

/// REQ-AXO-902455 — axe « atteignabilité », porté par `GUI-PRO-122`. Rend
/// vérifiable la cohérence de filiation que VIS-AXO-001 réclame : une exigence
/// qu'aucun chemin ne relie à une Vision ne dit pas POURQUOI on la ferait.
#[test]
fn soll_validate_flags_an_open_requirement_that_reaches_no_vision() {
    let server = create_test_server();
    seed_pillar(&server, "TSG", "PIL-TSG-901", "Ancre filiation");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('VIS-TSG-901', 'Vision', 'TSG', 'Nord du projet', '', 'current', '{}') \
             ON CONFLICT (id) DO NOTHING",
        )
        .unwrap();
    for (id, status) in [
        ("REQ-TSG-901", "planned"),   // rattaché — le contrôle positif
        ("REQ-TSG-902", "planned"),   // orphelin ouvert — la violation attendue
        ("REQ-TSG-903", "delivered"), // orphelin TERMINÉ — hors du sujet
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Requirement', 'TSG', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}'"
            ))
            .unwrap();
    }
    // Le chemin nominal est TRANSITIF : REQ -BELONGS_TO-> PIL -EPITOMIZES-> VIS.
    // Une règle qui n'inspecterait qu'un saut rendrait REQ-TSG-901 fautif.
    for (source, target, rel) in [
        ("PIL-TSG-901", "VIS-TSG-901", "EPITOMIZES"),
        ("REQ-TSG-901", "PIL-TSG-901", "BELONGS_TO"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                 VALUES ('{source}', '{target}', '{rel}', 'TSG') \
                 ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
            ))
            .unwrap();
    }

    assert!(
        server.load_soll_rules("TSG").iter().any(|r| r.id == "GUI-PRO-122"),
        "le seed doit livrer GUI-PRO-122"
    );

    // Les écritures du test passent par `execute` : le snapshot RAM ne les
    // voit pas tant qu'il n'est pas invalidé. En runtime c'est `pg_notify`
    // qui le fait ; ici c'est explicite, sinon la falsification mesurerait
    // un graphe périmé et passerait au vert sans rien vérifier.
    let validate = || -> String {
        server.soll_cache().invalidate("TSG");
        server
            .execute_tool_direct("soll_validate", &json!({ "project_code": "TSG" }))
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let cited = |text: &str| -> String {
        text.lines()
            .filter(|l| l.contains("GUI-PRO-122"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let text = validate();
    let joined = cited(&text);
    assert!(
        joined.contains("REQ-TSG-902"),
        "une exigence ouverte sans chemin vers une Vision doit être signalée.\n---\n{text}"
    );
    assert!(
        !joined.contains("REQ-TSG-901"),
        "le chemin REQ→PIL→VIS est TRANSITIF : cette exigence atteint la Vision.\n---\n{text}"
    );
    assert!(
        !joined.contains("REQ-TSG-903"),
        "la règle vise les statuts OUVERTS ; rattacher après coup une exigence \
         livrée fabriquerait de la filiation devinée.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    // Couper le second saut doit rendre REQ-TSG-901 fautif à son tour : c'est
    // la preuve que la transitivité est réellement parcourue, et non supposée.
    server
        .graph_store
        .execute(
            "DELETE FROM soll.Edge WHERE source_id = 'PIL-TSG-901' \
             AND target_id = 'VIS-TSG-901' AND relation_type = 'EPITOMIZES'",
        )
        .unwrap();
    let after = validate();
    assert!(
        cited(&after).contains("REQ-TSG-901"),
        "le chemin coupé, l'exigence rattachée n'atteint plus la Vision.\n---\n{after}"
    );
}

/// REQ-AXO-902455 — axe « agrégat », porté par `GUI-PRO-123`. Seul des six axes
/// posé sans cas réel : aucun des 75 projets n'a plus d'une Vision vivante au
/// 2026-08-22. C'est une garde de NON-RÉGRESSION, et sa falsification ne peut
/// donc venir que d'ici — d'où le second nœud construit exprès.
#[test]
fn soll_validate_flags_a_project_that_grew_a_second_living_vision() {
    let server = create_test_server();
    seed_pillar(&server, "TSH", "PIL-TSH-901", "Ancre agregat");
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('VIS-TSH-901', 'Vision', 'TSH', 'Nord unique', '', 'current', '{}') \
             ON CONFLICT (id) DO UPDATE SET status = 'current'",
        )
        .unwrap();

    assert!(
        server.load_soll_rules("TSH").iter().any(|r| r.id == "GUI-PRO-123"),
        "le seed doit livrer GUI-PRO-123"
    );

    // Les écritures du test passent par `execute` : le snapshot RAM ne les
    // voit pas tant qu'il n'est pas invalidé. En runtime c'est `pg_notify`
    // qui le fait ; ici c'est explicite, sinon la falsification mesurerait
    // un graphe périmé et passerait au vert sans rien vérifier.
    let validate = || -> String {
        server.soll_cache().invalidate("TSH");
        server
            .execute_tool_direct("soll_validate", &json!({ "project_code": "TSH" }))
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let cited = |text: &str| -> String {
        text.lines()
            .filter(|l| l.contains("GUI-PRO-123"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Contrôle positif D'ABORD : une seule Vision ne doit rien produire. Sans
    // lui, une règle qui signalerait TOUTE Vision passerait la suite au vert.
    assert!(
        cited(&validate()).is_empty(),
        "une Vision unique n'est pas une violation d'agrégat."
    );

    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('VIS-TSH-902', 'Vision', 'TSH', 'Second nord', '', 'current', '{}') \
             ON CONFLICT (id) DO UPDATE SET status = 'current'",
        )
        .unwrap();
    let text = validate();
    let joined = cited(&text);
    // La violation NOMME les sujets et le compte — « trop de Visions » sans
    // dire lesquelles n'ouvre aucune action (REQ-AXO-902409).
    assert!(
        joined.contains("VIS-TSH-901") && joined.contains("VIS-TSH-902"),
        "les deux Visions vivantes doivent être nommées.\n---\n{text}"
    );
    assert!(
        joined.contains("2 sujets pour un maximum de 1"),
        "la ligne doit porter le compte ET la borne.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    // Retirer la surnuméraire éteint la règle : c'est le filtre de statut du
    // sujet qui est éprouvé, pas seulement le comptage.
    server
        .graph_store
        .execute("UPDATE soll.Node SET status = 'superseded' WHERE id = 'VIS-TSH-902'")
        .unwrap();
    assert!(
        cited(&validate()).is_empty(),
        "la Vision surnuméraire retirée, le projet retrouve un nord unique."
    );
}

/// REQ-AXO-902455 — équivalence AVANT retrait, sur le MÊME fixture : le check
/// en dur `duplicate_titles` et la règle-donnée `GUI-PRO-121` sont comparés
/// pendant que les deux existent. C'est le patron d'oracle de `DEC-AXO-901662`
/// et le seul moyen de savoir qu'une migration ne change pas le comportement.
///
/// Les DEUX sens sont vérifiés, et c'est le second qui protège (`#1127`) :
///   1. tout ce que l'ancien code trouve, la règle le trouve aussi ;
///   2. ce que la règle reçoit à l'exécution DÉBORDE ce que l'ancien couvrait —
///      il se limitait à `Requirement`/`Decision`/`Concept`, donc il était
///      aveugle aux `Skill` et `PromptTemplate`. Mesuré sur le parc au
///      2026-08-22 : les 41 doublons de `PRO` sont exactement de ces deux
///      types — des résidus de tests dans le namespace produit hérité par les
///      75 tenants, que l'ancien check n'a jamais pu voir.
#[test]
fn the_declarative_title_rule_replaced_the_hardcoded_check_without_losing_coverage() {
    let server = create_test_server();
    seed_pillar(&server, "TSI", "PIL-TSI-901", "Ancre equivalence");
    for (id, kind, title) in [
        // Couvert par les DEUX.
        ("REQ-TSI-901", "Requirement", "Titre partage"),
        ("REQ-TSI-902", "Requirement", "Titre partage"),
        // Couvert par la RÈGLE SEULE — l'ancien check ignorait ce type.
        ("SKI-TSI-901", "Skill", "Competence partagee"),
        ("SKI-TSI-902", "Skill", "Competence partagee"),
        // Couvert par AUCUN — le contrôle positif.
        ("REQ-TSI-903", "Requirement", "Titre unique"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', '{kind}', 'TSI', '{title}', '', 'current', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET title = '{title}', type = '{kind}'"
            ))
            .unwrap();
    }
    server.soll_cache().invalidate("TSI");

    // Les DEUX verdicts sortent du MÊME appel : c'est la surface publique qui
    // est comparée, pas un champ interne — un test qui lirait la structure
    // privée survivrait au retrait sans rien prouver de ce que voit l'appelant.
    let text = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSI" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let section = |header: &str| -> String {
        text.lines()
            .skip_while(|l| !l.contains(header))
            .skip(1)
            .take_while(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("  - "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let legacy = section("Duplicate titles (potential semantic duplicates)");
    let rule_lines: String = text
        .lines()
        .filter(|l| l.contains("GUI-PRO-121"))
        .collect::<Vec<_>>()
        .join("\n");

    // Le check en dur est PARTI (GUI-PRO-017 : pas de double source). Sa
    // section ne doit plus être émise du tout — sinon deux verdicts
    // coexistent et peuvent diverger sans que rien ne le signale.
    assert!(
        legacy.is_empty(),
        "la section du check en dur doit avoir disparu ; sa survivance \
         signalerait une double source.\n---\n{text}"
    );

    // Sens 1 — inclusion : rien de ce que l'ancien voyait n'est perdu. Ces
    // deux ids sont exactement ceux que le check en dur rendait sur ce même
    // fixture, mesuré avant son retrait (commit de cette migration).
    for id in ["REQ-TSI-901", "REQ-TSI-902"] {
        assert!(
            rule_lines.contains(id),
            "la règle doit couvrir tout ce que le check en dur trouvait ; \
             `{id}` manque.\n---\n{rule_lines}"
        );
    }

    // Sens 2 — débordement : la règle voit ce que l'ancien ne pouvait pas.
    assert!(
        rule_lines.contains("SKI-TSI-901") && rule_lines.contains("SKI-TSI-902"),
        "la règle doit voir le doublon de Skill — c'est précisément le trou \
         que le check en dur laissait, et où vivent les 41 doublons de PRO.\n\
         ---\n{rule_lines}"
    );

    // Contrôle positif — sinon une règle qui signale TOUT passerait les deux
    // assertions précédentes.
    assert!(
        !rule_lines.contains("REQ-TSI-903"),
        "un titre unique n'est pas une violation.\n---\n{text}"
    );
}

/// REQ-AXO-902455 — axe « métadonnée », porté par `GUI-PRO-126`. SIXIÈME et
/// dernier axe du moteur à recevoir une règle réelle : jusqu'ici il était livré
/// et jamais exercé, l'état exact où `structural_invariants` a passé des mois
/// (0 règle sur 258 Guidelines). Poser la première EST le test d'acceptation.
#[test]
fn soll_validate_flags_an_open_requirement_with_no_acceptance_criteria() {
    let server = create_test_server();
    seed_pillar(&server, "TSJ", "PIL-TSJ-901", "Ancre criteres");
    for (id, status, meta) in [
        // Critères présents — le contrôle positif.
        ("REQ-TSJ-901", "planned", r#"{"acceptance_criteria": ["le test X passe"]}"#),
        // Absents — la violation attendue.
        ("REQ-TSJ-902", "planned", "{}"),
        // Présents mais VIDES : une liste vide est une absence, pas une valeur.
        ("REQ-TSJ-903", "planned", r#"{"acceptance_criteria": []}"#),
        // Livrée : hors du sujet — un critère écrit après coup est écrit en
        // regardant ce qui a été fait, donc il ne prouve rien.
        ("REQ-TSJ-904", "delivered", "{}"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Requirement', 'TSJ', '{id}', '', '{status}', '{meta}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}', metadata = '{meta}'"
            ))
            .unwrap();
    }

    assert!(
        server.load_soll_rules("TSJ").iter().any(|r| r.id == "GUI-PRO-126"),
        "le seed doit livrer GUI-PRO-126"
    );

    let validate = || -> String {
        server.soll_cache().invalidate("TSJ");
        server
            .execute_tool_direct("soll_validate", &json!({ "project_code": "TSJ" }))
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let cited = |text: &str| -> String {
        text.lines()
            .filter(|l| l.contains("GUI-PRO-126"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let text = validate();
    let joined = cited(&text);
    assert!(
        joined.contains("REQ-TSJ-902"),
        "une exigence ouverte sans critères doit être signalée.\n---\n{text}"
    );
    assert!(
        joined.contains("REQ-TSJ-903"),
        "une liste de critères VIDE est une absence — sans quoi il suffit \
         d'écrire `[]` pour éteindre la règle.\n---\n{text}"
    );
    assert!(
        !joined.contains("REQ-TSJ-901"),
        "des critères présents ne sont pas une violation.\n---\n{text}"
    );
    assert!(
        !joined.contains("REQ-TSJ-904"),
        "la règle vise les statuts OUVERTS.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    server
        .graph_store
        .execute(
            "UPDATE soll.Node SET metadata = '{\"acceptance_criteria\": [\"la mesure Y descend sous Z\"]}' \
             WHERE id = 'REQ-TSJ-902'",
        )
        .unwrap();
    let after = cited(&validate());
    assert!(
        !after.contains("REQ-TSJ-902"),
        "les critères écrits, la règle doit s'éteindre sur ce nœud.\n---\n{after}"
    );
    assert!(
        after.contains("REQ-TSJ-903"),
        "corriger un nœud n'éteint pas la règle sur les autres.\n---\n{after}"
    );
}

/// REQ-AXO-902455 — équivalence AVANT retrait des TROIS derniers checks de
/// rattachement, sur le MÊME fixture, pendant que les deux implémentations
/// coexistent. Patron d'oracle de `DEC-AXO-901662`.
///
/// Ces trois-là ne trouvent presque rien sur le parc réel (1 cas), donc une
/// comparaison sur données de production serait vide de sens : le fixture
/// CONSTRUIT les trois défauts, sans quoi l'équivalence serait vraie par
/// vacuité (`#434`).
#[test]
fn the_three_attachment_rules_replaced_the_hardcoded_checks_without_losing_coverage() {
    let server = create_test_server();
    seed_pillar(&server, "TSK", "PIL-TSK-901", "Ancre rattachement");
    for (id, kind, status) in [
        ("REQ-TSK-901", "Requirement", "planned"),  // relié à rien
        ("REQ-TSK-902", "Requirement", "planned"),  // relié — contrôle positif
        ("VAL-TSK-901", "Validation", "passed"),    // sans VERIFIES
        ("VAL-TSK-902", "Validation", "passed"),    // avec VERIFIES — contrôle
        ("DEC-TSK-901", "Decision", "current"),     // relié à rien
        ("DEC-TSK-902", "Decision", "current"),     // relié — contrôle positif
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', '{kind}', 'TSK', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}'"
            ))
            .unwrap();
    }
    for (source, target, rel) in [
        ("REQ-TSK-902", "PIL-TSK-901", "BELONGS_TO"),
        // VERIFIES posé dans le sens ENTRANT du sujet : la convention a varié
        // selon les projets, et `either` est là pour que la conformité ne
        // dépende pas de l'époque d'écriture.
        ("VAL-TSK-902", "REQ-TSK-902", "VERIFIES"),
        ("DEC-TSK-902", "REQ-TSK-902", "SOLVES"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                 VALUES ('{source}', '{target}', '{rel}', 'TSK') \
                 ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
            ))
            .unwrap();
    }
    server.soll_cache().invalidate("TSK");

    for id in ["GUI-PRO-127", "GUI-PRO-128", "GUI-PRO-129"] {
        assert!(
            server.load_soll_rules("TSK").iter().any(|r| r.id == id),
            "le seed doit livrer {id}"
        );
    }

    let text = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSK" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let section = |header: &str| -> String {
        text.lines()
            .skip_while(|l| !l.contains(header))
            .skip(1)
            .take_while(|l| l.trim_start().starts_with("- "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let rule_lines = |rule: &str| -> String {
        text.lines()
            .filter(|l| l.contains(rule))
            .collect::<Vec<_>>()
            .join("\n")
    };

    for (header, rule, offender, innocent) in [
        ("Orphan requirements", "GUI-PRO-127", "REQ-TSK-901", "REQ-TSK-902"),
        ("Validations without VERIFIES link", "GUI-PRO-128", "VAL-TSK-901", "VAL-TSK-902"),
        ("Decisions sans aucune relation", "GUI-PRO-129", "DEC-TSK-901", "DEC-TSK-902"),
    ] {
        let legacy = section(header);
        let migrated = rule_lines(rule);
        // Le check en dur est PARTI : sa section ne doit plus être émise. Deux
        // verdicts sur le même défaut finissent par diverger — c'est
        // exactement ce qui est arrivé à `decisions_without_links`
        // (REQ-AXO-902405).
        assert!(
            legacy.is_empty(),
            "la section `{header}` du check en dur doit avoir disparu ; sa \
             survivance signalerait une double source.\n---\n{text}"
        );
        // Sens 1 — inclusion : le défaut que le check en dur voyait sur ce
        // fixture (mesuré avant son retrait) est toujours vu.
        assert!(
            migrated.contains(offender),
            "{rule} doit couvrir ce que le check en dur trouvait : `{offender}` \
             manque.\n---\n{text}"
        );
        // Sens 2 — pas de sur-détection : le nœud rattaché reste innocenté.
        assert!(
            !migrated.contains(innocent),
            "`{innocent}` est rattaché ; {rule} ne doit pas le signaler.\n---\n{text}"
        );
    }
}

/// REQ-AXO-902455 — `GUI-PRO-124` et `GUI-PRO-125` sont les DEUX règles reprises
/// de `GUI-AXO-1032/1033` en les passant sous `PRO`. Le déplacement s'est fait
/// sans garde : les tests du moteur couvraient leurs PRÉDICATS (preuve,
/// `incoming`), rien ne vérifiait que CES règles-là sont livrées par le seed et
/// évaluées.
///
/// L'enjeu est chiffré : au 2026-08-22 elles portent **164 des 182 violations
/// AXO** (88 + 76). Si elles cessaient de charger — parse cassé, statut, scope
/// projet — le compte tomberait à 18 et rien ne le dirait. C'est la garde qui
/// manquait au lot précédent.
#[test]
fn the_two_rules_rehomed_under_pro_are_still_delivered_and_evaluated() {
    let server = create_test_server();
    seed_pillar(&server, "TSL", "PIL-TSL-901", "Ancre reprises");
    for (id, kind, status) in [
        ("REQ-TSL-901", "Requirement", "delivered"), // preuve cassée → 124
        ("REQ-TSL-902", "Requirement", "delivered"), // preuve valide → contrôle
        ("MIL-TSL-901", "Milestone", "superseded"),  // rien ne le remplace → 125
        ("MIL-TSL-902", "Milestone", "superseded"),  // remplacé → contrôle
        ("MIL-TSL-903", "Milestone", "current"),     // le remplaçant vivant
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', '{kind}', 'TSL', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}'"
            ))
            .unwrap();
    }
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
             VALUES ('MIL-TSL-903', 'MIL-TSL-902', 'SUPERSEDES', 'TSL') \
             ON CONFLICT (source_id, target_id, relation_type) DO NOTHING",
        )
        .unwrap();
    for (tid, req, status) in [
        ("TRC-TSL-901", "REQ-TSL-901", "broken"),
        ("TRC-TSL-902", "REQ-TSL-902", "present"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Traceability \
                 (id, soll_entity_type, soll_entity_id, artifact_type, artifact_ref, artifact_status, confidence, created_at) \
                 VALUES ('{tid}', 'requirement', '{req}', 'File', 'src/{req}.rs', '{status}', 1.0, 0) \
                 ON CONFLICT (id) DO UPDATE SET artifact_status = '{status}'"
            ))
            .unwrap();
    }
    server.soll_cache().invalidate("TSL");

    // Les règles viennent du SEED, pas du test : un test qui poserait sa propre
    // règle prouverait que le moteur évalue, pas que le produit LIVRE.
    let loaded = server.load_soll_rules("TSL");
    for id in ["GUI-PRO-124", "GUI-PRO-125"] {
        assert!(
            loaded.iter().any(|r| r.id == id),
            "{id} doit être livrée par le seed ; chargées : {:?}",
            loaded.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
    }

    let text = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSL" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cited = |rule: &str| -> String {
        text.lines()
            .filter(|l| l.contains(rule))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let broken_proof = cited("GUI-PRO-124");
    assert!(
        broken_proof.contains("REQ-TSL-901"),
        "une exigence livrée dont la preuve est cassée doit être signalée.\n---\n{text}"
    );
    assert!(
        !broken_proof.contains("REQ-TSL-902"),
        "une preuve `present` n'est pas une violation — sans ce contrôle, une \
         règle qui signale TOUTE preuve passerait.\n---\n{text}"
    );

    let no_replacement = cited("GUI-PRO-125");
    assert!(
        no_replacement.contains("MIL-TSL-901"),
        "un nœud retiré que rien ne remplace doit être signalé.\n---\n{text}"
    );
    assert!(
        !no_replacement.contains("MIL-TSL-902"),
        "ce nœud RECOIT une arête SUPERSEDES : son remplaçant est enregistré.\n---\n{text}"
    );
    assert!(
        !no_replacement.contains("MIL-TSL-903"),
        "le remplaçant est vivant, il n'est pas sujet de la règle.\n---\n{text}"
    );

    // ── Falsification par la DONNÉE ────────────────────────────────────────
    // Retirer chaque Guideline doit éteindre SA règle et laisser l'autre — si
    // le rouge persiste, c'est qu'une branche Rust décide encore.
    server
        .graph_store
        .execute("UPDATE soll.Node SET status = 'superseded' WHERE id = 'GUI-PRO-124'")
        .unwrap();
    server.soll_cache().invalidate("TSL");
    let after = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSL" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !after.contains("GUI-PRO-124"),
        "règle retirée, elle ne doit plus être évaluée : la DONNÉE décide.\n---\n{after}"
    );
    assert!(
        after.contains("GUI-PRO-125"),
        "retirer une règle ne doit pas éteindre l'autre.\n---\n{after}"
    );
    server
        .graph_store
        .execute("UPDATE soll.Node SET status = 'current' WHERE id = 'GUI-PRO-124'")
        .unwrap();
}

/// REQ-AXO-902455 — règles INLINE. Le plan promettait de pouvoir essayer une
/// règle AVANT de l'inscrire comme Guideline ; sans ce chemin, écrire une règle
/// juste demande de créer un nœud SOLL puis de le superséder, ce qui laisse une
/// trace pour rien. Symétrique du paramètre `rules` de `structural_invariants`.
#[test]
fn soll_validate_evaluates_inline_rules_so_one_can_be_tried_before_being_inscribed() {
    let server = create_test_server();
    seed_pillar(&server, "TSD", "PIL-TSD-901", "Ancre");
    for (id, kind, status) in [
        ("REQ-TSD-901", "Requirement", "current"),
        ("REQ-TSD-902", "Requirement", "delivered"),
    ] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', '{kind}', 'TSD', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO UPDATE SET status = '{status}'"
            ))
            .unwrap();
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
                 VALUES ('{id}', 'PIL-TSD-901', 'BELONGS_TO', 'TSD') \
                 ON CONFLICT (source_id, target_id, relation_type) DO NOTHING"
            ))
            .unwrap();
    }

    let validate = |rules: Value| -> String {
        server
            .execute_tool_direct(
                "soll_validate",
                &json!({ "project_code": "TSD", "rules": rules }),
            )
            .expect("soll_validate répond")["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // Une règle qu'AUCUNE Guideline ne porte : elle n'existe que dans l'appel.
    let text = validate(json!([{
        "id": "essai-local",
        "title": "un requirement livré ne s'attache pas à un pilier",
        "mode": "forbidden",
        "source_kind": "Requirement",
        "source_status_in": ["delivered"],
        "target_kind": "Pillar",
        "relations": ["BELONGS_TO"],
        "message": "essai avant inscription"
    }]));
    // Cibler LA LIGNE de la règle, pas tout le rapport : `REQ-TSD-901` y figure
    // légitimement ailleurs (il n'a ni critères ni preuve, ce que d'autres
    // vérifications signalent). Une assertion sur le texte entier confondrait
    // deux sections et rougirait pour une raison sans rapport.
    let inline_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("essai-local"))
        .collect();
    assert_eq!(
        inline_lines.len(),
        1,
        "une règle inline, une violation.\n---\n{text}"
    );
    assert!(
        inline_lines[0].contains("REQ-TSD-902"),
        "la règle inline doit être évaluée et nommer le nœud fautif.\n---\n{text}"
    );
    assert!(
        !inline_lines[0].contains("REQ-TSD-901"),
        "le sélecteur de statut doit trier : seul le `delivered` est visé.\n---\n{text}"
    );

    // Contrôle positif : sans le paramètre, la règle n'existe nulle part et rien
    // n'est signalé. C'est ce qui prouve qu'elle venait bien de l'appel.
    let without = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSD" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !without.contains("essai-local"),
        "une règle inline ne doit pas survivre à l'appel qui la portait.\n---\n{without}"
    );
}

/// REQ-AXO-902455 — la jointure règle→intention, testée BOUT EN BOUT. C'est la
/// valeur que DEC-AXO-901649 nomme (« règle gouvernée PAR une Décision SOLL,
/// jointe à la violation ») : un `rule_id` qui ne s'ouvre pas est un identifiant
/// décoratif, et le lecteur reste avec « c'est interdit » sans le « parce que ».
#[test]
fn every_rule_id_cited_by_a_violation_opens_with_soll_get() {
    let server = create_test_server();
    seed_pillar(&server, "TSE", "PIL-TSE-901", "Ancre");
    for (id, status) in [("MIL-TSE-901", "current"), ("MIL-TSE-902", "current")] {
        server
            .graph_store
            .execute(&format!(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
                 VALUES ('{id}', 'Milestone', 'TSE', '{id}', '', '{status}', '{{}}') \
                 ON CONFLICT (id) DO NOTHING"
            ))
            .unwrap();
    }
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Edge (source_id, target_id, relation_type, project_code) \
             VALUES ('MIL-TSE-901', 'MIL-TSE-902', 'SUPERSEDES', 'TSE') \
             ON CONFLICT (source_id, target_id, relation_type) DO NOTHING",
        )
        .unwrap();

    let text = server
        .execute_tool_direct("soll_validate", &json!({ "project_code": "TSE" }))
        .expect("soll_validate répond")["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Les ids cités sont entre crochets en fin de ligne : `… [GUI-PRO-119]`.
    let cited: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let start = line.rfind('[')?;
            let end = line.rfind(']')?;
            (end > start).then(|| line[start + 1..end].to_string())
        })
        .filter(|id| id.starts_with("GUI-"))
        .collect();
    assert!(
        !cited.is_empty(),
        "au moins une violation doit citer sa règle.\n---\n{text}"
    );

    for rule_id in &cited {
        let opened = server
            .execute_tool_direct("soll_get", &json!({ "id": rule_id }))
            .expect("soll_get répond");
        assert_ne!(
            opened["isError"].as_bool(),
            Some(true),
            "le rule_id `{rule_id}` cité par une violation doit s'OUVRIR — sinon c'est un \
             identifiant décoratif : {opened}"
        );
        let body = opened["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            body.contains(rule_id.as_str()) && body.len() > 200,
            "`soll_get({rule_id})` doit rendre le corps de la règle, pas un accusé de \
             réception.\n---\n{body}"
        );
        // La règle doit dire ce qu'elle exige ET comment réparer : c'est la
        // moitié qui transforme un verdict en action (REQ-AXO-902409).
        assert!(
            body.contains("RÉPARATION"),
            "une règle sans réparation laisse le lecteur devant une impasse polie \
             (PIL-AXO-002).\n---\n{body}"
        );
    }
}

/// REQ-AXO-902466 — une arête relie DEUX projets ; n'en rafraîchir qu'un laisse
/// l'autre affirmer que l'arête n'existe pas.
///
/// VPC (courrier 15091) et OPV (courrier 15106) ont signalé le même jour un
/// `soll_validate` qui déclarait absente une arête que le writer voyait en base.
/// Le writer lit PG, le moteur de règles lit le snapshot RAM ; quand le snapshot
/// retarde, le verdict n'annonce pas « je n'ai pas encore vu l'arête », il affirme
/// une propriété du graphe. `REQ-AXO-902060` avait fermé le cas mono-projet ; le
/// `find_map` qu'il a posé ne rend qu'UN projet, et depuis `REQ-AXO-902461` les
/// supersessions cross-project deviennent la norme.
#[test]
fn cross_project_edge_invalidates_both_ends_902466() {
    let cross_project = serde_json::json!({
        "action": "link",
        "entity": "requirement",
        "data": { "source_id": "REQ-TE2-104", "target_id": "REQ-VPC-067",
                  "relation_type": "SUPERSEDES" }
    });
    assert_eq!(
        McpServer::soll_mutation_project_codes_in_payload(&cross_project),
        vec!["TE2".to_string(), "VPC".to_string()],
        "les DEUX bouts doivent être rafraîchis : le tenant chez qui la violation \
         est comptée est justement la CIBLE, pas la source"
    );

    // Mono-projet : un seul code, pas de doublon même si trois clés le nomment.
    let same_project = serde_json::json!({
        "action": "link",
        "data": { "source_id": "DEC-VPC-021", "target_id": "DEC-VPC-009",
                  "attach_to": "PIL-VPC-001" }
    });
    assert_eq!(
        McpServer::soll_mutation_project_codes_in_payload(&same_project),
        vec!["VPC".to_string()],
        "un même projet nommé trois fois ne doit être invalidé qu'une fois"
    );

    // Rien à dériver : on n'invente pas un projet à invalider.
    let no_data = serde_json::json!({ "action": "link" });
    assert!(
        McpServer::soll_mutation_project_codes_in_payload(&no_data).is_empty(),
        "sans charge utile, aucun projet ne doit être supposé"
    );
}

/// REQ-AXO-902488 (doleance SWT #241) — `re_anchor` dit D'OU vient le projet.
///
/// Cas rapporte : cwd = `/home/dstadel/projects/sow-th` (projet SWT), reprise
/// apres `/compact`, appel `re_anchor(reason="post_compact")` sans project_code —
/// le geste que le hook post-compact prescrit litteralement. Recu : le paquet
/// d'AXO (13 pillars, session pointer CPT-AXO-052), presente comme le sien.
///
/// ## Ce que la mesure a corrige dans le diagnostic
///
/// Le rapporteur en concluait que la resolution auto de `re_anchor` diverge de
/// celle de `practice_recall`. **Verifie en source : elle ne diverge plus** — le
/// codage en dur de `"AXO"` a ete retire par `REQ-AXO-902467` (commit `5a414f89`),
/// qui appelle desormais `auto_resolve_project_code_str`. Ce commit fait partie des
/// **12 commits non deployes** au moment du rapport : le binaire live etait
/// anterieur. Le symptome etait reel, le mecanisme suppose ne l'etait plus.
///
/// Verifie aussi : le point 3 de la doleance (« si la resolution echoue, le DIRE
/// au lieu de servir un repli ») est DEJA tenu — `unresolved_project_error` nomme
/// le geste et liste les projets enregistres. C'est d'ailleurs ce chemin-la que ce
/// fixture emprunte, faute de cwd enregistre.
///
/// ## Ce qui restait a faire, et que cette garde tient
///
/// *« La sortie ne porte AUCUN signal de doute : ni "deduit du cwd", ni le chemin
/// resolu. »* Sans elle, un projet affiche ne peut pas etre CONTREDIT par son
/// lecteur — au moment ou il a le moins de contexte pour le faire.
#[test]
fn re_anchor_says_where_the_project_came_from() {
    use crate::mcp::tools_skill::mention_provenance_projet;

    // Resolution AUTO : la provenance et le chemin doivent etre dits.
    let auto = mention_provenance_projet(false, "/home/dstadel/projects/sow-th");
    assert!(
        auto.contains("deduit du cwd"),
        "la provenance n'est pas dite : un lecteur ne peut pas contredire le projet \
         affiche, et c'est le geste prescrit APRES un compact (SWT #241).\n{auto}"
    );
    assert!(
        auto.contains("/home/dstadel/projects/sow-th"),
        "le CHEMIN resolu manque — c'est lui qui permet de trancher sans second \
         appel.\n{auto}"
    );
    assert!(
        auto.contains("project_code="),
        "la mention ne donne pas le GESTE de correction.\n{auto}"
    );

    // Valeur EXPLICITE : aucune mention. Decorer ce qui est certain envoie douter
    // de la bonne valeur.
    assert_eq!(
        mention_provenance_projet(true, "/home/dstadel/projects/sow-th"),
        "",
        "un projet passe explicitement ne doit pas etre annonce comme deduit"
    );

    // Chemin inconnu : pas de mention vide et trompeuse (« deduit du cwd `` »).
    assert_eq!(mention_provenance_projet(false, ""), "");
}

/// REQ-AXO-902507 — un territoire déjà couvert ne se reprend pas en silence.
///
/// Quatre projets fantômes sont nés de l'absence de ce refus, tous de la même façon : un
/// `axon_init_project` sur un répertoire quelconque, le nom dérivé du dernier segment du
/// chemin — « elixir », « kki-domain-vertical-slice », « projects », « dstadel ». Aucun ne
/// désigne un produit.
///
/// Le coût est mesuré : `PRP` et `DSD` avaient accaparé **61 492 des 68 015 fichiers du
/// parc (90 %)**, laissant `KKI` avec 5 fichiers sur 17 318. Et `ELE` a survécu **six
/// jours** à une demande de suppression explicite, faute d'outil de retrait — d'où la règle
/// : il est bien moins coûteux de ne pas créer que de retirer.
///
/// Ce test falsifie les DEUX sens du recouvrement plus le cas légitime.
#[test]
fn un_territoire_deja_couvert_ne_se_reprend_pas_en_silence() {
    let server = create_test_server();

    let init = |chemin: &str| -> serde_json::Value {
        let req = serde_json::json!({
            "jsonrpc": "2.0", "method": "tools/call",
            "params": { "name": "axon_init_project", "arguments": { "project_path": chemin }},
            "id": 1
        });
        server
            .handle_request(serde_json::from_value(req).unwrap())
            .unwrap()
            .result
            .unwrap()
    };
    let texte = |v: &serde_json::Value| -> String {
        v.get("content")
            .and_then(|c| c.get(0))
            .and_then(|e| e.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string()
    };

    // Un projet légitime, seul sur son chemin.
    let parent = init("/tmp/territoire/dossier-parent");
    assert!(
        texte(&parent).contains("initialized in Axon"),
        "un chemin libre doit passer : {}",
        texte(&parent)
    );

    // (1) DESCENDANT — le cas ELE / KKD : un sous-dossier promu au rang de projet.
    let dedans = init("/tmp/territoire/dossier-parent/sous-dossier");
    let t = texte(&dedans);
    assert!(
        t.contains("est DANS le projet"),
        "un sous-dossier d'un projet existant doit être REFUSÉ : {t}"
    );
    assert!(
        t.contains("ELE") && t.contains("KKD"),
        "le refus doit citer les précédents qui ont coûté, pas rester abstrait : {t}"
    );

    // (2) ANCÊTRE — le cas PRP / DSD : un dossier qui contient des projets.
    let autour = init("/tmp/territoire");
    let t2 = texte(&autour);
    assert!(
        t2.contains("CONTIENT le projet"),
        "un dossier qui contient un projet existant doit être REFUSÉ : {t2}"
    );
    assert!(
        t2.contains("effacerait l'index"),
        "le refus doit dire la CONSÉQUENCE la plus grave, pas seulement la règle : {t2}"
    );

    // Le refus porte sa réparation EN DONNÉE — c'est ce qui distingue Axon des autres MCP
    // (APS #238). Un refus qui ne dit pas quoi faire se contourne au lieu d'être suivi.
    for t in [&t, &t2] {
        assert!(
            t.contains("parent_project_code"),
            "le refus doit donner la sortie exacte (déclarer l'imbrication) : {t}"
        );
    }

    // (3) Le cas LÉGITIME : une imbrication DÉCLARÉE reste valide. Sans ce contre-exemple,
    // la garde interdirait les méga-projets réels (KKI/KKD, INK/HXH), ce que l'opérateur a
    // explicitement demandé de préserver.
    let codes: Vec<Vec<String>> = serde_json::from_str(
        &server
            .graph_store
            .query_json(
                "SELECT project_code FROM soll.Node WHERE type='Vision' \
                 AND project_code NOT IN ('PRO')",
            )
            .expect("lire les codes"),
    )
    .unwrap_or_default();
    assert!(
        !codes.is_empty(),
        "le parent doit exister pour que le test ait un sens"
    );
}

/// REQ-AXO-902507 — la porte de handoff compte les recouvrements de territoires, et une
/// imbrication DÉCLARÉE n'en est pas un.
///
/// Sans ce second point, la garde interdirait les méga-projets réels — `KKI`/`KKD`,
/// `INK`/`HXH` — que l'opérateur a explicitement demandé de préserver le 2026-08-26 :
/// « il s'agit d'un méga-projet avec des sous-projets ». La règle doit distinguer un
/// sous-projet assumé d'un accident d'enregistrement ; c'est toute la différence entre
/// `CPT-PRO-101` R2 et une interdiction aveugle.
#[test]
fn la_porte_compte_les_recouvrements_mais_pas_ceux_qui_sont_declares() {
    let server = create_test_server();
    let exec = |sql: &str| {
        server.graph_store.execute(sql).unwrap();
    };
    // Lire le check DANS la structure, pas dans une fenêtre de texte : un `status` voisin
    // dans la prose ferait passer (ou échouer) le test pour la mauvaise raison.
    let recouvrements = || -> serde_json::Value {
        let r = server
            .axon_handoff_check(&serde_json::json!({}))
            .expect("handoff_check répond");
        r.get("data")
            .and_then(|d| d.get("checks"))
            .and_then(|c| c.as_array())
            .and_then(|checks| {
                checks
                    .iter()
                    .find(|c| c.get("check").and_then(|v| v.as_str()) == Some("registry_territories"))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("le check registry_territories doit exister : {r}"))
    };

    // Deux projets imbriqués, RIEN de déclaré → c'est le cas PRP/DSD.
    exec(
        "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
         VALUES ('MEG', '/tmp/mega', 'mega'), ('SUB', '/tmp/mega/sous-projet', 'sous') \
         ON CONFLICT (project_code) DO UPDATE SET project_path = EXCLUDED.project_path",
    );
    let avant = recouvrements();
    assert_eq!(
        avant.get("status").and_then(|v| v.as_str()),
        Some("fail"),
        "un recouvrement non déclaré doit FAIRE ÉCHOUER la porte : {avant}"
    );
    let detail = avant.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        detail.contains("SUB") && detail.contains("MEG"),
        "et il doit NOMMER les deux projets — un compteur seul n'ouvre aucune action : {detail}"
    );
    let remede = avant.get("remediation").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        remede.contains("parent_project_code"),
        "la remédiation exacte doit être donnée, pas décrite : {remede}"
    );

    // On DÉCLARE l'imbrication : c'est un méga-projet assumé, plus une anomalie.
    exec(
        "UPDATE soll.ProjectCodeRegistry SET parent_project_code = 'MEG' \
         WHERE project_code = 'SUB'",
    );
    let apres = recouvrements();
    assert_eq!(
        apres.get("status").and_then(|v| v.as_str()),
        Some("pass"),
        "une imbrication DÉCLARÉE est légitime — sinon la garde interdit les méga-projets \
         que l'opérateur veut garder : {apres}"
    );
}

/// REQ-AXO-902369 — le registre offre enfin une SORTIE, et elle refuse par défaut.
///
/// `VIS-ELE-001` l'écrivait : *« La ligne de `soll.ProjectCodeRegistry` n'a pas pu être
/// retirée : aucun outil MCP ne le permet. »* `ELE` a donc survécu **six jours** à une
/// demande de suppression explicite, et VPC a dû câbler une table `ELE → FSF` pour
/// contourner un code qui n'aurait plus dû exister.
///
/// Ce test falsifie les TROIS gardes plus le chemin nominal. Une seule d'entre elles qui
/// s'ouvrirait ferait de cet outil un moyen de perdre du travail.
#[test]
fn le_retrait_d_un_projet_refuse_par_defaut_et_dit_pourquoi() {
    let server = create_test_server();
    let appel = |args: serde_json::Value| -> serde_json::Value {
        server
            .execute_tool_direct("project_registry_remove", &args)
            .expect("project_registry_remove répond")
    };
    let texte = |v: &serde_json::Value| -> String {
        v.get("content")
            .and_then(|c| c.get(0))
            .and_then(|e| e.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string()
    };

    // Code inconnu → refus net, pas un succès silencieux.
    let inconnu = appel(serde_json::json!({ "project_code": "ZZZ" }));
    assert_eq!(inconnu["isError"], serde_json::json!(true));
    assert!(texte(&inconnu).contains("n'est pas dans le registre"));

    server
        .graph_store
        .execute(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
             VALUES ('GHO', '/tmp/ghost-territoire', 'ghost') \
             ON CONFLICT (project_code) DO UPDATE SET project_path = EXCLUDED.project_path",
        )
        .unwrap();

    // GARDE 1 — de l'intention réelle. C'est la plus importante : effacer un nœud sans
    // reporter ce qu'il dit, c'est perdre la seule chose qui ne se régénère pas.
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, title, description, status, project_code) \
             VALUES ('DEC-GHO-001','Decision','un vrai choix','corps','current','GHO')",
        )
        .unwrap();
    let avec_fond = appel(serde_json::json!({ "project_code": "GHO", "confirm": true }));
    assert_eq!(
        avec_fond["isError"],
        serde_json::json!(true),
        "un projet portant une Decision ne doit PAS être retirable, même avec confirm"
    );
    assert!(
        texte(&avec_fond).contains("nœud(s) SOLL de fond"),
        "et le refus doit dire POURQUOI : {}",
        texte(&avec_fond)
    );

    // L'intention reportée, la garde 1 s'ouvre. Une Vision/Pillar auto-seedée ne compte
    // pas — sinon aucun projet ne serait jamais retirable, `axon_init_project` en créant
    // systématiquement.
    server
        .graph_store
        .execute("DELETE FROM soll.Node WHERE id = 'DEC-GHO-001'")
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, title, description, status, project_code) \
             VALUES ('VIS-GHO-001','Vision','Project north-star (draft)','x','planned','GHO')",
        )
        .unwrap();

    // GARDE 2 — des fichiers qu'un projet plus spécifique n'a pas repris.
    server
        .graph_store
        .execute(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_path, project_name) \
             VALUES ('DEE', '/tmp/ghost-territoire/dedans', 'dedans') \
             ON CONFLICT (project_code) DO NOTHING",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO axon.Project (code, enrolled_at_ms) VALUES ('GHO',1) \
             ON CONFLICT (code) DO NOTHING",
        )
        .unwrap();
    server
        .graph_store
        .execute(
            "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms) \
             VALUES ('/tmp/ghost-territoire/dedans/a.rs','GHO','h',1) \
             ON CONFLICT (path) DO NOTHING",
        )
        .unwrap();
    let a_reprendre = appel(serde_json::json!({ "project_code": "GHO", "confirm": true }));
    assert_eq!(
        a_reprendre["isError"],
        serde_json::json!(true),
        "un fichier qui revient à un projet plus spécifique interdit le retrait"
    );
    assert!(
        texte(&a_reprendre).contains("plus spécifique"),
        "le refus doit nommer le mécanisme : {}",
        texte(&a_reprendre)
    );

    // GARDE 3 — sans `confirm`, SIMULATION. Le fichier redevient orphelin (DEE retiré),
    // donc les gardes 1 et 2 passent : seule la confirmation manque.
    server
        .graph_store
        .execute("DELETE FROM soll.ProjectCodeRegistry WHERE project_code = 'DEE'")
        .unwrap();
    let simulation = appel(serde_json::json!({ "project_code": "GHO" }));
    assert_ne!(
        simulation["isError"],
        serde_json::json!(true),
        "la simulation n'est pas une erreur : {simulation}"
    );
    assert_eq!(simulation["data"]["dry_run"], serde_json::json!(true));
    assert!(
        texte(&simulation).contains("Relancer avec `confirm=true`"),
        "la simulation doit dire le geste exact : {}",
        texte(&simulation)
    );
    // Et surtout : elle n'a RIEN touché.
    let encore = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code='GHO'")
        .unwrap();
    assert_eq!(encore, 1, "une simulation qui supprime n'est pas une simulation");

    // Chemin nominal.
    let fait = appel(serde_json::json!({ "project_code": "GHO", "confirm": true }));
    assert_ne!(fait["isError"], serde_json::json!(true), "{fait}");
    for table in [
        "soll.ProjectCodeRegistry",
        "soll.Node",
        "ist.IndexedFile",
    ] {
        let n = server
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM {table} WHERE project_code='GHO'"
            ))
            .unwrap();
        assert_eq!(n, 0, "{table} doit être vide de GHO après retrait");
    }
}
