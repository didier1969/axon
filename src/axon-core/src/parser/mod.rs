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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct Relation {
    pub from: String,
    pub to: String,
    pub rel_type: String,
    #[serde(default)]
    pub properties: std::collections::HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ExtractionResult {
    #[serde(default)]
    pub project_slug: Option<String>,
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
pub mod markdown;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod sql;
pub mod text;
pub mod typeql;
pub mod typescript;
pub mod yaml;

pub fn get_parser_for_file(path: &Path) -> Option<Box<dyn Parser>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "py" => Some(Box::new(python::PythonParser::new())),
        "ex" | "exs" => Some(Box::new(elixir::ElixirParser::new())),
        "rs" => Some(Box::new(rust::RustParser::new())),
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
        // NEXUS v7.5: Fallback to TextParser for Knowledge capturing
        "txt" | "conf" | "ini" => Some(Box::new(text::TextParser::new())),
        _ => None,
    }
}
