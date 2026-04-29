# Memgraph Query Pack Loader Evidence

Date: 2026-04-29

Scope: Axon human-only Memgraph IST/SOLL visualization.

## Finding

Memgraph Lab does not expose Axon's prepared query pack automatically unless the pack is installed into the active Memgraph graph at runtime.

## Decision

`./scripts/axon memgraph start` generates `queries/memgraph/bootstrap/axon_query_pack.cypherl` before starting Docker Compose.

Docker Compose starts `memgraph-query-pack-loader` as a one-shot service. The service waits for Memgraph and loads the generated Cypher file with `mgconsole`, creating:

- `PreparedQueryPack {id: "axon_memgraph_query_pack"}`
- 27 `PreparedQuery` nodes
- `HAS_PREPARED_QUERY` links from the pack to each query

## Verification

Command:

```bash
./scripts/axon memgraph query-pack-status
```

Observed result:

```text
pack_id="axon_memgraph_query_pack"
publication_id="standalone"
installed_prepared_queries=27
```

Command:

```bash
./scripts/axon memgraph smoke-queries
```

Observed result:

```text
memgraph query pack smoke passed (27 queries, mode=explain)
```

## Human Usage

Open Memgraph Lab at `http://localhost:3000`, connect to `memgraph:7687`, then run `queries/memgraph/prepared_queries.cypher` or its contents. The result lists every prepared query and includes direct `cypher_all_projects` variants for Lab sessions where parameters are inconvenient.

LLM clients must continue to use Axon MCP, not Memgraph.
