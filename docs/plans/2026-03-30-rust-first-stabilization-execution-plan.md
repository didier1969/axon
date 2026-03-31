# Rust-First Stabilization Execution Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Converge Axon toward a Rust-owned runtime with correct delta recovery, governed `SOLL`, and genuinely useful LLM-facing retrieval layers.

**Architecture:** Execute in risk-first waves. Rust owns runtime truth and ingestion. Elixir is reduced to visualization/operator consumption. `IST` remains reconstructible and adaptive; `SOLL` remains protected and progressively more governable. Semantic/vector layers stay derived and come only after structural truth.

**Tech Stack:** Rust, DuckDB, Phoenix/Elixir, Nix/Devenv, MCP/SQL, ONNX embeddings, timestamped `SOLL` exports.

---

## Wave 1: Make Rust The Sole Runtime Authority

### Task 1: Freeze Elixir ingestion authority inventory

**Files:**
- Modify: `docs/architecture/2026-03-30-rust-first-elixir-visualization.md`
- Modify: `docs/working-notes/reality-first-stabilization-handoff.md`
- Inspect: `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`
- Inspect: `src/dashboard/lib/axon_nexus/axon/watcher/pool_facade.ex`
- Inspect: `src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex`

**Step 1: Write the failing documentation expectation**

Document the remaining Elixir-owned runtime responsibilities still present today and mark them `to retire`.

**Step 2: Verify the current code still contains them**

Run:

```bash
rg -n "BackpressureController|PoolFacade|IndexingWorker|BatchDispatch|Watcher" src/dashboard/lib/axon_nexus/axon
```

Expected: multiple matches proving residual authority exists.

**Step 3: Update the architecture doc**

Record the exact modules that must be de-authorized, and the order of removal.

**Step 4: Validate doc consistency**

Run:

```bash
rg -n "Elixir.*ingestion|Rust.*canonical runtime|to retire" docs/architecture docs/working-notes
```

Expected: aligned wording across docs.

**Step 5: Commit**

```bash
git add docs/architecture/2026-03-30-rust-first-elixir-visualization.md docs/working-notes/reality-first-stabilization-handoff.md
git commit -m "docs: freeze remaining elixir ingestion authority"
```

### Task 2: Add red tests around Elixir de-authoring boundary

**Files:**
- Create/Modify: `src/dashboard/test/...` targeted watcher/control tests
- Modify: `src/dashboard/lib/axon_nexus/axon/watcher/server.ex`
- Modify: `src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex`

**Step 1: Write failing tests**

Add tests that express:

- UI still renders and polls state,
- no dashboard module is required for canonical ingestion to continue,
- no control decision taken in Elixir can block Rust truth.

**Step 2: Run tests to verify failure**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

Expected: failing assertions around the old authority assumptions.

**Step 3: Implement the minimal boundary changes**

Remove or neutralize decision paths while preserving rendering.

**Step 4: Re-run tests**

Run:

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

Expected: green.

**Step 5: Commit**

```bash
git add src/dashboard/lib src/dashboard/test
git commit -m "refactor: de-authorize elixir ingestion control"
```

### Task 3: Remove redundant Rust/Elixir backpressure split

**Files:**
- Modify: `src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `docs/architecture/2026-03-30-adaptive-ingestion-concept.md`

**Step 1: Write the failing expectation**

Define in tests/docs that canonical throttling lives in Rust only.

**Step 2: Verify current overlap**

Run:

```bash
rg -n "pressure|backpressure|pause|resume|queue" src/dashboard/lib src/axon-core/src
```

Expected: overlapping control logic found.

**Step 3: Minimize Elixir to display-only pressure state**

Keep telemetry display, remove canonical control semantics.

**Step 4: Validate Rust still protects SQL/MCP under load**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

Expected: green.

**Step 5: Commit**

```bash
git add src/dashboard/lib/axon_nexus/axon/backpressure_controller.ex src/axon-core/src/main_background.rs docs/architecture/2026-03-30-adaptive-ingestion-concept.md
git commit -m "refactor: converge canonical backpressure into rust"
```

## Wave 2: Finish Delta Restart And Recovery

### Task 4: Codify `IST` invalidation policy

**Files:**
- Create: `docs/architecture/2026-03-30-ist-invalidation-policy.md`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write failing tests**

Add explicit tests for:

- additive migration,
- delta restart on compatible `IST`,
- rebuild only on incompatible runtime metadata.

**Step 2: Run failing tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml maillon_2c
```

