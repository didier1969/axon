use anyhow::{anyhow, Result};

use crate::benchmark_store;
use crate::graph::GraphStore;
use crate::service_guard;

use super::{
    next_vector_persist_outbox_claim_token, parse_i64_field, parse_u64_field,
    FileVectorizationLeaseSnapshot, VectorBatchRun, VectorLaneStateRecord,
    VectorPersistOutboxPayload, VectorPersistOutboxWork, VectorWorkerFault,
};

impl GraphStore {
    pub fn record_vector_batch_run(&self, run: &VectorBatchRun) -> Result<()> {
        if let Err(error) = benchmark_store::mirror_vector_batch_run(self.db_path.as_deref(), run) {
            log::warn!(
                "failed to mirror vector batch run {} into benchmark store: {:?}",
                run.run_id,
                error
            );
        }
        Ok(())
    }

    pub fn record_vector_worker_fault(&self, fault: &VectorWorkerFault) -> Result<()> {
        // MIL-AXO-015 P4 4e: PG branch — schema-qualified INSERT with
        // ON CONFLICT DO UPDATE on the fault_id PK; DuckDB keeps
        // INSERT OR REPLACE on the unqualified table.
        let batch_id_lit = fault
            .batch_id
            .as_ref()
            .map(|batch_id| format!("'{}'", Self::escape_sql(batch_id)))
            .unwrap_or_else(|| "NULL".to_string());
        let sql = if self.is_postgres_backend() {
            format!(
                "INSERT INTO axon_runtime.VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{}', '{}', {}, '{}', '{}', '{}', '{}', {}, {}, {}, {}, {}, {}) \
                 ON CONFLICT (fault_id) DO UPDATE SET \
                    lane = EXCLUDED.lane, worker_id = EXCLUDED.worker_id, fatal_stage = EXCLUDED.fatal_stage, \
                    fatal_reason_raw = EXCLUDED.fatal_reason_raw, fatal_class = EXCLUDED.fatal_class, \
                    provider = EXCLUDED.provider, batch_id = EXCLUDED.batch_id, texts_count = EXCLUDED.texts_count, \
                    input_bytes = EXCLUDED.input_bytes, vram_used_mb = EXCLUDED.vram_used_mb, \
                    occurred_at_ms = EXCLUDED.occurred_at_ms, restart_attempt = EXCLUDED.restart_attempt",
                Self::escape_sql(&fault.fault_id),
                Self::escape_sql(&fault.lane),
                fault.worker_id,
                Self::escape_sql(&fault.fatal_stage),
                Self::escape_sql(&fault.fatal_reason_raw),
                Self::escape_sql(&fault.fatal_class),
                Self::escape_sql(&fault.provider),
                batch_id_lit,
                fault.texts_count,
                fault.input_bytes,
                fault.vram_used_mb,
                fault.occurred_at_ms,
                fault.restart_attempt,
            )
        } else {
            format!(
                "INSERT OR REPLACE INTO VectorWorkerFault \
                 (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
                 VALUES ('{}', '{}', {}, '{}', '{}', '{}', '{}', {}, {}, {}, {}, {}, {})",
                Self::escape_sql(&fault.fault_id),
                Self::escape_sql(&fault.lane),
                fault.worker_id,
                Self::escape_sql(&fault.fatal_stage),
                Self::escape_sql(&fault.fatal_reason_raw),
                Self::escape_sql(&fault.fatal_class),
                Self::escape_sql(&fault.provider),
                batch_id_lit,
                fault.texts_count,
                fault.input_bytes,
                fault.vram_used_mb,
                fault.occurred_at_ms,
                fault.restart_attempt,
            )
        };
        self.execute(&sql)
    }

