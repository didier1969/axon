# Memgraph IST/SOLL Human Projection - ID to Delivery

Date: 2026-04-29
Status: implemented-first-runtime-slice
Scope: human-only visualization of Axon IST + SOLL graph surfaces

## Intent

Publish Axon's IST and SOLL graph surfaces to a disposable Memgraph projection so humans can inspect topology, traceability, coverage, and structural risks visually.

LLM clients must continue to use Axon MCP. Memgraph is not canonical and is not an LLM retrieval surface.

## Superseded Direction

The earlier PuppyGraph-specific plan is superseded.

Reason:
- PuppyGraph's main value was direct graph querying over relational stores.
- Axon must not expose mutable canonical `ist.db` or `soll.db` to external readers.
- Once a separate snapshot/publication is required, a Memgraph in-memory projection with prepared human queries is the more durable product path.

PuppyGraph may remain a future optional secondary consumer, but it is no longer the primary architecture.

## Value Added

The projection gives humans a fast, visual, disposable graph lens without weakening Axon's canonical writer model:
- canonical IST/SOLL remain owned by Axon runtime authorities
- humans get navigable graph visualization for audits, demos, support, and architecture review
- LLMs keep MCP's token-optimized guidance, recovery, and traceability semantics
- publication artifacts are reproducible, inspectable, and safe to delete

## Architecture

1. Axon reads only controlled reader/snapshot files.
2. A publication command builds a graph-shaped Parquet directory.
3. The publication contains `nodes.parquet`, `edges.parquet`, and `manifest.json`.
4. Memgraph imports from that publication into a staging database.
5. Validation compares manifest counts, query-pack smoke results, and freshness.
6. Blue/green promotion exposes the active human Memgraph endpoint.
7. Dashboard/MCP status report projection freshness and the explicit LLM contract.

## Current Implementation Slice

Implemented:
- `./scripts/axon publish-memgraph`
- `./scripts/axon memgraph build-import`
- `./scripts/axon memgraph validate`
- `./scripts/axon memgraph start|stop|status|load`
- Rust publisher: `src/axon-plugin-duckdb/src/bin_memgraph_publication.rs`
- Versioned publication directory with manifest
- Graph-shaped Parquet:
  - `nodes.parquet`
  - `edges.parquet`
- `current` symlink and `current.json`
- retention cleanup for successful publications
- Cypher import generator: `scripts/memgraph_build_cypherl.py`
- publication validator: `scripts/memgraph_validate_publication.py`
- Docker Compose runtime: `docker-compose.memgraph.yml`
- pinned latest-stable Docker images as of 2026-04-29:
  - `memgraph/memgraph:3.9.0-relwithdebinfo-malloc`
  - `memgraph/lab:3.9.0`
  - `memgraph/mgconsole:1.5.0`
- Initial human query pack under `queries/memgraph/`
- `ist-query` panic fix for DuckDB column metadata access after execution

Smoke validation on 2026-04-29:
- publication scope: all projects, human visualization surface
- publication id: `smoke-20260429-all-projects-v4`
- nodes: `489471`
- edges: `380013`
- unresolved endpoint nodes: `77260`
- import file size: `427014530` bytes
- manifest: `/tmp/axon-memgraph-publications/smoke-20260429-all-projects-v4/manifest.json`
- validation status: `ok`
- runtime import status: Memgraph loaded successfully with `489471` nodes and `380013` edges
- query pack status: `./scripts/axon memgraph smoke-queries` passed for all `27` prepared queries in compact `EXPLAIN` mode
- query execution status: `./scripts/axon memgraph smoke-queries --mode execute` passed for all `27` prepared queries after index installation and query-shape cleanup
- query catalog status: `27` prepared queries are installed as `PreparedQuery` nodes inside Memgraph Lab
- query catalog compatibility: full catalog `EXPLAIN` validation passed against Memgraph 3.9.0
- index status: the generated import drops and recreates lookup indexes for `AxonNode` ids, project scope, common file/symbol/title fields, common IST/SOLL labels, and the `PreparedQuery` catalog so repeated publication loads remain idempotent
- performance correction: the project inventory and health scoreboard queries avoid large `collect(n)` materialization and avoid per-project full edge rescans; relationship inventory remains available through `project_relationships`