Expected: one or more failures until policy is explicit.

**Step 3: Implement minimal version-policy code**

Separate:

- additive repair,
- soft invalidation,
- hard rebuild.

**Step 4: Re-run focused and full tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml maillon_2
cargo test --manifest-path src/axon-core/Cargo.toml
```

Expected: green.

**Step 5: Commit**

```bash
git add docs/architecture/2026-03-30-ist-invalidation-policy.md src/axon-core/src/graph_bootstrap.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: codify ist invalidation and restart policy"
```

### Task 5: Complete watcher observability before full Elixir withdrawal

**Files:**
- Modify: `src/axon-core/src/watcher_probe.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `docs/architecture/2026-03-30-adaptive-ingestion-concept.md`
- Modify: `docs/working-notes/reality-first-stabilization-handoff.md`

**Step 1: Write the failing expectation**

Require visibility for:

- received paths,
- filtered paths,
- staged paths,
- salvage/rescan paths,
- no-op reasons.

**Step 2: Verify current checkpoint coverage**

Run:

```bash
rg -n "watcher\\." src/axon-core/src
```

Expected: incomplete or evolving coverage until finished.

**Step 3: Add the missing probes minimally**

Keep them non-blocking and runtime-safe.

**Step 4: Validate with live watcher run**

Run:

```bash
bash scripts/start-v2.sh
tmux capture-pane -pt axon:0 -S -4000 | rg "WatcherProbe"
```

Expected: checkpoints visible for a real delta flow.

**Step 5: Commit**

```bash
git add src/axon-core/src/watcher_probe.rs src/axon-core/src/main_background.rs docs/architecture/2026-03-30-adaptive-ingestion-concept.md docs/working-notes/reality-first-stabilization-handoff.md
git commit -m "feat: complete rust watcher observability checkpoints"
```

### Task 6: Cover deletes, renames, and crash-mid-index

**Files:**
- Modify: `src/axon-core/src/fs_watcher.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write failing tests**

Cover:

- deleted file disappears or becomes tombstoned correctly,
- rename does not leave ghost truth,
- crash during `indexing` replays safely.

**Step 2: Run focused failure**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml maillon_2
```

**Step 3: Implement the minimal replay/tombstone logic**

Keep `IST` truthful without full rescan.

**Step 4: Re-run tests**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml maillon_2
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/fs_watcher.rs src/axon-core/src/graph_ingestion.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: handle deletes renames and crash replay in ist"
```

## Wave 3: Complete Adaptive Ingestion

### Task 7: Introduce explicit `hot / bulk / titan` lanes

**Files:**
- Modify: `src/axon-core/src/queue.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/worker.rs`
- Modify: `docs/architecture/2026-03-30-adaptive-ingestion-concept.md`

**Step 1: Write failing scheduling tests**

Prove that:

- hot work cannot starve,
- bulk work slows first,
- titan files do not poison the common lane.

**Step 2: Run tests to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml queue
```

**Step 3: Implement lane separation minimally**

Keep the single writer path untouched.

**Step 4: Re-run focused + full tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/queue.rs src/axon-core/src/main_background.rs src/axon-core/src/worker.rs docs/architecture/2026-03-30-adaptive-ingestion-concept.md
git commit -m "feat: add explicit hot bulk titan ingestion lanes"
```

### Task 8: Tie live service health directly to ingestion policy

**Files:**
- Modify: `src/axon-core/src/service_guard.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/embedder.rs`

**Step 1: Write failing tests**

Express:

- MCP/SQL critical latency reduces claim depth,
- embeddings shut off before structure suffers,
- recovery back to normal throughput is gradual.

**Step 2: Run tests to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml service_guard
```

**Step 3: Implement the minimal adaptive policy**

No new authority outside Rust.

**Step 4: Re-run full tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/service_guard.rs src/axon-core/src/main_background.rs src/axon-core/src/embedder.rs
git commit -m "feat: tie live service health to rust ingestion policy"
```

## Wave 4: Consolidate SOLL Governance

### Task 9: Encode executable SOLL invariants

**Files:**
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`
- Create: `docs/architecture/2026-03-30-soll-invariants.md`

