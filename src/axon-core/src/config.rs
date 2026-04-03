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
    #[serde(default = "default_subtree_hint_cooldown_ms")]
    pub subtree_hint_cooldown_ms: u64,
    #[serde(default = "default_subtree_hint_retry_budget")]
    pub subtree_hint_retry_budget: u64,
}

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    load_config().unwrap_or_else(|_| Config {
        indexing: IndexingConfig {
            supported_extensions: default_supported_extensions(),
            ignored_directory_segments: default_ignored_directory_segments(),
            blocked_subtree_hint_segments: default_blocked_subtree_hint_segments(),
            subtree_hint_cooldown_ms: default_subtree_hint_cooldown_ms(),
            subtree_hint_retry_budget: default_subtree_hint_retry_budget(),
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
    vec![
        ".git".to_string(),
        ".svn".to_string(),
        ".hg".to_string(),
        ".mypy_cache".to_string(),
        ".pytest_cache".to_string(),
        ".ruff_cache".to_string(),
        "__pycache__".to_string(),
        ".venv".to_string(),
        ".fastembed_cache".to_string(),
        ".direnv".to_string(),
        ".devenv".to_string(),
        ".cache".to_string(),
        "node_modules".to_string(),
        "target".to_string(),
        "_build".to_string(),
        "deps".to_string(),
        "build".to_string(),
        "dist".to_string(),
        "vendor".to_string(),
    ]
}

fn default_blocked_subtree_hint_segments() -> Vec<String> {
    vec![
        ".git".to_string(),
        ".svn".to_string(),
        ".hg".to_string(),
        ".direnv".to_string(),
        ".devenv".to_string(),
        ".cache".to_string(),
        "node_modules".to_string(),
        "target".to_string(),
        "_build".to_string(),
        "deps".to_string(),
        "build".to_string(),
        "dist".to_string(),
        "vendor".to_string(),
        "pg_wal".to_string(),
    ]
}

fn default_subtree_hint_cooldown_ms() -> u64 {
    15_000
}

fn default_subtree_hint_retry_budget() -> u64 {
    3
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
