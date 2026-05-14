# Client Satisfaction 99 Plan Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Raise Axon MCP from strong delivery quality to a product state that is plausibly perceived as `99% satisfying` by a demanding autonomous client.

**Architecture:** This plan does not assume a rewrite. It extends the already-delivered TE2 and product-closure waves by standardizing the remaining weaker public surfaces, finishing end-to-end async/public coherence, and validating the result through client-real qualification scenarios that mirror long autonomous sessions. The plan keeps one canonical server truth, one async follow-up model, and one client-facing contract family.

**Tech Stack:** Rust (`axon-core` MCP surfaces), Python (`scripts/mcp_validate.py`, qualification runners), shell runtime wrappers, SOLL-derived docs, Axon MCP public contract tests

---

## Success Criteria

This plan is complete when all of the following are true:

- the remaining public tools with weaker UX contracts expose machine-readable next actions, blockers, and remediation where appropriate
- async/public flows are uniformly self-guiding from acceptance to terminal result
- client-real qualification covers not only happy-path tool calls, but also long-session reconstruction, SOLL mutation follow-up, and recovery/error guidance
- a demanding client is unlikely to encounter a structurally ambiguous or opaque MCP interaction during normal autonomous use

## Scope

Included:

- remaining public-surface standardization
- async/public coherence completion
- external qualification expansion
- documentation and skill updates that reflect the delivered reality

Excluded:

- broad runtime/orchestrator optimization unrelated to MCP client satisfaction
- one-off TE2-specific patches
- speculative redesign of the SOLL model

## Workstream A: Public Surface Standardization

### Task A1: Inventory the remaining weaker public surfaces

**Files:**
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`
- Inspect: `src/axon-core/src/mcp.rs`
- Inspect: `src/axon-core/src/mcp/tools_context.rs`
- Inspect: `src/axon-core/src/mcp/tools_framework.rs`
- Inspect: `src/axon-core/src/mcp/tools_soll.rs`
- Inspect: `scripts/mcp_validate.py`

**Step 1: Build a public-tool contract inventory**

List each current public tool and classify it as:

- `strong_contract`
- `acceptable_contract`
- `weak_contract`

Capture for each tool whether it already exposes:

- machine-readable status
- `next_action`
- blocking factors
- remediation guidance
- stable aliases for deep result data
- rationale or provenance hints

**Step 2: Identify the weakest remaining tools**

Select only tools that still fall short of the best current standard, especially tools likely to be touched by autonomous usage:

- `project_status`
- `inspect`
- `impact`
- `path`
- `anomalies`
- `project_registry_lookup`
- any SOLL public tool still missing corrective guidance or digestibility

**Step 3: Freeze the target contract shape**

Define a minimal reusable pattern for weak tools:

- `operator_guidance`
- `next_action`
- `actionable_now`
- `blocking_factors`
- `remediation_actions`

Only apply it where it materially improves client autonomy.

**Step 4: Record the inventory in the plan**

Add a short completion note under this task listing:

- which tools are already strong
- which tools require work in Tasks A2-A5

Completion note:

- strong_contract:
  - `status`
  - `mcp_surface_diagnostics`
  - `change_safety`
  - `job_status`
  - core `retrieve_context` rationale/wiring flows
- acceptable_contract:
  - `conception_view`
  - `soll_query_context`
  - `soll_verify_requirements`
  - `soll_apply_plan`
- weak_contract to raise next:
  - `project_status`
  - `anomalies`
  - `inspect`
  - `impact`
  - `path`
  - `project_registry_lookup`

### Task A2: Upgrade `project_status` to a stronger client contract

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Write failing tests for richer `project_status` fields**

Add tests asserting `project_status` exposes a machine-readable guidance block, for example:

- `operator_guidance`
- `next_action`
- `blocking_factors`
- `remediation_actions`

**Step 2: Run the targeted failing test**

Run:

```bash
cargo test --manifest-path src/axon-core/Cargo.toml test_status_reports_public_surface_and_runtime_truth -- --test-threads=1
```

Expected:

- a failing assertion for the new `project_status` contract fields

**Step 3: Implement the minimal richer contract**

Add only fields that help a client decide what to do next from `project_status`, without duplicating `status`.

**Step 4: Re-run the targeted tests**

Run the same targeted Rust tests plus any new `project_status`-specific test.

**Step 5: Extend client qualification**

Update `scripts/mcp_validate.py` so `project_status` fails if the new contract disappears.

Execution note:

- delivered:
  - `project_status.operator_guidance`
  - `project_status.next_action`
  - validator coverage for the richer contract
- defended by:
  - `test_project_status_assembles_live_project_situation_from_read_surfaces`
  - `test_status_reports_public_surface_and_runtime_truth`

### Task A3: Upgrade `inspect` and `impact` for client actionability

**Files:**
- Modify: `src/axon-core/src/mcp/tools_context.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Add failing tests**

