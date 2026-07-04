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
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "struct_type" {
                    kind = "struct".to_string();
                    break;
                } else if child.kind() == "interface_type" {
                    kind = "interface".to_string();
                    break;
                }
            }

            let is_public = name.chars().next().is_some_and(|c| c.is_uppercase());

            result.symbols.push(Symbol {
                name: name.clone(),
                kind,
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
