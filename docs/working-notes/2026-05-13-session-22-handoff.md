# Session 22 hand-off — MIL-AXO-017 designed, slices await execution

**Date** : 2026-05-13 ~14:00 UTC
**Branch** : `main` HEAD `9999ff8` (REQ-AXO-289 S7 cut-over shipped)
**Live state** : brain UP, indexer stopped, IST partially repop
**Canonical SOLL pointer** : `CPT-AXO-052` (updated this session)

---

## TL;DR

Two SOLL decisions + one milestone designed in this session :

| ID | Topic | Status |
|---|---|---|
| `DEC-AXO-081` | Single indexer serves N projects via per-file project_code resolution (REQ-AXO-289 S7 enabler) | accepted, shipped in commit `9999ff8` |
| `DEC-AXO-082` | SQL-file DDL + seed scripts replace Rust-string `bootstrap_global_pg_schema` | accepted, DDL files written (uncommitted) |
| `DEC-AXO-083` | Retire AGE entirely; unify IST edge storage on `public.Edge` + WITH RECURSIVE + `retrieve_context_v2` | accepted, drives MIL-AXO-017 |
| `MIL-AXO-017` | AGE retirement + PG-native unified retrieval | current, 7 REQ slices ready to execute |
| `VAL-AXO-073` | MIL-AXO-017 closing gates (5 perf + 2 correctness checks) | planned, attached to slice 7 |

**Zero code changes shipped for MIL-AXO-017 yet.** All slice work begins next session.

---

## How we got here this session

1. Continuing the REQ-AXO-289 streaming pipeline v2 work, hit `bootstrap_global_pg_schema` silent failure on fresh `axon_dev` (schemas created, but `public.*` IST tables silently absent despite the bootstrap log saying success).
2. Operator proposed externalising DDL to SQL files run via `psql -v ON_ERROR_STOP=1 -f`. → `DEC-AXO-082` logged + `db/ddl/00_extensions.sql` through `03_ist_schema.sql` written.
3. While discussing FTS perf, operator raised the question of AGE perf for deep traversals. Diagnosis showed `create_elabel()` doesn't auto-index `start_id` / `end_id` and `agtype` adds 2-5× per-row cost.
4. Operator chose to drop AGE entirely. Triggered `/grill-me` for design.
5. 4 grill-me Q/A rounds + 5 follow-up rounds → `DEC-AXO-083` + `MIL-AXO-017` + 7 REQ children + `VAL-AXO-073` topology built.

---

## MIL-AXO-017 — the milestone

### Driver

`DEC-AXO-083` — "Retire AGE; unify IST edge storage on `public.Edge` + WITH RECURSIVE SQL functions; fuse FTS+vector+graph in `retrieve_context_v2`".

### Slices (sequential REFINES chain, all BELONGS_TO PIL-AXO-001)

| # | REQ | Slice | Files touched | Estimated effort |
|---|---|---|---|---|
| 1 | `REQ-AXO-295` | DDL: `public.Edge` + 4 indexes | `db/ddl/03_ist_schema.sql` | 30 min |
| 2 | `REQ-AXO-296` | DDL: SQL function library (`impact`, `path`, `blast_radius`, `why_chain`, `callers_of`) | `db/ddl/04_graph_functions.sql` (new) | 1-2 h (WITH RECURSIVE bodies + cycle guards + tests) |
| 3 | `REQ-AXO-297` | A3 batched writer dual-writes `public.Edge` (transitional) | `src/axon-core/src/graph_ingestion.rs::upsert_graph_v2_batch` | 1 h |
| 4 | `REQ-AXO-298` | `retrieve_context_v2` SQL function (FTS + vector + graph RRF fusion) | `db/ddl/04_graph_functions.sql` (extend) | 2 h (RRF + 3-lane JOIN + EXPLAIN tuning) |
| 5 | `REQ-AXO-299` | MCP tools bascule (impact/path/why/anomalies/retrieve_context) onto SQL functions | `src/axon-core/src/graph_analytics.rs` (12 branches), `mcp/tools_*` | 2-3 h |
| 6 | `REQ-AXO-300` | `cypher`→`sql` rename + full AGE code retirement | `mcp/tools_system.rs`, `graph_ingestion.rs`, `postgres/age.rs` (delete), `db/ddl/00_extensions.sql` | 2 h |
| 7 | `REQ-AXO-301` | Live promote + VAL-AXO-073 bench | wipe + promote + bench | 1 h + indexer repop wait |

**Total estimated** : 10-13 h of focused work + repop time. Realistic at 2-4 sessions.

### Closing gate (VAL-AXO-073)

7 checks, all must pass to close MIL-AXO-017 :

1. `axon_graph` schema absent from `axon_live` (count = 0)
2. `pg_extension WHERE extname='age'` returns 0 rows
3. `impact('Symbol::X', 5)` p95 < 50ms (vs prior ~150ms via AGE)
4. `retrieve_context_v2` p95 < 100ms (3-lane fusion)
5. Live indexer sustained ≥ 116 ch/s
6. `qualify-mcp --surface core --checks quality,latency` verdict = ok
7. `grep -rE "ag_catalog|axon_graph|emit_age|skip_sql_relations" src/axon-core/src/` in non-test code = 0 matches

