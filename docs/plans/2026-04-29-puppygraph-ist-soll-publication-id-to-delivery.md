# PuppyGraph IST/SOLL Publication - ID to Delivery

Date: 2026-04-29
Status: planned
Scope: human visualization of Axon IST + SOLL graph surfaces

## Intent

Publish Axon's IST and SOLL structural surfaces to PuppyGraph so humans can visually inspect the graph and run prepared Gremlin/openCypher queries against a stable, read-only publication snapshot.

This does not change the LLM access contract. LLM clients continue to use Axon MCP as the canonical information surface. PuppyGraph is an auxiliary human visualization and exploration surface.

## Value Added

Axon already exposes high-quality machine access through MCP, but humans still need a navigable visual graph for trust-building, audits, demonstrations, and structural debugging. PuppyGraph can query relational data as a graph without loading it into a proprietary graph store, which lets Axon keep its canonical IST/SOLL stores while adding a human-oriented graph lens.

The product value is:
- Faster human comprehension of IST/SOLL topology.
- Demonstrable graph visualization for commercial conversations.
- Lower risk than replacing Axon's graph store because PuppyGraph is a read-only publication layer.
- Reusable query packs for support, audits, architecture reviews, and onboarding.

## External Findings

PuppyGraph current documentation confirms:
- Deployment is available through Docker and exposes UI plus Gremlin/Cypher endpoints.
- A graph is defined by uploading a schema JSON that maps vertices and edges to tables or views.
- DuckDB is supported through JDBC against a persistent database file.
- DuckDB cannot safely be read by PuppyGraph while another process writes to the same database file, so Axon must publish snapshots or read replicas rather than pointing PuppyGraph at a live writer.
- PuppyGraph supports Gremlin and openCypher, including schema discovery procedures such as `db.labels()`, `db.propertyKeys()`, and `db.relationshipTypes()`.

Sources:
- https://docs.puppygraph.com/getting-started/querying-duckdb-data-as-a-graph/
- https://docs.puppygraph.com/reference/schema/
- https://docs.puppygraph.com/connecting/connecting-to-duckdb/
- https://docs.puppygraph.com/querying/querying-using-opencypher/

## Axon Current Truth

SOLL already contains the product intent:
- `REQ-AXO-021`: PuppyGraph visualizes IST and SOLL for humans.
- `DEC-AXO-021`: PuppyGraph is human-only visualization; LLM access remains MCP.

Live runtime truth:
- `brain` is the public MCP and SOLL writer authority.
- `indexer` is the IST writer authority.
- `system_converged=true`.
- Public MCP remains the LLM contract.

Relevant IST/SOLL source tables discovered through MCP include:
- IST/source-code graph: `File`, `Symbol`, `Chunk`, `CALLS`, `CONTAINS`, `IMPACTS`, `SUBSTANTIATES`, `Node`, `Edge`, `Traceability`.
- Vector/analytics context: `ChunkEmbedding`, `GraphEmbedding`, `GraphProjection`, `FileVectorizationQueue`.
- SOLL canonical intent: `soll.Node`, `soll.Edge`, `soll.Traceability`.

## Non-Goals

- Do not expose PuppyGraph as an LLM access path.
- Do not make PuppyGraph a writer to IST or SOLL.
- Do not connect PuppyGraph directly to an active writer database if that creates DuckDB concurrency risk.
- Do not duplicate Axon's MCP guidance, argument repair, or LLM token-optimized workflows in PuppyGraph.
- Do not hard-code a graph schema that cannot evolve with Axon labels and relation types.

## Target Architecture

The safe architecture is snapshot publication:

1. Axon canonical stores remain authoritative.
2. A publication job creates a read-only PuppyGraph DuckDB database under `.axon/puppygraph/publication/`.
3. The publication DB contains stable views/tables optimized for graph mapping.
4. PuppyGraph Docker connects only to the publication DB.
5. A generated schema JSON maps Axon publication tables to graph vertices and edges.
6. A query pack provides prepared Gremlin/openCypher questions for human operators.
7. MCP reports publication freshness and points humans to PuppyGraph, while LLMs keep using MCP.

## Publication Data Model

Minimum vertices:
- `Project`: project identity and canonical code.
- `File`: indexed source/document files.
- `Symbol`: functions, modules, methods, sections, and code/doc symbols.
- `Chunk`: text/code chunks when useful for drill-down.
- `Intent`: SOLL nodes, with `intent_type` = Requirement, Decision, Concept, Milestone, Validation, Vision, Pillar.
- `TraceArtifact`: evidence and traceability artifacts.

