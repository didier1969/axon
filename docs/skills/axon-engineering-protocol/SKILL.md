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
- Distinguish endpoint classes:
  - `instance_identity.*_url` = runtime-local host truth
  - `advertised_endpoints.*` = client-facing endpoint truth for isolated clients/subagents
- Isolated clients must prefer `advertised_endpoints` when available; do not treat loopback runtime URLs as externally reachable by default.
- For MCP surface qualification, prefer `./scripts/axon qualify-mcp`; treat older entrypoints such as `quality-mcp`, `validate-mcp`, `measure-mcp`, `compare-mcp`, `robustness-mcp`, and `qualify-guidance` as expert or compatibility flows.
- Do not mutate `live` implicitly from development workflows.
- For release work on `live`, use the topological checklist:
  1. `./scripts/axon release-preflight`
  2. `./scripts/axon create-release-manifest --state qualified`
  3. `./scripts/axon promote-live --manifest <manifest> --restart-live`
  4. verify MCP `status`
  5. only then treat the release as `promoted`
- `release-preflight` must prove both metadata and artifact-body integrity:
  - `bin/axon-core.build-info` matches `git describe`
  - `bin/axon-core` checksum matches recorded artifact checksum
  - workspace `bin/axon-core` matches the canonical workspace release target `.axon/cargo-target/release/axon-core`
- During release or rollback:
  - MCP `status` is final runtime truth
  - `scripts/status*.sh` remains advisory only
  - `pending` and `current` must never drift silently

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
- one public MCP product surface
- two execution flows only:
  - `sync` by default
  - `async` only for allowlisted heavy operations
- the current async allowlist is published by `status` and `mcp_surface_diagnostics`; treat server truth as canonical
- classify a tool as `async` only if it is semantically heavy or repeatedly fails the `p95 < 200 ms` interaction budget

## Core/Public Tools
- `status`: runtime truth, availability, degradation, public surface.
- `mcp_surface_diagnostics`: compact diagnostics for server truth vs possible stale client binding.
- `mcp_surface_diagnostics` now exposes explicit client freshness semantics:
  - `session_freshness_status`
  - `canonical_refresh_instruction`
  - `safe_to_rely_on_now`
  - `may_require_client_refresh`
- `./scripts/axon sync-codex-mcp-config`: operator path to print or explicitly refresh Codex MCP config from advertised endpoints.
- `project_status`: compact live situation for one project.
- `project_registry_lookup`: resolve canonical project identity from code, name, or path.
- `query`: discovery / broad recall.
- `inspect`: precise zoom on a known target.
- `retrieve_context`: answerable evidence packet for LLM work.
- prefer `retrieve_context(..., mode="intent")` for project steering, SOLL mutation suggestions, concept docs, and implementation plan recovery.
- `why`: rationale view.
- `path`: topology / flow view.
- `impact`: blast radius for change.
- `anomalies`: structural findings.
- `anomalies`: structural findings; for SOLL/greenfield intent, treat it as heuristic unless it explicitly aligns with canonical completeness.
- `change_safety`: practical mutation safety.
- `conception_view`: derived module/interface/contract/flow map.
- `snapshot_history`, `snapshot_diff`: derived structural memory.
- advanced graph/system exploration: `refine_lattice`, `cypher`, `debug`, `schema_overview`, `list_labels_tables`, `query_examples`
- advanced runtime/analysis tools: `health`, `audit`, `batch`, `truth_check`, `diagnose_indexing`, `diff`, `semantic_clones`, `architectural_drift`, `bidi_trace`, `api_break_check`, `simulate_mutation`, `resume_vectorization`, `job_status`
- `axon_pre_flight_check`, `axon_commit_work`: validated delivery workflow.
- SOLL workflow: `soll_query_context`, `soll_work_plan`, `soll_validate`, `soll_export`, `soll_generate_docs`, `soll_verify_requirements`, `soll_relation_schema`, `soll_manager`, `infer_soll_mutation`, `entrench_nuance`, `soll_apply_plan`, `soll_commit_revision`, `soll_rollback_revision`, `axon_init_project`, `axon_apply_guidelines`.

## Expert/Internal Tools
- no additional MCP tools should be treated as hidden product surface by default
- true internals remain transport or implementation primitives outside the normal MCP tool contract

## First-Choice Routing
1. `status`
2. `mcp_surface_diagnostics` if the client-visible tool binding seems inconsistent with the public surface advertised by the server
   - if isolated clients cannot reach Axon while the runtime is healthy, compare `instance_identity` vs `advertised_endpoints`
3. `project_status` if you need the current project situation
4. `project_registry_lookup` if project identity is uncertain
5. `axon_init_project` when you want project initialization with canonical identity returned immediately
6. `query` / `inspect` / `retrieve_context`
   - `query` = discover
   - `inspect` = zoom
   - `retrieve_context` = compact answerable context