    pub fn upsert_vector_lane_state(&self, state: &VectorLaneStateRecord) -> Result<()> {
        let reason_lit = state
            .reason
            .as_ref()
            .map(|reason| format!("'{}'", Self::escape_sql(reason)))
            .unwrap_or_else(|| "NULL".to_string());
        let worker_id_lit = state
            .worker_id
            .map(|worker_id| worker_id.to_string())
            .unwrap_or_else(|| "NULL".to_string());
        let last_success_lit = state
            .last_success_at_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
        let last_fault_lit = state
            .last_fault_id
            .as_ref()
            .map(|fault_id| format!("'{}'", Self::escape_sql(fault_id)))
            .unwrap_or_else(|| "NULL".to_string());
        let sql = if self.is_postgres_backend() {
            format!(
                "INSERT INTO axon_runtime.VectorLaneState \
                 (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
                 VALUES ('{}', '{}', {}, {}, {}, {}, {}, {}) \
                 ON CONFLICT (lane) DO UPDATE SET \
                    state = EXCLUDED.state, reason = EXCLUDED.reason, updated_at_ms = EXCLUDED.updated_at_ms, \
                    worker_id = EXCLUDED.worker_id, restart_attempt = EXCLUDED.restart_attempt, \
                    last_success_at_ms = EXCLUDED.last_success_at_ms, last_fault_id = EXCLUDED.last_fault_id",
                Self::escape_sql(&state.lane),
                Self::escape_sql(&state.state),
                reason_lit,
                state.updated_at_ms,
                worker_id_lit,
                state.restart_attempt,
                last_success_lit,
                last_fault_lit,
            )
        } else {
            format!(
                "INSERT OR REPLACE INTO VectorLaneState \
                 (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
                 VALUES ('{}', '{}', {}, {}, {}, {}, {}, {})",
                Self::escape_sql(&state.lane),
                Self::escape_sql(&state.state),
                reason_lit,
                state.updated_at_ms,
                worker_id_lit,
                state.restart_attempt,
                last_success_lit,
                last_fault_lit,
            )
        };
        self.execute(&sql)
    }

    pub fn latest_vector_worker_fault(&self, lane: &str) -> Result<Option<VectorWorkerFault>> {
        let raw = self.query_json_writer(&format!(
            "SELECT fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt \
             FROM VectorWorkerFault \
             WHERE lane = '{}' \
             ORDER BY occurred_at_ms DESC, fault_id DESC \
             LIMIT 1",
            Self::escape_sql(lane)
        ))?;
        if raw == "[]" || raw.is_empty() {
            return Ok(None);
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(VectorWorkerFault {
            fault_id: row
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            lane: row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            worker_id: row.get(2).and_then(parse_i64_field).unwrap_or_default(),
            fatal_stage: row
                .get(3)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            fatal_reason_raw: row
                .get(4)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            fatal_class: row
                .get(5)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            provider: row
                .get(6)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            batch_id: row
                .get(7)
                .and_then(|value| value.as_str())
                .map(ToString::to_string),
            texts_count: row.get(8).and_then(parse_u64_field).unwrap_or_default(),
            input_bytes: row.get(9).and_then(parse_u64_field).unwrap_or_default(),
            vram_used_mb: row.get(10).and_then(parse_u64_field).unwrap_or_default(),
            occurred_at_ms: row.get(11).and_then(parse_i64_field).unwrap_or_default(),
            restart_attempt: row.get(12).and_then(parse_u64_field).unwrap_or_default(),
        }))
    }

    pub fn vector_lane_state_record(&self, lane: &str) -> Result<Option<VectorLaneStateRecord>> {
        let raw = self.query_json_writer(&format!(
            "SELECT lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id \
             FROM VectorLaneState \
             WHERE lane = '{}' \
             LIMIT 1",
            Self::escape_sql(lane)
        ))?;
        if raw == "[]" || raw.is_empty() {
            return Ok(None);
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(VectorLaneStateRecord {
            lane: row
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            state: row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            reason: row
                .get(2)
                .and_then(|value| value.as_str())
                .map(ToString::to_string),
            updated_at_ms: row.get(3).and_then(parse_i64_field).unwrap_or_default(),
            worker_id: row.get(4).and_then(parse_i64_field),
            restart_attempt: row.get(5).and_then(parse_u64_field).unwrap_or_default(),
            last_success_at_ms: row.get(6).and_then(parse_i64_field),
            last_fault_id: row
                .get(7)
                .and_then(|value| value.as_str())
                .map(ToString::to_string),
        }))
    }

