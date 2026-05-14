# Axon Inter-Project Relations: Intent, Benefits, And Implementation Proposal

## Purpose

This document clarifies:

- why Axon may need canonical inter-project relations
- what the existing project documents already say about cross-project analysis
- what should and should not be modeled as an inter-project relation
- a concrete implementation proposal compatible with the current canonical project identity rules

It is a design note, not yet an implementation plan.

## Context

Axon has now hardened a strict project identity contract:

- every real project must be registered canonically
- every project uses a canonical three-letter `project_code`
- there is no implicit fallback for project identity
- `PRO` remains reserved to the global SOLL guideline namespace, not to imported project files

This raises the natural follow-up question:

- if Axon wants cross-project analysis, how should inter-project relations be represented without breaking the project identity contract?

## What The Existing Documents Already Say

### 1. Central Omniscience Requires Cross-Project Analysis

[docs/architecture/2026-04-06-daemon-central-omniscience.md](/home/dstadel/projects/axon/docs/architecture/2026-04-06-daemon-central-omniscience.md) explicitly states that the sidecar architecture prevents:

- cross-project analysis
- system reflection on another project

The target architecture is a central daemon able to:

- process
- store
- analyze

the graphs of `N` registered projects simultaneously.

### 2. Workspace Federation Was Explicitly Planned

[docs/plans/2026-03-24-workspace-federation-hybrid.md](/home/dstadel/projects/axon/docs/plans/2026-03-24-workspace-federation-hybrid.md) explicitly proposes:

- extracting local path dependencies from Elixir, Python, and Rust projects
- building a unified cross-project dependency graph
- representing those links as `DEPENDS_ON` relations

### 3. MCP Was Expected To Eventually Use Those Relations

[docs/plans/2026-03-24-desilotisation.md](/home/dstadel/projects/axon/docs/plans/2026-03-24-desilotisation.md) proposes removing strict project silos in some MCP reasoning paths and leveraging `DEPENDS_ON` for:

- cross-project impact analysis
- cross-project grouping in reports

### 4. Delivery Discipline Already Warned Against Premature Generalization

[docs/plans/2026-04-01-axon-delivery-plan.md](/home/dstadel/projects/axon/docs/plans/2026-04-01-axon-delivery-plan.md) explicitly says:

- add cross-project reasoning only after single-project stability is strong

This is important. The design is intended, but the implementation should not outrun core correctness.

## Expected Benefits

If implemented correctly, inter-project relations can give Axon the following capabilities.

### 1. Explicit Local Dependency Awareness

Axon can know that:

- project `BKS` depends on project `FSC`
- project `AXO` depends on a local shared runtime library
- an umbrella or workspace has internal local dependency edges

This is stronger than plain file search because it captures project-level intent.

### 2. Cross-Project Impact Analysis

If a symbol or API changes in one project, Axon could estimate:

- which local dependent projects may be affected
- which MCP answers should mention a federated blast radius
- where a refactor crosses project boundaries rather than only module boundaries

### 3. Better Retrieval And Context Selection

A developer LLM could ask:

- “What other local projects depend on this service?”
- “Is this module mirrored or consumed elsewhere in the workspace?”
- “What is the minimal cross-project context needed before changing this API?”

This is especially useful in a central-daemon architecture.

### 4. Better Architectural Diagnostics

Inter-project relations can reveal:

- hidden local coupling
- accidental workspace entanglement
- direct code-level dependency where only a project-level interface should exist

### 5. Cleaner Separation Between Project Truth And Shared Global Rules

Today, `PRO` is correctly reserved for global SOLL guidelines.
Inter-project relations let Axon model project-to-project reality without abusing:

- `PRO`
- fake global project codes
- ambiguous shared pseudo-namespaces

## What Should Not Be Done

### 1. Do Not Give An Inter-Project Relation A Single Project Code

This is the core modeling rule.

An inter-project relation does not belong to only one project.
Therefore it should not be stored as:

- `project_code='AXO'`
- `project_code='PRO'`
- `project_code='PRJ'`
- or any synthetic single-code surrogate

That would destroy the semantics of the relation.

### 2. Do Not Reuse `PRO`

`PRO` remains reserved for:

- global SOLL guidelines
- shared governance rules

It must not become a catch-all code for cross-project structural data.

### 3. Do Not Infer Inter-Project Edges Implicitly From Weak Heuristics Alone

Inter-project relations should be created from explicit evidence such as:

- local path dependencies
- workspace manifests
- declared umbrella relations
- explicit shared component contracts

Not from vague name similarity.

## Canonical Modeling Proposal

### Target Principle

Projects keep their canonical three-letter `project_code`.
Inter-project relations are modeled as first-class relations between projects, not as ordinary graph edges overloaded with one `project_code`.

