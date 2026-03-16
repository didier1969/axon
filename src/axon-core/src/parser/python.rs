use super::{ExtractionResult, Parser, Symbol, parse_with_wasm_safe};
use tree_sitter::{Query, QueryCursor};

pub struct PythonParser {
    wasm_bytes: &'static [u8],
}

impl PythonParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-python.wasm"),
        }
    }
}

impl Parser for PythonParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let tree = match parse_with_wasm_safe("python", self.wasm_bytes, content) {
            Some(t) => t,
            None => return ExtractionResult { symbols: Vec::new(), relations: Vec::new() },
        };
        
        let language = tree.language();
        let query_str = r#"
            (class_definition name: (identifier) @class.name) @class
            (function_definition name: (identifier) @func.name) @func
        "#;
        
        let query = match Query::new(&language, query_str) {
            Ok(q) => q,
            Err(e) => {
                log::warn!("Failed to create Python query: {}", e);
                return ExtractionResult { symbols: Vec::new(), relations: Vec::new() };
            }
        };
        
        let mut cursor = QueryCursor::new();
        let mut symbols = Vec::new();
        
        for m in cursor.matches(&query, tree.root_node(), content.as_bytes()) {
            for capture in m.captures {
                let node = capture.node;
                let kind = query.capture_names()[capture.index as usize];
                
                // On ne garde que les noms pour identifier le symbole
                if kind.ends_with(".name") {
                    let name = node.utf8_text(content.as_bytes()).unwrap_or("").to_string();
                    let actual_kind = if kind == "class.name" { "class" } else { "function" };
                    
                    symbols.push(Symbol {
                        name: name.clone(),
                        kind: actual_kind.to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        is_public: !name.starts_with("_"),
                        properties: std::collections::HashMap::new(),
                    
                        embedding: None,
                    });
                }
            }
        }
        
        ExtractionResult { symbols, relations: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_python_simple() {
        let code = r#"
class MyClass:
    def my_method(self):
        pass

def my_function():
    pass
"#;
        let parser = PythonParser::new();
        let result = parser.parse(code);
        
        assert_eq!(result.symbols.len(), 3);
        assert!(result.symbols.iter().any(|s| s.name == "MyClass" && s.kind == "class"));
        assert!(result.symbols.iter().any(|s| s.name == "my_method" && s.kind == "function"));
        assert!(result.symbols.iter().any(|s| s.name == "my_function" && s.kind == "function"));
    }
}
