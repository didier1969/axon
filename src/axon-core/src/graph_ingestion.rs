// Copyright (c) Didier Stadelmann. All rights reserved.

use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{anyhow, Result};

use crate::code_chunker::build_symbol_chunks;
use crate::graph::GraphStore;

pub mod rows;
mod sql_helpers;
mod types;
mod vector_runtime;

// REQ-AXO-901653 slice-5c — sql_helpers re-exports trimmed ; deleted
// helpers (hourly_bucket_start_ms, next_vector_persist_outbox_claim_token,
// parse_file_ingress_row, parse_i64_field, parse_u64_field) had no usage
// inside graph_ingestion.rs root after the test mod purge. vector_runtime
// imports parse_i64_field + parse_u64_field directly from super::sql_helpers.
pub use types::{
    EmbedderLifecycleHeartbeatRecord, EmbedderObservedState, FileLifecycleEvent,
    FileVectorizationLeaseSnapshot, FileVectorizationWork, IgnoreReconcileStats, VectorBatchRun,
    VectorLaneStateRecord, VectorPersistOutboxPayload, VectorPersistOutboxUpdate,
    VectorPersistOutboxWork, VectorWorkerFault,
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
    pub fn count_persisted_file_pending(&self) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn count_graph_wip_files(&self) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn count_orphaned_file_vectorization_files(&self) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn count_stale_inflight_file_vectorization_files(
        &self,
        _now_ms: i64,
        _stale_threshold_ms: i64,
    ) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn oldest_graph_pending_age_ms(&self, _now_ms: i64) -> Result<u64> {
        Ok(0)
    } // slice-5b-stub
    pub fn oldest_semantic_pending_age_ms(&self, _now_ms: i64) -> Result<u64> {
        Ok(0)
    } // slice-5b-stub
    pub fn fetch_claimable_file_vectorization_queue_count(&self) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub

    // ---- state-machine no-ops (legacy callers, pipeline_v2 bypasses) ----
    pub fn backfill_file_vectorization_queue(&self) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn backfill_file_vectorization_queue_with_limit(&self, _limit: usize) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
    pub fn recover_stale_inflight_file_vectorization_work(
        &self,
        _now_ms: i64,
        _stale_threshold_ms: i64,
    ) -> Result<usize> {
        Ok(0)
    } // slice-5b-stub
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
            // REQ-AXO-901493 — parser-emitted edge kinds that were unmapped
            // (returned None → dropped before ever reaching the write path).
            "imports" => Some("IMPORTS"),
            "uses" => Some("USES"),
            "extends" => Some("EXTENDS"),
            "tests" => Some("TESTS"),
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
                    result.entry(fp.to_string()).or_default().push((
                        id.to_string(),
                        content.to_string(),
                        hash.to_string(),
                    ));
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
                        log::warn!("skipping update_symbol_embeddings for {}: {}", id, e);
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

    // MIL-AXO-017 slice 6B Phase C: dual_write_vertices_age / dual_write_relation_edges_age
    // helpers removed ; AGE retired entirely.
}

// MIL-AXO-017 slice 6B Phase C: age_dual_write_enabled() shim removed.

