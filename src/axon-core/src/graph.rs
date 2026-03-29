use std::ffi::{CString, c_void};
use libloading::{Library, Symbol as LibSymbol};
use std::path::PathBuf;
use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingFile {
    pub path: String,
    pub trace_id: String,
    pub priority: i64,
}

// FFI Types
type InitDbFunc = unsafe extern "C" fn(path: *const std::os::raw::c_char, read_only: bool) -> *mut c_void;
type ExecFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> bool;
type QueryJsonFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
type QueryCountFunc = unsafe extern "C" fn(ctx: *mut c_void, query: *const std::os::raw::c_char) -> i64;
type FreeStrFunc = unsafe extern "C" fn(ptr: *mut std::os::raw::c_char);
type CloseDbFunc = unsafe extern "C" fn(ctx: *mut c_void);

pub struct LatticePool {
    lib: Arc<Library>,
    writer_ctx: Mutex<*mut c_void>,
    reader_ctx: Mutex<*mut c_void>,
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

        let db_path_str = if is_memory { ":memory:".to_string() } else {
            let mut p = PathBuf::from(db_root);
            std::fs::create_dir_all(&p)?;
            p.push("ist.db");
            p.to_string_lossy().to_string()
        };
        let c_path = CString::new(db_path_str)?;

