// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (n:AxonNode)
WHERE project_code = '' OR n.project_code = project_code
WITH coalesce(n.project_code, 'unknown') AS project, labels(n) AS labels
UNWIND labels AS label
WITH project, label, count(*) AS count
WHERE label <> 'AxonNode'
RETURN project, label, count
ORDER BY project, count DESC, label;
