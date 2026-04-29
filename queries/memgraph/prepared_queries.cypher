// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (q:PreparedQuery)
RETURN
  q.rank AS rank,
  q.name AS name,
  q.description AS description,
  q.parameters AS parameters,
  q.usage AS usage,
  q.path AS source_file,
  q.cypher AS cypher,
  q.cypher_all_projects AS cypher_all_projects
ORDER BY rank, name;
