use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_NS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?ms)\(\s*ns\s+([a-zA-Z0-9_.-]+)(.*?)\)"#).expect("valid regex"));

static RE_REQUIRE_ITEM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\s*([a-zA-Z0-9_.-]+)").expect("valid regex"));

static RE_DEFN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)\(\s*defn(-)?\s+([a-zA-Z0-9_.*!+<>=?/-]+)").expect("valid regex")
});

static RE_DEFMACRO: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)\(\s*defmacro\s+([a-zA-Z0-9_.*!+<>=?/-]+)").expect("valid regex")
});

static RE_DEFRECORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)\(\s*defrecord\s+([A-Za-z0-9_.-]+)\s*\[([^\]]*)\]").expect("valid regex")
});

static RE_DEFPROTOCOL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)\(\s*defprotocol\s+([A-Za-z0-9_.-]+)").expect("valid regex"));

static RE_DEFMULTI: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)\(\s*defmulti\s+([a-zA-Z0-9_.*!+<>=?/-]+)").expect("valid regex")
});

static RE_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)\(\s*def\s+(?:\^:[a-zA-Z0-9_-]+\s+)?([a-zA-Z0-9_.*!+<>=?/-]+)")
        .expect("valid regex")
});

static RE_DEFTEST: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)\(\s*deftest\s+([a-zA-Z0-9_.*!+<>=?/-]+)").expect("valid regex"));

static RE_QUALIFIED_CALL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\(\s*([a-zA-Z0-9_.-]+)/([a-zA-Z0-9_.*!+<>=?/-]+)").expect("valid regex")
});

pub struct ClojureParser;

impl Default for ClojureParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ClojureParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for ClojureParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Namespace & Requires
        let mut current_ns = String::new();
        if let Some(cap) = RE_NS.captures(content) {
            let ns_name = cap.get(1).map_or("", |m| m.as_str());
            let ns_body = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            current_ns = ns_name.to_string();

            symbols.push(Symbol {
                name: ns_name.to_string(),
                kind: "clojure_namespace".to_string(),
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

            // Extract :require dependencies
            if let Some(req_pos) = ns_body.find(":require") {
                let req_body = &ns_body[req_pos..];
                for req_cap in RE_REQUIRE_ITEM.captures_iter(req_body) {
                    let dep = req_cap.get(1).map_or("", |m| m.as_str());
                    if dep != ":as" && dep != ":refer" {
                        relations.push(Relation {
                            from: ns_name.to_string(),
                            to: dep.to_string(),
                            rel_type: "imports_clojure".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
        }

        // 2. Records with PII detection
        for cap in RE_DEFRECORD.captures_iter(content) {
            let rname = cap.get(1).map_or("", |m| m.as_str());
            let fields_str = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut rec_props = HashMap::new();
            let mut has_sensitive = false;

            for ftoken in fields_str.split_whitespace() {
                let fname = ftoken.trim_matches(':').trim();
                if !fname.is_empty() {
                    let full_field_name = format!("{}.{}", rname, fname);
                    let mut fprops = HashMap::new();

                    if let Some(kind) = super::is_sensitive_name(fname) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: full_field_name.clone(),
                        kind: "clojure_field".to_string(),
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
                        from: rname.to_string(),
                        to: full_field_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            if has_sensitive {
                rec_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: rname.to_string(),
                kind: "clojure_record".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: rec_props,
                embedding: None,
            });
        }

        // 3. Functions (defn / defn-)
        for cap in RE_DEFN.captures_iter(content) {
            let is_private = cap.get(1).is_some();
            let fname = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let is_entry = fname == "-main";
            let is_unsafe = fname.contains("unsafe");

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "clojure_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_entry,
                is_public: !is_private,
                tested: false,
                is_nif: false,
                is_unsafe,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 4. Macros
        for cap in RE_DEFMACRO.captures_iter(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: "clojure_macro".to_string(),
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

        // 5. Protocols
        for cap in RE_DEFPROTOCOL.captures_iter(content) {
            let pname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: pname.to_string(),
                kind: "clojure_protocol".to_string(),
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

        // 6. Multimethods
        for cap in RE_DEFMULTI.captures_iter(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: "clojure_multimethod".to_string(),
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

        // 7. Vars (def)
        for cap in RE_DEF.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if vname != "record"
                && vname != "protocol"
                && vname != "multi"
                && vname != "method"
                && vname != "test"
            {
                if !symbols.iter().any(|s| s.name == vname) {
                    symbols.push(Symbol {
                        name: vname.to_string(),
                        kind: "clojure_var".to_string(),
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
        }

        // 8. Tests
        for cap in RE_DEFTEST.captures_iter(content) {
            let tname = cap.get(1).map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "clojure_test".to_string(),
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

        // 9. Qualified calls
        for cap in RE_QUALIFIED_CALL.captures_iter(content) {
            let target_ns = cap.get(1).map_or("", |m| m.as_str());
            let target_fn = cap.get(2).map_or("", |m| m.as_str());

            if target_ns != "clojure.test" && target_ns != "super" {
                relations.push(Relation {
                    from: if current_ns.is_empty() {
                        "root".to_string()
                    } else {
                        current_ns.clone()
                    },
                    to: format!("{}/{}", target_ns, target_fn),
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
    fn test_clojure_parser_basic_and_pii() {
        let code = r#"
(ns nexus.service.auth
  (:require [clojure.string :as str]
            [ring.util.response :as resp]))

(defrecord UserAccount [user-id username password-hash session-token])

(defprotocol IAuthenticator
  (verify-token [this token]))

(defmulti handle-auth-event :event-type)

(def ^:dynamic *auth-timeout* 5000)

(defn- hash-password [raw]
  (str "hashed-" raw))

(defn authenticate-user [user token]
  (str/trim token))

(defmacro with-secure-context [& body]
  `(do ~@body))

(defn -main [& args]
  (println "Auth service active"))

(deftest test-user-authentication
  (is (= 1 1)))
"#;

        let parser = ClojureParser::new();
        let res = parser.parse(code);

        let ns_sym = res
            .symbols
            .iter()
            .find(|s| s.kind == "clojure_namespace")
            .unwrap();
        assert_eq!(ns_sym.name, "nexus.service.auth");

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "clojure.string" && r.rel_type == "imports_clojure"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "ring.util.response" && r.rel_type == "imports_clojure"));

        let rec_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount")
            .unwrap();
        assert_eq!(
            rec_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserAccount.password-hash")
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
            .find(|s| s.name == "UserAccount.session-token")
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let proto_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "IAuthenticator")
            .unwrap();
        assert_eq!(proto_sym.kind, "clojure_protocol");

        let multi_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "handle-auth-event")
            .unwrap();
        assert_eq!(multi_sym.kind, "clojure_multimethod");

        let priv_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "hash-password")
            .unwrap();
        assert!(!priv_fn.is_public);

        let pub_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "authenticate-user")
            .unwrap();
        assert!(pub_fn.is_public);

        let macro_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "with-secure-context")
            .unwrap();
        assert_eq!(macro_sym.kind, "clojure_macro");

        let main_fn = res.symbols.iter().find(|s| s.name == "-main").unwrap();
        assert!(main_fn.is_entry_point);

        let test_sym = res
            .symbols
            .iter()
            .find(|s| s.kind == "clojure_test")
            .unwrap();
        assert_eq!(test_sym.name, "test-user-authentication");
        assert!(test_sym.tested);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "str/trim" && r.rel_type == "calls"));
    }
}
