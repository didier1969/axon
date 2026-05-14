# Session 33 hand-off — REQ-AXO-346 soll_work_plan petgraph refactor

**Date** : 2026-05-14 ~19:55 UTC
**Branch** : `main` HEAD `729876d8`
**Live build** : `v0.8.0-447-g729876d8` install_gen `live-20260514T195552Z`
**Canonical session_pointer** : `CPT-AXO-052`

---

## TL;DR

CPT-AXO-019 step 4 says `soll_work_plan` score is authoritative for prioritization. Audit found 3 of top 4 Wave-1 sources were `rejected` Decisions with inflated `unblocks N` scores. Two root causes : (1) `is_terminal_status` didn't recognize `rejected` ; (2) the hot path rebuilt adjacency / cycle detector / BFS / Kahn by hand even though `SollSnapshot` already shipped a `petgraph::Graph` (REQ-AXO-322).

REQ-AXO-346 closes both. Wave 1 cleaned ; top now = `DEC-AXO-083` (canonical AGE retirement, unblocking REQ-AXO-299 + REQ-AXO-300). Next session can trust the score directly.

---

## Operator-driven discovery

Three operator interventions shaped the work :

1. "Quel est le concept du work_plan ?" → I gave a "you-don't-need-it-trust-me" answer that subtly overrode the canonical `score` ordering. Operator pushed back : "il me semblait avoir donné l'ordre… créer automatiquement sur la base de SOLL un plan de travail avec priorisation des tâches. Recherche ce concept ?" → I retrieved `CPT-AXO-019` which states explicitly **"Score is authoritative for ordering — do not override with personal judgment unless the user explicitly directs."** I had violated this step 4 in my previous response.

2. "Pourquoi est-ce que cette commande n'utilise pas notre librairie Graph ?" → My first refactor attempt built a fresh `petgraph::Graph<String, ()>` per call from `load_work_plan_edges` output. Operator caught the duplication : "Confirme-moi que tu interroges bien le graphe existant et que tu ne crées pas ton propre graphe." → I rolled back and refactored to consume `SollSnapshot::graph()` directly with `petgraph::visit::EdgeFiltered`.

3. "Lance un sous-agent pour faire valider ton travail. Ne lui indique que le concept" → independent review caught a 70-line `build_waves` dead helper I had missed. Removed in follow-up commit.

---

## Diagnostic trail

| REQ | Role | Status |
|---|---|---|
| `REQ-AXO-346` | Bug 1+2+3 fix : terminal-status `rejected` + 7 hand-rolled helpers deleted + petgraph-only consumption | delivered `6fd03e12` + `729876d8` |

Cited canonical references : `CPT-AXO-019` (5-step protocol), `REQ-AXO-322` / `DEC-AXO-091` (SollSnapshot petgraph), `REQ-AXO-135` (is_terminal_status helper), `DEC-PRO-100` (canonical status vocabulary).

---

## Fix shipped (2 commits)

### Commit `6fd03e12` — main refactor

`src/axon-core/src/mcp/tools_soll/inference.rs`

- `is_terminal_status` extended : `delivered | superseded | completed | archived | rejected` (was missing `rejected`).
- 3 unit tests : `rejected_status_is_terminal`, `delivered_superseded_completed_archived_are_terminal`, `active_statuses_are_not_terminal`.
- Deleted : `build_adjacency_map`, `detect_cycle_sets` (recursive Tarjan), `collect_blocked_by_cycles`, `filter_adjacency`, `compute_descendant_counts`.

`src/axon-core/src/mcp/tools_soll/planning_work_plan.rs`

- Deleted `load_work_plan_edges` (no longer needed — snapshot edges queried inline via `edges_directed` + filter on `weight()`).
- New helpers operate on `&SollSnapshot` only :
  - `cycle_sets_snapshot(&snapshot)` → `petgraph::algo::tarjan_scc` on `EdgeFiltered::from_fn(snapshot.graph(), |e| is_work_plan_relation(e.weight().as_str()))`
  - `blocked_by_cycles_snapshot(&snapshot, cycle_node_ids)` → BFS via `snapshot.graph().edges_directed(...)`
  - `descendant_counts_snapshot(&snapshot, allowed)` → per-source BFS via `snapshot.graph().edges_directed(...)`
  - `build_waves_snapshot(&nodes, &snapshot, schedulable)` → Kahn's algorithm via `snapshot.graph().edges_directed(...)`
