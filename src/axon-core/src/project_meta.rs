use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectIdentity {
    pub slug: String,
    pub code: String,
    pub meta_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProjectMeta {
    slug: Option<String>,
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
        let Some(slug) = raw.slug.map(|value| value.trim().to_string()) else {
            continue;
        };
        let Some(code) = raw.code.map(|value| value.trim().to_ascii_uppercase()) else {
            continue;
        };
        if slug.is_empty() || !is_valid_project_code(&code) {
            continue;
        }
        if seen.insert(slug.clone()) {
            identities.push(CanonicalProjectIdentity {
                slug,
                code,
                meta_path,
            });
        }
    }

    identities.sort_by(|left, right| left.slug.cmp(&right.slug));
    identities
}

pub fn resolve_canonical_project_identity(project_slug: &str) -> Result<CanonicalProjectIdentity> {
    let requested = project_slug.trim();
    if requested.is_empty() {
        return Err(anyhow!("Projet canonique vide"));
    }

    let mut canonical_alias_hint: Option<String> = None;

    for dir in candidate_directories() {
        let meta_path = meta_path_for_directory(&dir);
        let Some(raw) = load_raw_project_meta(&meta_path) else {
            continue;
        };
        let Some(slug) = raw.slug.as_deref().map(str::trim) else {
            continue;
        };
        if slug == requested {
            let Some(code_raw) = raw.code.as_deref().map(str::trim) else {
                return Err(anyhow!(
                    "Projet canonique `{}` trouvé dans `{}` mais `code` manque dans `.axon/meta.json`",
                    requested,
                    meta_path.display()
                ));
            };
            let code = code_raw.to_ascii_uppercase();
            if !is_valid_project_code(&code) {
                return Err(anyhow!(
                    "Projet canonique `{}` trouvé dans `{}` mais `code` doit être alphanumérique sur 3 caractères",
                    requested,
                    meta_path.display()
                ));
            }
            return Ok(CanonicalProjectIdentity {
                slug: requested.to_string(),
                code,
                meta_path,
            });
        }

        if raw.code.as_deref().is_some_and(|code| code.trim().eq_ignore_ascii_case(requested))
            || raw.name.as_deref().is_some_and(|name| name.trim() == requested)
        {
            canonical_alias_hint = Some(slug.to_string());
        }
    }

    if let Some(canonical_slug) = canonical_alias_hint {
        return Err(anyhow!(
            "Projet canonique attendu `{}`, pas `{}`. Utiliser le slug déclaré dans `.axon/meta.json`",
            canonical_slug,
            requested
        ));
    }

    Err(anyhow!(
        "Projet canonique `{}` introuvable via `.axon/meta.json`",
        requested
    ))
}

pub fn is_valid_project_code(value: &str) -> bool {
    value.len() == 3 && value.chars().all(|ch| ch.is_ascii_alphanumeric())
}
