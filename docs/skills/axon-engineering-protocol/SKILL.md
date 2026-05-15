---
name: axon-engineering-protocol
description: Use when working in the Axon repository and choosing MCP tools, runtime entrypoints, SOLL mutation paths, qualification commands, or live/dev release actions.
---

# Axon Engineering Protocol — LLM contract (Axon-repo internal)

LLM-only doc per CPT-AXO-024 (LLM-only doc methodology); canonical bodies in SOLL via `soll_query_context`; scope `project_code=AXO`, consumer projects use `/axon-driven-development`.

IDs per DEC-AXO-085 (ID format); SQL renamed from cypher per MIL-AXO-017 (AGE retirement); graph traversal tools (`impact`, `path`, `why`, `anomalies`, `retrieve_context`) are thin wrappers on `db/ddl/04_graph_functions.sql` (`public.impact`/`callers_of`/`why_chain`/`blast_radius`/`path`/`retrieve_context_v2`) over `public.Edge` — `relation_type` values are UPPERCASE (`CALLS` / `CALLS_NIF` / `CONTAINS`, written by `stage_a3.rs`) (REQ-AXO-296 + REQ-AXO-297 dual-write + REQ-AXO-299 tool bascule + REQ-AXO-350 `skip_legacy_relations` retirement / `anomalies` returns real findings); `documente` / `save observation` → `document_intent` (REQ-AXO-141 (auto-classifier)), never `soll_manager`.

## Boot

Triggers `Axon init` / `init Axon` / `Axon démarre` / `go` / `continue` / `reprends` → `mcp__axon__axon_init_project project_path=<cwd>`; no trigger → `help()` → `status()` → `help(tool=X)`.

`data.kickoff_bundle`: run `kickoff_prompt` / `methodology_summary` / `entry_points` verbatim; apply `session_pointer` first; `bootstrap_required` → `/bootstrap-soll`.

## Truth & runtime

| Source | Authority |
|---|---|
| MCP `status` / `project_status` | runtime + project truth |
| `./scripts/axon-{live,dev} status` | local lifecycle probe |
| `data.readiness` / `data.subsystems[]` | per-subsystem (REQ-AXO-098 (readiness contract)) |
| `data.availability.ist_projection_fresh` | freshness lag, not a gate |

**live** ports 44127-44132 · **dev** ports 44137-44142 · **brain** MCP+SOLL writer · **indexer** canonical IST writer · **dashboard** observation only.

## Tool routing

| Task | Tool |
|---|---|
| Symbol / inspect / evidence | `query` / `inspect` / `retrieve_context` (or `retrieve_context_layered`) |
| Blast radius / why / flow | `impact` / `why` / `path` |
| Risks / safety / arch | `anomalies` / `change_safety` / `conception_view` |
| Runtime / project state | `status mode=brief` / `project_status` |
| SOLL intent / relation | `soll_query_context` / `soll_relation_schema` |
| Raw SQL (preceded by `schema_overview`/`list_labels_tables`) | `sql` |
| Document observation | `document_intent` |
| SOLL batch / single | `soll_apply_plan` (`dry_run=true`; REQ-AXO-323 (silent UPSERT)) / `soll_manager` |

## Recovery

Follow `next_action` > `operator_guidance.follow_up_tools` > `parameter_repair`. Never inspect Axon source as recovery.

| Status | Action |
|---|---|
| `input_invalid` / `invalid_arguments` | inspect `parameter_repair`; `help(tool=X)` |
| `wrong_project_scope` | `project_registry_lookup` → `axon_init_project` |
| `input_not_found` / `symbol_found=false` | `parameter_repair.suggestions` or widen `query` |
| `input_ambiguous` | pick exact symbol or narrow scope |
| `degraded` / `partial` / `rejected_all` | treat partial; inspect `parameter_repair`; retry |
| weak `why` | tighter `retrieve_context` then `inspect`/`impact` |
| `diagnose_indexing` | id + remediation (REQ-AXO-212 (machine-stable IDs)) |

## SOLL writes — `help(tool=X)` for shape

- `soll_apply_plan` — batch (`dry_run=true`, `logical_key`, `author`)
- `soll_manager create|update|link` — single op ; `create` returns `id_exists` error envelope on existing id (no silent overwrite per REQ-AXO-323 Fault 2) — use `update` for modifications ; registry counters self-seed from MAX(numeric_suffix) on `ensure_soll_registry_row` so post-hoc project registration cannot allocate colliding ids (REQ-AXO-323 Fault 3)
- `document_intent` — observation auto-classifier
- `soll_attach_evidence` — proof file/test/metric/diff
- `soll_commit_revision` — checkpoint per `preview_id`
- `axon_apply_methodology_bundle` — versioned bundle

