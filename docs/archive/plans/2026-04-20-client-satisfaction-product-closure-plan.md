# Client Satisfaction Product Closure Plan

Date: 2026-04-20
Status: executed
Scope: raise Axon MCP from strongly improved to client-close product satisfaction

## Goal

Close the remaining reasonable product gaps after the TE2 wave by working on:

1. public-surface standardization
2. async/public contract coherence
3. qualification from a generic MCP client perspective

This plan is not a rewrite.
It extends the already-delivered operator wave into broader product consistency.

## Principles

- no TE2-only hacks
- prefer reusable machine-readable contracts
- keep one canonical async follow-up
- validate from the client side, not only from Rust unit tests

## Workstreams

### W1: Public Surface Standardization

Objective:

- raise weaker public tools toward the standard already reached by the best surfaces

Target qualities:

- machine-readable fields first
- corrective guidance
- stable aliases when deep nesting is otherwise annoying
- explicit operator-facing remediation where risk or incompleteness exists

Current tranche:

- `change_safety` enriched with `operator_guidance`
- executed and validated in Rust tests plus client-real MCP qualification

### W2: Async/Public Contract Coherence

Objective:

- make async mutation acceptance and follow-up equally self-guiding

Target qualities:

- stable async acceptance envelope
- stable `job_status`
- terminal `next_action`
- terminal `result_data` alias
- no fragile client-side JSON path guessing

Current tranche:

- `job_status` now returns:
  - `known_ids`
  - `next_action`
  - `result_contract`
  - `polling_guidance`
  - `recovery_hint`
  - `result_data`
- executed and validated on real async follow-up from the MCP client side

### W3: Client-Real Qualification

Objective:

- make the qualification runner assert the richer contracts that the server now exposes

Target qualities:

- validate public tool contract quality from the client side
- detect regression when richer fields disappear
- ensure async flows remain machine-usable from an MCP client

Current tranche:

- `scripts/mcp_validate.py` now checks:
  - `change_safety.operator_guidance`
  - `soll_query_context.operational_digest`
  - `soll_verify_requirements.summary`
  - `soll_verify_requirements.requirements`
  - `soll_verify_requirements.completion_model`
  - `job_status.known_ids`
  - `job_status.result_contract`
  - `job_status.polling_guidance`
  - `job_status.recovery_hint`
  - `job_status.result_data`
- executed against `dev` MCP after rebuilding and resynchronizing the served runtime binary
- passing proof:
  - `/tmp/mcp-validate-core-client-closure.json`
  - `20/20 ok`, `transport_health=pass`, `semantic_quality=pass`

## Completion Signal

This plan is complete when:

- weak public surfaces have fewer opaque responses
- async flows are self-guiding end to end
- qualification catches contract regressions from the client side
- the next remaining gaps are mostly polish and broader scenario coverage, not structural ambiguity

## Outcome

Closed in this wave:

- public surface standardization advanced on a real weak surface
- async/public follow-up is now materially easier for generic clients
- client-real qualification now protects these richer contracts from regression

What remains after this plan is not structural closure work.
It is the next broader product wave: more scenario coverage, more public-surface convergence, and more final polish.
