# Document Truth Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce documentary ambiguity in Axon by separating canonical docs from archives, moving generated SOLL exports out of misleading locations, and aligning active docs with the verified Rust-first + Canard DB reality.

**Architecture:** Keep current runtime behavior intact except for one targeted hardening: make `SOLL` export/restore resolve the repository-root `docs/vision` path regardless of the current working directory. Archive obsolete documentation instead of deleting potentially useful history, and update the canonical entry documents so a new LLM or human starts from verified reality instead of stale status narratives.

**Tech Stack:** Rust, MCP tests, Markdown docs, repo-local file moves

---

### Task 1: Harden canonical SOLL export path resolution

**Files:**
- Modify: `src/axon-core/src/mcp/soll.rs`
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing test**

Add a test that changes the working directory to `src/axon-core`, calls `axon_export_soll`, and asserts the file is still written under repo-root `docs/vision/`.

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml test_axon_export_soll_resolves_repo_root_docs_vision'
```

Expected: failure proving the current relative path is wrong when run from crate-local `cwd`.

**Step 3: Write minimal implementation**

Implement a helper that resolves the repo root by walking ancestors from `current_dir()` and falling back to the compile-time crate path. Use it for:
- export directory resolution
- latest export lookup

**Step 4: Run focused tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml test_axon_export_soll test_axon_export_soll_resolves_repo_root_docs_vision test_axon_restore_soll'
```

Expected: green.

### Task 2: Create explicit archive structure

**Files:**
- Create: `docs/archive/README.md`

**Step 1: Write archive policy**

Document the three classes:
- canonical
- archive
- generated snapshots

Specify that:
- old v1/v2 docs are historical
- `docs/archive/soll-exports/` holds archived SOLL extracts
- repo-root `docs/vision/` remains the canonical live export location

### Task 3: Move obsolete docs and generated exports out of misleading locations

**Files:**
- Move: `INSTALL_AUDIT.md`
- Move: `expert_prompt.md`
- Move: `docs/v1.0/`
- Move: `docs/v2/`
- Move: `src/axon-core/docs/vision/SOLL_EXPORT_*.md`

**Step 1: Create destination directories**

Create:

```text
docs/archive/
docs/archive/root-docs/
docs/archive/soll-exports/
```

**Step 2: Move files**

Move the historical docs into archive subtrees and move all misplaced `SOLL_EXPORT_*.md` snapshots into `docs/archive/soll-exports/`.

**Step 3: Prevent future confusion**

Add a `.gitignore` rule for stray `src/axon-core/docs/vision/SOLL_EXPORT_*.md`.

### Task 4: Realign canonical docs with verified reality

**Files:**
- Modify: `README.md`
- Modify: `docs/getting-started.md`
- Modify: `STATE.md`
- Modify: `ROADMAP.md`
- Modify: `docs/working-notes/reality-first-stabilization-handoff.md`

**Step 1: Update project truth**

Make active docs state clearly:
- Rust-first is the canonical runtime split
- Canard DB (DuckDB) replaced KuzuDB in the nominal path
- current proof of health comes from tests/runtime, not aspirational labels

**Step 2: Add start-here guidance**

Point a new LLM/human to:
- `README.md`
- `docs/getting-started.md`
- latest handoffs in `docs/working-notes/`
- archive policy in `docs/archive/README.md`

**Step 3: Mark archive boundaries**

In the handoff/root docs, state that archived documents are historical context and not the current execution contract.

### Task 5: Verify cleanup claims

**Step 1: Run Rust verification**

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml test_axon_export_soll test_axon_export_soll_resolves_repo_root_docs_vision test_axon_restore_soll'
```

**Step 2: Run broader runtime verification**

```bash
devenv shell -- bash -lc 'cd src/axon-core && cargo test --manifest-path Cargo.toml'
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

**Step 3: Verify filesystem outcome**

Check that:
- no `SOLL_EXPORT_*.md` remain in `src/axon-core/docs/vision/`
- archived files exist under `docs/archive/`
- active docs now reference the canonical reading path
