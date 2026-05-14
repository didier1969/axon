# Why Rationale Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Deliver one bounded improvement phase that hardens the MCP `why` surface, realigns the Axon skill, strengthens adjacent rationale-surface contract coverage, and performs small supporting structural cleanups in `tools_framework.rs` and `tools_soll.rs` without broad retrieval-system redesign.

**Architecture:** Keep the phase bounded to the existing rationale packet pipeline and its nearest product/documentation surfaces. Add new evidence-state and classification fields in the packet/rendering path, derive compatibility views in the summarizer, prove behavior with targeted MCP tests against representative symbols and adjacent surfaces, then land one small structural cleanup in `tools_framework.rs`, one in `tools_soll.rs`, and the matching Axon skill update. Do not change routing or large runtime/status surfaces in this phase.

**Tech Stack:** Rust, Axon MCP server, serde_json, existing MCP test suite in `src/axon-core/src/mcp/tests.rs`

---

### Task 1: Freeze the v1 contract in tests

**Files:**
- Modify: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing tests for new `why` evidence-state fields**

Add targeted tests near the current `why` assertions for:
- `authority_class`
- `evidence_provenance`
- `link_mode`
- machine-visible missing/degraded evidence states
- informational-only `rationale_quality`
- negative prose safety for inferred and weak-correlation cases

Use representative symbols:
- `runtime_topology_snapshot`
- `axon_why`
- `axon_soll_work_plan`
- `axon_soll_attach_evidence`

Also add controlled `why` test fixtures or packet-shape assertions for:
- `missing_governing_intent`
- `correlation_only_support`
- `weak_artifact_only_support`
- `direct_governing_traceability`

**Step 2: Run the targeted `why` tests to verify failure**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- failures showing the new fields are missing or the shape is still legacy-only

**Step 3: Add focused shape assertions for compatibility**

Extend tests so legacy fields still exist in v1:
- `linked_intentions`
- `supporting_artifacts`
- `missing_evidence`

but must remain derived views of the new structured output.

**Step 4: Re-run the targeted tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- still failing, but now precisely specifying the required contract

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tests.rs
git commit -m "test: freeze why rationale hardening contract"
```

### Task 2: Add evidence-state builders to the retrieval layer

**Files:**
- Modify: `src/axon-core/src/mcp/tools_context.rs:2408-2719`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Add small helper builders for evidence-state classification**

Inside `tools_context.rs`, add helpers that classify:
- `authority_class`
- `evidence_provenance`
- `link_mode`
- missing/degraded evidence states

These helpers should operate on existing packet ingredients:
- `relevant_soll_entities`
- `direct_evidence`
- `supporting_chunks`
- `retrieval_diagnostics`
- `excluded_because`

**Step 2: Extend packet assembly to emit the new structured fields**

Update the `why` packet path around:
- packet assembly
- `build_answer_sketch(...)`
- `build_missing_evidence(...)`

Add new packet fields such as:
- `governing_requirements`
- `governing_decisions`
- `supporting_guidelines`
- `supporting_docs`
- `direct_code_evidence`
- `supporting_code_context`
- `evidence_states`

Each item must carry:
- `authority_class`
- `evidence_provenance`
- `link_mode`
- `inclusion_reason`

**Step 3: Run the focused tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- fewer failures
- failures now concentrated in rendering/summarization

**Step 4: Add mandatory narrow tests for the honesty contract**

Add explicit tests in `mcp/tests.rs` for:
- missing governing intent
- correlation-only support
- weak-artifact-only support
- direct governing traceability

Each test must assert both:
- machine-visible evidence states
- absence of unqualified causal wording when `link_mode != direct`

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_context.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: add why evidence-state packet semantics"
```

### Task 3: Rework `why` rendering to derive prose from evidence states

**Files:**
- Modify: `src/axon-core/src/mcp/tools_context.rs:2619-2719`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Rewrite rendered packet sections**

Update `render_evidence_packet(...)` so the text output prefers:
- governing intent sections first
- evidence-state explanation before confidence wording
- explicit degraded/missing evidence statements

Replace the current flat emphasis on:
- `Relevant SOLL entities`
- raw `Supporting chunks`

with sections that reflect the new contract.

**Step 2: Make causal wording conditional on `link_mode`**

Ensure fields such as:
- `exists_to`
- `governed_by`
- `implemented_at`
- `confidence_reason`

only speak strongly when `link_mode=direct`, and explicitly qualify inferred or weak-correlation cases.

Add explicit assertions in `mcp/tests.rs` that:
- inferred cases do not render unqualified `governed_by`
- weak-correlation cases do not render unqualified `exists_to`
- compatibility summary text does not launder inference into fact

**Step 3: Re-run targeted tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- text-oriented `why` assertions pass or fail only on summarizer compatibility details

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tools_context.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: render why from explicit evidence states"
```

### Task 4: Rebuild the compatibility summary in `tools_framework.rs`

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs:2310-2407`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Refactor `summarize_why_response(...)`**

