use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::graph::GraphStore;
use crate::scanner::Scanner;

pub const HOT_PRIORITY: i64 = 900;

pub fn stage_hot_delta(store: &GraphStore, watch_root: &Path, path: &Path, priority: i64) -> Result<bool> {
    let scanner = Scanner::new(watch_root.to_string_lossy().as_ref());

    if !scanner.should_process_path(path) {
        return Ok(false);
    }

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_file() {
        return Ok(false);
    }

    let size = metadata.len() as i64;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let project_slug = scanner.project_slug_for_path(&absolute);

    store.upsert_hot_file(
        &absolute.to_string_lossy(),
        &project_slug,
        size,
        mtime,
        priority,
    )?;

    Ok(true)
}

pub fn stage_hot_deltas<I>(
    store: &GraphStore,
    watch_root: &Path,
    paths: I,
    priority: i64,
) -> Result<usize>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut unique = HashSet::new();
    let mut staged = 0usize;

    for path in paths {
        let dedup_key = std::fs::canonicalize(&path).unwrap_or(path.clone());
        if !unique.insert(dedup_key) {
            continue;
        }

        if stage_hot_delta(store, watch_root, &path, priority)? {
            staged += 1;
        }
    }

    Ok(staged)
}
