# Cockpit Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the current machine-centric watcher cockpit with a native Phoenix LiveView operator cockpit that surfaces progress, backlog causality, project readiness, ingress state, and runtime truth without reintroducing control-plane authority.

**Architecture:** Keep Phoenix strictly read-only. Rebuild the cockpit as a single LiveView page driven by compact view models derived from `Progress`, `Telemetry`, and SQL-backed factual aggregates. Remove CDN dependencies from the watcher layout and rely on local asset pipeline bootstrapping only.

**Tech Stack:** Elixir, Phoenix 1.8, Phoenix LiveView 1.1, HEEx, Tailwind v4, local `app.js`, ETS-backed telemetry, SQL-backed progress snapshots.

---

### Task 1: Lock the no-CDN layout contract

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/layouts.ex`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add assertions that the rendered watcher root layout does not contain:

- `cdn.jsdelivr.net`
- `fonts.googleapis.com`
- `fonts.gstatic.com`

And does contain:

- local `app.js`
- local `app.css`

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_nexus/axon/watcher/cockpit_live_test.exs'
```

Expected:
- failure because the current layout still injects CDN resources

**Step 3: Write minimal implementation**

Refactor `layouts.ex` so the watcher root layout:

- links to local bundled assets only
- removes inline CDN Phoenix/LiveView bootstrapping
- relies on the existing asset pipeline bootstrapping in `assets/js/app.js`

**Step 4: Run test to verify it passes**

Run the same targeted test.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/layouts.ex src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: remove CDN dependencies from watcher layout"
```

### Task 2: Introduce a cockpit-native view model

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add tests that assert the LiveView now renders named operator sections:

- `Workspace`
- `Backlog`
- `Projects`
- `Runtime`
- `Memory`

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_nexus/axon/watcher/cockpit_live_test.exs'
```

Expected:
- failure because the current cockpit still renders the old `UNIT 01/02/...` layout

**Step 3: Write minimal implementation**

Refactor `CockpitLive` to:

- replace the current unit-based structure with a single operator-oriented page
- build compact assigns/view maps for:
  - workspace summary
  - backlog summary
  - runtime summary
  - memory summary

Keep all behavior read-only.

**Step 4: Run test to verify it passes**

Run the targeted cockpit test again.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: rebuild cockpit around operator view model"
```

### Task 3: Surface backlog causality explicitly

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/progress.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add a test that seeds factual backlog causes and expects the cockpit to render:

- top `status_reason`
- pending count
- indexing count
- degraded / oversized / skipped visibility

**Step 2: Run test to verify it fails**

Run the targeted cockpit test.

Expected:
- failure because the current progress/view layer does not expose this operator summary directly

**Step 3: Write minimal implementation**

Extend `Progress` with factual summary helpers backed by `SqlGateway`, for example:

- backlog counts
- top backlog reasons
- completion rate

Then render that summary in the `Backlog` band of `CockpitLive`.

**Step 4: Run test to verify it passes**

Run the targeted cockpit test.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/progress.ex src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: show backlog causality in cockpit"
```

### Task 4: Add project readiness as a first-class section

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/progress.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add assertions that the cockpit renders project rows containing:

- project slug
- completed / total
- readiness badge or partial-truth badge

**Step 2: Run test to verify it fails**

Run the targeted cockpit test.

Expected:
- failure because the current directory/project stats are too weakly represented

**Step 3: Write minimal implementation**

Extend the project summary layer to compute per-project:

- total
- completed
- completion ratio
- readiness state label

Render projects with LiveView streams if the collection is modeled as a list.

**Step 4: Run test to verify it passes**

Run the targeted cockpit test.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/progress.ex src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: add project readiness section to cockpit"
```

### Task 5: Integrate runtime, ingress, and memory bands cleanly

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Modify: `src/dashboard/assets/css/app.css`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add assertions that the cockpit exposes:

- `claim_mode`
- `service_pressure`
- queue depth
- ingress buffer fields
- `RSS`, `RssAnon`, `RssFile`
- DB / WAL size

**Step 2: Run test to verify it fails**

Run the targeted cockpit test.

Expected:
- failure because the old layout does not surface these with the new information hierarchy

**Step 3: Write minimal implementation**

Reorganize the telemetry rendering into dedicated runtime and memory sections.
Add local CSS classes for:

- operator cards
- readiness badges
- state colors
- compact metric grids

Do not add any external UI dependency.

**Step 4: Run test to verify it passes**

Run the targeted cockpit test.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/assets/css/app.css src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: integrate runtime and memory bands into cockpit"
```

### Task 6: Polish layout hierarchy and loading safety

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Modify: `src/dashboard/assets/css/app.css`
- Test: `src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs`

**Step 1: Write the failing test**

Add assertions for stable empty/loading behavior, for example:

- no crash when project/backlog lists are empty
- fallback labels such as `No backlog reason` or `No project data yet`

**Step 2: Run test to verify it fails**

Run the targeted cockpit test.

Expected:
- failure if the page assumes non-empty data everywhere

**Step 3: Write minimal implementation**

Add empty-state rendering and tighten layout structure:

- stable headings
- stable empty-state cards
- readable fallback labels
- no dead sections

**Step 4: Run test to verify it passes**

Run the targeted cockpit test.

Expected:
- pass

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/assets/css/app.css src/dashboard/test/axon_nexus/axon/watcher/cockpit_live_test.exs
git commit -m "feat: harden cockpit empty states and layout hierarchy"
```

### Task 7: Validate the full dashboard suite

**Files:**
- Verify existing tests only

**Step 1: Run the dashboard suite**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

Expected:
- full dashboard suite green

**Step 2: Run asset-aware validation if needed**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix precommit'
```

Expected:
- compile, format, and test all green

**Step 3: Commit**

```bash
git add src/dashboard
git commit -m "feat: redesign cockpit as a LiveView-native operator surface"
```

### Task 8: Runtime validation

**Files:**
- Verify runtime scripts only

**Step 1: Start the stack**

Run:

```bash
bash scripts/start-v2.sh
```

Expected:
- dashboard ready
- SQL ready
- MCP ready

**Step 2: Check the cockpit route**

Open or probe:

```bash
curl -sS http://127.0.0.1:44127/cockpit
```

Expected:
- HTML renders the new cockpit structure

**Step 3: Stop the stack**

Run:

```bash
bash scripts/stop-v2.sh
```

Expected:
- clean stop

**Step 4: Commit**

```bash
git add .
git commit -m "test: validate redesigned cockpit on live runtime"
```

Plan complete and saved to `docs/plans/2026-04-02-cockpit-redesign-implementation-plan.md`. Two execution options:

1. Subagent-Driven (this session) - I dispatch fresh subagent per task, review between tasks, fast iteration
2. Parallel Session (separate) - Open new session with executing-plans, batch execution with checkpoints
