// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (r)
WHERE 'Requirement' IN labels(r)
OPTIONAL MATCH (d)-[:SOLVES]->(r)
OPTIONAL MATCH (v)-[:VERIFIES]->(r)
OPTIONAL MATCH (r)-[:TRACEABLE_TO]->(e:Evidence)
RETURN
  r.title AS requirement,
  r.status AS status,
  count(DISTINCT d) AS solving_decisions,
  count(DISTINCT v) AS validations,
  count(DISTINCT e) AS evidence_items
ORDER BY validations ASC, evidence_items ASC, requirement
LIMIT 200;
