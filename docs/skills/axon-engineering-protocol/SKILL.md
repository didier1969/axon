---
name: axon-engineering-protocol
description: Use when working in the Axon repository and choosing MCP tools, runtime entrypoints, SOLL mutation paths, qualification commands, or live/dev release actions.
---

# Axon Engineering Protocol — LLM contract

LLM-only doc per CPT-AXO-024. For full prose see archived `docs/archive/2026-05-02/SKILL.md.bak`. For canonical concepts use `cypher SELECT description FROM soll.main.Node WHERE id='<ID>'`.

## Boot
On user phrase "Axon init" / "init Axon" / "Axon démarre" / "go" / "continue" / "reprends" → first call MUST be `mcp__axon__axon_init_project project_path=<cwd>`. Read `data.kickoff_bundle` (kickoff_prompt, methodology_summary, entry_points, session_pointer, active_handoff, in_progress_requirements, wave_1_unblockers, recent_req_commits, recent_soll_writes). REQ-AXO-143: `session_pointer = {kind, value, label?}` is the canonical workflow-agnostic onboarding pointer (`kind ∈ file|url|soll_node|none`); persist via `axon_init_project.session_pointer` arg. Apply the pointed artefact before anything else (file path, Linear ticket URL, SOLL node, …). `active_handoff` is preserved as a backward-compat alias mirroring `session_pointer.value` only when kind=file. REQ-AXO-176: the four recent-activity arrays (`in_progress_requirements`, `wave_1_unblockers` from `soll_work_plan top=3`, `recent_req_commits` matching `REQ-XXX-NNN`, `recent_soll_writes` top-8 by `metadata.updated_at`) collapse what was previously 4 separate calls into the single init response — no Session entity type was added; CPT-AXO-027-style Concept summaries remain an opt-in pattern.

Without trigger phrase: `help()` → `status()` → `help(tool=X)` for schemas. `project_code` auto-resolved from cwd.

## Truth hierarchy
| Source | Authority |
|---|---|
| MCP `status` | runtime/protocol truth |
| `project_status` | compact project truth |
| `./scripts/axon-{live,dev} status` | local lifecycle probe only |
| README / docs | supporting context, not runtime truth |
| `instance_identity.data_root_absolute` | cross-check with `ls`/`du` (not `data_root` compact form) |
| `data.readiness` (tristate) + `data.subsystems[]` | per-subsystem state (REQ-AXO-098) |
| `data.availability.ist_projection_fresh` (bool) | freshness lag indicator; does NOT gate any tool |

## Runtime model
- live = stable truth (release `bin/`, ports 44127-44132, `.axon/`, `priority=critical gpu=preferred watcher=full`)
- dev = isolated dev (debug `.axon/cargo-target/debug/`, ports 44137-44142, `.axon-dev/`, `priority=best_effort gpu=avoid watcher=bounded`)
- brain = MCP authority + SOLL writer
- indexer = canonical IST writer
- dashboard = observation only — NEVER canonical for IST/MCP/SQL/release

## Tool routing
| Task | Tool |
|---|---|
| Find symbol (multi-token, underscore-aware REQ-AXO-088) | `query` — under `AXON_DB_BACKEND=postgres` (MIL-AXO-015 P6), Symbol semantic search, chunk-fallback search, and project-scope/degraded-file truth notes emit schema-qualified queries when a single project is supplied; pgvector `<=>` cosine distance powers Symbol semantic ANN; `project="*"` (or `project=None`) cross-schema search lands in P9 |
| Inspect detail (callers/callees survive synthetic target_id format via name-suffix join, REQ-AXO-134) | `inspect` |
| Evidence packet | `retrieve_context` — under `AXON_DB_BACKEND=postgres` (MIL-AXO-015 P4 slice 4d), semantic ANN reads ChunkEmbedding via pgvector `<=>` cosine distance against the per-project schema (CPT-AXO-039); pass `project` for the search to resolve a schema (cross-project semantic search lands in P9). Under DuckDB the existing DEC-AXO-073 L.3 `AXON_PARQUET_EMBEDDING_STORE_ENABLED` Parquet path is preserved. |
| Blast radius | `impact` |
| Why it exists | `why` |
| Source-sink flow | `path` |
| Structural risks | `anomalies` |
| Mutation safety | `change_safety` |
| Architecture map | `conception_view` |
| Compact runtime | `status mode=brief` |
| Project state | `project_status` |
| SOLL intent | `soll_query_context` |
| Raw graph SQL escape | `cypher` (after `schema_overview` / `list_labels_tables` / `query_examples`) |

