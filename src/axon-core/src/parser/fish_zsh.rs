use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_FISH_FUNCTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?m)^\s*function\s+([a-zA-Z0-9_.:-]+)(?:\s+-[a-zA-Z0-9_-]+(?:\s+["'][^"']*["'])?)*\s*(?:#.*)?$"#,
    )
    .expect("valid regex")
});

static RE_ZSH_FUNCTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:function\s+([a-zA-Z0-9_.:-]+)(?:\s*\(\s*\))?|([a-zA-Z0-9_.:-]+)\s*\(\s*\))\s*\{"#)
        .expect("valid regex")
});

static RE_FISH_SET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*set\s+(?:-[a-zA-Z0-9_]+\s+)*([a-zA-Z0-9_]+)(?:\s+(.*))?"#)
        .expect("valid regex")
});

static RE_ZSH_EXPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:export|typeset\s+-x)\s+([a-zA-Z0-9_]+)(?:=(.*))?"#)
        .expect("valid regex")
});

static RE_SOURCE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*(?:source|\.)\s+([^#\n;]+)"#).expect("valid regex"));

static RE_AUTOLOAD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*autoload\s+(?:-[a-zA-Z0-9_]+\s+)*([a-zA-Z0-9_]+)"#).expect("valid regex")
});

pub struct FishZshParser;

impl Default for FishZshParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FishZshParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for FishZshParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols: Vec<Symbol> = Vec::new();
        let mut relations: Vec<Relation> = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Sources / Sourced scripts
        for cap in RE_SOURCE.captures_iter(content) {
            let path_raw = cap.get(1).map_or("", |m| m.as_str()).trim();
            let clean_path = path_raw.trim_matches('"').trim_matches('\'').trim();
            if !clean_path.is_empty() {
                relations.push(Relation {
                    from: "root".to_string(),
                    to: clean_path.to_string(),
                    rel_type: "includes_script".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // 2. Autoloads (Zsh)
        for cap in RE_AUTOLOAD.captures_iter(content) {
            let func_name = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "root".to_string(),
                to: func_name.to_string(),
                rel_type: "autoloads".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Fish Variables (set -gx / set)
        for cap in RE_FISH_SET.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            if !symbols.iter().any(|s| s.name == vname) {
                symbols.push(Symbol {
                    name: vname.to_string(),
                    kind: "fish_variable".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 4. Zsh Variables (export / typeset -x)
        for cap in RE_ZSH_EXPORT.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            if !symbols.iter().any(|s| s.name == vname) {
                symbols.push(Symbol {
                    name: vname.to_string(),
                    kind: "zsh_variable".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 5. Fish Functions
        for cap in RE_FISH_FUNCTION.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_test = fname.starts_with("test_");

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: if is_test {
                    "fish_test".to_string()
                } else {
                    "fish_function".to_string()
                },
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: fname == "main" || fname == "fish_greeting",
                is_public: !fname.starts_with('_'),
                tested: is_test,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 6. Zsh Functions
        for cap in RE_ZSH_FUNCTION.captures_iter(content) {
            let fname = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_test = fname.starts_with("test_");

            if !symbols.iter().any(|s| s.name == fname) {
                symbols.push(Symbol {
                    name: fname.to_string(),
                    kind: if is_test {
                        "zsh_test".to_string()
                    } else {
                        "zsh_function".to_string()
                    },
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: fname == "main",
                    is_public: !fname.starts_with('_'),
                    tested: is_test,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fish_parser_basic_and_pii() {
        let code = r#"
source /usr/share/fish/config.fish

set -gx SECRET_API_TOKEN "live_tok_9988"
set -gx DATABASE_PASSWORD "pass_123"
set -U EDITOR "nvim"

function my_prompt -d "custom prompt"
    echo "prompt> "
end

function test_prompt_rendering
    echo "testing prompt"
end
"#;

        let parser = FishZshParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "/usr/share/fish/config.fish" && r.rel_type == "includes_script"));

        let tok_var = res
            .symbols
            .iter()
            .find(|s| s.name == "SECRET_API_TOKEN")
            .unwrap();
        assert_eq!(
            tok_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "DATABASE_PASSWORD")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let ed_var = res.symbols.iter().find(|s| s.name == "EDITOR").unwrap();
        assert!(ed_var.properties.get("is_sensitive").is_none());

        let prompt_fn = res.symbols.iter().find(|s| s.name == "my_prompt").unwrap();
        assert_eq!(prompt_fn.kind, "fish_function");

        let test_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "test_prompt_rendering")
            .unwrap();
        assert_eq!(test_fn.kind, "fish_test");
        assert!(test_fn.tested);
    }

    #[test]
    fn test_zsh_parser_basic_and_pii() {
        let code = r#"
autoload -Uz compinit
source $ZSH/oh-my-zsh.sh

export ADMIN_PASSWORD="root_secret_pass"
typeset -x AUTH_TOKEN="bearer_token_xyz"

function setup_theme {
    PROMPT="%m%# "
}

main() {
    setup_theme
}
"#;

        let parser = FishZshParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "compinit" && r.rel_type == "autoloads"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "$ZSH/oh-my-zsh.sh" && r.rel_type == "includes_script"));

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "ADMIN_PASSWORD")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let tok_var = res.symbols.iter().find(|s| s.name == "AUTH_TOKEN").unwrap();
        assert_eq!(
            tok_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let theme_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "setup_theme")
            .unwrap();
        assert_eq!(theme_fn.kind, "zsh_function");

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);
    }
}
