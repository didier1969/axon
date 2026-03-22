use serde::{Deserialize, Serialize};
use std::path::Path;
use tree_sitter::wasmtime::Engine;
use once_cell::sync::Lazy;
use std::panic::catch_unwind;
use std::collections::HashMap;
use std::cell::RefCell;

pub static WASM_ENGINE: Lazy<Engine> = Lazy::new(|| Engine::default());

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

    let result = catch_unwind(move || {
        PARSER_CACHE.with(|cache_cell| {
            let mut cache = cache_cell.borrow_mut();

            if !cache.contains_key(&lang_name_str) {
                if let Ok(mut store) = tree_sitter::WasmStore::new(&*WASM_ENGINE) {
                    if let Ok(language) = store.load_language(&lang_name_str, &wasm_bytes_vec) {
                        let mut parser = tree_sitter::Parser::new();
                        if parser.set_wasm_store(store).is_ok() && parser.set_language(&language).is_ok() {
                            cache.insert(lang_name_str.clone(), parser);
                        }
                    }
                }
            }

            if let Some(parser) = cache.get_mut(&lang_name_str) {
                // Parse returns Option<Tree>
                parser.parse(&content_string, None)
            } else {
                None
            }
        })
    });

    match result {
        Ok(Some(tree)) => Some(tree),
        Ok(None) => {
            log::warn!("WASM parsing failed to produce a tree for {}", language_name);
            None
        },
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };
            log::warn!("WASM parsing Trap/Panic for {}: {}", language_name, msg);
            None
        }
    }
}

fn default_is_public() -> bool { true }

#[derive(Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub docstring: Option<String>,
    #[serde(default)]
    pub is_entry_point: bool,
    #[serde(default = "default_is_public")]
    pub is_public: bool,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub symbols: Vec<Symbol>,
    pub relations: Vec<Relation>,
}

pub trait Parser: Send + Sync {
    fn parse(&self, content: &str) -> ExtractionResult;
}

pub mod python;
pub mod elixir;
pub mod rust;
pub mod typescript;
pub mod go;
pub mod java;
pub mod yaml;
pub mod html;
pub mod css;
pub mod markdown;
pub mod sql;
pub mod typeql;
pub mod datalog;

pub fn get_parser_for_file(path: &Path) -> Option<Box<dyn Parser>> {
    match path.extension()?.to_str()? {
        "py" => Some(Box::new(python::PythonParser::new())),
        "ex" | "exs" => Some(Box::new(elixir::ElixirParser::new())),
        "rs" => Some(Box::new(rust::RustParser::new())),
        "ts" | "tsx" => Some(Box::new(typescript::TypeScriptParser::new())),
        "js" | "jsx" => Some(Box::new(typescript::TypeScriptParser::new())), // TS parser handles JS
        "go" => Some(Box::new(go::GoParser::new())),
        "java" => Some(Box::new(java::JavaParser::new())),
        "yaml" | "yml" => Some(Box::new(yaml::YamlParser::new())),
        "html" | "htm" => Some(Box::new(html::HtmlParser::new())),
        "css" | "scss" => Some(Box::new(css::CssParser::new())),
        "md" | "markdown" => Some(Box::new(markdown::MarkdownParser::new())),
        "sql" => Some(Box::new(sql::SqlParser::new())),
        "tql" | "typeql" => Some(Box::new(typeql::TypeQLParser::new())),
        "dl" | "datalog" => Some(Box::new(datalog::DatalogParser::new())),
        _ => None,
    }
}
