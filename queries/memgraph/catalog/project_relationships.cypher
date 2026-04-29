// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (a:AxonNode)-[r]->(b:AxonNode)
WHERE project_code = '' OR a.project_code = project_code OR b.project_code = project_code
RETURN
  coalesce(a.project_code, b.project_code, 'unknown') AS project,
  type(r) AS relation,
  count(r) AS count
ORDER BY project, count DESC, relation;
