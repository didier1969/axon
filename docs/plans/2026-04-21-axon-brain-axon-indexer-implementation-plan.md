# Axon Brain / Axon Indexer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split the current Axon runtime into `axon-brain` and `axon-indexer` without duplicating `SOLL` or `IST`, while preserving public MCP behavior, making freshness explicit, and avoiding a cutover before writer authority and cross-plane truth are proven.

**Architecture:** Keep one repository and one shared Rust core library. First define and validate the cross-plane contracts on top of the existing runtime and telemetry bridge, including single-writer enforcement, runtime-truth freshness, shared `IST` read safety, and proxied runtime-command semantics. Only then introduce separate binaries in shadow mode; only after shadow validation passes do scripts, dashboard, and public operator flows cut over.

**Tech Stack:** Rust (`tokio`, `axum`, current `axon-core` library), existing `bridge.rs` / telemetry feed, DuckDB plugin surface, existing Elixir dashboard, current Bash/Python operator scripts and qualification tooling.

---

## Validation Matrix

The split is not done until all of these pass:

- `axon-brain` can boot without `axon-indexer` and reports degraded freshness instead of false green
- `axon-indexer` can boot without `axon-brain` and continue writing `IST`
- public MCP stays on `axon-brain`
- `SOLL` has exactly one writer: `axon-brain`
- `IST` has exactly one writer: `axon-indexer`
- `status(full)` distinguishes:
  - `brain_ready`
  - `indexer_ready`
  - `system_converged`
- runtime-affecting MCP actions are either proxied to `axon-indexer` or explicitly refused with a clear degraded error contract
- `axon-brain` can read `IST` safely under active `axon-indexer` writes, or degrade explicitly if that read model is not trustworthy
- partial degradation is honest:
  - `axon-indexer` healthy and writing
  - `axon-brain` feed stale or disconnected
  - public MCP degrades and refuses/proxies correctly
- proxy timeout/retry does not create ambiguous duplicate mutation execution
- startup scripts and qualification can run:
  - brain only
  - indexer only
  - both in shadow mode
  - both in cutover mode
- dashboard and operator tooling surface stale runtime feed and stale `IST` snapshot separately

---

### Task 0: Freeze the split contracts before any topology cutover

**Files:**
- Create: `src/axon-core/src/runtime_topology.rs`
- Create: `src/axon-core/src/runtime_truth_contract.rs`
- Modify: `src/axon-core/src/lib.rs`
- Modify: `src/axon-core/src/runtime_mode.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`
- Modify: `docs/plans/2026-04-21-axon-brain-axon-indexer-concept.md`

**Step 1: Write the failing tests**

Add tests that prove:

```rust
#[test]
fn topology_roles_encode_brain_and_indexer_authority() {
    assert!(AxonProcessRole::Brain.serves_public_mcp());
    assert!(AxonProcessRole::Brain.owns_soll_writes());
    assert!(!AxonProcessRole::Brain.owns_ist_writes());
    assert!(AxonProcessRole::Indexer.owns_ist_writes());
    assert!(!AxonProcessRole::Indexer.serves_public_mcp());
}

#[test]
fn runtime_topology_requires_system_converged_for_full_green() {
    let topo = RuntimeTopologyStatus::degraded_brain_only();
    assert!(topo.brain_ready);
    assert!(!topo.system_converged);
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml topology_roles_encode_brain_and_indexer_authority -- --test-threads=1
cargo test --manifest-path src/axon-core/Cargo.toml runtime_topology_requires_system_converged_for_full_green -- --test-threads=1
```

Expected: FAIL because the split contracts do not exist yet.

**Step 3: Write the minimal implementation**

Implement the authoritative contract types for:

- process role:
  - `brain`
  - `indexer`
  - optional `legacy_monolith`
- writer ownership:
  - `SOLL`
  - `IST`
- readiness:
  - `brain_ready`
  - `indexer_ready`
  - `system_converged`
