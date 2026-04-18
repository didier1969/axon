# Live/Dev Dual Instance Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn Axon's partial `prod|dev` split into a real dual-instance architecture with one stable `live` runtime and one isolated `dev` runtime, both exposing the same MCP surface.

**Architecture:** Keep one Axon protocol and one codebase, but make runtime identity first-class. Separate instance roots, ports, sockets, process identity, wrappers, and status metadata. Reuse the existing `AXON_DB_ROOT` and `prod|dev` port split, then remove remaining singleton assumptions.

**Tech Stack:** Bash scripts, Python qualification scripts, Rust runtime (`axon-core`), tmux, DuckDB-backed IST/SOLL.

---

## Design Rules

The implementation must preserve these invariants:

1. Same MCP surface on `live` and `dev`.
2. `live` and `dev` never share:
   - `ist.db`
   - `soll.db`
   - WAL
   - sockets
   - tmux session
   - port family
   - pidfiles / runtime markers
3. An LLM must be able to prove which instance it is talking to from `status`.
4. Operator scripts must select an instance explicitly, not infer it from worktree magic alone.
5. `live` remains usable as the stable truth runtime.
6. `dev` remains free to evolve, migrate, reset, and qualify without impacting `live`.
7. The current live database root remains authoritative unless a separate migration explicitly changes it.
8. `status` MCP and `status.sh` must be distinct but consistent:
   - MCP `status` = protocol truth
   - `status.sh` = local lifecycle probe
9. `live` must announce a promotion-safe version identity:
   - `release_version`
   - `package_version`
   - `build_id`
   - `install_generation`
10. A GitHub push is never sufficient to mark a version as production:
   - `pushed`
   - `qualified`
   - `promoted`
   must remain distinct lifecycle states.

## Phase 0: Establish the Canonical Control Plane

### Task 0.1: Define the canonical façade and deprecation path

**Files:**
- Modify: [scripts/axon](/home/dstadel/projects/axon/scripts/axon)
- Update docs in the plan and operator notes

**Intent:**
Prevent two competing operator models.

**Decision to encode explicitly:**
- either `scripts/axon` becomes instance-aware and remains canonical;
- or `scripts/axon-live` / `scripts/axon-dev` become canonical and `scripts/axon` is explicitly deprecated for lifecycle commands.

**Acceptance criteria:**
- there is one documented primary control plane;
- no operator or LLM workflow needs to guess which family of commands is canonical.

### Task 0.2: Introduce a shared instance resolver

**Files:**
- Create: `scripts/lib/axon-instance.sh`
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- Modify: [scripts/axon](/home/dstadel/projects/axon/scripts/axon)

**Intent:**
Resolve instance identity once, then reuse it everywhere.

**Required outputs from the resolver:**
- `AXON_INSTANCE_KIND=live|dev`
- `AXON_RUNTIME_IDENTITY`
- `AXON_DB_ROOT`
- `AXON_RUN_ROOT`
- `AXON_TELEMETRY_SOCK`
- `AXON_PID_FILE`
- `AXON_MCP_URL`
- `AXON_SQL_URL`
- `AXON_DASHBOARD_URL`
- `AXON_MUTATION_POLICY`
- `TMUX_SESSION`

**Acceptance criteria:**
- `start`, `stop`, `restart`, `status`, and qualification wrappers all derive instance identity from the same helper;
- no lifecycle script duplicates instance-resolution logic.

## Phase 1: Define the Runtime Identity Contract

### Task 1: Add canonical instance environment variables

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)

**Intent:**
Replace the current hidden split with explicit runtime identity variables.

**Required variables:**
- `AXON_INSTANCE_KIND=live|dev`
- `AXON_RUNTIME_IDENTITY`
- `AXON_DB_ROOT`
- `AXON_RUN_ROOT`
- `AXON_TELEMETRY_SOCK`
- `AXON_MCP_URL`
- `AXON_SQL_URL`
- `AXON_DASHBOARD_URL`
- `AXON_MUTATION_POLICY`
- `AXON_PID_FILE`

**Acceptance criteria:**
- the runtime can boot from explicit instance variables;
- the scripts no longer need `.env.worktree` as the primary switch;
- `live` and `dev` can be selected by contract, not by directory convention.

### Task 2: Make runtime markers and sockets instance-specific

**Files:**
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)

**Intent:**
Remove singleton runtime assumptions.

**Target shape:**
- `live`
  - `/tmp/axon-live-telemetry.sock`
  - instance pidfile / run marker under a live run root
