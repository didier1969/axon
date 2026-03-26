use super::{ExtractionResult, Parser};
use std::process::Command;
use std::io::Write;
use tempfile::NamedTempFile;
use tracing::error;

pub struct TypeQLParser;

impl Default for TypeQLParser {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeQLParser {
    pub fn new() -> Self {
        Self {}
    }
}

impl Parser for TypeQLParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let mut temp_file = match NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for TypeQL parser: {}", e);
                return ExtractionResult { symbols, relations };
            }
        };

        if let Err(e) = temp_file.write_all(content.as_bytes()) {
            error!("Failed to write content to temp file for TypeQL parser: {}", e);
            return ExtractionResult { symbols, relations };
        }

        let current_dir = std::env::current_dir().unwrap_or_default();
        let script_path = if current_dir.ends_with("src/axon-core") {
            current_dir.join("src/parser/python_bridge/typeql_parser.py")
        } else {
            current_dir.join("src/axon-core/src/parser/python_bridge/typeql_parser.py")
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
                error!("TypeQL python parser script failed: {}", String::from_utf8_lossy(&out.stderr));
            }
            Err(e) => {
                error!("Failed to execute python TypeQL parser: {}", e);
            }
        }

        ExtractionResult { symbols, relations }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_typeql_ontology() {
        let code = r#"
        define
        person sub entity,
            owns name,
            plays parentship:parent,
            plays parentship:child;
            
        parentship sub relation,
            relates parent,
            relates child;
            
        rule-people-are-parents:
        rule when {
            (parent: $p, child: $c) isa parentship;
        } then {
            $p has name "Parent";
        };
        "#;
        
        let parser = TypeQLParser::new();
        let result = parser.parse(code);
        
        assert!(result.symbols.iter().any(|s| s.name == "person" && s.kind == "entity_type"));
        assert!(result.symbols.iter().any(|s| s.name == "parentship" && s.kind == "relation_type"));
        assert!(result.symbols.iter().any(|s| s.name == "rule-people-are-parents" && s.kind == "rule"));
        assert!(result.symbols.iter().any(|s| s.name == "name" && s.kind == "attribute"));
        
        assert!(result.relations.iter().any(|r| r.from == "person" && r.to == "name" && r.rel_type == "owns"));
    }
}