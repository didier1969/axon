//! REQ-AXO-902131 — best-practice memory MCP handlers.
//!
//! `practice_put` (WRITE-GATED by contradiction_check), `practice_recall` (scoped
//! pgvector), `practice_tick` (FSRS decay + prune), `practice_card` (summary). The
//! governance math is pure in [`crate::practice_memory`]; the DB ops + the internal
//! gate call live here (same writer/reader split as `tools_mailbox`).

use serde_json::{json, Value};

use super::McpServer;
use crate::practice_memory::{assess_stagnation, decay_trust, retrievability, should_prune};

/// A bounded scope is scanned EXACTLY (bypass HNSW — the REQ-AXO-902129 lesson).
const EXACT_SCAN_MAX: i64 = 50_000;

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn practice_err(msg: &str, status: &str) -> Value {
    json!({
        "content": [{"type":"text","text": format!("### 🧠 practice — {msg}")}],
        "isError": true,
        "data": {"status": status, "error": msg}
    })
}

impl McpServer {
    fn resolve_practice_scope(&self, args: &Value) -> String {
        args.get("scope")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.auto_resolve_project_code_str())
            .unwrap_or_else(|| "AXO".to_string())
    }

    /// REQ-AXO-902131 — store a governed best practice. WRITE-GATED: a practice that
    /// CONTRADICTS the scope's indexed base is rejected (anti-poison, SSGM gate). A
    /// `neutral`/`inconclusive`/unavailable gate passes (availability > rigidity; the
    /// tick/prune loop corrects a bad practice over time, and a fresh scope has no
    /// base to contradict).
    pub(crate) fn axon_practice_put(&self, args: &Value) -> Option<Value> {
        let scope = self.resolve_practice_scope(args);
        let context = args.get("context").and_then(Value::as_str).unwrap_or("").trim();
        let practice = args.get("practice").and_then(Value::as_str).unwrap_or("").trim();
        let evidence = args.get("evidence").and_then(Value::as_str).unwrap_or("");
        let source = args.get("from").and_then(Value::as_str).unwrap_or("");
        if context.is_empty() || practice.is_empty() {
            return Some(practice_err("context and practice are required", "input_invalid"));
        }

        // --- WRITE-GATE: reject a practice that contradicts the scope's base. ---
        let gate_args = json!({
            "candidate": format!("{context}\n{practice}"),
            "scope": {"project": scope},
            "threshold": 0.5
        });
        let gate = self.axon_contradiction_check(&gate_args);
        let verdict = gate
            .as_ref()
            .and_then(|g| g.get("data"))
            .and_then(|d| d.get("verdict"))
            .and_then(Value::as_str)
            .unwrap_or("ungated");
        if verdict == "contradicts" {
            let conflicts = gate
                .as_ref()
                .and_then(|g| g.get("data"))
                .and_then(|d| d.get("conflicts").or_else(|| d.get("top_conflicts")))
                .cloned()
                .unwrap_or(Value::Null);
            return Some(json!({
                "content": [{"type":"text","text": format!(
                    "### 🧠 practice_put — REJECTED (write-gate): the practice CONTRADICTS the indexed base of `{scope}`. Fix the practice or the base, then retry."
                )}],
                "isError": true,
                "data": {"status": "write_gate_rejected", "verdict": verdict, "conflicts": conflicts}
            }));
        }

        // --- EMBED the context (recall signature). NULL-safe if the worker is down. ---
        let embed_lit = match crate::embedder::batch_embed(vec![context.to_string()]) {
            Ok(v) => v
                .first()
                .and_then(|e| crate::postgres::vector::vector_literal(e).ok()),
            Err(_) => None,
        };
        // vector_literal already returns a QUOTED literal ('[...]'); append the cast
        // only — re-quoting doubles the quotes → SQL syntax error (caught by dev E2E).
        let embed_sql = embed_lit
            .as_deref()
            .map(|lit| format!("{lit}::vector"))
            .unwrap_or_else(|| "NULL".to_string());
        let embed_state = if embed_lit.is_some() { "embedded" } else { "deferred" };

        // --- UPSERT idempotent: re-put refreshes evidence, keeps accrued governance. ---
        let sql = format!(
            "INSERT INTO axon.practice (scope, context, practice, evidence, embedding, source_project) \
             VALUES ('{}', '{}', '{}', '{}', {}, '{}') \
             ON CONFLICT (scope, md5(practice)) DO UPDATE \
               SET evidence = EXCLUDED.evidence, \
                   embedding = COALESCE(EXCLUDED.embedding, axon.practice.embedding), \
                   updated_at = now() \
             RETURNING id, (xmax = 0) AS inserted",
            esc(&scope),
            esc(context),
            esc(practice),
            esc(evidence),
            embed_sql,
            esc(source)
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(practice_err(&format!("store failed: {e}"), "degraded")),
        };
        let (id, inserted) = rows
            .first()
            .map(|r| {
                let id = r.first().and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
                let ins = r.get(1).map(|v| v.as_bool().unwrap_or(false) || v.as_str() == Some("t")).unwrap_or(false);
                (id, ins)
            })
            .unwrap_or((0, false));

        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 🧠 practice_put — {} · scope=`{scope}` · gate={verdict} · embed={embed_state} · id={id}",
                if inserted {"stored"} else {"updated"}
            )}],
            "data": {"status":"ok","id":id,"inserted":inserted,"scope":scope,"gate":verdict,"embed":embed_state}
        }))
    }

    /// REQ-AXO-902131 — recall the most relevant governed practices for a query,
    /// scoped to the project + global ('*'). Re-ranks ANN by governance
    /// (trust × retrievability) and lightly reinforces the recalled ones (flux).
    pub(crate) fn axon_practice_recall(&self, args: &Value) -> Option<Value> {
        let scope = self.resolve_practice_scope(args);
        let query = args.get("query").and_then(Value::as_str).unwrap_or("").trim();
        let top_k = args.get("top_k").and_then(Value::as_u64).unwrap_or(5).clamp(1, 50) as usize;
        if query.is_empty() {
            return Some(practice_err("query is required", "input_invalid"));
        }
        let emb = match crate::embedder::batch_embed(vec![query.to_string()]) {
            Ok(v) => v.into_iter().next(),
            Err(e) => return Some(practice_err(&format!("embed failed: {e}"), "degraded")),
        };
        let vec_lit = match emb.as_ref().and_then(|e| crate::postgres::vector::vector_literal(e).ok()) {
            Some(l) => l,
            None => return Some(practice_err("query produced no embedding", "degraded")),
        };

        let scope_filter = format!("scope IN ('{}', '*')", esc(&scope));
        let scope_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM axon.practice WHERE status='active' AND embedding IS NOT NULL AND {scope_filter}"
            ))
            .unwrap_or(-1);
        let pool = (top_k * 3).clamp(top_k, 60);
        let sql = format!(
            "SELECT id, scope, practice, evidence, trust, stability, \
                    EXTRACT(EPOCH FROM (now() - last_used_at))/86400.0 AS days_since, \
                    (embedding <=> {vec_lit}::vector) AS dist \
             FROM axon.practice \
             WHERE status='active' AND embedding IS NOT NULL AND {scope_filter} \
             ORDER BY embedding <=> {vec_lit}::vector LIMIT {pool}"
        );
        let ann = if scope_count > 0 && scope_count <= EXACT_SCAN_MAX {
            self.graph_store.query_exact_scan_json(&sql)
        } else {
            self.graph_store.query_ann_json(&sql, (pool as u32).max(40).min(1000))
        };
        let rows: Vec<Vec<Value>> = match ann {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(practice_err(&format!("recall failed: {e}"), "degraded")),
        };

        let f = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) as f32;
        let mut scored: Vec<(f32, i64, Value)> = rows
            .iter()
            .map(|r| {
                let g = |i: usize| r.get(i).cloned().unwrap_or(Value::Null);
                let id = r.first().and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
                let trust = f(&g(4));
                let stability = f(&g(5));
                let days = f(&g(6));
                let dist = f(&g(7));
                let r_ret = retrievability(days, stability);
                // governance-weighted score: closeness × trust × retrievability.
                let score = (1.0 - dist).max(0.0) * (0.5 + 0.5 * trust) * r_ret;
                (score, id, json!({
                    "id": id,
                    "scope": g(1),
                    "practice": g(2),
                    "evidence": g(3),
                    "trust": trust,
                    "score": score
                }))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // Light Physarum flux: reinforce + mark the recalled practices.
        let recalled_ids: Vec<String> = scored.iter().map(|(_, id, _)| id.to_string()).collect();
        if !recalled_ids.is_empty() {
            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.practice \
                 SET use_count = use_count + 1, last_used_at = now(), \
                     trust = LEAST(1.0, trust + {gain} * (1.0 - trust)) \
                 WHERE id IN ({ids})",
                gain = crate::practice_memory::TRUST_REINFORCE_GAIN * 0.1,
                ids = recalled_ids.join(",")
            ));
        }
        let practices: Vec<Value> = scored.into_iter().map(|(_, _, v)| v).collect();
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 🧠 practice_recall — {} practice(s) · scope=`{scope}` (+global)", practices.len()
            )}],
            "data": {"status":"ok","scope":scope,"count":practices.len(),"practices":practices}
        }))
    }

    /// REQ-AXO-902131 — maintenance tick: FSRS decay of trust + prune of collapsed
    /// practices (status='pruned', never DELETE) + stagnation verdict.
    pub(crate) fn axon_practice_tick(&self, args: &Value) -> Option<Value> {
        let scope_filter = match args.get("scope").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(s) => format!("AND scope = '{}'", esc(s)),
            None => String::new(),
        };
        let sql = format!(
            "SELECT id, trust, stability, use_count, \
                    EXTRACT(EPOCH FROM (now() - last_used_at))/86400.0 AS days_since, \
                    EXTRACT(EPOCH FROM (now() - created_at))/86400.0 AS age_days \
             FROM axon.practice WHERE status='active' {scope_filter}"
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(practice_err(&format!("tick load failed: {e}"), "degraded")),
        };
        let f = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) as f32;
        let (mut decayed, mut pruned) = (0u32, 0u32);
        for r in &rows {
            let id = r.first().and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
            let trust = f(r.get(1).unwrap_or(&Value::Null));
            let stability = f(r.get(2).unwrap_or(&Value::Null));
            let use_count = r.get(3).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0) as i32;
            let days_since = f(r.get(4).unwrap_or(&Value::Null));
            let age_days = f(r.get(5).unwrap_or(&Value::Null));
            let r_ret = retrievability(days_since, stability);
            let new_trust = decay_trust(trust, r_ret);
            let prune = should_prune(new_trust, r_ret, use_count, age_days);
            let status = if prune { "pruned" } else { "active" };
            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.practice SET trust = {new_trust}, status = '{status}', updated_at = now() WHERE id = {id}"
            ));
            decayed += 1;
            if prune {
                pruned += 1;
            }
        }
        // stagnation over the 30d window.
        let adds = self.graph_store.query_count(&format!("SELECT count(*) FROM axon.practice WHERE created_at > now() - interval '30 days' {scope_filter}")).unwrap_or(0) as i32;
        let prunes30 = self.graph_store.query_count(&format!("SELECT count(*) FROM axon.practice WHERE status='pruned' AND updated_at > now() - interval '30 days' {scope_filter}")).unwrap_or(0) as i32;
        let mean_trust = self.graph_store.query_count(&format!("SELECT round(avg(trust)*1000) FROM axon.practice WHERE status='active' {scope_filter}")).unwrap_or(500) as f32 / 1000.0;
        let stag = assess_stagnation(adds, prunes30, decayed as i32, mean_trust, mean_trust);
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 🧠 practice_tick — decayed {decayed} · pruned {pruned} · mean_trust {:.2} · stagnation={}",
                mean_trust, stag.stagnating
            )}],
            "data": {"status":"ok","decayed":decayed,"pruned":pruned,"mean_trust":mean_trust,
                     "stagnation":{"stagnating":stag.stagnating,"churn":stag.churn,"reason":stag.reason}}
        }))
    }

    /// REQ-AXO-902131 — per-scope summary: counts, mean governance, top practices.
    pub(crate) fn axon_practice_card(&self, args: &Value) -> Option<Value> {
        let scope = self.resolve_practice_scope(args);
        let scope_filter = format!("scope IN ('{}', '*')", esc(&scope));
        let active = self.graph_store.query_count(&format!("SELECT count(*) FROM axon.practice WHERE status='active' AND {scope_filter}")).unwrap_or(0);
        let prunedc = self.graph_store.query_count(&format!("SELECT count(*) FROM axon.practice WHERE status='pruned' AND {scope_filter}")).unwrap_or(0);
        let mean_trust = self.graph_store.query_count(&format!("SELECT round(avg(trust)*1000) FROM axon.practice WHERE status='active' AND {scope_filter}")).unwrap_or(500) as f32 / 1000.0;
        let top_sql = format!(
            "SELECT practice, round(trust*100)/100.0, use_count FROM axon.practice \
             WHERE status='active' AND {scope_filter} ORDER BY trust DESC, use_count DESC LIMIT 5"
        );
        let top_rows: Vec<Vec<Value>> = self
            .graph_store
            .query_json(&top_sql)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let top: Vec<Value> = top_rows
            .iter()
            .map(|r| json!({
                "practice": r.first().cloned().unwrap_or(Value::Null),
                "trust": r.get(1).cloned().unwrap_or(Value::Null),
                "use_count": r.get(2).cloned().unwrap_or(Value::Null)
            }))
            .collect();
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 🧠 practice_card `{scope}` — {active} active, {prunedc} pruned, mean trust {:.2}", mean_trust
            )}],
            "data": {"status":"ok","scope":scope,"active":active,"pruned":prunedc,"mean_trust":mean_trust,"top":top}
        }))
    }
}
