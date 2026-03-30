use std::ffi::{c_void, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use libloading::{Library, Symbol as LibSymbol};

use crate::graph::{CloseDbFunc, ExecFunc, GraphStore, InitDbFunc, LatticePool};

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
        let c_path = CString::new(db_path_str)?;

        unsafe {
            let writer_ptr = init_fn(c_path.as_ptr(), false);
            if writer_ptr.is_null() {
                return Err(anyhow!("Failed to init DuckDB Writer"));
            }

            let reader_ptr = if is_memory {
                writer_ptr
            } else {
                init_fn(c_path.as_ptr(), true)
            };
            if reader_ptr.is_null() {
                return Err(anyhow!("Failed to init DuckDB Reader"));
            }

            let pool = Arc::new(LatticePool {
                lib,
                writer_ctx: Mutex::new(writer_ptr),
                reader_ctx: Mutex::new(reader_ptr),
            });
            let store = Self { pool: pool.clone() };

            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("sanctuary/soll.db");
                let attach_q = format!("ATTACH '{}' AS soll;", soll_path.to_string_lossy().replace("'", "''"));
                {
                    let w_guard = store.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
                    store.setup_session(*w_guard, &attach_q)?;
                }
                {
                    let r_guard = store.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
                    store.setup_session(*r_guard, &attach_q)?;
                }
            } else {
                let _ = store.execute("CREATE SCHEMA IF NOT EXISTS soll;");
            }

            store.init_schema(is_memory)?;
            store.execute("CHECKPOINT;")?;
            Ok(store)
        }
    }

    fn setup_session(&self, ctx: *mut c_void, attach_query: &str) -> Result<()> {
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            exec_fn(ctx, CString::new("INSTALL json; LOAD json;")?.as_ptr());
            exec_fn(ctx, CString::new("SET checkpoint_threshold = '1GB';")?.as_ptr());
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
            candidates.push(cwd.join("src/axon-plugin-duckdb/target/release/libaxon_plugin_duckdb.so"));
            candidates.push(cwd.join("src/axon-plugin-duckdb/target/debug/libaxon_plugin_duckdb.so"));
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
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, project_slug VARCHAR, embedding FLOAT[384])")?;
        self.execute("CREATE TABLE IF NOT EXISTS Project (name VARCHAR PRIMARY KEY)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CONTAINS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS_NIF (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS IMPACTS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS SUBSTANTIATES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (project_slug VARCHAR PRIMARY KEY DEFAULT 'AXON_GLOBAL', id VARCHAR DEFAULT 'AXON_GLOBAL', last_req BIGINT DEFAULT 0, last_cpt BIGINT DEFAULT 0, last_dec BIGINT DEFAULT 0, last_mil BIGINT DEFAULT 0, last_val BIGINT DEFAULT 0)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Vision (id VARCHAR PRIMARY KEY DEFAULT 'VIS-AXO-001', title VARCHAR, description VARCHAR, goal VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Pillar (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Requirement (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, status VARCHAR, priority VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Decision (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, context VARCHAR, rationale VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Milestone (id VARCHAR PRIMARY KEY, title VARCHAR, status VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Validation (id VARCHAR PRIMARY KEY, method VARCHAR, result VARCHAR, timestamp BIGINT, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Concept (name VARCHAR PRIMARY KEY, explanation VARCHAR, rationale VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Stakeholder (name VARCHAR PRIMARY KEY, role VARCHAR, metadata VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.EPITOMIZES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.BELONGS_TO (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.EXPLAINS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.SOLVES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.TARGETS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.VERIFIES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.ORIGINATES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.SUPERSEDES (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.CONTRIBUTES_TO (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.REFINES (source_id VARCHAR, target_id VARCHAR)")?;
        Ok(())
    }
}