---

## Process state at hand-off

### Live

- **Brain** pid 76375 — build `v0.8.0-391-g9999ff8` install gen `live-20260513T112536Z` — HEALTHY
- **Indexer** stopped (operator killed during dev test pre-grill-me)
- **PG** UP on 127.0.0.1:44144
- **MCP** UP on http://127.0.0.1:44129/mcp
- **IST state** : repop partial (~6 144 symbols / ~8 559 chunks committed pre-stop)

### Dev

- **Indexer** stopped (was `cargo-target/debug/axon-indexer`, contains cold-start poll + diag instrumentation in `bootstrap_global_pg_schema`)
- **`axon_dev`** : schemas `ag_catalog`, `axon_runtime`, `public`, `soll` exist ; `public.*` IST tables UNCREATED (open bug DEC-082 addresses) ; `soll` populated from live backup
- **`.axon-dev/graph_v2/`** : ist.db + ist-reader.db present from earlier runs

### Branches

- `main` HEAD `9999ff8` — pushed
- No feature branch for MIL-AXO-017 yet — to create at slice 1 start

### Working tree (uncommitted)

```
?? db/ddl/00_extensions.sql      # CREATE EXTENSION age (REMOVE in slice 6 of MIL-AXO-017)
?? db/ddl/01_soll_schema.sql     # soll.* tables + indexes (with pg_trgm fuzzy lookups)
?? db/ddl/02_axon_runtime.sql    # axon_runtime.* telemetry tables
?? db/ddl/03_ist_schema.sql      # public.* IST tables (NO public.Edge yet — slice 1 adds it)
M  src/axon-core/src/graph_bootstrap.rs  # diagnostic info!() in bootstrap_global_pg_schema (revert before commit)
?? docs/working-notes/2026-05-13-session-22-handoff.md   # this file
```

### Backups

- `~/backups/soll/axon_live-soll-pre-wipe-20260513T052748Z.sql.gz` (533K) — pre-wipe SOLL snapshot from earlier this session

---

## Next-session entry point

### Cold-start reading order

1. This file
2. `cypher SELECT description FROM soll.main.Node WHERE id='CPT-AXO-052'` (session pointer, updated)
3. `cypher SELECT description FROM soll.main.Node WHERE id='DEC-AXO-083'` (milestone driver)
4. `cypher SELECT description FROM soll.main.Node WHERE id='MIL-AXO-017'` (slice structure)
5. `mcp__axon__status mode=brief` (runtime truth)
6. `mcp__axon__soll_work_plan project_code=AXO format=brief top=5 limit=15` (work plan should now surface REQ-AXO-295 as top unblocker)

### Immediate next actions

1. **Commit the DEC-AXO-082 foundation** (uncommitted DDL files) as the prep for MIL-AXO-017. Suggested commit message :
   ```
   feat(db): DEC-AXO-082 — externalise DDL to db/ddl/*.sql for idempotent psql -f bootstrap

   Replaces the Rust-string bootstrap_global_pg_schema (which silently failed on
   fresh axon_dev despite logging success) with versioned SQL files:
     db/ddl/00_extensions.sql   — age + vector + pg_trgm + axon_graph init
     db/ddl/01_soll_schema.sql  — soll.* tables + indexes + GIN trgm
     db/ddl/02_axon_runtime.sql — axon_runtime.* telemetry tables
     db/ddl/03_ist_schema.sql   — public.* IST tables + indexes (no public.Edge yet)

   psql -f wiring into ensure-runtime.sh tracked separately (follow-up REQ).
   Note: 00_extensions.sql still contains `CREATE EXTENSION age` for now —
   removed in MIL-AXO-017 slice 6 (REQ-AXO-300).
   ```

2. **Revert the diagnostic instrumentation** in `graph_bootstrap.rs` (the per-stmt `info!()` calls I added during diagnosis) — no longer needed.

3. **Start MIL-AXO-017 slice 1** (`REQ-AXO-295`) : add `public.Edge` table + 4 indexes to `db/ddl/03_ist_schema.sql`. Tracer-bullet — non-destructive, no reader/writer changes. Demoable with `\dt public.Edge` + manual INSERT.

### Operator-gated stops (re-confirm before executing)

- **Slice 6 (REQ-AXO-300) destructive AGE drop on live** — operator confirms before merging to main.
- **Slice 7 (REQ-AXO-301) wipe live IST + promote** — SOLL backup mandatory ; no concurrent live indexer.

---

## Rejected duplicates (housekeeping note)

During the SOLL milestone build, a duplicate run created `MIL-AXO-018`, `REQ-AXO-302..308`, `VAL-AXO-074`, `DEC-AXO-084`. All marked `status=rejected` with `[REJECTED-DUPLICATE of X-AXO-N]` title suffix. The canonical IDs are `017 / 295-301 / 073 / 083`. `soll_validate` clean (only pre-existing REQ-AXO-294 orphan + REQ-AXO-272 missing evidence remain — both pre-date this session).

---

## Tags

`session-22-handoff`, `mil-axo-017-design-complete`, `dec-axo-083-driver`, `dec-axo-082-prereq`, `age-retirement`, `slice-execution-pending`, `grill-me-completed`
