use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use libloading::Library;
use tracing::{info, warn};

use crate::embedding_contract::{DIMENSION, GRAPH_MODEL_ID};
use crate::graph::{GraphStore, LatticePool};
use crate::runtime_mode::graph_embeddings_enabled;
use crate::runtime_mode::AxonRuntimeMode;
use crate::runtime_topology::current_runtime_process_role;
use crate::runtime_truth_contract::RuntimeFreshnessContract;

const IST_SCHEMA_VERSION: &str = "3";
const IST_INGESTION_VERSION: &str = "4";
// Bump to force a one-time rebuild of derived embedding storage after the
// crash-safe table reconstruction path was introduced.
const IST_EMBEDDING_VERSION: &str = "2";
const STARTUP_SEMANTIC_BACKFILL_FLOOR: usize = 64;

/// MIL-AXO-015 P3 slice 3b: resolve the connection URL to use for
/// the PostgreSQL plugin. Honoured precedence — `AXON_LIVE_DATABASE_URL`
/// > `AXON_DEV_DATABASE_URL` > `DATABASE_URL`. The preference for live
/// matches the brain's most common runtime mode; callers running a dev
/// loop must override `AXON_LIVE_DATABASE_URL` (e.g. set it to empty)
/// or set the dev URL explicitly. axon-core does not currently know
/// which AxonInstance the caller intends — that resolution lives one
/// layer up in `RuntimeProcessRole` and is wired in slice 3c.
fn resolve_pg_database_url() -> Result<String> {
    for var in [
        "AXON_LIVE_DATABASE_URL",
        "AXON_DEV_DATABASE_URL",
        "DATABASE_URL",
    ] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Ok(v);
            }
        }
    }
    Err(anyhow!(
        "no PostgreSQL connection URL configured (set AXON_LIVE_DATABASE_URL, \
         AXON_DEV_DATABASE_URL, or DATABASE_URL)"
    ))
}

pub fn canonical_soll_db_path(db_root: &str) -> Option<PathBuf> {
    if db_root == ":memory:" {
        return None;
    }

    let mut path = PathBuf::from(db_root);
    path.push("soll.db");
    Some(path)
}

pub fn canonical_ist_db_path(db_root: &str) -> Option<PathBuf> {
    if db_root == ":memory:" {
        return None;
    }

    let mut path = PathBuf::from(db_root);
    path.push("ist.db");
    Some(path)
}

pub fn canonical_ist_reader_db_path(db_root: &str) -> Option<PathBuf> {
    if db_root == ":memory:" {
        return None;
    }

    let mut path = PathBuf::from(db_root);
    path.push("ist-reader.db");
    Some(path)
}

fn reader_db_exists(db_path: &Option<PathBuf>) -> bool {
    db_path.as_ref().is_some_and(|path| path.exists())
}

fn startup_vector_backfill_limit(
    _structural_graph_backlog_depth: usize,
    graph_ready_depth: usize,
) -> usize {
    if graph_ready_depth == 0 {
        return 0;
    }
    let startup_budget = STARTUP_SEMANTIC_BACKFILL_FLOOR;
    startup_budget.min(graph_ready_depth)
}

