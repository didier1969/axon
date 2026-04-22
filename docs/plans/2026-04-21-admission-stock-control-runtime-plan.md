# Admission-First Stock-Control Runtime Plan

> **For Claude:** REQUIRED SUB-SKILL: Use `idea-to-delivery` / `consensus-driven-delivery` for execution and review checkpoints. Do not treat this plan as a justification for more downstream heuristics before upstream admission truth is fixed.

**Goal:** Replace the current weak-gain optimization pattern with a control model that maximizes end-to-end work conversion. The primary objective is to move work durably through the canonical stage boundaries:
- `buffered_discovery -> persisted_file`
- `persisted_file -> graph_ready`
- `graph_ready -> vector_ready`

**Why this plan replaces the current framing**
- Repeated runtime evidence shows `scan_buffered` can remain high and flat while `persisted_file_current` remains flat and `structural_graph_backlog` is often zero.
- That means the dominant choke point is upstream of graph/vector arbitration.
- Optimizing GPU fill, semantic batch shape, or graph projection behavior before fixing canonical admission produces high engineering effort and weak throughput gains.

**Architecture Decision**
- Use a **thin central admission controller** for inter-stage handoff and WIP budgets.
- Use **stock-control** at each canonical boundary with target bands, reorder points, and max WIP.
- Keep **local autonomy only inside a stage** for execution efficiency.
- Demote secondary/derived queues such as `GraphProjectionQueue` to diagnostic or execution-local status. They must not define the product-level control loop.
- Keep downstream heuristic layers such as `optimizer.rs` out of the authority path until deterministic admission semantics exist.

**Control layers**
1. `supply/discovery`
   - canonical boundary: `buffered_discovery -> persisted_file_pending`
   - responsible for creating upstream canonical work
   - operates as a `push` loop
2. `admission/production`
   - canonical boundary: `persisted_file_pending -> graph_ready`
   - responsible for turning pending canonical file work into graph-ready stock
   - continues the upstream `push` loop
3. `gpu-paced downstream`
   - canonical boundary: `graph_ready -> vector_ready`
   - responsible for downstream CPU prepare, ready reserve, GPU execution, and finalize
   - operates as a `pull` loop paced by real GPU demand

**Loop semantics**
- `Watcher + scan -> graph_ready` is a `push` system. It should create and replenish stock as quickly as upstream resources allow, without waiting for GPU cadence.
- `graph_ready -> vector_ready` is a `pull` system. It should wake, size, and drain work according to GPU/VRAM availability and may idle cleanly when `graph_ready = 0`.
- `finalize` is asynchronous. It must not sit on the hot GPU feed path unless a hard safety invariant requires it.

**Central stock doctrine**
- `persisted_file_pending` remains the critical throughput stock for the full system until runtime evidence proves otherwise.
- `graph_ready` is the central stock of the downstream GPU-fed production loop, not the central stock of the entire system.
- Improved `graph_ready` behavior must not be used to explain away missing upstream supply or absent `persisted_file_pending`.

**Canonical stage boundaries**
1. `buffered_discovery`
2. `persisted_file_pending`
3. `graph_ready`
4. `vector_ready`

**Canonical admission completion**
- A unit leaves `buffered_discovery` only when its canonical `File` row is durably persisted and marked as eligible pending work.
- `persisted_file_pending` means: durably persisted, not deleted/skipped/excluded, not yet `graph_ready`, and still eligible for graph production.

**Non-goals**
- Do not solve this by adding more downstream vector heuristics first.
- Do not treat `GraphProjectionQueue` as the canonical graph backlog.
- Do not make every lane globally autonomous before admission truth and ownership are explicit.
- Do not promote `graph_ready` to the primary global bottleneck surface while upstream supply/admission remains the dominant blocker in runtime evidence.

---

## Task 0: Freeze the new control doctrine in status and docs

**Files**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- `runtime_authority.proposed_control_model = admission_first_stock_control`
- explicit canonical boundaries:
  - `buffered_discovery`
  - `persisted_file_pending`
  - `graph_ready`
  - `vector_ready`
- explicit statement that:
  - `GraphProjectionQueue` is secondary
  - vectorization is downstream-only
  - the primary admission edge is `buffered_discovery -> persisted_file`
  - queue depth must not regain primary control status once `persisted_file_pending` is available
  - the upstream loop is `push`, the GPU-facing loop is `pull`, and `finalize` is asynchronous

**Validation**
- add/extend a status test proving the control model and stage boundaries are exposed canonically
- do not promote `proposed_control_model` to final `control_model` until Task 1 and Task 2 are green

---

## Task 1: Instrument the true choke point at the admission edge

