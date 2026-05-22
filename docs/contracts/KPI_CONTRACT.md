# KPI canonical contract — `status` tool

REQ-AXO-901657 Slice 6 — formalize the contract exposed by the `status`
MCP tool after the legacy v1 worker pool retirement (REQ-AXO-901653
slice-5c/d). Pipeline_v2 is the canonical ingestion path
(REQ-AXO-289 / CPT-AXO-054 / MIL-AXO-017).

## Modes

| Mode | Use | Token budget | Surface |
|---|---|---|---|
| `status mode="brief"` | LLM bootstrap, every turn | ≤ 800 tokens | `runtime_mode`, `runtime_profile`, `instance_kind`, `runtime_identity`, `ist_projection_freshness`, `trust_boundary`, `next_best_action`, `current_blocker`, `public_tools_count` |
| `status mode="verbose"` | Deep diagnostic | ≤ 6000 tokens | brief + `runtime_telemetry`, `embedding_contract`, `runtime_authority`, `staleness`, `embedder_provider`, full tool list |

The default is `brief`. Verbose payload is never auto-returned to the LLM
unless the caller explicitly requested it.

## Canonical fields post slice-5c/d

### File presence

| Field | Source | Pipeline_v2 semantic |
|---|---|---|
| `indexed_file_count` | `SELECT count(*) FROM public.IndexedFile` | All files the watcher has seen + ingested |
| `chunk_count` | `SELECT count(*) FROM public.Chunk` | Symbols / lifted blocks emitted by code_chunker |
| `chunk_embedding_count` | `SELECT count(*) FROM public.ChunkEmbedding` | Vector-ready chunks |
| `graph_ready_file_count` | `SELECT count(DISTINCT file_path) FROM public.Chunk` | Files whose Symbols are extracted |
| `vector_ready_file_count` | `SELECT count(DISTINCT c.file_path) FROM public.Chunk c JOIN public.ChunkEmbedding e ON e.chunk_id = c.id` | Files whose Chunks are embedded |

### Status enum (retired)

The legacy `public.File.status` enum (`pending`, `indexing`,
`indexed`, `indexed_degraded`, `skipped`, `deleted`,
`oversized_for_current_budget`) is **gone**. Pipeline_v2 has no per-file
status row : writes are in-line, failures surface via tracing logs +
`runtime_truth_feed`. The KPI surface reports `0` for every legacy
status counter — calling code must not pivot on these.

### Backlog

| Field | Pipeline_v2 source |
|---|---|
| `persisted_file_pending_depth` | always `0` (no on-disk pending queue) |
| `graph_projection_queue_depth` | always `0` (table dropped) |
| `file_vectorization_queue_depth` | always `0` (table dropped) |
| `orphan_vectorization_files` | always `0` |
| `stale_vector_inflight_files` | always `0` |
| `oldest_graph_pending_age_ms` | always `0` |
| `oldest_semantic_pending_age_ms` | always `0` |

Real back-pressure surface is **`runtime_truth_feed`** + per-stage
`items_in / items_out / err / bp` counters from pipeline_v2 stages
(A1/A2/A3/B1/B2/B3). Those are emitted via tracing and surfaced by
`bench-pipeline-v2`, not the status tool.

### Freshness

| Field | Source | Meaning |
|---|---|---|
| `ist_projection_freshness` | `runtime_authority::reader_snapshot_freshness_contract` | `fresh` (indexer publishing) / `degraded` (brain-only or stale reader) |
| `trust_boundary` | derived | `canonical` (freshness=fresh) / `degraded` (stale or indexer absent) |
| `staleness.last_publish_ts_ms` | `MAX(last_seen_ms) FROM public.IndexedFile` | Latest pipeline_v2 ingestion timestamp |

CPT-AXO-029 documents the IST freshness gate. Brain-only mode = degraded
by construction (no live indexer).

## Stability guarantees

- Field **names** in `data.runtime_version`, `data.runtime_mode`,
  `data.public_tools_count` are stable. Removing them is a major version
  bump.
- Field **values** within `data.runtime_telemetry` may shift when a
  pipeline stage is added or removed. Consumers must tolerate unknown keys.
- Tool count fluctuates as MCP surface evolves ; consumers may use it for
  bootstrap probes but not for assertions.

## Migration notes

Consumers reading `status` output before MIL-AXO-017 / REQ-AXO-289
expected `File`-status counters > 0 during indexing. Post-pipeline_v2,
those counters are always `0`. Migrate to :

- For "is indexing running" → check `runtime_truth_feed`.
- For "how many files indexed" → `indexed_file_count`.
- For "how many failed" → grep tracing logs (no on-disk persistence).

## Validation gates

- `mcp__axon__truth_check` covers the IndexedFile / Symbol / Chunk
  invariants. CALLS / CONTAINS / CALLS_NIF surfaces are AGE-canonical
  (post Stop A) but still queryable via SQL on legacy projects.
- `mcp__axon__anomalies` flags structural drift (god objects, cycles,
  orphans) — independent of the KPI contract.

## References

| Concept | Doc |
|---|---|
| Streaming pipeline v2 | CPT-AXO-054 / REQ-AXO-289 |
| PG canonical migration | MIL-AXO-017 / DEC-AXO-083 |
| Status tool routing | CLAUDE.md "Tool Routing" |
| KPI lean umbrella | REQ-AXO-901657 |
| Legacy purge umbrella | REQ-AXO-901653 |
