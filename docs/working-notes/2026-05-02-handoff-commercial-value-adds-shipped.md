# Handoff — 2026-05-02 (Claude Opus 4.7, REQ-080 closed + 6 commercial value-adds shipped + DEC-AXO-064 accepted)

> **Lis cette section en premier**. SOLL est canonique ; ce fichier est un artefact session-private qui sera supersédé par le prochain handoff.

---

## Part 1 — Cold-start (mandatoire)

### 1.1 Ordre de lecture

1. `~/.claude/CLAUDE.md`
2. `~/projects/axon/CLAUDE.md`
3. `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`
4. `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`
5. `mcp__axon__help` puis `mcp__axon__status mode=brief`
6. `mcp__axon__cypher SELECT description FROM soll.main.Node WHERE id IN ('CPT-AXO-024','CPT-AXO-025') AND project_code='AXO'` (méthodologie SOTA)
7. `mcp__axon__soll_validate project_code=AXO`
8. `mcp__axon__soll_work_plan project_code=AXO format=brief top=10`

### 1.2 IST en premier

Live brain post-promotion v0.8.0-132-ge003748 supporte recherche sémantique multi-tokens + structured-recovery sur cypher binder errors. Toujours :
- `mcp__axon__query` / `inspect` / `retrieve_context` AVANT grep
- `cypher` pour SOLL : canonical 7 cols `id, type, project_code, title, description, status, metadata` ; filtre métadata via `json_extract_string(metadata, '$.priority')`
- Sur cypher error : lis `data.parameter_repair.{missing_column, available_columns, hint}` (REQ-AXO-139 slice) avant retry

### 1.3 Discipline opérationnelle

- Observe → log SOLL (CPT-AXO-019 documente) → link → re-plan → execute relentlessly (CPT-AXO-020)
- 3-way Axon-issue triage (CPT-AXO-025) à chaque résultat MCP inattendu : Branch 1 hallucination (verify+control+repro) / Branch 2 vrai bug / Branch 3 commercial value-add
- PDCA avec SOLL (CPT-AXO-024) : Plan in SOLL avant Do, update REQ status post-commit
- UN FIX = UN COMMIT (~30-150 LOC)
- `axon_commit_work` auto-stage `diff_paths` depuis REQ-AXO-138 (refuse partial diff sur git add fail)

---

## Part 2 — État courant (snapshot 2026-05-02 fin de session)

### 2.1 Live runtime

- **v0.8.0-132-ge003748** (promoted 2026-05-02 17:53) HEALTHY
- 3 promotions cette session : g44eaec4 → gfbc1d17 → gc0d8c0a → ge003748
- Profile brain_only + indexer_full HEALTHY. Readiness contract + watchdog + BEAM alarms live.

### 2.2 21 commits livrés cette session (du plus récent au plus ancien)

| SHA | REQ | Commit message |
|-----|-----|----------------|
| `cbca185` | REQ-AXO-139 polish | fix(cypher) — parse_duckdb_binder_error ignores LINE marker single-candidate |
| `aae0c1d` | session hygiene | chore(archive) — supersede watchdog handoff |
| `e003748` | REQ-AXO-139 (slice) | feat(cypher) — binder errors return parameter_repair contract |
| `c0d8c0a` | REQ-AXO-137 | feat(soll) — apply_plan response surfaces resolved canonical ids |
| `2fd4646` | REQ-AXO-138 | feat(commit) — axon_commit_work refuses partial-diff on git add failure |
| `fbc1d17` | REQ-AXO-134 | feat(inspect) — callers/callees survive synthetic CALLS target_id format |
| `83ce5df` | REQ-AXO-136 | feat(soll) — verify_requirements recognizes terminal status as done |
| `5e37300` | REQ-AXO-135 | feat(soll) — work_plan excludes terminal-state nodes from waves |
| `7a5913e` | REQ-AXO-080 P6 | refactor(embedder) — extract vector_worker_loop hot path + build_model |
| `23ccaaf` | REQ-AXO-080 P5 | refactor(embedder) — vector_finalize_worker_loop + outbox helper |
| `e859832` | REQ-AXO-080 P4 | refactor(embedder) — vector_persist_worker_loop |
| `2fa7c36` | REQ-AXO-080 P3 | refactor(embedder) — vector_prepare_worker_loop |
| `7dd2053` | REQ-AXO-080 P2 | refactor(embedder) — vector_maintenance_worker_loop |
| `2670b2b` | REQ-AXO-133 | refactor(embedder) — vector_refill_worker_loop |
| `632b9d4` | REQ-AXO-131 | chore(archive) — 4 handoffs + BOOTSTRAP_PROMPT supersédés |
| `f6326e1` | REQ-AXO-056 | feat(memgraph) — Lab importable query collection deliverables |
| `3ecd47e` | REQ-AXO-054 | feat(qualify) — Polars temporal vector benchmark analyzer |
| `c6e7351` | REQ-AXO-053 | chore(bin) — 3 Rust binary entrypoint shims trackés |
| `9534f37` | REQ-AXO-130 | docs(skill) — LLM-only rationalization SKILL.md 497→161 lignes |

### 2.3 SOLL state final

