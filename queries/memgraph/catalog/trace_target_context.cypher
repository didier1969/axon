// Parameters: target required as id/path/title/symbol fragment, project_code optional.
WITH
  coalesce($project_code, '') AS project_code,
  coalesce($target, '') AS target
MATCH (n:AxonNode)
WHERE target <> ''
  AND (project_code = '' OR n.project_code = project_code)
  AND (
    n.id = target
    OR n.path = target
    OR n.title = target
    OR n.name = target
    OR n.symbol = target
    OR n.id CONTAINS target
    OR n.path CONTAINS target
    OR n.title CONTAINS target
  )
OPTIONAL MATCH (n)-[r1]-(one:AxonNode)
OPTIONAL MATCH (one)-[r2]-(two:AxonNode)
RETURN
  n.project_code AS project,
  labels(n) AS target_labels,
  coalesce(n.title, n.path, n.id) AS target_node,
  count(DISTINCT one) AS one_hop_neighbors,
  count(DISTINCT two) AS two_hop_neighbors,
  collect(DISTINCT type(r1))[0..20] AS one_hop_relations,
  collect(DISTINCT coalesce(one.title, one.path, one.id))[0..30] AS sample_one_hop,
  collect(DISTINCT coalesce(two.title, two.path, two.id))[0..30] AS sample_two_hop
ORDER BY one_hop_neighbors DESC, two_hop_neighbors DESC
LIMIT 50;