- freshness:
  - runtime feed freshness
  - `IST` read freshness

Also update the concept doc if the contract terminology changes during implementation.

**Step 4: Run tests to verify they pass**

Run the two targeted tests plus the canonical status test:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml topology_roles_encode_brain_and_indexer_authority -- --test-threads=1
cargo test --manifest-path src/axon-core/Cargo.toml runtime_topology_requires_system_converged_for_full_green -- --test-threads=1
cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_public_surface_and_runtime_truth -- --test-threads=1
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/runtime_topology.rs src/axon-core/src/runtime_truth_contract.rs src/axon-core/src/lib.rs src/axon-core/src/runtime_mode.rs src/axon-core/src/mcp/tests.rs docs/plans/2026-04-21-axon-brain-axon-indexer-concept.md
git commit -m "feat: freeze brain indexer authority contracts"
```

---

### Task 1: Decide and implement the runtime-truth transport on the existing bridge

**Files:**
- Modify: `src/axon-core/src/bridge.rs`
- Modify: `src/axon-core/src/main_telemetry.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing tests**

Add tests that require:

```rust
#[test]
fn runtime_truth_feed_marks_missing_heartbeat_stale() {
    let feed = RuntimeTruthFeed::from_last_heartbeat_ms(10_000, 2_000);
    assert!(feed.stale);
}
```

and:

```rust
assert_eq!(status["data"]["runtime_topology"]["indexer_feed"]["stale"].as_bool(), Some(true));
assert_eq!(status["data"]["runtime_topology"]["system_converged"].as_bool(), Some(false));
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml runtime_truth_feed_marks_missing_heartbeat_stale -- --test-threads=1
```

Expected: FAIL because the bridge does not yet carry the split runtime-truth contract.

**Step 3: Write the minimal implementation**

Extend the existing bridge/telemetry path instead of inventing a second transport by default.

Implement:

- heartbeat/snapshot semantics on the existing bridge
- stale threshold
- last-good payload timestamp
- degraded reason
- status mapping where:
  - brain healthy + indexer feed stale => degraded, not green

Only introduce a new transport module if the bridge cannot satisfy the contract after explicit evaluation.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml runtime_truth_feed_marks_missing_heartbeat_stale -- --test-threads=1
cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_public_surface_and_runtime_truth -- --test-threads=1
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/bridge.rs src/axon-core/src/main_telemetry.rs src/axon-core/src/main_background.rs src/axon-core/src/service_guard.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: extend bridge with split runtime truth"
```

---

### Task 2: Enforce single-writer ownership for `SOLL` and `IST`

**Files:**
- Create: `src/axon-core/src/runtime_writer_guard.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`

**Step 1: Write the failing tests**

Add tests that prove:

```rust
#[test]
fn indexer_refuses_second_ist_writer() {
    let first = WriterGuard::acquire_ist("test-root").unwrap();
    let second = WriterGuard::acquire_ist("test-root");
    assert!(second.is_err());
    drop(first);
}
```

and equivalent behavior for `SOLL`.

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml indexer_refuses_second_ist_writer -- --test-threads=1
```

Expected: FAIL because writer enforcement does not exist yet.

**Step 3: Write the minimal implementation**

Implement explicit single-writer enforcement with:

- acquisition on startup
- refusal semantics on double-start
- clear error text for operators
- release on shutdown

Do not rely on “only launch one process” as the enforcement mechanism.

**Step 4: Run tests to verify they pass**

Run the targeted tests plus a start/stop sanity check.

**Step 5: Commit**

```bash
git add src/axon-core/src/runtime_writer_guard.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/main.rs src/axon-core/src/mcp/tests.rs scripts/start.sh scripts/stop.sh
git commit -m "feat: enforce single writer ownership for soll and ist"
```

---

### Task 3: Prove the phase-1 `IST` read model under active writes

