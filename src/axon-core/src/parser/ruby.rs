use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct RubyParser {
    wasm_bytes: &'static [u8],
}

impl Default for RubyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl RubyParser {
    fn body_split_lines<'a>(body: Node<'a>) -> Vec<usize> {
        let mut cursor = body.walk();
        body.named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-ruby.wasm"),
        }
    }

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "method" | "singleton_method" => Self::extract_method(child, source_bytes, result),
                "class" | "module" => Self::extract_class(child, source_bytes, result),
                "call" => Self::extract_call(child, source_bytes, result, ""),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    /// one per decision point. Nested `method`/`singleton_method` bodies are
    /// skipped so a nested `def` would get its own complexity rather than
    /// inflating the enclosing one, IF this parser ever extracts nested defs
    /// as a separate Symbol — today it does not (`extract_method`'s body
    /// walk only calls `walk_for_calls`, never re-enters `walk`/
    /// `extract_method` for a nested `def`), so this guard is a defensive
    /// no-op, not a fix for that pre-existing extraction gap.
    fn count_branches(node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "method" || child.kind() == "singleton_method" {
                continue;
            }
            if matches!(
                child.kind(),
                "if" | "unless"
                    | "while"
                    | "until"
                    | "for"
                    | "when"
                    | "rescue"
                    | "elsif"
                    | "conditional"
            ) {
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
            let mut properties = HashMap::new();
            if let Some(body) = Self::find_child_by_type(node, "body_statement") {
                properties.insert(
                    "header_end_line".to_string(),
                    body.start_position().row.saturating_add(1).to_string(),
                );
                properties.insert(
                    "body_start_line".to_string(),
                    body.start_position().row.saturating_add(1).to_string(),
                );
                properties.insert(
                    "body_end_line".to_string(),
                    body.end_position().row.saturating_add(1).to_string(),
                );
                let split_lines = Self::body_split_lines(body);
                if split_lines.len() > 1 {
                    properties.insert(
                        "body_split_lines".to_string(),
                        split_lines
                            .into_iter()
                            .map(|line| line.to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
                let node_content = body.utf8_text(source_bytes).unwrap_or("");
                if node_content.contains("attach_function") || node_content.contains("FFI::") {
                    is_nif = true;
                }
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
                // REQ-AXO-91506 — propagate caller into call extraction.
                Self::walk_for_calls(body, source_bytes, result, &name);
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
                is_unsafe: false,
                properties,
                embedding: None,
            });
        }
    }

    fn extract_class<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = Self::find_child_by_type(node, "constant") {
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

            if let Some(body) = Self::find_child_by_type(node, "body_statement") {
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
        if let Some(func_node) = Self::find_child_by_type(node, "identifier") {
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
            if child.kind() == "call" {
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

impl Parser for RubyParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("ruby", self.wasm_bytes, content) {
            Self::walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> RubyParser {
        RubyParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("def f\n  x = 1\n  x\nend\n");
        if result.symbols.is_empty() {
            eprintln!("ruby wasm grammar unavailable, skipping");
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
            "def f(x)\n\
             \x20 if x > 0\n\
             \x20   1\n\
             \x20 end\n\
             \x20 case x\n\
             \x20 when 1\n\
             \x20   2\n\
             \x20 end\n\
             \x20 begin\n\
             \x20   risky\n\
             \x20 rescue\n\
             \x20   0\n\
             \x20 end\n\
             \x20 x > 0 ? 1 : 0\n\
             end\n",
        );
        if result.symbols.is_empty() {
            eprintln!("ruby wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + when + rescue + ternary(conditional) = 5
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("5")
        );
    }
}