For `inspect` and `impact`, require a compact decision aid such as:

- `next_action`
- `follow_up_tools`
- `confidence`
- `blocking_factors` when traceability or validation is weak

**Step 2: Implement the minimal reusable contract**

Prefer a shared helper if the same guidance shape appears in both tools.

**Step 3: Run targeted tests**

Use Rust targeted tests first, then `cargo fmt`.

**Step 4: Extend MCP validation**

Teach `scripts/mcp_validate.py` to assert these new fields in client-real runs.

Execution note:

- partial delivery completed on `inspect`:
  - structured `data.summary`
  - `operator_guidance`
  - `next_action`
  - `symbol_found`
- defended by:
  - `test_axon_inspect`
  - `test_axon_inspect_warns_when_symbol_is_degraded`
- `impact` delivered in the following tranche:
  - structured `summary`
  - `operator_guidance`
  - `next_action`
  - explicit degraded contract when impact truth is unavailable
- `path` delivered in the following tranche:
  - `operator_guidance`
  - `next_action`
- `project_registry_lookup` delivered in the following tranche:
  - `operator_guidance`
  - `next_action`

### Task A4: Upgrade `path` and `anomalies` for corrective usage

**Files:**
- Modify: `src/axon-core/src/mcp/tools_context.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Add failing tests**

For `path`, require a clearer follow-up contract when the source/sink topology is incomplete or ambiguous.

For `anomalies`, require explicit distinction between:

- heuristic findings
- canonical blockers
- recommended repair route

**Step 2: Implement without overloading the payload**

Do not bloat text sections. Add compact machine-readable fields only where they reduce ambiguity.

**Step 3: Validate in Rust and client qualification**

Run targeted Rust tests and update `scripts/mcp_validate.py`.

### Task A5: Standardize any remaining SOLL public weak points

**Files:**
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Re-scan the remaining SOLL public tools**

Focus on public tools still lacking strong corrective output after the TE2 wave.

**Step 2: Add only high-leverage fields**

Examples:

- `next_action`
- `result_digest`
- `repair_guidance`
- `identity_hints`
- `follow_up_contract`

**Step 3: Lock these with tests and qualification**

Any new field added here must be defended both in Rust tests and in client qualification.

## Workstream B: Async/Public Coherence Completion

### Task B1: Inventory every async-capable public flow

**Files:**
- Inspect: `src/axon-core/src/mcp.rs`
- Inspect: `src/axon-core/src/mcp/tests.rs`
- Inspect: `scripts/mcp_validate.py`
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`

**Step 1: Enumerate the async allowlist as served by the runtime**

Record which tools can produce async acceptance and which terminal results they expose.

**Step 2: Compare acceptance vs terminal follow-up**

Verify every async tool can be consumed with the same model:

- acceptance IDs
- polling guidance
- terminal `next_action`
- terminal `result_data`
- recovery guidance on failure

**Step 3: Record any divergence in the plan**

List the exact tools to fix in Tasks B2-B4.

Completion note:

- current async parity is already strong on:
  - acceptance `known_ids`
  - `next_action`
  - `result_contract`
  - `polling_guidance`
  - `recovery_hint`
  - terminal `result_data`
- no major field-parity divergence remains across the current allowlist
- the next async/public wave is therefore:
  - stronger failure-path consistency
  - richer external qualification scenarios
  - protection against regression from client-real validation

### Task B2: Normalize terminal result contracts across async tools

**Files:**
- Modify: `src/axon-core/src/mcp.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Add failing tests for async divergence**

Require all async public tools to expose:

- `known_ids`
- `next_action`
- `result_contract`
- `polling_guidance`
- `recovery_hint`
- `result_data`

**Step 2: Implement any missing normalization**

Centralize behavior in shared async helpers where possible.

**Step 3: Re-run targeted async tests**

Use the existing async tests first, then add tool-specific assertions where needed.

### Task B3: Normalize failure and recovery semantics

**Files:**
- Modify: `src/axon-core/src/mcp.rs`
- Test: `src/axon-core/src/mcp/tests.rs`
- Validate: `scripts/mcp_validate.py`

**Step 1: Add failing tests for failure-path guidance**

Require failed async jobs to expose a consistent corrective path, not only an error string.

**Step 2: Implement the shared failure contract**

Ensure failures converge on:

- `next_action.kind = fix_and_retry_original_mutation`
- stable recovery guidance
- relevant IDs preserved when possible

**Step 3: Defend it in client qualification**

Where practical, synthesize or simulate a recoverable failure path in validation.

### Task B4: Document the canonical async contract

**Files:**
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`

**Step 1: Update the skill**

Document the final canonical async/public pattern once the code is stable.

