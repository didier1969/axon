# IST / Indexer Reset And Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Finish the split architecture by making `indexer` fully autonomous from `brain`/`SOLL`, certify cold-start ingest from a clean `IST`, then refactor the `IST/indexer` code into maintainable modules without carrying legacy cross-dependencies forward.

**Architecture:** The work is split into strict closed tranches. First, remove all runtime-active `indexer -> SOLL` dependencies and certify the minimal split contract. Second, make cold reset and cold qualification authoritative so throughput and graph ramp can be measured from zero. Third, refactor the `IST/indexer` area file-by-file around explicit responsibilities, deleting legacy branches instead of moving them around unchanged.

**Tech Stack:** Rust (`axon-core`), DuckDB plugin FFI, Bash operator scripts, Python qualification scripts, tmux-based dev runtime orchestration.

---

## Delivery Discipline

This plan is **ID to Delivery strict** and must be executed with these rules:

1. **Holistic-first rule**
   - Never patch a local symptom before mapping the full problem class.
   - Identify all code paths, scripts, state files, runtime fallbacks, and status surfaces affected by the same mechanism before editing.

2. **Closed tranche rule**
   - Each tranche has:
     - a fixed scope
     - a fixed file set
     - explicit entry criteria
     - explicit exit criteria
   - Do not start the next tranche until the current tranche is validated.

3. **No partial success claims**
   - No “almost done”, “advanced on X”, or “one more fix”.
   - Only report:
     - plan fixed
     - tranche completed and validated
     - true blocker

4. **Legacy deletion rule**
   - If a legacy path is proven architecturally invalid for split runtime, delete or isolate it.
   - Do not keep active fallback code “just in case” inside the new path.

5. **Validation rule**
   - Validate at tranche end, not after each tiny edit.
   - If a tranche fails, debug the tranche coherently; do not drift back into patch-test thrash.

6. **Role autonomy rule**
   - `brain` tests and benchmarks must not depend on `indexer` unless the tranche explicitly targets split integration.
   - `indexer` tests and benchmarks must not depend on `brain` unless the tranche explicitly targets split integration.
   - For performance and qualification work, always prefer:
     - role-only validation first
     - split integration second

---

## Target Contract

### Runtime

- `brain`
  - public MCP authority
  - dashboard authority
  - `SOLL` writer
  - `IST` reader replica only
  - no control authority over `indexer`

- `indexer`
  - filesystem discovery authority
  - local project identity authority from `.axon/meta.json` and local filesystem metadata
  - filter authority (`.gitignore`, `.axonignore`, `.axoninclude`, etc.)
  - `IST` writer
  - no runtime dependency on `SOLL`
  - self-piloted pipeline
  - quiescent sleep when work is drained

### Measurement

- `reset-dev-baseline.sh`
  - must produce a stable, measurable `dev` split runtime
- `reset-dev-indexer-baseline.sh`
  - must produce a stable, measurable `dev` runtime for `indexer` alone
- `qualify-dev-cold.sh`
  - must prove cold-start behavior from zeroed `IST`
- `qualify-dev-indexer-cold.sh`
  - must prove cold-start behavior from zeroed `IST` with `indexer` as the only authority
- qualification default path
  - must not depend on rich MCP analytics or `SOLL`

### Telemetry And Dashboard

- `indexer`
  - produces the canonical ingestion/runtime telemetry
  - exposes it through local machine-readable truth:
    - runtime heartbeat JSON
    - local telemetry socket
  - does not depend on `brain` or MCP to describe its own state
- `brain`
  - consumes `indexer` telemetry as a read-only projection
  - displays `indexer` telemetry in the dashboard without recomputing it
  - must show freshness/age of `indexer` telemetry explicitly
- qualification / benchmarking
  - role-only paths must read authoritative telemetry from the role being tested
  - split-with-dashboard paths may run `brain` in parallel for visualization, but assertions remain anchored to `indexer` telemetry for `indexer` work

### Test / Benchmark Topology

- `brain`-only tests
  - validate MCP, dashboard, reader-replica behavior, and degraded-read behavior without requiring `indexer`
- `indexer`-only tests
  - validate discovery, ingress, graph/vector pipeline, and cold-start throughput without requiring `brain`
- split integration tests
  - only validate contracts that truly require both roles:
    - authority truth
    - heartbeat/runtime truth
    - `IST` reader replica visibility
    - operational convergence

### Refactor outcome

