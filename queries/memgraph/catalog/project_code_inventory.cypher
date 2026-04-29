// Parameters: none.
MATCH (n:AxonNode)
WITH coalesce(n.project_code, 'unknown') AS project, labels(n) AS node_labels
RETURN
  project,
  count(*) AS nodes,
  sum(CASE WHEN 'File' IN node_labels THEN 1 ELSE 0 END) AS files,
  sum(CASE WHEN 'Symbol' IN node_labels THEN 1 ELSE 0 END) AS symbols,
  sum(CASE WHEN 'Requirement' IN node_labels THEN 1 ELSE 0 END) AS requirements,
  sum(CASE WHEN 'Decision' IN node_labels THEN 1 ELSE 0 END) AS decisions,
  sum(CASE WHEN 'Validation' IN node_labels THEN 1 ELSE 0 END) AS validations,
  sum(CASE WHEN 'Evidence' IN node_labels THEN 1 ELSE 0 END) AS evidence,
  sum(CASE WHEN 'UnresolvedEndpoint' IN node_labels THEN 1 ELSE 0 END) AS unresolved_endpoints
ORDER BY nodes DESC, project;
