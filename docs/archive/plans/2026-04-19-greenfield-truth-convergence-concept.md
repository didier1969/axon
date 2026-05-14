# Greenfield Truth Convergence Concept

## Problem

Axon is now useful enough for real greenfield project shaping, but three product weaknesses still reduce trust:

1. read surfaces can disagree on the same project state
2. evidence semantics are not fully uniform across surfaces
3. higher-level authoring workflows are still thinner than the product now needs

The most urgent issue is not missing mutation power.
It is contradictory read truth.

Example from client feedback:
- `soll_validate`: concept baseline healthy
- `soll_verify_requirements`: `done=12 partial=0 missing=0`
- `anomalies`: still reports `orphan intent`

That forces the client to arbitrate server truth manually.

## Current Root Causes

### 1. Different tools encode different notions of completeness

`soll_validate`
- validates structural SOLL invariants
- focuses on orphan requirements, missing `VERIFIES`, missing decision links, relation-policy violations, duplicate titles, uncovered requirements

`soll_verify_requirements`
- computes requirement state from:
  - requirement status
  - acceptance criteria
  - count of `soll.Traceability`
- does not reason over the same structural graph invariants as `soll_validate`

`anomalies`
- mixes code smells and intent smells
- currently treats `orphan_intent` through a broad traceability/evidence lens
- does not appear aligned with the same concept-baseline semantics as SOLL validation

`soll_work_plan`
- builds its own work-plan state from node loading and requirement evidence counts
- is adjacent to, but not guaranteed identical with, the other read surfaces

### 2. Evidence semantics are fragmented

Observed and code-level signs:
- requirement verification counts traceability rows keyed by string entity type
- evidence attachment accepts arbitrary `entity_type` input
- some surfaces appear sensitive to normalization/casing conventions
- not every read surface appears to use the same evidence visibility rules

### 3. Product framing is missing a clean distinction

For greenfield work, Axon should distinguish:
- concept completeness
- implementation completeness

Today, some surfaces blur them.

## Desired Outcome

Axon should expose one coherent truth model for greenfield projects:

1. concept baseline truth
- Is the project structurally shaped enough to start implementation?

2. implementation evidence truth
- What proof or execution evidence still remains to be attached?

3. anomaly truth
- If a surface warns, it must either:
  - agree with the canonical model
  - or explain precisely why it differs

Canonical contract:
- `concept_completeness` = structural intentional graph baseline
- `implementation_completeness` = evidence/proof readiness
- `heuristic_anomalies` = non-canonical overlay that must never silently override the first two

## Reuse vs Change

Reuse:
- existing SOLL graph model
- existing `soll_validate`
- existing `soll_verify_requirements`
- existing `soll_work_plan`
- existing evidence attachment surface

Change:
- unify evidence normalization and counting
- define one canonical completeness model
- realign `anomalies` to that model, or explicitly downgrade it when the signal is only heuristic
- add higher-level greenfield authoring workflows only after truth convergence
- one server-side canonical completeness evaluator must be named explicitly; other read surfaces may project it differently but must derive from it or explain divergence

## Non-Goals

- not a full SOLL schema redesign
- not a rewrite of all MCP read surfaces
- not a broad anomaly-engine rewrite outside greenfield/SOLL truth
- not immediate implementation of every future authoring workflow in the same wave

## Product Decision

This work should happen in two ordered layers:

### Layer 1. Truth convergence
- unify read-surface semantics first
- evidence normalization
- concept vs implementation completeness distinction
- machine-visible payload semantics for these axes across the affected tools

### Layer 2. Greenfield authoring workflows
- add higher-level authoring only once the truth model is reliable

Scope discipline:
- this wave realigns only SOLL/greenfield-facing anomaly semantics
- it does not attempt to rewrite the full anomaly engine

## Acceptance Criteria

1. `soll_validate`, `soll_verify_requirements`, `anomalies`, and `soll_work_plan` no longer contradict each other silently on the same project baseline.
2. If they differ, the difference is explained by an explicit documented semantic boundary.
3. Differences are visible in the tool payloads, not only in prose or docs.
4. Evidence attachment is normalized consistently across surfaces.
5. The system distinguishes concept completeness from implementation completeness.
6. At least one higher-level greenfield workflow is planned on top of the converged truth model, not before it.
