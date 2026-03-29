use std::path::{Path, PathBuf};
use std::fs;
use crate::graph::GraphStore;
use std::sync::Arc;
use tracing::{info, error};
use walkdir::WalkDir;

struct ProjectDependency {
    path: String,
    to: String,
}

pub struct Scanner {
    root: PathBuf,
}

impl Scanner {
    pub fn new(root: &str) -> Self {
        Self {
            root: PathBuf::from(root),
        }
    }

    pub fn scan(&self, graph: Arc<GraphStore>) {
        info!("Lattice Engine: Initializing recursive traversal on {:?}", self.root);

        let mut batch = Vec::new();
        let mut total_files = 0;

        // NEXUS v10.1: Fallback to WalkDir for resilient filesystem traversal.
        // It ignores symlinks by default and never silently aborts on hidden files.
        for entry in WalkDir::new(&self.root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();

            if path.is_file() {
                // NEXUS: Manual filtering
                if !self.is_supported(&path) {
                    continue;
                }

                let project_name = self.extract_project_slug(&path);

                // Dependency detection
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "pyproject.toml" || name == "Cargo.toml" || name == "mix.exs" {
                        if let Ok(content) = fs::read_to_string(&path) {
                            let deps = extract_toml_dependencies(&content);
                            for dep in deps {
                                let _ = graph.insert_project_dependency(&project_name, &dep.to, &dep.path);
                            }
                        }
                    }
                }

                let path_str = if let Ok(abs_path) = fs::canonicalize(&path) {
                    abs_path.to_string_lossy().to_string()
                } else {
                    path.to_string_lossy().to_string()
                };

                let metadata = fs::metadata(&path);
                let size = metadata.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                let mtime = metadata.as_ref().ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64)
                    .unwrap_or(0);
                
                batch.push((path_str, project_name, size, mtime));
                
                if batch.len() >= 100 {
                    total_files += batch.len();
                    if let Err(e) = graph.bulk_insert_files(&batch) {
                        error!("Bulk insert failed: {:?}", e);
                    }
                    batch.clear();
                    info!("... {} files mapped", total_files);
                    // Minimal pause to yield DB lock
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }

        // Final batch
        if !batch.is_empty() {
            total_files += batch.len();
            let _ = graph.bulk_insert_files(&batch);
        }

        info!("🏁 Nexus Scan Complete: {} files mapped to DuckDB (status: pending).", total_files);
    }

    fn extract_project_slug(&self, path: &Path) -> String {
        if let Ok(relative) = path.strip_prefix(&self.root) {
            if let Some(first_dir) = relative.components().next() {
                return first_dir.as_os_str().to_string_lossy().to_string();
            }
        }
        "global".to_string()
    }

    fn is_supported(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy().to_lowercase();
        
        // 1. DIRECTORY NOISE FILTER (Strict)
        if path_str.contains("/.git/") || 
           path_str.contains("/.mypy_cache/") || 
           path_str.contains("/.pytest_cache/") ||
           path_str.contains("/__pycache__/") ||
           path_str.contains("/.venv/") ||
           path_str.contains("/node_modules/") ||
           path_str.contains("/target/") ||
           path_str.contains("/_build/") ||
           path_str.contains("/deps/") {
            return false;
        }

        // 2. HIDDEN FILE FILTER
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') && name != ".env" { 
                return false; 
            }
        }

        // 3. EXTENSION FILTER
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            crate::config::CONFIG.indexing.supported_extensions.iter().any(|e| e.to_lowercase() == ext_str)
        } else {
            false
        }
    }
}

// Temporary stubs for dependency extraction
fn extract_toml_dependencies(_content: &str) -> Vec<ProjectDependency> {
    Vec::new()
}
