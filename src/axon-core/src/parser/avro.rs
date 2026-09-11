use super::{ExtractionResult, Parser, Relation, Symbol};
use serde_json::Value;
use std::collections::HashMap;

pub struct AvroParser;

impl Default for AvroParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AvroParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for AvroParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let value: Value = match serde_json::from_str(content) {
            Ok(v) => v,
            Err(_) => {
                return ExtractionResult {
                    project_code: None,
                    symbols: Vec::new(),
                    relations: Vec::new(),
                }
            }
        };

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let schema_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("record");
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("AvroSchema");
        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let doc = value
            .get("doc")
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        let full_name = if namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", namespace, name)
        };

        let mut record_props = HashMap::new();
        record_props.insert("schema_type".to_string(), schema_type.to_string());
        if !namespace.is_empty() {
            record_props.insert("namespace".to_string(), namespace.to_string());
        }

        let sym_kind = format!("avro_{}", schema_type);
        symbols.push(Symbol {
            name: full_name.clone(),
            kind: sym_kind,
            start_line: 1,
            end_line: content.lines().count().max(1),
            docstring: doc,
            is_entry_point: true,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: record_props,
            embedding: None,
        });

        // Parse fields
        if let Some(fields) = value.get("fields").and_then(|v| v.as_array()) {
            let mut has_sensitive_fields = false;

            for field in fields {
                let Some(fname) = field.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let ftype = field
                    .get("type")
                    .map(|v| {
                        if v.is_string() {
                            v.as_str().unwrap().to_string()
                        } else {
                            v.to_string()
                        }
                    })
                    .unwrap_or_default();
                let fdoc = field
                    .get("doc")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string);

                let full_field_name = format!("{}.{}", full_name, fname);
                let mut field_props = HashMap::new();
                if !ftype.is_empty() {
                    field_props.insert("type".to_string(), ftype);
                }

                if let Some(kind) = super::is_sensitive_name(fname) {
                    has_sensitive_fields = true;
                    field_props.insert("is_sensitive".to_string(), "true".to_string());
                    field_props.insert("pii_kind".to_string(), kind.to_string());
                }

                symbols.push(Symbol {
                    name: full_field_name.clone(),
                    kind: "avro_field".to_string(),
                    start_line: 1,
                    end_line: 1,
                    docstring: fdoc,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: field_props,
                    embedding: None,
                });

                relations.push(Relation {
                    from: full_name.clone(),
                    to: full_field_name,
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }

            if has_sensitive_fields {
                if let Some(rec_sym) = symbols.iter_mut().find(|s| s.name == full_name) {
                    rec_sym
                        .properties
                        .insert("has_sensitive_fields".to_string(), "true".to_string());
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
    fn test_avro_schema_parsing() {
        let avsc = r#"{
  "type": "record",
  "name": "CustomerEvent",
  "namespace": "com.nexus.orders",
  "doc": "Customer order event",
  "fields": [
    {"name": "order_id", "type": "string"},
    {"name": "customer_id", "type": "string"},
    {"name": "credit_card", "type": "string"},
    {"name": "secret_token", "type": "string"}
  ]
}"#;

        let parser = AvroParser::new();
        let res = parser.parse(avsc);

        let rec = res
            .symbols
            .iter()
            .find(|s| s.name == "com.nexus.orders.CustomerEvent")
            .unwrap();
        assert_eq!(rec.kind, "avro_record");
        assert_eq!(
            rec.properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(rec.docstring.as_deref(), Some("Customer order event"));

        let cc = res
            .symbols
            .iter()
            .find(|s| s.name == "com.nexus.orders.CustomerEvent.credit_card")
            .unwrap();
        assert_eq!(
            cc.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            cc.properties.get("pii_kind").map(|s| s.as_str()),
            Some("financial")
        );

        let token = res
            .symbols
            .iter()
            .find(|s| s.name == "com.nexus.orders.CustomerEvent.secret_token")
            .unwrap();
        assert_eq!(
            token.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            token.properties.get("pii_kind").map(|s| s.as_str()),
            Some("secret")
        );
    }
}
