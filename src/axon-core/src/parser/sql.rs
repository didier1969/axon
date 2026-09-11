use super::{ExtractionResult, Parser, Relation, Symbol};
use regex::Regex;
use std::collections::HashMap;

pub struct SqlParser {
    create_table_re: Regex,
    create_view_re: Regex,
    create_func_re: Regex,
    create_macro_re: Regex,
    dml_re: Regex,
    table_fk_re: Regex,
    inline_fk_re: Regex,
    col_type_re: Regex,
}

impl Default for SqlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlParser {
    pub fn new() -> Self {
        Self {
            create_table_re: Regex::new(r"(?im)^\s*CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
            create_view_re: Regex::new(r"(?im)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:MATERIALIZED\s+)?VIEW\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
            create_func_re: Regex::new(r"(?im)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
            create_macro_re: Regex::new(r"(?im)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:TEMPORARY\s+)?MACRO\s+(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
            dml_re: Regex::new(r"(?im)^\s*(INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
            table_fk_re: Regex::new(r"(?im)(?:CONSTRAINT\s+[`\x22]?(\w+)[`\x22]?\s+)?FOREIGN\s+KEY\s*\(\s*[`\x22]?(\w+)[`\x22]?\s*\)\s*REFERENCES\s+[`\x22]?(\w+)[`\x22]?(?:\s*\(\s*[`\x22]?(\w+)[`\x22]?\s*\))?").unwrap(),
            inline_fk_re: Regex::new(r"(?im)REFERENCES\s+[`\x22]?(\w+)[`\x22]?(?:\s*\(\s*[`\x22]?(\w+)[`\x22]?\s*\))?").unwrap(),
            col_type_re: Regex::new(r"(?i)^[`\x22]?(\w+)[`\x22]?\s+([A-Za-z0-9_]+(?:\s*\([^)]*\))?(?:\s+(?:PRECISION|VARYING))?)").unwrap(),
        }
    }

    fn find_statement_end(lines: &[&str], start_idx: usize) -> usize {
        for (i, line) in lines.iter().enumerate().skip(start_idx) {
            if line.contains(';') {
                return i + 1;
            }
        }
        lines.len()
    }

    fn find_matching_paren(content: &str, open_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut depth = 0;
        let mut in_single_quote = false;
        let mut in_double_quote = false;

        for (i, &b) in bytes.iter().enumerate().skip(open_pos) {
            match b {
                b'\'' if !in_double_quote => in_single_quote = !in_single_quote,
                b'"' if !in_single_quote => in_double_quote = !in_double_quote,
                b'(' if !in_single_quote && !in_double_quote => depth += 1,
                b')' if !in_single_quote && !in_double_quote => {
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

    fn split_sql_definitions(body: &str) -> Vec<(usize, &str)> {
        let mut defs = Vec::new();
        let mut depth = 0;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut start = 0;

        let bytes = body.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'\'' if !in_double_quote => in_single_quote = !in_single_quote,
                b'"' if !in_single_quote => in_double_quote = !in_double_quote,
                b'(' if !in_single_quote && !in_double_quote => depth += 1,
                b')' if !in_single_quote && !in_double_quote => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                b',' if depth == 0 && !in_single_quote && !in_double_quote => {
                    let chunk = body[start..i].trim();
                    if !chunk.is_empty() {
                        defs.push((start, chunk));
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        let tail = body[start..].trim();
        if !tail.is_empty() {
            defs.push((start, tail));
        }
        defs
    }
}

impl Parser for SqlParser {
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

        let lines: Vec<&str> = content.lines().collect();

        let get_line_no = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        for cap in self.create_table_re.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str().to_string();
                let start_byte = cap.get(0).unwrap().start();
                let line_no = get_line_no(start_byte);
                let end_line = Self::find_statement_end(&lines, line_no.saturating_sub(1));

                symbols.push(Symbol {
                    name: name.clone(),
                    kind: "table".to_string(),
                    start_line: line_no,
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

                // Check for parenthesized body
                let search_slice = &content[cap.get(0).unwrap().end()..];
                if let Some(open_rel) = search_slice.find('(') {
                    let open_pos = cap.get(0).unwrap().end() + open_rel;
                    if let Some(close_pos) = Self::find_matching_paren(content, open_pos) {
                        let body = &content[open_pos + 1..close_pos];
                        let body_offset = open_pos + 1;

                        for (rel_start, def) in Self::split_sql_definitions(body) {
                            let def_offset = body_offset + rel_start;
                            let def_line = get_line_no(def_offset);
                            let def_upper = def.to_ascii_uppercase();

                            // 1. Check table-level foreign key
                            if let Some(fk_cap) = self.table_fk_re.captures(def) {
                                let from_col = fk_cap
                                    .get(2)
                                    .map(|c| c.as_str().to_string())
                                    .unwrap_or_default();
                                let to_table = fk_cap
                                    .get(3)
                                    .map(|c| c.as_str().to_string())
                                    .unwrap_or_default();
                                let to_col = fk_cap
                                    .get(4)
                                    .map(|c| c.as_str().to_string())
                                    .unwrap_or_default();

                                let mut props = HashMap::new();
                                if !from_col.is_empty() {
                                    props.insert("from_column".to_string(), from_col);
                                }
                                if !to_col.is_empty() {
                                    props.insert("to_column".to_string(), to_col);
                                }
                                if let Some(c_name) = fk_cap.get(1) {
                                    props.insert(
                                        "constraint".to_string(),
                                        c_name.as_str().to_string(),
                                    );
                                }

                                relations.push(Relation {
                                    from: name.clone(),
                                    to: to_table,
                                    rel_type: "references".to_string(),
                                    properties: props,
                                });
                                continue;
                            }

                            // 2. Skip table-level PRIMARY KEY / UNIQUE / CHECK constraints
                            if def_upper.starts_with("CONSTRAINT")
                                || def_upper.starts_with("PRIMARY KEY")
                                || def_upper.starts_with("UNIQUE")
                                || def_upper.starts_with("CHECK")
                            {
                                continue;
                            }

                            // 3. Column definition
                            if let Some(col_cap) = self.col_type_re.captures(def) {
                                let col_name = col_cap
                                    .get(1)
                                    .map(|c| c.as_str().to_string())
                                    .unwrap_or_default();
                                let col_type = col_cap
                                    .get(2)
                                    .map(|c| c.as_str().trim().to_string())
                                    .unwrap_or_default();
                                let full_col_name = format!("{}.{}", name, col_name);

                                let mut col_props = HashMap::new();
                                if !col_type.is_empty() {
                                    col_props.insert("type".to_string(), col_type);
                                }
                                if def_upper.contains("PRIMARY KEY") {
                                    col_props.insert("primary_key".to_string(), "true".to_string());
                                }
                                if def_upper.contains("NOT NULL") {
                                    col_props.insert("not_null".to_string(), "true".to_string());
                                }

                                symbols.push(Symbol {
                                    name: full_col_name.clone(),
                                    kind: "column".to_string(),
                                    start_line: def_line,
                                    end_line: def_line,
                                    docstring: None,
                                    is_entry_point: false,
                                    is_public: true,
                                    tested: false,
                                    is_nif: false,
                                    is_unsafe: false,
                                    properties: col_props,
                                    embedding: None,
                                });

                                relations.push(Relation {
                                    from: name.clone(),
                                    to: full_col_name,
                                    rel_type: "contains".to_string(),
                                    properties: HashMap::new(),
                                });

                                // Check inline REFERENCES
                                if let Some(inline_fk) = self.inline_fk_re.captures(def) {
                                    let to_table = inline_fk
                                        .get(1)
                                        .map(|c| c.as_str().to_string())
                                        .unwrap_or_default();
                                    let to_col = inline_fk
                                        .get(2)
                                        .map(|c| c.as_str().to_string())
                                        .unwrap_or_default();

                                    let mut fk_props = HashMap::new();
                                    fk_props.insert("from_column".to_string(), col_name);
                                    if !to_col.is_empty() {
                                        fk_props.insert("to_column".to_string(), to_col);
                                    }

                                    relations.push(Relation {
                                        from: name.clone(),
                                        to: to_table,
                                        rel_type: "references".to_string(),
                                        properties: fk_props,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        for cap in self.create_view_re.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str().to_string();
                let start_byte = cap.get(0).unwrap().start();
                let line_no = get_line_no(start_byte);
                let end_line = Self::find_statement_end(&lines, line_no.saturating_sub(1));

                symbols.push(Symbol {
                    name,
                    kind: "view".to_string(),
                    start_line: line_no,
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
            }
        }

        for cap in self.create_func_re.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str().to_string();
                let start_byte = cap.get(0).unwrap().start();
                let line_no = get_line_no(start_byte);
                let end_line = Self::find_statement_end(&lines, line_no.saturating_sub(1));

                symbols.push(Symbol {
                    name,
                    kind: "function".to_string(),
                    start_line: line_no,
                    end_line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: true, // SQL functions can be complex/unsafe
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        for cap in self.create_macro_re.captures_iter(content) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str().to_string();
                let start_byte = cap.get(0).unwrap().start();
                let line_no = get_line_no(start_byte);
                let end_line = Self::find_statement_end(&lines, line_no.saturating_sub(1));

                let mut props = HashMap::new();
                props.insert("dialect".to_string(), "duckdb".to_string());

                symbols.push(Symbol {
                    name,
                    kind: "macro".to_string(),
                    start_line: line_no,
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

        for cap in self.dml_re.captures_iter(content) {
            if let (Some(m1), Some(m2)) = (cap.get(1), cap.get(2)) {
                let action_raw = m1.as_str().to_uppercase();
                let action = action_raw
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                let table = m2.as_str().to_string();

                let is_dangerous = action.contains("DELETE") || action.contains("UPDATE");
                let mut props = HashMap::new();
                if is_dangerous {
                    props.insert("dangerous".to_string(), "true".to_string());
                }

                relations.push(Relation {
                    from: "".to_string(),
                    to: format!("{}:{}", action, table),
                    rel_type: "calls".to_string(),
                    properties: props,
                });
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
    fn req_902663_sql_create_table_extracts_columns_and_foreign_keys() {
        let sql = r#"
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    organization_id INT REFERENCES organizations(id),
    created_at TIMESTAMP
);

CREATE TABLE orders (
    id INT PRIMARY KEY,
    user_id INT NOT NULL,
    amount NUMERIC(10, 2),
    CONSTRAINT fk_order_user FOREIGN KEY (user_id) REFERENCES users(id)
);
"#;
        let parser = SqlParser::new();
        let result = parser.parse(sql);

        // Table symbols
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "users" && s.kind == "table"));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "orders" && s.kind == "table"));

        // Column symbols
        assert!(result.symbols.iter().any(|s| s.name == "users.id"
            && s.kind == "column"
            && s.properties.get("primary_key") == Some(&"true".to_string())));
        assert!(result.symbols.iter().any(|s| s.name == "users.name"
            && s.kind == "column"
            && s.properties.get("type") == Some(&"VARCHAR(255)".to_string())));
        assert!(result
            .symbols
            .iter()
            .any(|s| s.name == "users.organization_id"
                && s.kind == "column"
                && s.properties.get("type") == Some(&"INT".to_string())));
        assert!(result.symbols.iter().any(|s| s.name == "orders.amount"
            && s.kind == "column"
            && s.properties.get("type") == Some(&"NUMERIC(10, 2)".to_string())));

        // Table contains columns
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "users" && r.to == "users.id" && r.rel_type == "contains"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "orders" && r.to == "orders.amount" && r.rel_type == "contains"));

        // Foreign keys -> references relations
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "users" && r.to == "organizations" && r.rel_type == "references"));
        assert!(result
            .relations
            .iter()
            .any(|r| r.from == "orders" && r.to == "users" && r.rel_type == "references"));
    }

    #[test]
    fn test_duckdb_macro() {
        let sql = r#"
            CREATE MACRO add_tax(price, rate) AS price * (1 + rate);
            CREATE OR REPLACE TEMPORARY MACRO dynamic_pricing(base, surge) AS base * surge;
        "#;
        let parser = SqlParser::new();
        let result = parser.parse(sql);
        let add_tax = result
            .symbols
            .iter()
            .find(|s| s.name == "add_tax")
            .expect("add_tax symbol");
        assert_eq!(add_tax.kind, "macro");
        assert_eq!(
            add_tax.properties.get("dialect").map(String::as_str),
            Some("duckdb")
        );

        let dyn_price = result
            .symbols
            .iter()
            .find(|s| s.name == "dynamic_pricing")
            .expect("dynamic_pricing symbol");
        assert_eq!(dyn_price.kind, "macro");
        assert_eq!(
            dyn_price.properties.get("dialect").map(String::as_str),
            Some("duckdb")
        );
    }
}
