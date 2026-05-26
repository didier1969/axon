//! Generic phantom symbol extraction engine (REQ-AXO-901770).
//!
//! Extracts "implicit identifiers" (env vars, ports, routes, config keys,
//! metrics) from string literals using declarative pattern rules. The rules
//! are TOML files loaded at startup -- adding a language is configuration,
//! not code. Phantom symbols are stored in public.Symbol with dedicated
//! `kind` values and connected via READS/DECLARES/EXPOSES edges.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

use super::{ExtractionResult, Relation, Symbol};

#[derive(Debug, Clone, Deserialize)]
pub struct PhantomRuleFile {
    pub meta: RuleMeta,
    #[serde(default)]
    pub rules: Vec<PhantomRuleSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleMeta {
    pub language: String,
    #[serde(default)]
    pub file_extensions: Vec<String>,
    #[serde(default)]
    pub file_patterns: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhantomRuleSpec {
    pub id: String,
    pub pattern: String,
    pub phantom_kind: String,
    pub edge_type: String,
    #[serde(default = "default_min_length")]
    pub min_length: usize,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

fn default_min_length() -> usize {
    3
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub id: String,
    pub regex: Regex,
    pub phantom_kind: String,
    pub edge_type: String,
    pub min_length: usize,
    pub exclude_regexes: Vec<Regex>,
}

#[derive(Debug, Default)]
pub struct PhantomRuleEngine {
    rules_by_extension: HashMap<String, Vec<CompiledRule>>,
    rules_by_pattern: Vec<(Regex, Vec<CompiledRule>)>,
}

static GLOBAL_ENGINE: Lazy<RwLock<PhantomRuleEngine>> =
    Lazy::new(|| RwLock::new(PhantomRuleEngine::default()));

impl PhantomRuleEngine {
    pub fn load_rules_dir(dir: &Path) -> Result<Self, String> {
        let mut engine = PhantomRuleEngine::default();

        if !dir.is_dir() {
            return Ok(engine);
        }

        for entry in std::fs::read_dir(dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "toml") {
                continue;
            }
            let content =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let rule_file: PhantomRuleFile =
                toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))?;

            let compiled: Vec<CompiledRule> = rule_file
                .rules
                .into_iter()
                .filter_map(|spec| {
                    let regex = Regex::new(&spec.pattern).ok()?;
                    let exclude_regexes = spec
                        .exclude_patterns
                        .iter()
                        .filter_map(|p| Regex::new(p).ok())
                        .collect();
                    Some(CompiledRule {
                        id: spec.id,
                        regex,
                        phantom_kind: spec.phantom_kind,
                        edge_type: spec.edge_type,
                        min_length: spec.min_length,
                        exclude_regexes,
                    })
                })
                .collect();

            for ext in &rule_file.meta.file_extensions {
                engine
                    .rules_by_extension
                    .entry(ext.to_lowercase())
                    .or_default()
                    .extend(compiled.iter().cloned());
            }

            for pattern in &rule_file.meta.file_patterns {
                if let Ok(re) = Regex::new(pattern) {
                    engine
                        .rules_by_pattern
                        .push((re, compiled.clone()));
                }
            }
        }

        Ok(engine)
    }

    pub fn rules_for_file(&self, path: &Path) -> Vec<&CompiledRule> {
        let mut result = Vec::new();

        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(rules) = self.rules_by_extension.get(&ext.to_lowercase()) {
                result.extend(rules.iter());
            }
        }

        let path_str = path.to_string_lossy();
        for (pattern, rules) in &self.rules_by_pattern {
            if pattern.is_match(&path_str) {
                result.extend(rules.iter());
            }
        }

        result
    }
}

pub fn init_global_engine(rules_dir: &Path) {
    match PhantomRuleEngine::load_rules_dir(rules_dir) {
        Ok(engine) => {
            let count: usize = engine
                .rules_by_extension
                .values()
                .map(|v| v.len())
                .sum();
            tracing::info!(
                rules_count = count,
                "Phantom symbol engine loaded from {}",
                rules_dir.display()
            );
            *GLOBAL_ENGINE.write().unwrap() = engine;
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to load phantom rules, engine disabled");
        }
    }
}

