use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*import\??\s+['"]([^'"]+)['"]"#).expect("valid regex"));

static RE_SETTING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*set\s+([a-zA-Z0-9_-]+)(?:\s*:=\s*([^\n#]+))?").expect("valid regex")
});

static RE_VAR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:export\s+)?([a-zA-Z0-9_]+)\s*:=\s*(.*)"#).expect("valid regex")
});

pub struct JustfileParser;

impl Default for JustfileParser {
    fn default() -> Self {
        Self::new()
    }
}

impl JustfileParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for JustfileParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Imports
        for cap in RE_IMPORT.captures_iter(content) {
            let imp_path = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "justfile".to_string(),
                to: imp_path.to_string(),
                rel_type: "imports_just".to_string(),
                properties: HashMap::new(),
            });
        }

        // 2. Settings
        for cap in RE_SETTING.captures_iter(content) {
            let sname = cap.get(1).map_or("", |m| m.as_str());
            let sval = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("true");
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("value".to_string(), sval.to_string());

            symbols.push(Symbol {
                name: sname.to_string(),
                kind: "just_setting".to_string(),
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

        // 3. Variables with PII detection
        for cap in RE_VAR.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: vname.to_string(),
                kind: "just_variable".to_string(),
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

        // 4. Recipes
        let mut first_recipe = true;
        for (line_idx, line_str) in content.lines().enumerate() {
            let trimmed = line_str.trim();
            if trimmed.starts_with('#')
                || trimmed.is_empty()
                || line_str.starts_with(' ')
                || line_str.starts_with('\t')
            {
                continue;
            }
            if trimmed.contains(":=")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("set ")
            {
                continue;
            }
            if let Some(col_idx) = trimmed.find(':') {
                let left = trimmed[..col_idx].trim();
                let right = trimmed[col_idx + 1..].trim();

                let rname = left.split_whitespace().next().unwrap_or("");
                if rname.is_empty()
                    || rname == "export"
                    || !rname
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    continue;
                }

                let line = line_idx + 1;
                let is_entry = first_recipe || rname == "default" || rname == "all";
                let is_test =
                    rname == "test" || rname.starts_with("test-") || rname.starts_with("check");
                first_recipe = false;

                symbols.push(Symbol {
                    name: rname.to_string(),
                    kind: if is_test {
                        "just_test".to_string()
                    } else {
                        "just_recipe".to_string()
                    },
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_entry,
                    is_public: !rname.starts_with('_'),
                    tested: is_test,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });

                // Dependencies
                let deps_part = right.split('#').next().unwrap_or("").trim();
                for dep in deps_part.split_whitespace() {
                    let clean_dep = dep.trim();
                    if !clean_dep.is_empty()
                        && !clean_dep.starts_with('(')
                        && !clean_dep.starts_with('$')
                        && !clean_dep.starts_with("&&")
                    {
                        relations.push(Relation {
                            from: rname.to_string(),
                            to: clean_dep.to_string(),
                            rel_type: "depends_on".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
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
    fn test_justfile_parser_basic_and_pii() {
        let code = r#"
import 'ci.just'

set dotenv-load := true
set shell := ["bash", "-uc"]

export SECRET_API_KEY := "prod_secret_9988"
database_password := "pg_pass_123"
app_port := "8080"

default: build test

build target="release":
    cargo build --{{ target }}

test: build
    cargo test

_internal_cleanup:
    rm -rf target/tmp
"#;

        let parser = JustfileParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "ci.just" && r.rel_type == "imports_just"));

        let set_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "dotenv-load")
            .unwrap();
        assert_eq!(set_sym.kind, "just_setting");

        let key_var = res
            .symbols
            .iter()
            .find(|s| s.name == "SECRET_API_KEY")
            .unwrap();
        assert_eq!(
            key_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "database_password")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let port_var = res.symbols.iter().find(|s| s.name == "app_port").unwrap();
        assert!(port_var.properties.get("is_sensitive").is_none());

        let def_recipe = res.symbols.iter().find(|s| s.name == "default").unwrap();
        assert_eq!(def_recipe.kind, "just_recipe");
        assert!(def_recipe.is_entry_point);

        let build_recipe = res.symbols.iter().find(|s| s.name == "build").unwrap();
        assert_eq!(build_recipe.kind, "just_recipe");

        let test_recipe = res.symbols.iter().find(|s| s.name == "test").unwrap();
        assert_eq!(test_recipe.kind, "just_test");
        assert!(test_recipe.tested);

        let priv_recipe = res
            .symbols
            .iter()
            .find(|s| s.name == "_internal_cleanup")
            .unwrap();
        assert!(!priv_recipe.is_public);

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "default" && r.to == "build" && r.rel_type == "depends_on"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "default" && r.to == "test" && r.rel_type == "depends_on"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "test" && r.to == "build" && r.rel_type == "depends_on"));
    }
}
