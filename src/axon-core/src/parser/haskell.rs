use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_MODULE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*module\s+([A-Za-z0-9_.]+)\b(?:.*?)\bwhere\b").expect("valid regex")
});

static RE_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*import\s+(?:qualified\s+)?([A-Za-z0-9_.]+)(?:\s+as\s+[A-Za-z0-9_]+)?")
        .expect("valid regex")
});

static RE_DATA_RECORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?ms)^\s*data\s+([A-Za-z0-9_]+)[^{=]*=\s*[A-Za-z0-9_]+\s*\{(.*?)\}(?:\s*deriving\s*\([^)]*\))?")
        .expect("valid regex")
});

static RE_DATA_SIMPLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*data\s+([A-Za-z0-9_]+)[^=]*=\s*([A-Za-z0-9_|\s]+)").expect("valid regex")
});

static RE_NEWTYPE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*newtype\s+([A-Za-z0-9_]+)[^=]*=").expect("valid regex"));

static RE_TYPE_ALIAS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^\s*type\s+([A-Za-z0-9_]+)[^=]*=").expect("valid regex"));

static RE_CLASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*class\s+(?:[^=>]+=>\s*)?([A-Za-z0-9_]+)").expect("valid regex")
});

static RE_INSTANCE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*instance\s+(?:[^=>]+=>\s*)?([A-Za-z0-9_]+)\s+([A-Za-z0-9_()]+)")
        .expect("valid regex")
});

static RE_FOREIGN_IMPORT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*foreign\s+import\s+ccall\s+(?:["'][^"']*["']\s+)?([a-zA-Z0-9_]+)\s*::"#)
        .expect("valid regex")
});

static RE_FN_SIG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^\s*([a-z_][a-zA-Z0-9_']*)\s*::\s*([^=\n]+)").expect("valid regex")
});

static RE_TEST_CASE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:testCase|testProperty|it)\s+["']([^"']+)["']"#).expect("valid regex")
});

pub struct HaskellParser;

impl Default for HaskellParser {
    fn default() -> Self {
        Self::new()
    }
}