Minimum edges:
- `CONTAINS`: `File -> Symbol`, optionally `File -> Chunk`.
- `CALLS`: `Symbol -> Symbol`.
- `IMPACTS`: source to impacted target.
- `SUBSTANTIATES`: evidence/support relation.
- `SOLL_EDGE`: generic SOLL relation preserving `relation_type`.
- `SOLVES`: `Decision -> Requirement`.
- `VERIFIES`: `Validation -> Requirement`.
- `BELONGS_TO`: requirement to pillar/milestone/parent intent.
- `TRACEABLE_TO`: SOLL node to file/symbol/document/metric evidence.

Preferred physical tables in the publication DB:
- `pg_project`
- `pg_file`
- `pg_symbol`
- `pg_chunk`
- `pg_intent`
- `pg_trace_artifact`
- `pg_contains`
- `pg_calls`
- `pg_impacts`
- `pg_substantiates`
- `pg_soll_edge`
- `pg_traceable_to`
- `pg_publication_manifest`

Every edge table must have:
- stable `id`
- `from_id`
- `to_id`
- `project_code`
- relation-specific attributes

Every vertex table must have:
- stable `id`
- `project_code`
- human-readable label fields
- provenance/freshness fields where relevant

## Prepared Query Pack

The integration is not functional until the following queries are installed and verified.

Schema/health:
```cypher
CALL db.labels()
```

```cypher
CALL db.relationshipTypes()
```

```cypher
CALL db.schema.nodeTypeProperties()
```

Project overview:
```cypher
MATCH (p:Project)
RETURN p.project_code, p.name, p.publication_generated_at
```

Source-code neighborhood:
```cypher
MATCH p=(f:File)-[:CONTAINS]->(s:Symbol)-[:CALLS]->(t:Symbol)
RETURN p
LIMIT 50
```

Symbol dependency blast radius:
```cypher
MATCH p=(s:Symbol {name: $symbol})-[:CALLS*1..3]->(t:Symbol)
RETURN p
LIMIT 100
```

SOLL decision coverage:
```cypher
MATCH p=(d:Intent {intent_type: 'Decision'})-[:SOLVES]->(r:Intent {intent_type: 'Requirement'})
RETURN p
LIMIT 100
```

Requirement validation gaps:
```cypher
MATCH (r:Intent {intent_type: 'Requirement'})
WHERE NOT (()-[:VERIFIES]->(r))
RETURN r.id, r.title, r.status
ORDER BY r.id
```

IST/SOLL traceability:
```cypher
MATCH p=(i:Intent)-[:TRACEABLE_TO]->(a:TraceArtifact)
RETURN p
LIMIT 100
```

Cross-surface source-to-intent:
```cypher
MATCH p=(r:Intent {intent_type: 'Requirement'})-[:TRACEABLE_TO]->(:TraceArtifact)-[:REFERENCES]->(f:File)-[:CONTAINS]->(s:Symbol)
RETURN p
LIMIT 100
```

Topology demonstration:
```cypher
MATCH p=(:Intent)-[:SOLVES|VERIFIES|BELONGS_TO|TRACEABLE_TO*1..3]-()
RETURN p
LIMIT 150
```

## ID to Delivery Plan

### Phase 0 - Reality Gate

Deliverables:
- Confirm Docker availability and local port allocation.
- Confirm PuppyGraph image pull is allowed in target environments.
- Confirm Axon publication DB format and DuckDB/JDBC compatibility.
- Confirm no active-writer database is connected directly to PuppyGraph.

Exit criteria:
- A command can report `docker compose version`.
- A generated publication DB exists and is not the live writer DB.
- The plan documents the chosen refresh model.

### Phase 1 - Publication Snapshot

Deliverables:
- `scripts/puppygraph/export_publication.py` or equivalent Rust/Python exporter.
- Snapshot path: `.axon/puppygraph/publication/axon_graph.duckdb`.
- Manifest path: `.axon/puppygraph/publication/manifest.json`.
- Views/tables matching the publication data model.

Exit criteria:
- Export job succeeds from live data without mutating IST/SOLL.
- Publication manifest records source build, generation, timestamp, table counts, and row counts.
- DuckDB validation query confirms all vertex/edge tables exist.

