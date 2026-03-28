use std::ffi::{CString, c_void};
use libloading::{Library, Symbol as LibSymbol};
use std::path::PathBuf;
use anyhow::{Result, anyhow};
use tracing::{info, error, warn, debug};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingFile {
    pub path: String,
    pub trace_id: String,
    pub priority: i64,
}

// FFI Types
type InitDbFunc = unsafe extern "C" fn(path: *const std::os::raw::c_char, read_only: bool) -> *mut c_void;
type ExecFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> bool;
type ExecParamFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char, params: *const std::os::raw::c_char) -> bool;
type QueryJsonFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
type QueryJsonParamFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char, params: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
type QueryCountFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> i64;
type FreeStrFunc = unsafe extern "C" fn(ptr: *mut std::os::raw::c_char);
type CloseDbFunc = unsafe extern "C" fn(ctx: *mut c_void);

pub struct LatticePool {
    lib: Arc<Library>,
    writer_ctx: *mut c_void,
    ingest_reader_ctx: *mut c_void,
    ia_reader_ctx: *mut c_void,
}

unsafe impl Send for LatticePool {}
unsafe impl Sync for LatticePool {}

pub struct GraphStore {
    pool: Arc<LatticePool>,
}

impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = Arc::new(unsafe { Library::new(&plugin_path)? });
        let init_fn: LibSymbol<InitDbFunc> = unsafe { lib.get(b"duckdb_init_db\0")? };

        let db_path_str = if db_root == ":memory:" { ":memory:".to_string() } else {
            let mut p = PathBuf::from(db_root);
            std::fs::create_dir_all(&p)?;
            p.push("ist.db");
            p.to_string_lossy().to_string()
        };
        let c_path = CString::new(db_path_str)?;

        unsafe {
            let writer_ctx = init_fn(c_path.as_ptr(), false);
            if writer_ctx.is_null() { return Err(anyhow!("Failed to init DuckDB Writer")); }

            // CRITICAL FIX: In-memory databases MUST share the same connection context
            // otherwise they are totally isolated and don't see each other's tables.
            let is_memory = db_root == ":memory:";
            let ingest_reader_ctx = if is_memory { writer_ctx } else { init_fn(c_path.as_ptr(), true) };
            let ia_reader_ctx = if is_memory { writer_ctx } else { init_fn(c_path.as_ptr(), true) };

            let pool = Arc::new(LatticePool { lib, writer_ctx, ingest_reader_ctx, ia_reader_ctx });
            let store = Self { pool: pool.clone() };
            
            // --- SYNC ALL SESSIONS ---
            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("sanctuary/soll.db");
                
                // --- SOVEREIGNTY GUARDIAN (Shadow Mirror) ---
                let vault_path = PathBuf::from("/home/dstadel/projects/axon/.axon/vault/soll.db.shadow");
                if !soll_path.exists() && vault_path.exists() {
                    info!("🛡️  Sovereignty Alert: Sanctuary missing! Restoring from Shadow Mirror...");
                    if let Err(e) = std::fs::copy(&vault_path, &soll_path) {
                        error!("❌ Restoration Failed: {:?}", e);
                    } else {
                        info!("✅ Sanctuary Restored from Vault.");
                    }
                }

                let attach_q = format!("ATTACH '{}' AS soll;", soll_path.to_string_lossy().replace("'", "''"));

                // Configure All Sessions
                store.setup_session(pool.writer_ctx, &attach_q)?;
                if ingest_reader_ctx != writer_ctx { store.setup_session(pool.ingest_reader_ctx, &attach_q)?; }
                if ia_reader_ctx != writer_ctx { store.setup_session(pool.ia_reader_ctx, &attach_q)?; }
            } else {
                // In memory, we just create the schema
                let q = "CREATE SCHEMA IF NOT EXISTS soll;";
                store.execute(q)?;
            }

            store.init_schema(is_memory)?;
            
            // Force Header Write
            store.execute("CHECKPOINT;")?;
            
            // --- SYNC TO SHADOW MIRROR ---
            if !is_memory {
                let _ = store.sync_to_vault();
            }
            
            Ok(store)
        }
    }

    pub fn sync_to_vault(&self) -> Result<()> {
        // Since DuckDB may have locks, we use its internal EXPORT/COPY or a safe VACUUM INTO
        // But for v2.6, a safe checkpoint followed by a background copy is robust enough
        // provided the Writer is the only one.
        let vault_path = "/home/dstadel/projects/axon/.axon/vault/soll.db.shadow";
        let sanctuary_path = "/home/dstadel/projects/axon/.axon/graph_v2/sanctuary/soll.db";
        
        debug!("🔄 Syncing Sanctuary to Shadow Mirror...");
        let _ = self.execute("CHECKPOINT;");
        if let Err(e) = std::fs::copy(sanctuary_path, vault_path) {
            warn!("⚠️  Shadow Sync failed: {:?}", e);
            return Err(anyhow!("Sync Error: {:?}", e));
        }
        Ok(())
    }

    fn setup_session(&self, ctx: *mut c_void, attach_query: &str) -> Result<()> {
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            exec_fn(ctx, CString::new("INSTALL json; LOAD json;")?.as_ptr());
            
            // ROBUSTNESS SETTINGS
            exec_fn(ctx, CString::new("SET checkpoint_threshold = '1GB';")?.as_ptr());
            exec_fn(ctx, CString::new("SET threads = 4;")?.as_ptr());

            if !attach_query.is_empty() && attach_query.contains("soll.db") {
                exec_fn(ctx, CString::new(attach_query)?.as_ptr());
            }
            Ok(())
        }
    }

    pub fn execute(&self, query: &str) -> Result<()> {
        let start = Instant::now();
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(self.pool.writer_ctx, CString::new(query)?.as_ptr()) {
                return Err(anyhow!("Writer Error: {}", query));
            }
        }
        let dur = start.elapsed();
        if dur.as_millis() > 500 { debug!("[Telemetry] SLOW_EXEC: {:?} | {}", dur, query); }
        Ok(())
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        unsafe {
            let exec_fn: LibSymbol<ExecParamFunc> = self.pool.lib.get(b"duckdb_execute_param\0")?;
            let p_str = serde_json::to_string(params)?;
            if !exec_fn(self.pool.writer_ctx, CString::new(query)?.as_ptr(), CString::new(p_str)?.as_ptr()) {
                return Err(anyhow!("Param Writer Error: {}", query));
            }
            Ok(())
        }
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        self.query_on_ctx(query, self.pool.ia_reader_ctx)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        unsafe {
            let query_fn: LibSymbol<QueryJsonParamFunc> = self.pool.lib.get(b"duckdb_query_json_param\0")?;
            let free_fn: LibSymbol<FreeStrFunc> = self.pool.lib.get(b"duckdb_free_string\0")?;
            let p_str = serde_json::to_string(params)?;
            let ptr = query_fn(self.pool.ia_reader_ctx, CString::new(query)?.as_ptr(), CString::new(p_str)?.as_ptr());
            if ptr.is_null() { return Ok("[]".to_string()); }
            let res = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
            free_fn(ptr);
            Ok(res)
        }
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        unsafe {
            let count_fn: LibSymbol<QueryCountFunc> = self.pool.lib.get(b"duckdb_query_count\0")?;
            Ok(count_fn(self.pool.ia_reader_ctx, CString::new(query)?.as_ptr()))
        }
    }

    pub fn query_count_param(&self, query: &str, _params: &serde_json::Value) -> Result<i64> {
        self.query_count(query)
    }

    fn query_on_ctx(&self, query: &str, ctx: *mut c_void) -> Result<String> {
        unsafe {
            let query_fn: LibSymbol<QueryJsonFunc> = self.pool.lib.get(b"duckdb_query_json\0")?;
            let free_fn: LibSymbol<FreeStrFunc> = self.pool.lib.get(b"duckdb_free_string\0")?;
            let ptr = query_fn(ctx, CString::new(query)?.as_ptr());
            if ptr.is_null() { return Ok("[]".to_string()); }
            let res = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
            free_fn(ptr);
            Ok(res)
        }
    }

    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
        for (path, project, size, mtime) in file_paths {
            queries.push(format!("INSERT INTO Project (name) VALUES ('{}') ON CONFLICT DO NOTHING;", project.replace("'", "''")));
            queries.push(format!("INSERT INTO File (path, project_slug, size, mtime, status, priority) VALUES ('{}', '{}', {}, {}, 'pending', 100) ON CONFLICT(path) DO UPDATE SET mtime=EXCLUDED.mtime;", 
                path.replace("'", "''"), project.replace("'", "''"), size, mtime));
        }
        self.execute_batch(&queries)
    }

    pub fn insert_file_data_batch(&self, tasks: &[crate::worker::DbWriteTask]) -> Result<()> {
        if tasks.is_empty() { return Ok(()); }

        let mut queries = Vec::new();
        let mut indexed_paths = Vec::new();
        let mut symbol_values = Vec::new();

        for task in tasks {
            if let crate::worker::DbWriteTask::FileExtraction { path, extraction, .. } = task {
                indexed_paths.push(format!("'{}'", path.replace("'", "''")));
                let slug = extraction.project_slug.as_deref().unwrap_or("global");
                
                for sym in &extraction.symbols {
                    symbol_values.push(format!("('{}::{}', '{}', '{}', {}, {}, {}, {}, '{}')",
                        slug.replace("'", "''"), sym.name.replace("'", "''"), 
                        sym.name.replace("'", "''"), sym.kind, 
                        sym.tested, sym.is_public, sym.is_nif, sym.is_unsafe, 
                        slug.replace("'", "''")
                    ));
                }
            }
        }

        // 1. Batch Update File Status
        if !indexed_paths.is_empty() {
            let update_q = format!("UPDATE File SET status = 'indexed' WHERE path IN ({});", indexed_paths.join(","));
            queries.push(update_q);
        }

        // 2. Batch Insert Symbols (by chunks of 500 to avoid SQL length limits)
        for chunk in symbol_values.chunks(500) {
            queries.push(format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug) VALUES {} ON CONFLICT DO NOTHING;", 
                chunk.join(",")));
        }

        let res = self.execute_batch(&queries);
        if res.is_ok() && !indexed_paths.is_empty() {
            // Verification check: did the update actually change anything?
            // In DuckDB, we can check rows_affected if we use a different API, but here we just log success.
            debug!("GraphStore: Committed batch for {} files.", indexed_paths.len());
        }
        res
    }

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        self.execute(&format!("INSERT INTO CONTAINS (source_id, target_id) VALUES ('{}', '{}');", from, to))
    }

    pub fn fetch_pending_batch(&self, count: usize) -> Result<Vec<PendingFile>> {
        let query = format!("SELECT path, COALESCE(trace_id, 'none'), priority FROM File WHERE status = 'pending' ORDER BY priority DESC LIMIT {}", count);
        let res = self.query_on_ctx(&query, self.pool.ingest_reader_ctx)?;
        
        if res == "[]" || res == "" {
            return Ok(vec![]);
        }

        let raw: Vec<Vec<serde_json::Value>> = match serde_json::from_str(&res) {
            Ok(r) => r,
            Err(e) => {
                error!("JSON Deserialization Error: {:?} | Raw: {}", e, res);
                return Err(anyhow!("JSON Error: {}", e));
            }
        };
        let files: Vec<PendingFile> = raw.into_iter().filter_map(|row| {
            if row.len() >= 3 {
                let priority = row[2].as_i64().or_else(|| {
                    row[2].as_str().and_then(|s| s.parse::<i64>().ok())
                })?;
                
                Some(PendingFile {
                    path: row[0].as_str()?.to_string(),
                    trace_id: row[1].as_str()?.to_string(),
                    priority,
                })
            } else { None }
        }).collect();
        Ok(files)
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        self.execute("BEGIN TRANSACTION;")?;
        for q in queries { self.execute(q)?; }
        self.execute("COMMIT;")?;
        Ok(())
    }

    pub fn get_security_audit(&self, _proj: &str) -> Result<(usize, String)> { Ok((100, "[]".to_string())) }
    pub fn get_coverage_score(&self, _proj: &str) -> Result<usize> { Ok(0) }
    pub fn get_technical_debt(&self, _proj: &str) -> Result<Vec<(String, String)>> { Ok(vec![]) }
    pub fn get_god_objects(&self, _proj: &str) -> Result<Vec<(String, usize)>> { Ok(vec![]) }

    fn find_plugin_path() -> Result<PathBuf> {
        Ok(std::env::current_dir()?.join("bin/libaxon_plugin_duckdb.so"))
    }

    fn init_schema(&self, is_memory: bool) -> Result<()> {
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT, trace_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, project_slug VARCHAR, embedding FLOAT[384])")?;
        self.execute("CREATE TABLE IF NOT EXISTS Project (name VARCHAR PRIMARY KEY)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CONTAINS (source_id VARCHAR, target_id VARCHAR)")?;

        // Initializing the SOLL Plane Tables
        self.execute("CREATE TABLE IF NOT EXISTS soll.Vision (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Pillar (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Requirement (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, status VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS soll.Decision (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, status VARCHAR)")?;

        if is_memory { self.execute("CREATE SCHEMA IF NOT EXISTS soll")?; }
        Ok(())
    }
}