Important correction:
- Memgraph is a global human visualization surface for all project graphs.
- `--project-only` exists only as a diagnostic narrow export.
- `--project-code` remains available as a fallback/default metadata value when a source table does not carry project identity.

Runtime status:
- Docker Desktop WSL socket was restored.
- Memgraph and Lab are running through `docker-compose.memgraph.yml`.
- Active human endpoints:
  - Memgraph Bolt: `localhost:7687`
  - Memgraph Lab: `http://localhost:3000`
- Current publication loaded successfully with counts matching the manifest.

## Remaining Delivery Plan

### Phase 1 - Projection Publisher

Status: implemented.

Remaining:
- add checksum/validation hash fields once final import contract is fixed

### Phase 2 - Memgraph Import Adapter

Status: implemented as Parquet-to-Cypher adapter.

Delivered:
- `scripts/memgraph_build_cypherl.py` reads `nodes.parquet` and `edges.parquet`
- generates batch `UNWIND` Cypher
- creates Memgraph indexes for human navigation and prepared-query lookup
- preserves dynamic labels and relationship types with safe identifier normalization
- marks all imported entities `human_only=true` and carries `publication_id`
- keeps all projects by default so humans can inspect the full IST/SOLL graph estate
- materializes `UnresolvedEndpoint` nodes for edge endpoints that are referenced but not present as canonical source nodes
- deduplicates nodes by `id` before export so edge imports do not multiply through duplicate node matches
- installs the prepared query pack as `PreparedQuery` nodes, linked from `PreparedQueryPack`, so humans can access query text directly in Memgraph Lab
- each `PreparedQuery` carries `parameters`, `usage`, `cypher`, and `cypher_all_projects` so Lab users can either bind parameters or run an all-projects variant directly

Exit criteria:
- generated import file validates structurally
- Docker-level load matches manifest node and edge counts

### Phase 3 - Blue/Green Promotion

Status: implemented for publication artifacts; Memgraph database import is loaded for the current publication.

Delivered:
- `current` pointer
- `current.json`
- retained successful publication count via `--retain-successful`
- obsolete successful publication cleanup

Exit criteria:
- current and previous successful publications are retained
- stale/incomplete publication directories are not served
- failed Memgraph DB imports keep compact diagnostics only after runtime loader is available

### Phase 4 - Dashboard and MCP Observability

Deliver:
- projection freshness in operator status
- active publication id
- disk usage
- Memgraph human URL
- explicit message: `LLM clients use Axon MCP, not Memgraph`

Exit criteria:
- missing/stale projection gives actionable remediation
- dashboard links are human-only
- MCP guidance does not route LLMs to Memgraph

### Phase 5 - Query Pack Qualification

Deliver:
- executable smoke runner for `queries/memgraph/*.cypher`
- query fixtures or compact result summaries
- coverage for overview, SOLL decisions, requirement coverage, traceability, and hot files

Delivered:
- `./scripts/axon memgraph smoke-queries`
- query catalog materialized in Memgraph through `PreparedQuery` nodes
- catalog coverage includes project inventory, project dashboard, relationship inventory, health scoreboard, readiness/drift signals, hot files, hot symbols, high-degree nodes, unresolved endpoint analysis, SOLL coverage/risk, traceability gaps, target context, evidence inventory, and cross-project links
- default smoke behavior validates top-level and `catalog/` Cypher with parameter substitution and compact output to avoid LLM-context flooding
- execution smoke exposed expensive all-project query shapes; project inventory and health scoreboard were rewritten to stream aggregates instead of collecting full project node sets
- execution smoke exposed a Memgraph ordering incompatibility on list-valued `labels(n)`; `traceability_gaps` now projects a scalar intent label before sorting

Exit criteria:
- every prepared query executes against the active projection
- empty results are explicit and diagnostic, not silent failures

### Phase 6 - Future Incremental Refresh

Status: explicitly gated.

Do not implement until:
- stable source epochs are available
- tombstones are available
- replacement semantics are proven
- validation checksums prevent stale/duplicate human projections

Until then, use full rebuild into staging plus blue/green promotion.
