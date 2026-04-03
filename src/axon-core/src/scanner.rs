use crate::file_ingress_guard::{GuardDecision, SharedFileIngressGuard};
use crate::graph::GraphStore;
use crate::ingress_buffer::{
    IngressBuffer, IngressCause, IngressFileEvent, IngressSource, SharedIngressBuffer,
};
use crate::service_guard;
use ignore::{gitignore::Gitignore, WalkBuilder};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{error, info};

struct ProjectDependency {
    path: String,
    to: String,
}

pub struct Scanner {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct DiscoveryPolicy {
    sleep: std::time::Duration,
}

impl Scanner {
    pub fn new(root: &str) -> Self {
        Self {
            root: PathBuf::from(root),
        }
    }

    pub fn scan(&self, graph: Arc<GraphStore>) {
        self.scan_with_guard_and_ingress(graph, None, None);
    }

    pub fn scan_with_guard(&self, graph: Arc<GraphStore>, guard: Option<&SharedFileIngressGuard>) {
        self.scan_with_guard_and_ingress(graph, guard, None);
    }

    pub fn scan_with_guard_and_ingress(
        &self,
        graph: Arc<GraphStore>,
        guard: Option<&SharedFileIngressGuard>,
        ingress: Option<&SharedIngressBuffer>,
    ) {
        info!(
            "Lattice Engine: Initializing recursive traversal on {:?}",
            self.root
        );
        let total_files = self.scan_path(graph, &self.root, guard, ingress);
        info!(
            "🏁 Nexus Scan Complete: {} files mapped to DuckDB (status: pending).",
            total_files
        );
    }

    pub fn scan_subtree(&self, graph: Arc<GraphStore>, subtree: &Path) {
        self.scan_subtree_with_guard_and_ingress(graph, subtree, None, None);
    }

    pub fn scan_subtree_with_guard(
        &self,
        graph: Arc<GraphStore>,
        subtree: &Path,
        guard: Option<&SharedFileIngressGuard>,
    ) {
        self.scan_subtree_with_guard_and_ingress(graph, subtree, guard, None);
    }

    pub fn scan_subtree_with_guard_and_ingress(
        &self,
        graph: Arc<GraphStore>,
        subtree: &Path,
        guard: Option<&SharedFileIngressGuard>,
        ingress: Option<&SharedIngressBuffer>,
    ) {
        info!(
            "Lattice Engine: Prioritizing hot subtree traversal on {:?}",
            subtree
        );
        let total_files = self.scan_path(graph, subtree, guard, ingress);
        info!(
            "🔥 Hot subtree scan complete: {} files mapped from {:?}.",
            total_files, subtree
        );
    }

    pub fn should_process_path(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        if self.is_ignored_by_axon_ignore(path, false) {
            return false;
        }
        self.is_supported(path)
    }

    pub fn should_descend_into_directory(&self, path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        if self.is_ignored_by_axon_ignore(path, true) {
            return false;
        }
        !self.path_has_ignored_directory_noise(path)
    }

    pub fn should_buffer_subtree_hint(&self, path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        if !self.should_descend_into_directory(path) {
            return false;
        }
        !self.path_has_blocked_subtree_hint_segment(path)
    }

    pub fn project_slug_for_path(&self, path: &Path) -> String {
        self.extract_project_slug(path)
    }

    fn extract_project_slug(&self, path: &Path) -> String {
        if let Ok(relative) = path.strip_prefix(&self.root) {
            if let Some(first_dir) = relative.components().next() {
                return first_dir.as_os_str().to_string_lossy().to_string();
            }
        }
        "global".to_string()
    }

    fn build_walker_from(&self, start: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(start);
        builder.hidden(false);
        builder.git_ignore(false);
        builder.git_global(false);
        builder.git_exclude(false);
        builder.add_custom_ignore_filename(".axonignore");
        builder.add_custom_ignore_filename(".axonignore.local");
        builder
    }

    fn is_ignored_by_axon_ignore(&self, path: &Path, is_dir: bool) -> bool {
        let absolute = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => path.to_path_buf(),
        };
        let root = match std::fs::canonicalize(&self.root) {
            Ok(root) => root,
            Err(_) => self.root.clone(),
        };