The `IST/indexer` domain must be split by responsibility instead of remaining in very large mixed files:

- `project_identity`
- `ingress`
- `scan`
- `admission`
- `graph_pipeline`
- `vector_pipeline`
- `runtime_status`
- `split_topology`
- `store/bootstrap`

---

## Tranche 1: Remove Runtime-Active `indexer -> SOLL` Coupling

**Purpose:** Ensure split `indexer` no longer attaches or reads `soll.db` during normal runtime.

**Files:**
- Modify: `src/axon-core/src/graph.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/runtime_boot.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/project_meta.rs`
- Modify: `src/axon-core/src/scanner.rs`
- Modify: `src/axon-core/src/worker.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Update audit record: `docs/plans/2026-04-22-brain-indexer-cross-dependency-audit.md`

**Entry criteria:**
- Split runtime is understood.
- Residual `SOLL` touches are classified into:
  - structural store/bootstrap paths
  - status/telemetry paths
  - rich MCP/operator paths

**Implementation steps:**

1. Finalize a `GraphStore` mode for split `indexer` that does **not** attach `SOLL`.
2. Route `RuntimeBootRole::IndexerShadow` to the no-`SOLL` constructor.
3. Ensure reader refresh / reader replica publication works without any `SOLL` attach side-effects.
4. Keep `brain` on `SOLL` writer + `IST` reader replica contract.
5. Ensure project identity resolution remains local-first and does not regress to registered DB identities.
6. Ensure status for split `indexer` omits any `SOLL`-dependent counters or summaries.
7. Update the cross-dependency audit doc with:
   - deleted links
   - tolerated links
   - remaining links, if any

**Tests:**
- `cargo test --manifest-path src/axon-core/Cargo.toml test_indexer_store_can_boot_while_brain_holds_soll_writer -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_reader_replica_publish_reuses_path_when_duckdb_temp_dir_exists -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_status_indexer_split_omits_soll_mcp_job_counts -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_split_brain_and_indexer_authorities -- --test-threads=1`

**Validation commands:**
- `cargo fmt --manifest-path src/axon-core/Cargo.toml -- src/axon-core/src/graph.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/runtime_boot.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/project_meta.rs src/axon-core/src/scanner.rs src/axon-core/src/worker.rs src/axon-core/src/main_background.rs`
- `git diff --check src/axon-core/src/graph.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/runtime_boot.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/project_meta.rs src/axon-core/src/scanner.rs src/axon-core/src/worker.rs src/axon-core/src/main_background.rs`

**Exit criteria:**
- No normal split `indexer` runtime path attaches `soll.db`.
- `indexer` boots while `brain` holds the `SOLL` writer.
- `indexer` still publishes `ist-reader.db`.

---

## Tranche 2: Certify Dev Baseline And Cold Qualification

**Purpose:** Make the `dev` runtime reproducibly resettable and cold-measurable from zeroed `IST`.

**Files:**
- Modify: `scripts/lib/dev-baseline.sh`
- Modify: `scripts/reset-dev-baseline.sh`
- Add: `scripts/reset-dev-indexer-baseline.sh`
- Modify: `scripts/qualify-dev-cold.sh`
- Add: `scripts/qualify-dev-indexer-cold.sh`
- Modify: `scripts/qualify_ingestion_run.py`
- Modify if needed: `scripts/start.sh`
- Modify if needed: `scripts/status.sh`
- Modify if needed: `scripts/stop.sh`
- Modify if needed: `scripts/axon`
- Update: `docs/plans/2026-04-22-dev-baseline-and-cold-qualification-plan.md`

**Entry criteria:**
- Tranche 1 is green.
- Split runtime on `dev` can start with `indexer` detached from `SOLL`.

**Implementation steps:**

1. Define the canonical baseline contract:
   - `brain` healthy
   - `indexer` healthy
   - `indexer` canonical
   - `brain` allowed to lag briefly while `IST` reader replica catches up
2. Make `reset-dev-baseline.sh` stop, purge, restart, and wait on that exact contract.
3. Zero only the dev `IST` surfaces required for cold runs:
   - `ist.db`
   - `ist.db.wal`
   - `ist-reader.db`
   - stale `IST` locks/pids/sockets if needed
4. Ensure `qualify-dev-cold.sh` runs without requiring rich MCP diagnostics by default.
5. Add an `indexer`-only baseline/qualification path:
   - no `brain`
   - no MCP dependency
   - no SQL dependency
   - authoritative local telemetry only
6. Ensure seeded project identity for cold qualification comes from local metadata or explicit local seed path, not `SOLL`.
7. Archive authoritative cold-run artifacts:
   - summary
   - samples
   - runtime truth
8. Update the dev baseline plan doc with the stabilized contract and known limits.

**Tests / checks:**
- `python3 -m py_compile scripts/qualify_ingestion_run.py`
- `bash -n scripts/lib/dev-baseline.sh scripts/reset-dev-baseline.sh scripts/qualify-dev-cold.sh scripts/start.sh scripts/status.sh scripts/stop.sh scripts/axon`

**Runtime validation:**
- `bash scripts/reset-dev-baseline.sh`
- `bash scripts/qualify-dev-cold.sh --duration 20 --interval 5 --label canonical-cold`
- `bash scripts/reset-dev-indexer-baseline.sh`
- `bash scripts/qualify-dev-indexer-cold.sh --duration 20 --interval 5 --label canonical-indexer-cold`
- `env AXON_INSTANCE_KIND=dev bash scripts/status-brain.sh`
- `env AXON_INSTANCE_KIND=dev bash scripts/status-indexer.sh`
- `tmux capture-pane -pt axon-dev-indexer:core | tail -n 200`

**Exit criteria:**
- Baseline reset produces a stable measurable split runtime.
- `indexer`-only baseline reset produces a stable measurable role-only runtime.
- Cold qualification completes without `SOLL` lock conflicts.
- The archived role-only run proves whether discovery/admission/graph ramp is active from zero.

---

## Tranche 3: Make Indexer Telemetry Authoritative And Dashboard-Visible

**Purpose:** Establish one canonical `indexer` telemetry model, use it for role-only qualification, and project it into the `brain` dashboard without making `brain` authoritative for `indexer`.

**Files:**
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/main_telemetry.rs`
- Modify if needed: `src/axon-core/src/mcp/tools_framework.rs`
- Modify if needed: `scripts/qualify_ingestion_run.py`
- Modify if needed: `scripts/qualify-dev-cold.sh`
- Modify if needed: `scripts/qualify-dev-indexer-cold.sh`
- Modify if needed: dashboard files under `src/dashboard/` or equivalent UI surface

