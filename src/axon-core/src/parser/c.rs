use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct CParser {
    wasm_bytes: &'static [u8],
}

impl Default for CParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-c.wasm"),
        }
    }

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_definition" => Self::extract_function(child, source_bytes, result),
                "struct_specifier" | "union_specifier" | "enum_specifier" => Self::extract_struct(child, source_bytes, result),
                "call_expression" => Self::extract_call(child, source_bytes, result),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    fn extract_function<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut name = String::new();
        if let Some(decl) = Self::find_child_by_type(node, "function_declarator") {
            if let Some(id) = Self::find_child_by_type(decl, "identifier") {
                name = id.utf8_text(source_bytes).unwrap_or("").to_string();
            }
        }

        if !name.is_empty() {
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut is_nif = false;
            let node_content = node.utf8_text(source_bytes).unwrap_or("");
            if node_content.contains("JNIEXPORT") || node_content.contains("JNICALL") ||
               node_content.contains("__declspec(dllexport)") || node_content.contains("extern \"C\"") ||
               node_content.contains("PHP_FUNCTION") || node_content.contains("PHP_METHOD") ||
               node_content.contains("rb_define_method") || node_content.contains("Init_") ||
               node_content.contains("PyMODINIT_FUNC") || node_content.contains("ERL_NIF_INIT") {
                is_nif = true;
            }

            if let Some(body) = Self::find_child_by_type(node, "compound_statement") {
                Self::walk_for_calls(body, source_bytes, result);
            }

            result.symbols.push(Symbol {
                name,
                kind: "function".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_nif,
                is_public: true,
                tested: false,
                is_nif,
                is_unsafe: true,
                properties: HashMap::new(),
                embedding: None,
            });
        }
    }

    fn extract_struct<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = Self::find_child_by_type(node, "type_identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            result.symbols.push(Symbol {
                name,
                kind: "struct".to_string(),
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
        }
    }

    fn extract_call<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(func_node) = node.named_child(0) {
            if func_node.kind() == "identifier" {
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
        }
        Self::walk_for_calls(node, source_bytes, result);
    }

    fn walk_for_calls<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call_expression" {
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

impl Parser for CParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_slug: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("c", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}
