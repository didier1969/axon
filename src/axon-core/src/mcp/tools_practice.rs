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

/// REQ-AXO-902132 — does the practice DESCRIBE a failure mode / lesson-learned?
/// Such a practice ("a killed promote leaves query-embed dead") legitimately tensions
/// with the healthy-state base, so a `contradicts` verdict from the write-gate is
/// downgraded to advisory rather than a hard reject (the whole point of a lessons
/// memory is to capture failures). Conservative keyword match over context+practice.
fn is_failure_mode(context: &str, practice: &str) -> bool {
    const MARKERS: &[&str] = &[
        // EN
        "fail", "breaks", "broke", "crash", "bug", "leak", "regression", "race",
        "deadlock", "oom", "panic", "stale", "timeout", "wedge", "corrupt", "dead",
        "hang", "stall", "interrupted", "killed", "abort", "degraded", "pitfall",
        "gotcha", "footgun", "anti-pattern", "antipattern", "when it goes wrong",
        // FR
        "échec", "échoue", "casse", "cassé", "plante", "fuite", "régression",
        "interrompu", "tué", "corrompu", "bloqué", "mort", "panne", "piège",
        "ne pas", "à éviter", "attention", "mode d'échec",
    ];
    let hay = format!("{context} {practice}").to_lowercase();
    MARKERS.iter().any(|m| hay.contains(m))
}

/// REQ-AXO-902137 — cosine-distance threshold under which two practices count as
/// near-duplicates and get fused. 0.07 ≈ cosine similarity > 0.93 (BGE-large 1024d).
const FUSION_COSINE_EPS: f32 = 0.07;

/// REQ-AXO-902137 — two practices are near-duplicates (fuse candidates) when their
/// embedding cosine distance is below `eps`. Pure + unit-testable.
fn should_fuse(dist: f32, eps: f32) -> bool {
    dist < eps
}

