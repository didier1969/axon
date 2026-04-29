// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (r:Requirement)
WHERE project_code = '' OR r.project_code = project_code
OPTIONAL MATCH (d)-[:SOLVES]->(r)
OPTIONAL MATCH (v)-[:VERIFIES]->(r)
OPTIONAL MATCH (r)-[:TRACEABLE_TO]->(e)
WITH
  r,
  count(DISTINCT d) AS decisions,
  count(DISTINCT v) AS validations,
  count(DISTINCT e) AS evidence
RETURN
  r.project_code AS project,
  r.title AS requirement,
  r.status AS status,
  decisions,
  validations,
  evidence,
  CASE
    WHEN validations = 0 THEN 'missing_validation'
    WHEN evidence = 0 THEN 'missing_evidence'
    WHEN decisions = 0 THEN 'missing_decision'
    ELSE 'covered'
  END AS risk
ORDER BY risk DESC, validations ASC, evidence ASC, requirement
LIMIT 300;
