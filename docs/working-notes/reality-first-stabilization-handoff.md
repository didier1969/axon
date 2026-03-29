---
title: Reality-First Stabilization Handoff
date: 2026-03-30
branch: fix/native-ingestion-stability-audit
status: in-progress
---

# Scope

This handoff exists to preserve exact working state across context compaction.
Do not rely on conversational memory before resuming work. Re-read this file, then verify the repository state with Git and tests.

# Current Objective

Stabilize Axon before further sophistication:

1. Make `Nix + Devenv` the operational source of truth.
2. Harden Rust native ingestion and MCP/SOLL paths.
3. Keep progress measurable with concrete validation, not impression.
4. Only then continue with dashboard quality and broader architecture cleanup.

# Branch

Current working branch:

`fix/native-ingestion-stability-audit`

Base branch used for this work:

`feature/axon-native-ingestion`

# Skills Explicitly Used As Methodology

- `/home/dstadel/.claude/skills/mission-critical-architect/SKILL.md`
- `/home/dstadel/.claude/skills/system-observability-tracer/SKILL.md`
- `/home/dstadel/.claude/skills/hardware-aware-scaling/SKILL.md`
- `/home/dstadel/.claude/skills/devenv-nix-best-practices/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/axon-digital-thread/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`

Skills created during this work:

- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`
- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/agents/openai.yaml`
- `/home/dstadel/projects/axon/.claude/skills/axon-digital-thread/agents/openai.yaml`

# Method Being Applied

The workflow used so far is:

1. Understand the project vision and architecture before editing.
2. Separate vision, intended architecture, actual code, and actual runtime behavior.
3. Validate the real development environment before trusting diagnostics.
4. Prioritize dominant stability defects over exhaustive low-value cleanup.
5. Fix foundations first: environment, storage bootstrap, atomic claiming, protocol correctness, test reliability.
6. Measure progress after each phase with concrete test signals.

# High-Value Findings Identified Earlier

These were the dominant issues initially identified:

1. DuckDB plugin resolution depended on `cwd` and broke tests/runtime.
2. `pending -> claimed -> indexed` flow was not atomic.
3. Batch ACK semantics were not safely correlated.
4. Some Elixir audit/bridge paths were stale or inconsistent with current runtime.
5. MCP audit/health outputs overstated confidence while relying on stubs.
6. Ingestion still contained artificial throttling and blocking patterns.

This list was intentionally prioritized, not exhaustive.

# Changes Already Made

## Environment / Devenv

Files changed:

- `/home/dstadel/projects/axon/flake.nix`
- `/home/dstadel/projects/axon/devenv.yaml`
- `/home/dstadel/projects/axon/devenv.nix`
- `/home/dstadel/projects/axon/flake.lock`
- `/home/dstadel/projects/axon/devenv.lock`
- `/home/dstadel/projects/axon/README.md`
- `/home/dstadel/projects/axon/scripts/setup_v2.sh`
- `/home/dstadel/projects/axon/scripts/start-v2.sh`
- `/home/dstadel/projects/axon/scripts/validate-devenv.sh`

What changed:

- Shifted setup and start scripts to `devenv shell`.
- Added explicit environment validation script.
- Updated README to point contributors at `devenv shell` as the primary path.
- HydraDB was intentionally detached from the current Axon Devenv workflow.
- Active HydraDB coupling was removed from `flake.nix`, `devenv.nix`, `devenv.yaml`, `flake.lock`, and `devenv.lock`.
- `axon-db-start` is now a guarded placeholder instead of a live dependency path.

## Rust Core / Native Ingestion / MCP

Files changed:

- `/home/dstadel/projects/axon/src/axon-core/src/graph.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/go.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/mod.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/parser/sql.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/scanner.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/worker.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/bench_extraction.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/maillon_tests.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/tests/pipeline_test.rs`

What changed:

- DuckDB plugin path resolution now uses robust repo-relative discovery instead of fragile `cwd` assumptions.
- `GraphStore` bootstrap was hardened for `ist.db` and attached `soll.db`.
- In-memory DB handling was adjusted to avoid read-only attach failure patterns.
- `fetch_pending_batch()` was changed to claim work atomically under transaction.
- `worker_id` is now cleared when files transition to terminal states.
- SQL parameter handling now supports positional `?` arguments in addition to named params.
- MCP/SOLL test expectations were updated to match the current schema and export behavior.
- Several previously stub-like audit/health helpers in `graph.rs` were replaced with real graph-derived signals.
- One sandbox-sensitive Unix socket test was made robust by skipping only on `PermissionDenied`.
- Rust warnings were reduced by removing or renaming unused code and imports.

