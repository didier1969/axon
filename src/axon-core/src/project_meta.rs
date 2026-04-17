use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectIdentity {
    pub name: Option<String>,
    pub code: String,
    pub project_path: PathBuf,
    pub meta_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProjectMeta {
    code: Option<String>,
    name: Option<String>,
}

fn is_repo_root(path: &Path) -> bool {
    path.join("README.md").is_file()
        && path.join("docs").is_dir()
        && path.join("src/axon-core/Cargo.toml").is_file()
}

fn resolve_repo_root_from(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| is_repo_root(ancestor))
        .map(Path::to_path_buf)
}

fn resolve_repo_root() -> Option<PathBuf> {
    if let Ok(current_dir) = std::env::current_dir() {
        if let Some(repo_root) = resolve_repo_root_from(&current_dir) {
            return Some(repo_root);
        }
    }

    resolve_repo_root_from(Path::new(env!("CARGO_MANIFEST_DIR")))
}

fn candidate_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    let mut roots = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        roots.push(current_dir.clone());
        if let Some(parent) = current_dir.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    if let Some(repo_root) = resolve_repo_root() {
        roots.push(repo_root.clone());
        if let Some(parent) = repo_root.parent() {
            roots.push(parent.to_path_buf());
        }
    }

    for root in roots {
        if seen.insert(root.clone()) {
            dirs.push(root.clone());
        }
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && seen.insert(path.clone()) {
                dirs.push(path);
            }
        }
    }

    dirs
}

fn meta_path_for_directory(dir: &Path) -> PathBuf {
    dir.join(".axon").join("meta.json")
}

fn load_raw_project_meta(meta_path: &Path) -> Option<RawProjectMeta> {
    let content = fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn discover_project_identities() -> Vec<CanonicalProjectIdentity> {
    let mut identities = Vec::new();
    let mut seen = HashSet::new();

    for dir in candidate_directories() {
        let meta_path = meta_path_for_directory(&dir);
        let Some(raw) = load_raw_project_meta(&meta_path) else {
            continue;
        };
        let Some(code) = raw.code.map(|value| value.trim().to_ascii_uppercase()) else {
            continue;
        };
        if !is_valid_project_code(&code) {
            continue;
        }
        if seen.insert(code.clone()) {
            let project_path = dir.clone();
            identities.push(CanonicalProjectIdentity {
                name: raw
                    .name
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        dir.file_name()
                            .map(|value| value.to_string_lossy().trim().to_string())
                            .filter(|value| !value.is_empty())
                    }),
                code,
                project_path,
                meta_path,
            });
        }
    }

    identities.sort_by(|left, right| left.code.cmp(&right.code));
    identities
}

pub fn resolve_canonical_project_identity(project_code: &str) -> Result<CanonicalProjectIdentity> {
    let requested = project_code.trim();
    if requested.is_empty() {
        return Err(anyhow!("Code projet canonique vide"));
    }

    for dir in candidate_directories() {
        let meta_path = meta_path_for_directory(&dir);
        let Some(raw) = load_raw_project_meta(&meta_path) else {
            continue;
        };
        let Some(code_raw) = raw.code.as_deref().map(str::trim) else {
            continue;
        };
        let code = code_raw.to_ascii_uppercase();
        if code == requested {
            if !is_valid_project_code(&code) {
                return Err(anyhow!(
                    "Projet canonique `{}` trouvé dans `{}` mais `code` doit être alphanumérique sur 3 caractères",
                    requested,
                    meta_path.display()
                ));
            }
            return Ok(CanonicalProjectIdentity {
                name: raw
                    .name
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        dir.file_name()
                            .map(|value| value.to_string_lossy().trim().to_string())
                            .filter(|value| !value.is_empty())
                    }),
                code,
                project_path: dir.clone(),
                meta_path,
            });
        }
    }

    Err(anyhow!(
        "Projet canonique `{}` introuvable via `.axon/meta.json`",
        requested
    ))
}

