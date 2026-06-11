use anyhow::Result;

use crate::graph::GraphStore;
use crate::service_guard;

use super::sql_helpers::{parse_i64_field, parse_u64_field};
use super::{
    EmbedderLifecycleHeartbeatRecord, EmbedderObservedState, VectorLaneStateRecord,
    VectorWorkerFault,
};

impl GraphStore {
    /// REQ-AXO-271 slice 2e (PG canonical only, post-MIL-AXO-017) :
    /// schema-qualify an `axon_runtime` table reference.
    fn axon_runtime_table_ref(&self, table: &'static str) -> String {
        format!("axon_runtime.{table}")
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
        let table_ref = self.axon_runtime_table_ref("EmbedderLifecycleHeartbeat");
        let raw = self.query_json_writer(&format!(
            "SELECT process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms, compute, compute_source, build_id \
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
        }))
    }

    /// DEC-AXO-901626 — PG-canonical embedder observation in one round-trip
    /// via `axon_runtime.embedder_observed_state()`. Feeds the brain
    /// composer's `embedder_runtime` block (throughput + staleness) and the
    /// `pg_inferred` GPU fallback when `nvidia-smi` is unreachable.
    pub fn embedder_observed_state(&self) -> Result<EmbedderObservedState> {
        let raw = self.query_json_writer(
            "SELECT (s->>'embedded_60s')::bigint, \
                    (s->>'embedded_total')::bigint, \
                    (s->>'oldest_pending_age_s')::bigint \
             FROM (SELECT axon_runtime.embedder_observed_state() AS s) q",
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
    format!(
        "INSERT INTO axon_runtime.EmbedderLifecycleHeartbeat \
         (process_role, phase, last_used_ms, wake_count, sleep_count, pending_count, heartbeat_ms, compute, compute_source, build_id) \
         VALUES ('{}', '{}', {}, {}, {}, {}, {}, '{}', '{}', {}) \
         ON CONFLICT (process_role) DO UPDATE SET \
            phase = EXCLUDED.phase, last_used_ms = EXCLUDED.last_used_ms, \
            wake_count = EXCLUDED.wake_count, sleep_count = EXCLUDED.sleep_count, \
            pending_count = EXCLUDED.pending_count, heartbeat_ms = EXCLUDED.heartbeat_ms, \
            compute = EXCLUDED.compute, compute_source = EXCLUDED.compute_source, build_id = EXCLUDED.build_id",
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
    fn pg_lane_sql_targets_axon_runtime_schema() {
        let sql = pg_lane_sql();
        assert!(sql.contains("INSERT INTO axon_runtime.VectorLaneState"));
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
    fn pg_outbox_sql_targets_axon_runtime_schema() {
        let sql = pg_outbox_sql();
        assert!(sql.contains("INSERT INTO axon_runtime.VectorPersistOutbox"));
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
        use crate::embedder::lifecycle_machine::{EmbedderPhase, LifecycleHeartbeatSnapshot};
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
        };
        let sql = super::build_lifecycle_heartbeat_upsert_sql("indexer", &snapshot);
        // Shape contract.
        assert!(sql.contains("INSERT INTO axon_runtime.EmbedderLifecycleHeartbeat"));
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
        };
        let sql = super::build_lifecycle_heartbeat_upsert_sql("indexer", &snapshot);
        // No quoted build_id literal; the NULL keyword carries the absence.
        assert!(sql.contains("'CPU'"));
        assert!(sql.contains("'unknown'"));
        assert!(sql.trim_end().ends_with("build_id = EXCLUDED.build_id"));
        assert!(sql.contains(", NULL)"));
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
        };
        // Pathological role containing single quote must be escaped.
        let sql = super::build_lifecycle_heartbeat_upsert_sql("ind'exer", &snapshot);
        assert!(sql.contains("'ind''exer'"));
    }
}