**Entry criteria:**
- Tranche 2 is green.
- `indexer`-only qualification runs end-to-end from local telemetry without `brain`.

**Implementation steps:**

1. Define the canonical `indexer` telemetry schema for:
   - ingress
   - graph backlog
   - vector backlog
   - scheduler/claim state
   - telemetry freshness
2. Ensure `indexer` heartbeat/runtime truth always publishes those fields directly.
3. Ensure qualification scripts prefer this telemetry over SQL/MCP when validating `indexer`.
4. Project the same telemetry into `brain` dashboard as a read-only peer view.
5. Expose freshness and source information in the dashboard so lag is visible, not hidden.
6. Support two explicit qualification/display modes:
   - role-only truth (`indexer` only)
   - split-with-dashboard visibility (`brain` + `indexer`)

**Tests / validation:**
- targeted Rust tests for telemetry schema stability
- `bash scripts/qualify-dev-indexer-cold.sh --duration 20 --interval 5 --label telemetry-authority`
- `bash scripts/qualify-dev-cold.sh --duration 20 --interval 5 --label dashboard-projection`

**Exit criteria:**
- `indexer` telemetry is authoritative for `indexer` qualification.
- `brain` dashboard shows `indexer` telemetry without becoming its authority.
- split dashboard visibility no longer distorts role-only qualification.

---

## Tranche 4: Finish Push Ramp Visibility And Throughput Truth

**Purpose:** Certify where the real upstream bottleneck remains once cold qualification is valid.

**Files:**
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify if needed: `src/axon-core/src/main_telemetry.rs`
- Modify if needed: `src/axon-core/src/mcp/tools_framework.rs`
- Modify if needed: `scripts/qualify_ingestion_run.py`
- Modify if needed: `scripts/qualify-dev-cold.sh`

**Entry criteria:**
- Tranche 3 is green.
- Cold runs now produce real work in `IST`.

**Implementation steps:**

1. Use cold-run artifacts to identify the actual choke among:
   - discovery
   - buffered ingress
   - persisted pending
   - graph ready
2. Implement one coherent throughput batch only after this classification is complete.
3. Ensure status/telemetry report the real ramp:
   - known
   - pending
   - graph ready
   - graph queue
   - vector backlog
