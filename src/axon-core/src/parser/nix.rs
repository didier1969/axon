use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static IMPORT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)(?:^|\s)import\s+(?:<([^>]+)>|([a-zA-Z0-9_\.\/\-]+))"#).unwrap()
});
static IMPORTS_LIST_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)imports\s*=\s*\[(.*?)\]"#).unwrap());
static IMPORT_ITEM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"([a-zA-Z0-9_\.\/\-]+|<[^>]+>)"#).unwrap());

static FLAKE_INPUT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*inputs\.([a-zA-Z0-9_\-]+)\.url\s*=\s*["']([^"']+)["']"#).unwrap()
});
static FLAKE_INPUT_BLOCK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)inputs\s*=\s*\{(.*?)\};"#).unwrap());
static INPUT_ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"([a-zA-Z0-9_\-]+)\s*(?:\.url)?\s*=\s*(?:\{[^}]*url\s*=\s*)?["']([^"']+)["']"#)
        .unwrap()
});

static DERIVATION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)([a-zA-Z0-9_\.\-]+)\s*=\s*(?:[a-zA-Z0-9_\.]+\.)?(mkDerivation|buildRustPackage|buildPythonPackage|buildGoModule|writeShellScriptBin)\b"#).unwrap()
});
static BARE_DERIVATION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)(?:^|\s)(?:[a-zA-Z0-9_\.]+\.)?(mkDerivation|buildRustPackage|buildPythonPackage|buildGoModule)\s*(?:rec\s*)?\{"#).unwrap()
});

static MKSHELL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)([a-zA-Z0-9_\.\-]+)\s*=\s*(?:[a-zA-Z0-9_\.]+\.)?mkShell\b"#).unwrap()
});
static BARE_MKSHELL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)(?:^|\s)(?:[a-zA-Z0-9_\.]+\.)?mkShell\s*(?:rec\s*)?\{"#).unwrap()
});

static LET_BINDING_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*([a-zA-Z0-9_\-]+)\s*=\s*(?:[^;]+);"#).unwrap());

static BUILD_INPUTS_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)(?:buildInputs|nativeBuildInputs|propagatedBuildInputs|packages)\s*=\s*(?:with\s+([a-zA-Z0-9_\.]+);\s*)?\[(.*?)\]"#).unwrap()
});

static PKG_ITEM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"([a-zA-Z0-9_\.\-]+)"#).unwrap());

static PNAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*pname\s*=\s*["']([^"']+)["']"#).unwrap());
static NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*name\s*=\s*["']([^"']+)["']"#).unwrap());
static VERSION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*version\s*=\s*["']([^"']+)["']"#).unwrap());

pub struct NixParser;

impl Default for NixParser {
    fn default() -> Self {
        Self::new()
    }
}

impl NixParser {
    pub fn new() -> Self {
        Self
    }

    fn line_number(content: &str, byte_offset: usize) -> usize {
        content[..byte_offset.min(content.len())]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1
    }

