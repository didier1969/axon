//! Scheme / Atomese parser (REQ-AXO-901910).
//!
//! Wraps the `6cdh/tree-sitter-scheme` grammar (ABI 14, built with the 0.23.2
//! CLI to match the runtime). The grammar is *list-level*: it yields
//! `program` / `list` / `symbol` / `string` / `number` / `boolean` … nodes and
//! carries no Scheme or Atomese semantics. All meaning is recovered here, in
//! the walker, by interpreting the head of each `list`:
//!
//!  - layer (a) generic Scheme: `(define …)` / `define-public` / `define-syntax`
//!    / `define-record-type` → a `Symbol`.
//!  - layer (b) Atomese: a list whose head symbol ends in `Node` / `Link` /
//!    `Value` is an OpenCog atom. Named atoms (those carrying a string label)
//!    become `Symbol`s; `*Link` forms additionally emit `Relation`s wiring the
//!    atoms they contain — i.e. the hypergraph edges. The `Node`/`Link` suffix
//!    convention covers the vast majority of the atom-type taxonomy without
//!    needing `atom_types.script` (precise typing is a later refinement).

use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::Node;

pub struct SchemeParser {
    wasm_bytes: &'static [u8],
}

impl SchemeParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-scheme.wasm"),
        }
    }

    /// Named children of a node (skips the anonymous `(` `)` tokens).
    fn named<'a>(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    }

    fn text(&self, node: Node, source: &[u8]) -> String {
        node.utf8_text(source).unwrap_or("").to_string()
    }

    /// String literal payload without the surrounding quotes (the grammar keeps
    /// them in the `string` node text).
    fn string_value(&self, node: Node, source: &[u8]) -> String {
        let raw = self.text(node, source);
        raw.trim_matches('"').to_string()
    }

    fn walk(&self, node: Node, source: &[u8], result: &mut ExtractionResult) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "list" {
                self.handle_list(child, source, result);
            } else {
                self.walk(child, source, result);
            }
        }
    }

    fn handle_list(&self, list: Node, source: &[u8], result: &mut ExtractionResult) {
        let items = self.named(list);
        let Some(head) = items.first() else {
            return;
        };

        if head.kind() == "symbol" {
            let head_text = self.text(*head, source);
            match head_text.as_str() {
                "define" | "define*" | "define-public" | "define-syntax" | "define-syntax-rule"
                | "define-record-type" | "define-method" => {
                    self.extract_define(&head_text, list, &items, source, result);
                }
                _ if Self::is_atomese_type(&head_text) => {
                    self.extract_atomese(&head_text, list, &items, source, result);
                }
                _ => {}
            }
        }

        // Always descend: definitions and atoms nest arbitrarily deep.
        for child in &items {
            if child.kind() == "list" {
                self.handle_list(*child, source, result);
            } else {
                self.walk(*child, source, result);
            }
        }
    }

    /// Atomese atom types end in `Node`, `Link`, or `Value` by convention
    /// (ConceptNode, InheritanceLink, FloatValue, …). Require a leading
    /// uppercase to avoid matching ordinary identifiers like `add-link`.
    fn is_atomese_type(head: &str) -> bool {
        head.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && (head.ends_with("Node") || head.ends_with("Link") || head.ends_with("Value"))
    }

    fn push_symbol(
        &self,
        result: &mut ExtractionResult,
        name: String,
        kind: &str,
        node: Node,
        is_public: bool,
        properties: HashMap<String, String>,
    ) {
        result.symbols.push(Symbol {
            name,
            kind: kind.to_string(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
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

    fn extract_define(
        &self,
        head_text: &str,
        list: Node,
        items: &[Node],
        source: &[u8],
        result: &mut ExtractionResult,
    ) {
        let Some(target) = items.get(1) else {
            return;
        };
        let is_public = head_text == "define-public";

        let (name, kind) = match target.kind() {
            // (define (f a b) …) — curried/procedure definition.
            "list" => {
                let inner = self.named(*target);
                let Some(name_node) = inner.first() else {
                    return;
                };
                // `(define-syntax-rule (name …) …)` / `(define-method (name …) …)`
                // carry the name in a list just like a procedure head.
                let kind = if head_text == "define-syntax-rule" {
                    "macro"
                } else {
                    "function"
                };
                (self.text(*name_node, source), kind)
            }
            // (define x …) — value or lambda bound to a name.
            "symbol" => {
                let value_is_lambda = items.get(2).is_some_and(|v| {
                    v.kind() == "list"
                        && self
                            .named(*v)
                            .first()
                            .map(|h| self.text(*h, source))
                            .is_some_and(|h| h == "lambda" || h == "case-lambda")
                });
                let kind = match head_text {
                    "define-syntax" | "define-syntax-rule" => "macro",
                    "define-record-type" => "record",
                    _ if value_is_lambda => "function",
                    _ => "variable",
                };
                (self.text(*target, source), kind)
            }
            _ => return,
        };

        if name.is_empty() {
            return;
        }
        let mut props = HashMap::new();
        props.insert("lang".to_string(), "scheme".to_string());
        props.insert("define_form".to_string(), head_text.to_string());
        // Guile convention: a `%`-prefixed name is module-internal.
        let public = is_public || !name.starts_with('%');
        self.push_symbol(result, name, kind, list, public, props);
    }

    fn extract_atomese(
        &self,
        atom_type: &str,
        list: Node,
        items: &[Node],
        source: &[u8],
        result: &mut ExtractionResult,
    ) {
        // A named atom carries a string label as its first argument:
        // (ConceptNode "cat"). Anonymous links/values do not.
        let label = items
            .get(1)
            .filter(|n| n.kind() == "string")
            .map(|n| self.string_value(*n, source));

        if let Some(name) = label.clone() {
            if !name.is_empty() {
                let mut props = HashMap::new();
                props.insert("lang".to_string(), "scheme".to_string());
                props.insert("atomese".to_string(), "atom".to_string());
                props.insert("atom_type".to_string(), atom_type.to_string());
                self.push_symbol(result, name, atom_type, list, true, props);
            }
        }

        // `*Link` forms wire the atoms they contain: emit hypergraph edges
        // between the named atoms found as direct children.
        if atom_type.ends_with("Link") {
            let endpoints: Vec<String> = items
                .iter()
                .skip(1)
                .filter(|n| n.kind() == "list")
                .filter_map(|n| self.named_atom_label(*n, source))
                .collect();
            // Link semantics: first atom relates to each subsequent one.
            if let Some((from, rest)) = endpoints.split_first() {
                for to in rest {
                    result.relations.push(Relation {
                        from: from.clone(),
                        to: to.clone(),
                        rel_type: atom_type.to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }
    }

    /// If `list` is a named atom `(XxxNode "label")`, return its label.
    fn named_atom_label(&self, list: Node, source: &[u8]) -> Option<String> {
        let items = self.named(list);
        let head = items.first()?;
        if head.kind() != "symbol" {
            return None;
        }
        if !Self::is_atomese_type(&self.text(*head, source)) {
            return None;
        }
        let label = items.get(1).filter(|n| n.kind() == "string")?;
        let value = self.string_value(*label, source);
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }
}

impl Parser for SchemeParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        if let Some(tree) = parse_with_wasm_safe("scheme", self.wasm_bytes, content) {
            self.walk(tree.root_node(), content.as_bytes(), &mut result);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ExtractionResult {
        SchemeParser::new().parse(src)
    }

    #[test]
    fn extracts_procedure_definition() {
        let r = parse("(define (square x) (* x x))");
        let sym = r
            .symbols
            .iter()
            .find(|s| s.name == "square")
            .expect("square");
        assert_eq!(sym.kind, "function");
        assert!(sym.is_public);
    }

    #[test]
    fn extracts_value_and_lambda_and_macro() {
        let r = parse(
            "(define pi 3.14159)\n\
             (define add (lambda (a b) (+ a b)))\n\
             (define-syntax-rule (swap! a b) (let ((t a)) (set! a b) (set! b t)))",
        );
        assert_eq!(
            r.symbols.iter().find(|s| s.name == "pi").unwrap().kind,
            "variable"
        );
        assert_eq!(
            r.symbols.iter().find(|s| s.name == "add").unwrap().kind,
            "function"
        );
        assert_eq!(
            r.symbols.iter().find(|s| s.name == "swap!").unwrap().kind,
            "macro"
        );
    }

    #[test]
    fn module_internal_name_is_private() {
        let r = parse("(define %internal 42)");
        assert!(
            !r.symbols
                .iter()
                .find(|s| s.name == "%internal")
                .unwrap()
                .is_public
        );
    }

    #[test]
    fn extracts_atomese_nodes_as_symbols() {
        let r = parse("(ConceptNode \"cat\")\n(PredicateNode \"eats\")");
        let cat = r
            .symbols
            .iter()
            .find(|s| s.name == "cat")
            .expect("cat atom");
        assert_eq!(cat.kind, "ConceptNode");
        assert_eq!(
            cat.properties.get("atomese").map(String::as_str),
            Some("atom")
        );
        assert!(r
            .symbols
            .iter()
            .any(|s| s.name == "eats" && s.kind == "PredicateNode"));
    }

    #[test]
    fn atomese_link_emits_hypergraph_edges() {
        let r = parse("(InheritanceLink (ConceptNode \"cat\") (ConceptNode \"animal\"))");
        let edge = r
            .relations
            .iter()
            .find(|e| e.rel_type == "InheritanceLink")
            .expect("inheritance edge");
        assert_eq!(edge.from, "cat");
        assert_eq!(edge.to, "animal");
    }

    #[test]
    fn ordinary_call_is_not_mistaken_for_atom() {
        // lowercase head must never be treated as an atom type
        let r = parse("(add-link foo bar)");
        assert!(r.symbols.is_empty());
    }
}
