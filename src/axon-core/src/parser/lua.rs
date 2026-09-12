use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_REQUIRE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"\brequire\s*(?:\(\s*["']([^"']+)["']\s*\)|["']([^"']+)["'])"#)
        .expect("valid regex")
});

static RE_LOCAL_FN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*local\s+function\s+([a-zA-Z0-9_]+)\s*\(([^)]*)\)").expect("valid regex")
});

static RE_GLOBAL_FN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*function\s+([a-zA-Z0-9_.:]+)\s*\(([^)]*)\)").expect("valid regex")
});

static RE_TABLE_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*(?:local\s+)?([a-zA-Z0-9_.]+)\s*=\s*\{(.*?)\}").expect("valid regex")
});

static RE_TEST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)\b(?:describe|it|test)\s*\(\s*["']([^"']+)["']"#).expect("valid regex")
});

static RE_CALL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([a-zA-Z0-9_]+)[:.]([a-zA-Z0-9_]+)\s*\(").expect("valid regex"));

static RE_C_FFI: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bffi\.(?:cdef|C|load)\b").expect("valid regex"));

pub struct LuaParser;

impl Default for LuaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for LuaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Require / Modules
        for cap in RE_REQUIRE.captures_iter(content) {
            let mod_name = cap.get(1).or_else(|| cap.get(2)).map_or("", |m| m.as_str());
            if !mod_name.is_empty() {
                relations.push(Relation {
                    from: "root".to_string(),
                    to: mod_name.to_string(),
                    rel_type: "imports_lua".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // 2. Tables with PII detection
        for cap in RE_TABLE_DEF.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let tbody = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut table_props = HashMap::new();
            let mut has_sensitive = false;

            for fline in tbody.lines() {
                let trimmed = fline
                    .trim()
                    .trim_end_matches(',')
                    .trim_end_matches(';')
                    .trim();
                if trimmed.is_empty() || trimmed.starts_with("--") {
                    continue;
                }

                let (fname, fval) = if let Some(eq) = trimmed.find('=') {
                    (trimmed[..eq].trim(), trimmed[eq + 1..].trim())
                } else if let Some(col) = trimmed.find(':') {
                    (trimmed[..col].trim(), trimmed[col + 1..].trim())
                } else {
                    ("", "")
                };

                let clean_name = fname
                    .trim_matches('[')
                    .trim_matches(']')
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim();

                if !clean_name.is_empty()
                    && clean_name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let full_field_name = format!("{}.{}", tname, clean_name);
                    let mut fprops = HashMap::new();
                    if !fval.is_empty() {
                        fprops.insert("value".to_string(), fval.to_string());
                    }

                    if let Some(kind) = super::is_sensitive_name(clean_name) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "lua_field".to_string(),
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
                        from: tname.to_string(),
                        to: full_field_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            if has_sensitive {
                table_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "lua_table".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: table_props,
                embedding: None,
            });
        }

        // 3. Local Functions
        for cap in RE_LOCAL_FN.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "lua_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: fname.starts_with("unsafe"),
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 4. Global / Member Functions
        for cap in RE_GLOBAL_FN.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            let is_main = fname == "main";

            if !symbols
                .iter()
                .any(|s| s.name == fname && s.kind == "lua_function")
            {
                symbols.push(Symbol {
                    name: fname.to_string(),
                    kind: "lua_function".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_main,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: fname.contains("unsafe"),
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        // 5. C FFI
        if RE_C_FFI.is_match(content) {
            relations.push(Relation {
                from: "root".to_string(),
                to: "luajit_ffi".to_string(),
                rel_type: "calls_c_extern".to_string(),
                properties: HashMap::new(),
            });
        }

        // 6. Tests (Busted / LuaUnit)
        for cap in RE_TEST.captures_iter(content) {
            let tname = cap.get(1).map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "lua_test".to_string(),
                start_line: line,
                end_line: line,
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

        // 7. Method / Table calls
        for cap in RE_CALL.captures_iter(content) {
            let target_mod = cap.get(1).map_or("", |m| m.as_str());
            let target_fn = cap.get(2).map_or("", |m| m.as_str());

            if target_mod != "describe" && target_mod != "it" && target_mod != "assert" {
                relations.push(Relation {
                    from: "root".to_string(),
                    to: format!("{}.{}", target_mod, target_fn),
                    rel_type: "calls".to_string(),
                    properties: HashMap::new(),
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
    fn test_lua_parser_basic_and_pii() {
        let code = r#"
local cjson = require("cjson")
local socket = require "socket"
local ffi = require("ffi")

ffi.cdef[[
    int getpid(void);
]]

local UserConfig = {
    user_id = 101,
    password_hash = "sha256_hash",
    session_token = "sess_xyz",
    is_active = true,
}

local function internal_helper(x)
    return x * 2
end

function M.process_data(data)
    local encoded = cjson.encode(data)
    return encoded
end

function main()
    print("Lua daemon initialized")
end

describe("UserConfig authorization", function()
    it("validates session token", function()
        assert.is_true(true)
    end)
end)
"#;

        let parser = LuaParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "cjson" && r.rel_type == "imports_lua"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "socket" && r.rel_type == "imports_lua"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "luajit_ffi" && r.rel_type == "calls_c_extern"));

        let table_sym = res.symbols.iter().find(|s| s.name == "UserConfig").unwrap();
        assert_eq!(
            table_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserConfig.password_hash")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserConfig.session_token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let priv_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "internal_helper")
            .unwrap();
        assert!(!priv_fn.is_public);

        let pub_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "M.process_data")
            .unwrap();
        assert!(pub_fn.is_public);

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let test_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "UserConfig authorization")
            .unwrap();
        assert!(test_sym.tested);

        let it_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "validates session token")
            .unwrap();
        assert!(it_sym.tested);
    }
}
