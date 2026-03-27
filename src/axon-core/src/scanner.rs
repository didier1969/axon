use ignore::WalkBuilder;
use std::path::PathBuf;
use std::fs;
use std::sync::Arc;
use parking_lot::RwLock;
use crate::graph::GraphStore;

pub struct ProjectDependency {
    pub to: String,
    pub path: String,
}

pub fn extract_toml_dependencies(content: &str) -> Vec<ProjectDependency> {
    let mut deps = Vec::new();
    if let Ok(parsed) = content.parse::<toml::Value>() {
        // Look for [tool.poetry.dependencies]
        if let Some(tool) = parsed.get("tool") {
            if let Some(poetry) = tool.get("poetry") {
                if let Some(dependencies) = poetry.get("dependencies") {
                    if let Some(table) = dependencies.as_table() {
                        for (k, v) in table {
                            if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                                deps.push(ProjectDependency {
                                    to: k.clone(),
                                    path: path.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
        
        // Look for [dependencies] (for Cargo.toml)
        if let Some(dependencies) = parsed.get("dependencies") {
            if let Some(table) = dependencies.as_table() {
                for (k, v) in table {
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        deps.push(ProjectDependency {
                            to: k.clone(),
                            path: path.to_string(),
                        });
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

    pub fn scan(&self, graph: Option<Arc<GraphStore>>, queue: Option<Arc<crate::queue::QueueStore>>) {
        let project_name = self.root.file_name().unwrap_or_default().to_string_lossy().to_string();
        
        tracing::info!("🚀 Starting deep scan of sector: {}", self.root.display());

        let mut builder = WalkBuilder::new(&self.root);
        builder.hidden(false) 
               .git_ignore(false) // LIBÉRATION : On ne laisse pas gitignore brider la découverte globale
               .parents(false)    // Ne pas remonter aux dossiers parents pour chercher des ignores
               .ignore(true);     // Garder seulement le respect des fichiers .ignore spécifiques
               
        let global_axonignore = std::path::Path::new("/home/dstadel/projects/.axonignore");
        if global_axonignore.exists() {
            let _ = builder.add_ignore(global_axonignore);
        }
        
        let local_axonignore = self.root.join(".axonignore");
        if local_axonignore.exists() {
            let _ = builder.add_ignore(local_axonignore);
        }

        let walker = builder.build();
        let mut total_seen = 0;
        let mut files_queued = 0;

        for entry in walker.flatten() {
            total_seen += 1;
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                let path = entry.path().to_path_buf();
                
                // Dependency Extraction for Project Federation
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "pyproject.toml" || name == "Cargo.toml" || name == "mix.exs" {
                        if let Some(g) = &graph {
                            if let Ok(content) = fs::read_to_string(&path) {
                                let deps = extract_toml_dependencies(&content);
                                for dep in deps {
                                    let _ = g.insert_project_dependency(&project_name, &dep.to, &dep.path);
                                }
                            }
                        }
                    }
                }

                if self.is_supported(&path) {
                    if let Some(ref q) = queue {
                        let path_str = path.to_string_lossy().to_string();
                        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).map(|sys_time| sys_time.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64).unwrap_or(0);
                        if let Err(e) = q.push(&path_str, mtime, "none", 0, 0) {
                            tracing::error!("Failed to enqueue file {}: {:?}", path_str, e);
                        } else {
                            files_queued += 1;
                        }
                    }
                }
            }
        }
        tracing::info!("🏁 Sector scan complete: {} files queued ({} vus) dans {}", files_queued, total_seen, self.root.display());
    }

    fn is_supported(&self, path: &std::path::Path) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            crate::config::CONFIG.indexing.supported_extensions.iter().any(|e| e.to_lowercase() == ext_str)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_python_toml_extraction() {
        let toml = r#"
        [tool.poetry.dependencies]
        my_local_lib = { path = "../my_local_lib" }
        "#;
        let deps = extract_toml_dependencies(toml);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].to, "my_local_lib");
    }

    #[test]
    fn test_scanner_filters_files() {
        let dir = tempdir().unwrap();
        let py_file = dir.path().join("test.py");
        let bin_file = dir.path().join("ignore.exe");
        
        fs::write(&py_file, "print(1)").unwrap();
        fs::write(&bin_file, "ignore me").unwrap();

        let scanner = Scanner::new(dir.path().to_str().unwrap());
        // In a real test, we would mock the queue or use an in-memory DB. 
        // For now, we just ensure it doesn't panic.
        scanner.scan(None, None);
    }
}
