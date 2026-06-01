# Session 66 Hand Off — 2026-06-01

Compagnon de `2026-06-01-session-66-pipeline-execution-prompt.md` (prompt SOTA initial). Cette note documente le **pivot d'architecture mi-session** vers DEC-AXO-901626 (observable-derived runtime state).

## Récit en une page

Session démarrée sur le mandat opérateur « termine le projet Axon hors MIL-027, fais-le SOTA ». Phase A cartographie : 106 REQs ouverts hors MIL-027 = scope vraiment couvrable Wave 1 (P0/P1 bugs runtime + observabilité). Phase B prompt SOTA écrit. Phase C exécution Wave 1 :

1. **Slice livré (commit `074cfc2e`)** : bridge brain↔indexer via heartbeat JSON file. Indexer publie `embedder_provider` + `lane_parameters` dans `.axon-dev/run-indexer/runtime-heartbeat.json` ; brain lit + surface dans `indexer_runtime` + `resource_policy` + `vector_pipeline_telemetry.provider`. Couvre REQ-AXO-901798 + REQ-AXO-901836 (patches 1-4). E2E test pass + dev validation curl confirme. +88/-162 net (cleanup DuckDB-era helpers compris).

2. **Pivot architectural** (opérateur Didier) : « pour besoin d'une table ? et pas une proc ? » → « tu n'a aucun moyen de déterminer avec précision cpu ou gpu ? » → « cuda ou tensorrt = gpu, sinon cpu » → « go full SOLL ». La direction proposée et validée :
   - **Provider effectif n'est pas stocké** ; il est dérivé observationnellement
   - **nvidia-smi --query-compute-apps=pid** = vérité OS absolue pour GPU/CPU binaire
   - **SELECT count(*) FROM public.ChunkEmbedding WHERE inserted_at > now()-60s** = vraie vitesse PG-canonique
   - **Slot RAM `EMBEDDING_PROVIDER_DIAGNOSTICS`** = à supprimer (race condition prouvée)
   - **Filesystem heartbeat JSON** (mon ajout `embedder_provider`+`lane_parameters` de 074cfc2e) = à supprimer aussi
   - Documenté en `DEC-AXO-901626` (insérée via psql, 4 edges : REFINES PIL-001+002, SOLVES REQ-901798+901836).

3. **Tentative refactor RAM per-lane abandonnée** : j'avais commencé un refactor `HashMap<lane, EmbeddingProviderDiagnostics>` mais l'opérateur a montré que c'était résoudre un faux problème (stocker une donnée dérivable). `git restore` a annulé ce refactor. Le commit `074cfc2e` reste sur la branche comme étape intermédiaire ; DEC-AXO-901626 le supersede.

## État runtime @ hand-off

| Surface | État | Note |
|---|---|---|
| Branch `feature/pipeline-sq-reorder-point` HEAD `074cfc2e` | propre côté code | uncommitted seulement `db/ddl/02_axon_runtime.sql` (planifié) et CLAUDE.md / SOLL non-modifiés |
| Live brain port 44129 | UP build `v0.8.0-757-g6b75d7f7` (pré-bridge) | restart laborieux ; MCP réopérationnel |
| Dev brain port 44139 + indexer | UP build `v0.8.0-790-g074cfc2e` | inclut le bridge file-based ; heartbeat contient les nouveaux blocs |
| PG :44144 | UP | a crashé+redémarré mi-session ; template `axon_test_template` présent ; ~20 stale `axon_test_*` DBs à nettoyer |

## Backlog DEC-AXO-901626 (à implémenter prochaine session)

5 tâches granular (#13-16 dans le tracker) :

1. **DDL** : `ALTER axon_runtime.EmbedderLifecycleHeartbeat ADD COLUMN pid INTEGER NOT NULL DEFAULT 0, ADD COLUMN build_id TEXT` (idempotent dans 02_axon_runtime.sql) ; nouveau `db/ddl/09_embedder_observed.sql` avec `axon_runtime.embedder_observed_state() RETURNS jsonb`.
2. **Code Rust** : `src/axon-core/src/observed_gpu.rs` avec `observed_gpu_used_mib(pid: u32) -> Option<u64>` ; indexer UPSERT heartbeat lifecycle inclut pid + build_id.
3. **Brain composer** : nouveau bloc `embedder_runtime` dans `tools_framework_runtime_status.rs` composé de PG `embedder_observed_state()` + nvidia-smi sur le pid lu depuis `EmbedderLifecycleHeartbeat`.
4. **Suppression** : `EMBEDDING_PROVIDER_DIAGNOSTICS` slot + `set_embedding_provider_runtime_state` (6 callers : gpu_backend.rs:213 + embedder.rs:779/794/803/809/838) + `register_embedding_provider_diagnostics` + filesystem heartbeat `embedder_provider`+`lane_parameters` blocs (mes ajouts dans `main_telemetry.rs`) + brain composer `peer_embedder_provider` extraction (mes ajouts dans `tools_framework_runtime_status.rs` + `runtime_topology_support.rs` + `tools_framework_runtime_topology.rs`). Net : ~-150 LOC.
5. **Tests + dev validation + commit + SOLL VAL** : unit observed_gpu (mock nvidia-smi stdout) + integration brain composer + dev restart + curl status verifying `embedder_runtime.compute='GPU'` + indexer pid alive + commit granular avec `feat(mcp): DEC-AXO-901626 — observable-derived runtime state` + soll_attach_evidence VAL nodes pour DEC-901626 + UPDATE des REQs 901795 (« CPU fallback récurrent » devient irrelevant — observation directe).

## REQ status flips this session (via psql)

| REQ | Avant | Après | Edges DEC-901626 |
|---|---|---|---|
| REQ-AXO-901798 | `current` | `delivered` | SOLVES |
| REQ-AXO-901836 | `planned` → `current` (implicite, via psql) → `delivered` | `delivered` | SOLVES |

(Note : `delivered` reflète la livraison file-based en 074cfc2e. Le successeur observationnel DEC-901626 reste à implémenter.)

## Feedback memory ajoutée

- `feedback_sota_framing_not_minimal.md` — ne jamais cadrer une livraison comme « minimal/patch/quick win » ; SOTA framing exigé.

## 3 next-session actions prioritaires

1. **Fix live brain — promote 074cfc2e vers live** (`bash scripts/release/promote_live_safe.sh --project AXO`) — sinon les LLMs futurs lisent un brain pré-bridge qui ne sait rien des nouvelles colonnes heartbeat.
2. **Implémenter DEC-AXO-901626** end-to-end (5 tasks ci-dessus, ~4-6 h).
3. **Triage Wave 2** : bench harnesses REQ-AXO-259/260/261/257 + promote_live_safe.sh fiabilité REQ-AXO-901758.

## Blockers / operator-gated stops

- Live brain restart fragile (cette session : exit code 0 sans démarrer process-compose ; deuxième tentative a fonctionné). Investigation script `start.sh` requise.
- Implémentation DEC-901626 demande compréhension fastembed + SemanticWorkerPool spawn ordering pour suppression propre.

## Verification probes next LLM

```
git log --oneline -5
md5sum bin/axon-brain .axon/cargo-target/release/axon-brain
pgrep -af "axon-(brain|indexer)"
psql -h 127.0.0.1 -p 44144 -U axon -d axon_live -c \
  "SELECT id, status FROM soll.node WHERE id IN ('REQ-AXO-901798', 'REQ-AXO-901836', 'DEC-AXO-901626')"
```
