# Axon Final Operations Closeout Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Finish the remaining Axon work so the `axon-brain` / `axon-indexer` split is operationally trustworthy, the indexer follows the intended `push` upstream / `pull` GPU model, qualification is repeatable, and the architecture story is documented clearly enough for operators and buyers.

**Architecture:** Keep the validated split contract in place: `brain` owns public MCP, dashboard, and `SOLL`; `indexer` owns ingestion, graphing, vectorization, and `IST` writes. Finish by hardening cross-process operations, restoring fast upstream fill into `graph_ready`, proving the downstream GPU loop behaves as a true pull system, then rewriting the visual architecture artifact from macro to micro.

**Tech Stack:** Rust (`axon-core`, `tokio`, DuckDB), Bash operator scripts, Python qualification scripts, Elixir dashboard, single-repo split runtime (`axon-brain`, `axon-indexer`), HTML + Mermaid for architecture communication.

---

## Current State vs Target State

### Current state

- `dev` split is already proven green:
  - `brain => SOLL writer`
  - `indexer => IST writer`
  - `public_mcp_authority=brain`
  - `system_converged=true`
- `brain` now reads `IST` in reader-only mode without competing for the writer lock.
- split wrappers exist:
  - `start-brain.sh`
  - `start-indexer.sh`
  - `stop-*`
  - `status-*`
- runtime doctrine is already established:
  - upstream is `push`
  - downstream GPU is `pull`
  - `finalize` is asynchronous

### Still open

- `brain` observer noise and reader-only diagnostics still need cleanup.
- `brain` still has a dangerous `IST` read fallback path to the wrong writer context when reader visibility is lost; this must become an explicit degraded state instead.
- `live` split operations still need the same confidence level as `dev`.
- release/preflight/promote/rollback remain monolith-oriented and are not yet split-aware for `live`.
- runtime command proxying from `brain -> indexer` is not a real control path yet.
- upstream discovery/admission is still too slow compared to the intended architecture.
- upstream discovery and admission are still too coupled in the same hot path.
- the GPU loop needs final proof that it stays fed when `graph_ready` exists.
- clean-state qualification and benchmarking need a stable, documented procedure.
- `qualify_ingestion_run.py` still scores against stale monolith/shadow assumptions and must be realigned.
- the architecture visual explainer must be rewritten.

### Explicitly out of scope for this tranche

- temporary `IST` writer lease handoff from `brain` back to `indexer`
- replacing DuckDB with HydraDB
- introducing a second replica of `IST`

---

## Definition Of Done

The work is complete only when all of the following are true:

- `dev` split runtime is green:
  - `brain => SOLL writer`
  - `indexer => IST writer`
  - `public_mcp_authority=brain`
  - `system_converged=true`
- `live` can be started, stopped, and status-checked in split topology without ambiguous ownership or false green.
- `live` release, preflight, promote, and rollback flows are split-aware and no longer assume one `axon-core` monolith.
- `indexer` behaves as two loops:
  - upstream `push`: `Watcher + Scan -> buffered_discovery -> persisted_file_pending -> graph_ready`
  - downstream `pull`: `graph_ready -> prepare -> ready_batches -> GPU -> finalize`
- `brain` never falls back to the wrong writer context for `IST` reads; reader loss degrades honestly.
- runtime-affecting commands exposed through `brain` either proxy correctly to `indexer` or fail explicitly with a clear degraded contract.
- runtime proof shows the upstream loop is materially faster than the downstream loop at initial fill time.
- runtime proof shows the GPU loop stays fed whenever `graph_ready` is available.
- qualification from a clean `IST` root is reproducible and archived.
- `docs/architecture/visualize-nexus-pull.html` becomes a real macro-to-micro visual explainer with Mermaid diagrams and buyer-facing positioning.
- any intentionally deferred work is stated explicitly, not implied.

## Execution Order

1. Lock down split runtime and operator truth.
2. Make `brain` reader-only `IST` behavior fail-safe.
3. Productize split-aware `live` release and runtime command routing.
4. Fix upstream `indexer` throughput.
5. Re-verify downstream GPU pull behavior.
6. Run clean-state qualification and benchmarking.
7. Rewrite the architecture visual artifact.
8. Close with a final operator checklist and deferred-items note.

---

## Runtime Acceptance Gates

