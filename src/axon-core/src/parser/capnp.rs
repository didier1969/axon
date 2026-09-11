use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*using\s+(?:[a-zA-Z0-9_]+\s*=\s*)?import\s+["']([^"']+)["']\s*;"#)
        .expect("valid regex")
});

static RE_BLOCK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(struct|interface|enum)\s+([a-zA-Z0-9_]+)\s*\{([^}]*)\}")
        .expect("valid regex")
});

pub struct CapnpParser;

impl Default for CapnpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CapnpParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for CapnpParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        for cap in RE_IMPORT.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                relations.push(Relation {
                    from: "schema".to_string(),
                    to: m.as_str().to_string(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        for cap in RE_BLOCK.captures_iter(content) {
            let kind_raw = cap.get(1).map_or("struct", |m| m.as_str());
            let name = cap.get(2).map_or("", |m| m.as_str());
            let body = cap.get(3).map_or("", |m| m.as_str());

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

            let sym_kind = format!("capnp_{}", kind_raw);
            symbols.push(Symbol {
                name: name.to_string(),
                kind: sym_kind,
                start_line,
                end_line,
                docstring: None,
                is_entry_point: kind_raw == "interface",
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });

            // Extract members
            for line in body.lines() {
                let trimmed = line.trim().trim_end_matches(';').trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }

                if kind_raw == "struct" {
                    // id @0 :UInt32;
                    if let Some(colon_pos) = trimmed.find(':') {
                        let left = trimmed[..colon_pos].trim();
                        let field_name = left.split('@').next().unwrap_or(left).trim();
                        let field_type = trimmed[colon_pos + 1..].trim();

                        if !field_name.is_empty() {
                            let full_field_name = format!("{}.{}", name, field_name);
                            let mut field_props = HashMap::new();
                            field_props.insert("type".to_string(), field_type.to_string());

                            if let Some(pii) = super::is_sensitive_name(field_name) {
                                field_props.insert("is_sensitive".to_string(), "true".to_string());
                                field_props.insert("pii_kind".to_string(), pii.to_string());
                            }

                            symbols.push(Symbol {
                                name: full_field_name.clone(),
                                kind: "capnp_field".to_string(),
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
                                from: name.to_string(),
                                to: full_field_name,
                                rel_type: "contains".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                } else if kind_raw == "interface" {
                    // lookup @0 (name :Text) -> (person :Person);
                    let method_name = trimmed
                        .split('@')
                        .next()
                        .unwrap_or(trimmed)
                        .split('(')
                        .next()
                        .unwrap_or("")
                        .trim();

                    if !method_name.is_empty() {
                        let full_method_name = format!("{}.{}", name, method_name);
                        symbols.push(Symbol {
                            name: full_method_name.clone(),
                            kind: "capnp_method".to_string(),
                            start_line,
                            end_line,
                            docstring: None,
                            is_entry_point: true,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: name.to_string(),
                            to: full_method_name,
                            rel_type: "contains".to_string(),
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
    fn test_capnp_schema_parsing() {
        let capnp_content = r#"
@0xdbb9ad1f14bf0b36;

using Common = import "/capnp/common.capnp";

struct UserProfile {
  id @0 :UInt64;
  username @1 :Text;
  passwordHash @2 :Text;
}

interface AccountService {
  getProfile @0 (id :UInt64) -> (profile :UserProfile);
}
"#;

        let parser = CapnpParser::new();
        let res = parser.parse(capnp_content);

        let user_struct = res
            .symbols
            .iter()
            .find(|s| s.name == "UserProfile" && s.kind == "capnp_struct")
            .unwrap();
        assert!(user_struct.is_public);

        let pwd_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserProfile.passwordHash")
            .unwrap();
        assert_eq!(
            pwd_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            pwd_field.properties.get("pii_kind").map(|s| s.as_str()),
            Some("credential")
        );

        let iface = res
            .symbols
            .iter()
            .find(|s| s.name == "AccountService" && s.kind == "capnp_interface")
            .unwrap();
        assert!(iface.is_entry_point);

        let method = res
            .symbols
            .iter()
            .find(|s| s.name == "AccountService.getProfile")
            .unwrap();
        assert_eq!(method.kind, "capnp_method");

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "/capnp/common.capnp" && r.rel_type == "imports"));
    }
}
