use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Query, QueryCursor};

pub struct TypeScriptParser {
    wasm_bytes: &'static [u8],
}

impl Default for TypeScriptParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeScriptParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-tsx.wasm"),
        }
    }

    fn collect_exports<'a>(&self, node: Node<'a>, source: &[u8], exports: &mut HashSet<String>) {
        let kind = node.kind();
        if kind == "export_statement" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                let c_kind = child.kind();
                if [
                    "function_declaration",
                    "class_declaration",
                    "interface_declaration",
                    "type_alias_declaration",
                ]
                .contains(&c_kind)
                {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        if let Ok(name) = name_node.utf8_text(source) {
                            exports.insert(name.to_string());
                        }
                    }
                } else if ["lexical_declaration", "variable_declaration"].contains(&c_kind) {
                    let mut c2 = child.walk();
                    for sub in child.children(&mut c2) {
                        if sub.kind() == "variable_declarator" {
                            if let Some(name_node) = sub.child_by_field_name("name") {
                                if let Ok(name) = name_node.utf8_text(source) {
                                    exports.insert(name.to_string());
                                }
                            }
                        }
                    }
                } else if c_kind == "export_clause" {
                    let mut c2 = child.walk();
                    for spec in child.children(&mut c2) {
                        if spec.kind() == "export_specifier" {
                            if let Some(name_node) = spec.child_by_field_name("name") {
                                if let Ok(name) = name_node.utf8_text(source) {
                                    exports.insert(name.to_string());
                                }
                            }
                        }
                    }
                }
            }
        } else if kind == "expression_statement" {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "assignment_expression" {
                    if let (Some(left), Some(right)) = (
                        child.child_by_field_name("left"),
                        child.child_by_field_name("right"),
                    ) {
                        if let Ok(left_text) = left.utf8_text(source) {
                            if left_text == "module.exports" || left_text == "exports" {
                                if right.kind() == "identifier" {
                                    if let Ok(name) = right.utf8_text(source) {
                                        exports.insert(name.to_string());
                                    }
                                } else if right.kind() == "object" {
                                    let mut r_cursor = right.walk();
                                    for prop in right.children(&mut r_cursor) {
                                        if prop.kind() == "shorthand_property_identifier" {
                                            if let Ok(name) = prop.utf8_text(source) {
                                                exports.insert(name.to_string());
                                            }
                                        } else if prop.kind() == "pair" {
                                            if let Some(key_node) = prop.child_by_field_name("key")
                                            {
                                                if let Ok(name) = key_node.utf8_text(source) {
                                                    exports.insert(name.to_string());
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

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_exports(child, source, exports);
        }
    }

    fn find_class_name(&self, mut node: Node, source: &[u8]) -> Option<String> {
        while let Some(parent) = node.parent() {
            if parent.kind() == "class_declaration" {
                if let Some(name_node) = parent.child_by_field_name("name") {
                    return Some(name_node.utf8_text(source).unwrap_or("").to_string());
                }
            }
            node = parent;
        }
        None
    }

    /// REQ-AXO-91506 — ascend the AST from a `call_expression` / `new_expression`
    /// up to the enclosing function / method / arrow definition and return its
    /// declared name (qualified with the class for methods). Returns empty
    /// string for top-level calls (module-scope IIFE, init expressions).
    fn find_enclosing_function(&self, mut node: Node, source: &[u8]) -> String {
        let mut class_prefix: Option<String> = None;
        while let Some(parent) = node.parent() {
            match parent.kind() {
                "function_declaration" | "method_definition" | "function_expression" => {
                    if let Some(name_node) = parent.child_by_field_name("name") {
                        let name = name_node.utf8_text(source).unwrap_or("").to_string();
                        if let Some(cls) = class_prefix {
                            return format!("{}::{}", cls, name);
                        }
                        return name;
                    }
                    return String::new();
                }
                "arrow_function" => {
                    // arrow_function is anonymous ; climb one more level looking
                    // for a `variable_declarator` whose name is its binding.
                    if let Some(decl) = parent.parent() {
                        if decl.kind() == "variable_declarator" {
                            if let Some(name_node) = decl.child_by_field_name("name") {
                                return name_node.utf8_text(source).unwrap_or("").to_string();
                            }
                        }
                    }
                    return String::new();
                }
                "class_declaration" => {
                    if let Some(name_node) = parent.child_by_field_name("name") {
                        class_prefix =
                            Some(name_node.utf8_text(source).unwrap_or("").to_string());
                    }
                }
                _ => {}
            }
            node = parent;
        }
        String::new()
    }

    fn declaration_node_for_capture<'a>(&self, node: Node<'a>, kind: &str) -> Node<'a> {
        match kind {
            "function.name" | "method.name" => node.parent().unwrap_or(node),
            "arrow.name" => node.parent().unwrap_or(node),
            "class.name" | "interface.name" | "type_alias.name" => node.parent().unwrap_or(node),
            _ => node,
        }
    }

    fn body_node_for_declaration<'a>(&self, declaration: Node<'a>, kind: &str) -> Option<Node<'a>> {
        match kind {
            "function.name" | "method.name" => declaration.child_by_field_name("body"),
            "arrow.name" => declaration
                .child_by_field_name("value")
                .and_then(|value| value.child_by_field_name("body")),
            _ => None,
        }
    }

    fn structural_properties_for_declaration<'a>(
        &self,
        declaration: Node<'a>,
        kind: &str,
    ) -> HashMap<String, String> {
        let mut properties = HashMap::new();
        if let Some(body) = self.body_node_for_declaration(declaration, kind) {
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
            let mut cursor = body.walk();
            let split_lines = body
                .named_children(&mut cursor)
                .map(|child| child.start_position().row + 1)
                .collect::<Vec<_>>();
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
        properties
    }
}

impl Parser for TypeScriptParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let tree = match parse_with_wasm_safe("tsx", self.wasm_bytes, content) {
            Some(t) => t,
            None => {
                return ExtractionResult {
                    project_code: None,
                    symbols: Vec::new(),
                    relations: Vec::new(),
                }
            }
        };
        let language = tree.language();

        let source = content.as_bytes();
        let mut exports = HashSet::new();
        self.collect_exports(tree.root_node(), source, &mut exports);

        let query_str = r#"
            (class_declaration name: (type_identifier) @class.name)
            (interface_declaration name: (type_identifier) @interface.name)
            (type_alias_declaration name: (type_identifier) @type_alias.name)

            (function_declaration name: (identifier) @function.name)
            (method_definition name: (property_identifier) @method.name)
            
            (variable_declarator 
              name: (identifier) @arrow.name
              value: (arrow_function))

            (call_expression
              function: [
                (identifier) @call.name
                (member_expression property: (property_identifier) @call.name)
              ]
            )

            (new_expression
              constructor: [
                (identifier) @new.name
                (member_expression property: (property_identifier) @new.name)
              ]
            )

            (assignment_expression
              left: (member_expression property: (property_identifier) @sink.name)
              (#match? @sink.name "^(innerHTML|outerHTML)$")
            )
            
            (import_statement
              source: (string (string_fragment) @import.source)
            )

            (call_expression
              function: (identifier) @req.name
              arguments: (arguments (string (string_fragment) @require.source))
              (#eq? @req.name "require")
            )
        "#;

        let query = match Query::new(&language, query_str) {
            Ok(q) => q,
            Err(e) => {
                log::warn!("Failed to create TSX query: {}", e);
                return ExtractionResult {
                    project_code: None,
                    symbols: Vec::new(),
                    relations: Vec::new(),
                };
            }
        };
        let mut cursor = QueryCursor::new();
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut seen_nodes = HashSet::new();

        for m in cursor.matches(&query, tree.root_node(), source) {
            for capture in m.captures {
                let node = capture.node;
                let kind = query.capture_names()[capture.index as usize];

                if !seen_nodes.insert((node.id(), kind)) {
                    continue;
                }

                let text = node.utf8_text(source).unwrap_or("").to_string();
                let declaration = self.declaration_node_for_capture(node, kind);
                let declaration_start_line = declaration.start_position().row + 1;
                let declaration_end_line = declaration.end_position().row + 1;

                match kind {
                    "class.name" => {
                        symbols.push(Symbol {
                            name: text.clone(),
                            kind: "class".to_string(),
                            start_line: declaration_start_line,
                            end_line: declaration_end_line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: exports.contains(&text),
                            tested: text.contains("Test") || text.contains("Spec"),
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });

                        if let Some(parent) = node.parent() {
                            let mut p_cursor = parent.walk();
                            for child in parent.children(&mut p_cursor) {
                                if child.kind() == "class_heritage" {
                                    let mut h_cursor = child.walk();
                                    for sub in child.children(&mut h_cursor) {
                                        let rel_type = if sub.kind() == "extends_clause" {
                                            "extends"
                                        } else {
                                            "implements"
                                        };
                                        if sub.kind() == "extends_clause"
                                            || sub.kind() == "implements_clause"
                                        {
                                            let mut s_cursor = sub.walk();
                                            for type_node in sub.children(&mut s_cursor) {
                                                if type_node.kind() == "identifier"
                                                    || type_node.kind() == "type_identifier"
                                                {
                                                    relations.push(Relation {
                                                        from: text.clone(),
                                                        to: type_node
                                                            .utf8_text(source)
                                                            .unwrap_or("")
                                                            .to_string(),
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
                    "interface.name" => {
                        symbols.push(Symbol {
                            name: text.clone(),
                            kind: "interface".to_string(),
                            start_line: declaration_start_line,
                            end_line: declaration_end_line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: exports.contains(&text),
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });

                        if let Some(parent) = node.parent() {
                            let mut p_cursor = parent.walk();
                            for child in parent.children(&mut p_cursor) {
                                if child.kind() == "extends_type_clause" {
                                    let mut c_cursor = child.walk();
                                    for sub in child.children(&mut c_cursor) {
                                        if sub.kind() == "identifier"
                                            || sub.kind() == "type_identifier"
                                        {
                                            relations.push(Relation {
                                                from: text.clone(),
                                                to: sub.utf8_text(source).unwrap_or("").to_string(),
                                                rel_type: "extends".to_string(),
                                                properties: HashMap::new(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "type_alias.name" => {
                        symbols.push(Symbol {
                            name: text.clone(),
                            kind: "type_alias".to_string(),
                            start_line: declaration_start_line,
                            end_line: declaration_end_line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: exports.contains(&text),
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });
                    }
                    "function.name" | "arrow.name" => {
                        let lower_name = text.to_lowercase();
                        let is_entry = exports.contains(&text)
                            && ["handler", "route", "get", "post", "put", "delete"]
                                .iter()
                                .any(|&k| lower_name.contains(k));

                        let mut is_unsafe = false;
                        let properties =
                            self.structural_properties_for_declaration(declaration, kind);
                        let body_text_node = self
                            .body_node_for_declaration(declaration, kind)
                            .unwrap_or(declaration);
                        let body = body_text_node.utf8_text(source).unwrap_or("");
                        if body.contains("eval(") || body.contains("innerHTML") {
                            is_unsafe = true;
                        }

                        symbols.push(Symbol {
                            name: text.clone(),
                            kind: "function".to_string(),
                            start_line: declaration_start_line,
                            end_line: declaration_end_line,
                            docstring: None,
                            is_entry_point: is_entry,
                            is_public: exports.contains(&text),
                            tested: lower_name.contains("test") || lower_name.contains("spec"),
                            is_nif: false,
                            is_unsafe,
                            properties,
                            embedding: None,
                        });
                    }
                    "method.name" => {
                        let mut props =
                            self.structural_properties_for_declaration(declaration, kind);
                        if let Some(class_name) = self.find_class_name(node, source) {
                            props.insert("class_name".to_string(), class_name);
                        }

                        symbols.push(Symbol {
                            name: text.clone(),
                            kind: "method".to_string(),
                            start_line: declaration_start_line,
                            end_line: declaration_end_line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true, // TS methods are public by default unless private keyword
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: props,
                            embedding: None,
                        });
                    }
                    "call.name" | "new.name" | "sink.name" => {
                        // REQ-AXO-91506 — climb to the enclosing fn/method/arrow.
                        let from = self.find_enclosing_function(node, source);
                        relations.push(Relation {
                            from,
                            to: text,
                            rel_type: "calls".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                    "import.source" | "require.source" => {
                        // Imports are module-scope ; from stays "" by design.
                        relations.push(Relation {
                            from: String::new(),
                            to: text,
                            rel_type: "imports".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                    _ => {}
                }
            }
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TypeScriptParser;
    use crate::parser::Parser;

    #[test]
    fn parser_emits_structural_body_bounds_for_typescript_symbols() {
        let parser = TypeScriptParser::new();
        let result = parser.parse(
            r#"
export function routeHandler(
  request: Request,
) {
  const body = request.json();
  return body;
}

export const computeAnswer = (
  input: number,
) => {
  return input + 1;
};

class Batcher {
  shape(items: string[]) {
    return items.map((item) => item.trim());
  }
}
"#,
        );

        let function_symbol = result
            .symbols
            .iter()
            .find(|symbol| symbol.name == "routeHandler")
            .expect("routeHandler symbol");
        assert_eq!(function_symbol.start_line, 2);
        assert!(function_symbol.end_line >= 7);
        assert_eq!(
            function_symbol.properties.get("body_start_line"),
            Some(&"4".to_string())
        );

        let arrow_symbol = result
            .symbols
            .iter()
            .find(|symbol| symbol.name == "computeAnswer")
            .expect("computeAnswer symbol");
        assert!(arrow_symbol.end_line >= 12);
        assert_eq!(
            arrow_symbol.properties.get("body_start_line"),
            Some(&"11".to_string())
        );

        let method_symbol = result
            .symbols
            .iter()
            .find(|symbol| symbol.name == "shape")
            .expect("shape method symbol");
        assert_eq!(
            method_symbol.properties.get("class_name"),
            Some(&"Batcher".to_string())
        );
        assert_eq!(
            method_symbol.properties.get("body_start_line"),
            Some(&"16".to_string())
        );
    }
}
