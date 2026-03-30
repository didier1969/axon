use super::{ExtractionResult, Parser, Relation, Symbol, parse_with_wasm_safe};
use std::collections::HashMap;
use tree_sitter::Node;

const OTP_ENTRY_POINTS: &[&str] = &[
    "handle_call", "handle_cast", "handle_info", "handle_continue", "init", "start_link",
];

const IMPORT_DIRECTIVES: &[&str] = &["alias", "import", "use", "require"];

pub struct ElixirParser {
    wasm_bytes: &'static [u8],
}

impl Default for ElixirParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ElixirParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-elixir.wasm"),
        }
    }

    fn walk<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        pending_attrs: &mut Vec<String>,
    ) {
        let mut child_cursor = node.walk();
        let mut current_attrs = pending_attrs.clone();
        pending_attrs.clear();

        for child in node.named_children(&mut child_cursor) {
            match child.kind() {
                "call" => {
                    Self::handle_call_node(
                        child,
                        source_bytes,
                        content,
                        result,
                        module_name,
                        &current_attrs,
                    );
                    current_attrs.clear();
                }
                "unary_operator" => {
                    if let Some(attr_name) = Self::extract_attribute_name(child, source_bytes) {
                        current_attrs.push(attr_name);
                    }
                    Self::handle_behaviour_attribute(child, source_bytes, result, module_name);
                }
                _ => {
                    Self::walk(
                        child,
                        source_bytes,
                        content,
                        result,
                        module_name,
                        &mut current_attrs,
                    );
                    current_attrs.clear();
                }
            }
        }
    }

    fn handle_call_node<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        pending_attrs: &[String],
    ) {
        if let Some(identifier) = Self::call_identifier(node, source_bytes) {
            match identifier.as_str() {
                "defmodule" => Self::extract_module(node, source_bytes, content, result, pending_attrs),
                "def" | "defp" => Self::extract_function(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    pending_attrs,
                    identifier.as_str(),
                ),
                "defmacro" | "defmacrop" => Self::extract_macro(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    pending_attrs,
                    identifier.as_str(),
                ),
                x if IMPORT_DIRECTIVES.contains(&x) => {
                    Self::extract_import_directive(node, source_bytes, result, x, module_name)
                }
                _ => Self::extract_generic_call(node, source_bytes, result, module_name),
            }
        } else {
            Self::extract_generic_call(node, source_bytes, result, module_name);
        }
    }

    fn extract_module<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        content: &str,
        result: &mut ExtractionResult,
        _decorators: &[String],
    ) {
        let args = Self::find_child_by_type(node, "arguments");
        let mut new_module_name = String::new();

        if let Some(args_node) = args {
            if let Some(alias_node) = Self::find_child_by_type(args_node, "alias") {
                new_module_name = alias_node.utf8_text(source_bytes).unwrap_or("").to_string();
            }
        }

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        result.symbols.push(Symbol {
            name: new_module_name.clone(),
            kind: "module".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: new_module_name.ends_with("Test"),
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::walk(
                do_block,
                source_bytes,
                content,
                result,
                &new_module_name,
                &mut Vec::new(),
            );
        }
    }

    fn extract_function<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        _decorators: &[String],
        def_type: &str,
    ) {
        let func_name = match Self::extract_def_name(node, source_bytes) {
            Some(name) => name,
            None => return,
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let is_otp_entry = OTP_ENTRY_POINTS.contains(&func_name.as_str());

        let full_name = if module_name.is_empty() {
            func_name.clone()
        } else {
            format!("{}.{}", module_name, func_name)
        };

        let mut properties = HashMap::new();
        
        let node_content = node.utf8_text(source_bytes).unwrap_or("");
        let is_nif = node_content.contains(":erlang.nif_error") || node_content.contains(":nif_not_loaded");
        if node_content.contains("load_nif") {
            properties.insert("nif_loader".to_string(), "true".to_string());
        }

        if node_content.contains(":erlang.nif_error(:nif_not_loaded)") {
            result.relations.push(Relation {
                from: module_name.to_string(),
                to: func_name.clone(),
                rel_type: "calls_nif".to_string(),
                properties: std::collections::HashMap::new(),
            });
        }

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: "function".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: is_otp_entry || is_nif,
            is_public: def_type == "def",
            tested: func_name.starts_with("test_") || module_name.ends_with("Test"),
            is_nif,
            is_unsafe: false,
            properties,
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::extract_calls_from_block(do_block, source_bytes, result, &full_name);
        }
    }

    fn extract_macro<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        _decorators: &[String],
        def_type: &str,
    ) {
        let macro_name = match Self::extract_def_name(node, source_bytes) {
            Some(name) => name,
            None => return,
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let full_name = if module_name.is_empty() {
            macro_name.clone()
        } else {
            format!("{}.{}", module_name, macro_name)
        };

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: "macro".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: false,
            is_public: def_type == "defmacro",
            tested: macro_name.starts_with("test_") || module_name.ends_with("Test"),
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::extract_calls_from_block(do_block, source_bytes, result, &full_name);
        }
    }

    fn extract_import_directive<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        directive: &str,
        module_name: &str,
    ) {
        let args = Self::find_child_by_type(node, "arguments");
        if args.is_none() {
            return;
        }
        let args_node = args.unwrap();

        let mut module_alias = String::new();
        let mut cursor = args_node.walk();
        for child in args_node.named_children(&mut cursor) {
            if child.kind() == "alias" {
                module_alias = child.utf8_text(source_bytes).unwrap_or("").to_string();
                break;
            }
        }

        if module_alias.is_empty() {
            return;
        }

        if directive == "use" {
            result.relations.push(Relation {
                from: module_name.to_string(),
                to: module_alias.clone(),
                rel_type: "uses".to_string(),
                properties: HashMap::new(),
            });
        }
    }

    fn extract_calls_from_block<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        module_name: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::call_identifier(child, source_bytes) {
                    if ["def", "defp", "defmodule", "defmacro", "defmacrop", "defstruct"]
                        .contains(&ident.as_str())
                    {
                        continue;
                    }
                    if IMPORT_DIRECTIVES.contains(&ident.as_str()) {
                        continue;
                    }
                }
                Self::extract_generic_call(child, source_bytes, result, module_name);
            } else {
                Self::extract_calls_from_block(child, source_bytes, result, module_name);
            }
        }
    }

    fn extract_generic_call<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller_name: &str,
    ) {
        if let Some(dot_node) = Self::find_child_by_type(node, "dot") {
            let mut receiver = String::new();
            let mut func_name = String::new();
            
            let mut cursor = dot_node.walk();
            for child in dot_node.named_children(&mut cursor) {
                if child.kind() == "alias" {
                    receiver = child.utf8_text(source_bytes).unwrap_or("").to_string();
                } else if child.kind() == "identifier" {
                    func_name = child.utf8_text(source_bytes).unwrap_or("").to_string();
                }
            }

            if !func_name.is_empty() {
                let mut target_module = receiver.clone();
                let mut rel_type = "CALLS".to_string();

                let is_genserver = receiver == "GenServer" && (func_name == "call" || func_name == "cast");
                if is_genserver {
                    rel_type = "CALLS_OTP".to_string();
                    // Attempt to extract the target module from the first argument
                    if let Some(args_node) = Self::find_child_by_type(node, "arguments") {
                        let mut arg_cursor = args_node.walk();
                        for arg_child in args_node.named_children(&mut arg_cursor) {
                            if arg_child.kind() == "alias" {
                                target_module = arg_child.utf8_text(source_bytes).unwrap_or("").to_string();
                                break;
                            }
                        }
                    }
                }

                // Skip generic calls to standard library unless it's an OTP boundary we want to track
                if receiver != "Enum" && receiver != "String" && receiver != "Map" && receiver != "List" {
                    let mut props = HashMap::new();
                    if is_genserver {
                        props.insert("otp_boundary".to_string(), "true".to_string());
                        props.insert("call_type".to_string(), func_name.clone());
                    }

                    result.relations.push(Relation {
                        from: caller_name.to_string(),
                        to: target_module,
                        rel_type,
                        properties: props,
                    });
                }
            }
        } else if let Some(func_name) = Self::call_identifier(node, source_bytes) {
            let skip = [
                "def",
                "defp",
                "defmodule",
                "defmacro",
                "defmacrop",
                "defstruct",
                "alias",
                "import",
                "use",
                "require",
            ];

            if skip.contains(&func_name.as_str()) {
                return;
            }

            let target = if let Some((module_name, _)) = caller_name.rsplit_once('.') {
                format!("{}.{}", module_name, func_name)
            } else {
                func_name
            };

            result.relations.push(Relation {
                from: caller_name.to_string(),
                to: target,
                rel_type: "CALLS".to_string(),
                properties: HashMap::new(),
            });
        }
    }

    fn extract_attribute_name<'a>(node: Node<'a>, source_bytes: &[u8]) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::call_identifier(child, source_bytes) {
                    return Some(format!("@{}", ident));
                }
            }
        }
        None
    }

    fn handle_behaviour_attribute<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        module_name: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::call_identifier(child, source_bytes) {
                    if ident == "behaviour" {
                        if let Some(args) = Self::find_child_by_type(child, "arguments") {
                            if let Some(alias) = Self::find_child_by_type(args, "alias") {
                                let behaviour_name = alias.utf8_text(source_bytes).unwrap_or("").to_string();
                                result.relations.push(Relation {
                                    from: module_name.to_string(),
                                    to: behaviour_name,
                                    rel_type: "implements".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    fn call_identifier<'a>(node: Node<'a>, source_bytes: &[u8]) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                return Some(child.utf8_text(source_bytes).unwrap_or("").to_string());
            }
            if child.kind() == "dot" {
                return None;
            }
        }
        None
    }

    fn find_child_by_type<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        let res = node.named_children(&mut cursor).find(|&child| child.kind() == kind);
        res
    }

    fn extract_def_name<'a>(node: Node<'a>, source_bytes: &[u8]) -> Option<String> {
        let args = Self::find_child_by_type(node, "arguments")?;
        let mut cursor = args.walk();
        for child in args.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::find_child_by_type(child, "identifier") {
                    return Some(ident.utf8_text(source_bytes).unwrap_or("").to_string());
                }
            } else if child.kind() == "identifier" || child.kind() == "alias" {
                return Some(child.utf8_text(source_bytes).unwrap_or("").to_string());
            }
        }
        None
    }
}

