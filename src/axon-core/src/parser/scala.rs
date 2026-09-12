use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PACKAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_CASE_CLASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ms)^\s*case\s+class\s+([A-Za-z0-9_]+)\s*\((.*?)\)(?:\s+extends\s+([A-Za-z0-9_.]+))?",
    )
    .expect("valid regex")
});

static RE_CLASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:abstract\s+)?class\s+([A-Za-z0-9_]+)(?:\[[^\]]+\])?(?:\s*\([^)]*\))?(?:\s+extends\s+([A-Za-z0-9_.]+))?")
        .expect("valid regex")
});

static RE_TRAIT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*trait\s+([A-Za-z0-9_]+)(?:\[[^\]]+\])?(?:\s+extends\s+([A-Za-z0-9_.]+))?")
        .expect("valid regex")
});

static RE_OBJECT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*object\s+([A-Za-z0-9_]+)(?:\s+extends\s+([A-Za-z0-9_.]+))?")
        .expect("valid regex")
});

static RE_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:override\s+)?(?:protected\s+|private\s+)?def\s+([a-zA-Z0-9_!+<>=*/-]+)\s*(?:\[[^\]]+\])?\s*\(([^)]*)\)")
        .expect("valid regex")
});

static RE_SPEC_TEST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?ms)["']([^"']+)["']\s+(?:in|should|must)\s*\{"#).expect("valid regex")
});

static RE_CALL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b([A-Z][a-zA-Z0-9_]*)\.([a-z][a-zA-Z0-9_]*)\s*\(").expect("valid regex")
});

pub struct ScalaParser;