# Validation Signals Achieved

Rust validation reached a clean state during this session:

- `cargo test` in `/home/dstadel/projects/axon/src/axon-core`
- result reached: `26 passed; 0 failed`

Important note:

- This signal was obtained after stabilizing DuckDB path resolution, SOLL schema gaps, MCP behavior, and the sandbox-sensitive socket test.

# Elixir / Dashboard Validation State

Dashboard validation is now green under Devenv:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
- result reached: `20 tests, 0 failures`

Code fixes applied to reach this state:

- `CockpitLive` now subscribes to bridge events and tolerates `FileIndexed` / `ScanComplete`
- duplicate `:tick` handling was consolidated into a single runtime truth pull
- `Tracer` no longer crashes on partial or missing timestamps
- `BackpressureController` now scales according to the conservative policy expected by tests

Residual non-blocking warnings still visible during `mix test`:

- runtime warnings from intentionally simulated saturation in backpressure tests
- `os_mon` shutdown noise at the end of the test VM

# Current Git State Snapshot

At the time of writing, `git status --short --branch` showed:

```text
## fix/native-ingestion-stability-audit
 M .devenv/nix-eval-cache.db-shm
 M .devenv/nix-eval-cache.db-wal
 M .devenv/profile
 M .devenv/run
 M .devenv/tasks.db-shm
 M .devenv/tasks.db-wal
 M README.md
 M devenv.lock
 M devenv.yaml
 M scripts/setup_v2.sh
 M scripts/start-v2.sh
 M src/axon-core/src/graph.rs
 M src/axon-core/src/main.rs
 M src/axon-core/src/mcp.rs
 M src/axon-core/src/parser/go.rs
 M src/axon-core/src/parser/mod.rs
 M src/axon-core/src/parser/sql.rs
 M src/axon-core/src/scanner.rs
 M src/axon-core/src/tests/bench_extraction.rs
 M src/axon-core/src/tests/maillon_tests.rs
 M src/axon-core/src/tests/pipeline_test.rs
 M src/axon-core/src/worker.rs
 M src/dashboard/priv/native/libaxon_scanner.so
?? .devenv/bash-bash
?? .devenv/gc/shell
?? .devenv/gc/task-config-devenv-config-task-config
?? .devenv/shell-35750cfab17f5f4a.sh
?? .devenv/shell-95a6f6d95ce91d77.sh
?? RAPPORT_AUDIT_20260329T205124Z.md
?? scripts/validate-devenv.sh
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001137.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001213.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001250.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001343.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001416.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_001502.md
```

Interpretation:

- `.devenv/*` changes are mostly runtime artifacts from Devenv execution.
- `libaxon_scanner.so` changed and should be treated carefully as a generated native artifact.
- multiple `SOLL_EXPORT_*.md` files are considered legitimate historical exports of the `SOLL` conceptual layer and are intentionally kept.
- the initial audit report `RAPPORT_AUDIT_20260329T205124Z.md` was deemed obsolete for the current state and should not be committed.
- HydraDB should now be considered detached from the active Devenv workflow unless explicitly reintroduced later.

# Resume Checklist

When resuming after compaction, do this in order:

1. Read this file completely.
2. Run `git status --short --branch`.
3. Run `git diff --stat`.
4. Re-check the branch is still `fix/native-ingestion-stability-audit`.
5. Re-run Rust validation:
   - `cd src/axon-core && cargo test`
6. Continue the interrupted dashboard validation:
   - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
7. If both are green, move to warning cleanup and artifact review.

# Recommended Next Steps

Primary next step:

1. classify remaining git changes into:
   - source changes to keep
   - local runtime artifacts to drop
   - generated conceptual history to keep

Secondary next step:

2. decide whether to keep, ignore, or clean generated artifacts before commit:
   - `.devenv/*` runtime artifacts
   - native binary drift in `src/dashboard/priv/native/libaxon_scanner.so`

Method skill already created:

- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`

# Anti-Drift Rule

After compaction, do not trust any summary blindly, including this one.

Use this file as a map, then verify the code and runtime state directly.
