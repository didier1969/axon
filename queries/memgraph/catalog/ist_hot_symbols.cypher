// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (s:Symbol)
WHERE project_code = '' OR s.project_code = project_code
OPTIONAL MATCH (s)-[out]->()
OPTIONAL MATCH ()-[in]->(s)
RETURN
  s.project_code AS project,
  s.title AS symbol,
  s.kind AS kind,
  count(DISTINCT out) AS outgoing_edges,
  count(DISTINCT in) AS incoming_edges,
  count(DISTINCT out) + count(DISTINCT in) AS total_degree
ORDER BY total_degree DESC, incoming_edges DESC, outgoing_edges DESC
LIMIT 200;
