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

/// REQ-AXO-902502 — fraction de `net_margin` au-dessus de laquelle un verdict `neutral`
/// devient `neutral_borderline` (« je ne tranche pas ») au lieu de « pas un finding ».
///
/// 0,90 = les 10 % sous le seuil. Choisi, pas hérité : le cas d'OPV était à 0,5 % du
/// seuil, donc n'importe quelle bande raisonnable l'attrape ; 10 % laisse de la marge
/// sans transformer tout `neutral` en alerte. À REMESURER si le taux de
/// `neutral_borderline` dépasse ~15 % des appels — ce serait le signe que `net_margin`
/// lui-même est mal calibré, pas que la bande est trop large.
const BORDERLINE_RATIO: f32 = 0.90;

/// REQ-AXO-902602 — les identifiants SOLL cités EXPLICITEMENT dans le candidat.
///
/// Voix client KKI (feedback #399, satisfaction 4/10) : une affirmation citant
/// `REQ-KKI-126` n'a inclus AUCUN passage de ce nœud parmi 24 candidats, et a jugé
/// des artefacts sans rapport. La cause n'est pas un mauvais classement ANN — c'est
/// que le corpus interrogé ne contient PAS la SOLL : `ist.chunk` ne porte que
/// `source_type` `symbol` (532 538 lignes) et `file` (8 894), mesuré le 2026-09-05.
/// Le nœud cité était structurellement inatteignable. Aucun réglage de seuil ne
/// pouvait le faire apparaître.
///
/// La forme reconnue est la forme canonique : trois lettres, tiret, trois lettres,
/// tiret, chiffres. Écrite à la main plutôt que par regex — la dépendance n'existe
/// pas dans ce crate et le motif est trop simple pour la justifier.
pub(crate) fn soll_ids_cites(candidat: &str) -> Vec<String> {
    let octets = candidat.as_bytes();
    let mut trouves: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < octets.len() {
        // Un identifiant commence sur une frontière de mot : sinon `XREQ-AXO-1` et
        // le fragment `-AXO-1` d'un identifiant plus long deviendraient des ancres.
        let debut_de_mot = i == 0 || !(octets[i - 1].is_ascii_alphanumeric() || octets[i - 1] == b'-');
        if !debut_de_mot {
            i += 1;
            continue;
        }
        let reste = &octets[i..];
        if reste.len() < 9 {
            break;
        }
        let forme = reste[0].is_ascii_uppercase()
            && reste[1].is_ascii_uppercase()
            && reste[2].is_ascii_uppercase()
            && reste[3] == b'-'
            && reste[4].is_ascii_uppercase()
            && reste[5].is_ascii_uppercase()
            && reste[6].is_ascii_uppercase()
            && reste[7] == b'-'
            && reste[8].is_ascii_digit();
        if !forme {
            i += 1;
            continue;
        }
        let mut fin = i + 9;
        while fin < octets.len() && octets[fin].is_ascii_digit() {
            fin += 1;
        }
        let id = candidat[i..fin].to_string();
        if !trouves.contains(&id) {
            trouves.push(id);
        }
        i = fin;
    }
    trouves
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
        // REQ-AXO-902467 — ne plus deviner le projet courant.
        let Some(project) = explicit_project.or(auto.as_deref()) else {
            return Some(crate::mcp::guidance::unresolved_project_error(
                "nli",
                &self.known_project_codes_hint(),
            ));
        };
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
        let mut rows: Vec<Vec<Value>> = match ann_result {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(e) => return Some(err_json(format!("ANN shortlist failed: {e}"), "degraded")),
        };

        // REQ-AXO-902602 — les ancres citées passent DEVANT, et ne dépendent pas de
        // l'ANN. Un identifiant SOLL écrit dans le candidat est une réservation
        // déterministe : le nœud est chargé depuis `soll.node`, jamais recherché par
        // similarité. C'est la seule façon de le voir — `ist.chunk` ne contient pas
        // la SOLL, donc aucune shortlist ne pouvait le ramener.
        //
        // En TÊTE parce que le budget de jugement est borné : si le temps manque, ce
        // qui doit être jugé en premier est ce que l'appelant a explicitement cité,
        // pas le 24ᵉ voisin d'un plongement.
        let ancres_citees = soll_ids_cites(candidate);
        let mut ancres_absentes: Vec<String> = Vec::new();
        let mut ancres_chargees: Vec<String> = Vec::new();
        if !ancres_citees.is_empty() {
            // Borné à 8 : au-delà, le candidat n'est plus une affirmation ancrée mais
            // un catalogue, et jugerait le budget entier sur des citations.
            const MAX_ANCRES: usize = 8;
            let mut lignes_ancres: Vec<Vec<Value>> = Vec::new();
            for id in ancres_citees.iter().take(MAX_ANCRES) {
                let id_echappe = id.replace('\'', "''");
                let sql = format!(
                    "SELECT id, title, description FROM {} WHERE id = '{}'",
                    self.graph_store.soll_table("Node"),
                    id_echappe
                );
                let charge = self
                    .graph_store
                    .query_json(&sql)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Vec<Vec<String>>>(&raw).ok())
                    .and_then(|lignes| lignes.into_iter().next());
                match charge {
                    Some(ligne) if ligne.len() >= 3 => {
                        // Même forme que les lignes de l'ANN — la boucle de jugement
                        // retire l'en-tête sur `\n\n`, donc le modèle voit le corps.
                        let corps: String = ligne[2].chars().take(4_000).collect();
                        lignes_ancres.push(vec![
                            json!(id),
                            json!(format!("soll:{id}\n\n{}\n{}", ligne[1], corps)),
                            json!(format!("soll://{id}")),
                            json!(id),
                        ]);
                        ancres_chargees.push(id.clone());
                    }
                    _ => ancres_absentes.push(id.clone()),
                }
            }
            for id in ancres_citees.iter().skip(MAX_ANCRES) {
                ancres_absentes.push(id.clone());
            }
            lignes_ancres.append(&mut rows);
            rows = lignes_ancres;
        }
        let nb_ancres = ancres_chargees.len();

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
        // REQ-AXO-902602 — combien d'ancres ont RÉELLEMENT été jugées. Le budget peut
        // s'épuiser, un nœud peut manquer : le verdict doit le savoir, pas le supposer.
        let mut ancres_jugees = 0usize;
        for (position, row) in rows.iter().enumerate() {
            let est_ancre = position < nb_ancres;
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
                    if est_ancre {
                        ancres_jugees += 1;
                    }
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
                            // REQ-AXO-902602 — d'où vient ce passage. Sans cette
                            // provenance, l'appelant ne peut pas distinguer un
                            // jugement porté sur le nœud qu'il a cité d'un jugement
                            // porté sur le 24ᵉ voisin d'un plongement.
                            "provenance": if est_ancre { "soll_anchor" } else { "ann_shortlist" },
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
        // REQ-AXO-902502 — un verdict rendu à 0,003 près ne peut pas être catégorique.
        //
        // Mesuré chez OPV : `margin = 0,597` contre `net_margin = 0,60`. L'outil a rendu
        // `neutral` ET la phrase « flagged passages are noise, NOT A FINDING ». C'était
        // une vraie contradiction ; elle a survécu trois jours parce que le message
        // fermait la question au lieu de la poser.
        //
        // Un seuil ne cesse pas d'être arbitraire parce qu'on l'a écrit. À 0,5 % en
        // dessous, la seule chose honnête est : « je ne tranche pas ». C'est l'invariant
        // KKI #204 appliqué non plus à un COMPTE mais à un VERDICT — « non calculé » est
        // un état de premier rang, et « non concluant » aussi.
        //
        // ⚠️ Le corollaire est aussi important que le seuil : dans ce cas on ne VIDE PAS
        // `conflicts`. Un verdict qui dit « à relire » sans montrer quoi relire est une
        // alarme sans adresse.
        let borderline = !contradicted
            && !conflicts.is_empty()
            && max_contradiction >= threshold
            && margin >= net_margin * BORDERLINE_RATIO;
        // REQ-AXO-902602 — fail-closed sur l'ancre canonique. Si l'appelant a cité un
        // identifiant SOLL et que ce nœud n'a PAS été jugé — introuvable, ou budget
        // épuisé avant lui — le verdict ne peut pas être « pas de contradiction » : la
        // pièce que l'affirmation invoque n'a pas été lue. C'est exactement ce que KKI
        // a reçu, avec une satisfaction de 4/10 : un verdict rendu sur 24 artefacts
        // sans rapport et sur zéro passage du nœud cité.
        let ancre_manquee = !ancres_citees.is_empty() && ancres_jugees < ancres_citees.len();
        let verdict = if ancre_manquee && !contradicted {
            "inconclusive"
        } else if rows.is_empty() || truncated {
            "inconclusive"
        } else if contradicted {
            "contradicts"
        } else if borderline {
            "neutral_borderline"
        } else {
            "neutral"
        };
        // Only present conflict passages when the verdict is actually `contradicts` —
        // or when it is BORDERLINE, where the passages are precisely what the caller
        // must re-read. Below that they are noise, not a finding.
        if !contradicted && !borderline {
            conflicts.clear();
        }

        // REQ-AXO-902602 — l'ancre citée est annoncée dans le canal TEXTE, pas
        // seulement dans `data` : c'est là que l'appelant lit son verdict.
        let ancre_note = if ancres_citees.is_empty() {
            String::new()
        } else if ancre_manquee {
            format!(
                " ⚠️ {}/{} ancre(s) SOLL citée(s) JUGÉE(S) — non lues : {}. Le verdict ne peut pas être un feu vert : la pièce que l'affirmation invoque n'a pas été examinée.",
                ancres_jugees,
                ancres_citees.len(),
                ancres_absentes.join(", ")
            )
        } else {
            format!(
                " ✅ {} ancre(s) SOLL citée(s) chargée(s) depuis la SOLL et jugée(s) en tête de shortlist : {}.",
                ancres_jugees,
                ancres_chargees.join(", ")
            )
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
            let margin_note = if verdict == "neutral_borderline" {
                // REQ-AXO-902502 — dire l'écart, pas un verdict. Le lecteur décide.
                format!(
                    " ⚠️ NON CONCLUANT — À RELIRE : la contradiction ({:.3}) manque le seuil de {:.3} seulement ({:.1} % sous `net_margin`={:.2}). Ce n'est PAS un feu vert : les {} passage(s) ci-dessus sont conservés exprès pour que vous jugiez. Un écart de cette taille est du bruit de mesure, pas une décision.",
                    margin,
                    net_margin - margin,
                    (net_margin - margin) / net_margin * 100.0,
                    net_margin,
                    conflicts.len()
                )
            } else if verdict == "neutral" && max_contradiction >= threshold {
                format!(
                    " Contradiction does not outweigh support (margin {:.3} < {:.2}, soit {:.1} % sous le seuil) — flagged passages are noise, not a finding.",
                    margin,
                    net_margin,
                    (net_margin - margin) / net_margin * 100.0
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
            ) + &ancre_note
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
                // REQ-AXO-902602 — la provenance de la shortlist, dite plutôt que
                // supposée. `cited` vient du texte du candidat ; `judged` est ce qui a
                // RÉELLEMENT été lu ; `missing` nomme les nœuds introuvables ou
                // écartés par la borne. Un écart entre `cited` et `judged` force le
                // verdict à `inconclusive`.
                "soll_anchors": {
                    "cited": ancres_citees,
                    "loaded": ancres_chargees,
                    "judged": ancres_jugees,
                    "missing": ancres_absentes,
                    "note": "REQ-AXO-902602 — un identifiant SOLL cité dans le candidat réserve un slot DÉTERMINISTE en tête de shortlist : `ist.chunk` ne contient pas la SOLL (source_type ∈ {symbol, file}), donc aucune recherche ANN ne pouvait ramener le nœud cité."
                },
                "top_conflicts": conflicts
            }
        }))
    }
}

