use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

static RE_DATASOURCE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*datasource\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}").expect("valid regex")
});

static RE_GENERATOR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*generator\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}").expect("valid regex")
});

static RE_MODEL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*model\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}").expect("valid regex")
});

static RE_ENUM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*enum\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}").expect("valid regex"));

static RE_TYPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*type\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}").expect("valid regex"));

static RE_FIELD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*([a-zA-Z0-9_]+)\s+([a-zA-Z0-9_]+)(\[\]|\?)?(.*)$").expect("valid regex")
});

static RE_PII_ATTR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(password|passwd|secret|token|api_key|apikey|private_key|auth)\s*=\s*['"]([^'"]+)['"]"#).expect("valid regex")
});

static RE_URL_CREDENTIALS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[a-zA-Z0-9+.-]+://(?:[^:@/]+):([^@/]+)@"#).expect("valid regex"));

pub struct PrismaParser;

impl Default for PrismaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl PrismaParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for PrismaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols: Vec<Symbol> = Vec::new();
        let mut relations: Vec<Relation> = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        let mut known_models = HashSet::new();

        // Pass 0: gather model names
        for cap in RE_MODEL.captures_iter(content) {
            known_models.insert(cap.get(1).unwrap().as_str().to_string());
        }

        // 1. Datasource
        for cap in RE_DATASOURCE.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let body = cap.get(2).unwrap().as_str();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            for b_line in body.lines() {
                let trimmed = b_line.trim();
                if let Some((k, v)) = trimmed.split_once('=') {
                    props.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
                }
            }

            symbols.push(Symbol {
                name,
                kind: "datasource".to_string(),
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

        // 2. Generator
        for cap in RE_GENERATOR.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let body = cap.get(2).unwrap().as_str();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            for b_line in body.lines() {
                let trimmed = b_line.trim();
                if let Some((k, v)) = trimmed.split_once('=') {
                    props.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
                }
            }

            symbols.push(Symbol {
                name,
                kind: "generator".to_string(),
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

        // 3. Enums
        for cap in RE_ENUM.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let body = cap.get(2).unwrap().as_str();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let variants: Vec<String> = body
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with("//"))
                .map(|l| l.to_string())
                .collect();

            let mut props = HashMap::new();
            props.insert("variants".to_string(), variants.join(", "));

            symbols.push(Symbol {
                name,
                kind: "enum".to_string(),
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

        // 4. Composite Types
        for cap in RE_TYPE.captures_iter(content) {
            let name = cap.get(1).unwrap().as_str().to_string();
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name,
                kind: "type".to_string(),
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

        // 5. Models and Fields
        for cap in RE_MODEL.captures_iter(content) {
            let model_name = cap.get(1).unwrap().as_str().to_string();
            let body = cap.get(2).unwrap().as_str();
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut field_names = Vec::new();

            for b_line in body.lines() {
                let trimmed = b_line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("@@") {
                    continue;
                }

                if let Some(fcap) = RE_FIELD.captures(trimmed) {
                    let field_name = fcap.get(1).unwrap().as_str();
                    let field_type = fcap.get(2).unwrap().as_str();
                    let attributes = fcap.get(4).map(|m| m.as_str().trim()).unwrap_or("");

                    field_names.push(field_name.to_string());

                    // Check if field references another model
                    if known_models.contains(field_type) && field_type != model_name {
                        if !relations
                            .iter()
                            .any(|r| r.from == model_name && r.to == field_type)
                        {
                            relations.push(Relation {
                                from: model_name.clone(),
                                to: field_type.to_string(),
                                rel_type: "REFERENCES".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }

                    // Field symbol
                    let mut field_meta = HashMap::new();
                    field_meta.insert("model".to_string(), model_name.clone());
                    field_meta.insert("type".to_string(), field_type.to_string());
                    if !attributes.is_empty() {
                        field_meta.insert("attributes".to_string(), attributes.to_string());
                    }
                    if let Some(kind) = super::is_sensitive_name(field_name) {
                        field_meta.insert("is_sensitive".to_string(), "true".to_string());
                        field_meta.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: format!("{}.{}", model_name, field_name),
                        kind: "field".to_string(),
                        start_line: line,
                        end_line: line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: field_meta,
                        embedding: None,
                    });
                }
            }

            let mut model_meta = HashMap::new();
            model_meta.insert("fields".to_string(), field_names.join(", "));

            symbols.push(Symbol {
                name: model_name,
                kind: "model".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: model_meta,
                embedding: None,
            });
        }

        // 6. PII detection
        for cap in RE_PII_ATTR.captures_iter(content) {
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

        for cap in RE_URL_CREDENTIALS.captures_iter(content) {
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("is_sensitive".to_string(), "true".to_string());
            props.insert("url_credentials".to_string(), "true".to_string());

            symbols.push(Symbol {
                name: "database_url_credentials".to_string(),
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
    fn test_prisma_parser_basic_and_pii() {
        let code = r#"
        datasource db {
          provider = "postgresql"
          url      = "postgresql://admin:secret123@localhost:5432/mydb"
        }

        generator client {
          provider = "prisma-client-js"
        }

        enum Role {
          USER
          ADMIN
        }

        model User {
          id        Int      @id @default(autoincrement())
          email     String   @unique
          name      String?
          role      Role     @default(USER)
          posts     Post[]
          password  String   @default("hardcoded_prisma_pass")
        }

        model Post {
          id        Int      @id @default(autoincrement())
          title     String
          content   String?
          author    User     @relation(fields: [authorId], references: [id])
          authorId  Int
        }
        "#;

        let parser = PrismaParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "datasource" && s.name == "db"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "generator" && s.name == "client"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "enum" && s.name == "Role"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "model" && s.name == "User"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "model" && s.name == "Post"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "field" && s.name == "User.email"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "field" && s.name == "Post.title"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "Post" && r.rel_type == "REFERENCES"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "Post" && r.to == "User" && r.rel_type == "REFERENCES"));

        let pass_field = result
            .symbols
            .iter()
            .find(|s| s.name == "User.password")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );
    }
}