- `status-brain.sh` returns `STATUS HEALTHY`
- `status-indexer.sh` returns `STATUS HEALTHY`
- `public_mcp_authority=brain`
- `soll_writer_authority=brain`
- `ist_writer_authority=indexer`
- `brain_ready=true`
- `indexer_ready=true`
- `system_converged=true`
- `truth_status=canonical`
- `canonical_truth_restored=true`
- `promotion_allowed=true`
- `cutover_blocked=false`
- `brain` logs and status surfaces show honest degradation on `IST` reader loss instead of wrong-context reads
- runtime-affecting control operations are either proxied to `indexer` or rejected explicitly

## Release Acceptance Gates

- split `live` preflight validates the actual split artifacts, not only `bin/axon-core`
- split `live` promotion can start/restart both `brain` and `indexer`
- split `live` rollback can return cleanly to the previous topology
- release scripts verify writer ownership after restart:
  - `SOLL => brain`
  - `IST => indexer`
- no release script silently assumes monolith-only runtime semantics

## Performance Acceptance Gates

- initial corpus discovery happens in seconds, not in a long flatline crawl
- `buffered_discovery -> persisted_file_pending` rises rapidly on a clean run
- `persisted_file_pending -> graph_ready` builds reserve before GPU drain catches up
- `graph_ready > 0` leads to sustained downstream pull activity
- `finalize` stays off the hot GPU path
- downstream vector throughput is not blamed when upstream canonical stock is absent

## Canonical Boundary Table

| Boundary | Owner | Canonical Stock | Loop Mode | Blocking Authority | Conversion Metric | First Operator Action |
| --- | --- | --- | --- | --- | --- | --- |
| `buffered_discovery -> persisted_file_pending` | `indexer` | `persisted_file_pending` | `push` | admission/persistence blockers | `buffered_to_persisted_per_min` | inspect discovery, admission, persistence |
| `persisted_file_pending -> graph_ready` | `indexer` | `graph_ready` | `push` | graph WIP / graph blockers | `persisted_to_graph_ready_per_min` | inspect graph control + WIP caps |
| `graph_ready -> ready_batches/vector_ready` | `indexer` | `ready_batches` / `vector_ready` | `pull` | GPU / prepare / finalize safety | `graph_ready_to_vector_ready_per_min` | inspect GPU feed and reserve |

## Ownership And Decision Rights

- `brain`
  - owns public MCP
  - owns dashboard
  - owns `SOLL` writes
  - reads `IST`
- `indexer`
  - owns watcher/ingestion/graph/vector/finalize
  - owns `IST` writes
  - owns effective GPU consumption policy
- only `indexer` may redefine ingestion/vector execution behavior
- only `brain` may redefine public MCP/dashboard/operator truth surfaces

## Benchmark Protocol

1. stop `dev` split runtime cleanly
2. reset `IST dev` only
3. start `indexer`, then `brain`
4. verify split runtime gates before sampling
5. run qualification on the clean window
6. archive:
   - `runtime-status.json`
   - `runtime-quiescent-summary.json`
   - `runtime-resource-summary.json`
   - `summary.json`
7. record:
   - discovery time
   - `buffered_to_persisted_per_min`
   - `persisted_to_graph_ready_per_min`
   - `graph_ready_to_vector_ready_per_min`
   - GPU activity / reserve behavior

## Failure Modes And Operator Responses

- `brain up / indexer down`
  - MCP and dashboard may stay readable
  - runtime truth must degrade explicitly
  - no false green
- `indexer up / brain down`
  - indexing may continue
  - public MCP is unavailable
  - operator action is to restore `brain`, not stop `indexer` by default
- stale `IST` visibility in `brain`
  - degrade freshness and avoid pretending index truth is current
  - never fall back to non-authoritative writer-side reads
- stale runtime feed
  - degrade `system_converged`
  - keep last-good truth visibly marked stale
- writer lock conflict
  - treat as authority bug
  - stop the conflicting role and verify lock ownership
- GPU unavailable
  - downstream loop may idle or degrade
  - upstream push still must be diagnosable independently

## Rollback And Recovery

- split rollback remains an explicit return to the monolith
- required sequence:
  1. stop split runtime
  2. verify both writer guards are released
  3. restart monolith only if split recovery is not possible
- recovery proof must include:
  - released lockfiles
  - correct role ownership after restart
  - no stale split PID/session confusion

## Live Promotion Policy

- do not promote split topology to `live` until:
  - runtime gates are green on `dev`
  - benchmark protocol is repeatable
  - upstream fill and downstream GPU pull are both proven
- `dev` is the only place for resettable `IST` benchmarking
- `live` promotion requires explicit evidence, not just passing unit tests

