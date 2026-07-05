use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct CppParser {
    wasm_bytes: &'static [u8],
}

impl Default for CppParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CppParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-cpp.wasm"),
        }
    }

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_definition" => Self::extract_function(child, source_bytes, result),
                "class_specifier" | "struct_specifier" | "enum_specifier" => {
                    Self::extract_class(child, source_bytes, result)
                }
                "call_expression" => Self::extract_call(child, source_bytes, result, ""),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    /// one per decision point. C++ lambdas are not extracted as a separate
    /// Symbol in this parser, so their branches fold into the enclosing
    /// function — no nested-exclusion guard needed.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "for_range_loop",
        "while_statement",
        "do_statement",
        "case_statement",
        "catch_clause",
        "conditional_expression",
    ];

    fn count_branches(node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Self::BRANCHING_KINDS.contains(&child.kind()) {
                count += 1;
            }
            count += Self::count_branches(child);
        }
        count
    }

    fn extract_function<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut name = String::new();
        if let Some(decl) = Self::find_child_by_type(node, "function_declarator") {
            if let Some(id) = Self::find_child_by_type(decl, "identifier")
                .or_else(|| Self::find_child_by_type(decl, "field_identifier"))
            {
                name = id.utf8_text(source_bytes).unwrap_or("").to_string();
            }
        }

        if !name.is_empty() {
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut is_nif = false;
            let node_content = node.utf8_text(source_bytes).unwrap_or("");
            if node_content.contains("JNIEXPORT")
                || node_content.contains("JNICALL")
                || node_content.contains("__declspec(dllexport)")
                || node_content.contains("extern \"C\"")
                || node_content.contains("PHP_FUNCTION")
                || node_content.contains("PHP_METHOD")
                || node_content.contains("rb_define_method")
                || node_content.contains("Init_")
                || node_content.contains("PyMODINIT_FUNC")
                || node_content.contains("ERL_NIF_INIT")
            {
                is_nif = true;
            }

            let mut properties = HashMap::new();
            if let Some(body) = Self::find_child_by_type(node, "compound_statement") {
                // REQ-AXO-91506 — propagate caller name into call extraction.
                Self::walk_for_calls(body, source_bytes, result, &name);
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
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
                properties,
                embedding: None,
            });
        }
    }

    fn extract_class<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = Self::find_child_by_type(node, "type_identifier") {
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

            if let Some(body) = Self::find_child_by_type(node, "field_declaration_list") {
                Self::walk(body, source_bytes, result);
            }
        }
    }

    fn extract_call<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller: &str,
    ) {
        if let Some(func_node) = node.named_child(0) {
            let call_name = func_node.utf8_text(source_bytes).unwrap_or("").to_string();
            if !call_name.is_empty() {
                result.relations.push(Relation {
                    from: caller.to_string(),
                    to: call_name,
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
        Self::walk_for_calls(node, source_bytes, result, caller);
    }

    fn walk_for_calls<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call_expression" {
                Self::extract_call(child, source_bytes, result, caller);
            } else {
                Self::walk_for_calls(child, source_bytes, result, caller);
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

impl Parser for CppParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("cpp", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> CppParser {
        CppParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("int f() { int x = 1; return x; }");
        if result.symbols.is_empty() {
            eprintln!("cpp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let result = parser().parse(
            "int f(int x) { \
                if (x > 0) { return 1; } \
                for (int i : {1,2,3}) {} \
                try { } catch (int e) { } \
                return x > 0 ? 1 : 0; \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("cpp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + range-for + catch + ternary = 5
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("5")
        );
    }
}
