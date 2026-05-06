# Handoff — SOLL writes pending (MCP brain unavailable)

**Author:** Claude Opus 4.7 [1m]
**Date:** 2026-05-06 ~21:35 local (~19:35 UTC)
**Branch HEAD:** `2362097` (E.1+E.2+E.3a+E.4+E.5 + script env preserve list)
**Lib tests:** 955/0/2

This file is a **transient placeholder** per CPT-AXO-019 fallback (markdown allowed only when MCP is unrecoverable). Once `bin/axon-brain` recovers, transfer everything below into SOLL via `soll_manager` / `soll_apply_plan`, then **delete this file** (or commit `docs: drop markdown handoff — content moved to SOLL` per the lesson learned in REQ-AXO-196).

## Why MCP is down — REAL ROOT CAUSE (updated 2026-05-06 ~23:35)

`bin/axon-brain` crashes on startup with DuckDB FATAL exception:
```
{"exception_type":"FATAL","exception_message":"INTERNAL Error: Failed to append to PRIMARY_McpJob_0:
Constraint Error: PRIMARY KEY or UNIQUE constraint violation: duplicate key \"JOB-1778012869260\""}
```

**Initial hypothesis (incorrect)**: race condition in `mcp.rs:1297` (`JOB-{ms_timestamp}` collision). That bug DOES exist (separate REQ logged below) but it's NOT the cause of this specific crash.

**Actual root cause (confirmed via web search + reproduction with DuckDB CLI)**:
1. **graph_bootstrap.rs:1277** runs at every brain boot:
   ```rust
   self.execute("UPDATE soll.McpJob SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;
   ```