4. Keep operator telemetry separate from throughput-critical code paths.
5. Update qualification summary fields so throughput proof does not require cockpit scraping or rich MCP paths.

**Tests / validation:**
- targeted Rust tests for admission / queue / telemetry invariants
- `bash scripts/qualify-dev-cold.sh --duration 30 --interval 5 --label push-ramp-proof`

**Exit criteria:**
- Cold-run metrics clearly show the real upstream choke.
- No ambiguous reliance on stale dashboards or rich MCP diagnostics.

---

## Tranche 5: Stabilize Split Release / Tooling Chain

**Purpose:** Remove remaining monolith-first assumptions from release/outillage before larger refactors.

**Files:**
- Modify: `scripts/release/create_manifest.py`
- Modify: `scripts/release/preflight.sh`
- Modify: `scripts/release/promote_live.sh`
- Modify: `scripts/release/rollback_live.sh`
- Modify if needed: `scripts/start.sh`
- Modify if needed: `scripts/status.sh`
- Modify if needed: `scripts/stop.sh`

**Entry criteria:**
- Dev split contract is stable.
- Cold qualification is meaningful.

**Implementation steps:**

1. Re-audit the release chain for split vs monolith assumptions.
2. Make manifests topology-authoritative.
3. Make preflight and promote/rollback topology-aware end-to-end.
4. Remove operator outputs that imply monolithic runtime truth when in split mode.
5. Keep `live` safeguards intact; do not weaken dirty-tree or artifact-integrity guards.

**Validation:**
- `python3 -m py_compile scripts/release/create_manifest.py`
- `bash -n scripts/release/preflight.sh scripts/release/promote_live.sh scripts/release/rollback_live.sh`
- topology-targeted dry validation where possible

**Exit criteria:**
- Split release tooling no longer depends on monolith-first paths for correctness.

---

## Tranche 6: Consolidate Jobs, Scripts, And MCP Operator Surfaces

**Purpose:** Remove or isolate legacy operator logic that still mixes split and monolith assumptions, and use MCP/operator surfaces as audit inputs to classify dead, tolerated, and harmful active paths.

**Files:**
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Modify: `scripts/axon`
- Modify if needed: `scripts/release/create_manifest.py`
- Modify if needed: `scripts/release/preflight.sh`
- Modify if needed: `scripts/release/promote_live.sh`
- Modify if needed: `scripts/release/rollback_live.sh`
- Modify if needed: MCP-facing runtime/status surfaces under `src/axon-core/src/mcp/`
- Update: `docs/plans/2026-04-22-brain-indexer-cross-dependency-audit.md`

**Entry criteria:**
- Tranches 1-5 are green.
- Role-only and split qualification paths are both stable.

**Implementation steps:**

1. Audit jobs/scripts/surfaces by category:
   - indispensable
   - tolerated legacy
   - dead
   - active harmful
2. Remove or isolate monolith-first logic that is still active in split paths.
3. Separate operator commands by role where useful:
   - `brain`
   - `indexer`
   - split integration
4. Ensure release/qualification wrappers no longer mix role-only and split assumptions.
5. Use `brain` MCP/operator data as an inspection aid for topology and surface discovery, not as the sole authority for `indexer`.
6. Update the audit with explicit removals and surviving justified links.

**Validation:**
- `bash -n` on touched scripts
- `python3 -m py_compile` on touched Python tools
- `git diff --check`
- role-only and split smoke runs through canonical wrappers

**Exit criteria:**
- jobs/scripts/operator surfaces reflect the split architecture cleanly.
- dead or harmful active operator paths are removed or isolated.
- MCP/operator surfaces are classified and documented as audit inputs, not hidden dependencies.

---

## Tranche 7: Refactor IST / Indexer File-By-File

**Purpose:** Replace oversized mixed modules with a maintainable structure once the split contract is stable.

**Files:**
- Primary candidates:
  - `src/axon-core/src/main_background.rs`
  - `src/axon-core/src/graph_bootstrap.rs`
  - `src/axon-core/src/graph_query.rs`
  - `src/axon-core/src/runtime_boot.rs`
  - `src/axon-core/src/scanner.rs`
  - `src/axon-core/src/worker.rs`
  - `src/axon-core/src/project_meta.rs`
