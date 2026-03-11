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
        let walker = WalkBuilder::new(&self.root)
            .hidden(false) // On veut scanner les fichiers cachés si non ignorés
            .git_ignore(true)
            .build();

        for result in walker {
            if let Ok(entry) = result {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    let path = entry.path().to_path_buf();
                    if self.is_supported(&path) {
                        files.push(path);
                    }
                }
            }
        }
        files
    }

    fn is_supported(&self, path: &PathBuf) -> bool {
        if let Some(ext) = path.extension() {
            match ext.to_str() {
                Some("py") | Some("ex") | Some("exs") | Some("rs") | Some("ts") | Some("js") => true,
                _ => false,
            }
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
        let txt_file = dir.path().join("ignore.txt");
        
        fs::write(&py_file, "print(1)").unwrap();
        fs::write(&txt_file, "ignore me").unwrap();

        let scanner = Scanner::new(dir.path().to_str().unwrap());
        let files = scanner.scan();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test.py"));
    }
}