Async: `job_id` → `job_status`; `wait=true` blocks. `soll_work_plan` / `soll_verify_requirements` shape: CPT-AXO-024. `soll_work_plan` excludes terminal-status nodes (`delivered`/`superseded`/`completed`/`archived`/`rejected`) from Wave 1 and from `unblocks N`; cycle / descendant / wave algorithms consume the `SollSnapshot` petgraph (REQ-AXO-135 + REQ-AXO-346). REQ-AXO-91500 patch A: `unblocks N` counts the 6 canonical filiation relations (SOLVES/BELONGS_TO/TARGETS/REFINES/EXPLAINS/VERIFIES); cycle detection + Kahn waves keep the narrow SOLVES+BELONGS_TO scope. REQ-AXO-91484: `status mode=verbose|full` exposes `data.ist_call_graph_coverage` = `{per_project[code][lang]: {fns, outgoing_calls, coverage_ratio}, alerts: ["<proj>:<lang>:zero_outgoing_calls"]}` (lang ∈ rust/python/elixir/elixir_script/typescript/tsx ; alert fires when `fns > 100 ∧ outgoing_calls = 0`). REQ-AXO-91492: `soll_acyclic_audit project_code=<P>` enumerates SCC>1 and self-loops in the SOLL graph (pre-requisite for DEC-AXO-098 cycle validator activation). REQ-AXO-91486: `ist_snapshot_warm project_code=<P>` cold-loads the IST CSR snapshot into the process cache ; with `AXON_IST_RAM_ENABLED=1` the migrated call-sites (`get_circular_dependency_count_fast`, `collect_structural_neighbors`) dispatch to RAM (sub-µs neighbor lookup) with silent PG fallback on cache miss. REQ-AXO-91487: PG triggers on `public.symbol|edge` fire `pg_notify('ist_mutated', json)` ; the listener evicts the affected project from the cache (50ms coalescing window). REQ-AXO-91488: petgraph-backed tools over the CSR — `ist_centrality_pagerank top=N`, `ist_structural_sccs`, `ist_shortest_path from=<id> to=<id>` (all require `ist_snapshot_warm` first). REQ-AXO-91489: `mcp::tools_context::rrf_fusion::rrf_fuse(inputs, k=60, alpha, require_reachable, top)` implements Reciprocal Rank Fusion (Cormack 2009) across vector/FTS/graph rankings with optional PageRank centrality boost (`× (1 + α × pagerank_norm)`) and reachability filter for routes Impact/Wiring.

## Vision rule

`[Project] transforms [trapped/lost/expensive] into [accessible/durable/multiplied] for [humans/teams/enterprises]` — non-technical.

## Delivery — UN FIX = UN COMMIT

1. `status`
2. `query` / `inspect` / `retrieve_context`
3. `impact` / `why` / `path` / `anomalies` if needed
4. SOLL update (REQ + evidence BEFORE code)
5. `axon_pre_flight_check` (`incremental: true` per-file)
6. Pre-stage `git add <files>`
7. `axon_commit_work` (`project_path` or `project_code`)
8. Verify `git status`

~30-150 LOC + test + SKILL.md update only if `tools_*.rs` changed. Never `git commit` raw.

## Release & qualification

- `bash scripts/release/promote_live_safe.sh --project AXO` — never manual `cargo build --release` + copy
- `./scripts/axon qualify --profile <smoke|demo|full|ingestion|retrieval> [--cold] [--tensorrt]`
- `./scripts/axon qualify-mcp --surface <core|soll> --checks <quality,latency> --project AXO`

DEC-AXO-060 (4-verb runtime contract) folded 14 legacy verbs.

## 3-way triage (CPT-AXO-025 (triage policy)) — every unexpected MCP result

| Branch | Trigger | Action |
|---|---|---|
| 1 Hallucination | Unverified col/type/param | `schema_overview` + 3 repros; if explained → drop |
| 2 Axon bug | Reproducible vs contract | `soll_manager create requirement` tags `axon-bug` `llm-contract` |
| 3 Value-add | Documented but underperforms | `soll_manager create requirement` tags `axon-product-improvement` `commercial-value` |

Never log without a branch (REQ-AXO-129 (cautionary anti-pattern) corrupted CPT-AXO-021 (bootstrap prompt)).

## Sub-agents

Allowed: shell, doc, MCP-independent. Forbidden: code exploration (no MCP → 100-200K tokens). GUI-PRO-027 (token-economy sub-agent policy).

## Arch CPTs

CPT-AXO-054 (streaming pipeline v2), CPT-AXO-053 (canonical product split) for perf/arch.

## Hand Off

GUI-PRO-028 (Axon Hand Off, 5 steps). Body via `soll_query_context`. SKILL.md edits in step 4 forbidden unless tool name / surface visibility / runtime authority changes.