fn remove_path_if_exists(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn build_soll_attach_query(path: &std::path::Path, mode: SollAccessMode) -> String {
    match mode {
        SollAccessMode::ReadWrite => format!(
            "ATTACH '{}' AS soll;",
            path.to_string_lossy().replace('\'', "''")
        ),
        SollAccessMode::ReadOnlyOrEmptySchema => {
            if path.exists() {
                format!(
                    "ATTACH '{}' AS soll (READ_ONLY);",
                    path.to_string_lossy().replace('\'', "''")
                )
            } else {
                "CREATE SCHEMA IF NOT EXISTS soll;".to_string()
            }
        }
        SollAccessMode::Detached => String::new(),
    }
}

fn split_brain_ist_reader_soll_writer_mode() -> bool {
    if matches!(
        std::env::var("AXON_SPLIT_BRAIN_IST_READER_ONLY")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("yes") | Some("on")
    ) {
        return true;
    }
    matches!(
        current_runtime_process_role(),
        crate::runtime_topology::AxonProcessRole::Brain
    ) && matches!(AxonRuntimeMode::from_env(), AxonRuntimeMode::BrainOnly)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IstCompatibilityAction {
    Noop,
    AdditiveRepair,
    SoftDerivedInvalidation,
    SoftEmbeddingInvalidation,
    HardRebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SollAccessMode {
    ReadWrite,
    ReadOnlyOrEmptySchema,
    Detached,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        Self::new_with_modes(
            db_root,
            !db_root.eq(":memory:") && split_brain_ist_reader_soll_writer_mode(),
            SollAccessMode::ReadWrite,
        )
    }

    pub fn new_brain_reader_soll_writer(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, db_root != ":memory:", SollAccessMode::ReadWrite)
    }

    pub fn new_indexer_ist_writer_soll_reader(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, false, SollAccessMode::ReadOnlyOrEmptySchema)
    }

    pub fn new_indexer_ist_writer_without_soll(db_root: &str) -> Result<Self> {
        Self::new_with_modes(db_root, false, SollAccessMode::Detached)
    }

    pub fn new_indexer_ist_writer_split(db_root: &str) -> Result<Self> {
        Self::new_indexer_ist_writer_without_soll(db_root)
    }

    fn new_with_modes(
        db_root: &str,
        split_brain_mode: bool,
        soll_access_mode: SollAccessMode,
    ) -> Result<Self> {
        let backend = crate::graph::PluginBackend::current();
        let plugin_path = Self::find_plugin_path_for(backend)?;
        let lib = Arc::new(unsafe { Library::new(&plugin_path)? });
        let symbols = unsafe { crate::graph::PluginSymbols::resolve(&lib, backend) }?;
        let init_fn = symbols.init_fn;
        let close_fn = symbols.close_fn;
        let is_memory = db_root == ":memory:";
        // MIL-AXO-015 P3 slice 3b: split-brain is a duckdb-specific
        // file-isolation pattern. PostgreSQL's MVCC handles
        // reader/writer concurrency natively, so we collapse to a
        // single context regardless of caller intent.
        let split_brain_mode = !is_memory
            && split_brain_mode
            && backend == crate::graph::PluginBackend::Duckdb;
        info!(
            "GraphStore init modes: backend={:?}, db_root={}, split_brain_mode={}, soll_access_mode={:?}",
            backend, db_root, split_brain_mode, soll_access_mode
        );

        // MIL-AXO-015 P3 slice 3b: under PostgreSQL the "DB path" is a
        // DATABASE_URL passed verbatim to pg_init_db_compat. The SOLL /
        // IST file-layout below applies only to the duckdb backend;
        // PG keeps SOLL + per-project IST inside the same database via
        // schema namespacing (CPT-AXO-039).
        let pg_database_url: Option<String> = match backend {
            crate::graph::PluginBackend::Postgres => {
                Some(resolve_pg_database_url().with_context(|| {
                    "AXON_DB_BACKEND=postgres requires AXON_LIVE_DATABASE_URL, \
                     AXON_DEV_DATABASE_URL, or DATABASE_URL to be set"
                })?)
            }
            crate::graph::PluginBackend::Duckdb => None,
        };

        if backend == crate::graph::PluginBackend::Duckdb
            && !is_memory
            && matches!(soll_access_mode, SollAccessMode::ReadWrite)
        {
            let soll_dir = PathBuf::from(db_root);
            std::fs::create_dir_all(&soll_dir)?;

            let soll_path = canonical_soll_db_path(db_root)
                .ok_or_else(|| anyhow!("Failed to derive SOLL database path"))?;
            let soll_c_path = CString::new(soll_path.to_string_lossy().to_string())?;

            unsafe {
                let soll_ptr = init_fn(soll_c_path.as_ptr(), false);
                if soll_ptr.is_null() {
                    return Err(anyhow!("Failed to bootstrap SOLL database"));
                }
                close_fn(soll_ptr);
            }
        }

        let live_ist_path = if is_memory || backend == crate::graph::PluginBackend::Postgres {
            None
        } else {
            let ist_path = canonical_ist_db_path(db_root)
                .ok_or_else(|| anyhow!("Failed to derive IST database path"))?;
            std::fs::create_dir_all(
                ist_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            )?;
            Some(ist_path)
        };
        let reader_db_path = if split_brain_mode {
            canonical_ist_reader_db_path(db_root)
        } else {
            live_ist_path.clone()
        };
        let db_path_str = match (&pg_database_url, &reader_db_path) {
            (Some(url), _) => url.clone(),
            (None, Some(path)) => path.to_string_lossy().to_string(),
            (None, None) => ":memory:".to_string(),
        };
        let db_path = reader_db_path.clone();
        let writer_db_path = if let Some(url) = pg_database_url.as_ref() {
            url.clone()
        } else if split_brain_mode {
            ":memory:".to_string()
        } else if let Some(path) = live_ist_path.as_ref() {
            path.to_string_lossy().to_string()
        } else {
            ":memory:".to_string()
        };
        let writer_c_path = CString::new(writer_db_path)?;
        let reader_c_path = CString::new(db_path_str.clone())?;
        // Suppress the unused-let lint; reader_c_path is consumed when
        // the duckdb path opens the read-only context further below.
        let _ = &reader_c_path;

        unsafe {
            let writer_ptr = init_fn(writer_c_path.as_ptr(), false);
            if writer_ptr.is_null() {
                return Err(anyhow!("Failed to init DuckDB Writer"));
            }

            let pool = Arc::new(LatticePool {
                lib: lib.clone(),
                symbols,
                writer_ctx: Mutex::new(writer_ptr),
                reader_ctx: Mutex::new(std::ptr::null_mut()),
            });
            let store = Self {
                pool: pool.clone(),
                db_path,
                reader_only_ist_mode: split_brain_mode,
                soll_attached: !matches!(soll_access_mode, SollAccessMode::Detached),
                soll_read_only_mode: matches!(
                    soll_access_mode,
                    SollAccessMode::ReadOnlyOrEmptySchema
                ),
                recent_write_epoch_ms: AtomicU64::new(0),
                last_reader_refresh_epoch_ms: AtomicU64::new(Self::current_epoch_ms()),
                reader_refresh_failures_total: AtomicU64::new(0),
                reader_state: crate::graph::ReaderSnapshotState::new(Self::current_epoch_ms()),
                reader_refresh_wait: Mutex::new(1),
                reader_refresh_notify: std::sync::Condvar::new(),
            };

            if backend == crate::graph::PluginBackend::Postgres {
                // MIL-AXO-015 P3 slice 3c: bootstrap the PG global
                // schema (extensions + soll layer) via the canonical
                // DDL generator. Per-project IST schemas are deferred
                // to axon_init_project (P5). The duckdb-specific
                // ATTACH / additive-schema dance is intentionally
                // skipped — those code paths emit DuckDB dialect SQL
                // (FLOAT[1024], INSTALL json, ALTER TABLE quirks) that
                // is not portable to PostgreSQL. The PG dialect
                // equivalents live in `crate::postgres::ddl`.
                store.bootstrap_global_pg_schema()?;
                info!(
                    "GraphStore startup: PostgreSQL global schema bootstrapped (CPT-AXO-039 + CPT-AXO-040 + CPT-AXO-041)."
                );
            } else if !is_memory && store.soll_attached {
                let soll_path = canonical_soll_db_path(db_root)
                    .ok_or_else(|| anyhow!("Failed to derive SOLL database path"))?;
                let attach_q = build_soll_attach_query(&soll_path, soll_access_mode);
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

            if backend == crate::graph::PluginBackend::Postgres {
                // PG runtime compatibility / per-project IST recovery
                // are owned by separate sub-phases (P3 slices 3d-3e).
                // Skipping here keeps the boot path linear without
                // emitting duckdb-shaped SQL against PostgreSQL.
            } else if split_brain_mode {
                store.ensure_additive_soll_schema()?;
                info!(
                    "GraphStore startup: split brain mode active; IST writer bootstrap skipped and SOLL writer attached separately."
                );
            } else {
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
                    let structural_graph_backlog_depth =
                        store.count_persisted_file_pending().unwrap_or(0)
                            + store.count_graph_wip_files().unwrap_or(0);
                    let graph_ready_depth = store
                        .query_count("SELECT count(*) FROM File WHERE graph_ready = TRUE AND vector_ready = FALSE")
                        .unwrap_or(0) as usize;
                    let vector_backfill_limit = startup_vector_backfill_limit(
                        structural_graph_backlog_depth,
                        graph_ready_depth,
                    );
                    match store.rebuild_file_vectorization_queue_with_limit(vector_backfill_limit) {
                        Ok(count) if count > 0 => {
                            info!(
                                "Backfilled {} file vectorization queue entries for chunk embeddings with startup floor {} while structural graph backlog remained {} and graph_ready stock was {}",
                                count, vector_backfill_limit, structural_graph_backlog_depth, graph_ready_depth
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
            }

            let reader_db_available = reader_db_exists(&store.db_path);
            let _reader_ptr = if is_memory || !reader_db_available {
                if !is_memory && !reader_db_available {
                    warn!(
                        "GraphStore startup: reader database is not materialized yet; using writer as temporary reader until the first successful reader refresh."
                    );
                }
                if split_brain_mode {
                    std::ptr::null_mut()
                } else {
                    writer_ptr
                }
            } else {
                let ptr = init_fn(reader_c_path.as_ptr(), true);
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
                *reader_guard = if store.reader_only_ist_mode {
                    _reader_ptr
                } else {
                    writer_ptr
                };
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

            if !is_memory
                && store.soll_attached
                && !cfg!(test)
                && !_reader_ptr.is_null()
                && _reader_ptr != writer_ptr
            {
                let soll_path = canonical_soll_db_path(db_root)
                    .ok_or_else(|| anyhow!("Failed to derive SOLL database path"))?;
                let attach_q = build_soll_attach_query(&soll_path, soll_access_mode);
                let r_guard = store
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                store.setup_session(*r_guard, &attach_q)?;
            }
            info!("GraphStore startup: reader session setup complete.");
            store.sync_reader_epoch_to_commit();
            if !split_brain_mode && !is_memory {
                store.refresh_reader_snapshot()?;
                info!("GraphStore startup: initial IST reader replica published.");
            }

            Ok(store)
        }
    }

    pub fn refresh_reader_snapshot(&self) -> Result<()> {
        if self.db_path.is_none() {
            self.sync_reader_epoch_to_commit();
            self.reader_state
                .refresh_inflight
                .store(false, Ordering::Release);
            return Ok(());
        }

        if cfg!(test) && !self.reader_only_ist_mode {
            self.publish_ist_reader_replica()?;
            self.sync_reader_epoch_to_commit();
            self.reader_state
                .refresh_inflight
                .store(false, Ordering::Release);
            return Ok(());
        }

        let Some(db_path) = self.db_path.as_ref() else {
            unreachable!("db_path checked above")
        };

        if !db_path.exists() {
            self.sync_reader_epoch_to_commit();
            self.reader_state
                .refresh_inflight
                .store(false, Ordering::Release);
            return Ok(());
        }

        let requested_epoch = self
            .reader_state
            .refresh_requested_epoch
            .load(Ordering::Acquire);
        let commit_epoch = self.reader_state.commit_epoch.load(Ordering::Acquire);
        let target_epoch = requested_epoch.max(commit_epoch);

        let c_path = CString::new(db_path.to_string_lossy().to_string())?;
        let refresh_result = unsafe {
            let init_fn = self.pool.symbols.init_fn;
            let close_fn = self.pool.symbols.close_fn;

            if !self.reader_only_ist_mode {
                self.publish_ist_reader_replica()?;
            }

            let new_reader = init_fn(c_path.as_ptr(), true);
            if new_reader.is_null() {
                Err(anyhow!("Failed to init refreshed DuckDB Reader"))
            } else {
                if self.soll_attached {
                    let mut soll_path = db_path
                        .parent()
                        .ok_or_else(|| anyhow!("DB parent path unavailable for reader refresh"))?
                        .to_path_buf();
                    soll_path.push("soll.db");
                    let attach_q = build_soll_attach_query(
                        &soll_path,
                        if self.soll_read_only_mode {
                            SollAccessMode::ReadOnlyOrEmptySchema
                        } else {
                            SollAccessMode::ReadWrite
                        },
                    );
                    self.setup_session(new_reader, &attach_q)?;
                }

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
        if self.reader_only_ist_mode {
            let Some(db_path) = self.db_path.as_ref() else {
                return u64::MAX;
            };
            let Ok(metadata) = std::fs::metadata(db_path) else {
                return u64::MAX;
            };
            let Ok(modified) = metadata.modified() else {
                return u64::MAX;
            };
            let Ok(age) = std::time::SystemTime::now().duration_since(modified) else {
                return u64::MAX;
            };
            return age.as_millis() as u64;
        }

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
            if self.reader_only_ist_mode && reader_db_exists(&self.db_path) {
                self.refresh_reader_snapshot()?;
                return Ok(true);
            }
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

    pub fn wait_for_reader_refresh_signal(&self, timeout: std::time::Duration) -> bool {
        let guard = self
            .reader_refresh_wait
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let observed = *guard;
        let result = self
            .reader_refresh_notify
            .wait_timeout_while(guard, timeout, |target| *target == observed);
        let (guard, _) = result.unwrap_or_else(|poison| poison.into_inner());
        *guard != observed
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

    pub(crate) fn reader_snapshot_reader_available(&self) -> bool {
        let guard = self
            .pool
            .reader_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        !(*guard).is_null()
    }

    pub(crate) fn reader_snapshot_is_writer_alias(&self) -> bool {
        let reader_guard = self
            .pool
            .reader_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let writer_guard = self
            .pool
            .writer_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        !(*reader_guard).is_null() && *reader_guard == *writer_guard
    }

    pub(crate) fn reader_snapshot_freshness_contract(&self) -> RuntimeFreshnessContract {
        let diagnostics = self.reader_snapshot_diagnostics();
        let stale_after_ms = std::env::var("AXON_IST_SNAPSHOT_STALE_AFTER_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(30_000)
            .max(1);
        let observed_age_ms = match self.reader_snapshot_age_ms() {
            u64::MAX => None,
            age => Some(age),
        };

        if !self.reader_snapshot_reader_available() {
            return RuntimeFreshnessContract::degraded(
                observed_age_ms,
                stale_after_ms,
                "ist_reader_unavailable",
            );
        }

        if self.reader_snapshot_is_writer_alias() {
            return RuntimeFreshnessContract::degraded(
                observed_age_ms,
                stale_after_ms,
                "ist_reader_aliases_writer_direct_path",
            );
        }

        if diagnostics.refresh_inflight {
            return RuntimeFreshnessContract::degraded(
                observed_age_ms,
                stale_after_ms,
                "ist_reader_refresh_inflight",
            );
        }

        if diagnostics.reader_refresh_failures_total > 0 {
            return RuntimeFreshnessContract::degraded(
                observed_age_ms,
                stale_after_ms,
                "ist_reader_refresh_failures_observed",
            );
        }

        match observed_age_ms {
            None => RuntimeFreshnessContract::unknown(
                stale_after_ms,
                "ist_snapshot_missing_refresh_timestamp",
            ),
            Some(age) if age > stale_after_ms => RuntimeFreshnessContract::stale(
                age,
                stale_after_ms,
                "ist_snapshot_age_exceeded_threshold",
            ),
            Some(_) => RuntimeFreshnessContract::fresh(stale_after_ms),
        }
    }

    fn publish_ist_reader_replica(&self) -> Result<()> {
        let Some(live_db_path) = self
            .db_path
            .as_ref()
            .and_then(|path| path.parent().map(|parent| parent.join("ist.db")))
        else {
            return Ok(());
        };
        if !live_db_path.exists() {
            return Ok(());
        }
        let Some(replica_path) = live_db_path
            .parent()
            .map(|parent| parent.join("ist-reader.db"))
        else {
            return Ok(());
        };
        let temp_path = replica_path
            .parent()
            .map(|parent| parent.join("ist-reader.publish.tmp.db"))
            .ok_or_else(|| anyhow!("Failed to derive IST reader temp replica path"))?;
        let temp_wal_path = temp_path.with_extension("db.wal");
        let temp_shm_path = temp_path.with_extension("db.shm");
        let replica_wal_path = replica_path.with_extension("db.wal");
        let replica_shm_path = replica_path.with_extension("db.shm");

        unsafe {
            let exec_fn = self.pool.symbols.exec_fn;
            let writer_guard = self
                .pool
                .writer_ctx
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !exec_fn(*writer_guard, CString::new("CHECKPOINT;")?.as_ptr()) {
                return Err(anyhow!(
                    "Failed to checkpoint IST before publishing reader replica"
                ));
            }
        }

        let _ = remove_path_if_exists(&temp_path);
        let _ = remove_path_if_exists(&temp_wal_path);
        let _ = remove_path_if_exists(&temp_shm_path);
        std::fs::copy(&live_db_path, &temp_path)?;
        let _ = remove_path_if_exists(&replica_path);
        let _ = remove_path_if_exists(&replica_wal_path);
        let _ = remove_path_if_exists(&replica_shm_path);
        std::fs::rename(&temp_path, &replica_path)?;
        Ok(())
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
        let duckdb_memory_limit_gb = std::env::var("AXON_DUCKDB_MEMORY_LIMIT_GB")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(0);
        unsafe {
            let exec_fn = self.pool.symbols.exec_fn;
            exec_fn(ctx, CString::new("INSTALL json; LOAD json;")?.as_ptr());
            // DEC-AXO-072 follow-up: vector pipeline profiling (2026-05-05)
            // showed the Writer Actor commit_ms growing from 132ms to 12298ms
            // (100x slowdown) over 80s on the Axon repo because the prior
            // `SET checkpoint_threshold = '1GB'` lets the WAL accumulate to
            // ~1 GB before compaction, dragging every subsequent commit/SELECT
            // through ever-longer WAL replay. Lowering the threshold to 64MB
            // forces ~16x more checkpoints (each cheap) but caps the per-op
            // cost. Standard for OLTP-heavy DuckDB workloads.
            exec_fn(
                ctx,
                CString::new("SET checkpoint_threshold = '64MB';")?.as_ptr(),
            );
            if duckdb_memory_limit_gb > 0 {
                exec_fn(
                    ctx,
                    CString::new(format!(
                        "SET memory_limit = '{}GB';",
                        duckdb_memory_limit_gb
                    ))?
                    .as_ptr(),
                );
            }
            if !attach_query.is_empty() {
                exec_fn(ctx, CString::new(attach_query)?.as_ptr());
            }
            Ok(())
        }
    }

    fn find_plugin_path() -> Result<String> {
        Self::find_plugin_path_for(crate::graph::PluginBackend::current())
    }

    /// MIL-AXO-015 P3 slice 3b: backend-aware plugin discovery.
    /// Picks `libaxon_plugin_duckdb.so` or `libaxon_plugin_postgres.so`
    /// based on `AXON_DB_BACKEND`, defaulting to duckdb when unset.
    fn find_plugin_path_for(backend: crate::graph::PluginBackend) -> Result<String> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| anyhow!("Unable to resolve repository root from CARGO_MANIFEST_DIR"))?
            .to_path_buf();

        let crate_dir = backend.crate_dir();
        let so = backend.plugin_filename();

        let mut candidates = vec![
            repo_root.join(format!("{crate_dir}/target/release/{so}")),
            repo_root.join(format!("{crate_dir}/target/debug/{so}")),
        ];

        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(format!("{crate_dir}/target/release/{so}")));
            candidates.push(cwd.join(format!("{crate_dir}/target/debug/{so}")));
        }

        for path in candidates {
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
        Err(anyhow!(
            "Plugin not found for backend {:?} (expected {})",
            backend,
            so
        ))
    }

    /// MIL-AXO-015 P3 slice 3c: PostgreSQL global schema bootstrap.
    /// Idempotent. Executes the canonical DDL produced by
    /// `crate::postgres::ddl::generate_global_schema` (extensions +
    /// public.ProjectCodeRegistry + soll layer + cross-project
    /// indexes). Per-project IST schemas are created lazily by
    /// `axon_init_project` (P5).
    ///
    /// `CREATE EXTENSION` statements are run inside a graceful-degrade
    /// loop: if an extension is unavailable on the host PostgreSQL
    /// install (the image lacks AGE or pgvector), the bootstrap logs a
    /// warning and continues so the SOLL layer still comes up. Per
    /// DEC-AXO-075, production deployments MUST ship both extensions —
    /// the warning is the operator's signal to fix the install.
    ///
    /// Slice 5b: when `AXON_SOLL_SEED_PATH` points at a JSON seed and
    /// `soll.Node` is empty, load the snapshot via
    /// `crate::postgres::seed::load_seed_if_needed` so fresh
    /// deployments come up with canonical SOLL nodes preloaded.
    fn bootstrap_global_pg_schema(&self) -> Result<()> {
        for stmt in crate::postgres::ddl::generate_global_schema() {
            let trimmed = stmt.trim_start();
            let is_optional_extension = trimmed
                .to_uppercase()
                .starts_with("CREATE EXTENSION IF NOT EXISTS");
            match self.execute(&stmt) {
                Ok(()) => {}
                Err(err) if is_optional_extension => {
                    warn!(
                        statement = stmt.chars().take(80).collect::<String>().as_str(),
                        error = %err,
                        "PostgreSQL extension unavailable on this host; continuing without it. \
                         Install the extension to unlock dependent features (DEC-AXO-075)."
                    );
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "PostgreSQL global schema bootstrap failed on statement: {}",
                            stmt.chars().take(80).collect::<String>()
                        )
                    });
                }
            }
        }

        if let Ok(seed_path) = std::env::var("AXON_SOLL_SEED_PATH") {
            if !seed_path.trim().is_empty() {
                let path = std::path::Path::new(seed_path.trim());
                match crate::postgres::seed::load_seed_if_needed(self, path) {
                    Ok(0) => {
                        info!(
                            seed_path = seed_path.as_str(),
                            "SOLL seed loader: nothing to load (file missing or SOLL non-empty)."
                        );
                    }
                    Ok(n) => {
                        info!(
                            seed_path = seed_path.as_str(),
                            inserted = n,
                            "SOLL seed loaded into fresh PostgreSQL deployment."
                        );
                    }
                    Err(err) => {
                        warn!(
                            seed_path = seed_path.as_str(),
                            error = %err,
                            "SOLL seed loader failed; brain is starting with whatever \
                             SOLL state currently exists. Re-run after fixing the seed file."
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn init_schema(&self, _is_memory: bool) -> Result<()> {
        self.execute(
            "CREATE TABLE IF NOT EXISTS RuntimeMetadata (key VARCHAR PRIMARY KEY, value VARCHAR)",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_code VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE, first_seen_at_ms BIGINT, indexing_started_at_ms BIGINT, graph_ready_at_ms BIGINT, vectorization_started_at_ms BIGINT, vector_ready_at_ms BIGINT, last_state_change_at_ms BIGINT, last_error_at_ms BIGINT)")?;
        // REQ-AXO-289 S2 — minimal watcher filter table for streaming v2.
        // path PK + content_hash + last_seen_ms only. NO status machine.
        // Coexists with the legacy `File` table until slice S7 cut-over.
        self.execute("CREATE TABLE IF NOT EXISTS IndexedFile (path VARCHAR PRIMARY KEY, content_hash VARCHAR NOT NULL, last_seen_ms BIGINT NOT NULL)")?;
        self.execute(&format!("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, project_code VARCHAR, embedding FLOAT[{DIMENSION}])"))?;
        self.execute("CREATE TABLE IF NOT EXISTS Chunk (id VARCHAR PRIMARY KEY, source_type VARCHAR, source_id VARCHAR, project_code VARCHAR, file_path VARCHAR, kind VARCHAR, content VARCHAR, content_hash VARCHAR, start_line BIGINT, end_line BIGINT, chunk_part_index BIGINT, chunk_part_count BIGINT, chunk_path VARCHAR)")?;
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
        if self.soll_attached && !self.soll_read_only_mode {
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
        }
        self.execute("CREATE TABLE IF NOT EXISTS FileLifecycleEvent (file_path VARCHAR, project_code VARCHAR, stage VARCHAR, status VARCHAR, reason VARCHAR, at_ms BIGINT, worker_id BIGINT, trace_id VARCHAR, run_id VARCHAR)")?;
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
        self.execute("CREATE TABLE IF NOT EXISTS FileVectorizationQueue (file_path VARCHAR PRIMARY KEY, status VARCHAR DEFAULT 'queued', status_reason VARCHAR, attempts BIGINT DEFAULT 0, queued_at BIGINT, last_error_reason VARCHAR, last_attempt_at BIGINT, next_eligible_at_ms BIGINT, interactive_pause_count BIGINT DEFAULT 0, claim_token VARCHAR, claimed_at_ms BIGINT, lease_heartbeat_at_ms BIGINT, lease_owner VARCHAR, lease_epoch BIGINT DEFAULT 0, persist_started_at_ms BIGINT DEFAULT NULL)")?;
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
        let _ = self.execute("DROP INDEX IF EXISTS file_project_path_idx");

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
        self.execute("ALTER TABLE Chunk ADD COLUMN IF NOT EXISTS chunk_part_index BIGINT")?;
        self.execute("ALTER TABLE Chunk ADD COLUMN IF NOT EXISTS chunk_part_count BIGINT")?;
        self.execute("ALTER TABLE Chunk ADD COLUMN IF NOT EXISTS chunk_path VARCHAR")?;
        // DEC-AXO-074 Direction A: tracks whether Chunk.content has been
        // archived to the Parquet side-store and cleared from DuckDB. The
        // background archiver flips this to TRUE after a successful Parquet
        // append; retrieve_context COALESCEs DuckDB content with parquet_scan
        // so archived rows still surface their content (M.3b).
        // No DEFAULT clause — DuckDB rejects defaults on ALTER TABLE when
        // dependent indexes exist. Queries use COALESCE(c.content_archived, FALSE).
        self.execute("ALTER TABLE Chunk ADD COLUMN IF NOT EXISTS content_archived BOOLEAN")?;
        self.execute("CREATE TABLE IF NOT EXISTS FileLifecycleEvent (file_path VARCHAR, project_code VARCHAR, stage VARCHAR, status VARCHAR, reason VARCHAR, at_ms BIGINT, worker_id BIGINT, trace_id VARCHAR, run_id VARCHAR)")?;
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
        self.execute(
            "ALTER TABLE FileVectorizationQueue ADD COLUMN IF NOT EXISTS persist_started_at_ms BIGINT DEFAULT NULL",
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

        // REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): composite (project_code, key)
        // indexes for multi-tenant lookups on hot IST tables.
        self.execute("CREATE INDEX IF NOT EXISTS calls_project_source_idx ON CALLS(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_project_target_idx ON CALLS(project_code, target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_nif_project_source_idx ON CALLS_NIF(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS calls_nif_project_target_idx ON CALLS_NIF(project_code, target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS contains_project_source_idx ON CONTAINS(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS contains_project_target_idx ON CONTAINS(project_code, target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS impacts_project_source_idx ON IMPACTS(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS substantiates_project_source_idx ON SUBSTANTIATES(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS symbol_project_id_idx ON Symbol(project_code, id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS file_project_path_idx ON File(project_code, path)")?;

        Ok(())
    }

    fn ensure_additive_soll_schema(&self) -> Result<()> {
        if !self.soll_attached || self.soll_read_only_mode {
            return Ok(());
        }

        self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (project_code VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL', id VARCHAR DEFAULT 'AXON_GLOBAL', last_vis BIGINT DEFAULT 0, last_pil BIGINT DEFAULT 0, last_req BIGINT DEFAULT 0, last_cpt BIGINT DEFAULT 0, last_dec BIGINT DEFAULT 0, last_mil BIGINT DEFAULT 0, last_val BIGINT DEFAULT 0, last_stk BIGINT DEFAULT 0, last_gui BIGINT DEFAULT 0, last_prv BIGINT DEFAULT 0, last_rev BIGINT DEFAULT 0)")?;
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
        // REQ-AXO-143 — per-project session pointer (file|url|soll_node|none).
        // Stored as serialized JSON object: {kind, value, label?}.
        // NULL when the project does not declare a session-pointer convention.
        self.execute(
            "ALTER TABLE soll.ProjectCodeRegistry ADD COLUMN IF NOT EXISTS session_pointer_json VARCHAR",
        )?;
        self.normalize_project_code_registry_schema()?;
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

        // REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): denormalize project_code on
        // the remaining SOLL tables so per-tenant filtering does not require
        // joining soll.Node every time.
        self.execute("ALTER TABLE soll.Edge ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE soll.McpJob ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute("ALTER TABLE soll.Revision ADD COLUMN IF NOT EXISTS project_code VARCHAR")?;
        self.execute(
            "ALTER TABLE soll.RevisionChange ADD COLUMN IF NOT EXISTS project_code VARCHAR",
        )?;

        // Backfill is idempotent: edges inherit from the source Node when known,
        // everything else falls back to 'AXO' since pre-Phase-1 rows predate
        // multi-tenant scoping (single-project history).
        self.execute(
            "UPDATE soll.Edge SET project_code = COALESCE(
                NULLIF(soll.Edge.project_code, ''),
                (SELECT n.project_code FROM soll.Node n WHERE n.id = soll.Edge.source_id),
                'AXO'
            ) WHERE soll.Edge.project_code IS NULL OR soll.Edge.project_code = ''",
        )?;
        // DuckDB upstream issue #15836: UPDATE on a primary-keyed row
        // internally does DELETE+INSERT. For soll.McpJob's legacy rows that
        // were committed under different transaction shapes, this corrupts
        // the PK index — once corrupted, the UPDATE crashes the brain on
        // every boot AND a plain DELETE fails too ("Failed to delete all
        // rows from index. Only deleted 0 out of 4 rows."). Skip the
        // backfill when legacy NULL rows exist; emit a warning so we know
        // the table still needs migration. Proper fix: CTAS rebuild of
        // soll.McpJob OR bumping the bundled DuckDB to a version that ships
        // #15836's patch. Boot stays unblocked either way.
        let mcp_job_needs_backfill: i64 = self.query_count(
            "SELECT count(*) FROM soll.McpJob WHERE project_code IS NULL OR project_code = ''",
        )?;
        if mcp_job_needs_backfill > 0 {
            tracing::warn!(
                count = mcp_job_needs_backfill,
                reason = "duckdb_15836_workaround",
                "soll_mcpjob_backfill_skipped: legacy rows with NULL project_code retained to avoid PK-index corruption on UPDATE; CTAS rebuild required for proper migration"
            );
        }
        self.execute("UPDATE soll.Revision SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;
        self.execute("UPDATE soll.RevisionChange SET project_code = 'AXO' WHERE project_code IS NULL OR project_code = ''")?;

        // Composite (project_code, key) indexes for hot SOLL multi-tenant lookups.
        self.execute(
            "CREATE INDEX IF NOT EXISTS soll_node_project_id_idx ON soll.Node(project_code, id)",
        )?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_project_source_idx ON soll.Edge(project_code, source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_edge_project_target_idx ON soll.Edge(project_code, target_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_mcp_job_project_status_idx ON soll.McpJob(project_code, status)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_revision_project_idx ON soll.Revision(project_code, created_at)")?;
        self.execute("CREATE INDEX IF NOT EXISTS soll_revision_change_project_idx ON soll.RevisionChange(project_code, revision_id)")?;

        self.normalize_project_code_registry()?;
        self.seed_project_code_registry()?;
        self.normalize_soll_registry()?;
        self.normalize_revision_preview_schema()?;
        self.seed_global_guidelines()?;
        Ok(())
    }

    fn normalize_soll_registry(&self) -> Result<()> {
        let raw = self.query_json("SELECT * FROM pragma_table_info('soll.Registry')")?;
        let columns: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let has_project_code = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_code"))
                .unwrap_or(false)
        });
        let has_project_slug = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_slug"))
                .unwrap_or(false)
        });

        if !has_project_code {
            return Err(anyhow!(
                "Legacy soll.Registry schema detected: missing canonical project_code column"
            ));
        }
        if has_project_slug {
            return Err(anyhow!(
                "Legacy soll.Registry schema detected: forbidden project_slug column still present"
            ));
        }

        let raw_rows = self.query_json(
            "SELECT
                COALESCE(NULLIF(TRIM(project_code), ''), ''),
                COALESCE(id, 'AXON_GLOBAL')
             FROM soll.Registry",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw_rows).unwrap_or_default();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let project_code = row[0].trim();
            if project_code.is_empty() || !crate::project_meta::is_valid_project_code(project_code)
            {
                return Err(anyhow!(
                    "Invalid project_code in soll.Registry: {}",
                    project_code
                ));
            }
            let resolved = self.query_count(&format!(
                "SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
                project_code.replace('\'', "''")
            ))?;
            if resolved == 0 {
                return Err(anyhow!(
                    "Unknown project_code in soll.Registry: {}",
                    project_code
                ));
            }
        }
        Ok(())
    }

    fn normalize_revision_preview_schema(&self) -> Result<()> {
        let raw = self.query_json("SELECT * FROM pragma_table_info('soll.RevisionPreview')")?;
        let columns: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let has_project_code = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_code"))
                .unwrap_or(false)
        });
        let has_project_slug = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_slug"))
                .unwrap_or(false)
        });

        if !has_project_code {
            return Err(anyhow!(
                "Legacy soll.RevisionPreview schema detected: missing canonical project_code column"
            ));
        }
        if has_project_slug {
            return Err(anyhow!(
                "Legacy soll.RevisionPreview schema detected: forbidden project_slug column still present"
            ));
        }

        let raw_rows = self.query_json(
            "SELECT
                preview_id,
                COALESCE(author, ''),
                COALESCE(NULLIF(TRIM(project_code), ''), ''),
                COALESCE(payload, ''),
                COALESCE(created_at, 0)
             FROM soll.RevisionPreview
             ORDER BY created_at ASC, preview_id ASC",
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw_rows).unwrap_or_default();

        for row in rows {
            if row.len() < 5 {
                continue;
            }
            let preview_id = row[0].trim();
            if preview_id.is_empty() {
                continue;
            }
            let project_code = row[2].trim();
            if project_code.is_empty() || !crate::project_meta::is_valid_project_code(project_code)
            {
                return Err(anyhow!(
                    "Invalid project_code in soll.RevisionPreview: {}",
                    project_code
                ));
            }
            let resolved = self.query_count(&format!(
                "SELECT count(*) FROM soll.ProjectCodeRegistry WHERE project_code = '{}'",
                project_code.replace('\'', "''")
            ))?;
            if resolved == 0 {
                return Err(anyhow!(
                    "Unknown project_code in soll.RevisionPreview: {}",
                    project_code
                ));
            }

            if let Some((_, preview_code, _)) = parse_prefixed_entity_id(preview_id) {
                if preview_code != project_code {
                    return Err(anyhow!(
                        "RevisionPreview project_code mismatch: preview_id={} project_code={}",
                        preview_id,
                        project_code
                    ));
                }
            }
        }
        Ok(())
    }

    fn normalize_project_code_registry_schema(&self) -> Result<()> {
        let raw = self.query_json("SELECT * FROM pragma_table_info('soll.ProjectCodeRegistry')")?;
        let columns: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let has_project_code = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_code"))
                .unwrap_or(false)
        });
        let has_project_name = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_name"))
                .unwrap_or(false)
        });
        let has_project_path = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_path"))
                .unwrap_or(false)
        });
        let has_legacy_slug = columns.iter().any(|row| {
            row.get(1)
                .map(|value| value.eq_ignore_ascii_case("project_slug"))
                .unwrap_or(false)
        });

        if !has_project_code || !has_project_name || !has_project_path {
            return Err(anyhow!(
                "Legacy soll.ProjectCodeRegistry schema detected: canonical columns are incomplete"
            ));
        }
        if has_legacy_slug {
            return Err(anyhow!(
                "Legacy soll.ProjectCodeRegistry schema detected: forbidden project_slug column still present"
            ));
        }
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

    /// REQ-AXO-143 — persist a project's session pointer (file|url|soll_node|none).
    /// `pointer` is the canonical JSON object `{kind, value, label?}` or `None`
    /// to clear the field. Idempotent.
    pub(crate) fn write_session_pointer(
        &self,
        project_code: &str,
        pointer: Option<&serde_json::Value>,
    ) -> Result<()> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        if normalized_code.is_empty()
            || !crate::project_meta::is_valid_project_code(&normalized_code)
        {
            return Ok(());
        }
        let serialized = pointer
            .map(serde_json::Value::to_string)
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null);
        self.execute_param(
            "UPDATE soll.ProjectCodeRegistry SET session_pointer_json = ? WHERE project_code = ?",
            &serde_json::json!([serialized, normalized_code]),
        )?;
        Ok(())
    }

    /// REQ-AXO-143 — read a project's session pointer; returns `None` when
    /// the column is NULL or carries an unparseable string.
    pub(crate) fn read_session_pointer(
        &self,
        project_code: &str,
    ) -> Result<Option<serde_json::Value>> {
        let normalized_code = project_code.trim().to_ascii_uppercase();
        if normalized_code.is_empty() {
            return Ok(None);
        }
        let raw = self.query_json_param(
            "SELECT COALESCE(session_pointer_json, '') FROM soll.ProjectCodeRegistry WHERE project_code = ? LIMIT 1",
            &serde_json::json!([normalized_code]),
        )?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let payload = row.first().map(String::as_str).unwrap_or("").trim();
        if payload.is_empty() {
            return Ok(None);
        }
        Ok(serde_json::from_str::<serde_json::Value>(payload).ok())
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
        // REQ-AXO-248 / MIL-AXO-015 B.2 slice S4: under PG, also clear
        // the Apache AGE graph so post-reset reads return empty rather
        // than the pre-reset shadow. `MATCH (n) DETACH DELETE n` removes
        // every vertex + edge in axon_graph in one statement. Best-
        // effort under dual-write: failure logs at warn but does not
        // abort the SQL reset (reset is the operator's nuclear option).
        if self.is_postgres_backend() {
            let clear = "SELECT * FROM cypher('axon_graph', $$ MATCH (n) DETACH DELETE n RETURN 1 $$) AS (_ag_void agtype)";
            if let Err(e) = self.execute(clear) {
                log::warn!(
                    "AGE graph clear failed during reset_ist_state (SQL reset will still proceed): {}",
                    e
                );
            }
        }

        let mut cleanup_queries: Vec<&str> = Vec::with_capacity(8);
        // SQL relation tables — always under DuckDB; gated under PG
        // (skipped when the operator has flipped AXON_AGE_ONLY_RELATIONS
        // before running a reset, which is the post-Stop A state).
        let skip_sql_relations = self.is_postgres_backend()
            && crate::postgres::age::age_only_relations_enabled();
        if !skip_sql_relations {
            cleanup_queries.extend([
                "DELETE FROM CALLS_NIF",
                "DELETE FROM CALLS",
                "DELETE FROM CONTAINS",
                "DELETE FROM IMPACTS",
                "DELETE FROM SUBSTANTIATES",
            ]);
        }
        // IST tables (Symbol/Chunk/Project) always SQL on both backends.
        cleanup_queries.extend([
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "DELETE FROM Project",
        ]);

        for query in cleanup_queries {
            self.execute(query)?;
        }

        self.rebuild_graph_projection_runtime_tables()?;
        self.rebuild_embedding_runtime_tables()?;

        self.execute("DROP TABLE IF EXISTS File;")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_code VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE, first_seen_at_ms BIGINT, indexing_started_at_ms BIGINT, graph_ready_at_ms BIGINT, vectorization_started_at_ms BIGINT, vector_ready_at_ms BIGINT, last_state_change_at_ms BIGINT, last_error_at_ms BIGINT)",
        )?;

        // The DROP+CREATE above discards every index on File. Recreate the
        // multi-tenant indexes (REQ-AXO-066 Phase 1, DEC-AXO-064 Option A) so
        // post-hard-rebuild reads stay scale-correct.
        self.execute("CREATE INDEX IF NOT EXISTS file_project_code_idx ON File(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS file_status_idx ON File(status)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS file_project_path_idx ON File(project_code, path)",
        )?;

        info!("IST state reset complete. SOLL sanctuary preserved.");
        Ok(())
    }

    fn soft_invalidate_derived_state(&self) -> Result<()> {
        // REQ-AXO-248 / MIL-AXO-015 B.2 slice S4: same AGE clear as
        // reset_ist_state (above). Soft-invalidate keeps the File
        // backlog so post-clear ingestion replays everything against
        // a fresh AGE graph.
        if self.is_postgres_backend() {
            let clear = "SELECT * FROM cypher('axon_graph', $$ MATCH (n) DETACH DELETE n RETURN 1 $$) AS (_ag_void agtype)";
            if let Err(e) = self.execute(clear) {
                log::warn!(
                    "AGE graph clear failed during soft_invalidate_derived_state (SQL clear will still proceed): {}",
                    e
                );
            }
        }

        let skip_sql_relations = self.is_postgres_backend()
            && crate::postgres::age::age_only_relations_enabled();
        let mut cleanup_queries: Vec<&str> = Vec::with_capacity(8);
        if !skip_sql_relations {
            cleanup_queries.extend([
                "DELETE FROM CALLS_NIF",
                "DELETE FROM CALLS",
                "DELETE FROM CONTAINS",
                "DELETE FROM IMPACTS",
                "DELETE FROM SUBSTANTIATES",
            ]);
        }
        cleanup_queries.extend([
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "UPDATE File SET status = 'pending', worker_id = NULL, needs_reindex = FALSE, status_reason = 'soft_invalidated', file_stage = 'promoted', graph_ready = FALSE, vector_ready = FALSE",
        ]);

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

        // The DROP+RENAME above discards every index on File. Recreate the
        // multi-tenant indexes (REQ-AXO-066 Phase 1, DEC-AXO-064 Option A) so
        // post-soft-invalidation reads stay scale-correct.
        self.execute("CREATE INDEX IF NOT EXISTS file_project_code_idx ON File(project_code)")?;
        self.execute("CREATE INDEX IF NOT EXISTS file_status_idx ON File(status)")?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS file_project_path_idx ON File(project_code, path)",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod graph_bootstrap_tests {
    use super::{
        canonical_ist_reader_db_path, reader_db_exists, startup_vector_backfill_limit, GraphStore,
        STARTUP_SEMANTIC_BACKFILL_FLOOR,
    };
    use crate::embedding_contract::{
        CHUNK_MODEL_ID, DIMENSION, GRAPH_MODEL_ID, MODEL_NAME, MODEL_VERSION,
    };
    use crate::tests::test_helpers::create_test_db;
    use std::path::PathBuf;
    use tempfile::tempdir;

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
    fn test_normalize_soll_registry_accepts_canonical_schema() {
        let store = create_test_db().unwrap();

        store.normalize_soll_registry().unwrap();
    }

    #[test]
    fn test_normalize_soll_registry_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store.execute("DROP TABLE soll.Registry").unwrap();
        store
            .execute(
                "CREATE TABLE soll.Registry (
                project_slug VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL',
                id VARCHAR DEFAULT 'AXON_GLOBAL',
                last_req BIGINT DEFAULT 0,
                last_cpt BIGINT DEFAULT 0,
                last_dec BIGINT DEFAULT 0,
                last_mil BIGINT DEFAULT 0,
                last_val BIGINT DEFAULT 0,
                last_pil BIGINT DEFAULT 0,
                last_vis BIGINT DEFAULT 0,
                last_stk BIGINT DEFAULT 0,
                last_prv BIGINT DEFAULT 0,
                last_rev BIGINT DEFAULT 0,
                last_gui BIGINT DEFAULT 0
            )",
            )
            .unwrap();

        let err = store.normalize_soll_registry().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.Registry schema detected"));
    }

    #[test]
    fn test_normalize_project_code_registry_schema_accepts_canonical_schema() {
        let store = create_test_db().unwrap();
        store.normalize_project_code_registry_schema().unwrap();
    }

    #[test]
    fn test_indexer_store_can_boot_while_brain_holds_soll_writer() {
        let temp = tempdir().unwrap();
        let db_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&db_root).unwrap();
        let db_root_str = db_root.to_string_lossy().to_string();

        let brain = GraphStore::new_brain_reader_soll_writer(&db_root_str).unwrap();
        brain
            .execute(
                "INSERT INTO soll.ProjectCodeRegistry (project_code, project_name, project_path)
                 VALUES ('AXO', 'Axon', '/home/dstadel/projects/axon')
                 ON CONFLICT (project_code) DO NOTHING",
            )
            .unwrap();

        let indexer = GraphStore::new_indexer_ist_writer_without_soll(&db_root_str).unwrap();
        assert!(!indexer.soll_attached);
        indexer
            .execute(
                "INSERT INTO File (path, project_code, status, size, priority, mtime)
                 VALUES ('/tmp/indexer.txt', 'AXO', 'pending', 1, 1, 1)",
            )
            .unwrap();
        indexer.refresh_reader_snapshot().unwrap();
        let reader_db = canonical_ist_reader_db_path(&db_root_str).unwrap();
        assert!(
            reader_db.exists(),
            "reader replica should exist without SOLL"
        );
    }

    #[test]
    fn test_indexer_publishes_ist_reader_replica_for_brain_reads() {
        let temp = tempdir().unwrap();
        let db_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&db_root).unwrap();
        let db_root_str = db_root.to_string_lossy().to_string();

        let indexer = GraphStore::new(&db_root_str).unwrap();
        indexer
            .execute(
                "INSERT INTO File (path, project_code, status, size, priority, mtime)
                 VALUES ('/tmp/demo.txt', 'AXO', 'pending', 1, 1, 1)",
            )
            .unwrap();
        indexer.mark_writer_commit_visible();
        indexer.refresh_reader_snapshot().unwrap();

        let reader_db = canonical_ist_reader_db_path(&db_root_str).unwrap();
        assert!(reader_db.exists(), "reader replica should exist");

        let brain = GraphStore::new_brain_reader_soll_writer(&db_root_str).unwrap();
        let raw = brain
            .query_json_on_reader("SELECT count(*) FROM File")
            .unwrap();
        assert!(raw.contains("1"), "{raw}");
        assert!(matches!(
            brain.reader_snapshot_freshness_contract().state,
            crate::runtime_truth_contract::RuntimeFreshnessState::Fresh
        ));
    }

    #[test]
    fn test_reader_replica_publish_reuses_path_when_duckdb_temp_dir_exists() {
        let temp = tempdir().unwrap();
        let db_root = temp.path().join("graph_v2");
        std::fs::create_dir_all(&db_root).unwrap();
        let db_root_str = db_root.to_string_lossy().to_string();

        let indexer = GraphStore::new_indexer_ist_writer_without_soll(&db_root_str).unwrap();
        indexer
            .execute(
                "INSERT INTO File (path, project_code, status, size, priority, mtime)
                 VALUES ('/tmp/demo.txt', 'AXO', 'pending', 1, 1, 1)",
            )
            .unwrap();
        let conflicting_temp_dir = db_root.join("ist-reader.db.tmp");
        std::fs::create_dir_all(&conflicting_temp_dir).unwrap();

        indexer.refresh_reader_snapshot().unwrap();

        let reader_db = canonical_ist_reader_db_path(&db_root_str).unwrap();
        assert!(reader_db.exists(), "reader replica should exist");
        assert!(
            conflicting_temp_dir.is_dir(),
            "legacy duckdb temp directory should not block replica publication"
        );
    }

    #[test]
    fn test_normalize_project_code_registry_schema_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store
            .execute("DROP TABLE soll.ProjectCodeRegistry")
            .unwrap();
        store
            .execute(
                "CREATE TABLE soll.ProjectCodeRegistry (
                project_code VARCHAR PRIMARY KEY,
                project_slug VARCHAR,
                project_path VARCHAR,
                project_name VARCHAR
            )",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.ProjectCodeRegistry (project_code, project_slug, project_path, project_name)
                 VALUES ('AXO', 'axon', '/home/dstadel/projects/axon', 'Axon')",
            )
            .unwrap();

        let err = store.normalize_project_code_registry_schema().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.ProjectCodeRegistry schema detected"));
    }

    #[test]
    fn test_normalize_revision_preview_schema_accepts_canonical_schema() {
        let store = create_test_db().unwrap();
        store.normalize_revision_preview_schema().unwrap();
    }

    #[test]
    fn test_normalize_revision_preview_schema_rejects_legacy_slug_column() {
        let store = create_test_db().unwrap();
        store.execute("DROP TABLE soll.RevisionPreview").unwrap();
        store
            .execute(
                "CREATE TABLE soll.RevisionPreview (
                preview_id VARCHAR PRIMARY KEY,
                author VARCHAR,
                project_slug VARCHAR,
                payload VARCHAR,
                created_at BIGINT
            )",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.RevisionPreview (preview_id, author, project_slug, payload, created_at)
                 VALUES ('PRV-HYD-002', 'unknown', 'HydraDB', '{}', 42)",
            )
            .unwrap();

        let err = store.normalize_revision_preview_schema().unwrap_err();
        assert!(err
            .to_string()
            .contains("Legacy soll.RevisionPreview schema detected"));
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

    #[test]
    fn startup_vector_backfill_limit_keeps_vector_startup_bounded_by_graph_ready_stock() {
        assert_eq!(startup_vector_backfill_limit(0, 0), 0);
        assert_eq!(startup_vector_backfill_limit(0, 1), 1);
        assert_eq!(startup_vector_backfill_limit(1, 1), 1);
        assert_eq!(
            startup_vector_backfill_limit(0, 512),
            STARTUP_SEMANTIC_BACKFILL_FLOOR
        );
        assert_eq!(
            startup_vector_backfill_limit(512, 512),
            STARTUP_SEMANTIC_BACKFILL_FLOOR
        );
    }

    #[test]
    fn reader_db_exists_only_when_physical_db_path_is_present() {
        assert!(!reader_db_exists(&None));
        assert!(!reader_db_exists(&Some(PathBuf::from(
            "/tmp/axon-missing-reader-db-test.db"
        ))));
    }

    // REQ-AXO-066 Phase 1 (DEC-AXO-064 Option A): two projects coexist in the
    // shared SOLL store and remain semantically isolated under project_code
    // filters; the composite multi-tenant indexes are present after bootstrap.
    #[test]
    fn test_two_projects_are_semantically_isolated_in_soll() {
        let store = create_test_db().unwrap();

        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('REQ-AXO-90001', 'Requirement', 'AXO', 'AXO smoke', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('REQ-BKS-90001', 'Requirement', 'BKS', 'BKS smoke', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('CPT-AXO-90001', 'Concept', 'AXO', 'AXO concept', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Node (id, type, project_code, title, description, status, metadata)
                 VALUES ('CPT-BKS-90001', 'Concept', 'BKS', 'BKS concept', 'd', 'planned', '{}')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code)
                 VALUES ('REQ-AXO-90001', 'CPT-AXO-90001', 'BELONGS_TO', '{}', 'AXO')",
            )
            .unwrap();
        store
            .execute(
                "INSERT INTO soll.Edge (source_id, target_id, relation_type, metadata, project_code)
                 VALUES ('REQ-BKS-90001', 'CPT-BKS-90001', 'BELONGS_TO', '{}', 'BKS')",
            )
            .unwrap();

        let axo_nodes = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'AXO' AND id LIKE '%-90001'",
            )
            .unwrap();
        let bks_nodes = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'BKS' AND id LIKE '%-90001'",
            )
            .unwrap();
        assert_eq!(axo_nodes, 2, "AXO scope must see exactly 2 seeded nodes");
        assert_eq!(bks_nodes, 2, "BKS scope must see exactly 2 seeded nodes");

        // Cross-project leak: AXO scope must never expose BKS rows.
        let axo_seeing_bks = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'AXO' AND id LIKE '%-BKS-%'",
            )
            .unwrap();
        let bks_seeing_axo = store
            .query_count(
                "SELECT count(*) FROM soll.Node WHERE project_code = 'BKS' AND id LIKE '%-AXO-%'",
            )
            .unwrap();
        assert_eq!(axo_seeing_bks, 0, "AXO scope leaked BKS rows");
        assert_eq!(bks_seeing_axo, 0, "BKS scope leaked AXO rows");

        // Edge.project_code denormalization works under per-tenant filter.
        let axo_edges = store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE project_code = 'AXO' AND source_id = 'REQ-AXO-90001'",
            )
            .unwrap();
        let bks_edges = store
            .query_count(
                "SELECT count(*) FROM soll.Edge WHERE project_code = 'BKS' AND source_id = 'REQ-BKS-90001'",
            )
            .unwrap();
        assert_eq!(axo_edges, 1);
        assert_eq!(bks_edges, 1);

        // Composite indexes from REQ-AXO-066 Phase 1 are registered by bootstrap.
        let raw = store
            .query_json(
                "SELECT index_name FROM duckdb_indexes()
                 WHERE schema_name = 'main'
                   AND index_name IN (
                       'soll_node_project_id_idx',
                       'soll_edge_project_source_idx',
                       'soll_edge_project_target_idx',
                       'soll_mcp_job_project_status_idx',
                       'soll_revision_project_idx',
                       'soll_revision_change_project_idx',
                       'symbol_project_id_idx',
                       'calls_project_source_idx',
                       'file_project_path_idx'
                   )
                 ORDER BY index_name",
            )
            .unwrap();
        let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap();
        let names: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .collect();
        for expected in [
            "calls_project_source_idx",
            "file_project_path_idx",
            "soll_edge_project_source_idx",
            "soll_edge_project_target_idx",
            "soll_mcp_job_project_status_idx",
            "soll_node_project_id_idx",
            "soll_revision_change_project_idx",
            "soll_revision_project_idx",
            "symbol_project_id_idx",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing composite multi-tenant index `{expected}`; present: {names:?}"
            );
        }
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

#[cfg(test)]
mod tests;
