// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (u:UnresolvedEndpoint)
WHERE project_code = '' OR u.project_code = project_code
OPTIONAL MATCH (u)-[out]->()
OPTIONAL MATCH ()-[in]->(u)
RETURN
  u.project_code AS project,
  u.id AS endpoint,
  count(DISTINCT in) AS incoming_edges,
  count(DISTINCT out) AS outgoing_edges,
  count(DISTINCT in) + count(DISTINCT out) AS total_degree
ORDER BY total_degree DESC, incoming_edges DESC
LIMIT 300;
