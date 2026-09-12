use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_OPEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*open!?\s+([A-Za-z0-9_.]+)").expect("valid regex"));

static RE_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*include\s+([A-Za-z0-9_.]+)").expect("valid regex"));

static RE_MODULE_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*module\s+(?:type\s+)?([A-Za-z0-9_]+)\s*(?:=|:)").expect("valid regex")
});

static RE_TYPE_RECORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*type\s+([a-zA-Z0-9_]+)[^=]*=\s*\{(.*?)\}").expect("valid regex")
});

static RE_TYPE_DEF: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*type\s+([a-zA-Z0-9_]+)").expect("valid regex"));

static RE_LET_BINDING: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*let\s+(?:rec\s+)?([a-zA-Z0-9_]+)\b[^=\n]*=").expect("valid regex")
});

static RE_LET_ENTRY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*let\s+(?:\(\s*\)|_)\s*=").expect("valid regex"));

static RE_VAL_SIG: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*val\s+([a-zA-Z0-9_]+)\s*:\s*([^\n]+)").expect("valid regex"));

static RE_EXTERNAL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*external\s+([a-zA-Z0-9_]+)\s*:\s*[^=]+=\s*["']([^"']+)["']"#)
        .expect("valid regex")
});

static RE_TEST: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:let%test|let%expect_test|test_case)\s+["']([^"']+)["']"#)
        .expect("valid regex")
});

static RE_QUALIFIED_CALL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b([A-Z][a-zA-Z0-9_]*)\.([a-z][a-zA-Z0-9_]*)\s*\(").expect("valid regex")
});

pub struct OCamlParser;

impl Default for OCamlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OCamlParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for OCamlParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Open directives
        for cap in RE_OPEN.captures_iter(content) {
            let mod_name = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "root".to_string(),
                to: mod_name.to_string(),
                rel_type: "imports_ocaml".to_string(),
                properties: HashMap::new(),
            });
        }

        // 2. Include directives
        for cap in RE_INCLUDE.captures_iter(content) {
            let mod_name = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "root".to_string(),
                to: mod_name.to_string(),
                rel_type: "includes_ocaml".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Module definitions
        for cap in RE_MODULE_DEF.captures_iter(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: "ocaml_module".to_string(),
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

        // 4. Record types with PII detection
        for cap in RE_TYPE_RECORD.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let body = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut type_props = HashMap::new();
            let mut has_sensitive = false;

            for part in body.split(';') {
                let trimmed = part.trim();
                if trimmed.is_empty() || trimmed.starts_with("(*") {
                    continue;
                }
                // mutable field : type or field : type
                let decl = trimmed.strip_prefix("mutable").unwrap_or(trimmed).trim();
                if let Some(col) = decl.find(':') {
                    let fname = decl[..col].trim();
                    let ftype = decl[col + 1..].trim();

                    if !fname.is_empty() {
                        let full_field_name = format!("{}.{}", tname, fname);
                        let mut fprops = HashMap::new();
                        fprops.insert("type".to_string(), ftype.to_string());

                        if let Some(kind) = super::is_sensitive_name(fname) {
                            has_sensitive = true;
                            fprops.insert("is_sensitive".to_string(), "true".to_string());
                            fprops.insert("pii_kind".to_string(), kind.to_string());
                        }

                        symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "ocaml_field".to_string(),
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
            }

            if has_sensitive {
                type_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "ocaml_type".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: type_props,
                embedding: None,
            });
        }

        // 5. General types (if not already extracted as record)
        for cap in RE_TYPE_DEF.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols
                .iter()
                .any(|s| s.name == tname && s.kind == "ocaml_type")
            {
                symbols.push(Symbol {
                    name: tname.to_string(),
                    kind: "ocaml_type".to_string(),
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
        }

        // 6. External C-FFI
        for cap in RE_EXTERNAL.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let c_symbol = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("c_symbol".to_string(), c_symbol.to_string());

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "ocaml_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: true,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });

            relations.push(Relation {
                from: fname.to_string(),
                to: c_symbol.to_string(),
                rel_type: "calls_c_extern".to_string(),
                properties: HashMap::new(),
            });
        }

        // 7. Let entry points (let () = ... or let _ = ...)
        for cap in RE_LET_ENTRY.captures_iter(content) {
            let line = start_offset_line(cap.get(0).unwrap().start());
            symbols.push(Symbol {
                name: format!("entry_point_L{}", line),
                kind: "ocaml_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: true,
                is_public: false,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 8. Let bindings (functions and values)
        for cap in RE_LET_BINDING.captures_iter(content) {
            let bname = cap.get(1).map_or("", |m| m.as_str()).trim();
            let line = start_offset_line(cap.get(0).unwrap().start());
            let is_entry = bname == "main";
            let is_unsafe = bname.starts_with("unsafe") || content.contains("Obj.magic");

            if !symbols
                .iter()
                .any(|s| s.name == bname && s.kind == "ocaml_function")
            {
                symbols.push(Symbol {
                    name: bname.to_string(),
                    kind: "ocaml_function".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_entry,
                    is_public: !bname.starts_with('_'),
                    tested: false,
                    is_nif: false,
                    is_unsafe,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        // 8. Val signatures (.mli)
        for cap in RE_VAL_SIG.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let sig = cap.get(2).map_or("", |m| m.as_str()).trim();
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols.iter().any(|s| s.name == vname) {
                let mut props = HashMap::new();
                props.insert("signature".to_string(), sig.to_string());

                symbols.push(Symbol {
                    name: vname.to_string(),
                    kind: "ocaml_val".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: vname == "main",
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: sig.contains("unsafe"),
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 9. Tests
        for cap in RE_TEST.captures_iter(content) {
            let tname = cap.get(1).map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "ocaml_test".to_string(),
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

        // 10. Qualified calls
        for cap in RE_QUALIFIED_CALL.captures_iter(content) {
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
    fn test_ocaml_parser_basic_and_pii() {
        let code = r#"
open Stdlib
open! Core

module SessionManager = struct
    type token = string
end

type account = {
    id : int;
    email : string;
    password_hash : string;
    mutable session_token : string;
}

type status = Active | Suspended

external c_sha256 : string -> string = "caml_sha256"

let verify_user acc pass =
    let h = c_sha256 pass in
    h = acc.password_hash

let unsafe_ptr_cast x =
    Obj.magic x

let () =
    print_endline "OCaml daemon ready"

let%test "account verification works" =
    true
"#;

        let parser = OCamlParser::new();
        let res = parser.parse(code);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "Stdlib" && r.rel_type == "imports_ocaml"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "Core" && r.rel_type == "imports_ocaml"));

        let mod_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "SessionManager")
            .unwrap();
        assert_eq!(mod_sym.kind, "ocaml_module");

        let acc_sym = res.symbols.iter().find(|s| s.name == "account").unwrap();
        assert_eq!(
            acc_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name == "account.session_token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "account.password_hash")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let ext_fn = res.symbols.iter().find(|s| s.name == "c_sha256").unwrap();
        assert!(ext_fn.is_nif);
        assert!(res.relations.iter().any(|r| r.from == "c_sha256"
            && r.to == "caml_sha256"
            && r.rel_type == "calls_c_extern"));

        let entry = res.symbols.iter().find(|s| s.is_entry_point).unwrap();
        assert!(entry.name.starts_with("entry_point_L"));

        let test_sym = res.symbols.iter().find(|s| s.kind == "ocaml_test").unwrap();
        assert_eq!(test_sym.name, "account verification works");
        assert!(test_sym.tested);
    }
}