pub fn is_valid_project_code(value: &str) -> bool {
    value.len() == 3 && value.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn registered_project_identities(
    graph: &crate::graph::GraphStore,
) -> Result<Vec<CanonicalProjectIdentity>> {
    let raw = graph.query_json(
        "SELECT COALESCE(project_code, ''), COALESCE(project_name, ''), COALESCE(project_path, '') \
         FROM soll.ProjectCodeRegistry \
         WHERE project_code NOT IN ('', 'PRO')",
    )?;
    let rows: Vec<Vec<String>> = serde_json::from_str(&raw).unwrap_or_default();
    let mut identities = Vec::new();

    for row in rows {
        if row.len() < 3 {
            continue;
        }

        let code = row[0].trim().to_ascii_uppercase();
        let project_name = row[1].trim().to_string();
        let project_path = row[2].trim().to_string();
        if !is_valid_project_code(&code) || project_path.is_empty() {
            continue;
        }

        let project_path_buf = canonicalize_lossy(Path::new(&project_path));
        let meta_path = project_path_buf.join(".axon").join("meta.json");
        let name = (!project_name.is_empty())
            .then_some(project_name)
            .or_else(|| {
                project_path_buf
                    .file_name()
                    .map(|value| value.to_string_lossy().trim().to_string())
                    .filter(|value| !value.is_empty())
            });

        identities.push(CanonicalProjectIdentity {
            name,
            code,
            project_path: project_path_buf,
            meta_path,
        });
    }

    identities.sort_by(|left, right| left.code.cmp(&right.code));
    Ok(identities)
}

pub fn resolve_registered_project_identity(
    graph: &crate::graph::GraphStore,
    project_code: &str,
) -> Result<CanonicalProjectIdentity> {
    let requested = project_code.trim().to_ascii_uppercase();
    if !is_valid_project_code(&requested) {
        return Err(anyhow!(
            "Code projet canonique invalide `{}`: exactement 3 caractères alphanumériques attendus",
            project_code.trim()
        ));
    }

    registered_project_identities(graph)?
        .into_iter()
        .find(|identity| identity.code == requested)
        .ok_or_else(|| {
            anyhow!(
                "Projet canonique `{}` introuvable dans soll.ProjectCodeRegistry",
                requested
            )
        })
}

pub fn resolve_registered_project_identity_for_path(
    graph: &crate::graph::GraphStore,
    path: &Path,
) -> Result<CanonicalProjectIdentity> {
    let candidate = canonicalize_lossy(path);
    registered_project_identities(graph)?
        .into_iter()
        .filter(|identity| candidate.starts_with(&identity.project_path))
        .max_by_key(|identity| identity.project_path.as_os_str().len())
        .ok_or_else(|| {
            anyhow!(
                "Aucun projet canonique enregistré pour le chemin `{}`",
                candidate.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_project_code, resolve_canonical_project_identity,
        resolve_registered_project_identity, resolve_registered_project_identity_for_path,
    };
    use std::path::Path;

    #[test]
    fn canonical_project_code_must_have_three_alphanumeric_characters() {
        assert!(is_valid_project_code("AXO"));
        assert!(is_valid_project_code("BK1"));
        assert!(!is_valid_project_code("axon"));
        assert!(!is_valid_project_code("AX"));
        assert!(!is_valid_project_code("A-O"));
    }

    #[test]
    fn repo_meta_can_resolve_current_project_identity() {
        let identity = resolve_canonical_project_identity("AXO").unwrap();
        assert_eq!(identity.code, "AXO");
        assert_eq!(identity.name.as_deref(), Some("axon"));
        assert!(identity.meta_path.ends_with(".axon/meta.json"));
    }

    #[test]
    fn registered_project_registry_can_resolve_code_and_path() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();

        let by_code = resolve_registered_project_identity(&store, "BKS").unwrap();
        assert_eq!(by_code.code, "BKS");
        assert_eq!(by_code.name.as_deref(), Some("BookingSystem"));

        let by_path = resolve_registered_project_identity_for_path(
            &store,
            Path::new("/home/dstadel/projects/BookingSystem/lib/app.ex"),
        )
        .unwrap();
        assert_eq!(by_path.code, "BKS");
    }

    #[test]
    fn unregistered_path_is_rejected() {
        let store = crate::tests::test_helpers::create_test_db().unwrap();
        let error = resolve_registered_project_identity_for_path(
            &store,
            Path::new("/tmp/axon-unregistered-project/main.rs"),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("Aucun projet canonique enregistré"));
    }
}
