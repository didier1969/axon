// Copyright (c) Didier Stadelmann. All rights reserved.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::AtomicU64;

use anyhow::{anyhow, Result};

use crate::code_chunker::build_symbol_chunks;
use crate::graph::GraphStore;

// REQ-AXO-901653 slice-5b (recovery): restored consts/enum still consumed by
// sql_helpers + file_ingress + tests. G1 over-deleted these because the File
// state-machine purge stranded the live ingress path. Pipeline_v2 canonical
// keeps file ingress + de-dup but bypasses the queues.
pub(crate) const DEFAULT_GRAPH_EMBEDDING_RADIUS: i64 = 2;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS: i64 = 5_000;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT: i64 = 2;
pub(crate) static FILE_VECTORIZATION_CLAIM_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileUpsertSource {
    Scan,
    HotDelta,
}

pub mod async_writer;
mod file_ingress;
mod sql_helpers;
mod types;
mod vector_runtime;

use sql_helpers::{
    hourly_bucket_start_ms, next_vector_persist_outbox_claim_token, parse_file_ingress_row,
    parse_i64_field, parse_u64_field,
};
pub use types::{
    EmbedderLifecycleHeartbeatRecord, FileLifecycleEvent, FileVectorizationLeaseSnapshot,
    FileVectorizationWork, IgnoreReconcileStats, VectorBatchRun, VectorLaneStateRecord,
    VectorPersistOutboxPayload, VectorPersistOutboxUpdate, VectorPersistOutboxWork,
    VectorWorkerFault,
};

impl GraphStore {
    // REQ-AXO-901653 slice-5a: `claimable_file_vectorization_candidates_query`
    // deleted ; legacy FileVectorizationQueue + File join.

    // ============================================================================
    // REQ-AXO-901653 slice-5b STUBS (transition layer)
    //
    // G1 (slice-5a, commit d8e8f39a) deleted the `public.File` state-machine
    // SQL helpers + `enum FileState`. Their callers form an entire legacy
    // subsystem (worker.rs ~1028 LOC, main_background.rs count_* KPI surfaces,
    // file_ingress.rs upsert helpers, MCP runtime contracts). Deleting all
    // callers safely is multi-session work tracked as REQ-AXO-901653 slice-5c.
    //
    // To restore `cargo check` green NOW, these stubs return zero/empty/Ok.
    // Pipeline_v2 (REQ-AXO-289 / CPT-AXO-054) bypasses the queues entirely
    // by writing Chunk + ChunkEmbedding directly, so these dead callers are
    // observationally noop already — the stubs make the type system agree.
    //
    // Each stub is marked `slice-5b-stub` for grep-based cleanup in slice-5c.
    // ============================================================================

    // ---- count_* / oldest_*_age_ms : KPI display (return 0 — pipeline_v2
    //      tracks via IndexedFile + Chunk + ChunkEmbedding directly) ----
    pub fn count_persisted_file_pending(&self) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn count_graph_wip_files(&self) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn count_orphaned_file_vectorization_files(&self) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn count_stale_inflight_file_vectorization_files(&self, _now_ms: i64, _stale_threshold_ms: i64) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn oldest_graph_pending_age_ms(&self, _now_ms: i64) -> Result<u64> { Ok(0) } // slice-5b-stub
    pub fn oldest_semantic_pending_age_ms(&self, _now_ms: i64) -> Result<u64> { Ok(0) } // slice-5b-stub
    pub fn fetch_claimable_file_vectorization_queue_count(&self) -> Result<usize> { Ok(0) } // slice-5b-stub

    // ---- state-machine no-ops (legacy callers, pipeline_v2 bypasses) ----
    pub fn backfill_file_vectorization_queue(&self) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn backfill_file_vectorization_queue_with_limit(&self, _limit: usize) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn recover_stale_inflight_file_vectorization_work(&self, _now_ms: i64, _stale_threshold_ms: i64) -> Result<usize> { Ok(0) } // slice-5b-stub
    pub fn fetch_pending_batch(&self, _limit: usize) -> Result<Vec<crate::graph::PendingFile>> { Ok(Vec::new()) } // slice-5b-stub
    pub fn fetch_pending_candidates(&self, _limit: usize) -> Result<Vec<crate::graph::PendingFile>> { Ok(Vec::new()) } // slice-5b-stub
    pub fn mark_pending_files_deferred(&self, _paths: &[String]) -> Result<()> { Ok(()) } // slice-5b-stub
    pub fn mark_file_oversized_for_current_budget(&self, _path: &str) -> Result<()> { Ok(()) } // slice-5b-stub
    pub fn claim_pending_paths(&self, _paths: &[String]) -> Result<Vec<crate::graph::PendingFile>> { Ok(Vec::new()) } // slice-5b-stub
    pub fn requeue_claimed_file_with_reason(&self, _path: &str, _reason: &str) -> Result<()> { Ok(()) } // slice-5b-stub
    pub fn requeue_claimed_paths_with_reason(&self, _paths: &[String], _reason: &str) -> Result<()> { Ok(()) } // slice-5b-stub
    pub fn mark_claimed_file_writer_pending_commit(&self, _path: &str) -> Result<()> { Ok(()) } // slice-5b-stub
    pub fn insert_file_data_batch<T>(&self, _batch: &[T]) -> Result<()> { Ok(()) } // slice-5b-stub

