use std::path::Path;

use anyhow::Result;

use crate::file_ingress_guard::FileIngressRow;
use crate::graph::GraphStore;
use crate::ingress_buffer::{IngressDrainBatch, IngressPromotionStats, IngressSource};
use crate::postgres::age::cypher_cascade_delete_symbols_for_files;
use crate::watcher_probe;

use super::{parse_file_ingress_row, FileUpsertSource, IgnoreReconcileStats};

/// REQ-AXO-248 / MIL-AXO-015 B.2: under PG, mirror the SQL cascade in
/// Apache AGE so the relation edges (CONTAINS, CALLS, CALLS_NIF) are
/// removed from the canonical graph store. The cascade is paired with
/// the existing SQL chain — both fire under PG so AGE installs that
/// are still being seeded retain a working SQL fallback (REQ-AXO-216
/// drops the SQL relation tables only after the writers + readers
/// are end-to-end on AGE).
///
/// Empty `paths` is a no-op. AGE errors are downgraded to a warning
/// rather than aborting the SQL cascade — the caller's invariant is
/// "the file is gone from the SQL graph"; AGE drift is corrected on
/// the next ingest cycle.
fn run_age_symbol_cascade_under_pg(store: &GraphStore, paths: &[&str]) {
    if !store.is_postgres_backend() || paths.is_empty() {
        return;
    }
    let cypher = match cypher_cascade_delete_symbols_for_files("axon_graph", paths) {
        Ok(Some(sql)) => sql,
        Ok(None) => return,
        Err(e) => {
            log::warn!(
                "file_ingress AGE cascade build failed (paths={}): {}",
                paths.len(),
                e
            );
            return;
        }
    };
    if let Err(e) = store.execute(&cypher) {
        log::warn!(
            "file_ingress AGE cascade execute failed (paths={}): {}",
            paths.len(),
            e
        );
    }
}

/// Resolve a SQL `SELECT path FROM ...` selector to the list of
/// matching file paths. Used to feed the AGE cascade above which
/// requires concrete path values for the `f.path IN [...]` filter.
fn resolve_paths_from_selector(store: &GraphStore, selector: &str) -> Vec<String> {
    let raw = match store.query_json(selector) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("file_ingress: selector resolve failed: {}", e);
            return Vec::new();
        }
    };
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
    rows.into_iter()
        .filter_map(|row| {
            row.into_iter()
                .next()
                .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| Some(v.to_string())))
        })
        .collect()
}

impl GraphStore {
    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let batch = file_paths
            .iter()
            .map(|(path, project, size, mtime)| {
                (
                    path.clone(),
                    project.clone(),
                    *size,
                    *mtime,
                    100,
                    FileUpsertSource::Scan,
                )
            })
            .collect::<Vec<_>>();
        let queries = Self::bulk_upsert_file_queries(&batch);
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