Keep the existing legacy shape, but derive it from the new packet fields instead of directly from:
- `relevant_soll_entities`
- raw `supporting_chunks`

Add:
- `rationale_quality`
- `confidence_reason`
- explicit `evidence_states`

Constraint:
- legacy fields stay present in v1
- legacy fields must not diverge semantically from the new structured output

**Step 2: Mark `rationale_quality` informational only**

Encode that it is:
- operator-facing
- not a stable automation contract

Do this in both machine output and any explanatory text/comments that already exist in the response assembly path.

**Step 3: Re-run focused tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- targeted `why` tests pass

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: preserve why compatibility while adding structured rationale"
```

### Task 5: Surface-qualify non-governing artifact handling

**Files:**
- Modify: `src/axon-core/src/mcp/tools_context.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Add explicit qualification for non-governing artifacts**

Do not implement full filtering or exclusion policy yet. In v1, ensure that artifacts such as:
- benchmarks
- incidental tests
- auxiliary scripts

cannot appear as ordinary support without explicit qualification in the packet or rendered text.

Constraint:
- this task is presentation-only qualification
- it must not become broad suppression/exclusion policy work
- full filtering policy remains follow-on work

Use:
- `authority_class=correlated` or `supporting`
- `evidence_provenance=benchmark|test|script`
- `link_mode=weak_correlation|inferred`

**Step 2: Assert against current bad examples**

Add tests proving that symbols such as:
- `axon_soll_work_plan`
- `axon_soll_attach_evidence`

do not render `benchmark.py` as plain normal support anymore.

**Step 3: Run the targeted tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- bad-example regression tests pass

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tools_context.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: qualify weak why support artifacts explicitly"
```

### Task 6: Run full validation for the bounded v1 surface

**Files:**
- Modify: `docs/plans/2026-04-28-why-rationale-hardening-concept.md`
- Modify: `docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Run the relevant Rust validation set**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml --bins --no-run
cargo test --manifest-path src/axon-core/Cargo.toml --lib --no-run
```

Expected:
- both commands succeed

**Step 2: Re-run targeted `why` test selectors**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
```

Expected:
- pass

**Step 3: Re-check `why` through live MCP**

Run:
```bash
./scripts/axon --instance live mcp-call call why --args '{"symbol":"runtime_topology_snapshot","project":"AXO"}' --format text
./scripts/axon --instance live mcp-call call why --args '{"symbol":"axon_why","project":"AXO"}' --format text
./scripts/axon --instance live mcp-call call why --args '{"symbol":"axon_soll_work_plan","project":"AXO"}' --format text
./scripts/axon --instance live mcp-call call why --args '{"symbol":"axon_soll_attach_evidence","project":"AXO"}' --format text
```

Expected:
- new evidence-state semantics visible
- clearer governing/supporting split
- no unqualified benchmark-based support

**Step 4: Update the concept and plan docs with actual outcomes**

Record:
- contract delivered
- residual follow-on work
- any incompatibility or false-certainty edge cases still visible

**Step 5: Commit**

```bash
git add docs/plans/2026-04-28-why-rationale-hardening-concept.md docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md src/axon-core/src/mcp/tools_context.rs src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: harden why rationale semantics"
```

### Task 7: Realign the Axon operator skill

**Files:**
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`
- Test: `docs/plans/2026-04-28-why-rationale-hardening-concept.md`
- Test: `docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md`

**Step 1: Update the `why` guidance in the skill**

Document the new `why` reading contract:
- `authority_class`
- `evidence_provenance`
- `link_mode`
- `evidence_states`

Add explicit guidance that:
- machine evidence-state fields outrank prose
- `rationale_quality` is informational only
- weak or inferred rationale must not be treated as canonical intent

**Step 2: Add degraded-state interpretation guidance**

Teach the skill how to interpret:
- `missing_governing_intent`
- `no_direct_traceability`
- `retrieval_degraded`
- `support_only`

The skill should tell agents to escalate from `why` to `inspect`, `impact`, `path`, or direct code reading when these states appear.

**Step 3: Re-read the skill for consistency**

Run:
```bash
sed -n '1,260p' docs/skills/axon-engineering-protocol/SKILL.md
```

Expected:
- the `why` guidance matches the new product contract
- no contradictory older wording remains

**Step 4: Commit**

```bash
git add docs/skills/axon-engineering-protocol/SKILL.md
git commit -m "docs: realign axon skill with why rationale contract"
```

### Task 8: Strengthen adjacent MCP rationale-surface contract tests

**Files:**
- Modify: `src/axon-core/src/mcp/tests.rs`
- Modify: `src/axon-core/src/mcp/tools_context.rs`
- Modify: `src/axon-core/src/mcp/tools_framework.rs`

**Step 1: Add minimal contract assertions for adjacent surfaces**

Add targeted tests covering neighboring surfaces that can suffer from the same false-certainty pattern:
- `retrieve_context`
- `inspect`
- `conception_view`

Focus on:
- degraded or missing evidence visibility
- absence of over-strong prose when evidence is weak
- consistency with the new `why` contract where shared packet logic is reused

**Step 2: Run the targeted adjacent-surface tests**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml retrieve_context --lib
cargo test --manifest-path src/axon-core/Cargo.toml conception_view --lib
```

