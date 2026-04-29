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

The default smoke mode runs compact `EXPLAIN` checks for every top-level and `catalog/` query. Use `--mode execute` only when full result execution is intentionally needed.

The generated import drops and recreates navigation indexes for common human lookup paths, so repeated publication loads do not accumulate duplicate index entries:

- `AxonNode`: `id`, `project_code`, `path`, `title`, `name`, `symbol`, `kind`, `status`.
- Common labels: `File`, `Symbol`, `Requirement`, `Decision`, `Validation`, `Evidence`, `UnresolvedEndpoint`.
- Query catalog: `PreparedQuery.name` and `PreparedQuery.rank`.

## Queries

Start inside Memgraph Lab with `prepared_queries.cypher`. It lists every installed query, including parameterized catalog queries.

Each installed `PreparedQuery` carries:

- `cypher`: parameterized template, for example with `project_code`.
- `parameters`: compact parameter contract.
- `usage`: how to run it in Memgraph Lab.
- `cypher_all_projects`: direct all-projects variant when Lab parameters are inconvenient.

- `overview.cypher`: compact node/edge inventory.
- `prepared_queries.cypher`: list the installed query catalog inside Memgraph Lab.
- `soll_decisions.cypher`: current SOLL decision map.
- `requirement_coverage.cypher`: requirement to decision/validation/evidence coverage.
- `ist_soll_traceability.cypher`: SOLL intent to supporting evidence paths.
- `hot_files.cypher`: graph-ready/vector-ready file overview for human inspection.

Additional parameterized query templates live under `queries/memgraph/catalog/` and are installed into Memgraph as `PreparedQuery` nodes.
Most catalog templates accept `project_code`; use an empty string for all projects.

Recommended Lab flow:

1. Run `prepared_queries.cypher`.
2. Run `project_code_inventory` to see available projects.
3. Copy the `cypher` field for a focused query and set `project_code`, or copy `cypher_all_projects` for the full estate.
4. Use `trace_target_context` for a known id/path/title/symbol fragment.

The query pack is intentionally human-only. LLM clients must continue to use Axon MCP, which carries compact guidance, degraded-state semantics, and canonical recovery paths.
