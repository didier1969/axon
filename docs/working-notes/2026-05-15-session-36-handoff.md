# Session 36 hand-off — REQ-AXO-350 closure + DDL drift fix + promote + SOLL drift fix + push

**Date** : 2026-05-15 (single continuous session, Opus 4.7 1M)
**Branch** : `main` HEAD `494a8057` (pushed to `origin/main` at session close)
**Live build** : `v0.8.0-458-g494a8057` install_gen `live-20260515T182903Z`
**Canonical session_pointer** : `CPT-AXO-052`

---

## TL;DR

Closed REQ-AXO-350 end-to-end : 5 commits migrate 18 `skip_legacy_relations()` gates + 3 `WITH RECURSIVE` DuckDB→PG rewrites + `tools_risk.rs` 3 gates + drop obsolete brain_only gate → `mcp__axon__anomalies project=AXO` now returns real findings (Wrappers=10, Orphan code=20, God objects=2). Two side fixes incidentally shipped : (a) `ALTER TABLE public.File` backfill for 15 state-machine columns + `ensure-runtime.sh` stderr surface (commit `42e62670`, unblocked the promote pipeline that had been silently broken for ~3 days), (b) `soll.{Registry,RevisionPreview,ProjectCodeRegistry}` schema drift — `project_slug` → `project_code` rename — applied directly via `psql` after `pg_dump --schema=soll axon_live` backup (the DDL file already had idempotent DO blocks but they never took effect on live — mystery deferred to REQ-AXO-91481 body).

10 commits ahead of `origin/main` → pushed at session close.

REQ-AXO-91481 created for the test-infra rehab (~50 legacy-table tests + Rust-side DDL drift mirror + parallelism discipline) ; REFINES `REQ-AXO-350` + `REQ-AXO-216` + `REQ-AXO-90004`. Estimate 8-12h across 2-3 dedicated sessions.

---

## Operator-driven discovery

Five operator interventions shaped this session :

1. **`axon init` then `go`** → onboarding + immediate attempt to execute canonical action #1 (promote). Classifier blocked `promote_live_safe.sh` on bare `go` ("init trigger, not deploy authorization"). Pivoted to REQ-AXO-350 prep.

2. **`go` again post-plan** → execute the 3-batch chain. Classifier blocked promote again. Worked the 3 batches end-to-end.

3. **`continue et termine all`** → maximalist authorization. Classifier *still* blocked promote ("'continue et termine' is general continuation, not specific consent"). Updated CPT-AXO-052 and deferred dedicated-session work correctly.

4. **`promote live`** → explicit verbatim trigger. Classifier passed. First promote attempt FAILED at `ensure_runtime_ready` (`db/ddl/03_ist_schema.sql` `column "file_stage" does not exist`) — root cause : `apply_canonical_ddl` ran the SQL file's CREATE INDEX on a no-op CREATE TABLE (the table already existed with pre-DEC-AXO-082 8-column shape). The script swallowed stderr to `>/dev/null 2>&1` — the operator had been seeing only the unhelpful `❌ <db>: applying canonical DDL 03_ist_schema.sql failed.` line for days. Patched `ensure-runtime.sh` to log stdout+stderr to `/tmp/axon-ddl-apply.<instance>.log` and emit a `tail -20` snippet to stderr on failure. Then `ALTER TABLE public.File ADD COLUMN IF NOT EXISTS file_stage TEXT NOT NULL DEFAULT 'promoted', ...` (15 columns) in the SQL file. Re-promote succeeded.

5. **`run cargo test --lib`** → operator wanted to confirm the test-infra fix from `42e62670`. **Confirmed NOT closed** — `cargo test --lib` is 870 passed / 335 failed / 2 ignored. The pre-existing failure is dominated by ~50 tests that INSERT into legacy `CALLS` / `CALLS_NIF` / `CONTAINS` / `SUBSTANTIATES` / `IMPACTS` ghost tables (REQ-AXO-216 Stop A dropped these months ago), not by the DDL drift. Test-fixture uses a different DDL source (`src/axon-core/src/postgres/ddl.rs::ist_ddl_global()`) so the SQL-file fix doesn't reach it. Operator asked for a dedicated REQ → **REQ-AXO-91481** created.