**Step 1: Write failing tests**

Cover examples such as:

- `Requirement` not orphaned,
- `Validation` verifies something,
- `Decision` linked to a need or impact.

**Step 2: Run targeted failure**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml soll
```

**Step 3: Implement minimal invariant checks**

Prefer read-only validation first, not destructive repair.

**Step 4: Re-run tests**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_soll.rs src/axon-core/src/mcp/tests.rs docs/architecture/2026-03-30-soll-invariants.md
git commit -m "feat: add executable soll invariants"
```

### Task 10: Improve restore coverage of links and metadata

**Files:**
- Modify: `src/axon-core/src/mcp/soll.rs`
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing restore tests**

Require links and key metadata to survive `export -> restore`.

**Step 2: Run them to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml vcr4
```

**Step 3: Implement the minimum restore extension**

Keep the export format human-usable.

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/soll.rs src/axon-core/src/mcp/tools_soll.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: restore soll links and metadata more completely"
```

### Task 11: Decide and prototype granular SOLL disk projection

**Files:**
- Create: `docs/architecture/2026-03-30-soll-granular-projection.md`
- Optional prototype under: `src/axon-core/...`

**Step 1: Write comparison doc**

Compare:

- timestamped snapshot only,
- snapshot + per-item projection,
- snapshot + per-document projection.

**Step 2: Select one thin prototype**

No large implementation yet.

**Step 3: Validate review usefulness**

Show how Git diff/readability improves.

**Step 4: Commit**

```bash
git add docs/architecture/2026-03-30-soll-granular-projection.md
git commit -m "docs: evaluate granular soll disk projection"
```

## Wave 5: Build LLM Value Layers In The Right Order

### Task 12: Finish `Chunk` quality and retrieval usefulness

Status:

- completed on `feat/rust-first-control-plane`
- retrieval now keeps `symbol-first` behavior, then falls back to ranked chunk retrieval with explicit provenance (`docstring`, `chunk body`, `chunk metadata`, `file path`)
- derived chunk content now carries docstring text when present so natural developer questions can recover behaviorally relevant context without pretending semantic certainty

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing retrieval tests**

Ask natural development questions and require `Chunk`-level answers to improve over symbol-only retrieval.

**Step 2: Run to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml vcr1
```

**Step 3: Improve chunk formation minimally**

Keep structure truthful and derived.

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: improve chunk retrieval for llm developer queries"
```

### Task 13: Add explicit `Chunk` invalidation and recompute targeting

Status:

- completed on `feat/rust-first-control-plane`
- file-scoped reindex now drops only the affected `ChunkEmbedding` rows before replacing derived chunks
- `fetch_unembedded_chunks()` now treats `source_hash != content_hash` as stale and requeues only the affected chunks for semantic recompute
- unrelated project chunks and embeddings remain untouched

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write failing tests**

Require:

- file change invalidates affected chunks only,
- stale chunk embeddings are recomputed without full semantic replay,
- no unrelated project chunks are touched.

**Step 2: Run to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml chunk
```

**Step 3: Implement minimal invalidation**

Keep it path- and symbol-scoped.

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_ingestion.rs src/axon-core/src/embedder.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add targeted chunk invalidation"
```

### Task 14: Introduce `GraphProjection`

Status:

- completed on `feat/rust-first-control-plane`
- added a dedicated derived `GraphProjection` table; no truth tables (`CALLS`, `CONTAINS`, `Symbol`) were repurposed
- symbol projection now materializes a bounded call-neighborhood around an anchor symbol
- file projection now materializes a stable bounded neighborhood anchored on the file and its contained symbols
- `axon_impact` now appends an explicit local projection section, clearly labeled as derived context rather than canonical call-graph truth

**Files:**
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/graph_query.rs`
- Modify: `src/axon-core/src/mcp/tools_risk.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write failing tests**

Require neighborhood projection around a symbol or file.

**Step 2: Run to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml graph_projection
```

**Step 3: Implement minimal projection table and invalidation**

No graph embedding yet.

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_bootstrap.rs src/axon-core/src/graph_query.rs src/axon-core/src/mcp/tools_risk.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add graph projection layer"
```

