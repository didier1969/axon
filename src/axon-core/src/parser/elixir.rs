use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

const OTP_ENTRY_POINTS: &[&str] = &[
    "handle_call",
    "handle_cast",
    "handle_info",
    "handle_continue",
    "init",
    "start_link",
];

const IMPORT_DIRECTIVES: &[&str] = &["alias", "import", "use", "require"];

// REQ-AXO-901969 — Elixir control-flow special forms are themselves parsed as
// tree-sitter `call` nodes (e.g. `case x do … end`). They are NOT function
// calls: emitting a `Caller -> Module.case` edge is noise, and—worse—treating
// them as leaf calls hides every real call nested in their clauses/body. We
// must instead descend into them. `fn` is parsed as `anonymous_function`
// (already recursed via the non-call branch) so it is not listed here.
const CONTROL_FLOW_FORMS: &[&str] = &[
    "case", "cond", "with", "if", "unless", "for", "try", "receive", "quote",
];

pub struct ElixirParser {
    wasm_bytes: &'static [u8],
}

impl Default for ElixirParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ElixirParser {
    fn do_block_split_lines<'a>(do_block: Node<'a>) -> Vec<usize> {
        let mut cursor = do_block.walk();
        do_block
            .named_children(&mut cursor)
            .map(|child| child.start_position().row + 1)
            .collect()
    }

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
        aliases: &HashMap<String, String>,
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
                        aliases,
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
                        aliases,
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
        aliases: &HashMap<String, String>,
    ) {
        if let Some(identifier) = Self::call_identifier(node, source_bytes) {
            match identifier.as_str() {
                "defmodule" => Self::extract_module(
                    node,
                    source_bytes,
                    content,
                    result,
                    pending_attrs,
                    aliases,
                ),
                "def" | "defp" => Self::extract_function(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    pending_attrs,
                    identifier.as_str(),
                    aliases,
                ),
                "defmacro" | "defmacrop" => Self::extract_macro(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    pending_attrs,
                    identifier.as_str(),
                    aliases,
                ),
                x if IMPORT_DIRECTIVES.contains(&x) => {
                    Self::extract_import_directive(node, source_bytes, result, x, module_name)
                }
                _ => Self::extract_generic_call(node, source_bytes, result, module_name, aliases),
            }
        } else {
            Self::extract_generic_call(node, source_bytes, result, module_name, aliases);
        }
    }

    fn extract_module<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        content: &str,
        result: &mut ExtractionResult,
        _decorators: &[String],
        outer_aliases: &HashMap<String, String>,
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
            // REQ-AXO-901953 — a module inherits the enclosing scope's aliases
            // (lexical) and adds its own; the merged map lets qualified
            // cross-module calls inside this module resolve to the callee's
            // canonical Symbol.id rather than a dangling module-short-name.
            let mut module_aliases = outer_aliases.clone();
            module_aliases.extend(Self::collect_module_aliases(do_block, source_bytes));
            Self::walk(
                do_block,
                source_bytes,
                content,
                result,
                &new_module_name,
                &mut Vec::new(),
                &module_aliases,
            );
        }
    }

    /// REQ-AXO-901953 — map each `alias Fully.Qualified.Module` directive in a
    /// module body to `short_name -> FQN` (`Module -> Fully.Qualified.Module`)
    /// so qualified calls (`Module.fun()`) resolve to the callee's canonical
    /// Symbol.id (`Fully.Qualified.Module.fun`). Multi-aliases (`A.B.{C, D}`)
    /// and `as:` renames are intentionally skipped: resolving them wrongly is
    /// worse than leaving the edge unresolved, so we stay partial-but-correct.
    fn collect_module_aliases<'a>(
        do_block: Node<'a>,
        source_bytes: &[u8],
    ) -> HashMap<String, String> {
        let mut aliases = HashMap::new();
        let mut cursor = do_block.walk();
        for child in do_block.named_children(&mut cursor) {
            if child.kind() != "call" {
                continue;
            }
            if Self::call_identifier(child, source_bytes).as_deref() != Some("alias") {
                continue;
            }
            let Some(args) = Self::find_child_by_type(child, "arguments") else {
                continue;
            };
            let args_text = args.utf8_text(source_bytes).unwrap_or("");
            if args_text.contains('{') || args_text.contains("as:") {
                continue;
            }
            let mut fqn = String::new();
            let mut arg_cursor = args.walk();
            for arg in args.named_children(&mut arg_cursor) {
                if arg.kind() == "alias" {
                    fqn = arg.utf8_text(source_bytes).unwrap_or("").to_string();
                    break;
                }
            }
            if !fqn.contains('.') {
                continue;
            }
            let short = fqn.rsplit('.').next().unwrap_or(fqn.as_str()).to_string();
            aliases.insert(short, fqn);
        }
        aliases
    }

    fn extract_function<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        _decorators: &[String],
        def_type: &str,
        aliases: &HashMap<String, String>,
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

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            properties.insert(
                "header_end_line".to_string(),
                do_block.start_position().row.saturating_add(1).to_string(),
            );
            properties.insert(
                "body_start_line".to_string(),
                do_block.start_position().row.saturating_add(1).to_string(),
            );
            properties.insert(
                "body_end_line".to_string(),
                do_block.end_position().row.saturating_add(1).to_string(),
            );
            let split_lines = Self::do_block_split_lines(do_block);
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
        }

        let node_content = node.utf8_text(source_bytes).unwrap_or("");
        let is_nif =
            node_content.contains(":erlang.nif_error") || node_content.contains(":nif_not_loaded");
        if node_content.contains("load_nif") {
            properties.insert("nif_loader".to_string(), "true".to_string());
        }

        // REQ-AXO-901969 follow-up — emit a CALLS_NIF edge for ANY NIF stub
        // (was gated on the exact ":erlang.nif_error(:nif_not_loaded)" string,
        // so variant stubs like ":erlang.nif_error(:not_loaded)" produced
        // calls_nif=0 on real Rustler codebases). Anchor it on the canonical
        // function symbol `full_name` (not the bare module) so impact/inspect
        // resolve the source end. Full Elixir->Rust cross-language resolution
        // (traversing into the matching rustler::nif fn) is tracked separately.
        if is_nif {
            result.relations.push(Relation {
                from: full_name.clone(),
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
            Self::extract_calls_from_block(do_block, source_bytes, result, &full_name, aliases);
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
        aliases: &HashMap<String, String>,
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
            Self::extract_calls_from_block(do_block, source_bytes, result, &full_name, aliases);
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
        aliases: &HashMap<String, String>,
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::call_identifier(child, source_bytes) {
                    if [
                        "def",
                        "defp",
                        "defmodule",
                        "defmacro",
                        "defmacrop",
                        "defstruct",
                    ]
                    .contains(&ident.as_str())
                    {
                        continue;
                    }
                    if IMPORT_DIRECTIVES.contains(&ident.as_str()) {
                        continue;
                    }
                    // REQ-AXO-901969 — control-flow special form: not a call.
                    // Don't emit a bogus edge; descend into its clauses/body so
                    // the calls nested inside (the real callees) are captured.
                    if CONTROL_FLOW_FORMS.contains(&ident.as_str()) {
                        Self::extract_calls_from_block(
                            child,
                            source_bytes,
                            result,
                            module_name,
                            aliases,
                        );
                        continue;
                    }
                }
                Self::extract_generic_call(child, source_bytes, result, module_name, aliases);
                // REQ-AXO-901969 — recurse into the call's arguments so calls
                // passed as arguments or wrapped in anonymous functions
                // (e.g. `Enum.map(xs, fn x -> prepare_dataset(x) end)`) are not
                // lost. extract_generic_call only handles the call head.
                if let Some(args) = Self::find_child_by_type(child, "arguments") {
                    Self::extract_calls_from_block(
                        args,
                        source_bytes,
                        result,
                        module_name,
                        aliases,
                    );
                }
            } else {
                Self::extract_calls_from_block(child, source_bytes, result, module_name, aliases);
            }
        }
    }

    fn extract_generic_call<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller_name: &str,
        aliases: &HashMap<String, String>,
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
                // REQ-AXO-901953 — resolve the receiver short-name through the
                // module's alias map to the fully-qualified module, so the
                // CALLS edge targets the callee's canonical Symbol.id instead
                // of a dangling short-name that matches nothing in the IST
                // (the cause of "0 callers" for qualified Elixir calls).
                let resolved_receiver = aliases
                    .get(&receiver)
                    .cloned()
                    .unwrap_or_else(|| receiver.clone());
                let mut rel_type = "CALLS".to_string();

                let is_genserver =
                    receiver == "GenServer" && (func_name == "call" || func_name == "cast");
                // Default target = the callee FUNCTION symbol `Module.func`.
                let mut target = format!("{resolved_receiver}.{func_name}");
                if is_genserver {
                    rel_type = "CALLS_OTP".to_string();
                    // OTP boundary: the edge points at the target MODULE taken
                    // from the first argument (also alias-resolved), not a
                    // function — keep it module-level.
                    target = resolved_receiver.clone();
                    if let Some(args_node) = Self::find_child_by_type(node, "arguments") {
                        let mut arg_cursor = args_node.walk();
                        for arg_child in args_node.named_children(&mut arg_cursor) {
                            if arg_child.kind() == "alias" {
                                let server =
                                    arg_child.utf8_text(source_bytes).unwrap_or("").to_string();
                                target = aliases.get(&server).cloned().unwrap_or(server);
                                break;
                            }
                        }
                    }
                }

                // Skip generic calls to standard library unless it's an OTP boundary we want to track
                if receiver != "Enum"
                    && receiver != "String"
                    && receiver != "Map"
                    && receiver != "List"
                {
                    let mut props = HashMap::new();
                    if is_genserver {
                        props.insert("otp_boundary".to_string(), "true".to_string());
                        props.insert("call_type".to_string(), func_name.clone());
                    }

                    result.relations.push(Relation {
                        from: caller_name.to_string(),
                        to: target,
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
                                let behaviour_name =
                                    alias.utf8_text(source_bytes).unwrap_or("").to_string();
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
        let res = node
            .named_children(&mut cursor)
            .find(|&child| child.kind() == kind);
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

        let source_bytes = content.as_bytes();
        Self::walk(
            tree.root_node(),
            source_bytes,
            content,
            &mut result,
            "",
            &mut Vec::new(),
            &HashMap::new(),
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

        assert!(result
            .symbols
            .iter()
            .any(|sym| sym.name == "Axon.Sample.trigger_scan"));
        assert!(result
            .symbols
            .iter()
            .any(|sym| sym.name == "Axon.Sample.parse_batch"));
        assert!(result
            .relations
            .iter()
            .any(|rel| rel.from == "Axon.Sample.trigger_scan"
                && rel.to == "Axon.Sample.parse_batch"
                && rel.rel_type == "CALLS"));
    }

    #[test]
    fn test_elixir_parser_resolves_aliased_cross_module_call_to_canonical_symbol() {
        // REQ-AXO-901953 — the umbrella controller→context case that returned
        // "0 callers". `alias FiscalyCore.Governance.LegalSources` then
        // `LegalSources.list(params)` must emit a CALLS edge whose target is
        // the callee's canonical Symbol.id, so impact/inspect/bidi_trace
        // resolve it instead of a dangling module-short-name.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule FiscalyWeb.Api.V1.LegalSourceController do
          alias FiscalyCore.Governance.LegalSources

          def index(params) do
            LegalSources.list(params)
          end
        end
        "#;

        let result = parser.parse(content);

        assert!(
            result.relations.iter().any(|rel| rel.from
                == "FiscalyWeb.Api.V1.LegalSourceController.index"
                && rel.to == "FiscalyCore.Governance.LegalSources.list"
                && rel.rel_type == "CALLS"),
            "alias-resolved cross-module CALLS edge missing; got: {:?}",
            result.relations
        );
        // The dangling short-name target must no longer be emitted.
        assert!(
            !result
                .relations
                .iter()
                .any(|rel| rel.to == "LegalSources" && rel.rel_type == "CALLS"),
            "unresolved module-short-name target still emitted: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_qualified_call_targets_function_symbol_not_module() {
        // REQ-AXO-901953 — a fully-qualified call resolves to the callee
        // FUNCTION symbol (`Module.func`), not the bare module, so the CALLS
        // edge matches the indexed function Symbol.id.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule FiscalyCore.Governance.LegalSources do
          def list(params) do
            FiscalyCore.Repo.all(params)
          end
        end
        "#;

        let result = parser.parse(content);

        assert!(
            result.relations.iter().any(|rel| rel.from
                == "FiscalyCore.Governance.LegalSources.list"
                && rel.to == "FiscalyCore.Repo.all"
                && rel.rel_type == "CALLS"),
            "qualified call should target the function symbol; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_emits_body_split_lines_for_functions() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def trigger_scan do
            prepare()
            flush_ready_queue()
            persist()
          end
        end
        "#;

        let result = parser.parse(content);
        let symbol = result
            .symbols
            .iter()
            .find(|sym| sym.name == "Axon.Sample.trigger_scan")
            .expect("trigger_scan symbol");

        assert_eq!(
            symbol.properties.get("body_start_line"),
            Some(&"3".to_string())
        );
        assert_eq!(
            symbol.properties.get("body_split_lines"),
            Some(&"4,5,6".to_string())
        );
    }

    // REQ-AXO-901969 — calls wrapped in control-flow special forms
    // (case/with/cond/if/for/...) or passed as arguments / inside anonymous
    // functions were dropped: the special form is itself a tree-sitter `call`
    // node, so the resolver (a) emitted a bogus `Caller -> Module.case` edge and
    // (b) never descended into its body. Result: impact/inspect/path reported
    // "0 callers" for real callees (the TE2 prepare_dataset case).

    #[test]
    fn test_elixir_parser_resolves_calls_inside_case() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def run(x) do
            case x do
              :a -> prepare_dataset(x)
              _ -> :skip
            end
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.prepare_dataset"
                && rel.rel_type == "CALLS"),
            "call inside `case` missing; got: {:?}",
            result.relations
        );
        assert!(
            !result
                .relations
                .iter()
                .any(|rel| rel.to.ends_with(".case") && rel.rel_type == "CALLS"),
            "bogus CALLS edge to the `case` special form emitted: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_resolves_calls_inside_with() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def run(p) do
            with {:ok, ds} <- prepare_dataset(p) do
              label_multi_horizon(ds)
            end
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.prepare_dataset"
                && rel.rel_type == "CALLS"),
            "call inside `with` head missing; got: {:?}",
            result.relations
        );
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.label_multi_horizon"
                && rel.rel_type == "CALLS"),
            "call inside `with` body missing; got: {:?}",
            result.relations
        );
        assert!(
            !result
                .relations
                .iter()
                .any(|rel| rel.to.ends_with(".with") && rel.rel_type == "CALLS"),
            "bogus CALLS edge to the `with` special form emitted: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_resolves_calls_in_pipe_chain() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def run(p) do
            p
            |> prepare_dataset()
            |> label_multi_horizon()
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.prepare_dataset"
                && rel.rel_type == "CALLS"),
            "piped call prepare_dataset missing; got: {:?}",
            result.relations
        );
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.label_multi_horizon"
                && rel.rel_type == "CALLS"),
            "piped call label_multi_horizon missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_emits_anchored_calls_nif_for_rustler_stubs() {
        // REQ-AXO-901969 follow-up — calls_nif was emitted only for the exact
        // ":erlang.nif_error(:nif_not_loaded)" string and anchored on the bare
        // MODULE (to = bare func_name, dangling). Real Rustler codebases used
        // variant stubs / resolved nothing -> client saw calls_nif=0. Now: emit
        // for any NIF stub, anchored on the canonical function symbol.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Native do
          use Rustler, otp_app: :my_app, crate: "myapp_native"
          def add(_a, _b), do: :erlang.nif_error(:nif_not_loaded)
          def sub(_a, _b), do: :erlang.nif_error(:not_loaded)
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "calls_nif" && r.from == "MyApp.Native.add"),
            "calls_nif edge not anchored on the function symbol; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "calls_nif" && r.from == "MyApp.Native.sub"),
            "variant nif_error stub not detected (under-emission); got: {:?}",
            result.relations
        );
        assert!(
            result
                .symbols
                .iter()
                .any(|s| s.name == "MyApp.Native.add" && s.is_nif),
            "is_nif flag missing on the NIF stub symbol"
        );
    }

    #[test]
    fn test_elixir_parser_resolves_calls_in_anonymous_fn_argument() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def run(xs) do
            Enum.map(xs, fn x -> prepare_dataset(x) end)
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result.relations.iter().any(|rel| rel.from == "Axon.Sample.run"
                && rel.to == "Axon.Sample.prepare_dataset"
                && rel.rel_type == "CALLS"),
            "call inside anonymous fn argument missing; got: {:?}",
            result.relations
        );
    }
}