- `axon_soll_work_plan` body : 8 lines of orchestration consuming the snapshot directly. Zero `Graph::new` / `add_node` / `add_edge` in `tools_soll/`.

### Commit `729876d8` — follow-up

Subagent review flagged `build_waves` (inference.rs:214-286) as surviving dead code. Removed.

---

## Wave 1 verification (live, post-promote)

Before (REQ-AXO-345 build `v0.8.0-444`) — top 4 :
```
DEC-AXO-084  rejected   score=278  unblocks 7  ← all 7 SOLVES targets rejected
DEC-AXO-077  rejected   score=129  unblocks 3  ← all 3 SOLVES targets rejected
DEC-AXO-078  rejected   score=89   unblocks 2
DEC-AXO-083  current    score=89   unblocks 2
```

After (REQ-AXO-346 build `v0.8.0-447`) — top 4 :
```
DEC-AXO-083  current    score=89   unblocks REQ-AXO-299 + REQ-AXO-300  (canonical AGE retirement)
DEC-AXO-079  current    score=81   unblocks 2
DEC-AXO-003  current    score=79   unblocks 2
DEC-AXO-031  current    score=79   unblocks 2
```

CPT-AXO-019 step 4 contract restored. Next session can trust the score.

---

## Sub-agent validation (independent)

Spawned a general-purpose subagent with **concept only** (per operator directive). PASS on 5/5 checks :
- No legacy symbols anywhere (`build_adjacency_map`, `detect_cycle_sets`, …, `build_waves`, `load_work_plan_edges`).
- No `HashMap<String, BTreeSet<String>>` adjacency in `src/axon-core/src/`.
- `tarjan_scc` called once, `EdgeFiltered` used for SOLVES+BELONGS_TO scope, `edges_directed(...)` for BFS/Kahn.
- `is_terminal_status` matches 5 canonical states.
- Zero `Graph::new` / `add_node` / `add_edge` in `tools_soll/`.

---

## Process state at hand-off

- **Live** brain + indexer running `v0.8.0-447-g729876d8` ; qualify-mcp verdict=warn on quality (pre-existing, latency ok).
- **PG** UP on 127.0.0.1:44144.
- **MCP** UP on http://127.0.0.1:44129/mcp.
- `main` HEAD `729876d8` — committed, **not pushed** (operator-controlled).

---

## Next-session entry point

### Cold-start reading order

1. This file
2. `sql SELECT description FROM soll.node WHERE id='CPT-AXO-052'` (session pointer)
3. `sql SELECT description FROM soll.node WHERE id='REQ-AXO-346'` (refactor canonical record)
4. `mcp__axon__status mode=brief` (runtime truth)
5. `mcp__axon__soll_work_plan project_code=AXO format=brief top=10` (work plan now operator-trustworthy)

### Immediate next actions (per restored work_plan ordering)

1. **REQ-AXO-299** — MIL-AXO-017 slice 5 : bascule 6 MCP tools (impact/path/why/anomalies/retrieve_context + 1) off AGE Cypher onto SQL function library. `graph_analytics.rs` 12 `skip_sql_relations()` branches collapse to a single SQL helper.
2. **REQ-AXO-300** — MIL-AXO-017 slice 6 : `cypher`→`sql` MCP tool rename + AGE writes retirement (`emit_age` deletion + `pg_extension WHERE extname='age'` drop).
3. **REQ-AXO-323** — CRITICAL : silent UPSERT data-loss in `soll_manager.create` (mitigation `soll_apply_plan dry_run=true` for batches ; full fix pending).

---

## Tags

`session-33-handoff`, `req-axo-346-petgraph-refactor`, `cpt-axo-019-compliance`, `wave1-cleaned`, `subagent-validated`
