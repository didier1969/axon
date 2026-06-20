use anyhow::Result;

use crate::graph::GraphStore;
use crate::service_guard;

use super::sql_helpers::{parse_i64_field, parse_u64_field};
use super::{
    EmbedderLifecycleHeartbeatRecord, EmbedderObservedState, IndexerRuntimeTruthRecord,
    VectorLaneStateRecord, VectorWorkerFault,
};

impl GraphStore {
    /// REQ-AXO-271 slice 2e (PG canonical only, post-MIL-AXO-017) :
    /// schema-qualify an `axon` table reference.
    fn axon_table_ref(&self, table: &'static str) -> String {
        format!("axon.{table}")
    }

    pub fn latest_vector_worker_fault(&self, lane: &str) -> Result<Option<VectorWorkerFault>> {
        let table_ref = self.axon_table_ref("VectorWorkerFault");
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
        let table_ref = self.axon_table_ref("VectorLaneState");
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

    pub fn recover_stale_vector_persist_outbox_inflight(
        &self,
        stale_before_ms: i64,
    ) -> Result<usize> {
        let outbox_ref = self.axon_table_ref("VectorPersistOutbox");
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

    // REQ-AXO-901653 slice-5c — `refresh_hourly_vectorization_rollup` deleted ;
    // zero production callers, the implementation referenced the dropped
    // `public.File` table (vector_ready / vector_ready_at_ms columns gone).
    // Pipeline_v2 stage B3 emits per-chunk telemetry directly.

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
        let table_ref = self.axon_table_ref("EmbedderLifecycleHeartbeat");
        let raw = self.query_json_writer(&format!(
            "SELECT process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms, compute, compute_source, build_id, \
                    b3_consecutive_failures, b3_total_failures, b3_total_successes, b3_last_error, b3_last_error_count, b3_last_error_last_seen_ms \
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
            compute: row
                .get(7)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            compute_source: row
                .get(8)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            build_id: row
                .get(9)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            // REQ-AXO-902047 slice 1b — B3 health (columns added by additive
            // ALTER ; rows from a publisher predating the columns surface as 0 /
            // None, which the reader treats as "healthy / no error yet").
            b3_consecutive_failures: row.get(10).and_then(parse_i64_field).unwrap_or_default(),
            b3_total_failures: row.get(11).and_then(parse_i64_field).unwrap_or_default(),
            b3_total_successes: row.get(12).and_then(parse_i64_field).unwrap_or_default(),
            b3_last_error: row
                .get(13)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            b3_last_error_count: row.get(14).and_then(parse_i64_field).unwrap_or_default(),
            b3_last_error_last_seen_ms: row.get(15).and_then(parse_i64_field).unwrap_or_default(),
        }))
    }

    /// REQ-AXO-901854 — UPSERT the indexer's observed worker + embed-rate +
    /// in-flight + queue counters into the cross-process truth table. Called
    /// every heartbeat tick by the indexer (pipeline owner); the brain reads
    /// the row via `latest_indexer_runtime_truth`.
    pub fn record_indexer_runtime_truth(&self, row: &IndexerRuntimeTruthRecord) -> Result<()> {
        self.execute(&build_indexer_runtime_truth_upsert_sql(row))
    }

