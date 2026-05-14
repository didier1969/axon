# Graph-First Simplification Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Simplify Axon around the two canonical lanes `to_graph` then `to_vector`, while preserving the complexity that still provides real operational value.

**Architecture:** This tranche is a simplification tranche, not a rewrite. It keeps the split authority model, the explicit queue/state model, and the role-only qualification paths, then removes or freezes the surplus control-plane and multi-truth machinery that no longer improves graph-first throughput, safety, or observability.

**Tech Stack:** Rust (`axon-core`), Bash operator scripts, Python qualification scripts, Elixir/Phoenix dashboard, tmux-based dev runtime orchestration.

---

## Delivery Discipline

This tranche inherits the master plan rules and adds one hard invariant:

- `to_graph` is always higher priority than `to_vector`
- any mechanism that can slow `to_graph` in favor of `to_vector` is suspect by default
- complexity is preserved only if it clearly improves:
  - correctness
  - recovery
  - operator clarity
  - measured throughput

## Keep / Freeze / Remove Contract

### Keep

- split authority:
  - `brain` = MCP/dashboard/`SOLL`
  - `indexer` = filesystem/discovery/`IST`
- explicit persistence surfaces:
  - `File`
  - `GraphProjectionQueue`
  - `FileVectorizationQueue`
- ingress buffering and batched promotion
- local-first project identity and filter resolution
- `indexer` canonical runtime telemetry
- role-only baseline and qualification paths
- `IST` reader replica for `brain`

### Freeze

- Elixir dashboard as transitional read-only projection shell
- rich MCP/operator diagnostics
- release/outillage paths not on the critical graph-first path

### Simplify / Remove

- overlapping pipeline control layers
- public/runtime remnants of control-plane proxy behavior
- qualification paths that depend on HTML cockpit or rich MCP by default
- dashboard recomposition of multiple truths when Rust telemetry is already sufficient

## Tranche Scope

This simplification tranche is split into four closed sub-tranches.

### Sub-tranche A: Freeze The Canonical Runtime Contract

**Purpose:** Document and protect the pieces we are explicitly not simplifying away.

**Files:**
- Modify: `docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md`
- Modify: `docs/plans/2026-04-22-brain-indexer-cross-dependency-audit.md`
- Modify: `docs/plans/2026-04-22-elixir-dashboard-value-audit-and-rust-first-decision-plan.md`
- Add references to this plan where appropriate

**Implementation steps:**

1. Record the keep/freeze/remove contract from the audit.
2. State explicitly that queue/state persistence and role-only telemetry are protected.
3. State explicitly that graph-first priority overrides vector-first heuristics.
4. Record the components that are only transitional:
   - Elixir dashboard
   - rich MCP/operator diagnostics
   - non-essential release/control surfaces

**Exit criteria:**
- The simplification contract is written down before code is touched.
- Future edits can be judged against a fixed graph-first standard.

### Sub-tranche B: Reduce Pipeline Governance To One Readable Contract

**Purpose:** Collapse overlapping pipeline-control machinery so the runtime clearly expresses:
- `to_graph` first
- `to_vector` second

