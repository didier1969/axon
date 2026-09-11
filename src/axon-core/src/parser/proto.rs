use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static PACKAGE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*package\s+([a-zA-Z0-9_\.]+)\s*;"#).unwrap());
static IMPORT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*import\s+["']([^"']+)["']\s*;"#).unwrap());
static MESSAGE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*message\s+([a-zA-Z0-9_]+)\s*\{"#).unwrap());
static SERVICE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*service\s+([a-zA-Z0-9_]+)\s*\{"#).unwrap());
static ENUM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*enum\s+([a-zA-Z0-9_]+)\s*\{"#).unwrap());
static RPC_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*rpc\s+([a-zA-Z0-9_]+)\s*\(\s*(?:stream\s+)?([a-zA-Z0-9_\.]+)\s*\)\s*returns\s*\(\s*(?:stream\s+)?([a-zA-Z0-9_\.]+)\s*\)"#).unwrap()
});
static FIELD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:repeated\s+|optional\s+)?([a-zA-Z0-9_\.]+)\s+([a-zA-Z0-9_]+)\s*=\s*(\d+)\s*;"#).unwrap()
});

const PRIMITIVE_TYPES: &[&str] = &[
    "int32", "int64", "uint32", "uint64", "sint32", "sint64", "fixed32", "fixed64", "sfixed32",
    "sfixed64", "bool", "string", "bytes", "float", "double",
];

pub struct ProtoParser;

impl Default for ProtoParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtoParser {
    pub fn new() -> Self {
        Self
    }