        if !absolute.starts_with(&root) {
            return true;
        }

        let mut decision = None;
        for dir in ancestor_chain(&root, &absolute) {
            for ignore_name in [".axonignore", ".axonignore.local"] {
                let ignore_path = dir.join(ignore_name);
                if ignore_path.exists() {
                    let (matcher, _err) = Gitignore::new(&ignore_path);
                    let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                    if matched.is_ignore() {
                        decision = Some(true);
                    } else if matched.is_whitelist() {
                        decision = Some(false);
                    }
                }
            }
        }

        decision.unwrap_or(false)
    }

    fn is_supported(&self, path: &Path) -> bool {
        // 1. DIRECTORY NOISE FILTER (Strict)
        if self.path_has_ignored_directory_noise(path) {
            return false;
        }

        // 2. HIDDEN FILE FILTER
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') && name != ".env" {
                return false;
            }
        }

        // 3. EXTENSION FILTER
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            crate::config::CONFIG
                .indexing
                .supported_extensions
                .iter()
                .any(|e| e.to_lowercase() == ext_str)
        } else {
            false
        }
    }

    fn path_has_ignored_directory_noise(&self, path: &Path) -> bool {
        let relative = match path.strip_prefix(&self.root) {
            Ok(path) => path,
            Err(_) => return true,
        };
        relative.components().any(|component| {
            let Some(segment) = component.as_os_str().to_str() else {
                return false;
            };
            crate::config::CONFIG
                .indexing
                .ignored_directory_segments
                .iter()
                .any(|ignored| ignored == segment)
        })
    }

    fn path_has_blocked_subtree_hint_segment(&self, path: &Path) -> bool {
        let relative = match path.strip_prefix(&self.root) {
            Ok(path) => path,
            Err(_) => return true,
        };
        relative.components().any(|component| {
            let Some(segment) = component.as_os_str().to_str() else {
                return false;
            };
            crate::config::CONFIG
                .indexing
                .blocked_subtree_hint_segments
                .iter()
                .any(|blocked| blocked == segment)
        })
    }

    fn scan_path(
        &self,
        graph: Arc<GraphStore>,
        start: &Path,
        guard: Option<&SharedFileIngressGuard>,
        ingress: Option<&SharedIngressBuffer>,
    ) -> usize {
        let mut batch = Vec::new();
        let mut total_files = 0;
        let walker = self.build_walker_from(start);

        for entry in walker.build().filter_map(|e| e.ok()) {
            let path = entry.path();

            if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                if !self.is_supported(path) {
                    continue;
                }

                let project_name = self.extract_project_slug(path);

                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "pyproject.toml" || name == "Cargo.toml" || name == "mix.exs" {
                        if let Ok(content) = fs::read_to_string(path) {
                            let deps = extract_toml_dependencies(&content);
                            for dep in deps {
                                let _ = graph.insert_project_dependency(
                                    &project_name,
                                    &dep.to,
                                    &dep.path,
                                );
                            }
                        }
                    }
                }

                let path_str = if let Ok(abs_path) = fs::canonicalize(path) {
                    abs_path.to_string_lossy().to_string()
                } else {
                    path.to_string_lossy().to_string()
                };

                let metadata = fs::metadata(path);
                let size = metadata.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                let mtime = metadata
                    .as_ref()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64)
                    .unwrap_or(0);

                if let Some(shared_guard) = guard {
                    let decision = shared_guard
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .should_stage(Path::new(&path_str), mtime, size);
                    if decision == GuardDecision::SkipUnchanged {
                        continue;
                    }
                }

                batch.push((path_str, project_name, size, mtime));

                if batch.len() >= 100 {
                    total_files += batch.len();
                    if !dispatch_scanner_batch(&graph, &batch, guard, ingress) {
                        error!("Scanner batch dispatch failed");
                    }
                    batch.clear();
                    info!("... {} files mapped", total_files);
                    let pending = graph
                        .query_count("SELECT count(*) FROM File WHERE status = 'pending'")
                        .unwrap_or(0);
                    let policy = discovery_policy(
                        pending,
                        current_rss_bytes(),
                        memory_limit_bytes(),
                        service_guard::recent_peak_latency_ms(),
                    );
                    std::thread::sleep(policy.sleep);
                }
            }
        }

        if !batch.is_empty() {
            total_files += batch.len();
            let _ = dispatch_scanner_batch(&graph, &batch, guard, ingress);
        }

        total_files
    }
}