2. soll.db has 4 McpJob rows with `project_code = NULL` (pre-migration leftovers from soll_apply_plan jobs).
3. UPDATE on a primary-keyed row in DuckDB internally does DELETE + INSERT on the same PK.
4. This triggers **DuckDB upstream issue [#15836](https://github.com/duckdb/duckdb/issues/15836)** "Unable to open a database if wal file has same primary key delete + insert" — still "under review" as of January 2025, NOT fixed in v1.5.1 (brain) NOR v1.5.2 (CLI).
5. WORSE: the index is now in a state where even `DELETE FROM McpJob WHERE project_code IS NULL` fails with `FATAL Error: Invalid Input Error: Failed to delete all rows from index. Only deleted 0 out of 4 rows.` — deeper corruption than just #15836.

**Reproduction (confirmed in this session)**:
- DuckDB CLI v1.5.2 (newer than brain's v1.5.1 binding) running the same UPDATE statement crashes IDENTICALLY.
- Pure DELETE on those 4 rows also fails with the index corruption error.
- INSERT/CHECKPOINT operations on other tables succeed normally.

**4 problematic rows captured for audit** before any mutation attempt: `/tmp/mcpjob-deleted-rows-2026-05-06.json` (37 KB). They're succeeded `soll_apply_plan` jobs (admin history, no active work).

`bin/axon-indexer` (live, indexing /home/dstadel/projects) was HEALTHY at restart this session. Only the brain is crashed → MCP is down. `soll.db` byte-identical to its 20:27 backup (md5 `749765eb...`); WAL identical to backup (md5 `df9bb6b2...`); all session SOLL writes (REQ-AXO-193 update, REQ-AXO-196 creation) safe on disk.

## Pending SOLL writes (in priority order)

### 1. VAL-AXO-041 (validation, NEW, REFINES VAL-AXO-040, VERIFIES REQ-AXO-193)

```
title: Direction E (E.1+E.2+E.3a+E.4 consolidated) measurement on clean machine
status: failed (no significant improvement)
priority: P0
tags: [performance, vector-pipeline, dec-axo-074-followup, parquet, async-writer, direction-e, gate-fail-recovery, partial-analysis]

description:
## Setup

E.1+E.2+E.3a+E.4 (commits 2b0fd6a, 21f6cba, 12147d7) shipped on main, all env-gated default OFF. Lib tests preserved at 955/0/2 (was 943, +12 new async_writer unit tests).

3 fresh val41-clean-E probes + 1 baseline, **live indexer stopped** (clean GPU), env config:
- run1/2/3: AXON_PARQUET_EMBEDDING_STORE_ENABLED=true AXON_PARQUET_CHUNK_CONTENT_ENABLED=true AXON_ASYNC_WRITER_ENABLED=true (--tensorrt --indexer-full)
- baseline: all envs OFF (parquet embedding still default true per probe script)

## Result

| Probe | mean ch/s | steady-state ch/s (last 5) |
|---|---|---|
| baseline-1 (all OFF) | 25.16 | 32.82 |
| val41-clean-E-run1 | 25.37 | 34.00 |
| val41-clean-E-run2 | 22.99 | 32.54 |
| val41-clean-E-run3 | 24.52 | 34.98 |
| **E mean (n=3)** | **24.29 ± 1.0 (σ=4%)** | **33.84 ± 1.3** |

Δ vs baseline = -0.87 ch/s (-3.5%, within noise σ).

**Acceptance not met**: target ≥150 ch/s, measured 24 ch/s. Falls 6× short. Direction E (E.3a alone) is NEUTRAL — no regression, no improvement.

## Root cause confirmed by writer-actor.trace tail (run3, last 90s)

| Time elapsed | commit_ms |
|---|---|
| t+0s   |  1093 |
| t+30s  |  2443–2972 |
| t+60s  |  4254–5379 |
| t+90s  |  7225–9498 |

**Geometric growth pattern unchanged from VAL-AXO-038 / VAL-AXO-039**. The Writer Actor still serializes graph_projection's sync execute_batch with the new async writer's flushes — both grab the same DuckDB writer mutex. E.3a only routed `mark_file_vectorization_work_done` (vector-lane regression source, VAL-AXO-040 −56%); the producer hot path (`insert_file_data_batch_with_vectorization_policy`) still does `self.execute_batch(&queries)` synchronously due to inline-embed read-after-write coupling.

Memory First is **partial**: vector-lane mark_done is async, graph_projection is still sync.

## Implication for direction E

E.3a is necessary (covers the −56% regression introduced by REQ-AXO-194 Bug 2 fix) but not sufficient for ≥150 ch/s. The remaining headroom requires E.7 (typed-row producer refactor, ~250 LOC) which moves graph_projection's writes through the same async dispatcher. Until E.7 lands, the operator's "doubler le throughput" target stays unmet.

Alternatively, accept ~25 ch/s as the new operational baseline post-Bug-2-correctness-fix and revisit DEC-AXO-074 acceptance criteria.

## Tags

performance, vector-pipeline, async-writer, direction-e, gate-failed-on-acceptance, e7-required, partial-analysis

LINKS_TO_CREATE:
- VAL-AXO-041 REFINES VAL-AXO-040
- VAL-AXO-041 VERIFIES REQ-AXO-193 (gate-failed)
```

### 2. REQ-AXO-XXX-A (P0, brain/indexer lifecycle independence — operator request, **mentioned 10x**)

Already drafted in MCP attempt earlier (failed: backend down). See full body in this session's transcript or below.

```
title: axon-brain et axon-indexer DOIVENT être complètement indépendants (start/stop isolé par rôle)
priority: P0
tags: [operator-frustration, repeated-ask, lifecycle, isolation, axonctl, control-plane, deliverability, robustness, dec-axo-060-followup]

acceptance_criteria: |
  Pour live ET dev:
  - `./scripts/axon-{live,dev} stop --role indexer` arrête UNIQUEMENT axon-indexer (brain reste up + MCP répond).
  - `./scripts/axon-{live,dev} stop --role brain` arrête UNIQUEMENT axon-brain (indexer continue d'indexer).
  - `./scripts/axon-{live,dev} start --role indexer` démarre uniquement l'indexer SANS toucher au brain.
  - Inverse: `--role brain` ou `--brain-only` (déjà existant).
  - Aucune coordonnée partagée bloquante: si l'un meurt, l'autre continue.
  - `--hard` continue de fonctionner mais comportement EXPLICITE et non par défaut.
  - Test d'acceptation: (1) start brain seul, indexer reste DOWN; (2) start indexer alors que brain UP, brain reste up; (3) stop indexer, brain HEALTHY; (4) stop brain, indexer continue d'indexer.

description: |
  Operator: 'Repeated 10x' (verbatim 2026-05-06). Bug observé même session:
  `./scripts/axon-live stop --hard` arrête brain ET indexer ensemble.
  `./scripts/axon-live start --indexer-full` ne démarre QUE l'indexer (brain ne suit pas).
  Pour bench, on doit donc stopper TOUT pour libérer le GPU mono-shared, puis re-orchestrer brain seul, puis indexer seul. Friction systémique à chaque cycle bench.

  axonctl supervise --role <role> existe déjà (ps -af 2992) — la couche scripts ne l'expose juste pas en granularité role.

  Customer-value: independence brain/indexer permet rolling updates, debug isolé, redémarrer un indexer crashé sans interrompre les MCP clients.

LINKS:
- BELONGS_TO PIL-AXO-001 (Shared Runtime Truth, role-granular)
- BELONGS_TO PIL-AXO-004 (Dual-Instance Operational Discipline)
```

### 3. REQ-AXO-XXX-B (P0, brain crash race-condition — bug critical)

```
title: Brain crash on boot: McpJob job_id race condition (ms timestamp collision)
priority: P0
tags: [bug, brain, mcp, duckdb, race-condition, deliverability, robustness, axon-bug, llm-contract]

acceptance_criteria: |
  - job_id generation guarantees uniqueness even when called multiple times within the same millisecond.
  - Brain restart from a state with N pending McpJob rows succeeds (no duplicate-key crash on boot replay).
  - Test: spawn 1000 mcp.submit_async_job calls in tight loop; assert all ids distinct, no DB write fails.

description: |
  Cause racine: src/axon-core/src/mcp.rs:1297
  ```rust
  let submitted_at = Self::now_unix_ms();
  let job_id = format!("JOB-{submitted_at}");
  ```
  Deux mcp.submit dans la même ms → même job_id → DuckDB PRIMARY_McpJob_0 violation → terminate called → brain crash → exit 134.

  Reproduit deterministically aujourd'hui 2026-05-06 18:30-21:30 local: brain crashes immédiatement à chaque boot replay du WAL (35KB) qui contient les inserts McpJob fautifs des tentatives précédentes.

  Trace exacte capturée dans transcript de session.

  Fix candidat (~3 LOC): job_id = format!("JOB-{ms}-{counter:08}") avec AtomicU64 counter incrementé à chaque appel. Ou UUID v4. Ou (ms*1000+atomic_seq).

  Recovery actuel pour MCP downtime: déplacer .axon/graph_v2/soll.db.wal hors du chemin (réversible) puis restart brain. Risque: perte des transactions WAL post-flush (vraisemblablement uniquement McpJob admin records, pas de SOLL data utilisateur car soll.db a été flushé à 20:27 avant le burst de crashes).

LINKS:
- BELONGS_TO PIL-AXO-001 (Shared Runtime Truth)
- IMPACTS REQ-AXO-XXX-A (brain lifecycle independence)
```

### 4. REQ-AXO-XXX-C (P1, axon_init_project writes canonical project record to runtime DB — supersedes Fiscaly P1.1, P3.8, P2.4)

```
title: axon_init_project doit enregistrer le projet en DB runtime (registry canonique, pas marker file)
priority: P1
tags: [llm-contract, axon-product-improvement, commercial-value, adr-2026-04-18-followup, project-registry, deliverability]

acceptance_criteria: |
  - axon_init_project(project_path) écrit {code, path, name, registered_at_ms} dans le runtime project registry (DB).
  - Aucun marker file local (.axon/meta.json) n'est requis pour que l'indexer reconnaisse le projet.
  - L'indexer auto-discover via watch root + lookup runtime registry.
  - .axon/meta.json reste possible comme HINT optionnel pour outils externes (CI, scripts shell), pas comme source of truth.
  - Test: après init d'un projet vierge SANS .axon/meta.json, `query` du symbol résout depuis l'indexer en <2 minutes.

description: |
  Origine: rapport Fiscaly 2026-05-06 (P1.1, P3.8, P2.4) signale que le client perd 30 min à diagnostiquer pourquoi son projet n'est pas indexé après init. Vraie cause = .axon/meta.json local désaligné.

  Position Axon (operator validated): le rapport Fiscaly demande la mauvaise correction. Au lieu de FORCER axon_init à écrire le marker file (ce qui recrée la dualité de vérité que ADR-2026-04-18 a explicitement abolie), Axon doit FINIR la migration ADR: rendre le marker file non-requis. Le runtime DB est l'autorité unique.

  Ce qui change côté code:
  - axon_init_project: ajouter une INSERT dans la table project registry (créer la table si pas existante, schema {code TEXT PK, path TEXT UNIQUE, name TEXT, registered_at_ms BIGINT}).
  - Indexer scan loop: lookup project_code par path absolu via la table registry, pas via .axon/meta.json.
  - diagnose_indexing: nouvelle sous-cause path_not_in_runtime_registry (covers the case Fiscaly hit).

  Ce qui rend P3.8 inutile: si meta.json n'est plus canonique, son `slug` legacy n'a plus d'importance.
  Ce qui rend P2.4 inutile: pas besoin de import_meta MCP, l'indexer auto-découvre via registry.

LINKS:
- BELONGS_TO PIL-AXO-002 (Agent-Native MCP Product Surface)
- IMPACTS_REQUIREMENT (TODO: link to ADR-2026-04-18 entity if it exists in SOLL, else create)
```

### 5. REQ-AXO-XXX-D (P2, .axonignore LLM-managed scope filter)

```
title: .axonignore — filtre de scope géré par le LLM, spécialise .gitignore
priority: P2
tags: [llm-contract, axon-product-improvement, commercial-value, scope, indexing, feature-request]

acceptance_criteria: |
  - Le LLM peut LIRE/ÉCRIRE .axonignore via outil dédié (mcp__axon__axonignore_read, mcp__axon__axonignore_update).
  - Format hérite de .gitignore (mêmes patterns) avec deux extensions:
    - lignes commençant par `+` ré-incluent un fichier ignoré par .gitignore (e.g. `+docs/*.md` pour réintégrer la doc).
    - lignes normales = exclusion supplémentaire (au-dessus de .gitignore).
  - L'indexer respecte cumul: .gitignore exclut → .axonignore peut ré-inclure (`+`) ou exclure davantage (–).
  - LLM modifie le fichier en commit (pas de runtime hot-edit), revue humaine via PR si politique projet l'exige.
  - Test: projet avec .gitignore excluant *.md et .axonignore avec `+docs/*.md` → docs sont indexés, autres .md ne le sont pas.

description: |
  Origine: discussion operator 2026-05-06 ~19:00 local. Décide une séparation claire des concerns:
  - LLM = décide QUOI indexer (scope/filter via .axonignore, parce qu'il comprend le code et ce qui ajoute de la valeur).
  - Operator = décide QUAND/COMMENT indexer (lifecycle, ressources, schedule).

  Pas de chevauchement: pas de mcp__axon__trigger_indexing (rejeté explicitement, indexer reste opérateur-controlled background process).

  Use cases:
  - Réintégrer docs textuels exclus par .gitignore (doc/ARCHITECTURE.md, ADRs, cahiers des charges externes).
  - Exclure du bruit que .gitignore ne couvre pas (fixtures binaires gros, logs de bench, .csv probes de session).

  Implementation hint: extension du filtering layer dans file_ingress.rs / watcher; la logique gitignore existe déjà côté Axon, .axonignore = layer additionnel post-gitignore.

LINKS:
- BELONGS_TO PIL-AXO-002 (Agent-Native MCP Product Surface)
```

### 6. REQ-AXO-XXX-E (P2, diagnose_indexing actionable diagnostic with ADR-aligned vocabulary)

```
title: diagnose_indexing doit retourner sous-causes actionnables (ADR-2026-04-18 vocabulary)
priority: P2
tags: [llm-contract, mcp, diagnostics, axon-product-improvement, adr-2026-04-18-followup]

acceptance_criteria: |
  - Pour chaque sous-cause, message machine-actionable + remédiation 1-line:
    - path_not_in_runtime_registry → "run axon_init_project(project_path=<path>)"
    - runtime_mode_excludes_indexing → "current mode brain_only, switch to indexer_full"
    - watch_root_unconfigured → "set AXON_WATCH_DIR or watch_root in .axon/config.json"
    - axonignore_excludes_path → "edit .axonignore to re-include via +pattern"
    - file_too_large_for_budget → "increase AXON_QUEUE_MEMORY_BUDGET_BYTES or split file"
  - Plus jamais le générique scope_mismatch_or_wrong_project_code (Fiscaly P1.3 friction).

description: |
  Origine: rapport Fiscaly P1.3. Client perd 30 min à diagnostiquer un message générique. Avec REQ-AXO-XXX-C livrée (registry canonique), le bon vocabulaire devient ADR-aligned (path_not_in_runtime_registry au lieu de meta_json_missing).

LINKS:
- BELONGS_TO PIL-AXO-002 (Agent-Native MCP Product Surface)
- BUILDS_ON REQ-AXO-XXX-C (registry canonique pour le bon vocabulaire)
```

### 7. REQ-AXO-XXX-F (P3, vérifier soll_relation_schema sur HEAD)

```
title: Vérifier soll_relation_schema retourne data structurée (Fiscaly P2.6, post-REQ-AXO-189)
priority: P3
tags: [verification, mcp-contract, axon-product-improvement, llm-friction]

acceptance_criteria: |
  - soll_relation_schema(source_type=MIL, target_type=REQ) retourne data avec valid_relations, examples, constraints (pas juste un message).
  - Si déjà fixé en code mais pas promu live, déclencher promote_live_safe.
  - Si pas fixé, créer REQ pour le fix.

description: |
  Fiscaly P2.6 signale que soll_relation_schema retourne uniquement le narratif "Canonical SOLL relation policy resolved with explicit directional guidance.". MEMORY indique que did_you_mean a été ajouté (REQ-AXO-189 partial relief). Vérifier:
  1. Lire le code actuel de soll_relation_schema.
  2. Comparer avec ce que le client a observé (peut-être version live promue antérieure).
  3. Si delta, promouvoir; sinon créer REQ supplémentaire.
```

## Other Fiscaly findings (validated, not yet logged)

These have my agreement but lower priority — log when MCP recovers if not redundant with above:
- Fiscaly P1.2 axon_commit_work cwd bug (real, no debate, fix `Command::new("git").current_dir(project_path)`)
- Fiscaly P2.5 mcp__axon__batch returns [] silently (PIL-AXO-002 violation)
- Fiscaly P2.7 soll_attach_evidence schema auto-doc (CPT-AXO-018 hygiene)
- Fiscaly P3.10 expose job_status MCP tool (already exists server-side)
- Fiscaly P3.11 runtime mode × tools matrix (REQ-AXO-087/088 family)

## Recovery procedure (next session) — UPDATED after attempted recovery

The simple "move WAL aside" approach DOES NOT WORK. Tried and failed during this session. The McpJob index is genuinely corrupted: even a pure `DELETE FROM McpJob WHERE project_code IS NULL` returns `Failed to delete all rows from index. Only deleted 0 out of 4 rows.`

Two viable code-path recovery options:

### Option A — Patch graph_bootstrap.rs to skip the UPDATE when no rows match (~5 LOC)

Replace `src/axon-core/src/graph_bootstrap.rs:1277`:
```rust
self.execute("UPDATE soll.McpJob SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;
```
with a guarded form:
```rust
let needs_backfill: i64 = self
    .query_count("SELECT count(*) FROM soll.McpJob WHERE project_code IS NULL OR project_code = ''")?;
if needs_backfill > 0 {
    // TODO: avoid UPDATE here — it triggers DuckDB issue #15836 with the legacy NULL rows.
    // Use `INSERT INTO new_table SELECT ... ; DROP old; ALTER RENAME` instead, or
    // bump the bundled DuckDB to a version with #15836 patched.
    tracing::warn!(
        "soll_mcpjob_backfill_skipped count={} reason=duckdb_15836_workaround",
        needs_backfill
    );
}
```

This is a TEMPORARY guard. The 4 rows stay with `project_code = NULL` until proper fix. Brain boots clean. Acceptable since no MCP feature reads `McpJob.project_code` for routing on those legacy rows.

### Option B — Rebuild McpJob via CTAS into a healthy table (avoids UPDATE entirely)

Write a small one-shot Rust binary linking `axon-plugin-duckdb` (matches brain's DuckDB version) that:
1. `CREATE TABLE McpJob_new AS SELECT job_id, tool_name, COALESCE(NULLIF(project_code, ''), 'AXO') AS project_code, status, ... FROM McpJob;`
2. `DROP TABLE McpJob;`
3. `ALTER TABLE McpJob_new RENAME TO McpJob;`
4. Re-add PK + indexes.
5. CHECKPOINT.

Heavier (~50 LOC + cargo build) but rebuilds the corrupted index from scratch. Permanent fix.

### Recommended: Option A first (quick unblock), then Option B as proper migration.

After either recovery:
1. `./scripts/axon-live stop --hard`
2. `cargo build --manifest-path src/axon-core/Cargo.toml --bin axon-brain --release` (releases binary to bin/axon-brain via promote_live_safe.sh, or use scripts/axon-live start auto-rebuild path).
3. `./scripts/axon-live start --brain-only` — should succeed.
4. Verify: `curl -fs --max-time 2 -X POST http://127.0.0.1:44129/mcp -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"tools/list","id":1}' >/dev/null && echo OK`.
5. Restart indexer per policy (live indexer always ON): `./scripts/axon-live start --indexer-full --tensorrt`.
6. Open this file, transfer each section into SOLL via soll_manager.
7. Read-back-verify each write (REQ-AXO-196 lesson).
8. Commit `docs: drop markdown handoff — content moved to SOLL`.

## Backups in /tmp (safety net for next session)
- `/tmp/soll.db.backup-2026-05-06T23` (md5 `749765eb...`) — soll.db pre-mutation, byte-identical to current state.
- `/tmp/soll.db.wal.backup-2026-05-06T23` (md5 `df9bb6b2...`) — WAL pre-mutation.
- `/tmp/mcpjob-deleted-rows-2026-05-06.json` — full content of the 4 problematic rows (audit dump, JSON).
- `/tmp/soll.db.wal.before-fix-2329`, `/tmp/soll.db.wal.before-truncate-2322` — intermediate WAL snapshots from this session's recovery attempts.

## Live runtime state at handoff time

- bin/axon-indexer: HEALTHY (started 2026-05-06 ~21:00 local, watch root /home/dstadel/projects, mode indexer_full --tensorrt).
- bin/axon-brain: CRASHED on every boot attempt (race condition, see REQ-AXO-XXX-B).
- MCP backend: UNREACHABLE.

## Lib tests + git tip

- Lib tests: 955 / 0 / 2 (last: cargo test --manifest-path src/axon-core/Cargo.toml --lib at HEAD `2362097`).
- HEAD: `2362097 chore(scripts): forward AXON_ASYNC_WRITER_ENABLED to dev runtime (REQ-AXO-193 E.6 prep)`.
- Working tree: untracked CSV probes (val41-clean-* x4, val41-baseline-* x2, etc.) + this handoff. Operator can sweep CSVs separately or commit them as evidence under VAL-AXO-041.
