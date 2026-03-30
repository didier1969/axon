---
title: Reality-First Stabilization Handoff
date: 2026-03-30
branch: feat/axon-stabilization-continuation
status: in-progress
---

# Scope

This handoff exists to preserve exact working state across context compaction.
Do not rely on conversational memory before resuming work. Re-read this file, then verify the repository state with Git and tests.

# Current Objective

Stabilize Axon for real daily use before further sophistication:

1. Make `Nix + Devenv` the operational source of truth.
2. Harden Rust native ingestion and MCP/SOLL paths.
3. Turn visible operator and MCP surfaces into truthful, useful workflows for LLM-assisted development.
4. Keep progress measurable with concrete validation, not impression.
5. Only then continue with dashboard quality and broader architecture cleanup.

# Branch

Current working branch:

`feat/axon-stabilization-continuation`

This branch was created after merging the previous stabilization wave into `main`.

Historical base branch used earlier:

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
7. Prefer validation en conditions reelles over speculative product promises.

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
- `/home/dstadel/projects/axon/scripts/stop-v2.sh`
- `/home/dstadel/projects/axon/scripts/validate-devenv.sh`

What changed:

- Shifted setup and start scripts to `devenv shell`.
- Re-aligned stop script with the current local runtime instead of the old DB bootstrap path.
- Added explicit environment validation script.
- Updated README to point contributors at `devenv shell` as the primary path.
- HydraDB was intentionally detached from the current Axon Devenv workflow.
- Active HydraDB coupling was removed from `flake.nix`, `devenv.nix`, `devenv.yaml`, `flake.lock`, and `devenv.lock`.
- `axon-db-start` is now a guarded placeholder instead of a live dependency path.
- `setup_v2.sh` and `start_v2.sh` were corrected to use the Devenv `CARGO_TARGET_DIR` output path instead of the stale `src/axon-core/target/...` assumption.

## Rust Core / Native Ingestion / MCP

Files changed:

- `/home/dstadel/projects/axon/src/axon-core/src/graph.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_background.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_services.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/main_telemetry.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_analytics.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_ingestion.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/graph_query.rs`
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
- `axon_query` now reports its effective mode instead of overstating semantic availability.
- `axon_restore_soll` is being integrated as the official MCP-driven restore path from `SOLL_EXPORT_*.md`.
- One sandbox-sensitive Unix socket test was made robust by skipping only on `PermissionDenied`.
- Rust warnings were reduced by removing or renaming unused code and imports.

# Validation Signals Achieved

Rust validation reached a clean state during this session:

- `cargo test` in `/home/dstadel/projects/axon/src/axon-core`
- result reached first: `26 passed; 0 failed`
- result reached now: `27 passed; 0 failed`
- result reached now after VCR coverage expansion: `30 passed; 0 failed`

Important note:

- This signal was obtained after stabilizing DuckDB path resolution, SOLL schema gaps, MCP behavior, and the sandbox-sensitive socket test.

# Elixir / Dashboard Validation State

Dashboard validation is now green under Devenv:

- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
- result reached first: `20 tests, 0 failures`
- result reached now: `26 tests, 0 failures`
- result reached now after VCR-5 instrumentation: `27 tests, 0 failures`
- result reached now after transient progress truth support: `30 tests, 0 failures`

Code fixes applied to reach this state:

- `CockpitLive` now subscribes to bridge events and tolerates `FileIndexed` / `ScanComplete`
- duplicate `:tick` handling was consolidated into a single runtime truth pull
- `Tracer` no longer crashes on partial or missing timestamps
- `BackpressureController` now scales according to the conservative policy expected by tests

Residual non-blocking warnings still visible during `mix test`:

- runtime warnings from intentionally simulated saturation in backpressure tests
- `os_mon` shutdown noise at the end of the test VM

# Additional Work Completed On This Branch

## Operator Workflow / Validation En Conditions Reelles

Files changed:

- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/server.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/path_policy.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/batch_dispatch.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/sql_gateway.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_protocol.ex`
- `/home/dstadel/projects/axon/src/dashboard/lib/axon_nexus/axon/watcher/pool_event_handler.ex`
- `/home/dstadel/projects/axon/src/axon-core/src/main.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/protocol.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/catalog.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/dispatch.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/format.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/soll.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_dx.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_governance.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_risk.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_soll.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_system.rs`
- `/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs`

What changed:

- The manual scan action exposed by the cockpit is now actually wired to the Rust-side scan path instead of being a visible no-op.
- `PARSE_BATCH` now carries a `batch_id` across Elixir and Rust.
- `BATCH_ACCEPTED` acknowledgements are now correlated instead of freeing all pending callers at once.
- watcher path policy, batch dispatch, SQL gateway access, pool protocol helpers, and pool event effects are now extracted into dedicated Elixir modules
- `server.ex` now delegates file policy and dispatch concerns instead of holding all helper logic inline
- the high-density `handle_info({:ok, path}, state)` path is now split into named private steps for project resolution, reindex decision, and routing
- extracted watcher helpers now have direct Elixir test coverage in `test/axon_nexus/axon/watcher/path_policy_test.exs` and `test/axon_nexus/axon/watcher/pool_protocol_test.exs`
- manual scan truthfulness now emits telemetry at the operator edge and at the forwarding edge:
  - `[:axon, :watcher, :manual_scan_triggered]`
  - `[:axon, :watcher, :scan_forwarded]`
  - executable coverage lives in `test/axon_nexus/axon/watcher/server_test.exs`
- `Axon.Watcher.Progress` now maintains a transient operator overlay so the cockpit can show `indexing -> live` coherently even before the next DB-derived status refresh
  - executable coverage lives in `test/axon_nexus/axon/watcher/progress_test.exs`
- `axon_query` messaging was brought back in line with actual runtime capability: structural first, semantic only when available.
- `axon_restore_soll` is now covered by tests against the official Markdown export structure.
- `mcp.rs` phase-1 refactor is complete:
  - JSON-RPC protocol types now live in `src/axon-core/src/mcp/protocol.rs`
  - SOLL export parsing and restore helper types now live in `src/axon-core/src/mcp/soll.rs`
  - `mcp.rs` remains the public entrypoint for `McpServer` and tool behavior
- `mcp.rs` phase-2 refactor is complete:
  - MCP tool catalog now lives in `src/axon-core/src/mcp/catalog.rs`
  - MCP tool dispatch now lives in `src/axon-core/src/mcp/dispatch.rs`
  - MCP table formatting helper now lives in `src/axon-core/src/mcp/format.rs`
  - public tool names and `tools/list` / `tools/call` contracts were preserved
- `mcp.rs` phase-3 is complete:
  - SOLL handlers now live in `src/axon-core/src/mcp/tools_soll.rs`
  - DX handlers now live in `src/axon-core/src/mcp/tools_dx.rs`
  - governance handlers now live in `src/axon-core/src/mcp/tools_governance.rs`
  - risk handlers now live in `src/axon-core/src/mcp/tools_risk.rs`
  - system/lattice/debug/cypher batching handlers now live in `src/axon-core/src/mcp/tools_system.rs`
  - `mcp.rs` itself is now reduced to the MCP entrypoint and module wiring
  - MCP tests were moved into `src/axon-core/src/mcp/tests.rs`
- `graph.rs` first refactor slice is complete:
  - graph-derived audit/coverage/debt/god-object helpers now live in `src/axon-core/src/graph_analytics.rs`
  - `GraphStore` public API stayed unchanged
  - `graph.rs` second refactor slice is complete:
    - query and execute primitives now live in `src/axon-core/src/graph_query.rs`
    - `GraphStore` public methods were preserved
  - `graph.rs` third refactor slice is complete:
    - ingestion persistence now lives in `src/axon-core/src/graph_ingestion.rs`
    - pending claiming, symbol embedding updates, and batch write persistence were preserved
  - `graph.rs` fourth refactor slice is complete:
    - DB bootstrap, plugin discovery, session attach, and schema creation now live in `src/axon-core/src/graph_bootstrap.rs`
  - `graph.rs` is now largely reduced to FFI type definitions and pool lifecycle
- `main.rs` first refactor slice is complete:
  - incoming telemetry command handling now lives in `src/axon-core/src/main_telemetry.rs`
  - the runtime bootstrap and socket loop remain in `main.rs`
  - command behavior and test signals were preserved
- `main.rs` second refactor slice is complete:
  - watchdog memory loop, autonomous ingestor, and initial scan startup now live in `src/axon-core/src/main_background.rs`
  - per-connection telemetry handling now also lives in `src/axon-core/src/main_telemetry.rs`
  - listener accept loop and top-level runtime wiring remain in `main.rs`
- `main.rs` third refactor slice is complete:
  - worker pool startup, semantic worker startup, and MCP HTTP startup now live in `src/axon-core/src/main_services.rs`
  - `main.rs` is now primarily runtime bootstrap + telemetry accept loop
- A dedicated incremental refactor plan now exists for the oversized MCP module:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-mcp-refactor-plan.md`
- A follow-up mapping now exists for the next core refactor candidates:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-core-refactor-candidates.md`
- A validation en conditions reelles E2E plan now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-e2e.md`
- A validation en conditions reelles operational checklist now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-checklist.md`
  - VCR-1 and VCR-2 now also have executable MCP coverage in `src/axon-core/src/mcp/tests.rs`
  - VCR-4 now also has executable MCP continuity coverage in `src/axon-core/src/mcp/tests.rs`
- A validation en conditions reelles run log now exists:
  - `/home/dstadel/projects/axon/docs/plans/2026-03-30-validation-conditions-reelles-log.md`
  - latest live runtime finding: `/mcp` and `/sql` are reachable after nominal bootstrap/start, but live value on Axon itself is still limited by real index coverage on some watcher/Elixir symbols

# Validation In Conditions Reelles Priority

Commercialization is no longer the immediate leading phase.

Priority order is now:

1. validation en conditions reelles on Axon itself
2. product stabilization
3. commercialization

The active intent is to make Axon genuinely useful for LLM-assisted software development and project steering before optimizing for external packaging.

# Current Git State Snapshot

Current `git status --short --branch` shows:

```text
## feat/axon-stabilization-continuation
 M .devenv/nix-eval-cache.db-shm
 M .devenv/nix-eval-cache.db-wal
 M .devenv/profile
 M .devenv/run
 M .devenv/tasks.db-shm
 M .devenv/tasks.db-wal
 M README.md
 M docs/architecture/visualize-nexus-pull.html
 M docs/working-notes/reality-first-stabilization-handoff.md
 M scripts/setup_v2.sh
 M scripts/start-v2.sh
 M scripts/stop-v2.sh
 M src/axon-core/src/graph.rs
 M src/axon-core/src/lib.rs
 M src/axon-core/src/main.rs
 M src/axon-core/src/mcp.rs
 M src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex
 M src/dashboard/lib/axon_nexus/axon/watcher/server.ex
