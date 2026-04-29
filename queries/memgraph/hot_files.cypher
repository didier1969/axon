// Copyright (c) Didier Stadelmann. All rights reserved.
// Human-only Memgraph projection query. LLM clients must use Axon MCP.

MATCH (f:File)
RETURN
  f.project_code AS project,
  f.path AS path,
  f.status AS status,
  f.graph_ready AS graph_ready,
  f.vector_ready AS vector_ready,
  f.size_bytes AS size_bytes
ORDER BY coalesce(f.size_bytes, 0) DESC
LIMIT 100;
