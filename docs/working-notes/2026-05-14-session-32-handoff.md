# Session 32 hand-off — REQ-AXO-345 cascading bug fix supersedes REQ-AXO-329

**Date** : 2026-05-14 ~18:50 UTC
**Branch** : `main` HEAD `90ec32da`
**Live build** : `v0.8.0-444-g90ec32da` install_gen `live-20260514T183604Z`
**Canonical session_pointer** : `CPT-AXO-052`

---

## TL;DR

Session 30 reported "A1 admission stalled" (REQ-AXO-329, HIGH P0) — indexer alive but no new ingestion, coverage stuck at 7 045 / ~16 000 expected. Session 32 root-caused it as a **3-bug cascade** from incomplete v1→v2 migration and shipped the fix in `90ec32da` (REQ-AXO-345).

Live observation 20 min post-promote :
- `public.file` table : 0 → 4 764 rows
- `symbol` : 102 639 → 147 760 (+45 121)
- 16 → 20 projects with symbol rows ; late-alphabet projects newly indexed include zeroclaw (6 518 symbols), triolingo (10 062), nanobot-loop (2 399).

---

## Diagnostic trail (REQ chain)

| REQ | Role | Status |
|---|---|---|
| `REQ-AXO-331` | Watcher INFO log spam (784 MB/day) downgraded | delivered `defe0871` |
| `REQ-AXO-332` | Indexer file-log sink removed (stdout-only) | delivered `dc94d845` |
| `REQ-AXO-340` | Scope reconciliation orchestrator activated CPT-AXO-054 walk | delivered `69e40619` — revealed symptom |
| `REQ-AXO-344` | Drain / A3 flush / A3 upsert INFO traces | delivered `ea46f538` — narrowed to A1/A2/A3 |
| `REQ-AXO-345` | A1/A2 in/out traces + drain FIFO + file-table fix | delivered `90ec32da` — closed cascade |
| `REQ-AXO-329` | Session 30 carry-over | superseded by REQ-AXO-345 |

The instrumentation REQs (`REQ-AXO-344` `REQ-AXO-345` traces) **remain live** at INFO until the next promote — high log volume. Downgrade to DEBUG queued as `REQ-AXO-346` cleanup for next session.

---

## Root cause (canonical)

1. **`public.file` table empty in axon_live.** Pipeline v2 (`upsert_graph_v2_batch`) wrote IndexedFile / Symbol / Chunk / Edge but never the legacy `file` lifecycle table. Half-completed migration from REQ-AXO-289.
2. **`FileIngressGuard.hydrate_from_store` ineffective.** Hydrates from `SELECT path, status, mtime, size FROM File` → 0 rows → `should_stage` returns `StageNew` unconditionally for every path → Scanner re-pushes the entire universe on every reconciliation pass.
3. **`ingress_buffer::compare_buffered` starvation.** Sorted `(priority DESC, path ASC)`. Combined with #2 keeping the buffer perpetually saturated with scan-priority entries, late-alphabet projects (`n` `o` `r` `s` `t` `z`) sat at the sort-queue tail forever while Scanner refilled the head with capital-letter and `a-f` projects.

The 16 projects that **did** index pre-fix were all early-alphabet : `APS` `AXO` `CCL` `CTX` `DOC` `ERP` `EXA` `FLA` `FSF` `MFL` `OLL` `OPT` `RAL` `SWX` `TE2` `audit_db`. The 9 that didn't : `NBL` `NEX` `ODM` `RMC` `SOK` `SVZ` `TRD` `TRI` `ZCL` — all `n-z`.

---

## Fix shipped (commit `90ec32da`)

### Drain FIFO (`ingress_buffer.rs`)

```rust
enum BufferedIngress {
    File { event: IngressFileEvent, seq: u64 },
    Tombstone { path: String, source: IngressSource, seq: u64 },
}

struct IngressBuffer {
    // …
    next_seq: u64,
}

fn compare_buffered(left, right) -> Ordering {
    buffered_priority(right).cmp(&buffered_priority(left))
        .then_with(|| buffered_seq(left).cmp(&buffered_seq(right)))  // FIFO tiebreak
}
```

Every project gets equal drain turn regardless of alphabetical position.

### File-table population (`graph_ingestion.rs::upsert_graph_v2_batch`)

```sql
INSERT INTO file
  (path, project_code, status, size, mtime, graph_ready, last_state_change_at_ms)
VALUES …
ON CONFLICT (path) DO UPDATE SET
  project_code = EXCLUDED.project_code,
  status = 'committed',
  size = EXCLUDED.size,
  mtime = EXCLUDED.mtime,
  graph_ready = true,
  last_state_change_at_ms = EXCLUDED.last_state_change_at_ms;
```

Restores `FileIngressGuard.hydrate_from_store` on next boot → Scanner stops re-pushing already-indexed files.

---

## Process state at hand-off

### Live (axon-live)
- **Brain** pid 95213 (supervisor 94183) — build `v0.8.0-444-g90ec32da` install gen `live-20260514T183604Z` — HEALTHY
- **Indexer** running on the instrumented build — INFO traces from REQ-AXO-344/345 will inflate stdout volume until the cleanup REQ ships
- **PG** UP on 127.0.0.1:44144
- **MCP** UP on http://127.0.0.1:44129/mcp

### Branches
- `main` HEAD `90ec32da` — committed, **not pushed** (operator-controlled push step)
- Pending manifest cleared after promote

### IST coverage live (20 min post-promote)
- `file` 4 764
- `indexedfile` 9 632
- `symbol` 147 760
- 20 / 24 federation-eligible projects active in `symbol` ; 4 still draining (NEX / SOK / SVZ / TRD)

---

## Next-session entry point

### Cold-start reading order

1. This file
2. `sql SELECT description FROM soll.node WHERE id='CPT-AXO-052'` (session pointer)
3. `sql SELECT description FROM soll.node WHERE id='REQ-AXO-329'` (root-cause canonical record, now `delivered`)
4. `sql SELECT description FROM soll.node WHERE id='REQ-AXO-345'` (the cascade fix)
5. `mcp__axon__status mode=brief` (runtime truth)
6. `mcp__axon__embedding_status` (coverage state)

### Immediate next actions

1. **Verify pipeline drain completed** : `SELECT project_code, COUNT(*) FROM symbol WHERE project_code IN ('NEX','SOK','SVZ','TRD') GROUP BY 1`. If still 0 after 1 h post-handoff, log new REQ for deeper investigation.
2. **Log REQ-AXO-346 cleanup** : downgrade A1/A2/A3 `info!` traces (`pipeline_v2::a1`, `pipeline_v2::a2`, `pipeline_v2::a3`, `pipeline_v2::drain` targets) to `debug!`. Production log volume otherwise excessive at steady state (~36 files/sec × 4 traces = 144 lines/sec sustained).
3. **REQ-AXO-323** UPSERT bug fix (P2 outstanding from previous session).

---

## Open backlog

- **REQ-AXO-323** silent UPSERT data-loss `soll_manager.create` — mitigation `soll_apply_plan dry_run=true` for batch (pending).
- **soll_validate orphans** : 17 REQ orphan + 25 missing-criteria. Several pre-date session 32. Schedule `/curate-soll` pass.

---

## Tags

`session-32-handoff`, `req-axo-345-cascade-fix`, `req-axo-329-superseded`, `drain-fifo`, `file-table-population`, `v1-v2-migration-residual`
