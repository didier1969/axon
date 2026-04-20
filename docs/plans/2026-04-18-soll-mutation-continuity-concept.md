# Axon SOLL Mutation Continuity: Canonical Identity And Relation Guidance

## Context

Recent client feedback confirms that the global MCP contract is much better:

- canonical project identity is now returned immediately
- `project_registry_lookup` works
- async continuation is now usable

The main remaining server-side problem is narrower:

- SOLL mutation continuity in real project work is still not fully reliable

The concrete real-world failures are:

1. `soll_apply_plan` can still reject a canonical `project_code` even after:
   - `axon_init_project` returned it
   - `project_registry_lookup` confirmed it
2. when `soll_manager link` rejects a relation, the agent still lacks enough guidance to know the canonical next valid move without trial and error

## Reality Check

The code already contains most of the needed primitives.

### What already exists

- canonical project registry validation
- canonical project lookup
- relation policy selection
- explicit allowed/default relation sets per source/target type
- canonical relation enforcement in `soll_manager link`

So the main issue is not absence of rules.
It is inconsistency and insufficient exposure of those rules at the mutation boundary.

## Product Decision

### 1. Project identity must be accepted consistently across all SOLL mutation paths

If Axon says a project exists and its canonical code is `NTO`, then every SOLL mutation path must accept `NTO` as the same project identity:

- `soll_apply_plan`
- `soll_manager`
- `soll_commit_revision`
- related mutation helpers

Identity resolution and identity acceptance must behave as one coherent contract.

### 2. Relation rejection must become actionable

Rejecting an invalid SOLL relation is correct.
But the server must make the canonical next move obvious enough for an agent.

At minimum, a rejected relation should expose:

- source type
- target type
- whether the pair is disallowed or only the requested relation is disallowed
- allowed relation types for that pair when the pair is legal
- default relation when one exists
- suggested next valid action

Product gate:

- an agent must be able to choose the next mutation call from returned structured fields alone
- no source-code reading should be required to recover from a rejected link

### 3. Relation discoverability should be first-class

The agent should not need to discover relation policy only by failing.

The server should expose relation policy directly through one of:

- a dedicated `soll_relation_schema` tool
- or a compact discoverability surface equivalent in result quality

Shipping gate:

- this wave may ship without `soll_relation_schema` only if enriched link errors are sufficient to complete one-step correction in realistic agent flows
- otherwise `soll_relation_schema` becomes mandatory in this wave

This surface should support:

- relation policy by source/target type
- relation policy by concrete IDs
- relation policy lookup for “what can I link from this node”

### 4. High-level batch mutation must be the reliable path

`soll_apply_plan` is meant to be the efficient, canonical batch path.

If an agent routinely has to fall back to manual `soll_manager` calls to continue working, then the product contract is not yet complete.

The target is:

- `soll_apply_plan` works once canonical project identity is known
- fallback to entity-by-entity mutation becomes exceptional

## Constraints

- Do not redesign SOLL itself.
- Do not loosen canonical identity rules.
- Do not allow invalid relation structures just to make the flow easier.
- Prefer additive machine-readable guidance over prose-only hints.
- Keep the existing relation policy as the source of truth unless evidence forces policy changes.

## Non-goals

- no broad SOLL ontology redesign
- no speculative new foundation workflow in this phase
- no client-side binding redesign in this phase

## Likely Minimal Server Changes

### A. Harden `soll_apply_plan` identity continuity

Investigate and correct any path where:

- registry state
- canonical project resolution
- preview reservation
- batch payload project scope

can disagree for a project that was just initialized and confirmed.

Identity discipline rule:

- `soll_apply_plan` must accept the canonical code consistently
- it must not silently broaden or remap project scope beyond that canonical project

### B. Add relation guidance to `soll_manager link`

When `link` fails, return structured guidance that reflects the already-known canonical relation policy.

### C. Add explicit relation discovery surface

Add a tool such as `soll_relation_schema` if error-message enrichment alone is not enough.

## Expected Outcome

After this wave:

- a canonical project code returned by Axon is accepted consistently by SOLL mutation tools
- `soll_apply_plan` becomes reliable for normal concept seeding flows
- invalid link attempts tell the agent exactly what the canonical next step is
- manual trial-and-error during graph construction drops sharply
