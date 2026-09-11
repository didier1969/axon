use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct GoParser {
    wasm_bytes: &'static [u8],
}

impl Default for GoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GoParser {
    fn block_split_lines<'a>(block: Node<'a>) -> Vec<usize> {
        let mut cursor = block.walk();
        block
            .named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-go.wasm"),
        }
    }

    // REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    // one per decision point. `default_case`/wildcard included (mirrors the
    // Rust parser counting every `match_arm` including `_`). Go func literals
    // are not extracted as separate symbols (no nested-fn exclusion needed
    // here, same reasoning as Elixir anonymous fns): their branches simply
    // fold into the enclosing named function/method, matching how their
    // calls already fold in via walk_for_calls.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "expression_case",
        "type_case",
        "communication_case",
        "default_case",
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

    fn walk<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_declaration" => Self::extract_function(child, source_bytes, result),
                "method_declaration" => Self::extract_method(child, source_bytes, result),
                "type_declaration" => Self::extract_type_declaration(child, source_bytes, result),
                "import_declaration" => Self::extract_imports(child, source_bytes, result),
                // REQ-AXO-91506 — top-level call_expression carries no caller.
                "call_expression" => Self::extract_call(child, source_bytes, result, ""),
                _ => Self::walk(child, source_bytes, result),
            }
        }
    }

    fn extract_function<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let name_node = Self::find_child_by_type(node, "identifier");
        if let Some(n) = name_node {
            let name = n.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let name_lower = name.to_lowercase();
            let is_entry =
                name == "main" || name_lower.contains("handler") || name_lower.contains("route");

            let is_public = name.chars().next().is_some_and(|c| c.is_uppercase());
            let mut is_unsafe = false;
            let mut properties = HashMap::new();

            if let Some(body) = Self::find_child_by_type(node, "block") {
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
                let split_lines = Self::block_split_lines(body);
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
                if node_content.contains("unsafe.") {
                    is_unsafe = true;
                }
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
                // REQ-AXO-91506 — body calls carry the function name as caller.
                Self::walk_for_calls(body, source_bytes, result, false, &name);
            }

            result.symbols.push(Symbol {
                name: name.clone(),
                kind: "function".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_entry,
                is_public,
                tested: name.starts_with("Test"),
                is_nif: false,
                is_unsafe,
                properties,
                embedding: None,
            });
        }
    }

    fn extract_method<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let name_node = Self::find_child_by_type(node, "field_identifier");
        if let Some(n) = name_node {
            let name = n.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let is_public = name.chars().next().is_some_and(|c| c.is_uppercase());
            let mut is_unsafe = false;
            let mut properties = HashMap::new();

            let mut receiver_type = String::new();
            if let Some(param_list) = Self::find_child_by_type(node, "parameter_list") {
                let mut cursor = param_list.walk();
                for child in param_list.named_children(&mut cursor) {
                    if child.kind() == "parameter_declaration" {
                        if let Some(t_node) = Self::find_child_by_type(child, "type_identifier") {
                            receiver_type =
                                t_node.utf8_text(source_bytes).unwrap_or("").to_string();
                        } else if let Some(ptr_type) =
                            Self::find_child_by_type(child, "pointer_type")
                        {
                            if let Some(inner) =
                                Self::find_child_by_type(ptr_type, "type_identifier")
                            {
                                receiver_type =
                                    inner.utf8_text(source_bytes).unwrap_or("").to_string();
                            }
                        }
                    }
                }
            }

            if !receiver_type.is_empty() {
                properties.insert("class_name".to_string(), receiver_type);
            }

            if let Some(body) = Self::find_child_by_type(node, "block") {
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
                let split_lines = Self::block_split_lines(body);
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
                if node_content.contains("unsafe.") {
                    is_unsafe = true;
                }
                let complexity = 1 + Self::count_branches(body);
                properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
                // REQ-AXO-91506 — methods carry Type::method as caller.
                let caller = if let Some(rt) = properties.get("class_name") {
                    format!("{}::{}", rt, name)
                } else {
                    name.clone()
                };
                Self::walk_for_calls(body, source_bytes, result, false, &caller);
            }

            result.symbols.push(Symbol {
                name: name.clone(),
                kind: "method".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                is_public,
                tested: false,
                is_nif: false,
                is_unsafe,
                properties,
                embedding: None,
            });
        }
    }

    fn extract_type_declaration<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "type_spec" {
                Self::extract_type_spec(child, source_bytes, result);
            }
        }
    }

    fn extract_type_spec<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let name_node = Self::find_child_by_type(node, "type_identifier");
        if let Some(n) = name_node {
            let name = n.utf8_text(source_bytes).unwrap_or("").to_string();
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            let mut kind = "type_alias".to_string();
            let mut type_node = None;
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "struct_type" {
                    kind = "struct".to_string();
                    type_node = Some(child);
                    break;
                } else if child.kind() == "interface_type" {
                    kind = "interface".to_string();
                    type_node = Some(child);
                    break;
                }
            }

            let is_public = name.chars().next().is_some_and(|c| c.is_uppercase());

            result.symbols.push(Symbol {
                name: name.clone(),
                kind: kind.clone(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                is_public,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });

            if let Some(tn) = type_node {
                if kind == "struct" {
                    Self::extract_struct_fields(tn, source_bytes, result, &name);
                } else if kind == "interface" {
                    Self::extract_interface_members(tn, source_bytes, result, &name);
                }
            }
        }
    }

    fn extract_struct_fields<'a>(
        struct_node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        struct_name: &str,
    ) {
        if let Some(field_list) = Self::find_child_by_type(struct_node, "field_declaration_list") {
            let mut cursor = field_list.walk();
            for field in field_list.named_children(&mut cursor) {
                if field.kind() == "field_declaration" {
                    if Self::find_child_by_type(field, "field_identifier").is_none() {
                        let embedded_type = if let Some(t) =
                            Self::find_child_by_type(field, "type_identifier")
                        {
                            t.utf8_text(source_bytes).unwrap_or("").to_string()
                        } else if let Some(ptr) = Self::find_child_by_type(field, "pointer_type") {
                            if let Some(inner) = Self::find_child_by_type(ptr, "type_identifier") {
                                inner.utf8_text(source_bytes).unwrap_or("").to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        };

                        if !embedded_type.is_empty() {
                            result.relations.push(Relation {
                                from: struct_name.to_string(),
                                to: embedded_type,
                                rel_type: "embeds".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }
        }
    }

    fn extract_interface_members<'a>(
        iface_node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        interface_name: &str,
    ) {
        let mut cursor = iface_node.walk();
        for child in iface_node.named_children(&mut cursor) {
            match child.kind() {
                "method_spec" | "method_elem" => {
                    if let Some(id) = Self::find_child_by_type(child, "field_identifier")
                        .or_else(|| Self::find_child_by_type(child, "identifier"))
                    {
                        let method_name = id.utf8_text(source_bytes).unwrap_or("").to_string();
                        if !method_name.is_empty() {
                            let start_line = child.start_position().row + 1;
                            let end_line = child.end_position().row + 1;
                            let is_public =
                                method_name.chars().next().is_some_and(|c| c.is_uppercase());
                            let mut properties = HashMap::new();
                            properties.insert("interface".to_string(), interface_name.to_string());
                            result.symbols.push(Symbol {
                                name: method_name,
                                kind: "method".to_string(),
                                start_line,
                                end_line,
                                docstring: None,
                                is_entry_point: false,
                                is_public,
                                tested: false,
                                is_nif: false,
                                is_unsafe: false,
                                properties,
                                embedding: None,
                            });
                        }
                    }
                }
                "type_elem" | "type_identifier" => {
                    let embedded_iface =
                        if let Some(t) = Self::find_child_by_type(child, "type_identifier") {
                            t.utf8_text(source_bytes).unwrap_or("").to_string()
                        } else {
                            child
                                .utf8_text(source_bytes)
                                .unwrap_or("")
                                .trim()
                                .to_string()
                        };
                    if !embedded_iface.is_empty() {
                        result.relations.push(Relation {
                            from: interface_name.to_string(),
                            to: embedded_iface,
                            rel_type: "embeds".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn extract_imports<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "import_spec_list" {
                let mut spec_cursor = child.walk();
                for spec in child.named_children(&mut spec_cursor) {
                    if spec.kind() == "import_spec" {
                        Self::extract_import_spec(spec, source_bytes, result);
                    }
                }
            } else if child.kind() == "import_spec" {
                Self::extract_import_spec(child, source_bytes, result);
            } else if child.kind() == "interpreted_string_literal" {
                let path = child
                    .utf8_text(source_bytes)
                    .unwrap_or("")
                    .trim_matches('"')
                    .to_string();
                let properties = HashMap::new();
                result.relations.push(Relation {
                    from: "".to_string(),
                    to: path,
                    rel_type: "imports".to_string(),
                    properties,
                });
            }
        }
    }

    fn extract_import_spec<'a>(node: Node<'a>, source_bytes: &[u8], result: &mut ExtractionResult) {
        let mut alias = String::new();
        let mut path = String::new();

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "package_identifier" {
                alias = child.utf8_text(source_bytes).unwrap_or("").to_string();
            } else if child.kind() == "interpreted_string_literal" {
                path = child
                    .utf8_text(source_bytes)
                    .unwrap_or("")
                    .trim_matches('"')
                    .to_string();
            } else if child.kind() == "dot" {
                alias = ".".to_string();
            }
        }

        if !path.is_empty() {
            let mut properties = HashMap::new();
            if !alias.is_empty() {
                properties.insert("alias".to_string(), alias);
            }
            result.relations.push(Relation {
                from: "".to_string(),
                to: path,
                rel_type: "imports".to_string(),
                properties,
            });
        }
    }

    fn extract_call<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller: &str,
    ) {
        let func_node = node.named_child(0);
        if let Some(f_node) = func_node {
            let mut name = String::new();
            let mut receiver = String::new();

            if f_node.kind() == "identifier" {
                name = f_node.utf8_text(source_bytes).unwrap_or("").to_string();
            } else if f_node.kind() == "selector_expression" {
                if let Some(field) = Self::find_child_by_type(f_node, "field_identifier") {
                    name = field.utf8_text(source_bytes).unwrap_or("").to_string();
                }
                if let Some(operand) = f_node.named_child(0) {
                    receiver = operand.utf8_text(source_bytes).unwrap_or("").to_string();
                }
                // REQ-AXO-902200 — the receiver may itself be a call (`g(1).h()`).
                // The walk_for_calls(skip_first=true) below drops child(0) (this
                // whole selector_expression), losing the inner call. Walk the
                // selector here so the operand call_expression is recovered
                // (recursively handles `g().h().i()`). Mirrors the Rust fix (902195).
                Self::walk_for_calls(f_node, source_bytes, result, false, caller);
            }

            if !name.is_empty() {
                let mut properties = HashMap::new();
                if !receiver.is_empty() {
                    properties.insert("receiver".to_string(), receiver);
                }
                result.relations.push(Relation {
                    from: caller.to_string(),
                    to: name,
                    rel_type: "calls".to_string(),
                    properties,
                });
            }
        }

        Self::walk_for_calls(node, source_bytes, result, true, caller);
    }

    fn walk_for_calls<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        skip_first: bool,
        caller: &str,
    ) {
        let mut cursor = node.walk();
        let mut children = node.named_children(&mut cursor);
        if skip_first {
            children.next();
        }

        for child in children {
            if child.kind() == "call_expression" {
                Self::extract_call(child, source_bytes, result, caller);
            } else {
                Self::walk_for_calls(child, source_bytes, result, false, caller);
            }
        }
    }

    fn find_child_by_type<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let res = node
            .named_children(&mut cursor)
            .find(|&child| child.kind() == kind);
        res
    }
}

impl Parser for GoParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("go", self.wasm_bytes, content) {
            let source_bytes = content.as_bytes();
            Self::walk(tree.root_node(), source_bytes, &mut result);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> GoParser {
        GoParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let p = parser();
        let result = p.parse("package main\nfunc f() int {\n\treturn 1\n}\n");
        if result.symbols.is_empty() {
            eprintln!("go wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let p = parser();
        // 1 (base) + if + for + 2 switch cases (incl. default) = 5.
        let result = p.parse(
            "package main\n\
             func f(x int) int {\n\
             \tif x > 0 {\n\
             \t\treturn 1\n\
             \t}\n\
             \tfor i := 0; i < x; i++ {\n\
             \t}\n\
             \tswitch x {\n\
             \tcase 0:\n\
             \t\treturn 0\n\
             \tdefault:\n\
             \t\treturn -1\n\
             \t}\n\
             \treturn x\n\
             }\n",
        );
        if result.symbols.is_empty() {
            eprintln!("go wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("5"),
            "props: {:?}",
            f.properties
        );
    }

    #[test]
    fn method_gets_its_own_complexity() {
        let p = parser();
        let result = p.parse(
            "package main\n\
             type T struct{}\n\
             func (t *T) M(x int) int {\n\
             \tif x > 0 {\n\
             \t\treturn 1\n\
             \t}\n\
             \treturn 0\n\
             }\n",
        );
        if result.symbols.is_empty() {
            eprintln!("go wasm grammar unavailable, skipping");
            return;
        }
        let m = result.symbols.iter().find(|s| s.name == "M").unwrap();
        assert_eq!(
            m.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn go_parses_struct_and_interface_embedding_and_methods() {
        let p = parser();
        let code = r#"
        package main

        type Reader interface {
            Read(p []byte) (n int, err error)
        }

        type ReadCloser interface {
            Reader
            Close() error
        }

        type BaseService struct {
            ID string
        }

        type UserService struct {
            BaseService
            *Config
            Name string
        }
        "#;
        let result = p.parse(code);
        if result.symbols.is_empty() && result.relations.is_empty() {
            eprintln!("go wasm grammar unavailable, skipping");
            return;
        }

        // Check interfaces and structs
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "Reader" && s.kind == "interface"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "ReadCloser" && s.kind == "interface"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "UserService" && s.kind == "struct"));

        // Check interface embedding: ReadCloser embeds Reader
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "ReadCloser" && r.to == "Reader" && r.rel_type == "embeds"));

        // Check struct embedding: UserService embeds BaseService and Config
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "UserService" && r.to == "BaseService" && r.rel_type == "embeds"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "UserService" && r.to == "Config" && r.rel_type == "embeds"));

        // Check interface method
        assert!(result.symbols.iter().any(|s| s.name == "Read"
            && s.kind == "method"
            && s.properties.get("interface").map(String::as_str) == Some("Reader")));
    }
}
