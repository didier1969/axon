# Axon Pipeline End-to-End Execution Prompt — Session 66+

**Branch** : `feature/pipeline-sq-reorder-point`
**HEAD** : `30df5f53` (sessions 64 + 65 patches REQ-AXO-901835 + REQ-AXO-901840)
**Scope** : **Terminer le projet Axon hors MIL-AXO-027** (umbrella v4 SOLL contract + Tool consolidation MCP 67→15, explicitement exclu).
**Date** : 2026-06-01

---

## 0. Mission

Vous êtes ingénieur senior en système d'indexation GraphRAG. Votre mandat :

1. **Comprendre le pipeline tel qu'il est implémenté** avant tout changement (CPT-AXO-054 = topologie session-19 canonique : single parse en A2, chunking + persistance + FTS en A3, B1 fetch chunk_id only).
2. **Corriger les défauts identifiés** (priorité observabilité + bugs runtime + qualification) sans casser ce qui marche.
3. **Rendre plus efficient + simplifier** quand la simplification améliore performance/résilience.
4. **Livrer toutes les ondes** définies ci-dessous, sauf si l'opérateur arrête explicitement.

Discipline non-négociable : **TDD inversé** (E2E → intégration → unitaire), **zéro warning**, **Swiss-hiking** (faiblesse détectée = résolue OU annoncée SOLL), **dev FIRST** avant promote-live, **branche granular commit** (NOT squash), **soll_validate 0 violations** avant Hand Off.

---

## 1. Snapshot pipeline (lecture obligatoire AVANT toute modification)

### 1.1 Topologie canonique (CPT-AXO-054, session-19, code-anchored)

```
watcher (notify_debouncer 750ms) ──┐
                                   │ arc-swap<DashMap<path, content_hash>>
IndexedFile filter (cold-start ◄───┤ drop si (path, hash) déjà connus
populated via SELECT path,hash)    │
        │                          │
        ▼ (new/modified paths)
A1 work : read(path) + sha256 → PreparedFile
        │
        ▼ mpsc(1024) blocking
A2 transform : tree-sitter WASM → ParsedFile{symbols, edges, content}
        │
        ▼ mpsc(1024) blocking
A3 enregistrement (1 tx/fichier) :
        ├ volet graphe   : UPSERT Symbol + AGE/public.Edge + relations
        ├ volet substrat : build_symbol_chunks (4-layer 512-tok defense) + UPSERT Chunk
        │                  (content_tsv GENERATED → FTS lane auto, zéro stage séparé)
        └ volet filter   : UPSERT IndexedFile(path, content_hash, last_seen_ms)
        │
        ▼ try_send NON-BLOCKING (silent drop si plein, B1 poll rattrape)
b1_inbox mpsc(10_000)
        │   OR cold-start poll DB toutes les 30s :
        │   SELECT chunk_id FROM Chunk LEFT JOIN ChunkEmbedding WHERE ce.chunk_id IS NULL
        ▼
B1 chunk fetch (pure I/O lookup, AUCUN tree-sitter, AUCUN chunking)
        │
        ▼ mpsc(512) blocking
B2 embed GPU (1 worker/GPU, ORT TensorRT BGE-Large 1024d, bucket par seq-len)
        │
        ▼ mpsc(512) blocking
B3 enregistrement vecteur : UPSERT ChunkEmbedding (chunk_id, model_id) ON CONFLICT
```

### 1.2 Backpressure law
- A1→A2→A3 = backpressure-coupled (chaîne blocking)
- B1→B2→B3 = backpressure-coupled
- **A3→B1 volontairement découplé** (PIL-AXO-007 : graphe never waits for GPU)

### 1.3 Activation matrix (AXON_RUNTIME_MODE)
| Mode | A1 | A2 | A3 | B1 | B2 | B3 |
|---|---|---|---|---|---|---|
| `brain_only` | 0 | 0 | 0 | 0 | 0 | 0 |
| `indexer_graph` | N | N | N | 0 | 0 | 0 |
| `indexer_vector` | 0 | 0 | 0 | M | M | M |
| `indexer_full` | N | N | N | M | M | M |

### 1.4 (s, Q) replenishment policy DEC-AXO-901625 (REFINES DEC-AXO-901620)

