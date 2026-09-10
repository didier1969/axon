use crate::graph::GraphStore;
use crate::indexing_policy::{classify_path, PathDisposition};
use crate::parser::supported_parser_ecosystems;
use crate::service_guard;
use anyhow::Result;
use ignore::{
    gitignore::{Gitignore, GitignoreBuilder},
    WalkBuilder,
};
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

/// REQ-AXO-901932 F2 — one-call explanation of the eligible↔indexed gap.
/// `eligible` counts files the indexer WOULD enrol (same `should_process_path`
/// predicate as `enumerate_files`); `excluded_by_reason` buckets the rest by
/// the canonical [`Scanner::explain_ignore_decision`] taxonomy (single source
/// of truth, GUI-PRO-013). `noise_dirs_pruned` counts build/dependency/VCS
/// directories skipped at descent (node_modules / target / _build / .git /
/// .axon …) — excluded by design and intentionally NOT walked.
#[derive(Debug, Default, Clone)]
pub struct ScopeBreakdown {
    pub eligible: u64,
    pub walked_files: u64,
    /// `(reason, count)` sorted by count desc, then reason asc.
    pub excluded_by_reason: Vec<(String, u64)>,
    /// REQ-AXO-902636 — `(extension, count)` des fichiers ECARTES par le filtre
    /// d'extensions ALORS QU'UN PARSER EXISTE pour eux. Le compte agrege
    /// `ignored_by_extension_or_hidden_filter` melange deux choses tres
    /// differentes : du bruit legitime (binaire, media, verrous) et du CODE
    /// SOURCE que le systeme sait lire mais n'admet pas. Sans cette separation,
    /// `diagnose_indexing` a certifie MRG complet pendant que 100 % de ses
    /// en-tetes `.hpp` etaient ecartes.
    ///
    /// Ce compte lit la configuration EFFECTIVE, pas le defaut compile : la
    /// garde unitaire de REQ-AXO-902631 protege `default_supported_extensions()`,
    /// qu'un `.axon/capabilities.toml` peut surcharger.
    pub parsable_but_excluded: Vec<(String, u64)>,
}

impl ScopeBreakdown {
    /// REQ-AXO-902352 — nombre total de fichiers de code source écartés par le filtre.
    pub fn excluded_source_count(&self) -> u64 {
        self.parsable_but_excluded.iter().map(|(_, n)| *n).sum()
    }

    /// REQ-AXO-902352 — résumé textuel des extensions source écartées (ex: ".rs: 18").
    pub fn excluded_source_summary(&self) -> String {
        self.parsable_but_excluded
            .iter()
            .map(|(ext, count)| format!(".{ext}: {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// REQ-AXO-902045 MUR 0 — true when the file walker must NOT descend into
/// `dir`: it is a build-output / dependency-store / VCS / tooling-state
/// directory the indexing policy excludes (node_modules, target, _build,
/// .git, .axon, .venv, …), honouring the soft-exclude allowlist. Identical
/// verdict to the per-file noise filter (`classify_path`), lifted to DESCENT
/// so the reconciliation walk never enumerates the millions of build artefacts
/// it would reject per-file anyway — the host-wide-watch CPU spin. Single
/// source of truth with the per-file path (GUI-PRO-013).
pub(crate) fn is_noise_directory(root: &Path, dir: &Path) -> bool {
    !matches!(
        classify_path(
            root,
            dir,
            &crate::config::CONFIG.indexing,
            supported_parser_ecosystems(),
        ),
        PathDisposition::Allow
    )
}

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
    /// ist.IndexedFile. A file whose mtime/size changed is stamped
    /// `status='discovered'`; an unchanged row is not rewritten at all.
    ///
    /// REQ-AXO-902260 — that stamp is a RECORD, not an enqueue. The sentence
    /// removed from here ("the DBQ-A claim feeder (REQ-AXO-901897) drains those
    /// rows into pipeline A") described a component REQ-AXO-901916 / PIL-AXO-007
    /// deleted: nothing selects that column. Files reach pipeline A because the
    /// walk streams their paths into it directly, and a row can therefore sit at
    /// 'discovered' forever while being fully indexed (measured on AXO: 779 of
    /// 781 such rows were chunked). Never read this column as a backlog —
    /// coverage truth is chunk presence (`diagnose_indexing`, REQ-AXO-902254).
    ///
    /// The legacy in-memory ingress_buffer + FileIngressGuard push was RIPPED in
    /// the LEGACY FEED PURGE (REQ-AXO-901893).
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

    // REQ-AXO-902634 — `should_buffer_subtree_hint` a ete RETIREE le 2026-09-07.
    // Elle decidait s'il fallait bufferiser un hint de sous-arbre pour un
    // evenement de repertoire ; ses SEULS appelants etaient deux tests. Le
    // consommateur cible, `record_subtree_hint`, n'a jamais existe, et
    // `REQ-AXO-901893` a remplace tout le trajet par l'enrolement direct dans
    // `ist.IndexedFile`. Le pruning de repertoires vit desormais a un seul
    // endroit : `build_walker_from` -> `is_noise_directory` -> `classify_path`.

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

    /// REQ-AXO-901932 F2 — walk the project tree once and explain, in a single
    /// call, why the on-disk file population differs from the indexed set.
    /// Build/dependency/VCS directories are pruned at descent (so a 126k-file
    /// `cargo-target` / `node_modules` is never enumerated); every remaining
    /// file is bucketed as eligible (would be indexed) or excluded-by-reason
    /// using the same predicates the live indexer applies. Lets an LLM assert
    /// "all relevant source is indexed" without a second round-trip.
    pub fn scope_breakdown(&self) -> ScopeBreakdown {
        let mut eligible = 0u64;
        let mut walked = 0u64;
        let mut reasons: HashMap<String, u64> = HashMap::new();
        let mut parsables: HashMap<String, u64> = HashMap::new();
        // REQ-AXO-902636 — calcule UNE fois, depuis la config EFFECTIVE.
        let non_admises =
            extensions_parsables_non_admises(&crate::config::CONFIG.indexing.supported_extensions);
        // `build_walker_from` already prunes build/dependency/VCS/tooling
        // directories at descent (REQ-AXO-902045 MUR 0), so this walk only
        // enumerates the source neighbourhood — never the millions of build
        // artefacts. Per-file eligibility decides the rest.
        for entry in self
            .build_walker_from(&self.root)
            .build()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }
            walked += 1;
            let path = entry.path();
            if self.should_process_path(path) {
                eligible += 1;
            } else {
                let raison = self.explain_ignore_decision(path, false);
                // REQ-AXO-902636 — separer le bruit legitime du code source que
                // le systeme SAIT lire et n'admet pas.
                if raison == "ignored_by_extension_or_hidden_filter" {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext = ext.to_lowercase();
                        if non_admises.contains(&ext.as_str()) {
                            *parsables.entry(ext).or_insert(0) += 1;
                        }
                    }
                }
                *reasons.entry(raison).or_insert(0) += 1;
            }
        }

        let mut excluded_by_reason: Vec<(String, u64)> = reasons.into_iter().collect();
        excluded_by_reason.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut parsable_but_excluded: Vec<(String, u64)> = parsables.into_iter().collect();
        parsable_but_excluded.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        ScopeBreakdown {
            eligible,
            walked_files: walked,
            excluded_by_reason,
            parsable_but_excluded,
        }
    }

