use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::graph::GraphStore;
use crate::scanner::Scanner;
use crate::watcher_probe;

pub const HOT_PRIORITY: i64 = 900;

pub fn stage_hot_delta(store: &GraphStore, watch_root: &Path, path: &Path, priority: i64) -> Result<bool> {
    Ok(stage_hot_path_delta_count(store, watch_root, path, priority)? > 0)
}

fn stage_hot_path_delta_count(
    store: &GraphStore,
    watch_root: &Path,
    path: &Path,
    priority: i64,
) -> Result<usize> {
    let scanner = Scanner::new(watch_root.to_string_lossy().as_ref());

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tombstoned = store.tombstone_missing_path(path)?;
            if tombstoned == 0 {
                watcher_probe::record("watcher.missing", Some(path), "reason=not_found");
            }
            return Ok(tombstoned);
        }
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_dir() && !scanner.should_process_path(path) {
        watcher_probe::record("watcher.filtered", Some(path), "reason=not_processable");
        return Ok(0);
    }

    if metadata.is_dir() {
        let mut staged = 0usize;
        for entry in WalkDir::new(path).into_iter().filter_map(|entry| entry.ok()) {
            let candidate = entry.path();
            if !entry.file_type().is_file() || !scanner.should_process_path(candidate) {
                continue;
            }
            staged += stage_single_file_delta(store, &scanner, candidate, priority)? as usize;
        }
        return Ok(staged);
    }

    Ok(stage_single_file_delta(store, &scanner, path, priority)? as usize)
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

        staged += stage_hot_path_delta_count(store, watch_root, &path, priority)?;
    }

    Ok(staged)
}

fn stage_single_file_delta(
    store: &GraphStore,
    scanner: &Scanner,
    path: &Path,
    priority: i64,
) -> Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tombstoned = store.tombstone_missing_path(path)?;
            if tombstoned == 0 {
                watcher_probe::record("watcher.missing", Some(path), "reason=single_file_not_found");
            }
            return Ok(tombstoned > 0);
        }
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_file() || !scanner.should_process_path(path) {
        watcher_probe::record("watcher.filtered", Some(path), "reason=single_file_not_processable");
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

    watcher_probe::record(
        "watcher.staged",
        Some(&absolute),
        format!("project={} priority={} size={} mtime={}", project_slug, priority, size, mtime),
    );

    Ok(true)
}
