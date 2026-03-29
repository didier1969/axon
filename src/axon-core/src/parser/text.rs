use super::{ExtractionResult, Parser, Symbol};
use std::collections::HashMap;

pub struct TextParser;

impl TextParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for TextParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult::default();
        let lines: Vec<&str> = content.lines().collect();
        
        result.symbols.push(Symbol {
            name: "document_body".to_string(),
            kind: "markdown_content".to_string(),
            start_line: 1,
            end_line: lines.len().max(1),
            docstring: Some(content.to_string()), 
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: HashMap::new(),
            embedding: None,
        });

        result
    }
}