**Files**
- Modify: `src/axon-core/src/ingress_buffer.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `scripts/qualify_runtime.py`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- canonical telemetry for the admission edge:
  - `buffered_discovery_current`
  - `persisted_file_current`
  - `persisted_file_pending_current`
  - `admission_flush_count`
  - `admission_promoted_total`
  - `admission_last_promoted_count`
  - `admission_last_flush_duration_ms`
  - `admission_blocking_authority`
  - `admission_wip_current`
  - `admission_completion_surface`
- explicit diagnostic split between:
  - flush happened
  - durable `File` persistence completed
  - persistence completed but file was excluded from `persisted_file_pending`
- qualification summary that can answer:
  - is the system flushing?
  - is flush converting into `File`?
  - is the upstream buffer refilling faster than promotion?
  - what authority currently blocks the handoff?

**Validation**
- targeted status test for canonical admission telemetry
- `python3 -m py_compile scripts/qualify_runtime.py`
- one short runtime qualification showing whether the flatline sits in flush, persistence, or refill

---

## Task 2: Introduce a real admission controller for `buffered_discovery -> persisted_file`

**Files**
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- a single authority for the edge `buffered_discovery -> persisted_file`
- explicit stock parameters:
  - `admission_target_band`
  - `admission_reorder_point`
  - `admission_max_wip`
  - `admission_hold_window_ms`
  - `forced_bulk_fill_threshold`
- admission decisions based on stock deficit and blocking authority, not just timer cadence or downstream heuristics
- source-aware handling:
  - watcher-hot entries still get priority
  - scan backlog can bulk-fill when canonical stock is below target
- explicit anti-thrash behavior:
  - watcher-hot priority must not collapse scan bulk fill
  - scan bulk fill must not starve watcher-hot entries
  - hold windows and hysteresis must be visible in status

**Validation**
- failing then passing test proving:
  - hot watcher entries stay prioritized
  - large scan backlog can still bulk-fill the canonical workbase
  - the controller exposes why it is not admitting more when blocked

---

## Task 3: Make `persisted_file -> graph_ready` the primary production stage

**Files**
- Modify: `src/axon-core/src/graph_ingestion.rs`
- Modify: `src/axon-core/src/main_background.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- graph production is controlled as stock conversion:
  - `persisted_file_pending_current`
  - `graph_ready_current`
  - `graph_wip_current`
  - `graph_blocking_authority`
- graph work becomes the dominant production stage whenever `persisted_file_pending` exceeds target
- semantic/vector keeps only a bounded downstream floor
- recovery and repair paths remain fallback only
- explicit rule: `GraphProjectionQueue` depth may not be used as the primary control input when `persisted_file_pending_current` is available
- explicit rule: `graph_ready` is the replenishment stock for the GPU-facing loop, but it must not replace `persisted_file_pending` as the primary global throughput stock while upstream supply remains the dominant blocker

**Validation**
- failing then passing test showing:
  - graph production dominates when `persisted_file_pending` is above target
  - vector lane retains only its minimum floor
  - the state does not chatter between modes sample-to-sample

---

## Task 4: Reduce vector control to downstream stock management

**Files**
- Modify: `src/axon-core/src/vector_control.rs`
- Modify: `src/axon-core/src/graph_bootstrap.rs`
- Modify: `src/axon-core/src/service_guard.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- vector lane becomes explicitly downstream of `graph_ready`
- startup backfill remains bounded
- vector controller only manages:
  - `graph_ready -> vector_ready`
  - CPU prep reserve
  - GPU feed quality
  - persist pressure safety
- vector lane behaves as a GPU-paced `pull` loop and may idle when `graph_ready` is empty
- finalize remains asynchronous and outside the GPU hot path
- vector lane no longer implicitly defines product priority
- vector tuning may optimize only inside a fixed admitted stock budget and may not redefine upstream admission priority or WIP policy

**Validation**
- targeted tests proving:
  - no primary admission path bypasses `graph_ready`
  - startup cannot flood semantic work beyond the configured floor while upstream stock is missing

---

## Task 5: Make blocking authority first-class everywhere

**Files**
- Modify: `src/axon-core/src/runtime_profile.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Modify: `scripts/qualify_runtime.py`
- Test: `src/axon-core/src/mcp/tests.rs`

**Deliver**
- every canonical edge exposes:
  - `owner`
  - `blocking_authority`
  - `allowed_by_contract`
  - `allowed_under_current_runtime`
- examples of blocking authority:
  - `admission_flush_budget_exhausted`
  - `persistence_congested`
  - `graph_wip_cap_reached`
  - `interactive_guard_hold`
  - `vector_floor_reserved`

**Validation**
- status tests proving blocking authorities are explicit and machine-readable
- qualification summary that can identify the dominant blocking authority over a run window

---

## Task 6: Re-qualify the whole system against the new objective

**Files**
- Modify: `scripts/qualify_runtime.py`
- Modify: `docs/operations/2026-04-18-live-dev-runtime-operations.md`
- Optional: `docs/plans/2026-04-20-watcher-first-graph-vector-priority-implementation-plan.md`

**Deliver**
- qualification verdicts based on end-to-end conversion, not GPU symptoms
- required reported rates:
  - `buffered_to_persisted_per_min`
  - `persisted_to_graph_ready_per_min`
  - `graph_ready_to_vector_ready_per_min`
- diagnosis vocabulary updated to match the new model:
  - `admission_limited`
  - `persistence_limited`
  - `graph_production_limited`
  - `vector_downstream_limited`

**Validation**
- one `dev/full` run proving the new model can distinguish:
  - flush not happening
  - flush happening but not persisting
  - persist succeeding but graph lagging
  - graph ready healthy while vector remains downstream-limited

---

## Execution order
1. Task 0
2. Task 1
3. Task 2
4. Task 5
5. Task 3
6. Task 4
7. Task 6

This order is intentional. We do not optimize graph/vector control further until the admission edge is instrumented and governed.

## Review gate
- Require one continuity reviewer and one fresh reviewer before implementation starts.
- Reject the plan if reviewers conclude the current evidence still does not justify admission-first control.

## Success criteria
- `scan_buffered` no longer stays high and flat without an explicit blocking authority
- `persisted_file_pending_current` remains the controllable throughput stock for the full system unless future evidence disproves it
- `graph_ready_current` becomes the controllable downstream stock for GPU feeding, not a replacement for upstream supply/admission truth
- `persisted_file_pending_current` becomes the controllable stock for graph production, not `persisted_file_current` alone
- graph production is measured as conversion into `graph_ready`, not inferred from projection queues
- vector throughput becomes a downstream quality problem, not the leading product objective
- bounded envelopes exist for:
  - maximum buffered dwell time
  - maximum `persisted_file_pending` age
  - admission hold duration
  - scan backlog growth before forced bulk fill
