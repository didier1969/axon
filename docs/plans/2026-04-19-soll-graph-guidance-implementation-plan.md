# SOLL Graph Guidance Implementation Plan

## Objective

Close the remaining SOLL usability gap by making canonical graph construction
discoverable and repairable without trial-and-error.

## Phase 1. Strengthen `soll_relation_schema`

1. Add source-kind conventions.
   - for each supported source kind, expose:
     - graph role
     - canonical next link families
     - allowed target kinds
     - example edges

2. Support source-only queries explicitly.
   - `source_type` alone should return useful guidance, not just raw pair tables

3. Preserve pair-level truth.
   - `source + target` still returns exact allowed/default relation semantics

4. Add tests.
   - source-only `VIS`
   - source-only `PIL`
   - pair `DEC -> REQ`
   - unresolved ids still return constructive guidance

## Phase 2. Improve `soll_manager link` rejection guidance

1. Extend structured error payload.
   - include:
     - rejected pair
     - source/target kinds
     - allowed target kinds from source
     - canonical examples
     - next best actions

2. Ensure the data is derived from the same relation policy source of truth.

3. Add tests for:
   - invalid `VIS -> PIL` style attempt
   - invalid pair with actionable alternative

## Phase 3. Add repair guidance to `soll_validate`

1. Keep current diagnostics.
2. Add machine-readable `repair_guidance`.
   - orphan requirement:
     - valid parent/source types
   - validation without `VERIFIES`:
     - expected compatible targets
   - decision without `SOLVES/IMPACTS`:
     - expected compatible target classes
   - requirement without criteria/evidence:
     - required metadata / proof expectations
3. Add a compact completeness summary:
   - populated
   - structurally_connected
   - evidence_ready

4. Add tests covering:
   - orphan requirement
   - missing verifies
   - decision without links
   - requirement without criteria/evidence

## Phase 4. Qualification

1. Rust tests for the new payload contracts.
2. Runtime MCP validation on `dev`.
3. A realistic top-down NTO/AXO conceptual graph scenario:
   - create entities
   - ask for schema
   - link canonically
   - validate
   - repair remaining issues

## Constraints

- No ontology duplication
- No client-specific truth fork
- No mutation helper in phase 1 unless guidance-first fails

## Success Criteria

The following must become true:

1. A client can ask `soll_relation_schema` from a source kind alone and get constructive graph guidance.
2. A rejected link provides at least one clear canonical next move.
3. `soll_validate` produces repair guidance, not only findings.
4. A top-down SOLL graph can be assembled with minimal guesswork.
