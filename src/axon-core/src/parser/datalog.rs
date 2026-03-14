use super::{ExtractionResult, Parser, Symbol, Relation};
use std::process::Command;
use std::io::Write;
use tempfile::NamedTempFile;
use log::error;

pub struct DatalogParser;

impl Default for DatalogParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DatalogParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parser for DatalogParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut temp_file = match NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for Datalog parser: {}", e);
                return ExtractionResult { symbols, relations };
            }
        };

        if let Err(e) = temp_file.write_all(content.as_bytes()) {
            error!("Failed to write content to temp file for Datalog parser: {}", e);
            return ExtractionResult { symbols, relations };
        }

        let current_dir = std::env::current_dir().unwrap_or_default();
        let script_path = if current_dir.ends_with("src/axon-core") {
            current_dir.join("src/parser/python_bridge/datalog_parser.py")
        } else {
            current_dir.join("src/axon-core/src/parser/python_bridge/datalog_parser.py")
        };

        let output = Command::new("python3")
            .arg(script_path)
            .arg(temp_file.path())
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let json_str = String::from_utf8_lossy(&out.stdout);
                if let Ok(result) = serde_json::from_str::<ExtractionResult>(&json_str) {
                    symbols = result.symbols;
                    relations = result.relations;
                }
            }
            Ok(out) => {
                error!("Datalog python parser script failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Err(e) => {
                error!("Failed to execute python Datalog parser: {}", e);
            }
        }

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_datalog() {
        let code = r#"
        .decl parent(x: symbol, y: symbol)
        .decl ancestor(x: symbol, y: symbol)
        
        ancestor(x, y) :- parent(x, y).
        ancestor(x, y) :- parent(x, z), ancestor(z, y).
        "#;
        
        let parser = DatalogParser::new();
        let result = parser.parse(code);
        
        // Assert symbols
        assert!(result.symbols.iter().any(|s| s.name == "parent" && s.kind == "datalog_relation"));
        assert!(result.symbols.iter().any(|s| s.name == "ancestor" && s.kind == "datalog_relation"));
        
        // Assert relations
        assert!(result.relations.iter().any(|r| r.from == "ancestor" && r.to == "parent" && r.rel_type == "depends_on"));
    }
}