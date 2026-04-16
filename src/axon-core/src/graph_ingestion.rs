// Copyright (c) Didier Stadelmann. All rights reserved.

use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
use libloading::Symbol as LibSymbol;

use crate::embedding_contract::{CHUNK_MODEL_ID as CHUNK_EMBEDDING_MODEL_ID, DIMENSION};
use crate::file_ingress_guard::FileIngressRow;
use crate::graph::{ExecFunc, GraphStore, PendingFile};
use crate::ingress_buffer::{IngressDrainBatch, IngressPromotionStats, IngressSource};
use crate::queue::ProcessingMode;
use crate::runtime_mode::graph_embeddings_enabled;
use crate::runtime_mode::AxonRuntimeMode;
use crate::service_guard;
use crate::watcher_probe;

const DEFAULT_GRAPH_EMBEDDING_RADIUS: i64 = 2;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_COOLDOWN_MS: i64 = 5_000;
pub const INTERACTIVE_VECTORIZATION_REQUEUE_LIMIT: i64 = 2;
static FILE_VECTORIZATION_CLAIM_SEQ: AtomicU64 = AtomicU64::new(1);
const CHUNK_EMBEDDING_UPSERT_BATCH_ROWS: usize = 500;

#[derive(Debug, Clone, Copy)]
enum FileUpsertSource {
    Scan,
    HotDelta,
}

