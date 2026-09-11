use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

/// Framework/runtime entry-point callbacks in Elixir: functions the BEAM or a
/// framework invokes by contract with NO in-repo caller — GenServer, Supervisor,
/// Application, Phoenix LiveView/Component, GenStage, Plug, Mix.Task. Marking
/// them `is_entry_point` keeps reachability analytics (`orphan_clusters` /
/// `wiring`) from reading a live OTP process tree as dead code (REQ-AXO-902221,
/// NEX gate «organe câblé»). SINGLE shared source — `ist_snapshot::code_smells::
/// is_inferred_entry` matches against this very const (no duplication); the
/// analytics side PATH-SCOPES it to `.ex/.exs` so these ordinary names
/// (`init`/`render`/`run`/`call`) never mask a real orphan in another ecosystem.
/// Match is on the BARE function name.
pub const ELIXIR_ENTRY_POINTS: &[&str] = &[
    // GenServer
    "handle_call",
    "handle_cast",
    "handle_info",
    "handle_continue",
    // OTP lifecycle (GenServer / Supervisor / Application / Agent / Task)
    "init",
    "start_link",
    "start",
    "stop",
    "terminate",
    "code_change",
    "child_spec",
    "format_status",
    // GenStage / Broadway / Flow
    "handle_demand",
    "handle_subscribe",
    // Phoenix LiveView / LiveComponent / Component (framework-rendered)
    "mount",
    "handle_event",
    "handle_params",
    "render",
    "handle_async",
    // Plug / Mix.Task — framework-dispatched; path-scoping to Elixir keeps the
    // generic names `call`/`run` from over-marking entries in other languages.
    "call",
    "run",
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
                "test" => Self::extract_test_macro(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    aliases,
                ),
                "schema" | "embedded_schema" => Self::extract_ecto_schema(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    aliases,
                ),
                "dispatch" => Self::extract_commanded_dispatch(
                    node,
                    source_bytes,
                    result,
                    module_name,
                    aliases,
                ),
                "policies" => Self::extract_ash_policies(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
                    aliases,
                ),
                "plug" => Self::extract_plug(node, source_bytes, result, module_name, aliases),
                "pipeline" => Self::extract_pipeline(
                    node,
                    source_bytes,
                    content,
                    result,
                    module_name,
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

    /// REQ-AXO-901953 / REQ-AXO-902223 — map each `alias` directive in a module
    /// body to `short_name -> FQN` (`Module -> Fully.Qualified.Module`) so
    /// qualified calls (`Module.fun()`) resolve to the callee's canonical
    /// Symbol.id (`Fully.Qualified.Module.fun`). Handles the three forms
    /// tree-sitter-elixir produces (structure verified from the AST, never
    /// string-matched):
    ///   `alias A.B.C`        -> `C -> A.B.C`             (single)
    ///   `alias A.B.C, as: X` -> `X -> A.B.C`             (rename)
    ///   `alias A.B.{C, D}`   -> `C -> A.B.C`, `D -> A.B.D` (multi)
    /// Any shape we can't resolve unambiguously is skipped — a WRONG resolution
    /// is worse than a dangling short-name, so we stay partial-but-correct. The
    /// unresolved `{}`/`as:` forms were the dominant cause of dangling Elixir
    /// CALLS edges (~78% per REQ-AXO-902223 / APS), which made `wiring` see only
    /// test callers and mis-report live modules as `test_only`.
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
            Self::collect_alias_args(args, source_bytes, &mut aliases);
        }
        aliases
    }

    /// REQ-AXO-902223 — resolve one `alias` directive's `arguments` node into
    /// `short -> FQN` entries. See `collect_module_aliases` for the forms.
    fn collect_alias_args(args: Node, source_bytes: &[u8], out: &mut HashMap<String, String>) {
        // Multi-alias `A.B.{C, D}` parses as a `dot` whose children are the
        // `alias` prefix (`A.B`) and a `tuple` of `alias` short paths (`{C, D}`).
        if let Some(dot) = Self::find_child_by_type(args, "dot") {
            let mut dc = dot.walk();
            let children: Vec<Node> = dot.named_children(&mut dc).collect();
            let prefix = children.iter().find(|n| n.kind() == "alias");
            let tuple = children.iter().find(|n| n.kind() == "tuple");
            if let (Some(prefix), Some(tuple)) = (prefix, tuple) {
                let prefix_txt = prefix.utf8_text(source_bytes).unwrap_or("");
                if !prefix_txt.is_empty() {
                    let mut tc = tuple.walk();
                    for member in tuple.named_children(&mut tc) {
                        if member.kind() != "alias" {
                            continue;
                        }
                        let short_path = member.utf8_text(source_bytes).unwrap_or("");
                        if short_path.is_empty() {
                            continue;
                        }
                        let fqn = format!("{}.{}", prefix_txt, short_path);
                        let key = short_path
                            .rsplit('.')
                            .next()
                            .unwrap_or(short_path)
                            .to_string();
                        out.insert(key, fqn);
                    }
                }
                return;
            }
        }

        // Single (`alias A.B.C`) or rename (`alias A.B.C, as: X`): the FQN is the
        // top-level `alias` child; an optional `keywords` sibling carries `as: X`.
        let mut ac = args.walk();
        let mut fqn = String::new();
        let mut as_name: Option<String> = None;
        for child in args.named_children(&mut ac) {
            match child.kind() {
                "alias" if fqn.is_empty() => {
                    fqn = child.utf8_text(source_bytes).unwrap_or("").to_string();
                }
                "keywords" => {
                    as_name = Self::extract_as_rename(child, source_bytes);
                }
                _ => {}
            }
        }
        if fqn.is_empty() {
            return;
        }
        // A single-segment `alias Foo` is an identity no-op unless renamed.
        if !fqn.contains('.') && as_name.is_none() {
            return;
        }
        let key = match as_name {
            Some(x) => x,
            None => fqn.rsplit('.').next().unwrap_or(fqn.as_str()).to_string(),
        };
        out.insert(key, fqn);
    }

    /// REQ-AXO-902223 — pull `X` out of a `keywords` node holding `as: X`
    /// (`pair(keyword "as:", alias X)`). Returns None for any other keyword.
    fn extract_as_rename(keywords: Node, source_bytes: &[u8]) -> Option<String> {
        let mut kc = keywords.walk();
        for pair in keywords.named_children(&mut kc) {
            if pair.kind() != "pair" {
                continue;
            }
            let mut pc = pair.walk();
            let mut is_as = false;
            let mut rename: Option<String> = None;
            for pchild in pair.named_children(&mut pc) {
                match pchild.kind() {
                    "keyword" => {
                        let kw = pchild.utf8_text(source_bytes).unwrap_or("");
                        if kw.trim().trim_end_matches(':') == "as" {
                            is_as = true;
                        }
                    }
                    "alias" => {
                        rename = Some(pchild.utf8_text(source_bytes).unwrap_or("").to_string());
                    }
                    _ => {}
                }
            }
            if is_as {
                if let Some(r) = rename {
                    if !r.is_empty() {
                        return Some(r);
                    }
                }
            }
        }
        None
    }

    fn extract_test_macro<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        let args = match Self::find_child_by_type(node, "arguments") {
            Some(a) => a,
            None => return,
        };

        let mut test_name = String::new();
        let mut cursor = args.walk();
        for child in args.named_children(&mut cursor) {
            if child.kind() == "string" || child.kind() == "atom" || child.kind() == "identifier" {
                let raw_text = child.utf8_text(source_bytes).unwrap_or("");
                let clean_text = raw_text.trim_matches('"').trim_start_matches(':').trim();
                if !clean_text.is_empty() {
                    test_name = clean_text.to_string();
                    break;
                }
            }
        }

        if test_name.is_empty() {
            test_name = format!("test_at_line_{}", node.start_position().row + 1);
        }

        let full_name = if module_name.is_empty() {
            format!("test {}", test_name)
        } else {
            format!("{}.test {}", module_name, test_name)
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let mut properties = HashMap::new();
        properties.insert("test_framework".to_string(), "ex_unit".to_string());

        result.symbols.push(Symbol {
            name: full_name.clone(),
            kind: "function".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: true,
            is_public: false,
            tested: true,
            is_nif: false,
            is_unsafe: false,
            properties,
            embedding: None,
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::extract_calls_from_expression(
                do_block,
                source_bytes,
                result,
                &full_name,
                aliases,
            );
        }
    }

    fn extract_ecto_schema<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        let mut table_name = String::new();
        if let Some(args) = Self::find_child_by_type(node, "arguments") {
            let mut ac = args.walk();
            for child in args.named_children(&mut ac) {
                let text = child.utf8_text(source_bytes).unwrap_or("");
                if child.kind() == "string" || text.starts_with('"') {
                    table_name = text.trim_matches('"').to_string();
                    break;
                }
            }
        }

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let mut schema_props = HashMap::new();
        if !table_name.is_empty() {
            schema_props.insert("table".to_string(), table_name.clone());
        }

        result.symbols.push(Symbol {
            name: format!("{}.schema", module_name),
            kind: "schema".to_string(),
            start_line,
            end_line,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: schema_props,
            embedding: None,
        });

        if !table_name.is_empty() {
            result.relations.push(Relation {
                from: module_name.to_string(),
                to: table_name,
                rel_type: "references".to_string(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert("schema".to_string(), "true".to_string());
                    p
                },
            });
        }

        let Some(do_block) = Self::find_child_by_type(node, "do_block") else {
            return;
        };

        let mut cursor = do_block.walk();
        for child in do_block.named_children(&mut cursor) {
            if child.kind() != "call" {
                continue;
            }

            let Some(ident) = Self::call_identifier(child, source_bytes) else {
                continue;
            };

            let Some(args) = Self::find_child_by_type(child, "arguments") else {
                Self::extract_generic_call(child, source_bytes, result, module_name, aliases);
                continue;
            };

            let mut ac = args.walk();
            let arg_nodes: Vec<Node> = args.named_children(&mut ac).collect();

            match ident.as_str() {
                "field" => {
                    if let Some(first_arg) = arg_nodes.first() {
                        let raw_name = first_arg.utf8_text(source_bytes).unwrap_or("");
                        let field_name = raw_name.trim_start_matches(':').to_string();
                        let field_type = if let Some(second_arg) = arg_nodes.get(1) {
                            second_arg.utf8_text(source_bytes).unwrap_or("").to_string()
                        } else {
                            String::new()
                        };

                        let full_field_name = format!("{}.{}", module_name, field_name);
                        let mut props = HashMap::new();
                        if !field_type.is_empty() {
                            props.insert("type".to_string(), field_type);
                        }

                        let is_redacted = arg_nodes.iter().any(|arg| {
                            arg.utf8_text(source_bytes)
                                .unwrap_or("")
                                .contains("redact: true")
                        });

                        let sensitive_kind = if is_redacted {
                            Some("redacted")
                        } else {
                            super::is_sensitive_name(&field_name)
                        };

                        if let Some(kind) = sensitive_kind {
                            props.insert("is_sensitive".to_string(), "true".to_string());
                            props.insert("pii_kind".to_string(), kind.to_string());

                            let schema_sym_name = format!("{}.schema", module_name);
                            if let Some(schema_sym) = result
                                .symbols
                                .iter_mut()
                                .find(|s| s.name == schema_sym_name)
                            {
                                schema_sym
                                    .properties
                                    .insert("has_sensitive_fields".to_string(), "true".to_string());
                            }
                        }

                        result.symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "field".to_string(),
                            start_line: child.start_position().row + 1,
                            end_line: child.end_position().row + 1,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: props,
                            embedding: None,
                        });

                        result.relations.push(Relation {
                            from: module_name.to_string(),
                            to: full_field_name,
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
                "belongs_to" | "has_many" | "has_one" | "many_to_many" => {
                    if let Some(first_arg) = arg_nodes.first() {
                        let raw_name = first_arg.utf8_text(source_bytes).unwrap_or("");
                        let field_name = raw_name.trim_start_matches(':').to_string();

                        let target_mod = if let Some(second_arg) = arg_nodes.get(1) {
                            let raw_mod =
                                second_arg.utf8_text(source_bytes).unwrap_or("").to_string();
                            aliases.get(&raw_mod).cloned().unwrap_or(raw_mod)
                        } else {
                            String::new()
                        };

                        let mut rel_props = HashMap::new();
                        rel_props.insert("relation".to_string(), ident.clone());
                        rel_props.insert("field".to_string(), field_name.clone());

                        if !target_mod.is_empty() {
                            result.relations.push(Relation {
                                from: module_name.to_string(),
                                to: target_mod,
                                rel_type: "references".to_string(),
                                properties: rel_props,
                            });
                        }

                        let full_field_name = format!("{}.{}", module_name, field_name);
                        let mut field_props = HashMap::new();
                        field_props.insert("association".to_string(), ident);

                        result.symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "field".to_string(),
                            start_line: child.start_position().row + 1,
                            end_line: child.end_position().row + 1,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: field_props,
                            embedding: None,
                        });

                        result.relations.push(Relation {
                            from: module_name.to_string(),
                            to: full_field_name,
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
                _ => {
                    Self::extract_generic_call(child, source_bytes, result, module_name, aliases);
                }
            }
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
        aliases: &HashMap<String, String>,
    ) {
        let func_name = match Self::extract_def_name(node, source_bytes) {
            Some(name) => name,
            None => return,
        };

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        let is_framework_entry = ELIXIR_ENTRY_POINTS.contains(&func_name.as_str());
        // REQ-AXO-902227 — `@impl` annotation: framework-AGNOSTIC entry-point signal
        // (catches GenServer / Phoenix / Ecto AND project-defined behaviours without
        // hardcoding callback names). Feeds is_entry_point → reachability root seeding.
        let has_impl_entry = Self::has_impl_annotation(node, source_bytes);

        let full_name = if module_name.is_empty() {
            func_name.clone()
        } else {
            format!("{}.{}", module_name, func_name)
        };

        let mut properties = HashMap::new();

        if let Some((body_node, is_block)) = Self::find_function_body_node(node, source_bytes) {
            if is_block {
                properties.insert(
                    "header_end_line".to_string(),
                    body_node.start_position().row.saturating_add(1).to_string(),
                );
                properties.insert(
                    "body_start_line".to_string(),
                    body_node.start_position().row.saturating_add(1).to_string(),
                );
                properties.insert(
                    "body_end_line".to_string(),
                    body_node.end_position().row.saturating_add(1).to_string(),
                );
                let split_lines = Self::do_block_split_lines(body_node);
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
            } else {
                properties.insert("header_end_line".to_string(), start_line.to_string());
                properties.insert(
                    "body_start_line".to_string(),
                    body_node.start_position().row.saturating_add(1).to_string(),
                );
                properties.insert(
                    "body_end_line".to_string(),
                    body_node.end_position().row.saturating_add(1).to_string(),
                );
            }
            let complexity = 1 + Self::count_branches(body_node, source_bytes);
            properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
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
            is_entry_point: is_framework_entry || has_impl_entry || is_nif,
            is_public: def_type == "def",
            tested: func_name.starts_with("test_") || module_name.ends_with("Test"),
            is_nif,
            is_unsafe: false,
            properties,
            embedding: None,
        });

        if let Some((body_node, _)) = Self::find_function_body_node(node, source_bytes) {
            Self::extract_calls_from_expression(
                body_node,
                source_bytes,
                result,
                &full_name,
                aliases,
            );
        }

        if func_name == "deps" {
            if let Some((body_node, _)) = Self::find_function_body_node(node, source_bytes) {
                let body_text = body_node.utf8_text(source_bytes).unwrap_or("");
                for line in body_text.lines() {
                    let trimmed = line.trim();
                    if let Some(colon_pos) = trimmed.find("{:") {
                        let rest = &trimmed[colon_pos + 2..];
                        let end_atom = rest
                            .find(|c: char| !c.is_alphanumeric() && c != '_')
                            .unwrap_or(rest.len());
                        let dep_name = &rest[..end_atom];
                        if !dep_name.is_empty() {
                            let mut dep_props = HashMap::new();
                            dep_props.insert("ecosystem".to_string(), "hex".to_string());
                            result.symbols.push(Symbol {
                                name: dep_name.to_string(),
                                kind: "dependency".to_string(),
                                start_line,
                                end_line,
                                docstring: None,
                                is_entry_point: false,
                                is_public: true,
                                tested: false,
                                is_nif: false,
                                is_unsafe: false,
                                properties: dep_props,
                                embedding: None,
                            });
                            result.relations.push(Relation {
                                from: if module_name.is_empty() {
                                    full_name.clone()
                                } else {
                                    module_name.to_string()
                                },
                                to: dep_name.to_string(),
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }
        }

        if func_name == "join" {
            if let Some(args) = Self::find_child_by_type(node, "arguments") {
                let mut cursor = args.walk();
                for child in args.named_children(&mut cursor) {
                    if child.kind() == "call" {
                        if let Some(inner_args) = Self::find_child_by_type(child, "arguments") {
                            for s in Self::find_string_args(inner_args, source_bytes) {
                                if !result
                                    .symbols
                                    .iter()
                                    .any(|sym| sym.kind == "topic" && sym.name == s)
                                {
                                    result.symbols.push(Symbol {
                                        name: s.clone(),
                                        kind: "topic".to_string(),
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
                                }
                                result.relations.push(Relation {
                                    from: module_name.to_string(),
                                    to: s,
                                    rel_type: "subscribes_to".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }
                }
            }
        } else if func_name == "execute" || func_name == "apply" {
            let rel_type = if func_name == "execute" {
                "handles_command"
            } else {
                "applies_event"
            };
            if let Some(args) = Self::find_child_by_type(node, "arguments") {
                let mut cursor = args.walk();
                for child in args.named_children(&mut cursor) {
                    if child.kind() == "call" {
                        if let Some(inner_args) = Self::find_child_by_type(child, "arguments") {
                            let mut i_cursor = inner_args.walk();
                            let mut arg_idx = 0;
                            for inner_child in inner_args.named_children(&mut i_cursor) {
                                arg_idx += 1;
                                if arg_idx == 2 {
                                    let text = inner_child.utf8_text(source_bytes).unwrap_or("");
                                    if let Some(pos) = text.find('%') {
                                        let rest = &text[pos + 1..];
                                        let end_pos = rest
                                            .find(|c: char| {
                                                c == '{' || c == ' ' || c == '(' || c == '='
                                            })
                                            .unwrap_or(rest.len());
                                        let raw = rest[..end_pos].trim();
                                        if !raw.is_empty() {
                                            let resolved = aliases
                                                .get(raw)
                                                .cloned()
                                                .unwrap_or_else(|| raw.to_string());
                                            result.relations.push(Relation {
                                                from: module_name.to_string(),
                                                to: resolved,
                                                rel_type: rel_type.to_string(),
                                                properties: HashMap::new(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// REQ-AXO-902227 — does this `def` carry an `@impl` annotation? `@impl true` /
    /// `@impl SomeBehaviour` is the idiomatic Elixir marker that the function
    /// implements a behaviour callback invoked by the runtime/framework, not via a
    /// direct call. Framework-AGNOSTIC: catches GenServer / Phoenix / Ecto AND
    /// project-defined behaviours without hardcoding callback names. Scans back over
    /// any intervening module attributes (`@doc` / `@spec` / …) to the nearest
    /// `@impl`; `@impl false` (explicit "not a callback") does NOT count.
    fn has_impl_annotation<'a>(node: Node<'a>, source_bytes: &[u8]) -> bool {
        let mut sib = node.prev_named_sibling();
        while let Some(s) = sib {
            let text = s.utf8_text(source_bytes).unwrap_or("").trim_start();
            if let Some(rest) = text.strip_prefix("@impl") {
                // Require a word boundary after `@impl` (reject `@implementation`).
                if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace()) {
                    return !rest.trim_start().starts_with("false");
                }
            }
            // Keep scanning back over other module attributes (@doc/@spec/@tag…);
            // stop at the first non-attribute sibling (a prior def/expression).
            if text.starts_with('@') {
                sib = s.prev_named_sibling();
                continue;
            }
            break;
        }
        false
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

        if let Some((body_node, _)) = Self::find_function_body_node(node, source_bytes) {
            Self::extract_calls_from_expression(
                body_node,
                source_bytes,
                result,
                &full_name,
                aliases,
            );
        }
    }

    fn find_string_args<'a>(args_node: Node<'a>, source_bytes: &[u8]) -> Vec<String> {
        let mut strings = Vec::new();
        let mut cursor = args_node.walk();
        for child in args_node.named_children(&mut cursor) {
            if child.kind() == "string" {
                let s = child
                    .utf8_text(source_bytes)
                    .unwrap_or("")
                    .trim_matches('"');
                if !s.is_empty() {
                    strings.push(s.to_string());
                }
            }
        }
        strings
    }

    fn extract_keyword_props<'a>(
        args_node: Node<'a>,
        source_bytes: &[u8],
    ) -> HashMap<String, String> {
        let mut props = HashMap::new();
        let mut cursor = args_node.walk();
        for child in args_node.named_children(&mut cursor) {
            if child.kind() == "keywords" {
                let mut kw_cursor = child.walk();
                for pair in child.named_children(&mut kw_cursor) {
                    let text = pair.utf8_text(source_bytes).unwrap_or("").trim();
                    if let Some((k, v)) = text.split_once(':') {
                        let key = k.trim().to_string();
                        let val = v
                            .trim()
                            .trim_start_matches(':')
                            .trim_matches('"')
                            .to_string();
                        if !key.is_empty() && !val.is_empty() {
                            props.insert(key, val);
                        }
                    }
                }
            }
        }
        if props.is_empty() {
            let full_text = args_node.utf8_text(source_bytes).unwrap_or("");
            for part in full_text.split(',') {
                let trimmed = part.trim();
                if let Some((k, v)) = trimmed.split_once(':') {
                    let key = k.trim().to_string();
                    let val = v
                        .trim()
                        .trim_start_matches(':')
                        .trim_matches('"')
                        .to_string();
                    if !key.is_empty() && !val.is_empty() {
                        props.insert(key, val);
                    }
                }
            }
        }
        props
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

            let props = Self::extract_keyword_props(args_node, source_bytes);
            let start_line = node.start_position().row + 1;
            let end_line = node.end_position().row + 1;

            if module_alias == "Oban.Worker" || module_alias.ends_with(".Worker") {
                result.symbols.push(Symbol {
                    name: format!("{}.worker", module_name),
                    kind: "worker".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            } else if module_alias == "Phoenix.Channel" || module_alias.ends_with(".Channel") {
                result.symbols.push(Symbol {
                    name: format!("{}.channel", module_name),
                    kind: "channel".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            } else if module_alias.contains("Commanded.Projections")
                || module_alias.contains("Commanded.Event.Handler")
                || module_alias.ends_with("Projector")
            {
                result.symbols.push(Symbol {
                    name: format!("{}.projector", module_name),
                    kind: "projector".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }
    }

    fn extract_commanded_dispatch<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        module_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        if let Some(args) = Self::find_child_by_type(node, "arguments") {
            let mut cursor = args.walk();
            let mut command = String::new();
            let mut target = String::new();
            for child in args.named_children(&mut cursor) {
                if child.kind() == "alias" && command.is_empty() {
                    let cmd_raw = child.utf8_text(source_bytes).unwrap_or("");
                    command = aliases
                        .get(cmd_raw)
                        .cloned()
                        .unwrap_or_else(|| cmd_raw.to_string());
                } else if child.kind() == "keywords" {
                    let mut kw_cursor = child.walk();
                    for pair in child.named_children(&mut kw_cursor) {
                        if pair.kind() == "pair" {
                            let mut p_cursor = pair.walk();
                            let mut is_to = false;
                            for p_child in pair.named_children(&mut p_cursor) {
                                let text = p_child.utf8_text(source_bytes).unwrap_or("");
                                if p_child.kind() == "keyword" && text.starts_with("to") {
                                    is_to = true;
                                } else if is_to && p_child.kind() == "alias" {
                                    target = aliases
                                        .get(text)
                                        .cloned()
                                        .unwrap_or_else(|| text.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if !command.is_empty() {
                result.relations.push(Relation {
                    from: module_name.to_string(),
                    to: command.clone(),
                    rel_type: "dispatches_command".to_string(),
                    properties: HashMap::new(),
                });
                if !target.is_empty() {
                    result.relations.push(Relation {
                        from: command,
                        to: target,
                        rel_type: "routes_to".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }
    }

    fn extract_ash_policies<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        _content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        _aliases: &HashMap<String, String>,
    ) {
        let set_name = format!("{}.policies", module_name);
        let mut props = HashMap::new();
        props.insert("framework".to_string(), "ash".to_string());
        props.insert("is_auth_boundary".to_string(), "true".to_string());

        result.symbols.push(Symbol {
            name: set_name.clone(),
            kind: "ash_policy_set".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: props,
            embedding: None,
        });

        result.relations.push(Relation {
            from: module_name.to_string(),
            to: set_name.clone(),
            rel_type: "enforces_policy".to_string(),
            properties: HashMap::new(),
        });

        let Some(do_block) = Self::find_child_by_type(node, "do_block") else {
            return;
        };

        let mut cursor = do_block.walk();
        for child in do_block.named_children(&mut cursor) {
            if child.kind() != "call" {
                continue;
            }
            let Some(ident) = Self::call_identifier(child, source_bytes) else {
                continue;
            };

            if ident == "policy" || ident == "bypass" {
                let is_bypass = ident == "bypass";
                let args_text = Self::find_child_by_type(child, "arguments")
                    .and_then(|a| a.utf8_text(source_bytes).ok())
                    .unwrap_or("");

                let action_desc = if !args_text.is_empty() {
                    args_text
                        .replace(' ', "")
                        .replace(':', "")
                        .replace(['(', ')', '[', ']', ','], "_")
                } else {
                    child.start_position().row.to_string()
                };

                let rule_name = format!("{}.{}:{}", module_name, ident, action_desc);
                let mut rule_props = HashMap::new();
                rule_props.insert("framework".to_string(), "ash".to_string());
                rule_props.insert("is_bypass".to_string(), is_bypass.to_string());
                if !args_text.is_empty() {
                    rule_props.insert("action_type".to_string(), args_text.to_string());
                }

                result.symbols.push(Symbol {
                    name: rule_name.clone(),
                    kind: "policy_rule".to_string(),
                    start_line: child.start_position().row + 1,
                    end_line: child.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: rule_props,
                    embedding: None,
                });

                result.relations.push(Relation {
                    from: set_name.clone(),
                    to: rule_name,
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
    }

    fn extract_plug<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        module_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        let Some(args) = Self::find_child_by_type(node, "arguments") else {
            return;
        };
        let mut ac = args.walk();
        let first_arg = args.named_children(&mut ac).next();
        let Some(first_arg_node) = first_arg else {
            return;
        };

        let raw_plug_name = first_arg_node.utf8_text(source_bytes).unwrap_or("");
        let plug_name = raw_plug_name.trim_start_matches(':').to_string();
        let full_target = aliases
            .get(&plug_name)
            .cloned()
            .unwrap_or_else(|| plug_name.clone());

        let lower = plug_name.to_ascii_lowercase();
        let is_auth = lower.contains("auth")
            || lower.contains("jwt")
            || lower.contains("token")
            || lower.contains("session")
            || lower.contains("guardian")
            || lower.contains("pow")
            || lower.contains("security")
            || lower.contains("permission");

        let mut props = HashMap::new();
        if is_auth {
            props.insert("is_auth_boundary".to_string(), "true".to_string());
            let auth_mech = if lower.contains("jwt") || lower.contains("guardian") {
                "jwt"
            } else if lower.contains("session") {
                "session"
            } else {
                "auth"
            };
            props.insert("auth_mechanism".to_string(), auth_mech.to_string());

            let root_module = if let Some(idx) = module_name.find(".pipeline:") {
                &module_name[..idx]
            } else {
                module_name
            };

            if let Some(mod_sym) = result
                .symbols
                .iter_mut()
                .find(|s| s.name == root_module || s.name == module_name)
            {
                mod_sym
                    .properties
                    .insert("is_auth_boundary".to_string(), "true".to_string());
            }
            if root_module != module_name {
                if let Some(mod_sym) = result.symbols.iter_mut().find(|s| s.name == root_module) {
                    mod_sym
                        .properties
                        .insert("is_auth_boundary".to_string(), "true".to_string());
                }
            }

            let sym_name = format!("{}.plug:{}", module_name, plug_name);
            result.symbols.push(Symbol {
                name: sym_name,
                kind: "plug".to_string(),
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                docstring: None,
                is_entry_point: true,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props.clone(),
                embedding: None,
            });

            result.relations.push(Relation {
                from: root_module.to_string(),
                to: full_target,
                rel_type: "enforces_auth".to_string(),
                properties: props,
            });
        } else {
            let root_module = if let Some(idx) = module_name.find(".pipeline:") {
                &module_name[..idx]
            } else {
                module_name
            };
            result.relations.push(Relation {
                from: root_module.to_string(),
                to: full_target,
                rel_type: "uses_plug".to_string(),
                properties: HashMap::new(),
            });
        }
    }

    fn extract_pipeline<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        content: &str,
        result: &mut ExtractionResult,
        module_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        let pipe_name = Self::find_child_by_type(node, "arguments")
            .and_then(|a| a.utf8_text(source_bytes).ok())
            .map(|s| s.trim().trim_start_matches(':').to_string())
            .unwrap_or_else(|| "pipeline".to_string());

        let lower = pipe_name.to_ascii_lowercase();
        let is_auth = lower.contains("auth")
            || lower.contains("protect")
            || lower.contains("secure")
            || lower.contains("private");

        let mut props = HashMap::new();
        if is_auth {
            props.insert("is_auth_boundary".to_string(), "true".to_string());
        }

        let full_pipe_name = format!("{}.pipeline:{}", module_name, pipe_name);
        result.symbols.push(Symbol {
            name: full_pipe_name.clone(),
            kind: "pipeline".to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            docstring: None,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: props,
            embedding: None,
        });

        result.relations.push(Relation {
            from: module_name.to_string(),
            to: full_pipe_name.clone(),
            rel_type: "contains".to_string(),
            properties: HashMap::new(),
        });

        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            Self::walk(
                do_block,
                source_bytes,
                content,
                result,
                &full_pipe_name,
                &mut Vec::new(),
                aliases,
            );
        }
    }

    // REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity, base 1 +
    // one per branching special form encountered while walking the do_block.
    // Elixir control flow is macro/call-based (case/cond/with/if/unless/for
    // parse as `call` nodes, same as CONTROL_FLOW_FORMS above) rather than
    // dedicated statement kinds, so this mirrors extract_calls_from_block's
    // traversal shape instead of matching tree-sitter node kinds directly.
    // `try` and `receive` are deliberately excluded: unlike case/cond/with
    // whose whole purpose is branching, they are less clearly single decision
    // points in this grammar and are left for a follow-up once measured
    // against real code (same "don't guess a threshold" discipline as the
    // god_objects AND-classification and the GOD_OBJECT_* constants).
    const BRANCHING_FORMS: &[&str] = &["case", "cond", "with", "if", "unless", "for"];

    /// REQ-AXO-902450 — retrieve the body node of a function or macro clause.
    /// Supports both multiline `do ... end` blocks and inline `, do: <expr>` syntax.
    fn find_function_body_node<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
    ) -> Option<(Node<'a>, bool)> {
        if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
            return Some((do_block, true));
        }
        if let Some(args) = Self::find_child_by_type(node, "arguments") {
            let mut cursor = args.walk();
            for child in args.named_children(&mut cursor) {
                if child.kind() == "keywords" {
                    let mut kw_cursor = child.walk();
                    for pair in child.named_children(&mut kw_cursor) {
                        if pair.kind() == "pair" {
                            if let Some(val) = Self::extract_pair_do_value(pair, source_bytes) {
                                return Some((val, false));
                            }
                        }
                    }
                } else if child.kind() == "pair" {
                    if let Some(val) = Self::extract_pair_do_value(child, source_bytes) {
                        return Some((val, false));
                    }
                }
            }
        }
        None
    }

    fn extract_pair_do_value<'a>(pair: Node<'a>, source_bytes: &[u8]) -> Option<Node<'a>> {
        let mut cursor = pair.walk();
        let mut is_do = false;
        let mut value_node = None;
        for child in pair.named_children(&mut cursor) {
            if child.kind() == "keyword" {
                let text = child.utf8_text(source_bytes).unwrap_or("");
                if text.trim().trim_end_matches(':') == "do" {
                    is_do = true;
                }
            } else {
                value_node = Some(child);
            }
        }
        if is_do {
            value_node
        } else {
            None
        }
    }

    fn count_branches<'a>(node: Node<'a>, source_bytes: &[u8]) -> i32 {
        let mut count = 0i32;
        if node.kind() == "call" {
            if let Some(ident) = Self::call_identifier(node, source_bytes) {
                if Self::BRANCHING_FORMS.contains(&ident.as_str()) {
                    count += 1;
                }
                // A nested `def`/`defp`/`defmacro`/`defmacrop` gets its own
                // complexity count when `walk` visits it separately (same
                // discipline as Rust's nested `function_item` exclusion) —
                // never inflate the enclosing function's count with it.
                if matches!(ident.as_str(), "def" | "defp" | "defmacro" | "defmacrop") {
                    return 0;
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            count += Self::count_branches(child, source_bytes);
        }
        count
    }

    /// REQ-AXO-902450 / REQ-AXO-901969 — extract calls from ANY expression node
    /// (either a `do_block` or a one-line `, do: <expr>` body or sub-expression).
    fn extract_calls_from_expression<'a>(
        node: Node<'a>,
        source_bytes: &[u8],
        result: &mut ExtractionResult,
        caller_name: &str,
        aliases: &HashMap<String, String>,
    ) {
        if node.kind() == "call" {
            if let Some(ident) = Self::call_identifier(node, source_bytes) {
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
                    return;
                }
                if IMPORT_DIRECTIVES.contains(&ident.as_str()) {
                    return;
                }
                // Control-flow special forms: descend into body/clauses without emitting a fake call.
                if CONTROL_FLOW_FORMS.contains(&ident.as_str()) {
                    let mut cursor = node.walk();
                    for child in node.named_children(&mut cursor) {
                        Self::extract_calls_from_expression(
                            child,
                            source_bytes,
                            result,
                            caller_name,
                            aliases,
                        );
                    }
                    return;
                }
            }
            Self::extract_generic_call(node, source_bytes, result, caller_name, aliases);
            if let Some(dot_node) = Self::find_child_by_type(node, "dot") {
                let mut cursor = dot_node.walk();
                for child in dot_node.named_children(&mut cursor) {
                    if child.kind() != "identifier"
                        && child.kind() != "alias"
                        && child.kind() != "atom"
                    {
                        Self::extract_calls_from_expression(
                            child,
                            source_bytes,
                            result,
                            caller_name,
                            aliases,
                        );
                    }
                }
            }
            if let Some(args) = Self::find_child_by_type(node, "arguments") {
                Self::extract_calls_from_expression(
                    args,
                    source_bytes,
                    result,
                    caller_name,
                    aliases,
                );
            }
            if let Some(do_block) = Self::find_child_by_type(node, "do_block") {
                Self::extract_calls_from_expression(
                    do_block,
                    source_bytes,
                    result,
                    caller_name,
                    aliases,
                );
            }
        } else {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                Self::extract_calls_from_expression(
                    child,
                    source_bytes,
                    result,
                    caller_name,
                    aliases,
                );
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
                if child.kind() == "alias" || child.kind() == "atom" {
                    receiver = child.utf8_text(source_bytes).unwrap_or("").to_string();
                } else if child.kind() == "identifier" {
                    func_name = child.utf8_text(source_bytes).unwrap_or("").to_string();
                }
            }

            // REQ-AXO-902376 — A dot expression is only a function call if it has
            // an explicit module or atom receiver (e.g. `Module.func` or `:atom.func`).
            // A bare field access (`map.id`, `socket.assigns`, `user.current_org_id`)
            // has identifier-only operands and an empty receiver: emitting a CALLS edge
            // to `.{field}` fabricated tens of thousands of spurious edges across
            // Elixir projects (e.g. 11,040 on APS, 9,540 on TE2).
            if !receiver.is_empty() && !func_name.is_empty() {
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
                    if receiver.starts_with(':') {
                        props.insert("external".to_string(), "true".to_string());
                    }

                    result.relations.push(Relation {
                        from: caller_name.to_string(),
                        to: target,
                        rel_type,
                        properties: props,
                    });

                    let is_pubsub = receiver == "Phoenix.PubSub"
                        || receiver.ends_with("PubSub")
                        || receiver == "Endpoint";
                    let is_gnat = receiver == "Gnat";
                    let is_broadcast =
                        func_name.starts_with("broadcast") || (is_gnat && func_name == "pub");
                    let is_subscribe = func_name == "subscribe" || (is_gnat && func_name == "sub");

                    if (is_pubsub || is_gnat) && (is_broadcast || is_subscribe) {
                        if let Some(args_node) = Self::find_child_by_type(node, "arguments") {
                            for s in Self::find_string_args(args_node, source_bytes) {
                                let start_line = node.start_position().row + 1;
                                let end_line = node.end_position().row + 1;
                                if !result
                                    .symbols
                                    .iter()
                                    .any(|sym| sym.kind == "topic" && sym.name == s)
                                {
                                    result.symbols.push(Symbol {
                                        name: s.clone(),
                                        kind: "topic".to_string(),
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
                                }
                                result.relations.push(Relation {
                                    from: caller_name.to_string(),
                                    to: s,
                                    rel_type: if is_broadcast {
                                        "publishes_to".to_string()
                                    } else {
                                        "subscribes_to".to_string()
                                    },
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }

                    if func_name == "new"
                        && (resolved_receiver.ends_with("Worker")
                            || resolved_receiver.contains(".Workers.")
                            || receiver.ends_with("Worker"))
                    {
                        result.relations.push(Relation {
                            from: caller_name.to_string(),
                            to: resolved_receiver.clone(),
                            rel_type: "dispatches_job".to_string(),
                            properties: HashMap::new(),
                        });
                    }
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
        Self::extract_def_head_name(args, source_bytes)
    }

    /// REQ-AXO-902532 — a guarded definition is wrapped by the Elixir grammar:
    /// `def f(args) when guard(args)` puts the function-head `call` below a
    /// `binary_operator` instead of directly below `arguments`.  Looking only
    /// at direct children therefore skipped selected public functions while the
    /// file and their unguarded neighbours indexed normally.
    ///
    /// Walk the definition HEAD in source order and return the first callable
    /// name.  This helper receives only the outer `def` arguments, never its
    /// `do_block`, so calls in the function body cannot be mistaken for the
    /// definition name.  On a guard, the left-hand function call precedes the
    /// right-hand guard call by construction.
    fn extract_def_head_name<'a>(node: Node<'a>, source_bytes: &[u8]) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "call" {
                if let Some(ident) = Self::call_identifier(child, source_bytes) {
                    return Some(ident);
                }
            } else if child.kind() == "identifier" || child.kind() == "alias" {
                return Some(child.utf8_text(source_bytes).unwrap_or("").to_string());
            }
            if let Some(name) = Self::extract_def_head_name(child, source_bytes) {
                return Some(name);
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
    fn req_902532_guarded_controller_function_is_not_skipped() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule APS3DWeb.MESController do
          alias APS3D.MES.{SCADAInterface, OPCUAClient}

          def read_tag(conn, %{"name" => name}) do
            SCADAInterface.read_tag(org(conn), name)
          end

          def read_tags(conn, %{"names" => names}) when is_list(names) do
            case SCADAInterface.read_tags(org(conn), names) do
              {:ok, tags} -> json(conn, %{data: tags})
              {:error, reason} ->
                conn
                |> put_status(500)
                |> json(%{error: format_error(reason)})
            end
          end

          def update_tag(conn, %{"name" => name, "value" => value}) do
            SCADAInterface.update_tag(org(conn), name, value)
          end

          def opcua_read(conn, %{"node_ids" => node_ids}) when is_list(node_ids) do
            OPCUAClient.read(org(conn), node_ids)
          end

          def opcua_read(conn, %{"node_id" => node_id}) do
            OPCUAClient.read(org(conn), node_id)
          end
        end
        "#;

        let result = parser.parse(content);
        let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"APS3DWeb.MESController.read_tags"),
            "guarded read_tags missing; symbols={names:?}"
        );
        for adjacent in ["read_tag", "update_tag", "opcua_read"] {
            assert!(
                names.contains(&format!("APS3DWeb.MESController.{adjacent}").as_str()),
                "adjacent function {adjacent} regressed; symbols={names:?}"
            );
        }
        for callee in [
            "APS3D.MES.SCADAInterface.read_tags",
            "APS3DWeb.MESController.org",
            "APS3DWeb.MESController.json",
            "APS3DWeb.MESController.put_status",
            "APS3DWeb.MESController.format_error",
        ] {
            assert!(
                result.relations.iter().any(|rel| {
                    rel.from == "APS3DWeb.MESController.read_tags"
                        && rel.to == callee
                        && rel.rel_type == "CALLS"
                }),
                "read_tags -> {callee} missing; relations={:?}",
                result.relations
            );
        }
    }

    #[test]
    fn req_902532_guarded_function_with_struct_pattern_is_not_skipped() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule APS3D.Scheduling.Model do
          def count_site_transfers(%__MODULE__{} = model, assignments)
              when is_list(assignments) do
            calculate_transfers(model, assignments)
          end
        end
        "#;

        let result = parser.parse(content);
        assert!(
            result
                .symbols
                .iter()
                .any(|symbol| symbol.name == "APS3D.Scheduling.Model.count_site_transfers"),
            "guarded struct-pattern function missing; symbols={:?}",
            result.symbols
        );
        assert!(
            result.relations.iter().any(|rel| {
                rel.from == "APS3D.Scheduling.Model.count_site_transfers"
                    && rel.to == "APS3D.Scheduling.Model.calculate_transfers"
                    && rel.rel_type == "CALLS"
            }),
            "guarded struct-pattern body was not attributed; relations={:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_resolves_multi_alias_to_canonical_symbols() {
        // REQ-AXO-902223 — `alias A.B.{C, D}` must resolve BOTH short names to
        // their canonical FQN so the cross-module CALLS edges land on the real
        // symbols instead of dangling `C`/`D` (the ~78% dangling class that made
        // `wiring` see only test callers).
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Nexus.Parliament do
          alias Nexus.Agents.{Router, Memory}

          def dispatch do
            Router.route()
            Memory.store()
          end
        end
        "#;

        let result = parser.parse(content);

        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Nexus.Parliament.dispatch"
                    && rel.to == "Nexus.Agents.Router.route"
                    && rel.rel_type == "CALLS"),
            "multi-alias first member unresolved; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Nexus.Parliament.dispatch"
                    && rel.to == "Nexus.Agents.Memory.store"
                    && rel.rel_type == "CALLS"),
            "multi-alias second member unresolved; got: {:?}",
            result.relations
        );
        assert!(
            !result
                .relations
                .iter()
                .any(|rel| (rel.to == "Router" || rel.to == "Memory") && rel.rel_type == "CALLS"),
            "dangling short-name target still emitted: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_resolves_as_rename_alias_to_canonical_symbol() {
        // REQ-AXO-902223 — `alias A.B.C, as: X` must map the RENAME `X` to the
        // canonical FQN, so `X.fun()` resolves to `A.B.C.fun`.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Nexus.Controller do
          alias Nexus.Governance.LegalSources, as: Sources

          def index do
            Sources.list()
          end
        end
        "#;

        let result = parser.parse(content);

        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Nexus.Controller.index"
                    && rel.to == "Nexus.Governance.LegalSources.list"
                    && rel.rel_type == "CALLS"),
            "as: rename unresolved; got: {:?}",
            result.relations
        );
        assert!(
            !result
                .relations
                .iter()
                .any(|rel| rel.to == "Sources" && rel.rel_type == "CALLS"),
            "dangling rename target still emitted: {:?}",
            result.relations
        );
    }

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
            result
                .relations
                .iter()
                .any(|rel| rel.from == "FiscalyCore.Governance.LegalSources.list"
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
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
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
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
                    && rel.to == "Axon.Sample.prepare_dataset"
                    && rel.rel_type == "CALLS"),
            "call inside `with` head missing; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
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
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
                    && rel.to == "Axon.Sample.prepare_dataset"
                    && rel.rel_type == "CALLS"),
            "piped call prepare_dataset missing; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
                    && rel.to == "Axon.Sample.label_multi_horizon"
                    && rel.rel_type == "CALLS"),
            "piped call label_multi_horizon missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_elixir_parser_impl_annotation_marks_custom_callback_as_entry_point() {
        // REQ-AXO-902227 — `@impl` is the framework-AGNOSTIC entry-point signal.
        // A project-defined behaviour callback (`@impl MyApp.Brain def think`) whose
        // name is NOT in ELIXIR_ENTRY_POINTS must still be is_entry_point; a plain
        // public fn must not; and `@impl false` (explicit non-callback) must not.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Agent do
          @behaviour MyApp.Brain

          @impl MyApp.Brain
          def think(state), do: state

          @doc "public but not a callback"
          def plain_helper(x), do: x

          @impl false
          def not_a_callback(y), do: y
        end
        "#;
        let result = parser.parse(content);
        let is_entry = |suffix: &str| {
            result
                .symbols
                .iter()
                .find(|s| s.name.ends_with(suffix))
                .unwrap_or_else(|| panic!("symbol {suffix} missing; got {:?}", result.symbols))
                .is_entry_point
        };
        assert!(
            is_entry(".think"),
            "@impl-annotated custom callback must be an entry point"
        );
        assert!(
            !is_entry(".plain_helper"),
            "a non-@impl public fn must NOT be an entry point"
        );
        assert!(
            !is_entry(".not_a_callback"),
            "@impl false must NOT count as an entry point"
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
    fn test_elixir_parser_simple_function_has_complexity_one() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Axon.Sample do
          def f(x) do
            x + 1
          end
        end
        "#;
        let result = parser.parse(content);
        let f = result
            .symbols
            .iter()
            .find(|s| s.name == "Axon.Sample.f")
            .expect("f symbol");
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn test_elixir_parser_branching_function_counts_each_decision_point() {
        let parser = ElixirParser::new();
        // 1 (base) + case + if + for = 4.
        let content = r#"
        defmodule Axon.Sample do
          def f(x) do
            case x do
              :a -> :ok
              _ -> :err
            end
            if x > 0 do
              :pos
            end
            for i <- x, do: i
          end
        end
        "#;
        let result = parser.parse(content);
        let f = result
            .symbols
            .iter()
            .find(|s| s.name == "Axon.Sample.f")
            .expect("f symbol");
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("4"),
            "props: {:?}",
            f.properties
        );
    }

    #[test]
    fn test_elixir_parser_nested_def_gets_its_own_complexity_not_added_to_parent() {
        let parser = ElixirParser::new();
        // outer: base 1 + 1 if = 2. inner (nested defp): base 1 + 1 if = 2.
        let content = r#"
        defmodule Axon.Sample do
          def outer(x) do
            if x > 0 do
              :pos
            end
          end

          defp inner(y) do
            if y > 0 do
              :pos
            end
          end
        end
        "#;
        let result = parser.parse(content);
        let outer = result
            .symbols
            .iter()
            .find(|s| s.name == "Axon.Sample.outer")
            .expect("outer symbol");
        let inner = result
            .symbols
            .iter()
            .find(|s| s.name == "Axon.Sample.inner")
            .expect("inner symbol");
        assert_eq!(
            outer
                .properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("2")
        );
        assert_eq!(
            inner
                .properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("2")
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
            result
                .relations
                .iter()
                .any(|rel| rel.from == "Axon.Sample.run"
                    && rel.to == "Axon.Sample.prepare_dataset"
                    && rel.rel_type == "CALLS"),
            "call inside anonymous fn argument missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902450_multi_clause_and_one_line_do_extracts_calls_and_callers() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule T do
          def entry(x), do: pick(x)
          defp pick(%{a: _} = x), do: alpha(x)
          defp pick(x), do: beta(x)
          defp alpha(_), do: :a
          defp beta(_), do: :b
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "T.entry" && rel.to == "T.pick" && rel.rel_type == "CALLS"),
            "entry -> pick missing; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "T.pick" && rel.to == "T.alpha" && rel.rel_type == "CALLS"),
            "pick -> alpha missing; got: {:?}",
            result.relations
        );
        assert!(
            result
                .relations
                .iter()
                .any(|rel| rel.from == "T.pick" && rel.to == "T.beta" && rel.rel_type == "CALLS"),
            "pick -> beta missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902376_field_access_is_not_emitted_as_calls_edge() {
        // REQ-AXO-902376 — field accesses (e.g. data.id, socket.assigns, user.current_org_id)
        // must NOT be emitted as CALLS edges (.id, .assigns, etc.).
        // Real remote function calls (Accounts.notify) and Erlang calls (:crypto.strong_rand_bytes)
        // must still be properly extracted.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule Sample do
          def process(socket, user, data) do
            assigns = socket.assigns
            user_id = user.id
            status = data.status
            current_org_id = user.current_org_id

            token = :crypto.strong_rand_bytes(16)
            Accounts.notify(user_id, status)
            handle_status(status)
          end

          def handle_status(s), do: s
        end
        "#;
        let result = parser.parse(content);

        // No CALLS edge should target a bare leading dot (field access)
        let spurious_field_calls: Vec<&str> = result
            .relations
            .iter()
            .filter(|r| r.rel_type == "CALLS" && r.to.starts_with('.'))
            .map(|r| r.to.as_str())
            .collect();
        assert!(
            spurious_field_calls.is_empty(),
            "spurious field accesses emitted as CALLS edges: {:?}",
            spurious_field_calls
        );

        // Real calls must be present
        assert!(
            result.relations.iter().any(|r| r.from == "Sample.process"
                && r.to == "Accounts.notify"
                && r.rel_type == "CALLS"),
            "Accounts.notify call missing; got: {:?}",
            result.relations
        );
        assert!(
            result.relations.iter().any(|r| r.from == "Sample.process"
                && r.to == ":crypto.strong_rand_bytes"
                && r.rel_type == "CALLS"),
            ":crypto.strong_rand_bytes call missing; got: {:?}",
            result.relations
        );
        assert!(
            result.relations.iter().any(|r| r.from == "Sample.process"
                && r.to == "Sample.handle_status"
                && r.rel_type == "CALLS"),
            "Sample.handle_status call missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902421_calls_inside_macro_block_with_arguments_without_parens_are_extracted() {
        // REQ-AXO-902421 / TE2 #182: Calls inside macro do-blocks where arguments have no parens
        // (e.g. `Tracer.with_span "order_execution" do ... ExecutionRouter.open_position(...) end`)
        // must be extracted and resolve aliases properly.
        let parser = ElixirParser::new();
        let content = r#"
        defmodule TraderElixirV2.Agents.CryptoAgent do
          alias TraderElixirV2.Trading.ExecutionRouter

          def executing(:internal, :execute_order, data) do
            Tracer.with_span "order_execution" do
              Logger.info("[#{data.symbol}] Executing order via ExecutionRouter...")
              side = data.last_signal[:side] || :buy
              sent_at_us = System.monotonic_time(:microsecond)
              emit_decision_to_send(data, sent_at_us)

              result =
                ExecutionRouter.open_position(
                  data.paper_trading_id,
                  data.symbol,
                  side,
                  data.last_signal.size,
                  data.last_signal.price,
                  decided_at_us: data.last_signal[:decided_at_us],
                  sent_at_us: sent_at_us,
                  reference_price: data.last_signal.price,
                  campaign_id: optional_config([:trading, :campaign_id]),
                  strategy_version: optional_config([:trading, :strategy_version]),
                  hypothesis_id: data.last_signal[:hypothesis_id],
                  regime: data.macro_regime,
                  admission: data.last_signal[:admission],
                  sizing_policy: data.last_signal[:sizing_policy]
                )

              case result do
                :ok -> :ok
              end
            end
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.from == "TraderElixirV2.Agents.CryptoAgent.executing"
                    && r.to == "TraderElixirV2.Trading.ExecutionRouter.open_position"
                    && r.rel_type == "CALLS"),
            "ExecutionRouter.open_position inside Tracer.with_span do-block missing; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902421_moduledoc_examples_are_never_counted_as_calls() {
        // REQ-AXO-902421 criterion 4: An occurrence in a @moduledoc is NEVER counted
        // as a caller (trap reported by TE2 on ModelVersioning.register).
        let parser = ElixirParser::new();
        let content = r#"
        defmodule TraderElixirV2.ML.ModelVersioning do
          @moduledoc """
          Example:
            ModelVersioning.register(model_id, weights)
            register(foo)
          """

          def register(model_id, weights) do
            :ok
          end
        end
        "#;
        let result = parser.parse(content);
        assert!(
            !result
                .relations
                .iter()
                .any(|r| r.to.contains("register") && r.rel_type == "CALLS"),
            "Calls inside @moduledoc must never be emitted; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902660_exunit_test_macro_emits_tested_symbol_and_calls() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule CalculatorTest do
          use ExUnit.Case

          test "adds numbers correctly" do
            assert Calculator.add(1, 2) == 3
          end
        end
        "#;
        let result = parser.parse(content);
        let test_sym = result
            .symbols
            .iter()
            .find(|s| s.name.contains("adds numbers correctly"))
            .expect("ExUnit test macro must be extracted as a symbol");
        assert!(test_sym.tested, "ExUnit test symbol must have tested=true");
        assert_eq!(test_sym.kind, "function");

        assert!(
            result.relations.iter().any(|r| r.from == test_sym.name
                && r.to.contains("Calculator.add")
                && r.rel_type.to_lowercase() == "calls"),
            "Calls inside ExUnit test must be linked to the test symbol; got: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902663_elixir_ecto_schema_and_relations() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Accounts.User do
          use Ecto.Schema
          import Ecto.Changeset

          alias MyApp.Accounts.Organization
          alias MyApp.Blog.Post

          schema "users" do
            field :name, :string
            field :email, :string
            belongs_to :organization, Organization
            has_many :posts, Post
            has_one :profile, MyApp.Accounts.Profile
            timestamps()
          end
        end
        "#;
        let result = parser.parse(content);

        // Schema symbol
        let schema_sym = result
            .symbols
            .iter()
            .find(|s| s.kind == "schema")
            .expect("Ecto schema must be extracted as a symbol");
        assert_eq!(
            schema_sym.properties.get("table").map(|s| s.as_str()),
            Some("users")
        );

        // Field symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name.contains("MyApp.Accounts.User.name")
                && s.kind == "field"
                && s.properties.get("type") == Some(&":string".to_string())));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name.contains("MyApp.Accounts.User.email") && s.kind == "field"));

        // Schema references table
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "MyApp.Accounts.User"
                && r.to == "users"
                && r.rel_type == "references"));

        // belongs_to resolved via alias
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "MyApp.Accounts.User"
                && r.to == "MyApp.Accounts.Organization"
                && r.rel_type == "references"
                && r.properties.get("relation") == Some(&"belongs_to".to_string())));

        // has_many resolved via alias
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "MyApp.Accounts.User"
                && r.to == "MyApp.Blog.Post"
                && r.rel_type == "references"
                && r.properties.get("relation") == Some(&"has_many".to_string())));

        // has_one with full module name
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "MyApp.Accounts.User"
                && r.to == "MyApp.Accounts.Profile"
                && r.rel_type == "references"
                && r.properties.get("relation") == Some(&"has_one".to_string())));
    }

    #[test]
    fn test_mix_exs_deps_extraction() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.MixProject do
          use Mix.Project

          def project do
            [
              app: :my_app,
              version: "0.1.0",
              deps: deps()
            ]
          end

          defp deps do
            [
              {:phoenix, "~> 1.7.10"},
              {:ecto_sql, "~> 3.10"},
              {:postgrex, ">= 0.0.0"},
              {:jason, "~> 1.2"}
            ]
          end
        end
        "#;

        let result = parser.parse(content);
        let deps: Vec<&str> = result
            .symbols
            .iter()
            .filter(|s| s.kind == "dependency")
            .map(|s| s.name.as_str())
            .collect();

        assert!(
            deps.contains(&"phoenix"),
            "should contain phoenix: {deps:?}"
        );
        assert!(
            deps.contains(&"ecto_sql"),
            "should contain ecto_sql: {deps:?}"
        );
        assert!(
            deps.contains(&"postgrex"),
            "should contain postgrex: {deps:?}"
        );
        assert!(deps.contains(&"jason"), "should contain jason: {deps:?}");

        let refs: Vec<&str> = result
            .relations
            .iter()
            .filter(|r| r.rel_type == "references" && r.from == "MyApp.MixProject")
            .map(|r| r.to.as_str())
            .collect();
        assert!(refs.contains(&"phoenix"));
        assert!(refs.contains(&"ecto_sql"));
    }

    #[test]
    fn test_tranche7_oban_worker_and_dispatch() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Workers.Mailer do
          use Oban.Worker, queue: :mailers, max_attempts: 5

          @impl Oban.Worker
          def perform(%Oban.Job{args: args}) do
            deliver(args)
          end
        end

        defmodule MyApp.Accounts do
          alias MyApp.Workers.Mailer

          def register(user) do
            Mailer.new(%{email: user.email})
            |> Oban.insert()
          end
        end
        "#;

        let result = parser.parse(content);
        let worker_sym = result
            .symbols
            .iter()
            .find(|s| s.kind == "worker" && s.name == "MyApp.Workers.Mailer.worker");
        assert!(worker_sym.is_some(), "worker symbol should be extracted");
        let sym = worker_sym.unwrap();
        assert_eq!(sym.properties.get("queue"), Some(&"mailers".to_string()));
        assert_eq!(sym.properties.get("max_attempts"), Some(&"5".to_string()));

        let dispatch_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "dispatches_job" && r.to == "MyApp.Workers.Mailer");
        assert!(
            dispatch_rel.is_some(),
            "dispatches_job relation should be extracted to MyApp.Workers.Mailer, got: {:?}",
            result.relations
        );
    }

    #[test]
    fn test_tranche7_pubsub_kafka_nats_topics() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Events do
          def notify_order(order) do
            Phoenix.PubSub.broadcast(MyApp.PubSub, "orders:created", {:order, order})
            Gnat.pub(:gnat, "orders.nats", "payload")
          end

          def subscribe_events do
            Phoenix.PubSub.subscribe(MyApp.PubSub, "events:all")
            Gnat.sub(:gnat, self(), "orders.nats")
          end
        end
        "#;

        let result = parser.parse(content);
        let topic_names: Vec<&str> = result
            .symbols
            .iter()
            .filter(|s| s.kind == "topic")
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            topic_names.contains(&"orders:created"),
            "should extract orders:created topic: {topic_names:?}"
        );
        assert!(
            topic_names.contains(&"orders.nats"),
            "should extract orders.nats topic: {topic_names:?}"
        );
        assert!(
            topic_names.contains(&"events:all"),
            "should extract events:all topic: {topic_names:?}"
        );

        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "publishes_to" && r.to == "orders:created"),
            "should have publishes_to orders:created"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "publishes_to" && r.to == "orders.nats"),
            "should have publishes_to orders.nats"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "subscribes_to" && r.to == "events:all"),
            "should have subscribes_to events:all"
        );
    }

    #[test]
    fn test_tranche7_commanded_cqrs_event_sourcing() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Router do
          use Commanded.Commands.Router

          dispatch MyApp.OpenAccount, to: MyApp.AccountAggregate, identity: :account_id
        end

        defmodule MyApp.AccountAggregate do
          def execute(%MyApp.AccountAggregate{}, %MyApp.OpenAccount{} = cmd) do
            %MyApp.AccountOpened{account_id: cmd.account_id}
          end

          def apply(%MyApp.AccountAggregate{} = state, %MyApp.AccountOpened{} = evt) do
            %MyApp.AccountAggregate{state | account_id: evt.account_id}
          end
        end

        defmodule MyApp.AccountProjector do
          use Commanded.Projections.Ecto, name: "AccountProjector"
        end
        "#;

        let result = parser.parse(content);
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "dispatches_command" && r.to == "MyApp.OpenAccount"),
            "router should dispatch command MyApp.OpenAccount"
        );
        assert!(
            result.relations.iter().any(|r| r.rel_type == "routes_to"
                && r.from == "MyApp.OpenAccount"
                && r.to == "MyApp.AccountAggregate"),
            "command should route to MyApp.AccountAggregate"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "handles_command" && r.to == "MyApp.OpenAccount"),
            "aggregate should handle command MyApp.OpenAccount"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "applies_event" && r.to == "MyApp.AccountOpened"),
            "aggregate should apply event MyApp.AccountOpened"
        );
        assert!(
            result
                .symbols
                .iter()
                .any(|s| s.kind == "projector" && s.name == "MyApp.AccountProjector.projector"),
            "projector symbol should be extracted"
        );
    }

    #[test]
    fn test_tranche7_phoenix_channel_websocket() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyAppWeb.RoomChannel do
          use Phoenix.Channel

          def join("room:lobby", _message, socket) do
            {:ok, socket}
          end

          def handle_in("new_msg", %{"body" => body}, socket) do
            broadcast!(socket, "new_msg", %{body: body})
            {:noreply, socket}
          end
        end
        "#;

        let result = parser.parse(content);
        assert!(
            result
                .symbols
                .iter()
                .any(|s| s.kind == "channel" && s.name == "MyAppWeb.RoomChannel.channel"),
            "channel symbol should be extracted"
        );
        assert!(
            result
                .symbols
                .iter()
                .any(|s| s.kind == "topic" && s.name == "room:lobby"),
            "topic room:lobby should be extracted"
        );
        assert!(
            result
                .relations
                .iter()
                .any(|r| r.rel_type == "subscribes_to" && r.to == "room:lobby"),
            "channel should subscribe to room:lobby"
        );
    }

    #[test]
    fn test_elixir_security_policies_plugs_and_pii() {
        let parser = ElixirParser::new();
        let content = r#"
        defmodule MyApp.Accounts.User do
          use Ecto.Schema
          use Ash.Resource

          schema "users" do
            field :email, :string
            field :password_hash, :string, redact: true
            field :credit_card, :string
          end

          policies do
            policy action_type(:read) do
              authorize_if always()
            end

            bypass actor_attribute_equals(:admin, true) do
              authorize_if always()
            end
          end
        end

        defmodule MyAppWeb.Router do
          use Phoenix.Router

          pipeline :authenticated do
            plug MyAppWeb.AuthPlug
            plug Guardian.Plug.EnsureAuthenticated
          end

          scope "/api", MyAppWeb do
            pipe_through :authenticated
            get "/users", UserController, :index
          end
        end
        "#;

        let result = parser.parse(content);

        // 1. Verify Ecto sensitive fields & schema marking
        let schema_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Accounts.User.schema")
            .expect("schema symbol");
        assert_eq!(
            schema_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pwd_field = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Accounts.User.password_hash")
            .expect("password_hash field");
        assert_eq!(
            pwd_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let cc_field = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Accounts.User.credit_card")
            .expect("credit_card field");
        assert_eq!(
            cc_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            cc_field.properties.get("pii_kind").map(|s| s.as_str()),
            Some("financial")
        );

        // 2. Verify Ash policies
        let policy_set = result
            .symbols
            .iter()
            .find(|s| s.name == "MyApp.Accounts.User.policies")
            .expect("ash policy set");
        assert_eq!(
            policy_set
                .properties
                .get("is_auth_boundary")
                .map(|s| s.as_str()),
            Some("true")
        );
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "MyApp.Accounts.User"
                && r.to == "MyApp.Accounts.User.policies"
                && r.rel_type == "enforces_policy"));

        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "policy_rule" && s.name.contains("policy")));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.kind == "policy_rule" && s.name.contains("bypass")));

        // 3. Verify Plugs & Router auth boundary
        let router_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MyAppWeb.Router")
            .expect("router symbol");
        assert_eq!(
            router_sym
                .properties
                .get("is_auth_boundary")
                .map(|s| s.as_str()),
            Some("true")
        );

        assert!(result.relations.iter().any(|r| r.from == "MyAppWeb.Router"
            && r.to == "MyAppWeb.AuthPlug"
            && r.rel_type == "enforces_auth"));
        assert!(result.relations.iter().any(|r| r.from == "MyAppWeb.Router"
            && r.to == "Guardian.Plug.EnsureAuthenticated"
            && r.rel_type == "enforces_auth"));
    }
}