### Phase 2 - PuppyGraph Runtime

Deliverables:
- `deploy/puppygraph/docker-compose.yml`.
- `.env.example` for ports, credentials, and publication DB path.
- `scripts/puppygraph/start.sh`, `stop.sh`, `status.sh`.
- Runtime isolation from Axon brain/indexer lifecycle.

Exit criteria:
- PuppyGraph UI is reachable.
- Gremlin endpoint is reachable.
- Cypher/Bolt endpoint is reachable if enabled by PuppyGraph image.
- Startup does not require stopping Axon.

### Phase 3 - Schema Generation

Deliverables:
- `scripts/puppygraph/generate_schema.py`.
- Generated `schema.json` mapping Axon publication tables to graph labels.
- Schema upload command using PuppyGraph HTTP API.

Exit criteria:
- `curl -XPOST ... /schema` returns OK.
- PuppyGraph UI shows the schema.
- `CALL db.labels()` returns expected labels.
- `CALL db.relationshipTypes()` returns expected edge types.

### Phase 4 - Query Pack

Deliverables:
- `queries/puppygraph/*.cypher`.
- `queries/puppygraph/*.gremlin` where Gremlin gives better visualization.
- `scripts/puppygraph/run_query_pack.sh`.
- Query result fixtures for smoke validation.

Exit criteria:
- All prepared schema/health queries pass.
- At least one IST-only query returns data.
- At least one SOLL-only query returns data.
- At least one IST/SOLL traceability query returns data or produces a clear empty-state diagnostic.

### Phase 5 - MCP/Dashboard Integration

Deliverables:
- MCP `status` or `project_status` includes PuppyGraph publication freshness and human URL.
- Dashboard includes a human-only PuppyGraph link when publication is ready.
- Help/guidance states clearly that LLMs should continue using MCP.

Exit criteria:
- MCP does not expose PuppyGraph as an LLM data access substitute.
- Human operators can navigate from dashboard/status to PuppyGraph.
- Stale/missing publication gives actionable remediation.

### Phase 6 - Validation and Release

Deliverables:
- Local smoke test script.
- Release preflight check.
- SOLL validation node verifying `REQ-AXO-021`.
- Evidence attached to `REQ-AXO-021` and `DEC-AXO-021`.

Exit criteria:
- `REQ-AXO-021` can move from partial to done.
- PuppyGraph publication is reproducible from a clean checkout.
- Live promotion does not break MCP, dashboard, indexer, or TensorRT work.

## Validation Matrix

Functional:
- Docker compose starts PuppyGraph.
- Schema upload succeeds.
- Prepared queries execute.
- Graph UI renders vertices and edges.

Safety:
- PuppyGraph only reads publication snapshots.
- No live writer lock contention.
- Credentials are not committed.
- Publication can be regenerated and deleted safely.

Freshness:
- Manifest shows source generation.
- MCP status reports publication age.
- Query pack warns if publication is stale.

Operator experience:
- One command exports.
- One command starts.
- One command uploads schema.
- One command runs smoke queries.
- Dashboard/status provide the human URL.

## Risks and Mitigations

Risk: DuckDB concurrent access conflict.
Mitigation: publish snapshot DB; never connect PuppyGraph to active writer DB.

Risk: Schema drift between Axon tables and PuppyGraph schema.
Mitigation: generate schema from a declared publication model and validate table counts before upload.

Risk: Graph too dense for human visualization.
Mitigation: ship scoped query packs and logical partition examples; default query limits.

Risk: LLM clients bypass MCP and lose guidance/traceability semantics.
Mitigation: status/help/dashboard mark PuppyGraph as human-only; MCP remains canonical.

Risk: Publication stale relative to live runtime.
Mitigation: manifest age, status warnings, and explicit refresh command.

## First Implementation Slice

Build the smallest functional slice:
1. Export `Project`, `File`, `Symbol`, `Intent`, `CONTAINS`, `CALLS`, `SOLL_EDGE`.
2. Generate and upload PuppyGraph schema.
3. Start PuppyGraph with Docker compose.
4. Verify three queries:
   - `CALL db.labels()`
   - symbol call neighborhood
   - decision-to-requirement SOLL graph
5. Report publication freshness through MCP or dashboard.

This slice is enough to prove the architecture and unblock `REQ-AXO-021` implementation.
