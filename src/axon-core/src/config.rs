use serde::Deserialize;
use std::fs;
use once_cell::sync::Lazy;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub indexing: IndexingConfig,
}

#[derive(Debug, Deserialize)]
pub struct IndexingConfig {
    pub supported_extensions: Vec<String>,
}

pub static CONFIG: Lazy<Config> = Lazy::new(|| {
    load_config().unwrap_or_else(|_| Config {
        indexing: IndexingConfig {
            supported_extensions: vec![
                "py".to_string(), "ex".to_string(), "exs".to_string(), "rs".to_string(), 
                "go".to_string(), "java".to_string(), "c".to_string(), "cpp".to_string(), "h".to_string(),
                "js".to_string(), "jsx".to_string(), "ts".to_string(), "tsx".to_string(), "sql".to_string(), 
                "md".to_string(), "markdown".to_string(), "txt".to_string(), "json".to_string(), 
                "yml".to_string(), "yaml".to_string(), "toml".to_string(), "conf".to_string(), 
                "html".to_string(), "css".to_string()
            ]
        }
    })
});

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
        if !path.pop() { break; }
    }
    anyhow::bail!("Config not found")
}