Default `mode=brief`. `query` brain semantic search works under `brain_only` profile (REQ-AXO-128).

## Search recovery (server guidance is primary)
1. Follow `next_action` first
2. Follow `operator_guidance.follow_up_tools`
3. Apply `operator_guidance.llm_contract.{first|bad_args|partial|ask_user_only_if}`
4. Generic fallback only after server guidance exhausted

| Status returned | Recover via |
|---|---|
| `input_invalid` (missing_field) | retry with example call from `data.next_action` |
| `input_invalid` cypher binder (REQ-AXO-139 slice) | inspect `data.parameter_repair.{missing_column, available_columns, hint}` and retry with valid column. `available_columns` is clean of DuckDB `LINE N: ...` location markers |
| `rejected_all` / `partial` / `no_artifacts` from `soll_attach_evidence` (REQ-AXO-139 slice) | inspect `data.parameter_repair.{invalid_field, rejected_artifact_index, rejected_artifact_kind, primary_reason, accepted_aliases, required_field_hint, hint}`. For `invalid_field=artifact_type` also use `supplied_artifact_type` + `accepted_artifact_schema`. Per-kind `required_field_hint` covers File / Document / Symbol / Test / Metric / Validation / Rationale / Diff |
| `inspect` `symbol_found=false` (REQ-AXO-139 slice) | inspect `data.parameter_repair.{invalid_field, supplied_value, scope, suggestions, widening_actions, follow_up_tools, hint}`. With suggestions: pick one and retry `inspect`. Without: widen via `query` (less specific term) or `list_labels_tables` |
| `invalid_arguments` (any tool — REQ-AXO-139 slice) | inspect `data.parameter_repair.{tool, invalid_field, missing_required_fields, required_fields, supplied_arguments, input_schema, follow_up_tools, hint}`. `invalid_field` points at the first missing required field. Run `help(tool=<name>)` for the contract, fill the missing fields, retry the same tool. `data.status="input_invalid"` is also set for cross-slice consistency |
| `soll_apply_plan` unresolved logical_key in `relations[]` (REQ-AXO-139 slice) | inspect `data.errors[]` for `kind=unresolved_logical_key` rows + `data.parameter_repair.{invalid_field, operation_index, unresolved_keys, available_logical_keys, follow_up_tools, hint}`. Either reuse a `logical_key` declared as `kind=create\|update` earlier in the same `operations` batch, or pass a canonical `TYPE-CODE-NNN` id directly. Resolved/canonical link operations still apply; only unresolved ones are skipped |
| `restore_soll` / `soll_export` / `soll_validate` failure (REQ-AXO-147 slice) | inspect `data.parameter_repair.{invalid_field, step, entity_kind, supplied_value, hint, follow_up_tools}` + `data.diagnostic_excerpt`. For restore failures `step` ∈ {registry_seed, insert_node, insert_edge} and `entity_kind` ∈ {Vision, Pillar, Requirement, Decision, Concept, Milestone, Validation, Edge}. Unregistered `project_code` on `soll_export` now returns the canonical `wrong_project_scope` shape (matches `soll_validate` / `soll_query_context` / `soll_work_plan`) |
| `entrench_nuance` failure (REQ-AXO-147 slice) | inspect `data.parameter_repair.{invalid_field=target_ids, stage, expected_project_code?, supplied_target_ids?, invalid_target_ids?, ambiguity_warnings?, follow_up_tools, hint}`. Stages: `inference` / `input_validation` (empty target_ids) / `ambiguity_check` / `cross_project_check` / `baseline_snapshot` / `target_lookup` / `metadata_update` / `followup_snapshot`. For ambiguity → supply explicit `target_ids`. For cross-project → filter to `expected_project_code`. For empty target_ids → call `infer_soll_mutation` first |
| `soll_manager` failure (REQ-AXO-147 slice) | inspect `data.parameter_repair.{invalid_field, category?, supplied_value?, accepted_values?, supplied_source_id?, supplied_target_id?, follow_up_tools, hint}`. Categories: `entity` (unknown type → accepted_values list), `project_code` (unregistered → `project_registry_lookup` / `axon_init_project`), `relation_type` (forbidden link → `soll_relation_schema`), `data.title|description` (writer error — REQ-AXO-091 placeholder bug), `target_id` (target not found → `cypher`) |
| `axon_init_project` / `axon_apply_guidelines` / `axon_commit_work` failure (REQ-AXO-147 slice) | inspect `data.parameter_repair.{invalid_field, supplied_value, follow_up_tools, hint}`. `invalid_field` ∈ {project_path, project_code, git_environment}. For project_path issues call `help`; for project_code issues call `project_registry_lookup` then `axon_init_project`; for git failures call `axon_pre_flight_check` and `status` |
| `path` / `impact` / `simulate_mutation` / `semantic_clones` / `architectural_drift` / `change_safety` / `retrieve_context` / `why` / `soll_relation_schema` / `project_registry_lookup` / `soll_generate_docs` / `soll_rollback_revision` / `snapshot_diff` failure (REQ-AXO-147 slice) | inspect `data.parameter_repair.{invalid_field, supplied_value?, follow_up_tools, hint}`. `invalid_field` ∈ {symbol, source, sink, target, question, project_code, source_layer\|target_layer, site_root_dir\|output_dir, depth, revision_id, source_type\|target_type\|source_id\|target_id}. For symbol-based DX tools, follow_up via `query`/`inspect`. For project_code-based tools, follow_up via `project_registry_lookup` |
| `wrong_project_scope` | `project_registry_lookup` then `axon_init_project` |
| `input_not_found` | retry with suggested symbol or widen via `query` |
| `input_ambiguous` | pick exact symbol or narrow project scope |
| `degraded` | treat partial; retry after runtime stabilization |
| weak `why` | `retrieve_context` (tighter), then `inspect`, `query`, `impact`, `path`, `conception_view`, `project_status` |
| `diagnose_indexing` causes (REQ-AXO-212) | each cause renders machine-stable id + 1-line remediation: `watch_root_unconfigured`, `runtime_mode_excludes_indexing`, `path_not_in_runtime_registry`, `discovery_absent_or_filtered`, `file_too_large_for_budget`, `ingestion_not_completed`, `parser_extraction_gap`, `call_graph_gap`, `no_blocker_detected` — pick the remediation line and act |