    // ---- file_ingress upsert helpers (file_ingress.rs legacy, slice-5c will delete) ----
    pub fn upsert_file_queries(_path: &str, _project: &str, _at_ms: i64, _stage: i64, _source: i64, _kind: FileUpsertSource) -> Vec<String> { Vec::new() } // slice-5b-stub
    pub fn bulk_upsert_file_queries(_rows: &[(String, String, i64, i64, i64, FileUpsertSource)]) -> Vec<String> { Vec::new() } // slice-5b-stub

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

    // REQ-AXO-901653 slice-5a: `fetch_file_project_metadata` deleted ;
    // queried public.File for (project_code, worker_id, trace_id).

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
        // REQ-AXO-271 slice 2g : PG canonical only. INSERT ON CONFLICT
        // (decision_id) DO UPDATE refreshes every column on conflict ;
        // the DuckDB `INSERT OR REPLACE` arm is dead syntax.
        let sql = format!(
            "INSERT INTO axon_runtime.OptimizerDecisionLog (decision_id, at_ms, mode, host_snapshot_json, policy_snapshot_json, signal_snapshot_json, analytics_snapshot_json, action_profile_id, decision_json, constraints_triggered_json, would_apply, applied, evaluation_window_start_ms, evaluation_window_end_ms) \
             VALUES ('{}', {}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', {}, {}, {}, {}) \
             ON CONFLICT (decision_id) DO UPDATE SET \
                at_ms = EXCLUDED.at_ms, \
                mode = EXCLUDED.mode, \
                host_snapshot_json = EXCLUDED.host_snapshot_json, \
                policy_snapshot_json = EXCLUDED.policy_snapshot_json, \
                signal_snapshot_json = EXCLUDED.signal_snapshot_json, \
                analytics_snapshot_json = EXCLUDED.analytics_snapshot_json, \
                action_profile_id = EXCLUDED.action_profile_id, \
                decision_json = EXCLUDED.decision_json, \
                constraints_triggered_json = EXCLUDED.constraints_triggered_json, \
                would_apply = EXCLUDED.would_apply, \
                applied = EXCLUDED.applied, \
                evaluation_window_start_ms = EXCLUDED.evaluation_window_start_ms, \
                evaluation_window_end_ms = EXCLUDED.evaluation_window_end_ms",
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
        );
        self.execute(&sql)
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

    // REQ-AXO-901653 slice-5a: `next_file_vectorization_claim_token` deleted ;
    // legacy FileVectorizationQueue claim helper.

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

    // REQ-AXO-901653 slice-5a: `oldest_graph_pending_age_ms` +
    // `oldest_semantic_pending_age_ms` deleted ; both joined on
    // File.graph_ready / File.vector_ready / File.status.

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

    // REQ-AXO-901653 slice-5a: `insert_file_data_batch` +
    // `insert_file_data_batch_with_vectorization_policy` deleted ;
    // 800 LOC legacy DbWriteTask sink wrote to public.File +
    // FileVectorizationQueue + GraphProjectionQueue. Pipeline-v2's
    // `upsert_graph_v2_batch` is the canonical successor.

    // REQ-AXO-901653 slice-5a: `fetch_pending_batch`,
    // `fetch_pending_candidates`, `claim_pending_paths`,
    // `mark_file_oversized_for_current_budget`,
    // `mark_pending_files_deferred`, `requeue_claimed_file*`,
    // `mark_claimed_file_writer_pending_commit`,
    // `requeue_claimed_paths_with_reason` deleted ; all wrote to
    // public.File status state machine (pending/indexing/deleted/...) +
    // touched FileVectorizationQueue + GraphProjectionQueue.
    // Pipeline-v2 consumes file paths through the Scanner -> A1 channel,
    // not through SQL claim cursors.

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

