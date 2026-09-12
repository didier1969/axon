use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_NODE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\((?:([a-zA-Z_][a-zA-Z0-9_]*)\s*)?(?::([a-zA-Z_][a-zA-Z0-9_:]*))?(?:\s*\{([^}]*)\})?\)",
    )
    .expect("valid regex")
});

static RE_REL_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[(?:([a-zA-Z_][a-zA-Z0-9_]*)\s*)?(?::([a-zA-Z_][a-zA-Z0-9_|]*))?(?:\*([0-9..]*))?(?:\s*\{([^}]*)\})?\]").expect("valid regex")
});

static RE_PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\((?:[a-zA-Z_][a-zA-Z0-9_]*\s*)?:([a-zA-Z_][a-zA-Z0-9_]*)[^)]*\)\s*-\s*\[(?::([a-zA-Z_][a-zA-Z0-9_]*))?[^\]]*\]\s*->\s*\((?:[a-zA-Z_][a-zA-Z0-9_]*\s*)?:([a-zA-Z_][a-zA-Z0-9_]*)[^)]*\)").expect("valid regex")
});

static RE_CLAUSE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(MATCH|OPTIONAL\s+MATCH|CREATE|MERGE|RETURN|WITH|WHERE|UNWIND|CALL|YIELD|DETACH\s+DELETE|DELETE|SET|REMOVE)\b").expect("valid regex")
});

static RE_CONSTRAINT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)CREATE\s+CONSTRAINT\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:([a-zA-Z0-9_]+)\s+)?FOR\s+\([a-zA-Z0-9_]+:([a-zA-Z0-9_]+)\)\s+REQUIRE\s+([^\n;]+)").expect("valid regex")
});

static RE_INDEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)CREATE\s+INDEX\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:([a-zA-Z0-9_]+)\s+)?FOR\s+\([a-zA-Z0-9_]+:([a-zA-Z0-9_]+)\)\s+ON\s+\(([^\n;)]+)\)").expect("valid regex")
});

static RE_PII: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(password|passwd|secret|token|api_key|apikey|bearer|auth|private_key|credit_card|ssn)\s*[:=]\s*['"]([^'"]+)['"]"#).expect("valid regex")
});

pub struct CypherParser;

impl Default for CypherParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CypherParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for CypherParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Constraints
        for cap in RE_CONSTRAINT.captures_iter(content) {
            let name = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "constraint".to_string());
            let label = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let requirement = cap
                .get(3)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("target_label".to_string(), label.clone());
            props.insert("requirement".to_string(), requirement);

            symbols.push(Symbol {
                name: format!("{}:{}", name, label),
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

        // 2. Indexes
        for cap in RE_INDEX.captures_iter(content) {
            let name = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "index".to_string());
            let label = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let properties = cap
                .get(3)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("target_label".to_string(), label.clone());
            props.insert("properties".to_string(), properties);

            symbols.push(Symbol {
                name: format!("{}:{}", name, label),
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

        // 3. Node labels in patterns
        for cap in RE_NODE_PATTERN.captures_iter(content) {
            if let Some(labels_match) = cap.get(2) {
                let labels_str = labels_match.as_str();
                for label in labels_str.split(':').filter(|s| !s.is_empty()) {
                    let line = start_offset_line(labels_match.start());

                    if !symbols
                        .iter()
                        .any(|s| s.name == label && s.kind == "node_label")
                    {
                        let mut props = HashMap::new();
                        if let Some(var) = cap.get(1) {
                            props.insert("variable".to_string(), var.as_str().to_string());
                        }

                        symbols.push(Symbol {
                            name: label.to_string(),
                            kind: "node_label".to_string(),
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
        }

        // 4. Relationship types in patterns
        for cap in RE_REL_PATTERN.captures_iter(content) {
            if let Some(rel_match) = cap.get(2) {
                let rel_types_str = rel_match.as_str();
                for rel_type in rel_types_str.split('|').filter(|s| !s.is_empty()) {
                    let line = start_offset_line(rel_match.start());

                    if !symbols
                        .iter()
                        .any(|s| s.name == rel_type && s.kind == "relationship_type")
                    {
                        let mut props = HashMap::new();
                        if let Some(var) = cap.get(1) {
                            props.insert("variable".to_string(), var.as_str().to_string());
                        }

                        symbols.push(Symbol {
                            name: rel_type.to_string(),
                            kind: "relationship_type".to_string(),
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
        }

        // 5. Query Clauses
        for cap in RE_CLAUSE.captures_iter(content) {
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

        // 6. Path relations : (a:LabelA)-[:REL_TYPE]->(b:LabelB)
        for cap in RE_PATH_PATTERN.captures_iter(content) {
            let src = cap
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let rel = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "RELATES_TO".to_string());
            let tgt = cap
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            if !src.is_empty() && !tgt.is_empty() {
                relations.push(Relation {
                    from: src,
                    to: tgt,
                    rel_type: rel,
                    properties: HashMap::new(),
                });
            }
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
    fn test_cypher_parser_basic_and_pii() {
        let code = r#"
        // Cypher graph query
        CREATE CONSTRAINT n1_unique FOR (u:User) REQUIRE u.email IS UNIQUE;
        CREATE INDEX idx_user_name FOR (u:User) ON (u.name);

        MATCH (u:User)-[:POSTED]->(p:Post)
        WHERE u.active = true
        RETURN u.name, p.title;

        CREATE (a:Admin:Staff {name: "Root", password: "super_secret_cypher"});
        "#;

        let parser = CypherParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "constraint" && s.name.contains("User")));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "index" && s.name.contains("User")));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "node_label" && s.name == "User"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "node_label" && s.name == "Post"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "node_label" && s.name == "Admin"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "relationship_type" && s.name == "POSTED"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "clause" && s.name == "MATCH"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "clause" && s.name == "RETURN"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "Post" && r.rel_type == "POSTED"));

        let sec = result
            .symbols
            .iter()
            .find(|s| s.name == "password")
            .unwrap();
        assert_eq!(
            sec.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
    }
}
