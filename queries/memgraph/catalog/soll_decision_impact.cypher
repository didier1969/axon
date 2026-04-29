// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (d:Decision)
WHERE project_code = '' OR d.project_code = project_code
OPTIONAL MATCH p=(d)-[*1..2]->(target:AxonNode)
RETURN
  d.project_code AS project,
  d.title AS decision,
  d.status AS status,
  count(DISTINCT target) AS reachable_intent_nodes,
  collect(DISTINCT coalesce(target.title, target.id))[0..20] AS sample_targets
ORDER BY reachable_intent_nodes DESC, decision
LIMIT 200;
