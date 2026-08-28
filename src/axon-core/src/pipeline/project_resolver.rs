//! Per-file project identity for the multi-project pipeline (DEC-AXO-081).
//!
//! REQ-AXO-902541 makes resolution fail-closed: an unresolved path is a typed
//! error at the admission boundary, never a fabricated `UNK` project code.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Canonical three-letter tenant identity carried through pipeline A.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectCode(String);

impl ProjectCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, ProjectResolutionError> {
        let value = value.into();
        if value.len() == 3
            && value.bytes().all(|b| b.is_ascii_uppercase())
            && value != "PRO"
            && value != "UNK"
        {
            Ok(Self(value))
        } else {
            Err(ProjectResolutionError::InvalidCode(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectResolutionError {
    UnregisteredPath(PathBuf),
    InvalidCode(String),
    InvalidRegistryPath { code: String, path: String },
    RegistryUnavailable,
}

impl fmt::Display for ProjectResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnregisteredPath(path) => {
                write!(f, "path is outside ProjectCodeRegistry: {}", path.display())
            }
            Self::InvalidCode(code) => write!(f, "invalid canonical project code: {code:?}"),
            Self::InvalidRegistryPath { code, path } => {
                write!(f, "invalid ProjectCodeRegistry path for {code}: {path:?}")
            }
            Self::RegistryUnavailable => f.write_str("ProjectCodeRegistry is unavailable"),
        }
    }
}

impl std::error::Error for ProjectResolutionError {}

/// Immutable longest-prefix snapshot of the canonical PG registry.
#[derive(Debug, Clone, Default)]
pub struct ProjectRegistrySnapshot {
    entries: Vec<(PathBuf, ProjectCode)>,
}

impl ProjectRegistrySnapshot {
    pub fn from_rows<I>(rows: I) -> Result<Self, ProjectResolutionError>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut entries = Vec::new();
        for (code, path) in rows {
            let project_code = ProjectCode::parse(code.clone())?;
            let project_path = PathBuf::from(&path);
            if path.is_empty() || !project_path.is_absolute() {
                return Err(ProjectResolutionError::InvalidRegistryPath { code, path });
            }
            entries.push((project_path, project_code));
        }
        entries.sort_by(|a, b| b.0.as_os_str().len().cmp(&a.0.as_os_str().len()));
        Ok(Self { entries })
    }

    pub fn resolve(&self, path: &Path) -> Option<&ProjectCode> {
        self.entries
            .iter()
            .find(|(project_path, _)| path.starts_with(project_path))
            .map(|(_, code)| code)
    }

    pub fn project_paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|(path, _)| path.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Hot-swappable registry resolver shared by admission and dynamic discovery.
#[derive(Clone)]
pub struct ProjectCodeResolver {
    snapshot: Arc<RwLock<ProjectRegistrySnapshot>>,
    constant: Option<ProjectCode>,
}

impl ProjectCodeResolver {
    pub fn from_snapshot(snapshot: ProjectRegistrySnapshot) -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            constant: None,
        }
    }

    pub fn resolve(&self, path: &Path) -> Result<ProjectCode, ProjectResolutionError> {
        if let Some(code) = &self.constant {
            return Ok(code.clone());
        }
        let guard = self
            .snapshot
            .read()
            .map_err(|_| ProjectResolutionError::RegistryUnavailable)?;
        guard
            .resolve(path)
            .cloned()
            .ok_or_else(|| ProjectResolutionError::UnregisteredPath(path.to_path_buf()))
    }

    /// Replace the whole snapshot atomically for readers.
    pub fn replace(&self, snapshot: ProjectRegistrySnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot;
        }
    }

    pub fn project_paths(&self) -> Vec<PathBuf> {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.project_paths())
            .unwrap_or_default()
    }
}

pub fn const_resolver(project_code: impl Into<String>) -> ProjectCodeResolver {
    let code = ProjectCode::parse(project_code.into())
        .expect("test/bench project_code must be canonical (three uppercase letters)");
    ProjectCodeResolver {
        snapshot: Arc::new(RwLock::new(ProjectRegistrySnapshot::default())),
        constant: Some(code),
    }
}

/// Extract the project_code from a v2 chunk id.
pub fn project_code_from_chunk_id(chunk_id: &str) -> Option<&str> {
    chunk_id.split("::").next().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_code_rejects_non_canonical_values() {
        assert!(ProjectCode::parse("AXO").is_ok());
        assert!(ProjectCode::parse("PRO").is_err());
        assert!(ProjectCode::parse("UNK").is_err());
        assert!(ProjectCode::parse("axo").is_err());
        assert!(ProjectCode::parse("ABCD").is_err());
    }

    #[test]
    fn snapshot_resolves_longest_prefix() {
        let resolver = ProjectCodeResolver::from_snapshot(
            ProjectRegistrySnapshot::from_rows([
                ("AAA".into(), "/home/u/projects/a".into()),
                ("BBB".into(), "/home/u/projects/a/b".into()),
            ])
            .unwrap(),
        );
        assert_eq!(
            resolver
                .resolve(Path::new("/home/u/projects/a/b/src/f.rs"))
                .unwrap()
                .as_str(),
            "BBB"
        );
        assert_eq!(
            resolver
                .resolve(Path::new("/home/u/projects/a/x.rs"))
                .unwrap()
                .as_str(),
            "AAA"
        );
    }

    #[test]
    fn unresolved_path_is_an_error_never_a_sentinel() {
        let resolver = ProjectCodeResolver::from_snapshot(
            ProjectRegistrySnapshot::from_rows([("AXO".into(), "/home/u/projects/axon".into())])
                .unwrap(),
        );
        assert!(matches!(
            resolver.resolve(Path::new("/tmp/foo.rs")),
            Err(ProjectResolutionError::UnregisteredPath(_))
        ));
    }

    #[test]
    fn replacement_is_immediately_visible() {
        let resolver = ProjectCodeResolver::from_snapshot(ProjectRegistrySnapshot::default());
        assert!(resolver.resolve(Path::new("/p/x.rs")).is_err());
        resolver
            .replace(ProjectRegistrySnapshot::from_rows([("NEW".into(), "/p".into())]).unwrap());
        assert_eq!(
            resolver.resolve(Path::new("/p/x.rs")).unwrap().as_str(),
            "NEW"
        );
    }

    #[test]
    fn invalid_registry_row_fails_the_whole_snapshot() {
        assert!(ProjectRegistrySnapshot::from_rows([(
            "lower".into(),
            "/valid/absolute/path".into(),
        )])
        .is_err());
        assert!(
            ProjectRegistrySnapshot::from_rows([("AXO".into(), "relative/path".into())]).is_err()
        );
    }

    #[test]
    fn chunk_project_code_parser_preserves_existing_contract() {
        assert_eq!(
            project_code_from_chunk_id("AXO::path::name::chunk"),
            Some("AXO")
        );
        assert_eq!(project_code_from_chunk_id("::bad"), None);
        assert_eq!(project_code_from_chunk_id(""), None);
    }
}
