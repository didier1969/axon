// Parameters: project_code optional, min_degree optional. Use "" or null for all projects.
WITH
  coalesce($project_code, '') AS project_code,
  coalesce($min_degree, 25) AS min_degree
MATCH (n:AxonNode)
WHERE project_code = '' OR n.project_code = project_code
OPTIONAL MATCH (n)-[out]->()
WITH project_code, min_degree, n, count(DISTINCT out) AS outgoing_edges
OPTIONAL MATCH ()-[in]->(n)
WITH n, outgoing_edges, count(DISTINCT in) AS incoming_edges, min_degree
WITH n, labels(n) AS node_labels, incoming_edges, outgoing_edges, incoming_edges + outgoing_edges AS total_degree, min_degree
WHERE total_degree >= min_degree
RETURN
  n.project_code AS project,
  node_labels AS labels,
  coalesce(n.title, n.path, n.id) AS node,
  incoming_edges,
  outgoing_edges,
  total_degree
ORDER BY total_degree DESC, incoming_edges DESC, outgoing_edges DESC
LIMIT 300;
