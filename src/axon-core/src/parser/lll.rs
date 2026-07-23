use super::{ExtractionResult, Parser};
use std::io::Write;
use std::path::{Path, PathBuf};
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
/// When constructed with the on-disk path (`with_path`, the indexing path), the
/// parser runs `lll export-ist` on that file directly so its `import`s resolve
/// against the real workspace. Without a path (`new`), it falls back to a temp
/// file holding the passed content — correct for import-free single files.
///
/// The `lll` binary is resolved from `$LLL_BIN`, falling back to `lll` on PATH.
/// Missing binary or a load error (e.g. an unresolved import when only content
/// is available) degrades gracefully to an empty result — same contract as the
/// Datalog bridge on script failure, so indexing never fails on a `.lll` file.
pub struct LllParser {
    /// the on-disk path of the file being indexed, when known (lets `import`s
    /// resolve against the real workspace instead of a temp directory).
    path: Option<PathBuf>,
}

impl Default for LllParser {
    fn default() -> Self {
        Self::new()
    }
}

impl LllParser {
    pub fn new() -> Self {
        Self { path: None }
    }

    /// Construct with the file's real on-disk path so `lll export-ist` resolves
    /// the file's workspace (`import`s) correctly.
    pub fn with_path(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn binary() -> String {
        std::env::var("LLL_BIN").unwrap_or_else(|_| "lll".to_string())
    }

    /// Run `lll export-ist <path>` and deserialize its `ExtractionResult` JSON.
    /// Any failure (missing binary, load error, invalid JSON) → empty result.
    fn run(&self, path: &Path) -> ExtractionResult {
        let empty = ExtractionResult {
            project_code: None,
            symbols: Vec::new(),
            relations: Vec::new(),
        };
        let output = Command::new(Self::binary())
            .arg("export-ist")
            .arg(path)
            .output();
        match output {
            Ok(out) if out.status.success() => {
                match serde_json::from_str::<ExtractionResult>(&String::from_utf8_lossy(&out.stdout))
                {
                    Ok(result) => result,
                    Err(e) => {
                        error!("llmlang export-ist emitted invalid JSON: {}", e);
                        empty
                    }
                }
            }
            Ok(out) => {
                // Non-zero exit: usually a check/load error (e.g. an unresolved
                // import). Degrade to empty rather than fail indexing.
                error!("lll export-ist failed: {}", String::from_utf8_lossy(&out.stderr));
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

impl Parser for LllParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        // Preferred path: run on the real file so its `import`s resolve against
        // the actual workspace. The indexer reads the same file from disk, so
        // content == on-disk content in the batch/indexing flow.
        if let Some(p) = &self.path {
            if p.exists() {
                return self.run(p);
            }
        }
        // Fallback: no known path — write content to a temp `.lll` and run on it.
        // Correct for import-free files; a file with `import`s cannot resolve them
        // from a temp directory and degrades to empty.
        let mut temp_file = match Builder::new().suffix(".lll").tempfile() {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for llmlang parser: {}", e);
                return ExtractionResult::default();
            }
        };
        if let Err(e) = temp_file.write_all(content.as_bytes()) {
            error!("Failed to write content to temp file for llmlang parser: {}", e);
            return ExtractionResult::default();
        }
        self.run(temp_file.path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_present() -> bool {
        Command::new(LllParser::binary())
            .arg("--help")
            .output()
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false)
    }

    /// End-to-end bridge test — only meaningful when the `lll` binary is
    /// reachable (`$LLL_BIN` or PATH). When absent, the parser must still
    /// degrade to an empty result without panicking (indexing stays robust).
    #[test]
    fn lll_parser_extracts_symbols_when_binary_present() {
        let src = "module T:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n";
        let result = LllParser::new().parse(src);
        if binary_present() {
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
            // llmlang REQ-LLL-208 (DEC-LLL-081 tranche 1a): export-ist now carries the contract
            // PREDICATE TEXT (not just counts), so the generic `properties` map surfaces a REQ's
            // acceptance-criteria — the intention↔contract leg of the active loop. `inc`'s single
            // ensures renders `result == x + 1`; the string flows straight into `properties`.
            assert!(
                result.symbols.iter().any(|s| {
                    s.name == "inc"
                        && s.properties
                            .get("ensures")
                            .map(|e| e.contains("result") && e.contains("=="))
                            .unwrap_or(false)
                }),
                "inc must carry its ensures predicate TEXT (intention↔contract bridge)"
            );
            assert!(
                result.relations.iter().any(|r| r.from == "twice" && r.to == "inc"),
                "twice→inc call edge must be captured"
            );
        } else {
            assert!(result.symbols.is_empty() && result.relations.is_empty());
        }
    }

    /// With the real on-disk path, a file that `import`s another resolves the
    /// import against the workspace (temp-file mode could not) — the imported
    /// symbol is still extracted for the indexed file.
    #[test]
    fn lll_parser_resolves_imports_with_path() {
        if !binary_present() {
            return;
        }
        let dir = Builder::new().prefix("lll-idx-").tempdir().expect("tempdir");
        let lib = dir.path().join("lib.lll");
        let main = dir.path().join("main.lll");
        std::fs::write(&lib, "module Lib:\n\n  part inc(x: Int) -> Int:\n    yield x + 1\n").unwrap();
        std::fs::write(
            &main,
            "import \"lib.lll\"\n\nmodule Main:\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n",
        )
        .unwrap();
        let content = std::fs::read_to_string(&main).unwrap();
        let result = LllParser::with_path(main.clone()).parse(&content);
        // main.lll's own part is extracted, and the cross-file call resolves
        // (the workspace loaded, so `twice` type-checks and hashes).
        assert!(
            result.symbols.iter().any(|s| s.name == "twice" && s.kind == "function"),
            "twice must be extracted with imports resolved"
        );
    }
}
