// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (u:UnresolvedEndpoint)
WHERE project_code = '' OR u.project_code = project_code
OPTIONAL MATCH (source:AxonNode)-[incoming]->(u)
OPTIONAL MATCH (u)-[outgoing]->(target:AxonNode)
RETURN
  u.project_code AS project,
  u.id AS unresolved_endpoint,
  count(DISTINCT incoming) AS incoming_edges,
  count(DISTINCT outgoing) AS outgoing_edges,
  collect(DISTINCT type(incoming))[0..20] AS incoming_relations,
  collect(DISTINCT type(outgoing))[0..20] AS outgoing_relations,
  collect(DISTINCT coalesce(source.title, source.path, source.id))[0..20] AS sample_sources,
  collect(DISTINCT coalesce(target.title, target.path, target.id))[0..20] AS sample_targets
ORDER BY incoming_edges DESC, outgoing_edges DESC, unresolved_endpoint
LIMIT 200;