- **completed=108** (vs 34 pré-session, +74 grâce à REQ-AXO-136 fix)
- partial=33, missing=0, in_progress=2 (REQ-070 hygiène umbrella, REQ-080 closed mais pas re-flippé), planned=10
- 0 violations stable
- 18 SOLL nodes créés cette session : CPT-AXO-024/025, DEC-AXO-065, REQ-AXO-130..146

### 2.4 embedder.rs : 10,313 → 8,529 LOC (−17.3%)

REQ-AXO-080 closed. Tous les 6 worker loops extraits dans `embedder/`:
- vector_refill_loop, vector_maintenance_loop, vector_prepare_loop, vector_persist_loop, vector_finalize_loop, vector_worker_loop (+ build_vector_embedding_model)

### 2.5 Test suite : 920 → 943/0/2 (+23)

Zero warning maintenu. 9 nouveaux tests cette session (linkage + algorithm + parser).

---

## Part 3 — Travail en attente

### 3.1 DEC-AXO-064 ACCEPTÉE — picks A/D/F+H (multi-tenant Option A)

**Décision finale** : 1 IST + 1 SOLL partitionné par `project_code` (Option A), 1 watcher path-router (Option D), optional `project_code` + cwd fallback (Option F+H).

**Pourquoi A et pas B** : Didier est multi-projet mono-tenant (AXO + BKS + NTO + PRO sont tous SES projets). Option A gagne pour 7 raisons : cross-project queries triviales, best practice propagation native via `axon_apply_guidelines`, pattern mining cross-projet, pas de cap ATTACH ~64, schema evolution simple, FK cross-project possibles, operational simplicity. Option B (multi-attach DBs) était over-engineering pour SaaS multi-customer non-applicable.

**REQ-AXO-132 spec révisé Option A** (`planned`, ~200 LOC) :
- Audit cypher de chaque table principale → liste celles sans project_code OU sans index `(project_code, id)`
- ALTER TABLE + backfill `project_code='AXO'` pour rows historiques (idempotent)
- Indexes (project_code, id) sur Node, Edge, Symbol, CALLS (175k+ rows = scale-critical)
- Helper `scoped_query_filter()` standardisé
- Smoke test 2 projets isolés sémantiquement

### 3.2 Slices REQ-AXO-139 restants (universal parameter_repair contract)

Slice cypher binder livré. Restants :
- soll_attach_evidence : per-kind required-field hint (artifact_ref vs path vs uri vs file_path)
- inspect : symbol-not-found avec widening suggestions
- query : 'Invalid arguments' avec input_schema reference
- soll_apply_plan : unresolved logical_keys list dans errors[]
- Audit complet des MCP public tools restants

### 3.3 10 commercial value-adds restants (planned)

| REQ | Tier | LOC | Sujet |
|-----|------|-----|-------|
| REQ-AXO-066 Phase 1 | P1 | ~200 | Multi-tenant audit + indexes (DEC-064 accepted) |
| REQ-AXO-139 (slices) | P0 | ~200 | Universal parameter_repair restants |
| REQ-AXO-140 | P1 | ~400 | IST indexer canonical Symbol.id Rust cross-module |
| REQ-AXO-141 | P1 | ~200 | `document_intent` MCP tool first-class |
| REQ-AXO-142 | P2 | ~250 | Test fixtures Rust IST/SOLL |
| REQ-AXO-143 | P2 | ~150 | Configurable session_pointer |
| REQ-AXO-144 | P2 | ~100 | work_plan temporal score decay |
| REQ-AXO-145 | P2 | ~80 | pre_flight_check per-file mode |
| REQ-AXO-146 | P2 | ~120 | Async job_status event-driven |
| REQ-AXO-079 | — | ~300 | Single finalization path (bloqué arch decision) |

### 3.4 Hygiène différée

- 7 untracked sont gérés (active handoff seul reste — ce fichier sera supersédé)
- Status hygiene Phase 2 : ~15 REQs `current/in_progress` à auditer (REQ-AXO-001..049 reste)

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-02-handoff-commercial-value-adds-shipped.md`. Applique la Part 1 en entier avant toute action. Puis appelle `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`. **Utilise les outils Axon IST en premier.** Pour SOLL, utilise `retrieve_context` ou `cypher SELECT FROM soll.main.Node` (canonical 7 cols). Demande-moi quoi attaquer en priorité — sauf si je dis "go" dans quel cas attaque REQ-AXO-066 Phase 1 (multi-tenant audit + indexes, ~200 LOC, Option A confirmée).

### 4.2 Smoke test

```
mcp__axon__status mode=brief
```

Doit afficher `Runtime identity: axon-live-axon-brain` et `data.readiness.kind: ready`. Live build attendu : `v0.8.0-132-ge003748` ou plus récent.

### 4.3 Promotion live

`bash scripts/release/promote_live_safe.sh --project AXO` — seule voie autorisée, jamais cargo build manuel.

### 4.4 Next session priority recommandé

**Option A** (technique, fresh budget) : REQ-AXO-066 Phase 1 audit + indexes (~200 LOC, DDL hot path)
**Option B** (UX leverage) : REQ-AXO-141 `document_intent` MCP tool (~200 LOC, self-contained)
**Option C** (slice continuation) : REQ-AXO-139 next slice (soll_attach_evidence per-kind hint)

---

C'est tout. Bonne session.
