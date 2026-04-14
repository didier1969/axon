use super::{ExtractionResult, Parser, Relation, Symbol};
use regex::Regex;
use std::collections::HashMap;

pub struct SqlParser {
    create_table_re: Regex,
    create_view_re: Regex,
    create_func_re: Regex,
    dml_re: Regex,
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
            dml_re: Regex::new(r"(?im)^\s*(INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+(?:`|\x22)?(\w+)(?:`|\x22)?").unwrap(),
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
                    name,
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
