use super::{ExtractionResult, Parser, Relation, Symbol};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static RE_PROJECT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\bproject\s*\(\s*([a-zA-Z0-9_.-]+)"#).expect("valid regex"));

static RE_ADD_EXECUTABLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\badd_executable\s*\(\s*([a-zA-Z0-9_.-]+)"#).expect("valid regex")
});

static RE_ADD_LIBRARY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\badd_library\s*\(\s*([a-zA-Z0-9_.-]+)(?:\s+(STATIC|SHARED|MODULE|INTERFACE|OBJECT))?"#)
        .expect("valid regex")
});

static RE_TARGET_LINK_LIBRARIES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?ms)\btarget_link_libraries\s*\(\s*([a-zA-Z0-9_.-]+)(.*?)\)"#)
        .expect("valid regex")
});

static RE_FIND_PACKAGE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\bfind_package\s*\(\s*([a-zA-Z0-9_.-]+)"#).expect("valid regex")
});

static RE_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)\binclude\s*\(\s*([a-zA-Z0-9_./-]+)"#).expect("valid regex"));

static RE_SET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\bset\s*\(\s*([a-zA-Z0-9_]+)(?:\s+(?:CACHE\s+[^)]+|["']?[^)]*["']?))?\s*\)"#)
        .expect("valid regex")
});

pub struct CMakeParser;

impl Default for CMakeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl CMakeParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parser for CMakeParser {
    fn parse(&self, content: &str) -> ExtractionResult {
        let mut symbols = Vec::new();
        let mut relations = Vec::new();

        let start_offset_line = |offset: usize| -> usize {
            content[..offset].chars().filter(|&c| c == '\n').count() + 1
        };

        // 1. Project
        for cap in RE_PROJECT.captures_iter(content) {
            let pname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            symbols.push(Symbol {
                name: pname.to_string(),
                kind: "cmake_project".to_string(),
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

        // 2. Find package
        for cap in RE_FIND_PACKAGE.captures_iter(content) {
            let pkg = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "CMakeLists.txt".to_string(),
                to: pkg.to_string(),
                rel_type: "finds_package".to_string(),
                properties: HashMap::new(),
            });
        }

        // 3. Include
        for cap in RE_INCLUDE.captures_iter(content) {
            let inc = cap.get(1).map_or("", |m| m.as_str());
            relations.push(Relation {
                from: "CMakeLists.txt".to_string(),
                to: inc.to_string(),
                rel_type: "includes_cmake".to_string(),
                properties: HashMap::new(),
            });
        }

        // 4. Variables with PII detection
        for cap in RE_SET.captures_iter(content) {
            let vname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            if let Some(kind) = super::is_sensitive_name(vname) {
                props.insert("is_sensitive".to_string(), "true".to_string());
                props.insert("pii_kind".to_string(), kind.to_string());
            }

            symbols.push(Symbol {
                name: vname.to_string(),
                kind: "cmake_variable".to_string(),
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
        }

        // 5. Executables
        for cap in RE_ADD_EXECUTABLE.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("target_type".to_string(), "executable".to_string());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "cmake_target".to_string(),
                start_line: line,
                end_line: line,
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

        // 6. Libraries
        for cap in RE_ADD_LIBRARY.captures_iter(content) {
            let tname = cap.get(1).map_or("", |m| m.as_str());
            let ltype = cap.get(2).map(|m| m.as_str()).unwrap_or("STATIC");
            let line = start_offset_line(cap.get(0).unwrap().start());

            let mut props = HashMap::new();
            props.insert("target_type".to_string(), "library".to_string());
            props.insert("library_kind".to_string(), ltype.to_string());

            symbols.push(Symbol {
                name: tname.to_string(),
                kind: "cmake_target".to_string(),
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
        }

        // 7. Target link libraries
        for cap in RE_TARGET_LINK_LIBRARIES.captures_iter(content) {
            let target = cap.get(1).map_or("", |m| m.as_str());
            let libs_body = cap.get(2).map_or("", |m| m.as_str());

            for token in libs_body.split_whitespace() {
                let clean = token.trim();
                if clean != "PRIVATE"
                    && clean != "PUBLIC"
                    && clean != "INTERFACE"
                    && !clean.is_empty()
                {
                    relations.push(Relation {
                        from: target.to_string(),
                        to: clean.to_string(),
                        rel_type: "links_library".to_string(),
                        properties: HashMap::new(),
                    });
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
    fn test_cmake_parser_basic_and_pii() {
        let code = r#"
cmake_minimum_required(VERSION 3.20)
project(NexusEngine VERSION 2.0.0 LANGUAGES CXX)

find_package(OpenSSL REQUIRED)
find_package(Threads REQUIRED)

include(FetchContent)

set(SECRET_API_KEY "vault_secret_token")
set(DATABASE_PASSWORD "admin_pass")
set(CMAKE_CXX_STANDARD 20)

add_library(nexus_core STATIC
    src/core.cpp
    src/memory.cpp
)

add_executable(nexus_server
    src/main.cpp
)

target_link_libraries(nexus_server PRIVATE nexus_core OpenSSL::Crypto Threads::Threads)
"#;

        let parser = CMakeParser::new();
        let res = parser.parse(code);

        let proj_sym = res
            .symbols
            .iter()
            .find(|s| s.name == "NexusEngine")
            .unwrap();
        assert_eq!(proj_sym.kind, "cmake_project");

        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "OpenSSL" && r.rel_type == "finds_package"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "Threads" && r.rel_type == "finds_package"));
        assert!(res
            .relations
            .iter()
            .any(|r| r.to == "FetchContent" && r.rel_type == "includes_cmake"));

        let key_var = res
            .symbols
            .iter()
            .find(|s| s.name == "SECRET_API_KEY")
            .unwrap();
        assert_eq!(
            key_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let pass_var = res
            .symbols
            .iter()
            .find(|s| s.name == "DATABASE_PASSWORD")
            .unwrap();
        assert_eq!(
            pass_var.properties.get("is_sensitive").map(|s| s.as_str()),
            Some("true")
        );

        let lib_target = res.symbols.iter().find(|s| s.name == "nexus_core").unwrap();
        assert_eq!(lib_target.kind, "cmake_target");
        assert_eq!(
            lib_target.properties.get("target_type").map(|s| s.as_str()),
            Some("library")
        );

        let exe_target = res
            .symbols
            .iter()
            .find(|s| s.name == "nexus_server")
            .unwrap();
        assert_eq!(exe_target.kind, "cmake_target");
        assert_eq!(
            exe_target.properties.get("target_type").map(|s| s.as_str()),
            Some("executable")
        );
        assert!(exe_target.is_entry_point);

        assert!(res.relations.iter().any(|r| r.from == "nexus_server"
            && r.to == "nexus_core"
            && r.rel_type == "links_library"));
        assert!(res.relations.iter().any(|r| r.from == "nexus_server"
            && r.to == "OpenSSL::Crypto"
            && r.rel_type == "links_library"));
    }
}
