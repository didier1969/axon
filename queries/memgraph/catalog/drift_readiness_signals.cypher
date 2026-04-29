// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (f:File)
WHERE project_code = '' OR f.project_code = project_code
WITH
  f.project_code AS project,
  f.status AS status,
  f.graph_ready AS graph_ready,
  f.vector_ready AS vector_ready,
  count(*) AS files,
  sum(coalesce(f.size_bytes, 0)) AS total_size_bytes
RETURN
  project,
  status,
  graph_ready,
  vector_ready,
  files,
  total_size_bytes,
  CASE
    WHEN graph_ready = false AND vector_ready = false THEN 'graph_and_vector_missing'
    WHEN graph_ready = false THEN 'graph_missing'
    WHEN vector_ready = false THEN 'vector_missing'
    ELSE 'ready'
  END AS readiness_signal,
  CASE
    WHEN graph_ready = false AND vector_ready = false THEN 0
    WHEN graph_ready = false THEN 1
    WHEN vector_ready = false THEN 2
    ELSE 3
  END AS readiness_rank
ORDER BY
  readiness_rank,
  files DESC,
  total_size_bytes DESC;
