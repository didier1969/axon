use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::graph::{ExecFunc, GraphStore, PendingFile};

impl GraphStore {
    fn escape_sql(value: &str) -> String {
        value.replace("'", "''")
    }

    fn symbol_id(slug: &str, path: &str, name: &str) -> String {
        if Self::is_globally_qualified_symbol(name) {
            format!("{}::{}", slug, name)
        } else {
            format!("{}::{}::{}", slug, Self::symbol_path_namespace(path), name)
        }
    }

    fn relation_table(rel_type: &str) -> Option<&'static str> {
        match rel_type.to_lowercase().as_str() {
            "calls" | "calls_otp" => Some("CALLS"),
            "calls_nif" => Some("CALLS_NIF"),
            _ => None,
        }
    }

    fn chunk_id(symbol_id: &str) -> String {
        format!("{}::chunk", symbol_id)
    }

    fn is_globally_qualified_symbol(name: &str) -> bool {
        name.contains('.') || name.contains("::")
    }

    fn symbol_path_namespace(path: &str) -> String {
        let path = Path::new(path);
        let projects_root =
            std::env::var("AXON_PROJECTS_ROOT").unwrap_or_else(|_| "/home/dstadel/projects".to_string());
        let relative = path
            .strip_prefix(&projects_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        relative.replace('/', "::")
    }

    fn build_chunk_content(path: &str, symbol: &crate::parser::Symbol, content: &str) -> String {
        let lines: Vec<&str> = content.lines().collect();
        let start = symbol.start_line.saturating_sub(1).min(lines.len());
        let end = symbol.end_line.min(lines.len()).max(start);
        let snippet = if start < end {
            lines[start..end].join("\n")
        } else {
            String::new()
        };

        format!(
            "symbol: {}\nkind: {}\nfile: {}\nlines: {}-{}\n\n{}",
            symbol.name, symbol.kind, path, symbol.start_line, symbol.end_line, snippet
        )
    }

    fn stable_content_hash(value: &str) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
        for (path, project, size, mtime) in file_paths {
            queries.extend(Self::upsert_file_queries(path, project, *size, *mtime, 100));
        }
        self.execute_batch(&queries)
    }

    pub fn upsert_hot_file(&self, path: &str, project: &str, size: i64, mtime: i64, priority: i64) -> Result<()> {
        let queries = Self::upsert_file_queries(path, project, size, mtime, priority);
        self.execute_batch(&queries)
    }

    pub fn insert_file_data_batch(&self, tasks: &[crate::worker::DbWriteTask]) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let mut queries = Vec::new();
        let mut indexed_paths = Vec::new();
        let mut skipped_paths = Vec::new();
        let mut symbol_values = Vec::new();
        let mut chunk_values = Vec::new();
        let mut contains_values = Vec::new();
        let mut calls_values = Vec::new();
        let mut calls_nif_values = Vec::new();

        for task in tasks {
            match task {
                crate::worker::DbWriteTask::FileExtraction { path, content, extraction, .. } => {
                    indexed_paths.push(format!("'{}'", Self::escape_sql(path)));
                    let slug = extraction.project_slug.as_deref().unwrap_or("global");
                    for sym in &extraction.symbols {
                        let symbol_id = Self::symbol_id(slug, path, &sym.name);
                        let chunk_id = Self::chunk_id(&symbol_id);
                        let embedding_sql = if let Some(ref v) = sym.embedding {
                            format!("CAST({:?} AS FLOAT[384])", v)
                        } else {
                            "NULL".to_string()
                        };
                        let chunk_content = Self::build_chunk_content(path, sym, content);
                        let chunk_hash = Self::stable_content_hash(&chunk_content);

                        symbol_values.push(format!(
                            "('{}', '{}', '{}', {}, {}, {}, {}, '{}', {})",
                            Self::escape_sql(&symbol_id),
                            Self::escape_sql(&sym.name),
                            sym.kind,
                            sym.tested,
                            sym.is_public,
                            sym.is_nif,
                            sym.is_unsafe,
                            Self::escape_sql(slug),
                            embedding_sql
                        ));

                        contains_values.push(format!(
                            "('{}', '{}')",
                            Self::escape_sql(path),
                            Self::escape_sql(&symbol_id)
                        ));

                        chunk_values.push(format!(
                            "('{}', 'symbol', '{}', '{}', '{}', '{}', '{}', {}, {})",
                            Self::escape_sql(&chunk_id),
                            Self::escape_sql(&symbol_id),
                            Self::escape_sql(slug),
                            Self::escape_sql(&sym.kind),
                            Self::escape_sql(&chunk_content),
                            Self::escape_sql(&chunk_hash),
                            sym.start_line,
                            sym.end_line
                        ));
                    }

                    for relation in &extraction.relations {
                        let Some(table) = Self::relation_table(&relation.rel_type) else {
                            continue;
                        };

                        let relation_value = format!(
                            "('{}', '{}')",
                            Self::escape_sql(&Self::symbol_id(slug, path, &relation.from)),
                            Self::escape_sql(&Self::symbol_id(slug, path, &relation.to))
                        );

                        match table {
                            "CALLS" => calls_values.push(relation_value),
                            "CALLS_NIF" => calls_nif_values.push(relation_value),
                            _ => {}
                        }
                    }
                }
                crate::worker::DbWriteTask::FileSkipped { path, .. } => {
                    skipped_paths.push(format!("'{}'", Self::escape_sql(path)));
                }
                _ => {}
            }
        }

        if !indexed_paths.is_empty() {
            let indexed_filter = indexed_paths.join(",");
            queries.push(format!(
                "DELETE FROM CALLS WHERE source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})) OR target_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                indexed_filter, indexed_filter
            ));
            queries.push(format!(
                "DELETE FROM CALLS_NIF WHERE source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})) OR target_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                indexed_filter, indexed_filter
            ));
            queries.push(format!(
                "DELETE FROM Symbol WHERE id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                indexed_filter
            ));
            queries.push(format!(
                "DELETE FROM Chunk WHERE source_type = 'symbol' AND source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                indexed_filter
            ));
            queries.push(format!(
                "DELETE FROM CONTAINS WHERE source_id IN ({});",
                indexed_filter
            ));
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed' END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE \
                 WHERE path IN ({});",
                indexed_filter
            ));
        }
        if !skipped_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'skipped' END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE \
                 WHERE path IN ({});",
                skipped_paths.join(",")
            ));
        }
        for chunk in symbol_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug, embedding) VALUES {} ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe, project_slug=EXCLUDED.project_slug, embedding=EXCLUDED.embedding;",
                chunk.join(",")
            ));
        }
        for chunk in chunk_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO Chunk (id, source_type, source_id, project_slug, kind, content, content_hash, start_line, end_line) VALUES {} \
                 ON CONFLICT(id) DO UPDATE SET source_type=EXCLUDED.source_type, source_id=EXCLUDED.source_id, project_slug=EXCLUDED.project_slug, kind=EXCLUDED.kind, content=EXCLUDED.content, content_hash=EXCLUDED.content_hash, start_line=EXCLUDED.start_line, end_line=EXCLUDED.end_line;",
                chunk.join(",")
            ));
        }
        for chunk in contains_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO CONTAINS (source_id, target_id) VALUES {};",
                chunk.join(",")
            ));
        }
        for chunk in calls_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO CALLS (source_id, target_id) VALUES {};",
                chunk.join(",")
            ));
        }
        for chunk in calls_nif_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO CALLS_NIF (source_id, target_id) VALUES {};",
                chunk.join(",")
            ));
        }
        self.execute_batch(&queries)
    }

    pub fn fetch_pending_batch(&self, count: usize) -> Result<Vec<PendingFile>> {
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        let claim_id = chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;

            if !exec_fn(*guard, CString::new("BEGIN TRANSACTION;")?.as_ptr()) {
                return Err(anyhow!("Pending Fetch Error: BEGIN TRANSACTION failed"));
            }

            let claim_query = format!(
                "UPDATE File
                 SET status = 'indexing', worker_id = {}
                 WHERE path IN (
                    SELECT path FROM File
                    WHERE status = 'pending'
                    ORDER BY priority DESC
                    LIMIT {}
                 );",
                claim_id, count
            );

            if !exec_fn(*guard, CString::new(claim_query)?.as_ptr()) {
                let _ = exec_fn(*guard, CString::new("ROLLBACK;")?.as_ptr());
                return Err(anyhow!("Pending Fetch Error: claim update failed"));
            }
        }

        let fetch_query = format!(
            "SELECT path, COALESCE(trace_id, 'none'), priority
             FROM File
             WHERE status = 'indexing' AND worker_id = {}
             ORDER BY priority DESC",
            claim_id
        );
        let res = self.query_on_ctx(&fetch_query, *guard)?;

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Pending Fetch Error: COMMIT failed"));
            }
        }
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }
        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let files: Vec<PendingFile> = raw
            .into_iter()
            .filter_map(|row| {
                if row.len() >= 3 {
                    let priority = row[2]
                        .as_i64()
                        .or_else(|| row[2].as_str().and_then(|s| s.parse::<i64>().ok()))?;
                    Some(PendingFile {
                        path: row[0].as_str()?.to_string(),
                        trace_id: row[1].as_str()?.to_string(),
                        priority,
                    })
                } else {
                    None
                }
            })
            .collect();
        Ok(files)
    }

    pub fn fetch_unembedded_symbols(&self, count: usize) -> Result<Vec<(String, String)>> {
        let query = format!("SELECT id, name || ': ' || kind FROM Symbol WHERE embedding IS NULL LIMIT {}", count);
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        let res = self.query_on_ctx(&query, *guard)?;
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let symbols: Vec<(String, String)> = raw
            .into_iter()
            .filter_map(|row| {
                if row.len() >= 2 {
                    Some((row[0].as_str()?.to_string(), row[1].as_str()?.to_string()))
                } else {
                    None
                }
            })
            .collect();
        Ok(symbols)
    }

    pub fn ensure_embedding_model(
        &self,
        id: &str,
        kind: &str,
        model_name: &str,
        dimension: i64,
        version: &str,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self.execute(&format!(
            "INSERT INTO EmbeddingModel (id, kind, model_name, dimension, version, created_at) \
             VALUES ('{}', '{}', '{}', {}, '{}', {}) \
             ON CONFLICT(id) DO UPDATE SET \
                kind=EXCLUDED.kind, \
                model_name=EXCLUDED.model_name, \
                dimension=EXCLUDED.dimension, \
                version=EXCLUDED.version;",
            Self::escape_sql(id),
            Self::escape_sql(kind),
            Self::escape_sql(model_name),
            dimension,
            Self::escape_sql(version),
            now
        ))
    }

    pub fn fetch_unembedded_chunks(&self, model_id: &str, count: usize) -> Result<Vec<(String, String, String)>> {
        let query = format!(
            "SELECT c.id, c.content, c.content_hash \
             FROM Chunk c \
             LEFT JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{}' \
             WHERE ce.chunk_id IS NULL \
             LIMIT {}",
            Self::escape_sql(model_id),
            count
        );
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        let res = self.query_on_ctx(&query, *guard)?;
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let chunks: Vec<(String, String, String)> = raw
            .into_iter()
            .filter_map(|row| {
                if row.len() >= 3 {
                    Some((
                        row[0].as_str()?.to_string(),
                        row[1].as_str()?.to_string(),
                        row[2].as_str()?.to_string(),
                    ))
                } else {
                    None
                }
            })
            .collect();
        Ok(chunks)
    }

    pub fn update_symbol_embeddings(&self, updates: &[(String, Vec<f32>)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let mut queries = Vec::new();

        for chunk in updates.chunks(100) {
            for (id, vector) in chunk {
                let embedding_sql = format!("CAST({:?} AS FLOAT[384])", vector);
                queries.push(format!(
                    "UPDATE Symbol SET embedding = {} WHERE id = '{}';",
                    embedding_sql,
                    id.replace("'", "''")
                ));
            }
        }
        self.execute_batch(&queries)
    }

    pub fn update_chunk_embeddings(&self, model_id: &str, updates: &[(String, String, Vec<f32>)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let mut queries = Vec::new();
        let chunk_ids: Vec<String> = updates
            .iter()
            .map(|(chunk_id, _, _)| format!("'{}'", Self::escape_sql(chunk_id)))
            .collect();

        queries.push(format!(
            "DELETE FROM ChunkEmbedding WHERE model_id = '{}' AND chunk_id IN ({});",
            Self::escape_sql(model_id),
            chunk_ids.join(",")
        ));

        let values: Vec<String> = updates
            .iter()
            .map(|(chunk_id, source_hash, vector)| {
                format!(
                    "('{}', '{}', CAST({:?} AS FLOAT[384]), '{}')",
                    Self::escape_sql(chunk_id),
                    Self::escape_sql(model_id),
                    vector,
                    Self::escape_sql(source_hash)
                )
            })
            .collect();

        for chunk in values.chunks(100) {
            queries.push(format!(
                "INSERT INTO ChunkEmbedding (chunk_id, model_id, embedding, source_hash) VALUES {};",
                chunk.join(",")
            ));
        }

        self.execute_batch(&queries)
    }

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        self.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id) VALUES ('{}', '{}');",
            from, to
        ))
    }
}