    fn find_matching_brace(content: &str, open_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut depth = 0;
        let mut in_string = false;
        let mut quote_char = b'"';

        for (i, &b) in bytes.iter().enumerate().skip(open_pos) {
            match b {
                b'"' | b'\'' if !in_string => {
                    in_string = true;
                    quote_char = b;
                }
                b if in_string && b == quote_char => {
                    in_string = false;
                }
                b'{' if !in_string => {
                    depth += 1;
                }
                b'}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

impl Parser for ProtoParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        if content.is_empty() {
            return ExtractionResult {
                project_code: None,
                symbols,
                relations,
            };
        }

        let get_line_no = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        let package_name = PACKAGE_RE
            .captures(content)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());

        // Imports
        for cap in IMPORT_RE.captures_iter(content) {
            if let Some(import_path) = cap.get(1) {
                let from_name = package_name.clone().unwrap_or_default();
                relations.push(Relation {
                    from: from_name,
                    to: import_path.as_str().to_string(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // Messages
        for cap in MESSAGE_RE.captures_iter(content) {
            let msg_name = cap.get(1).unwrap().as_str().to_string();
            let start_byte = cap.get(0).unwrap().start();
            let open_brace = cap.get(0).unwrap().end() - 1;

            if let Some(close_brace) = Self::find_matching_brace(content, open_brace) {
                let start_line = get_line_no(start_byte);
                let end_line = get_line_no(close_brace);
                let body = &content[open_brace + 1..close_brace];
                let body_offset = open_brace + 1;

                let mut props = HashMap::new();
                if let Some(ref pkg) = package_name {
                    props.insert("package".to_string(), pkg.clone());
                }

                symbols.push(Symbol {
                    name: msg_name.clone(),
                    kind: "message".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });

                // Fields inside message
                for field_cap in FIELD_RE.captures_iter(body) {
                    let field_type = field_cap.get(1).unwrap().as_str().to_string();
                    let field_name = field_cap.get(2).unwrap().as_str().to_string();
                    let tag_num = field_cap.get(3).unwrap().as_str().to_string();
                    let field_offset = body_offset + field_cap.get(0).unwrap().start();
                    let field_line = get_line_no(field_offset);

                    let full_field_name = format!("{}.{}", msg_name, field_name);
                    let mut field_props = HashMap::new();
                    field_props.insert("type".to_string(), field_type.clone());
                    field_props.insert("tag".to_string(), tag_num);

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "field".to_string(),
                        start_line: field_line,
                        end_line: field_line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: field_props,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: msg_name.clone(),
                        to: full_field_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });

                    // References custom types
                    if !PRIMITIVE_TYPES.contains(&field_type.as_str()) {
                        let target_clean = field_type
                            .rsplit('.')
                            .next()
                            .unwrap_or(field_type.as_str())
                            .to_string();
                        relations.push(Relation {
                            from: msg_name.clone(),
                            to: target_clean,
                            rel_type: "references".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
        }

        // Enums
        for cap in ENUM_RE.captures_iter(content) {
            let enum_name = cap.get(1).unwrap().as_str().to_string();
            let start_byte = cap.get(0).unwrap().start();
            let open_brace = cap.get(0).unwrap().end() - 1;

            if let Some(close_brace) = Self::find_matching_brace(content, open_brace) {
                let start_line = get_line_no(start_byte);
                let end_line = get_line_no(close_brace);

                let mut props = HashMap::new();
                if let Some(ref pkg) = package_name {
                    props.insert("package".to_string(), pkg.clone());
                }

                symbols.push(Symbol {
                    name: enum_name,
                    kind: "enum".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // Services & RPCs
        for cap in SERVICE_RE.captures_iter(content) {
            let service_name = cap.get(1).unwrap().as_str().to_string();
            let start_byte = cap.get(0).unwrap().start();
            let open_brace = cap.get(0).unwrap().end() - 1;

            if let Some(close_brace) = Self::find_matching_brace(content, open_brace) {
                let start_line = get_line_no(start_byte);
                let end_line = get_line_no(close_brace);
                let body = &content[open_brace + 1..close_brace];
                let body_offset = open_brace + 1;

                let mut props = HashMap::new();
                if let Some(ref pkg) = package_name {
                    props.insert("package".to_string(), pkg.clone());
                }

                symbols.push(Symbol {
                    name: service_name.clone(),
                    kind: "service".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });

                for rpc_cap in RPC_RE.captures_iter(body) {
                    let method_name = rpc_cap.get(1).unwrap().as_str().to_string();
                    let input_type = rpc_cap.get(2).unwrap().as_str().to_string();
                    let output_type = rpc_cap.get(3).unwrap().as_str().to_string();
                    let rpc_offset = body_offset + rpc_cap.get(0).unwrap().start();
                    let rpc_line = get_line_no(rpc_offset);

                    let full_rpc_name = format!("{}.{}", service_name, method_name);

                    let mut rpc_props = HashMap::new();
                    rpc_props.insert("service".to_string(), service_name.clone());
                    rpc_props.insert("input_type".to_string(), input_type.clone());
                    rpc_props.insert("output_type".to_string(), output_type.clone());

                    symbols.push(Symbol {
                        name: full_rpc_name.clone(),
                        kind: "rpc".to_string(),
                        start_line: rpc_line,
                        end_line: rpc_line,
                        docstring: None,
                        is_entry_point: true,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: rpc_props,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: service_name.clone(),
                        to: full_rpc_name.clone(),
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });

                    let input_clean = input_type
                        .rsplit('.')
                        .next()
                        .unwrap_or(input_type.as_str())
                        .to_string();
                    let mut input_rel_props = HashMap::new();
                    input_rel_props.insert("role".to_string(), "input".to_string());
                    relations.push(Relation {
                        from: full_rpc_name.clone(),
                        to: input_clean,
                        rel_type: "references".to_string(),
                        properties: input_rel_props,
                    });

                    let output_clean = output_type
                        .rsplit('.')
                        .next()
                        .unwrap_or(output_type.as_str())
                        .to_string();
                    let mut output_rel_props = HashMap::new();
                    output_rel_props.insert("role".to_string(), "output".to_string());
                    relations.push(Relation {
                        from: full_rpc_name,
                        to: output_clean,
                        rel_type: "references".to_string(),
                        properties: output_rel_props,
                    });
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
    use super::*;

    #[test]
    fn req_902664_proto_parses_messages_services_and_references() {
        let proto = r#"
syntax = "proto3";

package acme.orders.v1;

import "acme/common.proto";

message OrderItem {
    string product_id = 1;
    int32 quantity = 2;
}

message CreateOrderRequest {
    string user_id = 1;
    repeated OrderItem items = 2;
}

message OrderResponse {
    string order_id = 1;
    string status = 2;
}

service OrderService {
    rpc CreateOrder (CreateOrderRequest) returns (OrderResponse);
}
"#;
        let parser = ProtoParser::new();
        let result = parser.parse(proto);

        // Service & RPC
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "OrderService" && s.kind == "service"));
        let rpc = result
            .symbols
            .iter()
            .find(|s| s.name == "OrderService.CreateOrder")
            .expect("RPC must exist");
        assert!(rpc.is_entry_point, "RPC method must be entry point");
        assert_eq!(rpc.kind, "rpc");

        // Messages & Fields
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "CreateOrderRequest" && s.kind == "message"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "OrderItem.product_id" && s.kind == "field"));

        // RPC relations to input/output
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "OrderService.CreateOrder"
                && r.to == "CreateOrderRequest"
                && r.rel_type == "references"
                && r.properties.get("role") == Some(&"input".to_string())));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "OrderService.CreateOrder"
                && r.to == "OrderResponse"
                && r.rel_type == "references"
                && r.properties.get("role") == Some(&"output".to_string())));

        // Field reference to custom message
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "CreateOrderRequest"
                && r.to == "OrderItem"
                && r.rel_type == "references"));

        // Import
        assert!(result
            .relations
            .iter()
            .any(|r| r.to == "acme/common.proto" && r.rel_type == "imports"));
    }
}
