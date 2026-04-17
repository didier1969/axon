use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use libloading::{Library, Symbol as LibSymbol};
use tracing::{info, warn};

use crate::embedding_contract::{DIMENSION, GRAPH_MODEL_ID};
use crate::graph::{CloseDbFunc, ExecFunc, GraphStore, InitDbFunc, LatticePool};
use crate::runtime_mode::graph_embeddings_enabled;
use crate::runtime_mode::AxonRuntimeMode;

const IST_SCHEMA_VERSION: &str = "3";
const IST_INGESTION_VERSION: &str = "4";
// Bump to force a one-time rebuild of derived embedding storage after the
// crash-safe table reconstruction path was introduced.
const IST_EMBEDDING_VERSION: &str = "2";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IstCompatibilityAction {
    Noop,
    AdditiveRepair,
    SoftDerivedInvalidation,
    SoftEmbeddingInvalidation,
    HardRebuild,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = Arc::new(unsafe { Library::new(&plugin_path)? });
        let init_fn: LibSymbol<InitDbFunc> = unsafe { lib.get(b"duckdb_init_db\0")? };
        let close_fn: LibSymbol<CloseDbFunc> = unsafe { lib.get(b"duckdb_close_db\0")? };
        let is_memory = db_root == ":memory:";

        if !is_memory {
            let soll_dir = PathBuf::from(db_root);
            std::fs::create_dir_all(&soll_dir)?;

            let mut soll_path = soll_dir.clone();
            soll_path.push("soll.db");
            let soll_c_path = CString::new(soll_path.to_string_lossy().to_string())?;

            unsafe {
                let soll_ptr = init_fn(soll_c_path.as_ptr(), false);
                if soll_ptr.is_null() {
                    return Err(anyhow!("Failed to bootstrap SOLL database"));
                }
                close_fn(soll_ptr);
            }
        }

        let db_path_str = if is_memory {
            ":memory:".to_string()
        } else {
            let mut p = PathBuf::from(db_root);
            std::fs::create_dir_all(&p)?;
            p.push("ist.db");
            p.to_string_lossy().to_string()
        };
        let db_path = if is_memory {
            None
        } else {
            Some(PathBuf::from(&db_path_str))
        };
        let c_path = CString::new(db_path_str)?;

        unsafe {
            let writer_ptr = init_fn(c_path.as_ptr(), false);
            if writer_ptr.is_null() {
                return Err(anyhow!("Failed to init DuckDB Writer"));
            }

            let pool = Arc::new(LatticePool {
                lib: lib.clone(),
                writer_ctx: Mutex::new(writer_ptr),
                reader_ctx: Mutex::new(std::ptr::null_mut()),
            });
            let store = Self {
                pool: pool.clone(),
                db_path,
                recent_write_epoch_ms: AtomicU64::new(0),
                last_reader_refresh_epoch_ms: AtomicU64::new(Self::current_epoch_ms()),
                reader_refresh_failures_total: AtomicU64::new(0),
                reader_state: crate::graph::ReaderSnapshotState::new(Self::current_epoch_ms()),
                reader_refresh_wait: Mutex::new(1),
                reader_refresh_notify: std::sync::Condvar::new(),
            };

            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("soll.db");
                let attach_q = format!(
                    "ATTACH '{}' AS soll;",
                    soll_path.to_string_lossy().replace("'", "''")
                );
                {
                    let w_guard = store
                        .pool
                        .writer_ctx
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    store.setup_session(*w_guard, &attach_q)?;
                }
            } else {
                let _ = store.execute("CREATE SCHEMA IF NOT EXISTS soll;");
            }

            store.init_schema(is_memory)?;
            store.ensure_additive_schema()?;
            store.ensure_additive_soll_schema()?;
            store.ensure_runtime_compatibility()?;
            info!("GraphStore startup: runtime compatibility checks complete.");
            store.recover_interrupted_indexing()?;
            info!("GraphStore startup: interrupted indexing recovery complete.");
            let _ = store.clear_stale_inflight_graph_projection_work();
            info!("GraphStore startup: stale graph projection inflight cleanup complete.");
            let _ = store.clear_stale_inflight_file_vectorization_work();
            info!("GraphStore startup: stale file vectorization inflight cleanup complete.");
            if graph_embeddings_enabled() {
                match store.backfill_graph_projection_queue_for_model(GRAPH_MODEL_ID) {
                    Ok(count) if count > 0 => {
                        info!(
                            "Backfilled {} graph projection queue entries for graph embeddings",
                            count
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            "Unable to backfill graph projection queue at startup: {:?}",
                            err
                        );
                    }
                }
            } else {
                info!(
                    "Skipping graph embedding queue backfill at startup because graph embeddings are disabled."
                );
            }
            info!("GraphStore startup: graph projection backfill complete.");
            if AxonRuntimeMode::from_env().background_vectorization_enabled() {
                match store.backfill_file_vectorization_queue() {
                    Ok(count) if count > 0 => {
                        info!(
                            "Backfilled {} file vectorization queue entries for chunk embeddings",
                            count
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            "Unable to backfill file vectorization queue at startup: {:?}",
                            err
                        );
                    }
                }
            } else {
                info!(
                    "Skipping file vectorization queue backfill at startup because runtime mode is {}.",
                    AxonRuntimeMode::from_env().as_str()
                );
            }
            info!("GraphStore startup: file vectorization backfill complete.");
            store.execute("CHECKPOINT;")?;
            info!("GraphStore startup: writer checkpoint complete.");

            let _reader_ptr = if is_memory {
                writer_ptr
            } else {
                let ptr = init_fn(c_path.as_ptr(), true);
                if ptr.is_null() {
                    return Err(anyhow!("Failed to init DuckDB Reader"));
                }
                ptr
            };
            info!("GraphStore startup: reader init complete.");

            #[cfg(test)]
            {
                let mut reader_guard = store
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                *reader_guard = writer_ptr;
            }
            #[cfg(not(test))]
            {
                let mut reader_guard = store
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                *reader_guard = _reader_ptr;
            }

            if !is_memory && !cfg!(test) {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("soll.db");
                let attach_q = format!(
                    "ATTACH '{}' AS soll;",
                    soll_path.to_string_lossy().replace("'", "''")
                );
                let r_guard = store
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                store.setup_session(*r_guard, &attach_q)?;
            }
            info!("GraphStore startup: reader session setup complete.");
            store.sync_reader_epoch_to_commit();

            Ok(store)
        }
    }

    pub fn refresh_reader_snapshot(&self) -> Result<()> {
        if cfg!(test) {
            self.sync_reader_epoch_to_commit();
            self.reader_state
                .refresh_inflight
                .store(false, Ordering::Release);
            return Ok(());
        }

        let Some(db_path) = self.db_path.as_ref() else {
            self.sync_reader_epoch_to_commit();
            self.reader_state
                .refresh_inflight
                .store(false, Ordering::Release);
            return Ok(());
        };

        let requested_epoch = self
            .reader_state
            .refresh_requested_epoch
            .load(Ordering::Acquire);
        let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Acquire);
        let target_epoch = requested_epoch.max(commit_epoch);

        let c_path = CString::new(db_path.to_string_lossy().to_string())?;
        let refresh_result = unsafe {
            let init_fn: LibSymbol<InitDbFunc> = self.pool.lib.get(b"duckdb_init_db\0")?;
            let close_fn: LibSymbol<CloseDbFunc> = self.pool.lib.get(b"duckdb_close_db\0")?;

            let new_reader = init_fn(c_path.as_ptr(), true);
            if new_reader.is_null() {
                Err(anyhow!("Failed to init refreshed DuckDB Reader"))
            } else {
                let mut soll_path = db_path
                    .parent()
                    .ok_or_else(|| anyhow!("DB parent path unavailable for reader refresh"))?
                    .to_path_buf();
                soll_path.push("soll.db");
                let attach_q = format!(
                    "ATTACH '{}' AS soll;",
                    soll_path.to_string_lossy().replace('\'', "''")
                );
                self.setup_session(new_reader, &attach_q)?;

                let writer_ctx = *self
                    .pool
                    .writer_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                let old_reader = {
                    let mut reader_guard = self
                        .pool
                        .reader_ctx
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    let previous = *reader_guard;
                    *reader_guard = new_reader;
                    previous
                };

                if !old_reader.is_null() && old_reader != writer_ctx {
                    close_fn(old_reader);
                }
                Ok(())
            }
        };

        let now_ms = Self::current_epoch_ms();
        match refresh_result {
            Ok(()) => {
                let commit_epoch_after = self.reader_state.commit_epoch.load(Ordering::Acquire);
                let requested_epoch_after = self
                    .reader_state
                    .refresh_requested_epoch
                    .load(Ordering::Acquire);
                let desired_epoch = commit_epoch_after
                    .max(requested_epoch_after)
                    .max(target_epoch);
                self.last_reader_refresh_epoch_ms
                    .store(now_ms, Ordering::Relaxed);
                self.reader_state
                    .reader_epoch
                    .store(desired_epoch, Ordering::Release);
                self.reader_state
                    .last_refresh_completed_ms
                    .store(now_ms, Ordering::Relaxed);
                let _ = self.bump_refresh_requested_epoch(desired_epoch);
                self.reader_state
                    .refresh_inflight
                    .store(false, Ordering::Release);
                Ok(())
            }
            Err(err) => {
                self.reader_refresh_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                self.reader_state
                    .refresh_inflight
                    .store(false, Ordering::Release);
                Err(err)
            }
        }
    }

    pub fn reader_snapshot_age_ms(&self) -> u64 {
        let ts = self.last_reader_refresh_epoch_ms.load(Ordering::Relaxed);
        if ts == 0 {
            return u64::MAX;
        }
        Self::current_epoch_ms().saturating_sub(ts)
    }

    pub fn reader_refresh_failures_total(&self) -> u64 {
        self.reader_refresh_failures_total.load(Ordering::Relaxed)
    }

    pub(crate) fn mark_writer_commit_visible(&self) {
        let now_ms = Self::current_epoch_ms();
        self.recent_write_epoch_ms.store(now_ms, Ordering::Relaxed);
        self.reader_state
            .commit_epoch
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn request_reader_refresh_up_to(&self, target_epoch: u64) {
        let target_epoch = self.bump_refresh_requested_epoch(target_epoch);

        if self
            .reader_state
            .refresh_inflight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.reader_state
                .last_refresh_started_ms
                .store(Self::current_epoch_ms(), Ordering::Relaxed);
            let mut wake_target = self
                .reader_refresh_wait
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            *wake_target = (*wake_target).max(target_epoch);
            self.reader_refresh_notify.notify_one();
        } else {
            self.reader_state
                .refresh_coalesced_total
                .fetch_add(1, Ordering::Relaxed);
            let mut wake_target = self
                .reader_refresh_wait
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            *wake_target = (*wake_target).max(target_epoch);
            self.reader_refresh_notify.notify_one();
        }
    }

    fn bump_refresh_requested_epoch(&self, target_epoch: u64) -> u64 {
        let target_epoch = target_epoch.max(self.reader_state.commit_epoch.load(Ordering::Acquire));
        let mut requested = self
            .reader_state
            .refresh_requested_epoch
            .load(Ordering::Acquire);
        while target_epoch > requested {
            match self.reader_state.refresh_requested_epoch.compare_exchange(
                requested,
                target_epoch,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return target_epoch,
                Err(actual) => requested = actual,
            }
        }
        requested
    }

    pub fn refresh_reader_snapshot_if_needed(&self) -> Result<bool> {
        let mut refreshed = false;
        for _ in 0..4 {
            let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Acquire);
            let reader_epoch = self.reader_state.reader_epoch.load(Ordering::Acquire);
            let requested_epoch = self
                .reader_state
                .refresh_requested_epoch
                .load(Ordering::Acquire);
            let target_epoch = commit_epoch.max(requested_epoch);
            if target_epoch > reader_epoch {
                self.request_reader_refresh_up_to(target_epoch);
            }

            if !self.reader_state.refresh_inflight.load(Ordering::Acquire) {
                return Ok(refreshed);
            }

            self.refresh_reader_snapshot()?;
            refreshed = true;
        }

        Ok(refreshed)
    }

    pub fn wait_for_reader_refresh_signal(&self, timeout: std::time::Duration) {
        let guard = self
            .reader_refresh_wait
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _ = self.reader_refresh_notify.wait_timeout(guard, timeout);
    }

    pub(crate) fn reader_snapshot_diagnostics(&self) -> crate::graph::ReaderSnapshotDiagnostics {
        let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Relaxed);
        let reader_epoch = self.reader_state.reader_epoch.load(Ordering::Relaxed);
        crate::graph::ReaderSnapshotDiagnostics {
            commit_epoch,
            reader_epoch,
            reader_epoch_lag: commit_epoch.saturating_sub(reader_epoch),
            refresh_inflight: self.reader_state.refresh_inflight.load(Ordering::Relaxed),
            refresh_requested_epoch: self
                .reader_state
                .refresh_requested_epoch
                .load(Ordering::Relaxed),
            last_refresh_started_ms: self
                .reader_state
                .last_refresh_started_ms
                .load(Ordering::Relaxed),
            last_refresh_completed_ms: self
                .reader_state
                .last_refresh_completed_ms
                .load(Ordering::Relaxed),
            refresh_coalesced_total: self
                .reader_state
                .refresh_coalesced_total
                .load(Ordering::Relaxed),
            reads_on_reader_total: self
                .reader_state
                .reads_on_reader_total
                .load(Ordering::Relaxed),
            reads_on_writer_total: self
                .reader_state
                .reads_on_writer_total
                .load(Ordering::Relaxed),
            fresh_required_fallback_writer_total: self
                .reader_state
                .fresh_required_fallback_writer_total
                .load(Ordering::Relaxed),
            reader_refresh_failures_total: self.reader_refresh_failures_total(),
        }
    }

    pub(crate) fn sync_reader_epoch_to_commit(&self) {
        let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Acquire);
        let now_ms = Self::current_epoch_ms();
        self.reader_state
            .reader_epoch
            .store(commit_epoch, Ordering::Release);
        self.reader_state
            .refresh_requested_epoch
            .store(commit_epoch, Ordering::Release);
        self.reader_state
            .last_refresh_completed_ms
            .store(now_ms, Ordering::Relaxed);
        self.last_reader_refresh_epoch_ms
            .store(now_ms, Ordering::Relaxed);
    }

    fn setup_session(&self, ctx: *mut c_void, attach_query: &str) -> Result<()> {
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            exec_fn(ctx, CString::new("INSTALL json; LOAD json;")?.as_ptr());
            exec_fn(
                ctx,
                CString::new("SET checkpoint_threshold = '1GB';")?.as_ptr(),
            );
            if !attach_query.is_empty() {
                exec_fn(ctx, CString::new(attach_query)?.as_ptr());
            }
            Ok(())
        }
    }

    fn find_plugin_path() -> Result<String> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| anyhow!("Unable to resolve repository root from CARGO_MANIFEST_DIR"))?
            .to_path_buf();

        let mut candidates = vec![
            repo_root.join("src/axon-plugin-duckdb/target/release/libaxon_plugin_duckdb.so"),
            repo_root.join("src/axon-plugin-duckdb/target/debug/libaxon_plugin_duckdb.so"),
        ];

        if let Ok(cwd) = std::env::current_dir() {
            candidates
                .push(cwd.join("src/axon-plugin-duckdb/target/release/libaxon_plugin_duckdb.so"));
            candidates
                .push(cwd.join("src/axon-plugin-duckdb/target/debug/libaxon_plugin_duckdb.so"));
        }

        for path in candidates {
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
        Err(anyhow!("Plugin not found"))
    }

    fn init_schema(&self, _is_memory: bool) -> Result<()> {
        self.execute(
            "CREATE TABLE IF NOT EXISTS RuntimeMetadata (key VARCHAR PRIMARY KEY, value VARCHAR)",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_code VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE, first_seen_at_ms BIGINT, indexing_started_at_ms BIGINT, graph_ready_at_ms BIGINT, vectorization_started_at_ms BIGINT, vector_ready_at_ms BIGINT, last_state_change_at_ms BIGINT, last_error_at_ms BIGINT)")?;
        self.execute(&format!("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, project_code VARCHAR, embedding FLOAT[{DIMENSION}])"))?;
        self.execute("CREATE TABLE IF NOT EXISTS Chunk (id VARCHAR PRIMARY KEY, source_type VARCHAR, source_id VARCHAR, project_code VARCHAR, file_path VARCHAR, kind VARCHAR, content VARCHAR, content_hash VARCHAR, start_line BIGINT, end_line BIGINT)")?;
        self.ensure_embedding_runtime_tables()?;
        self.ensure_graph_projection_runtime_tables()?;
        self.execute("CREATE TABLE IF NOT EXISTS Project (name VARCHAR PRIMARY KEY)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CONTAINS (source_id VARCHAR, target_id VARCHAR, project_code VARCHAR, PRIMARY KEY (source_id, target_id, project_code))")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS (source_id VARCHAR, target_id VARCHAR, project_code VARCHAR, PRIMARY KEY (source_id, target_id, project_code))")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS CALLS_NIF (source_id VARCHAR, target_id VARCHAR, project_code VARCHAR, PRIMARY KEY (source_id, target_id, project_code))",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS IMPACTS (source_id VARCHAR, target_id VARCHAR, project_code VARCHAR, PRIMARY KEY (source_id, target_id, project_code))")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS SUBSTANTIATES (source_id VARCHAR, target_id VARCHAR, project_code VARCHAR, PRIMARY KEY (source_id, target_id, project_code))",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (project_code VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL', id VARCHAR DEFAULT 'AXON_GLOBAL', last_vis BIGINT DEFAULT 0, last_pil BIGINT DEFAULT 0, last_req BIGINT DEFAULT 0, last_cpt BIGINT DEFAULT 0, last_dec BIGINT DEFAULT 0, last_mil BIGINT DEFAULT 0, last_val BIGINT DEFAULT 0, last_stk BIGINT DEFAULT 0, last_gui BIGINT DEFAULT 0, last_prv BIGINT DEFAULT 0, last_rev BIGINT DEFAULT 0)")?;
        let _ = self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_gui BIGINT DEFAULT 0",
        );
        self.execute("CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (project_code VARCHAR PRIMARY KEY, project_name VARCHAR, project_path VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Node (id VARCHAR PRIMARY KEY, type VARCHAR, project_code VARCHAR, title VARCHAR, description VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Revision (revision_id VARCHAR PRIMARY KEY, author VARCHAR, source VARCHAR, summary VARCHAR, status VARCHAR, created_at BIGINT, committed_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionChange (revision_id VARCHAR, entity_type VARCHAR, entity_id VARCHAR, action VARCHAR, before_json VARCHAR, after_json VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionPreview (preview_id VARCHAR PRIMARY KEY, author VARCHAR, project_code VARCHAR, payload VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Traceability (id VARCHAR PRIMARY KEY, soll_entity_type VARCHAR, soll_entity_id VARCHAR, artifact_type VARCHAR, artifact_ref VARCHAR, confidence DOUBLE, metadata VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS FileLifecycleEvent (file_path VARCHAR, project_code VARCHAR, stage VARCHAR, status VARCHAR, reason VARCHAR, at_ms BIGINT, worker_id BIGINT, trace_id VARCHAR, run_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorBatchRun (run_id VARCHAR PRIMARY KEY, started_at_ms BIGINT, finished_at_ms BIGINT, provider VARCHAR, model_id VARCHAR, chunk_count BIGINT, file_count BIGINT, input_bytes BIGINT, fetch_ms BIGINT, embed_ms BIGINT, db_write_ms BIGINT, mark_done_ms BIGINT, success BOOLEAN, error_reason VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorWorkerFault (fault_id VARCHAR PRIMARY KEY, lane VARCHAR, worker_id BIGINT, fatal_stage VARCHAR, fatal_reason_raw VARCHAR, fatal_class VARCHAR, provider VARCHAR, batch_id VARCHAR, texts_count BIGINT DEFAULT 0, input_bytes BIGINT DEFAULT 0, vram_used_mb BIGINT DEFAULT 0, occurred_at_ms BIGINT, restart_attempt BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorLaneState (lane VARCHAR PRIMARY KEY, state VARCHAR, reason VARCHAR, updated_at_ms BIGINT, worker_id BIGINT, restart_attempt BIGINT DEFAULT 0, last_success_at_ms BIGINT, last_fault_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorPersistOutbox (outbox_id VARCHAR PRIMARY KEY, run_id VARCHAR, model_id VARCHAR, status VARCHAR DEFAULT 'queued', attempts BIGINT DEFAULT 0, queued_at_ms BIGINT, claimed_at_ms BIGINT, completed_at_ms BIGINT, last_error_reason VARCHAR, claim_token VARCHAR, lease_heartbeat_at_ms BIGINT, lease_owner VARCHAR, lease_epoch BIGINT DEFAULT 0, chunk_count BIGINT DEFAULT 0, file_count BIGINT DEFAULT 0, input_bytes BIGINT DEFAULT 0, fetch_ms BIGINT DEFAULT 0, embed_ms BIGINT DEFAULT 0, payload_json VARCHAR)")?;
        self.execute("CREATE INDEX IF NOT EXISTS vector_persist_outbox_status_idx ON VectorPersistOutbox(status, queued_at_ms)")?;
        self.execute("CREATE TABLE IF NOT EXISTS HourlyVectorizationRollup (bucket_start_ms BIGINT, project_code VARCHAR, model_id VARCHAR, chunks_embedded BIGINT DEFAULT 0, files_vector_ready BIGINT DEFAULT 0, batches BIGINT DEFAULT 0, fetch_ms_total BIGINT DEFAULT 0, embed_ms_total BIGINT DEFAULT 0, db_write_ms_total BIGINT DEFAULT 0, mark_done_ms_total BIGINT DEFAULT 0, PRIMARY KEY (bucket_start_ms, project_code, model_id))")?;
        self.execute("CREATE TABLE IF NOT EXISTS OptimizerDecisionLog (decision_id VARCHAR PRIMARY KEY, at_ms BIGINT, mode VARCHAR, host_snapshot_json VARCHAR, policy_snapshot_json VARCHAR, signal_snapshot_json VARCHAR, analytics_snapshot_json VARCHAR, action_profile_id VARCHAR, decision_json VARCHAR, constraints_triggered_json VARCHAR, would_apply BOOLEAN, applied BOOLEAN, evaluation_window_start_ms BIGINT, evaluation_window_end_ms BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS RewardObservationLog (decision_id VARCHAR, observed_at_ms BIGINT, window_start_ms BIGINT, window_end_ms BIGINT, reward_json VARCHAR, throughput_chunks_per_hour DOUBLE, throughput_files_per_hour DOUBLE, constraint_violations_json VARCHAR, pressure_summary_json VARCHAR)")?;
        Ok(())
    }

    fn ensure_embedding_runtime_tables(&self) -> Result<()> {
        self.execute("CREATE TABLE IF NOT EXISTS EmbeddingModel (id VARCHAR PRIMARY KEY, kind VARCHAR, model_name VARCHAR, dimension BIGINT, version VARCHAR, created_at BIGINT)")?;
        self.execute(&format!("CREATE TABLE IF NOT EXISTS ChunkEmbedding (chunk_id VARCHAR, model_id VARCHAR, embedding FLOAT[{DIMENSION}], source_hash VARCHAR, embedded_at_ms BIGINT, PRIMARY KEY (chunk_id, model_id))"))?;
        self.execute("CREATE TABLE IF NOT EXISTS FileVectorizationQueue (file_path VARCHAR PRIMARY KEY, status VARCHAR DEFAULT 'queued', status_reason VARCHAR, attempts BIGINT DEFAULT 0, queued_at BIGINT, last_error_reason VARCHAR, last_attempt_at BIGINT, next_eligible_at_ms BIGINT, interactive_pause_count BIGINT DEFAULT 0, claim_token VARCHAR, claimed_at_ms BIGINT, lease_heartbeat_at_ms BIGINT, lease_owner VARCHAR, lease_epoch BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorWorkerFault (fault_id VARCHAR PRIMARY KEY, lane VARCHAR, worker_id BIGINT, fatal_stage VARCHAR, fatal_reason_raw VARCHAR, fatal_class VARCHAR, provider VARCHAR, batch_id VARCHAR, texts_count BIGINT DEFAULT 0, input_bytes BIGINT DEFAULT 0, vram_used_mb BIGINT DEFAULT 0, occurred_at_ms BIGINT, restart_attempt BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorLaneState (lane VARCHAR PRIMARY KEY, state VARCHAR, reason VARCHAR, updated_at_ms BIGINT, worker_id BIGINT, restart_attempt BIGINT DEFAULT 0, last_success_at_ms BIGINT, last_fault_id VARCHAR)")?;
        self.execute(&format!("CREATE TABLE IF NOT EXISTS GraphEmbedding (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, model_id VARCHAR, source_signature VARCHAR, projection_version VARCHAR, embedding FLOAT[{DIMENSION}], updated_at BIGINT)"))?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_embedding_anchor_model_idx ON GraphEmbedding(anchor_type, anchor_id, radius, model_id)")?;
        Ok(())
    }

    fn ensure_graph_projection_runtime_tables(&self) -> Result<()> {
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjection (anchor_type VARCHAR, anchor_id VARCHAR, target_type VARCHAR, target_id VARCHAR, edge_kind VARCHAR, distance BIGINT, radius BIGINT, projection_version VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjectionState (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, source_signature VARCHAR, projection_version VARCHAR, updated_at BIGINT)")?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_projection_state_anchor_idx ON GraphProjectionState(anchor_type, anchor_id, radius)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjectionQueue (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, status VARCHAR DEFAULT 'queued', attempts BIGINT DEFAULT 0, queued_at BIGINT, last_error_reason VARCHAR, last_attempt_at BIGINT)")?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_projection_queue_anchor_idx ON GraphProjectionQueue(anchor_type, anchor_id, radius)")?;
        Ok(())
    }

    fn rebuild_embedding_runtime_tables(&self) -> Result<()> {
        // Rebuild instead of row-level DELETE to tolerate corrupted vector pages.
        self.execute("DROP TABLE IF EXISTS GraphEmbedding")?;
        self.execute("DROP TABLE IF EXISTS ChunkEmbedding")?;
        self.execute("DROP TABLE IF EXISTS EmbeddingModel")?;
        self.execute("DROP TABLE IF EXISTS FileVectorizationQueue")?;
        self.execute("DROP TABLE IF EXISTS VectorWorkerFault")?;
        self.execute("DROP TABLE IF EXISTS VectorLaneState")?;
        self.ensure_embedding_runtime_tables()
    }

    fn rebuild_graph_projection_runtime_tables(&self) -> Result<()> {
        self.execute("DROP TABLE IF EXISTS GraphProjectionState")?;
        self.execute("DROP TABLE IF EXISTS GraphProjection")?;
        self.execute("DROP TABLE IF EXISTS GraphProjectionQueue")?;
        self.ensure_graph_projection_runtime_tables()
    }

    fn ensure_additive_schema(&self) -> Result<()> {
        // Drop indexes on File table to allow ALTER TABLE ... ADD COLUMN with DEFAULT values
        let _ = self.execute("DROP INDEX IF EXISTS file_project_code_idx");
        let _ = self.execute("DROP INDEX IF EXISTS file_status_idx");

        self.execute(
            "ALTER TABLE File ADD COLUMN IF NOT EXISTS needs_reindex BOOLEAN DEFAULT FALSE",
        )?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS last_error_reason VARCHAR")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS status_reason VARCHAR")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS defer_count BIGINT DEFAULT 0")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS last_deferred_at_ms BIGINT")?;
        self.execute(
            "ALTER TABLE File ADD COLUMN IF NOT EXISTS file_stage VARCHAR DEFAULT 'promoted'",
        )?;
        self.execute(
            "ALTER TABLE File ADD COLUMN IF NOT EXISTS graph_ready BOOLEAN DEFAULT FALSE",
        )?;
        self.execute(
            "ALTER TABLE File ADD COLUMN IF NOT EXISTS vector_ready BOOLEAN DEFAULT FALSE",
        )?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS first_seen_at_ms BIGINT")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS indexing_started_at_ms BIGINT")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS graph_ready_at_ms BIGINT")?;
        self.execute(
            "ALTER TABLE File ADD COLUMN IF NOT EXISTS vectorization_started_at_ms BIGINT",
        )?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS vector_ready_at_ms BIGINT")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS last_state_change_at_ms BIGINT")?;
        self.execute("ALTER TABLE File ADD COLUMN IF NOT EXISTS last_error_at_ms BIGINT")?;
        self.execute("ALTER TABLE ChunkEmbedding ADD COLUMN IF NOT EXISTS embedded_at_ms BIGINT")?;
        self.execute("CREATE TABLE IF NOT EXISTS FileLifecycleEvent (file_path VARCHAR, project_code VARCHAR, stage VARCHAR, status VARCHAR, reason VARCHAR, at_ms BIGINT, worker_id BIGINT, trace_id VARCHAR, run_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorBatchRun (run_id VARCHAR PRIMARY KEY, started_at_ms BIGINT, finished_at_ms BIGINT, provider VARCHAR, model_id VARCHAR, chunk_count BIGINT, file_count BIGINT, input_bytes BIGINT, fetch_ms BIGINT, embed_ms BIGINT, db_write_ms BIGINT, mark_done_ms BIGINT, success BOOLEAN, error_reason VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorWorkerFault (fault_id VARCHAR PRIMARY KEY, lane VARCHAR, worker_id BIGINT, fatal_stage VARCHAR, fatal_reason_raw VARCHAR, fatal_class VARCHAR, provider VARCHAR, batch_id VARCHAR, texts_count BIGINT DEFAULT 0, input_bytes BIGINT DEFAULT 0, vram_used_mb BIGINT DEFAULT 0, occurred_at_ms BIGINT, restart_attempt BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorLaneState (lane VARCHAR PRIMARY KEY, state VARCHAR, reason VARCHAR, updated_at_ms BIGINT, worker_id BIGINT, restart_attempt BIGINT DEFAULT 0, last_success_at_ms BIGINT, last_fault_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS VectorPersistOutbox (outbox_id VARCHAR PRIMARY KEY, run_id VARCHAR, model_id VARCHAR, status VARCHAR DEFAULT 'queued', attempts BIGINT DEFAULT 0, queued_at_ms BIGINT, claimed_at_ms BIGINT, completed_at_ms BIGINT, last_error_reason VARCHAR, claim_token VARCHAR, lease_heartbeat_at_ms BIGINT, lease_owner VARCHAR, lease_epoch BIGINT DEFAULT 0, chunk_count BIGINT DEFAULT 0, file_count BIGINT DEFAULT 0, input_bytes BIGINT DEFAULT 0, fetch_ms BIGINT DEFAULT 0, embed_ms BIGINT DEFAULT 0, payload_json VARCHAR)")?;
        self.execute("CREATE INDEX IF NOT EXISTS vector_persist_outbox_status_idx ON VectorPersistOutbox(status, queued_at_ms)")?;
        self.execute("CREATE TABLE IF NOT EXISTS HourlyVectorizationRollup (bucket_start_ms BIGINT, project_code VARCHAR, model_id VARCHAR, chunks_embedded BIGINT DEFAULT 0, files_vector_ready BIGINT DEFAULT 0, batches BIGINT DEFAULT 0, fetch_ms_total BIGINT DEFAULT 0, embed_ms_total BIGINT DEFAULT 0, db_write_ms_total BIGINT DEFAULT 0, mark_done_ms_total BIGINT DEFAULT 0, PRIMARY KEY (bucket_start_ms, project_code, model_id))")?;
        self.execute("CREATE TABLE IF NOT EXISTS OptimizerDecisionLog (decision_id VARCHAR PRIMARY KEY, at_ms BIGINT, mode VARCHAR, host_snapshot_json VARCHAR, policy_snapshot_json VARCHAR, signal_snapshot_json VARCHAR, analytics_snapshot_json VARCHAR, action_profile_id VARCHAR, decision_json VARCHAR, constraints_triggered_json VARCHAR, would_apply BOOLEAN, applied BOOLEAN, evaluation_window_start_ms BIGINT, evaluation_window_end_ms BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS RewardObservationLog (decision_id VARCHAR, observed_at_ms BIGINT, window_start_ms BIGINT, window_end_ms BIGINT, reward_json VARCHAR, throughput_chunks_per_hour DOUBLE, throughput_files_per_hour DOUBLE, constraint_violations_json VARCHAR, pressure_summary_json VARCHAR)")?;
        self.execute(
            "ALTER TABLE FileVectorizationQueue ADD COLUMN IF NOT EXISTS lease_heartbeat_at_ms BIGINT",
        )?;
        self.execute(
            "ALTER TABLE FileVectorizationQueue ADD COLUMN IF NOT EXISTS lease_owner VARCHAR",
        )?;
        self.execute(
            "ALTER TABLE FileVectorizationQueue ADD COLUMN IF NOT EXISTS lease_epoch BIGINT DEFAULT 0",
        )?;

        let columns = self.list_file_table_columns()?;
        let has_status = columns.contains("status");
        let has_file_stage = columns.contains("file_stage");
        let has_graph_ready = columns.contains("graph_ready");
        let has_vector_ready = columns.contains("vector_ready");

        if has_file_stage {
            if has_status {
                self.execute(
                    "UPDATE File \
                     SET file_stage = CASE \
                            WHEN status = 'indexing' THEN 'claimed' \
                            WHEN status IN ('indexed', 'indexed_degraded') THEN 'graph_indexed' \
                            WHEN status = 'skipped' THEN 'skipped' \
                            WHEN status = 'deleted' THEN 'deleted' \
                            ELSE 'promoted' \
                         END \
                         WHERE file_stage IS NULL",
                )?;
            } else {
                self.execute(
                    "UPDATE File \
                     SET file_stage = 'promoted' \
                     WHERE file_stage IS NULL",
                )?;
            }
        }

        if has_graph_ready {
            if has_status {
                self.execute(
                    "UPDATE File \
                     SET graph_ready = CASE WHEN status IN ('indexed', 'indexed_degraded') THEN TRUE ELSE COALESCE(graph_ready, FALSE) END \
                     WHERE graph_ready IS NULL OR status IN ('indexed', 'indexed_degraded')",
                )?;
            } else {
                self.execute(
                    "UPDATE File \
                     SET graph_ready = COALESCE(graph_ready, FALSE) \
                     WHERE graph_ready IS NULL",
                )?;
            }
        }

        if has_vector_ready {
            self.execute("UPDATE File SET vector_ready = COALESCE(vector_ready, FALSE) WHERE vector_ready IS NULL")?;
        }
        self.execute("UPDATE File SET first_seen_at_ms = COALESCE(first_seen_at_ms, last_deferred_at_ms, mtime) WHERE first_seen_at_ms IS NULL")?;
        self.execute("UPDATE File SET graph_ready_at_ms = COALESCE(graph_ready_at_ms, mtime) WHERE graph_ready = TRUE AND graph_ready_at_ms IS NULL")?;
        self.execute("UPDATE File SET vector_ready_at_ms = COALESCE(vector_ready_at_ms, graph_ready_at_ms, mtime) WHERE vector_ready = TRUE AND vector_ready_at_ms IS NULL")?;
        self.execute("UPDATE File SET last_state_change_at_ms = COALESCE(last_state_change_at_ms, vector_ready_at_ms, graph_ready_at_ms, first_seen_at_ms, mtime) WHERE last_state_change_at_ms IS NULL")?;
        self.execute("UPDATE File SET last_error_at_ms = COALESCE(last_error_at_ms, last_deferred_at_ms) WHERE last_error_reason IS NOT NULL AND last_error_at_ms IS NULL")?;
        let embedded_now_ms = chrono::Utc::now().timestamp_millis();
        self.execute(&format!(
            "UPDATE ChunkEmbedding SET embedded_at_ms = COALESCE(embedded_at_ms, {}) WHERE embedded_at_ms IS NULL",
            embedded_now_ms
        ))?;

        self.execute("ALTER TABLE Chunk ADD COLUMN IF NOT EXISTS file_path VARCHAR")?;
        self.execute(
            "UPDATE Chunk \
             SET file_path = CASE \
                 WHEN source_type = 'file' THEN source_id \
                 ELSE COALESCE((SELECT co.source_id FROM CONTAINS co WHERE co.target_id = Chunk.source_id LIMIT 1), file_path) \
             END \
             WHERE file_path IS NULL OR file_path = ''",
        )?;

        self.execute("ALTER TABLE CALLS ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE CALLS_NIF ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE CONTAINS ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE IMPACTS ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE SUBSTANTIATES ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;

        // Performance Indexes for Advanced Graph Heuristics
        self.execute("CREATE INDEX IF NOT EXISTS calls_source_idx ON CALLS(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_target_idx ON CALLS(target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_project_code_idx ON CALLS(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_nif_source_idx ON CALLS_NIF(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_nif_target_idx ON CALLS_NIF(target_id)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS calls_nif_project_code_idx ON CALLS_NIF(project_code)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS contains_source_idx ON CONTAINS(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS contains_target_idx ON CONTAINS(target_id)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS contains_project_code_idx ON CONTAINS(project_code)",
        )?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS impacts_project_code_idx ON IMPACTS(project_code)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS substantiates_project_code_idx ON SUBSTANTIATES(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS symbol_project_code_idx ON Symbol(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS file_project_code_idx ON File(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_project_code_idx ON Chunk(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_file_path_idx ON Chunk(file_path)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_project_file_path_idx ON Chunk(project_code, file_path)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_source_id_idx ON Chunk(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_content_hash_idx ON Chunk(content_hash)")?;
        self.execute("CREATE INDEX IF NOT EXISTS chunk_embedding_chunk_model_hash_idx ON ChunkEmbedding(chunk_id, model_id, source_hash)")?;
        self.execute("CREATE INDEX IF NOT EXISTS file_vectorization_queue_status_eligible_idx ON FileVectorizationQueue(status, next_eligible_at_ms)")?;
        self.execute("CREATE INDEX IF NOT EXISTS symbol_kind_idx ON Symbol(kind)")?;
        self.execute("CREATE INDEX IF NOT EXISTS symbol_is_public_idx ON Symbol(is_public)")?;
        if has_status {
            self.execute("CREATE INDEX IF NOT EXISTS file_status_idx ON File(status)")?;
        }

        Ok(())
    }

    fn ensure_additive_soll_schema(&self) -> Result<()> {
        let _ = self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_gui BIGINT DEFAULT 0",
        );
        self.execute("CREATE TABLE IF NOT EXISTS soll.ProjectCodeRegistry (project_code VARCHAR PRIMARY KEY, project_name VARCHAR, project_path VARCHAR)")?;
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS project_name VARCHAR",
        )?;
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS project_path VARCHAR",
        )?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS soll_project_code_registry_code_idx ON soll.ProjectCodeRegistry(project_code)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Node (id VARCHAR PRIMARY KEY, type VARCHAR, project_code VARCHAR, title VARCHAR, description VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Edge (source_id VARCHAR, target_id VARCHAR, relation_type VARCHAR, metadata VARCHAR, PRIMARY KEY (source_id, target_id, relation_type))")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.McpJob (job_id VARCHAR PRIMARY KEY, tool_name VARCHAR, status VARCHAR, submitted_at BIGINT, started_at BIGINT, finished_at BIGINT, request_json VARCHAR, reserved_ids_json VARCHAR, result_json VARCHAR, error_text VARCHAR)")?;

        // Performance Indexes
        self.execute("CREATE INDEX IF NOT EXISTS soll_node_type_idx ON soll.Node(type)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_node_project_code_idx ON soll.Node(project_code)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_source_idx ON soll.Edge(source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_target_idx ON soll.Edge(target_id)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_edge_relation_idx ON soll.Edge(relation_type)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_mcp_job_status_idx ON soll.McpJob(status)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_mcp_job_submitted_idx ON soll.McpJob(submitted_at)",
        )?;

        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_pil BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_vis BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_stk BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_prv BIGINT DEFAULT 0",
        )?;
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_rev BIGINT DEFAULT 0",
        )?;

        self.execute("CREATE TABLE IF NOT EXISTS soll.Revision (revision_id VARCHAR PRIMARY KEY, author VARCHAR, source VARCHAR, summary VARCHAR, status VARCHAR, created_at BIGINT, committed_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionChange (revision_id VARCHAR, entity_type VARCHAR, entity_id VARCHAR, action VARCHAR, before_json VARCHAR, after_json VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.RevisionPreview (preview_id VARCHAR PRIMARY KEY, author VARCHAR, project_code VARCHAR, payload VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Traceability (id VARCHAR PRIMARY KEY, soll_entity_type VARCHAR, soll_entity_id VARCHAR, artifact_type VARCHAR, artifact_ref VARCHAR, confidence DOUBLE, metadata VARCHAR, created_at BIGINT)")?;
        self.normalize_project_code_registry()?;
        self.seed_project_code_registry()?;
        self.seed_global_guidelines()?;
        Ok(())
    }

    fn normalize_project_code_registry(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(project_code,''), COALESCE(project_name,''), COALESCE(project_path,'')
             FROM soll.ProjectCodeRegistry",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_project_code = row[0].trim().to_ascii_uppercase();
            let existing_project_name = row[1].trim().to_string();
            let project_path = row[2].trim().to_string();
            if existing_project_code.is_empty()
                || !crate::project_meta::is_valid_project_code(&existing_project_code)
            {
                continue;
            }

            let normalized_name = std::path::Path::new(&project_path)
                .file_name()
                .map(|value| value.to_string_lossy().trim().to_string())
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    (!existing_project_name.is_empty()).then_some(existing_project_name.clone())
                })
                .unwrap_or_else(|| existing_project_code.clone());

            if existing_project_name != normalized_name {
                self.execute_param(
                    "UPDATE soll.ProjectCodeRegistry SET project_name = ? WHERE project_code = ?",
                    &serde_json::json!([normalized_name, existing_project_code]),
                )?;
            }

            if !project_path.is_empty() {
                self.execute_param(
                    "UPDATE soll.ProjectCodeRegistry SET project_path = ? WHERE project_code = ?",
                    &serde_json::json!([project_path, existing_project_code]),
                )?;
            }
        }
        Ok(())
    }

    fn seed_global_guidelines(&self) -> Result<()> {
        let guidelines = [
            (
                "GUI-PRO-001",
                "TDD Obligatoire",
                "Les tests doivent être écrits avant ou avec le code source.",
                "{\"phase\": \"pre-code\", \"trigger_path\": \"src/axon-core/src/*\", \"required_path\": \"tests.rs\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-002",
                "Documentation MCP",
                "Toute modification de src/mcp/tools_*.rs nécessite la mise à jour de SKILL.md",
                "{\"phase\": \"post-code\", \"trigger_path\": \"src/axon-core/src/mcp/tools_*\", \"required_path\": \"SKILL.md\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-003",
                "Zéro Warning & Fail-Fast",
                "Tout code doit compiler et passer l'analyse statique avec formellement zéro avertissement (ex: deny(warnings) en Rust, --strict en TS). La CI doit échouer immédiatement au premier avertissement détecté.",
                "{\"phase\": \"compile\", \"trigger_path\": \"*\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-004",
                "Vérité Physique (Zéro Mock I/O)",
                "Interdiction stricte d'utiliser des mocks ou stubs pour simuler les entrées/sorties (Réseau, FS, DB). Les tests d'intégration doivent instancier des ressources physiques isolées et éphémères (ex: DB temporaires sur disque) pour valider les comportements réels (verrous, WAL, concurrence).",
                "{\"phase\": \"test\", \"trigger_path\": \"*\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-005",
                "Séparation des Plans (Control vs Data Plane)",
                "Isolation architecturale obligatoire entre les processus gérant l'état/routage (Control Plane, asynchrone, faible latence) et les processus exécutant les calculs lourds ou la logique métier complexe (Data Plane, synchrone, intensif). Le Control Plane ne doit exécuter aucune logique bloquante.",
                "{\"phase\": \"architecture\", \"trigger_path\": \"*\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-006",
                "Builds Déterministes & Hermétiques",
                "La compilation d'un commit doit produire un artefact dont l'empreinte (SHA-256) est strictement identique partout (Tolérance 0%). 100% des dépendances (système et applicatives) doivent être épinglées via un fichier de verrouillage avec hash cryptographique. Le build doit réussir en isolation réseau (Air-Gap).",
                "{\"phase\": \"build\", \"trigger_path\": \"*\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-007",
                "Télémétrie Structurée Native",
                "100% des événements applicatifs doivent être émis au format structuré (JSON/OTLP). Interdiction absolue des logs textuels bruts sur stdout nécessitant un parsing par regex. Propagation obligatoire des trace_id dans tous les appels RPC/IPC.",
                "{\"phase\": \"runtime\", \"trigger_path\": \"*\", \"enforcement\": \"strict\"}"
            ),
            (
                "GUI-PRO-008",
                "Résilience Mécanique (Design for Failure)",
                "Les systèmes distribués doivent intégrer des patterns de résilience (Circuit Breakers, Back-pressure, Dégradation Gracieuse). Les seuils et mécanismes de défaillance doivent être spécifiés explicitement par des Décisions (DEC) ou Exigences (REQ) au niveau du projet.",
                "{\"phase\": \"architecture\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-009",
                "Performance comme Propriété Native",
                "La performance ne s'optimise pas a posteriori. Les budgets de latence (SLO/p99) et les contraintes de ressources (CPU/RAM) doivent être quantifiés et testés en CI pour chaque composant critique via des Exigences (REQ) locales du projet.",
                "{\"phase\": \"architecture\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-010",
                "Sécurité Shift-Left & Moindre Privilège",
                "La sécurité (scan de vulnérabilités, gestion des secrets) est automatisée dès la CI. L'accès aux ressources s'opère par RBAC granulaire. Les politiques exactes de rotation des secrets et d'authentification doivent être définies par les Décisions (DEC) du projet.",
                "{\"phase\": \"security\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-011",
                "Évolutivité Humaine & Accessibilité Cognitive",
                "L'architecture modulaire doit limiter la charge cognitive (DDD, Clean Architecture). Le nommage est un acte de design reflétant le métier. Le versioning des API doit être explicite. Les choix d'implémentation de ces frontières sont délégués aux projets.",
                "{\"phase\": \"design\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-012",
                "Infrastructure as Code (IaC) & Reproductibilité d'Environnement",
                "Les environnements doivent être éphémères et recréables à la demande. L'état de l'infrastructure est versionné (GitOps). L'outil d'automatisation (Nix, Terraform, Docker) est défini par les Décisions (DEC) spécifiques du projet.",
                "{\"phase\": \"infrastructure\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-013",
                "DRY (Don't Repeat Yourself) & Single Source of Truth",
                "Éviter de décrire deux fois la même chose. Chaque connaissance, logique ou règle métier doit posséder une représentation unique et non ambiguë dans le système pour éviter la désynchronisation.",
                "{\"phase\": \"coding\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": false}"
            ),
            (
                "GUI-PRO-014",
                "SRP (Single Responsibility Principle) & Cohésion",
                "Une fonction, une classe ou un fichier ne doit avoir qu'une seule raison de changer. Les 'God Objects' (fichiers monolithiques) sont proscrits. Les responsabilités doivent être isolées.",
                "{\"phase\": \"coding\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": false}"
            ),
            (
                "GUI-PRO-015",
                "KISS (Keep It Simple, Stupid) & YAGNI",
                "Ne pas sur-ingénieriser. Ne pas écrire de code 'au cas où' (You Aren't Gonna Need It) pour un besoin futur hypothétique. Privilégier la solution la plus simple et lisible permettant de résoudre le problème actuel.",
                "{\"phase\": \"coding\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": false}"
            ),
            (
                "GUI-PRO-016",
                "Limites Cognitives & Complexité Cyclomatique",
                "Limitation stricte de l'imbrication et de la longueur des fonctions/fichiers. Une fonction doit idéalement être lisible sur un seul écran sans défilement mental complexe. Les seuils précis doivent être validés par les linters du projet.",
                "{\"phase\": \"coding\", \"trigger_path\": \"*\", \"enforcement\": \"advisory\", \"requires_local_decision\": true}"
            ),
            (
                "GUI-PRO-017",
                "Clean-As-You-Go (Zéro Code Mort)",
                "Le code obsolète, commenté ou remplacé doit être immédiatement supprimé une fois la nouvelle implémentation testée. La base de code ne doit contenir aucun code mort (fonctions sans appelants actifs).",
                "{\"phase\": \"refactoring\", \"trigger_path\": \"*\", \"enforcement\": \"strict\", \"requires_local_decision\": false}"
            )
        ];

        for (id, title, desc, meta) in guidelines.iter() {
            match self.execute_param(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata) 
                 VALUES (?, 'Guideline', 'PRO', ?, ?, 'active', ?)
                 ON CONFLICT (id) DO UPDATE SET title = EXCLUDED.title, description = EXCLUDED.description, metadata = EXCLUDED.metadata",
                &serde_json::json!([id, title, desc, meta])
            ) {
                Ok(_) => {},
                Err(e) => {
                    println!("🚨 SEED GUIDELINE ERROR for {}: {:?}", id, e);
                    log::error!("SEED GUIDELINE ERROR for {}: {:?}", id, e);
                }
            }
        }
        Ok(())
    }

    fn seed_project_code_registry(&self) -> Result<()> {
        self.sync_project_registry_entry("PRO", Some("System Global Namespace"), None)?;
        Ok(())
    }

    pub(crate) fn sync_project_registry_entry(
        &self,
        project_code: &str,
        project_name: Option<&str>,
        project_path: Option<&str>,
    ) -> Result<()> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        let normalized_name = project_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                project_path
                    .and_then(|path| std::path::Path::new(path).file_name())
                    .map(|value| value.to_string_lossy().trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| normalized_code.clone());
        if normalized_code.is_empty()
            || !crate::project_meta::is_valid_project_code(&normalized_code)
        {
            return Ok(());
        }

        let normalized_path = project_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        self.execute_param(
            "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path) VALUES (?, ?, ?) ON CONFLICT (project_code) DO UPDATE SET project_name = EXCLUDED.project_name, project_path = EXCLUDED.project_path",
            &serde_json::json!([normalized_code, normalized_name, normalized_path]),
        )?;

        Ok(())
    }

    fn migrate_canonical_soll_ids(&self) -> Result<()> {
        self.migrate_prefixed_id_table("soll.Vision")?;
        self.migrate_prefixed_id_table("soll.Pillar")?;
        self.migrate_prefixed_id_table("soll.Requirement")?;
        self.migrate_prefixed_id_table("soll.Decision")?;
        self.migrate_prefixed_id_table("soll.Milestone")?;
        self.migrate_prefixed_id_table("soll.Validation")?;
        self.migrate_concepts_to_server_ids()?;
        self.migrate_stakeholders_to_server_ids()?;
        self.migrate_revision_preview_ids()?;
        self.migrate_revision_ids()?;
        Ok(())
    }

    fn migrate_revision_preview_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT preview_id, COALESCE(project_code,''), COALESCE(created_at, 0)
             FROM soll.RevisionPreview
             ORDER BY created_at ASC, preview_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let old_id = row[0].trim().to_string();
            let project_code = row[1].trim().to_string();
            if old_id.is_empty() || project_code.is_empty() {
                continue;
            }
            let (_, project_code) =
                self.resolve_or_seed_existing_project_identity(&project_code)?;
            let next = next_by_code.get(&project_code).copied().unwrap_or(0) + 1;
            next_by_code.insert(project_code.clone(), next);
            let new_id = format!("PRV-{}-{:03}", project_code, next);

            if old_id == new_id {
                continue;
            }

            if self.table_has_named_id("soll.RevisionPreview", "preview_id", &new_id)? {
                self.delete_row_by_named_id("soll.RevisionPreview", "preview_id", &old_id)?;
            } else {
                self.execute_param(
                    "UPDATE soll.RevisionPreview SET preview_id = ? WHERE preview_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_revision_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT revision_id, COALESCE(created_at, 0)
             FROM soll.Revision
             ORDER BY created_at ASC, revision_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let old_id = row[0].trim().to_string();
            let Some((_, project_part, _)) = parse_prefixed_entity_id(&old_id) else {
                continue;
            };
            let (_, project_code) = self.resolve_or_seed_existing_project_identity(project_part)?;
            let next = next_by_code.get(&project_code).copied().unwrap_or(0) + 1;
            next_by_code.insert(project_code.clone(), next);
            let new_id = format!("REV-{}-{:03}", project_code, next);

            if old_id == new_id {
                continue;
            }

            if self.table_has_named_id("soll.Revision", "revision_id", &new_id)? {
                self.execute_param(
                    "UPDATE soll.RevisionChange SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
                self.delete_row_by_named_id("soll.Revision", "revision_id", &old_id)?;
            } else {
                self.execute_param(
                    "UPDATE soll.Revision SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
                self.execute_param(
                    "UPDATE soll.RevisionChange SET revision_id = ? WHERE revision_id = ?",
                    &serde_json::json!([new_id, old_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_prefixed_id_table(&self, table: &str) -> Result<()> {
        let raw = self.query_json(&format!("SELECT id FROM {} ORDER BY id", table))?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            let Some(old_id) = row.first().cloned() else {
                continue;
            };
            let Some((prefix, project_part, number)) = parse_prefixed_entity_id(&old_id) else {
                continue;
            };
            let (_, project_code) = self.resolve_or_seed_existing_project_identity(project_part)?;
            let new_id = format!("{}-{}-{:03}", prefix, project_code, number);
            if new_id != old_id {
                if self.table_has_id(table, &new_id)? {
                    self.replace_soll_id_references(&old_id, &new_id)?;
                    self.delete_row_by_id(table, &old_id)?;
                } else {
                    self.execute_param(
                        &format!("UPDATE {} SET id = ? WHERE id = ?", table),
                        &serde_json::json!([new_id, old_id]),
                    )?;
                    self.replace_soll_id_references(&old_id, &new_id)?;
                }
            }
            if table == "soll.Vision" {
                self.execute_param(
                    "UPDATE soll.Vision SET project_code = ? WHERE id = ?",
                    &serde_json::json!([project_code, new_id]),
                )?;
            }
        }
        Ok(())
    }

    fn migrate_concepts_to_server_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(id,''), COALESCE(project_code,''), title
             FROM soll.Node WHERE type='Concept'
             ORDER BY title",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_id = row[0].clone();
            let existing_project_code = row[1].clone();
            let stored_name = row[2].clone();

            let source_id = if !existing_id.trim().is_empty() {
                existing_id.clone()
            } else if let Some((parsed_id, parsed_name)) = split_prefixed_display_name(&stored_name)
            {
                let _ = parsed_name;
                parsed_id
            } else {
                continue;
            };

            let Some((_, project_part, number)) = parse_prefixed_entity_id(&source_id) else {
                continue;
            };
            let project_code = if !existing_project_code.trim().is_empty() {
                existing_project_code.clone()
            } else {
                self.resolve_or_seed_existing_project_identity(project_part)?
                    .1
            };
            let new_id = format!("CPT-{}-{:03}", project_code, number);

            if new_id == existing_id && existing_project_code == project_code {
                continue;
            }

            if new_id != source_id && self.table_has_id("soll.Concept", &new_id)? {
                self.replace_soll_id_references(&source_id, &new_id)?;
                self.execute_param(
                    "DELETE FROM soll.Node WHERE type='Concept' AND COALESCE(id,'') = ? AND title = ?",
                    &serde_json::json!([existing_id, stored_name]),
                )?;
            } else if new_id == existing_id {
                self.execute_param(
                    "UPDATE soll.Concept
                     SET project_code = ?
                     WHERE id = ?",
                    &serde_json::json!([project_code, existing_id]),
                )?;
            } else {
                self.execute_param(
                    "UPDATE soll.Concept
                     SET id = ?, project_code = ?
                     WHERE COALESCE(id,'') = ? AND name = ?",
                    &serde_json::json!([new_id, project_code, existing_id, stored_name]),
                )?;

                if new_id != source_id {
                    self.replace_soll_id_references(&source_id, &new_id)?;
                }
            }
        }
        Ok(())
    }

    fn migrate_stakeholders_to_server_ids(&self) -> Result<()> {
        let raw = self.query_json(
            "SELECT COALESCE(id,''), COALESCE(project_code,''), title
             FROM soll.Node WHERE type='Stakeholder'
             ORDER BY title",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let mut next_by_code: HashMap<String, u64> = HashMap::new();

        for row in rows {
            if row.len() < 3 {
                continue;
            }
            let existing_id = row[0].clone();
            let existing_project_code = row[1].clone();
            let name = row[2].clone();

            let (project_code, source_id, new_id) = if let Some((prefix, project_part, number)) =
                parse_prefixed_entity_id(&existing_id)
            {
                let code = if !existing_project_code.trim().is_empty() {
                    existing_project_code.clone()
                } else {
                    self.resolve_or_seed_existing_project_identity(project_part)?
                        .1
                };
                (
                    code.clone(),
                    existing_id.clone(),
                    format!("{}-{}-{:03}", prefix, code, number),
                )
            } else {
                let initial_code = if existing_project_code.trim().is_empty() {
                    "AXO".to_string()
                } else {
                    existing_project_code.clone()
                };
                let (_, code) = self.resolve_or_seed_existing_project_identity(&initial_code)?;
                let next = match next_by_code.get(&code).copied() {
                    Some(current) => current + 1,
                    None => self.max_numeric_suffix_for_prefix(&format!("STK-{}-", code))? + 1,
                };
                next_by_code.insert(code.clone(), next);
                (
                    code.clone(),
                    if existing_id.trim().is_empty() {
                        name.clone()
                    } else {
                        existing_id.clone()
                    },
                    format!("STK-{}-{:03}", code, next),
                )
            };

            if new_id == existing_id && existing_project_code == project_code {
                continue;
            }

            if new_id != source_id && self.table_has_id("soll.Stakeholder", &new_id)? {
                self.replace_soll_id_references(&source_id, &new_id)?;
                self.execute_param(
                    "DELETE FROM soll.Node WHERE type='Stakeholder' AND COALESCE(id,'') = ? AND title = ?",
                    &serde_json::json!([existing_id, name]),
                )?;
            } else if new_id == existing_id {
                self.execute_param(
                    "UPDATE soll.Stakeholder
                     SET project_code = ?
                     WHERE id = ?",
                    &serde_json::json!([project_code, existing_id]),
                )?;
            } else {
                self.execute_param(
                    "UPDATE soll.Stakeholder
                     SET id = ?, project_code = ?
                     WHERE COALESCE(id,'') = ? AND name = ?",
                    &serde_json::json!([new_id, project_code, existing_id, name]),
                )?;

                if new_id != source_id {
                    self.replace_soll_id_references(&source_id, &new_id)?;
                }
            }
        }
        Ok(())
    }

    fn table_has_id(&self, table: &str, id: &str) -> Result<bool> {
        Ok(self.query_count(&format!(
            "SELECT count(*) FROM {} WHERE id = '{}'",
            table,
            id.replace('\'', "''")
        ))? > 0)
    }

    fn table_has_named_id(&self, table: &str, column: &str, id: &str) -> Result<bool> {
        Ok(self.query_count(&format!(
            "SELECT count(*) FROM {} WHERE {} = '{}'",
            table,
            column,
            id.replace('\'', "''")
        ))? > 0)
    }

    fn delete_row_by_id(&self, table: &str, id: &str) -> Result<()> {
        self.execute_param(
            &format!("DELETE FROM {} WHERE id = ?", table),
            &serde_json::json!([id]),
        )?;
        Ok(())
    }

    fn delete_row_by_named_id(&self, table: &str, column: &str, id: &str) -> Result<()> {
        self.execute_param(
            &format!("DELETE FROM {} WHERE {} = ?", table, column),
            &serde_json::json!([id]),
        )?;
        Ok(())
    }

    fn max_numeric_suffix_for_prefix(&self, prefix: &str) -> Result<u64> {
        let mut max_seen = 0u64;
        for table in [
            "soll.Vision",
            "soll.Pillar",
            "soll.Requirement",
            "soll.Decision",
            "soll.Milestone",
            "soll.Validation",
            "soll.Concept",
            "soll.Stakeholder",
        ] {
            let id_col = "id";
            let raw = self.query_json(&format!(
                "SELECT {} FROM {} WHERE {} LIKE '{}%'",
                id_col,
                table,
                id_col,
                prefix.replace('\'', "''")
            ))?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            for row in rows {
                if let Some(id) = row.first() {
                    if let Some((_, _, number)) = parse_prefixed_entity_id(id) {
                        max_seen = max_seen.max(number);
                    }
                }
            }
        }
        Ok(max_seen)
    }

    fn resolve_or_seed_existing_project_identity(
        &self,
        project_code: &str,
    ) -> Result<(String, String)> {
        let key = project_code.trim();
        if key.is_empty() {
            return Err(anyhow!("Empty project identifier"));
        }

        let by_code = self.query_json(&format!(
            "SELECT project_code FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
            key.replace('\'', "''")
        ))?;
        let code_rows: Vec<Vec<String>> = serde_json::from_str(&by_code).unwrap_or_default();
        if let Some(row) = code_rows.first() {
            if let Some(code) = row.first() {
                return Ok((code.clone(), code.clone()));
            }
        }

        Err(anyhow!("Missing project code registry entry for {}", key))
    }

    fn replace_soll_id_references(&self, old_id: &str, new_id: &str) -> Result<()> {
        if old_id == new_id {
            return Ok(());
        }
        for table in [
            "soll.EPITOMIZES",
            "soll.BELONGS_TO",
            "soll.EXPLAINS",
            "soll.SOLVES",
            "soll.TARGETS",
            "soll.VERIFIES",
            "soll.ORIGINATES",
            "soll.SUPERSEDES",
            "soll.CONTRIBUTES_TO",
            "soll.REFINES",
            "IMPACTS",
            "SUBSTANTIATES",
        ] {
            self.execute_param(
                &format!("UPDATE {} SET source_id = ? WHERE source_id = ?", table),
                &serde_json::json!([new_id, old_id]),
            )?;
            self.execute_param(
                &format!("UPDATE {} SET target_id = ? WHERE target_id = ?", table),
                &serde_json::json!([new_id, old_id]),
            )?;
        }

        self.execute_param(
            "UPDATE soll.Traceability SET soll_entity_id = ? WHERE soll_entity_id = ?",
            &serde_json::json!([new_id, old_id]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET entity_id = ? WHERE entity_id = ?",
            &serde_json::json!([new_id, old_id]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET before_json = REPLACE(before_json, ?, ?) WHERE before_json LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionChange SET after_json = REPLACE(after_json, ?, ?) WHERE after_json LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        self.execute_param(
            "UPDATE soll.RevisionPreview SET payload = REPLACE(payload, ?, ?) WHERE payload LIKE ?",
            &serde_json::json!([old_id, new_id, format!("%{}%", old_id)]),
        )?;
        Ok(())
    }

    fn ensure_runtime_compatibility(&self) -> Result<()> {
        let expected = [
            ("schema_version", IST_SCHEMA_VERSION),
            ("ingestion_version", IST_INGESTION_VERSION),
            ("embedding_version", IST_EMBEDDING_VERSION),
        ];

        let before_graph_ready = self
            .query_count("SELECT count(*) FROM File WHERE graph_ready = TRUE")
            .unwrap_or(0);
        let before_vector_ready = self
            .query_count("SELECT count(*) FROM File WHERE vector_ready = TRUE")
            .unwrap_or(0);
        let before_vec_queue_queued = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'queued'")
            .unwrap_or(0);
        let before_vec_queue_inflight = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'")
            .unwrap_or(0);

        let current = self.load_runtime_metadata()?;
        let schema_matches = current
            .get("schema_version")
            .is_some_and(|v| v == IST_SCHEMA_VERSION);
        let ingestion_matches = current
            .get("ingestion_version")
            .is_some_and(|v| v == IST_INGESTION_VERSION);
        let embedding_matches = current
            .get("embedding_version")
            .is_some_and(|v| v == IST_EMBEDDING_VERSION);

        info!(
            "IST compatibility preflight: current(schema={}, ingestion={}, embedding={}) expected(schema={}, ingestion={}, embedding={}) matches(schema={}, ingestion={}, embedding={}) before(graph_ready={}, vector_ready={}, vec_queue_queued={}, vec_queue_inflight={})",
            current.get("schema_version").map(String::as_str).unwrap_or("missing"),
            current.get("ingestion_version").map(String::as_str).unwrap_or("missing"),
            current.get("embedding_version").map(String::as_str).unwrap_or("missing"),
            IST_SCHEMA_VERSION,
            IST_INGESTION_VERSION,
            IST_EMBEDDING_VERSION,
            schema_matches,
            ingestion_matches,
            embedding_matches,
            before_graph_ready,
            before_vector_ready,
            before_vec_queue_queued,
            before_vec_queue_inflight
        );

        let mut applied = Vec::new();

        if !schema_matches {
            if self.is_known_additive_schema_repair(&current)? {
                info!("IST schema drift detected but preserved via additive repair.");
                applied.push(IstCompatibilityAction::AdditiveRepair);
            } else {
                warn!("IST schema drift is incompatible with current runtime. Rebuilding IST while preserving SOLL.");
                self.reset_ist_state()?;
                applied.push(IstCompatibilityAction::HardRebuild);
            }
        }

        if !ingestion_matches && !applied.contains(&IstCompatibilityAction::HardRebuild) {
            warn!("IST ingestion drift detected. Soft-invalidating derived structural layers while preserving File backlog.");
            self.soft_invalidate_derived_state()?;
            applied.push(IstCompatibilityAction::SoftDerivedInvalidation);
        } else if !embedding_matches && !applied.contains(&IstCompatibilityAction::HardRebuild) {
            warn!(
                "IST embedding drift detected. Soft-invalidating semantic embedding layers only."
            );
            self.soft_invalidate_embedding_state()?;
            applied.push(IstCompatibilityAction::SoftEmbeddingInvalidation);
        }

        if applied.is_empty() {
            info!("IST runtime metadata is compatible with current Axon Core.");
            applied.push(IstCompatibilityAction::Noop);
        }

        self.write_runtime_metadata(&expected)?;

        let after_graph_ready = self
            .query_count("SELECT count(*) FROM File WHERE graph_ready = TRUE")
            .unwrap_or(0);
        let after_vector_ready = self
            .query_count("SELECT count(*) FROM File WHERE vector_ready = TRUE")
            .unwrap_or(0);
        let after_vec_queue_queued = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'queued'")
            .unwrap_or(0);
        let after_vec_queue_inflight = self
            .query_count("SELECT count(*) FROM FileVectorizationQueue WHERE status = 'inflight'")
            .unwrap_or(0);

        info!(
            "IST compatibility actions={:?} after(graph_ready={}, vector_ready={}, vec_queue_queued={}, vec_queue_inflight={})",
            applied,
            after_graph_ready,
            after_vector_ready,
            after_vec_queue_queued,
            after_vec_queue_inflight
        );
        Ok(())
    }

    fn recover_interrupted_indexing(&self) -> Result<()> {
        let existing = self.query_json("SELECT name FROM pragma_table_info('File')")?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&existing).unwrap_or_default();
        let columns: std::collections::HashSet<String> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();

        if !columns.contains("status") {
            return Ok(());
        }

        let interrupted =
            self.query_count("SELECT count(*) FROM File WHERE status = 'indexing'")?;
        if interrupted > 0 {
            warn!(
                "Recovering {} interrupted indexing claim(s) back to pending during startup.",
                interrupted
            );
            self.execute(
                "UPDATE File SET status = 'pending', worker_id = NULL, status_reason = 'recovered_interrupted_indexing', file_stage = 'promoted' WHERE status = 'indexing'",
            )?;
        }
        Ok(())
    }

    fn load_runtime_metadata(&self) -> Result<std::collections::HashMap<String, String>> {
        let existing = self.query_json("SELECT key, value FROM RuntimeMetadata")?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&existing).unwrap_or_default();
        let mut current = std::collections::HashMap::new();
        for row in rows {
            if row.len() >= 2 {
                current.insert(row[0].clone(), row[1].clone());
            }
        }
        Ok(current)
    }

    fn write_runtime_metadata(&self, expected: &[(&str, &str)]) -> Result<()> {
        self.execute("DELETE FROM RuntimeMetadata")?;
        for (key, value) in expected {
            self.execute(&format!(
                "INSERT INTO RuntimeMetadata (key, value) VALUES ('{}', '{}')",
                key, value
            ))?;
        }
        Ok(())
    }

    fn is_known_additive_schema_repair(
        &self,
        current: &std::collections::HashMap<String, String>,
    ) -> Result<bool> {
        let schema_version = current.get("schema_version").map(String::as_str);
        if schema_version != Some("1") && schema_version != Some("2") {
            return Ok(false);
        }

        let columns = self.list_file_table_columns()?;

        let required = [
            "path",
            "project_code",
            "status",
            "size",
            "priority",
            "mtime",
            "worker_id",
            "trace_id",
            "needs_reindex",
            "file_stage",
            "graph_ready",
            "vector_ready",
        ];

        Ok(required.iter().all(|column| columns.contains(*column)))
    }

    fn list_file_table_columns(&self) -> Result<std::collections::HashSet<String>> {
        let existing = self.query_json("SELECT name FROM pragma_table_info('File')")?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&existing).unwrap_or_default();
        let columns: std::collections::HashSet<String> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        Ok(columns)
    }

    fn list_project_code_registry_columns(&self) -> Result<std::collections::HashSet<String>> {
        for target in ["soll.ProjectCodeRegistry", "ProjectCodeRegistry"] {
            let raw =
                self.query_json(&format!("SELECT name FROM pragma_table_info('{target}')"))?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            let columns: std::collections::HashSet<String> = rows
                .into_iter()
                .filter_map(|row| row.into_iter().next())
                .collect();
            if !columns.is_empty() {
                return Ok(columns);
            }
        }
        Ok(std::collections::HashSet::new())
    }

    fn list_soll_node_columns(&self) -> Result<std::collections::HashSet<String>> {
        for target in ["soll.Node", "Node"] {
            let raw =
                self.query_json(&format!("SELECT name FROM pragma_table_info('{target}')"))?;
            let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
            let columns: std::collections::HashSet<String> = rows
                .into_iter()
                .filter_map(|row| row.into_iter().next())
                .collect();
            if !columns.is_empty() {
                return Ok(columns);
            }
        }
        Ok(std::collections::HashSet::new())
    }

    fn reset_ist_state(&self) -> Result<()> {
        let cleanup_queries = [
            "DELETE FROM CALLS_NIF",
            "DELETE FROM CALLS",
            "DELETE FROM CONTAINS",
            "DELETE FROM IMPACTS",
            "DELETE FROM SUBSTANTIATES",
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "DELETE FROM Project",
        ];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        self.rebuild_graph_projection_runtime_tables()?;
        self.rebuild_embedding_runtime_tables()?;

        self.execute("DROP TABLE IF EXISTS File;")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_code VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE, first_seen_at_ms BIGINT, indexing_started_at_ms BIGINT, graph_ready_at_ms BIGINT, vectorization_started_at_ms BIGINT, vector_ready_at_ms BIGINT, last_state_change_at_ms BIGINT, last_error_at_ms BIGINT)",
        )?;

        info!("IST state reset complete. SOLL sanctuary preserved.");
        Ok(())
    }

    fn soft_invalidate_derived_state(&self) -> Result<()> {
        let cleanup_queries = [
            "DELETE FROM CALLS_NIF",
            "DELETE FROM CALLS",
            "DELETE FROM CONTAINS",
            "DELETE FROM IMPACTS",
            "DELETE FROM SUBSTANTIATES",
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "UPDATE File SET status = 'pending', worker_id = NULL, needs_reindex = FALSE, status_reason = 'soft_invalidated', file_stage = 'promoted', graph_ready = FALSE, vector_ready = FALSE",
        ];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        self.rebuild_file_runtime_table()?;
        self.rebuild_graph_projection_runtime_tables()?;
        self.rebuild_embedding_runtime_tables()?;

        info!("IST derived structural layers soft-invalidated. File backlog preserved for replay.");
        Ok(())
    }

    fn soft_invalidate_embedding_state(&self) -> Result<()> {
        let cleanup_queries = ["UPDATE File SET vector_ready = FALSE WHERE graph_ready = TRUE"];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        self.rebuild_embedding_runtime_tables()?;

        info!("IST embedding layers soft-invalidated. Structural truth preserved.");
        Ok(())
    }

    fn rebuild_file_runtime_table(&self) -> Result<()> {
        self.execute("DROP TABLE IF EXISTS File_rebuilt;")?;
        self.execute(
            "CREATE TABLE File_rebuilt (path VARCHAR PRIMARY KEY, project_code VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE, first_seen_at_ms BIGINT, indexing_started_at_ms BIGINT, graph_ready_at_ms BIGINT, vectorization_started_at_ms BIGINT, vector_ready_at_ms BIGINT, last_state_change_at_ms BIGINT, last_error_at_ms BIGINT)",
        )?;
        self.execute(
            "INSERT INTO File_rebuilt (path, project_code, status, size, priority, mtime, worker_id, trace_id, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, file_stage, graph_ready, vector_ready, first_seen_at_ms, indexing_started_at_ms, graph_ready_at_ms, vectorization_started_at_ms, vector_ready_at_ms, last_state_change_at_ms, last_error_at_ms) \
             SELECT path, project_code, status, size, priority, mtime, worker_id, trace_id, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, file_stage, graph_ready, vector_ready, first_seen_at_ms, indexing_started_at_ms, graph_ready_at_ms, vectorization_started_at_ms, vector_ready_at_ms, last_state_change_at_ms, last_error_at_ms \
             FROM ( \
                 SELECT path, project_code, status, size, priority, mtime, worker_id, trace_id, needs_reindex, last_error_reason, status_reason, defer_count, last_deferred_at_ms, file_stage, graph_ready, vector_ready, first_seen_at_ms, indexing_started_at_ms, graph_ready_at_ms, vectorization_started_at_ms, vector_ready_at_ms, last_state_change_at_ms, last_error_at_ms, \
                        ROW_NUMBER() OVER (PARTITION BY path ORDER BY COALESCE(mtime, 0) DESC, COALESCE(priority, 0) DESC, path ASC) AS rownum \
                 FROM File \
             ) ranked \
             WHERE rownum = 1;",
        )?;
        self.execute("DROP TABLE File;")?;
        self.execute("ALTER TABLE File_rebuilt RENAME TO File;")?;
        Ok(())
    }
}

#[cfg(test)]
mod graph_bootstrap_tests {
    use crate::embedding_contract::{
        CHUNK_MODEL_ID, DIMENSION, GRAPH_MODEL_ID, MODEL_NAME, MODEL_VERSION,
    };
    use crate::tests::test_helpers::create_test_db;

    #[test]
    fn test_normalize_project_code_registry_mirrors_code_and_derives_name_from_path() {
        let store = create_test_db().unwrap();
        store
            .execute_param(
                "UPDATE soll.ProjectCodeRegistry
                 SET project_code = ?, project_name = ?, project_path = ?
                 WHERE project_code = ?",
                &serde_json::json!([
                    "BKS",
                    "Legacy Human Name",
                    "/home/dstadel/projects/BookingSystem",
                    "BKS"
                ]),
            )
            .unwrap();

        store.normalize_project_code_registry().unwrap();

        let rows = store
            .query_json(
                "SELECT project_code, project_name, project_path
                 FROM soll.ProjectCodeRegistry
                 WHERE project_code = 'BKS'",
            )
            .unwrap();
        let parsed: Vec<Vec<String>> = serde_json::from_str(&rows).unwrap();
        let row = parsed.first().expect("registry row");
        assert_eq!(row[0], "BKS");
        assert_eq!(row[1], "BookingSystem");
        assert_eq!(row[2], "/home/dstadel/projects/BookingSystem");
    }

    #[test]
    fn test_soft_invalidate_embedding_state_rebuilds_embedding_tables() {
        let store = create_test_db().unwrap();
        store.execute(&format!("INSERT INTO EmbeddingModel (id, kind, model_name, dimension, version, created_at) VALUES ('{CHUNK_MODEL_ID}', 'chunk', '{MODEL_NAME}', {DIMENSION}, '{MODEL_VERSION}', 1)")).unwrap();
        store.execute(&format!("INSERT INTO ChunkEmbedding (chunk_id, model_id, embedding, source_hash) VALUES ('chunk-1', '{CHUNK_MODEL_ID}', CAST([1.0] || repeat([0.0], {}) AS FLOAT[{DIMENSION}]), 'hash-1')", DIMENSION - 1)).unwrap();
        store.execute(&format!("INSERT INTO GraphEmbedding (anchor_type, anchor_id, radius, model_id, source_signature, projection_version, embedding, updated_at) VALUES ('symbol', 'global::demo', 1, '{GRAPH_MODEL_ID}', 'sig-1', '1', CAST([1.0] || repeat([0.0], {}) AS FLOAT[{DIMENSION}]), 1)", DIMENSION - 1)).unwrap();
        store.execute("INSERT INTO FileVectorizationQueue (file_path, status, queued_at) VALUES ('/tmp/demo.rs', 'queued', 1)").unwrap();
        store.execute(&format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_code, embedding) VALUES ('global::demo', 'demo', 'function', FALSE, TRUE, FALSE, FALSE, 'AXO', CAST([1.0] || repeat([0.0], {}) AS FLOAT[{DIMENSION}]))", DIMENSION - 1)).unwrap();
        store.execute("INSERT INTO File (path, project_code, status, size, priority, mtime, graph_ready, vector_ready) VALUES ('/tmp/demo.rs', 'AXO', 'indexed', 1, 1, 1, TRUE, TRUE)").unwrap();

        store.soft_invalidate_embedding_state().unwrap();

        assert_eq!(
            store
                .query_count("SELECT count(*) FROM EmbeddingModel")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM ChunkEmbedding")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM GraphEmbedding")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM FileVectorizationQueue")
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .query_count("SELECT count(*) FROM File WHERE vector_ready = TRUE")
                .unwrap(),
            0
        );
    }
}

#[allow(dead_code)]
fn parse_prefixed_entity_id(value: &str) -> Option<(&str, &str, u64)> {
    let trimmed = value.trim();
    let mut parts = trimmed.splitn(3, '-');
    let prefix = parts.next()?;
    let project = parts.next()?;
    let number_str = parts.next()?;
    let number = number_str.parse::<u64>().ok()?;
    Some((prefix, project, number))
}

#[allow(dead_code)]
fn split_prefixed_display_name(value: &str) -> Option<(String, String)> {
    let (id_part, name_part) = value.split_once(':')?;
    let id = id_part.trim();
    parse_prefixed_entity_id(id)?;
    Some((id.to_string(), name_part.trim().to_string()))
}
