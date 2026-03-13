use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::{Language, Node, Parser as TSParser};

pub struct CssParser {
    language: Language,
}

impl Default for CssParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CssParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_css::LANGUAGE.into(),
        }
    }

    fn walk<'a>(&self, node: Node<'a>, source: &[u8], symbols: &mut Vec<Symbol>, relations: &mut Vec<Relation>) {
        let kind = node.kind();
        
        match kind {
            "id_selector" => self.extract_id_selector(node, source, symbols),
            "class_selector" => self.extract_class_selector(node, source, symbols),
            "import_statement" => self.extract_import(node, source, relations),
            "declaration" => self.extract_variable(node, source, symbols),
            "at_rule" => self.extract_at_rule(node, source, symbols),
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, source, symbols, relations);
        }
    }

    fn extract_id_selector(&self, node: Node, source: &[u8], symbols: &mut Vec<Symbol>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "id_name" {
                let name = format!("#{}", child.utf8_text(source).unwrap_or(""));
                symbols.push(Symbol {
                    name,
                    kind: "element".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    properties: HashMap::new(),
                
                    embedding: None,
                });
                break;
            }
        }
    }

    fn extract_class_selector(&self, node: Node, source: &[u8], symbols: &mut Vec<Symbol>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "class_name" {
                let name = format!(".{}", child.utf8_text(source).unwrap_or(""));
                symbols.push(Symbol {
                    name,
                    kind: "element".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    properties: HashMap::new(),
                
                    embedding: None,
                });
                break;
            }
        }
    }

    fn extract_variable(&self, node: Node, source: &[u8], symbols: &mut Vec<Symbol>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "property_name" {
                let name = child.utf8_text(source).unwrap_or("");
                if name.starts_with("--") {
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: "variable".to_string(),
                        start_line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        docstring: None,
                        is_entry_point: false,
                        properties: HashMap::new(),
                    
                        embedding: None,
                    });
                }
                break;
            }
        }
    }

    fn extract_at_rule(&self, node: Node, source: &[u8], symbols: &mut Vec<Symbol>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "at_keyword" {
                let name = child.utf8_text(source).unwrap_or("");
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "interface".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    properties: HashMap::new(),
                
                    embedding: None,
                });
                break;
            }
        }
    }

    fn extract_import(&self, node: Node, source: &[u8], relations: &mut Vec<Relation>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "string_value" || child.kind() == "call_expression" {
                let raw = child.utf8_text(source).unwrap_or("");
                let mut url = raw.trim();
                if url.starts_with("url(") {
                    url = &url[4..url.len() - 1];
                }
                url = url.trim_matches(|c| c == '"' || c == '\'');
                if !url.is_empty() {
                    relations.push(Relation {
                        from: "".to_string(),
                        to: url.to_string(),
                        rel_type: "imports".to_string(),
                        properties: HashMap::new(),
                    });
                }
                return;
            }
        }
    }
}

impl Parser for CssParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(&self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        self.walk(tree.root_node(), content.as_bytes(), &mut symbols, &mut relations);

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_css() {
        let code = r#"
            @import url("reset.css");
            @font-face {
                font-family: 'MyFont';
            }
            :root {
                --main-color: #333;
            }
            #app {
                background: var(--main-color);
            }
            .container {
                display: flex;
            }
        "#;
        let parser = CssParser::new();
        let result = parser.parse(code);

        assert!(result.symbols.iter().any(|s| s.name == "#app" && s.kind == "element"));
        assert!(result.symbols.iter().any(|s| s.name == ".container" && s.kind == "element"));
        assert!(result.symbols.iter().any(|s| s.name == "--main-color" && s.kind == "variable"));
        assert!(result.symbols.iter().any(|s| s.name == "@font-face" && s.kind == "interface"));

        assert!(result.relations.iter().any(|r| r.to == "reset.css" && r.rel_type == "imports"));
    }
}