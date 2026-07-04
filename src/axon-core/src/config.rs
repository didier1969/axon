use once_cell::sync::Lazy;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub indexing: IndexingConfig,
}

#[derive(Debug, Deserialize)]
pub struct IndexingConfig {
    #[serde(default = "default_supported_extensions")]
    pub supported_extensions: Vec<String>,
    #[serde(default = "default_ignored_directory_segments")]
    pub ignored_directory_segments: Vec<String>,
    #[serde(default = "default_blocked_subtree_hint_segments")]
    pub blocked_subtree_hint_segments: Vec<String>,
    #[serde(default = "default_soft_excluded_directory_segments_allowlist")]
    pub soft_excluded_directory_segments_allowlist: Vec<String>,
    #[serde(default = "default_subtree_hint_cooldown_ms")]
    pub subtree_hint_cooldown_ms: u64,
    #[serde(default = "default_subtree_hint_retry_budget")]
    pub subtree_hint_retry_budget: u64,
    #[serde(default = "default_use_git_global_ignore")]
    pub use_git_global_ignore: bool,
    #[serde(default = "default_legacy_axonignore_additive")]
    pub legacy_axonignore_additive: bool,
    #[serde(default = "default_ignore_reconcile_enabled")]
    pub ignore_reconcile_enabled: bool,
    #[serde(default = "default_ignore_reconcile_dry_run")]
    pub ignore_reconcile_dry_run: bool,
}

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    load_config().unwrap_or_else(|_| Config {
        indexing: IndexingConfig {
            supported_extensions: default_supported_extensions(),
            ignored_directory_segments: default_ignored_directory_segments(),
            blocked_subtree_hint_segments: default_blocked_subtree_hint_segments(),
            soft_excluded_directory_segments_allowlist:
                default_soft_excluded_directory_segments_allowlist(),
            subtree_hint_cooldown_ms: default_subtree_hint_cooldown_ms(),
            subtree_hint_retry_budget: default_subtree_hint_retry_budget(),
            use_git_global_ignore: default_use_git_global_ignore(),
            legacy_axonignore_additive: default_legacy_axonignore_additive(),
            ignore_reconcile_enabled: default_ignore_reconcile_enabled(),
            ignore_reconcile_dry_run: default_ignore_reconcile_dry_run(),
        },
    })
});

fn default_supported_extensions() -> Vec<String> {
    vec![
        "py".to_string(),
        "ex".to_string(),
        "exs".to_string(),
        "rs".to_string(),
        "go".to_string(),
        "java".to_string(),
        "c".to_string(),
        "cpp".to_string(),
        "h".to_string(),
        "js".to_string(),
        "jsx".to_string(),
        "ts".to_string(),
        "tsx".to_string(),
        "sql".to_string(),
        "md".to_string(),
        "markdown".to_string(),
        "txt".to_string(),
        "json".to_string(),
        "yml".to_string(),
        "yaml".to_string(),
        "toml".to_string(),
        "conf".to_string(),
        "html".to_string(),
        "css".to_string(),
        // llmlang: a `.lll` file is parsed by the shell-out bridge (parser/lll.rs
        // → `lll export-ist`), which yields semantic symbols (content-hash,
        // purity, contracts). Without this the scanner excludes it pre-parse
        // (ignored_by_extension) — REQ-LLL-021.
        "lll".to_string(),
    ]
}

fn default_ignored_directory_segments() -> Vec<String> {
    vec![".fastembed_cache".to_string()]
}

fn default_blocked_subtree_hint_segments() -> Vec<String> {
    vec![
        "_bmad".to_string(),
        "_bmad-output".to_string(),
        "pg_wal".to_string(),
    ]
}

fn default_soft_excluded_directory_segments_allowlist() -> Vec<String> {
    Vec::new()
}

fn default_subtree_hint_cooldown_ms() -> u64 {
    15_000
}

fn default_subtree_hint_retry_budget() -> u64 {
    3
}

fn default_use_git_global_ignore() -> bool {
    false
}

fn default_legacy_axonignore_additive() -> bool {
    true
}

fn default_ignore_reconcile_enabled() -> bool {
    true
}

fn default_ignore_reconcile_dry_run() -> bool {
    true
}

fn load_config() -> anyhow::Result<Config> {
    // Try to find .axon/capabilities.toml in current or parent dirs
    let mut path = std::env::current_dir()?;
    loop {
        let config_path = path.join(".axon").join("capabilities.toml");
        if config_path.exists() {
            let content = fs::read_to_string(config_path)?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }
        if !path.pop() {
            break;
        }
    }
    anyhow::bail!("Config not found")
}

/// REQ-AXO-902190 lot 3 — coverage for the `serde(default = "...")` value
/// providers. Trivial in isolation, but each IS the load-bearing fallback
/// when `.axon/capabilities.toml` omits a field; a silent value change here
/// changes indexing behavior repo-wide without any config diff to review.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_supported_extensions_covers_core_languages_and_lll() {
        let exts = default_supported_extensions();
        for expected in ["rs", "py", "ex", "md", "lll"] {
            assert!(
                exts.iter().any(|e| e == expected),
                "expected {expected} in {exts:?}"
            );
        }
    }

    #[test]
    fn default_ignored_directory_segments_excludes_fastembed_cache() {
        assert_eq!(default_ignored_directory_segments(), vec![".fastembed_cache"]);
    }

    #[test]
    fn default_blocked_subtree_hint_segments_excludes_pg_wal() {
        let segments = default_blocked_subtree_hint_segments();
        assert!(segments.contains(&"pg_wal".to_string()));
        assert!(segments.contains(&"_bmad".to_string()));
    }

    #[test]
    fn default_soft_excluded_directory_segments_allowlist_is_empty() {
        assert!(default_soft_excluded_directory_segments_allowlist().is_empty());
    }

    #[test]
    fn default_subtree_hint_cooldown_ms_is_15_seconds() {
        assert_eq!(default_subtree_hint_cooldown_ms(), 15_000);
    }

    #[test]
    fn default_subtree_hint_retry_budget_is_3() {
        assert_eq!(default_subtree_hint_retry_budget(), 3);
    }

    #[test]
    fn default_use_git_global_ignore_is_false() {
        assert!(!default_use_git_global_ignore());
    }

    #[test]
    fn default_legacy_axonignore_additive_is_true() {
        assert!(default_legacy_axonignore_additive());
    }

    #[test]
    fn default_ignore_reconcile_enabled_is_true() {
        assert!(default_ignore_reconcile_enabled());
    }

    #[test]
    fn default_ignore_reconcile_dry_run_is_true() {
        assert!(default_ignore_reconcile_dry_run());
    }
}
