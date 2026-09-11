use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use tree_sitter::Node;

static JPA_TABLE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"@Table\s*\(\s*(?:name\s*=\s*)?"([^"]+)""#).unwrap());

pub struct JavaParser {
    wasm_bytes: &'static [u8],
}

impl JavaParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-java.wasm"),
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
                    self.extract_class(child, content, symbols, relations);
                }
                "field_declaration" => {
                    self.extract_field(child, content, symbols, relations, class_name);
                }
                "method_declaration" => {
                    self.extract_method(child, content, symbols, relations, class_name);
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

    fn extract_class(
        &self,
        node: Node,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
    ) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let mut is_public = false;
                let mut is_entity = false;
                let mut table_name = None;

                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        if let Ok(mod_text) = child.utf8_text(content) {
                            if mod_text.contains("public") {
                                is_public = true;
                            }
                            if mod_text.contains("@Entity") {
                                is_entity = true;
                            }
                            if let Some(cap) = JPA_TABLE_RE.captures(mod_text) {
                                if let Some(m) = cap.get(1) {
                                    table_name = Some(m.as_str().to_string());
                                }
                            }
                        }
                    }
                }

                let mut properties = std::collections::HashMap::new();
                if is_entity {
                    properties.insert("is_entity".to_string(), "true".to_string());
                }
                if let Some(ref tbl) = table_name {
                    properties.insert("table".to_string(), tbl.clone());
                    let mut rel_props = std::collections::HashMap::new();
                    rel_props.insert("orm".to_string(), "jpa".to_string());
                    rel_props.insert("table".to_string(), tbl.clone());
                    relations.push(Relation {
                        from: name.to_string(),
                        to: tbl.clone(),
                        rel_type: "references".to_string(),
                        properties: rel_props,
                    });
                }

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "class".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: false,
                    is_public,
                    tested: name.contains("Test"),
                    is_nif: false,
                    is_unsafe: false,
                    properties,
                    embedding: None,
                });
            }
        }
    }

    fn extract_field(
        &self,
        node: Node,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        class_name: &str,
    ) {
        if class_name.is_empty() {
            return;
        }

        let mut mod_text = String::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                if let Ok(txt) = child.utf8_text(content) {
                    mod_text = txt.to_string();
                }
            }
        }

        let jpa_rel = if mod_text.contains("@ManyToOne") {
            Some("ManyToOne")
        } else if mod_text.contains("@OneToMany") {
            Some("OneToMany")
        } else if mod_text.contains("@OneToOne") {
            Some("OneToOne")
        } else if mod_text.contains("@ManyToMany") {
            Some("ManyToMany")
        } else {
            None
        };

        // Get variable declarator
        let mut field_name = String::new();
        let mut decl_cursor = node.walk();
        for child in node.children(&mut decl_cursor) {
            if child.kind() == "variable_declarator" {
                if let Some(name_node) = child.child_by_field_name("name") {
                    if let Ok(name) = name_node.utf8_text(content) {
                        field_name = name.to_string();
                    }
                }
            }
        }

        if field_name.is_empty() {
            return;
        }

        // Get type
        let type_node = node.child_by_field_name("type");
        let type_text = type_node
            .and_then(|n| n.utf8_text(content).ok())
            .unwrap_or("")
            .to_string();

        let mut target_entity = String::new();
        if let Some(tn) = type_node {
            if tn.kind() == "generic_type" {
                let mut tc = tn.walk();
                for child in tn.children(&mut tc) {
                    if child.kind() == "type_arguments" {
                        let mut arg_cursor = child.walk();
                        for arg in child.children(&mut arg_cursor) {
                            if arg.kind() == "type_identifier" {
                                if let Ok(arg_txt) = arg.utf8_text(content) {
                                    target_entity = arg_txt.to_string();
                                    break;
                                }
                            }
                        }
                    }
                }
            } else if tn.kind() == "type_identifier" {
                target_entity = type_text.clone();
            }
        }

        if let Some(rel) = jpa_rel {
            let mut props = std::collections::HashMap::new();
            props.insert("orm".to_string(), "jpa".to_string());
            props.insert("relation".to_string(), rel.to_string());
            props.insert("field".to_string(), field_name.clone());

            if !target_entity.is_empty() {
                relations.push(Relation {
                    from: class_name.to_string(),
                    to: target_entity,
                    rel_type: "references".to_string(),
                    properties: props,
                });
            }

            let mut field_props = std::collections::HashMap::new();
            if !type_text.is_empty() {
                field_props.insert("type".to_string(), type_text);
            }
            field_props.insert("jpa_relation".to_string(), rel.to_string());

            let full_field_name = format!("{}.{}", class_name, field_name);
            symbols.push(Symbol {
                name: full_field_name.clone(),
                kind: "field".to_string(),
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                docstring: None,
                is_entry_point: false,
                is_public: mod_text.contains("public"),
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: field_props,
                embedding: None,
            });

            relations.push(Relation {
                from: class_name.to_string(),
                to: full_field_name,
                rel_type: "contains".to_string(),
                properties: std::collections::HashMap::new(),
            });
        }
    }

    /// REQ-AXO-902185 (god-objects) — McCabe cyclomatic complexity: base 1 +
    /// one per decision point. Java has no lambda/anonymous-class extracted
    /// as a separate Symbol in this parser (only nested `class_declaration`
    /// recurses, tagged separately), so no nested-exclusion guard is needed
    /// here — mirrors the Go/C/PHP precedent. `switch_label` matches both
    /// `case` and `default` labels (mirrors Go counting `default_case` too).
    /// Boolean short-circuit operators (`&&`/`||`) are NOT counted, same
    /// first-pass scope as rust.rs.
    const BRANCHING_KINDS: &[&str] = &[
        "if_statement",
        "for_statement",
        "enhanced_for_statement",
        "while_statement",
        "do_statement",
        "switch_label",
        "catch_clause",
        "ternary_expression",
    ];

    fn count_branches(&self, node: Node) -> i32 {
        let mut count = 0i32;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if Self::BRANCHING_KINDS.contains(&child.kind()) {
                count += 1;
            }
            count += self.count_branches(child);
        }
        count
    }

    fn extract_method(
        &self,
        node: Node,
        content: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        class_name: &str,
    ) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content) {
                let mut is_entry = false;
                let mut is_public = false;
                let mut is_nif = false;
                let mut tested = false;
                let mut decorators = Vec::new();

                let mut modifiers_node = None;
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "modifiers" {
                        modifiers_node = Some(child);
                        if let Ok(mod_text) = child.utf8_text(content) {
                            if mod_text.contains("public") {
                                is_public = true;
                            }
                            if mod_text.contains("native") {
                                is_nif = true;
                            }
                        }
                    }
                }

                if let Some(modifiers) = modifiers_node {
                    let mut cursor = modifiers.walk();
                    for mod_node in modifiers.children(&mut cursor) {
                        if mod_node.kind() == "marker_annotation" || mod_node.kind() == "annotation"
                        {
                            if let Some(ann_name) = mod_node.child_by_field_name("name") {
                                if let Ok(ann_text) = ann_name.utf8_text(content) {
                                    decorators.push(ann_text.to_string());
                                    let ann_text_str = ann_text;
                                    if ann_text_str.contains("Test") {
                                        tested = true;
                                    }
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
                if let Some(body) = node.child_by_field_name("body") {
                    let complexity = 1 + self.count_branches(body);
                    properties.insert("cyclomatic_complexity".to_string(), complexity.to_string());
                }

                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: "method".to_string(),
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    docstring: None,
                    is_entry_point: is_entry || is_nif,
                    is_public,
                    tested,
                    is_nif,
                    is_unsafe: false,
                    properties,
                    embedding: None,
                });

                // REQ-AXO-902423 — emit Symbol -> Symbol CONTAINS relation (class -> method).
                if !class_name.is_empty() {
                    relations.push(Relation {
                        from: class_name.to_string(),
                        to: name.to_string(),
                        rel_type: "contains".to_string(),
                        properties: std::collections::HashMap::new(),
                    });
                }

                // REQ-AXO-902662 — emit calls_nif edge for Java native method stubs
                if is_nif {
                    let from_sym = if !class_name.is_empty() {
                        format!("{}.{}", class_name, name)
                    } else {
                        name.to_string()
                    };
                    relations.push(Relation {
                        from: from_sym,
                        to: name.to_string(),
                        rel_type: "calls_nif".to_string(),
                        properties: std::collections::HashMap::new(),
                    });
                }
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
                properties.insert(
                    "line".to_string(),
                    (node.start_position().row + 1).to_string(),
                );

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
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        if let Some(tree) = parse_with_wasm_safe("java", self.wasm_bytes, content) {
            self.walk(
                tree.root_node(),
                content.as_bytes(),
                &mut symbols,
                &mut relations,
                "",
            );
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
    //! REQ-AXO-902185 (god-objects) — cyclomatic complexity regression tests.
    use super::*;

    fn parser() -> JavaParser {
        JavaParser::new()
    }

    #[test]
    fn simple_function_has_complexity_one() {
        let result = parser().parse("class C { void f() { int x = 1; } }");
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn branching_function_counts_each_decision_point() {
        let result = parser().parse(
            "class C { \
                int f(int x) { \
                    if (x > 0) { return 1; } \
                    for (int i = 0; i < x; i++) {} \
                    switch (x) { case 1: break; default: break; } \
                    return x > 0 ? 1 : 0; \
                } \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let f = result.symbols.iter().find(|s| s.name == "f").unwrap();
        // base 1 + if + for + case + default + ternary = 6
        assert_eq!(
            f.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("6")
        );
    }

    #[test]
    fn method_gets_its_own_complexity() {
        let result = parser().parse(
            "class C { \
                void a() { if (true) {} } \
                void b() {} \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let a = result.symbols.iter().find(|s| s.name == "a").unwrap();
        let b = result.symbols.iter().find(|s| s.name == "b").unwrap();
        assert_eq!(
            a.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("2")
        );
        assert_eq!(
            b.properties
                .get("cyclomatic_complexity")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn test_req_902423_java_contains_relations() {
        let result = parser().parse(
            "class Calculator { \
                public int add(int a, int b) { return a + b; } \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let contains_calc_add = result
            .relations
            .iter()
            .any(|r| r.rel_type == "contains" && r.from == "Calculator" && r.to == "add");
        assert!(
            contains_calc_add,
            "Calculator must contain add: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902660_java_test_annotation_marks_method_tested() {
        let result = parser().parse(
            "class CalculatorTest { \
                @Test \
                public void testAdd() { assert true; } \
                @ParameterizedTest \
                public void parameterizedTest() {} \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let cls = result
            .symbols
            .iter()
            .find(|s| s.name == "CalculatorTest")
            .unwrap();
        assert!(cls.tested, "CalculatorTest class must be tested=true");
        let m1 = result.symbols.iter().find(|s| s.name == "testAdd").unwrap();
        assert!(m1.tested, "testAdd with @Test must be tested=true");
        let m2 = result
            .symbols
            .iter()
            .find(|s| s.name == "parameterizedTest")
            .unwrap();
        assert!(
            m2.tested,
            "parameterizedTest with @ParameterizedTest must be tested=true"
        );
    }

    #[test]
    fn req_902662_java_native_method_emits_calls_nif() {
        let result = parser().parse(
            "class NativeBridge { \
                public native int compute(int x); \
            }",
        );
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }
        let m = result.symbols.iter().find(|s| s.name == "compute").unwrap();
        assert!(m.is_nif, "compute must have is_nif=true");
        assert!(
            m.is_entry_point,
            "native method must have is_entry_point=true"
        );

        let calls_nif = result.relations.iter().any(|r| {
            r.rel_type == "calls_nif" && r.from == "NativeBridge.compute" && r.to == "compute"
        });
        assert!(
            calls_nif,
            "NativeBridge.compute must emit calls_nif relation to compute: {:?}",
            result.relations
        );
    }

    #[test]
    fn req_902663_java_jpa_entities_and_relationships() {
        let code = r#"
            @Entity
            @Table(name = "users")
            public class User {
                @Id
                private Long id;

                @ManyToOne
                @JoinColumn(name = "org_id")
                private Organization organization;

                @OneToMany(mappedBy = "user")
                private List<Post> posts;
            }
        "#;
        let result = parser().parse(code);
        if result.symbols.is_empty() {
            eprintln!("java wasm grammar unavailable, skipping");
            return;
        }

        let user_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "User")
            .expect("User class must exist");
        assert_eq!(
            user_sym.properties.get("is_entity").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            user_sym.properties.get("table").map(|s| s.as_str()),
            Some("users")
        );

        // References to table "users"
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "users" && r.rel_type == "references"));

        // Relationships reference Organization and Post
        assert!(result.relations.iter().any(|r| r.from == "User"
            && r.to == "Organization"
            && r.rel_type == "references"
            && r.properties.get("relation") == Some(&"ManyToOne".to_string())));
        assert!(result.relations.iter().any(|r| r.from == "User"
            && r.to == "Post"
            && r.rel_type == "references"
            && r.properties.get("relation") == Some(&"OneToMany".to_string())));

        // Field symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User.organization" && s.kind == "field"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User.posts" && s.kind == "field"));
    }
}