    // REQ-AXO-901653 slice-5a: `mark_file_vectorization_done`,
    // `finalize_file_vectorization_success_batch`, and
    // `finalize_file_vectorization_success_batch_for_owner` deleted ;
    // updated File.vector_ready + deleted FileVectorizationQueue rows +
    // upserted GraphProjectionQueue. Pipeline-v2 stage B3 writes
    // ChunkEmbedding rows directly without a per-file lease ledger.

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
        // REQ-AXO-271 slice 2i : PG canonical (post-MIL-AXO-017). Render
        // via pgvector `'[…]'::vector(N)` text literal ; Symbol.embedding
        // is `vector(1024)` (CPT-AXO-043). The DuckDB `CAST AS FLOAT[N]`
        // arm is dead syntax.
        for chunk in updates.chunks(100) {
            for (id, vector) in chunk {
                let embedding_sql = match crate::postgres::vector::vector_literal(vector) {
                    Ok(lit) => lit,
                    Err(e) => {
                        log::warn!(
                            "skipping update_symbol_embeddings for {}: {}",
                            id,
                            e
                        );
                        continue;
                    }
                };
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

        // MIL-AXO-015 P4 slice 4c: under the PostgreSQL backend, route
        // each row through `crate::postgres::vector::upsert_chunk_embedding_sql`
        // which emits the pgvector `'[…]'::vector(N)` text form +
        // `INSERT … ON CONFLICT (chunk_id, model_id) DO UPDATE`.
        //
        // The per-project schema namespacing (CPT-AXO-039) requires a
        // project_code; the indexer's vector_worker_loop currently
        // calls this method without one. Until P9 threads project_code
        // through the worker call sites, we resolve it from the first
        // chunk_id's project prefix (chunk_ids always start with the
        // project_code per the indexer's id-generation contract). For
        // single-project deployments this is exact; for multi-project
        // batches this method MUST be called once per project_code by
        // the worker, which is already the indexer's natural batching
        // boundary.
        // REQ-AXO-271 slice 2j : PG canonical only (post-MIL-AXO-017).
        // The DuckDB `INSERT OR REPLACE INTO ChunkEmbedding ... CAST AS
        // FLOAT[N]` arm is dead syntax + the hourly rollup it triggered
        // is DuckDB-shaped (`refresh_hourly_vectorization_rollup` left
        // for a follow-up if a PG equivalent is ever needed).
        //
        // Every chunk row carries `project_code` (CPT-AXO-039 supersedure)
        // so we infer it from the chunk_id prefix and inline it into the
        // upsert statement. Multi-project batches are handled
        // per-project upstream by the indexer's natural batching boundary.
        let project_code = updates
            .first()
            .and_then(|(chunk_id, _, _)| chunk_id.split('-').next())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!(
                "update_chunk_embeddings cannot infer project_code \
                 from empty/malformed chunk_id"
            ))?
            .to_string();

        // REQ-AXO-238: when AXON_BULK_WRITER_ENABLED, route the entire
        // batch through `crate::postgres::bulk_writer` which performs a
        // single COPY BINARY into a temp staging table followed by
        // `INSERT … SELECT … ON CONFLICT DO UPDATE`. Default OFF
        // preserves the legacy per-row INSERT path bit-for-bit.
        if crate::postgres::bulk_writer::bulk_writer_enabled() {
            let rows: Vec<crate::graph_ingestion::async_writer::ChunkEmbeddingPersistRow> =
                updates
                    .iter()
                    .map(|(chunk_id, source_hash, vector)| {
                        crate::graph_ingestion::async_writer::ChunkEmbeddingPersistRow {
                            chunk_id: chunk_id.clone(),
                            source_hash: source_hash.clone(),
                            embedding: vector.clone(),
                        }
                    })
                    .collect();
            crate::postgres::bulk_writer::flush_chunk_embeddings(
                &project_code,
                model_id,
                &rows,
                now_ms,
            )
            .map_err(|e| anyhow!("bulk_writer flush failed: {}", e))?;
            return Ok(());
        }

        let mut queries = Vec::with_capacity(updates.len());
        for (chunk_id, source_hash, vector) in updates {
            let sql = crate::postgres::vector::upsert_chunk_embedding_sql(
                chunk_id,
                model_id,
                &project_code,
                source_hash,
                vector,
                now_ms,
            )
            .map_err(|e| anyhow!("pgvector upsert SQL build failed: {}", e))?;
            queries.push(sql);
        }
        self.execute_batch(&queries)
    }

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        // MIL-AXO-017 slice 6B Phase C: AGE dual-write retired ; SQL canonical.
        self.execute(&format!(
            "INSERT INTO CONTAINS (source_id, target_id, project_code) VALUES ('{}', '{}', '{}') ON CONFLICT DO NOTHING;",
            from, to, from
        ))?;
        Ok(())
    }

    // MIL-AXO-017 slice 6B Phase C: dual_write_vertices_age / dual_write_relation_edges_age
    // helpers removed ; AGE retired entirely.
}

// MIL-AXO-017 slice 6B Phase C: age_dual_write_enabled() shim removed.

