# Axon V2 — Execution Plan: Indexing Reliability + Inter-Language Call Graph

Date: 2026-04-03
Status: proposed

## Goal

Make Axon “Day-1 reliable” on new repositories, improve impact accuracy across language boundaries, and reduce operator friction.

## Success Criteria (Global)

1. A project with `0 indexed files` always returns a structured root-cause report in < 5s.
2. `axon_impact` coverage increases measurably on Rust/Elixir bridges (FFI/NIF).
3. Cypher tooling is self-discoverable without manual schema knowledge.
4. Dashboard/MCP/reporting stay on one canonical truth path (no split values).

## Phase 1 — Day-1 Indexing Diagnostic (P0)

Acceptance:
1. `health/audit` never stop at “Found 0 files” without cause details.
2. Cause categories are explicit: watch root mismatch, ignore filters, unsupported parser, parse failure, permission, empty repo.

Tasks:
1. Add `diagnose_project_indexing(project_slug)` in core.
2. Return structured diagnostics in `health`, `audit`, and `debug`.
3. Add remediation hints per cause.
4. Add MCP tool `diagnose_indexing` (read-only).

Deliverables:
1. Root-cause payload schema.
2. Human-readable summary in MCP responses.

## Phase 2 — Inter-Language Call Graph Hardening (P0)

Acceptance:
1. `impact` on known cross-language symbols returns non-empty edges when calls exist.
2. Explicit confidence/coverage is reported.

Tasks:
1. Normalize symbol identity across Rust/Elixir boundaries.
2. Add bridge edge materialization pipeline (`CALLS_NIF`/FFI aliases).
3. Add fallback path traces when direct call edge is missing.
4. Expose `impact` confidence fields.

Deliverables:
1. Coverage counters: direct_edges, bridged_edges, inferred_edges.
2. Regression dataset for BookingSystem + Fiscaly bridge cases.

## Phase 3 — Cypher/SQL Discoverability (P1)

Acceptance:
1. A user can discover schema and run meaningful first queries without docs.
2. Empty query results are explained (scope empty vs query mismatch).

Tasks:
1. Add MCP tools:
   1. `schema_overview`
   2. `list_labels_tables`
   3. `query_examples`
2. Add query guardrails/hints for common mistakes.
3. Add “project scope quick-start” examples.

Deliverables:
1. Minimal introspection API.
2. Canonical starter query set.

## Phase 4 — Canonical Truth Contract (P1)

Acceptance:
1. Dashboard, SQL gateway, and `debug` agree on key counters in the same sampling window.
2. Reader snapshot age and refresh failures are visible.

Tasks:
1. Keep counters on writer-truth path for critical KPIs.
2. Keep reader refresh periodic and configurable.
3. Surface `reader_snapshot_age_ms` and refresh failure totals in MCP + cockpit.

Deliverables:
1. Truth contract doc.
2. Drift alert thresholds.

## Phase 5 — Qualification and Release Gate (P0)

Acceptance:
1. No release if Day-1 diagnostics or cross-language impact regress.
2. Qualification report produced automatically.

Tasks:
1. Add qualification script sections:
   1. Day-1 indexing diagnosis
   2. Impact bridge coverage
   3. Truth consistency checks
2. Add release gate in CI for those checks.

Deliverables:
1. Machine-readable qualification JSON.
2. Human report summary.

## KPIs

1. `day1_zero_index_unknown_causes` (target: 0)
2. `impact_cross_lang_non_empty_rate` (target: +30% vs baseline)
3. `truth_drift_events_per_hour` (target: 0)
4. `first_successful_query_time_seconds` (target: < 60s)

## Execution Order

1. Phase 1
2. Phase 2
3. Phase 4
4. Phase 3
5. Phase 5