- `dev`
  - `/tmp/axon-dev-telemetry.sock`
  - instance pidfile / run marker under a dev run root

**Important note:**
The current runtime actually binds the telemetry Unix socket, but does not expose a distinct MCP Unix socket.
Do not formalize `AXON_MCP_SOCK` unless that socket is intentionally implemented.

**Acceptance criteria:**
- starting `dev` does not delete `live` sockets;
- stop/status target the correct instance via pidfile/run marker, not generic `pgrep axon-core`.

### Task 3: Expose runtime identity in `status`

**Files:**
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)
- Test: [src/axon-core/src/mcp/tests.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tests.rs)

**Intent:**
Make instance identity protocol-visible.

**Fields to expose:**
- `instance_kind`
- `runtime_identity`
 - `data_root`
 - `run_root`
- `project_root`
- `mcp_url`
- `sql_url`
- `dashboard_url`
- `mutation_policy`
- `release_version`
- `package_version`
- `build_id`
- `install_generation`

**Acceptance criteria:**
- `status` returns a compact identity block;
- any LLM can verify whether it is on `live` or `dev` without guessing.

### Task 3b: Add production version identity to the runtime contract

**Files:**
- Modify: `scripts/lib/axon-version.sh`
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)
- Modify: [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)

**Intent:**
Make the running instance promotion-identifiable.

**Required outputs:**
- `AXON_RELEASE_VERSION`
- `AXON_PACKAGE_VERSION`
- `AXON_BUILD_ID`
- `AXON_INSTALL_GENERATION`

**Acceptance criteria:**
- `start.sh` prints the selected version identity;
- `status.sh` shows the selected version identity;
- MCP `status` returns a compact `runtime_version` block;
- `live` can later be promoted by replacing a known version with another known version.

## Phase 1b: Define the Release Qualification and Promotion Contract

### Task 3c: Formalize release lifecycle states

**Files:**
- Update: [docs/architecture/2026-04-18-live-dev-dual-instance-concept.md](/home/dstadel/projects/axon/docs/architecture/2026-04-18-live-dev-dual-instance-concept.md)
- Update: this plan

**Intent:**
Prevent “push to GitHub” from being confused with “safe for production”.

**Required states:**
- `pushed`
- `qualified`
- `promoted`

**Acceptance criteria:**
- the docs explicitly state that `push != qualified != promoted`;
- the future promotion flow is described as a separate explicit operation.

### Task 3d: Define the promotion inputs and outputs

**Files:**
- Future implementation files to add:
  - `scripts/release/create_manifest.py` or equivalent
  - `scripts/release/promote_live.sh` or equivalent
  - `scripts/release/rollback_live.sh` or equivalent

**Intent:**
Make `live` upgradeable by a controlled and optionally automated cycle.

**Required promotion inputs:**
- immutable tag or exact commit
- immutable installable artifact identity
- `release_version`
- `build_id`
- release manifest
- qualification evidence bundle

**Required promotion outputs:**
- updated live installation
- incremented `install_generation`
- post-promotion verification report

**Acceptance criteria:**
- promotion is modeled as a separate workflow from `git push`;
- `live` can prove what exact qualified version was installed;
- rollback target is explicit.

### Task 3e: Add a promotion gate to the future release cycle

**Intent:**
Ensure only qualified versions may replace `live`.

**Required future checks before promotion:**
- qualification gates pass
- manifest matches the candidate build
- manifest identifies the exact installable artifact and its digest/checksum
- `release_version` and `build_id` are stable
- post-check on `live` matches the manifest after install

**Acceptance criteria:**
- the future plan contains a dedicated promotion gate;
- no workflow marks a version “production” based only on GitHub state.

### Task 3f: Define rollback compatibility for live data/schema

**Intent:**
Prevent code-only rollback promises from masking non-rollbackable live migrations.

**Required rule:**
Every promotable release must explicitly declare one of:

- `backward_compatible_live_state = true`
- or a rollback data plan:
  - required snapshot/backup
  - rollback procedure
  - known irreversible boundaries

**Acceptance criteria:**
- the promotion contract covers both artifact rollback and data/schema rollback semantics;
- no release is considered safely promotable without a stated live-state rollback posture.

## Phase 2: Canonicalize State Roots

### Task 4: Define canonical data roots for `live` and `dev`

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)
- Modify: [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)

**Intent:**
Stop rooting runtime state implicitly under the current repo.

**Required rule:**
- preserve the current live root as the authoritative live storage unless a separate, explicit migration is performed.

