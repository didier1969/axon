// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (f:File)-[:CONTAINS]->(s:Symbol)
WHERE project_code = '' OR f.project_code = project_code OR s.project_code = project_code
WITH f, count(s) AS symbols
OPTIONAL MATCH (f)-[r]-()
RETURN
  f.project_code AS project,
  f.path AS file,
  f.status AS status,
  symbols,
  count(DISTINCT r) AS graph_degree,
  f.size_bytes AS size_bytes
ORDER BY graph_degree DESC, symbols DESC, size_bytes DESC
LIMIT 200;