### Task 15: Add explicit `GraphProjection` invalidation

Status:

- completed on `feat/rust-first-control-plane`
- added a dedicated `GraphProjectionState` table to track source signature and projection version per anchor/radius
- unchanged symbol/file projections are now reused without rewrite
- changed anchors refresh only their own projection rows; unrelated neighborhoods stay reusable

**Files:**
- Modify: `src/axon-core/src/graph_query.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/tests/maillon_tests.rs`

**Step 1: Write failing tests**

Require:

- graph projections invalidate when source structure changes,
- unaffected neighborhoods remain reusable,
- projection refresh is smaller than full rebuild.

**Step 2: Run to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml graph_projection
```

**Step 3: Implement minimal targeted invalidation**

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/graph_query.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/tests/maillon_tests.rs
git commit -m "feat: add targeted graph projection invalidation"
```

### Task 16: Add graph embeddings only after projection validation

Status:

- completed on `feat/rust-first-control-plane`
- added a dedicated derived `GraphEmbedding` table keyed by anchor/radius/model and tied to `GraphProjectionState`
- graph embeddings are refreshed only when the underlying projection signature or projection version drifts
- the semantic worker now computes graph embeddings only after chunk and symbol backlog is clear and the live service is `Healthy`
- MCP consumption stays honest: `axon_semantic_clones` appends a clearly labeled graph-derived neighborhood section, never canonical truth
- stale graph embeddings are ignored by joining back to current `GraphProjectionState`

**Files:**
- Modify: `src/axon-core/src/embedder.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing tests**

Prove graph-vector retrieval adds value beyond chunk-only retrieval.

**Step 2: Run to fail**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml graph_embedding
```

**Step 3: Implement minimally**

Derived only, versioned, disposable.

**Step 4: Re-run**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
```

**Step 5: Commit**

```bash
git add src/axon-core/src/embedder.rs src/axon-core/src/graph_bootstrap.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: add derived graph embeddings"
```

## Wave 6: Product Consolidation

### Task 17: Audit and retire obsolete Python scripts

Status:

- completed on `feat/rust-first-control-plane`
- runtime Python scope is now explicit: parser bridges only (`datalog`, `typeql`)
- a first high-confidence obsolete set was removed from the repo
- canonical startup scripts were revalidated and hardened while running this audit
- legacy Python still present in the repo is now intentionally treated as `tolerated`, not canonical

**Files:**
- Modify: `docs/plans/2026-03-30-commercial-stabilization-roadmap.md`
- Modify/Delete: legacy Python scripts/tests/benchmarks after explicit classification

**Step 1: Create inventory with status**

Classify each script:

- current,
- tolerated,
- obsolete.

**Step 2: Write failing expectation**

No obsolete Python path remains on the canonical startup path.

**Step 3: Remove or isolate obsolete paths**

Do not touch Datalog/TypeQL bridge yet unless explicitly planned.

**Step 4: Re-run validation**

```bash
bash scripts/setup_v2.sh
bash scripts/start-v2.sh
```

**Step 5: Commit**

```bash
git add scripts tests benchmarks docs
git commit -m "chore: retire obsolete python operational paths"
```

### Task 18: Finalize Elixir as visualization-only

**Files:**
- Modify: `src/dashboard/lib/...`
- Modify: `docs/architecture/2026-03-30-rust-first-elixir-visualization.md`

**Step 1: Remove last ingestion semantics**

Keep only:

- rendering,
- subscriptions,
- read-model display,
- operator diagnostics.

**Step 2: Validate UI still works**

```bash
devenv shell -- bash -lc 'cd src/dashboard && mix test'
```

**Step 3: Validate runtime still works**

```bash
cargo test --manifest-path src/axon-core/Cargo.toml
bash scripts/start-v2.sh
```

**Step 4: Commit**

```bash
git add src/dashboard/lib docs/architecture/2026-03-30-rust-first-elixir-visualization.md
git commit -m "refactor: reduce elixir to visualization role"
```

Completed on `2026-03-31` with this delivered slice:

- canonical dashboard supervisor no longer boots `Oban` or `Axon.Watcher.Server`
- `Axon.Watcher.Application` helper child list excludes `Staging`, `Oban`, and `Server`
- `BridgeClient` no longer fabricates local engine state on control casts
- `PoolEventHandler` no longer re-enqueues canonical pending work and emits an explicit ignored checkpoint
- cockpit controls were reduced to runtime status/diagnostics
- validations passed:
  - `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
  - `cargo test --manifest-path src/axon-core/Cargo.toml`
  - `bash scripts/stop-v2.sh`
  - `bash scripts/start-v2.sh`
  - `/sql` live check

