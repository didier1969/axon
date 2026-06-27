//! REQ-AXO-902096 — `contradiction_check` MCP tool (demande Nexus, DEC-AXO-901660).
//!
//! Two-stage anti-hallucination gate: (1) pgvector ANN shortlist of the scope's
//! chunks topically close to the candidate, (2) NLI cross-encoder re-rank — each
//! shortlisted passage is judged against the candidate and those whose
//! `contradiction` probability ≥ threshold are returned. A cosine proxy is
//! explicitly rejected (similarity ≠ entailment direction); when the NLI model
//! is not provisioned the tool returns an explicit `nli_unavailable`, never a
//! silent "no contradiction" (that would be the very hallucination it guards).
//!
//! REQ-AXO-902107 (post-incident hardening, Nexus verification s91): the re-rank
//! loop is bounded by a wall-clock budget (`AXON_NLI_BUDGET_MS`, default 20s) so a
//! slow provider (CPU NLI ≈ 5s/pair) or service pressure yields a partial-but-honest
//! verdict instead of blowing the ~30s MCP gateway timeout. An empty shortlist or a
//! budget-truncated run reports `verdict=inconclusive` (never a silent clean pass),
//! and `data.scope` exposes `passages_shortlisted`/`passages_judged`/`truncated` so
//! a 0-judged result is unambiguous (anti-théâtre, CPT-AXO-90054).

use std::cmp::Ordering;
use std::time::Instant;

use serde_json::{json, Value};

/// Wall-clock budget (ms) for the NLI re-rank loop. Bounds total inference time so
/// the tool returns a partial-but-honest verdict instead of blowing the MCP gateway
/// timeout (~30s) under a slow provider (CPU NLI ≈ 5s/pair) or service pressure.
/// Provider-agnostic safety net (GUI-PRO-107: bound the class, not the instance).
/// Override via `AXON_NLI_BUDGET_MS`.
const DEFAULT_NLI_BUDGET_MS: u128 = 20_000;

use super::McpServer;

fn err_json(msg: String, status: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": msg }],
        "isError": true,
        "data": { "status": status }
    })
}