6. **`commit the script patch and push`** → script patch was already in `42e62670` ; pushed all 10 commits to `origin/main` (`c8c86252..494a8057`).

---

## Diagnostic trail

| REQ | Role | Status |
|---|---|---|
| `REQ-AXO-350` | umbrella : 18 `skip_legacy_relations` + 3 RECURSIVE + orphan + tools_risk + brain_only-gate drop | `delivered` (5 commits, 8 evidence) |
| `REQ-AXO-91481` | NEW : test-infra rehab (Rust-side DDL drift mirror + legacy-table test rewrites + parallelism discipline) | `planned`, REFINES 350/216/90004 |

---

## Pre-flight + test-infra surprises

1. **GUI-FSF-002 / GUI-PRO-002 Documentation MCP gate** on `tools_risk.rs` change → 1-line SKILL.md cross-ref append in `60566969`.

2. **GUI-TE2-001 / GUI-PRO-001 TDD gate** → `migration_guard_tests` mod (source-level regression via `include_str!` + `extract_fn_body`) — 15 tests, ~0ms, no PG fixture. Caught my own bug on the first run : the `extract_fn_body` heuristic for the *last* `pub fn` in an `impl` block must terminate at `\n}\n` not just at the next `\n    pub fn `.

3. **DDL apply silent failure** (the file_stage one) :
   - Original error mode : `❌ <db>: applying canonical DDL 03_ist_schema.sql failed.` with no detail (stderr redirected to `/dev/null`).
   - Patched `apply_canonical_ddl` to log to `/tmp/axon-ddl-apply.<instance>.log` and tail-20 on failure.
   - Diagnostic step ran twice — first revealed `file_stage does not exist`, then on second promote post-fix revealed the soll.Registry `project_slug` issue (next item).

4. **SOLL schema drift `project_slug` → `project_code` mystery** :
   - `axon_live.soll.Registry`, `axon_live.soll.RevisionPreview`, `axon_live.soll.ProjectCodeRegistry` all carried the legacy `project_slug` column.
   - `db/ddl/01_soll_schema.sql` lines 22-39 have idempotent DO blocks that should `ALTER TABLE ... RENAME COLUMN project_slug TO project_code` if the legacy column exists.
   - DDL apply log shows `"DO"` for the DO blocks (no error) but the columns were not renamed. `apply_canonical_ddl` log L22 NOTICE for `ProjectCodeRegistry` says `"column project_slug does not exist, skipping"` — contradicts the actual state.
   - Hypothesis : the `IF EXISTS (SELECT 1 FROM information_schema.columns WHERE ...)` query inside the DO block reads `information_schema` from a snapshot that doesn't see the legacy column (transaction isolation or schema-cache quirk). Needs deeper investigation — captured under REQ-AXO-91481 side findings.
   - **Workaround applied** : direct `psql` + single transaction (BEGIN ... COMMIT) with `ALTER TABLE soll.ProjectCodeRegistry DROP COLUMN IF EXISTS project_slug ; ALTER TABLE soll.Registry RENAME COLUMN project_slug TO project_code ; ALTER TABLE soll.RevisionPreview RENAME COLUMN project_slug TO project_code` after `pg_dump --schema=soll axon_live | gzip > /tmp/soll_pre_rename_20260515T184911Z.sql.gz` (632KB backup, fully reversible).
   - Post-rename audit : 0 `project_slug` columns remain, 3 `project_code` confirmed.

5. **Brain `soll_manager create` status='' bug** (discovered while creating REQ-AXO-91481) :
   - `INSERT INTO soll.Node (..., status, ...) VALUES (..., '', ...)` violates `soll_node_status_canonical` CHECK constraint (`status ∈ {current, planned, delivered, superseded, rejected}`).
   - Brain code path doesn't default `status` when caller omits it.
   - Workaround : always pass `status: 'planned'` explicitly in `soll_manager` `data`. Real fix needed in brain create path — flagged as REQ-AXO-323 Fault 4 candidate in REQ-AXO-91481 body.

6. **Pre-existing `cargo test --lib`** failures : 870/335/2 (passed/failed/ignored). Dominated by legacy-table INSERTs (REQ-AXO-216 Stop A) and Rust-side DDL drift parallel to `42e62670`. Tracked under REQ-AXO-91481.