fn dispatch_scanner_batch(
    graph: &Arc<GraphStore>,
    batch: &[(String, String, i64, i64)],
    guard: Option<&SharedFileIngressGuard>,
    ingress: Option<&SharedIngressBuffer>,
) -> bool {
    if let Some(shared_ingress) = ingress {
        let mut locked = shared_ingress
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if locked.is_enabled() {
            enqueue_scanner_batch(&mut locked, batch);
            return true;
        }
    }

    if let Err(err) = graph.bulk_insert_files(batch) {
        error!("Bulk insert failed: {:?}", err);
        return false;
    }

    if let Some(shared_guard) = guard {
        let paths = batch
            .iter()
            .map(|(path, _, _, _)| path.clone())
            .collect::<Vec<_>>();
        if let Ok(rows) = graph.fetch_file_ingress_rows(&paths) {
            let mut locked = shared_guard
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            for row in rows {
                locked.record_committed_row(row);
            }
        }
    }

    true
}

fn enqueue_scanner_batch(buffer: &mut IngressBuffer, batch: &[(String, String, i64, i64)]) {
    for (path, project, size, mtime) in batch {
        buffer.record_file(IngressFileEvent::new(
            path.clone(),
            project.clone(),
            *size,
            *mtime,
            100,
            IngressSource::Scan,
            IngressCause::Discovered,
        ));
    }
}

fn ancestor_chain(root: &Path, path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap_or(path);
    let mut dirs = Vec::new();
    let mut current = Some(parent);

    while let Some(dir) = current {
        if dir.starts_with(root) {
            dirs.push(dir.to_path_buf());
        }
        if dir == root {
            break;
        }
        current = dir.parent();
    }

    dirs.reverse();
    dirs
}

fn current_rss_bytes() -> Option<u64> {
    let page_size = 4096;
    let content = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = content
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())?;
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

fn discovery_policy(
    pending_backlog: i64,
    rss_bytes: Option<u64>,
    memory_limit: u64,
    recent_service_latency_ms: u64,
) -> DiscoveryPolicy {
    let rss_ratio = rss_bytes
        .map(|rss| rss as f64 / memory_limit.max(1) as f64)
        .unwrap_or(0.0);

    if recent_service_latency_ms >= 1_500 || rss_ratio >= 0.90 || pending_backlog >= 20_000 {
        return DiscoveryPolicy {
            sleep: std::time::Duration::from_secs(2),
        };
    }

    if recent_service_latency_ms >= 500 || rss_ratio >= 0.80 || pending_backlog >= 10_000 {
        return DiscoveryPolicy {
            sleep: std::time::Duration::from_millis(500),
        };
    }

    if pending_backlog >= 5_000 {
        return DiscoveryPolicy {
            sleep: std::time::Duration::from_millis(150),
        };
    }

    DiscoveryPolicy {
        sleep: std::time::Duration::from_millis(50),
    }
}

#[cfg(test)]
mod tests {
    use super::{discovery_policy, Scanner};
    use crate::graph::GraphStore;
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn test_discovery_policy_is_fast_when_backlog_is_low() {
        let policy = discovery_policy(
            1_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.sleep, std::time::Duration::from_millis(50));
    }

    #[test]
    fn test_discovery_policy_slows_when_backlog_grows() {
        let policy = discovery_policy(
            6_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.sleep, std::time::Duration::from_millis(150));
    }

    #[test]
    fn test_discovery_policy_enters_guard_mode_when_service_is_degraded() {
        let policy = discovery_policy(
            2_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            700,
        );
        assert_eq!(policy.sleep, std::time::Duration::from_millis(500));
    }

    #[test]
    fn test_discovery_policy_pauses_harder_when_pressure_is_critical() {
        let policy = discovery_policy(2_000, Some(95 * 1024 * 1024), 100 * 1024 * 1024, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_secs(2));
    }