impl HaskellParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for HaskellParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Module
        let mut module_name = String::new();
        if let Some(cap) = RE_MODULE.captures(content) {
            let mname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());
            module_name = mname.to_string();

            symbols.push(Symbol {
                name: mname.to_string(),
                kind: "haskell_module".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: mname == "Main",
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 2. Imports
        for cap in RE_IMPORT.captures_iter(content) {
            let imp = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: if module_name.is_empty() {
                    "root".to_string()
                } else {
                    module_name.clone()
                },
                to: imp.to_string(),
                rel_type: "imports_haskell".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Data Records (with PII field analysis)
        for cap in RE_DATA_RECORD.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let fields_body = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut struct_props = HashMap::new();
            let mut has_sensitive = false;

            for part in fields_body.split(',') {
                let trimmed = part.trim();
                if let Some(col) = trimmed.find("::") {
                    let fname = trimmed[..col].trim();
                    let ftype = trimmed[col + 2..].trim();

                    if !fname.is_empty() {
                        let full_field_name = format!("{}.{}", tname, fname);
                        let mut fprops = HashMap::new();
                        fprops.insert("type".to_string(), ftype.to_string());

                        if let Some(kind) = super::is_sensitive_name(fname) {
                            has_sensitive = true;
                            fprops.insert("is_sensitive".to_string(), "true".to_string());
                            fprops.insert("pii_kind".to_string(), kind.to_string());
                        }

                        symbols.push(Symbol {
                            name: full_field_name.clone(),
                            kind: "haskell_field".to_string(),
                            start_line: line,
                            end_line: line,
                            docstring: None,
                            is_entry_point: false,
                            is_public: true,
                            tested: false,
                            is_nif: false,
                            is_unsafe: false,
                            properties: fprops,
                            embedding: None,
                        });

                        relations.push(Relation {
                            from: tname.to_string(),
                            to: full_field_name,
                            rel_type: "contains".to_string(),
                            properties: HashMap::new(),
                        });
                    }
                }
            }

            if has_sensitive {
                struct_props.insert("has_sensitive_fields".to_string(), "true".to_string());
            }

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "haskell_data".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: struct_props,
                embedding: None,
            });
        }

        // 4. Simple Data
        for cap in RE_DATA_SIMPLE.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            if !symbols
                .iter()
                .any(|s| s.name == tname && s.kind == "haskell_data")
            {
                symbols.push(Symbol {
                    name: tname.to_string(),
                    kind: "haskell_data".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: false,
                    is_public: true,
                    tested: false,
                    is_nif: false,
                    is_unsafe: false,
                    properties: HashMap::new(),
                    embedding: None,
                });
            }
        }

        // 5. Newtypes
        for cap in RE_NEWTYPE.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "haskell_newtype".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 6. Type Aliases
        for cap in RE_TYPE_ALIAS.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "haskell_type_alias".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 7. Classes
        for cap in RE_CLASS.captures_iter(content) {
            let cname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: cname.to_string(),
                kind: "haskell_class".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
        }

        // 8. Instances
        for cap in RE_INSTANCE.captures_iter(content) {
            let cname = cap.get(1).map_or("", |m| m.as_str());
            let tname = cap.get(2).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let inst_name = format!("{} for {}", cname, tname);
            symbols.push(Symbol {
                name: inst_name,
                kind: "haskell_instance".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });

            relations.push(Relation {
                from: tname.to_string(),
                to: cname.to_string(),
                rel_type: "implements_class".to_string(),
                properties: HashMap::new(),
            });
        }

        // 9. Foreign imports (FFI)
        for cap in RE_FOREIGN_IMPORT.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("ffi".to_string(), "foreign_import".to_string());

            symbols.push(Symbol {
                name: fname.to_string(),
                kind: "haskell_function".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: true,
                tested: false,
                is_nif: true,
                is_unsafe: false,
                properties: props,
                embedding: None,
            });
        }

        // 10. Functions (signatures)
        for cap in RE_FN_SIG.captures_iter(content) {
            let fname = cap.get(1).map_or("", |m| m.as_str());
            let sig = cap.get(2).map_or("", |m| m.as_str()).trim();
            let line = start_offset_line(cap.get(0).unwrap().start());

            if fname == "foreign" || fname == "data" || fname == "type" || fname == "newtype" {
                continue;
            }

            let is_main = fname == "main";
            let is_unsafe = fname.starts_with("unsafe") || sig.contains("unsafePerformIO");

            let mut props = HashMap::new();
            props.insert("signature".to_string(), sig.to_string());

            if !symbols
                .iter()
                .any(|s| s.name == fname && s.kind == "haskell_function")
            {
                symbols.push(Symbol {
                    name: fname.to_string(),
                    kind: "haskell_function".to_string(),
                    start_line: line,
                    end_line: line,
                    docstring: None,
                    is_entry_point: is_main,
                    is_public: !fname.starts_with('_'),
                    tested: false,
                    is_nif: false,
                    is_unsafe,
                    properties: props,
                    embedding: None,
                });
            }
        }

        // 11. Test Cases
        for cap in RE_TEST_CASE.captures_iter(content) {
            let tname = cap.get(1).map_or("test", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "haskell_test".to_string(),
                start_line: line,
                end_line: line,
                docstring: None,
                is_entry_point: false,
                is_public: false,
                tested: true,
                is_nif: false,
                is_unsafe: false,
                properties: HashMap::new(),
                embedding: None,
            });
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
    fn test_haskell_parser_basic_and_pii() {
        let code = r#"
module Security.Auth (authenticateUser, UserRecord(..)) where

import qualified Data.Text as T
import Network.HTTP.Client
import System.IO.Unsafe (unsafePerformIO)

foreign import ccall "c_crypto_verify" c_verify :: Ptr Word8 -> Int -> IO Bool

data UserRecord = UserRecord
    { userId       :: Int
    , userEmail    :: T.Text
    , userPassword :: T.Text
    , userToken    :: String
    } deriving (Show, Eq)

newtype SessionId = SessionId String

type TokenAlias = String

class MonadAuth m where
    checkToken :: TokenAlias -> m Bool

instance MonadAuth IO where
    checkToken _ = return True

authenticateUser :: UserRecord -> IO Bool
authenticateUser u = return True

unsafeBypassAuth :: Bool -> IO Bool
unsafeBypassAuth _ = return True

main :: IO ()
main = putStrLn "Starting Haskell Server"

testCase "Auth verification passes"
"#;

        let parser = HaskellParser::new();
        let res = parser.parse(code);

        let mod_sym = res
            .symbols
            .iter()
            .find(|s| s.kind == "haskell_module")
            .unwrap();
        assert_eq!(mod_sym.name, "Security.Auth");

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "Data.Text" && r.rel_type == "imports_haskell"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "Network.HTTP.Client" && r.rel_type == "imports_haskell"));

        let data_sym = res.symbols.iter().find(|s| s.name == "UserRecord").unwrap();
        assert_eq!(
            data_sym
                .properties
                .get("has_sensitive_fields")
                .map(|s| s.as_str()),
            Some("true")
        );

        let pass_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserRecord.userPassword")
            .unwrap();
        assert_eq!(
            pass_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let token_field = res
            .symbols
            .iter()
            .find(|s| s.name == "UserRecord.userToken")
            .unwrap();
        assert_eq!(
            token_field
                .properties
                .get("is_sensitive")
                .map(|s| s.as_str()),
            Some("true")
        );

        let newtype_sym = res.symbols.iter().find(|s| s.name == "SessionId").unwrap();
        assert_eq!(newtype_sym.kind, "haskell_newtype");

        let alias_sym = res.symbols.iter().find(|s| s.name == "TokenAlias").unwrap();
        assert_eq!(alias_sym.kind, "haskell_type_alias");

        let class_sym = res.symbols.iter().find(|s| s.name == "MonadAuth").unwrap();
        assert_eq!(class_sym.kind, "haskell_class");

        assert!(res
            .relations
            .iter()
            .any(|r| r.from == "IO" && r.to == "MonadAuth" && r.rel_type == "implements_class"));

        let ffi_sym = res.symbols.iter().find(|s| s.name == "c_verify").unwrap();
        assert!(ffi_sym.is_nif);

        let unsafe_fn = res
            .symbols
            .iter()
            .find(|s| s.name == "unsafeBypassAuth")
            .unwrap();
        assert!(unsafe_fn.is_unsafe);

        let main_fn = res.symbols.iter().find(|s| s.name == "main").unwrap();
        assert!(main_fn.is_entry_point);

        let test_sym = res
            .symbols
            .iter()
            .find(|s| s.kind == "haskell_test")
            .unwrap();
        assert_eq!(test_sym.name, "Auth verification passes");
        assert!(test_sym.tested);
    }
}
