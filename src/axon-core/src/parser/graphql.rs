use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static TYPE_DEF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(type|interface|input|enum)\s+([a-zA-Z0-9_]+)(?:\s+implements\s+([a-zA-Z0-9_,\s]+))?\s*\{"#).unwrap()
});
static UNION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*union\s+([a-zA-Z0-9_]+)\s*=\s*([^;\n]+)"#).unwrap());
static FIELD_DEF_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*([a-zA-Z0-9_]+)(?:\s*\([^)]*\))?\s*:\s*([a-zA-Z0-9_!\[\]]+)"#).unwrap()
});

const GRAPHQL_SCALARS: &[&str] = &["String", "Int", "Float", "Boolean", "ID"];

pub struct GraphQLParser;

impl Default for GraphQLParser {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphQLParser {
    pub fn new() -> Self {
        Self
    }

    fn find_matching_brace(content: &str, open_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut depth = 0;
        let mut in_string = false;

        for (i, &b) in bytes.iter().enumerate().skip(open_pos) {
            match b {
                b'"' => {
                    in_string = !in_string;
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

    fn clean_type(raw: &str) -> String {
        raw.replace(['!', '[', ']'], "").trim().to_string()
    }
}

impl Parser for GraphQLParser {
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

        // Unions
        for cap in UNION_RE.captures_iter(content) {
            let union_name = cap.get(1).unwrap().as_str().to_string();
            let targets_raw = cap.get(2).unwrap().as_str();
            let start_byte = cap.get(0).unwrap().start();
            let line_no = get_line_no(start_byte);

            symbols.push(Symbol {
                name: union_name.clone(),
                kind: "union".to_string(),
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

            for target in targets_raw.split('|') {
                let clean_target = target.trim();
                if !clean_target.is_empty() {
                    relations.push(Relation {
                        from: union_name.clone(),
                        to: clean_target.to_string(),
                        rel_type: "references".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // Type / Interface / Input / Enum definitions
        for cap in TYPE_DEF_RE.captures_iter(content) {
            let def_kind = cap.get(1).unwrap().as_str().to_string();
            let def_name = cap.get(2).unwrap().as_str().to_string();
            let implements_raw = cap.get(3).map(|m| m.as_str().to_string());
            let start_byte = cap.get(0).unwrap().start();
            let open_brace = cap.get(0).unwrap().end() - 1;

            if let Some(close_brace) = Self::find_matching_brace(content, open_brace) {
                let start_line = get_line_no(start_byte);
                let end_line = get_line_no(close_brace);
                let body = &content[open_brace + 1..close_brace];
                let body_offset = open_brace + 1;

                symbols.push(Symbol {
                    name: def_name.clone(),
                    kind: def_kind.clone(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });

                // Implements relations
                if let Some(impl_str) = implements_raw {
                    for iface in impl_str.split(',') {
                        let clean_iface = iface.trim();
                        if !clean_iface.is_empty() {
                            relations.push(Relation {
                                from: def_name.clone(),
                                to: clean_iface.to_string(),
                                rel_type: "implements".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }

                // Fields
                if def_kind != "enum" {
                    for field_cap in FIELD_DEF_RE.captures_iter(body) {
                        let field_name = field_cap.get(1).unwrap().as_str().to_string();
                        let raw_type = field_cap.get(2).unwrap().as_str();
                        let field_offset = body_offset + field_cap.get(0).unwrap().start();
                        let field_line = get_line_no(field_offset);

                        let full_field_name = format!("{}.{}", def_name, field_name);
                        let clean_type = Self::clean_type(raw_type);

                        let is_root_op =
                            matches!(def_name.as_str(), "Query" | "Mutation" | "Subscription");
                        let field_kind = if def_name == "Query" {
                            "query".to_string()
                        } else if def_name == "Mutation" {
                            "mutation".to_string()
                        } else if def_name == "Subscription" {
                            "subscription".to_string()
                        } else {
                            "field".to_string()
                        };

                        let mut field_props = HashMap::new();
                        field_props.insert("type".to_string(), clean_type.clone());

                        symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: field_kind,
                            start_line: field_line,
                            end_line: field_line,
                            docstring: None,
                            is_entry_point: is_root_op,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: field_props,
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: def_name.clone(),
                            to: full_field_name.clone(),
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });

                        if !GRAPHQL_SCALARS.contains(&clean_type.as_str()) {
                            relations.push(Relation {
                                from: full_field_name,
                                to: clean_type,
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_902664_graphql_parses_types_queries_mutations_and_references() {
        let schema = r#"
interface Node {
  id: ID!
}

type User implements Node {
  id: ID!
  name: String!
  posts: [Post!]!
}

type Post {
  id: ID!
  title: String!
  author: User!
}

type Query {
  getUser(id: ID!): User
  allPosts: [Post!]!
}

type Mutation {
  createPost(title: String!, authorId: ID!): Post!
}
"#;
        let parser = GraphQLParser::new();
        let result = parser.parse(schema);

        // Interface
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "Node" && s.kind == "interface"));

        // User implements Node
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "User" && s.kind == "type"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "User" && r.to == "Node" && r.rel_type == "implements"));

        // Query and Mutation as entry points
        let get_user = result
            .symbols
            .iter()
            .find(|s| s.name == "Query.getUser")
            .expect("Query.getUser must exist");
        assert!(get_user.is_entry_point);
        assert_eq!(get_user.kind, "query");

        let create_post = result
            .symbols
            .iter()
            .find(|s| s.name == "Mutation.createPost")
            .expect("Mutation.createPost must exist");
        assert!(create_post.is_entry_point);
        assert_eq!(create_post.kind, "mutation");

        // References to types
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "Query.getUser" && r.to == "User" && r.rel_type == "references"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "Mutation.createPost"
                && r.to == "Post"
                && r.rel_type == "references"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "Post.author" && r.to == "User" && r.rel_type == "references"));
    }
}
