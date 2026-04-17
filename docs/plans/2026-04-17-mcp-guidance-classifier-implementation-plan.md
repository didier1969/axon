# MCP Guidance Classifier Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** add a first narrow declarative guidance classifier for Axon MCP so `query` and `inspect` return compact, consistent, action-oriented guidance in degraded or ambiguous cases without turning MCP into a second protocol.

**Architecture:** keep runtime truth, query execution, and final MCP rendering in Rust; introduce a small normalized-facts extraction layer plus a guidance classifier that emits stable semantic keys only. Phase 1 is shadow-mode first and limited to `query` and `inspect`, with validation against a golden corpus before any broader rollout.

**Tech Stack:** Rust, Axon MCP core, DuckDB-backed runtime facts, JSON response assembly, test fixtures in `mcp/tests.rs`, Python evaluation scripts for replay/golden validation.

---

### Task 1: Freeze The Contract And Taxonomy

**Files:**
- Modify: `docs/plans/2026-04-17-mcp-guidance-declarative-reasoning-concept.md`
- Create: `docs/plans/2026-04-17-mcp-guidance-taxonomy.md`

**Step 1: Write the failing contract expectations as doc assertions**

Document the phase-1 contract explicitly:

- core fields: `status`, `problem_class`, `summary`, `next_best_actions`, `confidence`
- optional fields: `scope`, `canonical_sources`, `likely_cause`, `soll`
- `soll` subfields: `recommended_action`, `update_kind`, `reason`, `requires_authorization`

Define the initial `problem_class` set for `query` and `inspect` only:

- `none`
- `input_not_found`
- `input_ambiguous`
- `wrong_project_scope`
- `tool_unavailable`
- `index_incomplete`
- `vectorization_incomplete`
- `missing_rationale_in_soll`
- `intent_missing_in_soll`
- `backend_pressure`

**Step 2: Save the taxonomy and contract document**

Run:

```bash
sed -n '1,220p' docs/plans/2026-04-17-mcp-guidance-taxonomy.md
```

Expected: document exists and clearly freezes phase-1 classes, fields, and non-goals.

**Step 3: Commit**

```bash
git add docs/plans/2026-04-17-mcp-guidance-declarative-reasoning-concept.md docs/plans/2026-04-17-mcp-guidance-taxonomy.md
git commit -m "docs: freeze mcp guidance taxonomy"
```

### Task 2: Add Guidance Fact Types And Response Helpers

**Files:**
- Create: `src/axon-core/src/mcp/guidance.rs`
- Modify: `src/axon-core/src/mcp.rs`
- Modify: `src/axon-core/src/mcp/dispatch.rs`

**Step 1: Write the failing tests for the helper layer**

Add focused unit tests in `src/axon-core/src/mcp/tests.rs` or a new module for:

- response helper omits guidance when `problem_class = none`
- response helper includes compact guidance fields only when present
- SOLL block requires explicit `requires_authorization`

Example test skeleton:

```rust
#[test]
fn guided_response_omits_guidance_block_when_problem_class_is_none() {
    let response = build_guided_response(GuidanceOutcome::none(), json!({"status": "ok"}));
    assert!(response.get("problem_class").is_none());
    assert!(response.get("next_best_actions").is_none());
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test guided_response_ -- --test-threads=1
```

Expected: FAIL because helper types/functions do not exist yet.

**Step 3: Write minimal implementation**

In `guidance.rs`, define:

- `GuidanceFact`
- `GuidanceOutcome`
- `SollGuidance`
- helper constructors like `none()`
- a compact response assembler that merges tool payload + guidance payload

Keep this file free of tool-specific branching.

**Step 4: Wire the new module**

Expose the module from `mcp.rs` or the correct MCP module root so tool files can call it.

**Step 5: Run test to verify it passes**

Run:

```bash
cargo test guided_response_ -- --test-threads=1
```

Expected: PASS

**Step 6: Commit**

```bash
git add src/axon-core/src/mcp/guidance.rs src/axon-core/src/mcp.rs src/axon-core/src/mcp/dispatch.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: add mcp guidance helpers"
```

### Task 3: Introduce Phase-1 Fact Extraction For Query And Inspect

