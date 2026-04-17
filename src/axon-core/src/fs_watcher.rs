use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::file_ingress_guard::{GuardDecision, SharedFileIngressGuard};
use crate::graph::GraphStore;
use crate::ingress_buffer::{
    record_blocked_subtree_hint, IngressCause, IngressFileEvent, IngressSource, SharedIngressBuffer,
};
use crate::scanner::Scanner;
use crate::watcher_probe;

pub const HOT_PRIORITY: i64 = 900;
const CONTROL_FILE_SUBTREE_HINT_COOLDOWN_MS: u64 = 60_000;

pub fn stage_hot_delta(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    path: &Path,
    priority: i64,
) -> Result<bool> {
    Ok(stage_hot_path_delta_count(store, watch_root, project_code, path, priority, None)? > 0)
}

pub fn stage_hot_delta_with_guard(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    path: &Path,
    priority: i64,
    guard: &SharedFileIngressGuard,
) -> Result<bool> {
    Ok(
        stage_hot_path_delta_count(store, watch_root, project_code, path, priority, Some(guard))?
            > 0,
    )
}

pub fn enqueue_hot_delta_with_guard(
    watch_root: &Path,
    project_code: &str,
    path: &Path,
    priority: i64,
    guard: &SharedFileIngressGuard,
    ingress: &SharedIngressBuffer,
) -> Result<bool> {
    Ok(enqueue_hot_path_delta_count(
        watch_root,
        project_code,
        path,
        priority,
        Some(guard),
        ingress,
    )? > 0)
}

fn control_file_rescan_scope(path: &Path, watch_root: &Path) -> PathBuf {
    let canonical_root =
        std::fs::canonicalize(watch_root).unwrap_or_else(|_| watch_root.to_path_buf());
    let candidate = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let scoped = if candidate.ends_with(Path::new(".git/info/exclude")) {
        candidate
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
            .or_else(|| candidate.parent().map(Path::to_path_buf))
    } else {
        candidate.parent().map(Path::to_path_buf)
    }
    .unwrap_or_else(|| canonical_root.clone());

    if scoped.starts_with(&canonical_root) {
        scoped
    } else {
        canonical_root
    }
}

fn stage_hot_path_delta_count(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    path: &Path,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
) -> Result<usize> {
    let scanner = Scanner::new(watch_root.to_string_lossy().as_ref(), project_code);
    if scanner.is_ignore_control_path(path) {
        let control_scope = control_file_rescan_scope(path, watch_root);
        if !scanner.should_descend_into_directory(&control_scope)
            || !scanner.should_buffer_subtree_hint(&control_scope)
        {
            record_blocked_subtree_hint();
            watcher_probe::record(
                "watcher.control_file",
                Some(path),
                format!(
                    "reason=ignore_control_changed action=skip_scope_hint scope={} decision=blocked_control_scope",
                    control_scope.display()
                ),
            );
            return Ok(0);
        }
        match store.reconcile_ignore_rules_for_scope(&control_scope, &scanner) {
            Ok(stats) => watcher_probe::record(
                "watcher.control_file.reconcile",
                Some(path),
                format!(
                    "scope={} scanned={} newly_ignored={} newly_included={} dry_run={}",
                    control_scope.display(),
                    stats.scanned,
                    stats.newly_ignored,
                    stats.newly_included,
                    stats.dry_run
                ),
            ),
            Err(err) => watcher_probe::record(
                "watcher.control_file.reconcile",
                Some(path),
                format!("error={}", err),
            ),
        }
        watcher_probe::record(
            "watcher.control_file",
            Some(path),
            format!(
                "reason=ignore_control_changed action=rescan_scope scope={} decision={}",
                control_scope.display(),
                scanner.explain_ignore_decision(path, false)
            ),
        );
        return stage_hot_path_delta_count(
            store,
            watch_root,
            project_code,
            &control_scope,
            priority,
            guard,
        );
    }

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tombstoned = store.tombstone_missing_path(path)?;
            if tombstoned > 0 {
                if let Some(shared_guard) = guard {
                    shared_guard
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .record_tombstone(path);
                }
            }
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
        if !scanner.should_descend_into_directory(path) {
            watcher_probe::record(
                "watcher.filtered",
                Some(path),
                "reason=ignored_directory_event",
            );
            return Ok(0);
        }
        if !scanner.should_buffer_subtree_hint(path) {
            record_blocked_subtree_hint();
            watcher_probe::record(
                "watcher.filtered",
                Some(path),
                "reason=blocked_subtree_hint_segment",
            );
            return Ok(0);
        }
        let mut staged = 0usize;
        for entry in WalkDir::new(path)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let candidate = entry.path();
            if !entry.file_type().is_file() || !scanner.should_process_path(candidate) {
                continue;
            }
            staged +=
                stage_single_file_delta(store, &scanner, candidate, priority, guard)? as usize;
        }
        return Ok(staged);
    }

    Ok(stage_single_file_delta(store, &scanner, path, priority, guard)? as usize)
}

