// Parameters: project_code optional. Use "" or null for all projects.
WITH coalesce($project_code, '') AS project_code
MATCH (f:File)
WHERE project_code = '' OR f.project_code = project_code
RETURN
  f.project_code AS project,
  f.status AS status,
  f.graph_ready AS graph_ready,
  f.vector_ready AS vector_ready,
  count(*) AS files,
  sum(coalesce(f.size_bytes, 0)) AS total_size_bytes
ORDER BY project, files DESC;
