# Elixir Dashboard Value Audit And Rust-First Decision Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Decide whether Axon should keep Elixir/Phoenix/LiveView as its operator dashboard surface or move toward a Rust-first dashboard, based on current runtime reality rather than historical architecture.

**Architecture:** This tranche is a decision-and-proof tranche, not a speculative rewrite. It audits the real responsibilities still held by the Elixir dashboard, compares them to a Rust-first alternative, and produces an explicit recommendation with follow-up implementation tranches. The target is to avoid investing further in a heavy UI/control shell if it no longer has a justified mission.

**Tech Stack:** Rust (`axon-core`), Elixir/Phoenix/LiveView (`src/dashboard`), bridge telemetry socket, runtime heartbeat JSON, MCP/status surfaces, Bash operator scripts, plan/audit docs.

---

## Current-State Audit

### Observed Elixir Responsibilities Today

| Responsibility | Current owner | Classification | Notes |
| --- | --- | --- | --- |
| Ingestion / scheduling / queue control | Rust | runtime-critical | Elixir no longer owns this and tests explicitly forbid bringing it back. |
| `IST` write authority | Rust | runtime-critical | `indexer` is the sole writer. |
| `SOLL` write authority | Rust `brain` | runtime-critical | Elixir does not own mutation authority. |
| Public MCP surface | Rust `brain` | runtime-critical | Elixir is not the product API surface. |
| Runtime telemetry production | Rust | runtime-critical | Emitted from `main_telemetry.rs` and written into heartbeat JSON / bridge events. |
| Dashboard HTTP server / LiveView UI | Elixir | operator-facing only | Real remaining value today. |
| Telemetry socket consumption | Elixir | read-only projection | `BridgeClient` consumes Rust events and republishes them into PubSub/ETS. |
| ETS runtime cache | Elixir | duplicate projection | Stores a second copy of runtime data for LiveView rendering. |
| SQL snapshot summarization | Elixir | read-only projection | `Progress` composes dashboard-facing workspace summaries from SQL. |
| Legacy control-plane, queue control, mutable overlays | none | explicitly forbidden | Protected by tests that assert these surfaces are gone. |

### Evidence

- `src/dashboard/README.md`
  - explicitly describes the dashboard as a read-only operator surface for the Rust runtime
- `src/dashboard/lib/axon_dashboard/application.ex`
  - starts PubSub, tracer, telemetry store, repo, bridge client, and endpoint
  - does not start ingestion authority
- `src/dashboard/test/axon_dashboard/legacy_control_plane_boundary_test.exs`
  - explicitly forbids Oban, command bridge methods, mutable overlays, and several legacy modules
- `src/dashboard/test/axon_dashboard/application_visualization_test.exs`
  - asserts the dashboard supervisor does not boot canonical ingestion authority
- `src/dashboard/lib/axon_dashboard/bridge_client.ex`
  - consumes Rust `RuntimeTelemetry` and republishes it into Elixir-local telemetry state
- `src/dashboard/lib/axon_nexus/axon/watcher/progress.ex`
  - rebuilds dashboard snapshots from SQL as a read-side summarization layer

## Cost Audit

### Costs We Pay To Keep Elixir/Phoenix/LiveView

1. Extra runtime/process surface
   - Phoenix endpoint
   - bridge client
   - PubSub
   - ETS telemetry store
   - dashboard repo / SQL-side summarization layer

2. Extra dependency surface
   - Phoenix
   - LiveView
   - Bandit
   - Ecto SQLite
   - Tailwind / esbuild
   - Rustler and supporting dashboard-native stack

3. Extra maintenance/test surface
   - bridge client tests
   - LiveView rendering tests
   - telemetry store behavior tests
   - endpoint/router/runtime config maintenance

4. Duplicate telemetry/projection work
   - Rust emits authoritative telemetry
   - Elixir consumes it
   - Elixir stores it again
   - LiveView renders from the Elixir copy

5. Extra release/operator burden
   - another app to boot and keep healthy
   - another HTTP server
   - another config/runtime layer
   - more moving parts during `dev` and future `live` hardening

### Benefits Elixir Still Provides

1. A working operator web surface already exists.
2. LiveView is productive for rich, reactive operator UIs.
3. If Axon later needs a broader multi-user/operator product shell, Phoenix could still support it well.

### Current Reality

The current Axon direction has already removed the strongest reasons for keeping Elixir as a core architectural pillar:

- Elixir no longer pilots indexing.
- Elixir no longer owns queueing or distribution.
- Elixir no longer owns command/control over runtime.
- Elixir no longer owns canonical runtime truth.

What remains is primarily:

- a web UI shell
- a telemetry projection layer
- a SQL/read-side summarization layer

That is useful, but it is no longer enough to treat Elixir as an obviously justified long-term core.

## Decision Questions

