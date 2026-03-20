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
                properties: HashMap::new(),
            });
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            self.walk(body, source, result, &full_name);
        }
    }
    fn extract_call<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, scope: &str) {
        if scope.is_empty() {
            // We only care about calls inside functions/methods for taint analysis
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.walk(child, source, result, scope);
            }
            return;
        }

        let func_node = self.find_child_by_type(node, "identifier")
            .or_else(|| self.find_child_by_type(node, "attribute"));

        if let Some(n) = func_node {
            let call_name = n.utf8_text(source).unwrap_or("").to_string();
            
            // Extract the actual function name from an attribute (e.g. 'os.system' -> 'system' or keep 'os.system')
            // For taint analysis, keeping the full string is often better to match 'system' or 'os.system'
            
            result.relations.push(Relation {
                from: scope.to_string(),
                to: call_name,
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
            });
        }

        // Walk arguments
        if let Some(args) = self.find_child_by_type(node, "argument_list") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                self.walk(child, source, result, scope);
            }
        }
    }

    fn extract_import<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        // Find dotted_name or aliased_import
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "dotted_name" || child.kind() == "aliased_import" {
                let import_name = child.utf8_text(source).unwrap_or("").to_string();
                
                result.relations.push(Relation {
                    from: "module".to_string(), // Python files are modules
                    to: import_name,
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
    }
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
    fn test_parse_python_full() {
        let code = r#"
import os
from sys import argv

class MyClass(BaseClass):
    def my_method(self, data):
        os.system(data)
        eval(data)

def test_my_method():
    pass
"#;
        let parser = PythonParser::new();
        let result = parser.parse(code);
        
        // Check symbols
        assert!(result.symbols.iter().any(|s| s.name == "MyClass" && s.kind == "class"));
        assert!(result.symbols.iter().any(|s| s.name == "MyClass.my_method" && s.kind == "method"));
        assert!(result.symbols.iter().any(|s| s.name == "test_my_method" && s.kind == "function"));
        
        // Check relations
        assert!(result.relations.iter().any(|r| r.from == "MyClass" && r.to == "BaseClass" && r.rel_type == "extends"));
        assert!(result.relations.iter().any(|r| r.from == "MyClass.my_method" && r.to == "os.system" && r.rel_type == "calls"));
        assert!(result.relations.iter().any(|r| r.from == "MyClass.my_method" && r.to == "eval" && r.rel_type == "calls"));
        assert!(result.relations.iter().any(|r| r.from == "test_my_method" && r.to == "my_method" && r.rel_type == "tests"));
        assert!(result.relations.iter().any(|r| r.to == "os" && r.rel_type == "imports"));
    }
}