impl GraphStore {
    fn upsert_file_queries(path: &str, project: &str, size: i64, mtime: i64, priority: i64) -> Vec<String> {
        vec![
            format!(
                "INSERT INTO Project (name) VALUES ('{}') ON CONFLICT DO NOTHING;",
                Self::escape_sql(project)
            ),
            format!(
                "INSERT INTO File (path, project_slug, size, mtime, status, priority, needs_reindex) VALUES ('{}', '{}', {}, {}, 'pending', {}, FALSE) \
                 ON CONFLICT(path) DO UPDATE SET \
                    project_slug=EXCLUDED.project_slug, \
                    size=EXCLUDED.size, \
                    mtime=EXCLUDED.mtime, \
                    status = CASE \
                        WHEN File.status = 'indexing' THEN File.status \
                        ELSE 'pending' \
                    END, \
                    priority = EXCLUDED.priority, \
                    worker_id = CASE \
                        WHEN File.status = 'indexing' THEN File.worker_id \
                        ELSE NULL \
                    END, \
                    needs_reindex = CASE \
                        WHEN File.status = 'indexing' \
                             AND (File.mtime IS DISTINCT FROM EXCLUDED.mtime OR File.size IS DISTINCT FROM EXCLUDED.size) \
                        THEN TRUE \
                        WHEN File.status = 'indexing' THEN File.needs_reindex \
                        ELSE FALSE \
                    END \
                 WHERE File.project_slug IS DISTINCT FROM EXCLUDED.project_slug \
                    OR File.mtime IS DISTINCT FROM EXCLUDED.mtime \
                    OR File.size IS DISTINCT FROM EXCLUDED.size \
                    OR File.status <> 'indexed' \
                    OR File.priority IS DISTINCT FROM EXCLUDED.priority;",
                Self::escape_sql(path),
                Self::escape_sql(project),
                size,
                mtime,
                priority
            ),
        ]
    }
}