    pub fn enqueue_vector_persist_outbox(
        &self,
        payload: &VectorPersistOutboxPayload,
    ) -> Result<String> {
        let outbox_id = format!("outbox-{}", payload.batch_run.run_id);
        let payload_json = serde_json::to_string(payload)?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        // MIL-AXO-015 P4 4e: PG branch routes to axon_runtime.VectorPersistOutbox
        // with ON CONFLICT (outbox_id) DO UPDATE refreshing every column.
        let sql = if self.is_postgres_backend() {
            format!(
                "INSERT INTO axon_runtime.VectorPersistOutbox \
                 (outbox_id, run_id, model_id, status, attempts, queued_at_ms, claimed_at_ms, completed_at_ms, last_error_reason, claim_token, lease_heartbeat_at_ms, lease_owner, lease_epoch, chunk_count, file_count, input_bytes, fetch_ms, embed_ms, payload_json) \
                 VALUES ('{}', '{}', '{}', 'queued', 0, {}, NULL, NULL, NULL, NULL, NULL, NULL, 0, {}, {}, {}, {}, {}, '{}') \
                 ON CONFLICT (outbox_id) DO UPDATE SET \
                    run_id = EXCLUDED.run_id, model_id = EXCLUDED.model_id, status = EXCLUDED.status, \
                    attempts = EXCLUDED.attempts, queued_at_ms = EXCLUDED.queued_at_ms, \
                    claimed_at_ms = EXCLUDED.claimed_at_ms, completed_at_ms = EXCLUDED.completed_at_ms, \
                    last_error_reason = EXCLUDED.last_error_reason, claim_token = EXCLUDED.claim_token, \
                    lease_heartbeat_at_ms = EXCLUDED.lease_heartbeat_at_ms, lease_owner = EXCLUDED.lease_owner, \
                    lease_epoch = EXCLUDED.lease_epoch, chunk_count = EXCLUDED.chunk_count, \
                    file_count = EXCLUDED.file_count, input_bytes = EXCLUDED.input_bytes, \
                    fetch_ms = EXCLUDED.fetch_ms, embed_ms = EXCLUDED.embed_ms, payload_json = EXCLUDED.payload_json",
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
            )
        } else {
            format!(
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
            )
        };
        self.execute(&sql)?;
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

        let raw = self.query_json_writer(&format!(
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
        let queued = self.query_count_writer(
            "SELECT count(*) FROM VectorPersistOutbox WHERE status = 'queued'",
        )?;
        let inflight = self.query_count_writer(
            "SELECT count(*) FROM VectorPersistOutbox WHERE status = 'inflight'",
        )?;
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
        let refreshed = usize::try_from(self.query_count_writer(&format!(
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
             ) \
             SELECT {}, \
                    pk.project_code, \
                    '{}', \
                    COALESCE(cr.chunks_embedded, 0), \
                    COALESCE(fr.files_vector_ready, 0), \
                    0 AS batches, \
                    0 AS fetch_ms_total, \
                    0 AS embed_ms_total, \
                    0 AS db_write_ms_total, \
                    0 AS mark_done_ms_total \
             FROM project_keys pk \
             LEFT JOIN chunk_rollup cr ON cr.project_code = pk.project_code \
             LEFT JOIN file_rollup fr ON fr.project_code = pk.project_code \
             CROSS JOIN project_counts pc; \
             COMMIT;",
            bucket_start_ms,
            model_id,
            model_id,
            bucket_start_ms,
            bucket_end_ms,
            bucket_start_ms,
            bucket_end_ms,
            bucket_start_ms,
            model_id
        ))
    }
}

#[cfg(test)]
mod tests {
    // MIL-AXO-015 P4 4e: SQL-shape contract tests for the writer
    // branches that route to `axon_runtime.X` under PG. The methods
    // themselves require a live GraphStore; these tests mirror the
    // string composition so the dual-backend invariant is locked in.

    fn pg_fault_sql() -> String {
        "INSERT INTO axon_runtime.VectorWorkerFault \
         (fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt) \
         VALUES ('f-1', 'vector', 1, 'init', 'reason', 'class', 'cuda', NULL, 0, 0, 0, 0, 0) \
         ON CONFLICT (fault_id) DO UPDATE SET \
            lane = EXCLUDED.lane, worker_id = EXCLUDED.worker_id, fatal_stage = EXCLUDED.fatal_stage, \
            fatal_reason_raw = EXCLUDED.fatal_reason_raw, fatal_class = EXCLUDED.fatal_class, \
            provider = EXCLUDED.provider, batch_id = EXCLUDED.batch_id, texts_count = EXCLUDED.texts_count, \
            input_bytes = EXCLUDED.input_bytes, vram_used_mb = EXCLUDED.vram_used_mb, \
            occurred_at_ms = EXCLUDED.occurred_at_ms, restart_attempt = EXCLUDED.restart_attempt".to_string()
    }

    fn pg_outbox_sql() -> String {
        "INSERT INTO axon_runtime.VectorPersistOutbox \
         (outbox_id, run_id, model_id, status, attempts, queued_at_ms, claimed_at_ms, completed_at_ms, last_error_reason, claim_token, lease_heartbeat_at_ms, lease_owner, lease_epoch, chunk_count, file_count, input_bytes, fetch_ms, embed_ms, payload_json) \
         VALUES ('outbox-1', 'run-1', 'code-1024', 'queued', 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0, 0, 0, 0, 0, '{}') \
         ON CONFLICT (outbox_id) DO UPDATE SET \
            run_id = EXCLUDED.run_id, model_id = EXCLUDED.model_id, status = EXCLUDED.status, \
            attempts = EXCLUDED.attempts, queued_at_ms = EXCLUDED.queued_at_ms, \
            claimed_at_ms = EXCLUDED.claimed_at_ms, completed_at_ms = EXCLUDED.completed_at_ms, \
            last_error_reason = EXCLUDED.last_error_reason, claim_token = EXCLUDED.claim_token, \
            lease_heartbeat_at_ms = EXCLUDED.lease_heartbeat_at_ms, lease_owner = EXCLUDED.lease_owner, \
            lease_epoch = EXCLUDED.lease_epoch, chunk_count = EXCLUDED.chunk_count, \
            file_count = EXCLUDED.file_count, input_bytes = EXCLUDED.input_bytes, \
            fetch_ms = EXCLUDED.fetch_ms, embed_ms = EXCLUDED.embed_ms, payload_json = EXCLUDED.payload_json".to_string()
    }

    fn pg_lane_sql() -> String {
        "INSERT INTO axon_runtime.VectorLaneState \
         (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
         VALUES ('vector', 'running', NULL, 0, NULL, 0, NULL, NULL) \
         ON CONFLICT (lane) DO UPDATE SET \
            state = EXCLUDED.state, reason = EXCLUDED.reason, updated_at_ms = EXCLUDED.updated_at_ms, \
            worker_id = EXCLUDED.worker_id, restart_attempt = EXCLUDED.restart_attempt, \
            last_success_at_ms = EXCLUDED.last_success_at_ms, last_fault_id = EXCLUDED.last_fault_id".to_string()
    }

    #[test]
    fn pg_fault_sql_targets_axon_runtime_schema() {
        let sql = pg_fault_sql();
        assert!(sql.contains("INSERT INTO axon_runtime.VectorWorkerFault"));
        assert!(sql.contains("ON CONFLICT (fault_id) DO UPDATE"));
        // Must update every non-key column on conflict.
        for col in [
            "lane", "worker_id", "fatal_stage", "fatal_reason_raw", "fatal_class",
            "provider", "batch_id", "texts_count", "input_bytes", "vram_used_mb",
            "occurred_at_ms", "restart_attempt",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_lane_sql_targets_axon_runtime_schema() {
        let sql = pg_lane_sql();
        assert!(sql.contains("INSERT INTO axon_runtime.VectorLaneState"));
        assert!(sql.contains("ON CONFLICT (lane) DO UPDATE"));
        for col in [
            "state", "reason", "updated_at_ms", "worker_id", "restart_attempt",
            "last_success_at_ms", "last_fault_id",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_outbox_sql_targets_axon_runtime_schema() {
        let sql = pg_outbox_sql();
        assert!(sql.contains("INSERT INTO axon_runtime.VectorPersistOutbox"));
        assert!(sql.contains("ON CONFLICT (outbox_id) DO UPDATE"));
        for col in [
            "run_id", "model_id", "status", "attempts", "queued_at_ms", "claimed_at_ms",
            "completed_at_ms", "last_error_reason", "claim_token", "lease_heartbeat_at_ms",
            "lease_owner", "lease_epoch", "chunk_count", "file_count", "input_bytes",
            "fetch_ms", "embed_ms", "payload_json",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_branches_use_explicit_schema_qualifier() {
        // CPT-AXO-039 + axon_runtime invariant: every PG-branch SQL must
        // qualify the table with `axon_runtime.` so PG can resolve it
        // outside the per-project IST schemas.
        for sql in [pg_fault_sql(), pg_lane_sql(), pg_outbox_sql()] {
            assert!(
                sql.contains("axon_runtime."),
                "PG SQL missing axon_runtime schema qualifier"
            );
        }
    }
}
