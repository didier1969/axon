use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct PythonParser {
    wasm_bytes: &'static [u8],
}

impl PythonParser {
    fn block_split_lines<'a>(&self, block: Node<'a>) -> Vec<usize> {
        let mut cursor = block.walk();
        block
            .named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

    // REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    // one per decision point. Nested `function_definition`/`lambda` skipped:
    // they get their own count when `walk` visits them separately, so a
    // nested closure's branches must never inflate the enclosing function's.
    fn count_branches(&self, node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if matches!(child.kind(), "function_definition" | "lambda") {
                continue;
            }
            if matches!(
                child.kind(),
                "if_statement"
                    | "elif_clause"
                    | "for_statement"
                    | "while_statement"
                    | "except_clause"
                    | "case_clause"
                    | "conditional_expression"
            ) {
                count += 1;
            }
            count += self.count_branches(child);
        }
        count
    }

    #[allow(clippy::new_without_default)]
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
            "import_statement" | "import_from_statement" => {
                self.extract_import(node, source, result)
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child, source, result, scope);
                }
            }
        }
    }

    #[allow(clippy::manual_find)]
    fn find_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }

    fn extract_class<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        _scope: &str,
    ) {
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
            tested: name.starts_with("Test"),
            is_nif: false,
            is_unsafe: false,
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

    fn extract_function<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        scope: &str,
    ) {
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
        // REQ-AXO-901958 — recognise pytest fixtures (`@fixture` / `@pytest.fixture`
        // / `@pytest_asyncio.fixture`). A fixture is invoked by the test framework,
        // never by an explicit call expression, so it has no inbound CALLS edge and
        // was mis-reported as dead code. We fold it into the already-persisted
        // `tested` flag (which dead_code_count / orphan_code_symbols already skip),
        // avoiding a new Symbol column on the COPY-BINARY ingestion path.
        let mut is_fixture = false;
        if let Some(parent) = node.parent() {
            if parent.kind() == "decorated_definition" {
                let mut cursor = parent.walk();
                for child in parent.children(&mut cursor) {
                    if child.kind() == "decorator" {
                        let dec_text = child.utf8_text(source).unwrap_or("");
                        if dec_text.contains("fixture") {
                            is_fixture = true;
                        }
                        if let Some(id) = self.find_child_by_type(child, "identifier") {
                            let dec_name = id.utf8_text(source).unwrap_or("").to_string();
                            props.insert(format!("decorator_{}", dec_name), "true".to_string());
                        }
                    }
                }
            }
        }

        if let Some(body) = self.find_child_by_type(node, "block") {
            props.insert(
                "header_end_line".to_string(),
                body.start_position().row.to_string(),
            );
            props.insert(
                "body_start_line".to_string(),
                body.start_position().row.saturating_add(1).to_string(),
            );
            props.insert(
                "body_end_line".to_string(),
                body.end_position().row.saturating_add(1).to_string(),
            );
            let split_lines = self.block_split_lines(body);
            if split_lines.len() > 1 {
                props.insert(
                    "body_split_lines".to_string(),
                    split_lines
                        .into_iter()
                        .map(|line| line.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            let complexity = 1 + self.count_branches(body);
            props.insert(
                "cyclomatic_complexity".to_string(),
                complexity.to_string(),
            );
        }

        // --- UNSAFE DETECTION ---
        let mut is_unsafe = false;
        let mut is_nif = false;
        let body_text = node.utf8_text(source).unwrap_or("");
        if body_text.contains("eval(")
            || body_text.contains("exec(")
            || body_text.contains("os.system(")
            || body_text.contains("subprocess.run(")
        {
            is_unsafe = true;
        }
        if body_text.contains("ctypes") || body_text.contains("cffi") {
            is_nif = true;
        }

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: if is_method {
                "method".to_string()
            } else {
                "function".to_string()
            },
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: func_name == "main" || is_nif,
            is_public: !func_name.starts_with("_") || func_name == "__init__",
            // REQ-AXO-901958 — fixtures fold into `tested` (framework-invoked, no
            // inbound CALLS edge → would be mis-flagged as dead).
            tested: is_test || is_fixture,
            is_nif,
            is_unsafe,
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

    fn extract_call<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        scope: &str,
    ) {
        if scope.is_empty() {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.walk(child, source, result, scope);
            }
            return;
        }

        let func_node = self
            .find_child_by_type(node, "identifier")
            .or_else(|| self.find_child_by_type(node, "attribute"));

        if let Some(n) = func_node {
            let call_name = n.utf8_text(source).unwrap_or("").to_string();

            result.relations.push(Relation {
                from: scope.to_string(),
                to: call_name,
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
            });
        }

        if let Some(args) = self.find_child_by_type(node, "argument_list") {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                self.walk(child, source, result, scope);
            }
        }
    }

    fn extract_import<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "dotted_name" || child.kind() == "aliased_import" {
                let import_name = child.utf8_text(source).unwrap_or("").to_string();

                result.relations.push(Relation {
                    from: "module".to_string(),
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
            project_code: None,
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
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests,
    //! mirroring the Rust parser's coverage (parser/rust.rs) for the second
    //! language in the operator-chosen "all languages" scope.
    use super::*;
    use crate::parser::Parser;

    fn parser() -> PythonParser {
        PythonParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let p = parser();
        let result = p.parse("def f():\n    x = 1\n    y = x + 1\n");
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
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
        let p = parser();
        // 1 (base) + if + elif + while + for + except = 6.
        let result = p.parse(
            "def f(x):\n\
             \x20   if x > 0:\n\
             \x20       pass\n\
             \x20   elif x < 0:\n\
             \x20       pass\n\
             \x20   while x > 0:\n\
             \x20       break\n\
             \x20   for i in range(x):\n\
             \x20       pass\n\
             \x20   try:\n\
             \x20       pass\n\
             \x20   except ValueError:\n\
             \x20       pass\n",
        );
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("6"),
            "props: {:?}",
            f.properties
        );
    }

    #[test]
    fn nested_function_branch_does_not_inflate_enclosing_complexity() {
        let p = parser();
        // outer: base 1 + 1 if = 2. The Python parser does not currently
        // extract nested `def` as its own Symbol (pre-existing gap, out of
        // scope here) — but `count_branches` must still exclude the nested
        // `function_definition` subtree, so its `if` does not leak into
        // outer's count.
        let result = p.parse(
            "def outer(x):\n\
             \x20   if x > 0:\n\
             \x20       pass\n\
             \x20   def inner(y):\n\
             \x20       if y > 0:\n\
             \x20           pass\n",
        );
        if result.symbols.is_empty() {
            eprintln!("python wasm grammar unavailable, skipping");
            return;
        }
        let outer = result.symbols.iter().find(|s| s.name == "outer").unwrap();
        assert_eq!(
            outer.properties.get("cyclomatic_complexity").map(String::as_str),
            Some("2")
        );
    }
}
