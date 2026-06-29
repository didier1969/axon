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

/// REQ-AXO-902154 — does the practice read as an IMPERATIVE OPERATIONAL DIRECTIVE
/// ("always include the dashboard", "never force-push main", "mix assets.build after a
/// Tailwind class") rather than a declarative factual ASSERTION about what the code IS?
/// `contradiction_check` is an NLI over factual claims; it conflates a normative directive
/// with a claim contradicting the indexed base and wrongly rejects it (verdict 0.86–0.99
/// on imperative AXO practices, observed in the REQ-AXO-902146 dogfood migration). A
/// directive is not a falsifiable statement about the code, so a `contradicts` verdict on
/// it is downgraded to advisory (store + warn; the tick/prune/trust loop still governs it),
/// NOT a hard reject. The anti-poison hard reject is preserved for declarative factual
/// contradictions ("Axon uses MongoDB") that are neither failure-framed nor imperative.
/// Two conservative signals: a deontic marker anywhere, or a leading imperative verb on the
/// practice itself. Sibling of [`is_failure_mode`]. Pure + unit-testable.
fn is_imperative_directive(context: &str, practice: &str) -> bool {
    const DEONTIC: &[&str] = &[
        // EN normative / deontic register
        "always", "never", "must", "mustn't", "shall", "should", "do not", "don't",
        "ensure", "make sure", "avoid", "prefer", "require", "only use", "use only",
        // FR
        "toujours", "jamais", "ne pas", "pas de", "il faut", "veiller à", "éviter",
        "préférer", "doit", "ne doit", "exiger", "n'oublie", "assure", "à éviter",
    ];
    let hay = format!("{context} {practice}").to_lowercase();
    if DEONTIC.iter().any(|m| hay.contains(m)) {
        return true;
    }
    // Leading imperative verb on the practice itself (e.g. "mix assets.build…",
    // "use the promote script…", "garde le dashboard…").
    const IMPERATIVE_LEAD: &[&str] = &[
        "use", "run", "add", "keep", "set", "pass", "call", "check", "verify", "rebuild",
        "mix", "include", "delete", "remove", "restart", "build", "ship", "commit",
        "utilise", "lance", "ajoute", "garde", "vérifie", "relance", "passe", "inclus",
        "supprime", "reconstruis", "livre",
    ];
    let first = practice
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    let first = first.trim_matches(|c: char| !c.is_alphanumeric());
    IMPERATIVE_LEAD.iter().any(|v| first == *v)
}

/// REQ-AXO-902137 — cosine-distance threshold under which two practices count as
/// near-duplicates and get fused. 0.07 ≈ cosine similarity > 0.93 (BGE-large 1024d).
/// Single source of truth for the fuse threshold ; applied IN-DB by
/// [`Self::fuse_near_duplicates`] via the pgvector `<=>` self-join (no vector
/// round-trip to Rust, PIL-AXO-9002), so the comparison lives in SQL, not a predicate.
const FUSION_COSINE_EPS: f32 = 0.07;

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

// REQ-AXO-902138 — consolidation thresholds (episode → rule → principle). Driven
// by the SAME trust/use signals as FSRS — no LLM, no new scoring.
const RULE_TRUST: f32 = 0.70;
const RULE_MIN_USES: i64 = 3;
const PRINCIPLE_TRUST: f32 = 0.85;
const PRINCIPLE_MIN_USES: i64 = 8;
const PRINCIPLE_MIN_PROVENANCE: usize = 2;

