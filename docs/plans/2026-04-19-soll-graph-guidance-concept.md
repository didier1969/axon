# SOLL Graph Guidance Concept

## Problem

Axon now creates and tracks SOLL entities reliably, but it still does not guide LLMs
well enough when those entities must be assembled into a canonical intentional graph.

The remaining user friction is concentrated in four places:

1. `soll_relation_schema` answers too negatively.
   - it can confirm a known pair
   - it can list some allowed target kinds
   - but it does not yet explain the canonical top-down graph pattern well enough
2. `soll_manager link` rejects invalid links correctly, but still leaves too much guesswork.
3. `soll_validate` diagnoses structural gaps without enough canonical repair guidance.
4. entity creation success is not yet clearly separated from graph completeness.

## Reality Check

The relation policy itself is already encoded server-side in `relation_policy_for_pair(...)`.
This is good:

- the server already owns canonical link truth
- we do not need a new ontology engine

The main weakness is presentation and repair guidance:

- `soll_relation_schema` returns pair-specific policy data
- but not enough graph conventions, examples, or “what should come next”
- `soll_validate` reports:
  - orphan requirements
  - validations without `VERIFIES`
  - decisions without `SOLVES/IMPACTS`
  - requirements without criteria/evidence
  but not enough structured recovery steps

## Goal

Make SOLL graph construction usable top-down, without trial-and-error.

That means:

- a source-type query must explain what kinds can legitimately come next
- invalid link rejection must suggest at least one canonical next move
- validation must emit machine-readable repair guidance
- the system must distinguish:
  - entities exist
  - graph is structurally incomplete

## Non-Goals

- no rewrite of the canonical relation policy
- no second relation system
- no hidden client-only heuristics as source of truth
- no large mutation helper unless the guidance-first approach still proves insufficient

## Design Direction

### 1. Make `soll_relation_schema` constructive

It should support:

- source-only guidance:
  - `source_type=VIS`
  - `source_type=PIL`
  - etc.
- pair guidance:
  - `source_type + target_type`
  - `source_id + target_id`
- incoming guidance:
  - target-only view where useful

And it should return:

- canonical graph role of the source kind
- allowed target kinds
- allowed relation names
- default relation when any
- concrete valid examples
- canonical graph conventions / “typical next edges”

### 2. Make `soll_manager link` error payloads prescriptive

For rejected links, return structured fields such as:

- rejected pair
- resolved source/target kinds
- allowed target kinds from source
- allowed relations for the attempted pair if any
- one or more valid alternative examples
- next best action

### 3. Make `soll_validate` emit repair guidance

Keep diagnostics, but add machine-readable repair suggestions for each class:

- orphan requirement
- validation without `VERIFIES`
- decision without `SOLVES/IMPACTS`
- requirement without criteria/evidence
- relation policy violation

Guidance must remain canonical and policy-backed, not heuristic-only.

### 4. Surface graph completeness explicitly

Expose a compact completeness summary in `soll_validate` for this wave.

The user must be able to tell:

- graph populated
- graph canonically connected
- graph evidentially complete

## Reuse vs Change

Reuse:

- `relation_policy_for_pair(...)`
- existing link endpoint classification
- existing validation categories

Change:

- `soll_relation_schema` output model
- `soll_manager link` rejection payload
- `soll_validate` payload structure
- no `project_status` duplication in this first wave

## Escalation Rule

Do not add a high-level mutation helper such as `soll_seed_foundation` in the first wave
unless strengthened schema/link/validate guidance still fails real workflow qualification.
