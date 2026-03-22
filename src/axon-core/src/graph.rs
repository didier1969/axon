use libloading::{Library, Symbol};
use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::path::PathBuf;
use anyhow::{anyhow, Result};
use std::sync::{Arc, Mutex};

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
    write_mutex: Mutex<()>,
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

        let store = Self { lib, ctx, write_mutex: Mutex::new(()) };
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

    pub fn execute_param(&self, query: &str, params: &serde_json::Value) -> Result<bool> {
        unsafe {
            let exec_fn: Symbol<ExecuteParamFunc> = self.lib.get(b"ladybug_execute_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            Ok(exec_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr()))
        }
    }

    pub fn execute_batch(&self, queries: &[String]) -> Result<bool> {
        unsafe {
            let exec_batch_fn: Symbol<ExecuteBatchFunc> = self.lib.get(b"ladybug_execute_batch\0")?;
            let json_str = serde_json::to_string(queries)?;
            let c_query = CString::new(json_str)?;
            Ok(exec_batch_fn(self.ctx, c_query.as_ptr()))
        }
    }

    pub fn query_count(&self, query: &str) -> Result<i64> {
        unsafe {
            let count_fn: Symbol<QueryCountFunc> = self.lib.get(b"ladybug_query_count\0")?;
            let c_query = CString::new(query)?;
            Ok(count_fn(self.ctx, c_query.as_ptr()))
        }
    }

    pub fn query_count_param(&self, query: &str, params: &serde_json::Value) -> Result<i64> {
        unsafe {
            let count_fn: Symbol<QueryCountParamFunc> = self.lib.get(b"ladybug_query_count_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            Ok(count_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr()))
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

    pub fn query_json_param(&self, query: &str, params: &serde_json::Value) -> Result<String> {
        unsafe {
            let query_fn: Symbol<QueryJsonParamFunc> = self.lib.get(b"ladybug_query_json_param\0")?;
            let c_query = CString::new(query)?;
            let params_str = serde_json::to_string(params)?;
            let c_params = CString::new(params_str)?;
            let result_ptr = query_fn(self.ctx, c_query.as_ptr(), c_params.as_ptr());
            
            if result_ptr.is_null() {
                return Err(anyhow!("Query returned null pointer"));
            }
            
            let result_str = std::ffi::CStr::from_ptr(result_ptr).to_string_lossy().into_owned();
            
            if let Ok(free_fn) = self.lib.get::<FreeStringFunc>(b"ladybug_free_string\0") {
                free_fn(result_ptr);
            }
            
            if result_str.starts_with("Error:") {
                return Err(anyhow!("{}", result_str));
            }
            
            Ok(result_str)
        }
    }

    fn init_schema(&self) -> Result<()> {
        self.execute("CREATE NODE TABLE IF NOT EXISTS File (path STRING, PRIMARY KEY (path))")?;
        self.execute("CREATE NODE TABLE IF NOT EXISTS Symbol (name STRING, kind STRING, tested BOOLEAN, is_public BOOLEAN, is_unsafe BOOLEAN, is_nif BOOLEAN, is_entry_point BOOLEAN, embedding FLOAT[384], PRIMARY KEY (name))")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS CONTAINS (FROM File TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS CALLS (FROM Symbol TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS CALLS_NIF (FROM Symbol TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS IMPORTS (FROM Symbol TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS IMPLEMENTS (FROM Symbol TO Symbol)")?;
        self.execute("CREATE REL TABLE IF NOT EXISTS USES (FROM Symbol TO Symbol)")?;
        Ok(())
    }

    pub fn insert_file_data(&self, path: &str, result: &crate::parser::ExtractionResult) -> Result<()> {
        let _guard = self.write_mutex.lock().unwrap();

        // 1. Insert/Merge File node
        self.execute_param("MERGE (f:File {path: $p})", &serde_json::json!({"p": path}))?;

        // 2. Batch Insert Symbols using UNWIND
        if !result.symbols.is_empty() {
            let mut symbols_batch = Vec::new();
            let mut seen_symbols = std::collections::HashSet::new();

            for sym in &result.symbols {
                if seen_symbols.contains(&sym.name) { continue; }
                seen_symbols.insert(sym.name.clone());

                let is_test = sym.name.contains("test_") || path.contains("test");
                let is_unsafe = sym.properties.get("unsafe").map(|s| s == "true").unwrap_or(false);
                let is_nif = sym.properties.get("is_nif").map(|s| s == "true").unwrap_or(false);

                symbols_batch.push(serde_json::json!({
                    "name": sym.name,
                    "kind": sym.kind,
                    "tested": is_test,
                    "is_public": sym.is_public,
                    "is_unsafe": is_unsafe,
                    "is_nif": is_nif,
                    "is_entry_point": sym.is_entry_point,
                    "embedding": sym.embedding
                }));
            }

            let sym_query = "UNWIND $batch AS row \
                             MERGE (s:Symbol {name: row.name}) \
                             SET s.kind = row.kind, s.tested = row.tested, s.is_public = row.is_public, \
                                 s.is_unsafe = row.is_unsafe, s.is_nif = row.is_nif, \
                                 s.is_entry_point = row.is_entry_point, s.embedding = row.embedding \
                             WITH s, row \
                             MATCH (f:File {path: $path}) MERGE (f)-[:CONTAINS]->(s)";

            // Chunk execution to prevent KuzuDB transient memory bloat
            for chunk in symbols_batch.chunks(250) {
                self.execute_param(sym_query, &serde_json::json!({
                    "batch": chunk,
                    "path": path
                }))?;
            }
            }

            // 3. Batch Insert Relations using UNWIND
            if !result.relations.is_empty() {
            let valid_rels = ["CALLS", "IMPORTS", "IMPLEMENTS", "CALLS_NIF", "USES"];
            let mut rels_by_type: std::collections::HashMap<String, Vec<serde_json::Value>> = std::collections::HashMap::new();

            for rel in &result.relations {
                let rel_type = rel.rel_type.to_uppercase();
                let safe_rel_type = if valid_rels.contains(&rel_type.as_str()) { rel_type } else { "CALLS".to_string() };

                let entry = rels_by_type.entry(safe_rel_type).or_default();

                if rel.from.is_empty() || rel.from == "file" || rel.from == "method" {
                    for sym in &result.symbols {
                        entry.push(serde_json::json!({"from": sym.name, "to": rel.to}));
                    }
                } else {
                    entry.push(serde_json::json!({"from": rel.from, "to": rel.to}));
                }
            }

            for (rel_type, batch) in rels_by_type {
                let rel_query = format!(
                    "UNWIND $batch AS row \
                     MATCH (a:Symbol {{name: row.from}}), (b:Symbol {{name: row.to}}) \
                     MERGE (a)-[:{}]->(b)",
                    rel_type
                );

                for chunk in batch.chunks(250) {
                    self.execute_param(&rel_query, &serde_json::json!({"batch": chunk}))?;
                }
            }
            }
        Ok(())
    }
    pub fn get_security_audit(&self, project_name: &str) -> Result<(usize, String)> {
        let filter = if project_name == "*" || project_name.is_empty() {
            "".to_string()
        } else {
            format!("AND f.path CONTAINS '{}'", project_name)
        };

        // Taint analysis: Path from any dangerous sink BACKWARDS to a symbol in the file
        let count_query = format!(
            "MATCH (d:Symbol)<-[:CALLS|CALLS_NIF*1..4]-(s:Symbol)<-[:CONTAINS]-(f:File) \
             WHERE (d.name IN ['eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run'] OR d.is_unsafe = true) {} \
             RETURN count(DISTINCT s)",
            filter
        );
        let issues = self.query_count(&count_query)?;

        let score = if issues > 0 {
            (100 - (issues * 15).min(100)) as usize
        } else {
            100
        };

        let paths_query = format!(
            "MATCH path = (d:Symbol)<-[:CALLS|CALLS_NIF*1..4]-(s:Symbol)<-[:CONTAINS]-(f:File) \
             WHERE (d.name IN ['eval', 'exec', 'system', 'pickle', 'os.system', 'subprocess.run'] OR d.is_unsafe = true) {} \
             RETURN path LIMIT 5",
            filter
        );

        let paths_json = self.query_json(&paths_query).unwrap_or_else(|_| "[]".to_string());

        Ok((score, paths_json))
    }
    pub fn get_coverage_score(&self, project_name: &str) -> Result<usize> {
        let filter = if project_name == "*" || project_name.is_empty() {
            "".to_string()
        } else {
            format!("WHERE f.path CONTAINS '{}'", project_name)
        };

        let q_total = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) {} {} s.kind = 'function' RETURN count(s)", 
            filter, if filter.is_empty() { "WHERE" } else { "AND" });
        let q_tested = format!("MATCH (f:File)-[:CONTAINS]->(s:Symbol) {} {} s.kind = 'function' AND s.tested = true RETURN count(s)", 
            filter, if filter.is_empty() { "WHERE" } else { "AND" });
        
        let total = self.query_count(&q_total)?;
        let tested = self.query_count(&q_tested)?;
        
        if total <= 0 { return Ok(100); }
        Ok(((tested as f64 / total as f64) * 100.0) as usize)
    }

    pub fn get_god_objects(&self, project_name: &str) -> Result<Vec<String>> {
        // Find symbols in the project that have a high in-degree (>= 10 dependents)
        let query = format!(
            "MATCH (f:File)-[:CONTAINS]->(s:Symbol)<-[:CALLS]-(caller:Symbol) 
             WHERE f.path CONTAINS '{}' 
             WITH s, count(caller) AS degree 
             WHERE degree >= 10 
             RETURN s.name",
            project_name
        );
        let result_json = self.query_json(&query).unwrap_or_else(|_| "[]".to_string());
        
        let mut god_objects = Vec::new();
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result_json) {
            if let Some(arr) = parsed.as_array() {
                for item in arr {
                    if let Some(inner_arr) = item.as_array() {
                        for inner_item in inner_arr {
                            if let Some(val_str) = inner_item.as_str() {
                                if val_str.starts_with("String(\"") && val_str.ends_with("\")") {
                                    let name = val_str[8..val_str.len()-2].to_string();
                                    god_objects.push(name);
                                }
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
    fn test_kuzu_vector_support() {
        let store = GraphStore::new(":memory:").unwrap();
        let res = store.execute("CREATE NODE TABLE VectorNode (id INT64, vec FLOAT[3], PRIMARY KEY(id))");
        assert!(res.is_ok(), "Failed to create table with FLOAT[3]");
        
        let insert_res = store.execute("CREATE (n:VectorNode {id: 1, vec: [1.0, 2.0, 3.0]})");
        assert!(insert_res.unwrap(), "Failed to insert vector");
        
        let insert_res2 = store.execute("CREATE (n:VectorNode {id: 3})");
        println!("Insert missing vector: {:?}", insert_res2);

        // Try array_cosine_similarity
        let _ = store.execute("CREATE (n:VectorNode {id: 2, vec: [1.0, 2.0, 3.1]})");
        let query_res = store.query_json("MATCH (a:VectorNode {id: 1}), (b:VectorNode {id: 2}) RETURN array_cosine_similarity(a.vec, b.vec) AS sim");
        assert!(query_res.is_ok(), "array_cosine_similarity failed");
        let json_str = query_res.unwrap();
        println!("Similarity: {}", json_str);
    }
}
