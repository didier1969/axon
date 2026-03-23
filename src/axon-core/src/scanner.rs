use ignore::WalkBuilder;
use std::path::PathBuf;
use std::fs;
use std::sync::{Arc, RwLock};
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

    pub fn scan(&self, graph: Option<Arc<RwLock<GraphStore>>>) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let project_name = self.root.file_name().unwrap_or_default().to_string_lossy().to_string();
        
        let mut builder = WalkBuilder::new(&self.root);
        builder.hidden(false) // On veut scanner les fichiers cachés si non ignorés
               .git_ignore(true);
               
        // Respect Custom .axonignore from global and local dirs
        let global_axonignore = std::path::Path::new("/home/dstadel/projects/.axonignore");
        if global_axonignore.exists() {
            let _ = builder.add_ignore(global_axonignore);
        }
        
        let local_axonignore = self.root.join(".axonignore");
        if local_axonignore.exists() {
            let _ = builder.add_ignore(local_axonignore);
        }

        let walker = builder.build();

        for entry in walker.flatten() {
            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                let path = entry.path().to_path_buf();
                
                // Dependency Extraction for Project Federation
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "pyproject.toml" || name == "Cargo.toml" || name == "mix.exs" {
                        if let Some(g) = &graph {
                            if let Ok(content) = fs::read_to_string(&path) {
                                let deps = extract_toml_dependencies(&content);
                                if let Ok(store) = g.read() {
                                    for dep in deps {
                                        let _ = store.insert_project_dependency(&project_name, &dep.to, &dep.path);
                                    }
                                }
                            }
                        }
                    }
                }

                if self.is_supported(&path) {
                    files.push(path);
                }
            }
        }
        files
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
        let files = scanner.scan(None);

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test.py"));
    }
}