- **Code existe déjà** sous le nom `demand_pull` (`src/axon-core/src/pipeline_v2/demand_pull.rs`).
- `s` = `AXON_DEMAND_PULL_{A,B}_THRESHOLD` (alias additif `AXON_PIPELINE_{A,B}_SAFETY_STOCK`).
- `Q` = `AXON_DEMAND_PULL_{A,B}_BATCH` (alias additif `AXON_PIPELINE_{A,B}_BATCH_SIZE`).
- Reorder check : `if input_tx.capacity() < threshold { fetch(batch) }` (demand_pull.rs:206-209).
- Hot path PG NOTIFY listener (demand_pull.rs:144,331).
- 30s safety poll fallback pour NOTIFYs ratés.
- Tuning revisé : A 150/100 (~11s work), B 800/400 single-mode, ou bulk 1500/1000.

### 1.5 Sources de vérité code (NE JAMAIS chercher ailleurs)

| Stage | Fichier |
|---|---|
| A1 gate + work | `src/axon-core/src/pipeline_v2/stage_a1.rs` |
| A2 parser | `src/axon-core/src/pipeline_v2/stage_a2.rs` |
| A3 UPSERT | `src/axon-core/src/pipeline_v2/stage_a3.rs` |
| B1 fetch + cold-start poll | `src/axon-core/src/pipeline_v2/stage_b1.rs` |
| B2 GPU embedder | `src/axon-core/src/pipeline_v2/stage_b2.rs` |
| B3 vector UPSERT | `src/axon-core/src/pipeline_v2/stage_b3.rs` |
| Orchestrator + spawn | `src/axon-core/src/pipeline_v2/orchestrator.rs` |
| Channel capacities | `src/axon-core/src/pipeline_v2/channels.rs` |
| RAM filter | `src/axon-core/src/pipeline_v2/indexed_file_cache.rs` |
| Demand-pull (s,Q) | `src/axon-core/src/pipeline_v2/demand_pull.rs` |
| 4-layer chunking | `src/axon-core/src/code_chunker.rs` |
| Embedding contract | `src/axon-core/src/embedding_contract.rs` |
| GPU wrapper | `src/axon-core/src/embedder/` (GpuB2Embedder) |
| Runtime profile | `src/axon-core/src/runtime_profile.rs` |
| Telemetry pump | `src/axon-core/src/runtime_boot.rs` (spawn_runtime_telemetry) |
| Dashboard | `src/dashboard/` (Phoenix LiveView BEAM) |
| Promote-live | `scripts/release/promote_live_safe.sh` |
| Process-compose | `process-compose.{dev,live}.yaml` |

### 1.6 Données persistées vs RAM
- **PG canonical** : `public.Symbol`, `public.Chunk` (content_tsv GENERATED), `public.ChunkEmbedding`, `public.IndexedFile` (3 col), `public.Edge` (post MIL-017), `soll.node`/`soll.edge`/`soll.revision`.
- **RAM only** : arc-swap<DashMap> filter, per-stage metrics (items_in/out/inflight/bp), mpsc channels.
- **JAMAIS revenir à** : `public.file` 23 colonnes machine-à-états (status pending/indexing/indexed, worker_id, claim_at_ms…) éliminé par REQ-AXO-289.

---

## 2. Scope final (106 REQs ouverts hors MIL-AXO-027)

**Exclus** : tous descendants de MIL-AXO-027 (Layer A/B SOLL v4 contract, MVCC, tool consolidation MCP 67→15, pilote, migration progressive Pillars/Guidelines, self-introspection slice 8).

Catégorisation **HOT / WARM / DEEP / CHILL** par actionnabilité et coût :

### 2.1 WAVE 1 — HOT P0/P1 bugs runtime + observabilité (10 REQs)

