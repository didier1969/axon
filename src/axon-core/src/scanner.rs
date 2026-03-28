use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use crate::graph::GraphStore;

pub struct ProjectDependency {
    pub to: String,
    pub path: String,
}

pub fn extract_toml_dependencies(content: &str) -> Vec<ProjectDependency> {
    let mut deps = Vec::new();
    if let Ok(parsed) = content.parse::<toml::Value>() {
        if let Some(dependencies) = parsed.get("dependencies").or_else(|| parsed.get("tool").and_then(|t| t.get("poetry")).and_then(|p| p.get("dependencies"))) {
            if let Some(table) = dependencies.as_table() {
                for (k, v) in table {
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        deps.push(ProjectDependency { to: k.clone(), path: path.to_string() });
                    }
                }
            }
        }
    }
    deps
}

pub struct Scanner {
    pub root: PathBuf,
}

impl Scanner {
    pub fn new(path: &str) -> Self {
        Self {
            root: PathBuf::from(path),
        }
    }

    /// MBSE CERTIFICATION: REQ-AXO-002, REQ-AXO-003
    /// Deep scan of the filesystem using .axonignore sovereignty.
    /// Maps the IST layer (Physical Reality) into DuckDB.
    pub fn scan(&self, graph: Arc<GraphStore>) {
        tracing::info!("🚀 Starting Nexus Deep Scan of sector: {}", self.root.display());

        let mut builder = WalkBuilder::new(&self.root);
        builder.hidden(false) 
               .git_ignore(false) // REQ-AXO-002: Bypass Git convention for semantic sovereignty
               .parents(false)
               .ignore(true);
               
        // Load .axonignore patterns
        let global_axonignore = Path::new("/home/dstadel/projects/.axonignore");
        if global_axonignore.exists() { let _ = builder.add_ignore(global_axonignore); }
        let local_axonignore = self.root.join(".axonignore");
        if local_axonignore.exists() { let _ = builder.add_ignore(local_axonignore); }

        let walker = builder.build();
        let mut batch = Vec::new();
        let mut total_files = 0;

        for entry in walker.flatten() {
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                let path = entry.path().to_path_buf();
                let path_str = path.to_string_lossy().to_string();
                
                // Identify project name (first dir after root /home/dstadel/projects)
                let project_name = self.extract_project_slug(&path);

                // REQ-AXO-003: Sub-project detection
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

                if self.is_supported(&path) {
                    let metadata = fs::metadata(&path);
                    let size = metadata.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                    let mtime = metadata.and_then(|m| m.modified()).map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64).unwrap_or(0);
                    
                    batch.push((path_str, project_name, size, mtime));
                    
                    if batch.len() >= 1000 {
                        total_files += batch.len();
                        if let Err(e) = graph.bulk_insert_files(&batch) {
                            tracing::error!("Bulk insert failed: {:?}", e);
                        }
                        batch.clear();
                        tracing::info!("... {} files mapped", total_files);
                        
                        // NEXUS HARMONY: Release DuckDB lock and let Elixir pull data.
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        }

        // Final batch
        if !batch.is_empty() {
            total_files += batch.len();
            let _ = graph.bulk_insert_files(&batch);
        }

        tracing::info!("🏁 Nexus Scan Complete: {} files mapped to DuckDB (status: pending).", total_files);
    }

    fn extract_project_slug(&self, path: &Path) -> String {
        // We assume projects root is /home/dstadel/projects
        // We want the directory name immediately under that.
        let projects_root = Path::new("/home/dstadel/projects");
        if let Ok(relative) = path.strip_prefix(projects_root) {
            if let Some(first_dir) = relative.components().next() {
                return first_dir.as_os_str().to_string_lossy().to_string();
            }
        }
        "global".to_string()
    }

    fn is_supported(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            crate::config::CONFIG.indexing.supported_extensions.iter().any(|e| e.to_lowercase() == ext_str)
        } else {
            false
        }
    }
}