---

### Task 0: Freeze the validated split baseline

**Files:**
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Modify: `docs/plans/2026-04-21-axon-brain-axon-indexer-implementation-plan.md`
- Modify: `docs/plans/2026-04-21-axon-brain-axon-indexer-concept.md`

**Deliver:**
- Record the current validated baseline:
  - `brain` is `SOLL` writer
  - `indexer` is `IST` writer
  - `brain` reads `IST` in reader-only mode
  - fallback writer handoff is **not** implemented yet
- Mark which parts are already proven on `dev` so the remaining plan does not reopen settled work.

**Validation:**
- docs mention current truth consistently
- no document still claims `brain` owns or may implicitly acquire `IST` during normal split operation

---

### Task 1: Clean the remaining reader-only `brain` runtime noise

**Files:**
- Modify: `src/axon-core/src/graph_query.rs`
- Modify: `src/axon-core/src/bridge.rs`
- Modify: `src/axon-core/src/main_telemetry.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver:**
- Ensure `brain` never routes `IST` read-only operator queries through writer-only or `:memory:` paths.
- Remove the remaining wrong-context fallback when the reader snapshot disappears.
- Silence or downgrade expected reader-only background warnings that no longer represent user-visible failures.
- Make `brain` status degrade only on real cross-process freshness failures, not on harmless internal observer misses.

**Validation:**
- targeted Rust tests for reader-only routing pass
- `status-brain.sh` remains `HEALTHY` after restart
- `brain` logs no repeated `Table ... does not exist` or equivalent false-reader errors during steady state

---

### Task 2: Productize split-aware release, preflight, and runtime command routing

**Files:**
- Modify: `scripts/release/preflight.sh`
- Modify: `scripts/release/promote_live_safe.sh`
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Modify: `src/axon-core/src/runtime_command_proxy.rs`
- Modify: `src/axon-core/src/mcp_http.rs`
- Modify: `src/axon-core/src/mcp/catalog.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver:**
- Make `live` release scripts understand split topology and artifact truth.
- Replace `simulated_local_proxy` / test-only runtime command proxy behavior with a real split-safe contract, or explicitly reject unsupported commands.
- Keep runtime-affecting command ownership explicit:
  - `brain` public surface
  - `indexer` execution authority

**Validation:**
- split-aware release/preflight checks pass on local dry-run logic
- targeted tests prove unsupported runtime commands do not pretend to proxy
- targeted tests prove supported runtime commands route explicitly and safely

---

### Task 3: Harden split lifecycle commands and qualification for both `dev` and `live`

**Files:**
- Modify: `scripts/start.sh`
- Modify: `scripts/stop.sh`
- Modify: `scripts/status.sh`
- Modify: `scripts/start-brain.sh`
- Modify: `scripts/start-indexer.sh`
- Modify: `scripts/stop-brain.sh`
- Modify: `scripts/stop-indexer.sh`
- Modify: `scripts/status-brain.sh`
- Modify: `scripts/status-indexer.sh`
- Modify: `scripts/qualify_ingestion_run.py`
- Modify: `scripts/qualify_runtime.py`

**Deliver:**
- Ensure both split roles have clean lifecycle behavior on both instances:
  - rebuild when stale
  - no phantom tmux sessions
  - no PID ambiguity
  - no wrong port-as-liveness assumptions
- Ensure qualification understands split topology without regressing to monolith assumptions.
- Remove stale assumptions such as:
  - defaulting the wrong instance
  - requiring legacy compatibility shim truth
  - treating split convergence as inherently non-converged
- Keep rollback to monolith explicit and documented, not accidental.

**Validation:**
- `dev`:
  - start both
  - status both
  - stop both
  - restart both
- `live`:
  - advisory status path is correct and split-aware
- qualification scripts emit split-aware truth surfaces without false drift

---

### Task 4: Accelerate upstream discovery into canonical file stock

