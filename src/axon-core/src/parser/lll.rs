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

    /// REQ-AXO-902259 — drop symbols that this file merely IMPORTS.
    ///
    /// `lll export-ist` RESOLVES AND FLATTENS imports, so running it on a consumer emits
    /// the library's symbols too. AXO attributes every symbol of an extraction to the file
    /// being parsed, so a library function was materialised once PER CONSUMER, each copy
    /// attributed to the consumer: `query ledger_total` returned 3 hits for 1 symbol,
    /// `inspect` named the consumer as the definition site, and a project's symbol count
    /// grew with its import fan-out rather than its content. Reported by the LLL project
    /// (mailbox 3646) and confirmed on real data.
    ///
    /// `properties.source_file` (llmlang commit f9eafcb, specified with LLL in mailbox
    /// 3648) carries the file where the symbol is REALLY defined. A symbol whose
    /// `source_file` differs from the parsed path is an import: skip it here, because it is
    /// materialised — once, correctly attributed — when its OWN file is parsed. The
    /// name-based `calls` relations are untouched, so `report -> ledger_total` still
    /// resolves from the consumer.
    ///
    /// Deliberately conservative:
    /// * no `source_file` (older `lll`, or a symbol llmlang cannot attribute) → KEEP. A
    ///   missing field must never silently delete symbols; the pre-fix duplication is a
    ///   lesser evil than data loss.
    /// * paths compared canonicalised, since llmlang emits absolute canonical paths while
    ///   the indexer may pass a differently-spelled path to the same file.
    ///
    /// Known trade-off (flagged by LLL): if the defining file is OUTSIDE the watch scope,
    /// its symbols are now absent instead of duplicated into every in-scope consumer. That
    /// is the correct semantic — Axon does not index what it does not watch — but it IS a
    /// recall change, not purely a de-duplication.
    fn drop_imported_symbols(mut result: ExtractionResult, parsed: &Path) -> ExtractionResult {
        let own = std::fs::canonicalize(parsed).unwrap_or_else(|_| parsed.to_path_buf());
        result.symbols.retain(|s| match s.properties.get("source_file") {
            Some(src) if !src.trim().is_empty() => {
                let src_path = Path::new(src);
                let src_canon =
                    std::fs::canonicalize(src_path).unwrap_or_else(|_| src_path.to_path_buf());
                src_canon == own
            }
            // No attribution available → keep (never lose a symbol to a missing field).
            _ => true,
        });
        result
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
                    // REQ-AXO-902259 — strip the flattened imports before they reach the
                    // indexer, which would attribute them to the consumer.
                    Ok(result) => Self::drop_imported_symbols(result, path),
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

    /// Minimal `Symbol` fixture — `Symbol` has no `Default`, and spelling every field in
    /// each test would bury the one property under test.
    fn sym(name: &str, source_file: Option<&str>) -> crate::parser::Symbol {
        crate::parser::Symbol {
            name: name.into(),
            kind: "function".into(),
            start_line: 1,
            end_line: 1,
            docstring: None,
            is_entry_point: false,
            is_public: true,
            tested: false,
            is_nif: false,
            is_unsafe: false,
            properties: source_file
                .map(|f| [("source_file".to_string(), f.to_string())].into_iter().collect())
                .unwrap_or_default(),
            embedding: None,
        }
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
        // REQ-AXO-902259 — but `inc` belongs to lib.lll and must NOT be attributed to
        // main.lll. Before the fix, every consumer carried its own copy of every imported
        // symbol: `query inc` returned one hit per importer, and `inspect` pointed at the
        // importer as the definition site. `inc` is still materialised — once — when
        // lib.lll is itself indexed, and the twice→inc call edge below is unaffected
        // because relations are name-based.
        //
        // Guarded on the field being present, so an older `lll` on the host degrades this
        // to a no-op assertion instead of a spurious failure.
        let emits_source_file = result
            .symbols
            .iter()
            .any(|s| s.properties.contains_key("source_file"));
        if emits_source_file {
            assert!(
                !result.symbols.iter().any(|s| s.name == "inc"),
                "inc is defined in lib.lll — it must not be attributed to main.lll (REQ-AXO-902259)"
            );
        }
    }

    /// REQ-AXO-902259 — the conservative half of the contract, and the one that matters
    /// most: a MISSING `source_file` must never delete a symbol. An older `lll` on the
    /// host, or a symbol llmlang cannot attribute, would otherwise silently empty a file's
    /// extraction — data loss dressed up as de-duplication.
    #[test]
    fn missing_source_file_keeps_every_symbol() {
        let dir = Builder::new().prefix("lll-keep-").tempdir().expect("tempdir");
        let f = dir.path().join("a.lll");
        std::fs::write(&f, "x").unwrap();
        let mut result = ExtractionResult {
            project_code: None,
            symbols: vec![sym("no_attribution", None), sym("blank_attribution", Some("   "))],
            relations: Vec::new(),
        };
        result = LllParser::drop_imported_symbols(result, &f);
        assert_eq!(result.symbols.len(), 2, "no/blank attribution must never drop a symbol");
    }

    /// A symbol attributed to ANOTHER file is dropped even when neither path exists on
    /// disk — `canonicalize` fails there, and the fallback must still compare correctly
    /// rather than silently keep the duplicate.
    #[test]
    fn foreign_source_file_is_dropped_even_when_paths_do_not_exist() {
        let own = Path::new("/nonexistent/consumer.lll");
        let result = ExtractionResult {
            project_code: None,
            symbols: vec![
                sym("mine", Some(&own.display().to_string())),
                sym("imported", Some("/nonexistent/lib.lll")),
            ],
            relations: Vec::new(),
        };
        let out = LllParser::drop_imported_symbols(result, own);
        let names: Vec<&str> = out.symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["mine"]);
    }
}