**Files:**
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/vector_control.rs`
- Modify if needed: `src/axon-core/src/optimizer.rs`
- Modify tests in:
  - `src/axon-core/src/mcp/tests.rs`
  - `src/axon-core/src/tests/maillon_tests.rs`

**Audit findings driving this tranche:**
- too many active governors currently influence the same flow:
  - admission controller
  - claim policy
  - runtime priority contract
  - utility-first scheduler
  - optimizer heuristics

**Implementation steps:**

1. Keep one canonical contract for upstream:
   - watcher/discovery admission
   - persisted pending
   - graph work in progress
2. Keep one canonical contract for downstream:
   - vector work opens only when graph reserve conditions are satisfied
3. Remove or bypass overlapping decision layers that do not change outcomes materially.
4. Ensure the graph backlog is the dominant gating signal for vectorization.
5. Either:
   - demote the live optimizer out of the hot path
   - or freeze it behind a no-op/default-safe path unless explicitly enabled
6. Keep telemetry fields that describe:
   - current blocking authority
   - current graph backlog
   - current vector backlog
   - whether vector is currently held behind graph

**Validation:**
- targeted Rust tests around:
  - graph backlog blocks vector priority
  - vector opens only when graph reserve is satisfied
  - graph production state stays readable
- role-only cold qualification:
  - `bash scripts/qualify-dev-indexer-cold.sh --duration 20 --interval 5 --label graph-first-governance`

**Exit criteria:**
- There is one readable graph-first control contract.
- Vectorization cannot outrun graph backlog by policy accident.
- The hot path no longer depends on several overlapping governors.

### Sub-tranche C: Reduce Status And Qualification To One Machine Truth

**Purpose:** Simplify operator/runtime truth so qualification and status do not depend on polymorphic fallbacks and rich projections by default.

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify if needed: `src/axon-core/src/mcp/tools_system.rs`
- Modify: `scripts/qualify_ingestion_run.py`
- Modify if needed: `scripts/status.sh`
- Modify if needed: `scripts/lib/dev-baseline.sh`

**Implementation steps:**

1. Reduce the machine status core to:
   - authority
   - freshness
   - ingress counts
   - graph queue counts
   - vector queue counts
   - dominant blocking authority
2. Keep rich operator diagnostics behind explicit opt-in paths.
3. Remove HTML cockpit scraping from default qualification flow.
4. Keep `status_json` / role-local heartbeat as the default qualification source.
5. Preserve split visibility, but do not let it distort role-only truth.

**Validation:**
- `python3 -m py_compile scripts/qualify_ingestion_run.py`
- targeted Rust/MCP tests for compact status truth
- role-only and split qualification smoke runs

**Exit criteria:**
- Qualification is anchored to one machine-readable truth per role.
- Default status/qualification no longer depend on HTML cockpit parsing.
- Rich diagnostics remain available, but no longer sit on the critical path.

### Sub-tranche D: Bound The Dashboard To Read-Only Rust Projection

**Purpose:** Keep the dashboard useful without letting it remain a second truth engine.

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/telemetry.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Modify if needed: `src/dashboard/lib/axon_nexus/axon/watcher/progress.ex`
- Modify if needed: `src/dashboard/lib/axon_dashboard/bridge_client.ex`
- Modify tests under:
  - `src/dashboard/test/axon_dashboard/`
  - `src/dashboard/test/axon_dashboard_web/live/`
  - `src/dashboard/test/axon_nexus/axon/watcher/`

**Implementation steps:**

1. Keep bridge-fed Rust telemetry as the primary dashboard input.
2. Reduce dashboard-local recomposition to presentation-only where possible.
3. Isolate or demote SQL snapshot rebuilding that is not needed for graph-first operator visibility.
4. Make freshness/source visibly explicit in the UI.
5. Preserve useful operator sections only if they rely on authoritative Rust data or clearly labeled derived data.

**Validation:**
- `mix test` on touched dashboard tests
- dashboard still renders graph-first operational metrics correctly

**Exit criteria:**
- The dashboard is clearly a read-only projection shell.
- Operator visibility remains good.
- Multi-truth recomposition is reduced.

## Definition Of Done

This simplification tranche is done when:

- the graph-first invariant is explicitly encoded in code and docs
- overlapping pipeline governors are reduced to one readable contract
- qualification defaults to machine-readable role truth, not dashboard scraping
- the dashboard remains useful but bounded to read-only Rust projection
- no justified queue/state/recovery mechanism was removed merely for aesthetic simplicity

## Out Of Scope

- full `IST/indexer` module refactor
- full Rust-first dashboard rewrite
- release-chain simplification beyond what blocks graph-first operability
- schema-level removal of queue tables

## Expected Result

After this tranche, Axon should be easier to reason about as a machine with two lanes:

1. `filesystem -> discovery -> pending -> graph_ready`
2. `graph_ready -> vector_ready`

And the surrounding operator/tooling stack should describe that machine, not compete with it.