#[cfg(test)]
mod contrat_publie_tests {
    /// REQ-AXO-902502 / REQ-AXO-902513 — le contrat PUBLIÉ doit énumérer tous les
    /// verdicts que le code peut rendre.
    ///
    /// `neutral_borderline` a été ajouté au code sans être ajouté à la description
    /// de `contradiction_check` : le schéma annonçait toujours trois verdicts. Un
    /// appelant qui se fie au schéma — c'est-à-dire tout LLM — rencontrait un
    /// quatrième état non documenté et n'avait aucune raison de le traiter autrement
    /// qu'un `neutral` mal orthographié. Le correctif de fond était donc invisible
    /// pour ses destinataires.
    ///
    /// C'est la même classe que `entier_json` (REQ-AXO-902509) une strate plus haut :
    /// **une règle vit à deux endroits — le code et le contrat publié — et un seul
    /// est corrigé.** Ici le contrat affirmait le CONTRAIRE du code, ce qui est pire
    /// qu'un silence.
    ///
    /// La garde lit les DEUX sources : les littéraux que le code affecte au verdict,
    /// et la description que le catalogue publie. Elle n'a rien à savoir de la liste
    /// — c'est ce qui l'empêche de vieillir avec elle.
    #[test]
    fn tout_verdict_rendu_par_le_code_est_annonce_dans_la_description_publiee() {
        let source = include_str!("tools_nli.rs");
        // Le bloc `let verdict = if … };` est la SEULE origine du verdict rendu.
        let debut = source
            .find("let verdict = if")
            .expect("le bloc qui décide du verdict a été renommé — cette garde ne lit plus rien");
        let bloc = &source[debut..];
        let fin = bloc
            .find("\n        };")
            .expect("bloc de verdict non terminé comme attendu");
        let verdicts: Vec<&str> = bloc[..fin]
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| !s.is_empty())
            .collect();
        assert!(
            verdicts.len() >= 3,
            "seulement {} littéral(aux) trouvé(s) dans le bloc de verdict — la garde \
             lit mal le code, elle ne prouve rien : {verdicts:?}",
            verdicts.len()
        );

