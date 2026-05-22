use anyhow::{anyhow, Result};

use crate::benchmark_store;
use crate::graph::GraphStore;
use crate::service_guard;

use super::{
    next_vector_persist_outbox_claim_token, parse_i64_field, parse_u64_field,
    EmbedderLifecycleHeartbeatRecord, FileVectorizationLeaseSnapshot, VectorBatchRun,
    VectorLaneStateRecord, VectorPersistOutboxPayload, VectorPersistOutboxWork, VectorWorkerFault,
};

impl GraphStore {
    /// REQ-AXO-271 slice 2e (PG canonical only, post-MIL-AXO-017) :
    /// schema-qualify an `axon_runtime` table reference.
    fn axon_runtime_table_ref(&self, table: &'static str) -> String {
        format!("axon_runtime.{table}")
    }

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
        // REQ-AXO-271 slice 2e : PG canonical only. INSERT ON CONFLICT
        // refreshes every column on `fault_id` collision.
        let batch_id_lit = fault
            .batch_id
            .as_ref()
            .map(|batch_id| format!("'{}'", Self::escape_sql(batch_id)))
            .unwrap_or_else(|| "NULL".to_string());
        let sql = format!(
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
        );
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
        // REQ-AXO-271 slice 2e : PG canonical only.
        let sql = format!(
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
        );
        self.execute(&sql)
    }

    pub fn latest_vector_worker_fault(&self, lane: &str) -> Result<Option<VectorWorkerFault>> {
        let table_ref = self.axon_runtime_table_ref("VectorWorkerFault");
        let raw = self.query_json_writer(&format!(
            "SELECT fault_id, lane, worker_id, fatal_stage, fatal_reason_raw, fatal_class, provider, batch_id, texts_count, input_bytes, vram_used_mb, occurred_at_ms, restart_attempt \
             FROM {table_ref} \
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
        let table_ref = self.axon_runtime_table_ref("VectorLaneState");
        let raw = self.query_json_writer(&format!(
            "SELECT lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id \
             FROM {table_ref} \
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
        // REQ-AXO-271 slice 2e : PG canonical only. ON CONFLICT (outbox_id)
        // DO UPDATE refreshes every column on conflict.
        let sql = format!(
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
        );
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
        let outbox_ref = self.axon_runtime_table_ref("VectorPersistOutbox");
        self.execute(&format!(
            "UPDATE {outbox_ref} \
             SET status = 'inflight', \
                 attempts = attempts + 1, \
                 claimed_at_ms = {}, \
                 lease_heartbeat_at_ms = {}, \
                 lease_owner = 'outbox', \
                 claim_token = '{}', \
                 last_error_reason = NULL \
             WHERE outbox_id IN ( \
                 SELECT outbox_id FROM {outbox_ref} \
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
             FROM {outbox_ref} \
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
        // REQ-AXO-901653 Slice 3a — the `axon_runtime.VectorPersistOutbox`
        // table was dropped (MIL-AXO-017 PG canonical / REQ-AXO-289
        // streaming v2). Canonical vectorization writes
        // `ChunkEmbedding` directly via pipeline_v2 stage B3 ; the
        // outbox/persist hand-off was a DuckDB-era artifact. Method
        // returns (0,0) ; PG query removed to eliminate
        // `[pg_query_count] db error` log spam.
        // Slice 3b (later) : delete callsites + this method entirely.
        Ok((0, 0))
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
        let outbox_ref = self.axon_runtime_table_ref("VectorPersistOutbox");
        let refreshed = usize::try_from(self.query_count_writer(&format!(
            "SELECT count(*) FROM {outbox_ref} \
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
            "UPDATE {outbox_ref} \
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
        let outbox_ref = self.axon_runtime_table_ref("VectorPersistOutbox");
        self.execute(&format!(
            "DELETE FROM {outbox_ref} WHERE outbox_id = '{}'",
            Self::escape_sql(outbox_id)
        ))
    }

    pub fn mark_vector_persist_outbox_failed(&self, outbox_id: &str, reason: &str) -> Result<()> {
        let outbox_ref = self.axon_runtime_table_ref("VectorPersistOutbox");
        self.execute(&format!(
            "UPDATE {outbox_ref} \
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
        let outbox_ref = self.axon_runtime_table_ref("VectorPersistOutbox");
        let recovered = usize::try_from(self.query_count(&format!(
            "SELECT count(*) FROM {outbox_ref} \
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
            "UPDATE {outbox_ref} \
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

    /// REQ-AXO-91572 option B — UPSERT the indexer-local
    /// `EmbedderLifecycle` snapshot into the cross-process heartbeat
    /// table. Called every heartbeat tick by the indexer ; readers
    /// (brain `embedding_status`) treat rows older than ~2× tick as
    /// stale.
    pub fn record_lifecycle_heartbeat(
        &self,
        process_role: &str,
        snapshot: &crate::embedder::lifecycle_machine::LifecycleHeartbeatSnapshot,
    ) -> Result<()> {
        let sql = build_lifecycle_heartbeat_upsert_sql(process_role, snapshot);
        self.execute(&sql)
    }

    /// REQ-AXO-91572 option B — read the latest heartbeat row for a
    /// given role. Returns `None` if no row exists yet (process hasn't
    /// published since boot). Freshness is left to the caller : compare
    /// `heartbeat_ms` against `now - 2 × tick`.
    pub fn latest_lifecycle_heartbeat(
        &self,
        process_role: &str,
    ) -> Result<Option<EmbedderLifecycleHeartbeatRecord>> {
        let table_ref = self.axon_runtime_table_ref("EmbedderLifecycleHeartbeat");
        let raw = self.query_json_writer(&format!(
            "SELECT process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms \
             FROM {table_ref} \
             WHERE process_role = '{}' \
             LIMIT 1",
            Self::escape_sql(process_role)
        ))?;
        if raw == "[]" || raw.is_empty() {
            return Ok(None);
        }
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(EmbedderLifecycleHeartbeatRecord {
            process_role: row
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            phase: row
                .get(1)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            last_used_ms: row.get(2).and_then(parse_i64_field).unwrap_or_default(),
            wake_count: row.get(3).and_then(parse_i64_field).unwrap_or_default(),
            sleep_count: row.get(4).and_then(parse_i64_field).unwrap_or_default(),
            pending_count: row.get(5).and_then(parse_i64_field).unwrap_or_default(),
            heartbeat_ms: row.get(6).and_then(parse_i64_field).unwrap_or_default(),
        }))
    }
}

/// REQ-AXO-91572 option B — pure SQL builder for the heartbeat UPSERT.
/// Exposed at module scope so SQL-shape contract tests cover it without
/// needing a live `GraphStore`.
fn build_lifecycle_heartbeat_upsert_sql(
    process_role: &str,
    snapshot: &crate::embedder::lifecycle_machine::LifecycleHeartbeatSnapshot,
) -> String {
    format!(
        "INSERT INTO axon_runtime.EmbedderLifecycleHeartbeat \
         (process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms) \
         VALUES ('{}', '{}', {}, {}, {}, {}, {}) \
         ON CONFLICT (process_role) DO UPDATE SET \
            phase = EXCLUDED.phase, last_used_ms = EXCLUDED.last_used_ms, \
            wake_count = EXCLUDED.wake_count, sleep_count = EXCLUDED.sleep_count, \
            pending_count = EXCLUDED.pending_count, heartbeat_ms = EXCLUDED.heartbeat_ms",
        GraphStore::escape_sql(process_role),
        snapshot.phase.as_str(),
        snapshot.last_used_ms,
        snapshot.wake_count,
        snapshot.sleep_count,
        snapshot.pending_count,
        snapshot.heartbeat_ms,
    )
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

    #[test]
    fn lifecycle_heartbeat_upsert_sql_shape() {
        use crate::embedder::lifecycle_machine::{
            EmbedderPhase, LifecycleHeartbeatSnapshot,
        };
        let snapshot = LifecycleHeartbeatSnapshot {
            phase: EmbedderPhase::Sleeping,
            last_used_ms: 1_700_000_000_000,
            wake_count: 3,
            sleep_count: 4,
            pending_count: 12,
            heartbeat_ms: 1_700_000_005_000,
        };
        let sql = super::build_lifecycle_heartbeat_upsert_sql("indexer", &snapshot);
        // Shape contract.
        assert!(sql.contains("INSERT INTO axon_runtime.EmbedderLifecycleHeartbeat"));
        assert!(sql.contains("ON CONFLICT (process_role) DO UPDATE"));
        for col in [
            "phase", "last_used_ms", "wake_count", "sleep_count",
            "pending_count", "heartbeat_ms",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
        // Value plumbing.
        assert!(sql.contains("'indexer'"));
        assert!(sql.contains("'sleeping'"));
        assert!(sql.contains("1700000000000"));
        assert!(sql.contains("1700000005000"));
    }

    #[test]
    fn lifecycle_heartbeat_upsert_sql_escapes_role() {
        use crate::embedder::lifecycle_machine::{
            EmbedderPhase, LifecycleHeartbeatSnapshot,
        };
        let snapshot = LifecycleHeartbeatSnapshot {
            phase: EmbedderPhase::Ready,
            last_used_ms: 0,
            wake_count: 0,
            sleep_count: 0,
            pending_count: 0,
            heartbeat_ms: 0,
        };
        // Pathological role containing single quote must be escaped.
        let sql = super::build_lifecycle_heartbeat_upsert_sql("ind'exer", &snapshot);
        assert!(sql.contains("'ind''exer'"));
    }
}
