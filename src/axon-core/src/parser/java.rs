use super::{ExtractionResult, Parser, Symbol, Relation};
use tree_sitter::{Language, Parser as TSParser, Query, QueryCursor};

pub struct JavaParser {
    language: Language,
}

impl JavaParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_java::language(),
        }
    }
}

impl Parser for JavaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();
        
        let query_str = r#"
            (class_declaration name: (identifier) @class.name)
            (interface_declaration name: (identifier) @interface.name)
            (method_declaration name: (identifier) @method.name)
            (method_invocation name: (identifier) @call.name)
        "#;
        
        let query = Query::new(self.language, query_str).unwrap();
        let mut cursor = QueryCursor::new();
        let mut symbols = Vec::new();
        let mut relations = Vec::new();
        
        for m in cursor.matches(&query, tree.root_node(), content.as_bytes()) {
            for capture in m.captures {
                let node = capture.node;
                let kind = query.capture_names()[capture.index as usize].as_str();
                let name = node.utf8_text(content.as_bytes()).unwrap_or("unknown").to_string();

                match kind {
                    "class.name" | "interface.name" => symbols.push(Symbol {
                        name, kind: "type".to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        properties: std::collections::HashMap::new(),
                    }),
                    "method.name" => symbols.push(Symbol {
                        name, kind: "function".to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        properties: std::collections::HashMap::new(),
                    }),
                    "call.name" => relations.push(Relation {
                        from: "context".to_string(),
                        to: name,
                        rel_type: "CALLS".to_string(),
                                        properties: std::collections::HashMap::new(),
                    }),
                    _ => {}
                }
            }
        }
        
        ExtractionResult { symbols, relations }
    }
}
