use serde::{Deserialize, Serialize};
use std::path::Path;

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
    pub properties: std::collections::HashMap<String, String>,
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

pub fn get_parser_for_file(path: &Path) -> Option<Box<dyn Parser>> {
    match path.extension()?.to_str()? {
        "py" => Some(Box::new(python::PythonParser::new())),
        "ex" | "exs" => Some(Box::new(elixir::ElixirParser::new())),
        "rs" => Some(Box::new(rust::RustParser::new())),
        "ts" | "tsx" => Some(Box::new(typescript::TypeScriptParser::new())),
        "js" | "jsx" => Some(Box::new(typescript::TypeScriptParser::new())), // TS parser handles JS
        "go" => Some(Box::new(go::GoParser::new())),
        "java" => Some(Box::new(java::JavaParser::new())),
        _ => None,
    }
}