- Create new module tree under `src/axon-core/src/`:
  - `ist_store/`
  - `project_identity/`
  - `ingress/`
  - `scan/`
  - `admission/`
  - `graph_pipeline/`
  - `vector_pipeline/`
  - `runtime_status/`
  - `split_topology/`

**Entry criteria:**
- Tranches 1-6 are green.
- The valid runtime contract is stable enough to refactor without moving unknown behavior.

**Implementation steps:**

1. Map current responsibilities inside each large file.
2. Define the target module boundaries before moving code.
3. Extract one responsibility at a time, with compile/test at module milestones.
4. Delete legacy branches that no longer belong to the new responsibility.
5. Keep public APIs explicit at module boundaries.
6. Add short module-level docs where boundary clarity matters.

**Validation:**
- targeted Rust tests per extracted module
- full split runtime smoke on `dev`
- `git diff --check`

**Exit criteria:**
- Oversized mixed files are materially reduced.
- Core `indexer` logic is grouped by responsibility.
- Legacy monolith-era branches are removed, not merely moved.

---

## Tranche 8: Final Runtime And Documentation Closure

**Purpose:** Close the operational loop with runtime proof and final architecture explanation.

**Files:**
- Modify or recreate: `docs/architecture/visualize-nexus-pull.html`
- Update: closeout plan docs as needed
- Update runtime qualification artifacts

**Entry criteria:**
- Runtime and refactor tranches are green.

**Implementation steps:**

1. Run final `dev` qualification on the stable split architecture.
2. Archive evidence for:
   - `brain => SOLL`
   - `indexer => IST`
   - no `indexer -> SOLL`
   - cold reset reproducibility
   - push-ramp truth
3. Rebuild the architecture explainer:
   - macro to micro
   - split contract
   - push / graph / vector flow
   - explicit “why Axon exists” and “why split exists”

**Exit criteria:**
- The architecture is both operationally certified and documentarily explainable.

---

## Canonical Validation Matrix

Run this matrix at tranche boundaries where applicable:

### Rust
- `cargo test --manifest-path src/axon-core/Cargo.toml test_indexer_store_can_boot_while_brain_holds_soll_writer -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_reader_replica_publish_reuses_path_when_duckdb_temp_dir_exists -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_status_indexer_split_omits_soll_mcp_job_counts -- --test-threads=1`
- `cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_split_brain_and_indexer_authorities -- --test-threads=1`

### Scripts
- `bash -n scripts/lib/dev-baseline.sh scripts/reset-dev-baseline.sh scripts/reset-dev-indexer-baseline.sh scripts/qualify-dev-cold.sh scripts/qualify-dev-indexer-cold.sh scripts/start.sh scripts/status.sh scripts/stop.sh scripts/axon`
- `python3 -m py_compile scripts/qualify_ingestion_run.py scripts/release/create_manifest.py`

### Runtime
- `bash scripts/reset-dev-baseline.sh`
- `bash scripts/qualify-dev-cold.sh --duration 20 --interval 5 --label canonical-cold`
- `bash scripts/reset-dev-indexer-baseline.sh`
- `bash scripts/qualify-dev-indexer-cold.sh --duration 20 --interval 5 --label canonical-indexer-cold`
- `env AXON_INSTANCE_KIND=dev bash scripts/status-brain.sh`
- `env AXON_INSTANCE_KIND=dev bash scripts/status-indexer.sh`
- `tmux capture-pane -pt axon-dev-indexer:core | tail -n 200`

---

## Definition Of Done

The process is complete only when all of the following are true:

- `indexer` no longer depends on `SOLL` in normal split runtime.
- `brain` and `indexer` are operationally autonomous except for their explicit DB/data contracts.
- `reset-dev-baseline.sh` produces a stable measurable `dev` runtime.
- `reset-dev-indexer-baseline.sh` produces a stable measurable `indexer`-only runtime.
- `qualify-dev-cold.sh` proves cold ingest from a zeroed `IST`.
- `qualify-dev-indexer-cold.sh` proves cold ingest from a zeroed `IST` without `brain`.
- `indexer` telemetry is authoritative and can be projected into the dashboard through `brain`.
- upstream choke is measured from real cold-run evidence, not inferred from stale state.
- jobs/scripts/MCP operator surfaces no longer hide legacy split/monolith confusion.
- split release/tooling no longer depends on monolith-first assumptions for correctness.
- `IST/indexer` has been refactored into maintainable responsibility-based modules.
- final docs explain the stable architecture rather than the transition state.
