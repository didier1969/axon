# Memgraph Lab Query Collection Publication

## Finding

Axon already installs the 27 human Memgraph queries as graph-backed `PreparedQuery`
nodes through `memgraph-query-pack-loader`. This makes the catalog queryable from
Memgraph itself, but it does not automatically populate the native Memgraph Lab
Collections sidebar.

Memgraph Lab query collections are browser-side UI state. They are not canonical
Memgraph database objects and cannot be made universally visible to every Lab
browser session by only loading Cypher into Memgraph.

## Decision

Axon publishes both surfaces:

- Graph-backed catalog: `queries/memgraph/bootstrap/axon_query_pack.cypherl`.
- Lab import collection: `queries/memgraph/bootstrap/axon_lab_query_collection.json`.
- Same-origin browser installer: `queries/memgraph/bootstrap/axon_lab_install_collection.html`.

`./scripts/axon memgraph start` regenerates both files before starting Docker
Compose. The Lab container also serves the JSON file at:

```text
http://localhost:3000/axon_lab_query_collection.json
http://localhost:3000/axon_lab_install_collection.html
```

The JSON collection contains 27 click-oriented queries ordered for human use:
overview first, then project inventory/dashboard/health, then focused tracing
and risk queries. Parameter placeholders are replaced with safe defaults for
Lab execution:

- all projects by default
- `limit = 100`
- `min_degree = 25`
- `target = 'Axon'`

Humans can then edit project or target filters inside Lab when needed.

The installer writes the generated collection into the current browser profile's
`memgraph-lab-db.collections` IndexedDB store. It is intentionally same-origin
with Lab so browsers permit IndexedDB access without relaxing security.

## Validation

Commands executed:

```bash
./scripts/axon memgraph build-lab-collection
bash -n scripts/memgraph-projection.sh
python3 -m py_compile scripts/memgraph_build_cypherl.py scripts/memgraph_build_query_pack.py scripts/memgraph_build_lab_collection.py
docker compose -f docker-compose.memgraph.yml config
./scripts/axon memgraph start
curl -sS http://127.0.0.1:3000/axon_lab_query_collection.json
./scripts/axon memgraph query-pack-status
./scripts/axon memgraph status
./scripts/axon memgraph lab-collection-status
curl -sS http://127.0.0.1:3000/axon_lab_install_collection.html
```

Observed:

- generated Lab collection query count: `27`
- collection first entries: `Overview`, `Project Code Inventory`, `Project Dashboard`
- no unresolved Lab placeholders remain: `$project_code`, `$target`, `$limit`, `$min_degree`
- active Memgraph pack: `installed_prepared_queries = 27`
- Memgraph Lab container: running on port `3000`
- `lab-collection-status`: local and Lab-served collection both `click_ready=true`
- browser-side installer page: served by Lab container

## Contract

Memgraph remains a human-only visualization projection. LLM clients must use
Axon MCP for IST/SOLL truth, guidance, degraded-state semantics, and recovery
paths.
