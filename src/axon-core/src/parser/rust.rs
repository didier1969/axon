use super::{ExtractionResult, Parser, Symbol, Relation};
use tree_sitter::{Language, Parser as TSParser, Query, QueryCursor};

pub struct RustParser {
    language: Language,
}

impl RustParser {
    pub fn new() -> Self {
        Self {
            language: unsafe { std::mem::transmute(tree_sitter_rust::language()) },
        }
    }
}

impl Parser for RustParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();
        
        let query_str = r#"
            (struct_item name: (type_identifier) @struct.name)
            (enum_item name: (type_identifier) @enum.name)
            (function_item name: (identifier) @func.name)
            (impl_item type: (type_identifier) @impl.name)
            (call_expression function: (identifier) @call.name)
            (call_expression function: (field_expression field: (field_identifier) @call.name))
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
                    "struct.name" | "enum.name" | "impl.name" => symbols.push(Symbol {
                        name, kind: "type".to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        properties: std::collections::HashMap::new(),
                    }),
                    "func.name" => symbols.push(Symbol {
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
