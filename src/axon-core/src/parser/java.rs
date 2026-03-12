use super::{ExtractionResult, Parser, Relation, Symbol};
use tree_sitter::{Language, Node, Parser as TSParser};

pub struct JavaParser {
    language: Language,
}

impl JavaParser {
    pub fn new() -> Self {
        Self {
            language: tree_sitter_java::LANGUAGE.into(),
        }
    }

    fn walk<'a>(
        &self,
        node: Node<'a>,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        class_name: &str,
    ) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "class_declaration" => {
                    self.extract_class(child, content, symbols);
                }
                "method_declaration" => {
                    self.extract_method(child, content, symbols, class_name);
                }
                "import_declaration" => {
                    self.extract_import(child, content, relations);
                }
                "method_invocation" => {
                    self.extract_call(child, content, relations);
                }
                _ => {}
            }

            // Recurse for nested classes
            let mut new_class = class_name.to_string();
            if child.kind() == "class_declaration" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(content) {
                        new_class = name.to_string();
                    }
                }
            }

            self.walk(child, content, symbols, relations, &new_class);
        }
    }

    fn extract_class(&self, node: Node, content: &[u8], symbols: &mut Vec<Symbol>) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "class".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    properties: std::collections::HashMap::new(),
                });
            }
        }
    }

    fn extract_method(
        &self,
        node: Node,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        class_name: &str,
    ) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let mut is_entry = false;
                let mut decorators = Vec::new();

                let mut modifiers_node = None;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        modifiers_node = Some(child);
                        break;
                    }
                }

                if let Some(modifiers) = modifiers_node {
                    let mut cursor = modifiers.walk();
                    for mod_node in modifiers.children(&mut cursor) {
                        if mod_node.kind() == "marker_annotation" || mod_node.kind() == "annotation" {
                            if let Some(ann_name) = mod_node.child_by_field_name("name") {
                                if let Ok(ann_text) = ann_name.utf8_text(content) {
                                    decorators.push(ann_text.to_string());
                                    let ann_text_str = ann_text;
                                    if ann_text_str.contains("Mapping")
                                        || ann_text_str.contains("Route")
                                        || ann_text_str.contains("Endpoint")
                                        || ann_text_str.contains("GET")
                                        || ann_text_str.contains("POST")
                                        || ann_text_str.contains("PUT")
                                        || ann_text_str.contains("DELETE")
                                    {
                                        is_entry = true;
                                    }
                                }
                            }
                        }
                    }
                }

                let mut properties = std::collections::HashMap::new();
                if !class_name.is_empty() {
                    properties.insert("class_name".to_string(), class_name.to_string());
                }
                if !decorators.is_empty() {
                    properties.insert("decorators".to_string(), decorators.join(","));
                }

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "method".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: is_entry,
                    properties,
                });
            }
        }
    }

    fn extract_import(&self, node: Node, content: &[u8], relations: &mut Vec<Relation>) {
        if let Some(path_node) = node.named_child(0) {
            if let Ok(path) = path_node.utf8_text(content) {
                relations.push(Relation {
                    from: "file".to_string(),
                    to: path.to_string(),
                    rel_type: "imports".to_string(),
                    properties: std::collections::HashMap::new(),
                });
            }
        }
    }

    fn extract_call(&self, node: Node, content: &[u8], relations: &mut Vec<Relation>) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let receiver_name = if let Some(object_node) = node.child_by_field_name("object") {
                    object_node.utf8_text(content).unwrap_or("").to_string()
                } else {
                    "".to_string()
                };

                let target = if !receiver_name.is_empty() {
                    format!("{}.{}", receiver_name, name)
                } else {
                    name.to_string()
                };

                let mut properties = std::collections::HashMap::new();
                properties.insert("line".to_string(), (node.start_position().row + 1).to_string());

                relations.push(Relation {
                    from: "method".to_string(),
                    to: target,
                    rel_type: "calls".to_string(),
                    properties,
                });
            }
        }
    }
}

impl Parser for JavaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut parser = TSParser::new();
        parser.set_language(&self.language).unwrap();
        let tree = parser.parse(content, None).unwrap();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        self.walk(
            tree.root_node(),
            content.as_bytes(),
            &mut symbols,
            &mut relations,
            "",
        );

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_java_parser() {
        let code = r#"
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;
import com.example.service.MyService;

@RestController
public class MyController {
    private MyService myService;
    
    @GetMapping("/hello")
    public String sayHello() {
        return myService.greet();
    }
    
    public void normalMethod() {
        System.out.println("Normal");
    }
}
        "#;
        
        let parser = JavaParser::new();
        let result = parser.parse(code);
        
        // Imports
        assert!(result.relations.iter().any(|r| r.rel_type == "imports" && r.to == "org.springframework.web.bind.annotation.GetMapping"));
        assert!(result.relations.iter().any(|r| r.rel_type == "imports" && r.to == "org.springframework.web.bind.annotation.RestController"));
        assert!(result.relations.iter().any(|r| r.rel_type == "imports" && r.to == "com.example.service.MyService"));
        
        // Classes
        let _cls = result.symbols.iter().find(|s| s.name == "MyController" && s.kind == "class").unwrap();
        
        // Methods
        let say_hello = result.symbols.iter().find(|s| s.name == "sayHello" && s.kind == "method").unwrap();
        assert_eq!(say_hello.properties.get("class_name").unwrap(), "MyController");
        assert_eq!(say_hello.properties.get("decorators").unwrap(), "GetMapping");
        assert!(say_hello.is_entry_point);
        let normal_method = result.symbols.iter().find(|s| s.name == "normalMethod" && s.kind == "method").unwrap();

        assert_eq!(say_hello.properties.get("class_name").unwrap(), "MyController");
        assert!(!normal_method.properties.contains_key("decorators"));
        assert!(!normal_method.is_entry_point);
        
        // Calls
        assert!(result.relations.iter().any(|r| r.rel_type == "calls" && r.to == "myService.greet"));
        assert!(result.relations.iter().any(|r| r.rel_type == "calls" && r.to == "System.out.println"));
    }
}
