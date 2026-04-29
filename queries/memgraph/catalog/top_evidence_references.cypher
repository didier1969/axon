// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (intent:AxonNode)-[:TRACEABLE_TO]->(e:Evidence)
WHERE project_code = '' OR intent.project_code = project_code OR e.project_code = project_code
RETURN
  coalesce(e.project_code, intent.project_code) AS project,
  coalesce(e.title, e.path, e.id) AS evidence,
  e.kind AS evidence_type,
  count(DISTINCT intent) AS linked_intents,
  collect(DISTINCT coalesce(intent.title, intent.id))[0..20] AS sample_intents
ORDER BY linked_intents DESC, project, evidence
LIMIT 200;