impl Drop for LatticePool {
    fn drop(&mut self) {
        unsafe {
            let close_fn: LibSymbol<CloseDbFunc> = self.lib.get(b"duckdb_close_db\0").unwrap();
            if !self.writer_ctx.is_null() {
                close_fn(self.writer_ctx);
            }
            if !self.ingest_reader_ctx.is_null() && self.ingest_reader_ctx != self.writer_ctx {
                close_fn(self.ingest_reader_ctx);
            }
            if !self.ia_reader_ctx.is_null() && self.ia_reader_ctx != self.writer_ctx && self.ia_reader_ctx != self.ingest_reader_ctx {
                close_fn(self.ia_reader_ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_store_lifecycle() {
        let store = GraphStore::new(":memory:").expect("Failed to init store");
        
        // 1. Bulk Insert
        let files = vec![
            ("/tmp/test1.rs".to_string(), "test_proj".to_string(), 100, 12345),
        ];
        store.bulk_insert_files(&files).expect("Bulk insert failed");
        
        // 2. Direct Query check
        let dump = store.query_json("SELECT path, status, priority FROM File").expect("Dump failed");
        println!("Full Table Dump: {}", dump);
        
        let count = store.query_count("SELECT count(*) FROM File").expect("Count failed");
        println!("Files in DB after insert: {}", count);
        
        // 3. Fetch Pending
        let pending = store.fetch_pending_batch(10).expect("Fetch failed");
        assert_eq!(count, 1, "Count should be 1");
        assert_eq!(pending.len(), 1, "Should have 1 pending file in batch");
    }
}