**Files:**
- Modify: `src/axon-core/src/file_ingress_guard.rs`
- Modify: `src/axon-core/src/ingress_buffer.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `scripts/qualify_runtime.py`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver:**
- Make upstream behave as a real `push` system:
  - `Watcher + Scan -> buffered_discovery -> persisted_file_pending -> graph_ready`
- Decouple slow subtree/discovery walking from the hot admission flush path so canonical stock can be front-loaded quickly.
- Reduce the lag between scan/watch discovery and durable `persisted_file_pending`.
- Expose proof surfaces for:
  - discovery rate
  - admission rate
  - `persisted_file_pending` fill rate
  - `graph_ready` fill rate
- Keep watcher-hot priority without letting scan bulk fill collapse into starvation or tiny throttled batches.

**Validation:**
- targeted tests for admission hysteresis and watcher-hot priority stay green
- short runtime qualification from clean `IST` shows upstream rates materially above downstream rates at initial fill time
- `graph_ready` reaches a meaningful reserve quickly instead of filling at nearly the same pace as vector drain

---

### Task 5: Reassert graph production as the bridge stock into GPU pull

**Files:**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver:**
- Make `graph_ready` the central replenishment stock for the downstream loop.
- Keep vector control strictly downstream:
  - `graph_ready -> prepare -> ready_batches -> GPU -> finalize`
- Prevent vector-side heuristics from redefining upstream priority or graph production urgency.

**Validation:**
- tests prove no primary admission bypasses `graph_ready`
- runtime status exposes healthy separation:
  - upstream stock truth
  - downstream GPU reserve truth
- `graph_ready` can build ahead of GPU consumption on initial fill

---

### Task 6: Prove the GPU loop is truly pull-paced and kept fed

**Files:**
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `scripts/qualify_runtime.py`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver:**
- Expose and tune:
  - `ready_batches`
  - `gpu_busy_pct` or nearest existing truth surface
  - `graph_ready` reserve health
  - batching depth ahead of GPU
- Make finalize explicitly asynchronous in the surfaced loop semantics and diagnostics.
- Certify that when `graph_ready > 0`, the GPU loop pulls continuously inside VRAM/resource limits.

**Validation:**
- GPU-qualified runtime run on `dev`
- qualification artifacts show:
  - `graph_ready_to_vector_ready_per_min`
  - stable GPU activity when stock exists
  - no false blame on vector when upstream stock is absent

---

### Task 7: Run clean-state benchmark and qualification from reset `IST`

**Files:**
- Modify: `scripts/qualify_ingestion_run.py`
- Modify: `scripts/qualify_runtime.py`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`

**Deliver:**
- Establish a repeatable benchmark procedure:
  - stop `dev`
  - reset `IST`
  - restart split runtime
  - sample without false gate failures
- Make readiness stable enough that the qualification tool does not start too early or miss the real window.
- Make `qualify_ingestion_run.py` score the converged split truth correctly.
- Archive final artifacts for:
  - admission
  - graph production
  - vector throughput
  - GPU pull behavior

**Validation:**
- one clean run from reset `IST` completes without false `sql_known=ERR` style sampling drift
- archived summaries identify the dominant bottleneck honestly
- operator doc contains the repeatable benchmark sequence

---

### Task 8: Rewrite `visualize-nexus-pull.html` as the canonical macro-to-micro explainer

**Files:**
- Modify: `docs/architecture/visualize-nexus-pull.html`

**Deliver:**
- Replace the current page with a true explainer panel that can be shown to operators and buyers.
- The page must explain:
  - what Axon is
  - why Axon exists
  - what problem it solves
  - why the split exists
  - how the runtime works from macro to micro
- Include Mermaid diagrams for:
  - product vision
  - `brain` vs `indexer`
  - `push` upstream loop
  - `pull` GPU downstream loop
  - `SOLL` / `IST` ownership
- Make the output feel like a deliberate architectural poster, not a generic landing page.

**Validation:**
- HTML renders as a self-contained explainer
- Mermaid blocks are present and coherent
- narrative flows from buyer-facing value to technical decomposition

---

### Task 9: Close with operator truth and explicit defer list

**Files:**
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Modify: `docs/plans/2026-04-21-axon-final-operations-closeout-plan.md`

**Deliver:**
- Add a short explicit defer section:
  - `brain` temporary fallback `IST` writer lease is not implemented yet
  - only revisit after split + throughput + qualification are green
- Add a final operator checklist:
  - how to verify split ownership
  - how to benchmark from clean `IST`
  - how to interpret `push` vs `pull` metrics

**Validation:**
- docs clearly distinguish:
  - done now
  - deferred intentionally
- no hidden or ambiguous “future maybe” items remain in the operator story

---

## Recommended Execution Mode

- Same-session autonomous execution:
  - `idea-to-delivery`
  - `subagent-driven-development`
- Review sequence per task:
  1. implementation
  2. spec compliance review
  3. code quality review
- Do not start the fallback `IST` writer handoff until Tasks 1 through 6 are green.
