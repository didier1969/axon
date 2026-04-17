use crate::file_ingress_guard::{GuardDecision, SharedFileIngressGuard};
use crate::graph::GraphStore;
use crate::indexing_policy::{classify_path, classify_subtree_hint_path, PathDisposition};
use crate::ingress_buffer::{
    IngressBuffer, IngressCause, IngressFileEvent, IngressSource, SharedIngressBuffer,
};
use crate::parser::supported_parser_ecosystems;
use crate::service_guard;
use anyhow::Result;
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
    pub project_code: String,
}

#[derive(Debug, Clone, Copy)]
struct DiscoveryPolicy {
    sleep: std::time::Duration,
}

impl Scanner {
    pub fn new(root: &str, project_code: &str) -> Self {
        Self {
            root: PathBuf::from(root),
            project_code: project_code.to_string(),
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

    pub fn scan_subtree(&self, graph: Arc<GraphStore>, subtree: &Path) -> usize {
        self.scan_subtree_with_guard_and_ingress(graph, subtree, None, None)
    }

    pub fn scan_subtree_with_guard(
        &self,
        graph: Arc<GraphStore>,
        subtree: &Path,
        guard: Option<&SharedFileIngressGuard>,
    ) -> usize {
        self.scan_subtree_with_guard_and_ingress(graph, subtree, guard, None)
    }

    pub fn scan_subtree_with_guard_and_ingress(
        &self,
        graph: Arc<GraphStore>,
        subtree: &Path,
        guard: Option<&SharedFileIngressGuard>,
        ingress: Option<&SharedIngressBuffer>,
    ) -> usize {
        info!(
            "Lattice Engine: Prioritizing hot subtree traversal on {:?}",
            subtree
        );
        let total_files = self.scan_path(graph, subtree, guard, ingress);
        info!(
            "🔥 Hot subtree scan complete: {} files mapped from {:?}.",
            total_files, subtree
        );
        total_files
    }

    pub fn should_process_path(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        if self.path_has_ignored_directory_noise(path) {
            return false;
        }
        if self.is_ignored_by_legacy_axonignore(path, false) {
            return false;
        }
        if !self.is_supported(path) {
            return false;
        }
        if self.is_ignored_by_git_rules(path, false)
            && !self.is_included_by_axon_include(path, false)
        {
            return false;
        }
        true
    }

    pub fn should_descend_into_directory(&self, path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        if self.is_workspace_root_worktrees_dir(path) {
            return false;
        }
        if self.path_has_ignored_directory_noise(path) {
            return false;
        }
        if self.is_ignored_by_legacy_axonignore(path, true) {
            return false;
        }
        // We intentionally do not prune directories only because of .gitignore
        // so that `.axoninclude` can re-introduce selected descendants.
        true
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

    pub fn project_code_for_path(&self, graph: &GraphStore, path: &Path) -> Result<String> {
        self.extract_project_code(graph, path)
    }

    pub fn is_ignore_control_path(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        if matches!(
            name,
            ".gitignore" | ".axonignore" | ".axonignore.local" | ".axoninclude"
        ) {
            return true;
        }
        path.ends_with(".git/info/exclude")
    }

    pub fn explain_ignore_decision(&self, path: &Path, is_dir: bool) -> String {
        if self.is_workspace_root_worktrees_dir(path) {
            return "blocked_root_worktrees_hard_rule".to_string();
        }
        if self.path_has_ignored_directory_noise(path) {
            return "ignored_by_hard_deny_directory_segment".to_string();
        }
        if self.is_ignored_by_legacy_axonignore(path, is_dir) {
            return "ignored_by_legacy_axonignore".to_string();
        }
        if self.is_included_by_axon_include(path, is_dir) {
            return "included_by_axoninclude".to_string();
        }
        if self.is_ignored_by_git_rules(path, is_dir) {
            return "ignored_by_gitignore_or_exclude".to_string();
        }
        if !is_dir && !self.is_supported(path) {
            return "ignored_by_extension_or_hidden_filter".to_string();
        }
        "eligible".to_string()
    }

    fn extract_project_code(&self, graph: &GraphStore, path: &Path) -> Result<String> {
        let explicit = self.project_code.trim();
        if !explicit.is_empty() {
            return Ok(explicit.to_string());
        }

        crate::project_meta::resolve_registered_project_identity_for_path(graph, path)
            .map(|identity| identity.code)
    }

    fn build_walker_from(&self, start: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(start);
        builder.hidden(false);
        builder.git_ignore(false);
        builder.git_global(crate::config::CONFIG.indexing.use_git_global_ignore);
        builder.git_exclude(false);
        builder
    }

    fn is_ignored_by_git_rules(&self, path: &Path, is_dir: bool) -> bool {
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

        let mut decision: Option<bool> = None;
        for dir in ancestor_chain(&root, &absolute) {
            let ignore_path = dir.join(".gitignore");
            if ignore_path.exists() {
                let (matcher, _err) = Gitignore::new(&ignore_path);
                let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                if matched.is_ignore() {
                    decision = Some(true);
                } else if matched.is_whitelist() {
                    decision = Some(false);
                }
            }
            let exclude_path = dir.join(".git").join("info").join("exclude");
            if exclude_path.exists() {
                let (matcher, _err) = Gitignore::new(&exclude_path);
                let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                if matched.is_ignore() {
                    decision = Some(true);
                } else if matched.is_whitelist() {
                    decision = Some(false);
                }
            }
        }

        decision.unwrap_or(false)
    }

    fn is_included_by_axon_include(&self, path: &Path, is_dir: bool) -> bool {
        let absolute = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => path.to_path_buf(),
        };
        let root = match std::fs::canonicalize(&self.root) {
            Ok(root) => root,
            Err(_) => self.root.clone(),
        };

        if !absolute.starts_with(&root) {
            return false;
        }

        let mut included = false;
        for dir in ancestor_chain(&root, &absolute) {
            let include_path = dir.join(".axoninclude");
            if include_path.exists() {
                let (matcher, _err) = Gitignore::new(&include_path);
                let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                if matched.is_ignore() || matched.is_whitelist() {
                    included = true;
                }
            }
        }

        included
    }

    fn is_ignored_by_legacy_axonignore(&self, path: &Path, is_dir: bool) -> bool {
        if !crate::config::CONFIG.indexing.legacy_axonignore_additive {
            return false;
        }

        let absolute = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => path.to_path_buf(),
        };
        let root = match std::fs::canonicalize(&self.root) {
            Ok(root) => root,
            Err(_) => self.root.clone(),
        };

        if !absolute.starts_with(&root) {
            return false;
        }

        let mut decision: Option<bool> = None;
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
            if name.starts_with('.')
                && name != ".env"
                && !matches!(
                    name,
                    ".gitignore" | ".axoninclude" | ".axonignore" | ".axonignore.local"
                )
            {
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
        self.path_has_ignored_directory_noise_with_config(path, &crate::config::CONFIG.indexing)
    }

    fn path_has_blocked_subtree_hint_segment(&self, path: &Path) -> bool {
        self.path_has_blocked_subtree_hint_segment_with_config(
            path,
            &crate::config::CONFIG.indexing,
        )
    }

    fn path_has_ignored_directory_noise_with_config(
        &self,
        path: &Path,
        config: &crate::config::IndexingConfig,
    ) -> bool {
        !matches!(
            classify_path(&self.root, path, config, supported_parser_ecosystems()),
            PathDisposition::Allow
        )
    }

    fn path_has_blocked_subtree_hint_segment_with_config(
        &self,
        path: &Path,
        config: &crate::config::IndexingConfig,
    ) -> bool {
        !matches!(
            classify_subtree_hint_path(&self.root, path, config, supported_parser_ecosystems()),
            PathDisposition::Allow
        )
    }

    fn is_workspace_root_worktrees_dir(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let mut comps = relative.components();
        let first = comps.next();
        let second = comps.next();
        match (first, second) {
            (Some(a), None) => a.as_os_str() == ".worktrees",
            _ => false,
        }
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
                if !self.should_process_path(path) {
                    continue;
                }

                let project_name = match self.project_code_for_path(graph.as_ref(), path) {
                    Ok(project_code) => project_code,
                    Err(err) => {
                        info!(
                            "Scanner: chemin non admissible sans identité canonique {:?}: {}",
                            path, err
                        );
                        continue;
                    }
                };

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

// Temporary stubs for dependency extraction
fn extract_toml_dependencies(_content: &str) -> Vec<ProjectDependency> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{discovery_policy, Scanner};
    use crate::config::IndexingConfig;
    use std::path::Path;
    use std::sync::Arc;

    fn test_config() -> IndexingConfig {
        IndexingConfig {
            supported_extensions: vec![
                "ex".to_string(),
                "rs".to_string(),
                "js".to_string(),
                "ts".to_string(),
                "py".to_string(),
                "rb".to_string(),
            ],
            ignored_directory_segments: vec![],
            blocked_subtree_hint_segments: vec![],
            soft_excluded_directory_segments_allowlist: vec![],
            subtree_hint_cooldown_ms: 15_000,
            subtree_hint_retry_budget: 3,
            use_git_global_ignore: false,
            legacy_axonignore_additive: true,
            ignore_reconcile_enabled: true,
            ignore_reconcile_dry_run: true,
        }
    }

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
        let project = root.join("prj");
        let ignored = project.join("ignored");
        std::fs::create_dir_all(&ignored).unwrap();
        std::fs::write(project.join(".axonignore"), "ignored/\n!keep.ex\n").unwrap();
        let kept = project.join("keep.ex");
        let skipped = ignored.join("skip.ex");
        std::fs::write(&kept, "defmodule Keep do\nend\n").unwrap();
        std::fs::write(&skipped, "defmodule Skip do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        assert!(scanner.should_process_path(Path::new(&kept)));
        assert!(!scanner.should_process_path(Path::new(&skipped)));
    }

    #[test]
    fn test_workspace_root_axonignore_can_ignore_only_top_level_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let top_level_worktrees = root.join(".worktrees").join("scratch");
        let project_worktree = root.join("prj").join(".worktrees").join("feature");

        std::fs::create_dir_all(&top_level_worktrees).unwrap();
        std::fs::create_dir_all(&project_worktree).unwrap();
        std::fs::write(root.join(".axonignore"), "/.worktrees/\n").unwrap();

        let top_level_file = top_level_worktrees.join("drop.ex");
        let project_file = project_worktree.join("keep.ex");
        std::fs::write(&top_level_file, "defmodule Drop do\nend\n").unwrap();
        std::fs::write(&project_file, "defmodule Keep do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");

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
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");

        for relative in [
            Path::new("prj/.direnv"),
            Path::new("prj/.cache"),
            Path::new("prj/.ruff_cache"),
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
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");

        for relative in [
            Path::new("prj/_build"),
            Path::new("prj/node_modules"),
            Path::new("prj/.devenv/state/postgres/pg_wal"),
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
    fn test_generated_artifact_prefixes_are_treated_as_build_noise() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");

        for relative in [
            Path::new("prj/_build_truth_dashboard_ui"),
            Path::new("prj/_build_truth_journeys"),
            Path::new("prj/deps/pkg/.mix"),
            Path::new("prj/deps/pkg/ebin"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                !scanner.should_descend_into_directory(path.as_path()),
                "Le scanner doit traiter {:?} comme un artefact genere non indexable",
                relative
            );
            assert!(
                !scanner.should_buffer_subtree_hint(path.as_path()),
                "Le watcher ne doit pas bufferiser {:?} comme subtree_hint",
                relative
            );
        }
    }

    #[test]
    fn test_ecosystem_policy_blocks_framework_and_cache_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let config = test_config();

        for relative in [
            Path::new("prj/.next"),
            Path::new("prj/.gradle"),
            Path::new("prj/__pycache__"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                scanner.path_has_ignored_directory_noise_with_config(path.as_path(), &config),
                "La politique d'ecosysteme doit bloquer {:?}",
                relative
            );
        }
    }

    #[test]
    fn test_soft_excluded_vendor_can_be_reopened_for_scanner_and_subtree_hints() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let mut config = test_config();
        let vendor = root.join("prj/vendor");
        std::fs::create_dir_all(&vendor).unwrap();

        assert!(scanner.path_has_ignored_directory_noise_with_config(vendor.as_path(), &config));
        assert!(
            scanner.path_has_blocked_subtree_hint_segment_with_config(vendor.as_path(), &config)
        );

        config.soft_excluded_directory_segments_allowlist = vec!["vendor".to_string()];

        assert!(!scanner.path_has_ignored_directory_noise_with_config(vendor.as_path(), &config));
        assert!(
            !scanner.path_has_blocked_subtree_hint_segment_with_config(vendor.as_path(), &config)
        );
    }

    #[test]
    fn test_default_config_exposes_ignored_directory_segments() {
        let parsed: crate::config::Config = toml::from_str("[indexing]\n").unwrap();
        let ignored = &parsed.indexing.ignored_directory_segments;
        let blocked_hints = &parsed.indexing.blocked_subtree_hint_segments;
        assert!(ignored.iter().any(|segment| segment == ".fastembed_cache"));
        assert!(blocked_hints.iter().any(|segment| segment == "pg_wal"));
        assert!(!ignored.iter().any(|segment| segment == "vendor"));
        assert!(!ignored.iter().any(|segment| segment == "build"));
        assert!(!ignored.iter().any(|segment| segment == "dist"));
        assert!(parsed
            .indexing
            .soft_excluded_directory_segments_allowlist
            .is_empty());
        assert!(parsed.indexing.subtree_hint_cooldown_ms >= 1);
        assert!(parsed.indexing.subtree_hint_retry_budget >= 1);
    }

    #[test]
    fn test_scan_subtree_preserves_project_code_from_universe_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let project_a = root.join("proj_a");
        let project_b = root.join("proj_b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        std::fs::write(project_a.join("keep.ex"), "defmodule Keep do\nend\n").unwrap();
        std::fs::write(project_b.join("skip.ex"), "defmodule Skip do\nend\n").unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        scanner.scan_subtree(store.clone(), &project_a);

        let count_a = store
            .query_count("SELECT count(*) FROM File WHERE project_code = 'PRJ'")
            .unwrap();
        let count_b = store
            .query_count("SELECT count(*) FROM File WHERE project_code = 'proj_b'")
            .unwrap();

        assert_eq!(count_a, 1);
        assert_eq!(count_b, 0);
    }

    #[test]
    fn test_workspace_scan_must_not_assign_unregistered_project_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let registered_project = root.join("registered");
        let unknown_project = root.join("unknown");
        std::fs::create_dir_all(&registered_project).unwrap();
        std::fs::create_dir_all(&unknown_project).unwrap();
        std::fs::write(
            registered_project.join("keep.ex"),
            "defmodule Keep do\nend\n",
        )
        .unwrap();
        std::fs::write(unknown_project.join("drop.ex"), "defmodule Drop do\nend\n").unwrap();

        let store = Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        store
            .sync_project_registry_entry(
                "PRJ",
                Some("registered"),
                Some(registered_project.to_string_lossy().as_ref()),
            )
            .unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "");
        scanner.scan(store.clone());

        let registered_count = store
            .query_count("SELECT count(*) FROM File WHERE project_code = 'PRJ'")
            .unwrap();
        let unknown_count = store
            .query_count(&format!(
                "SELECT count(*) FROM File WHERE path LIKE '{}%'",
                unknown_project.to_string_lossy().replace('\'', "''")
            ))
            .unwrap();

        assert_eq!(registered_count, 1);
        assert_eq!(unknown_count, 0);
    }
}
