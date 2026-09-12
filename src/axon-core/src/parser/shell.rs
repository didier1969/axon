use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_FUNCTION_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:function\s+([a-zA-Z0-9_.:-]+)(?:\s*\(\s*\))?|([a-zA-Z0-9_.:-]+)\s*\(\s*\))\s*\{"#)
        .expect("valid regex")
});

static RE_SOURCE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*(?:source|\.)\s+([^#\n;]+)"#).expect("valid regex"));

static RE_EXPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*export\s+([a-zA-Z0-9_]+)(?:=(.*))?"#).expect("valid regex"));

static RE_VAR_ASSIGN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*([A-Z0-9_]{3,})\s*=\s*(.*)"#).expect("valid regex"));

static RE_TEST_FUNCTION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*(?:function\s+)?(test_[a-zA-Z0-9_]+)").expect("valid regex"));

pub struct ShellParser;

impl Default for ShellParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for ShellParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

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

        // 2. Exported variables (with PII detection)
        for cap in RE_EXPORT.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: vname.to_string(),
                kind: "shell_variable".to_string(),
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

        // 3. Sensitive assignments (UPPERCASE constants)
        for cap in RE_VAR_ASSIGN.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols.iter().any(|s| s.name == vname) {
                let mut props = HashMap::new();
                if let Some(kind) = super::is_sensitive_name(vname) {
                    props.insert("is_sensitive".to_string(), "true".to_string());
                    props.insert("pii_kind".to_string(), kind.to_string());

                    symbols.push(Symbol {
                        name: vname.to_string(),
                        kind: "shell_variable".to_string(),
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
        }

        // 4. Functions
        for cap in RE_FUNCTION_DEF.captures_iter(content) {
            let fname = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_main = fname == "main" || fname == "run";
            let is_test = fname.starts_with("test_");

            if !symbols
                .iter()
                .any(|s| s.name == fname && s.kind == "shell_function")
            {
                symbols.push(Symbol {
                    name: fname.to_string(),
                    kind: if is_test {
                        "shell_test".to_string()
                    } else {
                        "shell_function".to_string()
                    },
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_main,
                    is_public: true,
                    tested: is_test,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        // 5. Explicit test functions
        for cap in RE_TEST_FUNCTION.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols.iter().any(|s| s.name == tname) {
                symbols.push(Symbol {
                    name: tname.to_string(),
                    kind: "shell_test".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: true,
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
    fn test_shell_parser_basic_and_pii() {
        let code = r#"
#!/usr/bin/env bash
source /etc/profile.d/env.sh
. ./lib/common.sh

export DATABASE_PASSWORD="super_secret_db_pass"
export API_TOKEN="live_jwt_token"
export APP_PORT=8080

function setup_environment() {
    echo "Setting up..."
}

deploy_release() {
    echo "Deploying release..."
}

function main() {
    setup_environment
    deploy_release
}

test_database_connection() {
    echo "Testing DB..."
}
"#;

        let parser = ShellParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "/etc/profile.d/env.sh" && r.rel_type == "includes_script"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "./lib/common.sh" && r.rel_type == "includes_script"));

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "DATABASE_PASSWORD")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let tok_var = res.symbols.iter().find(|s| s.name == "API_TOKEN").unwrap();
        assert_eq!(
            tok_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let port_var = res.symbols.iter().find(|s| s.name == "APP_PORT").unwrap();
        assert!(port_var.properties.get("is_sensitive").is_none());

        let setup_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "setup_environment")
            .unwrap();
        assert_eq!(setup_fn.kind, "shell_function");

        let deploy_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "deploy_release")
            .unwrap();
        assert_eq!(deploy_fn.kind, "shell_function");

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let test_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "test_database_connection")
            .unwrap();
        assert_eq!(test_fn.kind, "shell_test");
        assert!(test_fn.tested);
    }
}