#[derive(Debug, Clone)]
pub struct GraphProjectionWork {
    pub anchor_type: String,
    pub anchor_id: String,
    pub radius: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileVectorizationWork {
    pub file_path: String,
    pub resumed_after_interactive_pause: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileVectorizationLeaseSnapshot {
    pub file_path: String,
    pub claim_token: String,
    pub lease_epoch: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileLifecycleEvent {
    pub file_path: String,
    pub project_code: String,
    pub stage: String,
    pub status: String,
    pub reason: Option<String>,
    pub at_ms: i64,
    pub worker_id: Option<i64>,
    pub trace_id: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorBatchRun {
    pub run_id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub provider: String,
    pub model_id: String,
    pub chunk_count: u64,
    pub file_count: u64,
    pub input_bytes: u64,
    pub fetch_ms: u64,
    pub embed_ms: u64,
    pub db_write_ms: u64,
    pub mark_done_ms: u64,
    pub success: bool,
    pub error_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorPersistOutboxUpdate {
    pub chunk_id: String,
    pub source_hash: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorPersistOutboxPayload {
    pub updates: Vec<VectorPersistOutboxUpdate>,
    pub completed_works: Vec<FileVectorizationWork>,
    pub completed_lease_snapshots: Vec<FileVectorizationLeaseSnapshot>,
    pub batch_run: VectorBatchRun,
}

#[derive(Debug, Clone)]
pub struct VectorPersistOutboxWork {
    pub outbox_id: String,
    pub payload: VectorPersistOutboxPayload,
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreReconcileStats {
    pub scanned: usize,
    pub newly_ignored: usize,
    pub newly_included: usize,
    pub dry_run: bool,
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

fn graph_projection_queue_upsert(
    anchor_type: &str,
    anchor_id: &str,
    radius: i64,
    now_ms: i64,
) -> String {
    let safe_anchor_type = anchor_type.replace('\'', "''");
    let safe_anchor_id = anchor_id.replace('\'', "''");
    format!(
        "INSERT INTO GraphProjectionQueue (anchor_type, anchor_id, radius, status, attempts, queued_at, last_error_reason, last_attempt_at) \
         VALUES ('{}', '{}', {}, 'queued', 0, {}, NULL, NULL) \
         ON CONFLICT(anchor_type, anchor_id, radius) DO UPDATE \
         SET status = 'queued', \
             attempts = 0, \
             queued_at = {}, \
             last_error_reason = NULL, \
             last_attempt_at = NULL;",
        safe_anchor_type,
        safe_anchor_id,
        radius,
        now_ms,
        now_ms
    )
}

fn file_vectorization_queue_upsert_if_needed(file_path: &str, now_ms: i64) -> String {
    let safe_path = file_path.replace('\'', "''");
    format!(
        "INSERT INTO FileVectorizationQueue (file_path, status, status_reason, attempts, queued_at, last_error_reason, last_attempt_at, claim_token, claimed_at_ms, lease_heartbeat_at_ms, lease_owner, lease_epoch) \
         SELECT path, 'queued', NULL, 0, {}, NULL, NULL, NULL, NULL, NULL, NULL, 0 \
         FROM File \
         WHERE path = '{}' \
           AND graph_ready = TRUE \
           AND vector_ready = FALSE \
           AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
           AND file_stage NOT IN ('deleted', 'skipped', 'oversized') \
         ON CONFLICT(file_path) DO UPDATE \
         SET status = 'queued', \
         status_reason = NULL, \
         attempts = 0, \
         queued_at = {}, \
         last_error_reason = NULL, \
         last_attempt_at = NULL, \
         claim_token = NULL, \
         claimed_at_ms = NULL, \
         lease_heartbeat_at_ms = NULL, \
         lease_owner = NULL, \
         lease_epoch = 0",
        now_ms, safe_path, now_ms
    )
}

fn hourly_bucket_start_ms(at_ms: i64) -> i64 {
    (at_ms / 3_600_000) * 3_600_000
}

fn next_vector_persist_outbox_claim_token(now_ms: i64) -> String {
    let seq = FILE_VECTORIZATION_CLAIM_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("outbox-claim-{}-{}", now_ms, seq)
}

fn sort_and_dedup_sql_tuples(values: &mut Vec<String>) {
    values.sort_unstable();
    values.dedup();
}

fn insert_unique_relation_queries(table: &str, values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|row| {
            format!(
                "INSERT INTO {table} (source_id, target_id, project_code) VALUES {row} \
                 ON CONFLICT DO NOTHING;",
                table = table,
                row = row
            )
        })
        .collect()
}

fn replace_relation_queries(table: &str, values: &[String], chunk_size: usize) -> Vec<String> {
    if values.is_empty() {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for chunk in values.chunks(chunk_size.max(1)) {
        let values_sql = chunk.join(",");
        queries.push(format!(
            "DELETE FROM {table} USING (VALUES {values_sql}) AS incoming(source_id, target_id, project_code) \
             WHERE {table}.source_id = incoming.source_id \
               AND {table}.target_id = incoming.target_id \
               AND {table}.project_code = incoming.project_code;",
            table = table,
            values_sql = values_sql
        ));
        queries.push(format!(
            "INSERT INTO {table} (source_id, target_id, project_code) VALUES {values_sql};",
            table = table,
            values_sql = values_sql
        ));
    }

    queries
}

fn dedup_file_batch_rows(
    rows: &[(String, String, i64, i64, i64, FileUpsertSource)],
) -> Vec<(String, String, i64, i64, i64, FileUpsertSource)> {
    let mut deduped = std::collections::BTreeMap::new();
    for (path, project, size, mtime, priority, source) in rows {
        deduped.insert(
            path.clone(),
            (project.clone(), *size, *mtime, *priority, *source),
        );
    }

    deduped
        .into_iter()
        .map(|(path, (project, size, mtime, priority, source))| {
            (path, project, size, mtime, priority, source)
        })
        .collect()
}

impl GraphStore {
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

    pub fn record_vector_batch_run(&self, run: &VectorBatchRun) -> Result<()> {
        self.execute(&format!(
            "INSERT OR REPLACE INTO VectorBatchRun (run_id, started_at_ms, finished_at_ms, provider, model_id, chunk_count, file_count, input_bytes, fetch_ms, embed_ms, db_write_ms, mark_done_ms, success, error_reason) \
             VALUES ('{}', {}, {}, '{}', '{}', {}, {}, {}, {}, {}, {}, {}, {}, {});",
            Self::escape_sql(&run.run_id),
            run.started_at_ms,
            run.finished_at_ms,
            Self::escape_sql(&run.provider),
            Self::escape_sql(&run.model_id),
            run.chunk_count,
            run.file_count,
            run.input_bytes,
            run.fetch_ms,
            run.embed_ms,
            run.db_write_ms,
            run.mark_done_ms,
            if run.success { "TRUE" } else { "FALSE" },
            run.error_reason
                .as_ref()
                .map(|reason| format!("'{}'", Self::escape_sql(reason)))
                .unwrap_or_else(|| "NULL".to_string())
        ))
    }

    pub fn enqueue_vector_persist_outbox(
        &self,
        payload: &VectorPersistOutboxPayload,
    ) -> Result<String> {
        let outbox_id = format!("outbox-{}", payload.batch_run.run_id);
        let payload_json = serde_json::to_string(payload)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&format!(
            "INSERT OR REPLACE INTO VectorPersistOutbox \
             (outbox_id, run_id, model_id, status, attempts, queued_at_ms, claimed_at_ms, completed_at_ms, last_error_reason, claim_token, lease_heartbeat_at_ms, lease_owner, lease_epoch, chunk_count, file_count, input_bytes, fetch_ms, embed_ms, payload_json) \
             VALUES ('{}', '{}', '{}', 'queued', 0, {}, NULL, NULL, NULL, NULL, NULL, NULL, 0, {}, {}, {}, {}, {}, '{}')",
            Self::escape_sql(&outbox_id),
            Self::escape_sql(&payload.batch_run.run_id),
            Self::escape_sql(&payload.batch_run.model_id),
            now_ms,
            payload.batch_run.chunk_count,
            payload.batch_run.file_count,
            payload.batch_run.input_bytes,
            payload.batch_run.fetch_ms,
            payload.batch_run.embed_ms,
            Self::escape_sql(&payload_json)
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(outbox_id)
    }

    pub fn enqueue_vector_persist_outbox_handoff(
        &self,
        payload: &VectorPersistOutboxPayload,
        lease_snapshots: &[FileVectorizationLeaseSnapshot],
    ) -> Result<String> {
        let outbox_id = format!("outbox-{}", payload.batch_run.run_id);
        let payload_json = serde_json::to_string(payload)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let lease_predicates = if lease_snapshots.is_empty() {
            None
        } else {
            Some(
                lease_snapshots
                    .iter()
                    .map(|item| {
                        format!(
                            "(file_path = '{}' AND claim_token = '{}' AND COALESCE(lease_epoch, 0) = {} AND COALESCE(lease_owner, '') = 'vector')",
                            Self::escape_sql(&item.file_path),
                            Self::escape_sql(&item.claim_token),
                            item.lease_epoch
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" OR "),
            )
        };

        if let Some(predicates) = lease_predicates.as_ref() {
            let matched = usize::try_from(self.query_count(&format!(
                "SELECT count(*) FROM FileVectorizationQueue \
                 WHERE status = 'inflight' \
                   AND claim_token IS NOT NULL \
                   AND ({})",
                predicates
            ))?)
            .unwrap_or(0);
            if matched != lease_snapshots.len() {
                return Err(anyhow!(
                    "outbox handoff refused: expected {} vector-owned rows, matched {}",
                    lease_snapshots.len(),
                    matched
                ));
            }
        }

        let mut queries = vec![format!(
            "INSERT OR REPLACE INTO VectorPersistOutbox \
             (outbox_id, run_id, model_id, status, attempts, queued_at_ms, claimed_at_ms, completed_at_ms, last_error_reason, claim_token, lease_heartbeat_at_ms, lease_owner, lease_epoch, chunk_count, file_count, input_bytes, fetch_ms, embed_ms, payload_json) \
             VALUES ('{}', '{}', '{}', 'queued', 0, {}, NULL, NULL, NULL, NULL, NULL, NULL, 0, {}, {}, {}, {}, {}, '{}')",
            Self::escape_sql(&outbox_id),
            Self::escape_sql(&payload.batch_run.run_id),
            Self::escape_sql(&payload.batch_run.model_id),
            now_ms,
            payload.batch_run.chunk_count,
            payload.batch_run.file_count,
            payload.batch_run.input_bytes,
            payload.batch_run.fetch_ms,
            payload.batch_run.embed_ms,
            Self::escape_sql(&payload_json)
        )];

        if let Some(predicates) = lease_predicates {
            queries.push(format!(
                "UPDATE FileVectorizationQueue \
                 SET lease_owner = 'outbox', \
                     lease_epoch = COALESCE(lease_epoch, 0) + 1, \
                     lease_heartbeat_at_ms = {}, \
                     last_attempt_at = {} \
                 WHERE status = 'inflight' \
                   AND claim_token IS NOT NULL \
                   AND ({})",
                now_ms, now_ms, predicates
            ));
        }

        self.execute_batch(&queries)?;
        service_guard::notify_vector_backlog_activity();
        Ok(outbox_id)
    }

    pub fn fetch_pending_vector_persist_outbox_work(
        &self,
        count: usize,
    ) -> Result<Vec<VectorPersistOutboxWork>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let claim_token = next_vector_persist_outbox_claim_token(now_ms);
        self.execute(&format!(
            "UPDATE VectorPersistOutbox \
             SET status = 'inflight', \
                 attempts = attempts + 1, \
                 claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 lease_owner = 'outbox', \
                 claim_token = '{}', \
                 last_error_reason = NULL \
             WHERE outbox_id IN ( \
                 SELECT outbox_id FROM VectorPersistOutbox \
                 WHERE status = 'queued' \
                 ORDER BY queued_at_ms, outbox_id \
                 LIMIT {} \
             )",
            now_ms,
            now_ms,
            Self::escape_sql(&claim_token),
            count
        ))?;

        let raw = self.query_json(&format!(
            "SELECT outbox_id, payload_json \
             FROM VectorPersistOutbox \
             WHERE claim_token = '{}' \
             ORDER BY queued_at_ms, outbox_id",
            Self::escape_sql(&claim_token)
        ))?;
        if raw == "[]" || raw.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let outbox_id = row.first()?.as_str()?.to_string();
                let payload_json = row.get(1)?.as_str()?.to_string();
                let payload =
                    serde_json::from_str::<VectorPersistOutboxPayload>(&payload_json).ok()?;
                Some(VectorPersistOutboxWork { outbox_id, payload })
            })
            .collect())
    }

    pub fn fetch_vector_persist_outbox_counts(&self) -> Result<(usize, usize)> {
        let queued =
            self.query_count("SELECT count(*) FROM VectorPersistOutbox WHERE status = 'queued'")?;
        let inflight =
            self.query_count("SELECT count(*) FROM VectorPersistOutbox WHERE status = 'inflight'")?;
        Ok((
            usize::try_from(queued).unwrap_or(0),
            usize::try_from(inflight).unwrap_or(0),
        ))
    }

    pub fn refresh_vector_persist_outbox_leases(&self, outbox_ids: &[String]) -> Result<usize> {
        if outbox_ids.is_empty() {
            return Ok(0);
        }
        let predicates = outbox_ids
            .iter()
            .map(|outbox_id| format!("(outbox_id = '{}')", Self::escape_sql(outbox_id)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let refreshed = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM VectorPersistOutbox \
             WHERE status = 'inflight' \
               AND COALESCE(lease_owner, '') = 'outbox' \
               AND ({})",
            predicates
        ))?)
        .unwrap_or(0);
        if refreshed == 0 {
            return Ok(0);
        }
        self.execute(&format!(
            "UPDATE VectorPersistOutbox \
             SET claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {} \
             WHERE status = 'inflight' \
               AND COALESCE(lease_owner, '') = 'outbox' \
               AND ({})",
            now_ms, now_ms, predicates
        ))?;
        Ok(refreshed)
    }

    pub fn mark_vector_persist_outbox_done(&self, outbox_id: &str) -> Result<()> {
        self.execute(&format!(
            "DELETE FROM VectorPersistOutbox WHERE outbox_id = '{}'",
            Self::escape_sql(outbox_id)
        ))
    }

    pub fn mark_vector_persist_outbox_failed(&self, outbox_id: &str, reason: &str) -> Result<()> {
        self.execute(&format!(
            "UPDATE VectorPersistOutbox \
             SET status = 'queued', \
                 last_error_reason = '{}', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL \
             WHERE outbox_id = '{}'",
            Self::escape_sql(reason),
            Self::escape_sql(outbox_id)
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(())
    }

    pub fn recover_stale_vector_persist_outbox_inflight(
        &self,
        stale_before_ms: i64,
    ) -> Result<usize> {
        let recovered = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM VectorPersistOutbox \
             WHERE status = 'inflight' \
               AND COALESCE(lease_heartbeat_at_ms, 0) > 0 \
               AND COALESCE(lease_heartbeat_at_ms, 0) < {}",
            stale_before_ms
        ))?)
        .unwrap_or(0);
        if recovered == 0 {
            return Ok(0);
        }
        self.execute(&format!(
            "UPDATE VectorPersistOutbox \
             SET status = 'queued', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 last_error_reason = 'recovered_stale_outbox_inflight' \
             WHERE status = 'inflight' \
               AND COALESCE(lease_heartbeat_at_ms, 0) > 0 \
               AND COALESCE(lease_heartbeat_at_ms, 0) < {}",
            stale_before_ms
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(recovered)
    }

    pub fn refresh_hourly_vectorization_rollup(
        &self,
        bucket_start_ms: i64,
        model_id: &str,
    ) -> Result<()> {
        let bucket_end_ms = bucket_start_ms.saturating_add(3_600_000);
        let model_id = Self::escape_sql(model_id);
        self.execute(&format!(
            "BEGIN TRANSACTION; \
             DELETE FROM HourlyVectorizationRollup WHERE bucket_start_ms = {} AND model_id = '{}'; \
             INSERT INTO HourlyVectorizationRollup (bucket_start_ms, project_code, model_id, chunks_embedded, files_vector_ready, batches, fetch_ms_total, embed_ms_total, db_write_ms_total, mark_done_ms_total) \
             WITH chunk_rollup AS ( \
                 SELECT c.project_code AS project_code, COUNT(*) AS chunks_embedded \
                 FROM ChunkEmbedding ce \
                 JOIN Chunk c ON c.id = ce.chunk_id \
                 WHERE ce.model_id = '{}' \
                   AND ce.embedded_at_ms >= {} \
                   AND ce.embedded_at_ms < {} \
                 GROUP BY c.project_code \
             ), \
             file_rollup AS ( \
                 SELECT project_code, COUNT(*) AS files_vector_ready \
                 FROM File \
                 WHERE vector_ready = TRUE \
                   AND vector_ready_at_ms IS NOT NULL \
                   AND vector_ready_at_ms >= {} \
                   AND vector_ready_at_ms < {} \
                 GROUP BY project_code \
             ), \
             project_keys AS ( \
                 SELECT project_code FROM chunk_rollup \
                 UNION \
                 SELECT project_code FROM file_rollup \
             ), \
             project_counts AS ( \
                 SELECT COUNT(*) AS project_count FROM project_keys \
             ), \
             batch_rollup AS ( \
                 SELECT COUNT(*) AS batches, \
                        COALESCE(SUM(fetch_ms), 0) AS fetch_ms_total, \
                        COALESCE(SUM(embed_ms), 0) AS embed_ms_total, \
                        COALESCE(SUM(db_write_ms), 0) AS db_write_ms_total, \
                        COALESCE(SUM(mark_done_ms), 0) AS mark_done_ms_total \
                 FROM VectorBatchRun \
                 WHERE model_id = '{}' \
                   AND finished_at_ms >= {} \
                   AND finished_at_ms < {} \
             ) \
             SELECT {}, \
                    pk.project_code, \
                    '{}', \
                    COALESCE(cr.chunks_embedded, 0), \
                    COALESCE(fr.files_vector_ready, 0), \
                    CASE \
                        WHEN pc.project_count = 1 \
                        THEN COALESCE(br.batches, 0) \
                        ELSE 0 \
                    END AS batches, \
                    CASE \
                        WHEN pc.project_count = 1 \
                        THEN COALESCE(br.fetch_ms_total, 0) \
                        ELSE 0 \
                    END AS fetch_ms_total, \
                    CASE \
                        WHEN pc.project_count = 1 \
                        THEN COALESCE(br.embed_ms_total, 0) \
                        ELSE 0 \
                    END AS embed_ms_total, \
                    CASE \
                        WHEN pc.project_count = 1 \
                        THEN COALESCE(br.db_write_ms_total, 0) \
                        ELSE 0 \
                    END AS db_write_ms_total, \
                    CASE \
                        WHEN pc.project_count = 1 \
                        THEN COALESCE(br.mark_done_ms_total, 0) \
                        ELSE 0 \
                    END AS mark_done_ms_total \
             FROM project_keys pk \
             LEFT JOIN chunk_rollup cr ON cr.project_code = pk.project_code \
             LEFT JOIN file_rollup fr ON fr.project_code = pk.project_code \
             CROSS JOIN batch_rollup br \
             CROSS JOIN project_counts pc; \
             COMMIT;",
            bucket_start_ms,
            model_id,
            model_id,
            bucket_start_ms,
            bucket_end_ms,
            bucket_start_ms,
            bucket_end_ms,
            model_id,
            bucket_start_ms,
            bucket_end_ms,
            bucket_start_ms,
            model_id
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
            "SELECT path, COALESCE(project_code, 'global'), worker_id, trace_id \
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
                .unwrap_or("global")
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

    fn build_chunk_content(_path: &str, symbol: &crate::parser::Symbol, content: &str) -> String {
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
            "symbol: {}\nkind: {}\n{}\
\n{}",
            symbol.name, symbol.kind, docstring, snippet
        )
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

    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
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
        for (path, project, size, mtime, priority, source) in dedup_file_batch_rows(&batch) {
            queries.extend(Self::upsert_file_queries(
                &path, &project, size, mtime, priority, source,
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

    pub fn promote_ingress_batch(
        &self,
        batch: &IngressDrainBatch,
    ) -> Result<IngressPromotionStats> {
        let mut queries = Vec::new();
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

        for (path, project_code, size, mtime, priority, source) in dedup_file_batch_rows(&file_rows)
        {
            queries.extend(Self::upsert_file_queries(
                &path,
                &project_code,
                size,
                mtime,
                priority,
                source,
            ));
        }

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
            "SELECT path, COALESCE(project_code, 'global'), status FROM File \
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
                let project = scanner.project_code_for_path(path_obj);
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
        Ok(rows
            .into_iter()
            .filter_map(parse_file_ingress_row)
            .collect())
    }

    pub fn enqueue_graph_projection_refresh(
        &self,
        anchor_type: &str,
        anchor_id: &str,
        radius: i64,
    ) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&graph_projection_queue_upsert(
            anchor_type,
            anchor_id,
            radius,
            now_ms,
        ))
    }

    pub fn enqueue_graph_projection_refresh_batch(&self, work: &[(&str, &str, i64)]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let queries = work
            .iter()
            .map(|(anchor_type, anchor_id, radius)| {
                graph_projection_queue_upsert(anchor_type, anchor_id, *radius, now_ms)
            })
            .collect::<Vec<_>>();

        self.execute_batch(&queries)
    }

    pub fn fetch_pending_graph_projection_work(
        &self,
        count: usize,
    ) -> Result<Vec<GraphProjectionWork>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let query = format!(
            "SELECT anchor_type, anchor_id, radius \
             FROM GraphProjectionQueue \
             WHERE status = 'queued' \
             ORDER BY COALESCE(queued_at, 0), anchor_type, anchor_id \
             LIMIT {}",
            count
        );
        let raw = self.query_json(&query)?;

        if raw == "[]" || raw.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut queue = Vec::new();
        for row in rows {
            let Some(anchor_type) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(anchor_id) = row.get(1).and_then(|value| value.as_str()) else {
                continue;
            };
            let radius = row
                .get(2)
                .and_then(|value| value.as_i64())
                .unwrap_or(DEFAULT_GRAPH_EMBEDDING_RADIUS);

            queue.push(GraphProjectionWork {
                anchor_type: anchor_type.to_string(),
                anchor_id: anchor_id.to_string(),
                radius,
            });
        }

        if queue.is_empty() {
            return Ok(queue);
        }

        let predicates = queue
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE GraphProjectionQueue \
             SET status = 'inflight', \
                 last_attempt_at = {}, \
                 attempts = attempts + 1 \
             WHERE status = 'queued' AND ({})",
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))?;
        Ok(queue)
    }

    pub fn mark_graph_projection_work_done(&self, work: &[GraphProjectionWork]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "DELETE FROM GraphProjectionQueue \
             WHERE status = 'inflight' AND ({})",
            predicates
        ))
    }

    pub fn mark_graph_projection_work_failed(
        &self,
        work: &[GraphProjectionWork],
        reason: &str,
    ) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| {
                format!(
                    "(anchor_type = '{}' AND anchor_id = '{}' AND radius = {})",
                    Self::escape_sql(&item.anchor_type),
                    Self::escape_sql(&item.anchor_id),
                    item.radius
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE GraphProjectionQueue \
             SET status = 'queued', \
                 last_error_reason = '{}', \
                 last_attempt_at = {}, \
                 attempts = attempts + 1 \
             WHERE status = 'inflight' AND ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))
    }

    pub fn clear_stale_inflight_graph_projection_work(&self) -> Result<()> {
        self.execute(
            "UPDATE GraphProjectionQueue \
             SET status = 'queued' \
             WHERE status = 'inflight'",
        )
    }

    pub fn backfill_graph_projection_queue_for_model(&self, model_id: &str) -> Result<usize> {
        let query = format!(
            "SELECT gps.anchor_type, gps.anchor_id, gps.radius \
             FROM GraphProjectionState gps \
             LEFT JOIN GraphEmbedding ge \
               ON ge.anchor_type = gps.anchor_type \
              AND ge.anchor_id = gps.anchor_id \
              AND ge.radius = gps.radius \
              AND ge.model_id = '{}' \
             WHERE ge.anchor_id IS NULL \
                OR ge.source_signature <> gps.source_signature \
                OR ge.projection_version <> gps.projection_version",
            Self::escape_sql(model_id)
        );
        let raw = self.query_json(&query)?;
        if raw == "[]" || raw.is_empty() {
            return Ok(0);
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut queries = Vec::new();
        for row in rows {
            let Some(anchor_type) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(anchor_id) = row.get(1).and_then(|value| value.as_str()) else {
                continue;
            };
            let radius = row
                .get(2)
                .and_then(|value| value.as_i64())
                .unwrap_or(DEFAULT_GRAPH_EMBEDDING_RADIUS);
            queries.push(graph_projection_queue_upsert(
                anchor_type,
                anchor_id,
                radius,
                now_ms,
            ));
        }

        let inserted = queries.len();
        if inserted == 0 {
            return Ok(0);
        }

        self.execute_batch(&queries)?;
        Ok(inserted)
    }

    pub fn fetch_graph_projection_queue_counts(&self) -> Result<(usize, usize)> {
        let queued =
            self.query_count("SELECT count(*) FROM GraphProjectionQueue WHERE status = 'queued'")?;
        let inflight = self
            .query_count("SELECT count(*) FROM GraphProjectionQueue WHERE status = 'inflight'")?;
        let queued = usize::try_from(queued).unwrap_or(0);
        let inflight = usize::try_from(inflight).unwrap_or(0);
        Ok((queued, inflight))
    }

    pub fn enqueue_file_vectorization_refresh(&self, file_path: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&file_vectorization_queue_upsert_if_needed(
            file_path, now_ms,
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(())
    }

    pub fn fetch_pending_file_vectorization_work(
        &self,
        count: usize,
    ) -> Result<Vec<FileVectorizationWork>> {
        if count == 0 {
            return Ok(Vec::new());
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let claim_token = Self::next_file_vectorization_claim_token(now_ms);
        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'inflight', \
                 status_reason = CASE \
                     WHEN status = 'paused_for_interactive_priority' THEN 'resumed_after_interactive_pause' \
                     ELSE NULL \
                 END, \
                 next_eligible_at_ms = NULL, \
                 last_attempt_at = {}, \
                 attempts = attempts + 1, \
                 claim_token = '{}', \
                 claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 lease_owner = 'vector', \
                 lease_epoch = COALESCE(lease_epoch, 0) \
             WHERE status IN ('queued', 'paused_for_interactive_priority') \
               AND file_path IN ( \
                   SELECT file_path \
                   FROM FileVectorizationQueue fq \
                   LEFT JOIN File f ON f.path = fq.file_path \
                   WHERE fq.status IN ('queued', 'paused_for_interactive_priority') \
                     AND COALESCE(f.vector_ready, FALSE) = FALSE \
                     AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                     AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
                     AND (fq.next_eligible_at_ms IS NULL OR fq.next_eligible_at_ms <= {}) \
                   ORDER BY COALESCE(queued_at, 0), fq.file_path \
                   LIMIT {} \
               )",
            now_ms,
            Self::escape_sql(&claim_token),
            now_ms,
            now_ms,
            now_ms,
            count
        ))?;

        let raw = self.query_json(&format!(
            "SELECT file_path, COALESCE(status_reason, '') \
             FROM FileVectorizationQueue \
             WHERE claim_token = '{}' \
             ORDER BY COALESCE(queued_at, 0), file_path",
            Self::escape_sql(&claim_token)
        ))?;

        if raw == "[]" || raw.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut queue = Vec::new();
        for row in rows {
            let Some(file_path) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };

            let resumed_after_interactive_pause = row
                .get(1)
                .and_then(|value| value.as_str())
                .map(|value| value == "resumed_after_interactive_pause")
                .unwrap_or(false);

            queue.push(FileVectorizationWork {
                file_path: file_path.to_string(),
                resumed_after_interactive_pause,
            });
        }

        Ok(queue)
    }

    pub fn mark_file_vectorization_started(&self, work: &[FileVectorizationWork]) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let predicates = work
            .iter()
            .map(|item| format!("(path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let started = usize::try_from(self.query_count(&format!(
            "SELECT count(*) \
             FROM File \
             WHERE vectorization_started_at_ms IS NULL \
               AND ({})",
            predicates
        ))?)
        .unwrap_or(0);

        self.execute(&format!(
            "UPDATE File \
             SET vectorization_started_at_ms = COALESCE(vectorization_started_at_ms, {}), \
                 last_state_change_at_ms = {} \
             WHERE ({})",
            now_ms, now_ms, predicates
        ))?;

        Ok(started)
    }

    pub fn pause_file_vectorization_work_for_interactive_priority(
        &self,
        work: &[FileVectorizationWork],
        cooldown_ms: i64,
        max_interruptions: i64,
    ) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let next_eligible_at_ms = now_ms.saturating_add(cooldown_ms.max(0));
        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        let affected_query = format!(
            "SELECT count(*) \
             FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND COALESCE(interactive_pause_count, 0) < {} \
               AND ({})",
            max_interruptions.max(0),
            predicates
        );
        let affected = usize::try_from(self.query_count(&affected_query)?).unwrap_or(0);

        if affected == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'paused_for_interactive_priority', \
                 status_reason = 'requeued_for_interactive_priority', \
                 last_error_reason = 'requeued_for_interactive_priority', \
                 next_eligible_at_ms = {}, \
                 interactive_pause_count = COALESCE(interactive_pause_count, 0) + 1, \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' \
               AND COALESCE(interactive_pause_count, 0) < {} \
               AND ({})",
            next_eligible_at_ms,
            max_interruptions.max(0),
            predicates
        ))?;

        Ok(affected)
    }

