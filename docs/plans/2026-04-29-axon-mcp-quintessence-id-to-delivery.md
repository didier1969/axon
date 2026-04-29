# Axon MCP Quintessence - ID to Delivery

Date: 2026-04-29

Status: active

Delivery id: `AXON-QUINTESSENCE-001`

SOLL anchor: `MIL-AXO-009`

Baseline assistance score: 78/100

Target assistance score: 95+/100

## Objective

Turn Axon MCP from a useful project memory into the highest-value operating layer for LLM software delivery.

The value added is explicit: Axon should make a coding LLM more reliable than filesystem plus terminal alone by giving it durable intent, current project truth, compact recovery guidance, evidence quality and an auditable delivery loop.

## Governing Decisions

- `DEC-AXO-039`: Axon MCP assistance score targets 95 plus.
- `DEC-AXO-040`: Axon quintessence is MCP-first project intelligence.

## Execution Contract

Axon MCP is dedicated to LLM clients. Its public tools must reduce token waste by returning compact, machine-actionable guidance. When truth is partial, stale or degraded, the server must say so and provide the next useful MCP move instead of forcing the LLM to inspect Axon implementation code.

Every delivery should use this loop:

1. Read runtime and project truth through MCP.
2. Create or update SOLL intent before broad implementation.
3. Implement against the smallest coherent slice.
4. Validate through MCP and local tests.
5. Attach evidence to SOLL.
6. Commit, push and run post-checks.

## Work Packages

### 1. Truth Cockpit

SOLL: `REQ-AXO-042`

Why: an LLM should not stitch low-level probes before acting.

How: enrich `status` and `project_status` with current blocker, next best action, confidence, freshness, proof gaps and compact recovery fields.

Progress: planned.

Acceptance criteria:

- `status(mode="brief")` exposes the next best MCP action and runtime trust boundary.
- `project_status(mode="brief")` exposes project blocker, confidence and freshness.
- Degraded IST/SOLL state is explicit and actionable.

### 2. Guidance Engine

SOLL: `REQ-AXO-043`

Why: client LLMs should recover through Axon instead of wasting tokens or reading Axon implementation code.

How: make every public MCP tool return compact `next_action` and `operator_guidance` for invalid arguments, empty results, degraded truth and ambiguous targets.

Progress: planned.

Acceptance criteria:

- Bad arguments return a repair instruction and one retry path.
- Empty or ambiguous results return follow-up tools.
- Degraded answers preserve evidence and state limits.

### 3. Evidence Grader

SOLL: `REQ-AXO-044`

Why: attached evidence alone is insufficient; Axon must know whether proof is strong, weak, stale, broken or missing.

How: extend SOLL verification to classify evidence quality and freshness, then surface those states in `soll_validate`, `soll_verify_requirements` and `soll_work_plan`.

Progress: planned.

Acceptance criteria:

- Requirement coverage distinguishes missing, weak, stale, broken and strong evidence.
- Broken file evidence is reported with repair guidance.
- Work planning prioritizes high-impact missing proof.

### 4. Executable SOLL Plans

SOLL: `REQ-AXO-045`

Why: long-running LLM work needs durable state, not chat memory.

How: represent plans as SOLL objects with tasks, why/how, progress, blockers, gates, evidence and completion state.

Progress: planned.

Acceptance criteria:

- `soll_apply_plan` can create and update a multi-node delivery plan idempotently.
- `soll_work_plan` returns actionable wave ordering and gates.
- Plan state is inspectable without reading chat history.

### 5. Fresh IST Contract

SOLL: `REQ-AXO-046`

Why: an LLM must know when it can trust structural answers without rereading code.

How: expose IST freshness, staleness and recovery guidance consistently across public read tools.

Progress: planned.

Acceptance criteria:

- Public read tools expose freshness state when using indexed projections.
- Stale projections are labelled as partial or degraded.
- Recovery guidance names the next MCP action.

### 6. LLM Help

SOLL: `REQ-AXO-047`

Why: MCP usability must be encoded in the server, not in human memory.

How: make `help` a compact LLM routing brain that describes the smallest useful tool chain, skill existence, bad-args repair and escalation boundary.

Progress: planned.

Acceptance criteria:

- `help(tool=...)` returns input schema, next action and LLM usage instruction.
- `help` references the Axon engineering skill where relevant.
- The output is optimized for LLM routing rather than human tutorials.

### 7. Delivery Closure

SOLL: `REQ-AXO-048`

Why: Axon proves its value only when long deliveries finish cleanly.

How: standardize implement, validate, attach evidence, update SOLL, commit, push and post-check as one audited procedure.

Progress: planned.

Acceptance criteria:

- A delivery can be closed with SOLL evidence, validation output and git state.
- The procedure catches missing evidence before final completion.
- Promotion status is reported with explicit residual risks.

## Current Findings

- Axon MCP live is reachable and exposes the public LLM surface.
- Runtime truth is `brain_only`; public read availability is correct for current policy.
- IST freshness is currently degraded/stale, so project-wide structural conclusions must remain labelled partial until the indexer refresh contract is healthy.
- TensorRT artifact build is still running separately and is not yet a completed proof for vector throughput.

## Next Delivery Slice

The first implementation slice should be `REQ-AXO-042` plus `REQ-AXO-043`: truth cockpit and guidance engine. These two changes create the highest immediate leverage because they improve every subsequent LLM interaction with Axon.
