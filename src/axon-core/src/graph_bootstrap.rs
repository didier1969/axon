use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use libloading::{Library, Symbol as LibSymbol};
use tracing::{info, warn};

use crate::graph::{CloseDbFunc, ExecFunc, GraphStore, InitDbFunc, LatticePool};

const IST_SCHEMA_VERSION: &str = "3";
const IST_INGESTION_VERSION: &str = "3";
const IST_EMBEDDING_VERSION: &str = "1";
const GRAPH_MODEL_ID: &str = "graph-bge-small-en-v1.5-384";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IstCompatibilityAction {
    Noop,
    AdditiveRepair,
    SoftDerivedInvalidation,
    SoftEmbeddingInvalidation,
    HardRebuild,
}

impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = Arc::new(unsafe { Library::new(&plugin_path)? });
        let init_fn: LibSymbol<InitDbFunc> = unsafe { lib.get(b"duckdb_init_db\0")? };
        let close_fn: LibSymbol<CloseDbFunc> = unsafe { lib.get(b"duckdb_close_db\0")? };
        let is_memory = db_root == ":memory:";

        if !is_memory {
            let mut soll_dir = PathBuf::from(db_root);
            soll_dir.push("sanctuary");
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
            };

            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("sanctuary/soll.db");
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
            store.recover_interrupted_indexing()?;
            let _ = store.clear_stale_inflight_graph_projection_work();
            let _ = store.clear_stale_inflight_file_vectorization_work();
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
            store.execute("CHECKPOINT;")?;

            let reader_ptr = if is_memory {
                writer_ptr
            } else {
                let ptr = init_fn(c_path.as_ptr(), true);
                if ptr.is_null() {
                    return Err(anyhow!("Failed to init DuckDB Reader"));
                }
                ptr
            };

            {
                let mut reader_guard = store
                    .pool
                    .reader_ctx
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                *reader_guard = reader_ptr;
            }

            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("sanctuary/soll.db");
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

            Ok(store)
        }
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
            repo_root.join("bin/libaxon_plugin_duckdb.so"),
        ];

        if let Ok(cwd) = std::env::current_dir() {
            candidates
                .push(cwd.join("src/axon-plugin-duckdb/target/release/libaxon_plugin_duckdb.so"));
            candidates
                .push(cwd.join("src/axon-plugin-duckdb/target/debug/libaxon_plugin_duckdb.so"));
            candidates.push(cwd.join("bin/libaxon_plugin_duckdb.so"));
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
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, project_slug VARCHAR, embedding FLOAT[384])")?;
        self.execute("CREATE TABLE IF NOT EXISTS Chunk (id VARCHAR PRIMARY KEY, source_type VARCHAR, source_id VARCHAR, project_slug VARCHAR, kind VARCHAR, content VARCHAR, content_hash VARCHAR, start_line BIGINT, end_line BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS EmbeddingModel (id VARCHAR PRIMARY KEY, kind VARCHAR, model_name VARCHAR, dimension BIGINT, version VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS ChunkEmbedding (chunk_id VARCHAR, model_id VARCHAR, embedding FLOAT[384], source_hash VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjection (anchor_type VARCHAR, anchor_id VARCHAR, target_type VARCHAR, target_id VARCHAR, edge_kind VARCHAR, distance BIGINT, radius BIGINT, projection_version VARCHAR, created_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjectionState (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, source_signature VARCHAR, projection_version VARCHAR, updated_at BIGINT)")?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_projection_state_anchor_idx ON GraphProjectionState(anchor_type, anchor_id, radius)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphProjectionQueue (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, status VARCHAR DEFAULT 'queued', attempts BIGINT DEFAULT 0, queued_at BIGINT, last_error_reason VARCHAR, last_attempt_at BIGINT)")?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_projection_queue_anchor_idx ON GraphProjectionQueue(anchor_type, anchor_id, radius)")?;
        self.execute("CREATE TABLE IF NOT EXISTS FileVectorizationQueue (file_path VARCHAR PRIMARY KEY, status VARCHAR DEFAULT 'queued', attempts BIGINT DEFAULT 0, queued_at BIGINT, last_error_reason VARCHAR, last_attempt_at BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS GraphEmbedding (anchor_type VARCHAR, anchor_id VARCHAR, radius BIGINT, model_id VARCHAR, source_signature VARCHAR, projection_version VARCHAR, embedding FLOAT[384], updated_at BIGINT)")?;
        self.execute("CREATE UNIQUE INDEX IF NOT EXISTS graph_embedding_anchor_model_idx ON GraphEmbedding(anchor_type, anchor_id, radius, model_id)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Project (name VARCHAR PRIMARY KEY)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CONTAINS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS CALLS_NIF (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS IMPACTS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS SUBSTANTIATES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (project_slug VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL', id VARCHAR DEFAULT 'AXON_GLOBAL', last_pil BIGINT DEFAULT 0, last_req BIGINT DEFAULT 0, last_cpt BIGINT DEFAULT 0, last_dec BIGINT DEFAULT 0, last_mil BIGINT DEFAULT 0, last_val BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Vision (id VARCHAR PRIMARY KEY DEFAULT 'VIS-AXO-001', title VARCHAR, description VARCHAR, goal VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Pillar (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Requirement (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, status VARCHAR, priority VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Decision (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, context VARCHAR, rationale VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Milestone (id VARCHAR PRIMARY KEY, title VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Validation (id VARCHAR PRIMARY KEY, method VARCHAR, result VARCHAR, timestamp BIGINT, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Concept (name VARCHAR PRIMARY KEY, explanation VARCHAR, rationale VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Stakeholder (name VARCHAR PRIMARY KEY, role VARCHAR, metadata VARCHAR)")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.EPITOMIZES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.BELONGS_TO (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.EXPLAINS (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.SOLVES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.TARGETS (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.VERIFIES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.ORIGINATES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.SUPERSEDES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.CONTRIBUTES_TO (source_id VARCHAR, target_id VARCHAR)",
        )?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS soll.REFINES (source_id VARCHAR, target_id VARCHAR)",
        )?;
        Ok(())
    }

    fn ensure_additive_schema(&self) -> Result<()> {
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
        Ok(())
    }

    fn ensure_additive_soll_schema(&self) -> Result<()> {
        self.execute(
            "ALTER TABLE soll.Registry ADD COLUMN IF NOT EXISTS last_pil BIGINT DEFAULT 0",
        )?;
        self.execute("ALTER TABLE soll.Vision ADD COLUMN IF NOT EXISTS goal VARCHAR")?;
        self.execute("ALTER TABLE soll.Vision ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;

        self.execute("ALTER TABLE soll.Pillar ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Requirement ADD COLUMN IF NOT EXISTS status VARCHAR")?;
        self.execute("ALTER TABLE soll.Requirement ADD COLUMN IF NOT EXISTS priority VARCHAR")?;
        self.execute("ALTER TABLE soll.Requirement ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Decision ADD COLUMN IF NOT EXISTS description VARCHAR")?;
        self.execute("ALTER TABLE soll.Decision ADD COLUMN IF NOT EXISTS context VARCHAR")?;
        self.execute("ALTER TABLE soll.Decision ADD COLUMN IF NOT EXISTS rationale VARCHAR")?;
        self.execute("ALTER TABLE soll.Decision ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Milestone ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Validation ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Concept ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        self.execute("ALTER TABLE soll.Stakeholder ADD COLUMN IF NOT EXISTS metadata VARCHAR")?;
        Ok(())
    }

    fn ensure_runtime_compatibility(&self) -> Result<()> {
        let expected = [
            ("schema_version", IST_SCHEMA_VERSION),
            ("ingestion_version", IST_INGESTION_VERSION),
            ("embedding_version", IST_EMBEDDING_VERSION),
        ];

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
            "project_slug",
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

    fn reset_ist_state(&self) -> Result<()> {
        let cleanup_queries = [
            "DELETE FROM CALLS_NIF",
            "DELETE FROM CALLS",
            "DELETE FROM CONTAINS",
            "DELETE FROM IMPACTS",
            "DELETE FROM SUBSTANTIATES",
            "DELETE FROM ChunkEmbedding",
            "DELETE FROM GraphEmbedding",
            "DELETE FROM GraphProjectionState",
            "DELETE FROM GraphProjection",
            "DELETE FROM GraphProjectionQueue",
            "DELETE FROM FileVectorizationQueue",
            "DELETE FROM EmbeddingModel",
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "DELETE FROM Project",
        ];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        self.execute("DROP TABLE IF EXISTS File;")?;
        self.execute(
            "CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR, needs_reindex BOOLEAN DEFAULT FALSE, last_error_reason VARCHAR, status_reason VARCHAR, defer_count BIGINT DEFAULT 0, last_deferred_at_ms BIGINT, file_stage VARCHAR DEFAULT 'promoted', graph_ready BOOLEAN DEFAULT FALSE, vector_ready BOOLEAN DEFAULT FALSE)",
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
            "DELETE FROM ChunkEmbedding",
            "DELETE FROM GraphEmbedding",
            "DELETE FROM GraphProjectionState",
            "DELETE FROM GraphProjection",
            "DELETE FROM GraphProjectionQueue",
            "DELETE FROM FileVectorizationQueue",
            "DELETE FROM EmbeddingModel",
            "DELETE FROM Chunk",
            "DELETE FROM Symbol",
            "UPDATE File SET status = 'pending', worker_id = NULL, needs_reindex = FALSE, status_reason = 'soft_invalidated', file_stage = 'promoted', graph_ready = FALSE, vector_ready = FALSE",
        ];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        info!("IST derived structural layers soft-invalidated. File backlog preserved for replay.");
        Ok(())
    }

    fn soft_invalidate_embedding_state(&self) -> Result<()> {
        let cleanup_queries = [
            "DELETE FROM ChunkEmbedding",
            "DELETE FROM GraphEmbedding",
            "DELETE FROM EmbeddingModel",
            "UPDATE Symbol SET embedding = NULL",
            "UPDATE File SET vector_ready = FALSE WHERE graph_ready = TRUE",
            "DELETE FROM FileVectorizationQueue",
        ];

        for query in cleanup_queries {
            self.execute(query)?;
        }

        info!("IST embedding layers soft-invalidated. Structural truth preserved.");
        Ok(())
    }
}
