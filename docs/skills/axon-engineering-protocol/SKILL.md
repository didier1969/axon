---
name: axon-engineering-protocol
description: Use in the Axon repository before coding, structural diagnostics, or SOLL mutation. Defines the canonical operator flow, identity rules, and MCP tool routing.
---

# Axon Engineering Protocol

## Core Rule
- Read IST/SOLL first, mutate second, certify before commit.
- Treat MCP output as authoritative. Do not invent IDs, project codes, or intent links.

## Runtime Rule
- Axon now has two valid runtime authorities:
  - `live`: stable truth runtime
  - `dev`: isolated development runtime
- Use explicit entrypoints:
  - `./scripts/axon-live ...`
  - `./scripts/axon-dev ...`
  - or `./scripts/axon --instance live|dev ...`
- `live` and `dev` share the same MCP surface, but not the same ports, sockets, pidfiles, or databases.
- `status` is always the first truth surface.
- Distinguish:
  - MCP `status` = protocol truth
  - `scripts/status*.sh` = local lifecycle probe
- For MCP surface qualification, prefer `./scripts/axon qualify-mcp`; treat older entrypoints such as `quality-mcp`, `validate-mcp`, `measure-mcp`, `compare-mcp`, `robustness-mcp`, and `qualify-guidance` as expert or compatibility flows.
- Do not mutate `live` implicitly from development workflows.

## Identity Contract
- Canonical IDs are server-owned: `TYPE-CODE-NNN`.
- `CODE` comes from `.axon/meta.json`, mirrored into `soll.ProjectCodeRegistry`.
- `axon_init_project` assigns `project_code` server-side and returns it; the LLM does not invent project codes.
- When a public mutation is async, the acceptance payload is authoritative:
  - `data.known_ids`
  - `data.next_action`
  - `data.result_contract`
  - canonical follow-up via `job_status`
- LLMs use returned IDs; they do not fabricate them.
- For batch plans, use `logical_key`; the server resolves canonical IDs.

## Surface Model
- `core/public`: default discovery layer for routine LLM developer work.
- `expert/internal`: advanced diagnostics, recovery, and low-level introspection.

## Core/Public Tools
- `status`: runtime truth, availability, degradation, public surface.
- `project_status`: compact live situation for one project.
- `project_registry_lookup`: resolve canonical project identity from code, name, or path.
- `query`: discovery / broad recall.
- `inspect`: precise zoom on a known target.
- `retrieve_context`: answerable evidence packet for LLM work.
- `why`: rationale view.
- `path`: topology / flow view.
- `impact`: blast radius for change.
- `anomalies`: structural findings.
- `change_safety`: practical mutation safety.
- `conception_view`: derived module/interface/contract/flow map.
- `snapshot_history`, `snapshot_diff`: derived structural memory.
- advanced graph/system exploration: `refine_lattice`, `cypher`, `debug`, `schema_overview`, `list_labels_tables`, `query_examples`
- advanced runtime/analysis tools: `health`, `audit`, `batch`, `truth_check`, `diagnose_indexing`, `diff`, `semantic_clones`, `architectural_drift`, `bidi_trace`, `api_break_check`, `simulate_mutation`, `resume_vectorization`, `job_status`
- `axon_pre_flight_check`, `axon_commit_work`: validated delivery workflow.
- SOLL workflow: `soll_query_context`, `soll_work_plan`, `soll_validate`, `soll_export`, `soll_verify_requirements`, `soll_manager`, `soll_apply_plan`, `soll_commit_revision`, `soll_rollback_revision`, `axon_init_project`, `axon_apply_guidelines`.

## Expert/Internal Tools
- no additional MCP tools should be treated as hidden product surface by default
- true internals remain transport or implementation primitives outside the normal MCP tool contract

## First-Choice Routing
1. `status`
2. `project_status` if you need the current project situation
3. `project_registry_lookup` if project identity is uncertain
4. `query` / `inspect` / `retrieve_context`
   - `query` = discover
   - `inspect` = zoom
   - `retrieve_context` = compact answerable context
5. `impact` before risky refactor/change
6. `why` when rationale matters
7. `path` when flow/topology matters
8. `anomalies` for cleanup, refactor, debt, or structural review
9. `change_safety` before risky mutation
10. `conception_view` if a derived architecture map is needed
11. `job_status` as the canonical follow-up for async public mutations
12. `axon_pre_flight_check`
13. `axon_commit_work`

## SOLL Model
- `Vision`: target outcome
- `Pillar`: strategic principle
- `Requirement`: testable capability
- `Decision`: technical choice
- `Concept`: domain vocabulary
- `Guideline`: durable engineering rule
- `Milestone`: delivery checkpoint
- `Validation`: proof
- `Stakeholder`: impacted actor

## Mutation Rules
- `soll_manager` for immediate unit mutations.
- `soll_apply_plan` for transactional batch mutations.
- `soll_commit_revision` to commit a preview.
- `soll_rollback_revision` to revert a revision.
- Re-run is expected to be idempotent.

## Mandatory Delivery Flow
1. Run `status`.
2. Read code truth with `query`, `inspect`, or `retrieve_context`.
3. Use `impact`, `why`, `path`, `anomalies`, `change_safety` only as needed.
4. Update SOLL in the same wave when intention changes.
5. Run `axon_pre_flight_check`.
6. Use `axon_commit_work`.

## Context Efficiency Rules
- Prefer `project_status` over composing many tools for a first pass.
- Prefer `retrieve_context` when an LLM needs a compact packet, not raw recall.
- Prefer `mode=brief` by default; only expand when the first answer is insufficient.
- Keep expert tools out of first-choice routing unless the task is truly diagnostic.

## Maintenance Rule
Update this skill immediately when:
- tool names or visibility change
- public/expert routing changes
- identity rules change
- SOLL workflow or schema changes
