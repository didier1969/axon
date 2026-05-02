# Handoff — 2026-05-02 (Claude Opus 4.7, watchdog detection-half + REQ-AXO-099 Phase 1-3)

> **Lis cette section en premier**. La méthodologie est le cœur de la coopération avec Didier dans ce dépôt.

---

## Part 1 — Cold-start (mandatoire)

### 1.1 Ordre de lecture

1. `~/.claude/CLAUDE.md`
2. `~/projects/axon/CLAUDE.md`
3. `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`
4. `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`
5. `mcp__axon__help` puis `mcp__axon__status mode=brief`
6. `mcp__axon__retrieve_context question="REQ-AXO-099 acceptance criteria phase 4"` (NE PAS utiliser `cypher` pour SOLL — voir REQ-AXO-129)
7. `mcp__axon__soll_validate project_code=AXO`
8. `mcp__axon__soll_work_plan project_code=AXO format=brief top=5`

### 1.2 IST en premier (rappel)

Le live brain (post-2026-05-02 promotion) supporte la recherche sémantique multi-tokens. Use Axon IST tools:
- `mcp__axon__query` / `inspect` / `retrieve_context` — TOUJOURS avant grep
- `cypher` est limité au DB IST; SOLL nodes ne sont PAS dans `main.Node` (REQ-AXO-129)

### 1.3 Discipline opérationnelle

- Observe → log SOLL → link → re-plan → execute relentlessly.
- UN FIX = UN COMMIT (~30-150 LOC).
- PRÉ-FLIGHT puis COMMIT. PRÉ-STAGE LES MODIFS (`git add <files>` AVANT `axon_commit_work`).

---

## Part 2 — État courant (snapshot 2026-05-02 fin de session)

### 2.1 Live runtime

- **Live brain** : **v0.8.0-115-g44eaec4** (re-promoted 2026-05-02 12:13 with the 4 REQs of "l'ensemble"). 13 session commits all live + pushed to origin/main.
- Qualify-mcp: verdict=ok (quality + latency).
- Profil: brain_only (brain) + indexer_full (indexer side). HEALTHY. Readiness contract + watchdog detection + BEAM alarm classification live.

### 2.2 Commits livrés cette session (13, du plus récent au plus ancien)

| SHA | REQ | Type | Note |
|---|---|---|---|
| 44eaec4 | REQ-AXO-094 | feat | BEAM alarm classification (Rust receiver + Elixir reporter big-bang) |
| 6224e0d | REQ-AXO-129 | fix | Plugin error envelope replaces silent [] for invalid SQL |
| 23ad251 | REQ-AXO-108 | feat | data_root_absolute on status for unambiguous cross-reference |
| f7a9f44 | REQ-AXO-096 | chore | Drop .tool-versions; devenv as sole toolchain |
| b67f5fe | REQ-AXO-097 | feat | axonctl auto-restart subcommand (cross-process restart half) |
| 76e1edd | REQ-AXO-099 P4 | test | HOME-leak root cause; suite 0 failures, 920 passed |
| a0297b6 | REQ-AXO-099 P3 | test | WATCHER_PROBE_GUARD for main_background watcher_probe tests |
| 09b6d71 | REQ-AXO-099 P2 | test | embedder gpu_telemetry + batch_lanes test isolation |
| 157ac07 | REQ-AXO-099 P1 | test | test_support::EnvVarGuard + env_test_lock; optimizer fixes |
| f5f4f2d | REQ-AXO-097 | feat | Watchdog detection-half (in-process staleness flipper + heartbeaters) |
| cd64ace | REQ-AXO-127 | chore | GUARD_CONSECUTIVE_RECYCLES dead_code allow |

REQ-AXO-129 (SOLL/cypher contract bug) created; REQ-AXO-097 status updated to `partially_satisfied`.

### 2.3 Test suite progress — CLOSED

- Pre-session: 24 cross-module failures.
- Post-session: **0 failures, 920 passed** (REQ-AXO-099 fully closed, commit 76e1edd).
- Root cause: a single test (`test_embedding_model_cache_dir_defaults_outside_workspace` in embedder.rs:7438) was unconditionally `remove_var("HOME")` at the end. DuckDB's `INSTALL json` needs HOME for the extension cache lookup; with HOME unset, every subsequent test using `attach_distinct_reader_snapshot` failed at INSTALL.
- Phases 1-3 had built isolation infrastructure (test_support module + EnvVarGuard + env_test_lock + per-cluster mutexes) that will prevent future regressions; Phase 4 found and patched the actual leak.

### 2.4 SOLL (project AXO)

- `soll_validate`: 0 violation.
- New: REQ-AXO-129 (cypher cannot read SOLL nodes — LLM-contract bug under CPT-AXO-018).
- Updated: REQ-AXO-097 (partially_satisfied), REQ-AXO-099 (Phase 1-3 shipped status).

