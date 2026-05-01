---
name: axon-engineering-protocol
description: Use when working in the Axon repository and choosing MCP tools, runtime entrypoints, SOLL mutation paths, qualification commands, or live/dev release actions.
---

# Axon Engineering Protocol

## Overview
Axon is MCP-first. Read truth from MCP before inferring from files or shell output.

Core rule:
- read `IST`/`SOLL` first
- mutate second
- certify before commit

Never invent:
- canonical IDs
- `project_code`
- intent links
- runtime authority

## First Truth
Use these in order:
1. `help()` — returns Axon identity, value proposition, and tool routing
2. `status` — returns runtime truth; `project_code` is auto-detected from cwd
3. `project_status` if the task is project-scoped
4. `mcp_surface_diagnostics` if client/server binding looks stale

Note: `project_code` is auto-resolved when omitted:
- single registered project: used automatically
- multiple projects: matched against working directory
- use `help(tool=X)` to see any tool's JSON input schema and examples

Truth hierarchy:
- MCP `status` = runtime/protocol truth
- `project_status` = compact project truth
- `./scripts/axon-live status` / `./scripts/axon-dev status` = local lifecycle probe only
- `README` / docs = supporting context, not runtime truth

## Runtime Model
Two operator instances:
- `live` = stable truth runtime
- `dev` = isolated development runtime

Use:
- `./scripts/axon --instance live ...`
- `./scripts/axon --instance dev ...`
- or `./scripts/axon-live ...` / `./scripts/axon-dev ...`

Shared contract:
- `live` and `dev` expose the same public MCP product surface
- ports, sockets, pidfiles, and state roots differ

Standard authority model:
- `brain` = public MCP authority
- `brain` = `SOLL` writer authority
- `indexer` = canonical `IST` writer authority
- fresh indexed projections come from `indexer`

Authority rule:
- public MCP status must not expose topology names or topology choices
- there is one runtime deployment contract, not `split`, `monolith`, or `standard` product variants
- describe runtime truth through `process_role`, writer authorities, readiness, freshness, and convergence

Dashboard rule:
- dashboard is an observation surface
- it is not canonical authority for ingestion, `IST`, `MCP`, `SQL`, or release truth

Endpoint rule:
- `instance_identity.*_url` = runtime-local truth
- `advertised_endpoints.*` = client-facing truth
- isolated clients should prefer `advertised_endpoints`

## Surface Model
Assume:
- one public MCP product surface
- `sync` by default
- `async` only for allowlisted heavy operations

Public information surface stays available in `brain_only`.
If freshness is missing:
- degrade payloads explicitly
- do not hide public read tools

Internal-only tools:
- are transport or implementation primitives
- are not hidden product value surfaces

## Tool Routing
Default routing:
1. `status`
2. `project_status`
3. `query` / `inspect` / `retrieve_context`
4. `impact`
5. `why`
6. `path`
7. `anomalies`
8. `change_safety`
9. `conception_view`
10. SOLL mutation tools
11. `axon_pre_flight_check`
12. `axon_commit_work`

Use:
- `query` = discover broadly
- `inspect` = zoom on a known target
- `retrieve_context` = compact answerable packet
- `impact` = blast radius
- `why` = rationale
- `path` = source-sink flow
- `anomalies` = structural findings
- `change_safety` = mutation safety
- `conception_view` = derived architecture map
- `schema_overview` / `list_labels_tables` / `query_examples` = advanced read-only schema guidance when product tools are insufficient
- `cypher` = advanced read-only graph query escape hatch after schema/examples, not the default answer path

Prefer:
- `project_status` over composing many probes
- `retrieve_context` over raw recall when context must stay compact
- `mode=brief` unless expansion is necessary
- product tools before raw schema/query tools

## Fallback Search
If the first Axon answer is weak, incomplete, or degraded, continue searching through Axon itself.

First read the recovery fields already returned by the server:
- `operator_guidance`
- `operator_guidance.llm_contract` when present
- `next_action`
- authoritative guidance fields when present:
  - `problem_class`
  - `likely_cause`
  - `next_best_actions`
  - `confidence`
- shadow guidance if exposed in `_shadow.guidance`

LLM guidance contract:
- `help` is an LLM-only routing tool and should stay isolated from the central MCP dispatcher implementation
- `operator_guidance.llm_contract.first` tells the client which field to execute first, normally `next_action`
- `operator_guidance.llm_contract.bad_args` is the canonical repair rule for malformed tool arguments
- `operator_guidance.llm_contract.partial` is the canonical rule for degraded or incomplete truth
- `operator_guidance.llm_contract.ask_user_only_if` is the boundary for explicit user input
- keep guidance strings compact and machine-actionable; avoid human-oriented explanations unless they add operational signal

