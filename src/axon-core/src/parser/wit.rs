use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug)]
struct BracedBlock<'a> {
    kind: &'a str,
    name: &'a str,
    body: &'a str,
    start_line: usize,
    end_line: usize,
}

fn extract_braced_blocks<'a>(
    content: &'a str,
    keywords: &[&'a str],
    base_line_offset: usize,
) -> Vec<BracedBlock<'a>> {
    let mut blocks = Vec::new();
    let bytes = content.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        // Skip single-line comments
        if i + 1 < n && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Skip multi-line comments /* ... */
        if i + 1 < n && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < n {
                i += 2;
            }
            continue;
        }

        let mut matched_kw = None;
        for &kw in keywords {
            if content[i..].starts_with(kw) {
                let before_ok = i == 0 || bytes[i - 1].is_ascii_whitespace();
                let after_pos = i + kw.len();
                let after_ok = after_pos < n && bytes[after_pos].is_ascii_whitespace();
                if before_ok && after_ok {
                    matched_kw = Some(kw);
                    break;
                }
            }
        }

        if let Some(kw) = matched_kw {
            let start_kw_pos = i;
            let mut cursor = i + kw.len();
            while cursor < n && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let name_start = cursor;
            while cursor < n
                && (bytes[cursor].is_ascii_alphanumeric()
                    || bytes[cursor] == b'_'
                    || bytes[cursor] == b'-'
                    || bytes[cursor] == b':'
                    || bytes[cursor] == b'@'
                    || bytes[cursor] == b'.')
            {
                cursor += 1;
            }
            let name = &content[name_start..cursor];
            if !name.is_empty() {
                while cursor < n
                    && bytes[cursor] != b'{'
                    && bytes[cursor] != b';'
                    && bytes[cursor] != b'\n'
                {
                    cursor += 1;
                }
                if cursor < n && bytes[cursor] == b'{' {
                    let brace_open = cursor;
                    let mut depth = 1;
                    cursor += 1;
                    while cursor < n && depth > 0 {
                        if cursor + 1 < n && bytes[cursor] == b'/' && bytes[cursor + 1] == b'/' {
                            while cursor < n && bytes[cursor] != b'\n' {
                                cursor += 1;
                            }
                            continue;
                        }
                        if bytes[cursor] == b'{' {
                            depth += 1;
                        } else if bytes[cursor] == b'}' {
                            depth -= 1;
                        }
                        cursor += 1;
                    }
                    if depth == 0 {
                        let brace_close = cursor - 1;
                        let body = &content[brace_open + 1..brace_close];
                        let start_line = base_line_offset
                            + content[..start_kw_pos]
                                .chars()
                                .filter(|&c| c == '\n')
                                .count()
                            + 1;
                        let end_line = base_line_offset
                            + content[..cursor].chars().filter(|&c| c == '\n').count()
                            + 1;
                        blocks.push(BracedBlock {
                            kind: kw,
                            name,
                            body,
                            start_line,
                            end_line,
                        });
                        i = cursor;
                        continue;
                    }
                }
            }
        }

        i += 1;
    }

    blocks
}

static RE_PACKAGE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*package\s+([a-zA-Z0-9_:-]+(?:@[a-zA-Z0-9_.-]+)?)\s*;")
        .expect("valid regex")
});

static RE_FUNC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)^\s*([a-zA-Z0-9_-]+)\s*:\s*func\s*\(([^)]*)\)(?:\s*->\s*([a-zA-Z0-9_<>, -]+))?",
    )
    .expect("valid regex")
});

static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*import\s+([a-zA-Z0-9_:/-]+(?:@[a-zA-Z0-9_.-]+)?)\s*;")
        .expect("valid regex")
});

static RE_EXPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*export\s+([a-zA-Z0-9_-]+)\s*:\s*func").expect("valid regex"));

pub struct WitParser;

impl Default for WitParser {
    fn default() -> Self {
        Self::new()
    }
}