    fn extract_project_code(&self, graph: &GraphStore, path: &Path) -> Result<String> {
        let explicit = self.project_code.trim();
        if !explicit.is_empty() {
            return crate::pipeline::project_resolver::ProjectCode::parse(explicit)
                .map(|code| code.as_str().to_string())
                .map_err(anyhow::Error::from);
        }

        let candidate = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        crate::project_meta::resolve_project_identity_for_path(graph, &candidate).and_then(
            |identity| {
                crate::pipeline::project_resolver::ProjectCode::parse(&identity.code)
                    .map(|code| code.as_str().to_string())
                    .map_err(anyhow::Error::from)
            },
        )
    }

    fn build_walker_from(&self, start: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(start);
        builder.hidden(false);
        builder.git_ignore(false);
        builder.git_global(crate::config::CONFIG.indexing.use_git_global_ignore);
        builder.git_exclude(false);
        // REQ-AXO-902045 MUR 0 — prune build/dependency/VCS/tooling directories
        // at DESCENT so a host-wide watch root never STATs the millions of build
        // artefacts under node_modules/target/.axon/_build/… just to keep the
        // ~tens of k source files (the reconciliation-walk CPU spin). Per-file
        // eligibility (`should_process_path`) still runs downstream unchanged, so
        // the YIELDED file set is identical — only the wasted traversal is cut.
        // 'static closure (captures the root only) per `ignore`'s filter_entry
        // contract.
        let prune_root = self.root.clone();
        builder.filter_entry(move |entry| {
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            !is_dir || entry.depth() == 0 || !is_noise_directory(&prune_root, entry.path())
        });
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
                self.cached_matcher_for(&self.gitignore_cache, &dir, &dir.join(".gitignore"))
            {
                let matched = matcher.matched_path_or_any_parents(&absolute, is_dir);
                if matched.is_ignore() {
                    decision = Some(true);
                } else if matched.is_whitelist() {
                    decision = Some(false);
                }
            }
            if let Some(matcher) = self.cached_matcher_for(
                &self.git_exclude_cache,
                &dir,
                &dir.join(".git/info/exclude"),
            ) {
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
                self.cached_matcher_for(&self.axoninclude_cache, &dir, &dir.join(".axoninclude"))
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
                if let Some(matcher) = self.cached_matcher_for(cache, &dir, &dir.join(ignore_name))
                {
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
                    // REQ-AXO-902655 (Feedback #425) — Mesurer l'enrôlement réel :
                    // N'incrémenter total_files que si la persistance dans ist.IndexedFile réussit.
                    if dispatch_scanner_batch(&graph, &batch) {
                        total_files += batch.len();
                    } else {
                        error!(
                            "Scanner: durable discovery batch dispatch failed for {} files",
                            batch.len()
                        );
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
            if dispatch_scanner_batch(&graph, &batch) {
                total_files += batch.len();
            } else {
                error!(
                    "Scanner: durable discovery batch dispatch failed for remaining {} files",
                    batch.len()
                );
            }
        }

        total_files
    }

    /// Build (and cache) an ignore matcher for `matcher_path`, ROOTED at
    /// `root_dir`. REQ-AXO-901893 — `Gitignore::new(path)` roots the matcher at
    /// `path.parent()`, which is correct for `<dir>/.gitignore` (parent == dir)
    /// but WRONG for `<dir>/.git/info/exclude` (parent == `<dir>/.git/info`).
    /// Querying such a matcher with a repo-relative file path under `<dir>` then
    /// panics inside the `ignore` crate ("path is expected to be under the
    /// root") and the indexer task that called it dies, silently DROPPING the
    /// file. Git's exclude patterns are relative to the repo root, so the
    /// matcher must be rooted at `root_dir` (== `dir`) regardless of how deeply
    /// the rule file is nested. `GitignoreBuilder::new(root_dir).add(file)`
    /// roots explicitly; we pass `dir` at every call site.
    fn cached_matcher_for(
        &self,
        cache: &MatcherCache,
        root_dir: &Path,
        matcher_path: &Path,
    ) -> Option<Arc<Gitignore>> {
        let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(existing) = cache.get(matcher_path) {
            return existing.clone();
        }

        let matcher = if matcher_path.exists() {
            let mut builder = GitignoreBuilder::new(root_dir);
            // `add` returns Some(Error) on a malformed pattern line; the other
            // lines still load, so we keep the partial matcher (best-effort
            // filtering, matching the old `Gitignore::new` tolerance). A hard
            // build failure → None (treat as "no rules" — index the file, the
            // safe default; never drop on a matcher we cannot construct).
            let _ = builder.add(matcher_path);
            builder.build().ok().map(Arc::new)
        } else {
            None
        };
        cache.insert(matcher_path.to_path_buf(), matcher.clone());
        matcher
    }

    /// REQ-AXO-902633 — perimer les matchers mis en cache pour UN fichier de
    /// controle d'ignore. Rend `true` si au moins une entree a ete retiree.
    ///
    /// Le defaut repare : `cached_matcher_for` memorise aussi l'ABSENCE (une
    /// entree `None`), et rien ne la retirait jamais. Une regle ecrite apres le
    /// demarrage — un `.axoninclude` cree pour reintroduire un locataire — ne
    /// pouvait donc etre lue qu'en redemarrant l'indexeur.
    ///
    /// La cle visee est celle que les cinq chargeurs construisent :
    /// `dir.join(<suffixe>)`, ou `dir` vient de `ancestor_chain(root, absolute)`
    /// et est donc ABSOLU et CANONIQUE. On canonicalise le repertoire porteur
    /// (il existe encore meme quand le fichier vient d'etre supprime) plutot que
    /// le fichier lui-meme. Purger la mauvaise cle rendrait cette fonction
    /// inerte sans qu'aucun appel n'echoue — c'est ce que la garde verifie.
    pub fn forget_ignore_rules_for(&self, control_path: &Path) -> bool {
        let Some((porteur, suffixe)) = dossier_porteur_de_controle(control_path) else {
            return false;
        };
        let porteur = std::fs::canonicalize(&porteur).unwrap_or(porteur);
        let cle = porteur.join(suffixe);

        let mut retire = false;
        for cache in [
            &self.gitignore_cache,
            &self.git_exclude_cache,
            &self.axoninclude_cache,
            &self.axonignore_cache,
            &self.axonignore_local_cache,
        ] {
            let mut cache = cache.lock().unwrap_or_else(|poison| poison.into_inner());
            if cache.remove(&cle).is_some() {
                retire = true;
            }
        }
        retire
    }
}

/// REQ-AXO-902633 — les suffixes qui PORTENT une regle d'ignore, dans la forme
/// dont la purge a besoin. Meme ensemble que `is_ignore_control_path`, mais vu
/// depuis la cle de cache : le suffixe permet de remonter au repertoire
/// porteur, seul endroit ou le matcher est enregistre.
///
/// `.axonignore.local` precede `.axonignore` par prudence de lecture ; l'ordre
/// n'est pas load-bearing (`Path::ends_with` compare des composants ENTIERS,
/// jamais des sous-chaines).
const IGNORE_CONTROL_SUFFIXES: &[&str] = &[
    ".gitignore",
    ".axonignore.local",
    ".axonignore",
    ".axoninclude",
    ".git/info/exclude",
];

/// Rend le repertoire porteur d'un fichier de controle et le suffixe reconnu.
/// `None` quand le chemin n'est pas un fichier de regles — la purge ne touche
/// alors a rien.
fn dossier_porteur_de_controle(control_path: &Path) -> Option<(PathBuf, &'static str)> {
    for suffixe in IGNORE_CONTROL_SUFFIXES {
        if control_path.ends_with(suffixe) {
            let profondeur = Path::new(suffixe).components().count();
            let mut porteur = control_path.to_path_buf();
            for _ in 0..profondeur {
                porteur.pop();
            }
            return Some((porteur, suffixe));
        }
    }
    None
}

fn dispatch_scanner_batch(graph: &Arc<GraphStore>, batch: &[(String, String, i64, i64)]) -> bool {
    // DEC-AXO-901619: durable discovery — batch UPSERT into ist.IndexedFile.
    // REQ-AXO-902260 — this UPSERT RECORDS what the walk saw; it does not enqueue
    // anything. The claim feeder that used to consume these rows (REQ-AXO-901897) was
    // deleted by REQ-AXO-901916 / PIL-AXO-007; the walk streams the paths into pipeline A
    // itself. The legacy in-memory ingress_buffer push was RIPPED in the LEGACY FEED
    // PURGE (REQ-AXO-901893).
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
    let mut values = Vec::with_capacity(batch.len());
    for (path, project, size, mtime) in batch {
        let safe_path = path.replace('\'', "''");
        let safe_project = project.replace('\'', "''");
        // mtime from scanner is seconds, convert to ms for consistency
        let mtime_ms = *mtime * 1000;
        values.push(format!(
            "('{safe_path}', '{safe_project}', '', {now_ms}, 'discovered', {now_ms}, {mtime_ms}, {size}, 0, NULL)"
        ));
    }
    // Tenant creation belongs exclusively to the canonical registry. The FK on
    // IndexedFile.project_code is the final fail-closed guard if registry and
    // runtime admission ever diverge.
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

/// REQ-AXO-902636 — les extensions qu'un parser sait lire et que la
/// configuration EFFECTIVE n'admet PAS. Fonction PURE : elle prend la liste
/// admise en parametre, parce que `config::CONFIG` est un `Lazy` global qu'un
/// test ne peut pas simuler — le patron `_with_config` du meme fichier.
///
/// Pourquoi la config EFFECTIVE et pas le defaut compile : la garde unitaire de
/// REQ-AXO-902631 protege `default_supported_extensions()`, mais
/// `supported_extensions` porte `#[serde(default = …)]` — un
/// `.axon/capabilities.toml` peut le surcharger et reproduire exactement le
/// defaut d'origine (1 054 fichiers du parc hors index, cinq langages muets)
/// sans qu'aucun test unitaire le voie. C'est le seul endroit ou la surcharge
/// se constate.
pub fn extensions_parsables_non_admises(admises: &[String]) -> Vec<&'static str> {
    crate::parser::PARSEABLE_EXTENSIONS
        .iter()
        .copied()
        .filter(|ext| !admises.iter().any(|a| a.to_lowercase() == *ext))
        .collect()
}

/// REQ-AXO-902632 — la decision de REFUS d'enrolement, extraite en fonction
/// PURE pour etre exercable sans serveur MCP.
///
/// `rescan_project` juge l'eligibilite depuis la racine du PROJET ; la marche de
/// reconciliation et la purge jugent depuis la racine de SURVEILLANCE. Quand les
/// deux divergent, l'enrolement ecrit des lignes que rien ne parsera et que la
/// purge effacera — 8 190 pour le tenant DFD le 2026-09-06, pendant que l'outil
/// rendait `enrolled:8190`.
///
/// Rend `Some(raison)` quand l'enrolement doit etre refuse, `None` quand il peut
/// avoir lieu. Un chemin HORS de la racine de surveillance n'est pas refuse :
/// cette racine ne le gouverne pas, et le refuser bloquerait les tenants hors
/// parc (BOO vit sous `/home/dstadel/`).
pub fn refus_d_enrolement(
    watch_root: &str,
    project_path: &Path,
    project_code: &str,
) -> Option<String> {
    if !project_path.starts_with(watch_root) {
        return None;
    }
    let gardien = Scanner::new(watch_root, project_code);
    let verdict = gardien.explain_ignore_decision(project_path, true);
    if verdict == "eligible" || verdict == "included_by_axoninclude" {
        return None;
    }
    Some(verdict)
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
    use super::{discovery_policy, is_noise_directory, Scanner};
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
            soft_excluded_directory_segments_allowlist: vec![],
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
        // REQ-AXO-902274 / REQ-AXO-902630 — l'etat de `service_guard` est
        // PROCESSUS-global : ce reset corrompait les tests qui le lisaient au
        // meme instant, pas celui-ci. La garde ne le voyait pas parce qu'elle
        // filtrait sur le CHEMIN du fichier.
        let _sg_guard = crate::test_support::service_guard_test_lock().lock();
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

    /// REQ-AXO-901932 F2 — scope_breakdown explains the eligible↔indexed gap
    /// on a real tree: source files count as eligible, out-of-ecosystem data
    /// files (.csv/.json) bucket under the extension reason, gitignored source
    /// buckets under the gitignore reason, and a node_modules dependency tree
    /// is pruned at descent (never enumerated), not counted file-by-file.
    #[test]
    fn test_scope_breakdown_buckets_excluded_files_by_reason_and_prunes_noise_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let prj = root.join("prj");
        std::fs::create_dir_all(prj.join("data")).unwrap();
        std::fs::create_dir_all(prj.join("node_modules").join("react")).unwrap();

        // Eligible source.
        std::fs::write(prj.join("main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(prj.join("lib.py"), "x = 1\n").unwrap();
        // Out-of-ecosystem data artefacts (the data-centric gap the F2 friction
        // could not explain). `.csv`/`.parquet` are NOT in supported_extensions
        // (unlike `.json`, which IS indexed), so both bucket under the extension
        // reason.
        std::fs::write(prj.join("data").join("panel.csv"), "a,b\n1,2\n").unwrap();
        std::fs::write(prj.join("data").join("prices.parquet"), "BINARY\n").unwrap();
        // Gitignored source.
        std::fs::write(prj.join(".gitignore"), "secret.py\n").unwrap();
        std::fs::write(prj.join("secret.py"), "TOKEN = 1\n").unwrap();
        // Dependency tree — must be pruned at descent, not walked file-by-file.
        std::fs::write(
            prj.join("node_modules").join("react").join("index.js"),
            "//\n",
        )
        .unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let bd = scanner.scope_breakdown();

        // main.rs + lib.py are eligible; secret.py is gitignored (not eligible),
        // csv/parquet are out-of-ecosystem.
        assert_eq!(
            bd.eligible, 2,
            "two source files should be eligible: {bd:?}"
        );

        let reason = |key: &str| -> u64 {
            bd.excluded_by_reason
                .iter()
                .find(|(r, _)| r == key)
                .map(|(_, c)| *c)
                .unwrap_or(0)
        };
        // .csv + .parquet (+ the .gitignore control file) → extension bucket.
        assert!(
            reason("ignored_by_extension_or_hidden_filter") >= 2,
            "csv+parquet data artefacts must bucket under the extension reason: {bd:?}"
        );
        // secret.py → gitignore bucket.
        assert_eq!(
            reason("ignored_by_gitignore_or_exclude"),
            1,
            "gitignored source must bucket under the gitignore reason: {bd:?}"
        );
        // node_modules was pruned at descent: its index.js is never enumerated,
        // so it appears in NO bucket (not even the hard-deny per-file one).
        assert!(
            !bd.excluded_by_reason
                .iter()
                .any(|(r, _)| r == "ignored_by_hard_deny_directory_segment"),
            "pruned dependency files must not appear as per-file exclusions: {bd:?}"
        );
    }

    /// REQ-AXO-902045 MUR 0 — is_noise_directory matches the indexing policy:
    /// build/dependency/VCS dirs are noise (prune at descent); ordinary source
    /// dirs and non-excluded dot-dirs (e.g. `.github`) are NOT pruned.
    #[test]
    fn test_is_noise_directory_matches_indexing_policy() {
        let root = Path::new("/workspace");
        for noise in [
            "/workspace/proj/node_modules",
            "/workspace/proj/target",
            "/workspace/proj/.git",
            "/workspace/proj/.axon",
            "/workspace/proj/_build",
        ] {
            assert!(
                super::is_noise_directory(root, Path::new(noise)),
                "must be pruned at descent: {noise}"
            );
        }
        for keep in [
            "/workspace/proj/src",
            "/workspace/proj/lib",
            "/workspace/proj/.github", // dot-dir but NOT a policy exclusion
        ] {
            assert!(
                !super::is_noise_directory(root, Path::new(keep)),
                "must NOT be pruned: {keep}"
            );
        }
    }

    /// REQ-AXO-902045 MUR 0 — the file walker prunes noise directories AT
    /// DESCENT: a dependency tree is never enumerated (the raw walker yields no
    /// entry under it), while ordinary source is walked. This is the cut that
    /// turns a 2M-entry host-wide traversal into the source neighbourhood.
    #[test]
    fn test_build_walker_prunes_noise_dirs_at_descent() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let prj = root.join("prj");
        std::fs::create_dir_all(prj.join("node_modules").join("react").join("deep")).unwrap();
        std::fs::create_dir_all(prj.join("src")).unwrap();
        std::fs::write(prj.join("src").join("main.rs"), "fn main(){}\n").unwrap();
        std::fs::write(
            prj.join("node_modules")
                .join("react")
                .join("deep")
                .join("index.js"),
            "//\n",
        )
        .unwrap();

        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let walked: Vec<String> = scanner
            .build_walker_from(root)
            .build()
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_string_lossy().into_owned())
            .collect();

        assert!(
            walked.iter().any(|p| p.ends_with("src/main.rs")),
            "source must be walked: {walked:?}"
        );
        assert!(
            !walked.iter().any(|p| p.contains("node_modules")),
            "node_modules must be pruned at descent, never enumerated: {walked:?}"
        );
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

    /// REQ-AXO-902634 — meme intention qu'avant, portee sur l'AUTORITE QUI
    /// DECIDE. Ces trois chemins etaient interroges via
    /// `should_buffer_subtree_hint`, dont aucun appelant de production
    /// n'existait : le test etait vert et le repertoire etait parcouru quand
    /// meme. `is_noise_directory` est le predicat que `build_walker_from`
    /// consulte reellement a la descente.
    #[test]
    fn le_walker_ne_descend_pas_dans_les_repertoires_de_build() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        for relative in [
            Path::new("prj/_build"),
            Path::new("prj/node_modules"),
            Path::new("prj/.devenv/state/postgres/pg_wal"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert!(
                is_noise_directory(root, path.as_path()),
                "Le walker ne doit pas descendre dans {:?}",
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
                is_noise_directory(root, path.as_path()),
                "Le walker doit pruner {:?} a la descente",
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
    fn test_soft_excluded_vendor_can_be_reopened_for_scanner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");
        let mut config = test_config();
        let vendor = root.join("prj/vendor");
        std::fs::create_dir_all(&vendor).unwrap();

        assert!(scanner.path_has_ignored_directory_noise_with_config(vendor.as_path(), &config));

        config.soft_excluded_directory_segments_allowlist = vec!["vendor".to_string()];

        assert!(!scanner.path_has_ignored_directory_noise_with_config(vendor.as_path(), &config));
    }

    #[test]
    fn test_default_config_exposes_ignored_directory_segments() {
        let parsed: crate::config::Config = toml::from_str("[indexing]\n").unwrap();
        let ignored = &parsed.indexing.ignored_directory_segments;
        assert!(ignored.iter().any(|segment| segment == ".fastembed_cache"));
        // REQ-AXO-902638 — arbitrage operateur du 2026-09-07 : les trois segments
        // que la liste morte de `REQ-AXO-902634` portait sans jamais les appliquer
        // sont ADMIS. Verifie ici sur la config PARSEE, pas seulement sur le
        // fournisseur de defaut : c'est ce chemin-la que le scanner emprunte.
        for admis in ["pg_wal", "_bmad", "_bmad-output"] {
            assert!(
                ignored.iter().any(|segment| segment == admis),
                "`{admis}` a ete admis par l'operateur et doit atteindre la config parsee"
            );
        }
        assert!(!ignored.iter().any(|segment| segment == "vendor"));
        assert!(!ignored.iter().any(|segment| segment == "build"));
        assert!(!ignored.iter().any(|segment| segment == "dist"));
        assert!(parsed
            .indexing
            .soft_excluded_directory_segments_allowlist
            .is_empty());
    }

    /// REQ-AXO-901893 — a `<repo>/.git/info/exclude` rule file is rooted at the
    /// REPO dir, NOT `<repo>/.git/info`. Before the fix the matcher was built via
    /// `Gitignore::new(.git/info/exclude)`, which the `ignore` crate roots at the
    /// file's parent (`.git/info`); querying a repo-relative file path then
    /// panicked ("path is expected to be under the root"), killing the indexer
    /// task and silently DROPPING the file. Every git repo with an exclude file
    /// under the watch root tripped it once inotify forced the scanner path.
    /// This locks: no panic + the exclude pattern is honoured at the repo root.
    #[test]
    fn git_info_exclude_rooted_at_repo_does_not_panic_and_is_honoured() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let scanner = Scanner::new(root.to_string_lossy().as_ref(), "PRJ");

        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git/info")).unwrap();
        std::fs::write(repo.join(".git/info/exclude"), "*.log\n").unwrap();

        let ignored = repo.join("debug.log");
        std::fs::write(&ignored, "x").unwrap();
        let kept = repo.join("main.rs");
        std::fs::write(&kept, "fn main() {}").unwrap();

        // The assertion that matters is that NEITHER call panics. The exclude
        // rule, now rooted at `repo`, must also actually match.
        assert!(
            scanner.is_ignored_by_git_rules(&ignored, false),
            "*.log must be ignored by .git/info/exclude rooted at the repo dir"
        );
        assert!(
            !scanner.is_ignored_by_git_rules(&kept, false),
            "main.rs must not be ignored"
        );
    }

    // ───────────────────────────────────────────────────────────────────
    // REQ-AXO-901901 — durable bootstrap/reconciliation walk regression tests.
    //
    // The live stall (this session) was: Watchman's fresh crawl under-delivered
    // the cold-start bulk (5.3K of 18K eligible), and nothing else enrolled the files
    // (the bootstrap scanner walk was removed in the LEGACY FEED PURGE but the comments
    // still claimed it ran). These lock the two invariants the wiring fix depends on:
    // scan() ENROLS eligible files, and the stale reconciliation it runs at the end of
    // every walk NEVER erodes existing indexed data.
    // ───────────────────────────────────────────────────────────────────

    /// scan() must UPSERT every eligible source file as status='discovered'
    /// (discovered_ms>0) while pruning build-output directories.
    ///
    /// REQ-AXO-902260 — what this asserts is ENROLMENT, not queueing. The claim feeder
    /// this docstring used to name was deleted (REQ-AXO-901916 / PIL-AXO-007); files
    /// reach pipeline A because the walk streams them there. The assertion is still
    /// worth keeping: enrolment is what the dedup cache and the stale-reconciliation
    /// pass are built on.
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
            .execute("INSERT INTO axon.Project (code, enrolled_at_ms) VALUES ('TST', 1) ON CONFLICT (code) DO NOTHING")
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
            .execute("INSERT INTO axon.Project (code, enrolled_at_ms) VALUES ('TST', 1) ON CONFLICT (code) DO NOTHING")
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

/// REQ-AXO-902632 — la divergence d'autorite qui a fabrique 8 190 lignes
/// fantomes : `rescan_project` juge l'eligibilite depuis la racine du PROJET,
/// la marche de reconciliation et la purge depuis la racine de SURVEILLANCE.
#[cfg(test)]
mod eligibilite_selon_la_racine_tests {
    use super::*;

    /// Le meme repertoire, juge depuis deux racines, rend deux verdicts
    /// OPPOSES. C'est le defaut entier, en une assertion : sans elle, un
    /// correctif qui ferait juger `rescan_project` depuis la mauvaise racine
    /// repasserait sans bruit.
    #[test]
    fn un_projet_exclu_par_l_ancetre_est_eligible_vu_de_lui_meme() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        std::fs::write(racine.join(".axonignore"), "/locataire/\n").unwrap();
        let projet = racine.join("locataire");
        std::fs::create_dir_all(projet.join("src")).unwrap();
        std::fs::write(projet.join("src/a.rs"), "fn a() {}").unwrap();

        let depuis_la_surveillance = Scanner::new(racine.to_str().unwrap(), "TST");
        let depuis_le_projet = Scanner::new(projet.to_str().unwrap(), "TST");

        assert_eq!(
            depuis_la_surveillance.explain_ignore_decision(&projet, true),
            "ignored_by_legacy_axonignore",
            "la racine de surveillance doit exclure le locataire"
        );
        assert_eq!(
            depuis_le_projet.explain_ignore_decision(&projet, true),
            "eligible",
            "vu de lui-meme le projet se croit eligible — c'est CE verdict qui \
             enrolait des lignes que la purge efface ensuite"
        );
    }

    /// Le pendant necessaire : sans lui, un refus qui bloquerait TOUT passerait
    /// la garde precedente. Un locataire non exclu doit rester enrolable.
    #[test]
    fn un_projet_non_exclu_reste_eligible_depuis_la_surveillance() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        std::fs::write(racine.join(".axonignore"), "/autre/\n").unwrap();
        let projet = racine.join("locataire");
        std::fs::create_dir_all(&projet).unwrap();

        let depuis_la_surveillance = Scanner::new(racine.to_str().unwrap(), "TST");
        assert_eq!(
            depuis_la_surveillance.explain_ignore_decision(&projet, true),
            "eligible"
        );
    }

    /// REQ-AXO-902636 — la decision, exercee dans les DEUX sens sur une liste
    /// admise FABRIQUEE : c'est le seul moyen de simuler la surcharge
    /// `.axon/capabilities.toml` que `CONFIG` (un `Lazy` global) rend
    /// intestable autrement.
    #[test]
    fn une_extension_parsable_absente_de_la_config_est_denoncee() {
        let amputee: Vec<String> = ["rs", "py", "md"].iter().map(|s| s.to_string()).collect();
        let manquantes = extensions_parsables_non_admises(&amputee);
        assert!(
            manquantes.contains(&"hpp"),
            "hpp a un parser et n'est pas admis — il doit etre denonce : {manquantes:?}"
        );
        assert!(
            !manquantes.contains(&"rs"),
            "rs est admis, il ne doit PAS etre denonce : {manquantes:?}"
        );
    }

    /// Le pendant NECESSAIRE : sans lui, une fonction qui denoncerait TOUT
    /// passerait la garde d'au-dessus.
    #[test]
    fn une_config_complete_ne_denonce_rien() {
        let complete: Vec<String> = crate::parser::PARSEABLE_EXTENSIONS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(extensions_parsables_non_admises(&complete).is_empty());
    }

    /// REQ-AXO-902636 — la garde qui MANQUAIT a REQ-AXO-902631 : celle-ci lit la
    /// configuration EFFECTIVE de CETTE machine, pas le defaut compile. Un
    /// `.axon/capabilities.toml` qui amputerait `supported_extensions` la fait
    /// rougir, la ou le test unitaire de `config.rs` resterait vert.
    #[test]
    fn la_config_effective_admet_tout_ce_que_le_parser_sait_lire() {
        let manquantes =
            extensions_parsables_non_admises(&crate::config::CONFIG.indexing.supported_extensions);
        assert!(
            manquantes.is_empty(),
            "la config EFFECTIVE ecarte des extensions parsables — leur parser est inatteignable sur cette machine : {manquantes:?}"
        );
    }

    /// REQ-AXO-902632 — LA garde du refus : un projet exclu par sa racine de
    /// surveillance ne doit PAS etre enrole, quelle que soit l'opinion qu'il a
    /// de lui-meme.
    #[test]
    fn le_refus_nomme_la_regle_qui_exclut() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path().to_str().unwrap();
        std::fs::write(parc.path().join(".axonignore"), "/locataire/\n").unwrap();
        let projet = parc.path().join("locataire");
        std::fs::create_dir_all(&projet).unwrap();

        assert_eq!(
            refus_d_enrolement(racine, &projet, "TST").as_deref(),
            Some("ignored_by_legacy_axonignore"),
            "le refus doit NOMMER la regle, pas juste refuser"
        );
    }

    /// Le pendant : sans lui, un refus systematique passerait la garde d'au-dessus.
    #[test]
    fn un_projet_eligible_n_est_pas_refuse() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path().to_str().unwrap();
        let projet = parc.path().join("locataire");
        std::fs::create_dir_all(&projet).unwrap();

        assert!(refus_d_enrolement(racine, &projet, "TST").is_none());
    }