    fn find_matching_brace(content: &str, open_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut depth = 0;
        let mut in_str = false;
        let mut quote_char = b'"';
        let mut in_single_comment = false;

        for (i, &b) in bytes.iter().enumerate().skip(open_pos) {
            if in_single_comment {
                if b == b'\n' {
                    in_single_comment = false;
                }
                continue;
            }

            if !in_str && b == b'#' {
                in_single_comment = true;
                continue;
            }

            if !in_str && (b == b'"' || b == b'\'') {
                in_str = true;
                quote_char = b;
                continue;
            }

            if in_str {
                if b == quote_char && (i == 0 || bytes[i - 1] != b'\\') {
                    in_str = false;
                }
                continue;
            }

            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

impl Parser for NixParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        // 1. Imports
        // imports = [ ./foo.nix ./bar.nix ];
        for cap in IMPORTS_LIST_RE.captures_iter(content) {
            if let Some(matched) = cap.get(1) {
                let block = matched.as_str();
                for item_cap in IMPORT_ITEM_RE.captures_iter(block) {
                    let target = item_cap[1].trim();
                    if !target.is_empty() && !target.starts_with('#') {
                        relations.push(Relation {
                            from: "root".to_string(),
                            to: target.to_string(),
                            rel_type: "imports".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }
        }

        // import ./path.nix or import <nixpkgs>
        for cap in IMPORT_RE.captures_iter(content) {
            let target = cap
                .get(1)
                .or_else(|| cap.get(2))
                .map(|m| m.as_str())
                .unwrap_or("");
            if !target.is_empty() {
                relations.push(Relation {
                    from: "root".to_string(),
                    to: target.to_string(),
                    rel_type: "imports".to_string(),
                    properties: HashMap::new(),
                });
            }
        }

        // 2. Flake inputs
        // inputs.nixpkgs.url = "...";
        for cap in FLAKE_INPUT_RE.captures_iter(content) {
            let input_name = &cap[1];
            let url = &cap[2];
            let start = cap.get(0).unwrap().start();
            let line = Self::line_number(content, start);
            let mut props = HashMap::new();
            props.insert("url".to_string(), url.to_string());
            symbols.push(Symbol {
                name: input_name.to_string(),
                kind: "input".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
            relations.push(Relation {
                from: "root".to_string(),
                to: input_name.to_string(),
                rel_type: "references".to_string(),
                properties: HashMap::new(),
            });
        }

        // inputs = { nixpkgs = { url = "..."; }; };
        for cap in FLAKE_INPUT_BLOCK_RE.captures_iter(content) {
            if let Some(matched) = cap.get(1) {
                let block = matched.as_str();
                let block_start = matched.start();
                for entry_cap in INPUT_ENTRY_RE.captures_iter(block) {
                    let input_name = &entry_cap[1];
                    let url = &entry_cap[2];
                    if input_name == "self" {
                        continue;
                    }
                    let line =
                        Self::line_number(content, block_start + entry_cap.get(0).unwrap().start());
                    let mut props = HashMap::new();
                    props.insert("url".to_string(), url.to_string());
                    symbols.push(Symbol {
                        name: input_name.to_string(),
                        kind: "input".to_string(),
                        start_line: line,
                        end_line: line,
                        docstring: None,
                        is_entry_point: false,
                        is_public: true,
                        tested: false,
                        is_nif: false,
                        is_unsafe: false,
                        properties: props,
                        embedding: None,
                    });
                    relations.push(Relation {
                        from: "root".to_string(),
                        to: input_name.to_string(),
                        rel_type: "references".to_string(),
                        properties: HashMap::new(),
                    });
                }
            }
        }

        // 3. Named derivations
        // foo = pkgs.stdenv.mkDerivation { ... }
        for cap in DERIVATION_RE.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let var_name = cap[1].trim();
            let builder = &cap[2];
            let start_pos = full_match.start();
            let start_line = Self::line_number(content, start_pos);

            let mut end_line = start_line;
            let mut props = HashMap::new();
            props.insert("builder".to_string(), builder.to_string());

            let mut derived_name = var_name.to_string();

            if let Some(brace_rel) = content[full_match.end()..].find('{') {
                let brace_pos = full_match.end() + brace_rel;
                if let Some(end_pos) = Self::find_matching_brace(content, brace_pos) {
                    end_line = Self::line_number(content, end_pos);
                    let body = &content[brace_pos..=end_pos];

                    if let Some(pname_cap) = PNAME_RE.captures(body) {
                        props.insert("pname".to_string(), pname_cap[1].to_string());
                        if derived_name.is_empty() || derived_name.contains('.') {
                            derived_name = pname_cap[1].to_string();
                        }
                    } else if let Some(name_cap) = NAME_RE.captures(body) {
                        props.insert("name".to_string(), name_cap[1].to_string());
                        if derived_name.is_empty() || derived_name.contains('.') {
                            derived_name = name_cap[1].to_string();
                        }
                    }

                    if let Some(ver_cap) = VERSION_RE.captures(body) {
                        props.insert("version".to_string(), ver_cap[1].to_string());
                    }

                    // Extract buildInputs/packages references
                    for input_cap in BUILD_INPUTS_RE.captures_iter(body) {
                        let prefix = input_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                        let items_str = &input_cap[2];
                        for item in PKG_ITEM_RE.captures_iter(items_str) {
                            let pkg = &item[1];
                            if pkg == "with" || pkg == "pkgs" || pkg == "lib" {
                                continue;
                            }
                            let target = if !prefix.is_empty() && !pkg.contains('.') {
                                format!("{prefix}.{pkg}")
                            } else {
                                pkg.to_string()
                            };
                            relations.push(Relation {
                                from: derived_name.clone(),
                                to: target,
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }

            let is_entry = var_name.ends_with(".default")
                || var_name == "defaultPackage"
                || var_name == "default";

            symbols.push(Symbol {
                name: derived_name,
                kind: "derivation".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: is_entry,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
        }

        // 4. Bare derivations (e.g. top-level { pkgs ? ... }: stdenv.mkDerivation { ... })
        if symbols.iter().all(|s| s.kind != "derivation") {
            for cap in BARE_DERIVATION_RE.captures_iter(content) {
                let full_match = cap.get(0).unwrap();
                let builder = &cap[1];
                let start_pos = full_match.start();
                let start_line = Self::line_number(content, start_pos);

                let brace_pos = full_match.end() - 1;
                let mut end_line = start_line;
                let mut props = HashMap::new();
                props.insert("builder".to_string(), builder.to_string());
                let mut derived_name = "default".to_string();

                if let Some(end_pos) = Self::find_matching_brace(content, brace_pos) {
                    end_line = Self::line_number(content, end_pos);
                    let body = &content[brace_pos..=end_pos];

                    if let Some(pname_cap) = PNAME_RE.captures(body) {
                        derived_name = pname_cap[1].to_string();
                        props.insert("pname".to_string(), pname_cap[1].to_string());
                    } else if let Some(name_cap) = NAME_RE.captures(body) {
                        derived_name = name_cap[1].to_string();
                        props.insert("name".to_string(), name_cap[1].to_string());
                    }
                    if let Some(ver_cap) = VERSION_RE.captures(body) {
                        props.insert("version".to_string(), ver_cap[1].to_string());
                    }

                    for input_cap in BUILD_INPUTS_RE.captures_iter(body) {
                        let prefix = input_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                        let items_str = &input_cap[2];
                        for item in PKG_ITEM_RE.captures_iter(items_str) {
                            let pkg = &item[1];
                            if pkg == "with" || pkg == "pkgs" || pkg == "lib" {
                                continue;
                            }
                            let target = if !prefix.is_empty() && !pkg.contains('.') {
                                format!("{prefix}.{pkg}")
                            } else {
                                pkg.to_string()
                            };
                            relations.push(Relation {
                                from: derived_name.clone(),
                                to: target,
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }

                symbols.push(Symbol {
                    name: derived_name,
                    kind: "derivation".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 5. DevShells (mkShell)
        for cap in MKSHELL_RE.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let shell_name = cap[1].trim();
            let start_pos = full_match.start();
            let start_line = Self::line_number(content, start_pos);
            let mut end_line = start_line;
            let props = HashMap::new();

            if let Some(brace_rel) = content[full_match.end()..].find('{') {
                let brace_pos = full_match.end() + brace_rel;
                if let Some(end_pos) = Self::find_matching_brace(content, brace_pos) {
                    end_line = Self::line_number(content, end_pos);
                    let body = &content[brace_pos..=end_pos];

                    for input_cap in BUILD_INPUTS_RE.captures_iter(body) {
                        let prefix = input_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                        let items_str = &input_cap[2];
                        for item in PKG_ITEM_RE.captures_iter(items_str) {
                            let pkg = &item[1];
                            if pkg == "with" || pkg == "pkgs" || pkg == "lib" {
                                continue;
                            }
                            let target = if !prefix.is_empty() && !pkg.contains('.') {
                                format!("{prefix}.{pkg}")
                            } else {
                                pkg.to_string()
                            };
                            relations.push(Relation {
                                from: shell_name.to_string(),
                                to: target,
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }
            }

            symbols.push(Symbol {
                name: shell_name.to_string(),
                kind: "devshell".to_string(),
                start_line,
                end_line,
                docstring: None,
                is_entry_point: true,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
        }

        // Bare mkShell (e.g. shell.nix or devenv.nix with pkgs.mkShell { ... })
        if symbols.iter().all(|s| s.kind != "devshell") {
            for cap in BARE_MKSHELL_RE.captures_iter(content) {
                let full_match = cap.get(0).unwrap();
                let start_pos = full_match.start();
                let start_line = Self::line_number(content, start_pos);
                let brace_pos = full_match.end() - 1;
                let mut end_line = start_line;
                let mut props = HashMap::new();
                let mut shell_name = "default".to_string();

                if let Some(end_pos) = Self::find_matching_brace(content, brace_pos) {
                    end_line = Self::line_number(content, end_pos);
                    let body = &content[brace_pos..=end_pos];

                    if let Some(name_cap) = NAME_RE.captures(body) {
                        shell_name = name_cap[1].to_string();
                        props.insert("name".to_string(), name_cap[1].to_string());
                    }

                    for input_cap in BUILD_INPUTS_RE.captures_iter(body) {
                        let prefix = input_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                        let items_str = &input_cap[2];
                        for item in PKG_ITEM_RE.captures_iter(items_str) {
                            let pkg = &item[1];
                            if pkg == "with" || pkg == "pkgs" || pkg == "lib" {
                                continue;
                            }
                            let target = if !prefix.is_empty() && !pkg.contains('.') {
                                format!("{prefix}.{pkg}")
                            } else {
                                pkg.to_string()
                            };
                            relations.push(Relation {
                                from: shell_name.clone(),
                                to: target,
                                rel_type: "references".to_string(),
                                properties: HashMap::new(),
                            });
                        }
                    }
                }

                symbols.push(Symbol {
                    name: shell_name,
                    kind: "devshell".to_string(),
                    start_line,
                    end_line,
                    docstring: None,
                    is_entry_point: true,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 6. Top-level let bindings
        if let Some(let_pos) = content.find("let") {
            if let Some(in_pos) = content[let_pos..].find("in") {
                let let_block = &content[let_pos + 3..let_pos + in_pos];
                let base_line = Self::line_number(content, let_pos);
                for cap in LET_BINDING_RE.captures_iter(let_block) {
                    let var_name = cap[1].trim();
                    let line =
                        base_line + Self::line_number(let_block, cap.get(0).unwrap().start()) - 1;
                    if !symbols.iter().any(|s| s.name == var_name) {
                        symbols.push(Symbol {
                            name: var_name.to_string(),
                            kind: "variable".to_string(),
                            start_line: line,
                            end_line: line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: false,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: HashMap::new(),
                            embedding: None,
                        });
                    }
                }
            }
        }

        ExtractionResult {
            project_code: None,
            symbols,
            relations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nix_derivation_extraction() {
        let code = r#"
        { pkgs ? import <nixpkgs> {} }:

        pkgs.stdenv.mkDerivation rec {
          pname = "axon";
          version = "0.8.0";

          src = ./.;

          buildInputs = [
            pkgs.openssl
            pkgs.pkg-config
          ];

          nativeBuildInputs = with pkgs; [
            cmake
            ninja
          ];
        }
        "#;
        let parser = NixParser::new();
        let res = parser.parse(code);

        assert!(!res.symbols.is_empty(), "should extract derivation symbol");
        let deriv = res
            .symbols
            .iter()
            .find(|s| s.name == "axon")
            .expect("axon derivation");
        assert_eq!(deriv.kind, "derivation");
        assert_eq!(
            deriv.properties.get("version").map(|s| s.as_str()),
            Some("0.8.0")
        );

        let refs: Vec<&str> = res.relations.iter().map(|r| r.to.as_str()).collect();
        assert!(
            refs.contains(&"pkgs.openssl"),
            "should reference pkgs.openssl: {refs:?}"
        );
        assert!(
            refs.contains(&"pkgs.pkg-config"),
            "should reference pkgs.pkg-config: {refs:?}"
        );
        assert!(
            refs.contains(&"pkgs.cmake"),
            "should reference pkgs.cmake: {refs:?}"
        );
        assert!(
            refs.contains(&"pkgs.ninja"),
            "should reference pkgs.ninja: {refs:?}"
        );
    }

    #[test]
    fn test_nix_flake_inputs_and_outputs() {
        let code = r#"
        {
          description = "Axon multi-language indexing core";

          inputs = {
            nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
            flake-utils.url = "github:numtide/flake-utils";
          };

          outputs = { self, nixpkgs, flake-utils }:
            flake-utils.lib.eachDefaultSystem (system:
              let
                pkgs = nixpkgs.legacyPackages.${system};
              in {
                packages.default = pkgs.rustPlatform.buildRustPackage {
                  pname = "axon-core";
                  version = "0.8.0";
                  src = ./.;
                  buildInputs = [ pkgs.openssl ];
                };
                devShells.default = pkgs.mkShell {
                  packages = [ pkgs.rustc pkgs.cargo ];
                };
              }
            );
        }
        "#;
        let parser = NixParser::new();
        let res = parser.parse(code);

        let input_names: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "input")
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            input_names.contains(&"nixpkgs"),
            "should find nixpkgs input: {input_names:?}"
        );
        assert!(
            input_names.contains(&"flake-utils"),
            "should find flake-utils input: {input_names:?}"
        );

        let deriv = res
            .symbols
            .iter()
            .find(|s| s.name == "axon-core")
            .expect("axon-core derivation");
        assert!(
            deriv.is_entry_point,
            "packages.default derivation should be entry point"
        );

        let shell = res
            .symbols
            .iter()
            .find(|s| s.name == "devShells.default")
            .expect("devShells.default");
        assert_eq!(shell.kind, "devshell");
        assert!(
            shell.is_entry_point,
            "devShells.default should be entry point"
        );
    }

    #[test]
    fn test_nix_imports_and_let_bindings() {
        let code = r#"
        { config, pkgs, ... }:

        let
          user = "developer";
          port = 8080;
        in {
          imports = [
            ./hardware.nix
            <nixpkgs/nixos>
          ];

          environment.systemPackages = [ pkgs.git ];
        }
        "#;
        let parser = NixParser::new();
        let res = parser.parse(code);

        let imports: Vec<&str> = res
            .relations
            .iter()
            .filter(|r| r.rel_type == "imports")
            .map(|r| r.to.as_str())
            .collect();
        assert!(
            imports.contains(&"./hardware.nix"),
            "should import ./hardware.nix: {imports:?}"
        );
        assert!(
            imports.contains(&"<nixpkgs/nixos>"),
            "should import <nixpkgs/nixos>: {imports:?}"
        );

        let vars: Vec<&str> = res
            .symbols
            .iter()
            .filter(|s| s.kind == "variable")
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            vars.contains(&"user"),
            "should find let binding user: {vars:?}"
        );
        assert!(
            vars.contains(&"port"),
            "should find let binding port: {vars:?}"
        );
    }
}