#[cfg(test)]
mod tests {
    use super::sql_helpers::{insert_unique_relation_queries, replace_relation_queries};
    use super::{
        dedup_file_batch_rows, parse_i64_field, FileUpsertSource,
        FileVectorizationLeaseSnapshot, FileVectorizationWork, VectorBatchRun,
        VectorLaneStateRecord, VectorPersistOutboxPayload, VectorPersistOutboxUpdate,
        VectorWorkerFault, CHUNK_EMBEDDING_MODEL_ID,
    };
    use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};
    use crate::parser::{ExtractionResult, Relation, Symbol};
    use crate::queue::ProcessingMode;
    use crate::worker::DbWriteTask;

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
    // REQ-AXO-901653 slice-5a: `count_persisted_file_pending` +
    // `count_graph_wip_files` deleted ; queried public.File status +
    // graph_ready + file_stage columns. Pipeline-v2 has no
    // status='pending'/'indexing' phase ; chunks land directly in
    // public.Chunk via A3.

    /// REQ-AXO-289 S3c — UPSERT a row into the v2 watcher filter table
    /// `IndexedFile(path, content_hash, last_seen_ms)`. Idempotent via
    /// `ON CONFLICT (path) DO UPDATE`. Standalone helper for path-only
    /// callers (e.g. cache reconstitution tests); the streaming A3 stage
    /// goes through [`upsert_graph_v2`] so the IndexedFile UPSERT lands
    /// inside the same transaction as the Symbol + AGE relation inserts.
    pub fn upsert_indexed_file(
        &self,
        path: &str,
        content_hash: &str,
        last_seen_ms: i64,
    ) -> Result<()> {
        let safe_path = Self::escape_sql(path);
        let safe_hash = Self::escape_sql(content_hash);
        self.execute(&format!(
            "INSERT INTO IndexedFile (path, content_hash, last_seen_ms) \
             VALUES ('{path}', '{hash}', {ts}) \
             ON CONFLICT (path) DO UPDATE SET \
                 content_hash = EXCLUDED.content_hash, \
                 last_seen_ms = EXCLUDED.last_seen_ms;",
            path = safe_path,
            hash = safe_hash,
            ts = last_seen_ms,
        ))
    }

    /// REQ-AXO-289 S3d+S4a (session 19 topology) — A3 atomic
    /// graph + chunks + FTS persistence. All artefacts a parsed file
    /// produces land in ONE PG transaction:
    ///
    ///   * `public.Symbol` (UPSERT, idempotent)
    ///   * AGE Symbol + File vertex enrichment (under PG)
    ///   * `CONTAINS` / `CALLS` / `CALLS_NIF` edges (SQL + AGE dual-write)
    ///   * `public.Chunk` — full `content` text stored so the
    ///     REQ-AXO-292 `content_tsv` GENERATED column populates the
    ///     GIN FTS index automatically. Lexical retrieval works
    ///     CPU-only, no GPU dependency.
    ///   * `public.IndexedFile(path, content_hash, last_seen_ms)`
    ///
    /// **Session 19 pivot** (operator critique 2026-05-12 post-S3d):
    /// putting chunking in B1 made FTS dependent on the GPU lane having
    /// run at least once. Moving it back to A keeps the CPU-only stack
    /// (graphe + FTS) authoritative and resilient; B becomes a thin
    /// "fetch chunk content from DB → GPU embed → UPSERT embedding"
    /// lane. SOTA hybrid retrieval pattern: lexical + structural on
    /// CPU, vector as optional enrichment.
    ///
    /// Idempotent — every INSERT uses `ON CONFLICT DO UPDATE` (Symbol,
    /// Chunk, IndexedFile) or `ON CONFLICT DO NOTHING` (relations).
    ///
    /// Returns the chunk_ids persisted. The A3 stage worker `try_send`s
    /// these to the B1 inbox so the GPU lane picks them up immediately
    /// in steady-state; B1 cold-start poll DB catches any drops.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_graph_v2(
        &self,
        path: &str,
        project_code: &str,
        content: &str,
        content_hash: &str,
        last_seen_ms: i64,
        symbols: &[crate::parser::Symbol],
        relations: &[crate::parser::Relation],
    ) -> Result<Vec<String>> {
        use crate::graph_ingestion::async_writer::{
            ChunkRow, RelationRow, SymbolRow, WriteAccumulator,
        };
        use std::collections::HashSet;

        // REQ-AXO-271 slice 2k : PG canonical only. The legacy SQL
        // relation render block (render_contains_pg / render_calls_pg /
        // render_calls_nif_pg) was gated on `!skip_legacy_relations`
        // which is always false under PG → that whole block was dead
        // code. `public.Edge` (REQ-AXO-295 / REQ-AXO-297) is the sole
        // structural edge storage.
        let mut symbol_rows: Vec<SymbolRow> = Vec::new();
        let mut chunk_rows: Vec<ChunkRow> = Vec::new();
        let mut contains_rows: Vec<RelationRow> = Vec::new();
        let mut calls_rows: Vec<RelationRow> = Vec::new();
        let mut calls_nif_rows: Vec<RelationRow> = Vec::new();
        let mut seen_symbols: HashSet<(String, String)> = HashSet::new();
        let mut seen_calls: HashSet<RelationRow> = HashSet::new();
        let mut seen_calls_nif: HashSet<RelationRow> = HashSet::new();
        let mut chunk_ids_emitted: Vec<String> = Vec::new();

        for sym in symbols {
            let symbol_id = Self::symbol_id(project_code, path, &sym.name);
            if !seen_symbols.insert((symbol_id.clone(), project_code.to_string())) {
                continue;
            }
            symbol_rows.push(SymbolRow {
                symbol_id: symbol_id.clone(),
                name: sym.name.clone(),
                kind: sym.kind.clone(),
                tested: sym.tested,
                is_public: sym.is_public,
                is_nif: sym.is_nif,
                is_unsafe: sym.is_unsafe,
                project_code: project_code.to_string(),
                embedding: sym.embedding.clone(),
            });
            contains_rows.push(RelationRow {
                source_id: path.to_string(),
                target_id: symbol_id.clone(),
                project_code: project_code.to_string(),
            });
            for derived_chunk in Self::build_chunk_content(sym, content) {
                let chunk_id = Self::chunk_part_id(
                    &symbol_id,
                    derived_chunk.part_index,
                    derived_chunk.part_count,
                );
                let chunk_hash = Self::stable_content_hash(&derived_chunk.content);
                chunk_rows.push(ChunkRow {
                    chunk_id: chunk_id.clone(),
                    source_type: "symbol".to_string(),
                    source_id: symbol_id.clone(),
                    project_code: project_code.to_string(),
                    file_path: path.to_string(),
                    kind: sym.kind.clone(),
                    content: derived_chunk.content.clone(),
                    content_hash: chunk_hash,
                    start_line: derived_chunk.start_line as i64,
                    end_line: derived_chunk.end_line as i64,
                    part_index: derived_chunk.part_index as i64,
                    part_count: derived_chunk.part_count as i64,
                    chunk_path: derived_chunk.chunk_path.clone(),
                    token_count: Some(derived_chunk.estimated_tokens as i64),
                });
                chunk_ids_emitted.push(chunk_id);
            }
        }
        contains_rows.sort_unstable();
        contains_rows.dedup();
        for relation in relations {
            let Some(table) = Self::relation_table(&relation.rel_type) else {
                continue;
            };
            let source_id = Self::symbol_id(project_code, path, &relation.from);
            let target_id = Self::symbol_id(project_code, path, &relation.to);
            let row = RelationRow {
                source_id,
                target_id,
                project_code: project_code.to_string(),
            };
            match table {
                "CALLS" => {
                    if seen_calls.insert(row.clone()) {
                        calls_rows.push(row);
                    }
                }
                "CALLS_NIF" => {
                    if seen_calls_nif.insert(row.clone()) {
                        calls_nif_rows.push(row);
                    }
                }
                _ => {}
            }
        }

        let mut acc = WriteAccumulator::new();
        acc.symbols = symbol_rows;
        acc.chunks = chunk_rows;
        acc.contains = contains_rows;
        acc.calls = calls_rows;
        acc.calls_nif = calls_nif_rows;

        // PG-canonical: `_pg` renderers emit pgvector literals for the
        // Symbol embedding column when set, and `NULL` when unset (the
        // streaming-v2 path leaves embeddings to B3, so symbols arrive
        // here with `embedding: None`). Chunk INSERT triggers the
        // REQ-AXO-292 `content_tsv` GENERATED column automatically.
        let mut queries: Vec<String> = Vec::new();
        queries.extend(acc.render_symbols_pg());
        queries.extend(acc.render_chunks_pg());
        // `public.Edge` (REQ-AXO-295 / REQ-AXO-297 UPSERTs) is the sole
        // structural edge storage post-MIL-AXO-017.
        queries.extend(acc.render_unified_edge_pg(last_seen_ms));

        let safe_path = Self::escape_sql(path);
        let safe_hash = Self::escape_sql(content_hash);
        queries.push(format!(
            "INSERT INTO IndexedFile (path, content_hash, last_seen_ms) \
             VALUES ('{path}', '{hash}', {ts}) \
             ON CONFLICT (path) DO UPDATE SET \
                 content_hash = EXCLUDED.content_hash, \
                 last_seen_ms = EXCLUDED.last_seen_ms;",
            path = safe_path,
            hash = safe_hash,
            ts = last_seen_ms,
        ));

        self.execute_batch(&queries)?;
        Ok(chunk_ids_emitted)
    }

    /// REQ-AXO-295 — Batched variant of [`Self::upsert_graph_v2`].
    ///
    /// Aggregates Symbol/Chunk/relation rows across `files` into a single
    /// [`WriteAccumulator`], renders them in one shot, appends N
    /// IndexedFile UPSERT statements, and dispatches the whole payload
    /// through `execute_batch` — one `BEGIN/COMMIT` for the entire batch
    /// instead of one per file. Empirically removes the lock-contention
    /// cliff measured 2026-05-12 (A3=2 → 57 ch/s, A3=6 → 22 ch/s in
    /// NoOp).
    ///
    /// Idempotent: every INSERT inherits the existing
    /// `ON CONFLICT DO UPDATE` / `DO NOTHING` semantics.
    ///
    /// Returns `Vec<Vec<String>>` aligned with the input slice — entry
    /// `i` is the chunk_ids persisted for `files[i]`. The order matches
    /// the order of insertion into the accumulator (stable across runs
    /// given identical inputs).
    pub fn upsert_graph_v2_batch(
        &self,
        files: &[crate::pipeline_v2::types::ParsedFile],
        project_code: &str,
    ) -> Result<Vec<Vec<String>>> {
        use crate::graph_ingestion::async_writer::{
            ChunkRow, RelationRow, SymbolRow, WriteAccumulator,
        };
        use std::collections::HashSet;

        if files.is_empty() {
            return Ok(Vec::new());
        }

        // REQ-AXO-271 slice 2k : PG canonical only (see upsert_graph_v2).
        // legacy relation render block + public.file fallback gate
        // collapsed below ; `public.Edge` (REQ-AXO-295 / REQ-AXO-297) is
        // the sole structural edge storage.

        // Per-file chunk_ids preserved for the return value.
        let mut chunk_ids_per_file: Vec<Vec<String>> = Vec::with_capacity(files.len());

        // Cross-file deduplication: a Symbol id is uniquely keyed by
        // (project_code, path, name); a Chunk id by symbol+part_index.
        // Within ONE batch the same file may appear once, but across
        // files we still dedupe on (symbol_id, project_code) tuples so
        // that two parser runs that resolved the same symbol id (very
        // rare cross-file) do not emit duplicate INSERT rows in the
        // same statement string.
        let mut symbol_rows: Vec<SymbolRow> = Vec::new();
        let mut chunk_rows: Vec<ChunkRow> = Vec::new();
        let mut contains_rows: Vec<RelationRow> = Vec::new();
        let mut calls_rows: Vec<RelationRow> = Vec::new();
        let mut calls_nif_rows: Vec<RelationRow> = Vec::new();
        let mut seen_symbols: HashSet<(String, String)> = HashSet::new();
        let mut seen_calls: HashSet<RelationRow> = HashSet::new();
        let mut seen_calls_nif: HashSet<RelationRow> = HashSet::new();
        // REQ-AXO-295 Phase 2 — IndexedFile rows accumulated for one
        // multi-row INSERT VALUES (was one INSERT per file).
        let mut indexed_file_rows: Vec<(String, String, i64)> = Vec::with_capacity(files.len());
        // REQ-AXO-901653 slice-5a: `file_rows` accumulator (REQ-AXO-345
        // public.file UPSERT) deleted ; legacy state-machine table.

        for parsed in files {
            let path_str = parsed.path.to_string_lossy().into_owned();
            let mut chunk_ids_emitted: Vec<String> = Vec::new();
            for sym in &parsed.symbols {
                let symbol_id = Self::symbol_id(project_code, &path_str, &sym.name);
                if !seen_symbols.insert((symbol_id.clone(), project_code.to_string())) {
                    continue;
                }
                symbol_rows.push(SymbolRow {
                    symbol_id: symbol_id.clone(),
                    name: sym.name.clone(),
                    kind: sym.kind.clone(),
                    tested: sym.tested,
                    is_public: sym.is_public,
                    is_nif: sym.is_nif,
                    is_unsafe: sym.is_unsafe,
                    project_code: project_code.to_string(),
                    embedding: sym.embedding.clone(),
                });
                contains_rows.push(RelationRow {
                    source_id: path_str.clone(),
                    target_id: symbol_id.clone(),
                    project_code: project_code.to_string(),
                });
                for derived_chunk in Self::build_chunk_content(sym, &parsed.content) {
                    let chunk_id = Self::chunk_part_id(
                        &symbol_id,
                        derived_chunk.part_index,
                        derived_chunk.part_count,
                    );
                    let chunk_hash = Self::stable_content_hash(&derived_chunk.content);
                    chunk_rows.push(ChunkRow {
                        chunk_id: chunk_id.clone(),
                        source_type: "symbol".to_string(),
                        source_id: symbol_id.clone(),
                        project_code: project_code.to_string(),
                        file_path: path_str.clone(),
                        kind: sym.kind.clone(),
                        content: derived_chunk.content.clone(),
                        content_hash: chunk_hash,
                        start_line: derived_chunk.start_line as i64,
                        end_line: derived_chunk.end_line as i64,
                        part_index: derived_chunk.part_index as i64,
                        part_count: derived_chunk.part_count as i64,
                        chunk_path: derived_chunk.chunk_path.clone(),
                        token_count: Some(derived_chunk.estimated_tokens as i64),
                    });
                    chunk_ids_emitted.push(chunk_id);
                }
            }
            for relation in &parsed.relations {
                let Some(table) = Self::relation_table(&relation.rel_type) else {
                    continue;
                };
                let source_id = Self::symbol_id(project_code, &path_str, &relation.from);
                let target_id = Self::symbol_id(project_code, &path_str, &relation.to);
                let row = RelationRow {
                    source_id,
                    target_id,
                    project_code: project_code.to_string(),
                };
                match table {
                    "CALLS" => {
                        if seen_calls.insert(row.clone()) {
                            calls_rows.push(row);
                        }
                    }
                    "CALLS_NIF" => {
                        if seen_calls_nif.insert(row.clone()) {
                            calls_nif_rows.push(row);
                        }
                    }
                    _ => {}
                }
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            indexed_file_rows.push((path_str.clone(), parsed.content_hash.clone(), now_ms));
            // REQ-AXO-901653 slice-5a: legacy `public.file` row push deleted.
            chunk_ids_per_file.push(chunk_ids_emitted);
        }

        contains_rows.sort_unstable();
        contains_rows.dedup();

        let mut acc = WriteAccumulator::new();
        acc.symbols = symbol_rows;
        acc.chunks = chunk_rows;
        acc.contains = contains_rows;
        acc.calls = calls_rows;
        acc.calls_nif = calls_nif_rows;

        let mut queries: Vec<String> = Vec::new();
        queries.extend(acc.render_symbols_pg());
        queries.extend(acc.render_chunks_pg());
        // `public.Edge` is the sole structural edge storage post-MIL-AXO-017.
        let now_ms = chrono::Utc::now().timestamp_millis();
        queries.extend(acc.render_unified_edge_pg(now_ms));

        // REQ-AXO-295 Phase 2 — IndexedFile multi-row INSERT instead
        // of one statement per file. Same shape as the multi-row
        // Symbol / Chunk inserts: ON CONFLICT (path) DO UPDATE.
        if !indexed_file_rows.is_empty() {
            let mut values_buf =
                String::with_capacity(indexed_file_rows.len() * 80);
            for (i, (path, hash, ts)) in indexed_file_rows.iter().enumerate() {
                if i > 0 {
                    values_buf.push(',');
                }
                values_buf.push_str(&format!(
                    "('{}', '{}', {})",
                    Self::escape_sql(path),
                    Self::escape_sql(hash),
                    ts
                ));
            }
            queries.push(format!(
                "INSERT INTO IndexedFile (path, content_hash, last_seen_ms) \
                 VALUES {values_buf} \
                 ON CONFLICT (path) DO UPDATE SET \
                     content_hash = EXCLUDED.content_hash, \
                     last_seen_ms = EXCLUDED.last_seen_ms;"
            ));
        }

        // REQ-AXO-901653 slice-5a: legacy `public.file` UPSERT block
        // (REQ-AXO-345 hydrate_from_store) deleted ; state-machine
        // table retired.

        self.execute_batch(&queries)?;
        Ok(chunk_ids_per_file)
    }

    /// REQ-AXO-289 S4b — Pipeline-v2 stage B3 UPSERT a ChunkEmbedding
    /// row produced by the GPU embedder (B2).
    ///
    /// `embedding` is the model output vector. PG stores it as
    /// `vector(N)` (pgvector). `source_hash` lets readers tell stale
    /// embeddings apart from current ones when a chunk's content
    /// changes between embedding runs.
    ///
    /// Idempotent: `ON CONFLICT (chunk_id, model_id) DO UPDATE`
    /// overwrites the previous embedding with the latest in place,
    /// matching CPT-AXO-054's idempotence contract for B3.
    pub fn upsert_chunk_embedding_v2(
        &self,
        chunk_id: &str,
        project_code: &str,
        source_hash: &str,
        embedding: &[f32],
        embedded_at_ms: i64,
    ) -> Result<()> {
        let model_id = crate::embedding_contract::CHUNK_MODEL_ID;
        let safe_chunk_id = Self::escape_sql(chunk_id);
        let safe_project = Self::escape_sql(project_code);
        let safe_source_hash = Self::escape_sql(source_hash);
        let safe_model_id = Self::escape_sql(model_id);

        // REQ-AXO-271 slice 2l : PG canonical only. pgvector literal +
        // the PG-shaped INSERT with `project_code` column (REQ-AXO-216).
        let embedding_literal = crate::postgres::vector::vector_literal(embedding).map_err(|e| {
            anyhow::anyhow!("upsert_chunk_embedding_v2: vector_literal failed: {e}")
        })?;
        let sql = format!(
            "INSERT INTO ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
             VALUES ('{cid}', '{mid}', '{pc}', '{sh}', {emb}, {ts}) \
             ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
                 source_hash = EXCLUDED.source_hash, \
                 embedding = EXCLUDED.embedding, \
                 project_code = EXCLUDED.project_code, \
                 embedded_at_ms = EXCLUDED.embedded_at_ms;",
            cid = safe_chunk_id,
            mid = safe_model_id,
            pc = safe_project,
            sh = safe_source_hash,
            emb = embedding_literal,
            ts = embedded_at_ms,
        );
        self.execute(&sql)
    }

    /// REQ-AXO-295 — Batched variant of [`Self::upsert_chunk_embedding_v2`].
    ///
    /// Each item is `(chunk_id, source_hash, embedding, embedded_at_ms)`.
    /// All rows are written through a single multi-statement
    /// `execute_batch` call, amortizing the BEGIN/COMMIT + HNSW
    /// contention cost the per-row variant pays once per embedding.
    pub fn upsert_chunk_embedding_v2_batch(
        &self,
        project_code: &str,
        items: &[(String, String, Vec<f32>, i64)],
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let model_id = crate::embedding_contract::CHUNK_MODEL_ID;
        let safe_project = Self::escape_sql(project_code);
        let safe_model_id = Self::escape_sql(model_id);

        // REQ-AXO-271 slice 2l : PG canonical only.
        let mut queries: Vec<String> = Vec::with_capacity(items.len());
        for (chunk_id, source_hash, embedding, embedded_at_ms) in items {
            let safe_chunk_id = Self::escape_sql(chunk_id);
            let safe_source_hash = Self::escape_sql(source_hash);
            let embedding_literal = crate::postgres::vector::vector_literal(embedding).map_err(|e| {
                anyhow::anyhow!("upsert_chunk_embedding_v2_batch: vector_literal failed: {e}")
            })?;
            let sql = format!(
                "INSERT INTO ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
                 VALUES ('{cid}', '{mid}', '{pc}', '{sh}', {emb}, {ts}) \
                 ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
                     source_hash = EXCLUDED.source_hash, \
                     embedding = EXCLUDED.embedding, \
                     project_code = EXCLUDED.project_code, \
                     embedded_at_ms = EXCLUDED.embedded_at_ms;",
                cid = safe_chunk_id,
                mid = safe_model_id,
                pc = safe_project,
                sh = safe_source_hash,
                emb = embedding_literal,
                ts = embedded_at_ms,
            );
            queries.push(sql);
        }
        let _ = safe_project;
        self.execute_batch(&queries)
    }

    /// REQ-AXO-289 S4c — Pipeline-v2 B1 cold-start poll: return up to
    /// `limit` chunk_ids that exist in `public.Chunk` but have no
    /// matching `public.ChunkEmbedding` row for the canonical model.
    ///
    /// Run by B1 at indexer boot, and once after any
    /// brain-only-to-indexer reactivation, to rattrape chunks that
    /// either never traversed the A3 → B1 `try_send` (drops on full
    /// buffer) or pre-date the v2 cut-over.
    pub fn select_chunks_needing_embedding(&self, limit: usize) -> Result<Vec<String>> {
        let model_id = crate::embedding_contract::CHUNK_MODEL_ID;
        let safe_model_id = Self::escape_sql(model_id);
        // Bucket-batching: order by token_count (BGE-Large estimate stored
        // by A3 at chunking time) so each B1→B2 batch is token-homogeneous.
        // Falls back to length(content)/3 (proxy for chunks pre-dating the
        // token_count column, which back-fill to NULL). BGE-Large transformer
        // cost = batch_size × max_seq_len² (TensorRT pads every item up to
        // the longest in the batch, then snaps to seq_buckets
        // [128, 256, 384, 512]). Token-sorted inputs keep each batch inside
        // a single bucket → near-zero wasted padding compute.
        let raw = self.query_json(&format!(
            "SELECT c.id FROM Chunk c \
             LEFT JOIN ChunkEmbedding ce \
               ON ce.chunk_id = c.id AND ce.model_id = '{safe_model_id}' \
             WHERE ce.chunk_id IS NULL \
             ORDER BY COALESCE(c.token_count, length(c.content) / 3) \
             LIMIT {limit}"
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect())
    }

    /// REQ-AXO-289 S4a (session 19) — Pipeline-v2 stage B1 fetch
    /// chunk content from PG for the GPU embedder lane.
    ///
    /// B1 receives a `chunk_id: String` from A3's `try_send` fan-out
    /// (or from the cold-start poll DB pathway) and needs to load the
    /// chunk's text content to feed B2 (GPU). A3 already persisted the
    /// row inside `public.Chunk`, so B1 just SELECTs it back.
    ///
    /// Returns `Ok(None)` if the chunk_id no longer exists (race with
    /// a re-parse that re-derived chunk_ids — caller drops silently and
    /// moves on). Returns `Ok(Some((content, content_hash)))` for the
    /// common case.
    /// REQ-AXO-314 batched fetch — same contract as
    /// [`Self::fetch_chunk_for_embedding`] but for a slice of chunk_ids
    /// in a single SQL roundtrip. Missing ids (race with re-parse) are
    /// silently absent from the result, mirroring the per-row
    /// `Ok(None)` semantics.
    ///
    /// DEC-AXO-086 follow-up — rows are returned `ORDER BY token_count`
    /// so consecutive items handed off to B2 fall into the same TensorRT
    /// seq_bucket → padding ≈ 0 per batch. NULL token_count (back-fill)
    /// is approximated via `length(content)/3`.
    ///
    /// Reads through the writer ctx for the same read-after-write
    /// reason as the per-row path (cross-pipeline try_send hand-off).
    pub fn fetch_chunks_for_embedding_batch(
        &self,
        chunk_ids: &[String],
    ) -> Result<Vec<(String, String, String)>> {
        if chunk_ids.is_empty() {
            return Ok(Vec::new());
        }
        let in_list = chunk_ids
            .iter()
            .map(|id| format!("'{}'", Self::escape_sql(id)))
            .collect::<Vec<_>>()
            .join(",");
        let raw = self.query_json_writer(&format!(
            "SELECT id, content, content_hash FROM Chunk WHERE id IN ({in_list}) \
             ORDER BY COALESCE(token_count, length(content) / 3)"
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id = row.first().and_then(|v| v.as_str()).map(|s| s.to_string());
            let content = row.get(1).and_then(|v| v.as_str()).map(|s| s.to_string());
            let hash = row.get(2).and_then(|v| v.as_str()).map(|s| s.to_string());
            if let (Some(id), Some(content), Some(hash)) = (id, content, hash) {
                out.push((id, content, hash));
            }
        }
        Ok(out)
    }

    pub fn fetch_chunk_for_embedding(
        &self,
        chunk_id: &str,
    ) -> Result<Option<(String, String)>> {
        let safe_id = Self::escape_sql(chunk_id);
        // Read from the writer ctx so B1 sees A3's freshly-committed
        // rows — the reader ctx may serve a slightly stale snapshot
        // under split-brain modes, which the cross-pipeline try_send
        // makes very likely (B1 picks up chunk_id microseconds after
        // A3 commits, before the reader catches the writer's epoch).
        let raw = self.query_json_writer(&format!(
            "SELECT content, content_hash FROM Chunk WHERE id = '{safe_id}'"
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let content = row
            .first()
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let content_hash = row
            .get(1)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        Ok(Some((content, content_hash)))
    }

    // REQ-AXO-901653 slice-5a: `upsert_file_queries`,
    // `bulk_upsert_file_queries`, `is_file_tombstoned` deleted ; all
    // built/read SQL targeting public.File +
    // GraphProjectionQueue/FileVectorizationQueue. The pipeline-v2
    // canonical writer (`upsert_graph_v2_batch`) does not stage rows
    // into public.File any more.
}
