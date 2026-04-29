// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (e:Evidence)
WHERE project_code = '' OR e.project_code = project_code
OPTIONAL MATCH (intent)-[:TRACEABLE_TO]->(e)
RETURN
  e.project_code AS project,
  e.kind AS evidence_type,
  count(DISTINCT e) AS evidence_items,
  count(DISTINCT intent) AS linked_intents
ORDER BY project, evidence_items DESC, evidence_type;