/// Extract phantom symbols from file content using the global rule engine.
/// Returns additional symbols and relations to merge into the parse result.
pub fn phantom_extract(
    path: &Path,
    content: &str,
    project_code: Option<&str>,
) -> (Vec<Symbol>, Vec<Relation>) {
    let engine = GLOBAL_ENGINE.read().unwrap();
    let rules = engine.rules_for_file(path);

    if rules.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let file_id = format!(
        "{}::{}",
        project_code.unwrap_or("_"),
        path.display()
    );

    let mut symbols = Vec::new();
    let mut relations = Vec::new();
    let mut seen: HashMap<(String, String), bool> = HashMap::new();

    for rule in &rules {
        for captures in rule.regex.captures_iter(content) {
            let captured = captures
                .get(1)
                .or_else(|| captures.get(0))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            if captured.len() < rule.min_length {
                continue;
            }

            if rule
                .exclude_regexes
                .iter()
                .any(|ex| ex.is_match(&captured))
            {
                continue;
            }

            let phantom_id = format!(
                "{}::phantom::{}::{}",
                project_code.unwrap_or("_"),
                rule.phantom_kind,
                captured
            );

            let dedup_key = (phantom_id.clone(), rule.edge_type.clone());
            if seen.contains_key(&dedup_key) {
                continue;
            }
            seen.insert(dedup_key, true);

            let line = content[..captures.get(0).unwrap().start()]
                .lines()
                .count()
                + 1;

            symbols.push(Symbol {
                name: captured.clone(),
                kind: rule.phantom_kind.clone(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: {
                    let mut p = HashMap::new();
                    p.insert("phantom".to_string(), "true".to_string());
                    p.insert("rule_id".to_string(), rule.id.clone());
                    p
                },
                embedding: None,
            });

            relations.push(Relation {
                from: file_id.clone(),
                to: phantom_id,
                rel_type: rule.edge_type.to_lowercase(),
                properties: HashMap::new(),
            });
        }
    }

    (symbols, relations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_rule(dir: &Path, name: &str, content: &str) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn loads_rules_and_extracts_env_var_from_rust() {
        let dir = TempDir::new().unwrap();
        write_rule(
            dir.path(),
            "rust.toml",
            r#"
[meta]
language = "rust"
file_extensions = ["rs"]

[[rules]]
id = "env_var_std"
pattern = 'env::var(?:_os)?\("([A-Z_][A-Z0-9_]*)"\)'
phantom_kind = "env_var"
edge_type = "READS"
"#,
        );

        let engine = PhantomRuleEngine::load_rules_dir(dir.path()).unwrap();
        assert!(!engine.rules_by_extension.is_empty());

        let content = r#"
fn main() {
    let port = std::env::var("AXON_BRAIN_PORT").unwrap();
    let db = env::var("DATABASE_URL").unwrap();
}
"#;
        *GLOBAL_ENGINE.write().unwrap() = engine;
        let (symbols, relations) =
            phantom_extract(Path::new("src/main.rs"), content, Some("AXO"));

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "AXON_BRAIN_PORT");
        assert_eq!(symbols[0].kind, "env_var");
        assert_eq!(symbols[1].name, "DATABASE_URL");

        assert_eq!(relations.len(), 2);
        assert_eq!(relations[0].rel_type, "reads");
        assert!(relations[0].to.contains("phantom::env_var::AXON_BRAIN_PORT"));
    }

    #[test]
    fn extracts_env_var_from_shell() {
        let dir = TempDir::new().unwrap();
        write_rule(
            dir.path(),
            "shell.toml",
            r#"
[meta]
language = "bash"
file_extensions = ["sh"]

[[rules]]
id = "env_var_export"
pattern = 'export\s+([A-Z_][A-Z0-9_]*)='
phantom_kind = "env_var"
edge_type = "DECLARES"

[[rules]]
id = "env_var_ref"
pattern = '\$\{?([A-Z_][A-Z0-9_]{2,})\}?'
phantom_kind = "env_var"
edge_type = "READS"
exclude_patterns = ["^(HOME|USER|PATH|PWD|SHELL|TERM|LANG|LC_)$"]
"#,
        );

        let engine = PhantomRuleEngine::load_rules_dir(dir.path()).unwrap();
        *GLOBAL_ENGINE.write().unwrap() = engine;

        let content = r#"
export AXON_BRAIN_PORT="44129"
echo "Port is $AXON_BRAIN_PORT"
nc -z localhost ${AXON_BRAIN_PORT}
"#;
        let (symbols, relations) =
            phantom_extract(Path::new("scripts/start.sh"), content, Some("AXO"));

        let declares: Vec<_> = relations.iter().filter(|r| r.rel_type == "declares").collect();
        let reads: Vec<_> = relations.iter().filter(|r| r.rel_type == "reads").collect();

        assert!(!declares.is_empty(), "should find DECLARES from export");
        assert!(!reads.is_empty(), "should find READS from $VAR reference");
    }

    #[test]
    fn deduplicates_same_phantom_in_one_file() {
        let dir = TempDir::new().unwrap();
        write_rule(
            dir.path(),
            "rust.toml",
            r#"
[meta]
language = "rust"
file_extensions = ["rs"]

[[rules]]
id = "env_var"
pattern = 'env::var\("([A-Z_][A-Z0-9_]*)"\)'
phantom_kind = "env_var"
edge_type = "READS"
"#,
        );

        let engine = PhantomRuleEngine::load_rules_dir(dir.path()).unwrap();
        *GLOBAL_ENGINE.write().unwrap() = engine;

        let content = r#"
let a = env::var("DATABASE_URL").ok();
let b = env::var("DATABASE_URL").ok();
let c = env::var("DATABASE_URL").ok();
"#;
        let (symbols, relations) =
            phantom_extract(Path::new("src/lib.rs"), content, Some("AXO"));

        assert_eq!(relations.len(), 1, "same phantom + same edge type = dedup");
    }

    #[test]
    fn file_pattern_matches_nix_files() {
        let dir = TempDir::new().unwrap();
        write_rule(
            dir.path(),
            "nix.toml",
            r#"
[meta]
language = "nix"
file_extensions = ["nix"]
file_patterns = ["devenv\\.nix$"]

[[rules]]
id = "nix_env_assign"
pattern = '([A-Z_][A-Z0-9_]{2,})\s*='
phantom_kind = "env_var"
edge_type = "DECLARES"
exclude_patterns = ["^(EOF|OK|NO|YES)$"]
min_length = 4
"#,
        );

        let engine = PhantomRuleEngine::load_rules_dir(dir.path()).unwrap();
        *GLOBAL_ENGINE.write().unwrap() = engine;

        let content = r#"
    AXON_BRAIN_PORT = 44129;
    PHX_PORT = 44127;
"#;
        let (symbols, _) =
            phantom_extract(Path::new("devenv.nix"), content, Some("AXO"));

        let names: Vec<_> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"AXON_BRAIN_PORT"));
        assert!(names.contains(&"PHX_PORT"));
    }
}