impl Parser for ElixirParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let tree = match parse_with_wasm_safe("elixir", self.wasm_bytes, content) {
            Some(t) => t,
            None => return ExtractionResult { project_slug: None, symbols: Vec::new(), relations: Vec::new() },
        };

        let mut result = ExtractionResult {
            project_slug: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        let source_bytes = content.as_bytes();
        Self::walk(
            tree.root_node(),
            source_bytes,
            content,
            &mut result,
            "",
            &mut Vec::new(),
        );

        result
    }
}

#[cfg(test)]
mod tests {
    use super::ElixirParser;
    use crate::parser::Parser;

    #[test]
    fn test_elixir_parser_tracks_local_function_calls_with_function_scope() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def trigger_scan do
            parse_batch()
          end

          defp parse_batch do
            :ok
          end
        end
        "#;

        let result = parser.parse(content);

        assert!(result.symbols.iter().any(|sym| sym.name == "Axon.Sample.trigger_scan"));
        assert!(result.symbols.iter().any(|sym| sym.name == "Axon.Sample.parse_batch"));
        assert!(result.relations.iter().any(|rel|
            rel.from == "Axon.Sample.trigger_scan"
                && rel.to == "Axon.Sample.parse_batch"
                && rel.rel_type == "CALLS"
        ));
    }
}
