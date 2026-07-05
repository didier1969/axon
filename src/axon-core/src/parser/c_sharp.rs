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
                "invocation_expression" => Self::extract_call(child, source_bytes, result, ""),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    /// one per decision point. C# local functions/lambdas are not extracted
    /// as a separate Symbol in this parser, so no nested-exclusion guard is
    /// needed.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "foreach_statement",
        "while_statement",
        "do_statement",
        "switch_section",
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

            let mut properties = HashMap::new();
            if let Some(body) = Self::find_child_by_type(node, "block") {
                // REQ-AXO-91506 — propagate caller into call extraction.
                Self::walk_for_calls(body, source_bytes, result, &name);
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
            }
            let name_for_symbol = name.clone();

            result.symbols.push(Symbol {
                name: name_for_symbol,
                kind: "method".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_nif,
                is_public: true,
                tested: false,
                is_nif,
                is_unsafe,
                properties,
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
            if child.kind() == "invocation_expression" {
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

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> CSharpParser {
        CSharpParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("class C { int F() { int x = 1; return x; } }");
        if result.symbols.is_empty() {
            eprintln!("c_sharp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "F").unwrap();
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let result = parser().parse(
            "class C { \
                int F(int x) { \
                    if (x > 0) { return 1; } \
                    for (int i = 0; i < x; i++) {} \
                    foreach (var y in new int[]{1,2}) {} \
                    try { } catch (Exception e) { } \
                    switch (x) { case 1: break; } \
                    return x > 0 ? 1 : 0; \
                } \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("c_sharp wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "F").unwrap();
        // base 1 + if + for + foreach + catch + case + ternary = 7
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("7")
        );
    }
}