Expected:
- pass, or fail in ways that identify concrete nearby false-certainty gaps to close in this phase

**Step 3: Apply only bounded fixes**

Fix only the issues that are:
- directly caused by shared rationale packet/rendering logic
- or directly exposed by this phase’s new evidence-state semantics

Do not broaden into a general retrieval redesign.

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tests.rs src/axon-core/src/mcp/tools_context.rs src/axon-core/src/mcp/tools_framework.rs
git commit -m "test: strengthen adjacent rationale surface contracts"
```

### Task 9: Land one bounded structural cleanup in `tools_framework.rs`

**Files:**
- Modify: `src/axon-core/src/mcp/tools_framework.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Choose one rationale-cluster extraction or rebucketing**

Restrict the cleanup to code directly supporting:
- `why`
- rationale summarization
- nearby evidence-state shaping

Examples of acceptable scope:
- a small helper extraction for `why` summary shaping
- a dedicated submodule for rationale-summary compatibility logic

Do not touch broad runtime/status ownership in this task.

**Step 2: Implement the minimal structural reduction**

Move only the cohesive helper cluster required to reduce local complexity while keeping the phase low-risk.

**Step 3: Run focused validation**

Run:
```bash
cargo test --manifest-path src/axon-core/Cargo.toml axon_why --lib
cargo test --manifest-path src/axon-core/Cargo.toml --bins --no-run
```

Expected:
- pass

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tools_framework.rs src/axon-core/src/mcp/tests.rs
git commit -m "refactor: reduce why rationale cluster in tools_framework"
```

### Task 10: Land one bounded structural cleanup in `tools_soll.rs`

**Files:**
- Modify: `src/axon-core/src/mcp/tools_soll.rs`
- Modify: `src/axon-core/src/mcp/tools_soll/*.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Identify one traceability-adjacent subdomain still inflating `tools_soll.rs`**

Choose a subdomain directly relevant to this phase’s rationale/traceability work.

Examples:
- additional evidence/traceability helpers
- completeness/query-context helpers closely tied to rationale

**Step 2: Extract or rebucket that one subdomain**

Keep the extraction small and cohesive. The goal is one further safe reduction, not a full decomposition.

**Step 3: Run focused validation**

Run:
```bash
cargo fmt --manifest-path src/axon-core/Cargo.toml
cargo test --manifest-path src/axon-core/Cargo.toml --bins --no-run
```

Expected:
- pass

**Step 4: Commit**

```bash
git add src/axon-core/src/mcp/tools_soll.rs src/axon-core/src/mcp/tools_soll
git commit -m "refactor: reduce traceability-adjacent tools_soll cluster"
```

### Task 11: Capture explicit residuals from the phase

**Files:**
- Modify: `docs/plans/2026-04-28-why-rationale-hardening-concept.md`
- Modify: `docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md`

**Step 1: Record residual findings**

Capture what this phase reveals about:
- `retrieve_context`
- `inspect`
- `conception_view`
- remaining `tools_framework.rs` hotspots
- remaining `tools_soll.rs` hotspots
- filtered unit-test execution caveat around `create_test_server()` and schema/bootstrap drift

Keep these as explicit residuals and follow-up work, not silent context in the engineer’s head.

**Step 2: Re-read the plan docs**

Run:
```bash
sed -n '1,260p' docs/plans/2026-04-28-why-rationale-hardening-concept.md
sed -n '1,360p' docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md
```

Expected:
- residuals and boundaries are explicit
- no accidental scope broadening is recorded as if it were delivered

**Step 3: Commit**

```bash
git add docs/plans/2026-04-28-why-rationale-hardening-concept.md docs/plans/2026-04-28-why-rationale-hardening-implementation-plan.md
git commit -m "docs: capture rationale hardening residuals"
```

## Validation Matrix

- Contract validation:
  - new evidence-state fields present
  - legacy fields preserved and derived consistently
- Honesty validation:
  - weak or inferred rationale is explicitly qualified
  - missing governing intent is machine-visible
- Regression validation:
  - representative symbols still return usable `why` output
  - no broad retrieval-system breakage implied by v1
- Adjacent-surface validation:
  - `retrieve_context`, `inspect`, and `conception_view` have minimal anti-false-certainty coverage
- Scope validation:
  - no large `tools_framework.rs` decomposition included
  - no broad traceability rewrite included
- Skill validation:
  - Axon operator skill reflects the new `why` contract and limitations
- Structural validation:
  - `tools_framework.rs` receives one bounded rationale-cluster cleanup
  - `tools_soll.rs` receives one bounded traceability-cluster cleanup
- Residual-capture validation:
  - explicit follow-up observations are recorded for adjacent surfaces and remaining hotspots