impl Default for ScalaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalaParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for ScalaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Package
        let package_name = if let Some(cap) = RE_PACKAGE.captures(content) {
            cap.get(1).map_or("", |m| m.as_str()).to_string()
        } else {
            String::new()
        };

        // 2. Imports
        for cap in RE_IMPORT.captures_iter(content) {
            let imp = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: if package_name.is_empty() {
                    "root".to_string()
                } else {
                    package_name.clone()
                },
                to: imp.to_string(),
                rel_type: "imports_scala".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Case Classes (with PII fields extraction)
        for cap in RE_CASE_CLASS.captures_iter(content) {
            let cname = cap.get(1).map_or("", |m| m.as_str());
            let params = cap.get(2).map_or("", |m| m.as_str());
            let parent = cap.get(3).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut class_props = HashMap::new();
            if let Some(p) = parent {
                class_props.insert("extends".to_string(), p.to_string());
                relations.push(Relation {
                    from: cname.to_string(),
                    to: p.to_string(),
                    rel_type: "extends".to_string(),
                    properties: HashMap::new(),
                });
            }

            let mut has_sensitive = false;
            for part in params.split(',') {
                let trimmed = part.trim();
                let clean = trimmed
                    .strip_prefix("val ")
                    .or_else(|| trimmed.strip_prefix("var "))
                    .unwrap_or(trimmed)
                    .trim();
                if let Some(col) = clean.find(':') {
                    let fname = clean[..col].trim();
                    let ftype = clean[col + 1..].trim();

                    if !fname.is_empty() {
                        let full_field_name = format!("{}.{}", cname, fname);
                        let mut fprops = HashMap::new();
                        fprops.insert("type".to_string(), ftype.to_string());

                        if let Some(kind) = super::is_sensitive_name(fname) {
                            has_sensitive = true;
                            fprops.insert("is_sensitive".to_string(), "true".to_string());
                            fprops.insert("pii_kind".to_string(), kind.to_string());
                        }

                        symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "scala_field".to_string(),
                            start_line: line,
                            end_line: line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: fprops,
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: cname.to_string(),
                            to: full_field_name,
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            if has_sensitive {
                class_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: cname.to_string(),
                kind: "scala_case_class".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: class_props,
                embedding: None,
            });
        }

        // 4. Standard Classes
        for cap in RE_CLASS.captures_iter(content) {
            let cname = cap.get(1).map_or("", |m| m.as_str());
            let parent = cap.get(2).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols.iter().any(|s| s.name == cname) {
                let mut props = HashMap::new();
                if let Some(p) = parent {
                    props.insert("extends".to_string(), p.to_string());
                    relations.push(Relation {
                        from: cname.to_string(),
                        to: p.to_string(),
                        rel_type: "extends".to_string(),
                        properties: HashMap::new(),
                    });
                }

                symbols.push(Symbol {
                    name: cname.to_string(),
                    kind: "scala_class".to_string(),
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

        // 5. Traits
        for cap in RE_TRAIT.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let parent = cap.get(2).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(p) = parent {
                props.insert("extends".to_string(), p.to_string());
                relations.push(Relation {
                    from: tname.to_string(),
                    to: p.to_string(),
                    rel_type: "extends".to_string(),
                    properties: HashMap::new(),
                });
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "scala_trait".to_string(),
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

        // 6. Objects (Singletons / Entrypoints)
        for cap in RE_OBJECT.captures_iter(content) {
            let oname = cap.get(1).map_or("", |m| m.as_str());
            let parent = cap.get(2).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            let mut is_entry = false;
            if let Some(p) = parent {
                props.insert("extends".to_string(), p.to_string());
                if p == "App" {
                    is_entry = true;
                }
                relations.push(Relation {
                    from: oname.to_string(),
                    to: p.to_string(),
                    rel_type: "extends".to_string(),
                    properties: HashMap::new(),
                });
            }

            symbols.push(Symbol {
                name: oname.to_string(),
                kind: "scala_object".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_entry,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
        }

        // 7. Methods (def)
        for cap in RE_DEF.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            let is_main = fname == "main";

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "scala_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_main,
                is_public: !fname.starts_with('_'),
                tested: false,
                is_nif: false,
                is_unsafe: fname.starts_with("unsafe"),
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 8. ScalaTest / Specs2 tests
        for cap in RE_SPEC_TEST.captures_iter(content) {
            let tname = cap.get(1).map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "scala_test".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: true,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 9. Qualified calls
        for cap in RE_CALL.captures_iter(content) {
            let target_obj = cap.get(1).map_or("", |m| m.as_str());
            let target_fn = cap.get(2).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "root".to_string(),
                to: format!("{}.{}", target_obj, target_fn),
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
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
    fn test_scala_parser_basic_and_pii() {
        let code = r#"
package com.nexus.auth

import cats.effect.IO
import org.http4s.circe._

trait AuthService[F[_]] {
    def authenticate(token: String): F[Boolean]
}

case class AuthSession(
    sessionId: Long,
    userEmail: String,
    passwordHash: String,
    secretToken: String
) extends Serializable

class DatabaseAuthService extends AuthService[IO] {
    override def authenticate(token: String): IO[Boolean] = {
        IO.pure(true)
    }
}

object AuthServer extends App {
    println("AuthServer starting...")
}

class AuthSpec {
    "User authentication" should {
        // test assertions
    }
}
"#;

        let parser = ScalaParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "cats.effect.IO" && r.rel_type == "imports_scala"));

        let trait_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "AuthService")
            .unwrap();
        assert_eq!(trait_sym.kind, "scala_trait");

        let case_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "AuthSession")
            .unwrap();
        assert_eq!(
            case_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            case_sym.properties.get("extends").map(|s| s.as_str()),
            Some("Serializable")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "AuthSession.passwordHash")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "AuthSession.secretToken")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let class_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "DatabaseAuthService")
            .unwrap();
        assert_eq!(class_sym.kind, "scala_class");
        assert_eq!(
            class_sym.properties.get("extends").map(|s| s.as_str()),
            Some("AuthService")
        );

        let obj_sym = res.symbols.iter().find(|s| s.name == "AuthServer").unwrap();
        assert_eq!(obj_sym.kind, "scala_object");
        assert!(obj_sym.is_entry_point);

        let test_sym = res.symbols.iter().find(|s| s.kind == "scala_test").unwrap();
        assert_eq!(test_sym.name, "User authentication");
        assert!(test_sym.tested);
    }
}
