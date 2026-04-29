// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (a:AxonNode)-[r]->(b:AxonNode)
WHERE a.project_code IS NOT NULL
  AND b.project_code IS NOT NULL
  AND a.project_code <> b.project_code
  AND (project_code = '' OR a.project_code = project_code OR b.project_code = project_code)
RETURN
  a.project_code AS from_project,
  b.project_code AS to_project,
  type(r) AS relation,
  count(r) AS count
ORDER BY count DESC, from_project, to_project
LIMIT 200;
