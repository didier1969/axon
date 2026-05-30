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
| Methodology surface (REQ-AXO-91580/81/82) | `skill_list` → `skill_invoke` ; `prompt_template_get` ; `re_anchor reason=<signal>` for single-call context refresh |

## Recovery

Follow `next_action` > `operator_guidance.follow_up_tools` > `parameter_repair`. Never read Axon source as recovery (PIL-AXO-002).

| Status | Action |
|---|---|
| `input_invalid` / `invalid_arguments` / `wrong_project_scope` | `parameter_repair` + `help(tool=X)` or `project_registry_lookup` |
| `input_not_found` / `symbol_found=false` | `parameter_repair.suggestions` or widen `query` |
| `degraded` / `partial` / `rejected_all` | treat partial ; inspect `parameter_repair` ; retry |
| `trust:degraded` + blocker `indexed_projections_not_fresh` | run `data.truth_cockpit.recovery_hint.command` (REQ-AXO-91497) ; never re-call `status` itself |
| soll_manager `category:writer_failed` | inspect `data.diagnostic_excerpt` for PG error ; recovery via `sql SELECT id FROM soll.node` |
| `status` `trust:degraded` | `data.truth_cockpit.staleness.{last_publish_ts,modified_files_since,oldest_modified_age_seconds,sample_paths[5]}` exposes magnitude (REQ-AXO-231) — route on counts/age, not just the boolean |
| `soll_relation_schema` returned | `data.canonical_direction` (`"SRC -> TGT"`) + `data.allowed_relation_types` (flat array) + `data.reverse_canonical` (legal inverse when pair forbidden) — REQ-AXO-91495 ; visible text now lists the actual relations, not just "resolved" |
| `query` results consumption | `data.results[{name,kind,uri,project,surface,score}]` (direct hits only ; `surface` ∈ `symbol_index` / `symbol_index_semantic` / `symbol_index_degraded`) + `data.context.related_symbols_via_graph[]` (string array, 1-hop neighbors via CALLS/CONTAINS) + `data.surfaces_used` + `data.surfaces_degraded` + `data.total_available` + `data.next_call_hint` + `data.pagination` — REQ-AXO-91508 ; markdown table in `content[0].text` preserved for backward compat |
| `inspect` envelope | Same envelope fields as `query` (`surfaces_used`/`surfaces_degraded`/`total_available`/`next_call_hint`/`pagination`) + `data.context.related_symbols_via_graph[]` ; no `results[]` (single-symbol drill-down preserves `data.symbol` / `data.summary` as canonical shape) — REQ-AXO-91509 |
| `path` envelope | Tri-modal envelope (`surfaces_used:["graph_ram"]` or `["graph_pg"]` + `surfaces_degraded:["graph_ram_unavailable"]` on PG fallback / `total_available` (1 if path found else 0) / `next_call_hint` / `pagination`) ; canonical result stays `data.path[]` + `data.edge_kinds[]` ; no `results[]` (path IS the result). RAM-first via `IstGraph::bfs_shortest_path` (PIL-AXO-9002) — REQ-AXO-91510 |
| `bidi_trace` envelope | Tri-modal envelope (`surfaces_used:["graph_ram"]`/`["graph_pg"]` + `surfaces_degraded` / `total_available`=callers+callees / `next_call_hint` / `pagination`) ; RAM-first via `IstGraphView::reverse_at_radius`+`forward_at_radius` ; PG `WITH RECURSIVE` fallback only when cache cold or query unscoped — REQ-AXO-91511 |
| `impact` envelope | Tri-modal envelope (`surfaces_used:["graph_ram"]` + `surfaces_degraded:["inferred_bridge_edges_unavailable_in_ram_v1"]` or `["graph_pg"]` + `["graph_ram_unavailable"]` / `total_available`=impact_radius / `next_call_hint` / `pagination`) ; RAM-first via `IstGraphView::reverse_at_radius`+`direct_edge_relation` (per-caller edge classification) ; PG `callers_of` fallback when cold — REQ-AXO-91512 |
| `soll_work_plan actionable=true` | Returns open Requirements (status non-terminal) ordered by `(parent_score DESC, priority ASC, score DESC, id ASC)` instead of parent Decisions/Milestones. Each item carries `reasons[0]="actionable_leaf (parent_score=N)"`. Orphan REQs (no schedulable parent) fall back to their own score. `data.metadata.actionable=true` confirms the alternate surface — REQ-AXO-346 Slice 2 |
| `embedding_status` runtime heartbeat | `structuredContent.runtime_pending_count` = process-global `EmbedderRuntimeState::pending_count()` ; `structuredContent.runtime_idle` = `pending_count == 0`. Compare against `pending_chunks` (DB-derived ground truth) — wide divergence flags NOTIFY listener drop or missed `mark_embedded`. Surface needed by Slice 3 `EmbedderLifecycle` T_idle sleep decision — REQ-AXO-90009 Slice 2 |
| `embedding_status` lifecycle phase | `structuredContent.lifecycle_phase` ∈ `{"ready","sleeping"}` (`EmbedderPhase`). `lifecycle_wake_count` / `lifecycle_sleep_count` count transitions. `lifecycle_last_used_ms` epoch ms of last `request_wake`. Slice 3A surfaces the state machine ; Slice 3B will wire the actual GpuB2Embedder session drop — REQ-AXO-90009 Slice 3 |
| `embedding_status` (s, Q) stock + replenish | `structuredContent.pipeline_a.stock_discovered` = pipeline A backlog via `GraphStore::pipeline_a_discovered_stock` (same WHERE clause as `select_and_claim_files_for_indexing` so stock reconciles with claim eligibility). `pipeline_a.replenish` + `pipeline_b.replenish` = in-process `DemandPullMetrics` snapshot (pulls / items_fed / empty_pulls / try_send_failures / skipped_above_threshold), `null` when the demand_pull spawn hasn't initialised the global pointer (brain_only mode). Pipeline B backlog stays surfaced as the top-level `pending_chunks` field — no duplicate stock_b — REQ-AXO-901816 |
| `axon_init_project` kickoff_bundle timeouts | `read_recent_req_commits` bounds the `git log --grep=…` shell-out to 2 s ; over-budget returns `[]` so the rest of the bundle still assembles. Prevents MCP gateway 30-s timeout under WSL2 fs latency / parallel build lock contention — REQ-AXO-287 |
| `change_safety` envelope | Tri-modal envelope (`surfaces_used:["symbol_index","soll_traceability"]` / `surfaces_degraded` / `total_available:1` / `next_call_hint` / `pagination`) ; no `results[]` (single-verdict shape preserved, same logic as inspect). Both surfaces stay PG-backed — RAM IST snapshot doesn't carry the `tested` flag — REQ-AXO-91514 |
| `axon_commit_work` refactor-exempt gates | Guidelines whose metadata carries `exempt_for_refactor: true` step aside when the commit message starts with a Conventional-Commits `refactor` type (`refactor:` / `refactor(<scope>):` / `refactor!:` / `refactor(<scope>)!:`). Default seeds set the flag on GUI-PRO-001 (TDD) + GUI-PRO-002 (Documentation MCP) so pure dead-code / SQL-dialect collapses stop being blocked. `feat`/`fix`/`chore` stay strictly gated. Live SOLL nodes seeded before this change keep the old metadata until `soll_manager action=update entity=guideline data={id,metadata}` refreshes them — REQ-AXO-91569 |