NEVER inspect Axon source as recovery path for ordinary LLM operation.

## Why contract field priority
1. `authority_class` (governing | supporting | correlated)
2. `evidence_provenance`
3. `link_mode` (direct | inferred | weak_correlation — `weak_correlation` is NOT canonical)
4. `evidence_states`
5. `rationale_quality`

## SOLL types & relations
Types: Vision, Pillar, Requirement, Decision, Concept, Guideline (`GUI-` prefix REQ-AXO-092), Milestone, Validation, Stakeholder. Unknown entity → `data.status="input_invalid"` + `data.accepted_entities`.

Canonical relations (from `Edge.relation_type`):
| Pair | Relation |
|---|---|
| DEC → REQ | SOLVES |
| DEC → DEC | REFINES, SUPERSEDES |
| CPT → REQ | EXPLAINS |
| CPT → PIL | BELONGS_TO |
| REQ → PIL | BELONGS_TO |
| REQ → REQ | REFINES |

Always use `soll_relation_schema` before unfamiliar pairs. Forbidden pair → `error: forbidden_relation` (no `did_you_mean`). Cypher canonical SOLL row: `SELECT id, type, project_code, title, description, status, metadata FROM soll.main.Node WHERE …`. Filters on metadata via `json_extract_string(metadata, '$.priority')`.

## Vision rule
North Star, NOT technical. Format: `[Project] transforms [trapped/lost/expensive thing] into [accessible/durable/multiplied value] for [humans/teams/enterprises].` No technologies (those go in Decisions). Changes 1-2x/year.

