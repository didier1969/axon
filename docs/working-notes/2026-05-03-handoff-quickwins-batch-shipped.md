# Handoff — 2026-05-03 (Claude Opus 4.7, P1 multi-tenant + 3 P2 quick-wins shipped, REQ-AXO-142 deferred)

> **Lis cette section en premier**. SOLL est canonique ; ce fichier est un artefact session-private qui sera supersédé par le prochain handoff.

---

## Part 1 — Cold-start (mandatoire)

### 1.1 Ordre de lecture

1. `~/.claude/CLAUDE.md`
2. `~/projects/axon/CLAUDE.md`
3. `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`
4. `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`
5. `mcp__axon__help` puis `mcp__axon__status mode=brief`
6. `mcp__axon__cypher SELECT description FROM soll.main.Node WHERE id IN ('CPT-AXO-024','CPT-AXO-025') AND project_code='AXO'`
7. `mcp__axon__soll_validate project_code=AXO`
8. `mcp__axon__soll_work_plan project_code=AXO format=brief top=10`

### 1.2 IST en premier

Live brain post-promotion **v0.8.0-152-g2308d88** : universal parameter_repair + REQ-AXO-148 path-router + REQ-AXO-145 incremental pre-flight + REQ-AXO-144 temporal score decay + REQ-AXO-146 job_status wait + REQ-AXO-143 session_pointer.

- `mcp__axon__query` / `inspect` / `retrieve_context` AVANT grep
- `cypher` pour SOLL : canonical 7 cols `id, type, project_code, title, description, status, metadata`
- Sur error : lis `data.parameter_repair` d'abord (pattern uniforme)
- **Async mutations** : passe `wait: true` à `job_status` (REQ-AXO-146) pour bloquer jusqu'au terminal en un round-trip
- **Pre-flight** : passe `incremental: true` à `axon_pre_flight_check` (REQ-AXO-145) pour validation par-fichier

### 1.3 Discipline opérationnelle

- Observe → log SOLL via `document_intent` OU `soll_manager create` → link → re-plan → execute relentlessly (CPT-AXO-020)
- 3-way Axon-issue triage (CPT-AXO-025) à chaque résultat MCP inattendu
- PDCA avec SOLL (CPT-AXO-024)
- UN FIX = UN COMMIT (~30-200 LOC ; jamais batch >5 fichiers sans `cargo test` complet entre — cf. feedback_disk_space_discipline.md)
- Avant tout batch : `df -h /home/dstadel/projects/axon/` ≥ 10GB libre, ≥30GB pour batch lourd (multi-fichiers + builds itératifs)

---

## Part 2 — État courant (snapshot 2026-05-03 fin de session)

### 2.1 Live runtime

- **v0.8.0-152-g2308d88** (promoted 2026-05-03) HEALTHY
- 4 promotions cette session (cumule 5 commits feature)
- qualify-mcp core verdict=ok (quality+latency) sur chaque promotion

### 2.2 5 commits feature livrés cette session (du plus récent au plus ancien)

| SHA | REQ | LOC | Sujet |
|-----|-----|-----|-------|
| `2308d88` | REQ-AXO-143 | +338/-7 | Configurable session_pointer (workflow-agnostic onboarding) |
| `27b4b74` | REQ-AXO-146 | +173/-3 | job_status wait mode (élimine polling) |
| `6eb4f29` | REQ-AXO-144 | +208/-6 | work_plan temporal score decay |
| `f2ec27e` | REQ-AXO-145 | +222/-6 | axon_pre_flight_check incremental per-file mode |
| `517a418` | REQ-AXO-148 | +168/-9 | fs-watcher: thread store through enqueue_* (path-router symmetry) |

LOC total cette session : +1109/-31 (5 features + handoff).

### 2.3 SOLL state final

- **completed=119** (was 114 prev session, +5 : REQ-AXO-148, REQ-AXO-145, REQ-AXO-144, REQ-AXO-146, REQ-AXO-143 closed)
- partial=27, in_progress=2 (REQ-070 hygiène umbrella, REQ-080 closed mais pas re-flippé)
- **0 violations stable**
- 1 missing : REQ-AXO-142 (test fixtures) — explicitement deferred (voir Part 3)
- Tests: 980 lib (start session 970 → end 980, +10 net cumulative across 5 features)

### 2.4 Customer-value features (REQ-AXO-145, 144, 146, 143)

Cette session a livré 4 améliorations CPT-AXO-025 Branch 3 (commercial-value / llm-friction) :