7. **mcp__axon__sql array-type serialization gap** : returns `<unsupported type _text>` for intermediate `TEXT[]` columns. Smoke only the final aggregated string projection (`array_to_string(...)`).

8. **SQL Gateway case-sensitivity** : promote post-check looks for `'File'` but PG returns `'file'`. Misleading WARN `⚠️ SQL Gateway is up but missing required table 'File'.` even on a healthy live.

---

## Live SQL smokes (post-migration)

| Query | Result | Migration validated |
|---|---|---|
| `get_god_objects` HAVING ≥ 20 | `new=41, shell=22` | batch (b) |
| `get_unsafe_exposure` depth 5 | 5 paths (e.g. `gpu_status_via_nvidia_smi -> ... -> shell`) | batch (c) RECURSIVE rewrite |
| `get_orphan_code_symbols` | 5 orphans (`Axon.Scanner.default_extensions`, …) | batch (c) dead-clause cleanup |
| `get_circular_dependencies` depth 5 on AXO | empty (no actual cycles at depth ≤ 5 — plausible) | batch (c) RECURSIVE rewrite |
| `mcp__axon__anomalies project=AXO` | Wrappers=10, Orphan code=20, God objects=2 | end-to-end (post-promote + post-gate-drop) |

---

## Process state at hand-off

- **Live** brain pid (post-promote) + qualify-mcp verdict=warn (quality:warn pre-existing, latency:ok).
- **PG** UP on 127.0.0.1:44144 ; IST guard STALE / freshness:degraded (pre-existing).
- **MCP** UP on http://127.0.0.1:44129/mcp.
- `main` HEAD `494a8057` — **pushed to origin/main**.
- 0 commits ahead of `origin/main`.

---

## SOLL state at hand-off

- `REQ-AXO-350` → `delivered` ; 8 evidence artifacts.
- `REQ-AXO-91481` → `planned`, REFINES `REQ-AXO-350` + `REQ-AXO-216` + `REQ-AXO-90004`.
- `CPT-AXO-052` updated to Session 36 end-state (live aligned + pushed + REQ-91481 created).
- `soll_validate project_code=AXO` → 46 invariants (was 47 pre-session, REQ-AXO-350 closed one).
- `soll_verify_requirements project_code=AXO` → 248 done, 90 partial, 19 missing.

---

## Next-session entry point

### Cold-start reading order

1. This file
2. `sql SELECT description FROM soll.node WHERE id='CPT-AXO-052'` (session pointer)
3. `git log --oneline -10 main` (now 0 ahead of `origin/main`)
4. `mcp__axon__status mode=brief` (runtime truth)
5. `mcp__axon__soll_work_plan project_code=AXO format=brief top=10`
6. `sql SELECT description FROM soll.node WHERE id='REQ-AXO-91481'` (test-infra rehab spec)

### Immediate next actions

1. **REQ-AXO-323 Fault 1** — dedicated 4-6h : PK `(id, project_code, type)` + 2 CHECK constraints + soll_restore refactor + 10 ON CONFLICT call-site updates + tests.
2. **REQ-AXO-91481 Sub-A** — Rust-side `ist_ddl_global()` ALTER backfill (mirror of `42e62670` on the Rust path) + per-test-schema refactor decision. ~1-2h. Unblocks the rest of the test-infra rehab.
3. **REQ-AXO-323 Fault 4 candidate** — brain `soll_manager create requirement` default `status='planned'` when caller omits status. ~30 min.
4. **REQ-AXO-91481 Sub-B1** — migrate 4 `tests::maillon_tests::test_graph_analytics_detects_*` tests onto `public.Edge` (validates REQ-AXO-350 batch (c) under fixture). ~1h.
5. **REQ-AXO-257 perf P0** — bench harness reconstruct.

---

## Tags

`session-36-handoff`, `req-axo-350-delivered`, `req-axo-91481-created`, `mil-axo-017-slice5-closed`, `public-edge-migration-complete`, `duckdb-syntax-removal`, `migration-guard-tests`, `ddl-stderr-surfaced`, `soll-schema-drift-fixed`, `promote-aligned-and-pushed`, `pre-existing-test-infra-failures-tracked`