## SOLL writes
| Tool | Use |
|---|---|
| `soll_apply_plan` | batch (`dry_run=true` first, `logical_key`, `author`); `relations` accept logical_keys AND canonical IDs — both resolve correctly during commit (REQ-AXO-137). `data.linked[]` exposes resolved canonical `source_id`/`target_id` plus `raw_source_id`/`raw_target_id` for audit |
| `soll_manager create/update` | exact single op |
| `soll_manager link` | post-batch relation creation on canonical IDs |
| `document_intent` (REQ-AXO-141) | discoverable entry point for "documente" / "document this" / "save observation" workflows — fresh LLM finds this in tools_catalog without per-client prompt config. `{intent, body, suggest_type?, tags?, project_code?}`; server-side classifier picks requirement (problem/gap/friction) / decision (choice/picks/we will) / concept (mental model) / guideline (rule/method) when `suggest_type` is omitted. Returns canonical SOLL id + `entity_type` + `classifier_reason`; follow up with `soll_manager(action=link)` to attach to a parent pillar/concept |
| `infer_soll_mutation` | read-only assistive scope check |
| `entrench_nuance` | bounded update of canonical entities |
| `soll_attach_evidence` | proof (file/test/metric/diff); `data.status` ∈ ok\|partial\|rejected_all\|no_artifacts |
| `soll_commit_revision` | atomic checkpoint per `preview_id` |

Async: response with `job_id` → `job_status(job_id)` until terminal (`succeeded` / `failed`). REQ-AXO-146: pass `wait: true` (with optional `timeout_ms` default 30000, `poll_interval_ms` default 250) to block until terminal in a single round-trip; on timeout the snapshot keeps `next_action.kind = continue_polling_until_terminal_state`. `data.wait_metadata` reports polls/elapsed_ms/timed_out/reached_terminal.

For `soll_work_plan`: `format=brief, limit, top` first; `include_validation_details=false` unless requirement-level detail needed. Terminal-state nodes (status ∈ `delivered`/`superseded`/`completed`/`archived`) are excluded from waves AND from descendant counting, so `unblocks N descendant(s)` reflects OPEN descendants only (REQ-AXO-135). Flip a closed item's status to mark it terminal — it disappears from wave 1 and stops inflating parent unblocker scores. Temporal score decay (REQ-AXO-144): `score *= exp(-age_days / half_life_days)` for nodes carrying `updated_at` metadata; defaults `include_decay=true`, `half_life_days=30`. `reasons[]` surfaces `decayed by age (factor X.XX)` when factor < 0.5. Disable with `include_decay=false` for benchmarking.

For `soll_verify_requirements`: a Requirement is **done** EITHER when status ∈ `completed`/`delivered` (terminal — strongest done signal, no metadata cross-check; REQ-AXO-136) OR when status ∈ `current`/`accepted` AND acceptance criteria exist AND supporting evidence is attached AND no broken file evidence remains. **partial** = some required dimensions exist; **missing** = mostly absent. Closing a REQ via `soll_manager update status=completed` immediately moves the `done` count by +1.

CLI bridge for large JSON: `./scripts/axon --instance live mcp-call call <tool> --args-file <file.json>` (or `--args-file -` for stdin). Avoid fragile inline shell JSON.

## Identity
Server-owned IDs `TYPE-CODE-NNN`. Never fabricate: project_code, preview, revision, SOLL IDs. `axon_init_project` returns `project_code` and `data.kickoff_bundle` (REQ-AXO-119) on first-init AND re-init. `data.path_exists_on_disk=false` → warning only, registration succeeds. Backend-agnostic: same contract under DuckDB or PostgreSQL deployment (DEC-AXO-075).

## Delivery flow
1. `status`
2. `query` / `inspect` / `retrieve_context`
3. `impact` / `why` / `path` / `anomalies` if needed
4. SOLL update if intent changed (REQ created/updated, evidence attached BEFORE code)
5. `axon_pre_flight_check` — add `incremental: true` (REQ-AXO-145) to validate each `diff_paths` entry individually; returns `data.per_file_violations` keyed by path so an LLM authoring N files sequentially can fix file 1 before authoring 2..N
6. **Pre-stage** `git add <Edit/Write modified files>` — `axon_commit_work` runs `git add <diff_paths>` itself and refuses partial-staging since REQ-AXO-138 (returns `data.git_add_exit_code` + `parameter_repair` if any path fails); pre-staging stays best practice for repos with conditional hooks
7. `axon_commit_work` — pass `project_path` (or `project_code`) for cross-project commits so git runs in the right tree (REQ-AXO-191)
8. **Verify** `git status` after — if `M` files remain, commit dropped modifications

UN FIX = UN COMMIT (~30-150 LOC + test + SKILL.md update if `tools_*.rs` changed). Do NOT use shell `git commit` for delivery.