### Task 19: Prepare the operator and product surface

**Files:**
- Modify: `README.md`
- Modify: startup/validation docs
- Modify: operator UX docs and dashboard wording

**Step 1: Write final usage checklist**

The product must be startable, diagnosable, and stoppable with the canonical flow only.

**Step 2: Validate on a clean restart**

```bash
bash scripts/stop-v2.sh
bash scripts/start-v2.sh
curl -sS -X POST http://127.0.0.1:44129/sql -H "content-type: application/json" --data "{\"query\":\"SELECT count(*) FROM File\"}"
```

**Step 3: Commit**

```bash
git add README.md docs scripts
git commit -m "docs: finalize operator-facing canonical workflow"
```

Completed on `2026-03-31` with this delivered slice:

- `README.md` and `docs/getting-started.md` now describe only the canonical source checkout workflow:
  - `devenv shell`
  - `./scripts/validate-devenv.sh`
  - `./scripts/setup_v2.sh`
  - `./scripts/start-v2.sh`
  - `./scripts/stop-v2.sh`
- operator wording now aligns with the runtime split:
  - Rust = canonical runtime and DuckDB truth
  - Elixir = visualization plane
- `start-v2.sh` now hard-fails on incomplete readiness and verifies:
  - live SQL schema
  - live MCP over HTTP when the tunnel binary is absent
- `stop-v2.sh` now matches the current repo slug and clears the full active local port set
- `validate-devenv.sh` now checks the operator-critical tools used by the nominal path:
  - `tmux`
  - `nc`
  - `curl`
- dashboard wording no longer implies that the visualization plane owns worker control or queue authority

Validations passed:

- `bash -n scripts/setup_v2.sh`
- `bash -n scripts/start-v2.sh`
- `bash -n scripts/stop-v2.sh`
- `devenv shell -- bash -lc 'cd src/dashboard && mix test'`
- `bash scripts/stop-v2.sh`
- `bash scripts/start-v2.sh`
- `curl -sS -X POST http://127.0.0.1:44129/sql -H "content-type: application/json" --data "{\"query\":\"SELECT count(*) AS file_count FROM File\"}"`
  - returned `40732`

## Final Gate

Before calling this program phase complete, confirm:

1. Rust is the only ingestion/control authority.
2. Elixir is only visualization/operator plane.
3. `IST` restart/delta behavior is trustworthy.
4. `SOLL` has executable governance and usable restore.
5. Chunk retrieval is useful in practice.
6. Graph vectorization exists only as derived truth.
7. Startup/shutdown/restart are canonical and repeatable.
8. Obsolete Python operational paths are retired.

## Suggested Commit Rhythm

- one commit per task or per tightly coupled pair of tasks,
- push after each wave,
- update handoff after every wave,
- never leave runtime-shaping work uncommitted.

## Post-Plan LLM Quality Work

After Tasks 15 to 19 and after the Final Gate, the next exploitation order is fixed:

1. `A` task-oriented retrieval for developer work
- help a coding LLM decide where to look, what to load, and what to modify

2. `B` pre-change safety and quality guardrails
- impact before edit, public surface warnings, SOLL constraints, test suggestions, risk framing

3. `C` richer project memory and conceptual continuity
- why the code exists, architectural intent, decisions, constraints, historical rationale

Decision recorded:

- do `A` first
- then `B`
- keep `C` for the very end

Plan complete and saved to `docs/plans/2026-03-30-rust-first-stabilization-execution-plan.md`. Two execution options:

1. Subagent-Driven (this session) - I dispatch fresh subagent per task, review between tasks, fast iteration
2. Parallel Session (separate) - Open new session with executing-plans, batch execution with checkpoints

Which approach?
