// Copyright (c) Didier Stadelmann. All rights reserved.

use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::code_chunker::build_symbol_chunks;
use crate::embedding_contract::{CHUNK_MODEL_ID as CHUNK_EMBEDDING_MODEL_ID, DIMENSION};
use crate::graph::{ExecFunc, GraphStore, PendingFile};
use crate::queue::ProcessingMode;
use crate::runtime_mode::graph_embeddings_enabled;
use crate::runtime_mode::AxonRuntimeMode;
use crate::service_guard;

const DEFAULT_GRAPH_EMBEDDING_RADIUS: i64 = 2;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS: i64 = 5_000;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT: i64 = 2;
static FILE_VECTORIZATION_CLAIM_SEQ: AtomicU64 = AtomicU64::new(1);
const CHUNK_EMBEDDING_UPSERT_BATCH_ROWS: usize = 500;

pub(crate) mod chunk_content_archiver;
mod file_ingress;
mod graph_projection_queue;
pub(crate) mod parquet_chunk_content_store;
mod sql_helpers;
mod types;
mod vector_runtime;
mod vectorization_queue;

use sql_helpers::{
    dedup_file_batch_rows, file_vectorization_queue_upsert_if_needed,
    graph_projection_queue_upsert, graph_projection_queue_upsert_if_needed_for_file,
    hourly_bucket_start_ms, insert_unique_relation_queries, next_vector_persist_outbox_claim_token,
    orphaned_file_vectorization_candidates_query, orphaned_file_vectorization_requeue_sql,
    parse_file_ingress_row, parse_i64_field, parse_pending_file_row, parse_u64_field,
    replace_relation_queries, sort_and_dedup_sql_tuples,
};
pub use types::{
    FileLifecycleEvent, FileVectorizationLeaseSnapshot, FileVectorizationWork, GraphProjectionWork,
    IgnoreReconcileStats, VectorBatchRun, VectorLaneStateRecord, VectorPersistOutboxPayload,
    VectorPersistOutboxUpdate, VectorPersistOutboxWork, VectorWorkerFault,
};

#[derive(Debug, Clone, Copy)]
enum FileUpsertSource {
    Scan,
    HotDelta,
}

impl GraphStore {
    fn claimable_file_vectorization_candidates_query(now_ms: i64) -> String {
        format!(
            "SELECT fq.file_path \
             FROM FileVectorizationQueue fq \
             LEFT JOIN File f ON f.path = fq.file_path \
             WHERE fq.status IN ('queued', 'paused_for_interactive_priority') \
               AND COALESCE(f.vector_ready, FALSE) = FALSE \
               AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
               AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND (fq.next_eligible_at_ms IS NULL OR fq.next_eligible_at_ms <= {})",
            now_ms
        )
    }

    pub fn append_file_lifecycle_events(&self, events: &[FileLifecycleEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }

        let values = events
            .iter()
            .map(|event| {
                format!(
                    "('{}', '{}', '{}', '{}', {}, {}, {}, {}, {})",
                    Self::escape_sql(&event.file_path),
                    Self::escape_sql(&event.project_code),
                    Self::escape_sql(&event.stage),
                    Self::escape_sql(&event.status),
                    event
                        .reason
                        .as_ref()
                        .map(|reason| format!("'{}'", Self::escape_sql(reason)))
                        .unwrap_or_else(|| "NULL".to_string()),
                    event.at_ms,
                    event
                        .worker_id
                        .map(|worker_id| worker_id.to_string())
                        .unwrap_or_else(|| "NULL".to_string()),
                    event
                        .trace_id
                        .as_ref()
                        .map(|trace_id| format!("'{}'", Self::escape_sql(trace_id)))
                        .unwrap_or_else(|| "NULL".to_string()),
                    event
                        .run_id
                        .as_ref()
                        .map(|run_id| format!("'{}'", Self::escape_sql(run_id)))
                        .unwrap_or_else(|| "NULL".to_string())
                )
            })
            .collect::<Vec<_>>();

        self.execute(&format!(
            "INSERT INTO FileLifecycleEvent (file_path, project_code, stage, status, reason, at_ms, worker_id, trace_id, run_id) VALUES {};",
            values.join(",")
        ))
    }