7. `impact` before risky refactor/change
8. `why` when rationale matters
9. `path` when flow/topology matters
10. `anomalies` for cleanup, refactor, debt, or structural review
11. `change_safety` before risky mutation
12. `conception_view` if a derived architecture map is needed
13. `soll_relation_schema` when SOLL link policy, valid target kinds, or canonical incoming/outgoing graph edges are unclear
14. `job_status` as the canonical follow-up for the async allowlist only, using the returned `polling_guidance`
15. `axon_pre_flight_check`
16. `axon_commit_work`

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
- `soll_manager action=create` may optionally use `attach_to` and `relation_hint` for canonical graph attachment in the same operation.
- `soll_relation_schema` before retrying an invalid SOLL link or when canonical outgoing or incoming graph edges are unclear.
- `infer_soll_mutation` for read-only assistive capture before a higher-level SOLL mutation; it may suggest scope, entity type, and target IDs, but it does not reserve IDs or mutate the graph.
- `entrench_nuance` is a bounded high-level workflow for wave 1:
  - it only updates existing canonical entities
  - it proposes first and requires `confirm=true` to write
  - if nuance truly requires new nodes or topology changes, fall back to `soll_manager` or `soll_apply_plan`
- `soll_generate_docs` for human-readable navigable docs derived from live SOLL.
- treat `soll_generate_docs` output as derived reading surface only; live SOLL and `soll_export` remain canonical.
- default derived output root is separate from canonical exports: `docs/derived/soll/<project_code>`.
- canonical auto-sync also maintains a global derived root at `docs/derived/soll/index.html`, listing all known projects.
- the global derived reading root is `GLO`; it is a portfolio navigation concept, not a canonical SOLL entity.
- if the derived site does not exist yet, generate a full project site; otherwise refresh incrementally and delete obsolete derived pages from the manifest.
- successful SOLL mutations should return machine-readable `data.derived_docs_refresh` so clients can see whether derived docs stayed fresh or became stale.
- the derived site should now be read as a three-pane shell:
  - left = collapsible tree navigation
  - center = hierarchy focus graph
  - right = structured details
- human navigation must not depend only on clickable Mermaid nodes; tree links and surrounding HTML links are the canonical path.
- side panes may be resized or collapsed completely; the center pane must expand accordingly.
- `soll_validate` now returns structured `repair_guidance` and `completeness`; use it to repair graph structure, not only to detect warnings.
- `soll_attach_evidence` should be read as an operational proof tool, not a blind append:
  - it accepts `artifact_ref`, `path`, `file_path`, or `uri`
  - file artifacts are normalized against the canonical project root when possible
  - it returns per-artifact diagnostics, accepted artifact schema, and fallback guidance on rejection
- `soll_verify_requirements` now returns richer requirement-level proof diagnostics:
  - `missing_dimensions`
  - `suggested_next_actions`
  - `validation_count`
  - `broken_file_evidence_count`
- successful bounded SOLL mutations should return machine-readable `mutation_feedback`:
  - `changed_entities`
  - `topology_delta`
  - `newly_unblocked`
  - `remaining_blockers`
  - `next_best_actions`
  - `completeness_before`
  - `completeness_after`
- prefer this feedback to improvise follow-up steps after `soll_manager` or `entrench_nuance`.
- canonical completeness model for greenfield work:
  - `concept_completeness` = structural intentional baseline
  - `implementation_completeness` = evidence/proof readiness
  - `heuristic anomalies` must not silently override the first two
- `soll_apply_plan` for transactional batch mutations.
- `soll_commit_revision` to commit a preview synchronously unless future qualification forces review.
- `soll_rollback_revision` to revert a revision.
- Re-run is expected to be idempotent.

## Mandatory Delivery Flow
1. Run `status`.
2. Read code truth with `query`, `inspect`, or `retrieve_context`.
3. Use `impact`, `why`, `path`, `anomalies`, `change_safety` only as needed.
4. Update SOLL in the same wave when intention changes.
5. Keep derived docs aligned:
   - automatic refresh should happen after successful SOLL mutations
   - `soll_generate_docs` remains the explicit operator tool for manual rebuilds or repairs
6. Run `axon_pre_flight_check`.
7. Use `axon_commit_work`.

## Commit Hygiene
- `axon_commit_work` no longer auto-stages the whole `docs/vision/` tree.
- archival `SOLL_EXPORT_*.md` snapshots must not pollute routine delivery commits by default.
- derived site outputs should be committed intentionally, not by broad `git add docs/vision/`.

## Context Efficiency Rules
- Prefer `project_status` over composing many tools for a first pass.
- Prefer `retrieve_context` when an LLM needs a compact packet, not raw recall.
- Prefer `mode=brief` by default; only expand when the first answer is insufficient.
- Keep expert tools out of first-choice routing unless the task is truly diagnostic.
- When `anomalies`, `soll_validate`, `soll_verify_requirements`, and `soll_work_plan` differ on a greenfield project, prefer the canonical completeness axes exposed by SOLL surfaces and treat anomaly-only intent gaps as heuristic until proven canonical.

## Maintenance Rule
Update this skill immediately when:
- tool names or visibility change
- public/expert routing changes
- identity rules change
- SOLL workflow or schema changes
- release/promotion protocol changes
