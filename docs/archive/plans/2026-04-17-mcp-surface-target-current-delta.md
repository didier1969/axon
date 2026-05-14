# Axon MCP Surface: Target, Current State, And Delta

## Purpose

This document defines:

- the target MCP surface Axon should expose to LLM developer agents
- the current MCP surface as implemented today
- the concrete delta required to reach the target

It is intentionally not an implementation plan. It is a gap document used to prepare the next planning phase.

## Context

Axon is not trying to expose "many tools" for their own sake.
Its goal is to provide an LLM with:

- fast project situation awareness
- precise code and structure lookup
- rationale and topology access
- safe change guidance
- access to intentional truth in SOLL
- expert/system diagnostics when needed

The main design risk is not lack of tools.
The main design risk is tool overlap, weak discoverability, and ambiguous first-choice routing for LLMs.

## Target State

### Target Principle

Axon should expose a layered MCP surface:

- a primary public surface for routine LLM developer work
- an expert/internal surface for deep diagnostics, low-level introspection, and recovery operations

The public surface must be:

- discoverable
- semantically stable
- non-overlapping at first choice
- sufficient for most development assistance tasks

The expert surface must remain available, but should not crowd the first layer of tool discovery.

### Target Public Surface

The public surface should contain the tools needed for the full normal workflow of a developer LLM:

- `status`
- `project_status`
- `query`
- `inspect`
- `retrieve_context`
- `why`
- `path`
- `impact`
- `anomalies`
- `change_safety`
- `conception_view`
- `snapshot_history`
- `snapshot_diff`
- `fs_read`
- `axon_pre_flight_check`
- `axon_commit_work`
- SOLL workflow tools:
  - `soll_query_context`
  - `soll_work_plan`
  - `soll_validate`
  - `soll_export`
  - `soll_verify_requirements`
  - `soll_manager`
  - `soll_apply_plan`
  - `soll_commit_revision`
  - `soll_rollback_revision`
  - `axon_init_project`
  - `axon_apply_guidelines`

### Target Expert/Internal Surface

These tools are useful, but should not be part of the main first-choice public catalog:

- `architectural_drift`
- `diagnose_indexing`
- `truth_check`
- `resume_vectorization`
- `cypher`
- `debug`
- `schema_overview`
- `list_labels_tables`
- `query_examples`
- `diff`
- `semantic_clones`
- `bidi_trace`
- `api_break_check`
- `simulate_mutation`
- `job_status`

### Target Tool Semantics

The public surface should obey these role boundaries:

- `status`
  - runtime truth only
- `project_status`
  - project situation only
- `query`
  - discovery and recall
- `inspect`
  - precise zoom on a known target
- `retrieve_context`
  - answerable context packet for an LLM
- `why`
  - rationale
- `path`
  - topology and flow
- `impact`
  - blast radius
- `anomalies`
  - structural findings
- `change_safety`
  - practical mutation safety

### Target Parameter Discipline

Axon should not add a universal second-axis `level=generalist|specialist` across the whole API.

Rationale:

- many tools already expose `mode=brief|verbose`
- adding another generic depth parameter would often duplicate existing semantics
- it would increase interface complexity without increasing capability

If a second-level parameter is introduced later, it must be limited to cases where it changes operator posture without duplicating `brief|verbose`.

## Current State

### Current Strengths

The current MCP surface is already rich and, in many places, aligned with Axon's intended developer-assistance role.

The following strengths are already present:

- strong public operator tools such as `status`, `project_status`, `why`, `path`, `anomalies`
- a meaningful retrieval trio in implementation:
  - `query`
  - `inspect`
  - `retrieve_context`
- explicit advanced/system tooling for low-level diagnostics
- runtime-profile based tool gating
- a strong SOLL workflow surface

### Current Weaknesses

The main issues are structural, not conceptual.

#### 1. Public catalog and intended usage are misaligned

The strongest example is `retrieve_context`.

- It is described in the Axon protocol as a first-rank context tool.
- It is implemented as an advanced retrieval surface.
- It is still hidden from the public catalog.

This is a major discoverability mismatch.

#### 2. Some public-facing tools overlap too much

The main overlapping tools are:

- `health`
- `audit`
- `project_status`
- `anomalies`
- `change_safety`

These tools are not all useless.
But in the current shape, they create too many adjacent choices for an LLM at the top layer.

#### 3. Too much flattening between routine tools and expert tools

Some expert tools are useful but should not be part of the first layer of reasoning for most LLM developer flows:

- `architectural_drift`
- `cypher`
- `truth_check`
- `resume_vectorization`
- `diagnose_indexing`

#### 4. Skill and catalog are not fully synchronized

The skill currently expresses the intended operator flow better than the catalog expresses the practical discoverability surface.

Examples:

- `retrieve_context` is core in the skill, hidden in the catalog
- some tool naming and exposure distinctions are clearer in the skill than in the implementation surface

## Delta To Reach Target

### Delta 1: Make the public catalog match the real public workflow

Required change:

- expose `retrieve_context` in the public catalog

Expected result:

- LLMs can naturally discover the tool that assembles an answerable evidence packet
- the public workflow becomes more coherent:
  - discover
  - inspect
  - assemble context

### Delta 2: Reposition overlapping tools out of the primary public surface

Required change:

- move these out of the first-rank public catalog:
  - `health`
  - `audit`
  - `batch`

Important:

- this does not mean deleting them immediately
- this means relayering them away from the primary discovery surface

Expected result:

- fewer ambiguous first-tool choices
- cleaner routing for routine LLM work

### Delta 3: Keep expert diagnostics available but clearly secondary

Required change:

- explicitly classify expert/internal tools as secondary surfaces rather than deleting them

Expected result:

- expert power is preserved
- routine discoverability improves

### Delta 4: Do not introduce a generalized `generalist|specialist` parameter now

Required change:

- reject a broad second-level parameterization across the command surface for now

Reason:

- it would overlap with `mode=brief|verbose`
- it would complicate the interface before solving the primary discoverability issue

Expected result:

- cleaner API evolution
- fewer redundant interface dimensions

### Delta 5: Realign the Axon skill with the catalog that LLMs actually see

Required change:

- update `docs/skills/axon-engineering-protocol/SKILL.md` so it reflects:
  - the core/public surface
  - the expert/internal surface
  - the canonical tool names actually exposed by the server
  - the real routing between `query`, `inspect`, `retrieve_context`, `why`, `path`, and `impact`

Expected result:

- the written operator protocol matches the real MCP surface
- LLMs stop learning a route that differs from the catalog they discover at runtime
- future qualification can check one contract instead of two drifting descriptions

### Delta 5: Realign the skill with the chosen public/expert layering

Required change:

- update the Axon skill after the catalog decision is implemented

Expected result:

- skill, implementation, and discoverability become consistent

## Non-Goals

This document does not propose:

- deleting large parts of the MCP surface immediately
- reducing Axon's expert power
- redesigning every tool contract at once
- implementing a universal two-level command taxonomy now

## Final Position

Axon already has enough tools.

The next improvement is not tool multiplication.
It is surface clarification:

- expose the right tools publicly
- keep expert tools available but secondary
- remove ambiguity from the primary discovery layer
- avoid adding redundant control parameters before the public surface is stable

The single most important concrete target is:

- `retrieve_context` must become publicly discoverable

The single most important caution is:

- do not generalize `generalist|specialist` while `brief|verbose` already exists on many tools