**Recommended target for `dev`:**
- `/home/dstadel/projects/axon/.axon-dev/graph_v2`

**Live target:**
- current effective live root, explicitly resolved and reported
- do not silently relocate it in this project

**Optional later migration target for `live`:**
- `live`
  - `/home/dstadel/projects/axon/.axon-live/graph_v2`

That later migration must be treated as a separate, rollback-capable operation if ever pursued.

**Acceptance criteria:**
- `AXON_DB_ROOT` is set from the selected instance;
- lock/WAL cleanup is limited to the selected instance root;
- live and dev databases are physically distinct.

### Task 5: Stop hard-coded URL reporting

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)

**Intent:**
Ensure operator feedback reflects the selected runtime.

**Acceptance criteria:**
- `start.sh` prints the actual instance URLs;
- `status.sh` defaults to the selected instance or a passed instance selector.

## Phase 3: Create an Explicit Operator Control Plane

### Task 6: Add canonical instance wrappers

**Files:**
- Create: `scripts/axon-live`
- Create: `scripts/axon-dev`
- Modify: [scripts/axon](/home/dstadel/projects/axon/scripts/axon)

**Intent:**
Give operators and LLM tooling explicit entrypoints.

**Wrapper behavior:**
- `axon-live`
  - exports live instance variables
  - forwards to `./scripts/axon`
- `axon-dev`
  - exports dev instance variables
  - forwards to `./scripts/axon`

**Acceptance criteria:**
- both wrappers address distinct endpoints predictably;
- no operator needs to remember port numbers manually.

### Task 7: Add lifecycle wrappers

**Files:**
- Create: `scripts/start-live.sh`
- Create: `scripts/start-dev.sh`
- Create: `scripts/stop-live.sh`
- Create: `scripts/stop-dev.sh`
- Create: `scripts/status-live.sh`
- Create: `scripts/status-dev.sh`

**Intent:**
Replace environment guesswork with named lifecycle commands.

**Acceptance criteria:**
- lifecycle commands are instance-explicit;
- they can coexist without ambiguity.

### Task 7b: Migrate existing `scripts/axon` lifecycle commands

**Files:**
- Modify: [scripts/axon](/home/dstadel/projects/axon/scripts/axon)
- Update related docs

**Intent:**
Make sure the existing primary operator entrypoint does not remain ambiguously singleton.

**Scope:**
- `start`
- `stop`
- `restart`
- `status`
- `quality-mcp`
- any other subcommand that shells into runtime-specific scripts

**Acceptance criteria:**
- either `scripts/axon` becomes instance-aware for these commands;
- or it explicitly defers users to the canonical live/dev wrappers.

## Phase 4: Replace the Old Sync Mechanism with a Real Dev Seed Flow

### Task 8: Design and implement explicit live-to-dev seed script

**Files:**
- Replace or supersede: [scripts/sync_to_dev.sh](/home/dstadel/projects/axon/scripts/sync_to_dev.sh)
- Create: `scripts/seed-dev-from-live.sh`
- Update docs accordingly

**Intent:**
Turn the old hard-coded copy script into a canonical seeding tool.

**Required behavior:**
- seed `dev` from `live` explicitly;
- support both `ist.db` and `soll.db`;
- define a coherent DuckDB snapshot workflow, not an opportunistic file copy;
- refuse unsafe copy conditions unless explicitly allowed;
- avoid hard-coded worktree paths.

**Minimum safety contract:**
- explicit precondition check on `live` and `dev`
- checkpoint/quiescence rule clearly defined
- copy behavior defined for:
  - `ist.db`
  - `soll.db`
  - WAL only when the snapshot contract says it is valid
- post-seed verification on `dev`

**Acceptance criteria:**
- developers can refresh dev state from live intentionally;
- the process is instance-aware and documented.

## Phase 5: Make Qualification Scripts Instance-Aware

### Task 9: Parameterize qualification defaults by selected instance

**Files:**
- Modify: [scripts/qualify_mcp.py](/home/dstadel/projects/axon/scripts/qualify_mcp.py)
- Modify: [scripts/mcp_validate.py](/home/dstadel/projects/axon/scripts/mcp_validate.py)
- Modify: [scripts/measure_mcp_suite.py](/home/dstadel/projects/axon/scripts/measure_mcp_suite.py)
- Modify: [scripts/qualify_mcp_guidance.py](/home/dstadel/projects/axon/scripts/qualify_mcp_guidance.py)
- Modify: [scripts/qualify_mcp_robustness.py](/home/dstadel/projects/axon/scripts/qualify_mcp_robustness.py)
- Modify: [scripts/work_plan.py](/home/dstadel/projects/axon/scripts/work_plan.py)
- Modify: [scripts/soll_import.py](/home/dstadel/projects/axon/scripts/soll_import.py)

