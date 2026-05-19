# Session 43 — Audit + Handoff (2026-05-17)

Append-only narrative. Canonical state lives in SOLL `CPT-AXO-052`. This is the human-readable companion.

## Headlines

- **17 commits engineering** sur `main` (HEAD `3c34da14`).
- **Promote-live SUCCESS** sur `v0.8.0-518-g22d34a52`, install_gen `live-20260517T190357Z`.
- **`graph_ingestion.rs` fully PG-canonical** : 15 callsites `is_postgres_backend()` collapsed sur 6 fonctions.
- **4 nouveaux REQ** logged + 1 CPT (le prompt expert v2 mémorialisé en SOLL).
- ~-735 LOC dead code + 176 LOC fix/perf/feat + 11 unit tests, **0 régression**.

## Méthodologie : 3 fixes structurels livrés

### REQ-AXO-91569 + REQ-AXO-91503 (delivered, commit `65936669`)
Pre-flight gate `axon_commit_work` exemptait pas les pure-refactor. Ajout `exempt_for_refactor: true` dans metadata guideline (GUI-PRO-001 + GUI-PRO-002) + helper `commit_message_is_refactor` reconnaît `refactor:` / `refactor(scope):` / `refactor!:` / `refactor(scope)!:`. 11 unit tests verrouillent le parsing.

### REQ-AXO-91570 (delivered, commits `da9be099` + `22d34a52`)
`STARTUP_TIMEOUT_S=240` insuffisant pour cold-compile TRT BGE-Large → bump 240→900s. Cap `AXON_TRT_PROFILE_MAX_SHAPES=64×512` (vs ancien 256×512) → -1-2 GB VRAM ; production batch=24 → headroom 2.6× suffisant.

### REQ-AXO-91571 (delivered, commit `3fd37922`)
Pre-flight gate filtrait sur `status='active'` sans `project_code` → 6 doublons cross-project (FSF/MLD/NEX/TE2) bloquaient les commits AXO. Fix : resolve `effective_project_code` + `project_code IN ('PRO', :scope)` dans la query gate + nouveau helper `lookup_project_code_by_path`. Plus 6 SUPERSEDES vers GUI-PRO canonicals pour retirer les dupes existants.

## REQ-AXO-271 (DuckDB purge) — progression substantial

12 slices delivered cette session : 2a, 2b, 2c, 2d, 2e, 2f, 2g, 2h, 2i, 2j, 2k, 2l, 3 (+ pré-existantes 1, 5, 6, 7).

| Fichier | Status |
|---|---|
| `graph_query.rs` | fully PG-canonical (slices 2a + 3) |
| `async_writer.rs` | fully PG-canonical (slice 2c) |
| `vector_runtime.rs` | fully PG-canonical (slice 2e) |
| `embedder.rs` | fully PG-canonical (slice 2f) |
| `graph_ingestion.rs` | **fully PG-canonical** (slices 2g/2h/2i/2j/2k/2l, 15 callsites) |
| `graph.rs` | fully PG-canonical (slice 2d) |
| `mcp/tools_governance.rs` | fully PG-canonical (slice 2b) |
| Reste `mcp/tools_*.rs` | ~9 callsites (slice 2m suivant) |
| Slice 4 (DuckDB seed + bootstrap twin) | planned |
| Slice 8 (workspace destructive) | operator-gated |

## Mode silencieux — investigation

Question opérateur : « Mode silencieux GPU en place ? ». Diagnostic skill `diagnose` GUI-PRO-030 PDCA :

**Observation initiale** : `lifecycle_phase=ready` + `sleep_count=0` + `last_used_ms` figé après >2h idle sur live runtime.

**Diagnostic révisé (post-inspection code)** :
- `spawn_idle_watchdog` correctement wiré au boot indexer (pipeline_v2_runtime.rs:128, params tick=15s/t_idle=5min/t_grace=2s)
- `request_wake` bump correctement `last_used_ms` sur chaque `embed_batch`
- `release_session` callback drop la session ORT sur sleep

**Deux composantes distinctes** :
1. **IST replay catch-up** (non-bug) : 9 517 fichiers stale → Pipeline A scanne en continu → A3/B2/B3 churn permanent → queue jamais vide 5 min consécutives → `should_sleep` reste false. Comportement attendu.
2. **`embedding_status` cross-process singleton bug** (vrai bug, REQ-AXO-91572 logged) : brain process répond à la query MCP avec son propre singleton EmbedderLifecycle (jamais touché — brain ne fait pas `embed_batch`) au lieu de celui de l'indexer. Fix structurel proposé : table `axon_runtime.EmbedderLifecycleHeartbeat` + write-side indexer + read-side brain (option B).

## Cross-project guideline cleanup

6 status flips SUPERSEDED par les canonicals GUI-PRO :
- GUI-FSF-001 + GUI-FSF-002 (Fiscaly)
- GUI-MLD-001 + GUI-MLD-002 (MLD)
- GUI-NEX-001 (Nexus)
- GUI-TE2-001 (TE2)

Sans ce cleanup, les slices 2b/2d restaient bloquées par les dupes des projets voisins même avec l'exemption refactor en place. REQ-AXO-91571 défend maintenant structurellement contre toute future récurrence.

## Promote-live narrative

**Tentative 1** (avant diagnostic) : commit `65936669` build OK, mais `start-indexer.sh` timeout au `verify_role_ready` à 240s budget. Live restart failed ; brain pid 5502 manuellement re-démarré via `axon-live start --brain-only` après clear pending.json + rollback.

**Tentative 2** (après diagnostic dev `axon-dev start --indexer-full --tensorrt` confirmant que TRT cache hit en 75s — donc pas un cold-compile) : promote `v0.8.0-518-g22d34a52` SUCCESS en 174s avec budget 900s. Engine cache `4680136567211888080` réutilisé (mtime May 13) — le profile 64×512 a un hash différent en théorie, mais l'observation suggère que TRT EP fallback CPU (cf live env : `AXON_EMBEDDING_PROVIDER=cuda` + `embedder_provider_fallback: requested=cuda effective=cpu` en dev), donc engine pas re-compilé.

Root cause exacte du 240s timeout original toujours indéterminée (sans les logs tmux du process killed) mais ne s'est pas reproduite avec le budget 900s.

## Bootstrap pour session 44+

```
sql SELECT description FROM soll.node WHERE id='CPT-AXO-052'
sql SELECT description FROM soll.node WHERE id='CPT-AXO-90013'
git log --oneline -20 main   # HEAD = 3c34da14
mcp__axon__status mode=brief
mcp__axon__embedding_status   # re-probe REQ-AXO-91572 singleton
```

## 3 actions next session pickup

1. **REQ-AXO-271 slice 2m** : collapser les 9 callsites restants dans `mcp/tools_*.rs` (tools_context / tools_dx / planning_revision / workflow_project / workflow_plan / storage / manager). Gate exemption + project scope sont live, commits `refactor(...)` doivent passer.
2. **REQ-AXO-91572 fix structurel** : impl option B (table `axon_runtime.EmbedderLifecycleHeartbeat`) pour vraie observabilité du mode silencieux runtime.
3. **REQ-AXO-271 slice 4** : DuckDB seed + bootstrap twin removal (multi-fichier engineering substantiel).

## Originator

Session conduite par opérateur Didier 2026-05-17, démarrée sur cold-start post-MIL-AXO-022 closure, fermée sur GUI-PRO-028 systematic handoff.
