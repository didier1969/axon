// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (intent)-[:TRACEABLE_TO]->(e:Evidence)
RETURN
  labels(intent) AS intent_labels,
  intent.title AS intent,
  intent.status AS status,
  e.kind AS evidence_type,
  e.path AS evidence_ref
ORDER BY intent, evidence_ref
LIMIT 200;