/// REQ-AXO-902137 — fold the provenance (`source_project`) of a fused duplicate
/// into the representative's: comma-separated union, insertion-order preserved,
/// blanks dropped, no loss of contributing tenant. Pure + unit-testable.
fn merge_provenance(existing: &str, incoming: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for tok in existing.split(',').chain(incoming.split(',')) {
        let t = tok.trim();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out.join(",")
}

/// REQ-AXO-902136 — dense encoding is CALLER-PROVIDED (the brain has no embedded
/// LLM to compact prose; the calling agent IS the compactor). The brain only
/// VALIDATES + normalises: trims ; an empty dense → fall back to the prose
/// `practice` (stored as `''`) ; a dense that is NOT shorter than the prose it
/// densifies is kept but flagged advisory (the caller mis-used the field). Pure +
/// unit-testable. Returns `(stored_dense, advisory_note)`.
fn resolve_dense_form(dense: &str, practice: &str) -> (String, Option<&'static str>) {
    let d = dense.trim();
    if d.is_empty() {
        return (String::new(), None);
    }
    let advisory = if d.chars().count() >= practice.chars().count() {
        Some("dense_not_shorter_than_practice")
    } else {
        None
    };
    (d.to_string(), advisory)
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
        let dense_arg = args.get("dense").and_then(Value::as_str).unwrap_or("");
        if context.is_empty() || practice.is_empty() {
            return Some(practice_err("context and practice are required", "input_invalid"));
        }
        // REQ-AXO-902136 — caller-provided dense form (brain validates, no LLM here).
        let (dense, dense_advisory) = resolve_dense_form(dense_arg, practice);

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
        // REQ-AXO-902132 — a lessons-memory EXISTS to capture failure modes. A practice
        // that DESCRIBES a failure ("a killed promote leaves query-embed dead") legitimately
        // tensions with the healthy-state base, so a `contradicts` verdict is ADVISORY
        // (store + warn) for failure-framed practices, and a HARD REJECT (anti-poison) only
        // for a factual contradiction of the base (e.g. "Axon uses MongoDB").
        let failure_framed = is_failure_mode(context, practice);
        let gate_label: &str = if verdict == "contradicts" {
            if failure_framed {
                "advisory_failure_mode"
            } else {
                let conflicts = gate
                    .as_ref()
                    .and_then(|g| g.get("data"))
                    .and_then(|d| d.get("conflicts").or_else(|| d.get("top_conflicts")))
                    .cloned()
                    .unwrap_or(Value::Null);
                return Some(json!({
                    "content": [{"type":"text","text": format!(
                        "### 🧠 practice_put — REJECTED (write-gate): the practice CONTRADICTS the indexed base of `{scope}` (and is not framed as a failure-mode lesson). Fix the practice or the base, then retry."
                    )}],
                    "isError": true,
                    "data": {"status": "write_gate_rejected", "verdict": verdict, "conflicts": conflicts}
                }));
            }
        } else {
            verdict
        };

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
            "INSERT INTO axon.practice (scope, context, practice, dense, evidence, embedding, source_project) \
             VALUES ('{}', '{}', '{}', '{}', '{}', {}, '{}') \
             ON CONFLICT (scope, md5(practice)) DO UPDATE \
               SET dense = EXCLUDED.dense, \
                   evidence = EXCLUDED.evidence, \
                   embedding = COALESCE(EXCLUDED.embedding, axon.practice.embedding), \
                   updated_at = now() \
             RETURNING id, (xmax = 0) AS inserted",
            esc(&scope),
            esc(context),
            esc(practice),
            esc(&dense),
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

        let dense_state = if dense.is_empty() { "prose_fallback" } else { "dense" };
        Some(json!({
            "content": [{"type":"text","text": format!(
                "### 🧠 practice_put — {} · scope=`{scope}` · gate={gate_label} · embed={embed_state} · encoding={dense_state} · id={id}{}",
                if inserted {"stored"} else {"updated"},
                dense_advisory.map(|a| format!(" · ⚠️ {a}")).unwrap_or_default()
            )}],
            "data": {"status":"ok","id":id,"inserted":inserted,"scope":scope,"gate":gate_label,"embed":embed_state,
                     "encoding":dense_state,"dense_advisory":dense_advisory}
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
            "SELECT id, scope, COALESCE(NULLIF(dense,''), practice) AS practice, evidence, trust, stability, \
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

    /// REQ-AXO-902137 — semantic fusion of near-duplicate practices. Greedy per
    /// representative (highest trust first): a practice within `FUSION_COSINE_EPS`
    /// cosine distance of a stronger one is FUSED — its use/win counts + provenance
    /// fold into the representative and it is marked `status='merged'` (never
    /// DELETE, audit-preserving like 'pruned'). The brain has no LLM so it does NOT
    /// synthesize new text; it keeps the strongest representative and aggregates
    /// governance. `scope_filter` is the tick-style `AND scope = '…'` (or empty).
    /// Returns the number of practices fused away.
    fn fuse_near_duplicates(&self, scope_filter: &str) -> u32 {
        let reps_sql = format!(
            "SELECT id, use_count, win_count, source_project FROM axon.practice \
             WHERE status='active' AND embedding IS NOT NULL {scope_filter} \
             ORDER BY trust DESC, use_count DESC, id ASC"
        );
        let reps: Vec<Vec<Value>> = self
            .graph_store
            .query_json_writer(&reps_sql)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let i64c = |r: &Vec<Value>, i: usize| {
            r.get(i)
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0)
        };
        let strc = |r: &Vec<Value>, i: usize| r.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut consumed: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut fused = 0u32;
        for rep in &reps {
            let rep_id = i64c(rep, 0);
            if rep_id == 0 || consumed.contains(&rep_id) {
                continue;
            }
            // Near-duplicates of rep: same scope, active, cosine dist < eps. The
            // self-join compares embeddings in-DB (no vector round-trips to Rust).
            let dup_sql = format!(
                "SELECT b.id, b.use_count, b.win_count, b.source_project \
                 FROM axon.practice a JOIN axon.practice b ON b.scope = a.scope \
                 WHERE a.id = {rep_id} AND b.id <> a.id AND b.status='active' \
                   AND b.embedding IS NOT NULL AND a.embedding IS NOT NULL \
                   AND (a.embedding <=> b.embedding) < {eps}",
                eps = FUSION_COSINE_EPS
            );
            let dups: Vec<Vec<Value>> = self
                .graph_store
                .query_json_writer(&dup_sql)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let (mut add_use, mut add_win) = (0i64, 0i64);
            let mut prov = strc(rep, 3);
            let mut merged_ids: Vec<i64> = Vec::new();
            for d in &dups {
                let did = i64c(d, 0);
                if did == 0 || consumed.contains(&did) {
                    continue;
                }
                consumed.insert(did);
                merged_ids.push(did);
                add_use += i64c(d, 1);
                add_win += i64c(d, 2);
                prov = merge_provenance(&prov, &strc(d, 3));
            }
            if merged_ids.is_empty() {
                continue;
            }
            consumed.insert(rep_id);
            let ids_list = merged_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            // Fold governance into the representative…
            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.practice SET use_count = use_count + {add_use}, \
                        win_count = win_count + {add_win}, source_project = '{}', updated_at = now() \
                 WHERE id = {rep_id}",
                esc(&prov)
            ));
            // …and tombstone the duplicates (never DELETE — audit).
            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.practice SET status='merged', updated_at = now() WHERE id IN ({ids_list})"
            ));
            fused += merged_ids.len() as u32;
        }
        fused
    }

    /// REQ-AXO-902131 — maintenance tick: FSRS decay of trust + prune of collapsed
    /// practices (status='pruned', never DELETE) + stagnation verdict.
    /// REQ-AXO-902137 — also fuses near-duplicates first (cluster → strongest rep).
    pub(crate) fn axon_practice_tick(&self, args: &Value) -> Option<Value> {
        let scope_filter = match args.get("scope").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
            Some(s) => format!("AND scope = '{}'", esc(s)),
            None => String::new(),
        };
        // REQ-AXO-902137 — fuse near-duplicates BEFORE decay so the decay/prune
        // pass operates on the de-duplicated set.
        let fused = self.fuse_near_duplicates(&scope_filter);
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
                "### 🧠 practice_tick — fused {fused} · decayed {decayed} · pruned {pruned} · mean_trust {:.2} · stagnation={}",
                mean_trust, stag.stagnating
            )}],
            "data": {"status":"ok","fused":fused,"decayed":decayed,"pruned":pruned,"mean_trust":mean_trust,
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

#[cfg(test)]
mod tests {
    use super::{is_failure_mode, merge_provenance, resolve_dense_form, should_fuse, FUSION_COSINE_EPS};

    #[test]
    fn should_fuse_902137() {
        // below eps → fuse ; at/above eps → keep distinct.
        assert!(should_fuse(0.02, FUSION_COSINE_EPS));
        assert!(!should_fuse(FUSION_COSINE_EPS, FUSION_COSINE_EPS));
        assert!(!should_fuse(0.5, FUSION_COSINE_EPS));
    }

    #[test]
    fn merge_provenance_902137() {
        // union, insertion-order preserved, blanks dropped, dedup.
        assert_eq!(merge_provenance("NEX", "AXO"), "NEX,AXO");
        assert_eq!(merge_provenance("NEX,AXO", "AXO"), "NEX,AXO");
        assert_eq!(merge_provenance("", "NEX"), "NEX");
        assert_eq!(merge_provenance(" NEX , ", " AXO ,NEX"), "NEX,AXO");
        assert_eq!(merge_provenance("", ""), "");
    }

    #[test]
    fn resolve_dense_form_902136() {
        // empty dense → prose fallback (stored ''), no advisory.
        assert_eq!(resolve_dense_form("", "the full prose practice"), (String::new(), None));
        assert_eq!(resolve_dense_form("   ", "prose"), (String::new(), None));
        // a genuinely denser form → stored trimmed, no advisory.
        let (d, adv) = resolve_dense_form("  use exact scan <=5k rows  ", "When the scope is small, prefer an exact scan over HNSW because it bypasses corruption.");
        assert_eq!(d, "use exact scan <=5k rows");
        assert_eq!(adv, None);
        // a dense NOT shorter than the prose → kept but flagged advisory.
        let (d2, adv2) = resolve_dense_form("this is actually longer than the source", "short");
        assert_eq!(d2, "this is actually longer than the source");
        assert_eq!(adv2, Some("dense_not_shorter_than_practice"));
    }

    #[test]
    fn failure_mode_detection_902132() {
        // failure-framed lessons → advisory (not rejected)
        assert!(is_failure_mode("livraison promote", "un promote tué laisse query-embed mort"));
        assert!(is_failure_mode("pipeline", "the indexer OOMs at full throughput"));
        assert!(is_failure_mode("HNSW", "duplicate vectors corrupt the graph"));
        // factual non-failure claims → NOT failure-framed (a contradiction stays a reject)
        assert!(!is_failure_mode("database choice", "Axon uses MongoDB for everything"));
        assert!(!is_failure_mode("vector search", "use exact scan for small scopes"));
    }
}