impl WitParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for WitParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let package_name = if let Some(cap) = RE_PACKAGE.captures(content) {
            cap.get(1).map_or("", |m| m.as_str()).to_string()
        } else {
            String::new()
        };

        if !package_name.is_empty() {
            symbols.push(Symbol {
                name: package_name.clone(),
                kind: "wit_package".to_string(),
                start_line: 1,
                end_line: 1,
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

        let top_blocks = extract_braced_blocks(content, &["world", "interface"], 0);

        for block in top_blocks {
            let is_world = block.kind == "world";
            let full_name = if package_name.is_empty() {
                block.name.to_string()
            } else {
                format!("{}.{}", package_name, block.name)
            };

            let start_line = block.start_line;
            let end_line = block.end_line;

            let sym_kind = format!("wit_{}", block.kind);
            symbols.push(Symbol {
                name: full_name.clone(),
                kind: sym_kind,
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_world,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });

            if !package_name.is_empty() {
                relations.push(Relation {
                    from: package_name.clone(),
                    to: full_name.clone(),
                    rel_type: "contains".to_string(),
                    properties: HashMap::new(),
                });
            }

            if is_world {
                // Imports
                for imp in RE_IMPORT.captures_iter(block.body) {
                    if let Some(target) = imp.get(1) {
                        relations.push(Relation {
                            from: full_name.clone(),
                            to: target.as_str().to_string(),
                            rel_type: "imports_wit_interface".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }

                // Exports
                for exp in RE_EXPORT.captures_iter(block.body) {
                    if let Some(exp_name) = exp.get(1) {
                        let export_sym = format!("{}.{}", full_name, exp_name.as_str());
                        symbols.push(Symbol {
                            name: export_sym.clone(),
                            kind: "wit_export".to_string(),
                            start_line,
                            end_line,
                            docstring: None,
                            is_entry_point: true,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: full_name.clone(),
                            to: export_sym,
                            rel_type: "exports".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            } else {
                // Interface: extract records and functions
                let record_blocks = extract_braced_blocks(block.body, &["record"], start_line);
                for rec in record_blocks {
                    let full_record_name = format!("{}.{}", full_name, rec.name);
                    let mut rec_props = HashMap::new();
                    let mut has_sensitive_fields = false;

                    for rline in rec.body.lines() {
                        let trimmed = rline.trim().trim_end_matches(',').trim();
                        if trimmed.is_empty() || trimmed.starts_with("//") {
                            continue;
                        }

                        if let Some(colon_pos) = trimmed.find(':') {
                            let fname = trimmed[..colon_pos].trim();
                            let ftype = trimmed[colon_pos + 1..].trim();
                            let full_field_name = format!("{}.{}", full_record_name, fname);

                            let mut field_props = HashMap::new();
                            field_props.insert("type".to_string(), ftype.to_string());

                            if let Some(kind) = super::is_sensitive_name(fname) {
                                has_sensitive_fields = true;
                                field_props.insert("is_sensitive".to_string(), "true".to_string());
                                field_props.insert("pii_kind".to_string(), kind.to_string());
                            }

                            symbols.push(Symbol {
                                name: full_field_name.clone(),
                                kind: "wit_field".to_string(),
                                start_line: rec.start_line,
                                end_line: rec.end_line,
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
                                from: full_record_name.clone(),
                                to: full_field_name,
                                rel_type: "contains".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }

                    if has_sensitive_fields {
                        rec_props.insert("has_sensitive_fields".to_string(), "true".to_string());
                    }

                    symbols.push(Symbol {
                        name: full_record_name.clone(),
                        kind: "wit_record".to_string(),
                        start_line: rec.start_line,
                        end_line: rec.end_line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: rec_props,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: full_name.clone(),
                        to: full_record_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
                }

                for func in RE_FUNC.captures_iter(block.body) {
                    let fname = func.get(1).map_or("", |m| m.as_str());
                    let full_func_name = format!("{}.{}", full_name, fname);

                    symbols.push(Symbol {
                        name: full_func_name.clone(),
                        kind: "wit_function".to_string(),
                        start_line,
                        end_line,
                        docstring: None,
                        is_entry_point: true,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: HashMap::new(),
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: full_name.clone(),
                        to: full_func_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
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
    fn test_wit_component_model_parsing() {
        let wit = r#"
package my-org:orders@1.0.0;

interface order-types {
    record order-payload {
        id: u64,
        secret-key: string,
    }

    process: func(p: order-payload) -> result<string, string>;
}

world order-world {
    import wasi:clocks/monotonic-clock@0.2.0;
    import order-types;

    export handle-order: func(id: u64) -> string;
}
"#;

        let parser = WitParser::new();
        let res = parser.parse(wit);

        let pkg = res
            .symbols
            .iter()
            .find(|s| s.kind == "wit_package")
            .unwrap();
        assert_eq!(pkg.name, "my-org:orders@1.0.0");

        let world = res.symbols.iter().find(|s| s.kind == "wit_world").unwrap();
        assert!(world.is_entry_point);

        let exp = res
            .symbols
            .iter()
            .find(|s| s.name.contains("handle-order"))
            .unwrap();
        assert_eq!(exp.kind, "wit_export");
        assert!(exp.is_entry_point);

        let rec = res.symbols.iter().find(|s| s.kind == "wit_record").unwrap();
        assert_eq!(
            rec.properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let key_field = res
            .symbols
            .iter()
            .find(|s| s.name.ends_with("secret-key"))
            .unwrap();
        assert_eq!(
            key_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "order-types" && r.rel_type == "imports_wit_interface"));
    }
}
