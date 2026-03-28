use libloading::{Library, Symbol};
use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::path::PathBuf;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing::{info, error};

type InitDbFunc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type ExecuteFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;
type ExecuteParamFunc = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> bool;
type ExecuteBatchFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;
type QueryCountFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> i64;
type QueryCountParamFunc = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> i64;
type QueryJsonFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
type QueryJsonParamFunc = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_char;
type FreeStringFunc = unsafe extern "C" fn(*mut c_char);
type CloseDbFunc = unsafe extern "C" fn(*mut c_void);

pub struct GraphStore {
    lib: Arc<Library>,
    ctx: *mut c_void,
}

unsafe impl Send for GraphStore {}
unsafe impl Sync for GraphStore {}

impl Drop for GraphStore {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                if let Ok(close_fn) = self.lib.get::<CloseDbFunc>(b"duckdb_close_db\0") {
                    close_fn(self.ctx);
                }
            }
        }
    }
}

impl GraphStore {
    pub fn new(db_root: &str) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = unsafe { 
            Library::new(&plugin_path)
                .map_err(|e| anyhow!("Failed to load plugin {}: {}", plugin_path.display(), e))? 
        };
        let lib = Arc::new(lib);

        let db_root_path = std::path::Path::new(db_root);
        let ist_path = db_root_path.join("ist.db");
        let soll_path = db_root_path.join("soll.db");

        // --- STEP 1: Bootstrap SOLL Sanctuary if it doesn't exist ---
        // DuckDB ATTACH fails if the target directory is not a valid database.
        if db_root != ":memory:" {
            unsafe {
                let init_fn: Symbol<InitDbFunc> = lib.get(b"duckdb_init_db\0")?;
                let close_fn: Symbol<CloseDbFunc> = lib.get(b"duckdb_close_db\0")?;
                let exec_fn: Symbol<ExecuteFunc> = lib.get(b"duckdb_execute\0")?;
                
                let c_soll_path = CString::new(soll_path.to_string_lossy().to_string())?;
                let tmp_ctx = init_fn(c_soll_path.as_ptr());
                if !tmp_ctx.is_null() {
                    let q = "CREATE TABLE IF NOT EXISTS Vision (title VARCHAR PRIMARY KEY, description VARCHAR, goal VARCHAR);
                             CREATE TABLE IF NOT EXISTS Pillar (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR);
                             CREATE TABLE IF NOT EXISTS Requirement (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, justification VARCHAR, priority VARCHAR);
                             CREATE TABLE IF NOT EXISTS Concept (name VARCHAR PRIMARY KEY, explanation VARCHAR, rationale VARCHAR);
                             CREATE TABLE IF NOT EXISTS Registry (id VARCHAR PRIMARY KEY, last_req BIGINT, last_cpt BIGINT, last_dec BIGINT);
                             CREATE TABLE IF NOT EXISTS EPITOMIZES (source_id VARCHAR, target_id VARCHAR);
                             CREATE TABLE IF NOT EXISTS BELONGS_TO (source_id VARCHAR, target_id VARCHAR);
                             CREATE TABLE IF NOT EXISTS EXPLAINS (source_id VARCHAR, target_id VARCHAR);
                             CREATE TABLE IF NOT EXISTS SUPERSEDES (source_id VARCHAR, target_id VARCHAR, reason VARCHAR);";
                    let c_q = CString::new(q)?;
                    exec_fn(tmp_ctx, c_q.as_ptr());
                    close_fn(tmp_ctx);
                }
            }
        }

        // --- STEP 2: Initialize IST Forge as Primary ---
        let ctx = unsafe {
            let init_fn: Symbol<InitDbFunc> = lib.get(b"duckdb_init_db\0")
                .map_err(|e| anyhow!("Failed to load symbol duckdb_init_db: {}", e))?;
            
            let c_path = if db_root == ":memory:" {
                CString::new(":memory:")?
            } else {
                CString::new(ist_path.to_string_lossy().to_string())?
            };
            
            let ctx = init_fn(c_path.as_ptr());
            if ctx.is_null() {
                return Err(anyhow!("Failed to initialize DuckDB database"));
            }
            ctx
        };