1. Does Elixir still own any responsibility that is runtime-critical for Axon today?
2. Is Elixir still the best place to host the operator dashboard?
3. Are we paying a disproportionate cost in process count, maintenance, release complexity, and duplicate telemetry projection?
4. If we move to a Rust-first dashboard, what exactly migrates and what can be retired?
5. If we keep Elixir temporarily, what must be frozen so it stops behaving like a hidden second platform?

## Architecture Options To Compare

### Option A: Keep Elixir as the long-lived operator web surface

Use when:
- future operator/product features truly need a separate web application platform
- Phoenix/LiveView remains a strategic choice beyond the Axon operator cockpit

Must prove:
- clear future responsibilities not already better served by Rust
- a meaningful productivity or product advantage
- acceptable maintenance cost

### Option B: Transitional model

Use when:
- the current dashboard still has short-term utility
- but the long-term target is Rust-first

Must prove:
- Elixir can be frozen to read-only projection
- no new control/runtime authority drifts back into Elixir
- migration seams are clear and bounded

### Option C: Rust-first operator dashboard

Use when:
- Elixir has become mostly a telemetry projection and UI shell
- runtime truth already lives in Rust
- the maintenance/release burden is no longer justified

Must prove:
- Rust can expose the required dashboard data directly
- the migration cost is lower than continued dual-stack maintenance
- the remaining Elixir-specific benefits are not strategically important

## Option Comparison

### Option A: Keep Elixir long-term

Assessment:
- technically viable
- weakly justified by present-day Axon runtime reality

Why it loses:
- today it is mostly projection, not authority
- it keeps a disproportionately large dependency and release surface
- it encourages two-stack maintenance for every operator-facing metric change

### Option B: Transitional freeze then migrate

Assessment:
- strongest fit to current reality

Why it wins:
- preserves the existing cockpit while we finish the split/refactor work
- avoids a rushed UI rewrite during ongoing runtime consolidation
- lets us freeze Elixir’s role strictly to read-only observation
- creates a clean path to Rust-first later without pretending Elixir is still central

### Option C: Immediate Rust-first rewrite

Assessment:
- strategically coherent
- tactically premature this instant

Why it does not win immediately:
- a complete dashboard rewrite now would compete with:
  - telemetry authority work
  - push-ramp proof
  - release/tooling cleanup
  - `IST/indexer` refactor
- the timing would increase blast radius during an already active architecture transition

## Recommendation

**Recommendation:** choose **Option B** now.

That means:
- keep Elixir/Phoenix/LiveView only as a **bounded transitional operator shell**
- forbid new runtime/control responsibilities from entering Elixir
- move new telemetry truth and future dashboard contracts to Rust-first surfaces
- plan an explicit later tranche to migrate the dashboard to Rust once the runtime and telemetry contracts are fully stabilized

### Why This Is The Best Trade-Off

1. It respects the current system reality.
   - Axon is already Rust-first for ingestion, runtime truth, MCP, SQL, IST, and SOLL authority.

2. It avoids investing further in a heavy dual-stack architecture.
   - Elixir keeps its current utility, but stops accumulating strategic importance.

3. It avoids a badly timed rewrite.
   - We do not pause critical split/runtime work just to replace the UI shell immediately.

4. It gives a clean architectural end-state.
   - The long-term target becomes:
     - Rust truth
     - Rust telemetry
     - Rust-served operator dashboard
   - Elixir survives only if a future product need justifies it again.

## Freeze Rules For The Transitional Period

1. Elixir remains read-only.
   - no ingestion control
   - no runtime command/control
   - no queue ownership
   - no mutation authority

2. New operator/runtime truth must originate in Rust.
   - Elixir may display it
   - Elixir must not become the defining source

3. New dashboard work should be judged against future Rust migration cost.
   - avoid clever LiveView-only abstractions that make migration harder

4. The bridge/ETS/LiveView path is transitional.
   - use it to observe
   - do not deepen the coupling

## Task 1: Audit Current Responsibilities

**Files:**
- Read: `src/dashboard/README.md`
- Read: `src/dashboard/mix.exs`
- Read: `src/dashboard/lib/axon_dashboard/application.ex`
- Read: `src/dashboard/lib/axon_dashboard/bridge_client.ex`
- Read: `src/dashboard/lib/axon_nexus/axon/watcher/telemetry.ex`
- Read: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Read: `src/dashboard/lib/axon_nexus/axon/watcher/progress.ex`
- Read: `src/axon-core/src/bridge.rs`
- Read: `src/axon-core/src/main_telemetry.rs`
- Read: `src/axon-core/src/mcp/tools_framework.rs`
- Update: `docs/plans/2026-04-22-brain-indexer-cross-dependency-audit.md`

**Step 1: Classify every live Elixir dashboard responsibility**

Produce a classification table:
- runtime-critical
- read-only projection
- duplicate truth
- legacy/dead/mostly dormant

