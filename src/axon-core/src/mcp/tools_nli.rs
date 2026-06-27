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
        // In-scope embedded-symbol count — decides the retrieval strategy (below) and
        // distinguishes a truly empty scope from a non-finding in the report.
        let scope_chunk_count = self
            .graph_store
            .query_count(&format!(
                "SELECT count(*) FROM ist.ChunkEmbedding ce \
                 JOIN ist.Chunk c ON c.id = ce.chunk_id \
                     AND c.project_code = '{proj}' AND c.source_type = 'symbol'",
                proj = proj
            ))
            .unwrap_or(-1);
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
        // REQ-AXO-902129 — for a BOUNDED scope, do an EXACT scan (brute-force cosine
        // over the in-scope vectors, ~tens of ms for ≤50k), bypassing the HNSW index.
        // This is correct-by-construction and IMMUNE to HNSW graph corruption — the
        // root cause of the 0-passage / wrong-pocket bug (REQ-902126): a corrupt
        // index returns a tiny arbitrary single-project pocket, so a candidate could
        // land in a non-AXO pocket and retrieve 0 in-scope rows even though its true
        // neighbourhood is AXO-rich. Exact scan over 17k vectors sidesteps that
        // entirely. Only fall back to HNSW for a scope too large to scan exactly.
        const EXACT_SCAN_MAX: i64 = 50_000;
        let ef_search = (pool as u32).max(40).min(1000);
        let ann_result = if scope_chunk_count > 0 && scope_chunk_count <= EXACT_SCAN_MAX {
            self.graph_store.query_exact_scan_json(&ann_sql)
        } else {
            self.graph_store.query_ann_json(&ann_sql, ef_search)
        };
        let exact_scan = scope_chunk_count > 0 && scope_chunk_count <= EXACT_SCAN_MAX;
        let rows: Vec<Vec<Value>> = match ann_result {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(err_json(format!("ANN shortlist failed: {e}"), "degraded")),
        };

        // 3. NLI re-rank: judge each passage (premise) vs the candidate (hypothesis).
        //    Bounded by a wall-clock budget so a slow provider (CPU ≈ 5s/pair) or
        //    service pressure degrades to a partial verdict, never a gateway timeout.
        let budget_ms = std::env::var("AXON_NLI_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse::<u128>().ok())
            .unwrap_or(DEFAULT_NLI_BUDGET_MS);
        // REQ-AXO-902125 — support-aware aggregation. The NLI is reliable PER passage
        // (golden test: prose claim 0.978 entail / 0.995 contra), but flagging
        // `contradicts` on ANY single passage crossing `threshold` gives systematic
        // false positives: a multi-language, mixed code/prose corpus always has a few
        // tangential/OOD passages that score contradiction even for a TRUE claim.
        // The real discriminator (measured live, REQ-AXO-902125): the NET MARGIN
        // between the corpus's strongest contradiction and its strongest support.
        // A TRUE claim has contradiction and support close (corpus both half-supports
        // and half-noise-contradicts → ambiguous): 'uses PostgreSQL' → contra 0.788 /
        // entail 0.378, margin 0.41. A FALSE claim has contradiction dominating with
        // no support: 'uses MongoDB' → contra 0.896 / entail 0.038, margin 0.86. So we
        // only call `contradicts` when contradiction clearly OUTWEIGHS support.
        let net_margin = std::env::var("AXON_NLI_NET_MARGIN")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.6);
        let started = Instant::now();
        let mut conflicts: Vec<Value> = Vec::new();
        let mut judged = 0usize;
        let mut truncated = false;
        let mut max_contradiction = 0f32;
        let mut max_entailment = 0f32;
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
                    max_contradiction = max_contradiction.max(scores.contradiction);
                    max_entailment = max_entailment.max(scores.entailment);
                    // A passage is a genuine conflict only if its ARGMAX verdict is
                    // Contradiction (more robust than a bare prob threshold) AND the
                    // probability clears `threshold`.
                    if scores.verdict() == crate::nli::NliVerdict::Contradiction
                        && scores.contradiction >= threshold
                    {
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
        // REQ-AXO-902125 — net-margin verdict (kills the Nexus #32 false positives).
        //   inconclusive: nothing judged (empty shortlist or budget-truncated) — never
        //                 a silent all-clear (CPT-AXO-90054 anti-théâtre).
        //   contradicts:  there is a real contradiction (max_contradiction ≥ threshold)
        //                 AND it OUTWEIGHS support by ≥ net_margin. A true claim's few
        //                 noisy contradiction passages can't win when the corpus also
        //                 supports it (small margin) → not flagged.
        //   neutral:      no net contradiction.
        let margin = max_contradiction - max_entailment;
        let contradicted =
            !conflicts.is_empty() && max_contradiction >= threshold && margin >= net_margin;
        let verdict = if rows.is_empty() || truncated {
            "inconclusive"
        } else if contradicted {
            "contradicts"
        } else {
            "neutral"
        };
        // Only present conflict passages when the verdict is actually `contradicts`;
        // otherwise they are noise below the net-margin, not a finding.
        if !contradicted {
            conflicts.clear();
        }

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
            let margin_note = if verdict == "neutral" && max_contradiction >= threshold {
                format!(
                    " Contradiction does not outweigh support (margin {:.3} < {:.2}) — flagged passages are noise, not a finding.",
                    margin, net_margin
                )
            } else {
                String::new()
            };
            format!(
                "### 🧪 contradiction_check\n\nverdict=**{}** — {}/{} judged in scope `{}` · max_contradiction={:.3} max_entailment={:.3} margin={:.3} (net_margin={:.2}) · {} conflict(s).{}{}",
                verdict,
                judged,
                rows.len(),
                project,
                max_contradiction,
                max_entailment,
                margin,
                net_margin,
                conflicts.len(),
                margin_note,
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
                    "exact_scan": exact_scan,
                    "truncated": truncated,
                    "budget_ms": budget_ms,
                    "threshold": threshold,
                    "net_margin": net_margin,
                    "max_contradiction": max_contradiction,
                    "max_entailment": max_entailment,
                    "margin": margin
                },
                "top_conflicts": conflicts
            }
        }))
    }
}
