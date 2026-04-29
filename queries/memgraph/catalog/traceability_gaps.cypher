// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (n:AxonNode)
WITH project_code, n, labels(n) AS node_labels
WHERE (project_code = '' OR n.project_code = project_code)
  AND ('Requirement' IN node_labels OR 'Decision' IN node_labels OR 'Validation' IN node_labels)
OPTIONAL MATCH (n)-[:TRACEABLE_TO]->(e)
WITH n, node_labels, count(DISTINCT e) AS evidence
WHERE evidence = 0
WITH
  n,
  CASE
    WHEN 'Requirement' IN node_labels THEN 'Requirement'
    WHEN 'Decision' IN node_labels THEN 'Decision'
    WHEN 'Validation' IN node_labels THEN 'Validation'
    ELSE 'Intent'
  END AS intent_label,
  evidence
RETURN
  n.project_code AS project,
  intent_label AS label,
  n.title AS intent,
  n.status AS status,
  evidence
ORDER BY project, label, intent
LIMIT 300;
