use crate::graph::GraphStore;
use crate::indexing_policy::{classify_path, classify_subtree_hint_path, PathDisposition};
use crate::parser::supported_parser_ecosystems;
use crate::service_guard;
use anyhow::Result;
use ignore::{gitignore::Gitignore, WalkBuilder};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

pub struct Scanner {
    root: PathBuf,
    root_canonical: PathBuf,
    pub project_code: String,
    gitignore_cache: MatcherCache,
    git_exclude_cache: MatcherCache,
    axoninclude_cache: MatcherCache,
    axonignore_cache: MatcherCache,
    axonignore_local_cache: MatcherCache,
}

type MatcherCache = Mutex<HashMap<PathBuf, Option<Arc<Gitignore>>>>;

#[derive(Debug, Clone, Copy)]
struct DiscoveryPolicy {
    sleep: std::time::Duration,
}

const SCANNER_BATCH_SIZE: usize = 512;

impl Scanner {
    pub fn new(root: &str, project_code: &str) -> Self {
        let root_path = PathBuf::from(root);
        let root_canonical =
            std::fs::canonicalize(&root_path).unwrap_or_else(|_| root_path.clone());
        Self {
            root: root_path,
            root_canonical,
            project_code: project_code.to_string(),
            gitignore_cache: Mutex::new(HashMap::new()),
            git_exclude_cache: Mutex::new(HashMap::new()),
            axoninclude_cache: Mutex::new(HashMap::new()),
            axonignore_cache: Mutex::new(HashMap::new()),
            axonignore_local_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Full walk under the root, UPSERTing every eligible file into
    /// ist.IndexedFile with status='discovered'. The DBQ-A claim feeder
    /// (REQ-AXO-901897) drains those rows into pipeline A. The legacy
    /// in-memory ingress_buffer + FileIngressGuard push was RIPPED in the
    /// LEGACY FEED PURGE (REQ-AXO-901893); discovery now lands directly in PG.
    pub fn scan(&self, graph: Arc<GraphStore>) {
        let scan_start_ms = chrono::Utc::now().timestamp_millis();
        info!(
            "Lattice Engine: Initializing recursive traversal on {:?}",
            self.root
        );
        let total_files = self.scan_path(graph.clone(), &self.root);
        info!(
            "🏁 Nexus Scan Complete: {} files mapped to graph store (status: pending).",
            total_files
        );
        // 9f: detect files that disappeared from the filesystem since last walk.
        // REQ-AXO-901831 — scope the purge to THIS walk's subtree so a
        // per-project scan never deletes sibling projects' IndexedFile rows.
        let root_prefix = self.root_canonical.to_string_lossy();
        // REQ-AXO-901950 — pass this scanner's eligibility verdict so files that
        // became gitignored/.axonignore'd since last walk are purged too, not
        // only the ones physically removed from disk.
        match graph.delete_stale_indexed_files(scan_start_ms, root_prefix.as_ref(), &|p| {
            self.should_process_path(p)
        }) {
            Ok(deleted) if !deleted.is_empty() => {
                info!(
                    "Lattice Engine: purged {} stale IndexedFile entries (not seen in this walk)",
                    deleted.len()
                );
            }
            Ok(_) => {}
            Err(e) => warn!("Lattice Engine: stale file cleanup failed: {e}"),
        }
    }

    /// Subtree walk → ist.IndexedFile status='discovered' (DBQ-A drains).
    pub fn scan_subtree(&self, graph: Arc<GraphStore>, subtree: &Path) -> usize {
        info!(
            "Lattice Engine: Prioritizing hot subtree traversal on {:?}",
            subtree
        );
        let total_files = self.scan_path(graph, subtree);
        info!(
            "🔥 Hot subtree scan complete: {} files mapped from {:?}.",
            total_files, subtree
        );
        total_files
    }

    /// Pure enumeration — no GraphStore mutation. Returns the same set
    /// of file paths that `scan_path` would dispatch through the
    /// ingress buffer, applying every Scanner filter (directory noise,
    /// hidden files, .gitignore / .axonignore stack, supported
    /// extensions). Used by benches and observability tooling that
    /// need the watcher's view of a tree without writing anything.
    pub fn enumerate_files(&self) -> Vec<PathBuf> {
        self.enumerate_files_under(&self.root.clone())
    }

    pub fn enumerate_files_under(&self, start: &Path) -> Vec<PathBuf> {
        let walker = self.build_walker_from(start);
        let mut out = Vec::new();
        for entry in walker.build().filter_map(|e| e.ok()) {
            let p = entry.path();
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }
            if !self.should_process_path(p) {
                continue;
            }
            out.push(p.to_path_buf());
        }
        out
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

        let candidate = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        crate::project_meta::resolve_project_identity_for_path(graph, &candidate)
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
        let root = &self.root_canonical;

        if !absolute.starts_with(root) {
            return true;
        }

        let mut decision: Option<bool> = None;
        for dir in ancestor_chain(root, &absolute) {
            if let Some(matcher) =
                self.cached_matcher_for(&self.gitignore_cache, &dir.join(".gitignore"))
            {
                let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                if matched.is_ignore() {
                    decision = Some(true);
                } else if matched.is_whitelist() {
                    decision = Some(false);
                }
            }
            if let Some(matcher) =
                self.cached_matcher_for(&self.git_exclude_cache, &dir.join(".git/info/exclude"))
            {
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
        let root = &self.root_canonical;

        if !absolute.starts_with(root) {
            return false;
        }

        let mut included = false;
        for dir in ancestor_chain(root, &absolute) {
            if let Some(matcher) =
                self.cached_matcher_for(&self.axoninclude_cache, &dir.join(".axoninclude"))
            {
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
        let root = &self.root_canonical;

        if !absolute.starts_with(root) {
            return false;
        }

        let mut decision: Option<bool> = None;
        for dir in ancestor_chain(root, &absolute) {
            for (cache, ignore_name) in [
                (&self.axonignore_cache, ".axonignore"),
                (&self.axonignore_local_cache, ".axonignore.local"),
            ] {
                if let Some(matcher) = self.cached_matcher_for(cache, &dir.join(ignore_name)) {
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

    fn scan_path(&self, graph: Arc<GraphStore>, start: &Path) -> usize {
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

                batch.push((path_str, project_name, size, mtime));

                if batch.len() >= SCANNER_BATCH_SIZE {
                    total_files += batch.len();
                    if !dispatch_scanner_batch(&graph, &batch) {
                        error!("Scanner batch dispatch failed");
                    }
                    batch.clear();
                    info!("... {} files mapped", total_files);
                    let policy = discovery_policy(
                        0,
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
            let _ = dispatch_scanner_batch(&graph, &batch);
        }

        total_files
    }

    fn cached_matcher_for(
        &self,
        cache: &MatcherCache,
        matcher_path: &Path,
    ) -> Option<Arc<Gitignore>> {
        let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(existing) = cache.get(matcher_path) {
            return existing.clone();
        }

        let matcher = if matcher_path.exists() {
            let (matcher, _err) = Gitignore::new(matcher_path);
            Some(Arc::new(matcher))
        } else {
            None
        };
        cache.insert(matcher_path.to_path_buf(), matcher.clone());
        matcher
    }
}

fn dispatch_scanner_batch(graph: &Arc<GraphStore>, batch: &[(String, String, i64, i64)]) -> bool {
    // DEC-AXO-901619: durable discovery — batch UPSERT into ist.IndexedFile
    // with status='discovered'. The DBQ-A claim feeder (REQ-AXO-901897) atomically
    // claims those rows into pipeline A. The legacy in-memory ingress_buffer push
    // was RIPPED in the LEGACY FEED PURGE (REQ-AXO-901893) — PG is the durable
    // work queue, no separate buffer to keep in sync.
    match persist_discovery_batch(graph, batch) {
        Ok(()) => true,
        Err(e) => {
            warn!("Scanner: durable discovery batch failed ({e})");
            false
        }
    }
}

/// DEC-AXO-901619 + C1/C2 fixes: persist discovered files in PG with
/// mtime+size change detection. Unchanged indexed files skip the UPDATE
/// entirely (zero WAL). Changed files force status='discovered'.
fn persist_discovery_batch(
    graph: &Arc<GraphStore>,
    batch: &[(String, String, i64, i64)],
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    // REQ-AXO-901860: IndexedFile.project_code is a NOT NULL FK to
    // ist.Project, so every project this batch references must exist BEFORE
    // the file rows. Enrol the distinct projects first (idempotent).
    let mut projects: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut values = Vec::with_capacity(batch.len());
    for (path, project, size, mtime) in batch {
        // REQ-AXO-901860 — NEVER enrol the "UNK" sentinel (unresolved project).
        // project_code is a NOT NULL FK to ist.Project; enrolling UNK both
        // resurrects the garbage bucket the canonical project/path resolution
        // retired AND creates an "UNK" ist.Project row (the `INSERT INTO Project
        // … ON CONFLICT DO NOTHING` below would mint it). Files that don't
        // resolve to a registered project are dropped here, mirroring
        // graph_ingestion's UNK skip — no UNK Project, no UNK IndexedFile, ever.
        if project == "UNK" {
            continue;
        }
        let safe_path = path.replace('\'', "''");
        let safe_project = project.replace('\'', "''");
        projects.insert(project.as_str());
        // mtime from scanner is seconds, convert to ms for consistency
        let mtime_ms = *mtime * 1000;
        values.push(format!(
            "('{safe_path}', '{safe_project}', '', {now_ms}, 'discovered', {now_ms}, {mtime_ms}, {size}, 0, NULL)"
        ));
    }
    if values.is_empty() {
        // Entire batch resolved to UNK / unresolved — nothing canonical to enrol.
        return Ok(());
    }
    let proj_values: Vec<String> = projects
        .iter()
        .map(|p| format!("('{}', {now_ms})", p.replace('\'', "''")))
        .collect();
    graph.execute(&format!(
        "INSERT INTO Project (code, enrolled_at_ms) VALUES {} ON CONFLICT (code) DO NOTHING",
        proj_values.join(", ")
    ))?;
    // REQ-AXO-901867 — enrich ist.Project.name / root_path from the canonical
    // registry (soll.ProjectCodeRegistry). The discovery INSERT above only
    // carries (code, enrolled_at_ms), leaving name/root_path blank, so the
    // project_telemetry view (and the dashboard reading it) showed empty
    // identity columns. Fill ONLY when empty → idempotent, never clobbers an
    // operator-set value, and a no-op once populated. Name falls back to the
    // project_path basename when the registry name is blank (parity with
    // project_meta::registered_project_identities).
    let code_list: Vec<String> = projects
        .iter()
        .map(|p| format!("'{}'", p.replace('\'', "''")))
        .collect();
    graph.execute(&format!(
        "UPDATE ist.Project p \
         SET name = COALESCE(NULLIF(r.project_name, ''), \
                             NULLIF(regexp_replace(r.project_path, '^.*/', ''), ''), p.name), \
             root_path = COALESCE(NULLIF(r.project_path, ''), p.root_path) \
         FROM soll.ProjectCodeRegistry r \
         WHERE upper(r.project_code) = p.code \
           AND (p.name = '' OR p.root_path = '') \
           AND p.code IN ({})",
        code_list.join(", ")
    ))?;
    // C1: mtime_ms + size_bytes enable change detection without reading content.
    // C2: WHERE clause skips UPDATE entirely when file is unchanged (zero WAL).
    // Changed file (mtime or size differ) → force status='discovered' for re-indexing.
    // Unchanged indexed file → no row update at all.
    let sql = format!(
        "INSERT INTO IndexedFile \
             (path, project_code, content_hash, last_seen_ms, status, discovered_ms, mtime_ms, size_bytes, retry_count, last_attempt_ms) \
         VALUES {} \
         ON CONFLICT (path) DO UPDATE SET \
             project_code  = EXCLUDED.project_code, \
             discovered_ms = EXCLUDED.discovered_ms, \
             last_seen_ms  = EXCLUDED.last_seen_ms, \
             mtime_ms      = EXCLUDED.mtime_ms, \
             size_bytes    = EXCLUDED.size_bytes, \
             retry_count   = CASE \
                 WHEN IndexedFile.mtime_ms != EXCLUDED.mtime_ms \
                   OR IndexedFile.size_bytes != EXCLUDED.size_bytes \
                 THEN 0 ELSE IndexedFile.retry_count END, \
             status = CASE \
                 WHEN IndexedFile.mtime_ms != EXCLUDED.mtime_ms \
                   OR IndexedFile.size_bytes != EXCLUDED.size_bytes \
                 THEN 'discovered' \
                 ELSE IndexedFile.status \
             END \
         WHERE IndexedFile.mtime_ms != EXCLUDED.mtime_ms \
            OR IndexedFile.size_bytes != EXCLUDED.size_bytes \
            OR IndexedFile.status = 'discovered'",
        values.join(", ")
    );
    graph.execute(&sql)
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
    _pending_backlog: i64,
    rss_bytes: Option<u64>,
    memory_limit: u64,
    recent_service_latency_ms: u64,
) -> DiscoveryPolicy {
    let rss_ratio = rss_bytes
        .map(|rss| rss as f64 / memory_limit.max(1) as f64)
        .unwrap_or(0.0);

    let sleep_ms = if recent_service_latency_ms >= 1_500 || rss_ratio >= 0.90 {
        250
    } else if recent_service_latency_ms >= 500 || rss_ratio >= 0.80 {
        50
    } else {
        0
    };

    DiscoveryPolicy {
        sleep: std::time::Duration::from_millis(sleep_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::{discovery_policy, Scanner};
    use crate::config::IndexingConfig;
    use crate::service_guard;
    use std::path::Path;

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
        assert_eq!(policy.sleep, std::time::Duration::ZERO);
    }

    #[test]
    fn test_discovery_policy_keeps_push_discovery_fast_even_when_backlog_grows() {
        let policy = discovery_policy(
            20_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            0,
        );
        assert_eq!(policy.sleep, std::time::Duration::ZERO);
    }

    #[test]
    fn test_discovery_policy_enters_guard_mode_when_service_is_degraded() {
        let policy = discovery_policy(
            2_000,
            Some(2 * 1024 * 1024 * 1024),
            10 * 1024 * 1024 * 1024,
            700,
        );
        assert_eq!(policy.sleep, std::time::Duration::from_millis(50));
    }

    #[test]
    fn test_discovery_policy_pauses_harder_when_pressure_is_critical() {
        let policy = discovery_policy(2_000, Some(95 * 1024 * 1024), 100 * 1024 * 1024, 0);
        assert_eq!(policy.sleep, std::time::Duration::from_millis(250));
    }

    #[test]
    fn test_discovery_policy_does_not_slow_down_just_because_runtime_is_quiescent() {
        service_guard::reset_for_tests();
        let policy = discovery_policy(0, Some(2 * 1024 * 1024 * 1024), 10 * 1024 * 1024 * 1024, 0);
        assert_eq!(policy.sleep, std::time::Duration::ZERO);
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

    // ───────────────────────────────────────────────────────────────────
    // REQ-AXO-901901 — durable bootstrap/reconciliation walk regression tests.
    //
    // The live stall (this session) was: Watchman's fresh crawl under-delivered
    // the cold-start bulk (5.3K of 18K eligible), and NOTHING else populated the
    // DBQ-A 'discovered' queue (the bootstrap scanner walk was removed in the
    // LEGACY FEED PURGE but the comments still claimed it ran). These lock the
    // two invariants the wiring fix depends on: scan() ENROLS eligible files as
    // 'discovered' (drainable by the claim feeder), and the stale reconciliation
    // it runs at the end of every walk NEVER erodes existing indexed data.
    // ───────────────────────────────────────────────────────────────────

    /// scan() must UPSERT every eligible source file as status='discovered'
    /// (discovered_ms>0) — the exact rows the DBQ-A claim feeder drains — while
    /// pruning build-output directories. Combined with demand_pull's
    /// `claim_against_real_pg_selects_only_claimable_rows`, this covers the full
    /// discovery → claim chain the live fix restores.
    #[tokio::test]
    async fn scan_enrols_new_eligible_files_as_discovered_and_prunes_build_dirs() {
        let store = std::sync::Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/keep_a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("src/keep_b.rs"), "fn b() {}\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/gen.rs"), "fn g() {}\n").unwrap();

        let root_canon = root.canonicalize().unwrap().to_string_lossy().to_string();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "TST");
        scanner.scan(store.clone());

        let discovered = store
            .query_count(&format!(
                "SELECT count(*) FROM ist.IndexedFile \
                 WHERE status='discovered' AND discovered_ms>0 AND path LIKE '{root_canon}/%'"
            ))
            .unwrap();
        assert_eq!(
            discovered, 2,
            "scanner walk must enrol exactly the 2 eligible source files as status='discovered'"
        );

        let in_build = store
            .query_count(&format!(
                "SELECT count(*) FROM ist.IndexedFile WHERE path LIKE '{root_canon}/target/%'"
            ))
            .unwrap();
        assert_eq!(
            in_build, 0,
            "build-dir files must be pruned, never enrolled"
        );

        let _ = store.execute(&format!(
            "DELETE FROM ist.IndexedFile WHERE path LIKE '{root_canon}/%'"
        ));
    }

    /// delete_stale_indexed_files (run at the end of every scan()) is the
    /// reconciliation that the boot/periodic walk performs. It MUST be
    /// non-destructive: purge only paths the filesystem confirms are gone, never
    /// the live A3 writeback rows (status='parsed', discovered_ms=0) nor present
    /// files merely missed by a partial walk. Guards REQ-AXO-901884 against the
    /// 36K→3.5K erosion regression now that scan() runs on a cadence again.
    #[tokio::test]
    async fn delete_stale_preserves_parsed_and_present_rows_purges_only_gone() {
        let store = std::sync::Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let root_canon = root.canonicalize().unwrap().to_string_lossy().to_string();

        std::fs::write(root.join("present.rs"), "fn p() {}\n").unwrap();
        let present = root
            .join("present.rs")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();
        std::fs::write(root.join("present2.rs"), "fn p2() {}\n").unwrap();
        let present2 = root
            .join("present2.rs")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let gone = format!("{root_canon}/gone.rs"); // never created on disk

        store
            .execute("INSERT INTO ist.Project (code, enrolled_at_ms) VALUES ('TST', 1) ON CONFLICT (code) DO NOTHING")
            .unwrap();
        // 1. parsed row, discovered_ms=0 (live A3 writeback shape) — excluded from
        //    the candidate set entirely (discovered_ms>0 filter) → must survive.
        store
            .execute(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, discovered_ms, mtime_ms, size_bytes, retry_count) \
                 VALUES ('{present}', 'TST', 'h', 1, 'parsed', 0, 1, 9, 0) \
                 ON CONFLICT (path) DO UPDATE SET status='parsed', discovered_ms=0"
            ))
            .unwrap();
        // 2. discovered row, old discovered_ms, file PRESENT on disk → survive.
        store
            .execute(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, discovered_ms, mtime_ms, size_bytes, retry_count) \
                 VALUES ('{present2}', 'TST', 'h', 1, 'discovered', 5, 1, 10, 0) \
                 ON CONFLICT (path) DO UPDATE SET status='discovered', discovered_ms=5"
            ))
            .unwrap();
        // 3. discovered row, old discovered_ms, file GONE → purge.
        store
            .execute(&format!(
                "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, discovered_ms, mtime_ms, size_bytes, retry_count) \
                 VALUES ('{gone}', 'TST', 'h', 1, 'discovered', 5, 1, 10, 0) \
                 ON CONFLICT (path) DO UPDATE SET status='discovered', discovered_ms=5"
            ))
            .unwrap();

        // scan_start far ahead so every candidate qualifies by timestamp; the
        // FS exists() check is then the sole discriminator.
        let scan_start = chrono::Utc::now().timestamp_millis() + 1_000_000;
        // Everything eligible → `exists()` is the sole discriminator (the
        // original REQ-AXO-901884 contract).
        let deleted = store
            .delete_stale_indexed_files(scan_start, &root_canon, &|_| true)
            .unwrap();

        assert!(
            deleted.contains(&gone),
            "FS-confirmed-gone file must be purged: {deleted:?}"
        );
        assert!(
            !deleted.contains(&present),
            "parsed row (discovered_ms=0) must never be a stale candidate"
        );
        assert!(
            !deleted.contains(&present2),
            "present file must survive even with an old discovered_ms"
        );

        let survivors = store
            .query_count(&format!(
                "SELECT count(*) FROM ist.IndexedFile WHERE path IN ('{present}','{present2}')"
            ))
            .unwrap();
        assert_eq!(
            survivors, 2,
            "both present files survive the stale reconciliation"
        );

        let _ = store.execute(&format!(
            "DELETE FROM ist.IndexedFile WHERE path LIKE '{root_canon}/%'"
        ));
    }

    /// REQ-AXO-901950 — a file still present on disk but no longer eligible
    /// (its directory was just added to `.gitignore` / `.axonignore`) is purged
    /// from the index as if deleted; an eligible-but-present file is KEPT
    /// (erosion guard, REQ-AXO-901884). Both discriminators in one test.
    #[tokio::test]
    async fn delete_stale_purges_present_but_ineligible_keeps_eligible() {
        let store = std::sync::Arc::new(crate::tests::test_helpers::create_test_db().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let root_canon = root.canonicalize().unwrap().to_string_lossy().to_string();

        // Both files PRESENT on disk, both with an old discovered_ms (stale
        // candidates by timestamp).
        std::fs::write(root.join("keep.rs"), "fn k() {}\n").unwrap();
        let keep = root
            .join("keep.rs")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();
        std::fs::write(root.join("now_ignored.rs"), "fn g() {}\n").unwrap();
        let ignored = root
            .join("now_ignored.rs")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();

        store
            .execute("INSERT INTO ist.Project (code, enrolled_at_ms) VALUES ('TST', 1) ON CONFLICT (code) DO NOTHING")
            .unwrap();
        for p in [&keep, &ignored] {
            store
                .execute(&format!(
                    "INSERT INTO ist.IndexedFile (path, project_code, content_hash, last_seen_ms, status, discovered_ms, mtime_ms, size_bytes, retry_count) \
                     VALUES ('{p}', 'TST', 'h', 1, 'discovered', 5, 1, 10, 0) \
                     ON CONFLICT (path) DO UPDATE SET status='discovered', discovered_ms=5"
                ))
                .unwrap();
        }

        let scan_start = chrono::Utc::now().timestamp_millis() + 1_000_000;
        // Eligibility predicate models a freshly-added .gitignore rule excluding
        // now_ignored.rs. Both files exist on disk → eligibility is the sole
        // discriminator here.
        let deleted = store
            .delete_stale_indexed_files(scan_start, &root_canon, &|p| {
                !p.to_string_lossy().ends_with("now_ignored.rs")
            })
            .unwrap();

        assert!(
            deleted.contains(&ignored),
            "present-but-now-ineligible file must be purged: {deleted:?}"
        );
        assert!(
            !deleted.contains(&keep),
            "present + eligible file must survive (erosion protection): {deleted:?}"
        );

        let _ = store.execute(&format!(
            "DELETE FROM ist.IndexedFile WHERE path LIKE '{root_canon}/%'"
        ));
    }
}
