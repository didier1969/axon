# Axon Dashboard

The dashboard is the read-only operator surface for the Rust runtime.

It does not own ingestion, scheduling, mutation, or recovery logic.
It reads canonical truth from the SQL gateway and runtime telemetry from the Rust bridge.

## What the Dashboard Shows

- File lifecycle truth from `File`
- Structural readiness from `graph_ready`
- Derived semantic coverage from chunk embedding completeness
- SOLL alignment summaries
- Runtime pressure, queue depth, memory, and ingress behavior

## Primary Operator Path

The primary product path is:

1. file discovery
2. structural indexing
3. graph readiness
4. semantic coverage
5. SOLL alignment

`Graph Embeddings` remain visible only as an advanced secondary signal.
They are not a primary KPI.

## Lifecycle Semantics

The dashboard treats these `File.status` values as terminal for operator progress:

- `indexed`
- `indexed_degraded`
- `skipped`
- `deleted`
- `oversized_for_current_budget`

Active backlog is restricted to non-terminal work:

- `pending`
- `indexing`

`Files With Semantic Coverage (Derived)` is not the raw `File.vector_ready` flag.
It is derived from graph-ready files whose current chunks no longer have missing embeddings for the active chunk model.

## Development

- `mix setup`
- `mix test`
- `mix phx.server`

The default local endpoint is `http://localhost:4000`.