## Release flow
Canonical for live: `bash scripts/release/promote_live_safe.sh --project AXO`. **Never** manual `cargo build --release` + copy. Manual recovery only if necessary: `release-preflight` → `create-release-manifest --state qualified` → `promote-live --manifest <m> --restart-live`. Fail closed if HEAD changes during release.

`promote-live-safe` proves: canonical rebuild, preflight, manifest, live restart, MCP runtime identity match, final qualify-mcp, final axon-live status.

## Qualification
- `./scripts/axon qualify --profile <smoke|demo|full|ingestion|retrieval> [--cold] [--tensorrt] [--build-tensorrt-from-tarball PATH]` — runtime (defaults to dev)
- `./scripts/axon qualify-mcp --surface <core|soll> --checks <quality,latency> --project AXO` — MCP surface

14 legacy verbs removed in DEC-AXO-060 (validate-mcp, measure-mcp, qualify-dev-cold, etc.) — folded into `qualify --profile ... --cold` etc. Do NOT reference them.

## Axon-issue 3-way triage (CPT-AXO-025) — every unexpected MCP result
| Branch | Trigger | Action |
|---|---|---|
| 1 — Hallucination | I assumed unverified column/type/param/behavior | Positive control + `schema_overview` / `list_labels_tables` + 3 reproductions changing one variable each. If any explains → drop, log nothing |
| 2 — Real Axon bug | Reproducible failure contradicts written contract (SKILL.md / SOLL DEC/REQ / tool description) | `soll_manager create requirement` tagged `axon-bug` `llm-contract`, evidence = exact reproductions + schema check + positive control |
| 3 — Commercial value-add | Works as documented but underperforms commercially (clarity / structured field / recovery hint / discoverability) | `soll_manager create requirement` tagged `axon-product-improvement` `commercial-value` `llm-friction`, framed as customer value (productivity gain, time saved, error avoided) |

Never log without picking a branch. REQ-AXO-129 is the cautionary anti-pattern (false bug claim that corrupted CPT-AXO-021).

