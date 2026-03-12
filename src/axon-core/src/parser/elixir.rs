use std::collections::HashMap;
use tree_sitter::{Language, Parser as TSParser};
use super::{Parser, ExtractionResult, Symbol, Relation};

pub struct ElixirParser {
    language: Language,
}

impl ElixirParser {
    pub fn new() -> Self {
        Self {
            language: unsafe { std::mem::transmute(tree_sitter_elixir::language()) },
        }
    }

    fn walk_ast(
        cursor: &mut tree_sitter::TreeCursor,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        current_module: Option<&str>,
    ) {
        let node = cursor.node();
        let mut next_module = current_module.map(|s| s.to_string());

        if node.kind() == "call" {
            if let Some(target) = node.child_by_field_name("target") {
                if let Ok(target_name) = target.utf8_text(content) {
                    if target_name == "defmodule" {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let mut w = args.walk();
                            for arg_child in args.children(&mut w) {
                                if arg_child.kind() == "alias" {
                                    if let Ok(mod_name) = arg_child.utf8_text(content) {
                                        next_module = Some(mod_name.to_string());
                                        symbols.push(Symbol {
                                            name: mod_name.to_string(),
                                            kind: "module".to_string(),
                                            start_line: node.start_position().row + 1,
                                            end_line: node.end_position().row + 1,
                                            docstring: None,
                                            is_entry_point: false,
                                            properties: HashMap::new(),
                                        });
                                    }
                                }
                            }
                        }
                    } else if ["def", "defp", "defmacro", "defmacrop"].contains(&target_name) {
                        if let Some(args) = node.child_by_field_name("arguments") {
                            let mut func_name = String::new();
                            let mut w = args.walk();
                            for arg_child in args.children(&mut w) {
                                if arg_child.kind() == "call" {
                                    if let Some(ft) = arg_child.child_by_field_name("target") {
                                        if ft.kind() == "identifier" {
                                            if let Ok(n) = ft.utf8_text(content) {
                                                func_name = n.to_string();
                                            }
                                        }
                                    }
                                } else if arg_child.kind() == "identifier" {
                                    if let Ok(n) = arg_child.utf8_text(content) {
                                        func_name = n.to_string();
                                    }
                                }
                            }

                            if !func_name.is_empty() {
                                let full_name = match &next_module {
                                    Some(m) => format!("{}.{}", m, func_name),
                                    None => func_name.clone(),
                                };

                                let is_entry = ["handle_call", "handle_cast", "handle_info", "handle_continue", "init", "start_link"]
                                    .contains(&func_name.as_str());

                                let mut properties = HashMap::new();

                                if let Ok(node_text) = node.utf8_text(content) {
                                    if node_text.contains("load_nif") {
                                        properties.insert("nif_loader".to_string(), "true".to_string());
                                    }
                                }

                                symbols.push(Symbol {
                                    name: full_name,
                                    kind: if target_name.starts_with("defmacro") {
                                        "macro".to_string()
                                    } else {
                                        "function".to_string()
                                    },
                                    start_line: node.start_position().row + 1,
                                    end_line: node.end_position().row + 1,
                                    docstring: None,
                                    is_entry_point: is_entry,
                                    properties,
                                });
                            }
                        }
                    } else if target.kind() == "dot" {
                        let mut w = target.walk();
                        let mut alias = "";
                        let mut method = "";
                        for child in target.children(&mut w) {
                            if child.kind() == "alias" {
                                if let Ok(a) = child.utf8_text(content) {
                                    alias = a;
                                }
                            } else if child.kind() == "identifier" {
                                if let Ok(m) = child.utf8_text(content) {
                                    method = m;
                                }
                            }
                        }

                        if alias == "GenServer" && (method == "call" || method == "cast") {
                            let mut props = HashMap::new();
                            props.insert("genserver".to_string(), "true".to_string());
                            let from = current_module.unwrap_or("unknown");
                            relations.push(Relation {
                                from: from.to_string(),
                                to: format!("{}.{}", alias, method),
                                rel_type: "CALLS".to_string(),
                                properties: props,
                            });
                        }
                    }
                }
            }
        } else if node.kind() == "unary_operator" {
            let mut w = node.walk();
            for child in node.children(&mut w) {
                if child.kind() == "call" {
                    if let Some(target) = child.child_by_field_name("target") {
                        if let Ok(name) = target.utf8_text(content) {
                            if name == "behaviour" {
                                if let Some(args) = child.child_by_field_name("arguments") {
                                    let mut aw = args.walk();
                                    for a in args.children(&mut aw) {
                                        if a.kind() == "alias" {
                                            if let Ok(beh_name) = a.utf8_text(content) {
                                                if let Some(cm) = current_module {
                                                    relations.push(Relation {
                                                        from: cm.to_string(),
                                                        to: beh_name.to_string(),
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
                    }
                }
            }
        }

        if cursor.goto_first_child() {
            loop {
                Self::walk_ast(cursor, content, symbols, relations, next_module.as_deref());
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }
}

unsafe impl Send for ElixirParser {}
unsafe impl Sync for ElixirParser {}

impl Parser for ElixirParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(self.language).unwrap();
        let tree = match parser.parse(content, None) {
            Some(t) => t,
            None => return ExtractionResult { symbols: vec![], relations: vec![] },
        };

        let mut symbols = Vec::new();
        let mut relations = Vec::new();
        let mut cursor = tree.walk();
        
        Self::walk_ast(&mut cursor, content.as_bytes(), &mut symbols, &mut relations, None);

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elixir_parser() {
        let code = r#"
defmodule MySystem.Worker do
  @behaviour GenServer

  def start_link(args) do
    GenServer.start_link(__MODULE__, args)
  end

  def init(state) do
    {:ok, state}
  end

  def handle_call(:do_work, _from, state) do
    GenServer.call(OtherWorker, :work)
    {:reply, :ok, state}
  end
  
  defp private_helper do
    :erlang.load_nif('./nif', 0)
  end
  
  defmacro my_macro(ast) do
    quote do: unquote(ast)
  end
end
"#;
        let parser = ElixirParser::new();
        let result = parser.parse(code);

        // Check module
        let modules: Vec<_> = result.symbols.iter().filter(|s| s.kind == "module").collect();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "MySystem.Worker");

        // Check functions
        let funcs: Vec<_> = result.symbols.iter().filter(|s| s.kind == "function").collect();
        let init_func = funcs.iter().find(|f| f.name.contains("init")).unwrap();
        assert!(init_func.is_entry_point);

        let handle_call_func = funcs.iter().find(|f| f.name.contains("handle_call")).unwrap();
        assert!(handle_call_func.is_entry_point);

        let priv_helper = funcs.iter().find(|f| f.name.contains("private_helper")).unwrap();
        assert_eq!(priv_helper.properties.get("nif_loader").map(|s| s.as_str()), Some("true"));

        // Check macros
        let macros: Vec<_> = result.symbols.iter().filter(|s| s.kind == "macro").collect();
        assert_eq!(macros.len(), 1);
        assert!(macros[0].name.contains("my_macro"));

        // Check relations
        let genserver_calls: Vec<_> = result.relations.iter().filter(|r| r.rel_type == "CALLS").collect();
        assert!(!genserver_calls.is_empty());
        assert_eq!(genserver_calls[0].properties.get("genserver").map(|s| s.as_str()), Some("true"));
        
        let implements: Vec<_> = result.relations.iter().filter(|r| r.rel_type == "implements").collect();
        assert_eq!(implements.len(), 1);
        assert_eq!(implements[0].to, "GenServer");
    }
}
