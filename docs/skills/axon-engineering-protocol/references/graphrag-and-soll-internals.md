# GraphRAG tools + SOLL snapshot internals

Load only when working on : GraphRAG retrieval tuning, IST snapshot cache, `soll_work_plan` scoring, `ist_*` MCP tools, RRF fusion parameters, or when an MCP call returns a field referenced below.

## SOLL writes — async + snapshot semantics

- Async tools : `job_id` → `job_status` ; `wait=true` blocks the call until done.
- `soll_work_plan` excludes terminal-status nodes (`delivered` / `superseded` / `rejected` per DEC-PRO-100) from Wave 1 and from `unblocks N`.
- `unblocks N` counts the 6 canonical filiation relations : SOLVES / BELONGS_TO / TARGETS / REFINES / EXPLAINS / VERIFIES (REQ-AXO-91500 patch A).
- Cycle detection + Kahn waves use the narrower SOLVES + BELONGS_TO scope.
- Both consume the in-process `SollSnapshot` petgraph (REQ-AXO-135 + REQ-AXO-346) — not direct PG reads.
- `soll_acyclic_audit project_code=<P>` enumerates SCC>1 and self-loops in the SOLL graph (pre-requisite to DEC-AXO-098 cycle validator activation).

## IST snapshot cache (post MIL-AXO-019)

- `ist_snapshot_warm project_code=<P>` cold-loads the CSR snapshot into the process cache.
- With `AXON_IST_RAM_ENABLED=1` (default on) , migrated call-sites (`get_circular_dependency_count_fast`, `collect_structural_neighbors`) dispatch to RAM (sub-µs neighbor lookup) with silent PG fallback on cache miss.
- PG triggers on `public.symbol` / `public.edge` fire `pg_notify('ist_mutated', json)` ; the listener evicts the affected project from the cache with a 50 ms coalescing window.
- Petgraph-backed tools (require `ist_snapshot_warm` first) :
  - `ist_centrality_pagerank top=N`
  - `ist_structural_sccs`
  - `ist_shortest_path from=<id> to=<id>`

## RRF fusion (REQ-AXO-91489)

`mcp::tools_context::rrf_fusion::rrf_fuse(inputs, k=60, alpha, require_reachable, top)` implements Reciprocal Rank Fusion (Cormack 2009) across vector / FTS / graph rankings. Optional PageRank centrality boost : score `× (1 + α × pagerank_norm)`. `require_reachable` filters routes for Impact/Wiring queries.

## `status` call-graph coverage (REQ-AXO-91484)

`status mode=verbose|full` surfaces `data.ist_call_graph_coverage` :

```
{
  per_project: {
    <code>: {
      <lang>: { fns, outgoing_calls, coverage_ratio }
    }
  },
  alerts: ["<proj>:<lang>:zero_outgoing_calls"]
}
```

`lang ∈ {rust, python, elixir, elixir_script, typescript, tsx}`. Alert fires when `fns > 100 ∧ outgoing_calls = 0`.

## Architecture notes (background context)

- IDs : `TYPE-PROJ-N` per DEC-AXO-085. Regex `^[A-Z]{3}-[A-Z][A-Z0-9]{2}-[0-9]{3,}$`.
- SQL tool : renamed from `cypher` per MIL-AXO-017 (AGE retirement). All Cypher syntax dropped, PG-only backend.
- Graph traversal tools (`impact`, `path`, `why`, `anomalies`, `retrieve_context`, `retrieve_context_v2`) are thin wrappers on `db/ddl/04_graph_functions.sql` (`public.impact` / `public.callers_of` / `public.why_chain` / `public.blast_radius` / `public.path` / `public.retrieve_context_v2`) over `public.Edge`.
- `relation_type` values UPPERCASE : `CALLS` / `CALLS_NIF` / `CONTAINS`, written by `stage_a3.rs`.
- `documente` / `save observation` → `document_intent` (REQ-AXO-141 auto-classifier), never `soll_manager` directly.
