use std::ffi::CString;

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::graph::{ExecFunc, GraphStore, PendingFile};

impl GraphStore {
    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
        for (path, project, size, mtime) in file_paths {
            queries.push(format!(
                "INSERT INTO Project (name) VALUES ('{}') ON CONFLICT DO NOTHING;",
                project.replace("'", "''")
            ));
            queries.push(format!(
                "INSERT INTO File (path, project_slug, size, mtime, status, priority) VALUES ('{}', '{}', {}, {}, 'pending', 100) ON CONFLICT(path) DO UPDATE SET mtime=EXCLUDED.mtime;",
                path.replace("'", "''"),
                project.replace("'", "''"),
                size,
                mtime
            ));
        }
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

        for task in tasks {
            match task {
                crate::worker::DbWriteTask::FileExtraction { path, extraction, .. } => {
                    indexed_paths.push(format!("'{}'", path.replace("'", "''")));
                    let slug = extraction.project_slug.as_deref().unwrap_or("global");
                    for sym in &extraction.symbols {
                        let embedding_sql = if let Some(ref v) = sym.embedding {
                            format!("CAST({:?} AS FLOAT[384])", v)
                        } else {
                            "NULL".to_string()
                        };

                        symbol_values.push(format!(
                            "('{}::{}', '{}', '{}', {}, {}, {}, {}, '{}', {})",
                            slug.replace("'", "''"),
                            sym.name.replace("'", "''"),
                            sym.name.replace("'", "''"),
                            sym.kind,
                            sym.tested,
                            sym.is_public,
                            sym.is_nif,
                            sym.is_unsafe,
                            slug.replace("'", "''"),
                            embedding_sql
                        ));
                    }
                }
                crate::worker::DbWriteTask::FileSkipped { path, .. } => {
                    skipped_paths.push(format!("'{}'", path.replace("'", "''")));
                }
                _ => {}
            }
        }

        if !indexed_paths.is_empty() {
            queries.push(format!(
                "UPDATE File SET status = 'indexed', worker_id = NULL WHERE path IN ({});",
                indexed_paths.join(",")
            ));
        }
        if !skipped_paths.is_empty() {
            queries.push(format!(
                "UPDATE File SET status = 'skipped', worker_id = NULL WHERE path IN ({});",
                skipped_paths.join(",")
            ));
        }
        for chunk in symbol_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug, embedding) VALUES {} ON CONFLICT(id) DO UPDATE SET embedding=EXCLUDED.embedding;",
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

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        self.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id) VALUES ('{}', '{}');",
            from, to
        ))
    }
}
