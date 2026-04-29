// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (n:AxonNode)
WHERE project_code = '' OR n.project_code = project_code
WITH coalesce(n.project_code, 'unknown') AS project, labels(n) AS node_labels, n
RETURN
  project,
  count(*) AS total_nodes,
  sum(CASE WHEN 'File' IN node_labels THEN 1 ELSE 0 END) AS files,
  sum(CASE WHEN 'Symbol' IN node_labels THEN 1 ELSE 0 END) AS symbols,
  sum(CASE WHEN 'Requirement' IN node_labels THEN 1 ELSE 0 END) AS requirements,
  sum(CASE WHEN 'Decision' IN node_labels THEN 1 ELSE 0 END) AS decisions,
  sum(CASE WHEN 'Validation' IN node_labels THEN 1 ELSE 0 END) AS validations,
  sum(CASE WHEN 'Evidence' IN node_labels THEN 1 ELSE 0 END) AS evidence,
  sum(CASE WHEN 'UnresolvedEndpoint' IN node_labels THEN 1 ELSE 0 END) AS unresolved_endpoints,
  sum(CASE WHEN n.graph_ready = true THEN 1 ELSE 0 END) AS graph_ready_files,
  sum(CASE WHEN n.vector_ready = true THEN 1 ELSE 0 END) AS vector_ready_files
ORDER BY total_nodes DESC, project;
