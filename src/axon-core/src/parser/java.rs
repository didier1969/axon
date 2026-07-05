use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use tree_sitter::Node;

pub struct JavaParser {
    wasm_bytes: &'static [u8],
}

impl JavaParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-java.wasm"),
        }
    }

    fn walk<'a>(
        &self,
        node: Node<'a>,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        class_name: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "class_declaration" => {
                    self.extract_class(child, content, symbols);
                }
                "method_declaration" => {
                    self.extract_method(child, content, symbols, class_name);
                }
                "import_declaration" => {
                    self.extract_import(child, content, relations);
                }
                "method_invocation" => {
                    self.extract_call(child, content, relations);
                }
                _ => {}
            }

            // Recurse for nested classes
            let mut new_class = class_name.to_string();
            if child.kind() == "class_declaration" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(content) {
                        new_class = name.to_string();
                    }
                }
            }

            self.walk(child, content, symbols, relations, &new_class);
        }
    }

    fn extract_class(&self, node: Node, content: &[u8], symbols: &mut Vec<Symbol>) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let mut is_public = false;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        if let Ok(mod_text) = child.utf8_text(content) {
                            if mod_text.contains("public") {
                                is_public = true;
                            }
                        }
                    }
                }
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "class".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public,
                    tested: name.contains("Test"),
                    is_nif: false,
                    is_unsafe: false,
                    properties: std::collections::HashMap::new(),
                    embedding: None,
                });
            }
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity: base 1 +
    /// one per decision point. Java has no lambda/anonymous-class extracted
    /// as a separate Symbol in this parser (only nested `class_declaration`
    /// recurses, tagged separately), so no nested-exclusion guard is needed
    /// here — mirrors the Go/C/PHP precedent. `switch_label` matches both
    /// `case` and `default` labels (mirrors Go counting `default_case` too).
    /// Boolean short-circuit operators (`&&`/`||`) are NOT counted, same
    /// first-pass scope as rust.rs.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "enhanced_for_statement",
        "while_statement",
        "do_statement",
        "switch_label",
        "catch_clause",
        "ternary_expression",
    ];

    fn count_branches(&self, node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Self::BRANCHING_KINDS.contains(&child.kind()) {
                count += 1;
            }
            count += self.count_branches(child);
        }
        count
    }

    fn extract_method(
        &self,
        node: Node,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        class_name: &str,
    ) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let mut is_entry = false;
                let mut is_public = false;
                let mut is_nif = false;
                let mut tested = false;
                let mut decorators = Vec::new();

                let mut modifiers_node = None;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        modifiers_node = Some(child);
                        if let Ok(mod_text) = child.utf8_text(content) {
                            if mod_text.contains("public") {
                                is_public = true;
                            }
                            if mod_text.contains("native") {
                                is_nif = true;
                            }
                        }
                    }
                }

                if let Some(modifiers) = modifiers_node {
                    let mut cursor = modifiers.walk();
                    for mod_node in modifiers.children(&mut cursor) {
                        if mod_node.kind() == "marker_annotation" || mod_node.kind() == "annotation"
                        {
                            if let Some(ann_name) = mod_node.child_by_field_name("name") {
                                if let Ok(ann_text) = ann_name.utf8_text(content) {
                                    decorators.push(ann_text.to_string());
                                    let ann_text_str = ann_text;
                                    if ann_text_str.contains("Test") {
                                        tested = true;
                                    }
                                    if ann_text_str.contains("Mapping")
                                        || ann_text_str.contains("Route")
                                        || ann_text_str.contains("Endpoint")
                                        || ann_text_str.contains("GET")
                                        || ann_text_str.contains("POST")
                                        || ann_text_str.contains("PUT")
                                        || ann_text_str.contains("DELETE")
                                    {
                                        is_entry = true;
                                    }
                                }
                            }
                        }
                    }
                }

                let mut properties = std::collections::HashMap::new();
                if !class_name.is_empty() {
                    properties.insert("class_name".to_string(), class_name.to_string());
                }
                if !decorators.is_empty() {
                    properties.insert("decorators".to_string(), decorators.join(","));
                }
                if let Some(body) = node.child_by_field_name("body") {
                    let complexity = 1 + self.count_branches(body);
                    properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
                }

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "method".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: is_entry || is_nif,
                    is_public,
                    tested,
                    is_nif,
                    is_unsafe: false,
                    properties,
                    embedding: None,
                });
            }
        }
    }

    fn extract_import(&self, node: Node, content: &[u8], relations: &mut Vec<Relation>) {
        if let Some(path_node) = node.named_child(0) {
            if let Ok(path) = path_node.utf8_text(content) {
                relations.push(Relation {
                    from: "file".to_string(),
                    to: path.to_string(),
                    rel_type: "imports".to_string(),
                    properties: std::collections::HashMap::new(),
                });
            }
        }
    }

    fn extract_call(&self, node: Node, content: &[u8], relations: &mut Vec<Relation>) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let receiver_name = if let Some(object_node) = node.child_by_field_name("object") {
                    object_node.utf8_text(content).unwrap_or("").to_string()
                } else {
                    "".to_string()
                };

                let target = if !receiver_name.is_empty() {
                    format!("{}.{}", receiver_name, name)
                } else {
                    name.to_string()
                };

                let mut properties = std::collections::HashMap::new();
                properties.insert(
                    "line".to_string(),
                    (node.start_position().row + 1).to_string(),
                );

                relations.push(Relation {
                    from: "method".to_string(),
                    to: target,
                    rel_type: "calls".to_string(),
                    properties,
                });
            }
        }
    }
}

impl Parser for JavaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        if let Some(tree) = parse_with_wasm_safe("java", self.wasm_bytes, content) {
            self.walk(
                tree.root_node(),
                content.as_bytes(),
                &mut symbols,
                &mut relations,
                "",
            );
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> JavaParser {
        JavaParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("class C { void f() { int x = 1; } }");
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
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
            "class C { \
                int f(int x) { \
                    if (x > 0) { return 1; } \
                    for (int i = 0; i < x; i++) {} \
                    switch (x) { case 1: break; default: break; } \
                    return x > 0 ? 1 : 0; \
                } \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + for + case + default + ternary = 6
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("6")
        );
    }

    #[test]
    fn method_gets_its_own_complexity() {
        let result = parser().parse(
            "class C { \
                void a() { if (true) {} } \
                void b() {} \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let a = result.symbols.iter().find(|s| s.name == "a").unwrap();
        let b = result.symbols.iter().find(|s| s.name == "b").unwrap();
        assert_eq!(
            a.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            b.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("1")
        );
    }
}
