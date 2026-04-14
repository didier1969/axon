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
