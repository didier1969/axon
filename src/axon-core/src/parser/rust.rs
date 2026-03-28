use super::{ExtractionResult, Parser, Relation, Symbol, parse_with_wasm_safe};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct RustParser {
    wasm_bytes: &'static [u8],
}

impl Default for RustParser {
    fn default() -> Self {
        Self::new()
    }
}

impl RustParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-rust.wasm"),
        }
    }

    fn find_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let res = node.children(&mut cursor).find(|&child| child.kind() == kind);
        res
    }

    fn has_visibility(&self, node: Node) -> bool {
        self.find_child_by_type(node, "visibility_modifier").is_some()
    }

    fn walk<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, class_name: &str) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_item" => self.extract_function(child, source, result, class_name),
                "function_signature_item" => self.extract_function_signature(child, source, result, class_name),
                "struct_item" => self.extract_struct(child, source, result),
                "enum_item" => self.extract_enum(child, source, result),
                "trait_item" => self.extract_trait(child, source, result),
                "impl_item" => self.extract_impl(child, source, result),
                "mod_item" => self.extract_mod(child, source, result),
                "type_item" => self.extract_type_alias(child, source, result),
                "use_declaration" => self.extract_use(child, source, result),
                "call_expression" => self.extract_call_expression(child, source, result),
                "method_call_expression" => self.extract_method_call(child, source, result),
                "macro_invocation" => self.extract_macro_invocation(child, source, result),
                "line_comment" | "block_comment" => self.extract_comment(child, source, result),
                _ => self.walk(child, source, result, class_name),
            }
        }
    }

    fn extract_comment<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        if let Ok(text) = node.utf8_text(source) {
            if text.contains("TODO") || text.contains("FIXME") {
                let kind = if text.contains("TODO") { "TODO" } else { "FIXME" };
                result.symbols.push(Symbol {
                    name: text.trim().to_string(),
                    kind: kind.to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_public: false,
                    is_entry_point: false,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }
    }

    fn extract_function<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, class_name: &str) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        let is_pub = self.has_visibility(node);
        let mut is_unsafe = false;
        let mut is_extern_c = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let mut check_node = |n: Node| {
                if n.kind() == "unsafe" || n.kind() == "unsafe_keyword" {
                    is_unsafe = true;
                }
                if n.kind() == "extern_modifier" {
                    if let Ok(text) = n.utf8_text(source) {
                        if text.contains('C') {
                            is_extern_c = true;
                        }
                    }
                }
            };
            
            check_node(child);
            if child.kind() == "function_modifiers" {
                let mut mod_cursor = child.walk();
                for m in child.children(&mut mod_cursor) {
                    check_node(m);
                }
            }
        }

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let kind = if class_name.is_empty() { "function" } else { "method" };

        let lower_name = name.to_lowercase();
        let is_entry = is_extern_c || (is_pub && (lower_name.contains("main") || lower_name.contains("handler") || lower_name.contains("nif_")));

        let mut props = HashMap::new();
        if !class_name.is_empty() {
            props.insert("class_name".to_string(), class_name.to_string());
        }

        let mut is_nif = false;
        let mut tested = false;
        let mut prev_node = node.prev_sibling();
        while let Some(sibling) = prev_node {
            if sibling.kind() == "attribute_item" {
                if let Ok(attr_text) = sibling.utf8_text(source) {
                    if attr_text.contains("rustler::nif") || attr_text.contains("no_mangle") {
                        is_nif = true;
                    }
                    if attr_text.contains("test") {
                        tested = true;
                    }
                }
            } else if sibling.kind() != "line_comment" && sibling.kind() != "block_comment" {
                break;
            }
            prev_node = sibling.prev_sibling();
        }

        result.symbols.push(Symbol {
            name,
            kind: kind.to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: is_entry || is_nif,
            is_public: is_pub,
            tested,
            is_nif,
            is_unsafe,
            properties: props,
            embedding: None,
        });

        if let Some(block) = self.find_child_by_type(node, "block") {
            self.walk(block, source, result, class_name);
        }
    }

    fn extract_function_signature<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, class_name: &str) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        let kind = if class_name.is_empty() { "function" } else { "method" };
        let mut props = HashMap::new();
        if !class_name.is_empty() {
            props.insert("class_name".to_string(), class_name.to_string());
        }

        result.symbols.push(Symbol {
            name,
            kind: kind.to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: props,
            embedding: None,
        });
    }

    fn extract_struct<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let name_node = self.find_child_by_type(node, "type_identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name,
            kind: "struct".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });
    }

    fn extract_enum<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let name_node = self.find_child_by_type(node, "type_identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name,
            kind: "enum".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });
    }

    fn extract_trait<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let name_node = self.find_child_by_type(node, "type_identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name: name.clone(),
            kind: "interface".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        if let Some(decl_list) = self.find_child_by_type(node, "declaration_list") {
            self.walk(decl_list, source, result, &name);
        }
    }

    fn extract_impl<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let mut type_nodes = Vec::new();
        let mut has_for = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type_identifier" {
                type_nodes.push(child);
            }
            if child.kind() == "for" {
                has_for = true;
            }
        }

        let mut struct_name = String::new();

        if has_for && type_nodes.len() >= 2 {
            let trait_name = type_nodes[0].utf8_text(source).unwrap_or("").to_string();
            struct_name = type_nodes[1].utf8_text(source).unwrap_or("").to_string();

            result.relations.push(Relation {
                from: struct_name.clone(),
                to: trait_name,
                rel_type: "implements".to_string(),
                properties: HashMap::new(),
            });
        } else if type_nodes.len() == 1 {
            struct_name = type_nodes[0].utf8_text(source).unwrap_or("").to_string();
        }

        if let Some(decl_list) = self.find_child_by_type(node, "declaration_list") {
            self.walk(decl_list, source, result, &struct_name);
        }
    }

    fn extract_mod<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name,
            kind: "module".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        if let Some(decl_list) = self.find_child_by_type(node, "declaration_list") {
            self.walk(decl_list, source, result, "");
        }
    }

    fn extract_type_alias<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let name_node = self.find_child_by_type(node, "type_identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        result.symbols.push(Symbol {
            name,
            kind: "type_alias".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: self.has_visibility(node),
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });
    }

    fn extract_use<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let k = child.kind();
            if k == "scoped_identifier" || k == "scoped_use_list" || k == "identifier" || k == "use_wildcard" {
                self.process_use_node(child, "", source, result);
                return;
            }
        }
    }

    fn process_use_node<'a>(&self, node: Node<'a>, prefix: &str, source: &[u8], result: &mut ExtractionResult) {
        match node.kind() {
            "scoped_identifier" => {
                let full_path = node.utf8_text(source).unwrap_or("").to_string();
                let parts: Vec<&str> = full_path.split("::").collect();
                if let Some(&name) = parts.last() {
                    result.relations.push(Relation {
                        from: "".to_string(),
                        to: name.to_string(),
                        rel_type: "imports".to_string(),
                        properties: {
                            let mut p = HashMap::new();
                            p.insert("module".to_string(), full_path);
                            p
                        },
                    });
                }
            }
            "scoped_use_list" => {
                let mut path_prefix = String::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    let k = child.kind();
                    if k == "scoped_identifier" || k == "identifier" {
                        path_prefix = child.utf8_text(source).unwrap_or("").to_string();
                    } else if k == "use_list" {
                        self.process_use_list(child, &path_prefix, source, result);
                    }
                }
            }
            "use_list" => {
                self.process_use_list(node, prefix, source, result);
            }
            "identifier" => {
                let node_text = node.utf8_text(source).unwrap_or("").to_string();
                let full_path = if prefix.is_empty() { node_text.clone() } else { format!("{}::{}", prefix, node_text) };
                result.relations.push(Relation {
                    from: "".to_string(),
                    to: node_text,
                    rel_type: "imports".to_string(),
                    properties: {
                        let mut p = HashMap::new();
                        p.insert("module".to_string(), full_path);
                        p
                    },
                });
            }
            _ => {}
        }
    }

    fn process_use_list<'a>(&self, node: Node<'a>, prefix: &str, source: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let k = child.kind();
            if k == "identifier" {
                let node_text = child.utf8_text(source).unwrap_or("").to_string();
                let full_path = if prefix.is_empty() { node_text.clone() } else { format!("{}::{}", prefix, node_text) };
                result.relations.push(Relation {
                    from: "".to_string(),
                    to: node_text,
                    rel_type: "imports".to_string(),
                    properties: {
                        let mut p = HashMap::new();
                        p.insert("module".to_string(), full_path);
                        p
                    },
                });
            } else if k == "scoped_identifier" || k == "scoped_use_list" {
                self.process_use_node(child, prefix, source, result);
            }
        }
    }

    fn extract_call_expression<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        if node.child_count() == 0 { return; }
        if let Some(func_node) = node.child(0) {
            match func_node.kind() {
                "identifier" => {
                    let name = func_node.utf8_text(source).unwrap_or("").to_string();
                    result.relations.push(Relation {
                        from: "".to_string(),
                        to: name,
                        rel_type: "calls".to_string(),
                        properties: HashMap::new(),
                    });
                }
                "field_expression" => {
                    if let Some(field_id) = self.find_child_by_type(func_node, "field_identifier") {
                        let name = field_id.utf8_text(source).unwrap_or("").to_string();
                        let receiver = if func_node.child_count() > 0 {
                            if let Some(obj) = func_node.child(0) {
                                obj.utf8_text(source).unwrap_or("").to_string()
                            } else { "".to_string() }
                        } else { "".to_string() };
                        let mut props = HashMap::new();
                        if !receiver.is_empty() {
                            props.insert("receiver".to_string(), receiver);
                        }
                        result.relations.push(Relation {
                            from: "".to_string(),
                            to: name,
                            rel_type: "calls".to_string(),
                            properties: props,
                        });
                    }
                }
                "scoped_identifier" => {
                    let full = func_node.utf8_text(source).unwrap_or("").to_string();
                    let parts: Vec<&str> = full.split("::").collect();
                    if let Some(&name) = parts.last() {
                        let receiver = if parts.len() > 1 { parts[..parts.len()-1].join("::") } else { "".to_string() };
                        let mut props = HashMap::new();
                        if !receiver.is_empty() {
                            props.insert("receiver".to_string(), receiver);
                        }
                        result.relations.push(Relation {
                            from: "".to_string(),
                            to: name.to_string(),
                            rel_type: "calls".to_string(),
                            properties: props,
                        });
                    }
                }
                _ => {}
            }
        }
        
        self.walk_for_calls(node, source, result, true);
    }

    fn extract_method_call<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = self.find_child_by_type(node, "field_identifier") {
            let name = name_node.utf8_text(source).unwrap_or("").to_string();
            let mut receiver = "".to_string();
            if let Some(recv_node) = node.child(0) {
                if recv_node.kind() == "identifier" || recv_node.kind() == "self" {
                    receiver = recv_node.utf8_text(source).unwrap_or("").to_string();
                }
            }
            let mut props = HashMap::new();
            if !receiver.is_empty() {
                props.insert("receiver".to_string(), receiver);
            }
            result.relations.push(Relation {
                from: "".to_string(),
                to: name,
                rel_type: "calls".to_string(),
                properties: props,
            });
        }
        self.walk_for_calls(node, source, result, false);
    }

    fn extract_macro_invocation<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        if let Some(name_node) = self.find_child_by_type(node, "identifier") {
            let name = format!("{}!", name_node.utf8_text(source).unwrap_or(""));
            result.relations.push(Relation {
                from: "".to_string(),
                to: name,
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
            });
        }
    }

    fn walk_for_calls<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult, skip_first: bool) {
        let mut cursor = node.walk();
        let mut children: Vec<Node> = node.children(&mut cursor).collect();
        if skip_first && !children.is_empty() {
            children.remove(0);
        }
        for child in children {
            match child.kind() {
                "call_expression" => self.extract_call_expression(child, source, result),
                "method_call_expression" => self.extract_method_call(child, source, result),
                "macro_invocation" => self.extract_macro_invocation(child, source, result),
                _ => self.walk_for_calls(child, source, result, false),
            }
        }
    }
}

impl Parser for RustParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let tree = match parse_with_wasm_safe("rust", self.wasm_bytes, content) {
            Some(t) => t,
            None => return ExtractionResult { project_slug: None, symbols: Vec::new(), relations: Vec::new() },
        };
        
        let mut result = ExtractionResult {
            project_slug: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };
        
        self.walk(tree.root_node(), content.as_bytes(), &mut result, "");
        
        result
    }
}
