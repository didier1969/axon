# SOLL Project Identity Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** remove `project_slug` from active Axon surfaces, lock `project_code` as the only valid project identifier, and prepare safe retirement of the remaining migration-only compatibility logic.

**Architecture:** keep runtime truth on `soll.ProjectCodeRegistry`, preserve migration-only legacy readers in `graph_bootstrap.rs` for now, and add lightweight non-regression checks so active code/docs cannot drift back to legacy vocabulary. Execute in small, low-risk slices because SOLL contains real data.

**Tech Stack:** Rust, DuckDB, Python, shell wrapper, Markdown docs.

---

### Task 1: Clean Active Documentation

**Files:**
- Modify: `docs/working-notes/2026-04-05-soll-canonical-ids-and-project-scope.md`
- Modify: `docs/working-notes/2026-04-01-reprise-handoff.md`
- Modify: `docs/working-notes/2026-04-01-wsl-install-runtime-notes.md`
- Modify: `docs/plans/2026-04-03-reliability-callgraph-execution-plan.md`
- Modify: `docs/plans/2026-04-07-omniscience-federation-design.md`
- Modify: `docs/archive/root-docs/expert_prompt.md`

**Step 1: Replace active references**

- replace legacy `project_slug` usage with `project_code`
- reword identity descriptions so `ProjectCodeRegistry` is the active runtime registry
- remove wording that implies slug/code duality still exists in active workflows

**Step 2: Keep archives factual**

- keep legacy explanation only where historically necessary
- do not rewrite exported SOLL snapshots in bulk in this slice

**Step 3: Verify**

Run:
```bash
rg -n "project_slug" docs/working-notes/2026-04-05-soll-canonical-ids-and-project-scope.md \
  docs/working-notes/2026-04-01-reprise-handoff.md \
  docs/working-notes/2026-04-01-wsl-install-runtime-notes.md \
  docs/plans/2026-04-03-reliability-callgraph-execution-plan.md \
  docs/plans/2026-04-07-omniscience-federation-design.md \
  docs/archive/root-docs/expert_prompt.md
```
Expected: no hits.

**Status:** completed

---

### Task 2: Add Non-Regression Guard

**Files:**
- Create: `scripts/check_no_project_slug.py`
- Modify: `scripts/axon`

**Step 1: Implement bounded scanner**

- scan only active surfaces:
  - `src/axon-core`
  - `scripts`
  - `docs/skills`
  - selected active docs
- explicitly allow:
  - `ADR-2026-04-18-project-registry-runtime-authority.md`
  - `src/axon-core/src/graph_bootstrap.rs`
  - the guard script itself
  - `scripts/axon`

**Step 2: Add CLI entrypoint**

- expose guard as:
```bash
./scripts/axon check-project-identity
```

**Step 3: Verify**

Run:
```bash
python3 -m py_compile scripts/check_no_project_slug.py
python3 scripts/check_no_project_slug.py
./scripts/axon check-project-identity
```
Expected: all pass.

**Status:** completed

---

### Task 3: Retire Migration Compatibility (Future Slice)

**Files:**
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Test: `src/axon-core/src/graph_bootstrap.rs`

**Step 1: Preconditions**

- confirm live runtime no longer exposes legacy `project_slug` columns
- confirm no live writes are still arriving under legacy identifiers
- confirm at least one stable restart cycle with current migration

**Step 2: Remove legacy readers**

- remove `project_slug`-aware compatibility logic from:
  - `normalize_soll_registry()`
  - `normalize_project_code_registry_schema()`
  - `normalize_revision_preview_schema()`
- keep only canonical `project_code` paths

**Step 3: Remove legacy tests**

- remove migration tests that create legacy schemas once compatibility is intentionally retired
- replace them with invariant tests asserting runtime schemas are canonical only

**Step 4: Verify**

Run:
```bash
cargo test normalize_ -- --test-threads=1
./scripts/axon check-project-identity
```

**Status:** pending

---

### Task 4: Strengthen SOLL Invariants (Next Hardening Wave)

**Files:**
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Reject non-canonical project identifiers on write**

- fail fast if a mutation payload uses anything other than canonical `project_code`
- return actionable MCP guidance

**Step 2: Strengthen uniqueness and anti-duplication**

- audit current `logical_key` usage
- add stricter duplicate detection before create operations where safe

**Step 3: Improve SOLL validation**

- extend `soll_validate` to surface identity and orphan integrity more explicitly

**Status:** pending