**Step 2: Record the delivered invariants in this plan**

Summarize the contract clients can rely on.

## Workstream C: Client-Real Qualification Expansion

### Task C1: Expand `mcp_validate.py` from surface smoke to autonomy-oriented checks

**Files:**
- Modify: `scripts/mcp_validate.py`
- Validate: `/tmp/mcp-validate-core-client-closure.json`

**Step 1: Add richer assertions for newly standardized tools**

Require the new public fields added in Workstream A.

**Step 2: Add explicit qualification sections**

Group checks by:

- public contract quality
- async/public coherence
- SOLL reconstruction quality
- recovery guidance quality

**Step 3: Preserve machine-readable output**

Ensure the JSON report remains stable and useful for automated comparison.

### Task C2: Add long-session reconstruction scenarios

**Files:**
- Modify: `scripts/mcp_validate.py`
- Inspect: `docs/plans/2026-04-20-te2-operator-feedback-delta.md`
- Inspect: `docs/plans/2026-04-18-llm-facing-mcp-ux-hardening-implementation-plan.md`

**Step 1: Define realistic client sequences**

Examples:

- discover project identity
- reconstruct intent from SOLL
- inspect rationale and topology
- verify requirements
- plan a guarded mutation
- follow async completion

**Step 2: Encode those sequences as validation scenarios**

Each scenario should fail if the client would be forced into guesswork.

**Step 3: Keep the scenarios generic**

Do not hardcode TE2-only assumptions; use AXO as a proof project, but keep the logic product-generic.

### Task C3: Add regression scenarios for stale client bindings and recovery

**Files:**
- Modify: `scripts/mcp_validate.py`
- Inspect: `src/axon-core/src/mcp/tools_framework.rs`
- Inspect: `src/axon-core/src/mcp.rs`

**Step 1: Add scenarios around `mcp_surface_diagnostics`**

Require useful recovery when the client binding and server truth drift.

**Step 2: Add scenarios around missing proof / missing traceability**

Require client-facing corrective guidance instead of ambiguous refusal.

**Step 3: Preserve fast feedback**

Keep the validator useful as a gate, not just as a giant exhaustive harness.

### Task C4: Produce a canonical closing qualification run

**Files:**
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`
- Validate: `/tmp/mcp-validate-core-client-99.json`

**Step 1: Run the expanded validator against `dev`**

Command target:

```bash
python3 scripts/mcp_validate.py --url http://127.0.0.1:44139/mcp --surface core --project AXO --timeout 30 --top-slowest 5 --json-out /tmp/mcp-validate-core-client-99.json
```

**Step 2: Require clean pass**

Expected:

- `fail=0`
- `transport_health=pass`
- `semantic_quality=pass`

**Step 3: Summarize remaining gaps**

Only leave gaps that are truly polish-level, not structural contract defects.

## Workstream D: Final Product Closure Readout

### Task D1: Update the closure docs

**Files:**
- Modify: `docs/plans/2026-04-20-client-satisfaction-product-closure-plan.md`
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`

**Step 1: Mark delivered workstreams**

Set the final status based on the executed reality.

**Step 2: Record proof artifacts**

Reference the qualification JSON outputs and the major tests used to defend the delivery.

### Task D2: Update the skill to match delivered reality

**Files:**
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`

**Step 1: Add only truly delivered product behavior**

Do not document aspirations.

**Step 2: Keep the skill concise and canonical**

Reflect the final public contract shape and qualification expectations.

### Task D3: Write the final client-satisfaction assessment

**Files:**
- Modify: `docs/plans/2026-04-20-client-satisfaction-99-plan.md`

**Step 1: Record the final estimated satisfaction**

State:

- what makes `99%` plausible
- what still prevents `100%`

**Step 2: Distinguish structural closure from future polish**

Make it explicit whether the next wave is optional polish or still required product repair.

## Current Proof

Client-real MCP qualification after this tranche:

- `/tmp/mcp-validate-core-pre99-tranche1.json`
- `20/20 ok`
- `transport_health=pass`
- `semantic_quality=pass`

Client-real MCP qualification after the current closure tranche:

- `/tmp/mcp-validate-core-pre99-tranche3.json`
- `20/20 ok`
- `transport_health=pass`
- `semantic_quality=pass`

## Closure Readout

Delivered in the `99%` wave so far:

- `project_status` raised to a prescriptive contract
- `inspect` raised to a structured autonomous contract
- `impact` raised to a structured autonomous contract
- `path` raised to a structured autonomous contract
- `project_registry_lookup` raised to a structured autonomous contract
- client-real qualification updated to guard these contracts end to end

Residual gaps after this wave are no longer the same class of product defect.
They are mostly:

- further qualification scenario expansion
- polish on remaining non-core surfaces
- optional consistency work beyond the main `core` public path
