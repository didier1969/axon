use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PACKAGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_.]+)").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*import\s+(?:static\s+)?([a-zA-Z0-9_.*]+)").expect("valid regex")
});

static RE_CLASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*(?:public\s+|private\s+|protected\s+)?(?:abstract\s+)?class\s+([A-Za-z0-9_]+)(?:\s+extends\s+([A-Za-z0-9_.]+))?(?:\s+implements\s+([A-Za-z0-9_.,\s]+))?\s*\{(.*?)\}")
        .expect("valid regex")
});

static RE_METHOD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:public\s+|private\s+|protected\s+)?(?:static\s+)?(?:def|void|[A-Za-z0-9_<>]+(?:\[\])?)\s+(?:["']([^"']+)["']|([a-zA-Z0-9_]+))\s*\(([^)]*)\)\s*\{"#)
        .expect("valid regex")
});

static RE_GRADLE_PLUGIN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\bid\s*\(?['"]([^'"]+)['"]\s*\)?"#).expect("valid regex"));

static RE_GRADLE_DEPENDENCY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"\b(?:implementation|api|testImplementation|compileOnly|runtimeOnly)\s*\(?['"]([^'"]+)['"]\s*\)?"#)
        .expect("valid regex")
});

static RE_GRADLE_TASK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:task\s+([a-zA-Z0-9_]+)|tasks\.register\s*\(\s*['"]([^'"]+)['"])"#)
        .expect("valid regex")
});

pub struct GroovyParser;

impl Default for GroovyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GroovyParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for GroovyParser {
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

        if !package_name.is_empty() {
            symbols.push(Symbol {
                name: package_name.clone(),
                kind: "groovy_package".to_string(),
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
                rel_type: "imports_groovy".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Classes with PII fields extraction
        for cap in RE_CLASS.captures_iter(content) {
            let cname = cap.get(1).map_or("", |m| m.as_str());
            let parent = cap.get(2).map(|m| m.as_str().trim());
            let interfaces = cap.get(3).map(|m| m.as_str().trim());
            let body = cap.get(4).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut class_props = HashMap::new();
            let is_spec = parent.map_or(false, |p| p.contains("Specification"))
                || cname.ends_with("Spec")
                || cname.ends_with("Test");

            if let Some(p) = parent {
                class_props.insert("extends".to_string(), p.to_string());
                relations.push(Relation {
                    from: cname.to_string(),
                    to: p.to_string(),
                    rel_type: "extends".to_string(),
                    properties: HashMap::new(),
                });
            }

            if let Some(ifaces) = interfaces {
                for iface in ifaces.split(',') {
                    let clean = iface.trim();
                    if !clean.is_empty() {
                        relations.push(Relation {
                            from: cname.to_string(),
                            to: clean.to_string(),
                            rel_type: "implements".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            let mut has_sensitive = false;
            for fline in body.lines() {
                let trimmed = fline.trim().trim_end_matches(';').trim();
                if trimmed.is_empty()
                    || trimmed.starts_with("//")
                    || trimmed.starts_with("def ")
                    || trimmed.contains('(')
                {
                    continue;
                }

                // field can be: String passwordHash or def passwordHash
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                let fname = if parts.len() >= 2 {
                    parts[1].trim()
                } else if parts.len() == 1 {
                    parts[0].trim()
                } else {
                    ""
                };

                let clean_name = fname.trim_matches(';').trim();
                if !clean_name.is_empty()
                    && clean_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let full_field_name = format!("{}.{}", cname, clean_name);
                    let mut fprops = HashMap::new();

                    if let Some(kind) = super::is_sensitive_name(clean_name) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "groovy_field".to_string(),
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

            if has_sensitive {
                class_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: cname.to_string(),
                kind: if is_spec {
                    "groovy_test".to_string()
                } else {
                    "groovy_class".to_string()
                },
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: is_spec,
                is_nif: false,
                is_unsafe: false,
                properties: class_props,
                embedding: None,
            });
        }

        // 4. Methods / Spock Features
        for cap in RE_METHOD.captures_iter(content) {
            let mname = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if mname == "if" || mname == "while" || mname == "for" || mname == "switch" {
                continue;
            }

            let is_main = mname == "main";
            let is_test =
                mname.starts_with("test") || mname.contains("should") || mname.contains(' ');

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: if is_test {
                    "groovy_test".to_string()
                } else {
                    "groovy_method".to_string()
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

        // 5. Gradle Plugins
        for cap in RE_GRADLE_PLUGIN.captures_iter(content) {
            let plugin_id = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "build.gradle".to_string(),
                to: plugin_id.to_string(),
                rel_type: "uses_plugin".to_string(),
                properties: HashMap::new(),
            });
        }

        // 6. Gradle Dependencies
        for cap in RE_GRADLE_DEPENDENCY.captures_iter(content) {
            let dep_coord = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "build.gradle".to_string(),
                to: dep_coord.to_string(),
                rel_type: "depends_on".to_string(),
                properties: HashMap::new(),
            });
        }

        // 7. Gradle Tasks
        for cap in RE_GRADLE_TASK.captures_iter(content) {
            let tname = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !tname.is_empty() {
                symbols.push(Symbol {
                    name: tname.to_string(),
                    kind: "gradle_task".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
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
    fn test_groovy_parser_basic_and_pii() {
        let code = r#"
package com.nexus.gateway

import org.slf4j.Logger
import org.slf4j.LoggerFactory

class SecurityCredential {
    String username
    String passwordHash
    String sessionToken
}

class GatewayServer {
    static void main(String[] args) {
        println("Gateway starting")
    }

    def processPayment(String amount) {
        println("Processing " + amount)
    }
}

class AuthSpec extends Specification {
    def "should validate valid token"() {
        expect:
        true == true
    }
}
"#;

        let parser = GroovyParser::new();
        let res = parser.parse(code);

        assert!(res
            .symbols
            .iter()
            .any(|s| s.name == "com.nexus.gateway" && s.kind == "groovy_package"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "org.slf4j.Logger" && r.rel_type == "imports_groovy"));

        let cred_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "SecurityCredential")
            .unwrap();
        assert_eq!(
            cred_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "SecurityCredential.passwordHash")
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
            .find(|s| s.name == "SecurityCredential.sessionToken")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let test_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "should validate valid token")
            .unwrap();
        assert!(test_fn.tested);
    }

    #[test]
    fn test_gradle_build_dsl() {
        let code = r#"
plugins {
    id 'java'
    id 'application'
}

dependencies {
    implementation 'org.slf4j:slf4j-api:2.0.7'
    testImplementation 'org.spockframework:spock-core:2.3-groovy-4.0'
}

task packageDistribution {
    doLast {
        println("Packaging distribution")
    }
}
"#;

        let parser = GroovyParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "java" && r.rel_type == "uses_plugin"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "org.slf4j:slf4j-api:2.0.7" && r.rel_type == "depends_on"));

        let task_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "packageDistribution")
            .unwrap();
        assert_eq!(task_sym.kind, "gradle_task");
        assert!(task_sym.is_entry_point);
    }
}