        let catalogue = crate::mcp::catalog::tools_catalog(true);
        let description = catalogue["tools"]
            .as_array()
            .expect("catalogue sans tableau `tools`")
            .iter()
            .find(|t| t["name"] == "contradiction_check")
            .expect("`contradiction_check` absent du catalogue publié")["description"]
            .as_str()
            .expect("description non textuelle")
            .to_string();

        let muets: Vec<&&str> = verdicts
            .iter()
            .filter(|v| !description.contains(**v))
            .collect();
        assert!(
            muets.is_empty(),
            "{} verdict(s) que le code peut rendre sont ABSENTS de la description \
             publiée : {muets:?} — un appelant qui se fie au schéma les rencontrera \
             sans savoir quoi en faire",
            muets.len()
        );
    }
}

// ---------------------------------------------------------------------------------
// REQ-AXO-902602 — l'extraction des ancres citées.
//
// Ce qui est couvert ICI : la reconnaissance des identifiants. Ce qui ne l'est PAS :
// le chargement depuis `soll.node`, le jugement NLI et le verdict fail-closed —
// tous trois exigent une base ET le modèle NLI provisionné. La preuve de bout en
// bout est un appel réel sur le cas KKI, décrit dans le nœud ; un test vert ici n'en
// dit rien, et c'est pourquoi les deux existent.
// ---------------------------------------------------------------------------------
#[cfg(test)]
mod soll_ids_cites_tests {
    use super::soll_ids_cites;