    pub fn fetch_file_project_metadata(
        &self,
        paths: &[String],
    ) -> Result<std::collections::HashMap<String, (String, Option<i64>, Option<String>)>> {
        if paths.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let selector = paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(", ");
        let raw = self.query_json(&format!(
            "SELECT path, COALESCE(project_code, ''), worker_id, trace_id \
             FROM File \
             WHERE path IN ({})",
            selector
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut result = std::collections::HashMap::new();
        for row in rows {
            let Some(path) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let project_code = row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let worker_id = row.get(2).and_then(parse_i64_field);
            let trace_id = row
                .get(3)
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());
            result.insert(path.to_string(), (project_code, worker_id, trace_id));
        }
        Ok(result)
    }

    pub fn log_optimizer_decision(
        &self,
        decision_id: &str,
        at_ms: i64,
        mode: &str,
        host_snapshot_json: &str,
        policy_snapshot_json: &str,
        signal_snapshot_json: &str,
        analytics_snapshot_json: &str,
        action_profile_id: &str,
        decision_json: &str,
        constraints_triggered_json: &str,
        would_apply: bool,
        applied: bool,
        evaluation_window_start_ms: i64,
        evaluation_window_end_ms: i64,
    ) -> Result<()> {
        self.execute(&format!(
            "INSERT OR REPLACE INTO OptimizerDecisionLog (decision_id, at_ms, mode, host_snapshot_json, policy_snapshot_json, signal_snapshot_json, analytics_snapshot_json, action_profile_id, decision_json, constraints_triggered_json, would_apply, applied, evaluation_window_start_ms, evaluation_window_end_ms) \
             VALUES ('{}', {}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, {}, {});",
            Self::escape_sql(decision_id),
            at_ms,
            Self::escape_sql(mode),
            Self::escape_sql(host_snapshot_json),
            Self::escape_sql(policy_snapshot_json),
            Self::escape_sql(signal_snapshot_json),
            Self::escape_sql(analytics_snapshot_json),
            Self::escape_sql(action_profile_id),
            Self::escape_sql(decision_json),
            Self::escape_sql(constraints_triggered_json),
            if would_apply { "TRUE" } else { "FALSE" },
            if applied { "TRUE" } else { "FALSE" },
            evaluation_window_start_ms,
            evaluation_window_end_ms
        ))
    }

    pub fn log_reward_observation(
        &self,
        decision_id: &str,
        observed_at_ms: i64,
        window_start_ms: i64,
        window_end_ms: i64,
        reward_json: &str,
        throughput_chunks_per_hour: f64,
        throughput_files_per_hour: f64,
        constraint_violations_json: &str,
        pressure_summary_json: &str,
    ) -> Result<()> {
        self.execute(&format!(
            "INSERT INTO RewardObservationLog (decision_id, observed_at_ms, window_start_ms, window_end_ms, reward_json, throughput_chunks_per_hour, throughput_files_per_hour, constraint_violations_json, pressure_summary_json) \
             VALUES ('{}', {}, {}, {}, '{}', {}, {}, '{}', '{}');",
            Self::escape_sql(decision_id),
            observed_at_ms,
            window_start_ms,
            window_end_ms,
            Self::escape_sql(reward_json),
            throughput_chunks_per_hour,
            throughput_files_per_hour,
            Self::escape_sql(constraint_violations_json),
            Self::escape_sql(pressure_summary_json)
        ))
    }

    fn next_file_vectorization_claim_token(now_ms: i64) -> String {
        let seq = FILE_VECTORIZATION_CLAIM_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("fvq-{}-{}", now_ms, seq)
    }

    fn canonicalize_sql_text(value: &str) -> String {
        value.replace('\0', " ")
    }

    fn escape_sql(value: &str) -> String {
        Self::canonicalize_sql_text(value).replace('\'', "''")
    }

    fn symbol_id(project_code: &str, path: &str, name: &str) -> String {
        if Self::is_globally_qualified_symbol(name) {
            format!("{}::{}", project_code, name)
        } else {
            format!(
                "{}::{}::{}",
                project_code,
                Self::symbol_path_namespace(path),
                name
            )
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

    fn chunk_part_id(symbol_id: &str, part_index: usize, part_count: usize) -> String {
        if part_count <= 1 {
            return Self::chunk_id(symbol_id);
        }
        format!("{}::chunk::part-{:02}", symbol_id, part_index)
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

    fn build_chunk_content(
        symbol: &crate::parser::Symbol,
        content: &str,
    ) -> Vec<crate::code_chunker::DerivedCodeChunk> {
        build_symbol_chunks(symbol, content)
    }

    fn stable_content_hash(value: &str) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        Self::canonicalize_sql_text(value).hash(&mut hasher);
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

    pub fn oldest_graph_pending_age_ms(&self, now_ms: i64) -> Result<u64> {
        let query = "SELECT min(COALESCE(first_seen_at_ms, last_state_change_at_ms, mtime, 0)) \
             FROM File \
             WHERE status IN ('pending', 'indexing') \
               AND COALESCE(graph_ready, FALSE) = FALSE \
               AND COALESCE(status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget', 'ignored_pending_purge')";
        let oldest_ms = self.query_single_i64_writer(query)?.unwrap_or(0).max(0);
        Ok(now_ms.saturating_sub(oldest_ms) as u64)
    }

    pub fn oldest_semantic_pending_age_ms(&self, now_ms: i64) -> Result<u64> {
        let query = format!(
            "SELECT min(COALESCE(f.graph_ready_at_ms, f.last_state_change_at_ms, f.mtime, 0)) \
             FROM File f \
             WHERE COALESCE(f.graph_ready, FALSE) = TRUE \
               AND COALESCE(f.vector_ready, FALSE) = FALSE \
               AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget', 'ignored_pending_purge') \
               AND EXISTS ( \
                   SELECT 1 \
                   FROM Chunk c \
                   JOIN CONTAINS co ON co.target_id = c.source_id \
                   LEFT JOIN ChunkEmbedding ce \
                     ON ce.chunk_id = c.id \
                    AND ce.model_id = '{}' \
                    AND ce.source_hash = c.content_hash \
                   WHERE co.source_id = f.path \
                     AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
               )",
            Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
        );
        let oldest_ms = self.query_single_i64_writer(&query)?.unwrap_or(0).max(0);
        Ok(now_ms.saturating_sub(oldest_ms) as u64)
    }

    pub fn fetch_graph_projection_state(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
    ) -> Result<Option<(String, String)>> {
        let query = format!(
            "SELECT source_signature, projection_version \
             FROM GraphProjectionState \
             WHERE anchor_type = '{}' \
               AND anchor_id = '{}' \
               AND radius = {} \
             LIMIT 1",
            Self::escape_sql(anchor_type),
            Self::escape_sql(anchor_id),
            radius
        );
        let raw = self.query_json(&query)?;

        if raw == "[]" || raw.is_empty() {
            return Ok(None);
        }
        let mut rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let row = rows.pop();
        if let Some(row) = row {
            let Some(source_signature) = row.first().and_then(|value| value.as_str()) else {
                return Ok(None);
            };
            let Some(projection_version) = row.get(1).and_then(|value| value.as_str()) else {
                return Ok(None);
            };
            return Ok(Some((
                source_signature.to_string(),
                projection_version.to_string(),
            )));
        }
        Ok(None)
    }

    pub fn has_matching_graph_projection_embedding(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
        model_id: &str,
        source_signature: &str,
        projection_version: &str,
    ) -> Result<bool> {
        let query = format!(
            "SELECT source_signature, projection_version \
             FROM GraphEmbedding \
             WHERE anchor_type = '{}' \
               AND anchor_id = '{}' \
               AND radius = {} \
               AND model_id = '{}' \
             LIMIT 1",
            Self::escape_sql(anchor_type),
            Self::escape_sql(anchor_id),
            radius,
            Self::escape_sql(model_id)
        );
        let raw = self.query_json(&query)?;
        if raw == "[]" || raw.is_empty() {
            return Ok(false);
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(false);
        };
        let Some(existing_signature) = row.first().and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        let Some(existing_projection_version) = row.get(1).and_then(|value| value.as_str()) else {
            return Ok(false);
        };
        Ok(existing_signature == source_signature
            && existing_projection_version == projection_version)
    }

    pub fn insert_file_data_batch(&self, tasks: &[crate::worker::DbWriteTask]) -> Result<()> {
        self.insert_file_data_batch_with_vectorization_policy(
            tasks,
            AxonRuntimeMode::from_env().background_vectorization_enabled(),
        )
    }

    pub(crate) fn insert_file_data_batch_with_vectorization_policy(
        &self,
        tasks: &[crate::worker::DbWriteTask],
        enqueue_vectorization: bool,
    ) -> Result<()> {
        if tasks.is_empty() {
            return Ok(());
        }
        let mut queries = Vec::new();
        let mut deleted_paths = Vec::new();
        let mut indexed_paths = Vec::new();
        let mut indexed_paths_raw = Vec::new();
        let mut degraded_paths = Vec::new();
        let mut degraded_paths_raw = Vec::new();
        let mut skipped_paths = Vec::new();
        let mut seen_symbols = std::collections::HashSet::new();
        let mut seen_calls = std::collections::HashSet::new();
        let mut seen_calls_nif = std::collections::HashSet::new();
        let mut symbol_values = Vec::new();
        let mut chunk_values = Vec::new();
        // DEC-AXO-071 H.2: when inline mode is enabled, capture chunk
        // metadata so we can embed inline immediately after the chunk
        // INSERT execute_batch returns. Empty when
        // AXON_VECTOR_PIPELINE_INLINE is unset or when the caller passed
        // enqueue_vectorization=false.
        let inline_enabled = enqueue_vectorization
            && crate::embedder::inline_embed::inline_pipeline_enabled();
        let mut inline_chunks: Vec<(String, String, String, String)> = Vec::new();
        let mut contains_values = Vec::new();
        let mut calls_values = Vec::new();
        let mut calls_nif_values = Vec::new();
        let mut file_vectorization_paths = Vec::new();
        let mut vectorizable_paths = std::collections::HashSet::new();

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
                        ProcessingMode::Full => {
                            indexed_paths.push(escaped_path.clone());
                            indexed_paths_raw.push(path.clone());
                        }
                        ProcessingMode::StructureOnly => {
                            degraded_paths.push(escaped_path.clone());
                            degraded_paths_raw.push(path.clone());
                        }
                    }
                    if enqueue_vectorization
                        && matches!(
                            processing_mode,
                            ProcessingMode::Full | ProcessingMode::StructureOnly
                        )
                    {
                        file_vectorization_paths.push(path.clone());
                    }
                    let Some(project_code) = extraction.project_code.as_deref() else {
                        skipped_paths.push(format!("'{}'", Self::escape_sql(path)));
                        continue;
                    };
                    for sym in &extraction.symbols {
                        let symbol_id = Self::symbol_id(project_code, path, &sym.name);
                        if !seen_symbols.insert((symbol_id.clone(), project_code.to_string())) {
                            continue; // Prevent UNIQUE constraint violation in DuckDB ON CONFLICT batches
                        }
                        let embedding_sql = if let Some(ref v) = sym.embedding {
                            format!("CAST({:?} AS FLOAT[{DIMENSION}])", v)
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
                            Self::escape_sql(project_code),
                            embedding_sql
                        ));

                        contains_values.push(format!(
                            "('{}', '{}', '{}')",
                            Self::escape_sql(path),
                            Self::escape_sql(&symbol_id),
                            Self::escape_sql(project_code)
                        ));

                        if matches!(processing_mode, ProcessingMode::Full) {
                            for derived_chunk in Self::build_chunk_content(
                                sym,
                                content.as_deref().unwrap_or_default(),
                            ) {
                                let chunk_id = Self::chunk_part_id(
                                    &symbol_id,
                                    derived_chunk.part_index,
                                    derived_chunk.part_count,
                                );
                                let chunk_hash = Self::stable_content_hash(&derived_chunk.content);
                                chunk_values.push(format!(
                                    "('{}', 'symbol', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, {}, {}, '{}')",
                                    Self::escape_sql(&chunk_id),
                                    Self::escape_sql(&symbol_id),
                                    Self::escape_sql(project_code),
                                    Self::escape_sql(path),
                                    Self::escape_sql(&sym.kind),
                                    Self::escape_sql(&derived_chunk.content),
                                    Self::escape_sql(&chunk_hash),
                                    derived_chunk.start_line,
                                    derived_chunk.end_line,
                                    derived_chunk.part_index,
                                    derived_chunk.part_count,
                                    Self::escape_sql(&derived_chunk.chunk_path)
                                ));
                                if inline_enabled {
                                    inline_chunks.push((
                                        path.clone(),
                                        chunk_id.clone(),
                                        derived_chunk.content.clone(),
                                        chunk_hash.clone(),
                                    ));
                                }
                            }
                            vectorizable_paths.insert(path.clone());
                        }
                    }

                    for relation in &extraction.relations {
                        let Some(table) = Self::relation_table(&relation.rel_type) else {
                            continue;
                        };

                        let source_id = Self::symbol_id(project_code, path, &relation.from);
                        let target_id = Self::symbol_id(project_code, path, &relation.to);

                        let relation_value = format!(
                            "('{}', '{}', '{}')",
                            Self::escape_sql(&source_id),
                            Self::escape_sql(&target_id),
                            Self::escape_sql(project_code)
                        );

                        let relation_key = (source_id, target_id, project_code.to_string());

                        match table {
                            "CALLS" => {
                                if seen_calls.insert(relation_key) {
                                    calls_values.push(relation_value);
                                }
                            }
                            "CALLS_NIF" => {
                                if seen_calls_nif.insert(relation_key) {
                                    calls_nif_values.push(relation_value);
                                }
                            }
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
                "DELETE FROM GraphProjectionQueue \
                 WHERE anchor_type = 'file' AND anchor_id IN ({});",
                deleted_paths.join(",")
            ));
            queries.push(format!(
                "DELETE FROM FileVectorizationQueue WHERE file_path IN ({});",
                deleted_paths.join(",")
            ));
            queries.push(format!(
                "UPDATE File SET status = 'deleted', worker_id = NULL, needs_reindex = FALSE, defer_count = 0, last_deferred_at_ms = NULL, file_stage = 'deleted', graph_ready = FALSE, vector_ready = FALSE, last_state_change_at_ms = {}, last_error_at_ms = NULL WHERE path IN ({});",
                chrono::Utc::now().timestamp_millis(),
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
                "DELETE FROM ChunkEmbedding WHERE chunk_id IN (SELECT id FROM Chunk WHERE source_type = 'symbol' AND source_id IN (SELECT target_id FROM CONTAINS WHERE source_id IN ({})));",
                indexed_filter
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
        }
        let indexed_vectorizable_paths = indexed_paths_raw
            .iter()
            .filter(|path| vectorizable_paths.contains(*path))
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>();
        let degraded_vectorizable_paths = degraded_paths_raw
            .iter()
            .filter(|path| vectorizable_paths.contains(*path))
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>();

        if !indexed_paths.is_empty() {
            let indexed_vector_ready_expr = if indexed_vectorizable_paths.is_empty() {
                format!(
                    "NOT EXISTS ( \
                         SELECT 1 \
                         FROM Chunk c \
                         JOIN CONTAINS co ON co.target_id = c.source_id \
                         LEFT JOIN ChunkEmbedding ce \
                           ON ce.chunk_id = c.id \
                          AND ce.model_id = '{}' \
                          AND ce.source_hash = c.content_hash \
                         WHERE co.source_id = File.path \
                           AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
                     )",
                    Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
                )
            } else {
                format!(
                    "CASE \
                         WHEN path IN ({}) THEN FALSE \
                         ELSE NOT EXISTS ( \
                             SELECT 1 \
                             FROM Chunk c \
                             JOIN CONTAINS co ON co.target_id = c.source_id \
                             LEFT JOIN ChunkEmbedding ce \
                               ON ce.chunk_id = c.id \
                              AND ce.model_id = '{}' \
                              AND ce.source_hash = c.content_hash \
                             WHERE co.source_id = File.path \
                               AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
                         ) \
                     END",
                    indexed_vectorizable_paths.join(","),
                    Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
                )
            };
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed' END, \
                     file_stage = CASE WHEN needs_reindex THEN 'promoted' ELSE 'graph_indexed' END, \
                     graph_ready = CASE WHEN needs_reindex THEN FALSE ELSE TRUE END, \
                     vector_ready = CASE \
                         WHEN needs_reindex THEN FALSE \
                         ELSE {} \
                     END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = NULL, \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE 'indexed_success_full' END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL, \
                     graph_ready_at_ms = CASE WHEN needs_reindex THEN File.graph_ready_at_ms ELSE COALESCE(File.graph_ready_at_ms, {}) END, \
                     last_state_change_at_ms = {}, \
                     last_error_at_ms = NULL \
                 WHERE path IN ({});",
                indexed_vector_ready_expr,
                chrono::Utc::now().timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                indexed_paths.join(",")
            ));
        }
        if !degraded_paths.is_empty() {
            let degraded_vector_ready_expr = if degraded_vectorizable_paths.is_empty() {
                format!(
                    "NOT EXISTS ( \
                         SELECT 1 \
                         FROM Chunk c \
                         JOIN CONTAINS co ON co.target_id = c.source_id \
                         LEFT JOIN ChunkEmbedding ce \
                           ON ce.chunk_id = c.id \
                          AND ce.model_id = '{}' \
                          AND ce.source_hash = c.content_hash \
                         WHERE co.source_id = File.path \
                           AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
                     )",
                    Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
                )
            } else {
                format!(
                    "CASE \
                         WHEN path IN ({}) THEN FALSE \
                         ELSE NOT EXISTS ( \
                             SELECT 1 \
                             FROM Chunk c \
                             JOIN CONTAINS co ON co.target_id = c.source_id \
                             LEFT JOIN ChunkEmbedding ce \
                               ON ce.chunk_id = c.id \
                              AND ce.model_id = '{}' \
                              AND ce.source_hash = c.content_hash \
                             WHERE co.source_id = File.path \
                               AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
                         ) \
                     END",
                    degraded_vectorizable_paths.join(","),
                    Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
                )
            };
            queries.push(format!(
                "UPDATE File \
                     SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed_degraded' END, \
                     file_stage = CASE WHEN needs_reindex THEN 'promoted' ELSE 'graph_indexed' END, \
                     graph_ready = CASE WHEN needs_reindex THEN FALSE ELSE TRUE END, \
                     vector_ready = CASE \
                         WHEN needs_reindex THEN FALSE \
                         ELSE {} \
                     END, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = 'degraded_structure_only', \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE 'degraded_structure_only' END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL, \
                     graph_ready_at_ms = CASE WHEN needs_reindex THEN File.graph_ready_at_ms ELSE COALESCE(File.graph_ready_at_ms, {}) END, \
                     last_state_change_at_ms = {}, \
                     last_error_at_ms = CASE WHEN needs_reindex THEN File.last_error_at_ms ELSE {} END \
                 WHERE path IN ({});",
                degraded_vector_ready_expr,
                chrono::Utc::now().timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                degraded_paths.join(",")
            ));
        }
        if !skipped_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'skipped' END, \
                     file_stage = CASE WHEN needs_reindex THEN 'promoted' ELSE 'skipped' END, \
                     graph_ready = FALSE, \
                     vector_ready = FALSE, \
                     worker_id = NULL, \
                     needs_reindex = FALSE, \
                     last_error_reason = 'worker_skipped_file', \
                     status_reason = CASE WHEN needs_reindex THEN 'needs_reindex_while_indexing' ELSE 'worker_skipped_file' END, \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL, \
                     last_state_change_at_ms = {}, \
                     last_error_at_ms = {} \
                 WHERE path IN ({});",
                chrono::Utc::now().timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                skipped_paths.join(",")
            ));
        }
        sort_and_dedup_sql_tuples(&mut contains_values);
        sort_and_dedup_sql_tuples(&mut calls_values);
        sort_and_dedup_sql_tuples(&mut calls_nif_values);
        for chunk in symbol_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) VALUES {} ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe, project_code=EXCLUDED.project_code, embedding=EXCLUDED.embedding;",
                chunk.join(",")
            ));
        }
        for chunk in chunk_values.chunks(500) {
            queries.push(format!(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line, chunk_part_index, chunk_part_count, chunk_path) VALUES {} \
                 ON CONFLICT(id) DO UPDATE SET source_type=EXCLUDED.source_type, source_id=EXCLUDED.source_id, project_code=EXCLUDED.project_code, file_path=EXCLUDED.file_path, kind=EXCLUDED.kind, content=EXCLUDED.content, content_hash=EXCLUDED.content_hash, start_line=EXCLUDED.start_line, end_line=EXCLUDED.end_line, chunk_part_index=EXCLUDED.chunk_part_index, chunk_part_count=EXCLUDED.chunk_part_count, chunk_path=EXCLUDED.chunk_path;",
                chunk.join(",")
            ));
        }
        queries.extend(insert_unique_relation_queries("CONTAINS", &contains_values));
        queries.extend(replace_relation_queries("CALLS", &calls_values, 200));
        queries.extend(replace_relation_queries(
            "CALLS_NIF",
            &calls_nif_values,
            200,
        ));
        let mut enqueued_vectorization = false;
        // DEC-AXO-071 H.2: paths that will skip the queue and be embedded
        // inline below. Failures fall back to the queue in the inline
        // pass, so the safety net is preserved.
        let mut inline_target_paths: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // DEC-AXO-072 J.3: when the hot status cache is enabled, route
        // ready-state writes through the cache instead of generating
        // INSERT FVQ SQL here. The cache batches and flushes
        // asynchronously (100ms window). Cache disabled -> existing
        // direct-DB path (commit G + H.2 behavior).
        let hot_cache_enabled = crate::hot_status_cache::cache_enabled();
        if !file_vectorization_paths.is_empty() {
            file_vectorization_paths.sort();
            file_vectorization_paths.dedup();
            let now_ms = chrono::Utc::now().timestamp_millis();
            for path in file_vectorization_paths {
                if vectorizable_paths.contains(&path) {
                    if inline_enabled {
                        inline_target_paths.insert(path.clone());
                    } else if hot_cache_enabled {
                        if let Some(cache) = crate::hot_status_cache::cache() {
                            cache.mark_ready(&path, now_ms);
                            enqueued_vectorization = true;
                        } else {
                            queries.push(file_vectorization_queue_upsert_if_needed(&path, now_ms));
                            enqueued_vectorization = true;
                        }
                    } else {
                        queries.push(file_vectorization_queue_upsert_if_needed(&path, now_ms));
                        enqueued_vectorization = true;
                    }
                } else {
                    queries.push(format!(
                        "DELETE FROM FileVectorizationQueue WHERE file_path = '{}';",
                        Self::escape_sql(&path)
                    ));
                }
            }
        }
        self.execute_batch(&queries)?;

        // DEC-AXO-071 H.2 inline embedding pass. Runs after chunks are
        // committed so embeddings reference rows that exist in the DB.
        // Each file is embedded as one batch via the shared vector lane
        // (single BGE-Large model — no multi-worker GPU contention,
        // REQ-AXO-181 step 4 cascade still avoided). On any failure
        // (channel timeout, lane error, mismatched count, persist error)
        // the affected file falls back to FileVectorizationQueue.
        if inline_enabled && !inline_target_paths.is_empty() {
            let mut by_file: std::collections::HashMap<String, Vec<(String, String, String)>> =
                std::collections::HashMap::new();
            for (file_path, chunk_id, content, content_hash) in inline_chunks {
                if !inline_target_paths.contains(&file_path) {
                    continue;
                }
                by_file
                    .entry(file_path)
                    .or_default()
                    .push((chunk_id, content, content_hash));
            }
            let mut inline_completed_paths: Vec<String> = Vec::new();
            let mut inline_failed_paths: Vec<String> = Vec::new();
            for (file_path, rows) in by_file {
                let texts: Vec<String> = rows
                    .iter()
                    .map(|(_, content, _)| content.clone())
                    .collect();
                match crate::embedder::inline_embed::embed_via_vector_lane(texts) {
                    Ok(embeddings) if embeddings.len() == rows.len() => {
                        let updates: Vec<(String, String, Vec<f32>)> = rows
                            .into_iter()
                            .zip(embeddings.into_iter())
                            .map(|((chunk_id, _, content_hash), emb)| {
                                (chunk_id, content_hash, emb)
                            })
                            .collect();
                        if let Err(e) =
                            self.update_chunk_embeddings(CHUNK_EMBEDDING_MODEL_ID, &updates)
                        {
                            tracing::warn!(
                                file_path = %file_path,
                                error = ?e,
                                "inline embed: persist failed; falling back to queue"
                            );
                            inline_failed_paths.push(file_path);
                        } else {
                            inline_completed_paths.push(file_path);
                        }
                    }
                    Ok(embeddings) => {
                        tracing::warn!(
                            file_path = %file_path,
                            expected = rows.len(),
                            received = embeddings.len(),
                            "inline embed: lane returned mismatched embedding count; falling back to queue"
                        );
                        inline_failed_paths.push(file_path);
                    }
                    Err(e) => {
                        tracing::warn!(
                            file_path = %file_path,
                            error = ?e,
                            "inline embed: vector lane error; falling back to queue"
                        );
                        inline_failed_paths.push(file_path);
                    }
                }
            }
            if !inline_completed_paths.is_empty() {
                self.mark_file_vectorization_done(
                    &inline_completed_paths,
                    CHUNK_EMBEDDING_MODEL_ID,
                )?;
            }
            if !inline_failed_paths.is_empty() {
                let now_ms_fallback = chrono::Utc::now().timestamp_millis();
                if hot_cache_enabled {
                    if let Some(cache) = crate::hot_status_cache::cache() {
                        for path in &inline_failed_paths {
                            cache.mark_ready(path, now_ms_fallback);
                        }
                    }
                } else {
                    let fallback_queries: Vec<String> = inline_failed_paths
                        .iter()
                        .map(|p| file_vectorization_queue_upsert_if_needed(p, now_ms_fallback))
                        .collect();
                    self.execute_batch(&fallback_queries)?;
                }
                enqueued_vectorization = true;
            }
        }
        let repaired_orphan_vectorization = if enqueue_vectorization {
            let graph_ready_paths = indexed_paths_raw
                .iter()
                .chain(degraded_paths_raw.iter())
                .cloned()
                .collect::<Vec<_>>();
            self.reconcile_orphaned_file_vectorization_paths(&graph_ready_paths)?
        } else {
            0
        };

        if enqueued_vectorization || repaired_orphan_vectorization > 0 {
            service_guard::notify_vector_backlog_activity();
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let graph_ready_paths = indexed_paths
            .iter()
            .chain(degraded_paths.iter())
            .map(|path| path.trim_matches('\'').to_string())
            .collect::<Vec<_>>();
        if !graph_ready_paths.is_empty() {
            let metadata = self.fetch_file_project_metadata(&graph_ready_paths)?;
            let events = graph_ready_paths
                .into_iter()
                .filter_map(|path| {
                    metadata
                        .get(&path)
                        .map(|(project_code, worker_id, trace_id)| FileLifecycleEvent {
                            file_path: path,
                            project_code: project_code.clone(),
                            stage: "graph".to_string(),
                            status: "ready".to_string(),
                            reason: None,
                            at_ms: now_ms,
                            worker_id: *worker_id,
                            trace_id: trace_id.clone(),
                            run_id: None,
                        })
                })
                .collect::<Vec<_>>();
            if !events.is_empty() {
                self.append_file_lifecycle_events(&events)?;
            }
        }

        Ok(())
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
                 SET status = 'indexing', worker_id = {}, status_reason = 'claimed_for_indexing', defer_count = 0, last_deferred_at_ms = NULL, file_stage = 'claimed', indexing_started_at_ms = COALESCE(indexing_started_at_ms, {}), last_state_change_at_ms = {}
                 WHERE path IN (
                    SELECT path FROM File
                    WHERE status = 'pending'
                    ORDER BY priority DESC, COALESCE(defer_count, 0) DESC, COALESCE(last_deferred_at_ms, 9223372036854775807) ASC, size ASC
                    LIMIT {}
                 );",
                claim_id, chrono::Utc::now().timestamp_millis(), chrono::Utc::now().timestamp_millis(), count
            );

            let c_query = match CString::new(claim_query) {
                Ok(c) => c,
                Err(e) => {
                    if let Ok(rb) = CString::new("ROLLBACK;") {
                        let _ = exec_fn(*guard, rb.as_ptr());
                    }
                    return Err(anyhow!("Pending Fetch Error (CString): {:?}", e));
                }
            };

            if !exec_fn(*guard, c_query.as_ptr()) {
                if let Ok(rb) = CString::new("ROLLBACK;") {
                    let _ = exec_fn(*guard, rb.as_ptr());
                }
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
        let res = match self.query_on_ctx(&fetch_query, *guard) {
            Ok(r) => r,
            Err(e) => {
                unsafe {
                    if let Ok(exec_fn) = self
                        .pool
                        .lib
                        .get::<LibSymbol<ExecFunc>>(b"duckdb_execute\0")
                    {
                        if let Ok(rb_query) = CString::new("ROLLBACK;") {
                            let _ = exec_fn(*guard, rb_query.as_ptr());
                        }
                    }
                }
                return Err(e);
            }
        };

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Pending Fetch Error: COMMIT failed"));
            }
        }
        self.mark_writer_commit_visible();
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
                 SET status = 'indexing', worker_id = {}, status_reason = 'claimed_for_indexing', defer_count = 0, last_deferred_at_ms = NULL, file_stage = 'claimed', indexing_started_at_ms = COALESCE(indexing_started_at_ms, {}), last_state_change_at_ms = {}
                 WHERE status = 'pending' AND path IN ({});",
                claim_id, chrono::Utc::now().timestamp_millis(), chrono::Utc::now().timestamp_millis(), path_list
            );

            let c_query = match CString::new(claim_query) {
                Ok(c) => c,
                Err(e) => {
                    if let Ok(rb) = CString::new("ROLLBACK;") {
                        let _ = exec_fn(*guard, rb.as_ptr());
                    }
                    return Err(anyhow!("Claim Paths Error (CString): {:?}", e));
                }
            };

            if !exec_fn(*guard, c_query.as_ptr()) {
                if let Ok(rb) = CString::new("ROLLBACK;") {
                    let _ = exec_fn(*guard, rb.as_ptr());
                }
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
        let res = match self.query_on_ctx(&fetch_query, *guard) {
            Ok(r) => r,
            Err(e) => {
                unsafe {
                    if let Ok(exec_fn) = self
                        .pool
                        .lib
                        .get::<LibSymbol<ExecFunc>>(b"duckdb_execute\0")
                    {
                        if let Ok(rb_query) = CString::new("ROLLBACK;") {
                            let _ = exec_fn(*guard, rb_query.as_ptr());
                        }
                    }
                }
                return Err(e);
            }
        };

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Claim Paths Error: COMMIT failed"));
            }
        }
        self.mark_writer_commit_visible();
        drop(guard);

        if res == "[]" || res.is_empty() {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        Ok(raw.into_iter().filter_map(parse_pending_file_row).collect())
    }

    pub fn mark_file_oversized_for_current_budget(&self, path: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let queries = vec![
            format!(
                "UPDATE File \
                 SET status = 'oversized_for_current_budget', \
                     file_stage = 'oversized', \
                     graph_ready = FALSE, \
                     vector_ready = FALSE, \
                     worker_id = NULL, \
                     last_error_reason = 'estimated cost exceeds current budget envelope', \
                     status_reason = 'oversized_for_current_budget', \
                     defer_count = 0, \
                     last_deferred_at_ms = NULL \
                 WHERE path = '{}';",
                Self::escape_sql(path)
            ),
            format!(
                "DELETE FROM FileVectorizationQueue WHERE file_path = '{}';",
                Self::escape_sql(path)
            ),
            format!(
                "INSERT INTO FileLifecycleEvent (file_path, project_code, stage, status, reason, at_ms, worker_id, trace_id, run_id) \
                 SELECT path, project_code, 'vectorization', 'oversized_for_current_budget', 'estimated cost exceeds current budget envelope', {}, NULL, NULL, NULL \
                 FROM File WHERE path = '{}' AND project_code IS NOT NULL AND trim(project_code) != '';",
                now_ms,
                Self::escape_sql(path)
            ),
        ];
        self.execute_batch(&queries)
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
                 last_deferred_at_ms = {}, \
                 status_reason = 'deferred_by_scheduler' \
             WHERE status = 'pending' AND path IN ({});",
            now_ms, path_list
        ))
    }

    pub fn requeue_claimed_file(&self, path: &str) -> Result<()> {
        self.requeue_claimed_file_with_reason(path, "manual_or_system_requeue")
    }

    pub fn requeue_claimed_file_with_reason(&self, path: &str, reason: &str) -> Result<()> {
        self.requeue_claimed_paths_with_reason(&[path.to_string()], reason)
    }

    pub fn mark_claimed_file_writer_pending_commit(&self, path: &str) -> Result<()> {
        self.execute(&format!(
            "UPDATE File \
             SET status_reason = 'writer_pending_commit' \
                 , file_stage = 'writer_pending_commit' \
             WHERE path = '{}' AND status = 'indexing';",
            Self::escape_sql(path)
        ))
    }

    pub fn requeue_claimed_paths_with_reason(&self, paths: &[String], reason: &str) -> Result<()> {
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
             SET status = 'pending', \
                 file_stage = 'promoted', \
                 graph_ready = FALSE, \
                 vector_ready = FALSE, \
                 worker_id = NULL, \
                 last_error_reason = NULL, \
                 status_reason = '{}', \
                 defer_count = COALESCE(defer_count, 0) + 1, \
                 last_deferred_at_ms = {} \
             WHERE path IN ({}) AND status = 'indexing';",
            Self::escape_sql(reason),
            now_ms,
            path_list
        ))
        .and_then(|_| {
            self.execute(&format!(
                "DELETE FROM FileVectorizationQueue WHERE file_path IN ({});",
                path_list
            ))
        })
    }

    pub fn fetch_unembedded_symbols(&self, count: usize) -> Result<Vec<(String, String)>> {
        let query = format!(
            "SELECT id, name || ': ' || kind FROM Symbol WHERE embedding IS NULL LIMIT {}",
            count
        );
        let res = self.query_json_writer(&query)?;

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

    pub fn fetch_unembedded_chunks_for_file(
        &self,
        file_path: &str,
        model_id: &str,
        count: usize,
    ) -> Result<Vec<(String, String, String)>> {
        let query = format!(
            "SELECT c.id, c.content, c.content_hash \
             FROM Chunk c \
             WHERE c.file_path = '{}' \
             AND NOT EXISTS ( \
                 SELECT 1 \
                 FROM ChunkEmbedding ce \
                 WHERE ce.chunk_id = c.id \
                   AND ce.model_id = '{}' \
                   AND ce.source_hash = c.content_hash \
             ) \
             LIMIT {}",
            Self::escape_sql(file_path),
            Self::escape_sql(model_id),
            count
        );
        let res = self.query_json_writer(&query)?;

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

    /// Batch-fetch unembedded chunks for multiple files in a single query.
    /// Returns a map from file_path to chunks (id, content, content_hash).
    /// Uses ROW_NUMBER window function to apply per-file limits server-side.
    pub fn fetch_unembedded_chunks_batch(
        &self,
        file_paths: &[&str],
        model_id: &str,
        per_file_limit: usize,
    ) -> Result<std::collections::HashMap<String, Vec<(String, String, String)>>> {
        use std::collections::HashMap;

        if file_paths.is_empty() {
            return Ok(HashMap::new());
        }

        let escaped_model = Self::escape_sql(model_id);
        let in_list: String = file_paths
            .iter()
            .map(|p| format!("'{}'", Self::escape_sql(p)))
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "SELECT file_path, id, content, content_hash FROM ( \
                 SELECT c.file_path, c.id, c.content, c.content_hash, \
                        ROW_NUMBER() OVER (PARTITION BY c.file_path) AS rn \
                 FROM Chunk c \
                 WHERE c.file_path IN ({}) \
                 AND NOT EXISTS ( \
                     SELECT 1 \
                     FROM ChunkEmbedding ce \
                     WHERE ce.chunk_id = c.id \
                       AND ce.model_id = '{}' \
                       AND ce.source_hash = c.content_hash \
                 ) \
             ) sub WHERE rn <= {}",
            in_list, escaped_model, per_file_limit
        );

        let res = self.query_json_writer(&query)?;

        let mut result: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        if res == "[]" || res.is_empty() {
            return Ok(result);
        }

        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        for row in raw {
            if row.len() >= 4 {
                if let (Some(fp), Some(id), Some(content), Some(hash)) = (
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                ) {
                    result
                        .entry(fp.to_string())
                        .or_default()
                        .push((id.to_string(), content.to_string(), hash.to_string()));
                }
            }
        }

        Ok(result)
    }

    pub fn fetch_segments_for_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<(String, String, String)>> {
        let query = format!(
            "SELECT c.id, c.content, c.content_hash \
             FROM Chunk c \
             WHERE c.file_path = '{}' \
             ORDER BY COALESCE(c.start_line, 9223372036854775807) ASC, \
                      COALESCE(c.end_line, 9223372036854775807) ASC, \
                      c.id ASC",
            Self::escape_sql(file_path),
        );
        let res = self.query_json_writer(&query)?;

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

    pub fn file_has_unembedded_chunks(&self, file_path: &str, model_id: &str) -> Result<bool> {
        let query = format!(
            "SELECT EXISTS (\
             SELECT 1 \
             FROM Chunk c \
             WHERE c.file_path = '{}' \
             AND NOT EXISTS ( \
                 SELECT 1 \
                 FROM ChunkEmbedding ce \
                 WHERE ce.chunk_id = c.id \
                   AND ce.model_id = '{}' \
                   AND ce.source_hash = c.content_hash \
             ) \
            )",
            Self::escape_sql(file_path),
            Self::escape_sql(model_id)
        );

        let raw = self.query_json_writer(&query)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(false);
        };
        Ok(row
            .first()
            .and_then(|value| value.as_bool())
            .unwrap_or(false))
    }

    pub fn mark_file_vectorization_done(&self, paths: &[String], _model_id: &str) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let filter = paths
            .iter()
            .map(|path| format!("'{}'", Self::escape_sql(path)))
            .collect::<Vec<_>>()
            .join(",");
        self.execute(&format!(
            "UPDATE File \
             SET vector_ready = TRUE, \
                 vector_ready_at_ms = COALESCE(vector_ready_at_ms, {}), \
                 last_state_change_at_ms = {} \
             WHERE graph_ready = TRUE AND path IN ({})",
            now_ms, now_ms, filter
        ))
    }

    pub fn finalize_file_vectorization_success_batch(
        &self,
        work: &[FileVectorizationWork],
        lease_snapshots: &[FileVectorizationLeaseSnapshot],
        _model_id: &str,
        projection_radius: i64,
    ) -> Result<()> {
        self.finalize_file_vectorization_success_batch_for_owner(
            work,
            lease_snapshots,
            "finalize",
            _model_id,
            projection_radius,
        )
    }

    pub fn finalize_file_vectorization_success_batch_for_owner(
        &self,
        work: &[FileVectorizationWork],
        lease_snapshots: &[FileVectorizationLeaseSnapshot],
        lease_owner: &str,
        _model_id: &str,
        projection_radius: i64,
    ) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }
        if lease_snapshots.len() != work.len() {
            return Err(anyhow!(
                "finalize refused: expected {} lease snapshots, got {}",
                work.len(),
                lease_snapshots.len()
            ));
        }

        let paths = work
            .iter()
            .map(|item| format!("'{}'", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(",");
        let lease_predicates = lease_snapshots
            .iter()
            .map(|item| {
                format!(
                    "(file_path = '{}' AND claim_token = '{}' AND COALESCE(lease_epoch, 0) = {} AND COALESCE(lease_owner, '') = '{}')",
                    Self::escape_sql(&item.file_path),
                    Self::escape_sql(&item.claim_token),
                    item.lease_epoch,
                    Self::escape_sql(lease_owner)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let matched = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND ({})",
            lease_predicates
        ))?)
        .unwrap_or(0);
        if matched != lease_snapshots.len() {
            return Err(anyhow!(
                "finalize refused: expected {} {}-owned rows, matched {}",
                lease_snapshots.len(),
                lease_owner,
                matched
            ));
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut queries = vec![
            format!(
                "UPDATE File \
                 SET vector_ready = TRUE, \
                     vector_ready_at_ms = COALESCE(vector_ready_at_ms, {}), \
                     last_state_change_at_ms = {} \
                 WHERE graph_ready = TRUE \
                   AND path IN ({}) \
                   AND EXISTS ( \
                       SELECT 1 FROM FileVectorizationQueue fvq \
                       WHERE fvq.file_path = File.path \
                         AND fvq.status = 'inflight' \
                         AND ({}) \
                   )",
                now_ms, now_ms, paths, lease_predicates
            ),
            format!(
                "DELETE FROM FileVectorizationQueue \
                 WHERE status = 'inflight' \
                   AND ({})",
                lease_predicates
            ),
        ];

        if graph_embeddings_enabled() {
            for item in work {
                queries.push(graph_projection_queue_upsert(
                    "file",
                    &item.file_path,
                    projection_radius,
                    now_ms,
                ));
            }
        }

        self.execute_batch(&queries)?;
        let metadata = self.fetch_file_project_metadata(
            &work
                .iter()
                .map(|item| item.file_path.clone())
                .collect::<Vec<_>>(),
        )?;
        let events = work
            .iter()
            .filter_map(|item| {
                metadata
                    .get(&item.file_path)
                    .map(|(project_code, worker_id, trace_id)| FileLifecycleEvent {
                        file_path: item.file_path.clone(),
                        project_code: project_code.clone(),
                        stage: "vectorization".to_string(),
                        status: "ready".to_string(),
                        reason: None,
                        at_ms: now_ms,
                        worker_id: *worker_id,
                        trace_id: trace_id.clone(),
                        run_id: None,
                    })
            })
            .collect::<Vec<_>>();
        if !events.is_empty() {
            self.append_file_lifecycle_events(&events)?;
        }
        self.refresh_hourly_vectorization_rollup(
            hourly_bucket_start_ms(now_ms),
            CHUNK_EMBEDDING_MODEL_ID,
        )?;
        Ok(())
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
             WHERE NOT EXISTS ( \
                 SELECT 1 \
                 FROM ChunkEmbedding ce \
                 WHERE ce.chunk_id = c.id \
                   AND ce.model_id = '{}' \
                   AND ce.source_hash = c.content_hash \
             ) \
             LIMIT {}",
            Self::escape_sql(model_id),
            count
        );
        let res = self.query_json_writer(&query)?;

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
                let embedding_sql = format!("CAST({:?} AS FLOAT[{DIMENSION}])", vector);
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

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut queries = Vec::new();
        let values: Vec<String> = updates
            .iter()
            .map(|(chunk_id, source_hash, vector)| {
                format!(
                    "('{}', '{}', CAST({:?} AS FLOAT[{DIMENSION}]), '{}', {})",
                    Self::escape_sql(chunk_id),
                    Self::escape_sql(model_id),
                    vector,
                    Self::escape_sql(source_hash),
                    now_ms
                )
            })
            .collect();

        for chunk in values.chunks(CHUNK_EMBEDDING_UPSERT_BATCH_ROWS) {
            queries.push(format!(
                "INSERT OR REPLACE INTO ChunkEmbedding (chunk_id, model_id, embedding, source_hash, embedded_at_ms) VALUES {};",
                chunk.join(",")
            ));
        }

        self.execute_batch(&queries)?;
        self.refresh_hourly_vectorization_rollup(hourly_bucket_start_ms(now_ms), model_id)?;
        Ok(())
    }

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        self.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('{}', '{}', '{}') ON CONFLICT DO NOTHING;",
            from, to, from
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        dedup_file_batch_rows, insert_unique_relation_queries, parse_i64_field,
        replace_relation_queries, sort_and_dedup_sql_tuples, FileUpsertSource,
        FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
        VectorLaneStateRecord, VectorPersistOutboxPayload, VectorPersistOutboxUpdate,
        VectorWorkerFault, CHUNK_EMBEDDING_MODEL_ID,
    };
    use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};
    use crate::parser::{ExtractionResult, Relation, Symbol};
    use crate::queue::ProcessingMode;
    use crate::worker::DbWriteTask;

    #[test]
    fn sort_and_dedup_sql_tuples_removes_duplicate_relation_rows() {
        let mut values = vec![
            "('b', 'c', 'PRJ')".to_string(),
            "('a', 'b', 'PRJ')".to_string(),
            "('b', 'c', 'PRJ')".to_string(),
            "('a', 'b', 'PRJ')".to_string(),
            "('c', 'd', 'PRJ')".to_string(),
        ];

        sort_and_dedup_sql_tuples(&mut values);

        assert_eq!(
            values,
            vec![
                "('a', 'b', 'PRJ')".to_string(),
                "('b', 'c', 'PRJ')".to_string(),
                "('c', 'd', 'PRJ')".to_string(),
            ]
        );
    }

    #[test]
    fn insert_unique_relation_queries_emit_conflict_safe_single_row_inserts() {
        let queries = insert_unique_relation_queries(
            "CALLS",
            &[
                "('a', 'b', 'PRJ')".to_string(),
                "('c', 'd', 'PRJ')".to_string(),
            ],
        );

        assert_eq!(queries.len(), 2);
        assert!(queries[0].contains("INSERT INTO CALLS"));
        assert!(queries[0].contains("ON CONFLICT DO NOTHING"));
        assert!(!queries[0].contains("LEFT JOIN"));
    }

    #[test]
    fn replace_relation_queries_delete_then_reinsert_exact_rows() {
        let queries = replace_relation_queries(
            "CALLS",
            &[
                "('a', 'b', 'PRJ')".to_string(),
                "('c', 'd', 'PRJ')".to_string(),
            ],
            200,
        );

        assert_eq!(queries.len(), 2);
        assert!(queries[0].contains("DELETE FROM CALLS USING (VALUES"));
        assert!(queries[1].contains("INSERT INTO CALLS"));
        assert!(!queries[1].contains("ON CONFLICT"));
    }

    #[test]
    fn dedup_file_batch_rows_collapses_duplicate_paths() {
        let rows = vec![
            (
                "/tmp/a.rs".to_string(),
                "PRJ".to_string(),
                10,
                1,
                100,
                FileUpsertSource::Scan,
            ),
            (
                "/tmp/a.rs".to_string(),
                "PRJ".to_string(),
                20,
                2,
                200,
                FileUpsertSource::HotDelta,
            ),
            (
                "/tmp/b.rs".to_string(),
                "PRJ".to_string(),
                30,
                3,
                100,
                FileUpsertSource::Scan,
            ),
        ];

        let deduped = dedup_file_batch_rows(&rows);

        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].0, "/tmp/a.rs");
        assert_eq!(deduped[0].2, 20);
        assert_eq!(deduped[0].3, 2);
        assert_eq!(deduped[0].4, 200);
        assert!(matches!(deduped[0].5, FileUpsertSource::HotDelta));
        assert_eq!(deduped[1].0, "/tmp/b.rs");
    }

    #[test]
    fn upsert_file_queries_use_conflict_safe_insert_for_new_rows() {
        let queries = crate::graph::GraphStore::upsert_file_queries(
            "/tmp/demo.rs",
            "PRJ",
            42,
            7,
            100,
            FileUpsertSource::Scan,
        );

        let insert_query = queries
            .iter()
            .find(|query| query.contains("INSERT INTO File"))
            .expect("expected INSERT INTO File query");

        assert!(insert_query.contains("ON CONFLICT(path) DO NOTHING"));
        assert!(!insert_query.contains("WHERE NOT EXISTS"));
    }

    #[test]
    fn bulk_upsert_file_queries_stays_set_based_for_multiple_rows() {
        let queries = crate::graph::GraphStore::bulk_upsert_file_queries(&[
            (
                "/tmp/a.rs".to_string(),
                "PRJ".to_string(),
                10,
                1,
                100,
                FileUpsertSource::Scan,
            ),
            (
                "/tmp/b.rs".to_string(),
                "PRJ".to_string(),
                20,
                2,
                200,
                FileUpsertSource::HotDelta,
            ),
        ]);

        assert_eq!(queries.len(), 4);
        assert!(queries[0].starts_with("WITH src("));
        assert!(queries[1].contains("UPDATE File"));
        assert!(queries[1].contains("FROM src"));
        assert!(queries[2].contains("INSERT INTO File"));
        assert!(queries[3].contains("INSERT INTO GraphProjectionQueue"));
        assert!(!queries
            .iter()
            .any(|query| query.contains("WHERE path = '/tmp/a.rs'")));
    }

    #[test]
    fn canonical_timestamp_columns_exist_on_file_and_chunk_embedding() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let raw = store
            .query_json(
                "SELECT table_name, column_name \
                 FROM information_schema.columns \
                 WHERE table_name IN ('File', 'ChunkEmbedding') \
                   AND column_name IN ('first_seen_at_ms', 'indexing_started_at_ms', 'graph_ready_at_ms', 'vectorization_started_at_ms', 'vector_ready_at_ms', 'last_state_change_at_ms', 'last_error_at_ms', 'embedded_at_ms')",
            )
            .unwrap();
        assert!(raw.contains("first_seen_at_ms"));
        assert!(raw.contains("embedded_at_ms"));
        assert!(raw.contains("vector_ready_at_ms"));
    }

    #[test]
    fn update_chunk_embeddings_persists_embedded_at_ms_and_refreshes_rollup() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-1', 'symbol', 'sym-1', 'PRJ', '/tmp/demo.rs', 'function', 'fn demo() {}', 'hash-1', 1, 1)",
            )
            .unwrap();

        store
            .update_chunk_embeddings(
                CHUNK_MODEL_ID,
                &[(
                    "chunk-1".to_string(),
                    "hash-1".to_string(),
                    vec![0.1_f32; crate::embedding_contract::DIMENSION],
                )],
            )
            .unwrap();

        let embedded_at_ms = store
            .query_count(
                "SELECT count(*) FROM ChunkEmbedding WHERE chunk_id = 'chunk-1' AND embedded_at_ms IS NOT NULL",
            )
            .unwrap();
        assert_eq!(embedded_at_ms, 1);

        let rollup_count = store
            .query_count(&format!(
                "SELECT count(*) FROM HourlyVectorizationRollup WHERE model_id = '{}'",
                CHUNK_MODEL_ID
            ))
            .unwrap();
        assert!(rollup_count >= 1);
    }

    #[test]
    fn finalize_file_vectorization_success_batch_records_vectorization_event() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, file_stage, graph_ready, vector_ready, size, mtime, priority) \
                 VALUES ('/tmp/vectorized.rs', 'PRJ', 'indexed', 'graph_indexed', TRUE, FALSE, 1, 1, 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
                 VALUES ('/tmp/vectorized.rs', 'inflight', 1, 'claim-1', 1, 1, 'finalize', 1)",
            )
            .unwrap();

        store
            .finalize_file_vectorization_success_batch(
                &[FileVectorizationWork {
                    file_path: "/tmp/vectorized.rs".to_string(),
                    resumed_after_interactive_pause: false,
                }],
                &[FileVectorizationLeaseSnapshot {
                    file_path: "/tmp/vectorized.rs".to_string(),
                    claim_token: "claim-1".to_string(),
                    lease_epoch: 1,
                }],
                CHUNK_MODEL_ID,
                2,
            )
            .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileLifecycleEvent WHERE file_path = '/tmp/vectorized.rs' AND stage = 'vectorization' AND status = 'ready'",
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM File WHERE path = '/tmp/vectorized.rs' AND vector_ready = TRUE AND vector_ready_at_ms IS NOT NULL",
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn insert_file_data_batch_builds_chunk_content_without_path_or_line_metadata() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/chunk_contract.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                reservation_id: "res-chunk-contract".to_string(),
                path: path.clone(),
                content: Some(
                    "fn chunk_contract() {\n    hydrate_context();\n    flush_ready_queue();\n}\n"
                        .to_string(),
                ),
                extraction: ExtractionResult {
                    project_code: Some("PRJ".to_string()),
                    symbols: vec![Symbol {
                        name: "chunk_contract".to_string(),
                        kind: "function".to_string(),
                        start_line: 1,
                        end_line: 3,
                        docstring: Some(
                            "Keeps only semantic symbol context in the embedded chunk.".to_string(),
                        ),
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    }],
                    relations: vec![],
                },
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-chunk-contract".to_string(),
                observed_cost_bytes: 1,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let raw = store
            .query_json(
                "SELECT content FROM Chunk WHERE file_path = '/tmp/chunk_contract.rs' AND project_code = 'PRJ'",
            )
            .unwrap();
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let content = rows[0][0].as_str().unwrap_or_default();

        assert!(content.contains("symbol: chunk_contract"), "{content}");
        assert!(content.contains("kind: function"), "{content}");
        assert!(
            content
                .contains("docstring: Keeps only semantic symbol context in the embedded chunk."),
            "{content}"
        );
        assert!(content.contains("hydrate_context();"), "{content}");
        assert!(!content.contains("file:"), "{content}");
        assert!(!content.contains("lines:"), "{content}");
    }

    #[test]
    fn oversized_symbol_emits_multiple_chunks_with_same_source_symbol() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/oversized_symbol.rs".to_string();

        unsafe {
            std::env::set_var("AXON_TARGET_CHUNK_TOKENS", "64");
            std::env::set_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH", "32");
            std::env::set_var("AXON_GRAY_ZONE_CHAR_THRESHOLD", "64");
        }

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                reservation_id: "res-oversized-symbol".to_string(),
                path: path.clone(),
                content: Some(
                    [
                        "fn oversized_symbol() {",
                        "    let alpha = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let beta = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let gamma = very_long_identifier_name_for_a_large_symbol_payload();",
                        "",
                        "    let delta = very_long_identifier_name_for_a_large_symbol_payload();",
                        "}",
                    ]
                    .join("\n"),
                ),
                extraction: ExtractionResult {
                    project_code: Some("PRJ".to_string()),
                    symbols: vec![Symbol {
                        name: "oversized_symbol".to_string(),
                        kind: "function".to_string(),
                        start_line: 1,
                        end_line: 9,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    }],
                    relations: vec![],
                },
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-oversized-symbol".to_string(),
                observed_cost_bytes: 1,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        let raw = store
            .query_json(
                "SELECT id, source_id, content, chunk_part_index, chunk_part_count, chunk_path FROM Chunk \
                 WHERE file_path = '/tmp/oversized_symbol.rs' AND project_code = 'PRJ' \
                 ORDER BY chunk_part_index, id",
            )
            .unwrap();
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        assert!(rows.len() > 1);
        let source_ids = rows
            .iter()
            .map(|row| row[1].as_str().unwrap_or_default().to_string())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(source_ids.len(), 1);
        let only_source_id = source_ids.iter().next().cloned().unwrap_or_default();
        assert!(only_source_id.starts_with("PRJ::"));
        assert!(only_source_id.ends_with("::oversized_symbol"));
        assert!(rows
            .iter()
            .all(|row| row[0].as_str().unwrap_or_default().contains("::chunk")));
        assert!(rows.iter().all(|row| row[2]
            .as_str()
            .unwrap_or_default()
            .contains("symbol: oversized_symbol")));
        let part_count = parse_i64_field(&rows[0][4]).unwrap_or_default();
        assert_eq!(part_count as usize, rows.len());
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(
                parse_i64_field(&row[3]).unwrap_or_default(),
                (index + 1) as i64
            );
            assert_eq!(parse_i64_field(&row[4]).unwrap_or_default(), part_count);
            assert_eq!(
                row[5].as_str().unwrap_or_default(),
                format!("{}/{}", index + 1, rows.len())
            );
        }

        unsafe {
            std::env::remove_var("AXON_TARGET_CHUNK_TOKENS");
            std::env::remove_var("AXON_SMALL_SYMBOL_CHAR_FAST_PATH");
            std::env::remove_var("AXON_GRAY_ZONE_CHAR_THRESHOLD");
        }
    }

    #[test]
    fn full_file_extraction_with_chunks_starts_not_vector_ready_and_enqueues_vectorization() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/full_vector_seed.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();

        store
            .insert_file_data_batch(&[DbWriteTask::FileExtraction {
                reservation_id: "res-full-vector-seed".to_string(),
                path: path.clone(),
                content: Some("fn full_vector_seed() { hydrate(); }".to_string()),
                extraction: ExtractionResult {
                    project_code: Some("PRJ".to_string()),
                    symbols: vec![Symbol {
                        name: "full_vector_seed".to_string(),
                        kind: "function".to_string(),
                        start_line: 1,
                        end_line: 1,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    }],
                    relations: vec![],
                },
                processing_mode: ProcessingMode::Full,
                trace_id: "trace-full-vector-seed".to_string(),
                observed_cost_bytes: 1,
                t0: 0,
                t1: 0,
                t2: 0,
                t3: 0,
            }])
            .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM File \
                     WHERE path = '/tmp/full_vector_seed.rs' \
                       AND status = 'indexed' \
                       AND file_stage = 'graph_indexed' \
                       AND graph_ready = TRUE \
                       AND vector_ready = FALSE"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM Chunk WHERE file_path = '/tmp/full_vector_seed.rs'"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/full_vector_seed.rs'")
                .unwrap(),
            1
        );
    }

    #[test]
    fn insert_file_data_batch_replay_does_not_duplicate_calls_edges() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/replay_calls.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();

        let make_extraction = || ExtractionResult {
            project_code: Some("PRJ".to_string()),
            symbols: vec![
                Symbol {
                    name: "Proj.Source.call".to_string(),
                    kind: "function".to_string(),
                    start_line: 1,
                    end_line: 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: Default::default(),
                    embedding: None,
                },
                Symbol {
                    name: "Proj.Target.case".to_string(),
                    kind: "function".to_string(),
                    start_line: 2,
                    end_line: 2,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: Default::default(),
                    embedding: None,
                },
            ],
            relations: vec![Relation {
                from: "Proj.Source.call".to_string(),
                to: "Proj.Target.case".to_string(),
                rel_type: "calls".to_string(),
                properties: Default::default(),
            }],
        };

        let make_task = || DbWriteTask::FileExtraction {
            reservation_id: "res-1".to_string(),
            path: path.clone(),
            content: Some("fn a() {}".to_string()),
            extraction: make_extraction(),
            processing_mode: ProcessingMode::Full,
            trace_id: "trace-1".to_string(),
            observed_cost_bytes: 1,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store.insert_file_data_batch(&[make_task()]).unwrap();
        store.insert_file_data_batch(&[make_task()]).unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM CALLS WHERE project_code = 'PRJ'")
                .unwrap(),
            1
        );
    }

    #[test]
    fn insert_file_data_batch_rewrites_shared_global_calls_edges_without_duplicates() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path_a = "/tmp/shared_calls_a.ex".to_string();
        let path_b = "/tmp/shared_calls_b.ex".to_string();

        store
            .bulk_insert_files(&[
                (path_a.clone(), "PRJ".to_string(), 42, 1),
                (path_b.clone(), "PRJ".to_string(), 42, 1),
            ])
            .unwrap();

        let make_task = |path: &str| DbWriteTask::FileExtraction {
            reservation_id: format!("res-{path}"),
            path: path.to_string(),
            content: Some("def call, do: :ok".to_string()),
            extraction: ExtractionResult {
                project_code: Some("PRJ".to_string()),
                symbols: vec![
                    Symbol {
                        name: "Proj.Source.call".to_string(),
                        kind: "function".to_string(),
                        start_line: 1,
                        end_line: 1,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    },
                    Symbol {
                        name: "Proj.Target.case".to_string(),
                        kind: "function".to_string(),
                        start_line: 2,
                        end_line: 2,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: Default::default(),
                        embedding: None,
                    },
                ],
                relations: vec![Relation {
                    from: "Proj.Source.call".to_string(),
                    to: "Proj.Target.case".to_string(),
                    rel_type: "calls".to_string(),
                    properties: Default::default(),
                }],
            },
            processing_mode: ProcessingMode::Full,
            trace_id: format!("trace-{path}"),
            observed_cost_bytes: 1,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store.insert_file_data_batch(&[make_task(&path_a)]).unwrap();
        store.insert_file_data_batch(&[make_task(&path_b)]).unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM CALLS WHERE project_code = 'PRJ'")
                .unwrap(),
            1
        );
    }

    #[test]
    fn bulk_insert_files_replay_keeps_single_row_per_path() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/replay_file_row.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 10, 1)])
            .unwrap();
        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 10, 1)])
            .unwrap();

        assert_eq!(
            store
                .query_count(&format!(
                    "SELECT count(*) FROM File WHERE path = '{}'",
                    path
                ))
                .unwrap(),
            1
        );
    }

    #[test]
    fn fetch_pending_file_vectorization_work_sets_exact_claim_token_batch() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
                 ('/tmp/claim_a.rs', 'queued', 1), \
                 ('/tmp/claim_b.rs', 'queued', 2), \
                 ('/tmp/claim_c.rs', 'queued', 3)",
            )
            .unwrap();

        let claimed = store.fetch_pending_file_vectorization_work(2).unwrap();
        assert_eq!(claimed.len(), 2);
        assert_eq!(claimed[0].file_path, "/tmp/claim_a.rs");
        assert_eq!(claimed[1].file_path, "/tmp/claim_b.rs");

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'"
                )
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(DISTINCT claim_token) FROM FileVectorizationQueue WHERE claim_token IS NOT NULL"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE claim_token IS NOT NULL AND claimed_at_ms IS NOT NULL"
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn fetch_pending_file_vectorization_work_reads_claimed_rows_from_writer_when_reader_is_stale() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
                 ('/tmp/stale-claim-a.rs', 'queued', 1), \
                 ('/tmp/stale-claim-b.rs', 'queued', 2)",
            )
            .unwrap();
        store.refresh_reader_snapshot().unwrap();

        let claimed = store.fetch_pending_file_vectorization_work(2).unwrap();
        assert_eq!(claimed.len(), 2);
        assert_eq!(claimed[0].file_path, "/tmp/stale-claim-a.rs");
        assert_eq!(claimed[1].file_path, "/tmp/stale-claim-b.rs");
    }

    #[test]
    fn enqueue_vector_persist_outbox_handoff_moves_lease_owner_and_exposes_work() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
                 VALUES ('/tmp/outbox_handoff.rs', 'inflight', 1, 'claim-outbox-handoff', 1, 1, 'vector', 0)",
            )
            .unwrap();

        let payload = VectorPersistOutboxPayload {
            updates: vec![VectorPersistOutboxUpdate {
                chunk_id: "chunk-outbox".to_string(),
                source_hash: "hash-outbox".to_string(),
                vector: vec![0.1_f32, 0.2_f32],
            }],
            completed_works: vec![FileVectorizationWork {
                file_path: "/tmp/outbox_handoff.rs".to_string(),
                resumed_after_interactive_pause: false,
            }],
            completed_lease_snapshots: vec![FileVectorizationLeaseSnapshot {
                file_path: "/tmp/outbox_handoff.rs".to_string(),
                claim_token: "claim-outbox-handoff".to_string(),
                lease_epoch: 1,
            }],
            batch_run: VectorBatchRun {
                run_id: "outbox-handoff-test".to_string(),
                prepare_started_at_ms: 0,
                prepare_finished_at_ms: 0,
                ready_enqueued_at_ms: 0,
                started_at_ms: 1,
                finished_at_ms: 1,
                gpu_started_at_ms: 0,
                gpu_finished_at_ms: 0,
                persist_enqueued_at_ms: 0,
                persist_started_at_ms: 0,
                persist_finished_at_ms: 0,
                finalize_enqueued_at_ms: 0,
                finalize_finished_at_ms: 0,
                provider: "cpu".to_string(),
                runner_kind: "test".to_string(),
                model_id: CHUNK_EMBEDDING_MODEL_ID.to_string(),
                chunk_count: 1,
                file_count: 1,
                input_bytes: 16,
                total_tokens: 0,
                max_item_tokens: 0,
                avg_item_tokens: 0.0,
                micro_batch_count: 0,
                max_micro_batch_tokens: 0,
                avg_micro_batch_tokens: 0.0,
                effective_vector_workers_admitted: 0,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 0,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: String::new(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 0,
                finalize_queue_wait_ms: 0,
                batch_lane: "mixed".to_string(),
                batch_shape: "homogeneous".to_string(),
                lane_small_max_tokens: 0,
                lane_medium_max_tokens: 0,
                fetch_ms: 1,
                embed_ms: 1,
                db_write_ms: 0,
                mark_done_ms: 0,
                success: true,
                error_reason: None,
            },
        };

        let outbox_id = store
            .enqueue_vector_persist_outbox_handoff(
                &payload,
                &[FileVectorizationLeaseSnapshot {
                    file_path: "/tmp/outbox_handoff.rs".to_string(),
                    claim_token: "claim-outbox-handoff".to_string(),
                    lease_epoch: 0,
                }],
            )
            .unwrap();

        let pending = store.fetch_pending_vector_persist_outbox_work(1).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].outbox_id, outbox_id);
        assert_eq!(pending[0].payload.batch_run.run_id, "outbox-handoff-test");
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/outbox_handoff.rs' \
                       AND lease_owner = 'outbox' \
                       AND COALESCE(lease_epoch, 0) = 1"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn fetch_pending_vector_persist_outbox_work_reads_claimed_rows_from_writer_when_reader_is_stale(
    ) {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let payload = VectorPersistOutboxPayload {
            updates: vec![VectorPersistOutboxUpdate {
                chunk_id: "chunk-stale-outbox".to_string(),
                source_hash: "hash-stale-outbox".to_string(),
                vector: vec![0.3_f32, 0.4_f32],
            }],
            completed_works: vec![],
            completed_lease_snapshots: vec![],
            batch_run: VectorBatchRun {
                run_id: "outbox-stale-reader-test".to_string(),
                prepare_started_at_ms: 0,
                prepare_finished_at_ms: 0,
                ready_enqueued_at_ms: 0,
                started_at_ms: 1,
                finished_at_ms: 1,
                gpu_started_at_ms: 0,
                gpu_finished_at_ms: 0,
                persist_enqueued_at_ms: 0,
                persist_started_at_ms: 0,
                persist_finished_at_ms: 0,
                finalize_enqueued_at_ms: 0,
                finalize_finished_at_ms: 0,
                provider: "cpu".to_string(),
                runner_kind: "test".to_string(),
                model_id: CHUNK_EMBEDDING_MODEL_ID.to_string(),
                chunk_count: 1,
                file_count: 0,
                input_bytes: 16,
                total_tokens: 0,
                max_item_tokens: 0,
                avg_item_tokens: 0.0,
                micro_batch_count: 0,
                max_micro_batch_tokens: 0,
                avg_micro_batch_tokens: 0.0,
                effective_vector_workers_admitted: 0,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 0,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: String::new(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 0,
                finalize_queue_wait_ms: 0,
                batch_lane: "mixed".to_string(),
                batch_shape: "homogeneous".to_string(),
                lane_small_max_tokens: 0,
                lane_medium_max_tokens: 0,
                fetch_ms: 1,
                embed_ms: 1,
                db_write_ms: 0,
                mark_done_ms: 0,
                success: true,
                error_reason: None,
            },
        };
        let payload_json = serde_json::to_string(&payload).unwrap();
        store
            .execute(&format!(
                "INSERT INTO VectorPersistOutbox (outbox_id, status, queued_at_ms, payload_json) \
                 VALUES ('stale-outbox-1', 'queued', 1, '{}')",
                crate::graph::GraphStore::escape_sql(&payload_json)
            ))
            .unwrap();
        store.refresh_reader_snapshot().unwrap();

        let claimed = store.fetch_pending_vector_persist_outbox_work(1).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].outbox_id, "stale-outbox-1");
        assert_eq!(
            claimed[0].payload.batch_run.run_id,
            "outbox-stale-reader-test"
        );
    }

    #[test]
    fn capture_file_vectorization_lease_snapshots_reads_writer_truth_when_reader_is_stale() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
                 ('/tmp/stale-lease-snapshot.rs', 'inflight', 1, 'claim-stale-lease', 1, 1, 'vector', 2)",
            )
            .unwrap();
        store.refresh_reader_snapshot().unwrap();
        store
            .execute(
                "UPDATE FileVectorizationQueue \
                 SET lease_epoch = 3 \
                 WHERE file_path = '/tmp/stale-lease-snapshot.rs'",
            )
            .unwrap();

        let snapshots = store
            .capture_file_vectorization_lease_snapshots(
                &[FileVectorizationWork {
                    file_path: "/tmp/stale-lease-snapshot.rs".to_string(),
                    resumed_after_interactive_pause: false,
                }],
                "vector",
            )
            .unwrap();

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].file_path, "/tmp/stale-lease-snapshot.rs");
        assert_eq!(snapshots[0].claim_token, "claim-stale-lease");
        assert_eq!(snapshots[0].lease_epoch, 3);
    }

    #[test]
    fn record_vector_worker_fault_persists_latest_fault_and_lane_state() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let fault = VectorWorkerFault {
            fault_id: "fault-vector-1".to_string(),
            lane: "vector".to_string(),
            worker_id: 7,
            fatal_stage: "embed".to_string(),
            fatal_reason_raw: "onnxruntime failure".to_string(),
            fatal_class: "onnxruntime".to_string(),
            provider: "cuda".to_string(),
            batch_id: Some("batch-1".to_string()),
            texts_count: 96,
            input_bytes: 8192,
            vram_used_mb: 4096,
            occurred_at_ms: 1234,
            restart_attempt: 2,
        };
        store.record_vector_worker_fault(&fault).unwrap();
        store
            .upsert_vector_lane_state(&VectorLaneStateRecord {
                lane: "vector".to_string(),
                state: "degraded".to_string(),
                reason: Some("recent fatal embed".to_string()),
                updated_at_ms: 1235,
                worker_id: Some(7),
                restart_attempt: 2,
                last_success_at_ms: Some(1200),
                last_fault_id: Some("fault-vector-1".to_string()),
            })
            .unwrap();

        let persisted_fault = store
            .latest_vector_worker_fault("vector")
            .unwrap()
            .expect("latest fault");
        assert_eq!(persisted_fault, fault);

        let lane_state = store
            .vector_lane_state_record("vector")
            .unwrap()
            .expect("lane state");
        assert_eq!(lane_state.state, "degraded");
        assert_eq!(lane_state.last_fault_id.as_deref(), Some("fault-vector-1"));
        assert_eq!(lane_state.restart_attempt, 2);
    }

    #[test]
    fn recover_stale_inflight_file_vectorization_work_only_requeues_expired_claims() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms) VALUES \
                 ('/tmp/stale.rs', 'inflight', 1, 'claim-stale', 1_000, 1_000), \
                 ('/tmp/fresh.rs', 'inflight', 2, 'claim-fresh', 9_500, 9_500), \
                 ('/tmp/queued.rs', 'queued', 3, NULL, NULL, NULL)",
            )
            .unwrap();

        let recovered = store
            .recover_stale_inflight_file_vectorization_work(10_000, 1_000)
            .unwrap();

        assert_eq!(recovered, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/stale.rs' \
                       AND status = 'queued' \
                       AND status_reason = 'recovered_after_stale_inflight' \
                       AND claim_token IS NULL \
                       AND claimed_at_ms IS NULL"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/fresh.rs' \
                       AND status = 'inflight' \
                       AND claim_token = 'claim-fresh' \
                       AND claimed_at_ms = 9500"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/queued.rs' \
                       AND status = 'queued'"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn refresh_inflight_file_vectorization_claims_updates_only_live_rows() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner) VALUES \
                 ('/tmp/live.rs', 'inflight', 1, 'claim-live', 1_000, 1_000, 'vector'), \
                 ('/tmp/queued.rs', 'queued', 2, NULL, NULL, NULL, NULL)",
            )
            .unwrap();

        let refreshed = store
            .refresh_inflight_file_vectorization_claims(&[FileVectorizationWork {
                file_path: "/tmp/live.rs".to_string(),
                resumed_after_interactive_pause: false,
            }])
            .unwrap();

        assert_eq!(refreshed, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/live.rs' \
                       AND status = 'inflight' \
                       AND claimed_at_ms > 1000 \
                       AND lease_heartbeat_at_ms > 1000"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/queued.rs' \
                       AND status = 'queued' \
                       AND claimed_at_ms IS NULL"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn recover_stale_inflight_file_vectorization_work_respects_recent_lease_heartbeat() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms) VALUES \
                 ('/tmp/live-tail.rs', 'inflight', 1, 'claim-live-tail', 1_000, 9_750)",
            )
            .unwrap();

        let recovered = store
            .recover_stale_inflight_file_vectorization_work(10_000, 1_000)
            .unwrap();

        assert_eq!(recovered, 0);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/live-tail.rs' \
                       AND status = 'inflight' \
                       AND claim_token = 'claim-live-tail' \
                       AND lease_heartbeat_at_ms = 9750"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn hourly_rollup_does_not_assign_batch_timings_to_multiple_projects() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
             ('chunk-a', 'symbol', 'sym-a', 'PJA', '/tmp/a.rs', 'function', 'fn a() {}', 'hash-a', 1, 1), \
             ('chunk-b', 'symbol', 'sym-b', 'PJB', '/tmp/b.rs', 'function', 'fn b() {}', 'hash-b', 1, 1)"
        ).unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, vector_ready, vector_ready_at_ms) VALUES \
             ('/tmp/a.rs', 'PJA', TRUE, 1000), \
             ('/tmp/b.rs', 'PJB', TRUE, 1000)",
            )
            .unwrap();
        store
            .update_chunk_embeddings(
                "chunk-bge-large-en-v1.5",
                &[
                    (
                        "chunk-a".to_string(),
                        "hash-a".to_string(),
                        vec![0.1; DIMENSION],
                    ),
                    (
                        "chunk-b".to_string(),
                        "hash-b".to_string(),
                        vec![0.2; DIMENSION],
                    ),
                ],
            )
            .unwrap();
        store
            .record_vector_batch_run(&super::VectorBatchRun {
                run_id: "run-1".to_string(),
                prepare_started_at_ms: 0,
                prepare_finished_at_ms: 0,
                ready_enqueued_at_ms: 0,
                started_at_ms: 900,
                finished_at_ms: 1000,
                gpu_started_at_ms: 920,
                gpu_finished_at_ms: 940,
                persist_enqueued_at_ms: 941,
                persist_started_at_ms: 942,
                persist_finished_at_ms: 970,
                finalize_enqueued_at_ms: 971,
                finalize_finished_at_ms: 1000,
                provider: "cuda".to_string(),
                runner_kind: "test".to_string(),
                model_id: "chunk-bge-large-en-v1.5".to_string(),
                chunk_count: 2,
                file_count: 2,
                input_bytes: 100,
                total_tokens: 0,
                max_item_tokens: 0,
                avg_item_tokens: 0.0,
                micro_batch_count: 0,
                max_micro_batch_tokens: 0,
                avg_micro_batch_tokens: 0.0,
                effective_vector_workers_admitted: 0,
                ready_queue_depth_at_gpu_start: 0,
                prepare_inflight_at_gpu_start: 0,
                ready_queue_chunks_at_gpu_start: 0,
                prepare_inflight_chunks_at_gpu_start: 0,
                vector_worker_admission_reason: String::new(),
                allowed_gpu_workers: 0,
                batch_wait_for_ready_ms: 0,
                persist_queue_wait_ms: 1,
                finalize_queue_wait_ms: 1,
                batch_lane: "mixed".to_string(),
                batch_shape: "homogeneous".to_string(),
                lane_small_max_tokens: 0,
                lane_medium_max_tokens: 0,
                fetch_ms: 10,
                embed_ms: 20,
                db_write_ms: 30,
                mark_done_ms: 40,
                success: true,
                error_reason: None,
            })
            .unwrap();

        store
            .refresh_hourly_vectorization_rollup(0, "chunk-bge-large-en-v1.5")
            .unwrap();

        let raw = store
            .query_json(
                "SELECT project_code, batches, fetch_ms_total, embed_ms_total, db_write_ms_total, mark_done_ms_total \
                 FROM HourlyVectorizationRollup \
                 WHERE bucket_start_ms = 0 AND model_id = 'chunk-bge-large-en-v1.5' \
                 ORDER BY project_code",
            )
            .unwrap();
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap();
        assert_eq!(rows.len(), 2);
        let as_u64 = |value: &serde_json::Value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().map(|raw| raw.max(0) as u64))
                .or_else(|| value.as_f64().map(|raw| raw.max(0.0) as u64))
                .unwrap_or(0)
        };
        for row in rows {
            assert_eq!(as_u64(&row[1]), 0);
            assert_eq!(as_u64(&row[2]), 0);
            assert_eq!(as_u64(&row[3]), 0);
            assert_eq!(as_u64(&row[4]), 0);
            assert_eq!(as_u64(&row[5]), 0);
        }
    }

    #[test]
    fn vector_batch_run_table_is_not_bootstrapped_in_ist() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM sqlite_master \
                     WHERE type = 'table' \
                       AND name = 'VectorBatchRun'"
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn fetch_segments_for_file_reads_writer_when_reader_snapshot_is_stale() {
        use std::sync::atomic::Ordering;

        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.refresh_reader_snapshot().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-stale', 'symbol', 'sym-stale', 'PRJ', '/tmp/stale.rs', 'function', 'fresh', 'hash-stale', 1, 2)",
            )
            .unwrap();

        store.recent_write_epoch_ms.store(0, Ordering::Relaxed);

        let chunks = store.fetch_segments_for_file("/tmp/stale.rs").unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, "chunk-stale");
    }

    #[test]
    fn enqueue_file_vectorization_refresh_skips_already_vector_ready_files() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/ready.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, TRUE)",
            )
            .unwrap();

        store
            .enqueue_file_vectorization_refresh("/tmp/ready.rs")
            .unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/ready.rs'"
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn enqueue_file_vectorization_refresh_adds_files_needing_vectorization() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/not_ready.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
            )
            .unwrap();

        store
            .enqueue_file_vectorization_refresh("/tmp/not_ready.rs")
            .unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/not_ready.rs'")
                .unwrap(),
            1
        );
    }

    #[test]
    fn insert_file_data_batch_does_not_queue_files_without_vectorizable_chunks() {
        use crate::parser::ExtractionResult;
        use crate::worker::DbWriteTask;

        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/no_chunks.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 10, 1)])
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES ('/tmp/no_chunks.rs', 'queued', 1)",
            )
            .unwrap();

        let task = DbWriteTask::FileExtraction {
            reservation_id: "res-no-chunks".to_string(),
            path: path.clone(),
            content: Some("".to_string()),
            extraction: ExtractionResult {
                project_code: Some("PRJ".to_string()),
                symbols: vec![],
                relations: vec![],
            },
            processing_mode: ProcessingMode::Full,
            trace_id: "trace-no-chunks".to_string(),
            observed_cost_bytes: 1,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store.insert_file_data_batch(&[task]).unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/no_chunks.rs'")
                .unwrap(),
            0
        );
    }

    #[test]
    fn structure_only_file_extraction_marks_file_vector_ready_and_does_not_enqueue_vectorization() {
        use crate::parser::ExtractionResult;
        use crate::worker::DbWriteTask;

        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/structure_only.rs".to_string();
        store
            .bulk_insert_files(&[(path.clone(), "PRJ".to_string(), 42, 1)])
            .unwrap();

        let task = DbWriteTask::FileExtraction {
            reservation_id: "res-structure-only".to_string(),
            path: path.clone(),
            content: None,
            extraction: ExtractionResult {
                project_code: Some("PRJ".to_string()),
                symbols: vec![Symbol {
                    name: "structure_only_fn".to_string(),
                    kind: "function".to_string(),
                    start_line: 1,
                    end_line: 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: Default::default(),
                    embedding: None,
                }],
                relations: vec![],
            },
            processing_mode: ProcessingMode::StructureOnly,
            trace_id: "trace-structure-only".to_string(),
            observed_cost_bytes: 1,
            t0: 0,
            t1: 0,
            t2: 0,
            t3: 0,
        };

        store.insert_file_data_batch(&[task]).unwrap();

        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM File \
                     WHERE path = '/tmp/structure_only.rs' \
                       AND status = 'indexed_degraded' \
                       AND file_stage = 'graph_indexed' \
                       AND graph_ready = TRUE \
                       AND vector_ready = TRUE"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM Chunk WHERE file_path = '/tmp/structure_only.rs'"
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/structure_only.rs'")
                .unwrap(),
            0
        );
    }

    #[test]
    fn backfill_file_vectorization_queue_skips_files_already_present_in_queue() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/already_queued.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-already-queued', 'symbol', 'sym-already-queued', 'PRJ', '/tmp/already_queued.rs', 'function', 'body', 'hash-already-queued', 1, 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id, project_code) \
                 VALUES ('/tmp/already_queued.rs', 'sym-already-queued', 'PRJ')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) \
                 VALUES ('/tmp/already_queued.rs', 'queued', 1)",
            )
            .unwrap();

        let inserted = store.backfill_file_vectorization_queue().unwrap();

        assert_eq!(
            inserted, 0,
            "Le backfill ne doit pas retraiter un fichier deja present dans la queue"
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/already_queued.rs'"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn backfill_file_vectorization_queue_skips_oversized_files() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/oversized.rs', 'PRJ', 'oversized_for_current_budget', 1, 1, 100, 'oversized', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-oversized', 'symbol', 'sym-oversized', 'PRJ', '/tmp/oversized.rs', 'function', 'body', 'hash-oversized', 1, 1)",
            )
            .unwrap();

        let inserted = store.backfill_file_vectorization_queue().unwrap();

        assert_eq!(inserted, 0);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/oversized.rs'"
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn backfill_file_vectorization_queue_reads_writer_truth_when_reader_snapshot_is_stale() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.refresh_reader_snapshot().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/stale-backfill.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-stale-backfill', 'symbol', 'sym-stale-backfill', 'PRJ', '/tmp/stale-backfill.rs', 'function', 'body', 'hash-stale-backfill', 1, 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id, project_code) \
                 VALUES ('/tmp/stale-backfill.rs', 'sym-stale-backfill', 'PRJ')",
            )
            .unwrap();

        let inserted = store.backfill_file_vectorization_queue().unwrap();

        assert_eq!(inserted, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/stale-backfill.rs'"
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn backfill_file_vectorization_queue_requeues_orphaned_graph_ready_file() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/orphan-vector.rs', 'PRJ', 'indexed', 1, 1, 900, 'graph_indexed', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-orphan-vector', 'symbol', 'sym-orphan-vector', 'PRJ', '/tmp/orphan-vector.rs', 'function', 'body', 'hash-orphan-vector', 1, 1)",
            )
            .unwrap();

        assert_eq!(store.count_orphaned_file_vectorization_files().unwrap(), 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/orphan-vector.rs'"
                )
                .unwrap(),
            0
        );

        let inserted = store.backfill_file_vectorization_queue().unwrap();

        assert_eq!(inserted, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue \
                     WHERE file_path = '/tmp/orphan-vector.rs' \
                       AND status = 'queued' \
                       AND status_reason = 'reconciled_orphan_vectorization_state'"
                )
                .unwrap(),
            1
        );
        assert_eq!(store.count_orphaned_file_vectorization_files().unwrap(), 0);
    }

    #[test]
    fn backfill_file_vectorization_queue_only_requeues_requested_paths() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        for path in ["/tmp/orphan-a.rs", "/tmp/orphan-b.rs"] {
            store
                .execute(&format!(
                    "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                     VALUES ('{}', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
                    path
                ))
                .unwrap();
            store
                .execute(&format!(
                    "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                     VALUES ('chunk-{}', 'symbol', 'sym-{}', 'PRJ', '{}', 'function', 'body', 'hash-{}', 1, 1)",
                    path.replace('/', "_"),
                    path.replace('/', "_"),
                    path,
                    path.replace('/', "_"),
                ))
                .unwrap();
        }

        let repaired = store
            .reconcile_orphaned_file_vectorization_paths(&["/tmp/orphan-a.rs".to_string()])
            .unwrap();

        assert_eq!(repaired, 1);
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/orphan-a.rs'"
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .query_count(
                    "SELECT count(*) FROM FileVectorizationQueue WHERE file_path = '/tmp/orphan-b.rs'"
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn rebuild_file_vectorization_queue_with_limit_trims_existing_queue_to_floor() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        for path in ["/tmp/floor-a.rs", "/tmp/floor-b.rs"] {
            store
                .execute(&format!(
                    "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                     VALUES ('{}', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
                    path
                ))
                .unwrap();
            store
                .execute(&format!(
                    "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                     VALUES ('chunk-{}', 'symbol', 'sym-{}', 'PRJ', '{}', 'function', 'body', 'hash-{}', 1, 1)",
                    path.replace('/', "_"),
                    path.replace('/', "_"),
                    path,
                    path.replace('/', "_"),
                ))
                .unwrap();
            store
                .execute(&format!(
                    "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES ('{}', 'queued', 1)",
                    path
                ))
                .unwrap();
        }

        let rebuilt = store
            .rebuild_file_vectorization_queue_with_limit(1)
            .unwrap();

        assert_eq!(rebuilt, 1);
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue")
                .unwrap(),
            1
        );
    }

    #[test]
    fn backfill_file_vectorization_queue_with_limit_adds_missing_work_without_trimming_existing() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        for path in ["/tmp/floor-a.rs", "/tmp/floor-b.rs", "/tmp/floor-c.rs"] {
            store
                .execute(&format!(
                    "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                     VALUES ('{}', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
                    path
                ))
                .unwrap();
            store
                .execute(&format!(
                    "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                     VALUES ('chunk-{}', 'symbol', 'sym-{}', 'PRJ', '{}', 'function', 'body', 'hash-{}', 1, 1)",
                    path.replace('/', "_"),
                    path.replace('/', "_"),
                    path,
                    path.replace('/', "_"),
                ))
                .unwrap();
        }
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES ('/tmp/floor-a.rs', 'queued', 1)",
            )
            .unwrap();

        let added = store
            .backfill_file_vectorization_queue_with_limit(1)
            .unwrap();

        assert_eq!(added, 1);
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue")
                .unwrap(),
            2
        );
    }

    #[test]
    fn claimable_vector_backlog_counts_only_queued_and_paused_rows() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
                 ('/tmp/claimable-queued.rs', 'queued', 1), \
                 ('/tmp/claimable-paused.rs', 'paused_for_interactive_priority', 2), \
                 ('/tmp/claimable-inflight.rs', 'inflight', 3)",
            )
            .unwrap();

        let claimable = store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap();

        assert_eq!(claimable, 2);
    }

    #[test]
    fn claimable_vector_backlog_excludes_inflight_rows() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
                 ('/tmp/claimable-live.rs', 'inflight', 1, 'claim-live', 1_000, 1_000, 'vector', 1)",
            )
            .unwrap();

        let claimable = store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap();

        assert_eq!(claimable, 0);
    }

    #[test]
    fn claimable_vector_backlog_excludes_persist_outbox_rows() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
                 ('/tmp/outbox-claimed.rs', 'inflight', 1, 'claim-outbox', 1_000, 1_000, 'outbox', 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO VectorPersistOutbox (outbox_id, status, queued_at_ms, payload_json) VALUES \
                 ('outbox-queued-1', 'queued', 1, '{\"updates\":[],\"completed_works\":[],\"completed_lease_snapshots\":[],\"batch_run\":{\"run_id\":\"claimable-outbox\",\"prepare_started_at_ms\":0,\"prepare_finished_at_ms\":0,\"ready_enqueued_at_ms\":0,\"started_at_ms\":1,\"finished_at_ms\":1,\"gpu_started_at_ms\":0,\"gpu_finished_at_ms\":0,\"provider\":\"cpu\",\"model_id\":\"chunk-bge-large-en-v1.5\",\"chunk_count\":0,\"file_count\":0,\"input_bytes\":0,\"total_tokens\":0,\"max_item_tokens\":0,\"avg_item_tokens\":0.0,\"micro_batch_count\":0,\"max_micro_batch_tokens\":0,\"avg_micro_batch_tokens\":0.0,\"effective_vector_workers_admitted\":0,\"ready_queue_depth_at_gpu_start\":0,\"prepare_inflight_at_gpu_start\":0,\"ready_queue_chunks_at_gpu_start\":0,\"prepare_inflight_chunks_at_gpu_start\":0,\"vector_worker_admission_reason\":\"\",\"allowed_gpu_workers\":0,\"batch_wait_for_ready_ms\":0,\"fetch_ms\":0,\"embed_ms\":0,\"db_write_ms\":0,\"mark_done_ms\":0,\"success\":true,\"error_reason\":null}}')",
            )
            .unwrap();

        let claimable = store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap();

        assert_eq!(claimable, 0);
    }

    #[test]
    fn claimable_vector_backlog_excludes_not_yet_eligible_rows() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let future_ms = chrono::Utc::now().timestamp_millis().saturating_add(60_000);
        store
            .execute(&format!(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, next_eligible_at_ms) VALUES \
                 ('/tmp/not-yet-eligible.rs', 'queued', 1, {})",
                future_ms
            ))
            .unwrap();

        let claimable = store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap();

        assert_eq!(claimable, 0);
    }

    #[test]
    fn claimable_vector_backlog_excludes_file_rows_that_are_already_vector_ready() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, mtime, size, file_stage, graph_ready, vector_ready) VALUES \
                 ('/tmp/already-vector-ready.rs', 'PRJ', 'indexed', 1, 1, 'indexed', TRUE, TRUE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES \
                 ('/tmp/already-vector-ready.rs', 'queued', 1)",
            )
            .unwrap();

        let claimable = store
            .fetch_claimable_file_vectorization_queue_count()
            .unwrap();

        assert_eq!(claimable, 0);
    }

    #[test]
    fn count_stale_inflight_file_vectorization_files_distinguishes_stale_from_fresh() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.execute(
            "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) VALUES \
             ('/tmp/stale.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE), \
             ('/tmp/fresh.rs', 'PRJ', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)"
        ).unwrap();
        store.execute(
            "INSERT INTO FileVectorizationQueue (file_path, status, queued_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) VALUES \
             ('/tmp/stale.rs', 'inflight', 1, 'claim-stale', 1_000, 1_000, 'vector', 1), \
             ('/tmp/fresh.rs', 'inflight', 1, 'claim-fresh', 9_800, 9_800, 'vector', 1)"
        ).unwrap();

        let stale = store
            .count_stale_inflight_file_vectorization_files(10_000, 1_000)
            .unwrap();

        assert_eq!(stale, 1);
    }
}