### Minimal Canonical Relation

For a first implementation, Axon should represent:

- `source_project_code`
- `target_project_code`
- `relation_type`
- `evidence_type`
- `evidence_ref`
- `confidence`
- `updated_at`

### Recommended First Relation Type

The first canonical inter-project relation should be:

- `DEPENDS_ON`

Because this is the one most explicitly described in the federation design docs and the least ambiguous to extract from manifests.

### Optional Future Relation Types

If later needed, Axon could add:

- `USES_SHARED_COMPONENT`
- `CALLS_ACROSS_PROJECT`
- `IMPLEMENTS_CONTRACT_FROM`
- `DERIVES_FROM`

But these should come only after `DEPENDS_ON` is stable.

## Proposed Storage Shape

### Option A: Dedicated Table In IST

Recommended initial runtime shape:

`ProjectRelation`

Suggested columns:

- `source_project_code VARCHAR NOT NULL`
- `target_project_code VARCHAR NOT NULL`
- `relation_type VARCHAR NOT NULL`
- `evidence_type VARCHAR`
- `evidence_ref VARCHAR`
- `confidence DOUBLE`
- `metadata JSON`
- `updated_at_ms BIGINT`

Primary key:

- `(source_project_code, target_project_code, relation_type, evidence_ref)`

This avoids overloading:

- `CALLS`
- `CONTAINS`
- `IMPACTS`

which currently behave like mono-project graph relations.

### Option B: Dedicated SOLL Mirror Later

If inter-project relations become strategic rather than purely operational, they could later be mirrored into SOLL as intentional/project governance entities.

That should not be wave one.

Wave one should stay in IST as operational truth.

## MCP Behavior Proposal

### Default

Normal project-scoped MCP calls remain isolated:

- `project_status(AXO)` stays project-local
- `impact(project=AXO)` stays project-local by default

### Explicit Federated Mode

Cross-project reasoning must be explicit.

Examples:

- `impact(project="AXO", include_dependent_projects=true)`
- `project_status(project_code="AXO", include_federation=true)`
- `query(scope="federated", project="AXO")`

This preserves the current isolation rule while allowing opt-in federation.

## Identity Contract For Inter-Project Relations

The strict canonical rules still apply:

- `source_project_code` must exist in `soll.ProjectCodeRegistry`
- `target_project_code` must exist in `soll.ProjectCodeRegistry`
- if either side is unknown, the relation must be rejected
- no implicit fallback to `PRO`, `GLOBAL`, `global`, `proj`, or any equivalent

## Incremental Implementation Proposal

### Wave 1: Stabilize Single-Project Correctness

Before inter-project delivery:

- finish strict project identity cleanup
- remove obsolete test fixtures and fallbacks
- keep MCP project-scoped behavior trustworthy

### Wave 2: Introduce `ProjectRelation`

Add a dedicated runtime table for:

- `DEPENDS_ON`

fed from explicit manifest extraction only.

### Wave 3: Expose Read-Only MCP Federation

Add federated read surfaces only after the relation is populated and reliable.

Examples:

- “dependent projects”
- “upstream local dependency”
- “cross-project impact summary”

### Wave 4: Cross-Project Impact

Only after waves 1 to 3 are stable should Axon let `impact` or related tools cross project boundaries automatically in explicit federated mode.

## Test Cleanup Implications

The current test suite contains many historical fixtures that:

- omit `project_code` in `CONTAINS` and `CALLS`
- use obsolete pseudo-codes
- simulate multi-project cases without a canonical inter-project model

Those tests should be cleaned in two categories:

### Category A: Mono-Project Fixtures

Normalize to:

- canonical 3-letter codes such as `PRJ`, `AXO`, `BKS`
- explicit `project_code` in all strict runtime relations

### Category B: Multi-Project Fixtures

Keep only if they test a real supported behavior.

If they are testing:

- shared file names
- cross-project retrieval
- future federated reasoning

they should be rewritten to use:

- explicit 3-letter project codes
- a future canonical `ProjectRelation` model when that feature exists

Obsolete tests that imply non-canonical behavior should be removed rather than preserved.

## Recommendation

The recommended Axon position is:

- yes to inter-project relations
- no to overloading them with a single `project_code`
- yes to a dedicated `ProjectRelation` runtime model
- yes to `DEPENDS_ON` as the first canonical relation type
- yes to explicit opt-in federated MCP behavior
- no to implementing this before single-project correctness is fully stabilized

## Next Step

If accepted, the next document should be an implementation plan for:

- `ProjectRelation`
- `DEPENDS_ON` extraction
- federated read-only MCP integration
- corresponding test migration rules
