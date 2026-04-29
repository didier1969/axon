// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (n:AxonNode)
WITH project_code, n, labels(n) AS node_labels
WHERE (project_code = '' OR n.project_code = project_code)
  AND ('Requirement' IN node_labels OR 'Decision' IN node_labels OR 'Validation' IN node_labels)
OPTIONAL MATCH (n)-[:TRACEABLE_TO]->(e)
WITH n, node_labels, count(DISTINCT e) AS evidence
WHERE evidence = 0
RETURN
  n.project_code AS project,
  node_labels AS labels,
  n.title AS intent,
  n.status AS status,
  evidence
ORDER BY project, labels, intent
LIMIT 300;
