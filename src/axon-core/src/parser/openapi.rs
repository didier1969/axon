use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

static YAML_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s{2,4}(/[a-zA-Z0-9_\-\/{}\.]+):\s*$"#).unwrap());
static YAML_METHOD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?im)^\s{4,6}(get|post|put|delete|patch|options|head):\s*$"#).unwrap()
});
static YAML_SCHEMA_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s{4,6}([a-zA-Z0-9_]+):\s*$"#).unwrap());
static YAML_REF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"["']?\$ref["']?\s*:\s*["']?#/(?:components/schemas|definitions)/([a-zA-Z0-9_]+)["']?"#,
    )
    .unwrap()
});

pub struct OpenApiParser;

impl Default for OpenApiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenApiParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_json(&self, content: &str) -> Option<ExtractionResult> {
        let json_val: Value = serde_json::from_str(content).ok()?;
        let obj = json_val.as_object()?;

        let is_openapi = obj.contains_key("openapi") || obj.contains_key("swagger");
        if !is_openapi && !obj.contains_key("paths") {
            return None;
        }

        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        // 1. Schemas
        let mut schema_map = HashMap::new();
        if let Some(components) = obj.get("components").and_then(|c| c.as_object()) {
            if let Some(schemas) = components.get("schemas").and_then(|s| s.as_object()) {
                schema_map.extend(schemas.clone());
            }
        }
        if let Some(definitions) = obj.get("definitions").and_then(|d| d.as_object()) {
            schema_map.extend(definitions.clone());
        }