        unsafe {
            let writer_ptr = init_fn(c_path.as_ptr(), false);
            if writer_ptr.is_null() { return Err(anyhow!("Failed to init DuckDB Writer")); }
            
            let reader_ptr = if is_memory {
                writer_ptr
            } else {
                init_fn(c_path.as_ptr(), true)
            };
            if reader_ptr.is_null() { return Err(anyhow!("Failed to init DuckDB Reader")); }

            let pool = Arc::new(LatticePool { 
                lib, 
                writer_ctx: Mutex::new(writer_ptr), 
                reader_ctx: Mutex::new(reader_ptr) 
            });
            let store = Self { pool: pool.clone() };
            
            if !is_memory {
                let mut soll_path = PathBuf::from(db_root);
                soll_path.push("sanctuary/soll.db");
                let attach_q = format!("ATTACH '{}' AS soll;", soll_path.to_string_lossy().replace("'", "''"));
                // Must hold lock while setting up session
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

    pub fn execute(&self, query: &str) -> Result<()> {
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new(query)?.as_ptr()) {
                return Err(anyhow!("Writer Error: {}", query));
            }
        }
        Ok(())
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        let expanded = Self::expand_named_params(query, params)?;
        self.execute(&expanded)
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(query, *guard)
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        let expanded = Self::expand_named_params(query, params)?;
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
        self.query_on_ctx(&expanded, *guard)
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        let guard = self.pool.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            let count_fn: LibSymbol<QueryCountFunc> = self.pool.lib.get(b"duckdb_query_count\0")?;
            Ok(count_fn(*guard, CString::new(query)?.as_ptr()))
        }
    }

    pub fn query_count_param(&self, query: &str, params: &serde_json::Value) -> Result<i64> {
        let res = self.query_json_param(query, params)?;
        let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res).unwrap_or_default();
        if let Some(row) = rows.get(0) {
            if let Some(val) = row.get(0) {
                return Ok(val.as_i64().unwrap_or(0));
            }
        }
        Ok(0)
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

    fn expand_named_params(query: &str, params: &serde_json::Value) -> Result<String> {
        if let Some(arr) = params.as_array() {
            let mut expanded = query.to_string();
            for value in arr {
                let replacement = match value {
                    serde_json::Value::Null => "NULL".to_string(),
                    serde_json::Value::Bool(v) => v.to_string(),
                    serde_json::Value::Number(v) => v.to_string(),
                    serde_json::Value::String(v) => format!("'{}'", v.replace('\'', "''")),
                    _ => {
                        return Err(anyhow!(
                            "Unsupported positional parameter type: {}",
                            value
                        ))
                    }
                };

                if let Some(pos) = expanded.find('?') {
                    expanded.replace_range(pos..=pos, &replacement);
                } else {
                    return Err(anyhow!("Too many positional parameters supplied"));
                }
            }
            return Ok(expanded);
        }

        let mut expanded = query.to_string();
        let obj = match params.as_object() {
            Some(obj) => obj,
            None => return Ok(expanded),
        };

        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::Null => "NULL".to_string(),
                serde_json::Value::Bool(v) => v.to_string(),
                serde_json::Value::Number(v) => v.to_string(),
                serde_json::Value::String(v) => format!("'{}'", v.replace('\'', "''")),
                _ => {
                    return Err(anyhow!(
                        "Unsupported parameter type for ${}: {}",
                        key,
                        value
                    ))
                }
            };
            expanded = expanded.replace(&format!("${}", key), &replacement);
        }

        Ok(expanded)
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
        let mut skipped_paths = Vec::new();
        let mut symbol_values = Vec::new();

        for task in tasks {
            match task {
                crate::worker::DbWriteTask::FileExtraction { path, extraction, .. } => {
                    indexed_paths.push(format!("'{}'", path.replace("'", "''")));
                    let slug = extraction.project_slug.as_deref().unwrap_or("global");
                    for sym in &extraction.symbols {
                        let embedding_sql = if let Some(ref v) = sym.embedding {
                            format!("CAST({:?} AS FLOAT[384])", v)
                        } else {
                            "NULL".to_string()
                        };

                        symbol_values.push(format!("('{}::{}', '{}', '{}', {}, {}, {}, {}, '{}', {})",
                            slug.replace("'", "''"), sym.name.replace("'", "''"), 
                            sym.name.replace("'", "''"), sym.kind, 
                            sym.tested, sym.is_public, sym.is_nif, sym.is_unsafe, 
                            slug.replace("'", "''"),
                            embedding_sql
                        ));
                    }
                },
                crate::worker::DbWriteTask::FileSkipped { path, .. } => {
                    skipped_paths.push(format!("'{}'", path.replace("'", "''")));
                },
                _ => {}
            }
        }

        if !indexed_paths.is_empty() {
            queries.push(format!("UPDATE File SET status = 'indexed', worker_id = NULL WHERE path IN ({});", indexed_paths.join(",")));
        }
        if !skipped_paths.is_empty() {
            queries.push(format!("UPDATE File SET status = 'skipped', worker_id = NULL WHERE path IN ({});", skipped_paths.join(",")));
        }
        for chunk in symbol_values.chunks(500) {
            queries.push(format!("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe, project_slug, embedding) VALUES {} ON CONFLICT(id) DO UPDATE SET embedding=EXCLUDED.embedding;", chunk.join(",")));
        }
        self.execute_batch(&queries)
    }

    pub fn fetch_pending_batch(&self, count: usize) -> Result<Vec<PendingFile>> {
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
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
                 SET status = 'indexing', worker_id = {}
                 WHERE path IN (
                    SELECT path FROM File
                    WHERE status = 'pending'
                    ORDER BY priority DESC
                    LIMIT {}
                 );",
                claim_id, count
            );

            if !exec_fn(*guard, CString::new(claim_query)?.as_ptr()) {
                let _ = exec_fn(*guard, CString::new("ROLLBACK;")?.as_ptr());
                return Err(anyhow!("Pending Fetch Error: claim update failed"));
            }
        }

        let fetch_query = format!(
            "SELECT path, COALESCE(trace_id, 'none'), priority
             FROM File
             WHERE status = 'indexing' AND worker_id = {}
             ORDER BY priority DESC",
            claim_id
        );
        let res = self.query_on_ctx(&fetch_query, *guard)?;

        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Pending Fetch Error: COMMIT failed"));
            }
        }
        drop(guard);
        
        if res == "[]" || res == "" { return Ok(vec![]); }
        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let files: Vec<PendingFile> = raw.into_iter().filter_map(|row| {
            if row.len() >= 3 {
                let priority = row[2].as_i64().or_else(|| row[2].as_str().and_then(|s| s.parse::<i64>().ok()))?;
                Some(PendingFile {
                    path: row[0].as_str()?.to_string(),
                    trace_id: row[1].as_str()?.to_string(),
                    priority,
                })
            } else { None }
        }).collect();
        Ok(files)
    }

    pub fn fetch_unembedded_symbols(&self, count: usize) -> Result<Vec<(String, String)>> {
        let query = format!("SELECT id, name || ': ' || kind FROM Symbol WHERE embedding IS NULL LIMIT {}", count);
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        let res = self.query_on_ctx(&query, *guard)?;
        drop(guard);

        if res == "[]" || res == "" { return Ok(vec![]); }
        
        let raw: Vec<Vec<serde_json::Value>> = serde_json::from_str(&res)?;
        let symbols: Vec<(String, String)> = raw.into_iter().filter_map(|row| {
            if row.len() >= 2 {
                Some((row[0].as_str()?.to_string(), row[1].as_str()?.to_string()))
            } else { None }
        }).collect();
        Ok(symbols)
    }

    pub fn update_symbol_embeddings(&self, updates: &[(String, Vec<f32>)]) -> Result<()> {
        if updates.is_empty() { return Ok(()); }
        let mut queries = Vec::new();
        
        for chunk in updates.chunks(100) {
            for (id, vector) in chunk {
                let embedding_sql = format!("CAST({:?} AS FLOAT[384])", vector);
                queries.push(format!("UPDATE Symbol SET embedding = {} WHERE id = '{}';", embedding_sql, id.replace("'", "''")));
            }
        }
        self.execute_batch(&queries)
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        if queries.is_empty() { return Ok(()); }
        
        // NEXUS v11.3: The Ironclad Lock
        // Hold the lock for the entire duration of the batch transaction
        let guard = self.pool.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
        
        unsafe {
            let exec_fn: LibSymbol<ExecFunc> = self.pool.lib.get(b"duckdb_execute\0")?;
            
            if !exec_fn(*guard, CString::new("BEGIN TRANSACTION;")?.as_ptr()) {
                return Err(anyhow!("Batch Writer Error: BEGIN TRANSACTION failed"));
            }
            
            for q in queries {
                if !exec_fn(*guard, CString::new(q.as_str())?.as_ptr()) {
                    let _ = exec_fn(*guard, CString::new("ROLLBACK;")?.as_ptr());
                    return Err(anyhow!("Batch Writer Error on query: {}", q));
                }
            }
            
            if !exec_fn(*guard, CString::new("COMMIT;")?.as_ptr()) {
                return Err(anyhow!("Batch Writer Error: COMMIT failed"));
            }
        }
        Ok(())
    }

    pub fn insert_project_dependency(&self, from: &str, to: &str, _path: &str) -> Result<()> {
        self.execute(&format!("INSERT INTO CONTAINS (source_id, target_id) VALUES ('{}', '{}');", from, to))
    }

    // --- Analytics Stubs ---
    pub fn get_security_audit(&self, _project: &str) -> Result<(i64, String)> {
        let query = "
            WITH dangerous_paths AS (
                SELECT s1.name, s2.name AS target_name
                FROM CALLS c
                JOIN Symbol s1 ON s1.id = c.source_id
                JOIN Symbol s2 ON s2.id = c.target_id
                WHERE s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM CALLS_NIF c
                JOIN Symbol s1 ON s1.id = c.source_id
                JOIN Symbol s2 ON s2.id = c.target_id
                WHERE s2.is_nif = true OR s2.is_unsafe = true
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM Symbol s1
                JOIN CALLS c1 ON c1.source_id = s1.id
                JOIN Symbol mid ON mid.id = c1.target_id
                JOIN CALLS c2 ON c2.source_id = mid.id
                JOIN Symbol s2 ON s2.id = c2.target_id
                WHERE s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')
                UNION ALL
                SELECT s1.name, s2.name AS target_name
                FROM Symbol s1
                JOIN CALLS_NIF c1 ON c1.source_id = s1.id
                JOIN Symbol mid ON mid.id = c1.target_id
                JOIN CALLS c2 ON c2.source_id = mid.id
                JOIN Symbol s2 ON s2.id = c2.target_id
                WHERE s2.is_unsafe = true OR lower(s2.name) IN ('eval', 'unwrap')
            )
            SELECT name, target_name FROM dangerous_paths
        ";

        let res = self.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        if rows.is_empty() {
            return Ok((100, "[]".to_string()));
        }

        let score = (100 - (rows.len() as i64 * 20)).max(0);
        Ok((score, serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())))
    }

    pub fn get_coverage_score(&self, _project: &str) -> Result<i64> {
        let total = self.query_count("SELECT count(*) FROM Symbol")?;
        if total <= 0 {
            return Ok(0);
        }
        let tested = self.query_count("SELECT count(*) FROM Symbol WHERE tested = true")?;
        Ok(((tested * 100) / total).clamp(0, 100))
    }

    pub fn get_technical_debt(&self, _project: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
        let query = "
            SELECT f.path, s.name
            FROM File f
            JOIN CONTAINS c ON c.source_id = f.path
            JOIN Symbol s ON s.id = c.target_id
            WHERE lower(s.name) LIKE '%todo%'
               OR lower(s.name) LIKE '%fixme%'
               OR lower(s.name) LIKE '%secret%'
               OR lower(s.name) LIKE '%hardcoded credential%'
               OR EXISTS (
                    SELECT 1 FROM CALLS call
                    JOIN Symbol target ON target.id = call.target_id
                    WHERE call.source_id = s.id
                      AND lower(target.name) IN ('unwrap', 'eval')
               )
        ";

        let res = self.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut findings = serde_json::Map::new();
        for row in rows {
            if row.len() >= 2 {
                findings.insert(row[0].clone(), serde_json::Value::String(row[1].clone()));
            }
        }
        Ok(findings)
    }

    pub fn get_god_objects(&self, _project: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
        let query = "
            SELECT s.name, count(*) AS fan_in
            FROM Symbol s
            JOIN CALLS c ON c.target_id = s.id
            GROUP BY s.name
            HAVING count(*) >= 5
        ";
        let res = self.query_json(query)?;
        let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
        let mut findings = serde_json::Map::new();
        for row in rows {
            if row.len() >= 2 {
                let count = row[1].parse::<i64>().unwrap_or(0);
                findings.insert(row[0].clone(), serde_json::Value::Number(count.into()));
            }
        }
        Ok(findings)
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

impl Drop for LatticePool {
    fn drop(&mut self) {
        unsafe {
            let close_fn: LibSymbol<CloseDbFunc> = self.lib.get(b"duckdb_close_db\0").unwrap();
            let writer_ctx = *self.writer_ctx.lock().unwrap_or_else(|p| p.into_inner());
            let reader_ctx = *self.reader_ctx.lock().unwrap_or_else(|p| p.into_inner());
            if !writer_ctx.is_null() { close_fn(writer_ctx); }
            if !reader_ctx.is_null() && reader_ctx != writer_ctx { close_fn(reader_ctx); }
        }
    }
}
