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

`status` IST freshness signal (REQ-AXO-106):
- canonical field: `data.availability.ist_projection_fresh` (bool)
- legacy alias: `data.availability.advanced_indexed_surfaces_visible` (kept for compat — read `ist_projection_fresh` instead)
- text label: `**IST projection freshness:** fresh|stale (hint)`
- `stale` does NOT gate any tool — structural reads remain authoritative; only consider it as a freshness lag indicator when interpreting results

Brain semantic search capability (REQ-AXO-128 / DEC-AXO-061 / CPT-AXO-022):
- the brain process spawns an in-process CPU query embedding worker at boot under non-indexer profiles (brain_only, indexer_graph). The worker uses the same fastembed model the indexer uses for chunk vectorization, so query and chunk embeddings live in the same vector space — DuckDB `array_cosine_distance` produces meaningful similarity scores.
- LLM clients can call `query` with multi-token natural-language input under brain_only and receive ranked semantic results; no profile change is required.
- the `unavailable_embedding_reason` message reached when the registered sender is None now describes either a transient indexer GPU subprocess outage OR a CPU embedder load failure (model snapshot missing / corrupt). Both are recoverable; neither is a permanent profile boundary anymore.

Subsystem-tagged readiness contract (REQ-AXO-098 / DEC-AXO-062 / CPT-AXO-023):
- `mcp__axon__status` exposes `data.readiness` (rolled-up tristate `ready | degraded { reasons[] } | failed { reasons[] }`) and `data.subsystems[]` (per-subsystem `name | state.kind | state.reason | last_observed_at_ms`).
- Subsystems: `brain_mcp`, `ist_writer`, `ist_reader`, `dashboard`, `embedder`, `watcher`. Failed dominates Degraded; Degraded dominates Ready. Empty registry collapses conservatively to Ready.
- Reasons are subsystem-prefixed (`embedder: model_load_failed`, `dashboard: sql_econnrefused`) so an LLM client can act on the right component without prose parsing.
- Legacy `data.truth_status` and `data.availability.degraded_notes[]` preserved as aliases.
- start.sh prints `Axon is Ready` when overall readiness is `ready`; `⚠️ Axon started DEGRADED: ...` or `⚠️ Axon FAILED to reach a ready state: ...` otherwise. The legacy unconditional `Axon is rising` message is kept as a fallback only when the readiness probe itself fails.
- This contract is the prerequisite for REQ-AXO-097 (watchdog) and REQ-AXO-094 BEAM alarm classification — both project per-subsystem state changes onto this registry rather than introducing parallel signals.

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

`query` underscore-aware fuzzy matching (REQ-AXO-088):
- `_`, `-`, `:`, and ` ` are token separators in the wildcard form, so `query reserve_budget` matches `reserve_memory_budget` via the `reserve%budget` LIKE pattern; do not pre-split underscore-separated terms before issuing a query.