---

## Part 3 — Travail en attente

### 3.1 REQ-AXO-099 — CLOSED

Diagnosis and final fix shipped in commit 76e1edd. See REQ-AXO-099 in SOLL for full audit log. Suite is now 929/0/2 (passed/failed/ignored).

### 3.2 Suite de session — phases supplémentaires shippées 2026-05-02 (lock by user "go")

REQ-AXO-097 (cross-process), REQ-AXO-094 (BEAM alarms), REQ-AXO-096 (toolchain), REQ-AXO-108 (data_root), REQ-AXO-129 (plugin error envelope), REQ-AXO-067 (SOLL coverage health — confirmed-already-delivered) tous CLOSED. Status hygiene Phase 1 = 8 REQs flippés (001/046/060/061/067/081/082/084/085). Live promu **v0.8.0-115-g44eaec4** + push origin/main confirmé.

### 3.3 DECs proposed pending user pick (ne PAS implémenter sans validation)

- **DEC-AXO-063** (REQ-AXO-094) — picks A/A/A actés, status devrait passer à `accepted` au prochain session-start (TODO).
- **DEC-AXO-064** (REQ-AXO-066 multi-tenant) — propose Q1=B (per-project IST/SOLL DB attached as schemas), Q2=D (one global watcher with path router), Q3=F+H (optional project_code + cwd fallback). 5-phase delivery ~1300 LOC over multiple sessions. Status `proposed`. Le user doit picker A/B/C par question avant Phase 1.

### 3.4 Deferred à fresh session (multi-session par nature)

- **REQ-AXO-080** : Extract worker loops from embedder.rs monolith — gros refactor, plusieurs sessions, demande IST-baseline avant et après.
- **REQ-AXO-066** : multi-tenant — code Phase 1 à 5 selon DEC-AXO-064 après validation user.
- **REQ-AXO-094 (a) zero-warning CI workflow** + **(c) layered boot output** : nécessite ajout d'un workflow CI Rust/Elixir dans `.github/workflows/` — touche infra partagée, mérite OK séparé.

### 3.5 Status hygiene Phase 2 (deferred, low-risk)

~30 "current" / "accepted" / "in_progress" / "planned" REQs en SOLL n'ont pas été audités pour status hygiene par manque de budget context. Opportunité : audit batch dans une fresh session — pour chaque REQ, vérifier si déjà délivré par le travail récent puis flip à `completed`.

### 3.2 Open architecture decisions

- **REQ-AXO-094 BEAM alarm classification** : push vs pull dashboard state — unresolved. Cross-language Elixir+Rust ~300 LOC. Needs DEC.
- **REQ-AXO-099 root cause** : production code in runtime_boot.rs:269-298 sets ~10 env vars as config bus. Even after Phase 4-N, the production pattern remains. Three directions documented in REQ-AXO-099. User has not picked A/B/C; current path is implicit B (medium audit).
- **REQ-AXO-097 cross-process restart half** : axonctl needs to poll `mcp__axon__status` data.subsystems and restart Failed roles. ~150 LOC, separate REQ-able.
- **REQ-AXO-108 IST roots** : six locations coexist; consolidation design pending.
- **REQ-AXO-096 toolchain** : drop mise OR drop devenv pinning. Decision pending.

### 3.3 Hygiène

- 7 untracked working-notes / scripts / queries / SOLL exports remain in working tree (visible in `git status`). User's call whether to clean.

---

## Part 4 — Comment démarrer la prochaine session

### 4.1 Phrase de boot

> Lis dans l'ordre : `~/.claude/CLAUDE.md`, `~/projects/axon/CLAUDE.md`, `~/.claude/projects/-home-dstadel-projects-axon/memory/MEMORY.md`, puis `docs/working-notes/2026-05-02-handoff-watchdog-and-test-isolation.md`. Applique la Part 1 en entier avant toute action. Puis appelle `mcp__axon__axon_init_project project_path=/home/dstadel/projects/axon`. **Utilise les outils Axon IST en premier.** Pour SOLL, utilise `retrieve_context` ou `soll_query_context`, JAMAIS `cypher SELECT FROM main.Node` (REQ-AXO-129). Demande-moi quoi attaquer en priorité — sauf si je dis "go" dans quel cas continue REQ-AXO-099 Phase 4 (runtime_surface).

### 4.2 Smoke test

```
mcp__axon__status mode=brief
```

Doit afficher `Runtime identity: axon-live-axon-brain` et `data.readiness.kind: ready`. Live n'a PAS encore les 5 derniers commits — ils sont dans dev/HEAD.

### 4.3 Promotion live

Quand le user voudra promouvoir HEAD (a0297b6 plus tard) en live :
```
bash scripts/release/promote_live_safe.sh --project AXO
```

C'est la seule voie autorisée (pas de cargo build manuel).

---

C'est tout. Bonne session.