Server guidance is primary. Generic fallback ordering is secondary.

Public tool contract:
- public tools should return a usable recovery path even on weak or empty answers
- `query` with zero exact hits should still return structured recovery guidance, not a dead-end empty answer
- `path` and comparable graph-flow tools should still expose canonical provenance and next-step guidance when anchors are missing
- invalid arguments should return a micro-instruction that tells the LLM how to repair the request before retry
- recovery hints must reflect the actual response state. `inspect`, `path` (axon_bidi_trace), `impact`, and `simulate_mutation` with zero suggestions must NOT advise "pick / retry with one suggested symbol"; they route the LLM to `query` with a broader term or to verify spelling/scope (REQ-AXO-043). All four tools now emit `data.next_action.kind` matched to whether suggestions actually exist (`pick_canonical_symbol` / `select_valid_symbol_then_retry_impact` / `select_valid_symbol_then_retry_simulate` when they do, `broaden_search` when they do not).
- `retrieve_context` with an empty `question` returns a structured contract: `data.status="input_invalid"`, `data.missing_field="question"`, `data.next_action` with a concrete example call, `data.operator_guidance.follow_up_tools=["inspect","query"]` (REQ-AXO-043).
- `why` with empty/whitespace `symbol` AND no `question` returns the same `input_invalid` contract with `data.missing_field="symbol_or_question"` instead of producing a malformed "Why does  exist?" question (REQ-AXO-043).
- `soll_query_context`, `soll_work_plan`, `anomalies`, `entrench_nuance`, `soll_validate`, `soll_verify_requirements`, and `infer_soll_mutation` with an unregistered `project_code` return `data.status="wrong_project_scope"`, `data.registered_project_codes` (array of valid codes), and `data.operator_guidance.follow_up_tools=["project_registry_lookup", "axon_init_project"]`. Previously these returned a mix of the framework's generic "Invalid arguments", silent `Status: ok` with empty Evidence, or a bare "Canonical project error: <anyhow>" / "Entrenchment failed: <anyhow>" / "Inference failed: <anyhow>" string. The seven call sites now share a single helper `wrong_project_scope_response` (in `tools_soll/project_registry.rs`); future tools that accept `project_code` should adopt it via the one-line call `return Some(self.wrong_project_scope_response(project_code, "TOOL"));` (REQ-AXO-043).

Search order for ordinary LLMs:
1. follow `next_action` if the response already provides one
2. follow `operator_guidance.follow_up_tools` when present
3. if guidance says `input_not_found`, retry with the suggested symbol or widen with `query`
4. if guidance says `input_ambiguous`, pick an exact symbol or narrow the project scope
5. if guidance says `wrong_project_scope`, recover the canonical project with `project_status`
6. if guidance says `degraded`, treat the result as partial and retry after runtime stabilization
7. otherwise tighten the question and call `retrieve_context`, then `inspect`, then `query`
8. use `impact` or `path` if the missing truth is authority, source/sink flow, or blast radius
9. use `conception_view` or `project_status` for architecture/runtime framing

Rules:
- search Axon through MCP before searching Axon through implementation
- do not use Axon source code as the ordinary recovery path
- read target project files only after the MCP search path is exhausted
- if Axon still cannot answer, report the degraded contract, the guidance fields returned by Axon, and the MCP calls already attempted

## Why Contract
Read `why` as a machine contract first, prose second.

Field priority:
1. `authority_class`
2. `evidence_provenance`
3. `link_mode`
4. `evidence_states`
5. `rationale_quality`

Meaning:
- `authority_class=governing` = canonical intent
- `authority_class=supporting` = useful support
- `authority_class=correlated` = weak signal

Meaning:
- `link_mode=direct` = explicit traceability
- `link_mode=inferred` = plausible derived intent
- `link_mode=weak_correlation` = do not treat as canonical

Important:
- `why` may recover governing intent through concept-linked `SOLL` requirements or decisions
- under `Recovering` pressure, rationale routes may still keep a minimal `SOLL` join
- graph-heavy expansion may still remain guarded

If `why` returns:
- `missing_governing_intent`
- `no_direct_traceability`
- `retrieval_degraded`
- `support_only`

Then escalate to:
- `retrieve_context` with a tighter target or question
- `inspect`
- `query`
- `impact`
- `path`
- `conception_view`
- `project_status`

Do not treat weak `why` output as permission to inspect Axon implementation code as primary truth.
If Axon answer quality is weak, Axon must still tell the LLM how to proceed through MCP.

