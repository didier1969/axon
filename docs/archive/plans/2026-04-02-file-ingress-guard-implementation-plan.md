# File Ingress Guard Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce a low-risk `FileIngressGuard` that suppresses redundant filesystem ingress before it rewrites `File`, while keeping DuckDB as the only canonical scheduler and status authority.

**Architecture:** Add a small Rust module that hydrates a derived file-stamp cache from `File` and exposes `should_stage` decisions to scanner and watcher. Keep all claims, priorities, and final status transitions in DuckDB; the guard only filters ingress and updates itself from the `File` row actually committed in DuckDB, never from write intent.

**Tech Stack:** Rust, DuckDB/Canard DB, Axon scanner and watcher runtime, existing Rust test suite.

---

### Task 1: Freeze the contract in tests and docs

**Files:**
- Modify: `task_plan.md`
- Modify: `findings.md`
- Modify: `progress.md`
- Test: `src/axon-core/src/tests/maillon_tests.rs`
- Test: `src/axon-core/src/tests/mod.rs`

**Step 1: Write the failing tests**

Add tests that express the contract without integrating the guard yet:

- hydrate file stamps from `File`
- unchanged `(mtime,size)` returns skip
- changed `mtime` or `size` returns stage
- unknown path returns stage
- missing path can be tombstoned and later re-staged if recreated
- guard failures must fail open, not block scanning
- `indexing + changed metadata` must never be suppressed because DuckDB must still arm `needs_reindex`
- rollout kill switch disables the guard path cleanly

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml file_ingress_guard -- --nocapture'
```

Expected:

- compile error or failing tests because the guard module does not exist yet

**Step 3: Update planning artefacts**

Record:

- the canonical contract
- the exact reasons this component is non-canonical
- the rollout order

**Step 4: Commit**

```bash
git add task_plan.md findings.md progress.md src/axon-core/src/tests/mod.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "test: freeze file ingress guard contract"
```

### Task 2: Introduce the isolated guard module

**Files:**
- Create: `src/axon-core/src/file_ingress_guard.rs`
- Modify: `src/axon-core/src/lib.rs`
- Modify: `src/axon-core/src/config.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the minimal implementation**

Add a small derived cache with:

- `FileIngressGuard`
- lightweight entry type holding only `path`, `mtime`, `size`, and `status`
- `GuardDecision`
- `hydrate_from_store`
- `should_stage`
- `record_committed_row`
- `record_tombstone`
- `invalidate_all`
- explicit kill switch wiring

Do not integrate it into scanner or watcher yet.

**Step 2: Run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml file_ingress_guard -- --nocapture'
```

Expected:

- new guard tests pass
- no runtime integration yet
- guard can be disabled explicitly

**Step 3: Commit**

```bash
git add src/axon-core/src/file_ingress_guard.rs src/axon-core/src/lib.rs src/axon-core/src/tests/mod.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add isolated file ingress guard"
```

### Task 3: Hydrate the guard at boot without changing scheduler authority

**Files:**
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing boot-level test**

Cover:

- startup recovery runs first
- guard hydrates from recovered `File`
- hydration happens before the guard-backed ingress path is exposed
- no claim path is moved out of DuckDB
- boot invalidation/recovery inside `GraphStore::new()` is reflected in hydration because hydration happens strictly after store initialization

**Step 2: Run targeted test**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml boot_guard -- --nocapture'
```

Expected:

- failing boot-level assertion before wiring

**Step 3: Wire hydration**

Pass a shared guard handle into background tasks only after:

- `GraphStore::new`
- startup recovery and compatibility handling implied by store initialization
- guard hydration

Also wire the kill switch so disabled mode uses the current scanner/watcher path verbatim.

Do not change:

- claim ordering
- `fetch_pending_candidates`
- `claim_pending_paths`

**Step 4: Re-run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml boot_guard -- --nocapture'
```

Expected:

- hydration test passes
- existing claim path remains DB-driven

**Step 5: Commit**

```bash
git add src/axon-core/src/main.rs src/axon-core/src/main_background.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: hydrate file ingress guard at boot"
```

### Task 4: Branch the hot watcher through the guard

**Files:**
- Modify: `src/axon-core/src/fs_watcher.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write the failing watcher tests**

Cover:

- duplicate hot event on unchanged file is ignored
- changed file still stages
- recreated tombstoned file stages again
- watcher remains fail-open if guard is unavailable
- `indexing + changed metadata` still reaches DuckDB so `needs_reindex` can be armed there

**Step 2: Run targeted watcher tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml watcher_guard -- --nocapture'
```

Expected:

- watcher guard tests fail before integration

**Step 3: Wire the watcher**

Replace direct `upsert_hot_file(...)` calls with:

- read metadata
- `guard.should_stage(...)`
- stage only on `StageNew` or `StageChanged`

Update the guard only after successful canonical write or tombstone.

Important:

- do not update from the watcher input tuple
- update from the `File` row actually committed, read back or returned by the canonical DB path

**Step 4: Re-run watcher tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml watcher_guard -- --nocapture'
```