- **REQ-AXO-145** : `axon_pre_flight_check incremental: true` retourne `data.per_file_violations` keyé par chemin. LLM authoring N fichiers détecte une TDD-gate failure sur le 1er sans avoir authé 2..N.
- **REQ-AXO-144** : `soll_work_plan` applique `score *= exp(-age_days / half_life_days)` aux nodes avec `updated_at`. Default `include_decay=true`, `half_life_days=30`. Decisions accepted matures sortent naturellement de wave 1.
- **REQ-AXO-146** : `job_status wait: true` bloque jusqu'au terminal en un round-trip. Élimine la latence polling (typique 2s × N polls). Default `wait=false` préserve le polling existant.
- **REQ-AXO-143** : `session_pointer = {kind, value, label?}` (kind ∈ file|url|soll_node|none) remplace le hardcoded `active_handoff` file-pattern. Workflow-agnostic — clients sur Linear/Notion/SOLL gardent leur convention. Persiste sur `axon_init_project` arg, surfacé sur `data.kickoff_bundle.session_pointer` ET `status.data.instance_identity.session_pointer`. Backward-compat alias `active_handoff` mirror kind=file.

### 2.5 REQ-AXO-148 — Phase 2 multi-tenant fermé

REQ-AXO-066 Phase 2 (path-router symmetry pour buffered FsWatcher events) : `store: Option<&GraphStore>` threadé à travers les 5 fonctions `enqueue_*`. Quand `scanner.project_code` est vide ET store est `Some`, résolution via `Scanner::project_code_for_path`. Back-compat : reject-with-probe préservé quand both sont vides. DEC-AXO-064 Q2=D fully delivered.

---

## Part 3 — Travail en attente

### 3.1 Top-priority (handoff précédent + cette session)

| REQ | Tier | LOC | Sujet | Status |
|-----|------|-----|-------|--------|
| REQ-AXO-142 | P2 | ~250 | Rust test_support module avec IST/SOLL fixtures | **Deferred fin de session** : disque à 29GB free / 97% used, sous le seuil 30GB pour batch lourd (nouveau module + ré-écriture de 3+ tests existants = nombreux cargo cycles). À reprendre avec disque ≥35GB. |
| REQ-AXO-066 Phase 3 | P1 | ~450 | axonctl register-project + Q3=F+H optional param + cwd fallback | planned |
| REQ-AXO-140 | P1 | ~400 | IST canonical Symbol.id Rust cross-module | planned (besoin disque ≥30GB) |
| REQ-AXO-079 | — | ~300 | Single finalization path (bloqué arch decision) | planned |

### 3.2 Hygiène différée

- ~14 REQs `current/in_progress` à auditer sur l'historique (REQ-AXO-001..049 reste — backlog faible priorité)
- **Disque à 97% post-session** (`/dev/sdc`, 29GB libre) — `feedback_disk_space_discipline.md` recommande monitoring continu. Cleanup recommandé avant prochaine session lourde.

### 3.3 Décisions matures sans evidence (waves 1 dilution candidates)

Avec REQ-AXO-144 livré, le score des Decisions accepted sans `updated_at` reste stable (decay skipped). Action recommandée : batch `soll_attach_evidence` sur DEC-AXO-003..064 pour les anciennes Decisions wave-1 (score=50 chaque, no_evidence reason). Estimé ~30 DECs à attacher, fait baisser leur signal en wave 1.

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-03-handoff-quickwins-batch-shipped.md`. Applique la Part 1 en entier. Puis `mcp__axon__axon_init_project`. **Outils Axon IST en premier.** Pour SOLL utilise `cypher SELECT FROM soll.main.Node` (canonical 7 cols). **VÉRIFIER `df -h` ≥30GB AVANT REQ-AXO-142.** Demande-moi quoi attaquer — sauf si je dis "go" auquel cas attaque REQ-AXO-142 (test fixtures, ~250 LOC).

### 4.2 Smoke test

```
mcp__axon__status mode=brief
```

Doit afficher `Runtime identity: axon-live-axon-brain` et `data.readiness.kind: ready`. Live build attendu : `v0.8.0-152-g2308d88` ou plus récent. `data.instance_identity.session_pointer` (REQ-AXO-143) doit être présent (peut être null ou dériver de l'active handoff existant).

### 4.3 Promotion live

`bash scripts/release/promote_live_safe.sh --project AXO` — seule voie autorisée.

### 4.4 Next session priority recommandé

**Option A** (deferred quick-win) : REQ-AXO-142 — test_support module avec IST/SOLL fixtures. ~250 LOC. **Vérifier disque ≥30GB free avant**. Crée `src/axon-core/src/test_support/ist_fixtures.rs` + ré-écrit 3+ tests existants comme regression baseline.

**Option B** (multi-tenant continuation) : REQ-AXO-066 Phase 3 — axonctl register-project + cwd fallback. ~450 LOC arch-sensible. Fresh budget recommandé.

**Option C** (IST scale-up) : REQ-AXO-140 — IST canonical Symbol.id cross-module. ~400 LOC, profond pipeline indexer. À faire seulement avec disque dégagé (≥30GB libre).

**Option D** (hygiène SOLL) : Batch `soll_attach_evidence` sur DEC-AXO-003..064 wave-1 (~30 DECs). Désinflate la wave 1 du work_plan.

---

C'est tout. Bonne session.
