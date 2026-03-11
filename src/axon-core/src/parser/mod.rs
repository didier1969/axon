use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub docstring: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub symbols: Vec<Symbol>,
}

pub trait Parser {
    fn parse(&self, content: &str) -> ExtractionResult;
}

pub mod python;
