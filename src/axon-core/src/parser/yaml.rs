use super::{ExtractionResult, Parser, Symbol};
use std::collections::HashMap;
use tree_sitter::{Node, Parser as TSParser};

pub struct YamlParser;

impl YamlParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for YamlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        let language: tree_sitter::Language = tree_sitter_yaml::LANGUAGE.into();
        parser.set_language(&language).expect("Error loading YAML grammar");

        let tree = parser.parse(content, None).unwrap();
        let root_node = tree.root_node();
        let source_bytes = content.as_bytes();

        let mut symbols = Vec::new();
        let relations = Vec::new();

        fn traverse(
            node: Node,
            source_bytes: &[u8],
            symbols: &mut Vec<Symbol>,
            current_path: &str,
            depth: usize,
        ) {
            let kind = node.kind();

            if kind == "block_mapping_pair" || kind == "flow_mapping_pair" {
                if let Some(key_node) = node.child_by_field_name("key") {
                    let mut key_name = String::new();
                    
                    // The key might be a plain_scalar or string_scalar etc.
                    // We can just get its text.
                    if let Ok(text) = key_node.utf8_text(source_bytes) {
                        key_name = text.trim().to_string();
                    }
                    
                    if !key_name.is_empty() {
                        let full_name = if current_path.is_empty() {
                            key_name.clone()
                        } else {
                            format!("{}.{}", current_path, key_name)
                        };

                        let is_sensitive = ["secret", "password", "token", "key"].iter()
                            .any(|s| key_name.to_lowercase().contains(s));

                        let mut properties = HashMap::new();
                        if is_sensitive {
                            properties.insert("sensitive".to_string(), "true".to_string());
                        }

                        // We extract top level and depth 1 (similar to Python), or all? Let's just do up to depth 1 
                        // as in the python parser, or we can just do it generically. The python parser did depth 1.
                        // We'll just do it for any mapping pair but only keep it if depth <= 1 (meaning it's 0 or 1 levels deep).
                        if depth <= 1 {
                            symbols.push(Symbol {
                                name: full_name.clone(),
                                kind: "function".to_string(), // python used "function" for yaml keys
                                start_line: key_node.start_position().row + 1,
                                end_line: key_node.end_position().row + 1,
                                docstring: None,
                                is_entry_point: false,
                        is_public: true,
                                properties,
                            
                                embedding: None,
                            });
                        }

                        if let Some(value_node) = node.child_by_field_name("value") {
                            let mut cursor = value_node.walk();
                            for child in value_node.children(&mut cursor) {
                                traverse(child, source_bytes, symbols, &full_name, depth + 1);
                            }
                        }
                    }
                }
            } else {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    traverse(child, source_bytes, symbols, current_path, depth);
                }
            }
        }

        traverse(root_node, source_bytes, &mut symbols, "", 0);

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yaml_parser() {
        let content = "
version: '3'
services:
  web:
    image: nginx
  db:
    image: postgres
secret_key: some_value
";
        let parser = YamlParser::new();
        let result = parser.parse(content);
        
        let mut names: Vec<_> = result.symbols.iter().map(|s| s.name.clone()).collect();
        names.sort();
        
        assert!(names.contains(&"version".to_string()));
        assert!(names.contains(&"services".to_string()));
        assert!(names.contains(&"services.web".to_string()));
        assert!(names.contains(&"services.db".to_string()));
        assert!(names.contains(&"secret_key".to_string()));
        
        // Verify sensitive property
        let secret_sym = result.symbols.iter().find(|s| s.name == "secret_key").unwrap();
        assert_eq!(secret_sym.properties.get("sensitive").unwrap(), "true");
    }
}