    /// REQ-AXO-901854 — read the latest indexer runtime-truth row for a role.
    /// Returns `None` if the indexer hasn't published since boot. Freshness is
    /// the caller's job: compare `heartbeat_ms` against `now - 2 × tick`.
    pub fn latest_indexer_runtime_truth(
        &self,
        process_role: &str,
    ) -> Result<Option<IndexerRuntimeTruthRecord>> {
        let table_ref = self.axon_table_ref("indexer_runtime_truth");
        let raw = self.query_json_writer(&format!(
            "SELECT process_role, heartbeat_ms, graph_workers_active, chunk_embeddings_per_second, \
                    in_flight_count, oldest_in_flight_path, oldest_in_flight_stage, \
                    oldest_in_flight_age_ms, ready_queue_chunks, persist_queue_depth \
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
        let opt_str = |v: Option<&serde_json::Value>| {
            v.and_then(|value| value.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        };
        Ok(Some(IndexerRuntimeTruthRecord {
            process_role: row
                .first()
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            heartbeat_ms: row.get(1).and_then(parse_i64_field).unwrap_or_default(),
            graph_workers_active: row.get(2).and_then(parse_i64_field).unwrap_or_default(),
            chunk_embeddings_per_second: row
                .get(3)
                .and_then(|value| value.as_f64().or_else(|| value.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or_default(),
            in_flight_count: row.get(4).and_then(parse_i64_field).unwrap_or_default(),
            oldest_in_flight_path: opt_str(row.get(5)),
            oldest_in_flight_stage: opt_str(row.get(6)),
            oldest_in_flight_age_ms: row.get(7).and_then(parse_i64_field).unwrap_or_default(),
            ready_queue_chunks: row.get(8).and_then(parse_i64_field).unwrap_or_default(),
            persist_queue_depth: row.get(9).and_then(parse_i64_field).unwrap_or_default(),
        }))
    }

    /// DEC-AXO-901626 — PG-canonical embedder observation in one round-trip
    /// via `axon.embedder_observed_state()`. Feeds the brain
    /// composer's `embedder_runtime` block (throughput + staleness) and the
    /// `pg_inferred` GPU fallback when `nvidia-smi` is unreachable.
    pub fn embedder_observed_state(&self) -> Result<EmbedderObservedState> {
        let raw = self.query_json_writer(
            "SELECT (s->>'embedded_60s')::bigint, \
                    (s->>'embedded_total')::bigint, \
                    (s->>'oldest_pending_age_s')::bigint \
             FROM (SELECT axon.embedder_observed_state() AS s) q",
        )?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Ok(EmbedderObservedState::default());
        };
        Ok(EmbedderObservedState {
            embedded_60s: row.first().and_then(parse_i64_field).unwrap_or_default(),
            embedded_total: row.get(1).and_then(parse_i64_field).unwrap_or_default(),
            oldest_pending_age_s: row.get(2).and_then(parse_i64_field).unwrap_or_default(),
        })
    }
}

/// REQ-AXO-91572 option B — pure SQL builder for the heartbeat UPSERT.
/// Exposed at module scope so SQL-shape contract tests cover it without
/// needing a live `GraphStore`.
fn build_lifecycle_heartbeat_upsert_sql(
    process_role: &str,
    snapshot: &crate::embedder::lifecycle_machine::LifecycleHeartbeatSnapshot,
) -> String {
    // DEC-AXO-901626 — `build_id` is NULL when the env var is unset; SQL
    // literal is built accordingly (quoted string vs NULL keyword).
    let build_id_sql = match snapshot.build_id.as_deref() {
        Some(value) => format!("'{}'", GraphStore::escape_sql(value)),
        None => "NULL".to_string(),
    };
    // REQ-AXO-902047 slice 1b — B3 health columns. `b3_last_error` is the full
    // anyhow chain (root PG message + SQLSTATE) or NULL when B3 has never failed.
    let (b3_last_error_sql, b3_last_error_count, b3_last_error_last_seen_ms) =
        match snapshot.b3.last_error.as_ref() {
            Some(rec) => (
                format!("'{}'", GraphStore::escape_sql(&rec.message)),
                rec.count,
                rec.last_seen_ms,
            ),
            None => ("NULL".to_string(), 0, 0),
        };
    format!(
        "INSERT INTO axon.EmbedderLifecycleHeartbeat \
         (process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms, compute, compute_source, build_id, \
          b3_consecutive_failures, b3_total_failures, b3_total_successes, b3_last_error, b3_last_error_count, b3_last_error_last_seen_ms) \
         VALUES ('{}', '{}', {}, {}, {}, {}, {}, '{}', '{}', {}, {}, {}, {}, {}, {}, {}) \
         ON CONFLICT (process_role) DO UPDATE SET \
            phase = EXCLUDED.phase, last_used_ms = EXCLUDED.last_used_ms, \
            wake_count = EXCLUDED.wake_count, sleep_count = EXCLUDED.sleep_count, \
            pending_count = EXCLUDED.pending_count, heartbeat_ms = EXCLUDED.heartbeat_ms, \
            compute = EXCLUDED.compute, compute_source = EXCLUDED.compute_source, build_id = EXCLUDED.build_id, \
            b3_consecutive_failures = EXCLUDED.b3_consecutive_failures, \
            b3_total_failures = EXCLUDED.b3_total_failures, \
            b3_total_successes = EXCLUDED.b3_total_successes, \
            b3_last_error = EXCLUDED.b3_last_error, \
            b3_last_error_count = EXCLUDED.b3_last_error_count, \
            b3_last_error_last_seen_ms = EXCLUDED.b3_last_error_last_seen_ms",
        GraphStore::escape_sql(process_role),
        snapshot.phase.as_str(),
        snapshot.last_used_ms,
        snapshot.wake_count,
        snapshot.sleep_count,
        snapshot.pending_count,
        snapshot.heartbeat_ms,
        GraphStore::escape_sql(&snapshot.compute),
        GraphStore::escape_sql(&snapshot.compute_source),
        build_id_sql,
        snapshot.b3.consecutive_failures,
        snapshot.b3.total_failures,
        snapshot.b3.total_successes,
        b3_last_error_sql,
        b3_last_error_count,
        b3_last_error_last_seen_ms,
    )
}

/// REQ-AXO-901854 — pure SQL builder for the indexer runtime-truth UPSERT.
/// Module-scope so the SQL-shape contract test covers it without a live
/// `GraphStore`. `chunk_embeddings_per_second` is a float — formatted with a
/// fixed precision so the literal is deterministic and locale-independent.
fn build_indexer_runtime_truth_upsert_sql(row: &IndexerRuntimeTruthRecord) -> String {
    // Optional oldest-in-flight identifiers: quoted string literal or NULL.
    let opt_text_sql = |value: &Option<String>| match value.as_deref() {
        Some(text) => format!("'{}'", GraphStore::escape_sql(text)),
        None => "NULL".to_string(),
    };
    format!(
        "INSERT INTO axon.indexer_runtime_truth \
         (process_role, heartbeat_ms, graph_workers_active, chunk_embeddings_per_second, \
          in_flight_count, oldest_in_flight_path, oldest_in_flight_stage, \
          oldest_in_flight_age_ms, ready_queue_chunks, persist_queue_depth) \
         VALUES ('{}', {}, {}, {:.6}, {}, {}, {}, {}, {}, {}) \
         ON CONFLICT (process_role) DO UPDATE SET \
            heartbeat_ms = EXCLUDED.heartbeat_ms, \
            graph_workers_active = EXCLUDED.graph_workers_active, \
            chunk_embeddings_per_second = EXCLUDED.chunk_embeddings_per_second, \
            in_flight_count = EXCLUDED.in_flight_count, \
            oldest_in_flight_path = EXCLUDED.oldest_in_flight_path, \
            oldest_in_flight_stage = EXCLUDED.oldest_in_flight_stage, \
            oldest_in_flight_age_ms = EXCLUDED.oldest_in_flight_age_ms, \
            ready_queue_chunks = EXCLUDED.ready_queue_chunks, \
            persist_queue_depth = EXCLUDED.persist_queue_depth",
        GraphStore::escape_sql(&row.process_role),
        row.heartbeat_ms,
        row.graph_workers_active,
        row.chunk_embeddings_per_second,
        row.in_flight_count,
        opt_text_sql(&row.oldest_in_flight_path),
        opt_text_sql(&row.oldest_in_flight_stage),
        row.oldest_in_flight_age_ms,
        row.ready_queue_chunks,
        row.persist_queue_depth,
    )
}

#[cfg(test)]
mod tests {
    // MIL-AXO-015 P4 4e: SQL-shape contract tests for the writer
    // branches that route to `axon.X` under PG. The methods
    // themselves require a live GraphStore; these tests mirror the
    // string composition so the dual-backend invariant is locked in.

    fn pg_fault_sql() -> String {
        "INSERT INTO axon.VectorWorkerFault \
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
        "INSERT INTO axon.VectorPersistOutbox \
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
        "INSERT INTO axon.VectorLaneState \
         (lane, state, reason, updated_at_ms, worker_id, restart_attempt, last_success_at_ms, last_fault_id) \
         VALUES ('vector', 'running', NULL, 0, NULL, 0, NULL, NULL) \
         ON CONFLICT (lane) DO UPDATE SET \
            state = EXCLUDED.state, reason = EXCLUDED.reason, updated_at_ms = EXCLUDED.updated_at_ms, \
            worker_id = EXCLUDED.worker_id, restart_attempt = EXCLUDED.restart_attempt, \
            last_success_at_ms = EXCLUDED.last_success_at_ms, last_fault_id = EXCLUDED.last_fault_id".to_string()
    }

    #[test]
    fn pg_fault_sql_targets_axon_schema() {
        let sql = pg_fault_sql();
        assert!(sql.contains("INSERT INTO axon.VectorWorkerFault"));
        assert!(sql.contains("ON CONFLICT (fault_id) DO UPDATE"));
        // Must update every non-key column on conflict.
        for col in [
            "lane",
            "worker_id",
            "fatal_stage",
            "fatal_reason_raw",
            "fatal_class",
            "provider",
            "batch_id",
            "texts_count",
            "input_bytes",
            "vram_used_mb",
            "occurred_at_ms",
            "restart_attempt",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_lane_sql_targets_axon_schema() {
        let sql = pg_lane_sql();
        assert!(sql.contains("INSERT INTO axon.VectorLaneState"));
        assert!(sql.contains("ON CONFLICT (lane) DO UPDATE"));
        for col in [
            "state",
            "reason",
            "updated_at_ms",
            "worker_id",
            "restart_attempt",
            "last_success_at_ms",
            "last_fault_id",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_outbox_sql_targets_axon_schema() {
        let sql = pg_outbox_sql();
        assert!(sql.contains("INSERT INTO axon.VectorPersistOutbox"));
        assert!(sql.contains("ON CONFLICT (outbox_id) DO UPDATE"));
        for col in [
            "run_id",
            "model_id",
            "status",
            "attempts",
            "queued_at_ms",
            "claimed_at_ms",
            "completed_at_ms",
            "last_error_reason",
            "claim_token",
            "lease_heartbeat_at_ms",
            "lease_owner",
            "lease_epoch",
            "chunk_count",
            "file_count",
            "input_bytes",
            "fetch_ms",
            "embed_ms",
            "payload_json",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
    }

    #[test]
    fn pg_branches_use_explicit_schema_qualifier() {
        // CPT-AXO-039 + axon invariant: every PG-branch SQL must
        // qualify the table with `axon.` so PG can resolve it
        // outside the per-project IST schemas.
        for sql in [pg_fault_sql(), pg_lane_sql(), pg_outbox_sql()] {
            assert!(
                sql.contains("axon."),
                "PG SQL missing axon schema qualifier"
            );
        }
    }

    #[test]
    fn lifecycle_heartbeat_upsert_sql_shape() {
        use crate::embedder::lifecycle_machine::{EmbedderPhase, LifecycleHeartbeatSnapshot};
        use crate::pipeline_v2::stage_health::{StageErrorRecord, StageHealthSnapshot};
        let snapshot = LifecycleHeartbeatSnapshot {
            phase: EmbedderPhase::Sleeping,
            last_used_ms: 1_700_000_000_000,
            wake_count: 3,
            sleep_count: 4,
            pending_count: 12,
            heartbeat_ms: 1_700_000_005_000,
            compute: "GPU".to_string(),
            compute_source: "nvidia_smi".to_string(),
            build_id: Some("v0.8.0-795-gf1cdab19".to_string()),
            // REQ-AXO-902047 slice 1b — populated B3 health so the shape test
            // covers the cross-process error plumbing.
            b3: StageHealthSnapshot {
                consecutive_failures: 9,
                total_failures: 9,
                total_successes: 100,
                last_error: Some(StageErrorRecord {
                    message: "missing chunk number 0 for toast value (XX001)".to_string(),
                    count: 9,
                    first_seen_ms: 1_700_000_001_000,
                    last_seen_ms: 1_700_000_004_000,
                }),
            },
        };
        let sql = super::build_lifecycle_heartbeat_upsert_sql("indexer", &snapshot);
        // Shape contract.
        assert!(sql.contains("INSERT INTO axon.EmbedderLifecycleHeartbeat"));
        assert!(sql.contains("ON CONFLICT (process_role) DO UPDATE"));
        for col in [
            "phase",
            "last_used_ms",
            "wake_count",
            "sleep_count",
            "pending_count",
            "heartbeat_ms",
            "compute",
            "compute_source",
            "build_id",
            "b3_consecutive_failures",
            "b3_total_failures",
            "b3_total_successes",
            "b3_last_error",
            "b3_last_error_count",
            "b3_last_error_last_seen_ms",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
        // Value plumbing (DEC-AXO-901626 observed compute + build_id).
        assert!(sql.contains("'indexer'"));
        assert!(sql.contains("'sleeping'"));
        assert!(sql.contains("1700000000000"));
        assert!(sql.contains("1700000005000"));
        assert!(sql.contains("'GPU'"));
        assert!(sql.contains("'nvidia_smi'"));
        assert!(sql.contains("'v0.8.0-795-gf1cdab19'"));
        // REQ-AXO-902047 slice 1b — the real PG error (root + SQLSTATE) must
        // reach the row verbatim, not the masked "stage merge".
        assert!(sql.contains("'missing chunk number 0 for toast value (XX001)'"));
    }

    #[test]
    fn indexer_runtime_truth_upsert_sql_shape() {
        use crate::graph_ingestion::IndexerRuntimeTruthRecord;
        let row = IndexerRuntimeTruthRecord {
            process_role: "indexer".to_string(),
            heartbeat_ms: 1_700_000_005_000,
            graph_workers_active: 7,
            chunk_embeddings_per_second: 124.5,
            in_flight_count: 3,
            oldest_in_flight_path: Some("/repo/src/big.rs".to_string()),
            oldest_in_flight_stage: Some("A2".to_string()),
            oldest_in_flight_age_ms: 4200,
            ready_queue_chunks: 512,
            persist_queue_depth: 9,
        };
        let sql = super::build_indexer_runtime_truth_upsert_sql(&row);
        assert!(sql.contains("INSERT INTO axon.indexer_runtime_truth"));
        assert!(sql.contains("ON CONFLICT (process_role) DO UPDATE"));
        for col in [
            "heartbeat_ms",
            "graph_workers_active",
            "chunk_embeddings_per_second",
            "in_flight_count",
            "oldest_in_flight_path",
            "oldest_in_flight_stage",
            "oldest_in_flight_age_ms",
            "ready_queue_chunks",
            "persist_queue_depth",
        ] {
            assert!(
                sql.contains(&format!("{col} = EXCLUDED.{col}")),
                "ON CONFLICT update should refresh column `{col}`"
            );
        }
        // No fabricated cumulative "workers_started" column — there is no
        // canonical pipeline_v2 source for it (REQ-AXO-901854 canonical-IO).
        assert!(!sql.contains("graph_workers_started"), "{sql}");
        assert!(sql.contains("'indexer'"));
        assert!(sql.contains("1700000005000"));
        assert!(sql.contains("7"));
        // Float formatted with fixed precision (locale-independent).
        assert!(sql.contains("124.500000"), "{sql}");
        // In-flight gauge + queues plumbed (REQ-AXO-901919 / queue depths).
        assert!(sql.contains("'/repo/src/big.rs'"), "{sql}");
        assert!(sql.contains("'A2'"), "{sql}");
        assert!(sql.contains("4200"), "{sql}");
        assert!(sql.contains("512"), "{sql}");
        // Oldest-in-flight identifiers are NULL when idle, never empty string.
        let idle = IndexerRuntimeTruthRecord {
            oldest_in_flight_path: None,
            oldest_in_flight_stage: None,
            in_flight_count: 0,
            ..row
        };
        let idle_sql = super::build_indexer_runtime_truth_upsert_sql(&idle);
        assert!(idle_sql.contains("NULL"), "{idle_sql}");
    }

    #[test]
    fn lifecycle_heartbeat_upsert_sql_emits_null_build_id_when_absent() {
        use crate::embedder::lifecycle_machine::{EmbedderPhase, LifecycleHeartbeatSnapshot};
        let snapshot = LifecycleHeartbeatSnapshot {
            phase: EmbedderPhase::Ready,
            last_used_ms: 1,
            wake_count: 0,
            sleep_count: 0,
            pending_count: 0,
            heartbeat_ms: 2,
            compute: "CPU".to_string(),
            compute_source: "unknown".to_string(),
            build_id: None,
            b3: Default::default(),
        };
        let sql = super::build_lifecycle_heartbeat_upsert_sql("indexer", &snapshot);
        // No quoted build_id literal; the NULL keyword carries the absence.
        assert!(sql.contains("'CPU'"));
        assert!(sql.contains("'unknown'"));
        // The UPSERT tail now ends with the last B3 column (slice 1b).
        assert!(sql
            .trim_end()
            .ends_with("b3_last_error_last_seen_ms = EXCLUDED.b3_last_error_last_seen_ms"));
        // Both the absent build_id AND the absent B3 last_error render as NULL.
        assert!(sql.contains("NULL"));
        // With no B3 failure yet, the row carries a NULL error then zero counts.
        assert!(sql.contains("NULL, 0, 0)"));
    }

    #[test]
    fn lifecycle_heartbeat_upsert_sql_escapes_role() {
        use crate::embedder::lifecycle_machine::{EmbedderPhase, LifecycleHeartbeatSnapshot};
        let snapshot = LifecycleHeartbeatSnapshot {
            phase: EmbedderPhase::Ready,
            last_used_ms: 0,
            wake_count: 0,
            sleep_count: 0,
            pending_count: 0,
            heartbeat_ms: 0,
            compute: "CPU".to_string(),
            compute_source: "unknown".to_string(),
            build_id: None,
            b3: Default::default(),
        };
        // Pathological role containing single quote must be escaped.
        let sql = super::build_lifecycle_heartbeat_upsert_sql("ind'exer", &snapshot);
        assert!(sql.contains("'ind''exer'"));
    }
}
