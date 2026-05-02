# Handoff — 2026-05-03 (Claude Opus 4.7, REQ-AXO-066 P1 + REQ-AXO-139 P0 closed + REQ-AXO-141 shipped + live promoted)

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

Live brain post-promotion **v0.8.0-140-g16568c7** supporte recherche sémantique multi-tokens + parameter_repair contract universel sur 5 tools (cypher / soll_attach_evidence / inspect / dispatcher Invalid-args / soll_apply_plan unresolved logical_keys). Toujours :
- `mcp__axon__query` / `inspect` / `retrieve_context` AVANT grep
- `cypher` pour SOLL : canonical 7 cols `id, type, project_code, title, description, status, metadata` ; filtre métadata via `json_extract_string(metadata, '$.priority')`
- Sur unexpected MCP error : lis `data.parameter_repair.{invalid_field, hint, follow_up_tools, ...}` d'abord ; le contrat est uniforme sur les 5 tools cités

### 1.3 Discipline opérationnelle

- Observe → log SOLL via `document_intent` (REQ-AXO-141 nouveau, discoverable) OU `soll_manager create` → link → re-plan → execute relentlessly (CPT-AXO-020)
- 3-way Axon-issue triage (CPT-AXO-025) à chaque résultat MCP inattendu : Branch 1 hallucination (verify+control+repro) / Branch 2 vrai bug / Branch 3 commercial value-add
- PDCA avec SOLL (CPT-AXO-024) : Plan in SOLL avant Do, update REQ status post-commit
- UN FIX = UN COMMIT (~30-150 LOC, exception légitime à 200-450 LOC pour les feat.s self-contained)
- `axon_commit_work` auto-stage `diff_paths` depuis REQ-AXO-138 (refuse partial diff sur git add fail)

---

## Part 2 — État courant (snapshot 2026-05-03 fin de session)

### 2.1 Live runtime

- **v0.8.0-140-g16568c7** (promoted 2026-05-03 01:08 UTC) HEALTHY
- 1 promotion cette session : g16568c7 (toutes les commits inclus)
- Profile brain_only + indexer_full HEALTHY. qualify-mcp core verdict=ok (quality+latency).

### 2.2 6 commits livrés cette session (du plus récent au plus ancien)

| SHA | REQ | Commit message |
|-----|-----|----------------|
| `16568c7` | REQ-AXO-141 | feat(mcp) — document_intent universal SOLL log entry point |
| `88d5e67` | REQ-AXO-139 slice 5 | feat(soll) — apply_plan unresolved logical_keys in errors[] + parameter_repair |
| `ce83627` | REQ-AXO-139 slice 4 | feat(dispatch) — Invalid-arguments fallback returns parameter_repair |
| `5e34748` | REQ-AXO-139 slice 3 | feat(inspect) — symbol-not-found returns parameter_repair contract |
| `f82bd2b` | REQ-AXO-139 slice 2 | feat(soll) — soll_attach_evidence returns parameter_repair contract |
| `c9041da` | REQ-AXO-066 Phase 1 | feat(soll) — multi-tenant project_code scoping + composite indexes |

LOC total: ~1450 ajoutées (graph_bootstrap +260 / scoped_query_filter +90 / evidence parameter_repair +330 / inspect +114 / dispatch +105 / apply_plan +208 / document_intent +427).

### 2.3 SOLL state final

- **completed=112+ (was 108 pre-session)**, planned=12, in_progress=2 (REQ-070 hygiène umbrella, REQ-080 closed mais pas re-flippé)
- 0 violations stable
- 3 nouveaux REQs créés cette session : REQ-AXO-147 (parameter_repair rollout ~70 sites restants), REQ-AXO-148 (FsWatcher path-router thread store), tous deux REFINES leurs parents (REQ-AXO-139 et REQ-AXO-066 respectivement)
- 2 REQs flippés terminal : REQ-AXO-132 (Phase 1 spec) et REQ-AXO-141 et REQ-AXO-139 → completed
- Tests: 920 → 943 (session prev) → 964 (cette session) : **+21 tests, 0 régression, 0 ignored** stables.

### 2.4 Universal parameter_repair contract (REQ-AXO-139 closed)

5 surfaces couvertes par le contrat canonique `data.parameter_repair.{invalid_field, hint, follow_up_tools, ...}`:
- cypher binder errors (e003748 prev session)
- soll_attach_evidence per-kind hint (f82bd2b)
- inspect symbol-not-found widening (5e34748)
- dispatcher Invalid-arguments fallback (ce83627)
- soll_apply_plan unresolved logical_keys (88d5e67)

Audit REQ-AXO-139 slice 6 a identifié **~70 sites isError supplémentaires** sans parameter_repair, dans 16 fichiers MCP-handler. Capturé comme **REQ-AXO-147** (rollout, ~800 LOC, 8-12 commits estimés).

### 2.5 Multi-tenant Axon (REQ-AXO-066 Phase 1 done, Phase 2 spec'd)

Phase 1 (commit c9041da) a livré :
- 10 IST composite indexes (CALLS / CALLS_NIF / CONTAINS / IMPACTS / SUBSTANTIATES / Symbol / File)
- 6 SOLL composite indexes (Node / Edge / McpJob / Revision / RevisionChange)
- ALTER + idempotent backfill sur soll.{Edge, McpJob, Revision, RevisionChange} pour ajouter project_code colonne
- `scoped_query_filter()` helper standardisé (2 sites migrés en démonstrateur)
- Latent fix : `rebuild_file_runtime_table` et `reset_ist_state` recréent maintenant les indexes File post-DROP

