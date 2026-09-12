use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_MODULE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*module\s+([a-zA-Z0-9_:]+)\s*\{").expect("valid regex"));

static RE_TYPE_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:(abstract)\s+)?type\s+([a-zA-Z0-9_]+)(?:\s+extending\s+([^{]+))?\s*\{")
        .expect("valid regex")
});

static RE_SCALAR_DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*scalar\s+type\s+([a-zA-Z0-9_]+)(?:\s+extending\s+([^;{]+))?")
        .expect("valid regex")
});

static RE_PROPERTY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:(required|optional)\s+)?property\s+([a-zA-Z0-9_]+)\s*->\s*([^;{\n]+)")
        .expect("valid regex")
});

static RE_LINK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:(required|optional|multi|single)\s+)*link\s+([a-zA-Z0-9_]+)\s*->\s*([a-zA-Z0-9_:]+)").expect("valid regex")
});

static RE_CONSTRAINT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*constraint\s+([a-zA-Z0-9_]+)(?:\s+on\s*\(([^)]+)\))?")
        .expect("valid regex")
});

static RE_INDEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*index\s+on\s*\(([^)]+)\)").expect("valid regex"));

static RE_QUERY_CLAUSE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(SELECT|INSERT|UPDATE|DELETE|FOR|FILTER|ORDER\s+BY|OFFSET|LIMIT|WITH)\b")
        .expect("valid regex")
});

static RE_PII: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)([a-zA-Z0-9_]*(?:password|passwd|secret|token|api_key|apikey|private_key|auth)[a-zA-Z0-9_]*)\s*(?::=|=)\s*['"]([^'"]+)['"]"#).expect("valid regex")
});

pub struct EdgeqlParser;

impl Default for EdgeqlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeqlParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for EdgeqlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Modules
        for cap in RE_MODULE.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name,
                kind: "module".to_string(),
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

        // 2. Types (and inheritance)
        for cap in RE_TYPE_DECL.captures_iter(content) {
            let is_abstract = cap.get(1).is_some();
            let type_name = cap.get(2).unwrap().as_str().to_string();
            let extends_opt = cap.get(3).map(|m| m.as_str().trim());

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if is_abstract {
                props.insert("abstract".to_string(), "true".to_string());
            }

            if let Some(extends_str) = extends_opt {
                props.insert("extends".to_string(), extends_str.to_string());
                for base in extends_str
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    relations.push(Relation {
                        from: type_name.clone(),
                        to: base.to_string(),
                        rel_type: "INHERITS".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            symbols.push(Symbol {
                name: type_name,
                kind: if is_abstract {
                    "abstract_type".to_string()
                } else {
                    "type".to_string()
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

        // 3. Scalar types
        for cap in RE_SCALAR_DECL.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(ext) = cap.get(2) {
                let ext_str = ext.as_str().trim();
                props.insert("extends".to_string(), ext_str.to_string());
                relations.push(Relation {
                    from: name.clone(),
                    to: ext_str.to_string(),
                    rel_type: "INHERITS".to_string(),
                    properties: HashMap::new(),
                });
            }

            symbols.push(Symbol {
                name,
                kind: "scalar_type".to_string(),
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

        // 4. Properties
        for cap in RE_PROPERTY.captures_iter(content) {
            let modifier = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "optional".to_string());
            let name = cap.get(2).unwrap().as_str().to_string();
            let prop_type = cap.get(3).unwrap().as_str().trim().to_string();

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("modifier".to_string(), modifier);
            props.insert("type".to_string(), prop_type);
            if let Some(kind) = super::is_sensitive_name(&name) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name,
                kind: "property".to_string(),
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

        // 5. Links
        for cap in RE_LINK.captures_iter(content) {
            let link_name = cap.get(2).unwrap().as_str().to_string();
            let target_type = cap.get(3).unwrap().as_str().to_string();

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("target".to_string(), target_type.clone());

            symbols.push(Symbol {
                name: link_name.clone(),
                kind: "link".to_string(),
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
                from: link_name,
                to: target_type,
                rel_type: "REFERENCES".to_string(),
                properties: HashMap::new(),
            });
        }

        // 6. Constraints & Indexes
        for cap in RE_CONSTRAINT.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(on_expr) = cap.get(2) {
                props.insert("on".to_string(), on_expr.as_str().trim().to_string());
            }

            symbols.push(Symbol {
                name,
                kind: "constraint".to_string(),
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

        for cap in RE_INDEX.captures_iter(content) {
            let on_expr = cap.get(1).unwrap().as_str().trim().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("on".to_string(), on_expr);

            symbols.push(Symbol {
                name: "index".to_string(),
                kind: "index".to_string(),
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

        // 7. Query Clauses
        for cap in RE_QUERY_CLAUSE.captures_iter(content) {
            let clause = cap.get(1).unwrap().as_str().to_uppercase();
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols
                .iter()
                .any(|s| s.name == clause && s.start_line == line)
            {
                symbols.push(Symbol {
                    name: clause,
                    kind: "clause".to_string(),
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
        }

        // 8. PII detection
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
    fn test_edgeql_parser_basic_and_pii() {
        let code = r#"
        module default {
          abstract type Auditable {
            property created_at -> datetime;
          }

          type User extending Auditable {
            required property name -> str;
            required property email -> str;
            multi link posts -> Post;
            constraint exclusive on (.email);
          }

          type Post extending Auditable {
            required property title -> str;
            link author -> User;
          }
        }

        select User {
          name,
          email
        } filter .name = 'Alice';

        with secret_pass := 'gel_secret_token_123'
        select User filter .name = secret_pass;
        "#;

        let parser = EdgeqlParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "module" && s.name == "default"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "abstract_type" && s.name == "Auditable"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "type" && s.name == "User"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "property" && s.name == "email"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "link" && s.name == "posts"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "constraint" && s.name == "exclusive"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "clause" && s.name == "SELECT"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "Auditable" && r.rel_type == "INHERITS"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "posts" && r.to == "Post" && r.rel_type == "REFERENCES"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "author" && r.to == "User" && r.rel_type == "REFERENCES"));

        let sec = result
            .symbols
            .iter()
            .find(|s| s.name == "secret_pass")
            .unwrap();
        assert_eq!(
            sec.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
    }
}