| # | REQ | Pri | Status | Titre | Skill |
|---|---|---|---|---|---|
| 1 | REQ-AXO-901798 | P0 | current | Dashboard probe provider=CPU faux alors que GPU réel (nvidia-smi preuve) | `/diagnose` |
| 2 | REQ-AXO-901831 | P1 | current | Scanner discovery gap : 9479 files perdus eligible vs enrolled | `/diagnose` + `/tdd` |
| 3 | REQ-AXO-901835 | P1 | current | Telemetry socket collision brain↔indexer (patches 1-3 done, patch 4 = #4) | none (in-flight) |
| 4 | REQ-AXO-901836 | P1 | planned | Bridge brain↔indexer runtime_truth PG-based (patch 4 de #3) | none (PG design) |
| 5 | REQ-AXO-901800 | P1 | current | Dashboard fallback SILENCIEUX cross-instance (sql_gateway.ex:16-25) | `/diagnose` |
| 6 | REQ-AXO-901795 | P1 | current | TensorRT → CPU fallback récurrent (peut être faux positif après fix #1) | `/diagnose` (re-validate) |
| 7 | REQ-AXO-901794 | P1 | current | `axon stop --role indexer` tue tout process-compose | `/tdd` |
| 8 | REQ-AXO-901796 | P2 | current | `axon start --indexer-graph`/`--indexer-vector` documentés non implémentés | `/tdd` |
| 9 | REQ-AXO-901838 | P1 | planned | qualify --reuse-runtime tue le dev runtime au shutdown | `/diagnose` + `/tdd` |
| 10 | REQ-AXO-901840 | P2 | current | sql plugin error surface pg_error.message/code/hint — commit 30df5f53 livré mais PAS dans live binary (md5 75ba1b44 = v0.8.0-757-g6b75d7f7 antérieur) | promote-live |

### 2.2 WAVE 2 — WARM P0/P1 bench + outillage (5 REQs)

| # | REQ | Pri | Status | Titre |
|---|---|---|---|---|
| 11 | REQ-AXO-259 | P0 | current | Bench 1 — graph projection harness (watcher → parse → chunk) |
| 12 | REQ-AXO-260 | P0 | current | Bench 3 — writer harness (synthetic chunks → DB persist) |
| 13 | REQ-AXO-261 | P0 | current | Bench 4 — end-to-end indexer probe summary (probe.sh + summarize) |
| 14 | REQ-AXO-257 | P0 | planned | Reconstruct throughput bench harness (lost from proto/gpu-saturation worktree) |
| 15 | REQ-AXO-901758 | P1 | current | promote_live_safe.sh — 100% fiabilité (log, état non-ambigu, résumé final) |

### 2.3 WAVE 3 — WARM P1 enhancements opérationnels (4 REQs)

| # | REQ | Pri | Status | Titre |
|---|---|---|---|---|
| 16 | REQ-AXO-293 | P1 | planned | axon start idempotent runtime bootstrap + --fast |
| 17 | REQ-AXO-901727 | P1 | planned | tech-debt tracking SOLL evolution (TechnologyMigration entity) |
| 18 | REQ-AXO-901750 | P1 | current | Strategic Relevance Signal — champ legacy_proximity MCP |
| 19 | REQ-AXO-901757 | P1 | current | SOLL searchable — embeddings + FTS sur descriptions + audit RAM |

### 2.4 WAVE 4 — DEEP refactor / architecture (10 REQs operator-gated)

| # | REQ | Pri | Status | Titre | Gate |
|---|---|---|---|---|---|
| 20 | REQ-AXO-901735 | P1 | planned | Phase 2 pivot orchestrateur process-compose + axonctl client métier | operator |
| 21 | REQ-AXO-218 | P2 | planned | Refondre taxonomie DEC/CPT stratégique vs opérationnel vs tactique | operator |
| 22 | REQ-AXO-219 | P2 | planned | Audit god-files (4 fichiers >3K LOC) vs APoSD | operator |
| 23 | REQ-AXO-256 | P2 | planned | Audit obsolete CLI verbs/scripts/code post-PG/AGE migration | operator |
| 24 | REQ-AXO-268 | P1 | current | Extend async_writer to producer hot path (REQ-AXO-193 follow-up) | auto |
| 25 | REQ-AXO-269 | P1 | current | Investigate graph projection lane bottleneck PG (Wave 5 ≤24 ch/s) | auto |
| 26 | REQ-AXO-270 | P1 | current | Refactor vector lane to 3-stages pipeline (AC5 unblocker) | auto |
| 27 | REQ-AXO-271 | P1 | current | System-wide DuckDB excision collapse dual-backend to PG-only | auto (largely done) |
| 28 | REQ-AXO-901624 | P1 | current | P4 Lazy Async TSV Build via pgmq — sort tsvector hot path A3 | auto |
| 29 | REQ-AXO-234 | P2 | planned | Automate single-GPU exclusion: pause live indexer when dev --indexer-full | auto |

### 2.5 WAVE 5 — CHILL P2/P3 maintenance + tests (40+ REQs)

Tests isolation (901718/719/720/721/91560/915), MCP enhancements (160-165, 239, 901618, 901792), SOLL admin (901732, 901749, 901613, 901593, 91493, 91496, 901665, 901634, 901680, 901682), Memgraph/autodoc (309/310/311/312/313), runtime tuning (065/066/078/225/229/240), legacy REQs P--/P1 status='current' sans priorité explicite (REQ-AXO-002...044, 161-165). Triage en SOLL : maintenir, archiver, ou re-prioriser par operator.

### 2.6 Hors scope confirmé
- **MIL-AXO-027** et tous descendants : Layer A/B SOLL v4, MVCC, tool consolidation MCP 67→15, pilote, migration v4, self-introspection slice 8.

---

## 3. Plan d'exécution wave par wave

### Wave 1 ORDRE strict (avec dépendances)

```
#1 REQ-AXO-901798 dashboard provider CPU/GPU
   ↓ (provider fiable → toutes les autres observations valides)
#5 REQ-AXO-901800 dashboard silent fallback
   ↓ (dashboard fiable)
#10 REQ-AXO-901840 promote-live commit 30df5f53 (rebuild + qualify + promote)
   ↓ (MCP sql tool surface pg_error → meilleur diagnostic suivant)
#3 REQ-AXO-901835 telemetry socket (vérifier patches 1-3 déjà mergés, fermer)
   ↓
#4 REQ-AXO-901836 bridge brain↔indexer PG runtime_truth
   ↓ (heartbeat fiable graph_workers + effective_provider)
#6 REQ-AXO-901795 TensorRT/CPU fallback (re-validate avec données fiables)
   ↓
#2 REQ-AXO-901831 scanner discovery gap (besoin données fiables)
   ↓
#7 REQ-AXO-901794 axon stop --role indexer chirurgical
   ↓
#8 REQ-AXO-901796 axon start --indexer-graph/--vector implémenter
   ↓
#9 REQ-AXO-901838 qualify --reuse-runtime no-kill
```

### Wave 2 (parallélisable après #10 promote-live)
- #11 #12 #13 : reconstruire bench harnesses
- #14 : porte d'entrée throughput bench (planifié → current)
- #15 : promote_live_safe.sh hardening

### Wave 3 (après Wave 1 + 2)
- #16 axon start idempotent + --fast
- #17 tech-debt SOLL entity
- #18 strategic relevance signal MCP
- #19 SOLL searchable embeddings/FTS

### Wave 4 (operator-gated check-in avant)
Architecture decisions et refactor. Réserver pour session dédiée. Surface findings à l'opérateur, attendre confirmation.

### Wave 5 (background continuous)
Triage SOLL, ne pas livrer en masse. Logger les findings, classer archived/in-flight/superseded selon evidence.

---

## 4. Contrat per-REQ (template TDD)

Pour chaque REQ dans Wave 1, suivre rigoureusement :

```
1. RACINE — Lire soll.node WHERE id=<REQ> description en full. NE PAS deviner cause.
2. REPRO — Empirique : commande shell, attendu, observé. Citation exacte.
3. TEST FAILING — Écrire test E2E ou intégration AVANT fix. Doit échouer sur HEAD courant.
4. FIX MINIMAL — Le strict nécessaire pour passer le test. Pas de cleanup latéral.
5. TEST PASSE — Re-run test. Doit passer green. Re-run tests adjacents pour zéro régression.
6. SOLL — soll_attach_evidence VAL-AXO-N avec commit SHA + test path + reproducibility note.
   soll_manager(action=update, status='delivered') si AC100% remplie.
7. COMMIT — Message canonique : `fix(<scope>): REQ-AXO-N — <one-line>` ou `feat(<scope>):...`.
   NO squash. Granular per slice.
8. DEV FIRST — Si touche pipeline runtime, ./scripts/axon-dev start + observer 5+ min AVANT promote-live.
9. NEXT — Reprendre #1 sur le prochain REQ.
```

Anti-patterns interdits :
- Fix sans repro empirique
- Fix qui supprime un test au lieu de le faire passer
- Commit sans soll_attach_evidence si REQ touchée
- Promote-live sans dev observation 5+ min
- Squash de slices

---

## 5. Verification gates entre waves

### Après chaque REQ
- `cargo build --manifest-path src/axon-core/Cargo.toml --release`
- `cargo test --manifest-path src/axon-core/Cargo.toml --lib` (zero warning, zero fail)
- Si REQ touche surface MCP : `axon_pre_flight_check diff_paths=[...]`

### Après chaque Wave
- `mcp__axon__soll_validate project_code=AXO` → 0 violations (sauf REQs en cours marquées partial)
- `mcp__axon__soll_work_plan project_code=AXO top=8` → wave-1 reflète réalité
- `mcp__axon__status mode=brief` → freshness fresh + trust canonical (sinon `axon-live start --indexer-graph`)

### Promote-live (au minimum après Wave 1 #10 et fin de Wave 1)
- `./scripts/axon-dev start full` + bench 5+ min sur source réel
- `bash scripts/release/promote_live_safe.sh --project AXO`
- Post-promote : `mcp__axon__status` runtime_version match manifest

### Bench validation (Wave 2)
```
export ORT_STRATEGY=system
export ORT_DYLIB_PATH=$(jq -r .core_lib .axon/ort-artifacts/onnxruntime-tensorrt-cudaPackages/current.json)
export LD_LIBRARY_PATH=/usr/lib/wsl/lib:$(dirname $ORT_DYLIB_PATH):${LD_LIBRARY_PATH:-}
export AXON_DEV_DATABASE_URL=postgres://axon@127.0.0.1:44144/axon_dev

cargo run --manifest-path src/axon-core/Cargo.toml --release \
  --bin axon-bench-pipeline-v2 -- \
  --source <PATH> --max-files N --gpu --human
```
Target : 60-100 ch/s sustained, 100+ peak (REQ-AXO-901820 baseline).

---

## 6. Sub-agent delegation policy

| Tâche | Sub-agent ? | Pourquoi |
|---|---|---|
| Code exploration / IST reconstruction | ❌ INTERDIT | Pas de MCP → 100-200K tokens wasted (GUI-PRO-027) |
| Symbol lookup `query/inspect/impact/why/path` | ❌ INTERDIT | Main thread MCP direct |
| Cargo build / test executor | ✅ OK | Shell exec, MCP-independent |
| Doc writing (working-notes, SOLL update via soll_manager) | ✅ OK | Pas de source reading |
| Bench harness execution | ✅ OK | Shell + jq + cargo bin |
| Architectural drift parallel audit | ✅ OK (via Agent Explore) | Doc-only |
| Dashboard Elixir tests | ✅ OK | mix test isolé |

Toujours :
- Sub-agent reçoit `project="AXO"` explicite (auto-detect via cwd casse en sub-agent).
- Sub-agent invoque MCP via `ToolSearch` deferred-load si besoin (`mcp__axon__sql`, `mcp__axon__status` etc.).
- Shell-bridge : `./scripts/axon mcp-call call <tool> --args '{...}'` pour >10 appels MCP en boucle.

---

## 7. Blockers + rollback contract

### Blockers immédiats
- **Live SQL plugin error nu** : commit 30df5f53 fix pas en live → bloque diagnostic des autres REQs. → WAVE 1 #10 EN PRIORITÉ.
- **IST freshness stale + trust degraded** : `axon-live start --indexer-graph` doit lever, sinon brain serve frozen snapshot.
- **Session_pointer drift** : CPT-AXO-052 à session 63, HEAD = session 65/66. Update au hand-off.

### Rollback chemin canonique
- Code : `git revert <SHA>` (NEVER force push, NEVER amend published)
- Live binary : `bash scripts/release/rollback_live.sh` → restaure previous manifest
- SOLL mutation : `soll_rollback_revision` (NEVER manual delete)
- Pipeline config : feature flag `AXON_REPLENISHMENT_MODE={legacy|sq}` pour (s,Q) replenishment

### Hard stops (interruption opérateur requise)
- Destructive irreversible (mass-delete SOLL, DROP TABLE)
- Architectural decision needing human (Wave 4 operator-gated)
- Hard external blocker (CI broken upstream, MCP unrecoverable)

---

## 8. Hand Off (post-livraison)

Suivre GUI-PRO-028 strict — 5 steps mandatory :
1. Update SOLL session_pointer CPT-AXO-052 (runtime state + branch + HEAD + REQs in-flight + 3 next actions + blockers).
2. SOLL cleanup + topological replan (`soll_validate` 0 violations, `soll_verify_requirements`, attach VAL evidence).
3. Boot-loaded docs prune (MEMORY.md, CLAUDE.md global/project, SKILL.md axon-engineering-protocol) — ZERO obsolete, tables over prose.
4. axon-engineering-protocol SKILL.md consolidation (LLM-contract only, pas de narrative).
5. Working-notes audit (sessions précédentes archivées, ce prompt référencé).

---

## 9. Réutilisation

Ce document est self-contained — copy-paste dans une fresh LLM session après un `/clear`. Le LLM exécute en suivant la cartographie (§1) + waves (§3) + contrat per-REQ (§4) + gates (§5).

**Originator** : Session 66 (2026-06-01), opérateur Didier sur trigger « termine ce projet hormis MIL-027 ». Cartographie produite via `psql` direct sur `axon_live` (workaround REQ-AXO-901840 fix pas en live).