Phase 2 NOT done : nécessite threading `Option<&GraphStore>` à travers la chaîne d'événements du watcher (5+ fonctions dont `enqueue_single_file_delta`). Capturé comme **REQ-AXO-148** (~300 LOC, fresh planning session).

### 2.6 document_intent MCP tool first-class (REQ-AXO-141 done)

Commit 16568c7 :
- Tool discoverable via tools_catalog : `{intent, body, suggest_type?, tags?, project_code?}`
- Server-side keyword classifier (4 classes : requirement / decision / concept / guideline) avec règle de priorité documentée (problem-class wins over concept-class)
- 5 unit tests + 2 integration tests via tools/call
- SKILL.md : table SOLL-writes le mentionne comme entry point canonique pour les workflows "documente"

Pour la prochaine session : un fresh LLM peut maintenant trouver `document_intent` dans tools_catalog dans la première minute, sans config CLAUDE.md.

---

## Part 3 — Travail en attente

### 3.1 Top-priority follow-ups créés cette session

| REQ | Tier | LOC | Sujet |
|-----|------|-----|-------|
| **REQ-AXO-147** | P1 | ~800 | parameter_repair coverage rollout — 70 sites restants sur 16 fichiers MCP-handler |
| **REQ-AXO-148** | P1 | ~300 | FsWatcher path-router : thread store via `enqueue_single_file_delta` (REQ-AXO-066 Phase 2) |

### 3.2 Commercial value-adds restants (handoff précédent + ajustements)

| REQ | Tier | LOC | Sujet | Statut session |
|-----|------|-----|-------|---------------|
| ~~REQ-AXO-132~~ | P1 | ~200 | Multi-tenant audit + indexes | **DONE c9041da** |
| ~~REQ-AXO-139~~ | P0 | ~200 | Universal parameter_repair | **DONE (5/5 slices + audit)** |
| ~~REQ-AXO-141~~ | P1 | ~200 | document_intent MCP tool | **DONE 16568c7** |
| REQ-AXO-066 Phase 2 | P1 | ~300 | path-router (subdivisé en REQ-AXO-148) | spec'd |
| REQ-AXO-066 Phase 3 | P1 | ~450 | axonctl register-project + Q3=F+H optional param | planned |
| REQ-AXO-140 | P1 | ~400 | IST canonical Symbol.id Rust cross-module | planned |
| REQ-AXO-147 | P1 | ~800 | parameter_repair rollout 70 sites | planned (this session) |
| REQ-AXO-148 | P1 | ~300 | FsWatcher path-router store threading | planned (this session) |
| REQ-AXO-142 | P2 | ~250 | Test fixtures Rust IST/SOLL | planned |
| REQ-AXO-143 | P2 | ~150 | Configurable session_pointer | planned |
| REQ-AXO-144 | P2 | ~100 | work_plan temporal score decay | planned |
| REQ-AXO-145 | P2 | ~80 | pre_flight_check per-file mode | planned |
| REQ-AXO-146 | P2 | ~120 | Async job_status event-driven | planned |
| REQ-AXO-079 | — | ~300 | Single finalization path (bloqué arch decision) | bloqué |

### 3.3 Hygiène différée

- 14 REQs `current/in_progress` à auditer sur l'historique (REQ-AXO-001..049 reste)
- 1 untracked: ce fichier handoff (sera supersédé par le prochain)

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-03-handoff-multi-tenant-and-parameter-repair.md`. Applique la Part 1 en entier avant toute action. Puis appelle `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`. **Utilise les outils Axon IST en premier.** Pour SOLL, utilise `retrieve_context` ou `cypher SELECT FROM soll.main.Node` (canonical 7 cols). Demande-moi quoi attaquer en priorité — sauf si je dis "go" dans quel cas attaque REQ-AXO-147 first slice (parameter_repair rollout, prioriser tools_soll/operations.rs +15 isError sites).

### 4.2 Smoke test

```
mcp__axon__status mode=brief
```

Doit afficher `Runtime identity: axon-live-axon-brain` et `data.readiness.kind: ready`. Live build attendu : `v0.8.0-140-g16568c7` ou plus récent.

### 4.3 Promotion live

`bash scripts/release/promote_live_safe.sh --project AXO` — seule voie autorisée, jamais cargo build manuel.

### 4.4 Next session priority recommandé

**Option A** (universal contract closure) : REQ-AXO-147 first slice — couvrir tools_soll/operations.rs (15 sites) qui est le plus gros fichier non-couvert. ~150 LOC + tests, très alignement avec la rigueur démontrée cette session.

**Option B** (multi-tenant continuation) : REQ-AXO-148 — FsWatcher path-router store threading. ~300 LOC arch-sensible, fresh budget recommandé. Closes DEC-AXO-064 Q2=D.

**Option C** (UX leverage) : tester `document_intent` côté LLM client — ajouter une wrapper test asseyant que la classification + persistence cycle complète bien sur `/document_intent` slash-command.

---

C'est tout. Bonne session.
