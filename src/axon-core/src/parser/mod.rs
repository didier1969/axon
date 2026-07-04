use crate::indexing_policy::EcosystemId;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::catch_unwind;
use std::path::Path;
use tree_sitter::wasmtime::Engine;

use tracing::{debug, warn};

pub static WASM_ENGINE: Lazy<Engine> = Lazy::new(Engine::default);

thread_local! {
    static PARSER_CACHE: RefCell<HashMap<String, tree_sitter::Parser>> = RefCell::new(HashMap::new());
}

pub fn parse_with_wasm_safe(
    language_name: &str,
    wasm_bytes: &[u8],
    content: &str,
) -> Option<tree_sitter::Tree> {
    let content_string = content.to_string();
    let lang_name_str = language_name.to_string();
    let wasm_bytes_vec = wasm_bytes.to_vec();

    debug!("[WASM] Starting parse for {}", lang_name_str);

    let result = catch_unwind(move || {
        let engine = &*WASM_ENGINE;
        PARSER_CACHE.with(|cache_cell| {
            let mut cache = cache_cell.borrow_mut();

            if !cache.contains_key(&lang_name_str) {
                if let Ok(mut store) = tree_sitter::WasmStore::new(engine) {
                    if let Ok(language) = store.load_language(&lang_name_str, &wasm_bytes_vec) {
                        let mut parser = tree_sitter::Parser::new();
                        if parser.set_wasm_store(store).is_ok()
                            && parser.set_language(&language).is_ok()
                        {
                            cache.insert(lang_name_str.clone(), parser);
                        }
                    }
                }
            }

            if let Some(parser) = cache.get_mut(&lang_name_str) {
                let tree = parser.parse(&content_string, None);
                parser.reset();
                tree
            } else {
                None
            }
        })
    });

    match result {
        Ok(Some(tree)) => Some(tree),
        Ok(None) => {
            warn!(
                "WASM parsing failed to produce a tree for {}",
                language_name
            );
            None
        }
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };
            warn!("WASM parsing Trap/Panic for {}: {}", language_name, msg);
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub docstring: Option<String>,
    #[serde(default)]
    pub is_entry_point: bool,
    #[serde(default)]
    pub is_public: bool,
    #[serde(default)]
    pub tested: bool,
    #[serde(default)]
    pub is_nif: bool,
    #[serde(default)]
    pub is_unsafe: bool,
    #[serde(default)]
    pub properties: std::collections::HashMap<String, String>,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub rel_type: String,
    #[serde(default)]
    pub properties: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtractionResult {
    #[serde(default)]
    pub project_code: Option<String>,
    pub symbols: Vec<Symbol>,
    pub relations: Vec<Relation>,
}

pub trait Parser: Send + Sync {
    fn parse(&self, content: &str) -> ExtractionResult;
}

pub fn scan_secrets(content: &str, result: &mut ExtractionResult) {
    use regex::Regex;

    let patterns = [
        (
            "SECRET_API_KEY",
            r#"(?i)(?:key|api|token|secret|password|passwd|auth)[\s:='\"\[\{]+[a-z0-9\/+]{32,45}"#,
        ),
        ("SECRET_AWS_KEY", r#"AKIA[0-9A-Z]{16}"#),
        (
            "SECRET_PRIVATE_KEY",
            r#"-----BEGIN [A-Z ]+ PRIVATE KEY-----"#,
        ),
        ("SECRET_DB_URL", r#"[a-zA-Z]+://[^:]+:[^@]+@[^/]+/[^?]+"#),
    ];

    for (kind, pattern) in patterns {
        if let Ok(re) = Regex::new(pattern) {
            for mat in re.find_iter(content) {
                let line = content[..mat.start()].lines().count() + 1;
                result.symbols.push(Symbol {
                    name: format!("{}: Found potential hardcoded credential", kind),
                    kind: kind.to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: false,
                    tested: false,
                    is_nif: false,
                    is_unsafe: true,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }
    }
}

pub mod phantom;

pub mod c;
pub mod c_sharp;
pub mod cpp;
pub mod css;
pub mod datalog;
pub mod elixir;
pub mod go;
pub mod html;
pub mod java;
pub mod kotlin;
pub mod lll;
pub mod markdown;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod scheme;
pub mod sql;
pub mod text;
pub mod typeql;
pub mod typescript;
pub mod yaml;

const SUPPORTED_PARSER_ECOSYSTEMS: &[EcosystemId] = &[
    EcosystemId::JavaScript,
    EcosystemId::TypeScript,
    EcosystemId::Python,
    EcosystemId::Elixir,
    EcosystemId::Erlang,
    EcosystemId::Rust,
    EcosystemId::Go,
    EcosystemId::Jvm,
    EcosystemId::C,
    EcosystemId::Cpp,
    EcosystemId::CSharp,
    EcosystemId::Ruby,
    EcosystemId::Php,
    EcosystemId::WebAssets,
    EcosystemId::DataLogic,
];

pub fn supported_parser_ecosystems() -> &'static [EcosystemId] {
    SUPPORTED_PARSER_ECOSYSTEMS
}

pub fn get_parser_for_file(path: &Path) -> Option<Box<dyn Parser>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "py" => Some(Box::new(python::PythonParser::new())),
        "ex" | "exs" => Some(Box::new(elixir::ElixirParser::new())),
        "rs" => Some(Box::new(rust::RustParser::new())),
        "scm" | "ss" | "sld" | "sls" => Some(Box::new(scheme::SchemeParser::new())),
        "ts" | "tsx" => Some(Box::new(typescript::TypeScriptParser::new())),
        "js" | "jsx" => Some(Box::new(typescript::TypeScriptParser::new())),
        "go" => Some(Box::new(go::GoParser::new())),
        "java" => Some(Box::new(java::JavaParser::new())),
        "c" | "h" => Some(Box::new(c::CParser::new())),
        "cpp" | "hpp" | "cc" | "cxx" | "hxx" => Some(Box::new(cpp::CppParser::new())),
        "cs" => Some(Box::new(c_sharp::CSharpParser::new())),
        "rb" | "ruby" => Some(Box::new(ruby::RubyParser::new())),
        "kt" | "kts" => Some(Box::new(kotlin::KotlinParser::new())),
        "php" => Some(Box::new(php::PhpParser::new())),
        "yaml" | "yml" => Some(Box::new(yaml::YamlParser::new())),
        "html" | "htm" => Some(Box::new(html::HtmlParser::new())),
        "css" | "scss" => Some(Box::new(css::CssParser::new())),
        "md" | "markdown" => Some(Box::new(markdown::MarkdownParser::new())),
        "sql" => Some(Box::new(sql::SqlParser::new())),
        "tql" | "typeql" => Some(Box::new(typeql::TypeQLParser::new())),
        "dl" | "datalog" => Some(Box::new(datalog::DatalogParser::new())),
        // llmlang: construct WITH the real path so `lll export-ist` resolves the
        // file's `import`s against the actual workspace (REQ-LLL-021).
        "lll" => Some(Box::new(lll::LllParser::with_path(path.to_path_buf()))),
        // NEXUS v7.5: Fallback to TextParser for Knowledge capturing
        "txt" | "conf" | "ini" => Some(Box::new(text::TextParser::new())),
        _ => None,
    }
}

#[cfg(test)]
mod wasm_grammar_health_tests {
    use super::*;

    /// REQ-AXO-901886 — every shipped WASM grammar must load + parse a
    /// trivial valid snippet into at least one symbol. A grammar whose
    /// `.wasm` is ABI-incompatible with the tree-sitter runtime fails
    /// `WasmStore::load_language` silently (parse_with_wasm_safe → None →
    /// zero symbols), leaving that language effectively unindexed (and,
    /// pre-REQ-AXO-901885, feeding the A2 re-parse loop).
    ///
    /// `KNOWN_BROKEN` pins the grammars currently shipping broken `.wasm`
    /// (diagnosed 2026-06-06: c / cpp / c-sharp / php yield zero symbols on
    /// valid input while ruby/kotlin from the same commit work — see
    /// REQ-AXO-901886). The test enforces TWO directions so it stays green
    /// today yet self-cleans on fix:
    ///   1. no healthy grammar may regress to zero symbols;
    ///   2. a KNOWN_BROKEN grammar that starts producing symbols must be
    ///      removed from the list (the assert fires to remind us).
    // REQ-AXO-901886 fixed 2026-06-06: c/cpp/c-sharp/php .wasm regenerated at
    // ABI 14 (tree-sitter v0.23.x grammars; c-sharp forced via
    // `tree-sitter generate --abi 14`) so they load in the 0.23.2 runtime.
    // Empty = every shipped grammar must extract ≥1 symbol.
    const KNOWN_BROKEN: &[&str] = &[];

    // REQ-AXO-902190 — the "safe" contract of the 17-caller wasm chokepoint: garbage bytes
    // must yield None, NEVER panic (catch_unwind). Calls parse_with_wasm_safe directly so the
    // covered flag flips (the grammar-health test reaches it only through wrappers).
    #[test]
    fn parse_with_wasm_safe_returns_none_on_garbage_wasm_without_panic() {
        let tree = parse_with_wasm_safe("bogus_lang", b"\x00\x01\x02 not wasm", "let x = 1;");
        assert!(tree.is_none());
    }

    #[test]
    fn shipped_wasm_grammars_extract_symbols_except_known_broken() {
        // (file extension, minimal valid snippet expected to yield ≥1 symbol)
        let cases: &[(&str, &str)] = &[
            ("demo.rs", "fn main() {}\n"),
            ("demo.py", "def main():\n    pass\n"),
            ("demo.ts", "function main(): void {}\n"),
            ("demo.js", "function main() {}\n"),
            ("demo.go", "package m\nfunc Main() {}\n"),
            ("demo.java", "class A { void m() {} }\n"),
            ("demo.ex", "defmodule A do\n  def f, do: 1\nend\n"),
            ("demo.rb", "def foo\n  1\nend\n"),
            ("demo.kt", "fun main() {}\n"),
            ("demo.c", "int main(void) { return 0; }\n"),
            ("demo.cpp", "int main() { return 0; }\n"),
            ("demo.cs", "class A { void M() {} }\n"),
            ("demo.php", "<?php\nfunction foo() { return 1; }\n"),
            ("demo.scm", "(define (foo x) x)\n"),
        ];

        let mut regressed: Vec<&str> = Vec::new();
        let mut unexpectedly_fixed: Vec<&str> = Vec::new();
        for (file, snippet) in cases {
            let parser = get_parser_for_file(Path::new(file))
                .unwrap_or_else(|| panic!("no parser registered for {file}"));
            let n = parser.parse(snippet).symbols.len();
            let known_broken = KNOWN_BROKEN.contains(file);
            match (known_broken, n) {
                (false, 0) => regressed.push(file),
                (true, n) if n > 0 => unexpectedly_fixed.push(file),
                _ => {}
            }
        }

        assert!(
            regressed.is_empty(),
            "healthy WASM grammar(s) regressed to zero symbols (broken .wasm? \
             see REQ-AXO-901886): {regressed:?}"
        );
        assert!(
            unexpectedly_fixed.is_empty(),
            "KNOWN_BROKEN grammar(s) now extract symbols — remove them from \
             KNOWN_BROKEN (REQ-AXO-901886 fixed for): {unexpectedly_fixed:?}"
        );
    }
}

#[cfg(test)]
mod chained_call_class_regression {
    use super::*;
    use std::path::Path;

    /// REQ-AXO-902200 (class-level regression for the REQ-AXO-902195 family) —
    /// a function call whose result is immediately method-chained
    /// (`g(...).h()`) must still yield a CALLS edge to the INNER call `g`, not
    /// only the outer method. The Rust parser lost it (walk_for_calls dropped
    /// the receiver, 902195); Go lost it the same way (skip_first=true, 902200);
    /// ruby/kotlin/cpp/c_sharp/php/c re-walk the whole node and were already
    /// correct. This test locks the whole CLASS across every live grammar so a
    /// future receiver-skip optimisation can't silently re-open it (which would
    /// under-count callers → wrong `covered`/`wiring`). Positive-control-derived
    /// snippets (each verified to yield the inner `g`).
    #[test]
    fn chained_call_captures_inner_call_all_langs() {
        // (file ext, snippet, inner-call target that MUST appear)
        let cases: &[(&str, &str, &str)] = &[
            ("demo.rs", "fn f() { let _ = g(1).unwrap(); }\n", "g"),
            ("demo.go", "package m\nfunc F() { g(1).H() }\n", "g"),
            ("demo.rb", "def f\n  g(1).h\nend\n", "g"),
            ("demo.kt", "fun f() { g(1).h() }\n", "g"),
            ("demo.cpp", "void f() { g(1).h(); }\n", "g"),
            ("demo.cs", "class A { void M() { g(1).H(); } }\n", "g"),
            ("demo.php", "<?php\nfunction f() { g(1)->h(); }\n", "g"),
            // C has no method chaining; the analogous inner-call case is a call
            // nested in an argument (`g(h())`), which the same walk must reach.
            ("demo.c", "int f(void) { return g(h()); }\n", "h"),
        ];
        for (file, snippet, inner) in cases {
            let parser = get_parser_for_file(Path::new(file)).unwrap();
            let r = parser.parse(snippet);
            if r.symbols.is_empty() {
                // Grammar wasm unavailable in this build — skip, the dedicated
                // grammar-health test owns that failure mode.
                eprintln!("{file}: grammar unavailable, skipping");
                continue;
            }
            let targets: Vec<&str> = r
                .relations
                .iter()
                .filter(|rel| rel.rel_type == "calls")
                .map(|rel| rel.to.as_str())
                .collect();
            assert!(
                targets.contains(inner),
                "{file}: inner call `{inner}` must be captured (REQ-AXO-902200 \
                 class regression), got {targets:?}"
            );
        }
    }
}
