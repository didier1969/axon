use super::{ExtractionResult, Parser, Symbol};
use std::collections::HashMap;

pub struct FallbackParser;

impl FallbackParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for FallbackParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut result = ExtractionResult::default();
        let lines: Vec<&str> = content.lines().collect();
        
        // On crée un symbole global représentant tout le fichier
        result.symbols.push(Symbol {
            name: "raw_content".to_string(),
            kind: "content".to_string(),
            start_line: 1,
            end_line: lines.len().max(1),
            docstring: Some(content.to_string()), // On stocke tout le texte ici
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