**Intent:**
Stop treating `44129` as the only meaningful default.

**Acceptance criteria:**
- scripts can run against `live` or `dev` without manual URL editing;
- wrappers provide the correct defaults cleanly.

### Task 10: Add qualification scenarios for dual-instance correctness

**Files:**
- Create or modify qualification scenario files under `scripts/mcp_scenarios/`
- Update qualification docs

**Intent:**
Verify the architecture itself, not only the MCP tools.

**Checks to add:**
- `status` on `live` reports `instance_kind=live`
- `status` on `dev` reports `instance_kind=dev`
- live and dev `data_root` differ
- live and dev `mcp_url` differ
- live and dev pidfiles / run roots differ
- live and dev can run concurrently

## Phase 6: Update the Protocol Early Enough for Safe Use

### Task 11: Update the engineering skill

**Files:**
- Modify: [docs/skills/axon-engineering-protocol/SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md)

**Intent:**
Replace the old single-production rule with a dual-instance runtime rule before publicizing the new wrappers.

**Required doctrinal change:**
- `status` remains first truth surface;
- `live` and `dev` are both valid Axon authorities;
- the user or workflow must select the intended instance explicitly;
- development work must not mutate `live` implicitly.
- clarify:
  - MCP `status` = protocol truth
  - `status.sh` = local lifecycle probe

### Task 12: Add operator documentation

**Files:**
- Create: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Update: [docs/getting-started.md](/home/dstadel/projects/axon/docs/getting-started.md) if needed

**Intent:**
Document:
- when to use `live`
- when to use `dev`
- how to seed `dev`
- how to verify the selected instance
- how to qualify both runtimes

## Phase 7: Deprecate the Legacy Hidden Split

### Task 13: Keep compatibility briefly, then de-emphasize `.env.worktree`

**Files:**
- Modify: [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- Modify: [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)
- Update docs

**Intent:**
Support transition without keeping the old model as the primary path.

**Acceptance criteria:**
- `.env.worktree` can remain as compatibility input temporarily;
- official docs and wrappers no longer depend on it.

## Validation Matrix

The implementation is complete only when all of the following are true:

1. `live` and `dev` can run at the same time.
2. `status` proves which instance is targeted.
3. `live` and `dev` have different:
   - ports
   - sockets
   - tmux sessions
   - DB roots
   - pidfiles / run roots
4. qualification scripts can target both instances cleanly.
5. operator wrappers make instance selection explicit.
6. no default script path silently kills or reports on the other instance.

## Risks

### Risk 1: Half-migrated singleton assumptions

Mitigation:
- audit every script default touching:
  - `44129`
  - `44127`
  - `/tmp/axon-*.sock`
  - `.axon/graph_v2`

### Risk 2: False sense of isolation

Mitigation:
- qualify concurrent `live+dev` startup explicitly;
- verify database roots, run roots, and sockets differ in tests and in `status`.

### Risk 3: Process targeting remains ambiguous

Mitigation:
- use pidfiles / run markers per instance;
- eliminate generic `pgrep axon-core` logic from canonical lifecycle flows.

### Risk 4: LLM/operator confusion persists

Mitigation:
- wrappers
- explicit `status`
- skill update
- no ambiguous primary alias once both runtimes are active

### Risk 5: Unsafe DuckDB seed semantics

Mitigation:
- define the seed flow as a coherent snapshot operation with explicit preconditions;
- do not use loose file-copy semantics as the canonical live-to-dev workflow.

## Recommended Execution Order

1. Canonical façade and shared instance resolver
2. Runtime identity variables + process identity + telemetry socket split
3. MCP `status` identity payload and `status.sh` contract separation
4. Canonical state roots without silently moving live
5. Lifecycle scripts and current `scripts/axon` migration
6. Qualification script parameterization
7. Instance wrappers
8. Seed-dev-from-live flow
9. Skill and operator docs
10. Compatibility cleanup

## Out of Scope for This Plan

This plan does not yet decide:

- whether `live` should run as `systemd`, tmux only, or another service manager;
- whether `live` mutations should become read-only by default immediately;
- how promotion from `dev` to `live` should be automated operationally.

Those can be layered after the dual-instance architecture itself is stable.
