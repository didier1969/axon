use super::{ExtractionResult, Parser, Relation, Symbol};
use std::collections::HashMap;
use tree_sitter::{Language, Node, Parser as TSParser};

pub struct HtmlParser {
    language: Language,
}

impl Default for HtmlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl HtmlParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_html::LANGUAGE.into(),
        }
    }

    fn walk<'a>(&self, node: Node<'a>, source: &[u8], symbols: &mut Vec<Symbol>, relations: &mut Vec<Relation>) {
        let kind = node.kind();
        
        if kind == "element" || kind == "script_element" || kind == "style_element" {
            self.process_element(node, source, symbols, relations);
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, source, symbols, relations);
        }
    }

    fn process_element<'a>(&self, node: Node<'a>, source: &[u8], symbols: &mut Vec<Symbol>, relations: &mut Vec<Relation>) {
        let mut start_tag = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "start_tag" || child.kind() == "self_closing_tag" {
                start_tag = Some(child);
                break;
            }
        }

        let start_tag = match start_tag {
            Some(t) => t,
            None => return,
        };

        let tag_name = self.get_tag_name(start_tag, source);
        let attrs = self.get_attributes(start_tag, source);

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        if let Some(id) = attrs.get("id") {
            let mut props = HashMap::new();
            props.insert("tag".to_string(), tag_name.clone());
            if let Some(cls) = attrs.get("class") {
                props.insert("classes".to_string(), cls.clone());
            }
            symbols.push(Symbol {
                name: format!("#{}", id),
                kind: "element".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: false,
                properties: props,
            });
        } else if let Some(cls) = attrs.get("class") {
            let mut props = HashMap::new();
            props.insert("tag".to_string(), tag_name.clone());
            props.insert("classes".to_string(), cls.clone());
            
            let first_class = cls.split_whitespace().next().unwrap_or("").to_string();
            if !first_class.is_empty() {
                symbols.push(Symbol {
                    name: format!(".{}", first_class),
                    kind: "element".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    properties: props,
                });
            }
        }

        if ["input", "textarea", "select", "form"].contains(&tag_name.as_str()) {
            let name = attrs.get("name").or(attrs.get("id")).cloned().unwrap_or_else(|| tag_name.clone());
            let mut props = HashMap::new();
            props.insert("tag".to_string(), tag_name.clone());
            props.insert("type".to_string(), attrs.get("type").cloned().unwrap_or_else(|| "text".to_string()));
            
            symbols.push(Symbol {
                name,
                kind: "field".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: true,
                properties: props,
            });
        }

        if tag_name == "script" {
            if let Some(src) = attrs.get("src") {
                relations.push(Relation {
                    from: "".to_string(),
                    to: src.clone(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        if tag_name == "link" {
            if let Some(href) = attrs.get("href") {
                relations.push(Relation {
                    from: "".to_string(),
                    to: href.clone(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        for (attr_name, attr_value) in &attrs {
            if attr_name.starts_with("on") {
                let func_name = attr_value.split('(').next().unwrap_or("").trim();
                if !func_name.is_empty() {
                    relations.push(Relation {
                        from: "".to_string(),
                        to: func_name.to_string(),
                        rel_type: "calls".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        if tag_name == "a" {
            if let Some(href) = attrs.get("href") {
                relations.push(Relation {
                    from: "".to_string(),
                    to: href.clone(),
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
    }

    fn get_tag_name(&self, start_tag: Node, source: &[u8]) -> String {
        let mut cursor = start_tag.walk();
        for child in start_tag.children(&mut cursor) {
            if child.kind() == "tag_name" {
                return child.utf8_text(source).unwrap_or("").to_lowercase();
            }
        }
        "".to_string()
    }

    fn get_attributes(&self, start_tag: Node, source: &[u8]) -> HashMap<String, String> {
        let mut attrs = HashMap::new();
        let mut cursor = start_tag.walk();
        for child in start_tag.children(&mut cursor) {
            if child.kind() == "attribute" {
                let mut attr_name = String::new();
                let mut attr_value = String::new();
                let mut ac_cursor = child.walk();
                for ac in child.children(&mut ac_cursor) {
                    if ac.kind() == "attribute_name" {
                        attr_name = ac.utf8_text(source).unwrap_or("").to_lowercase();
                    } else if ac.kind() == "quoted_attribute_value" {
                        let raw = ac.utf8_text(source).unwrap_or("");
                        attr_value = raw.trim_matches(|c| c == '"' || c == '\'').to_string();
                    } else if ac.kind() == "attribute_value" {
                        attr_value = ac.utf8_text(source).unwrap_or("").to_string();
                    }
                }
                if !attr_name.is_empty() {
                    attrs.insert(attr_name, attr_value);
                }
            }
        }
        attrs
    }
}

impl Parser for HtmlParser {
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
    fn test_parse_html() {
        let code = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <link href="style.css" rel="stylesheet" />
                <script src="app.js"></script>
            </head>
            <body>
                <div id="main-container" class="container dark">
                    <form id="login-form">
                        <input type="text" name="username" />
                        <input type="password" name="password" />
                        <button onclick="submitForm(event)">Login</button>
                    </form>
                </div>
                <a href="/about">About</a>
            </body>
            </html>
        "#;
        let parser = HtmlParser::new();
        let result = parser.parse(code);

        assert!(result.symbols.iter().any(|s| s.name == "#main-container" && s.kind == "element"));
        assert!(result.symbols.iter().any(|s| s.name == "username" && s.kind == "field" && s.is_entry_point));
        assert!(result.symbols.iter().any(|s| s.name == "password" && s.kind == "field" && s.is_entry_point));
        assert!(result.symbols.iter().any(|s| s.name == "login-form" && s.kind == "field" && s.is_entry_point));

        assert!(result.relations.iter().any(|r| r.to == "style.css" && r.rel_type == "imports"));
        assert!(result.relations.iter().any(|r| r.to == "app.js" && r.rel_type == "imports"));
        assert!(result.relations.iter().any(|r| r.to == "submitForm" && r.rel_type == "calls"));
        assert!(result.relations.iter().any(|r| r.to == "/about" && r.rel_type == "calls"));
    }
}