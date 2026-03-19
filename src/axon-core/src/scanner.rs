use ignore::WalkBuilder;
use std::path::PathBuf;

pub struct Scanner {
    pub root: PathBuf,
}

impl Scanner {
    pub fn new(path: &str) -> Self {
        Self {
            root: PathBuf::from(path),
        }
    }

    pub fn scan(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        
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
    fn test_scanner_filters_files() {
        let dir = tempdir().unwrap();
        let py_file = dir.path().join("test.py");
        let bin_file = dir.path().join("ignore.exe");
        
        fs::write(&py_file, "print(1)").unwrap();
        fs::write(&bin_file, "ignore me").unwrap();

        let scanner = Scanner::new(dir.path().to_str().unwrap());
        let files = scanner.scan();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test.py"));
    }
}