Fallback order for LLMs:
1. restate the target more narrowly and call `retrieve_context`
2. call `inspect` on the most concrete symbol, file, or entity already known
3. call `query` to widen recall if the anchor is still ambiguous
4. call `impact` or `path` if source/sink flow or authority is the missing dimension
5. call `conception_view` or `project_status` if the question is architectural
6. if the work concerns a user/project source target, read that target source only after the MCP surface has been exhausted
7. never use Axon implementation code as the recovery path for ordinary LLM operation; escalate through MCP, `status`, and operator guidance instead

Do not over-trust the prose summary.

## Async Contract
Server truth for async lives in:
- `status`
- `mcp_surface_diagnostics`

Canonical async follow-up fields:
- `known_ids`
- `next_action`
- `result_contract`
- `polling_guidance`
- `recovery_hint`
- `result_data`

Canonical follow-up tool:
- `job_status`

## Identity Contract
Canonical IDs are server-owned:
- `TYPE-CODE-NNN`

Rules:
- `project_code` comes from server truth
- `axon_init_project` assigns `project_code`
- clients reuse returned IDs
- batch plans should use `logical_key`

Never fabricate:
- `project_code`
- preview IDs
- revision IDs
- SOLL entity IDs

## SOLL Model
Canonical entity types (all reachable via `soll_manager(action=create)`):
- `Vision`
- `Pillar`
- `Requirement`
- `Decision`
- `Concept`
- `Guideline` — id prefix `GUI-<project>-NNN` (REQ-AXO-092 enabled this path; previously rejected by the create branch)
- `Milestone`
- `Validation`
- `Stakeholder`

Unknown-entity errors (REQ-AXO-043 contract): when `entity` is outside the enum, `soll_manager` returns `data.status="input_invalid"`, `data.accepted_entities`, `data.next_action`, and `data.operator_guidance.problem_class="input_invalid"`. Do not retry with cypher INSERT — that is "tricher avec le système"; file the missing-API requirement instead.

### Vision Formulation Rule

A Vision is the North Star of a project. It is NOT a technical description.

A Vision must answer:
- What problem does this project solve for humans and organizations?
- Why will people and enterprises pay for it?
- What transformation does it enable (before → after)?

Format: `[Project] transforms [trapped/lost/expensive thing] into [accessible/durable/multiplied value] for [humans/teams/enterprises].`

Rules:
- Never mention technologies, frameworks, protocols, or implementation details
- State the human and commercial value: productivity, knowledge retention, competitive advantage
- A new LLM reading the Vision must immediately understand this is a product people will pay for, not a technical exercise
- Technologies belong in Decisions, not in the Vision
- The Vision changes rarely (1-2x/year) and prevents scope drift

Example (good): "Axon makes every software team's accumulated knowledge instantly accessible and actionable for both human engineers and AI agents. A new team member reaches veteran-level effectiveness in minutes, not months."

Example (bad): "Axon is a Rust-first MCP server using DuckDB and ONNX Runtime for structural code analysis."

Read surfaces:
- `soll_query_context`
- `soll_work_plan`
- `soll_validate`
- `soll_verify_requirements`
- `soll_export`

Mutation surfaces:
- `soll_manager`
- `infer_soll_mutation`
- `entrench_nuance`
- `soll_apply_plan`
- `soll_commit_revision`
- `soll_rollback_revision`
- `soll_attach_evidence`

Use:
- `infer_soll_mutation` = read-only assistive scope check
- `entrench_nuance` = bounded update to existing canonical entities
- `soll_manager` = exact create/update/link
- `soll_apply_plan` = transactional batch

Before using an unfamiliar SOLL tool:
- call `help` with `tool=<tool_name>`
- read `data.input_schema.required`
- prefer the returned `usage_examples`
- follow `next_action` before exploring broadly

For `soll_apply_plan`:
- provide `project_code`
- start with `dry_run=true`
- include `author`
- use stable `logical_key` for idempotent creates/updates
- put batch entities under `plan.<type>s` — accepted: `pillars`, `requirements`, `decisions`, `milestones`, `visions`, `concepts`, `stakeholders`, `validations`, `guidelines` (REQ-AXO-092 closed the prior gap where guidelines/stakeholders/validations were silently dropped)
- put edges in top-level `relations`
- poll `job_status` when the response returns `job_id`

For `soll_work_plan`:
- use `format=brief`, `limit`, and `top` first
- keep `include_validation_details=false` unless requirement-level detail is explicitly needed
- expand with `soll_verify_requirements` or `include_validation_details=true` only after the compact answer identifies the gap

Before retrying a bad link:
- use `soll_relation_schema`

