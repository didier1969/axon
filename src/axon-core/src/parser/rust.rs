use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
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
    fn top_level_block_split_lines<'a>(&self, block: Node<'a>) -> Vec<usize> {
        let mut cursor = block.walk();
        block
            .named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-rust.wasm"),
        }
    }

    fn find_child_by_type<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let res = node
            .children(&mut cursor)
            .find(|&child| child.kind() == kind);
        res
    }

    fn has_visibility(&self, node: Node) -> bool {
        self.find_child_by_type(node, "visibility_modifier")
            .is_some()
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity: base 1 +
    /// one per decision point (if/if-let/while/while-let/for/match arm)
    /// anywhere in the subtree, EXCEPT inside a nested `function_item` (which
    /// gets its own separate count when `extract_function` visits it in the
    /// normal `walk`). Closures have no Symbol of their own in this parser,
    /// so their branches count toward the enclosing function — intentional.
    /// First-pass scope: `&&`/`||`/`?` short-circuit operators are NOT yet
    /// counted (deferred — the four decision-construct kinds already give a
    /// real, non-arbitrary signal without guessing at less certain grammar
    /// node shapes).
    fn count_branches(&self, node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "function_item" {
                continue;
            }
            if matches!(
                child.kind(),
                "if_expression"
                    | "if_let_expression"
                    | "while_expression"
                    | "while_let_expression"
                    | "for_expression"
                    | "match_arm"
            ) {
                count += 1;
            }
            count += self.count_branches(child);
        }
        count
    }

    fn walk<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        class_name: &str,
        current_function: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "function_item" => self.extract_function(child, source, result, class_name),
                "function_signature_item" => {
                    self.extract_function_signature(child, source, result, class_name)
                }
                "struct_item" => self.extract_struct(child, source, result),
                "enum_item" => self.extract_enum(child, source, result),
                "trait_item" => self.extract_trait(child, source, result),
                "impl_item" => self.extract_impl(child, source, result),
                "mod_item" => self.extract_mod(child, source, result),
                "type_item" => self.extract_type_alias(child, source, result),
                "use_declaration" => self.extract_use(child, source, result),
                "call_expression" => {
                    self.extract_call_expression(child, source, result, current_function)
                }
                "method_call_expression" => {
                    self.extract_method_call(child, source, result, current_function)
                }
                "macro_invocation" => {
                    self.extract_macro_invocation(child, source, result, current_function)
                }
                "line_comment" | "block_comment" => self.extract_comment(child, source, result),
                _ => self.walk(child, source, result, class_name, current_function),
            }
        }
    }

    fn extract_comment<'a>(&self, node: Node<'a>, source: &[u8], result: &mut ExtractionResult) {
        if let Ok(text) = node.utf8_text(source) {
            if text.contains("TODO") || text.contains("FIXME") {
                let kind = if text.contains("TODO") {
                    "TODO"
                } else {
                    "FIXME"
                };
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

    fn extract_function<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        class_name: &str,
    ) {
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
        let kind = if class_name.is_empty() {
            "function"
        } else {
            "method"
        };

        let lower_name = name.to_lowercase();
        let is_entry = is_extern_c
            || (is_pub
                && (lower_name.contains("main")
                    || lower_name.contains("handler")
                    || lower_name.contains("nif_")));

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

        if let Some(block) = self.find_child_by_type(node, "block") {
            if let Ok(body_text) = block.utf8_text(source) {
                if body_text.contains(".unwrap()")
                    || body_text.contains("panic!(")
                    || body_text.contains(".expect(")
                {
                    props.insert("can_panic".to_string(), "true".to_string());
                }
            }
            props.insert(
                "header_end_line".to_string(),
                block.start_position().row.saturating_add(1).to_string(),
            );
            props.insert(
                "body_start_line".to_string(),
                block.start_position().row.saturating_add(1).to_string(),
            );
            props.insert(
                "body_end_line".to_string(),
                block.end_position().row.saturating_add(1).to_string(),
            );
            let split_lines = self.top_level_block_split_lines(block);
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
            // REQ-AXO-902185 (god-objects) — base 1 + decision points in the body.
            let complexity = 1 + self.count_branches(block);
            props.insert("cyclomatic_complexity".to_string(), complexity.to_string());
        }

        // REQ-AXO-91504 — keep the function name alive after the Symbol
        // push so we can pass it as `current_function` when we recurse into
        // the body. The body scope is `name` for free fns, `Class::name`
        // for methods (graph_ingestion may strip the prefix during
        // resolution but the qualified form disambiguates collisions).
        let scope = if class_name.is_empty() {
            name.clone()
        } else {
            format!("{}::{}", class_name, name)
        };

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
            self.walk(block, source, result, class_name, &scope);
        }
    }

    fn extract_function_signature<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        class_name: &str,
    ) {
        let name_node = self.find_child_by_type(node, "identifier");
        let name = if let Some(n) = name_node {
            n.utf8_text(source).unwrap_or("").to_string()
        } else {
            return;
        };

        let kind = if class_name.is_empty() {
            "function"
        } else {
            "method"
        };
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
            self.walk(decl_list, source, result, &name, "");
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
        // REQ-AXO-901827 (MIL-AXO-032) — emit a Symbol for the impl
        // block itself so the IST can resolve `impl Foo` and
        // `impl Trait for Foo` blocks. Without this, methods orphan
        // their container in PG. Canonical name :
        //   * `impl Foo` → "Foo"
        //   * `impl Trait for Foo` → "Trait for Foo"
        // so the IST can disambiguate inherent vs trait impls.
        let mut impl_symbol_name: Option<String> = None;

        if has_for && type_nodes.len() >= 2 {
            let trait_name = type_nodes[0].utf8_text(source).unwrap_or("").to_string();
            struct_name = type_nodes[1].utf8_text(source).unwrap_or("").to_string();

            result.relations.push(Relation {
                from: struct_name.clone(),
                to: trait_name.clone(),
                rel_type: "implements".to_string(),
                properties: HashMap::new(),
            });

            impl_symbol_name = Some(format!("{} for {}", trait_name, struct_name));
        } else if type_nodes.len() == 1 {
            struct_name = type_nodes[0].utf8_text(source).unwrap_or("").to_string();
            impl_symbol_name = Some(struct_name.clone());
        }

        if let Some(name) = impl_symbol_name {
            result.symbols.push(Symbol {
                name,
                kind: "impl".to_string(),
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        if let Some(decl_list) = self.find_child_by_type(node, "declaration_list") {
            self.walk(decl_list, source, result, &struct_name, "");
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
            self.walk(decl_list, source, result, "", "");
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
            if k == "scoped_identifier"
                || k == "scoped_use_list"
                || k == "identifier"
                || k == "use_wildcard"
            {
                self.process_use_node(child, "", source, result);
                return;
            }
        }
    }

    fn process_use_node<'a>(
        &self,
        node: Node<'a>,
        prefix: &str,
        source: &[u8],
        result: &mut ExtractionResult,
    ) {
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
                let full_path = if prefix.is_empty() {
                    node_text.clone()
                } else {
                    format!("{}::{}", prefix, node_text)
                };
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

    fn process_use_list<'a>(
        &self,
        node: Node<'a>,
        prefix: &str,
        source: &[u8],
        result: &mut ExtractionResult,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let k = child.kind();
            if k == "identifier" {
                let node_text = child.utf8_text(source).unwrap_or("").to_string();
                let full_path = if prefix.is_empty() {
                    node_text.clone()
                } else {
                    format!("{}::{}", prefix, node_text)
                };
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

    fn extract_call_expression<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        current_function: &str,
    ) {
        if node.child_count() == 0 {
            return;
        }
        if let Some(func_node) = node.child(0) {
            match func_node.kind() {
                "identifier" => {
                    let name = func_node.utf8_text(source).unwrap_or("").to_string();
                    result.relations.push(Relation {
                        from: current_function.to_string(),
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
                            } else {
                                "".to_string()
                            }
                        } else {
                            "".to_string()
                        };
                        let mut props = HashMap::new();
                        if !receiver.is_empty() {
                            props.insert("receiver".to_string(), receiver);
                        }
                        result.relations.push(Relation {
                            from: current_function.to_string(),
                            to: name,
                            rel_type: "calls".to_string(),
                            properties: props,
                        });
                    }
                    // REQ-AXO-902195 — the receiver may itself be a call (`foo(...).bar()`).
                    // The walk_for_calls(skip_first=true) below drops child(0) (this whole
                    // field_expression), losing the inner call. Walk the receiver here to
                    // recover it (skip_first=false; the field_identifier is not a call node).
                    self.walk_for_calls(func_node, source, result, false, current_function);
                }
                "scoped_identifier" => {
                    let full = func_node.utf8_text(source).unwrap_or("").to_string();
                    let parts: Vec<&str> = full.split("::").collect();
                    if let Some(&name) = parts.last() {
                        let receiver = if parts.len() > 1 {
                            parts[..parts.len() - 1].join("::")
                        } else {
                            "".to_string()
                        };
                        let mut props = HashMap::new();
                        if !receiver.is_empty() {
                            props.insert("receiver".to_string(), receiver);
                        }
                        result.relations.push(Relation {
                            from: current_function.to_string(),
                            to: name.to_string(),
                            rel_type: "calls".to_string(),
                            properties: props,
                        });
                    }
                }
                _ => {}
            }
        }

        self.walk_for_calls(node, source, result, true, current_function);
    }

    fn extract_method_call<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        current_function: &str,
    ) {
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
                from: current_function.to_string(),
                to: name,
                rel_type: "calls".to_string(),
                properties: props,
            });
        }
        self.walk_for_calls(node, source, result, false, current_function);
    }

    fn extract_macro_invocation<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        current_function: &str,
    ) {
        if let Some(name_node) = self.find_child_by_type(node, "identifier") {
            let name = format!("{}!", name_node.utf8_text(source).unwrap_or(""));
            result.relations.push(Relation {
                from: current_function.to_string(),
                to: name,
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
            });
        }
        // CPT-AXO-90050 — capture function calls NESTED inside the macro's
        // arguments. tree-sitter does not expand macros, so the args are a raw
        // `token_tree` (no structured `call_expression`). Heuristic: an
        // `identifier` immediately followed by a `token_tree` is a call site
        // (e.g. `assert!(commit_message_is_refactor(x))` → calls
        // commit_message_is_refactor). Without this, the test→code-under-test
        // CALLS edges were dropped (tests call the SUT inside assert!/assert_eq!),
        // making test_impact/tests_for impossible and the audit over-report dead
        // code (REQ-901958, validated on OPV: 85% false positives).
        if let Some(tt) = self.find_child_by_type(node, "token_tree") {
            self.extract_calls_in_token_tree(tt, source, result, current_function);
        }
    }

    /// CPT-AXO-90050 — heuristic call extraction inside an unexpanded macro
    /// `token_tree`: `identifier` followed by a `token_tree` ⇒ a call; recurse
    /// into nested token_trees (e.g. `assert_eq!(a, foo(bar(x)))`).
    fn extract_calls_in_token_tree<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        current_function: &str,
    ) {
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        for (idx, child) in children.iter().enumerate() {
            match child.kind() {
                "identifier" => {
                    if children
                        .get(idx + 1)
                        .map(|n| n.kind() == "token_tree")
                        .unwrap_or(false)
                    {
                        if let Ok(name) = child.utf8_text(source) {
                            result.relations.push(Relation {
                                from: current_function.to_string(),
                                to: name.to_string(),
                                rel_type: "calls".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
                "token_tree" => {
                    self.extract_calls_in_token_tree(*child, source, result, current_function)
                }
                _ => {}
            }
        }
    }

    fn walk_for_calls<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        result: &mut ExtractionResult,
        skip_first: bool,
        current_function: &str,
    ) {
        let mut cursor = node.walk();
        let mut children: Vec<Node> = node.children(&mut cursor).collect();
        if skip_first && !children.is_empty() {
            children.remove(0);
        }
        for child in children {
            match child.kind() {
                "call_expression" => {
                    self.extract_call_expression(child, source, result, current_function)
                }
                "method_call_expression" => {
                    self.extract_method_call(child, source, result, current_function)
                }
                "macro_invocation" => {
                    self.extract_macro_invocation(child, source, result, current_function)
                }
                _ => self.walk_for_calls(child, source, result, false, current_function),
            }
        }
    }
}

impl Parser for RustParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let tree = match parse_with_wasm_safe("rust", self.wasm_bytes, content) {
            Some(t) => t,
            None => {
                return ExtractionResult {
                    project_code: None,
                    symbols: Vec::new(),
                    relations: Vec::new(),
                }
            }
        };

        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        self.walk(tree.root_node(), content.as_bytes(), &mut result, "", "");

        result
    }
}

#[cfg(test)]
mod tests {
    //! REQ-AXO-91504 regression tests — verify that `from:` on extracted
    //! `calls` relations carries the enclosing function name (previously
    //! always empty, which made the IST call graph for Rust = 0 edges).
    use super::*;
    use crate::parser::Parser;

    fn parser() -> RustParser {
        // The Rust grammar wasm is loaded from the same place the prod
        // pipeline uses. If absent in the test environment, the
        // RustParser falls back to an empty result — which would mask the
        // bug, so we skip the test rather than silently green-light it.
        RustParser::new()
    }

    fn calls(rel: &[Relation]) -> Vec<&Relation> {
        rel.iter().filter(|r| r.rel_type == "calls").collect()
    }

    #[test]
    fn free_function_calls_carry_caller_name() {
        let p = parser();
        let result = p.parse("fn foo() { bar(); baz(); }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let cs = calls(&result.relations);
        assert!(cs.len() >= 2, "expected >= 2 calls, got {}", cs.len());
        for c in &cs {
            assert_eq!(
                c.from, "foo",
                "every call inside `fn foo` must have from=foo, got {:?}",
                c
            );
        }
        let targets: Vec<&str> = cs.iter().map(|c| c.to.as_str()).collect();
        assert!(targets.contains(&"bar"));
        assert!(targets.contains(&"baz"));
    }

    #[test]
    fn chained_call_on_call_result_captures_inner_call() {
        // REQ-AXO-902195 — `foo(...).unwrap()` must yield a CALLS edge to `foo`, not only to
        // `unwrap`. Regression for walk_for_calls(skip_first) dropping the field_expression
        // receiver (the inner call), which silently under-counted callers → false covered/wiring.
        let p = parser();
        let result = p.parse("fn f() { let _ = g(1).unwrap(); }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let targets: Vec<&str> = calls(&result.relations)
            .iter()
            .map(|c| c.to.as_str())
            .collect();
        assert!(
            targets.contains(&"g"),
            "inner call `g` must be captured, got {:?}",
            targets
        );
        assert!(
            targets.contains(&"unwrap"),
            "chained `unwrap` also captured, got {:?}",
            targets
        );
    }

    #[test]
    fn method_calls_carry_qualified_caller_name() {
        let p = parser();
        let result = p.parse("impl Foo { fn bar() { qux(); } }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let cs = calls(&result.relations);
        assert!(!cs.is_empty(), "expected call to qux");
        let qux: Vec<&&Relation> = cs.iter().filter(|c| c.to == "qux").collect();
        assert_eq!(qux.len(), 1);
        assert_eq!(
            qux[0].from, "Foo::bar",
            "method call must carry `Class::method` qualified caller"
        );
    }

    #[test]
    fn macro_invocations_carry_caller_name() {
        let p = parser();
        let result = p.parse("fn alpha() { println!(\"hi\"); }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let cs = calls(&result.relations);
        let println_call: Vec<&&Relation> = cs.iter().filter(|c| c.to == "println!").collect();
        assert_eq!(println_call.len(), 1);
        assert_eq!(println_call[0].from, "alpha");
    }

    #[test]
    fn calls_nested_in_macro_args_are_captured() {
        // CPT-AXO-90050 — the call to the code-under-test lives INSIDE the
        // assert! macro; without token_tree recursion only `assert!` was caught,
        // dropping the test→SUT edge. Nested calls (assert_eq!(a, foo(bar(x))))
        // must surface foo AND bar.
        let p = parser();
        let result = p.parse(
            "fn t() { assert!(commit_message_is_refactor(\"refactor: x\")); assert_eq!(a, foo(bar(z))); }",
        );
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let targets: Vec<&str> = calls(&result.relations).iter().map(|c| c.to.as_str()).collect();
        assert!(targets.contains(&"commit_message_is_refactor"), "got {:?}", targets);
        assert!(targets.contains(&"foo"), "got {:?}", targets);
        assert!(targets.contains(&"bar"), "got {:?}", targets);
        assert!(targets.contains(&"assert!"), "macro name still captured: {:?}", targets);
    }

    #[test]
    fn top_level_calls_outside_function_have_empty_from() {
        // Top-level calls outside any fn (rare but possible e.g. macros)
        // should keep `from=""` — current_function is empty at module
        // scope. This pins that semantics so future refactors don't
        // accidentally hoist a wrong scope.
        let p = parser();
        let result = p.parse("const X: u32 = { foo(); 42 };");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        for c in calls(&result.relations) {
            assert_eq!(
                c.from, "",
                "top-level call should have empty caller, got {:?}",
                c
            );
        }
    }

    // REQ-AXO-901827 (MIL-AXO-032) — diagnostic regression : on production
    // PG, AXO project carries function:136 + module:14 and ZERO
    // struct/trait/impl/enum despite the parser code visibly calling
    // extract_struct/extract_trait/extract_impl/extract_enum. These tests
    // pin the parser-level emission contract for top-level type
    // declarations. If they fail, the bug is in the parser (e.g. wasm
    // grammar node kind mismatch). If they pass, the bug is downstream
    // (chunking / bulk_writer / DB schema).
    #[test]
    fn top_level_struct_emits_struct_symbol() {
        let p = parser();
        let result = p.parse("pub struct Foo { x: u32 }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let structs: Vec<&Symbol> = result
            .symbols
            .iter()
            .filter(|s| s.kind == "struct")
            .collect();
        assert_eq!(
            structs.len(),
            1,
            "expected 1 struct symbol, got {} : {:?}",
            structs.len(),
            result.symbols
        );
        assert_eq!(structs[0].name, "Foo");
        assert!(structs[0].is_public, "pub struct must mark is_public");
    }

    #[test]
    fn top_level_trait_emits_interface_symbol() {
        let p = parser();
        let result = p.parse("pub trait Foo { fn bar(&self); }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let traits: Vec<&Symbol> = result
            .symbols
            .iter()
            .filter(|s| s.kind == "interface")
            .collect();
        assert_eq!(
            traits.len(),
            1,
            "expected 1 interface (=trait) symbol, got {} : {:?}",
            traits.len(),
            result.symbols
        );
        assert_eq!(traits[0].name, "Foo");
    }

    #[test]
    fn top_level_enum_emits_enum_symbol() {
        let p = parser();
        let result = p.parse("pub enum Color { Red, Green, Blue }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let enums: Vec<&Symbol> = result.symbols.iter().filter(|s| s.kind == "enum").collect();
        assert_eq!(
            enums.len(),
            1,
            "expected 1 enum symbol, got {} : {:?}",
            enums.len(),
            result.symbols
        );
        assert_eq!(enums[0].name, "Color");
    }

    #[test]
    fn impl_block_emits_class_symbol_and_methods() {
        // REQ-AXO-901827 root cause #1 candidate : extract_impl currently
        // does NOT push a Symbol for the impl block itself — it only
        // walks the decl_list to extract nested methods. The impl block
        // is invisible in the IST, which is why `inspect Foo` cannot
        // reach `impl Foo`. Pin the desired behavior : impl block emits
        // an `impl` Symbol with name=type identifier, and methods inside
        // are emitted as `method` (kind already correct).
        let p = parser();
        let result = p.parse("impl Foo { fn bar(&self) {} fn baz(&self) {} }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let impls: Vec<&Symbol> = result.symbols.iter().filter(|s| s.kind == "impl").collect();
        assert_eq!(
            impls.len(),
            1,
            "expected 1 impl symbol, got {} : {:?}",
            impls.len(),
            result.symbols
        );
        assert_eq!(impls[0].name, "Foo");
        let methods: Vec<&Symbol> = result
            .symbols
            .iter()
            .filter(|s| s.kind == "method")
            .collect();
        assert_eq!(
            methods.len(),
            2,
            "expected 2 methods inside impl, got {}",
            methods.len()
        );
    }

    #[test]
    fn impl_trait_for_struct_emits_impl_symbol_with_qualified_name() {
        let p = parser();
        let result = p.parse("impl Display for Foo { fn fmt(&self, f: &mut Formatter) {} }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let impls: Vec<&Symbol> = result.symbols.iter().filter(|s| s.kind == "impl").collect();
        assert_eq!(
            impls.len(),
            1,
            "expected 1 impl symbol, got {} : {:?}",
            impls.len(),
            result.symbols
        );
        // Canonical name = "Display for Foo" so callers can disambiguate
        // bare `impl Foo` from `impl Trait for Foo`. The `implements`
        // relation already carries the from/to detail.
        assert_eq!(impls[0].name, "Display for Foo");
    }

    // REQ-AXO-902185 (god-objects) — cyclomatic complexity: base 1 + one per
    // decision point (if/if-let/while/while-let/for/match arm).
    #[test]
    fn simple_function_has_complexity_one() {
        let p = parser();
        let result = p.parse("fn f() { let x = 1; let y = x + 1; }");
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(f.properties.get("cyclomatic_complexity").map(String::as_str), Some("1"));
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let p = parser();
        // 1 (base) + if + while + for + 2 match arms = 6.
        let result = p.parse(
            "fn f(x: i32) -> i32 { \
                if x > 0 { return 1; } \
                while x > 0 { break; } \
                for i in 0..x {} \
                match x { 0 => 1, _ => 2 } \
             }",
        );
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
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
    fn nested_function_gets_its_own_complexity_not_added_to_parent() {
        let p = parser();
        // outer: base 1 + 1 if = 2. inner (nested fn): base 1 + 1 if = 2.
        // The nested fn's branch must NOT inflate outer's count.
        let result = p.parse(
            "fn outer(x: i32) { \
                if x > 0 {} \
                fn inner(y: i32) { if y > 0 {} } \
             }",
        );
        if result.symbols.is_empty() {
            eprintln!("rust wasm grammar unavailable, skipping");
            return;
        }
        let outer = result.symbols.iter().find(|s| s.name == "outer").unwrap();
        let inner = result.symbols.iter().find(|s| s.name == "inner").unwrap();
        assert_eq!(outer.properties.get("cyclomatic_complexity").map(String::as_str), Some("2"));
        assert_eq!(inner.properties.get("cyclomatic_complexity").map(String::as_str), Some("2"));
    }
}