/// REQ-AXO-902138 — maturity-based tier promotion. A practice EARNS its tier from
/// accrued governance: an episode that proves itself (trust + repeated use) becomes
/// a `rule` ; a rule sustained at high trust across ≥2 tenants becomes a
/// cross-cutting `principle`. Returns `Some(new_tier)` on promotion, else `None`
/// (at ceiling or not yet mature). Pure + unit-testable.
fn consolidate_tier(
    tier: &str,
    trust: f32,
    use_count: i64,
    provenance_count: usize,
) -> Option<&'static str> {
    match tier {
        "episode" if trust >= RULE_TRUST && use_count >= RULE_MIN_USES => Some("rule"),
        "rule"
            if trust >= PRINCIPLE_TRUST
                && use_count >= PRINCIPLE_MIN_USES
                && provenance_count >= PRINCIPLE_MIN_PROVENANCE =>
        {
            Some("principle")
        }
        _ => None,
    }
}

/// REQ-AXO-902138 — count distinct provenance tenants in a `source_project` cell
/// (comma-separated, blanks dropped). Reuses [`merge_provenance`]'s dedup.
fn provenance_count(source_project: &str) -> usize {
    merge_provenance("", source_project)
        .split(',')
        .filter(|t| !t.trim().is_empty())
        .count()
}

/// REQ-AXO-902141 — normalise a caller-supplied perishability class. `durable` is
/// the default (a best-practice store is durable by nature) ; only `perishable` is
/// accepted as the alternative (future news/context memory). Anything else →
/// `durable` (fail-safe: never silently apply time-decay to a best practice).
fn normalize_perishability(raw: &str) -> &'static str {
    if raw.trim().eq_ignore_ascii_case("perishable") {
        "perishable"
    } else {
        "durable"
    }
}

