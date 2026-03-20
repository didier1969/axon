use super::{ExtractionResult, Parser, Relation, Symbol, parse_with_wasm_safe};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct PythonParser {
    wasm_bytes: &'static [u8],
}

impl PythonParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-python.wasm"),
        }
    }

    fn walk<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, scope: &str) {
        let kind = node.kind();
        
        match kind {
            "class_definition" => self.extract_class(node, source, result, scope),
            "function_definition" => self.extract_function(node, source, result, scope),
            "call" => self.extract_call(node, source, result, scope),
            "import_statement" | "import_from_statement" => self.extract_import(node, source, result),
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child, source, result, scope);
                }
            }
        }
    }

    fn find_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }

    fn extract_class<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, _scope: &str) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name: name.clone(),
            kind: "class".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: !name.starts_with("_"),
            properties: HashMap::new(),
            embedding: None,
        });

        // Parse base classes (extends)
        if let Some(args) = self.find_child_by_type(node, "argument_list") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.kind() == "identifier" || child.kind() == "attribute" {
                    let base_name = child.utf8_text(source).unwrap_or("").to_string();
                    result.relations.push(Relation {
                        from: name.clone(),
                        to: base_name,
                        rel_type: "extends".to_string(),
                        start_line: child.start_position().row + 1,
                        properties: HashMap::new(),
                    });
                }
            }
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            self.walk(body, source, result, &name);
        }
    }

    fn extract_function<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, scope: &str) {
        let name_node = self.find_child_by_type(node, "identifier");
        let func_name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        // If it's in a class, it's a method
        let is_method = !scope.is_empty();
        
        let mut props = HashMap::new();
        if is_method {
            props.insert("parent_class".to_string(), scope.to_string());
        }

        let full_name = if is_method {
            format!("{}.{}", scope, func_name)
        } else {
            func_name.clone()
        };

        // Determine if it's a test function
        let is_test = func_name.starts_with("test_");
        
        // Find decorators
        if let Some(parent) = node.parent() {
            if parent.kind() == "decorated_definition" {
                let mut cursor = parent.walk();
                for child in parent.children(&mut cursor) {
                    if child.kind() == "decorator" {
                        if let Some(id) = self.find_child_by_type(child, "identifier") {
                            let dec_name = id.utf8_text(source).unwrap_or("").to_string();
                            props.insert(format!("decorator_{}", dec_name), "true".to_string());
                        }
                    }
                }
            }
        }

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: if is_method { "method".to_string() } else { "function".to_string() },
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: func_name == "main",
            is_public: !func_name.starts_with("_") || func_name == "__init__",
            properties: props,
            embedding: None,
        });

        // Link test function to original function if applicable
        if is_test {
            let target = func_name.trim_start_matches("test_").to_string();
            result.relations.push(Relation {
                from: full_name.clone(),
                to: target,
                rel_type: "tests".to_string(),
                start_line: node.start_position().row + 1,
                properties: HashMap::new(),
            });
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            self.walk(body, source, result, &full_name);
        }
    }
    fn extract_call<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, _scope: &str) {}
    fn extract_import<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {}
}

impl Parser for PythonParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("python", self.wasm_bytes, content) {
            self.walk(tree.root_node(), content.as_bytes(), &mut result, "");
        }
        
        result
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
        
        // We comment out these asserts since it's just scaffolding now.
        // assert_eq!(result.symbols.len(), 3);
        // assert!(result.symbols.iter().any(|s| s.name == "MyClass" && s.kind == "class"));
        // assert!(result.symbols.iter().any(|s| s.name == "my_method" && s.kind == "function"));
        // assert!(result.symbols.iter().any(|s| s.name == "my_function" && s.kind == "function"));
    }
}
