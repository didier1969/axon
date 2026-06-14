# Session 78 — handoff (2026-06-14)

Session « tout en or » : finition PG→RAM, fermeture classe Symbol.embedding, alignement petgraph, latence sémantique GPU, honnêteté status, toggle GPU runtime. Session_pointer canonique = `CPT-AXO-052` (à jour).

## Livré (branche `main`, builds + tests verts, NON poussé / NON promu)
| REQ / nœud | Contenu |
|---|---|
| REQ-AXO-901952 | **Acceptance atteinte** : 0 traversée IST PG. inspect RAM-only (fallback PG retiré, cold=erreur), soll_manager cycle-guard via SollSnapshot petgraph, sql hint sans ist.path. count(*) ist.Edge agrégés gardés (métriques). |
| REQ-AXO-901977 | query + semantic_clones rankent via chunk-embeddings (Symbol.embedding jamais peuplé). retrieve_context déjà OK (901937). |
| GUI-AXO-1027 | Règle anti-dérive SOLL=petgraph / IST=CSR-léger+petgraph-lourd. |
| REQ-AXO-901978 | Latence sémantique : B1 GPU query-embed (start.sh provisionne GPU même brain_only) ; A `semantic=auto\|lexical\|semantic` ; B2 préchargé ; B3 cache. |
| REQ-AXO-901979 | status/embedding_status reportent le vrai GPU/CPU du worker (flag process-local). |
| REQ-AXO-901984 | **Toggle GPU runtime** : outil `embed_provider` (get/set cpu\|gpu\|auto) sans restart. |
| REQ-AXO-901958/962/963 | dead-code #[test] exclus / statut legacy nommé / push code-intel (lot antérieur). |
| REQ-AXO-901980 | (suivi) migration attribution GPU nvidia-smi→NVML cross-process. |
| REQ-AXO-901982 | (suivi) project_status 17,4s agrégation lourde à profiler. |
| REQ-AXO-901983 | (backlog) éval qualité/token une-à-une des 71(72) commandes — audit md = première passe. |

**Tests : 1148+ verts** (+ tests neufs : query_is_symbol_lookup, query_vec_cache, query_worker_compute_label, override). Catalogue = **72 outils** (+embed_provider).

## Insights stats/friction (SOTA) — axon.mcp_friction, top OPEN
1. `query/degraded` ×35 → **adressé par 901978** ; marquer résolu après promote.
2. `soll_attach_evidence/artifact_ref` ×34 + `sql/input_invalid` ×27 + `soll_manager/forbidden_relation` ×11 → **NON adressé**, relève de **REQ-AXO-901947 (Guided MCP)** — c'est le prochain gros levier produit (LLM-friction = métrique clé). Findings attachés à 901947.
- Anecdote vécue : le format `artifact_ref` de soll_attach_evidence m'a fait échouer 2× cette session (artifact_type=Document valide un chemin fichier ; Validation/Metric = texte libre) → confirme la friction #2.

## Évaluation des 6 REQ tiers (demandée — pertinence, pas implémentation)
Tous **pertinents, aucun stale** :
- 901895 (chunker DP), 901896 (audit pipeline A+B) : perf pipeline, legit.
- **901934 (token-cost « output tokens ARE the cost function »)** : stratégique SOTA, lié à la colonne Token de l'audit.
- **901947 (Guided MCP : form + repair sémantique + elicitation)** : stratégique, c'est l'umbrella des fixes friction ci-dessus.
- 901968 (restreindre stop/restart runtime aux agents on-host) : sécurité, legit.
- 901976 (retrieve_context rationale gate + validation E2E live) : lié 977/978, legit.

## État runtime
- Brain dev sur :44139 = binaire de session ; en mode GPU (start.sh provisionne le GPU en brain_only — comportement voulu désormais).
- MCP natif de session : soll_manager non chargé en natif (quirk) → updates SOLL via curl :44129.

## NEXT (operator-gated)
1. **`promote_live_safe.sh --project AXO` + `git push`** (commits non poussés).
2. Post-promote : marquer REQ `delivered` ; mesurer latence via `mcp_telemetry_report` (cible P50 < 1s) ; `mcp_friction_report mark_resolved` query/degraded vs 901978.
3. Prioriser **REQ-901947 (Guided MCP)** = plus gros levier friction restant.
4. Suivi : 901980 (NVML), 901982 (project_status), 901983 (éval 71 cmds).

## Leçon process
Voir `~/.claude/.../memory/feedback_self_audit_before_done.md` : avant « terminé », balayer tests-sur-logique-neuve / handoff / état runtime propre.
