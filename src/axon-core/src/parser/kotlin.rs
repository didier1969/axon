use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct KotlinParser {
    wasm_bytes: &'static [u8],
}

impl Default for KotlinParser {
    fn default() -> Self {
        Self::new()
    }
}

impl KotlinParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-kotlin.wasm"),
        }
    }

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_declaration" => Self::extract_function(child, source_bytes, result),
                "class_declaration" | "object_declaration" | "interface_declaration" => {
                    Self::extract_class(child, source_bytes, result)
                }
                "call_expression" => Self::extract_call(child, source_bytes, result, ""),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    /// one per decision point. Kotlin local functions/lambdas are not
    /// extracted as a separate Symbol in this parser, so no nested-exclusion
    /// guard is needed.
    const BRANCHING_KINDS: &[&str] = &[
        "if_expression",
        "for_statement",
        "while_statement",
        "do_while_statement",
        "when_entry",
        "catch_block",
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
        if let Some(name_node) = Self::find_child_by_type(node, "simple_identifier") {
            let name = name_node.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut is_nif = false;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "modifiers" {
                    let mod_text = child.utf8_text(source_bytes).unwrap_or("");
                    if mod_text.contains("external") {
                        is_nif = true;
                    }
                }
            }

            let mut properties = HashMap::new();
            if let Some(body) = Self::find_child_by_type(node, "function_body") {
                // REQ-AXO-91506 — propagate caller into call extraction.
                Self::walk_for_calls(body, source_bytes, result, &name);
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
            }
            let name_for_symbol = name.clone();

            result.symbols.push(Symbol {
                name: name_for_symbol,
                kind: "function".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_nif,
                is_public: true,
                tested: false,
                is_nif,
                is_unsafe: false,
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

            if let Some(body) = Self::find_child_by_type(node, "class_body") {
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
        if let Some(func_node) = Self::find_child_by_type(node, "simple_identifier") {
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

impl Parser for KotlinParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("kotlin", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> KotlinParser {
        KotlinParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("fun f(): Int { val x = 1; return x }");
        if result.symbols.is_empty() {
            eprintln!("kotlin wasm grammar unavailable, skipping");
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
        let src = "fun f(x: Int): Int {\n\
            if (x > 0) {\n\
                return 1\n\
            }\n\
            for (i in 0..x) {\n\
            }\n\
            when (x) {\n\
                1 -> 1\n\
                else -> 0\n\
            }\n\
            try {\n\
            } catch (e: Exception) {\n\
            }\n\
            return 0\n\
        }\n";
        let result = parser().parse(src);
        if result.symbols.is_empty() {
            eprintln!("kotlin wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + for + 2 when-entries + catch = 6
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("6")
        );
    }
}
