---
name: axon-engineering-protocol
description: Use when working in the Axon repository on any MCP tool call, SOLL mutation, runtime command, build, qualification, promotion, commit, or recovery from an unexpected MCP result. Also use when the operator says "axon init", "go", "continue", "reprends", "comment je", "fait un commit", "promote", "qualify", "handoff", or asks how to do anything in this repo.
---

# Axon Engineering Protocol — LLM contract (Axon repo, `project_code=AXO`)

Canonical bodies via `soll_query_context` / `sql`. Consumer projects → `/axon-driven-development`. GraphRAG / IST / RRF / scoring detail → `references/graphrag-and-soll-internals.md`.

## Boot

`Axon init` / `init Axon` / `Axon démarre` / `go` / `continue` / `reprends` → `mcp__axon__axon_init_project project_path=<cwd>`. No trigger → `help()` → `status()` → `help(tool=X)`. Run `data.kickoff_bundle.kickoff_prompt` / `methodology_summary` / `entry_points` verbatim. Apply `session_pointer` first. `bootstrap_required` → `/bootstrap-soll`.

## Truth & runtime

| Source | Authority |
|---|---|
| MCP `status` / `project_status` | runtime + project truth |
| `./scripts/axon-{live,dev} status` | local lifecycle probe |
| `data.readiness` / `data.subsystems[]` | per-subsystem (REQ-AXO-098) ; freshness = lag, not a gate |

Live 44127-44132 / dev 44137-44142. Brain = MCP + SOLL writer. Indexer = IST writer. Dashboard = observation only.

## Tool routing

| Task | Tool |
|---|---|
| Symbol / inspect / evidence | `query` → `inspect` → `retrieve_context` |
| Blast radius / why / flow / risks | `impact` / `why` / `path` / `anomalies` |
| Runtime / project state | `status mode=brief` / `project_status` |
| SOLL intent / relation | `soll_query_context` / `soll_relation_schema` |
| Raw SQL (after `schema_overview`) | `sql` |
| SOLL batch / single | `soll_apply_plan` (`dry_run=true`) / `soll_manager` |

## Recovery

Follow `next_action` > `operator_guidance.follow_up_tools` > `parameter_repair`. Never read Axon source as recovery (PIL-AXO-002).

| Status | Action |
|---|---|
| `input_invalid` / `invalid_arguments` / `wrong_project_scope` | `parameter_repair` + `help(tool=X)` or `project_registry_lookup` |
| `input_not_found` / `symbol_found=false` | `parameter_repair.suggestions` or widen `query` |
| `degraded` / `partial` / `rejected_all` | treat partial ; inspect `parameter_repair` ; retry |

## SOLL writes

- `soll_apply_plan` — batch (`dry_run=true`, `logical_key`, `author`) ; `soll_commit_revision` checkpoint per `preview_id`.
- `soll_manager create|update|link` — single op ; contract details (id_exists, registry seeding, default `status=planned`, validation `result` precedence) : REQ-AXO-323. **MIL-AXO-020** : id is DB-allocated via `soll.allocate_node_id(type, project_code)` ; caller-provided `data.id`/`reserved_id` rejected with `id_field_forbidden`. `create` (non-Vision) requires `attach_to`+`relation_type` ; node + edge land in a single CTE so neither survives in isolation on failure. Vision creation forbidden outside `axon_init_project` (`vision_creation_forbidden`). `link` pre-checks cycles on the filiation set `{SOLVES, BELONGS_TO, REFINES, TARGETS, EXPLAINS, VERIFIES}` (DEC-AXO-098) — `cycle_detected` envelope with offending source/target. `SUPERSEDES` requires same-type endpoints + non-retired target ; INSERT edge + UPDATE source status='current' + UPDATE target status='superseded' land in one CTE. Envelopes : `attach_required`, `attach_target_not_found`, `forbidden_relation_for_type`, `cycle_detected`, `supersedes_type_mismatch`, `supersedes_target_already_retired`.

Snapshot / async / `soll_work_plan` scoring → `references/graphrag-and-soll-internals.md`.

## Gotchas — excuse → reality

| Excuse | Reality |
|---|---|
| "Quick `cargo build --release` + copy to `bin/` is faster than the promote script" | Live binaries must match manifest identity (PIL-AXO-005). Use `bash scripts/release/promote_live_safe.sh --project AXO`. |
| "I'll spawn a sub-agent to read Axon source — faster" | No MCP in sub-agent → reconstructs IST from raw reads = 100-200K tokens (GUI-PRO-027). Main-thread MCP = 5-50 tokens. |
| "Status looks fine, IST is probably fresh enough" | `trust:degraded` or `freshness:stale` → frozen snapshot. Restart indexer-graph or qualify before trusting `inspect` / `impact`. |
| "I'll delete the bad SOLL node and recreate clean" | SOLL is preserve-always (PIL-AXO-003). Use `soll_rollback_revision`. Mass-deletes destroy intent history. |
| "I'll log a CPT-AXO-025 issue without picking a branch" | REQ-AXO-129 corrupted CPT-AXO-021 exactly this way. Pick 1 / 2 / 3 first. |

## Examples

**Fix a symbol** : `status mode=brief` (verify `freshness:fresh` + `trust:canonical`) → `query symbol=X` → `inspect symbol=X mode=verbose` → `impact symbol=X` → write fix + test → `axon_pre_flight_check diff_paths=[…]` → `git add …` → `axon_commit_work` → verify `git status`.

**Log a contract bug from a surprising MCP result** : pick CPT-AXO-025 branch 2 → `document_intent intent="<one-line>" body="<repro+evidence>" tags=["axon-bug","llm-contract"]` (server classifies as Requirement, assigns canonical id, status defaults to `planned`) → if needed `soll_manager action=link relation_type=REFINES` to umbrella REQ → `soll_attach_evidence` with `path` to repro file.

## Delivery — one fix = one commit

1. `status mode=brief` → confirm `freshness:fresh` + `trust:canonical`.
2. `query` / `inspect` / `retrieve_context` ; `impact` / `why` / `path` if needed.
3. SOLL update : REQ + evidence BEFORE code.
4. `axon_pre_flight_check` (`incremental:true` per-file) — required when `tools_*.rs` or MCP contract changed (GUI-PRO-002).
5. `git add <files>` then `axon_commit_work` with `project_path` or `project_code` ; verify `git status` after. Never `git commit` raw.

## Release & qualification (DEC-AXO-060 — 4-verb contract)

- `bash scripts/release/promote_live_safe.sh --project AXO`.
- `./scripts/axon qualify --profile <smoke|demo|full|ingestion|retrieval> [--cold] [--tensorrt]`.
- `./scripts/axon qualify-mcp --surface <core|soll> --checks <quality,latency> --project AXO`.

## 3-way triage on unexpected MCP results (CPT-AXO-025)

Pick exactly one : (1) hallucination → `schema_overview` + 3 repros, drop if explained ; (2) axon bug (reproducible vs contract) → `document_intent` tags `axon-bug` `llm-contract` ; (3) value-add → tags `axon-product-improvement` `commercial-value`.

## Pointers

Sub-agents : shell / doc / MCP-independent only ; never code exploration (GUI-PRO-027). Arch : CPT-AXO-054 (streaming pipeline v2), CPT-AXO-053 (canonical product split). Hand Off : GUI-PRO-028 (5 steps, body via `soll_query_context`) ; SKILL.md edits in step 4 forbidden except tool rename / surface change / runtime authority change / methodology change (GUI-AXO-1002).
