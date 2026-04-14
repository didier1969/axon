use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct CSharpParser {
    wasm_bytes: &'static [u8],
}

impl Default for CSharpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CSharpParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-c-sharp.wasm"),
        }
    }

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "method_declaration" | "constructor_declaration" => {
                    Self::extract_method(child, source_bytes, result)
                }
                "class_declaration"
                | "struct_declaration"
                | "interface_declaration"
                | "enum_declaration" => Self::extract_class(child, source_bytes, result),
                "invocation_expression" => Self::extract_call(child, source_bytes, result),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    fn extract_method<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = Self::find_child_by_type(node, "identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut is_nif = false;
            let mut is_unsafe = false;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "attribute_list" {
                    let attr_text = child.utf8_text(source_bytes).unwrap_or("");
                    if attr_text.contains("DllImport") || attr_text.contains("LibraryImport") {
                        is_nif = true;
                    }
                }
                if child.kind() == "modifier" {
                    let mod_text = child.utf8_text(source_bytes).unwrap_or("");
                    if mod_text.contains("extern") {
                        is_nif = true;
                    }
                    if mod_text.contains("unsafe") {
                        is_unsafe = true;
                    }
                }
            }

            if let Some(body) = Self::find_child_by_type(node, "block") {
                Self::walk_for_calls(body, source_bytes, result);
            }

            result.symbols.push(Symbol {
                name,
                kind: "method".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_nif,
                is_public: true,
                tested: false,
                is_nif,
                is_unsafe,
                properties: HashMap::new(),
                embedding: None,
            });
        }
    }

    fn extract_class<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = Self::find_child_by_type(node, "identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            result.symbols.push(Symbol {
                name: name.clone(),
                kind: "class".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });

            if let Some(body) = Self::find_child_by_type(node, "declaration_list") {
                Self::walk(body, source_bytes, result);
            }
        }
    }

    fn extract_call<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(func_node) = node.named_child(0) {
            let call_name = func_node.utf8_text(source_bytes).unwrap_or("").to_string();
            if !call_name.is_empty() {
                result.relations.push(Relation {
                    from: "".to_string(),
                    to: call_name,
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
        Self::walk_for_calls(node, source_bytes, result);
    }

    fn walk_for_calls<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "invocation_expression" {
                Self::extract_call(child, source_bytes, result);
            } else {
                Self::walk_for_calls(child, source_bytes, result);
            }
        }
    }

    fn find_child_by_type<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }
}

impl Parser for CSharpParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("c_sharp", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}
