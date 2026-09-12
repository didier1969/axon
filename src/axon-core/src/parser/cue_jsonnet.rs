use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PACKAGE_CUE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_]+)").expect("valid regex"));

static RE_IMPORT_CUE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*import\s+(?:([a-zA-Z0-9_]+)\s+)?["']([^"']+)["']"#).expect("valid regex")
});

static RE_IMPORT_JSONNET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)\bimport(?:str|bin)?\s*\(?\s*['"]([^'"]+)['"]\s*\)?"#).expect("valid regex")
});

static RE_SCHEMA_DEF_CUE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*(#[a-zA-Z0-9_]+)\s*:\s*(\{)?").expect("valid regex"));

static RE_LOCAL_JSONNET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*local\s+([a-zA-Z0-9_]+)(?:\(([^)]*)\))?\s*=").expect("valid regex")
});

static RE_FIELD_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*([a-zA-Z0-9_#"-]+)\s*(?:::{1,3}|:\+|\+:|:)\s*([^{\n,;]+)?"#)
        .expect("valid regex")
});

static RE_PII: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(password|passwd|secret|token|api_key|apikey|private_key|auth)\s*(?:::{1,3}|:\+|\+:|:|=)\s*['"]([^'"]+)['"]"#).expect("valid regex")
});

pub struct CueJsonnetParser;

impl Default for CueJsonnetParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CueJsonnetParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for CueJsonnetParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. CUE package
        for cap in RE_PACKAGE_CUE.captures_iter(content) {
            let pkg_name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: pkg_name,
                kind: "package".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 2. CUE imports
        for cap in RE_IMPORT_CUE.captures_iter(content) {
            let alias = cap.get(1).map(|m| m.as_str().to_string());
            let path = cap.get(2).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(a) = alias {
                props.insert("alias".to_string(), a);
            }

            symbols.push(Symbol {
                name: path.clone(),
                kind: "import".to_string(),
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

            relations.push(Relation {
                from: "root".to_string(),
                to: path,
                rel_type: "IMPORTS".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Jsonnet imports
        for cap in RE_IMPORT_JSONNET.captures_iter(content) {
            let path = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols.iter().any(|s| s.name == path && s.kind == "import") {
                symbols.push(Symbol {
                    name: path.clone(),
                    kind: "import".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });

                relations.push(Relation {
                    from: "root".to_string(),
                    to: path,
                    rel_type: "IMPORTS".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // 4. CUE schema definitions (#Schema:)
        for cap in RE_SCHEMA_DEF_CUE.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name,
                kind: "schema_def".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 5. Jsonnet local functions & variables
        for cap in RE_LOCAL_JSONNET.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let params = cap.get(2).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_fn = params.is_some();
            let mut props = HashMap::new();
            if let Some(p) = params {
                props.insert("params".to_string(), p.to_string());
            }

            symbols.push(Symbol {
                name,
                kind: if is_fn {
                    "function".to_string()
                } else {
                    "variable".to_string()
                },
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

        // 6. Fields (top-level or structured fields)
        for cap in RE_FIELD_DEF.captures_iter(content) {
            let raw_key = cap.get(1).unwrap().as_str().trim_matches('"');
            if raw_key == "local" || raw_key == "import" || raw_key == "package" {
                continue;
            }

            let type_val = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            let line = start_offset_line(cap.get(0).unwrap().start());

            // Avoid duplicate with schema def
            if raw_key.starts_with('#')
                && symbols
                    .iter()
                    .any(|s| s.name == raw_key && s.kind == "schema_def")
            {
                continue;
            }

            let mut props = HashMap::new();
            if !type_val.is_empty() {
                props.insert("type_or_val".to_string(), type_val.to_string());
            }
            if let Some(kind) = super::is_sensitive_name(raw_key) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: raw_key.to_string(),
                kind: "field".to_string(),
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

        // 7. PII detection
        for cap in RE_PII.captures_iter(content) {
            let var_name = cap.get(1).unwrap().as_str();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("is_sensitive".to_string(), "true".to_string());
            props.insert("hardcoded_secret".to_string(), "true".to_string());
            if let Some(kind) = super::is_sensitive_name(var_name) {
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: var_name.to_string(),
                kind: "secret".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: true,
                properties: props,
                embedding: None,
            });
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
    fn test_cue_parser_basic_and_pii() {
        let code = r#"
        package config

        import "path/to/schemas"

        #Deployment: {
            name: string
            replicas: int & >=1
            secretKey: string
        }

        appDeployment: #Deployment & {
            name: "web-app"
            replicas: 3
            secretKey: "cue_secret_val_123"
        }
        "#;

        let parser = CueJsonnetParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "package" && s.name == "config"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "import" && s.name == "path/to/schemas"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "schema_def" && s.name == "#Deployment"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "field" && s.name == "appDeployment"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.to == "path/to/schemas" && r.rel_type == "IMPORTS"));

        let sec = result
            .symbols
            .iter()
            .find(|s| s.kind == "secret" || (s.kind == "field" && s.name == "secretKey"))
            .unwrap();
        assert_eq!(
            sec.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn test_jsonnet_parser_basic_and_pii() {
        let code = r#"
        local utils = import 'lib/utils.libsonnet';

        local renderName(prefix, id) = prefix + '-' + id;

        {
            api_key: 'jsonnet_api_secret_789',
            service: {
                name: renderName('svc', '01'),
                port: 8080,
            }
        }
        "#;

        let parser = CueJsonnetParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "import" && s.name == "lib/utils.libsonnet"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "function" && s.name == "renderName"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "variable" && s.name == "utils"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "field" && s.name == "service"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.to == "lib/utils.libsonnet" && r.rel_type == "IMPORTS"));

        let sec = result.symbols.iter().find(|s| s.name == "api_key").unwrap();
        assert_eq!(
            sec.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
    }
}