impl GraphStore {
    // REQ-AXO-901653 slice-5a: `count_persisted_file_pending` +
    // `count_graph_wip_files` deleted ; queried public.File status +
    // graph_ready + file_stage columns. Pipeline-v2 has no
    // status='pending'/'indexing' phase ; chunks land directly in
    // ist.Chunk via A3.

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
        // REQ-AXO-901897 (DBQ slice 1) — A3 stamps 'parsed' (A-graph done), not
        // the legacy 'indexed'. 'parsed' is an A-DONE state → load_all_indexed_files
        // hydrates the dedup cache from it, and the claimable index/feeder no
        // longer see this row. lease_until_ms is cleared (claim released).
        self.execute(&format!(
            "INSERT INTO IndexedFile (path, content_hash, last_seen_ms, status, retry_count, last_attempt_ms, lease_until_ms) \
             VALUES ('{path}', '{hash}', {ts}, 'parsed', 0, NULL, 0) \
             ON CONFLICT (path) DO UPDATE SET \
                 content_hash    = EXCLUDED.content_hash, \
                 last_seen_ms    = EXCLUDED.last_seen_ms, \
                 status          = 'parsed', \
                 retry_count     = 0, \
                 last_attempt_ms = NULL, \
                 lease_until_ms  = 0;",
            path = safe_path,
            hash = safe_hash,
            ts = last_seen_ms,
        ))
    }

    /// REQ-AXO-289 S3d+S4a (session 19 topology) — A3 atomic
    /// graph + chunks + FTS persistence. All artefacts a parsed file
    /// produces land in ONE PG transaction:
    ///
    ///   * `ist.Symbol` (UPSERT, idempotent)
    ///   * AGE Symbol + File vertex enrichment (under PG)
    ///   * `CONTAINS` / `CALLS` / `CALLS_NIF` edges (SQL + AGE dual-write)
    ///   * `ist.Chunk` — full `content` text stored so the out-of-band
    ///     pgmq tsv_worker (REQ-AXO-901624) can back-fill `content_tsv`
    ///     into the GIN FTS index. Lexical retrieval works
    ///     CPU-only, no GPU dependency.
    ///   * `ist.IndexedFile(path, content_hash, last_seen_ms)`
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
    /// Load all `(path, content_hash, last_seen_ms, mtime_ms, size_bytes)` from
    /// `IndexedFile` for hydrating the dedup cache at boot. mtime/size feed the
    /// level-1 (no-read) I/O pre-filter; content_hash the level-2 parse skip.
    pub fn load_all_indexed_files(&self) -> Result<Vec<(String, String, i64, i64, u64)>> {
        // PIL-AXO-007 (REQ-AXO-901916) — the dedup cache hydrates from rows that
        // carry a real content_hash, i.e. A3 actually indexed them. In the
        // status-free model an IndexedFile row exists ONLY after a successful A3
        // UPSERT (no pre-created 'discovered' placeholder), so `content_hash <> ''`
        // is the exact "A-DONE" predicate that replaces the old status filter.
        // A not-yet-indexed file is simply absent → should_read/should_index
        // return true → it gets read + parsed.
        let raw = self.query_json_writer(
            "SELECT path, content_hash, last_seen_ms, mtime_ms, size_bytes FROM IndexedFile \
             WHERE content_hash <> ''",
        )?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let path = row.first()?.as_str()?.to_string();
                let hash = row.get(1)?.as_str()?.to_string();
                let ts = row.get(2)?.as_i64().unwrap_or(0);
                let mtime = row.get(3)?.as_i64().unwrap_or(0);
                let size = row.get(4)?.as_i64().unwrap_or(0).max(0) as u64;
                Some((path, hash, ts, mtime, size))
            })
            .collect())
    }

    /// REQ-AXO-901809 / REQ-AXO-901891 — pipeline A discovered-backlog count:
    /// files enrolled but not yet parsed (status='discovered'). Post-reconciler
    /// this is the "pending parse" headline the bootstrap+drain are working
    /// through (the claim/retry machinery it used to mirror is gone).
    ///
    /// Source = `COUNT(*) FROM IndexedFile WHERE status='discovered'`
    /// — NOT `pg_stat_user_tables.n_live_tup`, which lags the
    /// autovacuum cadence and double-counts uncommitted rows.
    /// Used by the MCP `embedding_status` observability surface
    /// (REQ-AXO-901816) and reserved for the future watcher
    /// max-stock back-pressure (REQ-AXO-901815).
    pub fn pipeline_a_discovered_stock(&self, max_retry: i32) -> Result<u64> {
        let sql = format!(
            "SELECT count(*)::bigint FROM IndexedFile \
             WHERE status='discovered' AND retry_count<{max_retry}"
        );
        let raw = self.query_json(&sql)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0) as u64)
    }

    /// 9f: detect and remove files that disappeared from the filesystem.
    /// Call after a scanner walk. Rows with discovered_ms < scan_start_ms were
    /// not re-stamped in this walk — CANDIDATES for staleness, confirmed
    /// against the filesystem (`Path::exists`) before any deletion so a partial
    /// walk can never erode live data (REQ-AXO-901884). Returns the paths the
    /// FS confirmed gone and that were purged.
    ///
    /// REQ-AXO-901831 — the deletion MUST be scoped to the subtree that was
    /// actually walked (`root_prefix`). Reconciliation + federation invoke the
    /// scanner once PER candidate project; an unscoped DELETE wiped every other
    /// project's IndexedFile rows (their `discovered_ms` predates the current
    /// walk and unchanged files never get re-stamped), draining ~9479 enrolled
    /// files down to a single project's worth on the first 60 s reconciliation
    /// pass. Scoping by canonical path prefix is correct for the full-root
    /// indexer scan, per-project reconciliation, and UNK/empty project_code
    /// candidates alike (path is independent of code).
    pub fn delete_stale_indexed_files(
        &self,
        scan_start_ms: i64,
        root_prefix: &str,
    ) -> Result<Vec<String>> {
        let safe_prefix = root_prefix.replace('\'', "''");
        // REQ-AXO-901884 — NON-DESTRUCTIVE stale reconciliation. "Not re-stamped
        // in this walk" (last_seen_ms < scan_start_ms) is NOT proof a file is
        // gone: a partial/interrupted walk — resource pressure, FS-watcher
        // EMFILE (inotify-instance exhaustion), or a mid-scan error — leaves
        // REAL files un-restamped. The old code DELETE…RETURNING'd them in one
        // shot, eroding the index (observed 36K→3.5K files) and firing the
        // cascade on live data. The filesystem is the source of truth: gather
        // candidates, then purge ONLY the paths the FS confirms are actually
        // gone (`Path::exists() == false`). A missed-but-present file costs
        // freshness (re-stamped next walk), never data.
        // PIL-AXO-007 — keyed on last_seen_ms (A3 re-stamps it on every UPSERT),
        // since status='discovered'/discovered_ms were retired with the feeder.
        let raw = self.query_json_writer(&format!(
            "SELECT path FROM IndexedFile \
             WHERE last_seen_ms < {scan_start_ms} \
               AND path LIKE '{safe_prefix}/%'"
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let candidates: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.first()?.as_str().map(String::from))
            .collect();
        let mut deleted: Vec<String> = Vec::new();
        for path in candidates {
            // Source of truth = disk. Skip anything still present (a partial
            // walk merely missed it); purge only genuinely-removed files.
            if std::path::Path::new(&path).exists() {
                continue;
            }
            let _ = self.delete_file_cascade(&path);
            deleted.push(path);
        }
        Ok(deleted)
    }

    /// REQ-AXO-901893 — cascade-delete a single file's entire IST footprint:
    /// embeddings, chunks, contained symbols, edges (both directions of the
    /// CONTAINS fan-out), and the IndexedFile row. This is the atomic DELETE
    /// half of the Watchman feed — `exists=false` events (a genuine deletion,
    /// or the old side of a rename). Shared with [`delete_stale_indexed_files`]
    /// so the cascade SQL lives in exactly ONE place (no drift between the
    /// reconciliation purge and the live delete path).
    ///
    /// Unlike `delete_stale_indexed_files`, the caller is responsible for
    /// confirming the file is genuinely gone — Watchman's `exists=false` IS
    /// that confirmation (it observed the unlink), so there is no disk re-check
    /// here.
    pub fn delete_file_cascade(&self, path: &str) -> Result<()> {
        let safe = path.replace('\'', "''");
        self.execute(&format!(
            "DELETE FROM ChunkEmbedding WHERE chunk_id IN \
                (SELECT id FROM Chunk WHERE file_path = '{safe}'); \
             DELETE FROM Chunk WHERE file_path = '{safe}'; \
             DELETE FROM Symbol WHERE id IN \
                (SELECT target_id FROM Edge WHERE source_id = '{safe}' AND relation_type = 'CONTAINS'); \
             DELETE FROM Edge WHERE source_id = '{safe}' OR target_id IN \
                (SELECT target_id FROM Edge e2 WHERE e2.source_id = '{safe}' AND e2.relation_type = 'CONTAINS'); \
             DELETE FROM IndexedFile WHERE path = '{safe}';"
        ))
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
        use crate::graph_ingestion::rows::{ChunkRow, RelationRow, SymbolRow};
        use std::collections::HashSet;

        // REQ-AXO-901860 — skip the "UNK" sentinel (unregistered file) so a
        // single-file enrol can't pollute an UNK bucket or poison the writer
        // tx on the NOT NULL project_code FK. Mirrors upsert_graph_v2_batch.
        if project_code == "UNK" {
            return Ok(Vec::new());
        }

        // REQ-AXO-271 slice 2k : PG canonical only. The legacy SQL
        // relation render block (render_contains_pg / render_calls_pg /
        // render_calls_nif_pg) was gated on `!skip_legacy_relations`
        // which is always false under PG → that whole block was dead
        // code. `ist.Edge` (REQ-AXO-295 / REQ-AXO-297) is the sole
        // structural edge storage.
        let mut symbol_rows: Vec<SymbolRow> = Vec::new();
        let mut chunk_rows: Vec<ChunkRow> = Vec::new();
        let mut contains_rows: Vec<RelationRow> = Vec::new();
        let mut calls_rows: Vec<RelationRow> = Vec::new();
        let mut calls_nif_rows: Vec<RelationRow> = Vec::new();
        // REQ-AXO-901493 — generic bucket for every other mapped edge kind.
        let mut other_edge_rows: Vec<(&'static str, RelationRow)> = Vec::new();
        let mut seen_symbols: HashSet<(String, String)> = HashSet::new();
        let mut seen_calls: HashSet<RelationRow> = HashSet::new();
        let mut seen_calls_nif: HashSet<RelationRow> = HashSet::new();
        let mut seen_other: HashSet<(&'static str, RelationRow)> = HashSet::new();
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
        let fused =
            crate::code_chunker::fuse_small_chunks(tagged_chunks, profile.target_chunk_tokens);

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
                // REQ-AXO-901493 — persist every other mapped edge kind
                // (IMPLEMENTS/IMPORTS/USES/EXTENDS/READS/DECLARES/EXPOSES/TESTS)
                // instead of dropping it.
                other => {
                    if seen_other.insert((other, row.clone())) {
                        other_edge_rows.push((other, row));
                    }
                }
            }
        }

        // PG-canonical: COPY BINARY path via bulk_writer.
        let batch = crate::postgres::bulk_writer::PgBulkBatch {
            symbols: symbol_rows,
            chunks: chunk_rows,
            contains: contains_rows,
            calls: calls_rows,
            calls_nif: calls_nif_rows,
            other_edges: other_edge_rows
                .into_iter()
                .map(|(t, r)| (t.to_string(), r))
                .collect(),
            indexed_files: vec![(
                path.to_string(),
                content_hash.to_string(),
                last_seen_ms,
                last_seen_ms,
                content.len() as i64,
            )],
            project_code: project_code.to_string(),
        };
        crate::postgres::bulk_writer::flush_batch(&batch)?;
        Ok(chunk_ids_emitted)
    }

    /// REQ-AXO-295 — Batched variant of [`Self::upsert_graph_v2`].
    ///
    /// Aggregates Symbol/Chunk/relation [`rows`] across `files` into one
    /// [`crate::postgres::bulk_writer::PgBulkBatch`] flushed in a single
    /// `BEGIN/COMMIT` (one transaction for the whole batch instead of one
    /// per file). Empirically removes the lock-contention cliff measured
    /// 2026-05-12 (A3=2 → 57 ch/s, A3=6 → 22 ch/s in NoOp).
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
        use crate::graph_ingestion::rows::{ChunkRow, RelationRow, SymbolRow};
        use std::collections::HashSet;

        if files.is_empty() {
            return Ok(Vec::new());
        }

        // REQ-AXO-901860 — unregistered files resolve to the "UNK" sentinel
        // (pipeline_v2_runtime resolver fallback). project_code is now a NOT
        // NULL FK to ist.Project, so writing UNK rows would either resurrect
        // the "UNK" bucket the refonte deleted or fail the FK and poison the
        // pooled writer connection (25P02 cascade). Skip cleanly — the
        // resolver already logged the resolution failure (fail-loud, graceful).
        if project_code == "UNK" {
            return Ok(files.iter().map(|_| Vec::new()).collect());
        }

        // REQ-AXO-271 slice 2k : PG canonical only (see upsert_graph_v2).
        // legacy relation render block + public.file fallback gate
        // collapsed below ; `ist.Edge` (REQ-AXO-295 / REQ-AXO-297) is
        // the sole structural edge storage.

        // Per-file chunk_ids preserved for the return value.
        let mut chunk_ids_per_file: Vec<Vec<(String, String, String)>> =
            Vec::with_capacity(files.len());

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
        // REQ-AXO-901493 — generic bucket for every other mapped edge kind.
        let mut other_edge_rows: Vec<(&'static str, RelationRow)> = Vec::new();
        let mut seen_symbols: HashSet<(String, String)> = HashSet::new();
        let mut seen_calls: HashSet<RelationRow> = HashSet::new();
        let mut seen_calls_nif: HashSet<RelationRow> = HashSet::new();
        let mut seen_other: HashSet<(&'static str, RelationRow)> = HashSet::new();
        // REQ-AXO-295 Phase 2 — IndexedFile rows accumulated for one
        // multi-row INSERT VALUES (was one INSERT per file).
        let mut indexed_file_rows: Vec<(String, String, i64, i64, i64)> =
            Vec::with_capacity(files.len());
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
            let fused =
                crate::code_chunker::fuse_small_chunks(tagged_chunks, profile.target_chunk_tokens);

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
                let file_content =
                    format!("file: {}\nkind: file_context\n\n{}", path_str, truncated);
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
                    // REQ-AXO-901493 — persist every other mapped edge kind
                    // (IMPLEMENTS/IMPORTS/USES/EXTENDS/...) instead of dropping.
                    other => {
                        if seen_other.insert((other, row.clone())) {
                            other_edge_rows.push((other, row));
                        }
                    }
                }
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            indexed_file_rows.push((
                path_str.clone(),
                parsed.content_hash.clone(),
                now_ms,
                parsed.mtime_ms,
                parsed.size_bytes as i64,
            ));
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
            other_edges: other_edge_rows
                .into_iter()
                .map(|(t, r)| (t.to_string(), r))
                .collect(),
            indexed_files: indexed_file_rows,
            project_code: project_code.to_string(),
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
        let embedding_literal =
            crate::postgres::vector::vector_literal(embedding).map_err(|e| {
                anyhow::anyhow!("upsert_chunk_embedding_v2: vector_literal failed: {e}")
            })?;
        let sql = format!(
            "INSERT INTO ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
             VALUES ('{cid}', '{mid}', '{pc}', '{sh}', {emb}, {ts}) \
             ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
                 source_hash = EXCLUDED.source_hash, \
                 embedding = EXCLUDED.embedding, \
                 project_code = EXCLUDED.project_code, \
                 embedded_at_ms = EXCLUDED.embedded_at_ms; \
             UPDATE Chunk SET embed_status = 'embedded' WHERE id = '{cid}';",
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

        // REQ-AXO-901884 — dedup the batch by chunk_id (last-wins) BEFORE writing.
        // A single set-based `INSERT ... ON CONFLICT (chunk_id, model_id) DO UPDATE`
        // (text path) — and the COPY-staging merge — cannot affect the same
        // conflict target twice in one command (SQLSTATE 21000 "ON CONFLICT DO
        // UPDATE command cannot affect row a second time"). B3 batches can carry
        // the same chunk_id twice (demand-pull / cold-start-poll overlap, retried
        // flushes); since identical content yields an identical embedding,
        // last-wins is idempotent with the DO UPDATE. The legacy per-row INSERT
        // loop was immune (separate statements); the set-based form is not.
        let deduped: Vec<&(String, String, Vec<f32>, i64)> = {
            let mut seen: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::with_capacity(items.len());
            let mut out: Vec<&(String, String, Vec<f32>, i64)> = Vec::with_capacity(items.len());
            for it in items.iter() {
                match seen.get(it.0.as_str()) {
                    Some(&pos) => out[pos] = it,
                    None => {
                        seen.insert(it.0.as_str(), out.len());
                        out.push(it);
                    }
                }
            }
            out
        };

        // REQ-AXO-901881 W3 #33/#34 — adaptive dispatch on the REAL B3 write
        // path (this fn IS the live pipeline-v2 embedding writer, driven by
        // stage_b3::spawn_b3_batched_worker). For large batches (the deferred
        // one-shot full-IST load) route the embedding upsert through COPY
        // BINARY on THIS store's native pool — per-instance, so the bulk write
        // lands in the same DB the store reads from (also closes the embedding
        // half of the linchpin REQ-AXO-901877); for the small flushes of steady
        // cruise keep the per-row text-literal INSERT that VAL-AXO-067 proved
        // faster at that scale. All items in a B3 batch share one embedded_at_ms
        // (stage_b3.rs). The embed_status UPDATE runs either way (idempotent on
        // chunk_id; retried flushes converge).
        if crate::postgres::bulk_writer::should_use_bulk_writer(deduped.len()) {
            let embedded_at_ms = deduped.first().map(|it| it.3).unwrap_or(0);
            let rows: Vec<crate::graph_ingestion::rows::ChunkEmbeddingPersistRow> = deduped
                .iter()
                .map(
                    |it| crate::graph_ingestion::rows::ChunkEmbeddingPersistRow {
                        chunk_id: it.0.clone(),
                        source_hash: it.1.clone(),
                        embedding: it.2.clone(),
                    },
                )
                .collect();
            self.pool
                .native
                .flush_chunk_embeddings_copy(project_code, model_id, &rows, embedded_at_ms)
                .map_err(|e| anyhow!("upsert_chunk_embedding_v2_batch COPY flush failed: {e}"))?;
            let chunk_ids_in: Vec<String> = deduped
                .iter()
                .map(|it| format!("'{}'", Self::escape_sql(&it.0)))
                .collect();
            if !chunk_ids_in.is_empty() {
                self.execute(&format!(
                    "UPDATE Chunk SET embed_status = 'embedded' WHERE id IN ({}) AND embed_status = 'pending';",
                    chunk_ids_in.join(", ")
                ))?;
            }
            return Ok(());
        }

        // REQ-AXO-271 slice 2l : PG canonical only.
        // REQ-AXO-901884 — single set-based INSERT ... SELECT ... JOIN ist.Chunk
        // so a chunk_id deleted by re-index churn between the demand-pull SELECT
        // and this INSERT is dropped by the JOIN BEFORE the FK is evaluated (no
        // 23503 -> no tx abort -> no pooled-conn poison cascade). The casts
        // (::vector / ::bigint) pin the VALUES column types (a bare VALUES list
        // types every column as text).
        let mut values_rows: Vec<String> = Vec::with_capacity(deduped.len());
        for it in &deduped {
            let safe_chunk_id = Self::escape_sql(&it.0);
            let safe_source_hash = Self::escape_sql(&it.1);
            let embedding_literal =
                crate::postgres::vector::vector_literal(&it.2).map_err(|e| {
                    anyhow::anyhow!("upsert_chunk_embedding_v2_batch: vector_literal failed: {e}")
                })?;
            values_rows.push(format!(
                "('{cid}', '{mid}', '{pc}', '{sh}', {emb}, {ts})",
                cid = safe_chunk_id,
                mid = safe_model_id,
                pc = safe_project,
                sh = safe_source_hash,
                emb = embedding_literal,
                ts = it.3,
            ));
        }
        let mut queries: Vec<String> = Vec::with_capacity(2);
        queries.push(format!(
            "INSERT INTO ChunkEmbedding (chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
             SELECT v.chunk_id, v.model_id, v.project_code, v.source_hash, v.embedding::vector, v.embedded_at_ms::bigint \
             FROM (VALUES {rows}) \
                  AS v(chunk_id, model_id, project_code, source_hash, embedding, embedded_at_ms) \
             JOIN Chunk c ON c.id = v.chunk_id \
             ON CONFLICT (chunk_id, model_id) DO UPDATE SET \
                 source_hash = EXCLUDED.source_hash, \
                 embedding = EXCLUDED.embedding, \
                 project_code = EXCLUDED.project_code, \
                 embedded_at_ms = EXCLUDED.embedded_at_ms;",
            rows = values_rows.join(", "),
        ));
        // W3: batch-update embed_status after all embeddings are persisted.
        // `AND embed_status = 'pending'` guards against stamping a chunk that a
        // concurrent re-index deleted or reset to pending (stale-vector race).
        let chunk_ids_in: Vec<String> = deduped
            .iter()
            .map(|it| format!("'{}'", Self::escape_sql(&it.0)))
            .collect();
        if !chunk_ids_in.is_empty() {
            queries.push(format!(
                "UPDATE Chunk SET embed_status = 'embedded' WHERE id IN ({}) AND embed_status = 'pending';",
                chunk_ids_in.join(", ")
            ));
        }
        let _ = safe_project;
        self.execute_batch(&queries)
    }

    /// REQ-AXO-289 S4c — Pipeline-v2 B1 cold-start poll: return up to
    /// `limit` chunk_ids that exist in `ist.Chunk` but have no
    /// matching `ist.ChunkEmbedding` row for the canonical model.
    ///
    /// Run by B1 at indexer boot, and once after any
    /// brain-only-to-indexer reactivation, to rattrape chunks that
    /// either never traversed the A3 → B1 `try_send` (drops on full
    /// buffer) or pre-date the v2 cut-over.
    pub fn select_chunks_needing_embedding(&self, limit: usize) -> Result<Vec<String>> {
        // W3: partial index scan on embed_status='pending' replaces the
        // LEFT JOIN ChunkEmbedding anti-pattern. Scales O(pending) not O(total).
        // Token-count ordering preserved for GPU batch homogeneity.
        let raw = self.query_json(&format!(
            "SELECT id FROM Chunk \
             WHERE embed_status = 'pending' \
             ORDER BY COALESCE(token_count, length(content) / 3) \
             LIMIT {limit}"
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect())
    }

    /// Slice 5 SOTA — Pipeline-v2 demand-pull B: return chunks needing
    /// embedding **with their content** in a single round-trip.
    ///
    /// Collapses the previous 2-round-trip pattern (B1 SELECT id, then
    /// SELECT content WHERE id IN(...)) into one. Used by
    /// `demand_pull::pull_and_feed_b` to feed `ChunkForEmbedding`
    /// directly to B2 (GPU embedder). The B1 stage worker disappears as
    /// a result.
    ///
    /// Same partial index scan as `select_chunks_needing_embedding`
    /// (embed_status='pending') with the canonical token-count ordering
    /// so consecutive items fall into the same TensorRT seq_bucket.
    pub fn select_chunks_with_content_needing_embedding(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>> {
        let raw = self.query_json_writer(&format!(
            "SELECT id, content, content_hash FROM Chunk \
             WHERE embed_status = 'pending' \
             ORDER BY COALESCE(token_count, length(content) / 3) \
             LIMIT {limit}"
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

    /// REQ-AXO-289 S4a (session 19) — Pipeline-v2 stage B1 fetch
    /// chunk content from PG for the GPU embedder lane.
    ///
    /// B1 receives a `chunk_id: String` from A3's `try_send` fan-out
    /// (or from the cold-start poll DB pathway) and needs to load the
    /// chunk's text content to feed B2 (GPU). A3 already persisted the
    /// row inside `ist.Chunk`, so B1 just SELECTs it back.
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

    pub fn fetch_chunk_for_embedding(&self, chunk_id: &str) -> Result<Option<(String, String)>> {
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