pub fn stage_hot_deltas<I>(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    paths: I,
    priority: i64,
) -> Result<usize>
where
    I: IntoIterator<Item = PathBuf>,
{
    stage_hot_deltas_inner(store, watch_root, project_code, paths, priority, None)
}

pub fn stage_hot_deltas_with_guard<I>(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    paths: I,
    priority: i64,
    guard: &SharedFileIngressGuard,
) -> Result<usize>
where
    I: IntoIterator<Item = PathBuf>,
{
    stage_hot_deltas_inner(
        store,
        watch_root,
        project_code,
        paths,
        priority,
        Some(guard),
    )
}

pub fn enqueue_hot_deltas_with_guard<I>(
    watch_root: &Path,
    project_code: &str,
    paths: I,
    priority: i64,
    guard: &SharedFileIngressGuard,
    ingress: &SharedIngressBuffer,
) -> Result<usize>
where
    I: IntoIterator<Item = PathBuf>,
{
    enqueue_hot_deltas_inner(
        watch_root,
        project_code,
        paths,
        priority,
        Some(guard),
        ingress,
    )
}

fn stage_hot_deltas_inner<I>(
    store: &GraphStore,
    watch_root: &Path,
    project_code: &str,
    paths: I,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
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

        staged +=
            stage_hot_path_delta_count(store, watch_root, project_code, &path, priority, guard)?;
    }

    Ok(staged)
}

fn enqueue_hot_deltas_inner<I>(
    watch_root: &Path,
    project_code: &str,
    paths: I,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
    ingress: &SharedIngressBuffer,
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

        staged += enqueue_hot_path_delta_count(
            watch_root,
            project_code,
            &path,
            priority,
            guard,
            ingress,
        )?;
    }

    Ok(staged)
}

fn stage_single_file_delta(
    store: &GraphStore,
    scanner: &Scanner,
    path: &Path,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
) -> Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let tombstoned = store.tombstone_missing_path(path)?;
            if tombstoned > 0 {
                if let Some(shared_guard) = guard {
                    shared_guard
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .record_tombstone(path);
                }
            }
            if tombstoned == 0 {
                watcher_probe::record(
                    "watcher.missing",
                    Some(path),
                    "reason=single_file_not_found",
                );
            }
            return Ok(tombstoned > 0);
        }
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_file() || !scanner.should_process_path(path) {
        watcher_probe::record(
            "watcher.filtered",
            Some(path),
            "reason=single_file_not_processable",
        );
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
    let project_code = match scanner.project_code_for_path(store, &absolute) {
        Ok(project_code) => project_code,
        Err(err) => {
            watcher_probe::record(
                "watcher.filtered",
                Some(&absolute),
                format!("reason=unregistered_project_path error={}", err),
            );
            return Ok(false);
        }
    };

    if let Some(shared_guard) = guard {
        let decision = shared_guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .should_stage(&absolute, mtime, size);
        if decision == GuardDecision::SkipUnchanged {
            watcher_probe::record("watcher.filtered", Some(&absolute), "reason=guard_skip");
            return Ok(false);
        }
    }

    store.upsert_hot_file(
        &absolute.to_string_lossy(),
        &project_code,
        size,
        mtime,
        priority,
    )?;

    if let Some(shared_guard) = guard {
        if let Some(row) = store.fetch_file_ingress_row(&absolute.to_string_lossy())? {
            shared_guard
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .record_committed_row(row);
        }
    }

    watcher_probe::record(
        "watcher.staged",
        Some(&absolute),
        format!(
            "project={} priority={} size={} mtime={}",
            project_code, priority, size, mtime
        ),
    );

    Ok(true)
}

