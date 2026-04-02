// Copyright (c) Didier Stadelmann. All rights reserved.

use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::file_ingress_guard::FileIngressRow;
use crate::graph::{ExecFunc, GraphStore, PendingFile};
use crate::queue::ProcessingMode;
use crate::watcher_probe;

#[derive(Debug, Clone, Copy)]
enum FileUpsertSource {
    Scan,
    HotDelta,
}

fn parse_i64_field(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|v| v.min(i64::MAX as u64) as i64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn parse_u64_field(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v.max(0) as u64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn parse_pending_file_row(row: Vec<serde_json::Value>) -> Option<PendingFile> {
    if row.len() < 6 {
        return None;
    }

    let priority = parse_i64_field(&row[2])?;
    let size_bytes = parse_u64_field(&row[3]).unwrap_or(0);
    let defer_count = parse_u64_field(&row[4]).unwrap_or(0).min(u32::MAX as u64) as u32;
    let last_deferred_at_ms = parse_i64_field(&row[5]);

    Some(PendingFile {
        path: row[0].as_str()?.to_string(),
        trace_id: row[1].as_str()?.to_string(),
        priority,
        size_bytes,
        defer_count,
        last_deferred_at_ms,
    })
}

fn parse_file_ingress_row(row: Vec<serde_json::Value>) -> Option<FileIngressRow> {
    if row.len() < 4 {
        return None;
    }

    Some(FileIngressRow {
        path: row[0].as_str()?.to_string(),
        status: row[1].as_str()?.to_string(),
        mtime: parse_i64_field(&row[2]).unwrap_or_default(),
        size: parse_i64_field(&row[3]).unwrap_or_default(),
    })
}

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
        let projects_root = std::env::var("AXON_PROJECTS_ROOT")
            .unwrap_or_else(|_| "/home/dstadel/projects".to_string());
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
        let docstring = symbol
            .docstring
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| format!("docstring: {}\n", value))
            .unwrap_or_default();

        format!(
            "symbol: {}\nkind: {}\nfile: {}\nlines: {}-{}\n{}\
\n{}",
            symbol.name, symbol.kind, path, symbol.start_line, symbol.end_line, docstring, snippet
        )
    }

    fn stable_content_hash(value: &str) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn derived_cleanup_queries(source_selector: &str) -> Vec<String> {
        let affected_symbols = format!(
            "SELECT target_id FROM CONTAINS WHERE source_id IN ({})",
            source_selector
        );
        let affected_symbol_anchors = format!(
            "SELECT DISTINCT anchor_id FROM GraphProjection WHERE anchor_type = 'symbol' AND target_id IN ({})",
            affected_symbols
        );

        vec![
            format!(
                "DELETE FROM GraphEmbedding WHERE \
                 (anchor_type = 'file' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({}));",
                source_selector, affected_symbols, affected_symbol_anchors
            ),
            format!(
                "DELETE FROM GraphProjectionState WHERE \
                 (anchor_type = 'file' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({}));",
                source_selector, affected_symbols, affected_symbol_anchors
            ),
            format!(
                "DELETE FROM GraphProjection WHERE \
                 (anchor_type = 'file' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({})) \
                 OR (anchor_type = 'symbol' AND anchor_id IN ({})) \
                 OR target_id IN ({});",
                source_selector, affected_symbols, affected_symbol_anchors, affected_symbols
            ),
        ]
    }

    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
        for (path, project, size, mtime) in file_paths {
            queries.extend(Self::upsert_file_queries(
                path,
                project,
                *size,
                *mtime,
                100,
                FileUpsertSource::Scan,
            ));
        }
        self.execute_batch(&queries)
    }

    pub fn upsert_hot_file(
        &self,
        path: &str,
        project: &str,
        size: i64,
        mtime: i64,
        priority: i64,
    ) -> Result<()> {
        let queries = Self::upsert_file_queries(
            path,
            project,
            size,
            mtime,
            priority,
            FileUpsertSource::HotDelta,
        );
        self.execute_batch(&queries)?;
        watcher_probe::record(
            "watcher.db_upsert",
            Some(Path::new(path)),
            format!(
                "project={} priority={} size={} mtime={}",
                project, priority, size, mtime
            ),
        );
        Ok(())
    }

    pub fn tombstone_missing_path(&self, path: &Path) -> Result<usize> {
        let path = path.to_string_lossy().to_string();
        let escaped = Self::escape_sql(&path);
        let prefix = Self::escape_sql(&format!("{}/%", path.trim_end_matches('/')));
        let selector = format!(
            "SELECT path FROM File WHERE path = '{}' OR path LIKE '{}'",
            escaped, prefix
        );
        let affected = self.query_count(&format!(
            "SELECT count(*) FROM ({}) AS tombstone_paths",
            selector
        ))?;

        if affected == 0 {
            return Ok(0);
        }

        let mut queries = Self::derived_cleanup_queries(&selector);
        queries.push(format!(
                "DELETE FROM CALLS WHERE source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})) \
                 OR target_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                selector, selector
            ));
        queries.push(format!(
                "DELETE FROM CALLS_NIF WHERE source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})) \
                 OR target_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                selector, selector
            ));
        queries.push(format!(
                "DELETE FROM ChunkEmbedding WHERE chunk_id IN (SELECT id FROM Chunk WHERE source_type = 'symbol' \
                 AND source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})));",
                selector
            ));
        queries.push(format!(
                "DELETE FROM Chunk WHERE source_type = 'symbol' \
                 AND source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                selector
            ));
        queries.push(format!(
                "DELETE FROM Symbol WHERE id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({}));",
                selector
            ));
        queries.push(format!("DELETE FROM CONTAINS WHERE source_id IN ({});", selector));
        queries.push(format!(
                "UPDATE File SET status = 'deleted', worker_id = NULL, needs_reindex = FALSE, status_reason = 'tombstoned_missing' \
                 WHERE path IN ({});",
                selector
            ));

        self.execute_batch(&queries)?;
        watcher_probe::record(
            "watcher.tombstoned",
            Some(path.as_ref()),
            format!("affected={}", affected),
        );
        Ok(affected as usize)
    }

    pub fn fetch_file_ingress_row(&self, path: &str) -> Result<Option<FileIngressRow>> {
        let escaped = Self::escape_sql(path);
        let raw = self.query_json(&format!(
            "SELECT path, status, mtime, size FROM File WHERE path = '{}' LIMIT 1",
            escaped
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows.into_iter().next().and_then(parse_file_ingress_row))
    }

    pub fn fetch_file_ingress_rows(&self, paths: &[String]) -> Result<Vec<FileIngressRow>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }

        let selector = paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(", ");

        let raw = self.query_json(&format!(
            "SELECT path, status, mtime, size FROM File WHERE path IN ({})",
            selector
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows.into_iter().filter_map(parse_file_ingress_row).collect())
    }

    pub fn insert_file_data_batch(&self, tasks: &[crate::worker::DbWriteTask]) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let mut queries = Vec::new();
        let mut deleted_paths = Vec::new();
        let mut indexed_paths = Vec::new();
        let mut degraded_paths = Vec::new();
        let mut skipped_paths = Vec::new();
        let mut symbol_values = Vec::new();
        let mut chunk_values = Vec::new();
        let mut contains_values = Vec::new();
        let mut calls_values = Vec::new();
        let mut calls_nif_values = Vec::new();

        for task in tasks {
            match task {
                crate::worker::DbWriteTask::FileExtraction {
                    path,
                    content,
                    extraction,
                    processing_mode,
                    ..
                } => {
                    if self.is_file_tombstoned(path)? {
                        deleted_paths.push(format!("'{}'", Self::escape_sql(path)));
                        continue;
                    }
                    let escaped_path = format!("'{}'", Self::escape_sql(path));
                    match processing_mode {
                        ProcessingMode::Full => indexed_paths.push(escaped_path.clone()),
                        ProcessingMode::StructureOnly => degraded_paths.push(escaped_path.clone()),
                    }
                    let slug = extraction.project_slug.as_deref().unwrap_or("global");
                    for sym in &extraction.symbols {
                        let symbol_id = Self::symbol_id(slug, path, &sym.name);
                        let chunk_id = Self::chunk_id(&symbol_id);
                        let embedding_sql = if let Some(ref v) = sym.embedding {
                            format!("CAST({:?} AS FLOAT[384])", v)
                        } else {
                            "NULL".to_string()
                        };
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

                        if matches!(processing_mode, ProcessingMode::Full) {
                            let chunk_content = Self::build_chunk_content(
                                path,
                                sym,
                                content.as_deref().unwrap_or_default(),
                            );
                            let chunk_hash = Self::stable_content_hash(&chunk_content);
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
                    if self.is_file_tombstoned(path)? {
                        deleted_paths.push(format!("'{}'", Self::escape_sql(path)));
                        continue;
                    }
                    skipped_paths.push(format!("'{}'", Self::escape_sql(path)));
                }
                _ => {}
            }
        }

        if !deleted_paths.is_empty() {
            queries.push(format!(
                "UPDATE File SET status = 'deleted', worker_id = NULL, needs_reindex = FALSE, defer_count = 0, last_deferred_at_ms = NULL WHERE path IN ({});",
                deleted_paths.join(",")
            ));
        }
        let mut processed_paths = indexed_paths.clone();
        processed_paths.extend(degraded_paths.clone());

        if !processed_paths.is_empty() {
            let indexed_filter = processed_paths.join(",");
            queries.extend(Self::derived_cleanup_queries(&indexed_filter));
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
                "DELETE FROM ChunkEmbedding WHERE chunk_id IN (SELECT id FROM Chunk WHERE source_type = 'symbol' AND source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})));",
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
        }
        if !indexed_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed' END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = NULL, \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE NULL END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL \
                 WHERE path IN ({});",
                indexed_paths.join(",")
            ));
        }
        if !degraded_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed_degraded' END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = 'degraded_structure_only', \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE 'degraded_structure_only' END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL \
                 WHERE path IN ({});",
                degraded_paths.join(",")
            ));
        }
        if !skipped_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'skipped' END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = 'worker_skipped_file', \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE 'worker_skipped_file' END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL \
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
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
                 SET status = 'indexing', worker_id = {}, defer_count = 0, last_deferred_at_ms = NULL
                 WHERE path IN (
                    SELECT path FROM File
                    WHERE status = 'pending'
                    ORDER BY priority DESC, COALESCE(defer_count, 0) DESC, COALESCE(last_deferred_at_ms, 9223372036854775807) ASC, size ASC
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
            "SELECT path, COALESCE(trace_id, 'none'), priority, COALESCE(size, 0), COALESCE(defer_count, 0), last_deferred_at_ms
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
        let files: Vec<PendingFile> = raw.into_iter().filter_map(parse_pending_file_row).collect();
        Ok(files)
    }

    pub fn fetch_pending_candidates(&self, count: usize) -> Result<Vec<PendingFile>> {
        let query = format!(
            "SELECT path, COALESCE(trace_id, 'none'), priority, COALESCE(size, 0), COALESCE(defer_count, 0), last_deferred_at_ms
             FROM File
             WHERE status = 'pending'
             ORDER BY priority DESC, COALESCE(defer_count, 0) DESC, COALESCE(last_deferred_at_ms, 9223372036854775807) ASC, size ASC
             LIMIT {}",
            count
        );
        let raw = self.query_json(&query)?;
        if raw == "[]" || raw.is_empty() {
            return Ok(vec![]);
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw)?;
        Ok(rows
            .into_iter()
            .filter_map(parse_pending_file_row)
            .collect())
    }

    pub fn claim_pending_paths(&self, paths: &[String]) -> Result<Vec<PendingFile>> {
        if paths.is_empty() {
            return Ok(vec![]);
        }

        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let claim_id = chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
        let path_list = paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(",");

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;

            if !exec_fn(*guard, CString::new("BEGIN TRANSACTION;")?.as_ptr()) {
                return Err(anyhow!("Claim Paths Error: BEGIN TRANSACTION failed"));
            }

            let claim_query = format!(
                "UPDATE File
                 SET status = 'indexing', worker_id = {}, defer_count = 0, last_deferred_at_ms = NULL
                 WHERE status = 'pending' AND path IN ({});",
                claim_id, path_list
            );

            if !exec_fn(*guard, CString::new(claim_query)?.as_ptr()) {
                let _ = exec_fn(*guard, CString::new("ROLLBACK;")?.as_ptr());
                return Err(anyhow!("Claim Paths Error: claim update failed"));
            }
        }

        let fetch_query = format!(
            "SELECT path, COALESCE(trace_id, 'none'), priority, COALESCE(size, 0), COALESCE(defer_count, 0), last_deferred_at_ms
             FROM File
             WHERE status = 'indexing' AND worker_id = {}
             ORDER BY priority DESC, size ASC",
            claim_id
        );
        let res = self.query_on_ctx(&fetch_query, *guard)?;

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Claim Paths Error: COMMIT failed"));
            }
        }
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        Ok(raw.into_iter().filter_map(parse_pending_file_row).collect())
    }

    pub fn mark_file_oversized_for_current_budget(&self, path: &str) -> Result<()> {
        self.execute(&format!(
            "UPDATE File \
             SET status = 'oversized_for_current_budget', \
                 worker_id = NULL, \
                 last_error_reason = 'estimated cost exceeds current budget envelope', \
                 status_reason = 'oversized_for_current_budget', \
                 defer_count = 0, \
                 last_deferred_at_ms = NULL \
             WHERE path = '{}';",
            Self::escape_sql(path)
        ))
    }

    pub fn mark_pending_files_deferred(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let path_list = paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(",");

        self.execute(&format!(
            "UPDATE File \
             SET defer_count = COALESCE(defer_count, 0) + 1, \
                 last_deferred_at_ms = {} \
             WHERE status = 'pending' AND path IN ({});",
            now_ms, path_list
        ))
    }

    pub fn requeue_claimed_file(&self, path: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&format!(
            "UPDATE File \
             SET status = 'pending', \
                 worker_id = NULL, \
                 last_error_reason = NULL, \
                 status_reason = 'manual_or_system_requeue', \
                 defer_count = COALESCE(defer_count, 0) + 1, \
                 last_deferred_at_ms = {} \
             WHERE path = '{}' AND status = 'indexing';",
            now_ms,
            Self::escape_sql(path)
        ))
    }

    pub fn fetch_unembedded_symbols(&self, count: usize) -> Result<Vec<(String, String)>> {
        let query = format!(
            "SELECT id, name || ': ' || kind FROM Symbol WHERE embedding IS NULL LIMIT {}",
            count
        );
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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

    pub fn fetch_unembedded_chunks(
        &self,
        model_id: &str,
        count: usize,
    ) -> Result<Vec<(String, String, String)>> {
        let query = format!(
            "SELECT c.id, c.content, c.content_hash \
             FROM Chunk c \
             LEFT JOIN ChunkEmbedding ce ON ce.chunk_id = c.id AND ce.model_id = '{}' \
             WHERE ce.chunk_id IS NULL OR ce.source_hash <> c.content_hash \
             LIMIT {}",
            Self::escape_sql(model_id),
            count
        );
        let guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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

    pub fn update_chunk_embeddings(
        &self,
        model_id: &str,
        updates: &[(String, String, Vec<f32>)],
    ) -> Result<()> {
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
    fn upsert_file_queries(
        path: &str,
        project: &str,
        size: i64,
        mtime: i64,
        priority: i64,
        source: FileUpsertSource,
    ) -> Vec<String> {
        let metadata_changed_reason = match source {
            FileUpsertSource::Scan => "metadata_changed_scan",
            FileUpsertSource::HotDelta => "metadata_changed_hot_delta",
        };

        vec![
            format!(
                "INSERT INTO Project (name) VALUES ('{}') ON CONFLICT DO NOTHING;",
                Self::escape_sql(project)
            ),
            format!(
                "INSERT INTO File (path, project_slug, size, mtime, status, priority, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms) VALUES ('{}', '{}', {}, {}, 'pending', {}, FALSE, NULL, 'discovered_new', 0, NULL) \
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
                    last_error_reason = CASE \
                        WHEN File.status = 'indexing' THEN File.last_error_reason \
                        ELSE NULL \
                    END, \
                    status_reason = CASE \
                        WHEN File.status = 'indexing' THEN File.status_reason \
                        WHEN File.mtime IS DISTINCT FROM EXCLUDED.mtime OR File.size IS DISTINCT FROM EXCLUDED.size THEN '{}' \
                        WHEN File.project_slug IS DISTINCT FROM EXCLUDED.project_slug THEN 'manual_or_system_requeue' \
                        WHEN File.priority IS DISTINCT FROM EXCLUDED.priority THEN 'manual_or_system_requeue' \
                        ELSE COALESCE(File.status_reason, 'manual_or_system_requeue') \
                    END, \
                    defer_count = CASE \
                        WHEN File.status = 'indexing' THEN File.defer_count \
                        ELSE 0 \
                    END, \
                    last_deferred_at_ms = CASE \
                        WHEN File.status = 'indexing' THEN File.last_deferred_at_ms \
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
                priority,
                metadata_changed_reason
            ),
        ]
    }

    fn is_file_tombstoned(&self, path: &str) -> Result<bool> {
        Ok(self.query_count(&format!(
            "SELECT count(*) FROM File WHERE path = '{}' AND status = 'deleted'",
            Self::escape_sql(path)
        ))? > 0)
    }
}
