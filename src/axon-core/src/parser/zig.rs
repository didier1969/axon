use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

#[derive(Debug)]
#[allow(dead_code)]
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
        // Skip comments
        if i + 1 < n && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
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
            let (name, next_cursor) = if cursor < n && bytes[cursor] == b'"' {
                let quote_start = cursor + 1;
                cursor += 1;
                while cursor < n && bytes[cursor] != b'"' && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
                let str_name = &content[quote_start..cursor];
                if cursor < n && bytes[cursor] == b'"' {
                    cursor += 1;
                }
                (str_name, cursor)
            } else {
                let name_start = cursor;
                while cursor < n && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
                {
                    cursor += 1;
                }
                (&content[name_start..cursor], cursor)
            };
            cursor = next_cursor;

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

        i += 1;
    }

    blocks
}

static RE_FN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:(pub|export)\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(([^)]*)\)")
        .expect("valid regex")
});

static RE_STRUCT_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*(?:pub\s+)?const\s+([a-zA-Z0-9_]+)\s*=\s*(?:extern\s+|packed\s+)?struct\s*\{([^}]*)\}")
        .expect("valid regex")
});

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"@import\s*\(\s*["']([^"']+)["']\s*\)"#).expect("valid regex"));

static RE_C_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"@cImport\s*\("#).expect("valid regex"));

static RE_CALL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([a-zA-Z0-9_]+)\.([a-zA-Z0-9_]+)\s*\(").expect("valid regex"));

pub struct ZigParser;

impl Default for ZigParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ZigParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for ZigParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // Imports
        for cap in RE_IMPORT.captures_iter(content) {
            if let Some(target) = cap.get(1) {
                relations.push(Relation {
                    from: "root".to_string(),
                    to: target.as_str().to_string(),
                    rel_type: "imports_zig".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        if RE_C_IMPORT.is_match(content) {
            relations.push(Relation {
                from: "root".to_string(),
                to: "c_headers".to_string(),
                rel_type: "calls_c_extern".to_string(),
                properties: HashMap::new(),
            });
        }

        // Structs with fields
        for cap in RE_STRUCT_DEF.captures_iter(content) {
            let sname = cap.get(1).map_or("", |m| m.as_str());
            let sbody = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut struct_props = HashMap::new();
            let mut has_sensitive = false;

            for fline in sbody.lines() {
                let trimmed = fline.trim().trim_end_matches(',').trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }

                if let Some(col_pos) = trimmed.find(':') {
                    let fname = trimmed[..col_pos].trim();
                    let ftype = trimmed[col_pos + 1..].trim();
                    let full_field_name = format!("{}.{}", sname, fname);

                    let mut fprops = HashMap::new();
                    fprops.insert("type".to_string(), ftype.to_string());

                    if let Some(kind) = super::is_sensitive_name(fname) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "zig_field".to_string(),
                        start_line: line,
                        end_line: line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: fprops,
                        embedding: None,
                    });

                    relations.push(Relation {
                        from: sname.to_string(),
                        to: full_field_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            if has_sensitive {
                struct_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: sname.to_string(),
                kind: "zig_struct".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: struct_props,
                embedding: None,
            });
        }

        // Functions
        for cap in RE_FN.captures_iter(content) {
            let vis = cap.get(1).map_or("", |m| m.as_str());
            let fname = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_public = vis == "pub" || vis == "export";
            let is_export = vis == "export";
            let is_main = fname == "main";
            let is_entry = is_main || is_export;

            let mut fn_props = HashMap::new();
            if is_export {
                fn_props.insert("is_nif".to_string(), "true".to_string());
                fn_props.insert("ffi_export".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "zig_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_entry,
                is_public,
                tested: false,
                is_nif: is_export,
                is_unsafe: false,
                properties: fn_props,
                embedding: None,
            });
        }

        // Tests
        let test_blocks = extract_braced_blocks(content, &["test"], 0);
        for tb in test_blocks {
            symbols.push(Symbol {
                name: tb.name.to_string(),
                kind: "zig_test".to_string(),
                start_line: tb.start_line,
                end_line: tb.end_line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: true,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // Method / Namespace calls
        for cap in RE_CALL.captures_iter(content) {
            let target_mod = cap.get(1).map_or("", |m| m.as_str());
            let target_fn = cap.get(2).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "root".to_string(),
                to: format!("{}.{}", target_mod, target_fn),
                rel_type: "calls".to_string(),
                properties: HashMap::new(),
            });
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
    fn test_zig_parser_basic_and_pii() {
        let code = r#"
const std = @import("std");

pub const AuthPayload = struct {
    id: u64,
    secret_token: []const u8,
    password_hash: []const u8,
};

pub fn main() !void {
    std.debug.print("Hello from Zig!\n", .{});
}

export fn process_order(id: u64) c_int {
    return 0;
}

test "payload validation" {
    const p = AuthPayload{ .id = 1, .secret_token = "abc", .password_hash = "xyz" };
    try std.testing.expect(p.id == 1);
}
"#;

        let parser = ZigParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "std" && r.rel_type == "imports_zig"));

        let st = res.symbols.iter().find(|s| s.kind == "zig_struct").unwrap();
        assert_eq!(st.name, "AuthPayload");
        assert_eq!(
            st.properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "AuthPayload.secret_token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let exp_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "process_order")
            .unwrap();
        assert!(exp_fn.is_entry_point);
        assert!(exp_fn.is_nif);

        let test_sym = res.symbols.iter().find(|s| s.kind == "zig_test").unwrap();
        assert_eq!(test_sym.name, "payload validation");
        assert!(test_sym.tested);
    }
}
