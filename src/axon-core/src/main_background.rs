use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::path::{Path, PathBuf};

use axon_core::graph::GraphStore;
use axon_core::queue::QueueStore;
use axon_core::service_guard;
use axon_core::scanner::Scanner;
use axon_core::fs_watcher::{self, HOT_PRIORITY};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use notify_debouncer_full::notify::RecursiveMode;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
struct WatchTarget {
    path: PathBuf,
    recursive: bool,
}

pub(crate) fn start_memory_watchdog() {
    std::thread::spawn(|| {
        let page_size = 4096;
        let limit_bytes = memory_limit_bytes();
        let mut above_limit = false;
        loop {
            if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
                if let Some(rss_pages) = parse_rss_from_statm(&content) {
                    let rss_bytes = rss_pages * page_size;
                    if rss_bytes > limit_bytes {
                        if !above_limit {
                            error!(
                            "CRITICAL: Memory threshold reached ({} GB). Holding runtime in degraded mode instead of suicide...",
                            rss_bytes / 1024 / 1024 / 1024
                            );
                            above_limit = true;
                        }
                    } else if above_limit {
                        warn!(
                            "Memory watchdog: RSS returned below threshold ({} GB).",
                            rss_bytes / 1024 / 1024 / 1024
                        );
                        above_limit = false;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

pub(crate) fn spawn_autonomous_ingestor(
    store: Arc<GraphStore>,
    queue: Arc<QueueStore>,
) {
    tokio::spawn(async move {
        info!("Autonomous Ingestor: Ignition. Monitoring DuckDB for work...");
        let memory_limit = memory_limit_bytes();
        loop {
            let policy = claim_policy(
                queue.len(),
                current_rss_bytes(),
                memory_limit,
                service_guard::recent_peak_latency_ms(),
            );
            if policy.claim_count > 0 {
                if let Ok(files) = store.fetch_pending_batch(policy.claim_count) {
                    if !files.is_empty() {
                        debug!("Autonomous Ingestor: Feeding {} tasks to workers.", files.len());
                        for f in files {
                            let _ = queue.push(&f.path, 0, &f.trace_id, 0, 0, false);
                        }
                    }
                }
            }
            tokio::time::sleep(policy.sleep).await;
        }
    });
}

pub(crate) fn spawn_initial_scan(store: Arc<GraphStore>, projects_root: String) {
    std::thread::spawn(move || {
        info!("🚀 Auto-Ignition: Beginning initial workspace mapping...");
        axon_core::scanner::Scanner::new(&projects_root).scan(store);
        info!("✅ Auto-Ignition: Initial mapping sequence complete.");
    });
}

pub(crate) fn spawn_hot_delta_watcher(store: Arc<GraphStore>, projects_root: String) {
    std::thread::spawn(move || {
        let watch_root = PathBuf::from(projects_root);
        let preferred_project_root = std::env::var("AXON_PROJECT_ROOT")
            .ok()
            .map(PathBuf::from);
        let watcher_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            info!(
                "Rust FS watcher preparing targets under {}",
                watch_root.display()
            );

            let callback_root = watch_root.clone();
            let callback_store = store.clone();
            let rescan_guard = Arc::new(AtomicBool::new(false));
            let callback_rescan_guard = rescan_guard.clone();

            let mut debouncer = match new_debouncer(Duration::from_millis(750), None, move |result: DebounceEventResult| {
                handle_watcher_events(
                    callback_store.clone(),
                    callback_root.clone(),
                    callback_rescan_guard.clone(),
                    result,
                );
            }) {
                Ok(debouncer) => debouncer,
                Err(err) => {
                    error!("Rust FS watcher initialization failed: {}", err);
                    return;
                }
            };

            let mut armed = 0usize;
            for target in watch_targets(&watch_root, preferred_project_root.as_deref()) {
                let mode = if target.recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                match debouncer.watch(&target.path, mode) {
                    Ok(_) => {
                        armed += 1;
                        debug!(
                            "Rust FS watcher armed target {} ({})",
                            target.path.display(),
                            if target.recursive { "recursive" } else { "non-recursive" }
                        );
                    }
                    Err(err) => {
                        warn!(
                            "Rust FS watcher skipped target {}: {}",
                            target.path.display(),
                            err
                        );
                    }
                }
            }

            if armed == 0 {
                error!(
                    "Rust FS watcher failed to arm any target under {}",
                    watch_root.display()
                );
                return;
            }

            info!(
                "Rust FS watcher armed on {} target(s) under {}",
                armed,
                watch_root.display()
            );

            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }));

        if let Err(payload) = watcher_result {
            let reason = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            error!("Rust FS watcher thread panicked: {}", reason);
        }
    });
}

fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok())
}

fn current_rss_bytes() -> Option<u64> {
    let page_size = 4096;
    let content = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = parse_rss_from_statm(&content)?;
    Some(rss_pages * page_size)
}

fn memory_limit_bytes() -> u64 {
    let gb = std::env::var("AXON_MEMORY_LIMIT_GB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 2)
        .unwrap_or(14);
    gb * 1024 * 1024 * 1024
}

fn watch_targets(root: &Path, preferred_root: Option<&Path>) -> Vec<WatchTarget> {
    let mut targets = vec![WatchTarget {
        path: root.to_path_buf(),
        recursive: false,
    }];

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return targets,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if std::fs::read_dir(&path).is_err() {
            continue;
        }
        targets.push(WatchTarget {
            path,
            recursive: true,
        });
    }

    if let Some(preferred_root) = preferred_root {
        if let Some(index) = targets
            .iter()
            .position(|target| target.recursive && target.path == preferred_root)
        {
            let preferred = targets.remove(index);
            targets.insert(1, preferred);
        }
    }

    targets
}

#[derive(Debug, Clone, Copy)]
struct ClaimPolicy {
    claim_count: usize,
    sleep: std::time::Duration,
}

fn claim_policy(
    queue_len: usize,
    rss_bytes: Option<u64>,
    memory_limit: u64,
    recent_service_latency_ms: u64,
) -> ClaimPolicy {
    let rss_ratio = rss_bytes
        .map(|rss| rss as f64 / memory_limit.max(1) as f64)
        .unwrap_or(0.0);

    if recent_service_latency_ms >= 1_500 || rss_ratio >= 0.92 || queue_len >= 6_000 {
        return ClaimPolicy {
            claim_count: 0,
            sleep: std::time::Duration::from_millis(1_000),
        };
    }

    if recent_service_latency_ms >= 500 || rss_ratio >= 0.82 || queue_len >= 3_000 {
        return ClaimPolicy {
            claim_count: 100,
            sleep: std::time::Duration::from_millis(500),
        };
    }

    if queue_len >= 1_500 {
        return ClaimPolicy {
            claim_count: 500,
            sleep: std::time::Duration::from_millis(250),
        };
    }

    ClaimPolicy {
        claim_count: 2_000,
        sleep: std::time::Duration::from_millis(100),
    }
}

fn handle_watcher_events(
    store: Arc<GraphStore>,
    watch_root: std::path::PathBuf,
    rescan_guard: Arc<AtomicBool>,
    result: DebounceEventResult,
) {
    match result {
        Ok(events) => {
            let mut paths = Vec::new();
            let mut rescan_requested = false;

            for event in events {
                if event.need_rescan() {
                    rescan_requested = true;
                }
                paths.extend(event.paths.iter().cloned());
            }

            if rescan_requested
                && !rescan_guard.swap(true, Ordering::SeqCst)
            {
                let rescan_store = store.clone();
                let rescan_root = watch_root.clone();
                let rescan_guard_release = rescan_guard.clone();
                std::thread::spawn(move || {
                    warn!(
                        "Rust FS watcher requested a safety rescan on {}",
                        rescan_root.display()
                    );
                    Scanner::new(rescan_root.to_string_lossy().as_ref()).scan(rescan_store);
                    rescan_guard_release.store(false, Ordering::SeqCst);
                });
            }

            match fs_watcher::stage_hot_deltas(&store, &watch_root, paths, HOT_PRIORITY) {
                Ok(staged) if staged > 0 => {
                    debug!("Rust FS watcher staged {} hot delta(s).", staged);
                }
                Ok(_) => {}
                Err(err) => warn!("Rust FS watcher failed to stage hot delta(s): {}", err),
            }
        }
        Err(errors) => {
            for err in errors {
                warn!("Rust FS watcher event error: {}", err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{claim_policy, handle_watcher_events, memory_limit_bytes, watch_targets};
    use axon_core::graph::GraphStore;
    use notify_debouncer_full::notify::{Event, EventKind};
    use notify_debouncer_full::notify::event::ModifyKind;
    use notify_debouncer_full::DebouncedEvent;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    #[test]
    fn test_memory_limit_uses_default_when_env_missing() {
        unsafe { std::env::remove_var("AXON_MEMORY_LIMIT_GB"); }
        assert_eq!(memory_limit_bytes(), 14 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_memory_limit_uses_env_when_valid() {
        unsafe { std::env::set_var("AXON_MEMORY_LIMIT_GB", "10"); }
        assert_eq!(memory_limit_bytes(), 10 * 1024 * 1024 * 1024);
        unsafe { std::env::remove_var("AXON_MEMORY_LIMIT_GB"); }
    }

    #[test]
    fn test_claim_policy_is_fast_when_system_is_healthy() {
        let policy = claim_policy(
            200,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.claim_count, 2_000);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(100));
    }

    #[test]
    fn test_claim_policy_slows_when_queue_grows() {
        let policy = claim_policy(
            2_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.claim_count, 500);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(250));
    }

    #[test]
    fn test_claim_policy_enters_guard_mode_when_queue_is_high() {
        let policy = claim_policy(
            3_500,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.claim_count, 100);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_claim_policy_pauses_claiming_when_pressure_is_critical() {
        let policy = claim_policy(500, Some(95 * 1024 * 1024), 100 * 1024 * 1024, 0);
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_claim_policy_slows_when_live_service_latency_rises() {
        let policy = claim_policy(200, Some(2 * 1024 * 1024 * 1024), 10 * 1024 * 1024 * 1024, 700);
        assert_eq!(policy.claim_count, 100);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_claim_policy_pauses_when_live_service_is_critically_slow() {
        let policy = claim_policy(200, Some(2 * 1024 * 1024 * 1024), 10 * 1024 * 1024 * 1024, 2_000);
        assert_eq!(policy.claim_count, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(1_000));
    }

    #[test]
    fn test_handle_watcher_events_stages_modified_file_as_hot_delta() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let file_path = project.join("watch.ex");
        std::fs::write(&file_path, "defmodule Watch do\nend\n").unwrap();

        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        let event = DebouncedEvent::new(
            Event {
                kind: EventKind::Modify(ModifyKind::Data(notify_debouncer_full::notify::event::DataChange::Any)),
                paths: vec![file_path.clone()],
                attrs: Default::default(),
            },
            std::time::Instant::now(),
        );

        handle_watcher_events(
            store.clone(),
            root.to_path_buf(),
            Arc::new(AtomicBool::new(false)),
            Ok(vec![event]),
        );

        let row = store
            .query_json(&format!(
                "SELECT status, priority FROM File WHERE path = '{}'",
                file_path.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert!(row.contains("pending"));
        assert!(row.contains("900"));
    }

    #[test]
    fn test_watch_targets_split_root_and_accessible_projects() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("proj_a")).unwrap();
        std::fs::create_dir_all(root.join("proj_b")).unwrap();
        std::fs::write(root.join("README.md"), "# root").unwrap();

        let targets = watch_targets(root, None);
        let rendered: Vec<(String, bool)> = targets
            .into_iter()
            .map(|target| (target.path.to_string_lossy().to_string(), target.recursive))
            .collect();

        assert!(
            rendered.iter().any(|(path, recursive): &(String, bool)| path == &root.to_string_lossy() && !*recursive),
            "La racine doit etre surveillee en non-recursif"
        );
        assert!(
            rendered.iter().any(|(path, recursive): &(String, bool)| path.ends_with("proj_a") && *recursive),
            "Chaque projet accessible doit etre surveille recursivement"
        );
        assert!(
            rendered.iter().any(|(path, recursive): &(String, bool)| path.ends_with("proj_b") && *recursive),
            "Chaque projet accessible doit etre surveille recursivement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_watch_targets_skip_unreadable_projects() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let root = temp.path();
        let locked = root.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let targets = watch_targets(root, None);
        let rendered: Vec<String> = targets
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();

        assert!(
            !rendered.iter().any(|path: &String| path.ends_with("locked")),
            "Un sous-arbre illisible ne doit pas bloquer l'armement global du watcher"
        );

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_watch_targets_prioritize_active_project() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let proj_a = root.join("proj_a");
        let proj_b = root.join("proj_b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();

        let targets = watch_targets(root, Some(proj_b.as_path()));
        let rendered: Vec<String> = targets
            .into_iter()
            .map(|target| target.path.to_string_lossy().to_string())
            .collect();

        assert_eq!(rendered[0], root.to_string_lossy(), "La racine doit rester observee en premier");
        assert_eq!(rendered[1], proj_b.to_string_lossy(), "Le projet actif doit etre arme avant les autres");
    }
}
