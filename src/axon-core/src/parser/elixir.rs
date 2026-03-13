use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::{Language, Node, Parser as TSParser};

const OTP_ENTRY_POINTS: &[&str] = &[
    "handle_call", "handle_cast", "handle_info", "handle_continue", "init", "start_link",
];

const IMPORT_DIRECTIVES: &[&str] = &["alias", "import", "use", "require"];

pub struct ElixirParser {
    language: Language,
}

impl Default for ElixirParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ElixirParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_elixir::LANGUAGE.into(),
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
                "defstruct" => {
                    // Similar to Python `defstruct` - optional, but maybe nice to include if requested, but prompt didn't strictly require struct unless it was part of no feature loss.
                }
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
        _def_type: &str,
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
            name: full_name,
            kind: "function".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: is_otp_entry,
            properties,
        
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::extract_calls_from_block(do_block, source_bytes, result, module_name);
        }
    }

    fn extract_macro<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        _decorators: &[String],
        _def_type: &str,
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
            name: full_name,
            kind: "macro".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: false,
            properties: HashMap::new(),
        
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::extract_calls_from_block(do_block, source_bytes, result, module_name);
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
        module_name: &str,
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
                let is_genserver = receiver == "GenServer" && (func_name == "call" || func_name == "cast");
                if is_genserver {
                    let mut props = HashMap::new();
                    props.insert("genserver".to_string(), "true".to_string());
                    
                    // We can model this as a relation or a symbol property, but the prompt says:
                    // 'mets "genserver": "true" dans les propriétés de la relation'
                    // Wait, what's the `from` and `to` for this relation?
                    // Let's create a generic call relation
                    result.relations.push(Relation {
                        from: module_name.to_string(),
                        to: receiver,
                        rel_type: "calls".to_string(),
                        properties: props,
                    });
                }
            }
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
        let mut parser = TSParser::new();
        parser.set_language(&self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();

        let mut result = ExtractionResult {
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
    use super::*;

    #[test]
    fn test_elixir_parser() {
        let code = r#"
        defmodule MyModule do
            @behaviour MyBehaviour
            use GenServer

            def start_link(arg) do
                GenServer.call(__MODULE__, :start)
            end

            def handle_call(:msg, _from, state) do
                {:reply, :ok, state}
            end

            def my_func() do
                load_nif("my_nif", 0)
            end

            defmacro my_macro() do
                quote do
                    1 + 1
                end
            end
        end
        "#;

        let parser = ElixirParser::new();
        let result = parser.parse(code);

        // Modules
        assert!(result.symbols.iter().any(|s| s.name == "MyModule" && s.kind == "module"));
        
        // Functions
        let start_link = result.symbols.iter().find(|s| s.name == "MyModule.start_link").unwrap();
        assert_eq!(start_link.kind, "function");
        assert!(start_link.is_entry_point);

        let handle_call = result.symbols.iter().find(|s| s.name == "MyModule.handle_call").unwrap();
        assert!(handle_call.is_entry_point);

        let my_func = result.symbols.iter().find(|s| s.name == "MyModule.my_func").unwrap();
        assert_eq!(my_func.properties.get("nif_loader").map(|s| s.as_str()), Some("true"));

        let my_macro = result.symbols.iter().find(|s| s.name == "MyModule.my_macro").unwrap();
        assert_eq!(my_macro.kind, "macro");

        // Relations
        let behaviour_rel = result.relations.iter().find(|r| r.rel_type == "implements").unwrap();
        assert_eq!(behaviour_rel.from, "MyModule");
        assert_eq!(behaviour_rel.to, "MyBehaviour");

        let use_rel = result.relations.iter().find(|r| r.rel_type == "uses").unwrap();
        assert_eq!(use_rel.to, "GenServer");

        let genserver_rel = result.relations.iter().find(|r| r.rel_type == "calls").unwrap();
        assert_eq!(genserver_rel.properties.get("genserver").map(|s| s.as_str()), Some("true"));
        assert_eq!(genserver_rel.to, "GenServer");
    }

    #[test]
    fn test_elixir_nif_resolution() {
        let code = r#"
            defmodule Axon.Scanner do
              use Rustler, otp_app: :axon_watcher, crate: "axon_scanner"
              def scan(_path), do: :erlang.nif_error(:nif_not_loaded)
            end
        "#;
        let parser = ElixirParser::new();
        let result = parser.parse(code);

        let has_nif_call = result.relations.iter().any(|r| {
            r.rel_type == "calls_nif" && r.to == "scan"
        });
        
        assert!(has_nif_call, "Elixir NIF resolution failed");
    }
}
