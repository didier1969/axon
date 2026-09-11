use super::{parse_with_wasm_safe, ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use tree_sitter::Node;

static CALL_RE: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(r#"(?:[a-zA-Z_][a-zA-Z0-9_]*\.)*([a-zA-Z_][a-zA-Z0-9_]*)\s*\("#).unwrap()
});

static RECORD_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r#"<record\s+[^>]*id=["']([^"']+)["'][^>]*>"#).unwrap());

static BUTTON_RE: Lazy<regex::Regex> = Lazy::new(|| {
    regex::Regex::new(r#"<button\s+[^>]*name=["']([^"']+)["'][^>]*type=["']object["'][^>]*>"#)
        .unwrap()
});

pub struct XmlParser {
    wasm_bytes: &'static [u8],
}

impl Default for XmlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl XmlParser {
    pub fn new() -> Self {
        Self {
            wasm_bytes: include_bytes!("../../parsers/tree-sitter-html.wasm"),
        }
    }

    fn walk<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        current_scope: &str,
    ) {
        let kind = node.kind();
        let mut next_scope = current_scope.to_string();

        if kind == "element" {
            if let Some(new_scope) =
                self.process_element(node, source, symbols, relations, current_scope)
            {
                next_scope = new_scope;
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, source, symbols, relations, &next_scope);
        }
    }

    fn process_element<'a>(
        &self,
        node: Node<'a>,
        source: &[u8],
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
        parent_scope: &str,
    ) -> Option<String> {
        let mut start_tag = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "start_tag" || child.kind() == "self_closing_tag" {
                start_tag = Some(child);
                break;
            }
        }

        let start_tag = start_tag?;
        let tag_name = self.get_tag_name(start_tag, source);
        let attrs = self.get_attributes(start_tag, source);

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;

        match tag_name.as_str() {
            "record" => {
                if let Some(id) = attrs.get("id") {
                    let mut props = HashMap::new();
                    props.insert("tag".to_string(), "record".to_string());
                    if let Some(model) = attrs.get("model") {
                        props.insert("model".to_string(), model.clone());
                    }
                    symbols.push(Symbol {
                        name: id.clone(),
                        kind: "record".to_string(),
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
                    return Some(id.clone());
                }
            }
            "template" => {
                if let Some(id) = attrs.get("id") {
                    let mut props = HashMap::new();
                    props.insert("tag".to_string(), "template".to_string());
                    if let Some(name) = attrs.get("name") {
                        props.insert("name".to_string(), name.clone());
                    }
                    symbols.push(Symbol {
                        name: id.clone(),
                        kind: "template".to_string(),
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
                    return Some(id.clone());
                }
            }
            "menuitem" => {
                if let Some(id) = attrs.get("id") {
                    let mut props = HashMap::new();
                    props.insert("tag".to_string(), "menuitem".to_string());
                    if let Some(name) = attrs.get("name") {
                        props.insert("name".to_string(), name.clone());
                    }
                    symbols.push(Symbol {
                        name: id.clone(),
                        kind: "menuitem".to_string(),
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
                    if let Some(action) = attrs.get("action") {
                        relations.push(Relation {
                            from: id.clone(),
                            to: action.clone(),
                            rel_type: "uses".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                    return Some(id.clone());
                }
            }
            "button" => {
                let is_object_action = attrs.get("type").map(String::as_str) == Some("object")
                    || attrs
                        .get("name")
                        .map(|n| n.starts_with("action_") || n.starts_with("button_"))
                        .unwrap_or(false);
                if is_object_action {
                    if let Some(action_name) = attrs.get("name") {
                        let caller = if parent_scope.is_empty() {
                            "view".to_string()
                        } else {
                            parent_scope.to_string()
                        };
                        relations.push(Relation {
                            from: caller,
                            to: action_name.clone(),
                            rel_type: "framework_invokes".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
            "field" => {
                if attrs.get("name").map(String::as_str) == Some("code") {
                    let text = node.utf8_text(source).unwrap_or("");
                    let caller = if parent_scope.is_empty() {
                        "action".to_string()
                    } else {
                        parent_scope.to_string()
                    };
                    for cap in CALL_RE.captures_iter(text) {
                        if let Some(m) = cap.get(1) {
                            let method_name = m.as_str();
                            if !matches!(
                                method_name,
                                "print"
                                    | "len"
                                    | "str"
                                    | "int"
                                    | "bool"
                                    | "list"
                                    | "dict"
                                    | "set"
                                    | "range"
                                    | "enumerate"
                                    | "isinstance"
                                    | "issubclass"
                                    | "field"
                            ) {
                                relations.push(Relation {
                                    from: caller.clone(),
                                    to: method_name.to_string(),
                                    rel_type: "framework_invokes".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }
                } else if let Some(ref_val) = attrs.get("ref") {
                    if !parent_scope.is_empty() {
                        relations.push(Relation {
                            from: parent_scope.to_string(),
                            to: ref_val.clone(),
                            rel_type: "uses".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
            _ => {}
        }

        None
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

    fn parse_fallback(
        &self,
        content: &str,
        symbols: &mut Vec<Symbol>,
        relations: &mut Vec<Relation>,
    ) {
        for cap in RECORD_RE.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let id = m.as_str().to_string();
                symbols.push(Symbol {
                    name: id,
                    kind: "record".to_string(),
                    start_line: 1,
                    end_line: 1,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        for cap in BUTTON_RE.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let btn_name = m.as_str().to_string();
                relations.push(Relation {
                    from: "view".to_string(),
                    to: btn_name,
                    rel_type: "framework_invokes".to_string(),
                    properties: HashMap::new(),
                });
            }
        }
    }
}

impl Parser for XmlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        if let Some(tree) = parse_with_wasm_safe("html", self.wasm_bytes, content) {
            self.walk(
                tree.root_node(),
                content.as_bytes(),
                &mut symbols,
                &mut relations,
                "",
            );
        }

        if symbols.is_empty() {
            self.parse_fallback(content, &mut symbols, &mut relations);
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
    use super::*;

    #[test]
    fn test_req_902330_xml_extracts_odoo_records_and_buttons() {
        let p = XmlParser::new();
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<odoo>
    <record id="view_partner_form" model="ir.ui.view">
        <field name="name">res.partner.form</field>
        <field name="model">res.partner</field>
        <field name="arch" type="xml">
            <form>
                <header>
                    <button name="action_confirm" string="Confirm" type="object" class="oe_highlight"/>
                    <button name="action_cancel" string="Cancel" type="object"/>
                </header>
            </form>
        </field>
    </record>

    <record id="cron_cleanup_data" model="ir.cron">
        <field name="name">Data Cleaner</field>
        <field name="model_id" ref="model_res_partner"/>
        <field name="state">code</field>
        <field name="code">
            model.action_clean_old_partners()
        </field>
    </record>

    <template id="portal_my_home" name="My Portal">
        <div>Home</div>
    </template>

    <menuitem id="menu_partner_root" name="Partners" action="action_partner_form"/>
</odoo>
"#;
        let result = p.parse(xml);
        assert!(!result.symbols.is_empty(), "Must extract symbols from XML");

        let record_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "view_partner_form");
        assert!(
            record_sym.is_some(),
            "Record view_partner_form must be extracted"
        );
        assert_eq!(record_sym.unwrap().kind, "record");
        assert!(record_sym.unwrap().is_entry_point);

        let template_sym = result.symbols.iter().find(|s| s.name == "portal_my_home");
        assert!(
            template_sym.is_some(),
            "Template portal_my_home must be extracted"
        );

        let menu_sym = result
            .symbols
            .iter()
            .find(|s| s.name == "menu_partner_root");
        assert!(
            menu_sym.is_some(),
            "Menu menu_partner_root must be extracted"
        );

        let confirm_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "framework_invokes" && r.to == "action_confirm");
        assert!(
            confirm_rel.is_some(),
            "Button action_confirm must emit framework_invokes relation: {:?}",
            result.relations
        );
        assert_eq!(confirm_rel.unwrap().from, "view_partner_form");

        let cancel_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "framework_invokes" && r.to == "action_cancel");
        assert!(
            cancel_rel.is_some(),
            "Button action_cancel must emit framework_invokes relation"
        );

        let cron_rel = result
            .relations
            .iter()
            .find(|r| r.rel_type == "framework_invokes" && r.to == "action_clean_old_partners");
        assert!(
            cron_rel.is_some(),
            "Cron code field must emit framework_invokes for action_clean_old_partners: {:?}",
            result.relations
        );
    }
}
