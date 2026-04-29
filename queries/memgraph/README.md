# Memgraph Human IST/SOLL Query Pack

This query pack is for human graph visualization only.

LLM clients must use Axon MCP as the source of truth. Memgraph is a disposable projection built from versioned Parquet publications.

## Publication Contract

Generate the current graph-shaped Parquet publication with:

```bash
./scripts/axon publish-memgraph
```

The standard publication covers all projects. Use `--project-only --project-code AXO` only for a diagnostic narrow export.

The publication directory contains:

- `nodes.parquet`
- `edges.parquet`
- `manifest.json`

The initial import/promotion path must validate counts before exposing the Memgraph endpoint.

Validate the prepared query pack against the active Memgraph runtime with:

```bash
./scripts/axon memgraph smoke-queries
```

## Queries

- `overview.cypher`: compact node/edge inventory.
- `soll_decisions.cypher`: current SOLL decision map.
- `requirement_coverage.cypher`: requirement to decision/validation/evidence coverage.
- `ist_soll_traceability.cypher`: SOLL intent to supporting evidence paths.
- `hot_files.cypher`: graph-ready/vector-ready file overview for human inspection.