CLI bridge:
- use `./scripts/axon --instance live mcp-call call <tool> --args-file <file.json>` for large JSON payloads
- use `--args-file -` when streaming JSON from stdin
- avoid fragile inline shell JSON for large plans

## SOLL Evidence
Treat `soll_attach_evidence` as proof, not blind append.

It accepts:
- `artifact_ref`
- `path`
- `file_path`
- `uri`

Typical evidence kinds:
- `document`
- `file`
- `symbol`
- `test`
- `metric`
- `validation`
- `rationale`
- `diff`

Result contract (REQ-AXO-043):
- `data.status` ∈ `ok` | `partial` | `rejected_all` | `no_artifacts`
- `data.attached` / `data.total` — counts
- `data.next_action` — single-string remediation hint when `status != ok`
- `data.operator_guidance.problem_class` — `ok` | `input_empty` | `input_invalid` | `partial_input_invalid`
- `data.operator_guidance.next_best_actions` — array of remediation steps
- `data.artifact_diagnostics` — per-artifact reasons (always present)
- `content[0].text` — the LLM-visible line surfaces the failure mode (never just "Attached 0")

Requirement verification truth lives in:
- `missing_dimensions`
- `missing_dimensions_detailed`
- `suggested_next_actions`
- `coverage_reason`
- `completion_model`

## Delivery Flow
Use this default flow:
1. `status`
2. `query` / `inspect` / `retrieve_context`
3. `impact` / `why` / `path` / `anomalies` only if needed
4. update `SOLL` in the same wave if intent changed
5. `axon_pre_flight_check`
6. `axon_commit_work`

Do not use shell `git commit` for delivery in this repo.

## Release Flow
Canonical path for `live`:
1. `./scripts/axon promote-live-safe --project AXO`

Manual recovery only if necessary:
- `./scripts/axon release-preflight`
- `./scripts/axon create-release-manifest --state qualified`
- `./scripts/axon promote-live --manifest <manifest> --restart-live`

Promotion truth:
- MCP `status`
- runtime version in manifest/current release state

`promote-live-safe` must prove:
- canonical rebuild
- release preflight
- manifest creation
- live restart
- MCP runtime identity match
- final `qualify-mcp`
- final `axon-live status`

Fail closed if `HEAD` changes during the one-shot release sequence.

## Qualification
Use:
- `./scripts/axon qualify ...` — runtime qualification (defaults to dev)
- `./scripts/axon qualify-mcp ...` — MCP-surface qualification (core/SOLL)

Removed in DEC-AXO-060 Phase 3 (no longer dispatched):
- `quality-mcp`, `validate-mcp`, `measure-mcp`, `compare-mcp`, `robustness-mcp`
- `qualify-guidance`, `qualify-guidance-live`
- `qualify-dev-cold`, `qualify-dev-indexer-cold`, `qualify-dev-indexer-tensorrt-cold`, `build-and-qualify-tensorrt-cold`
- `reset-dev-baseline`, `reset-dev-indexer-baseline`
- `upgrade-topology`

Their behaviour folds into `qualify --profile <smoke|demo|full|ingestion|retrieval>` plus the `--cold` flag (resets the dev runtime baseline before qualifying — DEC-AXO-060 Phase 4 / REQ-AXO-113), the `--tensorrt` flag (TensorRT GPU envelope), and `--build-tensorrt-from-tarball PATH` (optional artifact build). Example replacements:
- legacy `qualify-dev-cold`            → `axon-dev qualify --profile ingestion --cold --mode brain_only`
- legacy `qualify-dev-indexer-cold`    → `axon-dev qualify --profile ingestion --cold --mode indexer_full`
- legacy `qualify-dev-indexer-tensorrt-cold` → `axon-dev qualify --profile ingestion --cold --tensorrt --max-vram-used-mb 2048`
- legacy `build-and-qualify-tensorrt-cold` → add `--build-tensorrt-from-tarball PATH` to the line above.

Use `./scripts/axon-live qualify ...` (or `--instance live`) only when you intentionally assess promoted `live`.

## Derived Docs
Derived docs are reading surfaces, not canonical truth.

Canonical:
- live `SOLL`
- `soll_export`

Derived:
- `soll_generate_docs`
- `docs/derived/soll/...`

Do not treat derived docs as restorable source of truth.

## Commit Hygiene
- `axon_commit_work` does not mean “stage everything”
- archival `SOLL_EXPORT_*.md` should not pollute routine delivery commits
- derived outputs should be committed intentionally

## Maintenance
Update this skill when:
- tool names change
- surface visibility changes
- runtime authority changes
- SOLL workflow/schema changes
- qualification or release protocol changes
