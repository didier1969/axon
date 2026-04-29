// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (s:Symbol)
WHERE project_code = '' OR s.project_code = project_code
OPTIONAL MATCH (s)-[r]-()
WITH s, count(DISTINCT r) AS degree
WHERE degree = 0
RETURN
  s.project_code AS project,
  s.title AS symbol,
  s.kind AS kind,
  s.status AS status,
  degree
ORDER BY project, symbol
LIMIT 300;