impl McpServer {
    pub(crate) fn axon_contradiction_check(&self, args: &Value) -> Option<Value> {
        let candidate = match args.get("candidate").and_then(Value::as_str) {
            Some(c) if !c.trim().is_empty() => c.trim(),
            _ => {
                return Some(err_json(
                    "contradiction_check requires `candidate` (the fact/passage to check for contradiction against the scope).".to_string(),
                    "input_invalid",
                ))
            }
        };
        let scope = args.get("scope").cloned().unwrap_or_else(|| json!({}));
        let explicit_project = scope.get("project").and_then(Value::as_str);
        let auto = if explicit_project.is_none() {
            self.auto_resolve_project_code_str()
        } else {
            None
        };
        let project = explicit_project.or(auto.as_deref()).unwrap_or("AXO");
        let threshold = args
            .get("threshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.5) as f32;
        let top_k = args
            .get("top_k")
            .and_then(Value::as_u64)
            .unwrap_or(8)
            .clamp(1, 50) as usize;

        // 1. Embed the candidate (reuses the canonical BGE embedder).
        let emb = match crate::embedder::batch_embed(vec![candidate.to_string()]) {
            Ok(v) => v.into_iter().next(),
            Err(e) => return Some(err_json(format!("candidate embed failed: {e}"), "degraded")),
        };
        let Some(emb) = emb else {
            return Some(err_json(
                "candidate produced no embedding".to_string(),
                "degraded",
            ));
        };
        // REQ-AXO-902110 instrumentation (Nexus #29): surface the candidate vector
        // shape so a future "0 passage" is self-diagnosing (degenerate embed vs
        // empty scope vs over-filtering).
        let embed_dim = emb.len();
        let embed_norm = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        let vec_lit = match crate::postgres::vector::vector_literal(&emb) {
            Ok(s) => s,
            Err(e) => return Some(err_json(format!("vector literal: {e}"), "degraded")),
        };

        // 2. ANN shortlist over the scope's symbol chunks (pool a bit wider than
        //    top_k so the NLI re-rank has candidates to filter).
        let proj = project.replace('\'', "''");
        let pool = (top_k * 3).clamp(top_k, 60);
        let ann_sql = format!(
            "SELECT c.id, c.content, c.file_path, c.source_id \
             FROM ist.ChunkEmbedding ce \
             JOIN ist.Chunk c ON c.id = ce.chunk_id \
                 AND c.project_code = '{proj}' AND c.source_type = 'symbol' \
             ORDER BY ce.embedding <=> {vec} LIMIT {pool}",
            proj = proj,
            vec = vec_lit,
            pool = pool
        );
        // REQ-AXO-902110 — route through the SAME shared ANN path the (working)
        // code_band of retrieve_context_layered uses (`query_ann_json`). That path
        // now sets `hnsw.iterative_scan = relaxed_order` (pgvector 0.8+), which is
        // the real fix: it keeps scanning until enough IN-SCOPE rows survive the
        // JOIN filter, regardless of how small the scope is vs the whole corpus.
        // Before, the raw `query_json` used the default ef_search (40) HNSW scan →
        // global neighbours → in-scope filter decimated them to ~0 ("shortlist
        // empty" on a healthy 17k-chunk corpus). ef_search here is just first-pass
        // breadth; iterative_scan handles the tail.
        let ef_search = (pool as u32).max(40).min(1000);
        let rows: Vec<Vec<Value>> = match self.graph_store.query_ann_json(&ann_sql, ef_search) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(err_json(format!("ANN shortlist failed: {e}"), "degraded")),
        };
        // Instrumentation: in-scope embedded-symbol count, so "shortlist empty"
        // distinguishes a truly empty scope from an over-filtered ANN (Nexus #29).
        let scope_chunk_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM ist.ChunkEmbedding ce \
                 JOIN ist.Chunk c ON c.id = ce.chunk_id \
                     AND c.project_code = '{proj}' AND c.source_type = 'symbol'",
                proj = proj
            ))
            .unwrap_or(-1);

        // 3. NLI re-rank: judge each passage (premise) vs the candidate (hypothesis).
        //    Bounded by a wall-clock budget so a slow provider (CPU ≈ 5s/pair) or
        //    service pressure degrades to a partial verdict, never a gateway timeout.
        let budget_ms = std::env::var("AXON_NLI_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse::<u128>().ok())
            .unwrap_or(DEFAULT_NLI_BUDGET_MS);
        let started = Instant::now();
        let mut conflicts: Vec<Value> = Vec::new();
        let mut judged = 0usize;
        let mut truncated = false;
        for row in &rows {
            if started.elapsed().as_millis() > budget_ms {
                // Budget exhausted before judging the whole shortlist — stop and
                // flag it so the verdict is honest about partial coverage.
                truncated = true;
                break;
            }
            let content = row.get(1).and_then(Value::as_str).unwrap_or("");
            if content.is_empty() {
                continue;
            }
            let id = row.first().and_then(Value::as_str).unwrap_or("");
            let file_path = row.get(2).and_then(Value::as_str).unwrap_or("");
            let symbol = row.get(3).and_then(Value::as_str).unwrap_or(id);
            // Strip the chunk header (`symbol:/kind:/part:` + blank line) so the
            // NLI model sees the actual code/prose, not the metadata preamble.
            let passage = content.splitn(2, "\n\n").nth(1).unwrap_or(content);
            match crate::nli::judge_global(passage, candidate) {
                Ok(scores) => {
                    judged += 1;
                    if scores.contradiction >= threshold {
                        conflicts.push(json!({
                            "id": symbol,
                            "file_path": file_path,
                            "contradiction": scores.contradiction,
                            "entailment": scores.entailment,
                            "verdict": scores.verdict().as_str(),
                        }));
                    }
                }
                Err(e) => {
                    // Model not provisioned → explicit unavailable, never a silent
                    // pass (the anti-théâtre principle of CPT-AXO-90054).
                    return Some(json!({
                        "content": [{ "type": "text", "text": format!(
                            "contradiction_check: NLI model unavailable ({e}). Provision it via `scripts/provision_nli_model.sh` (exports tasksource/ModernBERT-base-nli)."
                        )}],
                        "isError": true,
                        "data": {
                            "status": "nli_unavailable",
                            "recovery": "run scripts/provision_nli_model.sh"
                        }
                    }));
                }
            }
        }

        conflicts.sort_by(|a, b| {
            b.get("contradiction")
                .and_then(Value::as_f64)
                .partial_cmp(&a.get("contradiction").and_then(Value::as_f64))
                .unwrap_or(Ordering::Equal)
        });
        conflicts.truncate(top_k);
        // Honest verdict (CPT-AXO-90054 anti-théâtre): a clean `neutral` is only
        // legitimate when the WHOLE shortlist was actually judged. An empty shortlist
        // or a budget-truncated run is `inconclusive`, never a silent all-clear.
        let verdict = if !conflicts.is_empty() {
            "contradicts"
        } else if rows.is_empty() || truncated {
            "inconclusive"
        } else {
            "neutral"
        };

        let report = if rows.is_empty() {
            format!(
                "### 🧪 contradiction_check\n\nverdict=**inconclusive** — 0 passage retrieved from scope `{}`. Diagnostic: {} embedded symbol-chunk(s) exist in scope, candidate embed dim={} norm={:.3}, ef_search={}. (count>0 + valid embed ⇒ ANN/over-filtering, not an empty scope or a failed embed.) NOT a clean bill of health — nothing was checked.",
                project, scope_chunk_count, embed_dim, embed_norm, ef_search
            )
        } else {
            let trunc_note = if truncated {
                format!(
                    " ⚠️ budget-bounded: only {}/{} shortlisted passages judged within {}ms (slow NLI provider or service pressure). verdict=inconclusive — raise `AXON_NLI_BUDGET_MS`, promote the GPU NLI build, or narrow `top_k` for full coverage.",
                    judged,
                    rows.len(),
                    budget_ms
                )
            } else {
                String::new()
            };
            format!(
                "### 🧪 contradiction_check\n\nverdict=**{}** — {}/{} shortlisted passage(s) judged in scope `{}`, {} conflict(s) ≥ {:.2}.{}",
                verdict,
                judged,
                rows.len(),
                project,
                conflicts.len(),
                threshold,
                trunc_note
            )
        };
        Some(json!({
            "content": [{ "type": "text", "text": report }],
            "data": {
                "status": "ok",
                "verdict": verdict,
                "candidate_preview": candidate.chars().take(160).collect::<String>(),
                "scope": {
                    "project": project,
                    "project_resolved": project,
                    "passages_shortlisted": rows.len(),
                    "passages_judged": judged,
                    "shortlist_pool": rows.len(),
                    "judged": judged,
                    "scope_chunk_count": scope_chunk_count,
                    "candidate_embed_dim": embed_dim,
                    "candidate_embed_norm": embed_norm,
                    "ef_search": ef_search,
                    "truncated": truncated,
                    "budget_ms": budget_ms,
                    "threshold": threshold
                },
                "top_conflicts": conflicts
            }
        }))
    }
}