fn enqueue_hot_path_delta_count(
    watch_root: &Path,
    project_code: &str,
    path: &Path,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
    ingress: &SharedIngressBuffer,
) -> Result<usize> {
    let scanner = Scanner::new(watch_root.to_string_lossy().as_ref(), project_code);
    if scanner.is_ignore_control_path(path) {
        let control_scope = control_file_rescan_scope(path, watch_root);
        if !scanner.should_descend_into_directory(&control_scope)
            || !scanner.should_buffer_subtree_hint(&control_scope)
        {
            record_blocked_subtree_hint();
            watcher_probe::record(
                "watcher.control_file",
                Some(path),
                format!(
                    "reason=ignore_control_changed action=skip_scope_hint scope={} decision=blocked_control_scope",
                    control_scope.display()
                ),
            );
            return Ok(0);
        }
        watcher_probe::record(
            "watcher.control_file",
            Some(path),
            format!(
                "reason=ignore_control_changed action=enqueue_scope_hint scope={} decision={}",
                control_scope.display(),
                scanner.explain_ignore_decision(path, false)
            ),
        );
        ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .record_subtree_hint_with_cooldown(
                control_scope.to_string_lossy().to_string(),
                priority,
                IngressSource::Watcher,
                CONTROL_FILE_SUBTREE_HINT_COOLDOWN_MS,
            );
        return Ok(1);
    }

    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut locked = ingress.lock().unwrap_or_else(|poison| poison.into_inner());
            locked.record_tombstone(path.to_string_lossy().to_string(), IngressSource::Watcher);
            watcher_probe::record("watcher.buffered_tombstone", Some(path), "reason=not_found");
            return Ok(1);
        }
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_dir() && !scanner.should_process_path(path) {
        watcher_probe::record("watcher.filtered", Some(path), "reason=not_processable");
        return Ok(0);
    }

    if metadata.is_dir() {
        if !scanner.should_descend_into_directory(path) {
            watcher_probe::record(
                "watcher.filtered",
                Some(path),
                "reason=ignored_directory_event",
            );
            return Ok(0);
        }
        if !scanner.should_buffer_subtree_hint(path) {
            record_blocked_subtree_hint();
            watcher_probe::record(
                "watcher.filtered",
                Some(path),
                "reason=blocked_subtree_hint_segment",
            );
            return Ok(0);
        }
        let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .record_subtree_hint(
                absolute.to_string_lossy().to_string(),
                priority,
                IngressSource::Watcher,
            );
        watcher_probe::record(
            "watcher.buffered_subtree_hint",
            Some(&absolute),
            "reason=directory_event",
        );
        return Ok(1);
    }

    enqueue_single_file_delta(&scanner, path, priority, guard, ingress)
}

fn enqueue_single_file_delta(
    scanner: &Scanner,
    path: &Path,
    priority: i64,
    guard: Option<&SharedFileIngressGuard>,
    ingress: &SharedIngressBuffer,
) -> Result<usize> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            ingress
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .record_tombstone(path.to_string_lossy().to_string(), IngressSource::Watcher);
            watcher_probe::record(
                "watcher.buffered_tombstone",
                Some(path),
                "reason=single_file_not_found",
            );
            return Ok(1);
        }
        Err(err) => return Err(err.into()),
    };

    if !metadata.is_file() || !scanner.should_process_path(path) {
        watcher_probe::record(
            "watcher.filtered",
            Some(path),
            "reason=single_file_not_processable",
        );
        return Ok(0);
    }

    let size = metadata.len() as i64;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let project_code = match scanner.project_code.trim() {
        "" => {
            watcher_probe::record(
                "watcher.filtered",
                Some(&absolute),
                "reason=missing_explicit_project_context_for_buffered_delta",
            );
            return Ok(0);
        }
        explicit => explicit.to_string(),
    };

    if let Some(shared_guard) = guard {
        let decision = shared_guard
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .should_stage(&absolute, mtime, size);
        if decision == GuardDecision::SkipUnchanged {
            watcher_probe::record("watcher.filtered", Some(&absolute), "reason=guard_skip");
            return Ok(0);
        }
    }

    ingress
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .record_file(IngressFileEvent::new(
            absolute.to_string_lossy().to_string(),
            project_code.clone(),
            size,
            mtime,
            priority,
            IngressSource::Watcher,
            IngressCause::Modified,
        ));

    watcher_probe::record(
        "watcher.buffered",
        Some(&absolute),
        format!(
            "project={} priority={} size={} mtime={}",
            project_code, priority, size, mtime
        ),
    );

    Ok(1)
}
