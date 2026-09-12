use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

static RE_MODULE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*-\s*module\s*\(\s*([a-zA-Z0-9_]+)\s*\)\s*\.").expect("valid regex")
});

static RE_BEHAVIOUR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*-\s*behaviou?r\s*\(\s*([a-zA-Z0-9_]+)\s*\)\s*\.").expect("valid regex")
});

static RE_EXPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?ms)-\s*export\s*\(\s*\[(.*?)\]\s*\)\s*\.").expect("valid regex"));

static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)-\s*import\s*\(\s*([a-zA-Z0-9_]+)\s*,\s*\[(.*?)\]\s*\)\s*\.")
        .expect("valid regex")
});

static RE_INCLUDE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*-\s*include(?:_lib)?\s*\(\s*["']([^"']+)["']\s*\)\s*\."#)
        .expect("valid regex")
});

static RE_RECORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*-\s*record\s*\(\s*([a-zA-Z0-9_]+)\s*,\s*\{(.*?)\}\s*\)\s*\.")
        .expect("valid regex")
});

static RE_FUNC_DEF: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-zA-Z0-9_]+)\s*\(([^)]*)\)\s*(?:when\s+[^->]+)?\s*->")
        .expect("valid regex")
});

static RE_REMOTE_CALL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b([a-zA-Z0-9_]+):([a-zA-Z0-9_]+)\s*\(").expect("valid regex"));

pub struct ErlangParser;

impl Default for ErlangParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ErlangParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for ErlangParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let module_name = if let Some(cap) = RE_MODULE.captures(content) {
            cap.get(1).map_or("", |m| m.as_str()).to_string()
        } else {
            String::new()
        };

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        if !module_name.is_empty() {
            symbols.push(Symbol {
                name: module_name.clone(),
                kind: "erlang_module".to_string(),
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

        // Behaviours
        for cap in RE_BEHAVIOUR.captures_iter(content) {
            if let Some(beh) = cap.get(1) {
                let beh_name = beh.as_str().to_string();
                if !module_name.is_empty() {
                    relations.push(Relation {
                        from: module_name.clone(),
                        to: beh_name,
                        rel_type: "implements_behaviour".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // Includes
        for cap in RE_INCLUDE.captures_iter(content) {
            if let Some(inc) = cap.get(1) {
                let inc_path = inc.as_str().to_string();
                if !module_name.is_empty() {
                    relations.push(Relation {
                        from: module_name.clone(),
                        to: inc_path,
                        rel_type: "includes".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // Imports
        for cap in RE_IMPORT.captures_iter(content) {
            if let Some(imp_mod) = cap.get(1) {
                let mod_target = imp_mod.as_str().to_string();
                if !module_name.is_empty() {
                    relations.push(Relation {
                        from: module_name.clone(),
                        to: mod_target,
                        rel_type: "imports_module".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // Exported functions set
        let mut exported_fns: HashSet<String> = HashSet::new();
        for cap in RE_EXPORT.captures_iter(content) {
            if let Some(body) = cap.get(1) {
                for item in body.as_str().split(',') {
                    let trimmed = item.trim();
                    if let Some(slash_pos) = trimmed.find('/') {
                        let fname = trimmed[..slash_pos].trim();
                        let arity = trimmed[slash_pos + 1..].trim();
                        exported_fns.insert(format!("{}/{}", fname, arity));
                    }
                }
            }
        }

        // Records
        for cap in RE_RECORD.captures_iter(content) {
            let rname = cap.get(1).map_or("", |m| m.as_str());
            let rfields = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let full_record_name = if module_name.is_empty() {
                rname.to_string()
            } else {
                format!("{}.#{}", module_name, rname)
            };

            let mut rec_props = HashMap::new();
            let mut has_sensitive = false;

            for fitem in rfields.split(',') {
                let ftrimmed = fitem.trim();
                let fname = if let Some(eq_pos) = ftrimmed.find('=') {
                    ftrimmed[..eq_pos].trim()
                } else if let Some(col_pos) = ftrimmed.find("::") {
                    ftrimmed[..col_pos].trim()
                } else {
                    ftrimmed
                };

                if !fname.is_empty() {
                    let field_sym_name = format!("{}.{}", full_record_name, fname);
                    let mut fprops = HashMap::new();
                    if let Some(kind) = super::is_sensitive_name(fname) {
                        has_sensitive = true;
                        fprops.insert("is_sensitive".to_string(), "true".to_string());
                        fprops.insert("pii_kind".to_string(), kind.to_string());
                    }

                    symbols.push(Symbol {
                        name: field_sym_name.clone(),
                        kind: "erlang_record_field".to_string(),
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
                        from: full_record_name.clone(),
                        to: field_sym_name,
                        rel_type: "contains".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }

            if has_sensitive {
                rec_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: full_record_name.clone(),
                kind: "erlang_record".to_string(),
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

            if !module_name.is_empty() {
                relations.push(Relation {
                    from: module_name.clone(),
                    to: full_record_name,
                    rel_type: "defines_record".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // Functions
        let mut seen_functions: HashSet<String> = HashSet::new();
        for cap in RE_FUNC_DEF.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let args_raw = cap.get(2).map_or("", |m| m.as_str());
            let arity = if args_raw.trim().is_empty() {
                0
            } else {
                args_raw.split(',').count()
            };

            let fa = format!("{}/{}", fname, arity);
            if seen_functions.contains(&fa) {
                continue;
            }
            seen_functions.insert(fa.clone());

            let line = start_offset_line(cap.get(0).unwrap().start());
            let is_exported = exported_fns.contains(&fa);
            let is_otp_callback = matches!(
                fname,
                "init"
                    | "handle_call"
                    | "handle_cast"
                    | "handle_info"
                    | "terminate"
                    | "code_change"
                    | "start_link"
            );

            let is_entry = is_exported || is_otp_callback;
            let full_fn_name = if module_name.is_empty() {
                fa.clone()
            } else {
                format!("{}:{}", module_name, fa)
            };

            let mut fn_props = HashMap::new();
            fn_props.insert("arity".to_string(), arity.to_string());
            if is_exported {
                fn_props.insert("exported".to_string(), "true".to_string());
            }
            if is_otp_callback {
                fn_props.insert("otp_callback".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: full_fn_name.clone(),
                kind: "erlang_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: is_entry,
                is_public: is_exported,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: fn_props,
                embedding: None,
            });

            if !module_name.is_empty() {
                relations.push(Relation {
                    from: module_name.clone(),
                    to: full_fn_name.clone(),
                    rel_type: "defines_function".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // Remote function calls
        for cap in RE_REMOTE_CALL.captures_iter(content) {
            let target_mod = cap.get(1).map_or("", |m| m.as_str());
            let target_fn = cap.get(2).map_or("", |m| m.as_str());
            let target_symbol = format!("{}:{}", target_mod, target_fn);

            if !module_name.is_empty() && target_mod != module_name {
                relations.push(Relation {
                    from: module_name.clone(),
                    to: target_symbol,
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
    fn test_erlang_parser_basic_and_pii() {
        let code = r#"
-module(order_service).
-behaviour(gen_server).
-include("order.hrl").
-export([start_link/0, create_order/2, init/1, handle_call/3]).

-record(order_state, {
    order_id,
    user_token,
    password_hash
}).

start_link() ->
    gen_server:start_link({local, ?MODULE}, ?MODULE, [], []).

init([]) ->
    {ok, #order_state{}}.

create_order(OrderId, Amount) ->
    payment_gw:charge(OrderId, Amount),
    gen_server:call(?MODULE, {create, OrderId, Amount}).

handle_call({create, OrderId, Amount}, _From, State) ->
    {reply, ok, State}.
"#;

        let parser = ErlangParser::new();
        let res = parser.parse(code);

        let mod_sym = res
            .symbols
            .iter()
            .find(|s| s.kind == "erlang_module")
            .unwrap();
        assert_eq!(mod_sym.name, "order_service");

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "gen_server" && r.rel_type == "implements_behaviour"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "order.hrl" && r.rel_type == "includes"));

        let rec = res
            .symbols
            .iter()
            .find(|s| s.kind == "erlang_record")
            .unwrap();
        assert_eq!(
            rec.properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let tok_field = res
            .symbols
            .iter()
            .find(|s| s.name.contains("user_token"))
            .unwrap();
        assert_eq!(
            tok_field.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let fns: Vec<_> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "erlang_function")
            .collect();
        assert!(fns.len() >= 4);

        let start_link = fns
            .iter()
            .find(|s| s.name.contains("start_link/0"))
            .unwrap();
        assert!(start_link.is_entry_point);

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "payment_gw:charge" && r.rel_type == "calls"));
    }
}