    /// REQ-AXO-902632 — le cas de DVM et SWT apres la decision operateur du
    /// 2026-09-06 : exclus de GIT (donnees sensibles, jamais dans le depot
    /// partage) mais REINTRODUITS dans l'index par `.axoninclude`. Le refus doit
    /// laisser passer, sinon la reintroduction ne sert a rien.
    #[test]
    fn un_axoninclude_leve_le_refus_pose_par_gitignore() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path().to_str().unwrap();
        std::fs::write(parc.path().join(".gitignore"), "sensible/\n").unwrap();
        let projet = parc.path().join("sensible");
        std::fs::create_dir_all(&projet).unwrap();

        assert_eq!(
            refus_d_enrolement(racine, &projet, "TST").as_deref(),
            Some("ignored_by_gitignore_or_exclude"),
            "sans .axoninclude le refus doit tomber"
        );

        std::fs::write(parc.path().join(".axoninclude"), "sensible/\n").unwrap();
        let apres = refus_d_enrolement(racine, &projet, "TST");
        assert!(
            apres.is_none(),
            "l'.axoninclude doit lever le refus, sinon la decision operateur \
             (indexer ce que git ecarte) reste lettre morte — obtenu {apres:?}"
        );
    }

    /// Un tenant HORS de la racine de surveillance (BOO vit sous `/home/dstadel/`)
    /// ne doit pas etre refuse : cette racine ne le gouverne pas.
    #[test]
    fn un_chemin_hors_de_la_racine_n_est_pas_refuse() {
        let parc = tempfile::tempdir().unwrap();
        let ailleurs = tempfile::tempdir().unwrap();
        std::fs::write(parc.path().join(".axonignore"), "/*\n").unwrap();
        assert!(
            refus_d_enrolement(parc.path().to_str().unwrap(), ailleurs.path(), "TST").is_none()
        );
    }

    /// Le cas reel de DVM et SWT : exclus par le `.gitignore` de la racine, pas
    /// par le `.axonignore`. `should_descend_into_directory` ne les voit PAS
    /// (il ne prune jamais sur gitignore seul) — d'ou le choix
    /// d'`explain_ignore_decision` comme predicat du refus.
    #[test]
    fn une_exclusion_gitignore_de_l_ancetre_est_vue_par_explain() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        std::fs::write(racine.join(".gitignore"), "sensible/\n").unwrap();
        let projet = racine.join("sensible");
        std::fs::create_dir_all(&projet).unwrap();

        let depuis_la_surveillance = Scanner::new(racine.to_str().unwrap(), "TST");
        assert_eq!(
            depuis_la_surveillance.explain_ignore_decision(&projet, true),
            "ignored_by_gitignore_or_exclude"
        );
        assert!(
            depuis_la_surveillance.should_descend_into_directory(&projet),
            "should_descend_into_directory ne prune PAS sur gitignore — c'est \
             pourquoi il ne peut pas servir de predicat au refus"
        );
    }
}

