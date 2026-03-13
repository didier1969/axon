use libloading::{Library, Symbol};
use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::path::PathBuf;
use anyhow::{anyhow, Result};
use std::sync::Arc;

type InitDbFunc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type ExecuteFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> bool;
type QueryCountFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> i64;
type QueryJsonFunc = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char;
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
                if let Ok(close_fn) = self.lib.get::<CloseDbFunc>(b"ladybug_close_db\0") {
                    close_fn(self.ctx);
                }
            }
        }
    }
}

impl GraphStore {
    pub fn new(db_path: &str) -> Result<Self> {
        let plugin_path = Self::find_plugin_path()?;
        let lib = unsafe { 
            Library::new(&plugin_path)
                .map_err(|e| anyhow!("Failed to load plugin {}: {}", plugin_path.display(), e))? 
        };
        let lib = Arc::new(lib);

        let ctx = unsafe {
            let init_fn: Symbol<InitDbFunc> = lib.get(b"ladybug_init_db\0")
                .map_err(|e| anyhow!("Failed to load symbol ladybug_init_db: {}", e))?;
            let c_path = CString::new(db_path)?;
            let ctx = init_fn(c_path.as_ptr());
            if ctx.is_null() {
                return Err(anyhow!("Failed to initialize Ladybug database at {}", db_path));
            }
            ctx
        };

        let store = Self { lib, ctx };
        store.init_schema()?;

        Ok(store)
    }

    fn find_plugin_path() -> Result<PathBuf> {
        let plugin_name = if cfg!(target_os = "macos") {
            "libaxon_plugin_ladybug.dylib"
        } else if cfg!(target_os = "windows") {
            "axon_plugin_ladybug.dll"
        } else {
            "libaxon_plugin_ladybug.so"
        };

        let current_dir = std::env::current_dir()?;
        let search_paths = vec![
            current_dir.join(plugin_name),
            current_dir.join(format!("../axon-plugin-ladybug/target/release/{}", plugin_name)),
            current_dir.join(format!("../axon-plugin-ladybug/target/debug/{}", plugin_name)),
            current_dir.join(format!("../../target/release/{}", plugin_name)),
            current_dir.join(format!("../../target/debug/{}", plugin_name)),
            // If running from root of workspace
            current_dir.join(format!("src/axon-plugin-ladybug/target/release/{}", plugin_name)),
            current_dir.join(format!("src/axon-plugin-ladybug/target/debug/{}", plugin_name)),
        ];

        for path in search_paths {
            if path.exists() {
                return Ok(path);
            }
        }

        Err(anyhow!("Could not find plugin {}. Expected it to be compiled in axon-plugin-ladybug/target. You might need to run: cd src/axon-plugin-ladybug && cargo build", plugin_name))
    }

    pub fn execute(&self, query: &str) -> Result<bool> {
        unsafe {
            let exec_fn: Symbol<ExecuteFunc> = self.lib.get(b"ladybug_execute\0")?;
            let c_query = CString::new(query)?;
            Ok(exec_fn(self.ctx, c_query.as_ptr()))
        }
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        unsafe {
            let count_fn: Symbol<QueryCountFunc> = self.lib.get(b"ladybug_query_count\0")?;
            let c_query = CString::new(query)?;
            Ok(count_fn(self.ctx, c_query.as_ptr()))
        }
    }

    pub fn query_json(&self, query: &str) -> Result<String> {
        unsafe {
            let query_fn: Symbol<QueryJsonFunc> = self.lib.get(b"ladybug_query_json\0")?;
            let c_query = CString::new(query)?;
            let result_ptr = query_fn(self.ctx, c_query.as_ptr());
            
            if result_ptr.is_null() {
                return Err(anyhow!("Query returned null pointer"));
            }
            
            let result_str = std::ffi::CStr::from_ptr(result_ptr).to_string_lossy().into_owned();
            
            if let Ok(free_fn) = self.lib.get::<FreeStringFunc>(b"ladybug_free_string\0") {
                free_fn(result_ptr);
            }
            
            Ok(result_str)
        }
    }

    fn init_schema(&self) -> Result<()> {
        self.execute("CREATE NODE TABLE IF NOT EXISTS File (path STRING, PRIMARY KEY (path))")?;
        self.execute("CREATE NODE TABLE IF NOT EXISTS Symbol (name STRING, kind STRING, tested BOOLEAN, PRIMARY KEY (name))")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS CONTAINS (FROM File TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS CALLS (FROM Symbol TO Symbol)")?;
        Ok(())
    }

    pub fn insert_file_data(&self, path: &str, result: &crate::parser::ExtractionResult) -> Result<()> {
        let safe_path = path.replace("'", "''");

        // Use transaction if possible, if not, ignore error and continue
        let _ = self.execute("BEGIN TRANSACTION");

        self.execute(&format!("MERGE (f:File {{path: '{}'}})", safe_path))?;

        for sym in &result.symbols {
            let safe_name = sym.name.replace("'", "''");
            let is_test = safe_name.contains("test_") || safe_path.contains("test");

            self.execute(&format!(
                "MERGE (s:Symbol {{name: '{}', kind: '{}', tested: {}}})",
                safe_name, sym.kind, is_test
            )).ok();

            self.execute(&format!(
                "MATCH (f:File {{path: '{}'}}), (s:Symbol {{name: '{}'}}) MERGE (f)-[:CONTAINS]->(s)",
                safe_path, safe_name
            )).ok();
        }

        for rel in &result.relations {
            let safe_to = rel.to.replace("'", "''");
            for sym in &result.symbols {
                let safe_from = sym.name.replace("'", "''");
                self.execute(&format!(
                    "MATCH (a:Symbol {{name: '{}'}}), (b:Symbol {{name: '{}'}}) MERGE (a)-[:CALLS]->(b)",
                    safe_from, safe_to
                )).ok();
            }
        }

        let _ = self.execute("COMMIT");

        Ok(())
    }
    pub fn get_security_score(&self, project_name: &str) -> Result<usize> {
        let query = format!(
            "MATCH (f:File)-[:CONTAINS]->(s:Symbol)-[:CALLS]->(d:Symbol) 
             WHERE f.path CONTAINS '{}' AND d.name IN ['eval', 'exec', 'system', 'pickle'] 
             RETURN count(DISTINCT s)",
            project_name
        );
        let issues = self.query_count(&query)?;
        if issues > 0 {
            Ok((100 - (issues * 15).min(100)) as usize)
        } else {
            Ok(100)
        }
    }

    pub fn get_coverage_score(&self, project_name: &str) -> Result<usize> {
        let q_total = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' AND s.kind = 'function' RETURN count(s)", project_name);
        let q_tested = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) WHERE f.path CONTAINS '{}' AND s.kind = 'function' AND s.tested = true RETURN count(s)", project_name);
        
        let total = self.query_count(&q_total)?;
        let tested = self.query_count(&q_tested)?;
        
        if total <= 0 { return Ok(100); }
        Ok(((tested as f64 / total as f64) * 100.0) as usize)
    }
}
