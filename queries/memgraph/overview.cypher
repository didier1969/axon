// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (n)
UNWIND labels(n) AS label
WITH label, count(n) AS node_count
WHERE label <> 'AxonNode'
RETURN 'node' AS kind, label AS type, node_count AS count
UNION ALL
MATCH ()-[r]->()
RETURN 'edge' AS kind, type(r) AS type, count(r) AS count
ORDER BY kind, type;
