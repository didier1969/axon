// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (caller:Symbol)-[r:CALLS]->(callee:Symbol)
WHERE project_code = '' OR caller.project_code = project_code OR callee.project_code = project_code
OPTIONAL MATCH (file:File)-[:CONTAINS]->(caller)
RETURN
  coalesce(caller.project_code, callee.project_code, file.project_code) AS project,
  file.path AS file,
  caller.title AS caller,
  count(DISTINCT callee) AS callees,
  collect(DISTINCT callee.title)[0..20] AS sample_callees
ORDER BY callees DESC, file, caller
LIMIT 200;
