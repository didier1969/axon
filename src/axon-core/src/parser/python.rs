use super::{ExtractionResult, Parser, Symbol};
use tree_sitter::{Language, Parser as TSParser, Query, QueryCursor};

pub struct PythonParser {
    language: Language,
}

impl PythonParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_python::LANGUAGE.into(),
        }
    }
}

impl Parser for PythonParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(&self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();
        
        let query_str = r#"
            (class_definition name: (identifier) @class.name) @class
            (function_definition name: (identifier) @func.name) @func
        "#;
        
        let query = Query::new(&self.language, query_str).unwrap();
        let mut cursor = QueryCursor::new();
        let mut symbols = Vec::new();
        
        for m in cursor.matches(&query, tree.root_node(), content.as_bytes()) {
            for capture in m.captures {
                let node = capture.node;
                let kind = query.capture_names()[capture.index as usize];
                
                // On ne garde que les noms pour identifier le symbole
                if kind.ends_with(".name") {
                    let name = node.utf8_text(content.as_bytes()).unwrap().to_string();
                    let actual_kind = if kind == "class.name" { "class" } else { "function" };
                    
                    symbols.push(Symbol {
                        name,
                        kind: actual_kind.to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None, // TODO: Extraction docstrings,
                        is_entry_point: false,
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