    pub fn promote_ingress_batch(
        &self,
        batch: &IngressDrainBatch,
    ) -> Result<IngressPromotionStats> {
        let file_rows = batch
            .files
            .iter()
            .map(|file| {
                let source = match file.source {
                    IngressSource::Watcher => FileUpsertSource::HotDelta,
                    IngressSource::Scan => FileUpsertSource::Scan,
                };
                (
                    file.path.clone(),
                    file.project_code.clone(),
                    file.size,
                    file.mtime,
                    file.priority,
                    source,
                )
            })
            .collect::<Vec<_>>();
        let queries = Self::bulk_upsert_file_queries(&file_rows);

        if !queries.is_empty() {
            self.execute_batch(&queries)?;
        }

        let mut promoted_tombstones = 0usize;
        for path in &batch.tombstones {
            promoted_tombstones += self.tombstone_missing_path(Path::new(path))?;
        }

        Ok(IngressPromotionStats {
            promoted_files: batch.files.len(),
            promoted_tombstones,
        })
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

        // REQ-AXO-248 / MIL-AXO-015 B.2: mirror the cascade in Apache AGE
        // under PG so the canonical graph store loses these symbols + their
        // edges. Runs BEFORE the SQL chain so a partial failure leaves the
        // SQL DELETE chain to resolve the rest atomically via execute_batch.
        let cascading_paths = resolve_paths_from_selector(self, &selector);
        let cascading_path_refs: Vec<&str> = cascading_paths.iter().map(String::as_str).collect();
        run_age_symbol_cascade_under_pg(self, &cascading_path_refs);

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
        queries.push(format!(
            "DELETE FROM CONTAINS WHERE source_id IN ({});",
            selector
        ));
        queries.push(format!(
            "UPDATE File SET status = 'deleted', worker_id = NULL, needs_reindex = FALSE, status_reason = 'tombstoned_missing', file_stage = 'deleted', graph_ready = FALSE, vector_ready = FALSE \
             WHERE path IN ({});",
            selector
        ));
        queries.push(format!(
            "DELETE FROM FileVectorizationQueue WHERE file_path IN ({});",
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

    pub fn reconcile_ignore_rules_for_scope(
        &self,
        scope_root: &Path,
        scanner: &crate::scanner::Scanner,
    ) -> Result<IgnoreReconcileStats> {
        if !crate::config::CONFIG.indexing.ignore_reconcile_enabled {
            return Ok(IgnoreReconcileStats::default());
        }

        let dry_run = crate::config::CONFIG.indexing.ignore_reconcile_dry_run;
        let scope = std::fs::canonicalize(scope_root).unwrap_or_else(|_| scope_root.to_path_buf());
        let scope_str = scope.to_string_lossy().to_string();
        let prefix = Self::escape_sql(&format!("{}/%", scope_str.trim_end_matches('/')));
        let escaped_scope = Self::escape_sql(&scope_str);

        let raw = self.query_json(&format!(
            "SELECT path, COALESCE(project_code, ''), status FROM File \
             WHERE path = '{}' OR path LIKE '{}';",
            escaped_scope, prefix
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();

        let mut newly_ignored: Vec<String> = Vec::new();
        let mut newly_included: Vec<String> = Vec::new();

        for row in &rows {
            if row.len() < 3 {
                continue;
            }
            let Some(path) = row[0].as_str() else {
                continue;
            };
            let status = row[2].as_str().unwrap_or("unknown");
            let path_obj = Path::new(path);
            let eligible = scanner.should_process_path(path_obj);

            if !eligible && status != "deleted" && status != "ignored_pending_purge" {
                newly_ignored.push(path.to_string());
            } else if eligible && (status == "deleted" || status == "ignored_pending_purge") {
                newly_included.push(path.to_string());
            }
        }

        if dry_run {
            watcher_probe::record(
                "ignore.reconcile",
                Some(scope.as_path()),
                format!(
                    "mode=dry_run scanned={} newly_ignored={} newly_included={}",
                    rows.len(),
                    newly_ignored.len(),
                    newly_included.len()
                ),
            );
            return Ok(IgnoreReconcileStats {
                scanned: rows.len(),
                newly_ignored: newly_ignored.len(),
                newly_included: newly_included.len(),
                dry_run: true,
            });
        }

        if !newly_ignored.is_empty() {
            for chunk in newly_ignored.chunks(300) {
                // REQ-AXO-248 / MIL-AXO-015 B.2: mirror cascade in AGE
                // under PG. Same chunk slice feeds the SQL chain below.
                let chunk_refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
                run_age_symbol_cascade_under_pg(self, &chunk_refs);
                let selector = chunk
                    .iter()
                    .map(|p| format!("'{}'", Self::escape_sql(p)))
                    .collect::<Vec<_>>()
                    .join(",");
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
                queries.push(format!(
                    "DELETE FROM CONTAINS WHERE source_id IN ({});",
                    selector
                ));
                queries.push(format!(
                    "DELETE FROM FileVectorizationQueue WHERE file_path IN ({});",
                    selector
                ));
                queries.push(format!(
                    "UPDATE File SET status = 'ignored_pending_purge', worker_id = NULL, needs_reindex = FALSE, \
                     status_reason = 'ignore_rules_changed', file_stage = 'deleted', graph_ready = FALSE, vector_ready = FALSE \
                     WHERE path IN ({});",
                    selector
                ));
                self.execute_batch(&queries)?;
            }
        }

        if !newly_included.is_empty() {
            let mut queries = Vec::new();
            for path in &newly_included {
                let path_obj = Path::new(path);
                let metadata = match std::fs::metadata(path_obj) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if !metadata.is_file() {
                    continue;
                }
                let project = match scanner.project_code_for_path(self, path_obj) {
                    Ok(project_code) => project_code,
                    Err(_) => continue,
                };
                let size = metadata.len() as i64;
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                queries.extend(Self::upsert_file_queries(
                    path,
                    &project,
                    size,
                    mtime,
                    900,
                    FileUpsertSource::HotDelta,
                ));
            }
            if !queries.is_empty() {
                self.execute_batch(&queries)?;
            }
        }

        watcher_probe::record(
            "ignore.reconcile",
            Some(scope.as_path()),
            format!(
                "mode=apply scanned={} newly_ignored={} newly_included={}",
                rows.len(),
                newly_ignored.len(),
                newly_included.len()
            ),
        );

        Ok(IgnoreReconcileStats {
            scanned: rows.len(),
            newly_ignored: newly_ignored.len(),
            newly_included: newly_included.len(),
            dry_run: false,
        })
    }

    pub fn fetch_file_ingress_row(&self, path: &str) -> Result<Option<FileIngressRow>> {
        let escaped = Self::escape_sql(path);
        let raw = self.query_json(&format!(
            "SELECT path, status, mtime, size, COALESCE(file_stage, ''), COALESCE(status_reason, ''), COALESCE(graph_ready, FALSE) \
             FROM File WHERE path = '{}' LIMIT 1",
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
            "SELECT path, status, mtime, size, COALESCE(file_stage, ''), COALESCE(status_reason, ''), COALESCE(graph_ready, FALSE) \
             FROM File WHERE path IN ({})",
            selector
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(parse_file_ingress_row)
            .collect())
    }
}
