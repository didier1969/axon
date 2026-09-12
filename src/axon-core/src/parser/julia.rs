use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_MODULE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*(?:bare)?module\s+([a-zA-Z0-9_!]+)").expect("valid regex"));

static RE_USING: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*using\s+([^#\n]+)").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*import\s+([^#\n]+)").expect("valid regex"));

static RE_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)\binclude\s*\(\s*["']([^"']+)["']\s*\)"#).expect("valid regex"));

static RE_STRUCT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*(?:mutable\s+)?struct\s+([a-zA-Z0-9_!]+)(?:\s*<:\s*([a-zA-Z0-9_!.]+))?\s*\n(.*?)\n\s*end\b")
        .expect("valid regex")
});

static RE_ABSTRACT_TYPE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*abstract\s+type\s+([a-zA-Z0-9_!]+)(?:\s*<:\s*([a-zA-Z0-9_!.]+))?\s+end\b")
        .expect("valid regex")
});

static RE_FUNCTION_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*function\s+([a-zA-Z0-9_!]+)\s*\(([^)]*)\)").expect("valid regex")
});

static RE_COMPACT_FN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-zA-Z0-9_!]+)\s*\(([^)]*)\)\s*=\s*[^=\n]+").expect("valid regex")
});

static RE_MACRO_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*macro\s+([a-zA-Z0-9_!]+)\s*\(([^)]*)\)").expect("valid regex")
});

static RE_CCALL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:\bccall\s*\(\s*\(\s*:(?:[a-zA-Z0-9_]+)\s*,\s*["']|@ccall\b)"#)
        .expect("valid regex")
});

static RE_TESTSET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*@testset\s+(?:["']([^"']+)["']|([a-zA-Z0-9_]+))"#).expect("valid regex")
});

