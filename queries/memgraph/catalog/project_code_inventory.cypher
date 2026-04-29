// Parameters: none.
MATCH (n:AxonNode)
WITH coalesce(n.project_code, 'unknown') AS project, collect(n) AS nodes
OPTIONAL MATCH (a:AxonNode)-[r]->(b:AxonNode)
WHERE coalesce(a.project_code, b.project_code, 'unknown') = project
RETURN
  project,
  size(nodes) AS nodes,
  count(DISTINCT r) AS relationships,
  size([n IN nodes WHERE 'File' IN labels(n)]) AS files,
  size([n IN nodes WHERE 'Symbol' IN labels(n)]) AS symbols,
  size([n IN nodes WHERE 'Requirement' IN labels(n)]) AS requirements,
  size([n IN nodes WHERE 'Decision' IN labels(n)]) AS decisions,
  size([n IN nodes WHERE 'UnresolvedEndpoint' IN labels(n)]) AS unresolved_endpoints
ORDER BY nodes DESC, relationships DESC, project;