Expected:

- watcher guard tests pass

**Step 5: Commit**

```bash
git add src/axon-core/src/fs_watcher.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: filter hot watcher ingress with guard"
```

### Task 5: Branch the scanner through the guard

**Files:**
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Test: `src/axon-core/src/tests/maillon_tests.rs`
- Test: `src/axon-core/src/tests/pipeline_test.rs`

**Step 1: Write the failing scanner tests**

Cover:

- initial scan with hydrated guard skips unchanged known files
- changed file still enters `bulk_insert_files`
- unknown file is inserted
- scan keeps current `.axonignore` behavior
- guard unavailable falls back to current scanner behavior
- files currently `indexing` with changed metadata are never silently skipped

**Step 2: Run targeted scanner tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml scanner_guard -- --nocapture'
```

Expected:

- scanner guard tests fail before integration

**Step 3: Wire scanner filtering**

Before adding a file to the bulk insert batch:

- compute `project_slug`, `mtime`, `size`
- ask the guard whether staging is needed
- only append stage-worthy rows to the batch

Update the guard only from committed `File` rows after successful `bulk_insert_files`.

Do not let the scanner teach the guard from its own intended values.

**Step 4: Re-run scanner tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml scanner_guard -- --nocapture'
```

Expected:

- scanner guard tests pass
- no regression on `.axonignore`

**Step 5: Commit**

```bash
git add src/axon-core/src/scanner.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/maillon_tests.rs src/axon-core/src/tests/pipeline_test.rs
git commit -m "feat: filter scanner ingress with guard"
```

### Task 6: Add minimal telemetry and prove non-regression

**Files:**
- Modify: `src/axon-core/src/bridge.rs`
- Modify: `src/axon-core/src/main_telemetry.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `docs/working-notes/2026-04-01-wsl-install-runtime-notes.md`
- Test: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Add minimal telemetry**

Expose only:

- `guard_hits`
- `guard_misses`
- `guard_hydrated_entries`
- `guard_hydration_duration_ms`
- `guard_bypassed_total`
- lightweight probes for why a file is re-opened toward `pending`

Do not expose a second source of truth for priority or status.

**Step 2: Add non-regression tests**

Cover:

- claims still come from DuckDB
- priorities still come from DuckDB
- guard does not change scheduler order
- fallback still works when guard is disabled or empty
- boot order remains `store init -> hydration -> ingress tasks`
- divergence path is safe: if guard state is missing or stale, DB path still works

**Step 3: Run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml guard_ -- --nocapture'
```

Expected:

- telemetry and non-regression tests pass

**Step 4: Commit**

```bash
git add src/axon-core/src/bridge.rs src/axon-core/src/main_telemetry.rs src/axon-core/src/main_background.rs src/axon-core/src/tests/maillon_tests.rs docs/working-notes/2026-04-01-wsl-install-runtime-notes.md
git commit -m "feat: add file ingress guard telemetry"
```

### Task 7: Run the full verification gate

**Files:**
- Modify if needed: `STATE.md`
- Modify if needed: `docs/working-notes/2026-04-01-reprise-handoff.md`

**Step 1: Run Rust tests**

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'
```

Expected:

- full core suite green

**Step 2: Run dashboard tests**

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

Expected:

- dashboard suite green

**Step 3: Run runtime startup checks**

```bash
bash scripts/start-v2.sh
bash scripts/stop-v2.sh
```

Expected:

- dashboard, SQL and MCP checks green
- clean shutdown

**Step 4: Sanity-check the target symptoms**

Validate manually or with SQL probes:

- unchanged files are no longer mass-restaged
- `pending` churn drops on stable repos
- MCP remains available during heavy scan
- no new scheduler authority appears outside DuckDB

**Step 5: Commit docs alignment**

```bash
git add STATE.md docs/working-notes/2026-04-01-reprise-handoff.md
git commit -m "docs: align runtime truth after file ingress guard"
```

## Risks To Watch

- accidental duplication of scheduling authority in memory
- guard updated before DB commit
- guard updated from write intent instead of committed `File` row
- stale guard after invalidation or recovery
- missing kill switch during rollout
- false `SkipUnchanged` while a file is already `indexing`
- hidden project favoritism sneaking into the guard path
- guard becoming required for correctness instead of optional for optimization

## Acceptance Criteria

- scanner and watcher no longer blindly upsert unchanged files
- DuckDB remains the only source of truth for claims and priority
- boot remains restart-safe
- the system fails open if the guard is unavailable
- the observed `pending` churn is materially reduced on stable repositories