static RE_CALL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([a-zA-Z0-9_]+)\.([a-zA-Z0-9_!]+)\s*\(").expect("valid regex"));

pub struct JuliaParser;

impl Default for JuliaParser {
    fn default() -> Self {
        Self::new()
    }
}

impl JuliaParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for JuliaParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Modules
        let mut current_module = String::new();
        for cap in RE_MODULE.captures_iter(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            if current_module.is_empty() {
                current_module = mname.to_string();
            }

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: "julia_module".to_string(),
                start_line: line,
                end_line: line,
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

        // 2. Using / Imports
        for cap in RE_USING.captures_iter(content) {
            let raw_imports = cap.get(1).map_or("", |m| m.as_str());
            for part in raw_imports.split(',') {
                let pkg_part = part.split(':').next().unwrap_or("").trim();
                let pkg_name = pkg_part.split('.').next().unwrap_or("").trim();
                if !pkg_name.is_empty() {
                    relations.push(Relation {
                        from: if current_module.is_empty() {
                            "root".to_string()
                        } else {
                            current_module.clone()
                        },
                        to: pkg_name.to_string(),
                        rel_type: "imports_julia".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        for cap in RE_IMPORT.captures_iter(content) {
            let raw_imports = cap.get(1).map_or("", |m| m.as_str());
            for part in raw_imports.split(',') {
                let pkg_part = part.split(':').next().unwrap_or("").trim();
                let pkg_name = pkg_part.split('.').next().unwrap_or("").trim();
                if !pkg_name.is_empty() {
                    relations.push(Relation {
                        from: if current_module.is_empty() {
                            "root".to_string()
                        } else {
                            current_module.clone()
                        },
                        to: pkg_name.to_string(),
                        rel_type: "imports_julia".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // 3. Includes
        for cap in RE_INCLUDE.captures_iter(content) {
            let target_path = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: if current_module.is_empty() {
                    "root".to_string()
                } else {
                    current_module.clone()
                },
                to: target_path.to_string(),
                rel_type: "includes".to_string(),
                properties: HashMap::new(),
            });
        }

        // 4. Abstract types
        for cap in RE_ABSTRACT_TYPE.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let supertype = cap.get(2).map(|m| m.as_str().trim());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(st) = supertype {
                props.insert("supertype".to_string(), st.to_string());
                relations.push(Relation {
                    from: tname.to_string(),
                    to: st.to_string(),
                    rel_type: "subtypes".to_string(),
                    properties: HashMap::new(),
                });
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "julia_abstract_type".to_string(),
                start_line: line,
                end_line: line,
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

        // 5. Structs & Fields with PII detection
        for cap in RE_STRUCT.captures_iter(content) {
            let sname = cap.get(1).map_or("", |m| m.as_str());
            let supertype = cap.get(2).map(|m| m.as_str().trim());
            let body = cap.get(3).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut struct_props = HashMap::new();
            if let Some(st) = supertype {
                struct_props.insert("supertype".to_string(), st.to_string());
                relations.push(Relation {
                    from: sname.to_string(),
                    to: st.to_string(),
                    rel_type: "subtypes".to_string(),
                    properties: HashMap::new(),
                });
            }

            let mut has_sensitive = false;
            for fline in body.lines() {
                let trimmed = fline.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("function")
                {
                    continue;
                }
                // Field can be `name::Type` or simply `name`
                let fname = if let Some(col) = trimmed.find("::") {
                    trimmed[..col].trim()
                } else {
                    trimmed.split_whitespace().next().unwrap_or("")
                };

                if !fname.is_empty() && fname.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let full_field_name = format!("{}.{}", sname, fname);
                    let mut fprops = HashMap::new();
                    if let Some(col) = trimmed.find("::") {
                        fprops.insert("type".to_string(), trimmed[col + 2..].trim().to_string());
                    }

                    if let Some(kind) = super::is_sensitive_name(fname) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "julia_field".to_string(),
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
                kind: "julia_struct".to_string(),
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

        // 6. Functions (standard defs)
        for cap in RE_FUNCTION_DEF.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            let is_main = fname == "main" || fname == "julia_main";
            let is_unsafe = fname.starts_with("unsafe_");

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "julia_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_main,
                is_public: !fname.starts_with('_'),
                tested: false,
                is_nif: false,
                is_unsafe,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 7. Compact functions: `fname(x) = ...` (ignore if already added or keywords like if/while)
        for cap in RE_COMPACT_FN.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            if fname == "if" || fname == "while" || fname == "for" || fname == "return" {
                continue;
            }
            let line = start_offset_line(cap.get(0).unwrap().start());
            if !symbols
                .iter()
                .any(|s| s.name == fname && s.kind == "julia_function")
            {
                symbols.push(Symbol {
                    name: fname.to_string(),
                    kind: "julia_function".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: fname == "main" || fname == "julia_main",
                    is_public: !fname.starts_with('_'),
                    tested: false,
                    is_nif: false,
                    is_unsafe: fname.starts_with("unsafe_"),
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        // 8. Macros
        for cap in RE_MACRO_DEF.captures_iter(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: format!("@{}", mname),
                kind: "julia_macro".to_string(),
                start_line: line,
                end_line: line,
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

        // 9. C-FFI detection
        if RE_CCALL.is_match(content) {
            relations.push(Relation {
                from: if current_module.is_empty() {
                    "root".to_string()
                } else {
                    current_module.clone()
                },
                to: "c_runtime".to_string(),
                rel_type: "calls_c_extern".to_string(),
                properties: HashMap::new(),
            });
        }

        // 10. Test sets
        for cap in RE_TESTSET.captures_iter(content) {
            let tname = cap
                .get(1)
                .or_else(|| cap.get(2))
                .map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "julia_test".to_string(),
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

        // 11. Qualified calls
        for cap in RE_CALL.captures_iter(content) {
            let mod_prefix = cap.get(1).map_or("", |m| m.as_str());
            let fn_call = cap.get(2).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: if current_module.is_empty() {
                    "root".to_string()
                } else {
                    current_module.clone()
                },
                to: format!("{}.{}", mod_prefix, fn_call),
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
    fn test_julia_parser_basic_and_pii() {
        let code = r#"
module AuthManager

using SHA, HTTP: Request, Response
include("utils.jl")

abstract type AbstractUser end

mutable struct UserAccount <: AbstractUser
    id::Int64
    username::String
    secret_token::String
    password_hash::String
end

function authenticate(user::UserAccount, token::String)
    @ccall libc.verify(token::Cstring)::Cint
end

fast_hash(x::String) = bytes2hex(sha256(x))

macro safe_audit(expr)
    quote
        $expr
    end
end

function unsafe_ptr_load(addr::UInt64)
    # raw unsafe memory access
end

function main()
    println("Starting AuthManager")
end

@testset "UserAccount validation" begin
    # test assertions
end

end
"#;

        let parser = JuliaParser::new();
        let res = parser.parse(code);

        assert!(res
            .symbols
            .iter()
            .any(|s| s.name == "AuthManager" && s.kind == "julia_module"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "SHA" && r.rel_type == "imports_julia"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "HTTP" && r.rel_type == "imports_julia"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "utils.jl" && r.rel_type == "includes"));

        let st = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount")
            .unwrap();
        assert_eq!(
            st.properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(
            st.properties.get("supertype").map(|s| s.as_str()),
            Some("AbstractUser")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount.secret_token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount.password_hash")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let fn_auth = res
            .symbols
            .iter()
            .find(|s| s.name == "authenticate")
            .unwrap();
        assert_eq!(fn_auth.kind, "julia_function");

        let fn_compact = res.symbols.iter().find(|s| s.name == "fast_hash").unwrap();
        assert_eq!(fn_compact.kind, "julia_function");

        let macro_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "@safe_audit")
            .unwrap();
        assert_eq!(macro_sym.kind, "julia_macro");

        let unsafe_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "unsafe_ptr_load")
            .unwrap();
        assert!(unsafe_fn.is_unsafe);

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let tset = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount validation")
            .unwrap();
        assert!(tset.tested);

        assert!(res.relations.iter().any(|r| r.rel_type == "calls_c_extern"));
    }
}