**Files:**
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/graph_query.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`
- Modify: `scripts/qualify_runtime.py`

**Step 1: Write the failing tests**

Add an integration-style test proving that a read-only/snapshot-safe brain-side reader can still:

- read `IST`
- observe freshness metadata
- degrade explicitly if the read becomes unstable

Example test skeleton:

```rust
#[test]
fn read_only_ist_reader_degrades_instead_of_false_green_when_snapshot_is_unstable() {
    let status = simulated_brain_status_with_unstable_ist_snapshot();
    assert_eq!(status["runtime_topology"]["ist_snapshot"]["healthy"].as_bool(), Some(false));
    assert_eq!(status["runtime_topology"]["system_converged"].as_bool(), Some(false));
}
```

**Step 2: Run tests to verify they fail**

Run the targeted test and any relevant runtime qualification probe.

**Step 3: Write the minimal implementation**

Implement the phase-1 reader contract:

- direct read-only/snapshot-safe access against the same `IST`
- explicit freshness surface
- explicit unsafe-read detection
- explicit fallback:
  - if safe reads cannot be maintained, `axon-brain` degrades to control-plane plus stale-index truth
- explicit trust boundary:
  - who computes freshness
  - what makes a read unsafe
  - whether degraded brain remains query-capable or only control-plane-capable

**Step 4: Run tests to verify they pass**

Run the targeted tests and one short qualification proving concurrent writes do not silently produce false-green status.

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_bootstrap.rs src/axon-core/src/graph_query.rs src/axon-core/src/mcp/tests.rs scripts/qualify_runtime.py
git commit -m "feat: prove safe ist read model for axon-brain"
```

---

### Task 4: Define the runtime-command proxy protocol before any public cutover

