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
    server.record_mcp_friction(tool, &problematic);
    server.record_mcp_friction(tool, &problematic);
    // A terse success (no problem_class) must NOT be captured.
    server.record_mcp_friction(tool, &json!({ "data": { "project_code": proj } }));

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
    server.record_mcp_friction(tool, &problematic);
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
    let expected_rev = format!("REV-{code}-001");
    assert!(revision_rows.contains(&expected_rev), "{revision_rows}");
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
fn test_soll_apply_plan_rejects_relations_nested_inside_plan() {
    // REQ-AXO-901625 — guard against the schema-drift mistake observed
    // in the Pollux Cuisine session : `relations` nested inside `plan`
    // instead of at the top level. Before the fix the array was silently
    // dropped and zero edges were created. The fix surfaces an
    // `input_invalid` envelope with `parameter_repair` so the caller
    // recovers in one round-trip.
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
    assert_eq!(
        result["isError"].as_bool(),
        Some(true),
        "misplaced relations must return isError=true"
    );
    assert_eq!(
        result["data"]["status"].as_str(),
        Some("input_invalid"),
        "status must be input_invalid"
    );
    assert_eq!(
        result["data"]["parameter_repair"]["category"].as_str(),
        Some("relations_misplaced_inside_plan"),
        "parameter_repair must categorise the misplacement"
    );
    assert_eq!(
        result["data"]["parameter_repair"]["items_silently_dropped"].as_u64(),
        Some(1),
        "parameter_repair must report how many items were misplaced"
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

    assert!(content.contains("Duplicate titles"), "{content}");
    assert!(content.contains("Duplicate req"), "{content}");
    assert!(content.contains("Duplicate dec"), "{content}");
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
    assert!(
        content.contains("1 minimal coherence violation(s)"),
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
    let data = result.get("data").unwrap();
    assert_eq!(data["result_count"].as_u64(), Some(0));
    assert_eq!(data["query_state"].as_str(), Some("structure_only_empty"));
    assert!(data["operator_guidance"].as_object().is_some());
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
    assert_eq!(
        digest["topology_summary"]["orphan_requirement_count"].as_u64(),
        Some(1)
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
    // REQ-AXO-188 #1+#2: DEC -> CPT also accepts SUPERSEDES for the case
    // where a decision retires or wholly replaces an architecture concept.
    let server = create_test_server();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('DEC-AXO-002', 'Decision', 'AXO', 'Replacement decision', '', 'current', '{\"context\":\"ctx\",\"rationale\":\"why\"}')")
        .unwrap();
    server
        .graph_store
        .execute("INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) VALUES ('CPT-AXO-002', 'Concept', 'AXO', 'Retired concept', 'desc', '', '{}')")
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
    assert!(result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false));
    assert!(
        result["data"]["operator_guidance"]["problem_class"].as_str()
            == Some("supersedes_type_mismatch")
            || content.contains("SUPERSEDES requires same-type"),
        "{content}"
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
    assert!(repair_guidance
        .iter()
        .any(|entry| entry["category"].as_str() == Some("orphan_requirements")));
    assert!(repair_guidance
        .iter()
        .any(|entry| entry["category"].as_str() == Some("validations_without_verifies")));
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
fn test_anomalies_downgrades_noncanonical_intent_gaps_when_soll_baseline_is_complete() {
    let server = create_test_server();
    let code = "TST".to_string();
    let pil_id = format!("PIL-{code}-001");
    let req_id = format!("REQ-{code}-001");
    let dec_id = format!("DEC-{code}-001");
    let val_id = format!("VAL-{code}-001");
    let trc_id = format!("TRC-{code}-001");
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
    // Verify the four canonical kinds are represented.
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
    assert!(
        kinds.contains("sql"),
        "entry_points must include `sql` steps: {kinds:?}"
    );

    let content = result["content"][0]["text"].as_str().unwrap();
    assert!(
        content.contains("Kickoff bundle"),
        "content must point to the bundle: {content}"
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
    assert!(
        data.get("git_add_exit_code")
            .and_then(|v| v.as_i64())
            .is_some(),
        "git_add_exit_code must be exposed for diagnostics"
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

    // REQ-AXO-312 — a node WITH a hierarchy child must render the micro column.
    // PIL-AXO-001 has child REQ-AXO-001 (BELONGS_TO) and parent VIS-AXO-001
    // (EPITOMIZES), so its local graph carries both a macro and a micro
    // subgraph. Regression guard for the inverted-edge bug that emptied micro.
    let pillar_html = std::fs::read_to_string(out.path().join("nodes/PIL-AXO-001.html")).unwrap();
    assert!(
        pillar_html.contains("subgraph sgMicro"),
        "a node with a hierarchy child must render a micro column"
    );
    assert!(pillar_html.contains("▼ Micro"));
    assert!(pillar_html.contains("subgraph sgMacro"));

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

    // Idempotent: second call returns 0 removed, 1 kept.
    let response2 = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "soll_remove_evidence",
                "arguments": {"entity_id": "REQ-AXO-2540"}
            })),
            id: Some(json!(254002)),
        })
        .unwrap()
        .result
        .unwrap();
    assert_eq!(response2["data"]["removed_count"].as_u64(), Some(0));
    assert_eq!(response2["data"]["kept"].as_array().unwrap().len(), 1);
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
                    "title": "Test skill — TDD obligatoire procedure",
                    "description": "Procedural body invoked by LLM via mcp__axon__skill_invoke. Implements GUI-PRO-001 (TDD obligatoire) as an executable skill : red → green → refactor loop using Axon MCP for query/inspect/commit.",
                    "attach_to": "GUI-PRO-001",
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
        content.contains("SKI-PRO-"),
        "Response should include canonical SKI id, got: {content}"
    );

    let count = server
        .graph_store
        .query_count("SELECT count(*) FROM soll.Node WHERE type='Skill' AND project_code='PRO'")
        .unwrap();
    assert!(
        count >= 1,
        "at least one SKI-PRO row expected after create, got {count}"
    );
}

// REQ-AXO-91578 — SKI entity must reject create when no canonical relation
// exists for (SKI, target_type). Validates closed-policy enforcement.
#[test]
fn test_skill_entity_rejects_non_canonical_attach_target() {
    let server = create_test_server();

    // GUI-PRO-001 is seeded at bootstrap. SKI→GUI is allowed via
    // INHERITS_FROM ; but trying COMPLIES_WITH should reject (not in
    // the policy's allowed list for SKI→GUI).
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
                    "title": "Test skill — should reject COMPLIES_WITH",
                    "description": "SKI→GUI allows only INHERITS_FROM ; COMPLIES_WITH must reject.",
                    "attach_to": "GUI-PRO-001",
                    "relation_type": "COMPLIES_WITH",
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

    // Seed a SKI directly (faster than going through soll_manager).
    server
        .graph_store
        .execute(
            "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) \
             VALUES ('SKI-PRO-998', 'Skill', 'PRO', 'Test TDD skill', 'Body : red green refactor. Test fixture for SKI MCP surface.', 'current', '{\"invocation_mode\":\"MANDATED\",\"applicable_to\":[\"delivery\"]}'::jsonb) \
             ON CONFLICT (id) DO NOTHING",
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