        for (schema_name, schema_val) in &schema_map {
            let mut props = HashMap::new();
            if let Some(t) = schema_val.get("type").and_then(|t| t.as_str()) {
                props.insert("type".to_string(), t.to_string());
            }

            symbols.push(Symbol {
                name: schema_name.clone(),
                kind: "schema".to_string(),
                start_line: 1,
                end_line: 1,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });

            if let Some(properties) = schema_val.get("properties").and_then(|p| p.as_object()) {
                for (prop_name, prop_val) in properties {
                    let full_field = format!("{}.{}", schema_name, prop_name);
                    let mut field_props = HashMap::new();
                    if let Some(t) = prop_val.get("type").and_then(|t| t.as_str()) {
                        field_props.insert("type".to_string(), t.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field.clone(),
                        kind: "field".to_string(),
                        start_line: 1,
                        end_line: 1,
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
                        from: schema_name.clone(),
                        to: full_field,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });

                    // Check $ref in property
                    if let Some(ref_str) = prop_val.get("$ref").and_then(|r| r.as_str()) {
                        if let Some(target) = ref_str.rsplit('/').next() {
                            relations.push(Relation {
                                from: schema_name.clone(),
                                to: target.to_string(),
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }
        }

        // 2. Paths & Operations
        if let Some(paths) = obj.get("paths").and_then(|p| p.as_object()) {
            for (path, path_item) in paths {
                if let Some(methods) = path_item.as_object() {
                    for (method, op_val) in methods {
                        let method_upper = method.to_ascii_uppercase();
                        if !["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"]
                            .contains(&method_upper.as_str())
                        {
                            continue;
                        }

                        let op_name = format!("{} {}", method_upper, path);
                        let mut op_props = HashMap::new();
                        op_props.insert("method".to_string(), method_upper.clone());
                        op_props.insert("path".to_string(), path.clone());

                        if let Some(op_id) = op_val.get("operationId").and_then(|o| o.as_str()) {
                            op_props.insert("operation_id".to_string(), op_id.to_string());
                        }

                        symbols.push(Symbol {
                            name: op_name.clone(),
                            kind: "endpoint".to_string(),
                            start_line: 1,
                            end_line: 1,
                            docstring: op_val
                                .get("summary")
                                .and_then(|s| s.as_str())
                                .map(|s| s.to_string()),
                            is_entry_point: true,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: op_props,
                            embedding: None,
                        });

                        // Scan op_val for $ref
                        let op_str = op_val.to_string();
                        for cap in YAML_REF_RE.captures_iter(&op_str) {
                            if let Some(target) = cap.get(1) {
                                relations.push(Relation {
                                    from: op_name.clone(),
                                    to: target.as_str().to_string(),
                                    rel_type: "references".to_string(),
                                    properties: HashMap::new(),
                                });
                            }
                        }
                    }
                }
            }
        }

        Some(ExtractionResult {
            project_code: None,
            symbols,
            relations,
        })
    }

    fn parse_yaml_lines(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let lines: Vec<&str> = content.lines().collect();
        let get_line_no = |idx: usize| -> usize { idx + 1 };

        let mut in_paths = false;
        let mut in_components = false;
        let mut in_schemas = false;
        let mut current_path = String::new();
        let mut current_endpoint = String::new();
        let mut current_schema = String::new();

        for (idx, line) in lines.iter().enumerate() {
            let line_no = get_line_no(idx);
            let trimmed = line.trim();

            if trimmed == "paths:" {
                in_paths = true;
                in_components = false;
                in_schemas = false;
                continue;
            } else if trimmed == "components:" || trimmed == "definitions:" {
                in_paths = false;
                in_components = true;
                in_schemas = trimmed == "definitions:";
                continue;
            } else if in_components && (trimmed == "schemas:" || trimmed == "models:") {
                in_schemas = true;
                continue;
            } else if !line.starts_with(' ') && !trimmed.is_empty() && trimmed.ends_with(':') {
                in_paths = false;
                in_components = false;
                in_schemas = false;
                continue;
            }

            if in_paths {
                if let Some(cap) = YAML_PATH_RE.captures(line) {
                    current_path = cap.get(1).unwrap().as_str().to_string();
                    current_endpoint.clear();
                    continue;
                }

                if let Some(cap) = YAML_METHOD_RE.captures(line) {
                    let method = cap.get(1).unwrap().as_str().to_ascii_uppercase();
                    current_endpoint = format!("{} {}", method, current_path);

                    let mut props = HashMap::new();
                    props.insert("method".to_string(), method);
                    props.insert("path".to_string(), current_path.clone());

                    symbols.push(Symbol {
                        name: current_endpoint.clone(),
                        kind: "endpoint".to_string(),
                        start_line: line_no,
                        end_line: line_no,
                        docstring: None,
                        is_entry_point: true,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: props,
                        embedding: None,
                    });
                    continue;
                }

                if !current_endpoint.is_empty() {
                    if let Some(cap) = YAML_REF_RE.captures(line) {
                        let target = cap.get(1).unwrap().as_str().to_string();
                        relations.push(Relation {
                            from: current_endpoint.clone(),
                            to: target,
                            rel_type: "references".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            if in_schemas {
                if let Some(cap) = YAML_SCHEMA_RE.captures(line) {
                    let s_name = cap.get(1).unwrap().as_str().to_string();
                    current_schema = s_name.clone();

                    symbols.push(Symbol {
                        name: s_name,
                        kind: "schema".to_string(),
                        start_line: line_no,
                        end_line: line_no,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: HashMap::new(),
                        embedding: None,
                    });
                    continue;
                }

                if !current_schema.is_empty() {
                    if let Some(cap) = YAML_REF_RE.captures(line) {
                        let target = cap.get(1).unwrap().as_str().to_string();
                        relations.push(Relation {
                            from: current_schema.clone(),
                            to: target,
                            rel_type: "references".to_string(),
                            properties: HashMap::new(),
                        });
                    }
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

impl Parser for OpenApiParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        if let Some(json_res) = self.parse_json(content) {
            return json_res;
        }

        self.parse_yaml_lines(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_902664_openapi_parses_json_spec() {
        let json_spec = r##"{
  "openapi": "3.0.0",
  "paths": {
    "/users": {
      "get": {
        "operationId": "listUsers",
        "responses": {
          "200": {
            "content": {
              "application/json": {
                "schema": {
                  "$ref": "#/components/schemas/UserList"
                }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "User": {
        "type": "object",
        "properties": {
          "id": { "type": "string" },
          "name": { "type": "string" }
        }
      },
      "UserList": {
        "type": "object",
        "properties": {
          "items": {
            "$ref": "#/components/schemas/User"
          }
        }
      }
    }
  }
}"##;
        let parser = OpenApiParser::new();
        let result = parser.parse(json_spec);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "GET /users" && s.kind == "endpoint" && s.is_entry_point));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == "schema"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User.id" && s.kind == "field"));

        // Endpoint references UserList
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "GET /users" && r.to == "UserList" && r.rel_type == "references"));

        // UserList references User
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "UserList" && r.to == "User" && r.rel_type == "references"));
    }

    #[test]
    fn req_902664_openapi_parses_yaml_spec() {
        let yaml_spec = r##"
openapi: 3.0.0
paths:
  /orders:
    post:
      summary: Create order
      responses:
        201:
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Order'
components:
  schemas:
    Order:
      type: object
"##;
        let parser = OpenApiParser::new();
        let result = parser.parse(yaml_spec);

        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "POST /orders" && s.kind == "endpoint" && s.is_entry_point));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "Order" && s.kind == "schema"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "POST /orders" && r.to == "Order" && r.rel_type == "references"));
    }
}