**Files:**
- Create: `src/axon-core/src/runtime_command_proxy.rs`
- Modify: `src/axon-core/src/mcp.rs`
- Modify: `src/axon-core/src/mcp_http.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing tests**

Write contract tests for:

- proxied runtime mutation success
- proxied runtime mutation timeout
- proxied mutation refusal when indexer feed is stale
- async job ownership and returned error/result shape

Example:

```rust
#[test]
fn proxied_runtime_mutation_is_refused_when_indexer_feed_is_stale() {
    let response = simulated_brain_proxy_response("resume_vectorization", true);
    assert_eq!(response["error"]["code"].as_str(), Some("indexer_unavailable"));
}
```

**Step 2: Run tests to verify they fail**

Run the proxy contract tests.

**Step 3: Write the minimal implementation**

Define and implement:

- request/response model
- timeout model
- retry/idempotency expectations
- degraded refusal model
- async mutation ownership

Do not treat proxying as a route label only. This task is about protocol and failure semantics.

**Step 4: Run tests to verify they pass**

Run the proxy contract test suite plus targeted MCP tests.

**Step 5: Commit**

```bash
git add src/axon-core/src/runtime_command_proxy.rs src/axon-core/src/mcp.rs src/axon-core/src/mcp_http.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: define runtime command proxy protocol"
```

---

### Task 5: Add split boot profiles and binaries in shadow mode

**Files:**
- Create: `src/axon-core/src/runtime_boot.rs`
- Create: `src/axon-core/src/bin/axon-brain.rs`
- Create: `src/axon-core/src/bin/axon-indexer.rs`
- Modify: `src/axon-core/src/main.rs`
- Modify: `src/axon-core/Cargo.toml`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing tests**

Add tests that prove:

```rust
#[test]
fn split_boot_roles_enable_only_owned_services() {
    let brain = RuntimeBootProfile::brain_shadow();
    assert!(brain.start_mcp_http);
    assert!(!brain.start_ingestion_workers);

    let indexer = RuntimeBootProfile::indexer_shadow();
    assert!(!indexer.start_mcp_http);
    assert!(indexer.start_ingestion_workers);
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml split_boot_roles_enable_only_owned_services -- --test-threads=1
```

Expected: FAIL because the split boot profiles do not exist yet.

**Step 3: Write the minimal implementation**

Implement:

- shared boot profiles in-library
- `axon-brain` binary
- `axon-indexer` binary
- keep the monolith boot path unchanged and still authoritative
- make split binaries shadow-capable first:
  - they can boot
  - they expose split status
  - they do not yet replace the operator-default runtime path
  - they are explicitly non-promotable before Task 6 gates pass

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml split_boot_roles_enable_only_owned_services -- --test-threads=1
cargo build --manifest-path src/axon-core/Cargo.toml --bin axon-brain --bin axon-indexer
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/axon-core/src/runtime_boot.rs src/axon-core/src/bin/axon-brain.rs src/axon-core/src/bin/axon-indexer.rs src/axon-core/src/main.rs src/axon-core/Cargo.toml src/axon-core/src/mcp/tests.rs
git commit -m "feat: add shadow-mode brain and indexer binaries"
```

---

### Task 6: Add process-level split integration and qualification gates

**Files:**
- Modify: `scripts/qualify_runtime.py`
- Modify: `scripts/qualify_ingestion_run.py`
- Create: `scripts/start-brain.sh`
- Create: `scripts/start-indexer.sh`
- Create: `scripts/status-brain.sh`
- Create: `scripts/status-indexer.sh`
- Modify: `scripts/start.sh`
- Modify: `scripts/status.sh`

**Step 1: Write the failing integration checks**

Define checks for:

- brain only
- indexer only
- both in shadow mode
- stale runtime feed
- safe proxied refusal path

**Step 2: Run the current scripts to verify they fail**

Run the intended entrypoints and capture the current monolith-only gap.

**Step 3: Write the minimal implementation**

Implement:

- split script entrypoints
- split qualification modes
- status outputs that distinguish:
  - `brain_ready`
  - `indexer_ready`
  - `system_converged`
  - stale runtime feed
  - stale `IST` snapshot

Keep `start.sh` as the operator-default monolith wrapper during this task. The split path remains shadow-only here.

**Step 4: Run integration checks**

Run the new split entrypoints and qualification commands.

**Step 5: Commit**

```bash
git add scripts/qualify_runtime.py scripts/qualify_ingestion_run.py scripts/start-brain.sh scripts/start-indexer.sh scripts/status-brain.sh scripts/status-indexer.sh scripts/start.sh scripts/status.sh
git commit -m "feat: add split qualification and shadow scripts"
```

---

### Task 7: Prepare rollback and promotion gates

**Files:**
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Modify: `scripts/qualify_runtime.py`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`

**Step 1: Write the failing checks**

Define rollback expectations for:

- split-process shutdown order
- monolith reactivation path
- writer-guard release after split shutdown
- status/qualification proof that rollback restores canonical truth

**Step 2: Run the current rollback path to verify the gap**

Run the current operator scripts and capture that split rollback semantics are not explicit yet.

**Step 3: Write the minimal implementation**

Implement:

- an explicit rollback procedure from split mode back to monolith
- clear promotion guardrails:
  - split shadow mode not promotable before gates are green
  - cutover blocked if rollback path is red
- qualification hooks that verify rollback restores canonical truth and authority

**Step 4: Run rollback checks**

Run the rollback flow and qualification checks in dev.

**Step 5: Commit**

```bash
git add scripts/start.sh scripts/stop.sh scripts/status.sh scripts/qualify_runtime.py docs/operations/2026-04-18-live-dev-runtime-operations.md
git commit -m "feat: add split rollback and promotion gates"
```

---

### Task 8: Cut over public MCP and operator flows to `axon-brain`

**Files:**
- Modify: `src/axon-core/src/mcp.rs`
- Modify: `src/axon-core/src/mcp_http.rs`
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the failing end-to-end checks**

Define end-to-end checks that require:

- public MCP on `axon-brain`
- runtime mutations proxied or refused correctly
- `system_converged` only when both planes are healthy and fresh

**Step 2: Verify shadow prerequisites**

Before cutover, all of these must already pass:

- Task 1 through Task 6
- Task 7 rollback and promotion gates
- split binaries boot independently
- qualification understands split readiness
- proxy failure semantics are green
- `IST` read-model proof is green

If any are red, stop here.

**Step 3: Write the minimal implementation**

Switch the default operator path so:

- `axon-brain` becomes the public MCP/control-plane authority
- `axon-indexer` stays private and writer-focused
- stale/degraded conditions remain explicit

**Step 4: Run end-to-end checks**

Run:

```bash
python3 scripts/mcp_validate.py --url http://127.0.0.1:44139/mcp --surface core --project AXO
python3 scripts/qualify_runtime.py --instance dev --profile ingestion --mode full --reuse-runtime
```

Expected: PASS or explicit degraded reporting with no false-green state.

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp.rs src/axon-core/src/mcp_http.rs scripts/start.sh scripts/stop.sh scripts/status.sh src/axon-core/src/mcp/tests.rs
git commit -m "feat: cut over public mcp to axon-brain"
```

---

### Task 9: Align dashboard, docs, and retire monolith-first assumptions

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex`
- Modify: `src/dashboard/test/axon_dashboard_web/live/status_live_test.exs`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`
- Modify: `src/axon-core/src/main_services.rs`
- Modify: `src/axon-core/src/runtime_mode.rs`

**Step 1: Write the failing tests**

Add a dashboard test that requires:

```elixir
assert html =~ "brain_ready"
assert html =~ "indexer_ready"
assert html =~ "system_converged"
```

and that a stale indexer feed does not render as full green.

**Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test test/axon_dashboard_web/live/status_live_test.exs'
```

Expected: FAIL because the dashboard and docs still assume monolith-first semantics.

**Step 3: Write the minimal implementation**

Update:

- dashboard readiness presentation
- operator docs
- skill/operator routing docs
- remaining monolith-first assumptions in runtime services and mode docs

Do not remove compatibility code that still protects production unless the cutover validations and rollback checks are green.

**Step 4: Run full validation**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml -- --test-threads=1
devenv shell -- bash -lc 'cd src/dashboard && mix test'
python3 scripts/qualify_runtime.py --instance dev --profile ingestion --mode full --reuse-runtime
python3 scripts/mcp_validate.py --url http://127.0.0.1:44139/mcp --surface core --project AXO
```

Expected: PASS or explicitly documented residual warnings with no authority ambiguity.

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/watcher/cockpit_live.ex src/dashboard/test/axon_dashboard_web/live/status_live_test.exs docs/operations/2026-04-18-live-dev-runtime-operations.md docs/skills/axon-engineering-protocol/SKILL.md src/axon-core/src/main_services.rs src/axon-core/src/runtime_mode.rs
git commit -m "feat: finalize split readiness and retire monolith-first assumptions"
```

---

## Rollout Notes

- Phase 1 must keep schema continuity.
- Do not split `SOLL` or `IST` into duplicates.
- Do not introduce a second public MCP surface.
- Do not cut over public/operator defaults before:
  - runtime-truth feed is proven
  - writer guards are proven
  - `IST` read safety is proven
  - proxy semantics are proven
- Do not promote split mode without a green rollback path.
- If shared-file `IST` reading is unstable, stop the rollout and keep split mode shadow-only until a safer read model is added.

Plan complete and saved to `docs/plans/2026-04-21-axon-brain-axon-indexer-implementation-plan.md`. Two execution options:

1. Subagent-Driven (this session) - I dispatch fresh subagent per task, review between tasks, fast iteration

2. Parallel Session (separate) - Open new session with executing-plans, batch execution with checkpoints

Which approach?