    #[test]
    fn test_should_process_path_respects_hierarchical_axonignore() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project = root.join("proj");
        let ignored = project.join("ignored");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(project.join(".axonignore"), "ignored/\n!keep.ex\n").unwrap();
        let kept = project.join("keep.ex");
        let skipped = ignored.join("skip.ex");
        std::fs::write(&kept, "defmodule Keep do\nend\n").unwrap();
        std::fs::write(&skipped, "defmodule Skip do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref());
        assert!(scanner.should_process_path(Path::new(&kept)));
        assert!(!scanner.should_process_path(Path::new(&skipped)));
    }

    #[test]
    fn test_workspace_root_axonignore_can_ignore_only_top_level_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let top_level_worktrees = root.join(".worktrees").join("scratch");
        let project_worktree = root.join("proj").join(".worktrees").join("feature");

        std::fs::create_dir_all(&top_level_worktrees).unwrap();
        std::fs::create_dir_all(&project_worktree).unwrap();
        std::fs::write(root.join(".axonignore"), "/.worktrees/\n").unwrap();

        let top_level_file = top_level_worktrees.join("drop.ex");
        let project_file = project_worktree.join("keep.ex");
        std::fs::write(&top_level_file, "defmodule Drop do\nend\n").unwrap();
        std::fs::write(&project_file, "defmodule Keep do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref());

        assert!(
            !scanner.should_descend_into_directory(root.join(".worktrees").as_path()),
            "La regle racine doit ignorer seulement le subtree .worktrees du workspace"
        );
        assert!(
            scanner.should_process_path(project_file.as_path()),
            "Une worktree locale a un projet ne doit pas etre bannie par une regle racine ancree"
        );
        assert!(
            !scanner.should_process_path(top_level_file.as_path()),
            "Le subtree .worktrees du workspace doit rester ignore"
        );
    }

    #[test]
    fn test_hard_directory_noise_rejects_direnv_cache_and_ruff_cache_without_ignore_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref());

        for relative in [
            Path::new("proj/.direnv"),
            Path::new("proj/.cache"),
            Path::new("proj/.ruff_cache"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                !scanner.should_descend_into_directory(path.as_path()),
                "Le filtre dur doit bloquer {:?} meme sans .axonignore",
                relative
            );
        }
    }

    #[test]
    fn test_blocked_subtree_hint_segments_reject_build_like_directory_events() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref());

        for relative in [
            Path::new("proj/_build"),
            Path::new("proj/node_modules"),
            Path::new("proj/.devenv/state/postgres/pg_wal"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                !scanner.should_buffer_subtree_hint(path.as_path()),
                "Le watcher ne doit pas créer de subtree_hint pour {:?}",
                relative
            );
        }
    }

    #[test]
    fn test_default_config_exposes_ignored_directory_segments() {
        let ignored = &crate::config::CONFIG.indexing.ignored_directory_segments;
        assert!(ignored.iter().any(|segment| segment == ".devenv"));
        assert!(ignored.iter().any(|segment| segment == "_build"));
        assert!(ignored.iter().any(|segment| segment == "node_modules"));
        assert!(crate::config::CONFIG.indexing.subtree_hint_cooldown_ms >= 1);
        assert!(crate::config::CONFIG.indexing.subtree_hint_retry_budget >= 1);
    }

    #[test]
    fn test_scan_subtree_preserves_project_slug_from_universe_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project_a = root.join("proj_a");
        let project_b = root.join("proj_b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        std::fs::write(project_a.join("keep.ex"), "defmodule Keep do\nend\n").unwrap();
        std::fs::write(project_b.join("skip.ex"), "defmodule Skip do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref());
        let store = Arc::new(GraphStore::new(":memory:").unwrap());
        scanner.scan_subtree(store.clone(), &project_a);

        let count_a = store
            .query_count("SELECT count(*) FROM File WHERE project_slug = 'proj_a'")
            .unwrap();
        let count_b = store
            .query_count("SELECT count(*) FROM File WHERE project_slug = 'proj_b'")
            .unwrap();

        assert_eq!(count_a, 1);
        assert_eq!(count_b, 0);
    }
}

// Temporary stubs for dependency extraction
fn extract_toml_dependencies(_content: &str) -> Vec<ProjectDependency> {
    Vec::new()
}
