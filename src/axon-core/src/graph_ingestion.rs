// Copyright (c) Didier Stadelmann. All rights reserved.

use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{anyhow, Result};

use crate::code_chunker::build_symbol_chunks;
use crate::graph::GraphStore;


pub mod async_writer;
mod file_ingress;
mod sql_helpers;
mod types;
mod vector_runtime;

// REQ-AXO-901653 slice-5c — sql_helpers re-exports trimmed ; deleted
// helpers (hourly_bucket_start_ms, next_vector_persist_outbox_claim_token,
// parse_file_ingress_row, parse_i64_field, parse_u64_field) had no usage
// inside graph_ingestion.rs root after the test mod purge. vector_runtime
// imports parse_i64_field + parse_u64_field directly from super::sql_helpers.
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
    // REQ-AXO-901653 slice-5c — 11 stubs DELETED (zero callers post worker.rs +
    // spawn_autonomous_ingestor + enqueue_claimed_files purge) :
    // fetch_pending_batch, fetch_pending_candidates, mark_pending_files_deferred,
    // mark_file_oversized_for_current_budget, claim_pending_paths,
    // requeue_claimed_file_with_reason, requeue_claimed_paths_with_reason,
    // mark_claimed_file_writer_pending_commit, insert_file_data_batch,
    // upsert_file_queries, bulk_upsert_file_queries.

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
            "reads" => Some("READS"),
            "declares" => Some("DECLARES"),
            "exposes" => Some("EXPOSES"),
            "implements" => Some("IMPLEMENTS"),
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
    /// Load all `(path, content_hash, last_seen_ms)` from `IndexedFile`
    /// for hydrating the dedup cache at boot.
    pub fn load_all_indexed_files(&self) -> Result<Vec<(String, String, i64)>> {
        let raw = self.query_json_writer(
            "SELECT path, content_hash, last_seen_ms FROM IndexedFile"
        )?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw)
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let path = row.first()?.as_str()?.to_string();
                let hash = row.get(1)?.as_str()?.to_string();
                let ts = row.get(2)?.as_i64().unwrap_or(0);
                Some((path, hash, ts))
            })
            .collect())
    }

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
            ChunkRow, RelationRow, SymbolRow,
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

        let mut tagged_chunks: Vec<crate::code_chunker::TaggedChunk> = Vec::new();
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
                tagged_chunks.push(crate::code_chunker::TaggedChunk {
                    symbol_id: symbol_id.clone(),
                    symbol_name: sym.name.clone(),
                    chunk: derived_chunk,
                });
            }
        }

        let profile = crate::code_chunker::active_chunk_profile();
        let fused = crate::code_chunker::fuse_small_chunks(
            tagged_chunks,
            profile.target_chunk_tokens,
        );

        for tagged in fused {
            let chunk_id = Self::chunk_part_id(
                &tagged.symbol_id,
                tagged.chunk.part_index,
                tagged.chunk.part_count,
            );
            let chunk_hash = Self::stable_content_hash(&tagged.chunk.content);
            chunk_rows.push(ChunkRow {
                chunk_id: chunk_id.clone(),
                source_type: "symbol".to_string(),
                source_id: tagged.symbol_id,
                project_code: project_code.to_string(),
                file_path: path.to_string(),
                kind: tagged.symbol_name,
                content: tagged.chunk.content.clone(),
                content_hash: chunk_hash,
                start_line: tagged.chunk.start_line as i64,
                end_line: tagged.chunk.end_line as i64,
                part_index: tagged.chunk.part_index as i64,
                part_count: tagged.chunk.part_count as i64,
                chunk_path: tagged.chunk.chunk_path,
                token_count: Some(tagged.chunk.estimated_tokens as i64),
            });
            chunk_ids_emitted.push(chunk_id);
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

        // PG-canonical: COPY BINARY path via bulk_writer.
        let batch = crate::postgres::bulk_writer::PgBulkBatch {
            symbols: symbol_rows,
            chunks: chunk_rows,
            contains: contains_rows,
            calls: calls_rows,
            calls_nif: calls_nif_rows,
            indexed_files: vec![(path.to_string(), content_hash.to_string(), last_seen_ms)],
        };
        crate::postgres::bulk_writer::flush_batch(&batch)?;
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
    /// REQ-AXO-901746 — each inner Vec carries (chunk_id, content, content_hash)
    /// so A3 can forward chunks inline to B1 without a PG round-trip.
    pub fn upsert_graph_v2_batch(
        &self,
        files: &[crate::pipeline_v2::types::ParsedFile],
        project_code: &str,
    ) -> Result<Vec<Vec<(String, String, String)>>> {
        use crate::graph_ingestion::async_writer::{
            ChunkRow, RelationRow, SymbolRow,
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
        let mut chunk_ids_per_file: Vec<Vec<(String, String, String)>> = Vec::with_capacity(files.len());

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
            let mut chunk_ids_emitted: Vec<(String, String, String)> = Vec::new();

            // Phase 1: collect symbols + per-symbol chunks as tagged items.
            let mut tagged_chunks: Vec<crate::code_chunker::TaggedChunk> = Vec::new();
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
                    tagged_chunks.push(crate::code_chunker::TaggedChunk {
                        symbol_id: symbol_id.clone(),
                        symbol_name: sym.name.clone(),
                        chunk: derived_chunk,
                    });
                }
            }

            // Phase 2: fuse small adjacent chunks into context groups.
            let profile = crate::code_chunker::active_chunk_profile();
            let fused = crate::code_chunker::fuse_small_chunks(
                tagged_chunks,
                profile.target_chunk_tokens,
            );

            // Phase 3: generate chunk rows from the fused result.
            for tagged in fused {
                let chunk_id = Self::chunk_part_id(
                    &tagged.symbol_id,
                    tagged.chunk.part_index,
                    tagged.chunk.part_count,
                );
                let chunk_hash = Self::stable_content_hash(&tagged.chunk.content);
                let chunk_content = tagged.chunk.content.clone();
                chunk_rows.push(ChunkRow {
                    chunk_id: chunk_id.clone(),
                    source_type: "symbol".to_string(),
                    source_id: tagged.symbol_id,
                    project_code: project_code.to_string(),
                    file_path: path_str.clone(),
                    kind: tagged.symbol_name,
                    content: tagged.chunk.content,
                    content_hash: chunk_hash.clone(),
                    start_line: tagged.chunk.start_line as i64,
                    end_line: tagged.chunk.end_line as i64,
                    part_index: tagged.chunk.part_index as i64,
                    part_count: tagged.chunk.part_count as i64,
                    chunk_path: tagged.chunk.chunk_path,
                    token_count: Some(tagged.chunk.estimated_tokens as i64),
                });
                chunk_ids_emitted.push((chunk_id, chunk_content, chunk_hash));
            }

            // Phase 4: file-level chunk for files with no symbols (config,
            // README, imports-only) or to capture top-of-file context that
            // falls outside any symbol range. Only emitted when no symbol
            // chunks were produced for this file.
            if chunk_ids_emitted.is_empty() && !parsed.content.is_empty() {
                let file_chunk_id = format!("{}::{}::file_context::chunk", project_code, path_str);
                let truncated = match parsed.content.char_indices().nth(2000) {
                    Some((idx, _)) => &parsed.content[..idx],
                    None => &parsed.content,
                };
                let file_content = format!(
                    "file: {}\nkind: file_context\n\n{}",
                    path_str, truncated
                );
                let token_count = crate::code_chunker::measured_symbol_token_count(&file_content)
                    .unwrap_or(file_content.len() / 3);
                let chunk_hash = Self::stable_content_hash(&file_content);
                chunk_rows.push(ChunkRow {
                    chunk_id: file_chunk_id.clone(),
                    source_type: "file".to_string(),
                    source_id: path_str.clone(),
                    project_code: project_code.to_string(),
                    file_path: path_str.clone(),
                    kind: "file_context".to_string(),
                    content: file_content.clone(),
                    content_hash: chunk_hash.clone(),
                    start_line: 1,
                    end_line: truncated.lines().count() as i64,
                    part_index: 1,
                    part_count: 1,
                    chunk_path: "1/1".to_string(),
                    token_count: Some(token_count as i64),
                });
                chunk_ids_emitted.push((file_chunk_id, file_content, chunk_hash));
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

        // REQ-AXO-901747 — COPY BINARY path via bulk_writer.
        let batch = crate::postgres::bulk_writer::PgBulkBatch {
            symbols: symbol_rows,
            chunks: chunk_rows,
            contains: contains_rows,
            calls: calls_rows,
            calls_nif: calls_nif_rows,
            indexed_files: indexed_file_rows,
        };
        crate::postgres::bulk_writer::flush_batch(&batch)?;
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