Public tool contract:
- public tools should return a usable recovery path even on weak or empty answers
- `query` with zero exact hits should still return structured recovery guidance, not a dead-end empty answer
- `path` and comparable graph-flow tools should still expose canonical provenance and next-step guidance when anchors are missing
- invalid arguments should return a micro-instruction that tells the LLM how to repair the request before retry
- recovery hints must reflect the actual response state. `inspect`, `path` (axon_bidi_trace), `impact`, and `simulate_mutation` with zero suggestions must NOT advise "pick / retry with one suggested symbol"; they route the LLM to `query` with a broader term or to verify spelling/scope (REQ-AXO-043). All four tools now emit `data.next_action.kind` matched to whether suggestions actually exist (`pick_canonical_symbol` / `select_valid_symbol_then_retry_impact` / `select_valid_symbol_then_retry_simulate` when they do, `broaden_search` when they do not).
- `retrieve_context` with an empty `question` returns a structured contract: `data.status="input_invalid"`, `data.missing_field="question"`, `data.next_action` with a concrete example call, `data.operator_guidance.follow_up_tools=["inspect","query"]` (REQ-AXO-043).
- `why` with empty/whitespace `symbol` AND no `question` returns the same `input_invalid` contract with `data.missing_field="symbol_or_question"` instead of producing a malformed "Why does  exist?" question (REQ-AXO-043).
- `soll_query_context`, `soll_work_plan`, `anomalies`, `entrench_nuance`, `soll_validate`, `soll_verify_requirements`, `infer_soll_mutation`, `conception_view`, and `change_safety` with an unregistered `project_code` return `data.status="wrong_project_scope"`, `data.registered_project_codes` (array of valid codes), and `data.operator_guidance.follow_up_tools=["project_registry_lookup", "axon_init_project"]`. Previously these returned a mix of the framework's generic "Invalid arguments", silent `Status: ok` with empty Evidence (conception_view returning 0 modules / change_safety returning Safety=unsafe with low confidence — both misreadable by an LLM caller as real signals rather than invalid inputs), or a bare "Canonical project error: <anyhow>" / "Entrenchment failed: <anyhow>" / "Inference failed: <anyhow>" string. The nine call sites now share a single helper `wrong_project_scope_response` (in `tools_soll/project_registry.rs`); future tools that accept `project_code` should adopt it via the one-line call `return Some(self.wrong_project_scope_response(project_code, "TOOL"));` (REQ-AXO-043).
- `axon_apply_guidelines` with an empty `accepted_global_rule_ids` array OR an array containing only unknown/unregistered rule IDs surfaces `isError=true` plus `data.applied=[]`, `data.unknown_global_rule_ids=[<bad ids>]`, `data.recovery_hint`, and (for empty input) `data.empty_input=true`. Previously the tool returned `"Inheritance applied. New local rules created: []"` with no error indicator, misleading the LLM into thinking the operation succeeded. Partial-success calls (mix of valid + unknown IDs) keep `isError` absent but populate both `data.applied` and `data.unknown_global_rule_ids` so the caller can retry the unknowns separately (REQ-AXO-043).
- `soll_manager` writer errors (insert/update branches) are normalized via the `normalized_soll_writer_error` helper. The LLM-visible `content.text` shows only the action kind (insert_failed / update_failed) plus a category (duckdb_writer / forbidden_relation / target_not_found / registry_unknown_id_kind / unknown) and a tailored recovery hint — never the raw DuckDB INSERT/UPDATE statement or partially-substituted bound metadata. The truncated raw error is preserved under `data.diagnostic_excerpt` (240 chars max, newlines stripped) for opt-in inspection. The duckdb_writer category specifically points to REQ-AXO-091 (placeholder bug, fixed in dev, pending live promotion) so an LLM that hits a `?` collision gets actionable guidance immediately (REQ-AXO-125).
- `soll_export` is **snapshot-per-release** (REQ-AXO-126 final policy, decided 2026-05-01). The auto-export hook on `axon_commit_work` is removed — exports are part of the qualified-release lineage (PIL-AXO-005), not a side-effect of routine commits. The MCP tool stays available on demand; the canonical caller is `scripts/release/promote_live_safe.sh` which fires `soll_export` once after the final qualify-mcp passes. Operators may also call it ad-hoc via `./scripts/axon --instance live mcp-call call soll_export`. There is no env-var gate (the prior `AXON_SOLL_EXPORT_ENABLED` was removed) because the per-call rate is now bounded by promotion frequency. The 764 accumulated `docs/vision/SOLL_EXPORT_*.md` files were deleted on 2026-05-01.
- `status` brief mode (the default) no longer inlines the ~60-name `public_tools` catalog in the human-readable text. Brief shows the count plus a pointer ("full list available via `status mode=verbose` or in `data.public_tools`"); verbose keeps the inline list. `data.public_tools` is still always-on so machine consumers (LLM clients, dashboards) are unaffected. The change frees ~700 chars of LLM context window on every status call (REQ-AXO-104).
- `axon_init_project` returns a stable kickoff bundle in `data.kickoff_bundle` on every call (first-init AND re-init), not just the project_code assignment. Bundle fields: `kickoff_prompt` (DEC-PRO-001 description verbatim, with hardcoded fallback), `methodology_summary` (CPT-AXO-019 description verbatim, with hardcoded fallback), `entry_points` (machine-readable cold-start reading order — file/mcp/cypher steps in order), `active_handoff` (path to the most recent `docs/working-notes/<date>-handoff-*.md` if one exists, otherwise null). The bundle is identical for first-init and re-init so an LLM with only Axon MCP access can call axon_init_project once and have everything it needs to onboard, regardless of whether the project is already known to the registry. Future changes to the kickoff protocol go in DEC-PRO-001 / CPT-AXO-019 (single source of truth) and the bundle picks them up automatically (REQ-AXO-119).
- `retrieve_context`, `query`, and `inspect` auto-resolve `project` from the cwd (via `AXON_PROJECT_ROOT` first, falling back to `current_dir`) when the caller omits it, by matching against `soll.ProjectCodeRegistry`. When exactly one registered project's `project_path` matches the cwd or is a prefix ancestor of it, the response scope becomes `project:<code>` instead of `workspace:*`. Ambiguous matches and unmatched cwds preserve the historic `workspace:*` fallback. The shared helper `auto_resolve_project_code_str` lives in `tools_framework_runtime_status.rs`; future IST/DX tools that accept an optional `project` should call it the same way (REQ-AXO-089). Side-effect of the fix: the legacy `auto_detect_project_code_from_cwd` (used in status output) was silently returning `null` because it parsed `query_json` rows as objects (`row.get("project_code")`) when the actual format is array-of-arrays — the new helper parses correctly so status now exposes the auto-detected code.

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
- `axon_init_project` assigns `project_code`. The response includes `data.path_exists_on_disk` and a `data.warnings` array; when `path_exists_on_disk=false`, the warning carries `kind="path_does_not_exist_on_disk"` and the LLM-visible content paragraph instructs `mkdir -p` or re-init with the corrected path. Registration still succeeds (REQ-AXO-118 — non-blocking warning, preserves the "register a future project" use case).
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
- canonical pair `CPT -BELONGS_TO-> PIL` exists for Concepts that formalize Pillar-level operational protocols (e.g. `CPT-AXO-019 -> PIL-AXO-003`); use it instead of routing the dependency through a Requirement (REQ-AXO-115)
- when a link request fails, `content.text` is sanitized: human-readable cardinality / policy errors pass through verbatim, but DuckDB writer errors are replaced with a recovery hint (REQ-AXO-043 / REQ-AXO-125). `data` keeps the flat `relation_guidance` shape (`pair_allowed`, `allowed_relations`, `canonical_examples`).

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
