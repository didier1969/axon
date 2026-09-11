use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_NAMESPACE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*namespace\s+([a-zA-Z0-9_.]+)\s*;").expect("valid regex"));

static RE_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*include\s+["']([^"']+)["']\s*;"#).expect("valid regex"));

static RE_ROOT_TYPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*root_type\s+([a-zA-Z0-9_]+)\s*;").expect("valid regex"));

static RE_BLOCK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\s*(table|struct|enum)\s+([a-zA-Z0-9_]+)(?:\s*:\s*[a-zA-Z0-9_]+)?\s*\{([^}]*)\}",
    )
    .expect("valid regex")
});

pub struct FbsParser;

impl Default for FbsParser {
    fn default() -> Self {
        Self::new()
    }
}

impl FbsParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for FbsParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let namespace = if let Some(cap) = RE_NAMESPACE.captures(content) {
            cap.get(1).map_or("", |m| m.as_str()).to_string()
        } else {
            String::new()
        };

        if !namespace.is_empty() {
            symbols.push(Symbol {
                name: namespace.clone(),
                kind: "fbs_namespace".to_string(),
                start_line: 1,
                end_line: 1,
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

        for cap in RE_INCLUDE.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                relations.push(Relation {
                    from: if namespace.is_empty() {
                        "root".to_string()
                    } else {
                        namespace.clone()
                    },
                    to: m.as_str().to_string(),
                    rel_type: "includes".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        let root_type = RE_ROOT_TYPE
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str());

        for cap in RE_BLOCK.captures_iter(content) {
            let kind_raw = cap.get(1).map_or("table", |m| m.as_str());
            let name_raw = cap.get(2).map_or("", |m| m.as_str());
            let body = cap.get(3).map_or("", |m| m.as_str());

            let full_name = if namespace.is_empty() {
                name_raw.to_string()
            } else {
                format!("{}.{}", namespace, name_raw)
            };

            let is_root = root_type == Some(name_raw);
            let mut props = HashMap::new();
            if is_root {
                props.insert("is_root_type".to_string(), "true".to_string());
            }

            let start_line = content[..cap.get(0).unwrap().start()]
                .chars()
                .filter(|&c| c == '\n')
                .count()
                + 1;
            let end_line = content[..cap.get(0).unwrap().end()]
                .chars()
                .filter(|&c| c == '\n')
                .count()
                + 1;

            let sym_kind = format!("fbs_{}", kind_raw);
            symbols.push(Symbol {
                name: full_name.clone(),
                kind: sym_kind,
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_root,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });

            if !namespace.is_empty() {
                relations.push(Relation {
                    from: namespace.clone(),
                    to: full_name.clone(),
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }

            // Extract fields
            for line in body.lines() {
                let trimmed = line.trim().trim_end_matches(';').trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }

                if let Some(colon_pos) = trimmed.find(':') {
                    let field_name = trimmed[..colon_pos].trim();
                    let field_type_part = trimmed[colon_pos + 1..].trim();
                    let field_type = field_type_part
                        .split('=')
                        .next()
                        .unwrap_or(field_type_part)
                        .trim();

                    let full_field_name = format!("{}.{}", full_name, field_name);
                    let mut field_props = HashMap::new();
                    field_props.insert("type".to_string(), field_type.to_string());

                    if let Some(kind) = super::is_sensitive_name(field_name) {
                        field_props.insert("is_sensitive".to_string(), "true".to_string());
                        field_props.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "fbs_field".to_string(),
                        start_line,
                        end_line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: field_props,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: full_name.clone(),
                        to: full_field_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
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
    fn test_flatbuffers_schema_parsing() {
        let fbs = r#"
namespace Game.Sample;

include "common.fbs";

enum Color : byte { Red = 0, Green, Blue = 2 }

struct Vec3 {
  x: float;
  y: float;
  z: float;
}

table Monster {
  pos: Vec3;
  hp: short = 100;
  name: string;
  auth_token: string;
}

root_type Monster;
"#;

        let parser = FbsParser::new();
        let res = parser.parse(fbs);

        let ns = res
            .symbols
            .iter()
            .find(|s| s.kind == "fbs_namespace")
            .unwrap();
        assert_eq!(ns.name, "Game.Sample");

        let monster = res
            .symbols
            .iter()
            .find(|s| s.name == "Game.Sample.Monster")
            .unwrap();
        assert_eq!(monster.kind, "fbs_table");
        assert_eq!(
            monster.properties.get("is_root_type").map(|s| s.as_str()),
            Some("true")
        );
        assert!(monster.is_entry_point);

        let token_field = res
            .symbols
            .iter()
            .find(|s| s.name == "Game.Sample.Monster.auth_token")
            .unwrap();
        assert_eq!(
            token_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            token_field.properties.get("pii_kind").map(|s| s.as_str()),
            Some("secret")
        );

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "Game.Sample" && r.to == "common.fbs" && r.rel_type == "includes"));
    }
}