## PDCA with SOLL (CPT-AXO-024 — hard rule, set 2026-05-02)
- **P**lan: research SOLL+IST first; create REQ/DEC BEFORE code; `soll_manager link` to PIL/CPT.
- **D**o: execute highest-score wave-1 from `soll_work_plan`; one fix one commit; `axon_pre_flight_check` then `axon_commit_work`.
- **C**heck: tests; query live MCP for status (don't trust conversation context — lossy on compaction); cross-check SOLL acceptance criteria.
- **A**ct: `soll_manager update` REQ status + commit SHA + evidence; `soll_validate` (target 0); `soll_work_plan` next.

## Hygiene
- TDD gate (GUI-PRO-001 / REQ-AXO-121): `.rs` containing `#[cfg(test)]` substring satisfies tests.rs requirement; sibling `_tests.rs` / `/tests/` still valid; files with neither remain blocked
- IST/SOLL test fixtures (REQ-AXO-142): use `crate::test_support::ist_fixtures::{SymbolFixture, CallFixture, SollNodeFixture, EdgeFixture, IstSeed, create_test_server_with_ist_seed}` for new IST/SOLL projection tests instead of inline `INSERT INTO Symbol/CALLS/...` SQL. `CallFixture::synthetic(file, name, project)` covers the REQ-AXO-134 `<file>::<name>` target_id form; canonical Symbol.id form via `CallFixture::canonical(...)`.
- soll.Edge INSERT hygiene (REQ-AXO-152): every `INSERT INTO soll.Edge` MUST populate `project_code` (5-column form `(source_id, target_id, relation_type, metadata, project_code)`). NULL `project_code` rows brick brain boot via DuckDB WAL replay / backfill PK conflict (observed 2026-05-03 promotion). Derive via `super::shared::project_code_from_canonical_entity_id(source_id).or_else(|| ...(target_id)).unwrap_or_else(|| "AXO".into())`, or pass an in-scope `p_code` directly when the caller already has it.
- `axon_commit_work` does NOT mean "stage everything" — only auto-stages git-rm
- archival `SOLL_EXPORT_*.md` not in routine commits (REQ-AXO-126 — `soll_export` is snapshot-per-release, fired by `promote_live_safe.sh`)
- Derived docs (`soll_generate_docs`, `docs/derived/soll/...`) are read surfaces, NOT canonical truth — don't restore from them
- Canonical truth = live SOLL + `soll_export`
- `status mode=brief` no longer inlines `public_tools` catalog — count + pointer (REQ-AXO-104); `data.public_tools` still always-on for machines
- `status mode=brief` text rendering surfaces `Trust boundary:` + `Next best action:` lines (REQ-AXO-042) so an LLM reading the markdown can act without parsing raw `data.truth_cockpit`. When `degraded_notes` are present, a `Current blocker:` line follows. Always inspect these three lines first when `truth_status != canonical` — they name the exact recovery tool.
- Multi-tenant query scoping (REQ-AXO-066 Phase 1 / DEC-AXO-064 Option A): use `scoped_query_filter(project_code, prefix)` from `tools_soll/shared.rs` for the canonical `AND <prefix>project_code = '<code>'` fragment. Composite `(project_code, key)` indexes are present on hot IST tables (CALLS / CALLS_NIF / CONTAINS / IMPACTS / SUBSTANTIATES / Symbol / File) and SOLL tables (Node / Edge / McpJob / Revision / RevisionChange). `soll.Edge`, `soll.McpJob`, `soll.Revision`, `soll.RevisionChange` carry a denormalized `project_code` column (backfilled to `'AXO'` on first boot for legacy rows; edges inherit from source Node when known)

## Architecture-state CPTs (load these at session start for perf work)
Each CPT below names a load-bearing structural fact discovered the hard way. Fetch via `cypher SELECT description FROM soll.main.Node WHERE id='<CPT-ID>'` (IDs assigned when entries are created in SOLL).

| CPT | Anchor | When to load |
|---|---|---|
| Single writer mutex serialization | `graph.rs:33` | Any perf or contention work |
| DuckDB plugin layer (FFI) | `axon-plugin-duckdb/src/lib.rs` | Extending DB capabilities (Appender, Parquet, etc.) |
| Writer Actor batching pattern | `worker.rs:296` | Throughput / pipeline work |
| Memgraph publication contract | `bin_memgraph_publication.rs` | Visualization or publication |
| Env var 2-layer propagation | `axon-instance.sh` + `start.sh` | Adding new `AXON_*` env knobs |
| DuckDB workload boundaries | empirical (VAL-AXO-034/036) | Any storage or write-heavy redesign |

## Performance investigation playbook
1. **Instrument first** — add per-stage `Instant::now()` to `vector_worker_loop` + `writer_actor`; emit timings via sidecar trace files (pattern: `writer_actor_trace()` / `vector_trace()`). Bypass tracing subscriber so signal stays clean under noisy `RUST_LOG`.
2. **Run 90s diagnostic probes** with fresh `.axon-dev/graph_v2`. Short windows surface upstream-feed stalls vs sink stalls; long windows hide them.
3. **Falsify hypotheses cheaply** — single-line env-flag toggles before refactors. Example: `AXON_DIAG_SKIP_CHUNKEMBED` isolated 70-75% of the bottleneck in 5 minutes; a cache refactor would have taken days to falsify the same hypothesis.
4. **Capture VAL even when hypothesis fails** — failed VALs (e.g. VAL-AXO-033 rejecting REQ-AXO-187) prevent re-treading dead ends. Link `VERIFIES`/`REJECTS` to the parent REQ/DEC with the measurement CSV as evidence.

## Sub-agent delegation rules (Axon-specific)
| Mode | Rule |
|---|---|
| ALLOWED | shell exec (probe scripts, `cargo build`/`cargo test`), doc writing, MCP-only tasks (`soll_manager` / `soll_apply_plan` calls) |
| FORBIDDEN | code exploration that would force IST reconstruction — sub-agents have no MCP, so they re-read source and burn 100-200K tokens |
| PERMISSION CAVEAT | sub-agents can be denied `axon-dev start` permissions. Always verify by starting long-running services from the parent shell. Pattern: parent runs `./scripts/axon-dev start ...`; sub-agent runs `sleep N; capture heartbeat snapshots; ./scripts/axon-dev stop` |

## Maintenance
Update this skill when: tool names change, surface visibility changes, runtime authority changes, SOLL workflow/schema changes, qualification or release protocol changes. Concept canonicals (CPT-AXO-018/019/020/021/024/025) live in SOLL — `soll_manager(action=update)` them, never copy-paste.