/// REQ-AXO-902141 — does this perishability class decay BY TIME ? Only perishable
/// knowledge (news, market state) goes stale with age (FSRS). Durable knowledge (a
/// code/op best practice: "never force-push main") is timeless — it decays ONLY by
/// supersession (upsert, same context) or contradiction (contradiction_check), never
/// by the clock. Pure + unit-testable. THE core of the model fix: the symptom
/// (trust 0.00 in 1 day on a durable practice) was a wrong CRITERION, not a tuning.
fn perishability_decays_by_time(perishability: &str) -> bool {
    normalize_perishability(perishability) == "perishable"
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

/// REQ-AXO-902149 — normalise a partitioning-axis tag (role / model). Blank → `'*'`
/// (the shared/agnostic sentinel, reused from the scope-global convention) ; otherwise
/// trimmed + lowercased to a stable token. Default = shared/agnostic (N1 stigmergy);
/// a concrete tag is the private/model-specific opt-in (H1). Pure + unit-testable.
fn normalize_axis_tag(raw: &str) -> String {
    let t = raw.trim().to_lowercase();
    if t.is_empty() {
        "*".to_string()
    } else {
        t
    }
}

/// REQ-AXO-902149 — covering scope set for hierarchical inheritance. A recall at
/// `NEX/coder` inherits from its ancestors: `["NEX/coder","NEX","*"]` (most specific
/// → global). `/` is the hierarchy delimiter (no existing project code contains it).
/// A practice stored at an ancestor level is visible to all its descendants, never
/// the inverse. Pure + unit-testable.
fn covering_scopes(scope: &str) -> Vec<String> {
    let parts: Vec<&str> = scope.split('/').filter(|p| !p.is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    for i in (1..=parts.len()).rev() {
        out.push(parts[..i].join("/"));
    }
    if !out.iter().any(|s| s == "*") {
        out.push("*".to_string());
    }
    out
}

/// REQ-AXO-902149 — recall set for a partitioning axis (role/model): always the
/// shared/agnostic sentinel `'*'`, plus the caller's own value when set (≠ `'*'`).
/// So an omitted axis recalls only the shared/agnostic practices (retro-compatible:
/// every legacy row is `'*'`); a concrete value ALSO surfaces that partition's
/// private practices. Pure + unit-testable.
fn axis_recall_set(caller: &str) -> Vec<String> {
    let mut v = vec!["*".to_string()];
    let c = caller.trim().to_lowercase();
    if !c.is_empty() && c != "*" {
        v.push(c);
    }
    v
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
        // REQ-AXO-902141 — perishability class (durable by default: a best practice
        // is timeless, it must NOT be time-decayed).
        let perishability =
            normalize_perishability(args.get("perishability").and_then(Value::as_str).unwrap_or(""));
        // REQ-AXO-902149 — multi-agent partitioning axes (default '*' = shared/agnostic).
        let role = normalize_axis_tag(args.get("role").and_then(Value::as_str).unwrap_or(""));
        let model = normalize_axis_tag(args.get("model").and_then(Value::as_str).unwrap_or(""));

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
        // REQ-AXO-902154 — likewise, an IMPERATIVE OPERATIONAL DIRECTIVE ("toujours inclure
        // le dashboard", "never force-push main") is normative, not a falsifiable claim about
        // the code; the NLI wrongly flags it as contradicting the indexed base. Downgrade it
        // to advisory too. The hard reject survives only for a declarative factual
        // contradiction that is neither failure-framed nor imperative.
        let failure_framed = is_failure_mode(context, practice);
        let imperative_framed = is_imperative_directive(context, practice);
        let gate_label: &str = if verdict == "contradicts" {
            if failure_framed {
                "advisory_failure_mode"
            } else if imperative_framed {
                "advisory_imperative_directive"
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
            "INSERT INTO axon.practice (scope, context, practice, dense, evidence, embedding, source_project, perishability, role, model) \
             VALUES ('{}', '{}', '{}', '{}', '{}', {}, '{}', '{}', '{}', '{}') \
             ON CONFLICT (scope, role, model, md5(practice)) DO UPDATE \
               SET dense = EXCLUDED.dense, \
                   evidence = EXCLUDED.evidence, \
                   embedding = COALESCE(EXCLUDED.embedding, axon.practice.embedding), \
                   perishability = EXCLUDED.perishability, \
                   updated_at = now() \
             RETURNING id, (xmax = 0) AS inserted",
            esc(&scope),
            esc(context),
            esc(practice),
            esc(&dense),
            esc(evidence),
            embed_sql,
            esc(source),
            perishability,
            esc(&role),
            esc(&model)
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
                "### 🧠 practice_put — {} · scope=`{scope}` · gate={gate_label} · embed={embed_state} · encoding={dense_state} · {perishability} · id={id}{}",
                if inserted {"stored"} else {"updated"},
                dense_advisory.map(|a| format!(" · ⚠️ {a}")).unwrap_or_default()
            )}],
            "data": {"status":"ok","id":id,"inserted":inserted,"scope":scope,"gate":gate_label,"embed":embed_state,
                     "encoding":dense_state,"dense_advisory":dense_advisory,"perishability":perishability}
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

        // REQ-AXO-902149 — partition: hierarchical scope inheritance × role × model.
        // scope covers its ancestors (NEX/coder → NEX → '*'); role/model each recall
        // the shared/agnostic '*' plus the caller's own value. Omitted axis → '*' only
        // (retro-compatible: every legacy row is role='*' model='*'). Re-rank unchanged.
        let role = args.get("role").and_then(Value::as_str).unwrap_or("");
        let model = args.get("model").and_then(Value::as_str).unwrap_or("");
        let in_list =
            |vals: &[String]| vals.iter().map(|v| format!("'{}'", esc(v))).collect::<Vec<_>>().join(", ");
        let scope_filter = format!(
            "scope IN ({}) AND role IN ({}) AND model IN ({})",
            in_list(&covering_scopes(&scope)),
            in_list(&axis_recall_set(role)),
            in_list(&axis_recall_set(model))
        );
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
                    (embedding <=> {vec_lit}::vector) AS dist, tier, perishability, role, model \
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
                    "tier": g(8),
                    "perishability": g(9),
                    "role": g(10),
                    "model": g(11),
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
                 FROM axon.practice a JOIN axon.practice b \
                   ON b.scope = a.scope AND b.role = a.role AND b.model = a.model \
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
                    EXTRACT(EPOCH FROM (now() - created_at))/86400.0 AS age_days, \
                    tier, source_project, perishability \
             FROM axon.practice WHERE status='active' {scope_filter}"
        );
        let rows: Vec<Vec<Value>> = match self.graph_store.query_json_writer(&sql) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(practice_err(&format!("tick load failed: {e}"), "degraded")),
        };
        let f = |v: &Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0) as f32;
        let (mut decayed, mut pruned, mut consolidated, mut preserved) = (0u32, 0u32, 0u32, 0u32);
        for r in &rows {
            let id = r.first().and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
            let trust = f(r.get(1).unwrap_or(&Value::Null));
            let stability = f(r.get(2).unwrap_or(&Value::Null));
            let use_count = r.get(3).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0) as i32;
            let days_since = f(r.get(4).unwrap_or(&Value::Null));
            let age_days = f(r.get(5).unwrap_or(&Value::Null));
            let tier = r.get(6).and_then(|v| v.as_str()).unwrap_or("episode").to_string();
            let prov_n = provenance_count(r.get(7).and_then(|v| v.as_str()).unwrap_or(""));
            let perishability = r.get(8).and_then(|v| v.as_str()).unwrap_or("durable");
            // REQ-AXO-902138 — promotion uses the PRE-decay trust (the accrued
            // governance the record earned). Runs for durable + perishable alike.
            let promoted = consolidate_tier(&tier, trust, use_count as i64, prov_n);
            if promoted.is_some() {
                consolidated += 1;
            }
            // REQ-AXO-902141 — DURABLE knowledge (best practices) is timeless: the
            // tick must NOT decay or age-prune it (the bug: trust 0.00 in 1 day).
            // It only loses trust via supersession (upsert, same context) or
            // contradiction (contradiction_check at put) — both OUTSIDE this loop.
            // Time-decay + age-prune apply ONLY to perishable knowledge.
            if !perishability_decays_by_time(perishability) {
                preserved += 1;
                if let Some(t) = promoted {
                    let _ = self.graph_store.execute(&format!(
                        "UPDATE axon.practice SET tier = '{t}', updated_at = now() WHERE id = {id}"
                    ));
                }
                continue;
            }
            let effective_tier = promoted.unwrap_or(tier.as_str());
            let r_ret = retrievability(days_since, stability);
            let new_trust = decay_trust(trust, r_ret);
            // principles survive the decay-prune (transverse) ; episodes/rules prune.
            let prune = should_prune(new_trust, r_ret, use_count, age_days) && effective_tier != "principle";
            let status = if prune { "pruned" } else { "active" };
            let tier_set = promoted.map(|t| format!(", tier = '{t}'")).unwrap_or_default();
            let _ = self.graph_store.execute(&format!(
                "UPDATE axon.practice SET trust = {new_trust}, status = '{status}'{tier_set}, updated_at = now() WHERE id = {id}"
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
                "### 🧠 practice_tick — fused {fused} · consolidated {consolidated} · preserved {preserved} (durable) · decayed {decayed} · pruned {pruned} · mean_trust {:.2} · stagnation={}",
                mean_trust, stag.stagnating
            )}],
            "data": {"status":"ok","fused":fused,"consolidated":consolidated,"preserved":preserved,"decayed":decayed,"pruned":pruned,"mean_trust":mean_trust,
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
    use super::{
        axis_recall_set, consolidate_tier, covering_scopes, is_failure_mode,
        is_imperative_directive, merge_provenance, normalize_axis_tag, normalize_perishability,
        perishability_decays_by_time, provenance_count, resolve_dense_form,
    };

    #[test]
    fn perishability_902141() {
        // durable is the default + fail-safe ; only explicit "perishable" opts into time-decay.
        assert_eq!(normalize_perishability(""), "durable");
        assert_eq!(normalize_perishability("garbage"), "durable");
        assert_eq!(normalize_perishability("DURABLE"), "durable");
        assert_eq!(normalize_perishability(" Perishable "), "perishable");
        // the core model fix: durable knowledge NEVER decays by time ; perishable does.
        assert!(!perishability_decays_by_time("durable"));
        assert!(!perishability_decays_by_time("")); // default durable → no time decay
        assert!(perishability_decays_by_time("perishable"));
    }

    #[test]
    fn partitioning_axes_902149() {
        // normalize_axis_tag: blank → '*' (shared/agnostic sentinel) ; else trimmed+lowercased.
        assert_eq!(normalize_axis_tag(""), "*");
        assert_eq!(normalize_axis_tag("   "), "*");
        assert_eq!(normalize_axis_tag(" Coder "), "coder");
        assert_eq!(normalize_axis_tag("*"), "*");
        // covering_scopes: hierarchical inheritance, most-specific → global, '*' always last.
        assert_eq!(covering_scopes("NEX/coder"), vec!["NEX/coder", "NEX", "*"]);
        assert_eq!(covering_scopes("AXO"), vec!["AXO", "*"]);
        assert_eq!(covering_scopes("*"), vec!["*"]); // global stays a singleton, no dup
        assert_eq!(covering_scopes("A/b/c"), vec!["A/b/c", "A/b", "A", "*"]);
        // axis_recall_set: always the shared '*', plus the caller's own value (≠ '*').
        assert_eq!(axis_recall_set(""), vec!["*"]); // omitted → shared only (retro-compatible)
        assert_eq!(axis_recall_set("*"), vec!["*"]);
        assert_eq!(axis_recall_set("Coder"), vec!["*", "coder"]);
    }

    #[test]
    fn consolidate_tier_902138() {
        // episode → rule: needs trust ≥ 0.70 AND ≥ 3 uses.
        assert_eq!(consolidate_tier("episode", 0.72, 3, 1), Some("rule"));
        assert_eq!(consolidate_tier("episode", 0.69, 9, 3), None); // trust too low
        assert_eq!(consolidate_tier("episode", 0.9, 2, 3), None); // not enough uses
        // rule → principle: trust ≥ 0.85 AND ≥ 8 uses AND ≥ 2 tenants.
        assert_eq!(consolidate_tier("rule", 0.86, 8, 2), Some("principle"));
        assert_eq!(consolidate_tier("rule", 0.86, 8, 1), None); // single-tenant, not transverse
        assert_eq!(consolidate_tier("rule", 0.84, 20, 5), None); // trust too low
        // principle is the ceiling; episode never skips straight to principle.
        assert_eq!(consolidate_tier("principle", 0.99, 100, 9), None);
        assert_eq!(consolidate_tier("episode", 0.99, 100, 9), Some("rule"));
    }

    #[test]
    fn provenance_count_902138() {
        assert_eq!(provenance_count(""), 0);
        assert_eq!(provenance_count("NEX"), 1);
        assert_eq!(provenance_count("NEX,AXO"), 2);
        assert_eq!(provenance_count(" NEX , AXO ,NEX"), 2); // dedup + trim
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

    #[test]
    fn imperative_directive_detection_902154() {
        // deontic register → directive (downgraded to advisory, not rejected)
        assert!(is_imperative_directive("dashboard", "toujours inclure le dashboard"));
        assert!(is_imperative_directive("git", "never force-push main"));
        assert!(is_imperative_directive("DDL", "pas de CHECK vocab au bootstrap DDL"));
        assert!(is_imperative_directive("delivery", "you must run the promote script"));
        // leading imperative verb (no explicit deontic marker)
        assert!(is_imperative_directive("Tailwind", "mix assets.build after a new class"));
        assert!(is_imperative_directive("runtime", "garde le brain et l'indexer séparés"));
        // declarative factual assertions → NOT a directive (anti-poison reject preserved)
        assert!(!is_imperative_directive("database choice", "Axon uses MongoDB for everything"));
        assert!(!is_imperative_directive("storage", "the canonical store is PostgreSQL"));
    }
}