#[cfg(test)]
mod invalidation_des_regles_tests {
    use super::*;

    /// REQ-AXO-902633 — LE defaut, en trois observations sur la MEME decision :
    /// elle est fausse tant que le cache n'est pas purge, et elle devient juste
    /// apres. Sans l'observation du milieu, une purge inerte passerait la garde.
    #[test]
    fn une_regle_ecrite_apres_la_mise_en_cache_reste_invisible_jusqu_a_la_purge() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        std::fs::write(racine.join(".gitignore"), "note.rs\n").unwrap();
        let note = racine.join("note.rs");
        std::fs::write(&note, "fn note() {}").unwrap();

        let scanner = Scanner::new(racine.to_str().unwrap(), "TST");

        // 1. Premiere decision : exclue. Elle met AUSSI en cache l'ABSENCE du
        //    `.axoninclude` — l'entree `None` que rien ne retirait jamais.
        assert!(
            !scanner.should_process_path(&note),
            "le .gitignore doit ecarter le fichier"
        );

        // 2. La regle qui le reintroduit est ecrite maintenant, indexeur vivant.
        let inclusion = racine.join(".axoninclude");
        std::fs::write(&inclusion, "note.rs\n").unwrap();
        assert!(
            !scanner.should_process_path(&note),
            "sans purge la regle neuve reste INVISIBLE — c'est le defaut repare"
        );