    pub fn mark_file_vectorization_work_done(&self, work: &[FileVectorizationWork]) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "DELETE FROM FileVectorizationQueue \
             WHERE ({})",
            predicates
        ))
    }

    pub fn refresh_inflight_file_vectorization_claims(
        &self,
        work: &[FileVectorizationWork],
    ) -> Result<usize> {
        self.refresh_file_vectorization_leases_for_owner(work, "vector")
    }

    pub fn refresh_file_vectorization_leases_for_owner(
        &self,
        work: &[FileVectorizationWork],
        lease_owner: &str,
    ) -> Result<usize> {
        if work.is_empty() {
            return Ok(0);
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let refreshed = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(lease_owner),
            predicates
        ))?)
        .unwrap_or(0);

        if refreshed == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 last_attempt_at = {} \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            now_ms,
            now_ms,
            now_ms,
            Self::escape_sql(lease_owner),
            predicates
        ))?;

        Ok(refreshed)
    }

    pub fn transfer_file_vectorization_lease_owner(
        &self,
        snapshots: &[FileVectorizationLeaseSnapshot],
        from_owner: &str,
        to_owner: &str,
    ) -> Result<Vec<FileVectorizationLeaseSnapshot>> {
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }

        let predicates = snapshots
            .iter()
            .map(|item| {
                format!(
                    "(file_path = '{}' AND claim_token = '{}' AND COALESCE(lease_epoch, 0) = {})",
                    Self::escape_sql(&item.file_path),
                    Self::escape_sql(&item.claim_token),
                    item.lease_epoch
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let now_ms = chrono::Utc::now().timestamp_millis();
        let transferred = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(from_owner),
            predicates
        ))?)
        .unwrap_or(0);

        if transferred != snapshots.len() {
            return Err(anyhow!(
                "lease owner transfer refused: expected {} rows, matched {}",
                snapshots.len(),
                transferred
            ));
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET lease_owner = '{}', \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1, \
                 lease_heartbeat_at_ms = {}, \
                 last_attempt_at = {} \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({})",
            Self::escape_sql(to_owner),
            now_ms,
            now_ms,
            Self::escape_sql(from_owner),
            predicates
        ))?;

        Ok(snapshots
            .iter()
            .map(|item| FileVectorizationLeaseSnapshot {
                file_path: item.file_path.clone(),
                claim_token: item.claim_token.clone(),
                lease_epoch: item.lease_epoch.saturating_add(1),
            })
            .collect())
    }

    pub fn capture_file_vectorization_lease_snapshots(
        &self,
        work: &[FileVectorizationWork],
        lease_owner: &str,
    ) -> Result<Vec<FileVectorizationLeaseSnapshot>> {
        if work.is_empty() {
            return Ok(Vec::new());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let raw = self.query_json(&format!(
            "SELECT file_path, claim_token, COALESCE(lease_epoch, 0) \
             FROM FileVectorizationQueue \
             WHERE status = 'inflight' \
               AND claim_token IS NOT NULL \
               AND COALESCE(lease_owner, '') = '{}' \
               AND ({}) \
             ORDER BY file_path",
            Self::escape_sql(lease_owner),
            predicates
        ))?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut snapshots = rows
            .into_iter()
            .filter_map(|row| {
                let file_path = row.first()?.as_str()?.to_string();
                let claim_token = row.get(1)?.as_str()?.to_string();
                let lease_epoch = parse_u64_field(row.get(2)?).unwrap_or(0);
                Some(FileVectorizationLeaseSnapshot {
                    file_path,
                    claim_token,
                    lease_epoch,
                })
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|a, b| a.file_path.cmp(&b.file_path));

        let mut expected_paths = work
            .iter()
            .map(|item| item.file_path.clone())
            .collect::<Vec<_>>();
        expected_paths.sort();
        let actual_paths = snapshots
            .iter()
            .map(|item| item.file_path.clone())
            .collect::<Vec<_>>();
        if actual_paths != expected_paths {
            return Err(anyhow!(
                "lease snapshot capture mismatch: expected {:?}, got {:?}",
                expected_paths,
                actual_paths
            ));
        }

        Ok(snapshots)
    }

    pub fn mark_file_vectorization_work_failed(
        &self,
        work: &[FileVectorizationWork],
        reason: &str,
    ) -> Result<()> {
        if work.is_empty() {
            return Ok(());
        }

        let predicates = work
            .iter()
            .map(|item| format!("(file_path = '{}')", Self::escape_sql(&item.file_path)))
            .collect::<Vec<_>>()
            .join(" OR ");

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                status_reason = NULL, \
                last_error_reason = '{}', \
                last_attempt_at = {}, \
                attempts = attempts + 1, \
                claim_token = NULL, \
                claimed_at_ms = NULL, \
                lease_heartbeat_at_ms = NULL, \
                lease_owner = NULL, \
                lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' AND ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            predicates
        ))?;
        self.execute(&format!(
            "UPDATE File \
             SET last_error_reason = '{}', \
                 last_error_at_ms = {}, \
                 last_state_change_at_ms = {} \
             WHERE ({})",
            Self::escape_sql(reason),
            chrono::Utc::now().timestamp_millis(),
            chrono::Utc::now().timestamp_millis(),
            predicates.replace("file_path", "path")
        ))?;
        service_guard::notify_vector_backlog_activity();
        Ok(())
    }

    pub fn clear_stale_inflight_file_vectorization_work(&self) -> Result<()> {
        let recovered = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'")?;
        self.execute(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                 status_reason = 'recovered_after_stale_inflight', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight'",
        )?;
        if recovered > 0 {
            service_guard::notify_vector_backlog_activity();
        }
        Ok(())
    }

    pub fn recover_stale_inflight_file_vectorization_work(
        &self,
        now_ms: i64,
        max_claim_age_ms: i64,
    ) -> Result<usize> {
        let cutoff_ms = now_ms.saturating_sub(max_claim_age_ms.max(0));
        let recovered = usize::try_from(self.query_count(&format!(
            "SELECT count(*) \
             FROM FileVectorizationQueue fq \
             LEFT JOIN File f ON f.path = fq.file_path \
             WHERE fq.status = 'inflight' \
               AND COALESCE(f.vector_ready, FALSE) = FALSE \
               AND fq.claim_token IS NOT NULL \
               AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
               AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) IS NOT NULL \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) <= {}",
            cutoff_ms
        ))?)
        .unwrap_or(0);

        if recovered == 0 {
            return Ok(0);
        }

        self.execute(&format!(
            "UPDATE FileVectorizationQueue \
             SET status = 'queued', \
                 status_reason = 'recovered_after_stale_inflight', \
                 claim_token = NULL, \
                 claimed_at_ms = NULL, \
                 lease_heartbeat_at_ms = NULL, \
                 lease_owner = NULL, \
                 lease_epoch = COALESCE(lease_epoch, 0) + 1 \
             WHERE status = 'inflight' \
               AND file_path IN ( \
                   SELECT fq.file_path \
                   FROM FileVectorizationQueue fq \
                   LEFT JOIN File f ON f.path = fq.file_path \
                   WHERE fq.status = 'inflight' \
                     AND COALESCE(f.vector_ready, FALSE) = FALSE \
                     AND fq.claim_token IS NOT NULL \
                     AND COALESCE(f.status, '') NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
                     AND COALESCE(f.file_stage, '') NOT IN ('deleted', 'skipped', 'oversized') \
                     AND COALESCE(fq.lease_heartbeat_at_ms, fq.claimed_at_ms) IS NOT NULL \
                     AND COALESCE(fq.lease_heartbeat_at_ms, fq.claimed_at_ms) <= {} \
               ) \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) IS NOT NULL \
               AND COALESCE(lease_heartbeat_at_ms, claimed_at_ms) <= {}",
            cutoff_ms, cutoff_ms
        ))?;
        service_guard::notify_vector_backlog_activity();

        Ok(recovered)
    }

    pub fn backfill_file_vectorization_queue(&self) -> Result<usize> {
        let query = format!(
            "SELECT path \
             FROM File \
             WHERE status IN ('indexed', 'indexed_degraded') \
               AND file_stage = 'graph_indexed' \
               AND graph_ready = TRUE \
               AND vector_ready = FALSE \
               AND status NOT IN ('deleted', 'skipped', 'oversized_for_current_budget') \
               AND file_stage NOT IN ('deleted', 'skipped', 'oversized') \
               AND NOT EXISTS ( \
                   SELECT 1 \
                   FROM FileVectorizationQueue fvq \
                   WHERE fvq.file_path = File.path \
               ) \
               AND EXISTS ( \
                   SELECT 1 \
                   FROM Chunk c \
                   LEFT JOIN ChunkEmbedding ce \
                     ON ce.chunk_id = c.id \
                    AND ce.model_id = '{}' \
                    AND ce.source_hash = c.content_hash \
                   WHERE c.file_path = File.path \
                     AND (ce.chunk_id IS NULL OR ce.source_hash IS DISTINCT FROM c.content_hash) \
               )",
            Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID)
        );
        let raw = self.query_json(&query)?;
        if raw == "[]" || raw.is_empty() {
            return Ok(0);
        }

        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        if rows.is_empty() {
            return Ok(0);
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut queries = Vec::new();
        for row in rows {
            let Some(file_path) = row.first().and_then(|value| value.as_str()) else {
                continue;
            };
            queries.push(file_vectorization_queue_upsert_if_needed(file_path, now_ms));
        }

        let inserted = queries.len();
        if inserted == 0 {
            return Ok(0);
        }

        self.execute_batch(&queries)?;
        service_guard::notify_vector_backlog_activity();
        Ok(inserted)
    }

    pub fn fetch_file_vectorization_queue_counts(&self) -> Result<(usize, usize)> {
        let queued = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status IN ('queued', 'paused_for_interactive_priority')")?;
        let inflight = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'")?;
        let queued = usize::try_from(queued).unwrap_or(0);
        let inflight = usize::try_from(inflight).unwrap_or(0);
        Ok((queued, inflight))
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
        let mut degraded_paths = Vec::new();
        let mut skipped_paths = Vec::new();
        let mut seen_symbols = std::collections::HashSet::new();
        let mut seen_calls = std::collections::HashSet::new();
        let mut seen_calls_nif = std::collections::HashSet::new();
        let mut symbol_values = Vec::new();
        let mut chunk_values = Vec::new();
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
                        ProcessingMode::Full => indexed_paths.push(escaped_path.clone()),
                        ProcessingMode::StructureOnly => degraded_paths.push(escaped_path.clone()),
                    }
                    if enqueue_vectorization
                        && matches!(
                            processing_mode,
                            ProcessingMode::Full | ProcessingMode::StructureOnly
                        )
                    {
                        file_vectorization_paths.push(path.clone());
                    }
                    let project_code = extraction.project_code.as_deref().unwrap_or("global");
                    for sym in &extraction.symbols {
                        let symbol_id = Self::symbol_id(project_code, path, &sym.name);
                        if !seen_symbols.insert((symbol_id.clone(), project_code.to_string())) {
                            continue; // Prevent UNIQUE constraint violation in DuckDB ON CONFLICT batches
                        }
                        let chunk_id = Self::chunk_id(&symbol_id);
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
                            let chunk_content = Self::build_chunk_content(
                                path,
                                sym,
                                content.as_deref().unwrap_or_default(),
                            );
                            let chunk_hash = Self::stable_content_hash(&chunk_content);
                            chunk_values.push(format!(
                                "('{}', 'symbol', '{}', '{}', '{}', '{}', '{}', '{}', {}, {})",
                                Self::escape_sql(&chunk_id),
                                Self::escape_sql(&symbol_id),
                                Self::escape_sql(project_code),
                                Self::escape_sql(path),
                                Self::escape_sql(&sym.kind),
                                Self::escape_sql(&chunk_content),
                                Self::escape_sql(&chunk_hash),
                                sym.start_line,
                                sym.end_line
                            ));
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
        if !indexed_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                 SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed' END, \
                     file_stage = CASE WHEN needs_reindex THEN 'promoted' ELSE 'graph_indexed' END, \
                     graph_ready = CASE WHEN needs_reindex THEN FALSE ELSE TRUE END, \
                     vector_ready = CASE \
                         WHEN needs_reindex THEN FALSE \
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
                Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID),
                chrono::Utc::now().timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                indexed_paths.join(",")
            ));
        }
        if !degraded_paths.is_empty() {
            queries.push(format!(
                "UPDATE File \
                     SET status = CASE WHEN needs_reindex THEN 'pending' ELSE 'indexed_degraded' END, \
                     file_stage = CASE WHEN needs_reindex THEN 'promoted' ELSE 'graph_indexed' END, \
                     graph_ready = CASE WHEN needs_reindex THEN FALSE ELSE TRUE END, \
                     vector_ready = CASE \
                         WHEN needs_reindex THEN FALSE \
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
                Self::escape_sql(CHUNK_EMBEDDING_MODEL_ID),
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
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES {} \
                 ON CONFLICT(id) DO UPDATE SET source_type=EXCLUDED.source_type, source_id=EXCLUDED.source_id, project_code=EXCLUDED.project_code, file_path=EXCLUDED.file_path, kind=EXCLUDED.kind, content=EXCLUDED.content, content_hash=EXCLUDED.content_hash, start_line=EXCLUDED.start_line, end_line=EXCLUDED.end_line;",
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
        if !file_vectorization_paths.is_empty() {
            file_vectorization_paths.sort();
            file_vectorization_paths.dedup();
            let now_ms = chrono::Utc::now().timestamp_millis();
            for path in file_vectorization_paths {
                if vectorizable_paths.contains(&path) {
                    queries.push(file_vectorization_queue_upsert_if_needed(&path, now_ms));
                    enqueued_vectorization = true;
                } else {
                    queries.push(format!(
                        "DELETE FROM FileVectorizationQueue WHERE file_path = '{}';",
                        Self::escape_sql(&path)
                    ));
                }
            }
        }
        self.execute_batch(&queries)?;
        if enqueued_vectorization {
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
        self.recent_write_epoch_ms.store(
            chrono::Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
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
        self.recent_write_epoch_ms.store(
            chrono::Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
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
                 SELECT path, COALESCE(project_code, 'proj'), 'vectorization', 'oversized_for_current_budget', 'estimated cost exceeds current budget envelope', {}, NULL, NULL, NULL \
                 FROM File WHERE path = '{}';",
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
        dedup_file_batch_rows, insert_unique_relation_queries, replace_relation_queries,
        sort_and_dedup_sql_tuples, FileUpsertSource, FileVectorizationLeaseSnapshot,
        FileVectorizationWork, VectorBatchRun, VectorPersistOutboxPayload,
        VectorPersistOutboxUpdate, CHUNK_EMBEDDING_MODEL_ID,
    };
    use crate::embedding_contract::{CHUNK_MODEL_ID, DIMENSION};
    use crate::parser::{ExtractionResult, Relation, Symbol};
    use crate::queue::ProcessingMode;
    use crate::worker::DbWriteTask;

    #[test]
    fn sort_and_dedup_sql_tuples_removes_duplicate_relation_rows() {
        let mut values = vec![
            "('b', 'c', 'proj')".to_string(),
            "('a', 'b', 'proj')".to_string(),
            "('b', 'c', 'proj')".to_string(),
            "('a', 'b', 'proj')".to_string(),
            "('c', 'd', 'proj')".to_string(),
        ];

        sort_and_dedup_sql_tuples(&mut values);

        assert_eq!(
            values,
            vec![
                "('a', 'b', 'proj')".to_string(),
                "('b', 'c', 'proj')".to_string(),
                "('c', 'd', 'proj')".to_string(),
            ]
        );
    }

    #[test]
    fn insert_unique_relation_queries_emit_conflict_safe_single_row_inserts() {
        let queries = insert_unique_relation_queries(
            "CALLS",
            &[
                "('a', 'b', 'proj')".to_string(),
                "('c', 'd', 'proj')".to_string(),
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
                "('a', 'b', 'proj')".to_string(),
                "('c', 'd', 'proj')".to_string(),
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
                "proj".to_string(),
                10,
                1,
                100,
                FileUpsertSource::Scan,
            ),
            (
                "/tmp/a.rs".to_string(),
                "proj".to_string(),
                20,
                2,
                200,
                FileUpsertSource::HotDelta,
            ),
            (
                "/tmp/b.rs".to_string(),
                "proj".to_string(),
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
            "proj",
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
                 VALUES ('chunk-1', 'symbol', 'sym-1', 'proj', '/tmp/demo.rs', 'function', 'fn demo() {}', 'hash-1', 1, 1)",
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
                 VALUES ('/tmp/vectorized.rs', 'proj', 'indexed', 'graph_indexed', TRUE, FALSE, 1, 1, 1)",
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
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 42, 1)])
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
                    project_code: Some("proj".to_string()),
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
                "SELECT content FROM Chunk WHERE file_path = '/tmp/chunk_contract.rs' AND project_code = 'proj'",
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
    fn insert_file_data_batch_replay_does_not_duplicate_calls_edges() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/replay_calls.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 42, 1)])
            .unwrap();

        let make_extraction = || ExtractionResult {
            project_code: Some("proj".to_string()),
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
                .query_count("SELECT count(*) FROM CALLS WHERE project_code = 'proj'")
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
                (path_a.clone(), "proj".to_string(), 42, 1),
                (path_b.clone(), "proj".to_string(), 42, 1),
            ])
            .unwrap();

        let make_task = |path: &str| DbWriteTask::FileExtraction {
            reservation_id: format!("res-{path}"),
            path: path.to_string(),
            content: Some("def call, do: :ok".to_string()),
            extraction: ExtractionResult {
                project_code: Some("proj".to_string()),
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
                .query_count("SELECT count(*) FROM CALLS WHERE project_code = 'proj'")
                .unwrap(),
            1
        );
    }

    #[test]
    fn bulk_insert_files_replay_keeps_single_row_per_path() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let path = "/tmp/replay_file_row.rs".to_string();

        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10, 1)])
            .unwrap();
        store
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10, 1)])
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
                started_at_ms: 1,
                finished_at_ms: 1,
                provider: "cpu".to_string(),
                model_id: CHUNK_EMBEDDING_MODEL_ID.to_string(),
                chunk_count: 1,
                file_count: 1,
                input_bytes: 16,
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
             ('chunk-a', 'symbol', 'sym-a', 'proj-a', '/tmp/a.rs', 'function', 'fn a() {}', 'hash-a', 1, 1), \
             ('chunk-b', 'symbol', 'sym-b', 'proj-b', '/tmp/b.rs', 'function', 'fn b() {}', 'hash-b', 1, 1)"
        ).unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, vector_ready, vector_ready_at_ms) VALUES \
             ('/tmp/a.rs', 'proj-a', TRUE, 1000), \
             ('/tmp/b.rs', 'proj-b', TRUE, 1000)",
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
                started_at_ms: 900,
                finished_at_ms: 1000,
                provider: "cuda".to_string(),
                model_id: "chunk-bge-large-en-v1.5".to_string(),
                chunk_count: 2,
                file_count: 2,
                input_bytes: 100,
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
    fn fetch_segments_for_file_reads_writer_when_reader_snapshot_is_stale() {
        use std::sync::atomic::Ordering;

        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store.refresh_reader_snapshot().unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) VALUES \
                 ('chunk-stale', 'symbol', 'sym-stale', 'proj', '/tmp/stale.rs', 'function', 'fresh', 'hash-stale', 1, 2)",
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
                 VALUES ('/tmp/ready.rs', 'proj', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, TRUE)",
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
                 VALUES ('/tmp/not_ready.rs', 'proj', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
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
            .bulk_insert_files(&[(path.clone(), "proj".to_string(), 10, 1)])
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
                project_code: Some("proj".to_string()),
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
    fn backfill_file_vectorization_queue_skips_files_already_present_in_queue() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        store
            .execute(
                "INSERT INTO File (path, project_code, status, size, mtime, priority, file_stage, graph_ready, vector_ready) \
                 VALUES ('/tmp/already_queued.rs', 'proj', 'indexed', 1, 1, 100, 'graph_indexed', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-already-queued', 'symbol', 'sym-already-queued', 'proj', '/tmp/already_queued.rs', 'function', 'body', 'hash-already-queued', 1, 1)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO CONTAINS (source_id, target_id, project_code) \
                 VALUES ('/tmp/already_queued.rs', 'sym-already-queued', 'proj')",
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
                 VALUES ('/tmp/oversized.rs', 'proj', 'oversized_for_current_budget', 1, 1, 100, 'oversized', TRUE, FALSE)",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO Chunk (id, source_type, source_id, project_code, file_path, kind, content, content_hash, start_line, end_line) \
                 VALUES ('chunk-oversized', 'symbol', 'sym-oversized', 'proj', '/tmp/oversized.rs', 'function', 'body', 'hash-oversized', 1, 1)",
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
        let now_ms = chrono::Utc::now().timestamp_millis();
        let safe_path = Self::escape_sql(path);
        let safe_project = Self::escape_sql(project);
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
                "INSERT INTO File (path, project_code, size, mtime, status, priority, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, first_seen_at_ms, last_state_change_at_ms) \
                 VALUES ('{}', '{}', {}, {}, 'pending', {}, FALSE, NULL, 'discovered_new', 0, NULL, {}, {}) \
                 ON CONFLICT(path) DO NOTHING;",
                safe_path,
                safe_project,
                size,
                mtime,
                priority,
                now_ms,
                now_ms
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