    #[test]
    fn LE_cas_KKI_l_identifiant_cite_est_reconnu() {
        let ids = soll_ids_cites("La claim affirme que REQ-KKI-126 impose un oracle.");
        assert_eq!(ids, vec!["REQ-KKI-126".to_string()]);
    }

    #[test]
    fn plusieurs_identifiants_sans_doublon_dans_l_ordre_du_texte() {
        let ids = soll_ids_cites("DEC-AXO-098 raffine REQ-AXO-902602, cf. DEC-AXO-098.");
        assert_eq!(
            ids,
            vec!["DEC-AXO-098".to_string(), "REQ-AXO-902602".to_string()],
            "un identifiant répété ne doit réserver qu'UN slot"
        );
    }

    #[test]
    fn un_fragment_au_MILIEU_d_un_mot_n_est_pas_un_identifiant() {
        // Sans la frontière de mot, `XREQ-AXO-1` et le suffixe d'un identifiant plus
        // long deviendraient des ancres, et chaque fausse ancre coûte un jugement NLI
        // (~5 s sur CPU) prélevé sur le budget des vraies.
        assert!(soll_ids_cites("XREQ-AXO-126").is_empty());
        assert!(soll_ids_cites("req-axo-126").is_empty(), "la casse est significative");
        assert!(soll_ids_cites("REQ-AXO-").is_empty(), "sans chiffre, pas d'identifiant");
        assert!(soll_ids_cites("REQAXO126").is_empty());
    }

    #[test]
    fn MUTANT_un_texte_sans_citation_ne_reserve_RIEN() {
        // Sans ce contrôle, une extraction qui rendrait toujours quelque chose
        // passerait les cas ci-dessus : ils vérifient ce qu'elle TROUVE, jamais
        // qu'elle sait ne rien trouver. Or c'est ce cas qui décide que le
        // comportement historique — shortlist ANN pure — reste intact.
        assert!(soll_ids_cites("Le service utilise PostgreSQL et non MongoDB.").is_empty());
        assert!(soll_ids_cites("").is_empty());
    }
}