?? .devenv/bash-bash
?? .devenv/gc/shell
?? .devenv/gc/task-config-devenv-config-task-config
?? .devenv/shell-*.sh
?? docs/plans/2026-03-30-commercial-stabilization-roadmap.md
?? docs/plans/2026-03-30-core-refactor-candidates.md
?? docs/plans/2026-03-30-mcp-refactor-plan.md
?? src/axon-core/docs/vision/SOLL_EXPORT_2026-03-30_*.md
?? src/axon-core/src/graph_analytics.rs
?? src/axon-core/src/graph_bootstrap.rs
?? src/axon-core/src/graph_ingestion.rs
?? src/axon-core/src/graph_query.rs
?? src/axon-core/src/main_background.rs
?? src/axon-core/src/main_services.rs
?? src/axon-core/src/main_telemetry.rs
?? src/axon-core/src/mcp/
```

Interpretation of the current snapshot:

- `.devenv/*` changes are mostly runtime artifacts from Devenv execution.
- multiple `SOLL_EXPORT_*.md` files are considered legitimate historical exports of the `SOLL` conceptual layer and are intentionally kept.
- HydraDB should now be considered detached from the active Devenv workflow unless explicitly reintroduced later.

Re-check current Git state before acting.

# Resume Checklist

When resuming after compaction, do this in order:

1. Read this file completely.
2. Run `git status --short --branch`.
3. Run `git diff --stat`.
4. Re-check the branch is still `feat/axon-stabilization-continuation`.
5. Re-run Rust validation:
   - `cd src/axon-core && cargo test`
6. Continue the interrupted dashboard validation:
   - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
7. If both are green, continue with:
   - operator workflow truthfulness
   - MCP usefulness for LLM development
   - SOLL export / restore reliability
   - progressive refactoring of the dashboard watcher layer

# Recommended Next Steps

Primary next step:

1. keep `cargo test` green after the completed `mcp.rs` split
2. continue refactoring `server.ex` and `pool_facade.ex` by responsibility without changing operator-visible behavior
3. execute the new validation en conditions reelles checklist on Axon itself and record evidence
4. continue improving MCP usefulness for LLM-assisted development

Secondary next step:

5. keep aligning dashboard actions and MCP outputs with real value for LLM-assisted development

Method skill already created:

- `/home/dstadel/projects/axon/.claude/skills/reality-first-stabilization/SKILL.md`

# Anti-Drift Rule

After compaction, do not trust any summary blindly, including this one.

Use this file as a map, then verify the code and runtime state directly.
