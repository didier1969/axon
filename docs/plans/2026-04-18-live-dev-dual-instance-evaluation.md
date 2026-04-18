# Live/Dev Dual Instance Evaluation

## Goal

Evaluate the current Axon runtime and script surface against the target architecture:

- one stable `live` Axon instance, using the current live ports and current populated databases;
- one isolated `dev` Axon instance for development, migration, qualification, and break/fix work;
- same MCP surface on both instances;
- explicit runtime identity so an LLM never has to guess which instance it is using.

This document is an evaluation of the current state and of earlier attempts. It is not yet the implementation plan.

## Baseline Concept

The target concept is already documented in:

- [2026-04-18-live-dev-dual-instance-concept.md](/home/dstadel/projects/axon/docs/architecture/2026-04-18-live-dev-dual-instance-concept.md)

Historical predecessor:

- [2026-04-05-dual-track-environment.md](/home/dstadel/projects/axon/docs/architecture/2026-04-05-dual-track-environment.md)

## Executive Verdict

Axon already contains a **partial dual-track runtime scaffold**, but it is not a true dual-instance architecture yet.

What exists today is mostly:

- a `prod|dev` port switch in startup/shutdown scripts;
- a worktree-oriented development convention;
- some historical documentation for a `dev` runtime;
- many other scripts and runtime assumptions still hard-coded to the live instance.

The result is an architecture that can look dual-track from the outside, while still behaving operationally like a **single authoritative runtime with an optional shadow variant**.

That is why previous attempts likely did not carry through.

## Findings

### 1. There is already a real seed of dual-track support

Current scripts already contain a `prod|dev` split:

- [scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh)
- [scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh)

Observed behavior:

- `AXON_ENV=prod`
  - dashboard `44127`
  - HTTP/MCP `44129`
  - tmux session `axon`
  - Elixir node `axon_nexus`
- `AXON_ENV=dev`
  - dashboard `44137`
  - HTTP/MCP `44139`
  - tmux session `axon-dev`
  - Elixir node `axon_dev_nexus`

This is useful. It means Axon does not start from zero on this subject.

### 2. The split is still hidden and convention-based

The current split depends mainly on:

- `.env.worktree`
- `AXON_ENV`
- current working directory conventions

That makes the runtime distinction too implicit.

For a durable operator and LLM architecture, this is insufficient:

- the selected instance is not explicit enough at the protocol level;
- the routing depends too much on shell context;
- the operational identity is not strongly self-describing.

### 3. Scripts still assume the live endpoint as the default truth

Many scripts still default to `44129` or to the repository root directly.

Examples:

