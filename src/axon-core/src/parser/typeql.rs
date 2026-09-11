use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_ENTITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)([a-zA-Z0-9_-]+)\s+sub\s+entity\b").expect("valid regex"));

static RE_RELATION: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)([a-zA-Z0-9_-]+)\s+sub\s+relation\b").expect("valid regex"));

static RE_RULE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)([a-zA-Z0-9_-]+):\s*rule\s+when\s*\{").expect("valid regex"));

static RE_OWNS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"owns\s+([a-zA-Z0-9_-]+)").expect("valid regex"));

static RE_SUB_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([a-zA-Z0-9_-]+)\s+sub").expect("valid regex"));

pub struct TypeQLParser;

impl Default for TypeQLParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeQLParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parser for TypeQLParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        // 1. Entities
        for cap in RE_ENTITY.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "entity_type".to_string(),
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
        }

        // 2. Relations
        for cap in RE_RELATION.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "relation_type".to_string(),
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
        }

        // 3. Rules
        for cap in RE_RULE.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "rule".to_string(),
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
        }

        // 4. Ownerships (owns)
        for block in content.split(';') {
            if block.contains("sub entity") || block.contains("sub relation") {
                let mut current_entity = None;
                for line in block.lines() {
                    if line.contains("sub entity") || line.contains("sub relation") {
                        if let Some(cap) = RE_SUB_NAME.captures(line) {
                            current_entity = Some(cap[1].to_string());
                        }
                    } else if let Some(ref entity) = current_entity {
                        if line.contains("owns") {
                            for cap in RE_OWNS.captures_iter(line) {
                                let attr = cap[1].to_string();
                                if !symbols
                                    .iter()
                                    .any(|s| s.name == attr && s.kind == "attribute")
                                {
                                    symbols.push(Symbol {
                                        name: attr.clone(),
                                        kind: "attribute".to_string(),
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
                                relations.push(Relation {
                                    from: entity.clone(),
                                    to: attr,
                                    rel_type: "owns".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
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
    fn test_parse_typeql_ontology() {
        let code = r#"
        define
        person sub entity,
            owns name,
            plays parentship:parent,
            plays parentship:child;
            
        parentship sub relation,
            relates parent,
            relates child;
            
        rule-people-are-parents:
        rule when {
            (parent: $p, child: $c) isa parentship;
        } then {
            $p has name "Parent";
        };
        "#;

        let parser = TypeQLParser::new();
        let result = parser.parse(code);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "person" && s.kind == "entity_type"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "parentship" && s.kind == "relation_type"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "rule-people-are-parents" && s.kind == "rule"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "name" && s.kind == "attribute"));

        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "person" && r.to == "name" && r.rel_type == "owns"));
    }
}