impl GraphStore {
    pub fn count_persisted_file_pending(&self) -> Result<usize> {
        Ok(usize::try_from(self.query_count(
            "SELECT count(*) FROM File \
             WHERE status = 'pending' \
               AND COALESCE(graph_ready, FALSE) = FALSE \
               AND COALESCE(file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget')",
        )?)
        .unwrap_or(0))
    }

    pub fn count_graph_wip_files(&self) -> Result<usize> {
        Ok(usize::try_from(self.query_count(
            "SELECT count(*) FROM File \
             WHERE status = 'indexing' \
               AND COALESCE(graph_ready, FALSE) = FALSE \
               AND COALESCE(file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget')",
        )?)
        .unwrap_or(0))
    }

    fn upsert_file_queries(
        path: &str,
        project: &str,
        size: i64,
        mtime: i64,
        priority: i64,
        source: FileUpsertSource,
    ) -> Vec<String> {
        let discovered_reason = match source {
            FileUpsertSource::Scan => "scan_identified",
            FileUpsertSource::HotDelta => "watcher_hot_identified",
        };
        let metadata_changed_reason = match source {
            FileUpsertSource::Scan => "metadata_changed_scan",
            FileUpsertSource::HotDelta => "metadata_changed_hot_delta",
        };
        let now_ms = chrono::Utc::now().timestamp_millis();
        let safe_path = Self::escape_sql(path);
        let safe_project = Self::escape_sql(project);
        let safe_discovered_reason = Self::escape_sql(discovered_reason);
        let safe_reason = Self::escape_sql(metadata_changed_reason);

        vec![
            format!(
                "INSERT INTO Project (name) VALUES ('{}') ON CONFLICT DO NOTHING;",
                safe_project
            ),
            format!(
                "UPDATE File SET \
                    project_code='{safe_project}', \
                    size={size}, \
                    mtime={mtime}, \
                    status = CASE \
                        WHEN File.status = 'indexing' THEN File.status \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'pending' \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN 'pending' \
                        ELSE File.status \
                    END, \
                    priority = {priority}, \
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
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'manual_or_system_requeue' \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} THEN '{safe_reason}' \
                        WHEN File.project_code IS DISTINCT FROM '{safe_project}' THEN 'manual_or_system_requeue' \
                        WHEN File.priority IS DISTINCT FROM {priority} THEN 'priority_adjusted_no_requeue' \
                        ELSE COALESCE(File.status_reason, 'stable_metadata_no_requeue') \
                    END, \
                    file_stage = CASE \
                        WHEN File.status = 'indexing' THEN File.file_stage \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'promoted' \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN 'promoted' \
                        ELSE File.file_stage \
                    END, \
                    graph_ready = CASE \
                        WHEN File.status = 'indexing' THEN File.graph_ready \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN FALSE \
                        ELSE File.graph_ready \
                    END, \
                    vector_ready = CASE \
                        WHEN File.status = 'indexing' THEN File.vector_ready \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN FALSE \
                        ELSE File.vector_ready \
                    END, \
                    defer_count = CASE \
                        WHEN File.status = 'indexing' THEN File.defer_count \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 0 \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN 0 \
                        ELSE File.defer_count \
                    END, \
                    last_deferred_at_ms = CASE \
                        WHEN File.status = 'indexing' THEN File.last_deferred_at_ms \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN NULL \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN NULL \
                        ELSE File.last_deferred_at_ms \
                    END, \
                    last_state_change_at_ms = CASE \
                        WHEN File.project_code IS DISTINCT FROM '{safe_project}' \
                             OR File.mtime IS DISTINCT FROM {mtime} \
                             OR File.size IS DISTINCT FROM {size} \
                             OR File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                             OR File.priority IS DISTINCT FROM {priority} \
                        THEN {now_ms} \
                        ELSE File.last_state_change_at_ms \
                    END, \
                    needs_reindex = CASE \
                        WHEN File.status = 'indexing' \
                             AND (File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size}) \
                        THEN TRUE \
                        WHEN File.status = 'indexing' THEN File.needs_reindex \
                        WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                        WHEN File.mtime IS DISTINCT FROM {mtime} OR File.size IS DISTINCT FROM {size} OR File.project_code IS DISTINCT FROM '{safe_project}' THEN FALSE \
                        ELSE File.needs_reindex \
                    END \
                 WHERE path = '{safe_path}' AND ( \
                    File.project_code IS DISTINCT FROM '{safe_project}' \
                    OR File.mtime IS DISTINCT FROM {mtime} \
                    OR File.size IS DISTINCT FROM {size} \
                    OR File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                    OR File.priority IS DISTINCT FROM {priority} \
                 );"
            ),
            format!(
                "INSERT INTO File (path, project_code, size, mtime, status, priority, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, first_seen_at_ms, last_state_change_at_ms, file_stage, graph_ready, vector_ready) \
                 VALUES ('{}', '{}', {}, {}, 'pending', {}, FALSE, NULL, '{}', 0, NULL, {}, {}, 'promoted', FALSE, FALSE) \
                 ON CONFLICT(path) DO NOTHING;",
                safe_path,
                safe_project,
                size,
                mtime,
                priority,
                safe_discovered_reason,
                now_ms,
                now_ms
            ),
            graph_projection_queue_upsert_if_needed_for_file(path, now_ms),
        ]
    }

    fn bulk_upsert_file_queries(
        rows: &[(String, String, i64, i64, i64, FileUpsertSource)],
    ) -> Vec<String> {
        let deduped = dedup_file_batch_rows(rows);
        if deduped.is_empty() {
            return Vec::new();
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let values = deduped
            .iter()
            .map(|(path, project, size, mtime, priority, source)| {
                let (discovered_reason, metadata_changed_reason) = match source {
                    FileUpsertSource::Scan => ("scan_identified", "metadata_changed_scan"),
                    FileUpsertSource::HotDelta => {
                        ("watcher_hot_identified", "metadata_changed_hot_delta")
                    }
                };
                format!(
                    "('{}', '{}', {}, {}, {}, '{}', '{}')",
                    Self::escape_sql(path),
                    Self::escape_sql(project),
                    size,
                    mtime,
                    priority,
                    Self::escape_sql(discovered_reason),
                    Self::escape_sql(metadata_changed_reason),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let src_cte = format!(
            "WITH src(path, project_code, size, mtime, priority, discovered_reason, metadata_changed_reason) AS (VALUES {})",
            values
        );

        vec![
            format!(
                "{src_cte} \
                 INSERT INTO Project (name) \
                 SELECT DISTINCT project_code FROM src \
                 ON CONFLICT DO NOTHING;"
            ),
            format!(
                "{src_cte} \
                 UPDATE File \
                 SET project_code = src.project_code, \
                     size = src.size, \
                     mtime = src.mtime, \
                     status = CASE \
                         WHEN File.status = 'indexing' THEN File.status \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'pending' \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN 'pending' \
                         ELSE File.status \
                     END, \
                     priority = src.priority, \
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
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'manual_or_system_requeue' \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size THEN src.metadata_changed_reason \
                         WHEN File.project_code IS DISTINCT FROM src.project_code THEN 'manual_or_system_requeue' \
                         WHEN File.priority IS DISTINCT FROM src.priority THEN 'priority_adjusted_no_requeue' \
                         ELSE COALESCE(File.status_reason, 'stable_metadata_no_requeue') \
                     END, \
                     file_stage = CASE \
                         WHEN File.status = 'indexing' THEN File.file_stage \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 'promoted' \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN 'promoted' \
                         ELSE File.file_stage \
                     END, \
                     graph_ready = CASE \
                         WHEN File.status = 'indexing' THEN File.graph_ready \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN FALSE \
                         ELSE File.graph_ready \
                     END, \
                     vector_ready = CASE \
                         WHEN File.status = 'indexing' THEN File.vector_ready \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN FALSE \
                         ELSE File.vector_ready \
                     END, \
                     defer_count = CASE \
                         WHEN File.status = 'indexing' THEN File.defer_count \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN 0 \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN 0 \
                         ELSE File.defer_count \
                     END, \
                     last_deferred_at_ms = CASE \
                         WHEN File.status = 'indexing' THEN File.last_deferred_at_ms \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN NULL \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN NULL \
                         ELSE File.last_deferred_at_ms \
                     END, \
                     last_state_change_at_ms = CASE \
                         WHEN File.project_code IS DISTINCT FROM src.project_code \
                              OR File.mtime IS DISTINCT FROM src.mtime \
                              OR File.size IS DISTINCT FROM src.size \
                              OR File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                              OR File.priority IS DISTINCT FROM src.priority \
                         THEN {now_ms} \
                         ELSE File.last_state_change_at_ms \
                     END, \
                     needs_reindex = CASE \
                         WHEN File.status = 'indexing' \
                              AND (File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size) \
                         THEN TRUE \
                         WHEN File.status = 'indexing' THEN File.needs_reindex \
                         WHEN File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') THEN FALSE \
                         WHEN File.mtime IS DISTINCT FROM src.mtime OR File.size IS DISTINCT FROM src.size OR File.project_code IS DISTINCT FROM src.project_code THEN FALSE \
                         ELSE File.needs_reindex \
                     END \
                 FROM src \
                 WHERE File.path = src.path \
                   AND (File.project_code IS DISTINCT FROM src.project_code \
                        OR File.mtime IS DISTINCT FROM src.mtime \
                        OR File.size IS DISTINCT FROM src.size \
                        OR File.status IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                        OR File.priority IS DISTINCT FROM src.priority);"
            ),
            format!(
                "{src_cte} \
                 INSERT INTO File (path, project_code, size, mtime, status, priority, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, first_seen_at_ms, last_state_change_at_ms, file_stage, graph_ready, vector_ready) \
                 SELECT path, project_code, size, mtime, 'pending', priority, FALSE, NULL, discovered_reason, 0, NULL, {now_ms}, {now_ms}, 'promoted', FALSE, FALSE \
                 FROM src \
                 ON CONFLICT(path) DO NOTHING;"
            ),
            format!(
                "{src_cte} \
                 INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) \
                 SELECT 'file', f.path, {DEFAULT_GRAPH_EMBEDDING_RADIUS}, 'queued', 0, {now_ms}, NULL, NULL \
                 FROM File f \
                 JOIN src ON src.path = f.path \
                 WHERE f.status = 'pending' \
                   AND COALESCE(f.file_stage, 'promoted') = 'promoted' \
                   AND COALESCE(f.graph_ready, FALSE) = FALSE \
                   AND COALESCE(f.vector_ready, FALSE) = FALSE \
                   AND f.status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                 ON CONFLICT(anchor_type, anchor_id, radius) DO UPDATE \
                 SET status = 'queued', \
                     attempts = 0, \
                     queued_at = {now_ms}, \
                     last_error_reason = NULL, \
                     last_attempt_at = NULL;"
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