        // 3. La purge vise la cle reelle du cache ; la decision change.
        assert!(
            scanner.forget_ignore_rules_for(&inclusion),
            "la purge doit avoir retire une entree — sinon elle vise la mauvaise cle"
        );
        assert!(
            scanner.should_process_path(&note),
            "apres purge la regle neuve doit s'appliquer"
        );
    }

    /// Le cas symetrique : un fichier de regles qui EXISTAIT deja et qu'on
    /// modifie. Sans lui, une purge qui ne saurait traiter que l'entree `None`
    /// passerait la garde precedente.
    #[test]
    fn une_regle_modifiee_est_relue_apres_la_purge() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        let regles = racine.join(".gitignore");
        std::fs::write(&regles, "autre.rs\n").unwrap();
        let note = racine.join("note.rs");
        std::fs::write(&note, "fn note() {}").unwrap();

        let scanner = Scanner::new(racine.to_str().unwrap(), "TST");
        assert!(scanner.should_process_path(&note));

        std::fs::write(&regles, "note.rs\n").unwrap();
        assert!(
            scanner.should_process_path(&note),
            "le matcher en cache tient encore l'ancienne regle"
        );

        assert!(scanner.forget_ignore_rules_for(&regles));
        assert!(
            !scanner.should_process_path(&note),
            "apres purge la regle modifiee doit exclure"
        );
    }

    /// La purge doit viser la CLE REELLE, absolue et canonique, celle que
    /// `ancestor_chain` construit. Un chemin non canonique (`.../a/../`) qui
    /// designe le meme fichier doit purger la meme entree : sinon le flux
    /// Watchman, qui joint sa racine telle qu'elle est configuree, purgerait
    /// dans le vide sans qu'aucun appel n'echoue.
    #[test]
    fn la_purge_vise_la_cle_canonique_pas_le_chemin_ecrit() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        std::fs::write(racine.join(".gitignore"), "note.rs\n").unwrap();
        let note = racine.join("note.rs");
        std::fs::write(&note, "fn note() {}").unwrap();
        std::fs::create_dir(racine.join("detour")).unwrap();

        let scanner = Scanner::new(racine.to_str().unwrap(), "TST");
        assert!(!scanner.should_process_path(&note));

        let inclusion = racine.join(".axoninclude");
        std::fs::write(&inclusion, "note.rs\n").unwrap();

        // Le MEME fichier, ecrit par un detour : la purge doit le reconnaitre.
        let detourne = racine.join("detour").join("..").join(".axoninclude");
        assert!(
            scanner.forget_ignore_rules_for(&detourne),
            "un chemin non canonique doit purger la meme entree"
        );
        assert!(scanner.should_process_path(&note));
    }

    /// Un chemin ordinaire ne purge rien : la fonction ne doit pas vider les
    /// caches a chaque fichier d'un lot Watchman, ce qui annulerait le cache.
    ///
    /// Les deux assertions ne disent PAS la meme chose, et la seconde est celle
    /// qui mord. Le controle-mutant l'a montre : rendre `false` parce que la
    /// cle visee n'etait de toute facon pas en cache est indistinguable d'un
    /// refus de reconnaitre le chemin. Une reconnaissance rendue AVEUGLE
    /// laissait passer la premiere assertion — c'est `dossier_porteur_de_controle`
    /// qu'il faut interroger pour prouver le REFUS lui-meme.
    #[test]
    fn un_chemin_ordinaire_ne_purge_rien() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        let scanner = Scanner::new(racine.to_str().unwrap(), "TST");

        for ordinaire in ["src/main.rs", "gitignore", ".gitignore.bak", "info/exclude"] {
            let chemin = racine.join(ordinaire);
            assert!(
                dossier_porteur_de_controle(&chemin).is_none(),
                "{ordinaire} n'est pas un fichier de regles — il doit etre REFUSE, \
                 pas simplement rendre une cle absente du cache"
            );
            assert!(!scanner.forget_ignore_rules_for(&chemin));
        }
    }

    /// Les CINQ familles de regles sont couvertes, `.git/info/exclude` compris —
    /// dont la cle n'est pas `dir.join(<nom de fichier>)` mais porte trois
    /// composants. Une purge qui n'aurait su remonter que d'un cran le raterait.
    #[test]
    fn les_cinq_familles_de_regles_sont_reconnues() {
        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        for suffixe in [
            ".gitignore",
            ".axonignore",
            ".axonignore.local",
            ".axoninclude",
            ".git/info/exclude",
        ] {
            let chemin = racine.join(suffixe);
            let (porteur, reconnu) = dossier_porteur_de_controle(&chemin)
                .unwrap_or_else(|| panic!("{suffixe} doit etre reconnu comme fichier de regles"));
            assert_eq!(reconnu, suffixe);
            assert_eq!(
                porteur, racine,
                "le repertoire porteur de {suffixe} est la racine, pas un intermediaire"
            );
        }
    }

    /// REQ-AXO-902655 (Feedback #425 DVM) — scan_subtree ne doit PAS comptabiliser
    /// les fichiers comme enrôlés si la persistance dans ist.IndexedFile échoue.
    #[test]
    fn scan_subtree_ne_compte_pas_les_fichiers_si_le_dispatch_echoue() {
        let store = crate::tests::test_helpers::create_test_db().expect("create test db");
        // Neutraliser l'auto-seed pour que la FK indexedfile_project_code_fkey s'applique
        crate::test_support::test_db::neutraliser_autoseed_des_parents_fk(&store).unwrap();

        let parc = tempfile::tempdir().unwrap();
        let racine = parc.path();
        let fichier = racine.join("main.rs");
        std::fs::write(&fichier, "fn main() {}\n").unwrap();

        // Project code inconnu dans axon.Project : le dispatch de persistance échouera sur la FK
        let scanner = Scanner::new(racine.to_str().unwrap(), "UNKNOWN_UNENROLLED_CODE");
        let enrolled = scanner.scan_subtree(std::sync::Arc::new(store), racine);

        assert_eq!(
            enrolled, 0,
            "scan_subtree ne doit pas compter comme enrôlé un fichier dont l'insertion a échoué"
        );
    }
}
