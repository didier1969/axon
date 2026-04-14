use super::{parse_with_wasm_safe, ExtractionResult, Parser, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct YamlParser {
    wasm_bytes: &'static [u8],
}

impl YamlParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-yaml.wasm"),
        }
    }
}

impl Parser for YamlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let relations = Vec::new();

        if let Some(tree) = parse_with_wasm_safe("yaml", self.wasm_bytes, content) {
            let root_node = tree.root_node();
            let source_bytes = content.as_bytes();

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

                        if let Ok(text) = key_node.utf8_text(source_bytes) {
                            key_name = text.trim().to_string();
                        }

                        if !key_name.is_empty() {
                            let full_name = if current_path.is_empty() {
                                key_name.clone()
                            } else {
                                format!("{}.{}", current_path, key_name)
                            };

                            let is_sensitive = ["secret", "password", "token", "key"]
                                .iter()
                                .any(|s| key_name.to_lowercase().contains(s));

                            let mut properties = HashMap::new();
                            if is_sensitive {
                                properties.insert("sensitive".to_string(), "true".to_string());
                            }

                            if depth <= 1 {
                                symbols.push(Symbol {
                                    name: full_name.clone(),
                                    kind: "config_key".to_string(),
                                    start_line: key_node.start_position().row + 1,
                                    end_line: key_node.end_position().row + 1,
                                    docstring: None,
                                    is_entry_point: false,
                                    is_public: true,
                                    tested: false,
                                    is_nif: false,
                                    is_unsafe: false,
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
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}