**Files:**
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Modify: `src/axon-core/src/mcp/guidance.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing tests for normalized fact extraction**

Add tests for these situations:

- `query` exact symbol miss with suggestions
- `inspect` symbol miss with canonical project supplied
- duplicate symbol names across projects resolved to ambiguity
- invalid project scope returns canonical code hints
- degraded/index-partial result emits incomplete-index facts

Prefer one test per case.

Example skeleton:

```rust
#[test]
fn query_guidance_facts_capture_exact_symbol_miss_with_suggestion() {
    let facts = extract_query_guidance_facts(/* fixture result */);
    assert!(facts.contains(&GuidanceFact::problem_signal("input_not_found")));
    assert!(facts.contains(&GuidanceFact::candidate_symbol("Axon.Scanner.scan")));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test query_guidance_facts_ -- --test-threads=1
cargo test inspect_guidance_facts_ -- --test-threads=1
```

Expected: FAIL because extractors do not exist.

**Step 3: Implement minimal fact extraction**

In `tools_dx.rs`, after existing query/inspect result computation:

- extract phase-1 normalized facts
- include:
  - resolved project scope
  - candidate symbol names
  - empty/degraded result signals
  - index/vector partial signals when already known by tool
  - canonical sources when present

Do not yet change the user-visible response.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test query_guidance_facts_ -- --test-threads=1
cargo test inspect_guidance_facts_ -- --test-threads=1
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/guidance.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: extract guidance facts for query and inspect"
```

### Task 4: Implement The First Guidance Classifier In Shadow Mode

**Files:**
- Modify: `src/axon-core/src/mcp/guidance.rs`
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing classifier tests**

Add tests for classifier outputs:

- exact symbol miss + candidate => `input_not_found` + `retry_with_suggested_symbol`
- duplicate matches => `input_ambiguous`
- bad project code => `wrong_project_scope`
- degraded result => `index_incomplete` or `vectorization_incomplete`
- evidence found without rationale marker => `missing_rationale_in_soll`

Example skeleton:

```rust
#[test]
fn classify_guidance_marks_wrong_project_scope() {
    let outcome = classify_guidance(&facts_for_wrong_scope());
    assert_eq!(outcome.problem_class.as_deref(), Some("wrong_project_scope"));
    assert!(outcome.next_best_actions.contains(&"use_canonical_project_code".into()));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test classify_guidance_ -- --test-threads=1
```

Expected: FAIL because classifier does not exist or does not classify correctly.

**Step 3: Implement minimal classifier**

In `guidance.rs`:

- implement a small, explicit phase-1 classifier over normalized facts
- emit only semantic keys:
  - `problem_class`
  - `likely_cause`
  - `next_best_actions`
  - optional SOLL recommendation block

Important:

- no prose generation in classifier
- no direct SOLL mutations
- default output is `none`

**Step 4: Add shadow-mode plumbing**

In `tools_dx.rs`:

- run the classifier in shadow mode first
- store the classifier output in a debug-only or explicitly shadow-only field such as `guidance_shadow`
- do not make it authoritative yet

Guard with an env flag or runtime toggle such as:

- `AXON_MCP_GUIDANCE_SHADOW=1`

**Step 5: Run tests to verify they pass**

Run:

```bash
cargo test classify_guidance_ -- --test-threads=1
```

Expected: PASS

**Step 6: Commit**

```bash
git add src/axon-core/src/mcp/guidance.rs src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: add shadow guidance classifier for query and inspect"
```

### Task 5: Add Golden Corpus And Replay Harness

**Files:**
- Create: `scripts/mcp_guidance_goldens.json`
- Create: `scripts/qualify_mcp_guidance.py`
- Modify: `scripts/axon`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write the golden corpus**

Create at least 10 cases:

- query not found with suggestion
- query ambiguous
- query wrong project scope
- inspect exact success no guidance
- inspect degraded result
- inspect missing rationale in SOLL
- tool unavailable in runtime profile
- incomplete index
- incomplete vectorization
- backend pressure

For each case, freeze:

- input tool and args
- fixture/setup reference
- expected `problem_class`
- expected next action keys
- whether guidance should be absent

**Step 2: Write failing replay script behavior**

`qualify_mcp_guidance.py` should:

- replay goldens against MCP or fixture outputs
- compare actual classifier output to expected semantic keys
- report accuracy, false positives, and mismatches

**Step 3: Run script to verify initial failure**

Run:

```bash
python3 scripts/qualify_mcp_guidance.py --goldens scripts/mcp_guidance_goldens.json
```

Expected: FAIL until harness is complete and classifier is wired.

**Step 4: Implement the harness**

Keep it narrow:

- no prose scoring
- compare only stable semantic outputs
- report:
  - matches
  - mismatches
  - false-positive guidance on clean success

**Step 5: Add a wrapper command**

In `scripts/axon`, add:

```bash
./scripts/axon qualify-guidance
```

**Step 6: Run to verify it passes**

Run:

```bash
python3 scripts/qualify_mcp_guidance.py --goldens scripts/mcp_guidance_goldens.json
```

Expected: PASS with explicit summary and no hidden skips.

**Step 7: Commit**

```bash
git add scripts/mcp_guidance_goldens.json scripts/qualify_mcp_guidance.py scripts/axon src/axon-core/src/mcp/tests.rs
git commit -m "test: add mcp guidance golden replay harness"
```

### Task 6: Promote Guidance From Shadow To Authoritative For Query

**Files:**
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing response-shape tests for query**

Add tests that assert:

- clean query success omits guidance
- not-found query returns compact guidance fields
- wrong scope query returns canonical next action
- partial/degraded query returns compact warning + next step

Example skeleton:

```rust
#[test]
fn axon_query_includes_compact_guidance_for_wrong_project_scope() {
    let response = call_query(/* fixture */);
    assert_eq!(response["problem_class"], "wrong_project_scope");
    assert!(response["next_best_actions"].to_string().contains("use_canonical_project_code"));
}
```

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test axon_query_includes_compact_guidance -- --test-threads=1
```

Expected: FAIL because query is still in shadow mode only.

**Step 3: Promote query guidance**

Make query authoritative:

- merge compact guidance fields into the real response
- keep guidance absent for clean success
- keep any heavy debug payload out of the public response

**Step 4: Run tests to verify it passes**

Run:

```bash
cargo test axon_query_includes_compact_guidance -- --test-threads=1
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: make query guidance authoritative"
```

### Task 7: Promote Guidance From Shadow To Authoritative For Inspect

**Files:**
- Modify: `src/axon-core/src/mcp/tools_dx.rs`
- Test: `src/axon-core/src/mcp/tests.rs`

**Step 1: Write failing response-shape tests for inspect**

Add tests that assert:

- clean inspect success omits guidance
- symbol miss returns `input_not_found`
- duplicate symbol case returns `input_ambiguous`
- canonical project mismatch returns `wrong_project_scope`
- missing rationale returns compact SOLL recommendation block only when materially relevant

**Step 2: Run test to verify it fails**

Run:

```bash
cargo test axon_inspect_includes_compact_guidance -- --test-threads=1
```

Expected: FAIL because inspect is still shadow-only.

**Step 3: Promote inspect guidance**

Make inspect authoritative with the same compact response rules as query.

**Step 4: Run test to verify it passes**

Run:

```bash
cargo test axon_inspect_includes_compact_guidance -- --test-threads=1
```

Expected: PASS

**Step 5: Commit**

```bash
git add src/axon-core/src/mcp/tools_dx.rs src/axon-core/src/mcp/tests.rs
git commit -m "feat: make inspect guidance authoritative"
```

### Task 8: Validate, Measure Noise, And Document The Result

**Files:**
- Modify: `docs/skills/axon-engineering-protocol/SKILL.md`
- Modify: `docs/plans/2026-04-17-mcp-guidance-declarative-reasoning-concept.md`
- Create: `docs/plans/2026-04-17-mcp-guidance-phase1-report.md`

**Step 1: Run the focused Rust tests**

Run:

```bash
cargo test classify_guidance_ -- --test-threads=1
cargo test axon_query_includes_compact_guidance -- --test-threads=1
cargo test axon_inspect_includes_compact_guidance -- --test-threads=1
```

Expected: PASS

**Step 2: Run the replay harness**

Run:

```bash
python3 scripts/qualify_mcp_guidance.py --goldens scripts/mcp_guidance_goldens.json
```

Expected: PASS with explicit mismatch count = 0 and low false-positive guidance on clean success.

**Step 3: Run the MCP quality gate**

Run:

```bash
bash scripts/mcp_quality_gate.sh
```

Expected: PASS and no regression caused by guidance changes.

**Step 4: Update the Axon skill minimally**

Only document MCP-facing reality:

- `query` and `inspect` may return compact guidance fields in degraded, ambiguous, invalid-scope, or materially incomplete cases
- SOLL guidance is recommendation-oriented unless explicit mutation authorization is present

Do not document internal scripts or harness plumbing in the skill.

**Step 5: Write the phase-1 report**

Include:

- final taxonomy used
- tools covered
- false-positive rate
- cases intentionally not covered yet
- explicit decision on whether Datalog is still justified for phase 2

**Step 6: Commit**

```bash
git add docs/skills/axon-engineering-protocol/SKILL.md docs/plans/2026-04-17-mcp-guidance-declarative-reasoning-concept.md docs/plans/2026-04-17-mcp-guidance-phase1-report.md
git commit -m "docs: report mcp guidance phase 1"
```

### Task 9: Decide Phase 2 Or Stop

**Files:**
- Modify: `docs/plans/2026-04-17-mcp-guidance-phase1-report.md`

**Step 1: Make the explicit go/no-go decision**

Only proceed to `retrieve_context`, `why`, and `impact` if:

- guidance is low-noise
- contract stayed compact
- no MCP regression was introduced
- the team still prefers Datalog over a lighter declarative substrate after phase-1 evidence

**Step 2: Record the decision**

Write one of:

- `GO_PHASE_2`
- `STOP_AT_PHASE_1`
- `REWORK_CLASSIFIER_BEFORE_EXPANSION`

**Step 3: Commit**

```bash
git add docs/plans/2026-04-17-mcp-guidance-phase1-report.md
git commit -m "docs: record mcp guidance phase decision"
```

