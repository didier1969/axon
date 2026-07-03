use super::{ExtractionResult, Parser};
use std::io::Write;
use std::process::Command;
use tempfile::Builder;
use tracing::error;

/// Parser for llmlang (`.lll`) source. Unlike the tree-sitter parsers, llmlang
/// owns its own front-end (the `lll` compiler): identity is a content-hash and
/// purity/contracts are semantic facts only the compiler can compute. Rather
/// than re-implement a grammar here, this parser shells out to
/// `lll export-ist <file>`, which emits Axon's `ExtractionResult` JSON directly
/// (function/type Symbols + `calls` Relations, enriched with content_hash,
/// purity and contract counts). DRY bridge — see llmlang DEC-LLL-032.
///
/// The `lll` binary is resolved from `$LLL_BIN`, falling back to `lll` on PATH.
/// When the binary is absent or the module fails to load (e.g. unresolved
/// imports in a single-file slice), extraction degrades gracefully to empty —
/// same contract as the Datalog bridge on script failure.
pub struct LllParser;

impl Default for LllParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LllParser {
    pub fn new() -> Self {
        Self {}
    }

    fn binary() -> String {
        std::env::var("LLL_BIN").unwrap_or_else(|_| "lll".to_string())
    }
}

impl Parser for LllParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let empty = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };

        // `lll export-ist` takes a path (it resolves the workspace); write the
        // indexed content to a temp file whose extension the compiler accepts.
        let mut temp_file = match Builder::new().suffix(".lll").tempfile() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for llmlang parser: {}", e);
                return empty;
            }
        };
        if let Err(e) = temp_file.write_all(content.as_bytes()) {
            error!("Failed to write content to temp file for llmlang parser: {}", e);
            return empty;
        }

        let output = Command::new(Self::binary())
            .arg("export-ist")
            .arg(temp_file.path())
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let json_str = String::from_utf8_lossy(&out.stdout);
                match serde_json::from_str::<ExtractionResult>(&json_str) {
                    Ok(result) => result,
                    Err(e) => {
                        error!("llmlang export-ist emitted invalid JSON: {}", e);
                        empty
                    }
                }
            }
            Ok(out) => {
                // Non-zero exit: usually a check/load error (unresolved import in
                // a single-file slice). Degrade to empty rather than fail indexing.
                error!(
                    "lll export-ist failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                empty
            }
            Err(e) => {
                // Binary not found on this host — llmlang indexing is best-effort.
                error!("Failed to execute `lll` for llmlang parser: {}", e);
                empty
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end bridge test — only meaningful when the `lll` binary is
    /// reachable (`$LLL_BIN` or PATH). When absent, the parser must still
    /// degrade to an empty result without panicking (indexing stays robust).
    #[test]
    fn lll_parser_extracts_symbols_when_binary_present() {
        let src = "module T:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n";
        let parser = LllParser::new();
        let result = parser.parse(src);

        let binary_present = Command::new(LllParser::binary())
            .arg("--help")
            .output()
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false);

        if binary_present {
            assert!(
                result.symbols.iter().any(|s| s.name == "inc" && s.kind == "function"),
                "inc must surface as a function symbol"
            );
            assert!(
                result.symbols.iter().any(|s| {
                    s.name == "inc" && s.properties.get("purity").map(|p| p == "pure").unwrap_or(false)
                }),
                "inc must carry purity=pure"
            );
            assert!(
                result.relations.iter().any(|r| r.from == "twice" && r.to == "inc"),
                "twice→inc call edge must be captured"
            );
        } else {
            // No binary → graceful empty, no panic.
            assert!(result.symbols.is_empty() && result.relations.is_empty());
        }
    }
}
