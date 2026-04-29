// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (n:AxonNode)
WHERE project_code = '' OR n.project_code = project_code
WITH coalesce(n.project_code, 'unknown') AS project, collect(n) AS nodes
RETURN
  project,
  size(nodes) AS total_nodes,
  size([n IN nodes WHERE 'File' IN labels(n)]) AS files,
  size([n IN nodes WHERE 'Symbol' IN labels(n)]) AS symbols,
  size([n IN nodes WHERE 'Requirement' IN labels(n)]) AS requirements,
  size([n IN nodes WHERE 'Decision' IN labels(n)]) AS decisions,
  size([n IN nodes WHERE 'Validation' IN labels(n)]) AS validations,
  size([n IN nodes WHERE 'Evidence' IN labels(n)]) AS evidence,
  size([n IN nodes WHERE 'UnresolvedEndpoint' IN labels(n)]) AS unresolved_endpoints,
  size([n IN nodes WHERE n.graph_ready = true]) AS graph_ready_files,
  size([n IN nodes WHERE n.vector_ready = true]) AS vector_ready_files
ORDER BY total_nodes DESC, project;