- [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- [scripts/mcp_validate.py](/home/dstadel/projects/axon/scripts/mcp_validate.py)
- [scripts/qualify_mcp.py](/home/dstadel/projects/axon/scripts/qualify_mcp.py)
- [scripts/measure_mcp_suite.py](/home/dstadel/projects/axon/scripts/measure_mcp_suite.py)
- [scripts/qualify_mcp_guidance.py](/home/dstadel/projects/axon/scripts/qualify_mcp_guidance.py)
- [scripts/work_plan.py](/home/dstadel/projects/axon/scripts/work_plan.py)
- [scripts/soll_import.py](/home/dstadel/projects/axon/scripts/soll_import.py)

This means the toolchain still treats `live` as the default singleton runtime.

### 3b. Process targeting is still singleton in practice

This is not only a ports problem.
It is also a process identity problem.

[scripts/stop.sh](/home/dstadel/projects/axon/scripts/stop.sh) still identifies Axon processes mainly from:

- the current `PROJECT_ROOT`
- the current binary path
- generic patterns around `axon-core`

[scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh) still uses a generic:

- `pgrep -af axon-core`

This is too weak for a true dual-instance architecture if both `live` and `dev` can run from the same checkout.

Without a per-instance process identity, one instance can accidentally report on, stop, or interfere with the other.

### 4. `start.sh` contains a mixed state: partial split, incomplete isolation

[scripts/start.sh](/home/dstadel/projects/axon/scripts/start.sh) currently does two contradictory things:

- it switches ports between `prod` and `dev`;
- but it still performs several actions as if the runtime were singular.

Concrete issues:

- it always removes `/tmp/axon-telemetry.sock` and `/tmp/axon-mcp.sock`
  - these socket paths are not instance-specific
- it always removes locks under `$PROJECT_ROOT/.axon/graph_v2`
  - this assumes state rooted under the current repo, not under an explicit instance data root
- the final printed URLs are hard-coded to `44127` and `44129`
  - even in `dev`
- it still loads binaries from the current repo tree, not from an explicit runtime install root

This is a critical reason prior attempts were fragile:

- the ports diverged;
- the state identity did not.

### 5. Runtime state is only partially parameterized

The Rust runtime already supports a configurable database root:

- [src/axon-core/src/main.rs](/home/dstadel/projects/axon/src/axon-core/src/main.rs)
- [src/axon-core/src/graph_bootstrap.rs](/home/dstadel/projects/axon/src/axon-core/src/graph_bootstrap.rs)

`GraphStore::new(db_root)` creates:

- `ist.db`
- `soll.db`

under the provided root.

This is very good news. It means the data split does **not** require a fundamental storage redesign.

However:

- `main.rs` still hard-codes:
  - `/tmp/axon-telemetry.sock`
  - `/tmp/axon-mcp.sock`
- `HYDRA_HTTP_PORT` still defaults to `44129`

So the runtime is ready for a dual database root, but not yet for a full dual runtime identity.

Important nuance:

- the existing live root must be treated as the current stable truth;
- moving the live database root is a separate migration problem;
- dualization should not silently relocate the live instance by default.

### 6. The protocol does not yet expose the runtime identity we need

The concept requires `status` to expose at least:

- `instance_kind`
- `runtime_identity`
- `data_root`
- `project_root`
- `mutation_policy`
- `mcp_url`

Current MCP `status` diagnostics are rich, but they do not yet expose this minimal runtime identity contract explicitly.

This is the main missing piece for LLM safety:

- today, an agent can infer too much from convention;
- tomorrow, it must be able to verify the target instance from protocol truth alone.

### 6b. `status.sh` and MCP `status` are currently too easy to confuse

Axon currently has two different notions of “status”:

- shell/operator probe:
  - [scripts/status.sh](/home/dstadel/projects/axon/scripts/status.sh)
- MCP protocol tool:
  - [src/axon-core/src/mcp/tools_framework.rs](/home/dstadel/projects/axon/src/axon-core/src/mcp/tools_framework.rs)

The target architecture requires both, but with a strict separation of meaning:

- `status` MCP
  - protocol truth for LLMs and MCP clients
- `status.sh`
  - local lifecycle and health probe for operators

This distinction must be made explicit during implementation.

### 7. The engineering skill is currently incompatible with the new target

[docs/skills/axon-engineering-protocol/SKILL.md](/home/dstadel/projects/axon/docs/skills/axon-engineering-protocol/SKILL.md) currently says:

- production MCP is the server on `44129`
- do not start a second MCP server from a worktree or sandbox

That was coherent for a single-authority runtime.
It is not coherent anymore with the target dual-instance architecture.

So the migration is not just technical:

- it also requires a protocol and operator doctrine update.

### 8. The historical `sync_to_dev.sh` approach is too narrow

[scripts/sync_to_dev.sh](/home/dstadel/projects/axon/scripts/sync_to_dev.sh) is tightly coupled to:

- one fixed worktree path
- only `ist.db`
- ad hoc file copying

That is useful as a historical clue, but not sufficient as a canonical mechanism for the new architecture.

For the target design, synchronization must become:

- explicit;
- instance-aware;
- able to cover both `ist.db` and `soll.db`;
- safe around WAL and running processes;
- decoupled from one hard-coded worktree.

It must also stop being a loose file copy pattern.
For DuckDB-backed state, the seed operation must be defined as a coherent snapshot workflow with explicit preconditions.

## Why Earlier Attempts Likely Failed

The old direction was not wrong, but it was incomplete in four structural ways:

1. It treated the problem mainly as a **worktree problem**, not as a **runtime identity problem**.
2. It separated ports, but not all state-bearing resources:
   - sockets
   - printed URLs
   - tool defaults
   - state roots
3. It did not make the distinction explicit in the MCP protocol.
4. It did not turn the operator scripts into a first-class `live|dev` control plane.

The effect was predictable:

- development isolation existed only partially;
- the live runtime remained the de facto center of gravity;
- the mental model stayed ambiguous.

## What Is Reusable

The following pieces are strong and should be reused:

- `AXON_DB_ROOT` support in the Rust runtime
- `prod|dev` port families in `start.sh` and `stop.sh`
- distinct tmux sessions and Elixir node names
- the new concept document
- the already-unified qualification direction:
  - [scripts/qualify_mcp.py](/home/dstadel/projects/axon/scripts/qualify_mcp.py)
  - [scripts/axon](/home/dstadel/projects/axon/scripts/axon)

However, those qualification/operator entrypoints are not yet dual-instance-ready.
They are reusable as a direction and as structure, not yet as a final live/dev control plane.

## What Must Change

The following must change for the architecture to become real:

1. instance identity must be explicit and parameterized everywhere;
2. state roots must be separated and canonicalized;
3. sockets must be instance-specific;
4. operator wrappers must target a selected instance explicitly;
5. `status` must expose runtime identity;
6. qualification scripts must be instance-aware by default;
7. the skill/protocol must stop assuming that `44129` is the only valid Axon authority;
8. process identity must become instance-specific, not just port-specific.

## Final Evaluation

The current codebase is **close enough to make this feasible without a rewrite**, but **far enough from the target that a disciplined implementation plan is required**.

This is not a greenfield design problem.
It is a consolidation and completion problem.

That is favorable:

- the needed architecture is achievable;
- the existing base can be reused;
- but the migration must be explicit, rigorous, and staged.