At minimum classify:
- telemetry socket consumer
- telemetry ETS store
- LiveView cockpit
- SQL snapshot projection
- Phoenix endpoint/router/application
- any leftover control-plane assumptions

**Step 2: Record the current authority map**

Capture the current truth sources:
- `indexer` runtime telemetry truth
- `brain` MCP/runtime topology truth
- SQL truth
- Elixir dashboard projection truth

Explicitly note where Elixir is authoritative vs only reflective.

**Step 3: Update the cross-dependency audit**

Add a subsection for:
- legitimate remaining Elixir links
- tolerated transitional links
- links that would disappear in a Rust-first dashboard

## Task 2: Quantify Cost And Friction

**Files:**
- Read: `src/dashboard/config/*.exs`
- Read: `src/dashboard/test/**/*.exs`
- Read: `scripts/start.sh`
- Read: `scripts/status.sh`
- Read: `scripts/stop.sh`
- Read if needed: release scripts under `scripts/release/`
- Update: `docs/plans/2026-04-22-elixir-dashboard-value-audit-and-rust-first-decision-plan.md`

**Step 1: Enumerate the real cost of keeping the dashboard stack**

Measure or document:
- extra process/runtime surface
- dependency tree and build toolchain
- test surface
- release/operator surface
- duplication of telemetry/schema work

**Step 2: Identify where Elixir actively slows the current direction**

Look for:
- duplicate telemetry projection code
- extra tests needed for every runtime metric addition
- extra orchestration burden during dev/live
- stale assumptions inherited from the old control-plane era

**Step 3: Distinguish “future possibility” from “current necessity”**

Separate:
- current needs
- plausible future product needs
- historical justifications that no longer drive architecture

## Task 3: Compare Rust-First Feasibility

**Files:**
- Read: `src/axon-core/src/mcp_http.rs`
- Read: `src/axon-core/src/main_telemetry.rs`
- Read: `src/axon-core/src/bridge.rs`
- Read if needed: any existing Rust HTTP/server surfaces
- Update: `docs/plans/2026-04-22-elixir-dashboard-value-audit-and-rust-first-decision-plan.md`

**Step 1: Define the minimum Rust-served dashboard scope**

Scope only the operator surface already needed:
- runtime telemetry
- split topology truth
- queue/backlog visibility
- SQL/IST read-only summary where still required

**Step 2: Identify what can be served directly from Rust**

Classify data by source:
- already emitted by Rust
- trivially exposable by Rust
- still indirectly derived through Elixir today

**Step 3: Identify migration seams**

Document:
- which Elixir modules would disappear first
- which bridge/socket layers would be replaced
- whether the UI could be:
  - Rust-served static/HTMX-like pages
  - Rust-served SPA/static assets
  - a thinner read-only web shell

## Task 4: Make The Recommendation Explicit

**Files:**
- Update: `docs/plans/2026-04-22-elixir-dashboard-value-audit-and-rust-first-decision-plan.md`
- Update: `docs/plans/2026-04-22-ist-indexer-reset-and-refactor-implementation-plan.md`

**Step 1: Write the recommendation**

Choose one:
- keep Elixir
- transitional freeze then migrate
- commit to Rust-first now

**Step 2: Justify the recommendation**

The recommendation must explicitly cover:
- coherence
- efficiency
- maintenance cost
- delivery speed
- future extensibility
- blast radius

**Step 3: Convert the recommendation into follow-up tranches**

If Rust-first wins, define at least:
- telemetry/dashboard migration tranche
- Elixir freeze/removal tranche
- operator script/release cleanup tranche

If Elixir stays, define:
- boundaries that prevent architecture drift
- rules forbidding new control/runtime logic in Elixir

## Follow-Up Tranches Triggered By This Recommendation

### Immediate next tranche

- finish authoritative `indexer` telemetry and project it into the existing dashboard with explicit source/freshness metadata
- but keep all assertions anchored to Rust/runtime truth

### Later migration tranche

- design and build a Rust-served operator dashboard surface
- replace the Elixir bridge/ETS/UI stack in bounded steps
- retire Elixir once parity is sufficient

### Cleanup tranche after migration

- remove Phoenix/LiveView dashboard runtime from operator scripts and release expectations
- collapse duplicated telemetry projection layers
- delete dead dashboard-specific compatibility code

## Validation

- The plan is grounded in current code and runtime surfaces, not only prior vision docs.
- The comparison covers both current cost and future optionality.
- The resulting recommendation is actionable enough to drive the next tranche without re-litigating the whole question.

## Exit Criteria

- We have a documented, explicit architecture recommendation for Elixir/Phoenix/LiveView.
- The master plan reflects the decision tranche in the right order.
- The next implementation tranche is unambiguous:
  - either reinforce Elixir’s bounded role
  - or begin Rust-first dashboard migration.