        let store = Self { lib, ctx };
        
        // --- STEP 3: Attach the SOLL Sanctuary (DuckDB Lifecycle) ---
        
        // Load the vss extension for vector similarity search
        if let Err(e) = store.execute("INSTALL vss; LOAD vss;") {
            error!("Warning: Failed to load vss extension: {}", e);
        }

        if db_root != ":memory:" {
            let attach_query = format!("ATTACH '{}' AS soll (READ_ONLY);", soll_path.to_string_lossy().replace("'", "\\'"));
            if let Err(e) = store.execute(&attach_query) {
                error!("CRITICAL: Failed to attach SOLL sanctuary: {}", e);
                return Err(e);
            }
        }
        
        store.init_schema(db_root == ":memory:")?;

        Ok(store)
    }

    fn find_plugin_path() -> Result<PathBuf> {
        let plugin_name = if cfg!(target_os = "macos") {
            "libaxon_plugin_duckdb.dylib"
        } else if cfg!(target_os = "windows") {
            "axon_plugin_duckdb.dll"
        } else {
            "libaxon_plugin_duckdb.so"
        };

        let current_dir = std::env::current_dir()?;
        let search_paths = vec![
            current_dir.join(plugin_name),
            current_dir.join(format!("../axon-plugin-duckdb/target/release/{}", plugin_name)),
            current_dir.join(format!("../axon-plugin-duckdb/target/debug/{}", plugin_name)),
            current_dir.join(format!("../../target/release/{}", plugin_name)),
            current_dir.join(format!("../../target/debug/{}", plugin_name)),
            // If running from root of workspace
            current_dir.join(format!("src/axon-plugin-duckdb/target/release/{}", plugin_name)),
            current_dir.join(format!("src/axon-plugin-duckdb/target/debug/{}", plugin_name)),
        ];

        for path in search_paths {
            if path.exists() {
                return Ok(path);
            }
        }

        Err(anyhow!("Could not find plugin {}. Expected it to be compiled in axon-plugin-duckdb/target. You might need to run: cd src/axon-plugin-duckdb && cargo build", plugin_name))
    }

    pub fn execute(&self, query: &str) -> Result<()> {
        unsafe {
            let exec_fn: Symbol<ExecuteFunc> = self.lib.get(b"duckdb_execute\0")?;
            let c_query = CString::new(query)?;
            if !exec_fn(self.ctx, c_query.as_ptr()) {
                return Err(anyhow!("Execution failed for query: {}", query));
            }
            Ok(())
        }
    }

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<()> {
        unsafe {
            let exec_fn: Symbol<ExecuteParamFunc> = self.lib.get(b"duckdb_execute_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            if !exec_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr()) {
                return Err(anyhow!("Execution failed for parameterized query: {}", query));
            }
            Ok(())
        }
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<()> {
        unsafe {
            let exec_batch_fn: Symbol<ExecuteBatchFunc> = self.lib.get(b"duckdb_execute_batch\0")?;
            let json_str = serde_json::to_string(queries)?;
            let c_query = CString::new(json_str)?;
            if !exec_batch_fn(self.ctx, c_query.as_ptr()) {
                return Err(anyhow!("Batch execution failed"));
            }
            Ok(())
        }
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        unsafe {
            let count_fn: Symbol<QueryCountFunc> = self.lib.get(b"duckdb_query_count\0")?;
            let c_query = CString::new(query)?;
            Ok(count_fn(self.ctx, c_query.as_ptr()))
        }
    }

    pub fn query_count_param(&self, query: &str, params: &serde_json::Value) -> Result<i64> {
        unsafe {
            let count_fn: Symbol<QueryCountParamFunc> = self.lib.get(b"duckdb_query_count_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            Ok(count_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr()))
        }
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        unsafe {
            let query_fn: Symbol<QueryJsonFunc> = self.lib.get(b"duckdb_query_json\0")?;
            let c_query = CString::new(query)?;
            let result_ptr = query_fn(self.ctx, c_query.as_ptr());
            
            if result_ptr.is_null() {
                return Err(anyhow!("Query returned null pointer"));
            }
            
            let result_str = std::ffi::CStr::from_ptr(result_ptr).to_string_lossy().into_owned();
            
            if let Ok(free_fn) = self.lib.get::<FreeStringFunc>(b"duckdb_free_string\0") {
                free_fn(result_ptr);
            }
            
            Ok(result_str)
        }
    }

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        unsafe {
            let query_fn: Symbol<QueryJsonParamFunc> = self.lib.get(b"duckdb_query_json_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            let result_ptr = query_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr());
            
            if result_ptr.is_null() {
                return Err(anyhow!("Query returned null pointer"));
            }
            
            let result_str = std::ffi::CStr::from_ptr(result_ptr).to_string_lossy().into_owned();
            
            if let Ok(free_fn) = self.lib.get::<FreeStringFunc>(b"duckdb_free_string\0") {
                free_fn(result_ptr);
            }
            
            if result_str.starts_with("Error:") {
                return Err(anyhow!("{}", result_str));
            }
            
            Ok(result_str)
        }
    }

    fn init_schema(&self, is_memory: bool) -> Result<()> {
        // --- IST LAYER (Physical Reality) ---
        self.execute("CREATE TABLE IF NOT EXISTS File (path VARCHAR PRIMARY KEY, project_slug VARCHAR, status VARCHAR, size BIGINT, priority BIGINT, mtime BIGINT, worker_id BIGINT)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Symbol (id VARCHAR PRIMARY KEY, name VARCHAR, kind VARCHAR, tested BOOLEAN, is_public BOOLEAN, is_nif BOOLEAN, is_unsafe BOOLEAN, embedding FLOAT[384], project_slug VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS Project (name VARCHAR PRIMARY KEY)")?;
        
        // --- SOLL LAYER (Intention & Factual Pillars) ---
        if is_memory {
            self.execute("CREATE SCHEMA IF NOT EXISTS soll")?;
            self.execute("CREATE TABLE IF NOT EXISTS soll.Vision (title VARCHAR PRIMARY KEY, description VARCHAR, goal VARCHAR)")?;
            self.execute("CREATE TABLE IF NOT EXISTS soll.Pillar (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR)")?;
            self.execute("CREATE TABLE IF NOT EXISTS soll.Requirement (id VARCHAR PRIMARY KEY, title VARCHAR, description VARCHAR, justification VARCHAR, priority VARCHAR)")?;
            self.execute("CREATE TABLE IF NOT EXISTS soll.Concept (name VARCHAR PRIMARY KEY, explanation VARCHAR, rationale VARCHAR)")?;
            self.execute("CREATE TABLE IF NOT EXISTS soll.Registry (id VARCHAR PRIMARY KEY, last_req BIGINT, last_cpt BIGINT, last_dec BIGINT)")?;

            // --- RELATIONS (SOLL) ---
            self.execute("CREATE TABLE IF NOT EXISTS soll.EPITOMIZES (source_id VARCHAR, target_id VARCHAR)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_epitomizes_src ON soll.EPITOMIZES (source_id)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_epitomizes_tgt ON soll.EPITOMIZES (target_id)")?;

            self.execute("CREATE TABLE IF NOT EXISTS soll.BELONGS_TO (source_id VARCHAR, target_id VARCHAR)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_belongs_src ON soll.BELONGS_TO (source_id)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_belongs_tgt ON soll.BELONGS_TO (target_id)")?;

            self.execute("CREATE TABLE IF NOT EXISTS soll.EXPLAINS (source_id VARCHAR, target_id VARCHAR)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_explains_src ON soll.EXPLAINS (source_id)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_explains_tgt ON soll.EXPLAINS (target_id)")?;

            self.execute("CREATE TABLE IF NOT EXISTS soll.SUPERSEDES (source_id VARCHAR, target_id VARCHAR, reason VARCHAR)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_supersedes_src ON soll.SUPERSEDES (source_id)")?;
            self.execute("CREATE INDEX IF NOT EXISTS idx_soll_supersedes_tgt ON soll.SUPERSEDES (target_id)")?;
        }

        // --- RELATIONS (IST) ---
        self.execute("CREATE TABLE IF NOT EXISTS CONTAINS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_contains_source ON CONTAINS (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_contains_target ON CONTAINS (target_id)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_source ON CALLS (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_target ON CALLS (target_id)")?;
        self.execute("CREATE TABLE IF NOT EXISTS CALLS_NIF (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_nif_source ON CALLS_NIF (source_id)")?;
        self.execute("CREATE INDEX IF NOT EXISTS idx_calls_nif_target ON CALLS_NIF (target_id)")?;
        self.execute("CREATE TABLE IF NOT EXISTS BELONGS_TO (source_id VARCHAR, target_id VARCHAR)")?;
        self.execute("CREATE TABLE IF NOT EXISTS HAS_SUBPROJECT (source_id VARCHAR, target_id VARCHAR)")?;

        // --- DIGITAL THREAD (SOLL <-> IST) ---
        // Note: Cross-database relations are stored in the primary database (IST)
        self.execute("CREATE TABLE IF NOT EXISTS SUBSTANTIATES (source_id VARCHAR, target_id VARCHAR)")?;

        Ok(())    }

    pub fn insert_project_dependency(&self, from_project: &str, to_project: &str, _path: &str) -> Result<()> {
        let query = "INSERT INTO Project (name) VALUES (?) ON CONFLICT (name) DO NOTHING;
                     INSERT INTO Project (name) VALUES (?) ON CONFLICT (name) DO NOTHING;
                     INSERT INTO HAS_SUBPROJECT (source_id, target_id) VALUES (?, ?);";
        let params = json!([
            from_project,
            to_project,
            from_project,
            to_project
        ]);
        self.execute_param(query, &params)
    }

    pub fn bulk_insert_files(&self, file_paths: &[(String, String, i64, i64)]) -> Result<()> {
        let mut queries = Vec::new();
        for (path, project, size, mtime) in file_paths {
            let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("");
            let priority = match ext {
                "ex" | "exs" | "rs" | "py" | "go" => 100,
                "ts" | "js" | "c" | "cpp" => 80,
                "md" | "json" | "yml" | "yaml" | "toml" => 50,
                _ => 10,
            };

            queries.push(format!(
                "INSERT INTO File (path, project_slug, size, mtime, status, priority) 
                 VALUES ('{}', '{}', {}, {}, 'pending', {}) 
                 ON CONFLICT (path) DO UPDATE SET project_slug=EXCLUDED.project_slug, size=EXCLUDED.size, mtime=EXCLUDED.mtime, status='pending', priority=EXCLUDED.priority;",
                path.replace("'", "''"),
                project.replace("'", "''"),
                size,
                mtime,
                priority
            ));
        }

        if !queries.is_empty() {
            self.execute_batch(&queries)?;        }
        Ok(())
    }

    pub fn insert_file_data_batch(&self, tasks: &[crate::worker::DbWriteTask]) -> Result<()> {
        let mut queries = Vec::new();

        for task in tasks {
            if let crate::worker::DbWriteTask::FileExtraction { path, extraction, .. } = task {
                let slug = extraction.project_slug.clone().unwrap_or_else(|| "global".to_string());

                // 1. File node
                queries.push(format!(
                    "INSERT INTO File (path, project_slug) VALUES ('{}', '{}') ON CONFLICT (path) DO UPDATE SET project_slug=EXCLUDED.project_slug;",
                    path.replace("'", "''"), slug.replace("'", "''")
                ));

                // 2. Symbols (Batching symbols per file)
                let mut seen_symbols = std::collections::HashSet::new();
                for sym in &extraction.symbols {
                    if seen_symbols.contains(&sym.name) { continue; }
                    seen_symbols.insert(sym.name.clone());

                    let is_test = sym.name.contains("test_") || path.contains("test");
                    let is_unsafe = sym.properties.get("unsafe").map(|s| s == "true").unwrap_or(false);
                    let is_nif = sym.properties.get("is_nif").map(|s| s == "true").unwrap_or(false);
                    let fqn = format!("{}::{}", slug, sym.name);

                    queries.push(format!(
                        "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe) VALUES ('{}', '{}', '{}', {}, {}, {}, {}) ON CONFLICT (id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe",
                        fqn.replace("'", "''"),
                        sym.name.replace("'", "''"),
                        sym.kind.replace("'", "''"),
                        is_test,
                        sym.is_public,
                        is_nif,
                        is_unsafe
                    ));
                    queries.push(format!(
                        "INSERT INTO CONTAINS (source_id, target_id) VALUES ('{}', '{}')",
                        path.replace("'", "''"),
                        fqn.replace("'", "''")
                    ));
                }

                // 3. Relations
                let valid_rels = ["CALLS", "IMPORTS", "IMPLEMENTS", "CALLS_NIF", "USES"];
                for rel in &extraction.relations {
                    let rel_type = rel.rel_type.to_uppercase();
                    let safe_rel_type = if valid_rels.contains(&rel_type.as_str()) { rel_type } else { "CALLS".to_string() };

                    if rel.from.is_empty() || rel.from == "file" || rel.from == "method" {
                        for sym in &extraction.symbols {
                            let from_fqn = format!("{}::{}", slug, sym.name);
                            let to_fqn = format!("{}::{}", slug, rel.to);
                            queries.push(format!(
                                "INSERT INTO {} (source_id, target_id) VALUES ('{}', '{}');",
                                safe_rel_type,
                                from_fqn.replace("'", "''"),
                                to_fqn.replace("'", "''")
                            ));
                        }
                    } else {
                        let from_fqn = format!("{}::{}", slug, rel.from);
                        let to_fqn = format!("{}::{}", slug, rel.to);
                        queries.push(format!(
                            "INSERT INTO {} (source_id, target_id) VALUES ('{}', '{}');",
                            safe_rel_type,
                            from_fqn.replace("'", "''"),
                            to_fqn.replace("'", "''")
                        ));
                    }
                }
            }
        }

        if !queries.is_empty() {
            self.execute_batch(&queries)?;
        }
        Ok(())
    }
    pub fn insert_file_data(&self, path: &str, result: &crate::parser::ExtractionResult) -> Result<()> {
        let slug = result.project_slug.clone().unwrap_or_else(|| "global".to_string());

        // 1. Insert/Merge File node
        self.execute_param("INSERT INTO File (path, project_slug) VALUES (?, ?) ON CONFLICT (path) DO UPDATE SET project_slug=EXCLUDED.project_slug;", &serde_json::json!([path, slug]))?;

        // 2. Insert Symbols
        let mut seen_symbols = std::collections::HashSet::new();
        for sym in &result.symbols {
            if seen_symbols.contains(&sym.name) { continue; }
            seen_symbols.insert(sym.name.clone());

            let is_test = sym.name.contains("test_") || path.contains("test");
            let is_unsafe = sym.properties.get("unsafe").map(|s| s == "true").unwrap_or(false);
            let is_nif = sym.properties.get("is_nif").map(|s| s == "true").unwrap_or(false);

            let fqn = format!("{}::{}", slug, sym.name);

            let sym_query = "INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, is_unsafe) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET name=EXCLUDED.name, kind=EXCLUDED.kind, tested=EXCLUDED.tested, is_public=EXCLUDED.is_public, is_nif=EXCLUDED.is_nif, is_unsafe=EXCLUDED.is_unsafe";
            let contains_query = "INSERT INTO CONTAINS (source_id, target_id) VALUES (?, ?)";

            let sym_params = serde_json::json!([
                fqn,
                sym.name,
                sym.kind,
                is_test,
                sym.is_public,
                is_nif,
                is_unsafe
            ]);
            let contains_params = serde_json::json!([path, fqn]);

            self.execute_param(sym_query, &sym_params)?;
            self.execute_param(contains_query, &contains_params)?;
        }

        // 3. Insert Relations
        let valid_rels = ["CALLS", "IMPORTS", "IMPLEMENTS", "CALLS_NIF", "USES"];
        for rel in &result.relations {
            let rel_type = rel.rel_type.to_uppercase();
            let safe_rel_type = if valid_rels.contains(&rel_type.as_str()) { rel_type } else { "CALLS".to_string() };

            if rel.from.is_empty() || rel.from == "file" || rel.from == "method" {
                for sym in &result.symbols {
                    let from_fqn = format!("{}::{}", slug, sym.name);
                    let to_fqn = format!("{}::{}", slug, rel.to);
                    let rel_query = format!("INSERT INTO {} (source_id, target_id) VALUES (?, ?);", safe_rel_type);
                    self.execute_param(&rel_query, &serde_json::json!([from_fqn, to_fqn]))?;
                }
            } else {
                let from_fqn = format!("{}::{}", slug, rel.from);
                let to_fqn = format!("{}::{}", slug, rel.to);
                let rel_query = format!("INSERT INTO {} (source_id, target_id) VALUES (?, ?);", safe_rel_type);
                self.execute_param(&rel_query, &serde_json::json!([from_fqn, to_fqn]))?;
            }
        }
        Ok(())
    }
    pub fn get_security_audit(&self, project_name: &str) -> Result<(usize, String)> {
        let count_query = if project_name == "*" || project_name.is_empty() {
            "SELECT count(*) FROM Symbol WHERE name IN ('eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run') OR is_unsafe = true".to_string()
        } else {
            format!("SELECT count(*) FROM Symbol WHERE project_slug = '{}' AND (name IN ('eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run') OR is_unsafe = true)", project_name.replace("'", "''"))
        };

        let issues = self.query_count(&count_query).unwrap_or(0);
        let score = if issues > 0 {
            (100 - (issues * 15).min(100)) as usize
        } else {
            100
        };

        let paths_json = if issues > 0 {
            let path_query = if project_name == "*" || project_name.is_empty() {
                "WITH RECURSIVE all_calls AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL SELECT source_id, target_id FROM CALLS_NIF \
                 ), \
                 traverse(root_caller, callee, depth) AS ( \
                    SELECT source_id as root_caller, target_id as callee, 1 as depth FROM all_calls \
                    UNION ALL \
                    SELECT t.root_caller, c.target_id, t.depth + 1 \
                    FROM all_calls c JOIN traverse t ON c.source_id = t.callee \
                    WHERE t.depth < 3 \
                 ) \
                 SELECT f.path || ' -> ' || s.name || ' calls ' || danger.name AS path \
                 FROM traverse t \
                 JOIN Symbol s ON t.root_caller = s.id \
                 JOIN CONTAINS c ON s.id = c.target_id \
                 JOIN File f ON f.path = c.source_id \
                 JOIN Symbol danger ON t.callee = danger.id \
                 WHERE danger.name IN ('eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run') OR danger.is_unsafe = true \
                 LIMIT 5".to_string()
            } else {
                format!("WITH RECURSIVE all_calls AS ( \
                    SELECT source_id, target_id FROM CALLS \
                    UNION ALL SELECT source_id, target_id FROM CALLS_NIF \
                 ), \
                 traverse(root_caller, callee, depth) AS ( \
                    SELECT source_id as root_caller, target_id as callee, 1 as depth FROM all_calls \
                    UNION ALL \
                    SELECT t.root_caller, c.target_id, t.depth + 1 \
                    FROM all_calls c JOIN traverse t ON c.source_id = t.callee \
                    WHERE t.depth < 3 \
                 ) \
                 SELECT f.path || ' -> ' || s.name || ' calls ' || danger.name AS path \
                 FROM traverse t \
                 JOIN Symbol s ON t.root_caller = s.id \
                 JOIN CONTAINS c ON s.id = c.target_id \
                 JOIN File f ON f.path = c.source_id \
                 JOIN Symbol danger ON t.callee = danger.id \
                 WHERE f.project_slug = '{0}' AND s.project_slug = '{0}' AND \
                 (danger.name IN ('eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run') OR danger.is_unsafe = true) \
                 LIMIT 5", project_name.replace("'", "''"))
            };
            self.query_json(&path_query).unwrap_or_else(|_| "[]".to_string())
        } else {
            "[]".to_string()
        };

        Ok((score, paths_json))
    }
    pub fn get_technical_debt(&self, project_name: &str) -> Result<Vec<(String, String)>> {
        let query = if project_name == "*" || project_name.is_empty() {
            "SELECT DISTINCT f.path, COALESCE(debt.name, s.kind || ': ' || s.name) as issue \
             FROM Symbol s \
             JOIN CONTAINS c ON s.id = c.target_id \
             JOIN File f ON f.path = c.source_id \
             LEFT JOIN CALLS call ON s.id = call.source_id \
             LEFT JOIN Symbol debt ON call.target_id = debt.id \
             WHERE (debt.name IN ('unwrap', 'expect', 'panic!') OR s.kind IN ('TODO', 'FIXME') OR s.kind LIKE 'SECRET_%') \
             LIMIT 50".to_string()
        } else {
            format!("SELECT DISTINCT f.path, COALESCE(debt.name, s.kind || ': ' || s.name) as issue \
                     FROM Symbol s \
                     JOIN CONTAINS c ON s.id = c.target_id \
                     JOIN File f ON f.path = c.source_id \
                     LEFT JOIN CALLS call ON s.id = call.source_id \
                     LEFT JOIN Symbol debt ON call.target_id = debt.id \
                     WHERE f.project_slug = '{0}' AND s.project_slug = '{0}' AND \
                     (debt.name IN ('unwrap', 'expect', 'panic!') OR s.kind IN ('TODO', 'FIXME') OR s.kind LIKE 'SECRET_%') \
                     LIMIT 50", project_name.replace("'", "''"))
        };

        match self.query_json(&query) {
            Ok(res) => {
                let rows: Vec<Vec<String>> = serde_json::from_str(&res).unwrap_or_default();
                Ok(rows.into_iter().filter_map(|r| {
                    if r.len() >= 2 {
                        Some((r[0].clone(), r[1].clone()))
                    } else {
                        None
                    }
                }).collect())
            },
            Err(_) => Ok(vec![])
        }
    }

    pub fn get_coverage_score(&self, project_name: &str) -> Result<usize> {
        let (q_total, q_tested) = if project_name == "*" || project_name.is_empty() {
            (
                "SELECT count(*) FROM Symbol WHERE kind = 'function'".to_string(),
                "SELECT count(*) FROM Symbol WHERE kind = 'function' AND tested = true".to_string()
            )
        } else {
            (
                format!("SELECT count(*) FROM Symbol WHERE project_slug = '{}' AND kind = 'function'", project_name.replace("'", "''")),
                format!("SELECT count(*) FROM Symbol WHERE project_slug = '{}' AND kind = 'function' AND tested = true", project_name.replace("'", "''"))
            )
        };
        
        let total = self.query_count(&q_total)?;
        let tested = self.query_count(&q_tested)?;
        
        if total <= 0 { return Ok(100); }
        Ok(((tested as f64 / total as f64) * 100.0) as usize)
    }

    pub fn get_god_objects(&self, project_name: &str) -> Result<Vec<String>> {
        let query = if project_name == "*" || project_name.is_empty() {
            "SELECT s.name \
             FROM Symbol s \
             JOIN CALLS c ON s.id = c.target_id \
             GROUP BY s.name \
             HAVING count(c.source_id) >= 10".to_string()
        } else {
            format!("SELECT s.name \
             FROM Symbol s \
             JOIN CALLS c ON s.id = c.target_id \
             WHERE s.project_slug = '{0}' \
             GROUP BY s.name \
             HAVING count(c.source_id) >= 10", project_name.replace("'", "''"))
        };
        // Find symbols in the project that have a high in-degree (>= 10 dependents)

        let result_json = self.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        let mut god_objects = Vec::new();
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result_json) {
            if let Some(arr) = parsed.as_array() {
                for item in arr {
                    if let Some(inner_arr) = item.as_array() {
                        for inner_item in inner_arr {
                            if let Some(val_str) = inner_item.as_str() {
                                god_objects.push(val_str.to_string());
                            }
                        }
                    } else if let Some(obj) = item.as_object() {
                        if let Some(name) = obj.get("s.name").and_then(|v| v.as_str()) {
                            god_objects.push(name.to_string());
                        }
                    }
                }
            }
        }
        
        Ok(god_objects)
    }

    pub fn generate_mermaid_flow(paths_json: &str) -> String {
        let mut mermaid = String::from("```mermaid\ngraph TD\n");
        let mut has_paths = false;
        
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(paths_json) {
            if let Some(arr) = parsed.as_array() {
                for item in arr {
                    if let Some(path_str) = item.get("path").and_then(|v| v.as_str()) {
                        let parts: Vec<&str> = path_str.split("-->").map(|s| s.trim()).collect();
                        for i in 0..parts.len().saturating_sub(1) {
                            mermaid.push_str(&format!("    {} --> {}\n", parts[i], parts[i+1]));
                            has_paths = true;
                        }
                    }
                }
            }
        }
        
        if !has_paths {
            return String::new();
        }
        
        mermaid.push_str("```\n");
        mermaid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mermaid_generation() {
        // Simulating JSON returned by Kuzu for paths
        let paths_json = r#"[{"path": "source --> sanitizer --> sink"}]"#;
        let mermaid = GraphStore::generate_mermaid_flow(paths_json);
        
        assert!(mermaid.contains("graph TD"));
        assert!(mermaid.contains("source --> sanitizer"));
        assert!(mermaid.contains("sanitizer --> sink"));
    }

    #[test]
    fn test_debug_technical_debt() {
        let store = GraphStore::new(":memory:").unwrap();
        store.execute("INSERT INTO File (path, project_slug) VALUES ('src/config.rs', 'global')").unwrap();
        store.execute("INSERT INTO Symbol (id, name, kind, tested, is_public, is_nif, project_slug) VALUES ('global::secret1', 'SECRET_API_KEY: Found potential hardcoded credential', 'SECRET_API_KEY', false, true, false, 'global')").unwrap();
        store.execute("INSERT INTO CONTAINS (source_id, target_id) VALUES ('src/config.rs', 'global::secret1')").unwrap();
        
        let res = store.get_technical_debt("*").unwrap();
        assert!(!res.is_empty());
    }
    #[test]
    fn test_duckdb_vector_support() {
        let store = GraphStore::new(":memory:").unwrap();
        let res = store.execute("CREATE TABLE VectorNode (id BIGINT PRIMARY KEY, vec FLOAT[3])");
        assert!(res.is_ok(), "Failed to create table with FLOAT[3]");
        
        let _ = store.execute("INSERT INTO VectorNode VALUES (1, [1.0, 2.0, 3.0])");
        let _ = store.execute("INSERT INTO VectorNode(id) VALUES (3)");

        // Try list_cosine_similarity
        let _ = store.execute("INSERT INTO VectorNode VALUES (2, [1.0, 2.0, 3.1])");
        let query_res = store.query_json("SELECT list_cosine_similarity(a.vec, b.vec) AS sim FROM VectorNode a, VectorNode b WHERE a.id = 1 AND b.id = 2");
        assert!(query_res.is_ok(), "list_cosine_similarity failed");
    }

    #[test]
    fn test_duckdb_indices_exist() {
        let store = GraphStore::new(":memory:").unwrap();
        let index_count = store.query_count("SELECT count(*) FROM duckdb_indexes() WHERE index_name LIKE 'idx_%'").unwrap_or(0);
        assert!(index_count >= 6, "Expected at least 6 indices to be created");
    }

    #[test]
    fn test_graph_insertion_persistence() {
        let store = GraphStore::new(":memory:").unwrap();

        let result = crate::parser::ExtractionResult {
            project_slug: None,
            symbols: vec![
                crate::parser::Symbol {
                    name: "DummyFunc".to_string(),
                    kind: "function".to_string(),
                    start_line: 1, end_line: 2, docstring: None, is_public: true,
                    properties: std::collections::HashMap::new(), embedding: None, is_entry_point: false
                }
            ],
            relations: vec![]
        };

        let path = "/test/path.ex";
        store.insert_file_data(path, &result).unwrap();

        let query = format!("SELECT count(*) FROM Symbol s JOIN CONTAINS c ON s.id = c.target_id WHERE c.source_id = '{}'", path);
        let count = store.query_count(&query).unwrap();
        assert_eq!(count, 1, "Graph insertion failed: 0 symbols found for file");
    }
}