## SOLL writes

- `soll_apply_plan` — batch (`dry_run=true`, `logical_key`, `author`) ; `soll_commit_revision` checkpoint per `preview_id`.
- `soll_manager create|update|link` — single op ; contract details (id_exists, registry seeding, default `status=planned`, validation `result` precedence) : REQ-AXO-323. **MIL-AXO-020** : id is DB-allocated via `soll.allocate_node_id(type, project_code)` ; caller-provided `data.id`/`reserved_id` rejected with `id_field_forbidden`. `create` (non-Vision) requires `attach_to`+`relation_type` ; node + edge land in a single CTE so neither survives in isolation on failure. Vision creation forbidden outside `axon_init_project` (`vision_creation_forbidden`). `link` pre-checks cycles on the filiation set `{SOLVES, BELONGS_TO, REFINES, TARGETS, EXPLAINS, VERIFIES}` (DEC-AXO-098) — `cycle_detected` envelope with offending source/target. `SUPERSEDES` requires same-type endpoints + non-retired target ; INSERT edge + UPDATE source status='current' + UPDATE target status='superseded' land in one CTE. Envelopes : `attach_required`, `attach_target_not_found`, `forbidden_relation_for_type`, `cycle_detected`, `supersedes_type_mismatch`, `supersedes_target_already_retired`. Note : MCP parameterised queries always use `?` placeholders (graph_query.rs::expand_named_params translates them to plugin-native syntax) — never `$N`.

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
