// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (d)
WHERE 'Decision' IN labels(d)
OPTIONAL MATCH (d)-[rel]->(target)
RETURN
  d.title AS decision,
  d.status AS status,
  collect(DISTINCT type(rel) + ' -> ' + coalesce(target.title, target.id)) AS outgoing_links
ORDER BY decision
LIMIT 200;
