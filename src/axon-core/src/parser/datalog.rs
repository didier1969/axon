use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_DECL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\.decl\s+([a-zA-Z0-9_-]+)\s*\(").expect("valid regex"));

static RE_RULE_HEAD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^([a-zA-Z0-9_-]+)\s*\([^)]*\)\s*:-").expect("valid regex"));

static RE_ATOM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([a-zA-Z0-9_-]+)\s*\(").expect("valid regex"));

pub struct DatalogParser;

impl Default for DatalogParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DatalogParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parser for DatalogParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        // 1. Extract declarations
        for cap in RE_DECL.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "datalog_relation".to_string(),
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

        // 2. Extract rule symbols
        for cap in RE_RULE_HEAD.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str();
                if !symbols
                    .iter()
                    .any(|s| s.name == name && s.kind == "datalog_rule")
                {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: "datalog_rule".to_string(),
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
        }

        // 3. Extract rule dependencies (head :- body)
        for line in content.lines() {
            if let Some((head_part, body_part)) = line.split_once(":-") {
                if let Some(head_cap) = RE_ATOM.captures(head_part) {
                    let head = &head_cap[1];
                    for body_cap in RE_ATOM.captures_iter(body_part) {
                        let body_rel = &body_cap[1];
                        relations.push(Relation {
                            from: head.to_string(),
                            to: body_rel.to_string(),
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
    fn test_parse_datalog() {
        let code = r#"
        .decl parent(x: symbol, y: symbol)
        .decl ancestor(x: symbol, y: symbol)
        
        ancestor(x, y) :- parent(x, y).
        ancestor(x, y) :- parent(x, z), ancestor(z, y).
        "#;

        let parser = DatalogParser::new();
        let result = parser.parse(code);

        // Assert symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "parent" && s.kind == "datalog_relation"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "ancestor" && s.kind == "datalog_relation"));

        // Assert relations
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "ancestor" && r.to == "parent" && r.rel_type == "depends_on"));
    }
}
